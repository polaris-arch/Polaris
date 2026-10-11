import { useCallback, useEffect, useRef, useState } from 'react';
import type { AutoSelectStatus } from '@/contracts/auto-select';
import type { UserConfig } from '@/contracts/types';
import { api } from '@/ipc';

export interface AutoSelectStatusState {
  report: AutoSelectStatus | null;
  loading: boolean;
  failed: boolean;
}

/** Read-only, scoped to the saved configuration. Events supersede in-flight snapshots. */
export function useAutoSelectStatus(config: UserConfig | null) {
  const enabled = !!config && Object.prototype.hasOwnProperty.call(config, 'selectionIntent');
  const [state, setState] = useState<AutoSelectStatusState & { context: UserConfig | null }>({
    context: null, report: null, loading: false, failed: false,
  });
  const refreshRef = useRef<(() => void) | null>(null);
  const refresh = useCallback(() => refreshRef.current?.(), []);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    let pending = false;
    let eventRevision = 0;
    const publish = (next: AutoSelectStatusState) => {
      if (active) setState({ ...next, context: config });
    };
    let off: (() => void) | null = null;
    let registrationAttempt = 0;
    const detach = (unsubscribe: () => void) => {
      const failed = (error: unknown) => console.error('[autoSelectStatus] unsubscribe failed:', error);
      try { void Promise.resolve(unsubscribe()).catch(failed); } catch (error) { failed(error); }
    };
    const load = async () => {
      // Guard both registration and the snapshot before React disables the button.
      if (!active || pending) return;
      pending = true;
      publish({ report: null, loading: true, failed: false });
      try {
        if (!off) {
          const attempt = ++registrationAttempt;
          let unsubscribe: () => void;
          try {
            unsubscribe = await api.server.onAutoSelectStatusReady((report) => {
              if (!active || attempt !== registrationAttempt) return;
              eventRevision += 1;
              publish({ report, loading: pending, failed: false });
            });
          } catch {
            // A failed registration may still deliver a queued callback.
            registrationAttempt += 1;
            publish({ report: null, loading: false, failed: true });
            return;
          }
          if (!active) {
            detach(unsubscribe);
            return;
          }
          off = unsubscribe;
        }
        // Read only after registration, closing the initial event delivery gap.
        const revision = eventRevision;
        try {
          const report = await api.server.autoSelectStatus();
          if (!report?.intent) throw new TypeError();
          if (active && revision === eventRevision) publish({ report, loading: false, failed: false });
        } catch {
          if (active && revision === eventRevision) publish({ report: null, loading: false, failed: true });
        }
      } finally {
        pending = false;
        if (active) setState(previous => ({ ...previous, loading: false }));
      }
    };
    refreshRef.current = () => { void load(); };
    void load();
    return () => {
      // Invalidate before detaching: a failed detach must not revive this session.
      active = false;
      registrationAttempt += 1;
      refreshRef.current = null;
      if (off) detach(off);
    };
  }, [config, enabled]);

  return {
    ...(enabled && state.context === config ? state : { report: null, loading: enabled, failed: false }),
    enabled,
    refresh,
  };
}
