import { useEffect, useRef, type Dispatch, type RefObject, type SetStateAction } from 'react';
import { api } from '@/ipc';
import { normalizePendingChanges, useAppStore } from '@/store/app-store';
import type { PendingApplyResult } from '@/contracts/types';
import type { ProxyErrorTranslate } from '@/domain/proxy-error-text';
import {
  applyReceiptState, hostEffectsAccepted, runtimeApplyRejectedKey, runtimeApplyConflictKey, runtimePersistenceRecoveryEnded,
  type ApplyState, type RuntimeCoreRecovery,
} from './pending-bar-logic';
import { runtimeApplyPollDisposition, runtimeHostPollDisposition, startRuntimeExecutionPolling } from './runtime-apply-polling';

interface Attempt {
  cancelled: boolean;
  awaitingReply?: boolean;
  intentGeneration: number | undefined;
}

function sameCore(state: ApplyState, owner: RuntimeCoreRecovery): boolean {
  const current = state.coreRecovery;
  return current ? current.transactionId === owner.transactionId
    && current.runtimeGeneration === owner.runtimeGeneration
    && current.selectionIntentGeneration === owner.selectionIntentGeneration
    : state.reconciling === true && state.transactionId === owner.transactionId
      && state.mainGeneration === owner.runtimeGeneration;
}

/** Shared observers; a host id is never passed to the Core transaction status API. */
export function useRuntimeApplyReconciliation(
  apply: ApplyState,
  setApply: Dispatch<SetStateAction<ApplyState>>,
  attemptRef: RefObject<Attempt | null>,
  translate: ProxyErrorTranslate,
): { retryPersistence: () => Promise<void>; checkHost: () => void } {
  const status = useAppStore((state) => state.proxyStatus);
  const t = useRef(translate);
  t.current = translate;
  const latest = useRef(apply);
  latest.current = apply;
  const retrying = useRef(false);
  const core = apply.coreRecovery ?? (apply.reconciling && apply.transactionId
    && apply.mainGeneration !== undefined && attemptRef.current?.intentGeneration !== undefined
    ? { transactionId: apply.transactionId, runtimeGeneration: apply.mainGeneration,
      selectionIntentGeneration: attemptRef.current.intentGeneration } : undefined);

  const acceptCore = (owner: RuntimeCoreRecovery, result: PendingApplyResult): boolean => {
    const originalOwner = result.transactionId === owner.transactionId && result.runtimeGeneration === owner.runtimeGeneration
      && Number.isSafeInteger(result.selectionIntentGeneration) && (result.selectionIntentGeneration ?? -1) >= 0;
    const disposition = owner.selectionIntentGeneration === undefined && originalOwner ? 'owned'
        : runtimeApplyPollDisposition(result, owner.transactionId, owner.runtimeGeneration, owner.selectionIntentGeneration);
    if (disposition === 'ignore') return false;
    setApply((state) => {
      if (!sameCore(state, owner)) return state;
      if (disposition === 'failure') return { ...state, phase: 'failed', reconciling: false,
        persistenceRecoveryEnded: state.persistenceOnly === true && runtimePersistenceRecoveryEnded(result.status, result.reasons),
        reason: t.current(result.status === 'conflict' ? runtimeApplyConflictKey(result.reasons)
          : result.status === 'rejected' ? runtimeApplyRejectedKey(result.reasons) : 'home.pendingHotUnconfirmed') };
      const observed = result.status === 'hotApplied' && result.configurationDurable !== false
        ? { ...result, configurationDurable: true }
        : result.status === 'hotDurabilityPending' ? { ...result, configurationDurable: false } : result;
      const bound = owner.selectionIntentGeneration === undefined && originalOwner && state.coreRecovery
        ? { ...state, coreRecovery: { ...state.coreRecovery, selectionIntentGeneration: result.selectionIntentGeneration } } : state;
      const next = applyReceiptState(bound, observed);
      return observed.configurationDurable === true
        ? { ...next, phase: 'idle', reason: null, reconciling: false } : next;
    });
    return disposition === 'failure' || result.status === 'hotApplied' && result.configurationDurable !== false
      || result.configurationDurable === true;
  };

  useEffect(() => {
    if (!apply.reconciling || !core) return;
    const owner = core;
    const current = () => {
      const live = useAppStore.getState().proxyStatus;
      return sameCore(latest.current, owner) && attemptRef.current?.cancelled !== true && attemptRef.current?.awaitingReply !== true
        && live?.running === true && live.mainGeneration === owner.runtimeGeneration;
    };
    if (!current()) {
      setApply((state) => sameCore(state, owner) ? { ...state, reconciling: false } : state);
      return;
    }
    return startRuntimeExecutionPolling({
      execution: { kind: 'core', transactionId: owner.transactionId }, current,
      read: () => api.proxy.runtimeApplyStatus(owner.transactionId),
      accept: (result) => {
        const ended = acceptCore(owner, result);
        if (ended && result.status === 'hotApplied') {
          const request = attemptRef.current;
          void api.proxy.getPendingChanges().then((pending) => {
            const live = useAppStore.getState().proxyStatus;
            if (attemptRef.current === request && request?.cancelled !== true && live?.running === true
                && live.mainGeneration === owner.runtimeGeneration && latest.current.transactionId === owner.transactionId) {
              useAppStore.getState().setPendingChanges(normalizePendingChanges(pending));
            }
          }).catch(() => undefined);
        }
        return ended;
      },
    });
  }, [apply.reconciling, core?.transactionId, core?.runtimeGeneration, core?.selectionIntentGeneration,
    status?.running, status?.mainGeneration, attemptRef, setApply]);

  useEffect(() => {
    const host = apply.host;
    if (!host || host.phase === 'settled') return;
    const request = attemptRef.current;
    const same = (state: ApplyState) => state.host?.executionId === host.executionId
      && state.host.runtimeGeneration === host.runtimeGeneration
      && state.host.selectionIntentGeneration === host.selectionIntentGeneration;
    const current = () => {
      const live = useAppStore.getState().proxyStatus;
      const intent = live?.ownedStopContext?.selectionIntentGeneration;
      return same(latest.current) && attemptRef.current === request && request?.cancelled !== true
        && live?.running === true && live.mainGeneration === host.runtimeGeneration
        && (intent === undefined || intent === host.selectionIntentGeneration);
    };
    if (!current()) {
      setApply((state) => same(state) && !state.host?.superseded ? { ...state, hostReconciling: false,
        host: { ...host, phase: 'failed', superseded: true } } : state);
      return;
    }
    if (!apply.hostReconciling) return;
    return startRuntimeExecutionPolling({
      execution: { kind: 'host', hostExecutionId: host.executionId }, current,
      read: () => api.proxy.runtimeHostEffectStatus(host.executionId),
      accept: (result) => {
        if (runtimeHostPollDisposition(result, host.executionId, host.runtimeGeneration, host.selectionIntentGeneration) !== 'owned') return false;
        setApply((state) => same(state) ? applyReceiptState(state, result) : state);
        const ended = ['hostEffectsFailed', 'conflict', 'rejected', 'skipped', 'cancelled'].includes(result.status) || (result.status === 'hotApplied' || result.status === 'hotDurabilityPending')
          && hostEffectsAccepted(result.hostEffects);
        if (ended) {
          void api.proxy.getPendingChanges().then((pending) => {
            if (current()) useAppStore.getState().setPendingChanges(normalizePendingChanges(pending));
          }).catch(() => undefined);
        }
        return ended;
      },
    });
  }, [apply.hostReconciling, apply.host?.executionId, apply.host?.runtimeGeneration,
    apply.host?.selectionIntentGeneration, status?.running, status?.mainGeneration,
    status?.ownedStopContext?.selectionIntentGeneration, attemptRef, setApply]);

  return {
    retryPersistence: async () => {
      const owner = latest.current.coreRecovery ?? core;
      const request = attemptRef.current;
      const live = useAppStore.getState().proxyStatus;
      if (!owner || owner.selectionIntentGeneration === undefined || retrying.current || live?.running !== true || live.mainGeneration !== owner.runtimeGeneration) return;
      retrying.current = true;
      setApply((state) => sameCore(state, owner) ? { ...state, persistenceRetrying: true } : state);
      try {
        const result = await api.proxy.retryRuntimePersistence(owner.transactionId);
        const current = useAppStore.getState().proxyStatus;
        if (attemptRef.current === request && request?.cancelled !== true
            && current?.running === true && current.mainGeneration === owner.runtimeGeneration) acceptCore(owner, result);
      } catch { /* keep original Core recovery and host debt; no Apply replay */ }
      finally {
        retrying.current = false;
        setApply((state) => sameCore(state, owner) || state.transactionId === owner.transactionId
          ? { ...state, persistenceRetrying: false } : state);
      }
    },
    checkHost: () => {
      setApply((state) => state.host && state.host.phase !== 'settled' && !state.host.superseded
        ? { ...state, hostReconciling: true } : state);
    },
  };
}
