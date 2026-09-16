/**
 * 「Auth Key 已保存 / 未保存」状态行的**渲染门**。
 *
 * # 为什么必须是渲染门
 *
 * 本行要守的不变式是「界面上永远不出现 key 的任何片段」。这是**关于 DOM 的**断言：
 * 逻辑单测证明得了 `hasTsAuthKey` 返回布尔，证明不了弹窗没在别处把 `node.tailscaleSettings.authKey`
 * 直接插进 JSX（那正是最容易顺手写出来的一行：`<span>{ts.authKey}</span>` 做「回显」）。
 * 手段沿用本仓既有先例（`harness-screens.test.tsx` / `FieldSpec.switch-disabled.test.tsx`）：
 * node 环境 + `react-dom/server`，本仓刻意不装 jsdom / testing-library，不为这道门破例。
 *
 * # 哨兵
 *
 * 喂进去的 key 是 `tskey-auth-SENTINEL-DO-NOT-RENDER` —— 特征串全仓只此一处，于是「有没有漏出去」
 * 是可判的；前缀保持真 key 形状，免得判据被「长得不像 key」绕过。
 *
 * 下面每条否定断言都配了**正向对照**：只写 `not.toContain` 的门会被「整个弹窗根本没渲染出来」骗成绿的。
 *
 * # 抓不到什么
 *
 * 只有首帧（`useEffect` 在 SSR 不跑 ⇒ 出口候选列表恒空、`connected` 恒 null），以及零 CSS、
 * 零交互 —— 「点两下真的清掉了」归 `ts-settings-logic.test.ts` 的纯逻辑判据与真机。
 */
import { describe, it, expect, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ServerConfig } from '@/contracts/types';

/** t() 桩：返回 key 本身（同 harness-screens 先例）——断言落在结构上，与语种文案解耦。 */
vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: 'zh-CN' } }),
}));

/** node 无 document，而 `@/i18n` 模块加载期就写 `<html dir/lang>`、Csel 还要 portal 到 body。 */
(globalThis as unknown as { document: unknown }).document = {
  documentElement: { dir: '', lang: '', getAttribute: () => null, setAttribute: () => {} },
  body: { nodeType: 1 },
};

const { useAppStore } = await import('@/store/app-store');
const { default: TsSettingsDialog } = await import('./TsSettingsDialog');

const SENTINEL = 'tskey-auth-SENTINEL-DO-NOT-RENDER';

const tsNode = (authKey?: string): ServerConfig =>
  ({
    id: 'ts1',
    name: 'Tailscale',
    protocol: 'tailscale',
    address: '',
    port: 0,
    tailscaleSettings: authKey === undefined ? {} : { authKey },
  }) as ServerConfig;

/**
 * zustand v4 在服务端渲染下读的是**初始态**（`api.getServerState || api.getInitialState`），
 * 只 `setState` 的话每次都在渲染空 store —— 那会让下面的否定断言退化成「空屏也全绿」。
 * 故对初始态对象就地播种（同 `harness-screens.test.tsx` 的那条腿）。
 */
const seed = (servers: ServerConfig[]) => {
  useAppStore.setState({ servers } as never);
  Object.assign(useAppStore.getInitialState(), { servers });
};

const render = (servers: ServerConfig[]): string => {
  seed(servers);
  // 本弹窗现在按 serverId 寻址（不再自查 protocol==='tailscale'）；固定节点 id 与 tsNode() 保持一致。
  return renderToStaticMarkup(<TsSettingsDialog serverId="ts1" />);
};

describe('TsSettingsDialog：Auth Key 状态行只渲染布尔事实', () => {
  it('正向对照：哨兵串真的能被 renderToStaticMarkup 渲染出来（否定断言有牙）', () => {
    const leak = renderToStaticMarkup(<div>{tsNode(SENTINEL).tailscaleSettings?.authKey}</div>);
    expect(leak).toContain('SENTINEL');
  });

  /** 清除按钮的**精确**针（`ts.authKeyClear` 是 `...ClearHint` / `...ClearPending` 的前缀，裸搜会误判）。 */
  const CLEAR_BTN = '>ts.authKeyClear</button>';

  it('判据4 不泄露：已存 key 时界面只出现「已保存」，key 的任何片段都不进 DOM', () => {
    const html = render([tsNode(SENTINEL)]);
    // 正向对照：这一行真的渲染出来了（否则下面三条否定断言全是空转）。
    expect(html).toContain('ts.authKeySaved');
    expect(html).toContain(CLEAR_BTN);
    // 本门的本体：哨兵串、其片段、乃至 key 前缀都不许出现。
    expect(html).not.toContain('SENTINEL');
    expect(html).not.toContain('DO-NOT-RENDER');
    expect(html).not.toContain('tskey');
  });

  it('未存 key ⇒ 显示「未保存」，且不给清除按钮（没有对象可清）', () => {
    const html = render([tsNode()]);
    expect(html).toContain('ts.authKeyNone');
    expect(html).not.toContain('ts.authKeySaved');
    expect(html).not.toContain(CLEAR_BTN);
    // 说明性 tip 常驻（它解释的是「这一行是什么」，与有没有 key 无关）。
    expect(html).toContain('ts.authKeyClearHint');
  });

  it('状态行与登录态两条动作并存且互不冒充（退出登录仍在，清除不是它）', () => {
    const html = render([tsNode(SENTINEL)]);
    expect(html).toContain('ts.logout');
    expect(html).toContain(CLEAR_BTN);
  });
});

/**
 * F1a 判据 1/2：Tailscale 不再是单例，`TsSettingsDialog` 必须按 `serverId` 精确寻址，
 * 不许 `.find(protocol==='tailscale')` 取到任意一个。hostname 是可区分两个节点的字段
 * （FormTabs 默认落在 basic 页，SSR 首帧即可见，无需交互）。
 */
describe('TsSettingsDialog：按 serverId 寻址（不再是单例，不许自查任意一个）', () => {
  const tsNodeWithHostname = (id: string, hostname: string): ServerConfig =>
    ({
      id,
      name: 'Tailscale',
      protocol: 'tailscale',
      address: '',
      port: 0,
      tailscaleSettings: { hostname },
    }) as ServerConfig;

  const twoNodes = [tsNodeWithHostname('ts-a', 'host-a'), tsNodeWithHostname('ts-b', 'host-b')];

  it('正向：两个 TS 节点，携 ts-b 的 serverId 渲染出的是 ts-b 的 hostname', () => {
    seed(twoNodes);
    const html = renderToStaticMarkup(<TsSettingsDialog serverId="ts-b" />);
    expect(html).toContain('value="host-b"');
    expect(html).not.toContain('value="host-a"');
  });

  it('反向对照：携 ts-a 的 serverId 时渲染出的是 ts-a（证明上一条断言真的挂在寻址上，不是巧合）', () => {
    seed(twoNodes);
    const html = renderToStaticMarkup(<TsSettingsDialog serverId="ts-a" />);
    expect(html).toContain('value="host-a"');
    expect(html).not.toContain('value="host-b"');
  });

  it('serverId 取不到（节点已删 / id 不匹配）→ 空态，绝不回落到任意一个既有节点', () => {
    seed(twoNodes);
    const html = renderToStaticMarkup(<TsSettingsDialog serverId="ts-missing" />);
    expect(html).toContain('ts.noNode');
    expect(html).not.toContain('host-a');
    expect(html).not.toContain('host-b');
  });
});
