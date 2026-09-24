/**
 * 组网接入卡的 **Tailscale 多节点寻址门**。
 *
 * # 守什么
 *
 * Tailscale 的全局单例槽已撤（`meshSingletonConflict` 只剩 WARP 一支）⇒ 配置里可以有 N 个 TS 节点。
 * 而这张卡上挂着**全仓唯一的 Taildrop 入口**（`kind:'taildrop'` 在生产代码里只此一处 `go`）：
 * 只要它还用 `servers.find(protocol === 'tailscale')` 取「任意一个」，第二个及以后的账号就
 * **永远进不去自己的收件箱**，而且界面上看不出少了什么 —— 静默失效。
 *
 * 两条判据成对，缺一条都能被绕过：
 *  - **判据 1（零回归）**：恰好 1 个 TS 节点时，这张卡的结构与此前逐项相同（单块 tile、标题恒为
 *    `Tailscale`、三颗动作按钮同序同类名）。
 *  - **判据 2（各自寻址）**：≥2 个时每节点一行，**第 N 行的每颗按钮都携第 N 个节点的 id**。
 *    反向对照写在同一组断言里：第 1 行必须是 ts-a、第 2 行必须是 ts-b —— 改回 `.find()` 时
 *    整张卡只剩一行，「两行」那条当场转红。
 *
 * # 为什么能在 node 环境测「点击」
 *
 * 本仓 vitest 是 `environment:'node'`、刻意不装 jsdom（见 `vite.config.ts`），组件交互按惯例测不了。
 * 这里把三个 hook 模块整体 mock 成纯函数 ⇒ `MeshJoinDialog` 本身变成一个**无 hook 的纯函数**，
 * 可以直接调用并拿到 React 元素树，再从树上取出真实的 `onClick` 闭包执行。
 * 这不是「测方法体」：取出来的就是生产 JSX 上挂的那一个闭包，它调的就是真的 `openDialog`。
 *
 * 结构那一半用 `renderToStaticMarkup`（同 `TsSettingsDialog.auth-key-row.test.tsx` 的先例）。
 *
 * # 抓不到什么（射程自曝）
 *
 * 只有首帧、零 CSS、零真实焦点/键盘。「逐像素相同」这句话在本层只能落到**结构等价**
 * （tile 数 / 标题 / 描述键 / 按钮序与类名），像素归真机。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { isValidElement, type ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { ServerConfig } from '@/contracts/types';
import type { TailscaleStatusEvent } from '@/contracts/tailscale-status';

/** 三个 hook 模块的共享可变替身（`vi.mock` 工厂被提升，只能经 `vi.hoisted` 拿到它）。 */
const h = vi.hoisted(() => ({
  servers: [] as unknown[],
  statuses: {} as Record<string, unknown>,
  opened: [] as unknown[],
  closes: 0,
}));

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: 'zh-CN' } }),
}));

vi.mock('@/store/app-store', () => ({
  useEffectiveServers: () => h.servers,
  useAppStore: (selector: (state: unknown) => unknown) =>
    selector({ tailscaleStatuses: h.statuses }),
}));

vi.mock('./dialog-store', () => ({
  useDialogStore: (selector: (state: unknown) => unknown) =>
    selector({
      open: (next: unknown) => h.opened.push(next),
      close: () => {
        h.closes += 1;
      },
    }),
}));

/** `@/i18n` 在模块加载期写 `<html dir/lang>`；node 环境没有 document。 */
(globalThis as unknown as { document: unknown }).document = {
  documentElement: { dir: '', lang: '', getAttribute: () => null, setAttribute: () => {} },
  body: { nodeType: 1 },
};

const { MeshJoinDialog } = await import('./MeshJoinDialog');

// ── 夹具 ────────────────────────────────────────────────────────────────────

const tsNode = (id: string, name: string): ServerConfig =>
  ({ id, name, protocol: 'tailscale', address: '', port: 0 }) as ServerConfig;

/** 只填 `tsAccountLabel` 读的两段（登录名唯一 + tailnet 名），其余字段本门不消费。 */
const status = (login: string, tailnet: string): TailscaleStatusEvent =>
  ({
    serverId: 'x',
    backendState: 'Running',
    loggedIn: true,
    tailscaleIPs: [],
    expired: false,
    peers: [],
    details: {
      stateText: 'Running',
      networkName: tailnet,
      magicDNSSuffix: '',
      keyAuth: false,
      userGroups: [{ userID: 1, loginName: login, displayName: login, profilePicURL: '', peers: [] }],
    },
  }) as unknown as TailscaleStatusEvent;

const logouts: ServerConfig[] = [];
const props = {
  onTsLogout: (node: ServerConfig) => void logouts.push(node),
  onWarpReregister: () => {},
  onWarpDeregister: () => {},
};

beforeEach(() => {
  h.servers = [];
  h.statuses = {};
  h.opened = [];
  h.closes = 0;
  logouts.length = 0;
});

// ── 元素树遍历（无 jsdom，故直接在 React 元素上找按钮并执行它的 onClick）─────────────

type AnyEl = ReactElement<Record<string, unknown>>;

/** 深度遍历**全部 props**（不只 children）——动作按钮住在 `Choice` 的 `actions` prop 里。 */
function walk(node: unknown, visit: (el: AnyEl) => void): void {
  if (Array.isArray(node)) {
    for (const child of node) walk(child, visit);
    return;
  }
  if (!isValidElement(node)) return;
  const el = node as AnyEl;
  visit(el);
  for (const value of Object.values(el.props)) walk(value, visit);
}

/**
 * 卡片 tile（内部 `Choice` 组件）按文档顺序。判据取 `title + icon + description` 三件套：
 * 只认前两个会把外层 `Modal`（同样有 title/icon）也算进来。
 */
function tiles(tree: unknown): AnyEl[] {
  const out: AnyEl[] = [];
  walk(tree, (el) => {
    if (
      typeof el.type === 'function' &&
      'title' in el.props &&
      'icon' in el.props &&
      'description' in el.props
    ) {
      out.push(el);
    }
  });
  return out;
}

/** 一段子树里的 `<button>`，按文档顺序，带它的可见文案与 onClick。 */
function buttons(node: unknown): { label: string; click: () => void }[] {
  const out: { label: string; click: () => void }[] = [];
  walk(node, (el) => {
    if (el.type !== 'button') return;
    const text: string[] = [];
    const collect = (n: unknown): void => {
      if (typeof n === 'string') text.push(n);
      else if (Array.isArray(n)) n.forEach(collect);
      else if (isValidElement(n)) collect((n as AnyEl).props.children);
    };
    collect(el.props.children);
    out.push({ label: text.join(''), click: el.props.onClick as () => void });
  });
  return out;
}

const tree = () => MeshJoinDialog(props) as unknown;
const html = () => renderToStaticMarkup(<MeshJoinDialog {...props} />);

// ════════════════════════════════════════════════════════════════════════════
// 判据 1：恰好 1 个 TS 节点 ⇒ 与改动前结构逐项相同
// ════════════════════════════════════════════════════════════════════════════

describe('判据 1 —— 单个 Tailscale 节点：结构零回归', () => {
  it('仍是单块 tile，标题恒为 `Tailscale`（不带节点名后缀）', () => {
    h.servers = [tsNode('ts-a', '家里')];
    const all = tiles(tree());
    const titles = all.map((el) => el.props.title);
    // 六块 tile 的完整清单与顺序（WARP 在 Tailscale 之前，隧道四块在后；MASQUE 2026-09-24 追加在末尾）。
    expect(titles).toEqual([
      'Cloudflare WARP',
      'Tailscale',
      'OpenConnect',
      'OpenVPN',
      'WireGuard',
      'MASQUE',
    ]);
  });

  it('描述用 `meshJoin.tsConfigured`，三颗动作按钮同序同类名', () => {
    h.servers = [tsNode('ts-a', '家里')];
    const ts = tiles(tree()).find((el) => el.props.title === 'Tailscale')!;
    expect(ts.props.description).toBe('meshJoin.tsConfigured');
    const acts = buttons(ts.props.actions);
    expect(acts.map((b) => b.label)).toEqual([
      'meshJoin.taildrop',
      'meshJoin.switchAccount',
      'meshJoin.logout',
    ]);
    const classes: string[] = [];
    walk(ts.props.actions, (el) => {
      if (el.type === 'button') classes.push(el.props.className as string);
    });
    expect(classes).toEqual(['btn ghost sm', 'btn ghost sm', 'btn ghost sm danger-text']);
  });

  it('三颗动作 + tile 自身都指向那唯一一个节点', () => {
    h.servers = [tsNode('ts-a', '家里')];
    const ts = tiles(tree()).find((el) => el.props.title === 'Tailscale')!;
    (ts.props.onClick as () => void)();
    buttons(ts.props.actions)[0].click();
    buttons(ts.props.actions)[1].click();
    expect(h.opened).toEqual([
      { kind: 'ts-settings', serverId: 'ts-a' },
      { kind: 'taildrop', serverId: 'ts-a' },
      { kind: 'ts-login', serverId: 'ts-a' },
    ]);
    buttons(ts.props.actions)[2].click();
    expect(logouts.map((n) => n.id)).toEqual(['ts-a']);
  });

  it('0 个 TS 节点：仍是单块 tile、描述回 `meshJoin.tsNew`、无动作、点击去登录', () => {
    h.servers = [];
    const ts = tiles(tree()).find((el) => el.props.title === 'Tailscale')!;
    expect(ts.props.description).toBe('meshJoin.tsNew');
    expect(buttons(ts.props.actions)).toEqual([]);
    (ts.props.onClick as () => void)();
    expect(h.opened).toEqual([{ kind: 'ts-login' }]);
  });

  it('真渲染一遍：单节点时 DOM 里只有一块 Tailscale tile 与一颗 Taildrop 按钮', () => {
    h.servers = [tsNode('ts-a', '家里')];
    const markup = html();
    expect(markup.match(/>Tailscale</g) ?? []).toHaveLength(1);
    expect(markup.match(/meshJoin\.taildrop/g) ?? []).toHaveLength(1);
    // 正向对照：这张卡真的渲染出来了（否则上面两条计数断言在空串上一样"通过"）。
    expect(markup).toContain('Cloudflare WARP');
    expect(markup).toContain('meshJoin.title');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// 判据 2：≥2 个 TS 节点 ⇒ 每行各自寻址
// ════════════════════════════════════════════════════════════════════════════

describe('判据 2 —— 多个 Tailscale 节点：逐行各自寻址', () => {
  const two = () => {
    h.servers = [tsNode('ts-a', '家里'), tsNode('ts-b', '公司')];
    h.statuses = {
      'ts-a': status('alice@example.com', 'alice-tailnet'),
      'ts-b': status('bob@example.com', 'bob-tailnet'),
    };
  };

  it('两个节点 ⇒ 两行，标题各带自己的节点名', () => {
    // 这一条就是「改回 servers.find(...)」的反向对照：那时只会有一行，本条当场转红。
    two();
    const titles = tiles(tree()).map((el) => el.props.title);
    expect(titles).toEqual([
      'Cloudflare WARP',
      'Tailscale · 家里',
      'Tailscale · 公司',
      'OpenConnect',
      'OpenVPN',
      'WireGuard',
      'MASQUE',
    ]);
  });

  it('副标题用 tsAccountLabel 区分账号（同为「已配置」时唯一能区分的那一维）', () => {
    two();
    const rows = tiles(tree()).filter((el) => String(el.props.title).startsWith('Tailscale'));
    expect(rows.map((el) => el.props.description)).toEqual([
      'alice@example.com · alice-tailnet',
      'bob@example.com · bob-tailnet',
    ]);
  });

  it('**第二行的每颗按钮都携第二个节点的 id**（第一行携第一个，互为反向对照）', () => {
    two();
    const rows = tiles(tree()).filter((el) => String(el.props.title).startsWith('Tailscale'));
    expect(rows).toHaveLength(2);

    const second = buttons(rows[1].props.actions);
    expect(second.map((b) => b.label)).toEqual([
      'meshJoin.taildrop',
      'meshJoin.switchAccount',
      'meshJoin.logout',
    ]);
    second[0].click();
    second[1].click();
    (rows[1].props.onClick as () => void)();
    expect(h.opened).toEqual([
      { kind: 'taildrop', serverId: 'ts-b' },
      { kind: 'ts-login', serverId: 'ts-b' },
      { kind: 'ts-settings', serverId: 'ts-b' },
    ]);
    second[2].click();
    expect(logouts.map((n) => n.id)).toEqual(['ts-b']);

    h.opened = [];
    logouts.length = 0;
    const first = buttons(rows[0].props.actions);
    first[0].click();
    (rows[0].props.onClick as () => void)();
    first[2].click();
    expect(h.opened).toEqual([
      { kind: 'taildrop', serverId: 'ts-a' },
      { kind: 'ts-settings', serverId: 'ts-a' },
    ]);
    expect(logouts.map((n) => n.id)).toEqual(['ts-a']);
  });

  it('每一行动作前都先关掉本弹窗（与单节点腿同一条 `go`/`action` 语义）', () => {
    two();
    const rows = tiles(tree()).filter((el) => String(el.props.title).startsWith('Tailscale'));
    buttons(rows[1].props.actions)[0].click();
    expect(h.closes).toBe(1);
  });

  it('三个节点也各自成行（不是「只放宽到两个」）', () => {
    h.servers = [tsNode('ts-a', 'A'), tsNode('ts-b', 'B'), tsNode('ts-c', 'C')];
    const rows = tiles(tree()).filter((el) => String(el.props.title).startsWith('Tailscale'));
    expect(rows.map((el) => el.props.title)).toEqual([
      'Tailscale · A',
      'Tailscale · B',
      'Tailscale · C',
    ]);
    for (const row of rows) buttons(row.props.actions)[0].click();
    expect(h.opened).toEqual([
      { kind: 'taildrop', serverId: 'ts-a' },
      { kind: 'taildrop', serverId: 'ts-b' },
      { kind: 'taildrop', serverId: 'ts-c' },
    ]);
  });

  it('真渲染一遍：两行都在 DOM 里，Taildrop 入口出现两次', () => {
    two();
    const markup = html();
    expect(markup).toContain('家里');
    expect(markup).toContain('公司');
    expect(markup.match(/meshJoin\.taildrop/g) ?? []).toHaveLength(2);
  });
});

describe('隧道接入：MASQUE 入口', () => {
  it('点 MASQUE 打开通用节点弹窗并预选 masque-client（取的是生产 JSX 上挂的那个闭包）', () => {
    const masque = tiles(tree()).find((el) => el.props.title === 'MASQUE');
    expect(masque, '组网弹窗里没有 MASQUE 卡片').toBeDefined();
    expect(masque!.props.description).toBe('meshJoin.masque');
    (masque!.props.onClick as () => void)();
    expect(h.opened).toEqual([{ kind: 'node', initialProto: 'masque-client' }]);
  });
});
