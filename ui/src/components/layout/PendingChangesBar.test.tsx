import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { PendingApplyResult } from '@/contracts/types';
import type { ApplyState } from './pending-bar-logic';

// Render the actual desktop handlers and shared observers in the existing node suite.
const h = vi.hoisted(() => ({
  states: [] as unknown[], refs: [] as Array<{ current: unknown }>,
  stateIndex: 0, refIndex: 0, effectIndex: 0,
  effects: [] as Array<{ deps?: readonly unknown[]; cleanup?: () => void }>,
  pendingEffects: [] as Array<() => void>,
  applyNow: vi.fn(), freshApply: vi.fn(), hostRead: vi.fn(), coreRead: vi.fn(),
  retryPersistence: vi.fn(), pendingRead: vi.fn(), cancelImpact: vi.fn(),
  lifecycleReady: vi.fn(), offLifecycle: vi.fn(),
  app: {
    pendingChanges: { added: ['node'], modified: [] as string[], removed: [] as string[], restartDeferred: false },
    proxyStatus: { running: true, mainGeneration: 7 }, config: null as Record<string, unknown> | null,
    servers: [] as Array<{ id: string; name: string }>, setPendingChanges: vi.fn(),
  },
}));
vi.mock('react', async original => ({
  ...await original<typeof import('react')>(),
  useState: (initial: unknown) => {
    const i = h.stateIndex++;
    if (!(i in h.states)) h.states[i] = typeof initial === 'function' ? initial() : initial;
    return [h.states[i], (next: unknown) => { h.states[i] = typeof next === 'function' ? next(h.states[i]) : next; }];
  },
  useRef: (initial: unknown) => h.refs[h.refIndex++] ??= { current: initial },
  useEffect: (effect: () => void | (() => void), deps?: readonly unknown[]) => {
    const i = h.effectIndex++, previous = h.effects[i];
    if (previous && deps && previous.deps && deps.length === previous.deps.length
      && deps.every((value, j) => Object.is(value, previous.deps![j]))) return;
    h.pendingEffects.push(() => {
      previous?.cleanup?.();
      const cleanup = effect();
      h.effects[i] = { deps: deps?.slice(), cleanup: cleanup || undefined };
    });
  },
}));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/lib/confirm-twice', () => ({ useConfirmTwice: () => ({ armed: null, confirmTwice: vi.fn() }) }));
vi.mock('@/components/dialogs/StagedConflictDialog', () => ({ StagedConflictDialog: () => null }));
vi.mock('@/components/dialogs/RestartApplyDialog', () => ({ RestartApplyDialog: () => null }));
vi.mock('@/components/dialogs/ImpactApplyDialog', () => ({ ImpactApplyDialog: () => null }));
vi.mock('@/ipc', () => ({ api: { proxy: {
  onLifecycleReady: h.lifecycleReady, applyPendingChanges: h.freshApply,
  runtimeHostEffectStatus: h.hostRead, runtimeApplyStatus: h.coreRead,
  retryRuntimePersistence: h.retryPersistence, getPendingChanges: h.pendingRead,
  cancelRuntimeImpact: h.cancelImpact, getStatus: vi.fn(),
} } }));
vi.mock('@/store/app-store', () => ({
  normalizePendingChanges: (pending: unknown) => pending,
  useAppStore: Object.assign((select: (state: typeof h.app) => unknown) => select(h.app), { getState: () => h.app }),
}));
vi.mock('@/store/staged-config-store', () => {
  const state = { entries: [], saveStatus: 'idle', conflict: null, applyNow: h.applyNow };
  return { useStagedConfigStore: Object.assign((select: (value: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('@/lib/error-handler', () => ({ toast: { info: vi.fn(), error: vi.fn(), warning: vi.fn(), success: vi.fn() } }));

import PendingChangesBar from './PendingChangesBar';
import { ImpactApplyDialog } from '@/components/dialogs/ImpactApplyDialog';
import { toast } from '@/lib/error-handler';

type Props = { children?: ReactNode; onClick?: () => void; disabled?: boolean; onConfirm?: () => void };
function elements(node: ReactNode): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}
function text(node: ReactNode): string {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  return isValidElement<Props>(node) ? text(node.props.children) : '';
}
function renderBar(): ReactElement | null {
  h.stateIndex = h.refIndex = h.effectIndex = 0;
  const tree = PendingChangesBar();
  h.pendingEffects.splice(0).forEach(run => run());
  return tree;
}
function button(label: string): ReactElement<Props> {
  const found = elements(renderBar()).find(e => e.type === 'button' && text(e.props.children) === label);
  expect(found, label).toBeDefined();
  return found!;
}
function click(label: string): void {
  const found = button(label);
  expect(found.props.disabled).not.toBe(true);
  found.props.onClick!();
}
function markup(): string { return renderToStaticMarkup(renderBar()); }
function unmount(): void {
  h.effects.forEach(effect => effect.cleanup?.());
  h.effects = []; h.pendingEffects = [];
}
async function microtasks(): Promise<void> { for (let i = 0; i < 10; i++) await Promise.resolve(); }

const effect = { preview: { kind: 'systemProxy' as const, scopeDigest: 'scope', beforeDigest: 'before', targetDigest: 'target' },
  outcome: 'applied' as const, receiptId: 'physical-receipt', cleanupPending: false, retryable: false, reasonCode: null };
function owned(status: PendingApplyResult['status']): PendingApplyResult {
  return { ok: true, status, hostExecutionId: 'host-only', runtimeGeneration: 7,
    selectionIntentGeneration: 9, coreState: 'unchanged', configurationDurable: true };
}
async function begin(result: PendingApplyResult): Promise<void> {
  h.applyNow.mockResolvedValueOnce({ ...result, saved: true });
  click('home.pendingApplyBtn');
  await vi.waitFor(() => expect((h.states[0] as ApplyState).host?.executionId).toBe('host-only'));
  h.app.pendingChanges = { added: [], modified: [], removed: [], restartDeferred: false };
  vi.useFakeTimers();
  renderBar();
}

beforeEach(() => {
  unmount(); vi.resetAllMocks(); h.states = []; h.refs = [];
  h.app.config = null;
  h.app.pendingChanges = { added: ['node'], modified: [], removed: [], restartDeferred: false };
  h.app.proxyStatus = { running: true, mainGeneration: 7 };
  h.app.setPendingChanges.mockImplementation(pending => { h.app.pendingChanges = pending; });
  h.pendingRead.mockImplementation(async () => h.app.pendingChanges);
  h.lifecycleReady.mockResolvedValue(h.offLifecycle);
  h.cancelImpact.mockResolvedValue({ ok: true, status: 'cancelled' });
  h.coreRead.mockRejectedValue(new Error('Core status unavailable'));
  h.hostRead.mockRejectedValue(new Error('Host status unavailable'));
});
afterEach(() => { unmount(); if (vi.isFakeTimers()) vi.clearAllTimers(); vi.useRealTimers(); });

describe('desktop host execution and four-account feedback', () => {
  it('polls HostOnly without a transaction and retains the bar at D=R until a physical receipt', async () => {
    await begin(owned('hostEffectsPending'));
    expect((h.states[0] as ApplyState).transactionId).toBeUndefined();
    expect(markup()).toContain('home.pendingHostEffectsPending');
    h.hostRead.mockResolvedValueOnce({ ...owned('hotApplied'), hostEffects: [effect] });
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.hostRead).toHaveBeenCalledExactlyOnceWith('host-only');
    expect(renderBar()).toBeNull();
    expect(h.coreRead).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });

  it('requests a fresh Plan once and waits for its new confirmation rather than replaying Core Apply', async () => {
    await begin({ ...owned('hostEffectsFailed'),
      hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    h.freshApply.mockResolvedValueOnce({ ok: false, status: 'impactConfirmationRequired', runtimeGeneration: 7,
      impactConfirmation: 'new-plan', coreImpacts: [], hostEffectPreviews: [effect.preview] });
    const review = button('home.pendingRetryHostBtn');
    review.props.onClick!(); review.props.onClick!();
    await microtasks();
    expect(h.freshApply).toHaveBeenCalledExactlyOnceWith();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.retryPersistence).not.toHaveBeenCalled();
    const dialog = elements(renderBar()).find(e => e.type === ImpactApplyDialog);
    expect(dialog).toBeDefined();
    expect(h.states[2]).toMatchObject({ token: 'new-plan', hostEffects: [effect.preview] });
    h.applyNow.mockResolvedValueOnce({ saved: true, ...owned('hostEffectsPending'), hostExecutionId: 'host-next' });
    dialog!.props.onConfirm!();
    await microtasks();
    expect(h.applyNow).toHaveBeenNthCalledWith(2, undefined, 'new-plan');
    expect((h.states[0] as ApplyState).host).toMatchObject({ executionId: 'host-next', phase: 'pending' });
  });

  it('observes Core durability and host pending independently, without regressing a completed persistence receipt', async () => {
    await begin({ ...owned('hostEffectsPending'), transactionId: 'original-core',
      configurationDurable: false, coreState: 'committed' });
    h.coreRead.mockResolvedValueOnce({ ok: true, status: 'hotApplied', transactionId: 'original-core',
      runtimeGeneration: 7, selectionIntentGeneration: 9, configurationDurable: true });
    h.hostRead.mockResolvedValueOnce({ ...owned('hostEffectsPending'), transactionId: 'original-core',
      configurationDurable: false, coreState: 'committed' });
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.coreRead).toHaveBeenCalledExactlyOnceWith('original-core');
    expect(h.hostRead).toHaveBeenCalledExactlyOnceWith('host-only');
    const state = h.states[0] as ApplyState;
    expect(state.configurationDurable).toBe(true);
    expect(state.coreRecovery).toBeUndefined();
    expect(state.host?.phase).toBe('pending');
    expect(markup()).toContain('home.pendingHostCoreCommitted');
    expect(markup()).not.toContain('home.pendingRetryPersistenceBtn');
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });

  it('does not accept an empty host result merely because the Core status says hotApplied', async () => {
    await begin(owned('hostEffectsPending'));
    h.hostRead.mockResolvedValueOnce({ ...owned('hotApplied'), hostEffects: [] });
    await vi.advanceTimersByTimeAsync(2000);
    expect((h.states[0] as ApplyState).host?.phase).toBe('pending');
    expect(markup()).toContain('home.pendingHostEffectsPending');
    expect(toast.info).not.toHaveBeenCalled();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });

  it('retains both debts when an explicit original-transaction persistence retry fails', async () => {
    await begin({ ...owned('hostEffectsFailed'), transactionId: 'original-core',
      configurationDurable: false, coreState: 'committed',
      hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    h.retryPersistence.mockRejectedValueOnce(new Error('disk unavailable'));
    click('home.pendingRetryPersistenceBtn');
    await microtasks();
    const state = h.states[0] as ApplyState;
    expect(state.coreRecovery?.transactionId).toBe('original-core');
    expect(state.configurationDurable).toBe(false);
    expect(state.host).toMatchObject({ executionId: 'host-only', phase: 'failed' });
    expect(markup()).toContain('home.pendingHostAndDurabilityPending');
    expect(h.retryPersistence).toHaveBeenCalledExactlyOnceWith('original-core');
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.freshApply).not.toHaveBeenCalled();
  });

  it('offers only a host status read while cleanup is unknown, without any automatic retry', async () => {
    const unknown = { ...effect, outcome: 'unknown' as const, cleanupPending: true, retryable: false };
    await begin({ ...owned('hostEffectsFailed'), hostEffects: [unknown] });
    expect(elements(renderBar()).some(e => e.type === 'button' && text(e.props.children) === 'home.pendingRetryHostBtn')).toBe(false);
    h.hostRead.mockResolvedValueOnce({ ...owned('hostEffectsFailed'), hostEffects: [unknown] });
    click('home.pendingCheckHostBtn');
    renderBar();
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.hostRead).toHaveBeenCalledExactlyOnceWith('host-only');
    expect((h.states[0] as ApplyState).host?.phase).toBe('failed');
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });
});


describe('desktop new HostOnly attempt retains original Core recovery', () => {
  it('keeps the original transaction leg after the same-intent HostOnly retry completes', async () => {
    await begin({ ...owned('hostEffectsFailed'), transactionId: 'original-core',
      configurationDurable: false, coreState: 'committed',
      hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    h.freshApply.mockResolvedValueOnce({ ...owned('hostEffectsPending'), hostExecutionId: 'host-next',
      selectionIntentGeneration: 9, configurationDurable: false });
    click('home.pendingRetryHostBtn');
    await microtasks();
    renderBar();
    expect((h.states[0] as ApplyState).coreRecovery).toEqual({ transactionId: 'original-core',
      runtimeGeneration: 7, selectionIntentGeneration: 9 });
    h.hostRead.mockResolvedValueOnce({ ...owned('hotApplied'), hostExecutionId: 'host-next',
      selectionIntentGeneration: 9, hostEffects: [effect], configurationDurable: true });
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.hostRead).toHaveBeenCalledExactlyOnceWith('host-next');
    const state = h.states[0] as ApplyState;
    expect(state.host?.phase).toBe('settled');
    expect(state.configurationDurable).toBe(false);
    expect(state.coreRecovery?.transactionId).toBe('original-core');
    expect(state.phase).toBe('durabilityPending');
    expect(markup()).toContain('home.pendingHotDurabilityPending');
    expect(markup()).toContain('home.pendingRetryPersistenceBtn');
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });

  it('exits only a source-confirmed obsolete Core recovery and leaves failed host debt visible', async () => {
    await begin({ ...owned('hostEffectsFailed'), transactionId: 'original-core',
      configurationDurable: false, coreState: 'committed',
      hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    h.coreRead.mockResolvedValueOnce({ ok: false, status: 'conflict', transactionId: 'original-core',
      reasons: [{ code: 'OWNER_STATE_SUPERSEDED', path: 'desired' }] });
    await vi.advanceTimersByTimeAsync(2000);
    expect((h.states[0] as ApplyState).persistenceRecoveryEnded).toBe(true);
    click('home.pendingExitPersistenceRecoveryBtn');
    const state = h.states[0] as ApplyState;
    expect(state.coreRecovery).toBeUndefined();
    expect(state.host).toMatchObject({ executionId: 'host-only', phase: 'failed' });
    expect(markup()).toContain('home.pendingHostCoreCommittedFailed');
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });
});


describe('host review owns its saved candidate and explicit Stop boundary', () => {
  it('cancels a fresh impact offer when the saved candidate changes during the request', async () => {
    await begin({ ...owned('hostEffectsFailed'),
      hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    let finish!: (result: PendingApplyResult) => void;
    h.freshApply.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    click('home.pendingRetryHostBtn');
    await microtasks();
    expect(h.freshApply).toHaveBeenCalledTimes(1);
    h.app.config = { savedVersion: 'changed-during-plan' };
    finish({ ok: false, status: 'impactConfirmationRequired', impactConfirmation: 'obsolete-offer',
      runtimeGeneration: 7, coreImpacts: [], hostEffectPreviews: [effect.preview] });
    await microtasks();
    expect(h.cancelImpact).toHaveBeenCalledExactlyOnceWith('obsolete-offer');
    expect(h.states[2]).toBeNull();
    expect((h.states[0] as ApplyState).host?.phase).not.toBe('settled');
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });

  it('does not issue the fresh Plan request after an attributed Stop already cancelled the caller', async () => {
    await begin({ ...owned('hostEffectsFailed'),
      hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    click('home.pendingRetryHostBtn');
    const listener = h.lifecycleReady.mock.calls[0][0] as (event: unknown) => void;
    listener({ kind: 'stop', phase: 'stopped', terminal: true, mainGeneration: 8 });
    await microtasks();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect((h.states[0] as ApplyState).host?.phase).not.toBe('settled');
  });
});


describe('desktop HostOnly projection preserves the old persistence account', () => {
  it('finishes host observation and then retries only the separately observed original Core transaction', async () => {
    await begin({ ...owned('hostEffectsPending'), persistenceTransactionId: 'original-core', configurationDurable: false });
    expect((h.states[0] as ApplyState).transactionId).toBeUndefined();
    expect((h.states[0] as ApplyState).coreRecovery?.selectionIntentGeneration).toBeUndefined();
    expect(button('home.pendingRetryPersistenceBtn').props.disabled).toBe(true);
    h.coreRead.mockResolvedValueOnce({ ok: true, status: 'hotDurabilityPending', transactionId: 'original-core',
      runtimeGeneration: 7, selectionIntentGeneration: 9, configurationDurable: false });
    h.hostRead.mockResolvedValueOnce({ ...owned('hotApplied'), persistenceTransactionId: 'original-core',
      configurationDurable: false, hostEffects: [effect] });
    await vi.advanceTimersByTimeAsync(2000);
    renderBar();
    expect(h.coreRead).toHaveBeenCalledExactlyOnceWith('original-core');
    expect(h.hostRead).toHaveBeenCalledExactlyOnceWith('host-only');
    expect((h.states[0] as ApplyState).host?.phase).toBe('settled');
    expect((h.states[0] as ApplyState).coreRecovery?.selectionIntentGeneration).toBe(9);
    expect(markup()).toContain('home.pendingHotDurabilityPending');
    h.retryPersistence.mockResolvedValueOnce({ ok: true, status: 'hotApplied', transactionId: 'original-core',
      runtimeGeneration: 7, selectionIntentGeneration: 9, configurationDurable: true });
    click('home.pendingRetryPersistenceBtn');
    await microtasks();
    expect(h.retryPersistence).toHaveBeenCalledExactlyOnceWith('original-core');
    expect(renderBar()).toBeNull();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });
});
