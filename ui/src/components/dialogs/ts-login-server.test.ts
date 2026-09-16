/**
 * 提交计划纯逻辑单测（vitest，node 环境）。
 * 守的是「登录成果孤儿化」那条 bug：新建路径必须带真实 id 落盘后再登录。
 */
import { describe, expect, it } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import { planTsLoginSubmit } from './ts-login-server';

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
