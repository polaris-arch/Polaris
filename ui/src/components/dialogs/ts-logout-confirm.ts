import type { TFunction } from 'i18next';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { useAppStore } from '@/store/app-store';
import { useDialogStore } from './dialog-store';
import { captureMainCoreOwner, stopOwnedCoreThenLogout } from './ts-logout-flow';

/**
 * Desktop: the backend refused logout because the running proxy holds this node. Ask once more,
 * then stop exactly the captured run and log out. The proxy is left stopped.
 */
export async function confirmStopThenTsLogout(
  serverId: string,
  t: TFunction,
  onLoggedOut: () => void,
): Promise<void> {
  const owner = captureMainCoreOwner(serverId, useAppStore.getState().selectedServerId,
    await api.proxy.getStatus().catch(() => null));
  if (!owner) {
    toast.info(t('ts.logoutStopChanged'));
    return;
  }
  const { open, close } = useDialogStore.getState();
  open({
    kind: 'confirm',
    payload: {
      title: t('ts.logoutStopTitle'),
      message: t('ts.logoutStopMessage'),
      confirmLabel: t('ts.logoutStopConfirm'),
      danger: true,
      onConfirm: async () => {
        close();
        const result = await stopOwnedCoreThenLogout(owner, {
          selectedId: () => useAppStore.getState().selectedServerId,
          serverPresent: (id) => useAppStore.getState().servers.some((s) => s.id === id),
          status: () => api.proxy.getStatus(),
          stop: () => useAppStore.getState().stopProxy(),
          logout: async (id) => { await api.server.tailscaleLogout(id); },
        });
        if (result.kind === 'loggedOut') onLoggedOut();
        else if (result.kind === 'changed') toast.info(t('ts.logoutStopChanged'));
        else if (result.kind === 'stopFailed') toast.error(t('ts.logoutStopFailed'));
        else toast.error(t(result.code === 'TAILSCALE_LOGOUT_MAIN_CORE'
          ? 'ts.reasonMainCoreInUse' : 'nodes.meshTsLogoutFail'));
      },
    },
  });
}
