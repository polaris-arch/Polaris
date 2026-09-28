/** Mobile TUN only offers the endpoint calculation that Android/iOS can actually perform. */
import { describe, it, expect, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';

import type { ServerConfig } from '@/contracts/types';
import type {
  AbsorbedCidr,
  EndpointForceRouteReport,
  ForceRouteCoverage,
  ServerForceRoute,
} from '@/contracts/endpoint-force-route-report';

type Dict = Record<string, unknown>;
const locale = (name: string): Dict => JSON.parse(readFileSync(
  fileURLToPath(new URL(`../../i18n/locales/${name}.json`, import.meta.url)), 'utf8',
)) as Dict;
const LANGS = ['zh-CN', 'en-US'] as const;
const DICTS: Record<string, Dict> = Object.fromEntries(LANGS.map((lang) => [lang, locale(lang)]));
const h = vi.hoisted(() => ({ lang: 'zh-CN' }));
vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) => translate(h.lang, key, vars),
    i18n: { language: h.lang },
  }),
}));
function translate(lang: string, key: string, vars?: Record<string, unknown>): string {
  let node: unknown = DICTS[lang];
  for (const segment of key.split('.')) node = (node as Dict | undefined)?.[segment];
  if (typeof node !== 'string') throw new Error(`[tun-reports] ${lang} missing ${key}`);
  return node.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars?.[name] ?? ''));
}
const { MobileEndpointForceRouteBlock } = await import('./TunReports');

describe('mobile platform boundary', () => {
  it('does not request or render Android/iOS foreign-tunnel probing, and retains endpoint calculation', () => {
    const backend = readFileSync(fileURLToPath(new URL('../../../../crates/system-integration/src/route_probe.rs', import.meta.url)), 'utf8');
    const page = readFileSync(fileURLToPath(new URL('./TunPage.tsx', import.meta.url)), 'utf8');
    expect(backend).toMatch(/Platform::Android\s*\|\s*Platform::Ios\s*=>\s*Ok\(TunnelProbeOutcome::Unsupported/);
    expect(page).not.toContain('.tunnelConflictReport(');
    expect(page).not.toContain('<MobileTunnelConflictBlock');
    expect(page).toContain('.endpointForceRouteReport(');
    expect(page).toContain('<MobileEndpointForceRouteBlock');
  });
});

/* ────────────────────────── ② 组网网段结算 ────────────────────────── */

const SERVERS: readonly ServerConfig[] = [
  { id: 's1', name: '家里', protocol: 'tailscale', address: '', port: 0 },
  { id: 's2', name: '公司', protocol: 'tailscale', address: '', port: 0 },
  { id: 's3', name: 'WG 机房', protocol: 'wireguard', address: '', port: 0 },
];

/**
 * 逐节点结算的夹具工厂。
 *
 * 🔴 **一个字段都不用 `as` 补齐**：`ServerForceRoute` 的 `leg` / `emitted` /
 * `externalRuleSetCidrs` / `coverage` 与呈现无关，但把它们 `as` 掉等于让 tsc 不再核对这份夹具 ——
 * 契约哪天改了字段名，这道门会静默继续绿，而真正的渲染腿早就读不到东西了
 * （本文件初稿就写了一个并不存在的 `cidrs` 字段，`as` 把它藏住了）。
 */
function node(
  serverId: string,
  opts: {
    leg?: ServerForceRoute['leg'];
    emitted?: string[];
    externalRuleSetCidrs?: string[];
    absorbed?: AbsorbedCidr[];
    hasObservation?: boolean;
    coverage?: ForceRouteCoverage;
  } = {},
): ServerForceRoute {
  const absorbed = opts.absorbed ?? [];
  return {
    serverId,
    leg: opts.leg ?? 'inline',
    hasObservation: opts.hasObservation ?? true,
    emitted: opts.emitted ?? [],
    externalRuleSetCidrs: opts.externalRuleSetCidrs ?? [],
    absorbed,
    coverage: opts.coverage ?? (absorbed.length > 0 ? 'absorbedEmpty' : 'covered'),
  };
}

const forceMarkup = (report: EndpointForceRouteReport | null, lang: string): string => {
  h.lang = lang;
  return renderToStaticMarkup(
    <MobileEndpointForceRouteBlock report={report} servers={SERVERS} />,
  );
};

const EMPTY: EndpointForceRouteReport = {
  servers: [],
  zeroCoverageServerIds: [],
  absorbedCount: 0,
};

const CLEAN: EndpointForceRouteReport = {
  servers: [node('s1', { emitted: ['10.1.0.0/16'] }), node('s3', { emitted: ['10.2.0.0/16'] })],
  zeroCoverageServerIds: [],
  absorbedCount: 0,
};

/** 部分被吸收：摘要只报数量；展开后必须能看到败方与具体段。 */
const PARTIAL: EndpointForceRouteReport = {
  servers: [
    node('s1', { emitted: ['10.1.0.0/16'] }),
    node('s3', {
      emitted: ['10.2.0.0/16'],
      absorbed: [{ cidr: '10.1.0.0/16', byServerId: 's1' }],
      coverage: 'covered',
    }),
  ],
  zeroCoverageServerIds: [],
  absorbedCount: 1,
};

/** 静默失效：`s3` 的段被 `s1` 吃干净 —— 节点活着、engaged，流量一条都不到。 */
const ZERO_COVERAGE: EndpointForceRouteReport = {
  servers: [
    node('s1', { emitted: ['10.1.0.0/16'] }),
    node('s3', { absorbed: [{ cidr: '10.1.0.0/16', byServerId: 's1' }] }),
  ],
  zeroCoverageServerIds: ['s3'],
  absorbedCount: 1,
};

/** 败方是**没有运行期观测地址的 Tailscale 节点** ⇒ 这次重合可能只是两份默认常量。 */
const WEAK_EVIDENCE: EndpointForceRouteReport = {
  servers: [
    node('s1', { emitted: ['100.64.0.0/10'], hasObservation: false }),
    node('s2', {
      absorbed: [{ cidr: '100.64.0.0/10', byServerId: 's1' }],
      hasObservation: false,
    }),
  ],
  zeroCoverageServerIds: ['s2'],
  absorbedCount: 1,
};

/** An external rule-set has no inline `emitted` entries, but does carry real calculated CIDRs. */
const EXTERNAL_AND_PREFERRED: EndpointForceRouteReport = {
  servers: [
    node('s1', { leg: 'externalRuleSet', emitted: [], externalRuleSetCidrs: ['100.101.0.0/16'] }),
    node('s3', { leg: 'preferredBy', emitted: [], externalRuleSetCidrs: [], coverage: 'nothingToRoute', hasObservation: false }),
  ],
  zeroCoverageServerIds: [],
  absorbedCount: 0,
};

describe('② 组网网段结算：四条腿各说各的，被吃干净的点名到节点', () => {
  for (const lang of LANGS) {
    it(`[${lang}] 拉不到（null）：说读不到，不说「没有节点被抢」`, () => {
      const markup = forceMarkup(null, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.forceRouteUnavailable'));
      expect(markup).not.toContain(translate(lang, 'settings.tun.forceRouteNone', { count: 0 }));
    });

    it(`[${lang}] 没有会发段的节点：说清「没有」，不冒充一次结算`, () => {
      expect(forceMarkup(EMPTY, lang)).toContain(translate(lang, 'mobileSettings.forceRouteEmpty'));
    });

    it(`[${lang}] 结算过、谁都没被抢：报节点数`, () => {
      expect(forceMarkup(CLEAN, lang)).toContain(
        translate(lang, 'mobileSettings.forceRouteNone', { count: 2 }),
      );
    });

    it(`[${lang}] 部分被吸收：摘要报数量，展开后给出败方和实际覆盖者`, () => {
      const markup = forceMarkup(PARTIAL, lang);
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRouteAbsorbedOnly', { count: 1 }).replaceAll("'", '&#x27;'));
      expect(markup).not.toContain(translate(lang, 'mobileSettings.forceRouteZeroCoverage'));
      expect(markup).toContain('<details');
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRouteDetails', { count: 2 }).replaceAll("'", '&#x27;'));
      expect(markup).toContain('WG 机房');
      expect(markup).toContain('10.2.0.0/16');
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRouteAbsorbedBy', { node: '家里' }));
    });

    it(`[${lang}] 🔴 被吃干净：点名到节点 + 报出实际生效的是谁（静默失效唯一的出口）`, () => {
      const markup = forceMarkup(ZERO_COVERAGE, lang);
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRouteZeroCoverage'));
      expect(markup).toContain('WG 机房');
      expect(markup).toContain('10.1.0.0/16');
      expect(markup).toContain(
        translate(lang, 'mobileSettings.forceRouteAbsorbedBy', { node: '家里' }),
      );
      // 这一支绝不能同时说「谁都没被抢」。
      expect(markup).not.toContain(translate(lang, 'mobileSettings.forceRouteNone', { count: 2 }));
    });

    it(`[${lang}] 证据强度：败方是没有观测地址的 Tailscale 节点时补一句，别让人据此删节点`, () => {
      expect(forceMarkup(WEAK_EVIDENCE, lang)).toContain(
        translate(lang, 'mobileSettings.forceRouteNoObservation'),
      );
    });

    it(`[${lang}] 反向对照：有观测地址时**不**补那句（否则它恒显，等于没有信息）`, () => {
      expect(forceMarkup(ZERO_COVERAGE, lang)).not.toContain(
        translate(lang, 'mobileSettings.forceRouteNoObservation'),
      );
    });

    it(`[${lang}] 外部规则集和 preferredBy 不被空的 inline 数组冒充零网段`, () => {
      const markup = forceMarkup(EXTERNAL_AND_PREFERRED, lang);
      expect(markup).toContain('100.101.0.0/16');
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRouteExternal'));
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRoutePreferredBy'));
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRouteNothingToRoute'));
      expect(markup).not.toContain(translate(lang, 'mobileSettings.forceRouteUnobservedTs'));
      expect(markup).toContain(translate(lang, 'mobileSettings.forceRouteCalculationHint'));
    });
  }

  it('守卫自检：五条腿两两渲染结果互不相同', () => {
    const bodies = [null, EMPTY, CLEAN, PARTIAL, ZERO_COVERAGE, WEAK_EVIDENCE].map((r) =>
      forceMarkup(r, 'zh-CN'),
    );
    expect(new Set(bodies).size, '有两条腿渲染出了同一段界面文本').toBe(bodies.length);
  });
});
