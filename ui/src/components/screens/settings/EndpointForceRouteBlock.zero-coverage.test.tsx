/**
 * 「组网网段结算」块的**零覆盖可见性门**。
 *
 * # 守什么
 *
 * `zeroCoverageServerIds` 说的是：这个节点活着、engaged、用户以为它在工作，而它的网段已被更早
 * 声明的节点**全部**抢走 —— 流量一条都不会到它那儿。这是**静默失效**，不是一条信息：
 * 界面上若只给一个「有 N 段被吸收」的计数，用户永远不知道是**哪个节点**整个废了。
 * 故本门断言：零覆盖非空时，界面**点名到节点**（节点名 + 被谁抢走）。
 *
 * 三条对照成组，缺一条都能被绕过：
 *  - 正面：零覆盖节点名、抢占者节点名、被抢的那一段都出现；
 *  - 反向：同一批节点、同一份段，只把 `zeroCoverageServerIds` 清空 ⇒ 那句告警必须消失
 *    （证明这句话挂在 `zeroCoverageServerIds` 上，不是无论如何都画一行）；
 *  - 证据强度：败方是**没有运行期观测**的 Tailscale 节点时，必须补那句「两个没连上的节点段必然
 *    重合、这不是账号撞车的证据」；有观测时不许出现（否则它就成了永远挂着的免责声明）。
 *
 * `t` 直接查 zh-CN 真 locale：判据是界面文本，不是 key。
 */
import { describe, it, expect, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';

import type { ServerConfig } from '@/contracts/types';
import type {
  EndpointForceRouteReport,
  ServerForceRoute,
} from '@/contracts/endpoint-force-route-report';

type Dict = Record<string, unknown>;

const ZH: Dict = JSON.parse(
  readFileSync(fileURLToPath(new URL('../../../i18n/locales/zh-CN.json', import.meta.url)), 'utf8'),
) as Dict;

function translate(key: string, vars?: Record<string, unknown>): string {
  let node: unknown = ZH;
  for (const seg of key.split('.')) node = (node as Dict | undefined)?.[seg];
  if (typeof node !== 'string') throw new Error(`[force-route] zh-CN 缺键 ${key}`);
  return node.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars?.[name] ?? ''));
}

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) => translate(key, vars),
    i18n: { language: 'zh-CN' },
  }),
}));

const { EndpointForceRouteBlock } = await import('./EndpointForceRouteBlock');

// ── 夹具 ────────────────────────────────────────────────────────────────────

const node = (id: string, name: string, protocol = 'tailscale'): ServerConfig =>
  ({ id, name, protocol, address: '', port: 0 }) as ServerConfig;

const SERVERS = [node('ts-a', '家里'), node('ts-b', '公司')];

const leg = (over: Partial<ServerForceRoute> & { serverId: string }): ServerForceRoute => ({
  leg: 'inline',
  hasObservation: true,
  emitted: [],
  absorbed: [],
  coverage: 'covered',
  ...over,
});

/** ts-a 先声明 100.64.0.0/10 并占住，ts-b 的同一段被吸收干净 ⇒ ts-b 零覆盖。 */
const winnerLoser = (hasObservation: boolean): EndpointForceRouteReport => ({
  servers: [
    leg({ serverId: 'ts-a', emitted: ['100.64.0.0/10'], hasObservation }),
    leg({
      serverId: 'ts-b',
      absorbed: [{ cidr: '100.64.0.0/10', byServerId: 'ts-a' }],
      coverage: 'absorbedEmpty',
      hasObservation,
    }),
  ],
  zeroCoverageServerIds: ['ts-b'],
  absorbedCount: 1,
});

const render = (report: EndpointForceRouteReport | null, servers = SERVERS): string =>
  renderToStaticMarkup(<EndpointForceRouteBlock report={report} servers={servers} />);

const ZERO_LINE = translate('settings.tun.forceRouteZeroCoverage');

describe('判据 4 —— 零覆盖节点必须被点名', () => {
  it('零覆盖非空：告警出现，且点名到「哪个节点」「被谁抢走」「抢的哪一段」', () => {
    const markup = render(winnerLoser(true));
    expect(markup).toContain(ZERO_LINE);
    expect(markup).toContain('公司'); // 废掉的那个
    expect(markup).toContain('100.64.0.0/10'); // 被抢的段
    expect(markup).toContain(translate('settings.tun.forceRouteAbsorbedBy', { node: '家里' }));
  });

  it('反向对照：同一份结算、只把 zeroCoverageServerIds 清空 ⇒ 那句告警消失', () => {
    // 证明「点名」这件事挂在 zeroCoverageServerIds 上，不是无条件画一行。
    const markup = render({ ...winnerLoser(true), zeroCoverageServerIds: [] });
    expect(markup).not.toContain(ZERO_LINE);
    expect(markup).not.toContain('公司');
    // 但「有段被吸收」这件事仍要说 —— 不是零覆盖就当作什么都没发生。
    expect(markup).toContain(translate('settings.tun.forceRouteAbsorbedOnly', { count: 1 }));
  });

  it('一条都没被吸收：如实说「没有被抢走」（空态是结论，不是空白）', () => {
    const markup = render({
      servers: [leg({ serverId: 'ts-a', emitted: ['10.0.0.0/24'] })],
      zeroCoverageServerIds: [],
      absorbedCount: 0,
    });
    expect(markup).toContain(translate('settings.tun.forceRouteNone', { count: 1 }));
    expect(markup).not.toContain(ZERO_LINE);
  });

  it('没有会发射网段的组网节点 / 报告读不到：两种「什么都没有」分开说', () => {
    const empty = render({ servers: [], zeroCoverageServerIds: [], absorbedCount: 0 });
    expect(empty).toContain(translate('settings.tun.forceRouteEmpty'));
    const unavailable = render(null);
    expect(unavailable).toContain(translate('settings.tun.forceRouteUnavailable'));
    expect(unavailable).not.toContain(translate('settings.tun.forceRouteEmpty'));
  });

  it('节点已被删（id 在报告里、不在配置里）→ 退回显示 id，绝不隐去这一行', () => {
    const markup = render(winnerLoser(true), []);
    expect(markup).toContain(ZERO_LINE);
    expect(markup).toContain('ts-b');
  });
});

describe('判据 4b —— 证据强度：没有观测时不许把「段重合」说成「账号撞车」', () => {
  const CAVEAT = translate('settings.tun.forceRouteNoObservation');

  it('败方是**没有观测**的 Tailscale 节点 ⇒ 补上证据强度说明', () => {
    expect(render(winnerLoser(false))).toContain(CAVEAT);
  });

  it('有观测 ⇒ 不出现那句说明（否则它就成了永远挂着的免责声明）', () => {
    expect(render(winnerLoser(true))).not.toContain(CAVEAT);
  });

  it('败方是 WireGuard（段是用户自己填的，不存在默认常量重合）⇒ 同样不出现', () => {
    const wgServers = [node('wg-a', '机房', 'wireguard'), node('wg-b', '备用', 'wireguard')];
    const report: EndpointForceRouteReport = {
      servers: [
        leg({ serverId: 'wg-a', emitted: ['10.0.0.0/24'], hasObservation: false }),
        leg({
          serverId: 'wg-b',
          absorbed: [{ cidr: '10.0.0.0/24', byServerId: 'wg-a' }],
          coverage: 'absorbedEmpty',
          hasObservation: false,
        }),
      ],
      zeroCoverageServerIds: ['wg-b'],
      absorbedCount: 1,
    };
    const markup = render(report, wgServers);
    expect(markup).toContain(ZERO_LINE); // 正向对照：这一格确实渲染了
    expect(markup).toContain('备用');
    expect(markup).not.toContain(CAVEAT);
  });
});
