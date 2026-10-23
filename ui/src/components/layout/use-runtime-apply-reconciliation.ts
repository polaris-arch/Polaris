import { useEffect, useRef, type Dispatch, type RefObject, type SetStateAction } from 'react';
import { api } from '@/ipc';
import { normalizePendingChanges, useAppStore } from '@/store/app-store';
import type { ProxyErrorTranslate } from '@/domain/proxy-error-text';
import { runtimeApplyRejectedKey, runtimeApplyConflictKey, runtimePersistenceRecoveryEnded, type ApplyState } from './pending-bar-logic';
import { runtimeApplyPollDisposition, startRuntimeApplyPolling } from './runtime-apply-polling';

interface Attempt {
  cancelled: boolean;
  intentGeneration: number | undefined;
}

/** Shared mobile/desktop reconciliation, tied to the original request and identity. */
export function useRuntimeApplyReconciliation(
  apply: ApplyState,
  setApply: Dispatch<SetStateAction<ApplyState>>,
  attemptRef: RefObject<Attempt | null>,
  translate: ProxyErrorTranslate,
): void {
  const status = useAppStore((state) => state.proxyStatus);
  const t = useRef(translate);
  t.current = translate;
  useEffect(() => {
    const attempt = attemptRef.current;
    const transactionId = apply.transactionId;
    if (!apply.reconciling || !transactionId || !attempt) return;
    const current = () => {
      const status = useAppStore.getState().proxyStatus;
      const intent = status?.ownedStopContext?.selectionIntentGeneration;
      return attemptRef.current === attempt && !attempt.cancelled && status?.running === true
        && (apply.mainGeneration === undefined || status.mainGeneration === apply.mainGeneration)
        && (intent === undefined || intent === attempt.intentGeneration);
    };
    if (!current()) {
      setApply((state) => state.transactionId === transactionId ? { ...state, reconciling: false } : state);
      return;
    }
    return startRuntimeApplyPolling({ transactionId, current,
      read: (id) => api.proxy.runtimeApplyStatus(id),
      accept: (result) => {
        const disposition = runtimeApplyPollDisposition(result, transactionId,
          apply.mainGeneration, attempt.intentGeneration);
        if (disposition === 'failure') {
          setApply((state) => state.transactionId === transactionId
            ? { ...state, phase: 'failed', reconciling: false,
              persistenceRecoveryEnded: state.persistenceOnly === true && runtimePersistenceRecoveryEnded(result.status, result.reasons),
              reason: t.current(result.status === 'conflict' ? runtimeApplyConflictKey(result.reasons) : result.status === 'rejected' ? runtimeApplyRejectedKey(result.reasons) : 'home.pendingHotUnconfirmed') }
            : state);
          return true;
        }
        if (disposition !== 'owned') return false;
        if (result.status === 'hotApplied') {
          setApply((state) => state.transactionId === transactionId
            ? { phase: 'idle', reason: null, reconciling: false } : state);
          // Refresh the pending projection only while this exact attempt still owns it.
          void api.proxy.getPendingChanges().then((pending) => {
            if (current()) useAppStore.getState().setPendingChanges(normalizePendingChanges(pending));
          }).catch(() => undefined);
          return true;
        }
        if (result.status === 'hotDurabilityPending') {
          setApply((state) => state.transactionId === transactionId
            ? { ...state, phase: 'durabilityPending', reason: null } : state);
        }
        // Unconfirmed, wrong transaction, and failed reads never clear known runtime
        // application or mark persistence as complete. Keep checking this transaction.
        return false;
      },
    });
  }, [apply.reconciling, apply.transactionId, apply.mainGeneration, status?.running,
    status?.mainGeneration, status?.ownedStopContext?.selectionIntentGeneration, attemptRef, setApply]);
}
