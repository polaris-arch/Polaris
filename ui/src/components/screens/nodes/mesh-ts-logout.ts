/** Mesh Join 登出结果。失败原因取后端结构化 code，绝不解析错误文案。 */
export type MeshTsLogoutResult =
  | { ok: true; noticeKey: 'nodes.meshTsLogoutOk' }
  | { ok: false; noticeKey: 'ts.reasonMainCoreInUse' | 'ts.reasonStaleLoginCore' | 'nodes.meshTsLogoutFail'; error: unknown };

export async function performMeshTsLogout(
  serverId: string,
  logout: (serverId: string) => Promise<unknown>,
  setLoginState: (serverId: string, loggedIn: boolean) => void,
): Promise<MeshTsLogoutResult> {
  try {
    await logout(serverId);
    setLoginState(serverId, false);
    return { ok: true, noticeKey: 'nodes.meshTsLogoutOk' };
  } catch (error) {
    const mainCoreInUse = error !== null && typeof error === 'object' &&
      'code' in error && error.code === 'TAILSCALE_LOGOUT_MAIN_CORE';
    const staleLoginCore = error !== null && typeof error === 'object' &&
      'code' in error && error.code === 'TAILSCALE_LOGOUT_STALE_LOGIN_CORE';
    return {
      ok: false,
      noticeKey: mainCoreInUse ? 'ts.reasonMainCoreInUse'
        : staleLoginCore ? 'ts.reasonStaleLoginCore' : 'nodes.meshTsLogoutFail',
      error,
    };
  }
}
