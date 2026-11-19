import { useEffect, useRef, type Dispatch, type RefObject, type SetStateAction } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { api } from '@/ipc';
import { useAppStore } from '@/store/app-store';
import { dispatchSavedApply, useStagedConfigStore, type SavedRuntimeApply } from '@/store/staged-config-store';
import { toast } from '@/lib/error-handler';
import type { PendingApplyResult } from '@/contracts/types';
import type { ProxyErrorTranslate } from '@/domain/proxy-error-text';
import {
  applyReceiptState, hostEffectsAccepted, mergeNativeRetirements, nativeRetirementDebts, preserveNativeRetirements, runtimeApplyRejectedKey, runtimeApplyConflictKey, runtimePersistenceRecoveryEnded,
  type ApplyState, type RuntimeCoreRecovery,
} from './pending-bar-logic';
import { runtimeApplyPollDisposition, runtimeHostPollDisposition, runtimeNativePollDisposition, startRuntimeExecutionPolling } from './runtime-apply-polling';

interface Attempt {
  cancelled: boolean;
  awaitingReply?: boolean;
  intentGeneration: number | undefined;
  runtimeGeneration?: number;
  precedingSave?: PendingApplyResult | null;
}

function sameCore(state: ApplyState, owner: RuntimeCoreRecovery): boolean {
  const current = state.coreRecovery;
  return current ? current.transactionId === owner.transactionId
    && current.runtimeGeneration === owner.runtimeGeneration
    && current.selectionIntentGeneration === owner.selectionIntentGeneration
    : state.reconciling === true && state.transactionId === owner.transactionId
      && state.mainGeneration === owner.runtimeGeneration;
}

function ownsSaveRecovery(save: SavedRuntimeApply | null, owner: RuntimeCoreRecovery): save is SavedRuntimeApply {
  return save?.transactionId === owner.transactionId && save.runtimeGeneration === owner.runtimeGeneration
    && save.selectionIntentGeneration === owner.selectionIntentGeneration;
}

function showSavedChanges(save: SavedRuntimeApply, result: PendingApplyResult, apply: ApplyState, text: TFunction): void {
  const staged = useStagedConfigStore.getState();
  const { pendingChanges, proxyStatus } = useAppStore.getState();
  const intent = proxyStatus?.ownedStopContext?.selectionIntentGeneration;
  const changes = save.changes;
  if (!changes?.length || !result.ok || result.status !== 'hotApplied'
      || result.coreState !== 'committed' || result.configurationDurable !== true
      || staged.savedRuntimeApply || staged.saveStatus === 'saving'
      || save.saveRevision !== staged.completedSaveRevision || save.configVersion !== staged.baseVersion
      || (intent ?? -1) > (save.selectionIntentGeneration ?? -1)
      || result.nativeRetirements?.length || pendingChanges.nativeRetirements?.length || apply.nativeRetirements?.length
      || apply.host && apply.host.phase !== 'settled') return;
  if (changes.length === 1 && changes[0].logLevel) {
    toast.info(text('home.pendingSavedLogLevelApplied', { level: changes[0].logLevel }));
    return;
  }
  const labels = changes.slice(0, 3).map((change) => change.logLevel
    ? `${text('logs.currentLevel')} ${change.logLevel}` : change.label);
  const summary = labels.join(' · ');
  const remaining = changes.length - labels.length;
  toast.info(remaining > 0 ? text('home.pendingSavedMoreChangesApplied', { changes: summary, count: remaining })
    : text('home.pendingSavedChangesApplied', { changes: summary }));
}

/** Shared observers; a host id is never passed to the Core transaction status API. */
export function useRuntimeApplyReconciliation(
  apply: ApplyState,
  setApply: Dispatch<SetStateAction<ApplyState>>,
  attemptRef: RefObject<Attempt | null>,
  translate: ProxyErrorTranslate,
) {
  const status = useAppStore((state) => state.proxyStatus);
  const { t: text } = useTranslation();
  const savedRuntimeApply = useStagedConfigStore((state) => state.savedRuntimeApply);
  const pendingNative = useAppStore((state) => state.pendingChanges.nativeRetirements);
  const nativeEndedBeforeGeneration = useAppStore((state) => state.nativeRetirementEndedBeforeGeneration);
  const t = useRef(translate);
  t.current = translate;
  const latest = useRef(apply);
  latest.current = apply;
  const savedRecovery = useRef<SavedRuntimeApply | null>(null);
  const retrying = useRef(false);
  const checkingNative = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  // Rust's original-operation observer pushes delayed UNKNOWN even when there are no deleted nodes.
  // This also restores debt on a fresh desktop/mobile mount without manufacturing an Apply owner.
  useEffect(() => {
    setApply((state) => mergeNativeRetirements(state, pendingNative, nativeEndedBeforeGeneration));
  }, [pendingNative, nativeEndedBeforeGeneration, setApply]);

  useEffect(() => {
    if (!savedRuntimeApply || !status) return;
    const generation = savedRuntimeApply.runtimeGeneration;
    const observed = status.mainGeneration;
    const olderLifecycle = generation !== undefined && generation < (apply.mainGeneration ?? 0);
    if (!olderLifecycle && generation !== undefined && (observed === undefined || generation > observed
        || generation === observed && !status.running)) return;
    const attempt = attemptRef.current;
    if (attempt?.awaitingReply || apply.phase === 'applying') return;
    if (useStagedConfigStore.getState().savedRuntimeApply !== savedRuntimeApply) return;
    useStagedConfigStore.setState({ savedRuntimeApply: null });
    savedRecovery.current = null;
    const savedIntent = savedRuntimeApply.selectionIntentGeneration ?? -1;
    const newerIntent = ((attempt?.runtimeGeneration ?? apply.mainGeneration) === generation
        && ((attempt?.intentGeneration ?? -1) > savedIntent
          || savedIntent >= 0 && attempt?.intentGeneration === savedIntent && attempt.precedingSave === savedRuntimeApply))
      || (apply.coreRecovery?.runtimeGeneration === generation
        && (apply.coreRecovery?.selectionIntentGeneration ?? -1) > savedIntent);
    if (generation === undefined || generation !== observed || !status.running || olderLifecycle || newerIntent) {
      setApply((state) => mergeNativeRetirements(state, savedRuntimeApply.nativeRetirements));
      return;
    }
    // This Save has its own runtime owner. A prior explicit Apply's terminal or
    // cancelled attempt cannot decide this recovery.
    attemptRef.current = null;
    if ((savedRuntimeApply.status === 'hotDurabilityPending' || savedRuntimeApply.status === 'unconfirmed')
        && savedRuntimeApply.transactionId && savedRuntimeApply.changes?.length
        && (savedRuntimeApply.persistenceTransactionId ?? savedRuntimeApply.transactionId) === savedRuntimeApply.transactionId
        && Number.isSafeInteger(savedIntent) && savedIntent >= 0) {
      savedRecovery.current = savedRuntimeApply;
    }
    setApply((state) => {
      const initial = preserveNativeRetirements(state, { phase: 'idle', reason: null, mainGeneration: generation,
        ...(state.host && state.host.phase !== 'settled' ? { host: state.host, hostReconciling: state.hostReconciling } : {}) });
      const next = applyReceiptState(initial, savedRuntimeApply, true);
      if (savedRuntimeApply.status === 'unconfirmed' && savedRuntimeApply.transactionId
          && Number.isSafeInteger(savedRuntimeApply.selectionIntentGeneration)) {
        return { ...next, transactionId: savedRuntimeApply.transactionId, reconciling: true,
          coreRecovery: { transactionId: savedRuntimeApply.transactionId,
            runtimeGeneration: generation,
            selectionIntentGeneration: savedRuntimeApply.selectionIntentGeneration,
            persistenceRetryable: savedRuntimeApply.persistenceRetryable } };
      }
      return next;
    });
    showSavedChanges(savedRuntimeApply, savedRuntimeApply, apply, text);
  }, [apply, attemptRef, savedRuntimeApply, setApply, status, text]);
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
    const ended = disposition === 'failure' || result.status === 'hotApplied' && result.configurationDurable !== false
      || result.configurationDurable === true;
    const save = savedRecovery.current;
    if (ended && ownsSaveRecovery(save, owner)) {
      // A retry and a poll can complete together. Consume the original Save notice once.
      savedRecovery.current = null;
      if (disposition === 'owned' && !attemptRef.current && mounted.current && sameCore(latest.current, owner)) {
        showSavedChanges(save, result, latest.current, text);
      }
    }
    setApply((state) => {
      if (!sameCore(state, owner)) return state;
      if (disposition === 'failure') return { ...mergeNativeRetirements(state, result.nativeRetirements), phase: 'failed', reconciling: false,
        persistenceRecoveryEnded: state.persistenceOnly === true && runtimePersistenceRecoveryEnded(result.status, result.reasons),
        reason: t.current(result.status === 'conflict' ? runtimeApplyConflictKey(result.reasons)
          : result.status === 'rejected' ? runtimeApplyRejectedKey(result.reasons) : 'home.pendingHotUnconfirmed') };
      const observed = result.status === 'hotApplied' && result.configurationDurable !== false
        ? { ...result, configurationDurable: true }
        : result.status === 'hotDurabilityPending' ? { ...result, configurationDurable: false } : result;
      const bound = owner.selectionIntentGeneration === undefined && originalOwner && state.coreRecovery
        ? { ...state, coreRecovery: { ...state.coreRecovery, selectionIntentGeneration: result.selectionIntentGeneration } } : state;
      const next = applyReceiptState(bound, observed);
      if (observed.status === 'nativeCleanupUnknown' && !next.nativeRetirements?.length) {
        return { ...next, phase: 'failed', reason: t.current('home.pendingHotUnconfirmed'), reconciling: false };
      }
      return observed.configurationDurable === true
        ? { ...next, phase: 'idle', reason: null, reconciling: false } : next;
    });
    return ended;
  };

  const refreshCorePending = (owner: RuntimeCoreRecovery) => {
    const request = attemptRef.current;
    return useAppStore.getState().refreshPendingChanges(() => {
      const live = useAppStore.getState().proxyStatus;
      return attemptRef.current === request && request?.cancelled !== true && live?.running === true
        && live.mainGeneration === owner.runtimeGeneration && latest.current.transactionId === owner.transactionId;
    });
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
        if (ended && result.status === 'hotApplied') void refreshCorePending(owner);
        return ended;
      },
    });
  }, [apply.reconciling, core?.transactionId, core?.runtimeGeneration, core?.selectionIntentGeneration,
    status?.running, status?.mainGeneration, attemptRef, setApply, text]);

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
        const ended = ['hostEffectsFailed', 'conflict', 'rejected', 'skipped', 'cancelled'].includes(result.status) || (result.status === 'hotApplied' || result.status === 'hotDurabilityPending' || result.status === 'nativeCleanupUnknown')
          && hostEffectsAccepted(result.hostEffects);
        if (ended) {
          void useAppStore.getState().refreshPendingChanges(current);
        }
        return ended;
      },
    });
  }, [apply.hostReconciling, apply.host?.executionId, apply.host?.runtimeGeneration,
    apply.host?.selectionIntentGeneration, status?.running, status?.mainGeneration,
    status?.ownedStopContext?.selectionIntentGeneration, attemptRef, setApply]);

  return {
    hasSaveRecovery: !!core && ownsSaveRecovery(savedRecovery.current, core),
    checkNative: async () => {
      if (checkingNative.current) return;
      const owners = nativeRetirementDebts(latest.current.nativeRetirements);
      if (owners.length === 0) return;
      checkingNative.current = true;
      setApply((state) => ({ ...state, nativeChecking: true }));
      try {
        // One read per exact original transaction, independent of the current Host or persistence intent.
        await Promise.all(owners.map(async (owner) => {
          try {
            const result = await api.proxy.runtimeNativeRetirementStatus(owner.transactionId);
            if (!mounted.current || runtimeNativePollDisposition(result, owner) !== 'owned') return;
            setApply((state) => mergeNativeRetirements(state, result.nativeRetirements));
          } catch { /* An unavailable observation preserves the original UNKNOWN; no Apply or close retry. */ }
        }));
        await useAppStore.getState().refreshPendingChanges(() => mounted.current);
      } finally {
        checkingNative.current = false;
        if (mounted.current) setApply((state) => ({ ...state, nativeChecking: false }));
      }
    },
    retryPersistence: async () => {
      const owner = latest.current.coreRecovery ?? core;
      const request = attemptRef.current;
      const live = useAppStore.getState().proxyStatus;
      if (!owner || owner.selectionIntentGeneration === undefined || latest.current.coreRecovery?.persistenceRetryable === false
          || retrying.current || live?.running !== true || live.mainGeneration !== owner.runtimeGeneration) return;
      const saved = ownsSaveRecovery(savedRecovery.current, owner);
      retrying.current = true;
      setApply((state) => sameCore(state, owner) ? { ...state, persistenceRetrying: true } : state);
      try {
        const retry = () => api.proxy.retryRuntimePersistence(owner.transactionId);
        const current = () => {
          const status = useAppStore.getState().proxyStatus;
          return mounted.current && attemptRef.current === request && request?.cancelled !== true
            && status?.running === true && status.mainGeneration === owner.runtimeGeneration;
        };
        const result = saved ? await dispatchSavedApply(retry, () => current() && sameCore(latest.current, owner)) : await retry();
        if (result && current() && acceptCore(owner, result) && saved && result.status === 'hotApplied') {
          void refreshCorePending(owner);
        }
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
