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
    const load = () => {
      // React has not necessarily rendered a disabled button before the next click.
      if (!active || pending) return;
      pending = true;
      const revision = eventRevision;
      publish({ report: null, loading: true, failed: false });
      void api.server.autoSelectStatus().then((report) => {
        if (!report?.intent) throw new TypeError();
        if (active && revision === eventRevision) publish({ report, loading: false, failed: false });
      }).catch(() => {
        if (active && revision === eventRevision) publish({ report: null, loading: false, failed: true });
      }).finally(() => {
        pending = false;
        if (active && revision !== eventRevision) setState(previous => ({ ...previous, loading: false }));
      });
    };
    const off = api.server.onAutoSelectStatus((report) => {
      if (!active) return;
      eventRevision += 1;
      publish({ report, loading: pending, failed: false });
    });
    refreshRef.current = load;
    load();
    return () => {
      // Invalidate before detaching: a failed detach must not revive this session.
      active = false;
      refreshRef.current = null;
      try { off(); } catch (error) { console.error('[autoSelectStatus] unsubscribe failed:', error); }
    };
  }, [config, enabled]);

  return {
    ...(enabled && state.context === config ? state : { report: null, loading: enabled, failed: false }),
    enabled,
    refresh,
  };
}
