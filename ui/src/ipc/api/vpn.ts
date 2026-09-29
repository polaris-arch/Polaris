import { invoke, listen } from '../ipc-client';
import { IPC_CHANNELS } from '../../domain/ipc-channels';
import type { OpenConnectBrowserCookieInput, OpenConnectBrowserHeaderInput, OpenConnectStatusEvent, OpenVpnStatusEvent, VpnStatusSnapshot } from '../../contracts/vpn-status';

/**
 * 系统 VPN 授权状态的三态（Rust `runtime::proxy::android_bridge::VpnAuthState` 的线上形态）。
 *
 * 🔴 `'unknown'` **不是** `'denied'`：桥未接线 / 超时 / 本平台没有这个对象都落 `'unknown'`。
 * 把它显示成「未授权」会把用户指去系统设置里授予一个可能已经给过的权限 —— 那是编事实，
 * 而 IA §2.4 对「没有登记来源的数据位」的处置是**说读不到**，不是猜一个。
 */
export type VpnAuthState = 'authorized' | 'denied' | 'unknown';

export const vpnApi = {
  getStatus(): Promise<VpnStatusSnapshot> {
    return invoke(IPC_CHANNELS.VPN_GET_STATUS);
  },
  /**
   * 读一次系统 VPN 授权状态。**只读、无副作用**（Kotlin 侧刻意不 `startActivityForResult`）。
   *
   * 没有订阅腿可用：授权在应用之外被改，Android 不推这件事的变更事件（`onRevoke` 只在本应用的
   * 隧道被顶掉时回调）。故时机由调用点定 —— 见 `mobile/settings/MobileSettingsScreen.tsx`。
   */
  getAuthStatus(): Promise<VpnAuthState> {
    return invoke(IPC_CHANNELS.VPN_AUTH_STATUS);
  },
  onOpenConnectStatus(listener: (data: OpenConnectStatusEvent) => void): () => void {
    return listen(IPC_CHANNELS.EVENT_OPENCONNECT_STATUS, listener);
  },
  onOpenVpnStatus(listener: (data: OpenVpnStatusEvent) => void): () => void {
    return listen(IPC_CHANNELS.EVENT_OPENVPN_STATUS, listener);
  },
  submitOpenConnectForm(
    serverId: string,
    challengeId: string,
    values: Record<string, string>
  ): Promise<void> {
    return invoke(IPC_CHANNELS.OPENCONNECT_SUBMIT_AUTH_FORM, { serverId, challengeId, values });
  },
  submitOpenConnectBrowser(
    serverId: string,
    challengeId: string,
    finalUrl: string,
    cookies: OpenConnectBrowserCookieInput[],
    headers: OpenConnectBrowserHeaderInput[]
  ): Promise<void> {
    return invoke(IPC_CHANNELS.OPENCONNECT_SUBMIT_AUTH_BROWSER, {
      serverId,
      challengeId,
      finalUrl,
      cookies,
      headers,
    });
  },
  cancelOpenConnect(serverId: string, challengeId: string): Promise<void> {
    return invoke(IPC_CHANNELS.OPENCONNECT_CANCEL_AUTH, { serverId, challengeId });
  },
  submitOpenVpn(
    serverId: string,
    challengeId: string,
    username: string,
    password: string,
    secret: string
  ): Promise<void> {
    return invoke(IPC_CHANNELS.OPENVPN_SUBMIT_CHALLENGE, {
      serverId,
      challengeId,
      username,
      password,
      secret,
    });
  },
  cancelOpenVpn(serverId: string, challengeId: string): Promise<void> {
    return invoke(IPC_CHANNELS.OPENVPN_CANCEL_CHALLENGE, { serverId, challengeId });
  },
};
