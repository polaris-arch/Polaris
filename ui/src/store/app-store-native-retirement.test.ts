import { beforeEach, expect, it } from 'vitest';
import type { NativeRetirementDebt, PendingNodeChanges } from '@/contracts/types';
import { normalizePendingChanges, useAppStore } from './app-store';
import { applyReceiptState, composeBarView, mergeNativeRetirements } from '@/components/layout/pending-bar-logic';

const empty: PendingNodeChanges = { added: [], modified: [], removed: [], restartDeferred: false };
const native: NativeRetirementDebt = { transactionId: 'native-A', runtimeGeneration: 7, selectionIntentGeneration: 9,
  retirementState: 'UNKNOWN', reason: { code: 'LISTENER_RETIREMENT_FAILED', path: 'inbounds' } };
beforeEach(() => useAppStore.setState({ pendingChanges: empty, nativeRetirementEndedBeforeGeneration: 0 }));
it('real pending pull/push and store updates cannot let a later durable HostOnly B hide native A', () => {
  const feed = (raw: PendingNodeChanges) => useAppStore.getState().setPendingChanges(normalizePendingChanges(raw));
  feed({ ...empty, nativeRetirements: [native] });
  let state = mergeNativeRetirements({ phase: 'idle', reason: null }, useAppStore.getState().pendingChanges.nativeRetirements);
  feed(empty);
  // A surface can unmount here. A fresh surface must still discover A from the real global store.
  state = mergeNativeRetirements({ phase: 'idle', reason: null }, useAppStore.getState().pendingChanges.nativeRetirements);
  expect(state.nativeRetirements?.map(debt => debt.transactionId)).toEqual(['native-A']);
  state = applyReceiptState(state, { status: 'hotApplied', hostExecutionId: 'host-B', runtimeGeneration: 7,
    selectionIntentGeneration: 10, configurationDurable: true, coreState: 'unchanged',
    hostEffects: [{ preview: { kind: 'dns', scopeDigest: 'dns', beforeDigest: 'old', targetDigest: 'new' },
      outcome: 'applied', receiptId: 'confirmed', cleanupPending: false, retryable: false, reasonCode: null }] }, true);
  feed({ ...empty, nativeRetirements: [native] });
  state = mergeNativeRetirements(state, useAppStore.getState().pendingChanges.nativeRetirements);
  const view = composeBarView({ stagedPhase: 'clean', pendingPhase: 'none', stagedCount: 0, pendingCount: 0,
    applyError: null, coreRunning: true, coreGeneration: 7, host: state.host, nativeRetirements: state.nativeRetirements });
  expect(view).toMatchObject({ visible: true, err: true, titleKey: 'home.pendingNativeCleanupUnknown' });
  expect(view.accountDetailKeys).toContain('home.pendingNativeHostSettled');
  expect(state.nativeRetirements?.map(debt => [debt.transactionId, debt.selectionIntentGeneration])).toEqual([['native-A', 9]]);
});

it('only verified process completion clears global debt, fences late pushes, and never clears successor B', () => {
  const store = () => useAppStore.getState();
  const feed = (debts: NativeRetirementDebt[]) => store().setPendingChanges(normalizePendingChanges({ ...empty, nativeRetirements: debts }));
  feed([native]);
  const stop = { kind: 'stop' as const, phase: 'stopped' as const, terminal: true, mainGeneration: 8 };
  store().confirmNativeRetirementEnd({ ...stop, terminal: false }, { running: false, mainGeneration: 8 });
  store().confirmNativeRetirementEnd({ ...stop, kind: 'restart', phase: 'failed' }, { running: true, mainGeneration: 9 });
  store().confirmNativeRetirementEnd(stop, { running: true, mainGeneration: 8 });
  expect(store().pendingChanges.nativeRetirements).toEqual([native]);
  store().confirmNativeRetirementEnd(stop, { running: false, mainGeneration: 8 });
  feed([native]);
  expect(store().pendingChanges.nativeRetirements).toEqual([]);
  const remounted = mergeNativeRetirements({ phase: 'idle', reason: null }, store().pendingChanges.nativeRetirements,
    store().nativeRetirementEndedBeforeGeneration);
  expect(remounted.nativeRetirements ?? []).toEqual([]);
  const successor = { ...native, transactionId: 'native-B', runtimeGeneration: 9, selectionIntentGeneration: 10 };
  feed([successor]);
  store().confirmNativeRetirementEnd(stop, { running: true, mainGeneration: 9 });
  feed([native]);
  expect(store().pendingChanges.nativeRetirements).toEqual([successor]);
});
