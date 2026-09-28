/**
 * 移动端 TUN 页两块只读报告的**分支门**。
 *
 * # 为什么移动端需要自己这一道（桌面那两道不够）
 *
 * `TunnelConflictBlock.status-branches.test.tsx` 与 `EndpointForceRouteBlock.zero-coverage.test.tsx`
 * 守的是**桌面那两份实现**。移动端是另一份呈现（不能 import 桌面那条层叠链，理由见
 * `TunReports.tsx` 头注）—— 两份实现各自都可能把四态折塌，而折塌的表现完全一样：
 * 一句自信的「无冲突」。故这一道按同一套判据钉移动端这一份。
 *
 * 🔴 **这道门在 Android 上守的是最要紧的那一支**：`probe_foreign_tunnels` 今天在 Android 上答
 * `Unsupported`，手机上恒走那一档 —— 它要是被折成「无冲突」，用户会据此排除掉真正的病因，
 * 而那正是 2026-09-08 报障那台 macOS 的原样形态。
 *
 * # 两个方向都说话
 *
 *  · **否定**：四态里不含任何等价于「无冲突」的说法（针眼中英各一）；
 *  · **正面**：各自那句话必须真的出现 —— 只写否定断言的门会被「整块根本没渲染」骗成绿；
 *  · **正向对照**：`probed` 且 `conflicts` 为空时针眼**必须**出现，证明针眼是活的。
 *
 * `t` 直接查 `locales/*.json`（不用桩）：判据是「**界面文本**里没有等价说法」，
 * 桩 `t` 只能证明「没用到某个 key」—— 把译文改成「本平台无冲突」，桩版照样全绿。
 */
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
import type { TunnelConflictReport } from '@/contracts/tunnel-conflict-report';

type Dict = Record<string, unknown>;

const locale = (name: string): Dict =>
  JSON.parse(
    readFileSync(fileURLToPath(new URL(`../../i18n/locales/${name}.json`, import.meta.url)), 'utf8'),
  ) as Dict;

/**
 * 中英两份。针眼是**逐字**取自译文的，五份全上要另写三套针眼与禁词表，
 * 而那一层（译文本身不许出现「无冲突」的等价说法）已经由桌面那道门按五语种守着 ——
 * 本门守的是**这一份实现的分支结构**，两份译文足够让它有牙。
 */
const LANGS = ['zh-CN', 'en-US'] as const;
const DICTS: Record<string, Dict> = Object.fromEntries(LANGS.map((l) => [l, locale(l)]));

/** 当前语种（`vi.mock` 工厂被提升，只能经 `vi.hoisted` 共享）。 */
const h = vi.hoisted(() => ({ lang: 'zh-CN' }));

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) => translate(h.lang, key, vars),
    i18n: { language: h.lang },
  }),
}));

/** 点分寻址 + `{{x}}` 插值。取不到就抛 —— 缺键静默回落成 key 会让否定断言全部空转变绿。 */
function translate(lang: string, key: string, vars?: Record<string, unknown>): string {
  let node: unknown = DICTS[lang];
  for (const seg of key.split('.')) {
    node = (node as Dict | undefined)?.[seg];
  }
  if (typeof node !== 'string') throw new Error(`[tun-reports] ${lang} 缺键 ${key}`);
  return node.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars?.[name] ?? ''));
}

const { MobileEndpointForceRouteBlock, MobileTunnelConflictBlock } = await import('./TunReports');

/**
 * 「无冲突」的针眼。它们逐字出现在 `tunnelConflictNone` / `tunnelConflictNoBusinessRanges`
 * 那两句译文里，由下面的正向对照钉住；其余任何一态出现它们即是本门要抓的那种谎。
 */
const NO_CONFLICT_NEEDLES: Record<(typeof LANGS)[number], string> = {
  'zh-CN': '无冲突',
  'en-US': 'no conflict',
};

const NOT_PROBED: TunnelConflictReport = { status: 'notProbed' };
/** Android 今天恒走这一支（后端给的是 Node 约定名，认不出就原样显示）。 */
const UNSUPPORTED_ANDROID: TunnelConflictReport = { status: 'unsupported', platform: 'android' };
const UNSUPPORTED_MAC: TunnelConflictReport = { status: 'unsupported', platform: 'darwin' };
const PROBE_FAILED: TunnelConflictReport = { status: 'probeFailed', error: 'ip: command not found' };
const PROBED_CLEAN: TunnelConflictReport = {
  status: 'probed',
  foreignTunnels: [{ interface: 'utun4', prefix: '198.18.0.0/16' }],
  suppressedRoutes: 0,
  foreignDefaultRoutes: [],
  conflicts: [],
  criteria: { fakeipRanges: [], meshCidrs: [], tunAddresses: [] },
};
/** 展示面为零那一支：若沿用「另有 0 条」，界面上它就与「压根没探」同形了。 */
const PROBED_QUIET: TunnelConflictReport = {
  status: 'probed',
  foreignTunnels: [],
  suppressedRoutes: 36,
  foreignDefaultRoutes: [],
  conflicts: [],
  criteria: { fakeipRanges: ['198.18.0.0/15'], meshCidrs: [], tunAddresses: ['172.19.0.1/30'] },
};
const PROBED_CONFLICT: TunnelConflictReport = {
  status: 'probed',
  foreignTunnels: [{ interface: 'utun4', prefix: '32.0.0.0/24' }],
  suppressedRoutes: 36,
  foreignDefaultRoutes: [],
  conflicts: [{ interface: 'utun4', prefix: '32.0.0.0/24', kind: 'meshOverlap' }],
  criteria: { fakeipRanges: [], meshCidrs: ['32.0.0.0/24'], tunAddresses: [] },
};

const conflictMarkup = (report: TunnelConflictReport | null, lang: string): string => {
  h.lang = lang;
  return renderToStaticMarkup(<MobileTunnelConflictBlock report={report} />);
};

describe('① 外来隧道报告：未探测 ≠ 无冲突（逐态断言）', () => {
  for (const lang of LANGS) {
    const needle = NO_CONFLICT_NEEDLES[lang];

    it(`[${lang}] 正向对照：探过了且真的没冲突时，针眼「${needle}」确实出现（否定断言有牙）`, () => {
      expect(conflictMarkup(PROBED_CLEAN, lang)).toContain(needle);
    });

    it(`[${lang}] notProbed：说清「本次没探」，且不含「${needle}」`, () => {
      const markup = conflictMarkup(NOT_PROBED, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictNotProbed'));
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] 🔴 unsupported（Android 恒走这一支）：说「判定未进行」，且不含「${needle}」`, () => {
      const markup = conflictMarkup(UNSUPPORTED_ANDROID, lang);
      expect(markup).toContain(
        translate(lang, 'settings.tun.tunnelConflictUnsupported', { platform: 'android' }),
      );
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] unsupported：认得出的平台名换成产品名（darwin → macOS），认不出的原样显示`, () => {
      expect(conflictMarkup(UNSUPPORTED_MAC, lang)).toContain('macOS');
      // Android 不在映射表里 —— 原样摆出来，不编一个名字（那会往「判定未进行」里掺没核实的信息）。
      expect(conflictMarkup(UNSUPPORTED_ANDROID, lang)).toContain('android');
    });

    it(`[${lang}] probeFailed：说清失败 + 带后端诊断串，且不含「${needle}」`, () => {
      const markup = conflictMarkup(PROBE_FAILED, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictFailed'));
      expect(markup).toContain('ip: command not found');
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] probed 且有冲突：逐条列出接口、前缀与类别，且不含「${needle}」`, () => {
      const markup = conflictMarkup(PROBED_CONFLICT, lang);
      expect(markup).toContain('utun4');
      expect(markup).toContain('32.0.0.0/24');
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictKindMesh'));
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] 报告拉不到（null）：说读不到，且不含「${needle}」`, () => {
      const markup = conflictMarkup(null, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.tunnelConflictUnavailable'));
      expect(markup).not.toContain(needle);
    });

    it(`[${lang}] 展示面为零：换一句话说，并摆出被收掉的条数（不退成「另有 0 条」）`, () => {
      const markup = conflictMarkup(PROBED_QUIET, lang);
      expect(markup).toContain(
        translate(lang, 'settings.tun.tunnelConflictNoBusinessRanges', { count: 36 }),
      );
      expect(markup).not.toContain(translate(lang, 'settings.tun.tunnelConflictNone', { count: 0 }));
    });
  }

  it('守卫自检：六态两两渲染结果互不相同（折塌成一句话时本条转红）', () => {
    const bodies = [
      NOT_PROBED,
      UNSUPPORTED_ANDROID,
      PROBE_FAILED,
      PROBED_CLEAN,
      PROBED_QUIET,
      PROBED_CONFLICT,
      null,
    ].map((r) => conflictMarkup(r, 'zh-CN'));
    expect(new Set(bodies).size, '有两态渲染出了同一段界面文本').toBe(bodies.length);
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
    emitted?: string[];
    absorbed?: AbsorbedCidr[];
    hasObservation?: boolean;
    coverage?: ForceRouteCoverage;
  } = {},
): ServerForceRoute {
  const absorbed = opts.absorbed ?? [];
  return {
    serverId,
    leg: 'inline',
    hasObservation: opts.hasObservation ?? true,
    emitted: opts.emitted ?? [],
    externalRuleSetCidrs: [],
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

/** 部分被吸收：涉及的节点仍有别的段生效 ⇒ 不点名、只报条数。 */
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

describe('② 组网网段结算：四条腿各说各的，被吃干净的点名到节点', () => {
  for (const lang of LANGS) {
    it(`[${lang}] 拉不到（null）：说读不到，不说「没有节点被抢」`, () => {
      const markup = forceMarkup(null, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.forceRouteUnavailable'));
      expect(markup).not.toContain(translate(lang, 'settings.tun.forceRouteNone', { count: 0 }));
    });

    it(`[${lang}] 没有会发段的节点：说清「没有」，不冒充一次结算`, () => {
      expect(forceMarkup(EMPTY, lang)).toContain(translate(lang, 'settings.tun.forceRouteEmpty'));
    });

    it(`[${lang}] 结算过、谁都没被抢：报节点数`, () => {
      expect(forceMarkup(CLEAN, lang)).toContain(
        translate(lang, 'settings.tun.forceRouteNone', { count: 2 }),
      );
    });

    it(`[${lang}] 只是部分被吸收：报条数，**不**点名（那些节点仍有别的段生效）`, () => {
      const markup = forceMarkup(PARTIAL, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.forceRouteAbsorbedOnly', { count: 1 }));
      expect(markup).not.toContain(translate(lang, 'settings.tun.forceRouteZeroCoverage'));
    });

    it(`[${lang}] 🔴 被吃干净：点名到节点 + 报出实际生效的是谁（静默失效唯一的出口）`, () => {
      const markup = forceMarkup(ZERO_COVERAGE, lang);
      expect(markup).toContain(translate(lang, 'settings.tun.forceRouteZeroCoverage'));
      expect(markup).toContain('WG 机房');
      expect(markup).toContain('10.1.0.0/16');
      expect(markup).toContain(
        translate(lang, 'settings.tun.forceRouteAbsorbedBy', { node: '家里' }),
      );
      // 这一支绝不能同时说「谁都没被抢」。
      expect(markup).not.toContain(translate(lang, 'settings.tun.forceRouteNone', { count: 2 }));
    });

    it(`[${lang}] 证据强度：败方是没有观测地址的 Tailscale 节点时补一句，别让人据此删节点`, () => {
      expect(forceMarkup(WEAK_EVIDENCE, lang)).toContain(
        translate(lang, 'settings.tun.forceRouteNoObservation'),
      );
    });

    it(`[${lang}] 反向对照：有观测地址时**不**补那句（否则它恒显，等于没有信息）`, () => {
      expect(forceMarkup(ZERO_COVERAGE, lang)).not.toContain(
        translate(lang, 'settings.tun.forceRouteNoObservation'),
      );
    });
  }

  it('守卫自检：五条腿两两渲染结果互不相同', () => {
    const bodies = [null, EMPTY, CLEAN, PARTIAL, ZERO_COVERAGE, WEAK_EVIDENCE].map((r) =>
      forceMarkup(r, 'zh-CN'),
    );
    expect(new Set(bodies).size, '有两条腿渲染出了同一段界面文本').toBe(bodies.length);
  });
});
