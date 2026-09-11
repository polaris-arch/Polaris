/**
 * 节点卡「网段被覆盖」角标的**真值源**门。
 *
 * # 守的是什么
 *
 * 角标此前由渲染端**自己重算**（`meshShadowedCidrs(meshForceRoutedServers(...))`），而那份重算
 * 走 `endpointForcedRouteCidrs`，对 Tailscale **恒**产出 `TAILNET_CGNAT` + `TAILNET_ULA_V6`
 * 两条硬编码常量 ⇒ **任意两个 Tailscale 节点的段必然完全重合** ⇒ 第二个恒亮「被覆盖」角标，
 * 无论两个 tailnet 是否真的相交。
 *
 * 那个前提已被真控制面实测推翻：自建 headscale 的 `prefixes.v4` 可自定义，实测把地址发成
 * `32.0.0.28` —— 与官方 CGNAT 段零相交。于是「两个 TS 节点」这个形态下角标说的每一句都是假的，
 * 而它长得和真冲突一模一样。
 *
 * 这正是 `builder::tun_exclusion_preview` 模块头注记的那次事故的同型：**前端拿自己那份计算冒充
 * 后端真值**（那次是折叠计数显示 1 条、内核实际排除 0 条）。权威真值今天已经有了 ——
 * `endpoint_force_route_report` 命令给的是**本次实际结算**（逐节点 emitted / absorbed / coverage，
 * 且带 `hasObservation` 证据强度位），角标必须消费它。
 *
 * # 为什么第一条用「首帧真渲染」而不是只测纯函数
 *
 * 纯函数测的是「给它一份报告它算得对」，测不出「生产到底有没有在用它」。SSR 不跑 `useEffect`
 * ⇒ 报告必然还没回来 ⇒ 首帧必须一个角标都不画。缺陷版（渲染端同步重算）在这一帧就会把两张
 * TS 卡里的第二张画上角标 —— 门看到的就是用户眼里那第一帧。
 * 手法沿用本仓既有先例（`initial-tab-first-frame.test.tsx` / `nodes-render-budget.test.tsx`）：
 * node 环境 + `react-dom/server`，**不引 jsdom**。
 */
import { describe, it, expect, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ServerConfig, UserConfig } from '@/contracts/types';
import type {
  EndpointForceRouteReport,
  ServerForceRoute,
} from '@/contracts/endpoint-force-route-report';
import { DEMO_CONFIG } from '../../../../harness-fixture';
import { shadowedCidrNamed } from './nodes-logic';

/**
 * t() 桩：返回 `key|名=值` —— 与语种解耦（同上述先例的 key-only 桩），但把插值**留在输出里**。
 *
 * 角标要答的「哪一段 · 被谁抢走」全部经 `t('nodes.shadowedHint', { cidrs, by })` 插值进 tooltip；
 * key-only 桩会把它们整段吃掉，于是「角标画出来了」与「角标画出来但没说被谁抢走」不可区分 ——
 * 而后者正是这条判据要防的（只给 cidr 用户仍无从下手，`AbsorbedCidr` 当初补 `byServerId` 就是这个理由）。
 */
vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) =>
      vars
        ? `${key}|${Object.entries(vars)
            .map(([k, v]) => `${k}=${String(v)}`)
            .join('|')}`
        : key,
    i18n: { language: 'zh-CN' },
  }),
}));

/** node 无 document，而部分模块加载期就要写 `<html dir/lang>` / portal 到 body。 */
(globalThis as unknown as { document: unknown }).document = {
  documentElement: { dir: '', lang: '', getAttribute: () => null, setAttribute: () => {} },
  body: { nodeType: 1 },
};

const ts = (id: string, name: string): ServerConfig =>
  ({
    id,
    name,
    protocol: 'tailscale',
    address: '',
    port: 0,
    tailscaleSettings: {},
  }) as ServerConfig;

/**
 * 两个 tailnet **不相交**的 Tailscale 节点。
 *
 * 「不相交」这件事渲染端根本看不见 —— 真实前缀只存在于运行期观测地址里
 * （`ts-home` 观测到 `32.0.0.28` 的自建 tailnet、`ts-office` 观测到 `100.64.5.5` 的官方段），
 * 而 `ServerConfig` 上没有这个维度。故渲染端**无法**对这个形态给出正确答案，只能不画。
 */
const SERVERS = [ts('ts-home', '家里'), ts('ts-office', '公司')];

const firstFrame = async (): Promise<string> => {
  const config: UserConfig = {
    ...DEMO_CONFIG,
    servers: SERVERS,
    subscriptions: [],
    selectedServerId: 'ts-home',
    customRules: [],
    appRules: [],
  };
  const seed = { config, servers: SERVERS, selectedServerId: 'ts-home', rules: [] };
  const { useAppStore } = await import('@/store/app-store');
  useAppStore.setState(seed);
  // zustand 在服务端渲染下读初始态快照，只 setState 会对着空 store 渲染（同 initial-tab-first-frame）。
  Object.assign(useAppStore.getInitialState(), seed);
  const NodesScreen = (await import('./NodesScreen')).default;
  return renderToStaticMarkup(<NodesScreen />);
};

const occurrences = (src: string, needle: string): number => src.split(needle).length - 1;

const NAMES: ReadonlyMap<string, string> = new Map(SERVERS.map((s) => [s.id, s.name]));

const leg = (over: Partial<ServerForceRoute> & { serverId: string }): ServerForceRoute => ({
  leg: 'inline',
  hasObservation: true,
  emitted: [],
  absorbed: [],
  coverage: 'covered',
  ...over,
});

/**
 * 两个 tailnet 不相交时后端**真正**会给出的结算：各发各的观测段，谁也没吃掉谁。
 * `hasObservation: true` 是这个形态的前提 —— 没有观测时两个 TS 节点发的都是默认常量段，
 * 后端也只能报重合（那时的重合不是证据，由设置页那块补证据强度说明，与角标无关）。
 */
const DISJOINT: EndpointForceRouteReport = {
  servers: [
    leg({ serverId: 'ts-home', emitted: ['32.0.0.28/32'] }),
    leg({ serverId: 'ts-office', emitted: ['100.64.5.5/32'] }),
  ],
  zeroCoverageServerIds: [],
  absorbedCount: 0,
};

/** 真的相交：`ts-home` 先占住 `100.64.0.0/10`，`ts-office` 的同一段被吃干净 ⇒ 零覆盖。 */
const OVERLAPPING: EndpointForceRouteReport = {
  servers: [
    leg({ serverId: 'ts-home', emitted: ['100.64.0.0/10'] }),
    leg({
      serverId: 'ts-office',
      absorbed: [{ cidr: '100.64.0.0/10', byServerId: 'ts-home' }],
      coverage: 'absorbedEmpty',
    }),
  ],
  zeroCoverageServerIds: ['ts-office'],
  absorbedCount: 1,
};

const cardMarkup = async (badges: { cidr: string; by: string }[] | undefined): Promise<string> => {
  const { NodeCard } = await import('./NodeCard');
  return renderToStaticMarkup(<NodeCard server={SERVERS[1]} shadowedCidrs={badges} />);
};

describe('判据 1 · 两个 tailnet 不相交的 TS 节点：两张卡都不画「被覆盖」角标', () => {
  it('首帧（报告尚未返回）一个 `nd-cap shadow` 都没有', async () => {
    const html = await firstFrame();
    // 自曝：两张卡真的渲出来了。渲染失败/空态时下面的负向断言恒真。
    expect(html).toContain('node-grid');
    expect(occurrences(html, 'class="nd-card')).toBe(2);
    expect(html).toContain('家里');
    expect(html).toContain('公司');

    // 缺陷版在这一帧就会给「公司」画上角标（两条硬编码 tailnet 常量必然重合）。
    expect(occurrences(html, 'nd-cap shadow')).toBe(0);
  });

  it('报告回来了、且说两边各发各的 ⇒ 角标表为空', () => {
    expect([...shadowedCidrNamed(DISJOINT, NAMES).keys()]).toEqual([]);
  });
});

describe('判据 2 · 真的相交：被吸收那个显示角标，且点名抢占者', () => {
  it('角标表只含败方，并带上「哪一段 · 被谁抢走」', () => {
    const named = shadowedCidrNamed(OVERLAPPING, NAMES);
    expect([...named.keys()]).toEqual(['ts-office']);
    // 抢占者以**显示名**出现（只给 id 用户仍无从下手）。
    expect(named.get('ts-office')).toEqual([{ cidr: '100.64.0.0/10', by: '家里' }]);
  });

  it('成品真的画到了卡上：角标出现，tooltip 携带段与抢占者', async () => {
    const html = await cardMarkup(shadowedCidrNamed(OVERLAPPING, NAMES).get('ts-office'));
    expect(html).toContain('nd-cap shadow');
    expect(html).toContain('100.64.0.0/10');
    expect(html).toContain('家里');
  });
});

describe('判据 3 · 报告拿不到：不画角标，也不画「无冲突」', () => {
  it('`null`（命令失败 / 尚未返回）⇒ 空表，不退回本地重算', () => {
    expect(shadowedCidrNamed(null, NAMES).size).toBe(0);
  });

  it('卡面上「没拿到报告」与「报告说没冲突」逐字节同形 —— 卡片没有说「无冲突」的位置', async () => {
    const noReport = await cardMarkup(undefined);
    const noConflict = await cardMarkup(shadowedCidrNamed(DISJOINT, NAMES).get('ts-office'));
    // 正向对照：同一张卡在有冲突时确实会变（否则下面这条相等恒真、无信息量）。
    const conflicted = await cardMarkup(shadowedCidrNamed(OVERLAPPING, NAMES).get('ts-office'));
    expect(conflicted).not.toBe(noReport);
    expect(noConflict).toBe(noReport);
    expect(noReport).not.toContain('nd-cap shadow');
  });
});

describe('口径一致 · 角标与设置页那份报告读同一个 `absorbed`', () => {
  it('同一份结算：角标点名的败方 = 设置页块点名的零覆盖节点，段与抢占者逐字相同', async () => {
    const named = shadowedCidrNamed(OVERLAPPING, NAMES);
    const { EndpointForceRouteBlock } = await import('../settings/EndpointForceRouteBlock');
    const block = renderToStaticMarkup(
      <EndpointForceRouteBlock report={OVERLAPPING} servers={SERVERS} />,
    );
    // 自曝：设置页那块真的渲出了结算内容（渲成 null/空时下面三条恒真）。
    expect(block).toContain('cidr-eff-list');

    const loser = [...named.keys()];
    expect(loser).toEqual(OVERLAPPING.zeroCoverageServerIds);
    for (const id of loser) expect(block).toContain(NAMES.get(id));
    for (const { cidr, by } of named.get('ts-office')!) {
      expect(block).toContain(cidr);
      expect(block).toContain(by);
    }
  });

  it('角标表的键集 = 报告里 `absorbed` 非空的节点集（不是零覆盖集，也不是全部节点）', () => {
    const mixed: EndpointForceRouteReport = {
      servers: [
        leg({ serverId: 'ts-home', emitted: ['10.0.0.0/24'] }),
        // 被吃掉一段、但自己还剩一段能发 ⇒ 不是零覆盖，角标照样要画（那一段确实不生效）。
        leg({
          serverId: 'ts-office',
          emitted: ['10.1.0.0/24'],
          absorbed: [{ cidr: '10.0.0.0/24', byServerId: 'ts-home' }],
        }),
      ],
      zeroCoverageServerIds: [],
      absorbedCount: 1,
    };
    expect([...shadowedCidrNamed(mixed, NAMES).keys()]).toEqual(['ts-office']);
  });
});

/* ════════════════════════════════════════════════════════════════════════════
 * 接线门 —— 「纯函数算得对」测不出「生产在不在用它」
 *
 * 这几条同时是**判据 4（反向对照）的常驻形态**：把角标改回本地重算，无论改在节点屏哪个文件，
 * 下面的负向断言都会转红 —— 而不是等某个纯函数用例恰好覆盖到那个形态。
 * 手段与射程同 `nodes-render-budget.test.tsx`（源码结构门只用于「接线在不在」这类在 node 环境
 * 不可观测、却正是缺陷复发那一层的事实）。
 * ════════════════════════════════════════════════════════════════════════════ */

const read = (rel: string): string => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');

/** 去注释 —— 负向断言必须跑在它上面：本文件与被扫文件的注释里都逐字写着被删掉的旧形态。 */
const code = (src: string): string =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/.*$/gm, '$1');

const NODES_RAW = read('./NodesScreen.tsx');
const LOGIC_RAW = read('./nodes-logic.ts');
const ROUTES_RAW = read('../../../domain/endpoint-routes.ts');
const NODES = code(NODES_RAW);
const LOGIC = code(LOGIC_RAW);
const GRID = code(read('./NodesGrid.tsx'));
const CARD = code(read('./NodeCard.tsx'));
const ROUTES = code(ROUTES_RAW);
/** 节点屏的负向断言必须扫全部拆出的视图块，否则缺陷换个文件出现就检测不到。 */
const SCREEN_ALL = [NODES, LOGIC, GRID, CARD].join('\n');

describe('接线 · 角标的真值源是后端报告，渲染端不得再重算', () => {
  it('自曝：取材面还在（去注释后仍是可断言的代码）', () => {
    expect(NODES_RAW.length).toBeGreaterThan(1000);
    expect(LOGIC_RAW.length).toBeGreaterThan(1000);
    expect(ROUTES_RAW.length).toBeGreaterThan(1000);
    expect(NODES).toContain('export function NodesScreen');
    expect(LOGIC).toContain('export function shadowedCidrNamed');
    expect(ROUTES).toContain('export function meshForceRoutedServers');
  });

  it('NodesScreen 拉 `endpoint_force_route_report`，并把它原样喂给角标', () => {
    expect(NODES).toContain('api.config');
    expect(NODES).toContain('.endpointForceRouteReport()');
    expect(NODES).toMatch(/const shadowedNamed = useMemo\(/);
    expect(NODES).toContain('shadowedCidrNamed(forceRouteReport, serverNameById)');
  });

  it('角标读的是报告的 `absorbed`，不是任何本地推算', () => {
    expect(LOGIC).toContain('report.servers');
    expect(LOGIC).toContain('s.absorbed');
    // 拿不到报告时**返回空表**，不是退回重算 —— 这一行就是空态纪律本身。
    expect(LOGIC).toMatch(/if \(report === null\) return named;/);
  });

  it('节点屏一行本地重算都不剩（判据 4 的常驻形态）', () => {
    for (const banned of [
      'meshShadowedCidrs',
      'shadowedCidrIndex',
      'endpointForcedRouteCidrs',
      'meshForceRoutedServers',
    ]) {
      expect(SCREEN_ALL, `节点屏又出现了本地重算：${banned}`).not.toContain(banned);
    }
  });

  it('`meshShadowedCidrs` 已从 domain 层删除（不是留着没人用）', () => {
    // 留一份没人用的同义实现，下一个人接上它就等于回归；墓碑注释在原文里，故扫去注释面。
    expect(ROUTES).not.toContain('meshShadowedCidrs');
    expect(ROUTES).not.toContain('ShadowedCidr');
    // 正向对照：墓碑说明还在原文里（防止有人把整段连同解释一起删掉）。
    expect(ROUTES_RAW).toContain('`meshShadowedCidrs` / `ShadowedCidr` 已删');
  });
});
