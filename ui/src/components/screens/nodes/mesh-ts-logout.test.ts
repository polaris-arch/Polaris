import { describe, expect, it, vi } from 'vitest';
import { performMeshTsLogout } from './mesh-ts-logout';

describe('Mesh Join Tailscale 登出', () => {
  it('主核占用时显示可行动原因，保留登录态', async () => {
    const error = { code: 'TAILSCALE_LOGOUT_MAIN_CORE', message: 'backend detail' };
    const logout = vi.fn(async () => { throw error; });
    const setLoginState = vi.fn();

    const result = await performMeshTsLogout('ts-one', logout, setLoginState);

    expect(logout).toHaveBeenCalledExactlyOnceWith('ts-one');
    expect(result).toEqual({ ok: false, noticeKey: 'ts.reasonMainCoreInUse', error });
    expect(setLoginState).not.toHaveBeenCalled();
  });

  it('普通错误仍显示登出失败，保留登录态', async () => {
    const error = { code: 'OTHER_FAILURE' };
    const logout = vi.fn(async () => { throw error; });
    const setLoginState = vi.fn();

    const result = await performMeshTsLogout('ts-one', logout, setLoginState);

    expect(result).toEqual({ ok: false, noticeKey: 'nodes.meshTsLogoutFail', error });
    expect(setLoginState).not.toHaveBeenCalled();
  });

  it('成功后只将目标节点登录态置为 false 并报告成功', async () => {
    const logout = vi.fn(async () => undefined);
    const setLoginState = vi.fn();

    const result = await performMeshTsLogout('ts-one', logout, setLoginState);

    expect(logout).toHaveBeenCalledExactlyOnceWith('ts-one');
    expect(setLoginState).toHaveBeenCalledExactlyOnceWith('ts-one', false);
    expect(result).toEqual({ ok: true, noticeKey: 'nodes.meshTsLogoutOk' });
  });
});
