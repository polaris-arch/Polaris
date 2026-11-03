import type { NativeRetirementDebt, ProxyLifecycleEvent } from '@/contracts/types';

/** Only exact original UNKNOWN plus the fixed listening-ingress failure is public cleanup debt. */
export function nativeRetirementDebts(raw: NativeRetirementDebt[] | undefined): NativeRetirementDebt[] {
  return Array.isArray(raw) ? raw.filter(debt => debt && typeof debt.transactionId === 'string' && debt.transactionId.length > 0
    && Number.isSafeInteger(debt.runtimeGeneration) && debt.runtimeGeneration > 0
    && Number.isSafeInteger(debt.selectionIntentGeneration) && debt.selectionIntentGeneration >= 0
    && debt.retirementState === 'UNKNOWN' && debt.reason?.code === 'LISTENER_RETIREMENT_FAILED'
    && debt.reason.path === 'inbounds') : [];
}

/** Empty or missing receipts cannot settle UNKNOWN; only a separately verified process-end floor can. */
export function mergeNativeRetirementDebts(
  current: NativeRetirementDebt[] | undefined, raw: NativeRetirementDebt[] | undefined, endedBeforeGeneration = 0,
): NativeRetirementDebt[] | undefined {
  const retained = nativeRetirementDebts(current).filter(debt => debt.runtimeGeneration >= endedBeforeGeneration);
  const debts = [...retained];
  for (const debt of nativeRetirementDebts(raw)) {
    if (debt.runtimeGeneration < endedBeforeGeneration) continue;
    if (!debts.some(known => known.transactionId === debt.transactionId && known.runtimeGeneration === debt.runtimeGeneration
      && known.selectionIntentGeneration === debt.selectionIntentGeneration)) debts.push(debt);
  }
  if (debts.length === (current?.length ?? 0) && debts.every((debt, index) => debt === current?.[index])) return current;
  return debts;
}

/** Both generations come from LifecycleGate, which advances BEFORE teardown.
 * Require terminal completion plus its matching fresh status; a larger number alone proves nothing.
 */
export function nativeRetirementEndGeneration(
  event: ProxyLifecycleEvent, live: { running: boolean; mainGeneration?: number } | null | undefined,
): number | undefined {
  const stopped = event.phase === 'stopped' && event.kind === 'stop';
  const ready = event.phase === 'ready' && (event.kind === 'start' || event.kind === 'restart');
  return event.terminal === true && (stopped || ready) && Number.isSafeInteger(event.mainGeneration)
    && event.mainGeneration > 0 && live?.mainGeneration === event.mainGeneration && live.running === ready
    ? event.mainGeneration : undefined;
}
