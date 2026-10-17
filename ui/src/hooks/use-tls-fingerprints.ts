import { useCallback, useSyncExternalStore } from 'react';
import { coreApi } from '@/ipc/api/updater';
import { proxyApi } from '@/ipc/api/proxy';
import type { TlsFingerprintSnapshot } from '@/domain/utls';

// One live read model for both renderers and settings. It owns no persistent
// cache: last unmount clears it; start/stop/focus invalidate before querying.
let snapshot: TlsFingerprintSnapshot | undefined;
const listeners = new Set<() => void>();
let epoch = 0;
let inFlight = false;
let cleanup: (() => void) | undefined;
const publish = () => { for (const listener of listeners) listener(); };

function refresh() {
  ++epoch;
  snapshot = undefined;
  publish();
  if (!inFlight) query();
}
function query() {
  if (listeners.size === 0) return;
  const requested = epoch;
  inFlight = true;
  void coreApi.getTlsFingerprints().then(info => {
    if (requested === epoch && listeners.size > 0) {
      snapshot = info;
      publish();
    }
  }).catch((error: unknown) => {
    if (requested === epoch) {
      snapshot = { observationScope: 'unavailable', byteIdentityScope: 'unavailable', identity: null,
        binarySha256: null, document: null, unavailableReason: error instanceof Error ? error.message : String(error) };
      publish();
    }
  }).finally(() => {
    inFlight = false;
    if (requested !== epoch && listeners.size > 0) query();
  });
}
export function subscribeTlsFingerprints(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    const offStarted = proxyApi.onStarted(refresh);
    const offStopped = proxyApi.onStopped(refresh);
    window.addEventListener('focus', refresh);
    window.addEventListener('polaris-libbox-ready', refresh);
    cleanup = () => { offStarted(); offStopped(); window.removeEventListener('focus', refresh); window.removeEventListener('polaris-libbox-ready', refresh); };
    refresh();
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      cleanup?.(); cleanup = undefined;
      ++epoch; snapshot = undefined;
    }
  };
}
export const currentTlsFingerprints = () => snapshot;
const none = () => undefined;
export function useTlsFingerprints(active = true) {
  const subscribeActive = useCallback((listener: () => void) => active ? subscribeTlsFingerprints(listener) : () => {}, [active]);
  return useSyncExternalStore(subscribeActive, active ? currentTlsFingerprints : none, none);
}
