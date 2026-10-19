import { api } from '@/ipc';
import { useAppStore } from '@/store/app-store';
import { useVpnStatusStore } from '@/store/use-vpn-status-store';
import { startVpnStatusSync } from './vpn-status-sync';

/** One lifecycle owner in either root; callers provide only their presentation route. */
export function startAppVpnStatusSync(open: (protocol: 'openconnect' | 'openvpn', serverId: string) => void): () => void {
  const sync = startVpnStatusSync({ ...api.vpn, onLifecycleReady: api.proxy.onLifecycleReady, onStartedReady: api.proxy.onStartedReady, onStoppedReady: api.proxy.onStoppedReady }, {
    snapshot(snapshot) {
      useVpnStatusStore.getState().replace(snapshot.connected, snapshot.openConnect ?? [], snapshot.openVpn ?? []);
      for (const status of snapshot.openConnect ?? []) if (status.authChallenge) open('openconnect', status.serverId);
      for (const status of snapshot.openVpn ?? []) if (status.challenge) open('openvpn', status.serverId);
    },
    openConnect(status) {
      useVpnStatusStore.getState().setOpenConnect(status);
      if (status.authChallenge) open('openconnect', status.serverId);
    },
    openVpn(status) {
      useVpnStatusStore.getState().setOpenVpn(status);
      if (status.challenge) open('openvpn', status.serverId);
    },
  });
  // Poll/read-based convergence also invalidates in-flight reads after crashes or missed lifecycle events.
  const off = useAppStore.subscribe((state, previous) => {
    const current = state.proxyStatus;
    const old = previous.proxyStatus;
    if (current && (current.running !== old?.running || current.mainGeneration !== old?.mainGeneration)) sync.boundary(current.running);
  });
  return () => { off(); sync.dispose(); };
}
