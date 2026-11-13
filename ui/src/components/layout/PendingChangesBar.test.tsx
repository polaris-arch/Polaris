import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { NativeRetirementDebt, PendingApplyResult, PendingNodeChanges } from '@/contracts/types';
import type { ApplyState } from './pending-bar-logic';

// Render the actual desktop handlers and shared observers in the existing node suite.
const h = vi.hoisted(() => ({
  states: [] as unknown[], refs: [] as Array<{ current: unknown }>,
  stateIndex: 0, refIndex: 0, effectIndex: 0,
  effects: [] as Array<{ deps?: readonly unknown[]; cleanup?: () => void }>,
  pendingEffects: [] as Array<() => void>,
  applyNow: vi.fn(), freshApply: vi.fn(), hostRead: vi.fn(), coreRead: vi.fn(), nativeRead: vi.fn(),
  retryPersistence: vi.fn(), pendingRead: vi.fn(), cancelImpact: vi.fn(),
  lifecycleReady: vi.fn(), offLifecycle: vi.fn(), statusRead: vi.fn(),
  app: {
    pendingChanges: { added: ['node'], modified: [] as string[], removed: [] as string[], restartDeferred: false } as PendingNodeChanges,
    proxyStatus: { running: true, mainGeneration: 7 }, config: null as Record<string, unknown> | null,
    nativeRetirementEndedBeforeGeneration: 0, confirmNativeRetirementEnd: vi.fn(),
    servers: [] as Array<{ id: string; name: string }>, setPendingChanges: vi.fn(),
    applyRequested: false, setApplyRequested: vi.fn(),
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
  runtimeNativeRetirementStatus: h.nativeRead,
  retryRuntimePersistence: h.retryPersistence, getPendingChanges: h.pendingRead,
  cancelRuntimeImpact: h.cancelImpact, getStatus: h.statusRead,
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
  h.app.nativeRetirementEndedBeforeGeneration = 0;
  h.app.pendingChanges = { added: ['node'], modified: [], removed: [], restartDeferred: false };
  h.app.proxyStatus = { running: true, mainGeneration: 7 };
  h.app.applyRequested = false;
  h.app.setPendingChanges.mockImplementation(pending => { h.app.pendingChanges = pending; });
  h.pendingRead.mockImplementation(async () => h.app.pendingChanges);
  h.lifecycleReady.mockResolvedValue(h.offLifecycle);
  h.statusRead.mockImplementation(async () => h.app.proxyStatus);
  h.cancelImpact.mockResolvedValue({ ok: true, status: 'cancelled' });
  h.coreRead.mockRejectedValue(new Error('Core status unavailable'));
  h.nativeRead.mockRejectedValue(new Error('Native-only status unavailable'));
  h.hostRead.mockRejectedValue(new Error('Host status unavailable'));
});
afterEach(() => { unmount(); if (vi.isFakeTimers()) vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('pre-confirmed apply request', () => {
  it('applies the saved candidate and confirms its restart token without a second prompt or draft save', async () => {
    h.app.applyRequested = true;
    h.app.setApplyRequested.mockImplementation(requested => { h.app.applyRequested = requested; });
    h.freshApply.mockResolvedValueOnce({ ok: false, status: 'restartRequired', restartConfirmation: 'restart-token',
      reasons: [{ code: 'HOST_CARRIER_TRANSITION_REQUIRES_RESTART', path: 'proxyModeType' }] });
    h.applyNow.mockResolvedValueOnce({ saved: true, ok: true, status: 'applied', applyId: 3, runtimeGeneration: 7 });
    renderBar();
    await microtasks();
    renderBar();
    expect(h.app.setApplyRequested).toHaveBeenCalledExactlyOnceWith(false);
    expect(h.freshApply).toHaveBeenCalledExactlyOnceWith();
    expect(h.applyNow).toHaveBeenCalledExactlyOnceWith('restart-token');
    expect(h.states[1]).toBeNull();
  });
});

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


describe('desktop native listener cleanup stays independent', () => {
  const native: NativeRetirementDebt = { transactionId: 'native-A', runtimeGeneration: 7, selectionIntentGeneration: 9,
    retirementState: 'UNKNOWN', reason: { code: 'LISTENER_RETIREMENT_FAILED', path: 'inbounds' } };
  const empty = (): PendingNodeChanges => ({ added: [], modified: [], removed: [], restartDeferred: false });
  const receipt = (over: Partial<PendingApplyResult> = {}): PendingApplyResult => ({ ok: false,
    status: 'nativeCleanupUnknown', transactionId: 'native-A', runtimeGeneration: 7, selectionIntentGeneration: 9,
    coreState: 'committed', configurationDurable: true, retirementState: 'UNKNOWN', nativeRetirements: [native], ...over });
  async function start(over: Partial<PendingApplyResult> = {}) {
    h.applyNow.mockResolvedValueOnce({ ...receipt(over), saved: true });
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]));
    h.app.pendingChanges = empty();
    renderBar();
  }

  it('shows the real durable committed result at empty deletion scope and never toasts success', async () => {
    await start();
    const html = markup();
    expect(html).toContain('home.pendingNativeCleanupUnknown');
    expect(html).toContain('home.pendingNativeSavedActive');
    expect(html).toContain('home.pendingNativeCleanupDetail');
    expect(html).toContain('home.pendingCheckNativeBtn');
    expect(html).not.toContain('home.pendingRetryBtn');
    expect(html).not.toContain('home.pendingIgnoreBtn');
    expect(toast.info).not.toHaveBeenCalled();
    expect(h.nativeRead).not.toHaveBeenCalled();
    expect(h.coreRead).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    const details = elements(renderBar()).find(element => element.type === 'button'
      && text(element.props.children).includes('home.pendingNativeCleanupUnknown'));
    details!.props.onClick!();
    const expanded = markup();
    expect(expanded).toContain('white-space:normal');
    expect(expanded).toContain('<p>home.pendingNativeCleanupDetail</p>');
    expect(expanded).not.toContain('native-A');
  });

  it('the real popover effect keeps browser outside-click/Escape behavior and releases its listeners', async () => {
    const add = vi.fn(), remove = vi.fn();
    vi.stubGlobal('document', { addEventListener: add, removeEventListener: remove });
    await start();
    const inside = {}, outside = {};
    const root = renderBar() as ReactElement<{ ref: { current: { contains: (target: unknown) => boolean } | null } }>;
    root.props.ref.current = { contains: target => target === inside };
    const open = () => {
      const details = elements(renderBar()).find(element => element.type === 'button'
        && text(element.props.children).includes('home.pendingNativeCleanupUnknown'));
      details!.props.onClick!();
      return markup();
    };
    expect(add).not.toHaveBeenCalled();
    expect(open()).toContain('class="pd-pop"');
    expect(add.mock.calls.map(([kind]) => kind)).toEqual(['mousedown', 'keydown']);
    const down = add.mock.calls[0][1] as (event: { target: unknown }) => void;
    const key = add.mock.calls[1][1] as (event: { key: string }) => void;
    down({ target: inside }); key({ key: 'Enter' });
    expect(markup()).toContain('class="pd-pop"');
    key({ key: 'Escape' });
    expect(markup()).not.toContain('class="pd-pop"');
    expect(remove).toHaveBeenCalledWith('mousedown', down);
    expect(remove).toHaveBeenCalledWith('keydown', key);
    expect(open()).toContain('class="pd-pop"');
    const outsideDown = add.mock.calls[2][1] as (event: { target: unknown }) => void;
    outsideDown({ target: outside });
    expect(markup()).not.toContain('class="pd-pop"');
    expect(remove).toHaveBeenCalledWith('mousedown', outsideDown);
    expect(open()).toContain('class="pd-pop"');
    const finalDown = add.mock.calls[4][1], finalKey = add.mock.calls[5][1];
    unmount();
    expect(remove).toHaveBeenCalledWith('mousedown', finalDown);
    expect(remove).toHaveBeenCalledWith('keydown', finalKey);
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.nativeRead).not.toHaveBeenCalled();
    expect(h.freshApply).not.toHaveBeenCalled();
  });

  it('keeps normal accepted-carrier PENDING successful, then receives delayed UNKNOWN through the pending feed', async () => {
    h.applyNow.mockResolvedValueOnce({ ...receipt({ ok: true, status: 'hotApplied', retirementState: 'PENDING',
      nativeRetirements: undefined }), saved: true });
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect((h.states[0] as ApplyState).configurationDurable).toBe(true));
    h.app.pendingChanges = empty();
    expect(renderBar()).toBeNull();
    expect(toast.info).toHaveBeenCalledExactlyOnceWith('home.pendingHotApplied');
    h.app.pendingChanges = { ...empty(), nativeRetirements: [native] };
    expect(markup()).toContain('home.pendingNativeCleanupUnknown');
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.freshApply).not.toHaveBeenCalled();
  });

  it('restores a native-only pending push with no local Apply owner and keeps it after an empty late frame', () => {
    h.app.pendingChanges = { ...empty(), nativeRetirements: [native] };
    expect(markup()).toContain('home.pendingNativeCleanupUnknown');
    expect((h.states[0] as ApplyState).transactionId).toBeUndefined();
    h.app.pendingChanges = empty();
    expect(markup()).toContain('home.pendingNativeCleanupUnknown');
    expect(markup()).not.toContain('home.pendingNativeSavedActive');
    expect(h.applyNow).not.toHaveBeenCalled();
    expect(h.nativeRead).not.toHaveBeenCalled();
    expect(h.coreRead).not.toHaveBeenCalled();
  });

  it('retains A through a successor HostOnly B acceptance and checks only A original transaction', async () => {
    await start({ hostExecutionId: 'host-B', selectionIntentGeneration: 10, transactionId: undefined,
      coreState: 'unchanged', hostEffects: [effect] });
    expect((h.states[0] as ApplyState).host).toMatchObject({ executionId: 'host-B', phase: 'settled' });
    expect(markup()).toContain('home.pendingNativeHostSettled');
    Object.assign(h.app.proxyStatus, { ownedStopContext: { selectionIntentGeneration: 11 } });
    h.nativeRead.mockResolvedValueOnce(receipt({ nativeRetirements: [], retirementState: 'DRAINED' }));
    const check = button('home.pendingCheckNativeBtn');
    check.props.onClick!(); check.props.onClick!();
    await microtasks();
    expect(h.nativeRead).toHaveBeenCalledExactlyOnceWith('native-A');
    expect(h.coreRead).not.toHaveBeenCalled();
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
    expect((h.states[0] as ApplyState).host?.phase).toBe('settled');
    expect(markup()).toContain('home.pendingNativeCleanupUnknown');
    expect(h.hostRead).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });

  it.each(['wrong transaction', 'wrong generation', 'wrong intent', 'missing owner'] as const)(
    'a %s native check cannot add a foreign proof or clear A', async route => {
      await start();
      const foreign = { ...native, transactionId: 'foreign' };
      h.nativeRead.mockResolvedValueOnce(receipt({ nativeRetirements: [foreign],
        ...(route === 'wrong transaction' ? { transactionId: 'other' } : route === 'wrong generation'
          ? { runtimeGeneration: 8 } : route === 'wrong intent' ? { selectionIntentGeneration: 10 }
            : { transactionId: undefined, hostExecutionId: 'native-A' }) }));
      click('home.pendingCheckNativeBtn'); await microtasks();
      expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
      expect(markup()).toContain('home.pendingNativeCleanupUnknown');
      expect(h.nativeRead).toHaveBeenCalledExactlyOnceWith('native-A');
      expect(h.coreRead).not.toHaveBeenCalled();
    });

  it('a native-only check stays read-only while the original generic status later advances pending D', async () => {
    h.applyNow.mockResolvedValueOnce({ ...receipt({ configurationDurable: false }), saved: true });
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect((h.states[0] as ApplyState).configurationDurable).toBe(false));
    h.app.pendingChanges = empty();
    vi.useFakeTimers();
    renderBar();
    h.nativeRead.mockResolvedValueOnce(receipt({ configurationDurable: false }));
    click('home.pendingCheckNativeBtn'); await microtasks();
    expect(h.nativeRead).toHaveBeenCalledExactlyOnceWith('native-A');
    expect(h.coreRead).not.toHaveBeenCalled();
    expect((h.states[0] as ApplyState).configurationDurable).toBe(false);
    // Provider readiness is observed by the original D endpoint, whose backend may perform its existing full-owner ACK.
    h.coreRead.mockResolvedValueOnce(receipt({ configurationDurable: true }));
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.coreRead).toHaveBeenCalledExactlyOnceWith('native-A');
    expect(h.nativeRead).toHaveBeenCalledTimes(1);
    expect((h.states[0] as ApplyState).coreRecovery).toBeUndefined();
    expect((h.states[0] as ApplyState).configurationDurable).toBe(true);
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
    expect(markup()).toContain('home.pendingNativeCleanupUnknown');
    expect(markup()).toContain('home.pendingNativeSavedActive');
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.freshApply).not.toHaveBeenCalled();
  });

  it('finishes D independently and keeps native A while the same host receipt remains accepted', async () => {
    await start({ configurationDurable: false, hostExecutionId: 'host-B', hostEffects: [effect] });
    expect(markup()).toContain('home.pendingNativeSaveUnconfirmed');
    h.retryPersistence.mockResolvedValueOnce(receipt({ hostExecutionId: 'host-B', hostEffects: [effect] }));
    click('home.pendingRetryPersistenceBtn'); await microtasks();
    expect(h.retryPersistence).toHaveBeenCalledExactlyOnceWith('native-A');
    expect((h.states[0] as ApplyState).coreRecovery).toBeUndefined();
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
    expect(markup()).toContain('home.pendingNativeSavedActive');
    expect(markup()).toContain('home.pendingNativeHostSettled');
    expect(toast.info).not.toHaveBeenCalled();
  });

  it('a new successful Apply racing a native feed cannot overwrite the newly observed original debt', async () => {
    let finish!: (result: unknown) => void;
    h.applyNow.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    click('home.pendingApplyBtn'); await microtasks();
    h.app.pendingChanges = { ...empty(), nativeRetirements: [native] };
    renderBar();
    finish({ saved: true, ok: true, status: 'hotApplied', transactionId: 'new-B', runtimeGeneration: 7,
      selectionIntentGeneration: 10, coreState: 'committed', configurationDurable: true });
    await microtasks();
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
    expect(markup()).toContain('home.pendingNativeCleanupUnknown');
    expect(toast.info).not.toHaveBeenCalled();
  });

  it('clears only after terminal process completion plus matching fresh status and rejects late A push', async () => {
    await start();
    const listener = h.lifecycleReady.mock.calls[0][0] as (event: unknown) => void;
    h.app.proxyStatus = { running: true, mainGeneration: 8 };
    renderBar();
    expect(markup()).toContain('home.pendingNativeCleanupUnknown');
    listener({ kind: 'restart', phase: 'failed', terminal: true, mainGeneration: 8 });
    await microtasks();
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
    listener({ kind: 'stop', phase: 'stopped', terminal: false, mainGeneration: 8 });
    await microtasks();
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([native]);
    h.app.proxyStatus = { running: false, mainGeneration: 8 };
    listener({ kind: 'stop', phase: 'stopped', terminal: true, mainGeneration: 8 });
    await microtasks();
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([]);
    h.app.pendingChanges = { ...empty(), nativeRetirements: [native] };
    expect(renderBar()).toBeNull();
    expect((h.states[0] as ApplyState).nativeRetirements).toEqual([]);
  });
});
