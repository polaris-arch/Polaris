import type { OpenConnectStatusEvent, OpenVpnStatusEvent, VpnStatusSnapshot } from '@/contracts/vpn-status';
import type { ProxyLifecycleEvent } from '@/contracts/types';

export interface VpnStatusPort {
  onOpenConnectStatusReady(listener: (event: OpenConnectStatusEvent) => void): Promise<() => void>;
  onOpenVpnStatusReady(listener: (event: OpenVpnStatusEvent) => void): Promise<() => void>;
  onLifecycleReady(listener: (event: ProxyLifecycleEvent) => void): Promise<() => void>;
  onStartedReady(listener: () => void): Promise<() => void>;
  onStoppedReady(listener: () => void): Promise<() => void>;
  getStatus(): Promise<VpnStatusSnapshot>;
}
export interface VpnStatusSink {
  snapshot(snapshot: VpnStatusSnapshot): void;
  openConnect(event: OpenConnectStatusEvent): void;
  openVpn(event: OpenVpnStatusEvent): void;
}
/** Same ready/revision/disposal pattern as privacy-mode-sync; the existing VPN store remains authoritative. */
export function startVpnStatusSync(port: VpnStatusPort, sink: VpnStatusSink) {
  let disposed = false;
  let generation = 0;
  let revision = 0;
  let request = 0;
  let ready = false;
  let running: boolean | undefined;
  let registrationRetry: ReturnType<typeof setTimeout> | null = null;
  let readRetry: ReturnType<typeof setTimeout> | null = null;
  const offs: Array<() => void> = [];
  const release = () => { for (const off of offs.splice(0)) off(); };
  const clear = () => sink.snapshot({ connected: false, openConnect: [], openVpn: [] });
  const clearReadRetry = () => {
    if (readRetry !== null) { clearTimeout(readRetry); readRetry = null; }
  };
  const canRead = () => ready && !disposed && running !== false;
  const refresh = async () => {
    if (!canRead()) return;
    clearReadRetry();
    const before = revision;
    const attempt = generation;
    const read = ++request;
    try {
      const snapshot = await port.getStatus();
      if (!disposed && ready && attempt === generation && read === request) {
        if (before === revision) sink.snapshot(snapshot);
        // An event vetoes the whole multi-endpoint snapshot. Re-read so unrelated
        // endpoints present only in the snapshot are not lost indefinitely.
        else void refresh();
      }
    } catch {
      // Revision vetoes old data, not recovery: an event for A cannot hide
      // snapshot-only B forever when this still-current read fails.
      if (canRead() && attempt === generation && read === request) {
        clearReadRetry();
        readRetry = setTimeout(() => { readRetry = null; void refresh(); }, 1000);
      }
    }
  };
  const boundary = (isRunning: boolean) => {
    if (disposed) return;
    running = isRunning;
    // Keep registration recovery alive. Until ready, boundaries must not spawn
    // another connect attempt or cancel the only pending registration retry.
    clearReadRetry();
    revision++;
    request++;
    clear();
    if (isRunning) void refresh();
  };
  const connect = () => {
    const attempt = ++generation;
    const active = () => !disposed && attempt === generation;
    ready = false;
    const register = async (operation: Promise<() => void>) => {
      const off = await operation;
      if (!active()) off(); else offs.push(off);
    };
    void Promise.all([
      register(port.onOpenConnectStatusReady(event => { if (active() && running !== false) { revision++; sink.openConnect(event); } })),
      register(port.onOpenVpnStatusReady(event => { if (active() && running !== false) { revision++; sink.openVpn(event); } })),
      register(port.onStartedReady(() => { if (active()) boundary(true); })),
      register(port.onStoppedReady(() => { if (active()) boundary(false); })),
      register(port.onLifecycleReady(event => { if (active()) boundary(event.phase === 'ready'); })),
    ]).then(() => {
      if (!active()) return;
      ready = true;
      void refresh();
    }).catch(() => {
      if (!active()) return;
      generation++;
      release();
      registrationRetry = setTimeout(() => { registrationRetry = null; if (!disposed) connect(); }, 1000);
    });
  };
  connect();
  return {
    boundary,
    dispose() {
      disposed = true; ready = false; generation++; request++;
      if (registrationRetry !== null) clearTimeout(registrationRetry);
      clearReadRetry();
      release();
    },
  };
}
