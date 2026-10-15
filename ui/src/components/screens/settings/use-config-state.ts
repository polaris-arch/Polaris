import type { UserConfig } from '@/contracts/types';

export interface DirectOperation {
  id: number;
  patch: Partial<UserConfig>;
  phase: 'pending' | 'committed';
  barrier: number;
}
export interface ConfigEditState {
  baseline: UserConfig | null;
  serial: number;
  operations: readonly DirectOperation[];
}

export function configEditState(baseline: UserConfig | null): ConfigEditState {
  return { baseline, serial: 0, operations: [] };
}

/** Local intent overlays never become a global or disk-save baseline. */
export function displayConfig(state: ConfigEditState): UserConfig | null {
  if (!state.baseline || !state.operations.length) return state.baseline;
  return Object.assign({}, state.baseline, ...state.operations.map((operation) => operation.patch));
}

export function beginDirect(state: ConfigEditState, id: number, patch: Partial<UserConfig>): ConfigEditState {
  return { ...state, operations: [...state.operations, { id, patch, phase: 'pending', barrier: Infinity }] };
}

export function failDirect(state: ConfigEditState, id: number): ConfigEditState {
  return { ...state, operations: state.operations.filter((operation) => operation.id !== id) };
}

export function commitDirect(state: ConfigEditState, id: number, saved: UserConfig, barrier: number): ConfigEditState {
  const operations = state.operations.map((operation): DirectOperation => {
    if (operation.id !== id) return operation;
    const patch: Partial<UserConfig> = {};
    for (const key of Object.keys(operation.patch) as (keyof UserConfig)[]) {
      Object.assign(patch, { [key]: saved[key] });
    }
    return { ...operation, patch, phase: 'committed', barrier };
  });
  // At most one committed value per top-level key, without reordering pending
  // operations. A long read outage must not accumulate per-keystroke history.
  const newest = new Map<string, { id: number; barrier: number }>();
  for (const operation of operations) {
    if (operation.phase !== 'committed') continue;
    for (const key of Object.keys(operation.patch)) {
      const previous = newest.get(key);
      newest.set(key, { id: Math.max(previous?.id ?? -1, operation.id), barrier: Math.max(previous?.barrier ?? -1, operation.barrier) });
    }
  }
  return { ...state, operations: operations.flatMap((operation) => {
    if (operation.phase === 'pending') return [operation];
    const keys = Object.keys(operation.patch).filter((key) => newest.get(key)?.id === operation.id);
    if (!keys.length) return [];
    return [{ ...operation, patch: Object.fromEntries(keys.map((key) => [key, operation.patch[key as keyof UserConfig]])),
      barrier: Math.max(...keys.map((key) => newest.get(key)!.barrier)) }];
  }) };
}

export function acceptConfig(state: ConfigEditState, baseline: UserConfig, serial: number, epoch: number): ConfigEditState {
  if (serial <= state.serial) return state;
  return { baseline, serial, operations: state.operations.filter((operation) =>
    operation.phase === 'pending' || operation.barrier > epoch) };
}

export function directKeys(state: ConfigEditState): Set<string> {
  return new Set(state.operations.flatMap((operation) => Object.keys(operation.patch)));
}
