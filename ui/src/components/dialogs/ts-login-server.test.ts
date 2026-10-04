/**
 * 提交计划纯逻辑单测（vitest，node 环境）。
 * 守的是「登录成果孤儿化」那条 bug：新建路径必须带真实 id 落盘后再登录。
 */
import { describe, expect, it } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import { supportsTsAccountActions, nextTsNodeName, planTsLoginSubmit, tsLoginMainCoreView, tsLoginFailureKey } from './ts-login-server';

const MINTED = 'minted-id-1';
const mint = () => MINTED;

function tsNode(overrides: Partial<ServerConfig> = {}): ServerConfig {
  return {
    id: 'ts-existing',
    name: 'Tailscale',
    protocol: 'tailscale',
    address: '',
    port: 0,
    tailscaleSettings: {},
    ...overrides,
  } as ServerConfig;
}

/** 入参默认值：只覆写用例真正关心的那几项，其余保持"未填/无既有状态"。 */
function base(
  overrides: Partial<Parameters<typeof planTsLoginSubmit>[0]> = {}
): Parameters<typeof planTsLoginSubmit>[0] {
  return { mode: 'browser', authKey: '', controlUrl: '', hasState: false, mintId: mint, ...overrides };
}

describe('planTsLoginSubmit —— 新建路径', () => {
  it('组网 TAB 中的默认名跳过已占用序号，大小写与首尾空格视为同名', () => {
    expect(nextTsNodeName(['WireGuard', 'tailscale ', 'TAILSCALE 3'])).toBe('Tailscale 2');
    expect(nextTsNodeName(['Tailscale', 'Tailscale 2'])).toBe('Tailscale 3');
    expect(nextTsNodeName(['WireGuard'])).toBe('Tailscale');
  });

  it('手填名称只裁剪首尾空格，不自动改写', () => {
    const { server } = planTsLoginSubmit(base({ name: '  办公室 TS  ' }));
    expect(server.name).toBe('办公室 TS');
  });

  it('无既有节点 → 带 mint 的真实 id，persist=add（绝不发空串 id）', () => {
    const { server, persist } = planTsLoginSubmit(base({ mode: 'browser' }));
    expect(server.id).toBe(MINTED);
    expect(server.id).not.toBe('');
    expect(persist).toBe('add');
    expect(server.protocol).toBe('tailscale');
  });

  it('authkey 模式新建 → key 落进 tailscaleSettings 一并写盘（不再只发给登录核）', () => {
    const { server, persist } = planTsLoginSubmit(base({ mode: 'authkey', authKey: '  tskey-auth-abc  ' }));
    expect(server.tailscaleSettings?.authKey).toBe('tskey-auth-abc');
    expect(persist).toBe('add');
  });

  it('默认设置留空 → 不覆写后端缺省（allowInternet / alwaysRouteSubnets 缺省即 true）', () => {
    const { server } = planTsLoginSubmit(base({ mode: 'browser' }));
    expect(server.tailscaleSettings).toEqual({});
  });
});

describe('planTsLoginSubmit —— 既有节点路径', () => {
  it('只改名称也要更新，保留既有 id 和登录设置', () => {
    const existing = tsNode({ name: 'Tailscale 2', tailscaleSettings: { sourceTag: 'mesh-a', authKey: 'old' } });
    const { server, persist } = planTsLoginSubmit(base({ existing, name: '  办公室 TS  ', mode: 'authkey', authKey: 'old' }));
    expect(persist).toBe('update');
    expect(server).toMatchObject({ id: existing.id, name: '办公室 TS', tailscaleSettings: existing.tailscaleSettings });
    expect(existing.name).toBe('Tailscale 2');
  });

  it('browser 模式 → 复用既有 id，persist=none（不写盘、不触发 CONFIG_CHANGED）', () => {
    const { server, persist } = planTsLoginSubmit(base({ existing: tsNode(), mode: 'browser' }));
    expect(server.id).toBe('ts-existing');
    expect(persist).toBe('none');
  });

  it('authkey 变更 → persist=update（key 必须落盘，否则 UI 报成功而配置里没有）', () => {
    const { server, persist } = planTsLoginSubmit(
      base({
        existing: tsNode({ tailscaleSettings: { authKey: 'old' } }),
        mode: 'authkey',
        authKey: 'new-key',
      })
    );
    expect(server.tailscaleSettings?.authKey).toBe('new-key');
    expect(persist).toBe('update');
  });

  it('authkey 未变 → persist=none（省一次无谓写盘/重启）', () => {
    const { persist } = planTsLoginSubmit(
      base({
        existing: tsNode({ tailscaleSettings: { authKey: 'same' } }),
        mode: 'authkey',
        authKey: '  same  ',
      })
    );
    expect(persist).toBe('none');
  });

  it('绝不 mutate 既有节点（app-store.servers 的 live 引用）', () => {
    const existing = tsNode({ tailscaleSettings: { authKey: 'old' } });
    const { server } = planTsLoginSubmit(base({ existing, mode: 'authkey', authKey: 'new-key' }));
    expect(existing.tailscaleSettings?.authKey).toBe('old');
    expect(server.tailscaleSettings).not.toBe(existing.tailscaleSettings);
  });

  it('多 TS 节点换授权方式仍保留该节点的来源标识与其他设置', () => {
    const existing = tsNode({ tailscaleSettings: { sourceTag: 'subscription-a', hostname: 'phone-a', authKey: 'old' } });
    const { server, persist } = planTsLoginSubmit(base({ existing, mode: 'authkey', authKey: 'new-key' }));
    expect(persist).toBe('update');
    expect(server.id).toBe(existing.id);
    expect(server.tailscaleSettings).toMatchObject({ sourceTag: 'subscription-a', hostname: 'phone-a', authKey: 'new-key' });
    expect(existing.tailscaleSettings?.authKey).toBe('old');
  });
});

describe('planTsLoginSubmit —— 自建控制面（controlUrl）', () => {
  it('新建时填 controlUrl → 一并落进 tailscaleSettings（首次登录就能打向 headscale）', () => {
    // 这条钉的是本轮修掉的缺口：此前登录弹窗没有该字段，而「TS 设置」弹窗要求节点已存在
    // ⇒ 自建控制面用户的第一次登录必然打向官方 controlplane，只能先登错一次再改。
    const { server, persist } = planTsLoginSubmit(
      base({ controlUrl: '  https://hs.example.com  ' })
    );
    expect(server.tailscaleSettings?.controlUrl).toBe('https://hs.example.com');
    expect(persist).toBe('add');
  });

  it('留空 → 删键而不是写空串（缺省即官方控制面）', () => {
    const { server } = planTsLoginSubmit(
      base({ existing: tsNode({ tailscaleSettings: { controlUrl: 'https://old.example' } }) })
    );
    expect(server.tailscaleSettings).not.toHaveProperty('controlUrl');
  });

  it('只改 controlUrl（browser 模式）也要落盘 —— 否则改了控制面却不生效', () => {
    const { persist } = planTsLoginSubmit(
      base({
        existing: tsNode({ tailscaleSettings: { controlUrl: 'https://old.example' } }),
        controlUrl: 'https://new.example',
      })
    );
    expect(persist).toBe('update');
  });

  it('controlUrl 未变 + browser → 仍 persist=none（不制造无谓重启）', () => {
    const { persist } = planTsLoginSubmit(
      base({
        existing: tsNode({ tailscaleSettings: { controlUrl: 'https://same.example' } }),
        controlUrl: 'https://same.example',
      })
    );
    expect(persist).toBe('none');
  });
});

describe('planTsLoginSubmit —— 切 auth_key 必须先退出登录', () => {
  it('authkey + 已有 state → requiresLogout（tsnet 有有效 node key 就不会去用 auth_key）', () => {
    const { requiresLogout } = planTsLoginSubmit(
      base({ existing: tsNode(), mode: 'authkey', authKey: 'k', hasState: true })
    );
    expect(requiresLogout).toBe(true);
  });

  it('authkey + 无 state → 不必退出（没什么可退）', () => {
    const { requiresLogout } = planTsLoginSubmit(
      base({ existing: tsNode(), mode: 'authkey', authKey: 'k', hasState: false })
    );
    expect(requiresLogout).toBe(false);
  });

  it('browser 模式即使有 state 也不退出（交互登录本来就会换身份）', () => {
    const { requiresLogout } = planTsLoginSubmit(
      base({ existing: tsNode(), mode: 'browser', hasState: true })
    );
    expect(requiresLogout).toBe(false);
  });
});


describe('移动登录：保存后的重试与安全授权状态', () => {
  it('保存后的节点即使刷新或登录失败，重试也不新增；自定义和官方控制面复用同 id', () => {
    const first = planTsLoginSubmit(base({ controlUrl: 'https://control.example.test' }));
    const retry = planTsLoginSubmit(base({ existing: first.server, controlUrl: 'https://control.example.test', mintId: () => { throw new Error('must not mint'); } }));
    expect(retry.persist).toBe('none');
    expect(retry.server.id).toBe(first.server.id);
    const official = planTsLoginSubmit(base({ existing: retry.server, controlUrl: '' }));
    expect(official.persist).toBe('update');
    expect(official.server.id).toBe(first.server.id);
    expect(official.server.tailscaleSettings?.controlUrl).toBeUndefined();
    expect(first.server.tailscaleSettings?.controlUrl).toBe('https://control.example.test');
  });
  const frame = (serverId: string, backendState = 'Running', loggedIn = true, expired = false, authURL?: string) => ({
    serverId, backendState, loggedIn, expired, authURL, tailscaleIPs: [], peers: [], canShareFiles: false,
    waitingFileCount: 0, receivingFileCount: 0, unreadFileCount: 0,
  });
  it('陈旧 STATUS、其它节点、启动过渡不能宣称授权；当前 Running 才可确认主核身份', () => {
    expect(tsLoginMainCoreView('a', { connected: false, statuses: [frame('a')] }, null).state).toBe('unknown');
    expect(tsLoginMainCoreView('a', { connected: true, statuses: [frame('b')] }, null).state).toBe('unknown');
    expect(tsLoginMainCoreView('a', { connected: true, statuses: [frame('a', 'NoState', false)] }, null).state).toBe('unknown');
    expect(tsLoginMainCoreView('a', { connected: true, statuses: [frame('a', 'Starting')] }, null).state).toBe('unknown');
    expect(tsLoginMainCoreView('a', { connected: true, statuses: [frame('a')] }, null).state).toBe('authorized');
    expect(tsLoginMainCoreView('a', { connected: true, statuses: [frame('a', 'Running', true, true)] }, null).state).toBe('needs-login');
  });
  it('主核 NeedsLogin 的 URL 可显示；授权后的新鲜帧压过旧 URL', () => {
    const url = 'https://login.example.test/authorize';
    expect(tsLoginMainCoreView('a', { connected: true, statuses: [frame('a', 'NeedsLogin', false, false, url)] }, null)).toEqual({ state: 'url', authUrl: url });
    expect(tsLoginMainCoreView('a', null, url)).toEqual({ state: 'url', authUrl: url });
    expect(tsLoginMainCoreView('a', { connected: true, statuses: [frame('a')] }, url)).toEqual({ state: 'authorized', authUrl: null });
  });
  it('只按稳定错误码分类，秘密/URL/path 的原始错误不进入展示键', () => {
    expect(tsLoginFailureKey({ code: 'TAILSCALE_LOGIN_FAILED', message: 'tskey-secret https://private /data/user' })).toBe('ts.loginStartFailed');
    expect(tsLoginFailureKey({ code: 'ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED', message: 'private' })).toBe('errors.androidNativeCapacityClosed');
    expect(tsLoginFailureKey({ message: 'ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED' })).toBe('ts.loginAttemptFailed');
    expect(tsLoginFailureKey(new Error('tskey-secret https://private /data/user'))).toBe('ts.loginAttemptFailed');
    expect(tsLoginFailureKey({ code: 'https://private' })).toBe('ts.loginAttemptFailed');
  });
});

// iOS owns only the main VPN; Android and desktop retain their account sessions.
it.each(['android', 'windows', 'macos', undefined])('keeps account actions on %s', (os) => {
  expect(supportsTsAccountActions(os)).toBe(true);
});
it('guards standalone iOS account actions', () => {
  expect(supportsTsAccountActions('ios')).toBe(false);
});
