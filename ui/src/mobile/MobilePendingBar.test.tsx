import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { PendingApplyResult } from '@/contracts/types';
import type { ApplyState } from '@/components/layout/pending-bar-logic';
import type { FormSheetProps } from './forms/FormSheet';

// Exercise the real handlers in the existing node environment. Keep hook state/refs across
// renders and flush dependency-aware effects after rendering, as in UpdatePage's handler tests.
const h = vi.hoisted(() => ({
  states: [] as unknown[],
  refs: [] as Array<{ current: unknown }>,
  stateIndex: 0,
  refIndex: 0,
  effectIndex: 0,
  effects: [] as Array<{ deps?: readonly unknown[]; cleanup?: () => void }>,
  pendingEffects: [] as Array<() => void>,
  applyNow: vi.fn(),
  retryPersistence: vi.fn(),
  hostRead: vi.fn(),
  coreRead: vi.fn(),
  pendingRead: vi.fn(),
  freshApply: vi.fn(),
  cancelImpact: vi.fn(),
  realReconciliation: false,
  lifecycleReady: vi.fn(),
  offLifecycle: vi.fn(),
  app: {
    pendingChanges: { added: ['node-1'], modified: [], removed: [], restartDeferred: false },
    proxyStatus: { running: true, mainGeneration: 7,
      ownedStopContext: undefined as { selectionIntentGeneration: number } | undefined },
    config: null as Record<string, unknown> | null,
    setPendingChanges: vi.fn(),
  },
}));

vi.mock('react', async (original) => ({
  ...await original<typeof import('react')>(),
  useState: (initial: unknown) => {
    const index = h.stateIndex++;
    if (!(index in h.states)) h.states[index] = typeof initial === 'function' ? initial() : initial;
    return [h.states[index], (value: unknown) => {
      h.states[index] = typeof value === 'function' ? value(h.states[index]) : value;
    }];
  },
  useRef: (initial: unknown) => h.refs[h.refIndex++] ??= { current: initial },
  useEffect: (effect: () => void | (() => void), deps?: readonly unknown[]) => {
    const index = h.effectIndex++;
    const previous = h.effects[index];
    if (previous && deps && previous.deps && deps.length === previous.deps.length &&
        deps.every((value, i) => Object.is(value, previous.deps![i]))) return;
    h.pendingEffects.push(() => {
      previous?.cleanup?.();
      const cleanup = effect();
      h.effects[index] = { deps: deps?.slice(), cleanup: cleanup || undefined };
    });
  },
}));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/lib/confirm-twice', () => ({
  useConfirmTwice: () => ({ armed: null, confirmTwice: vi.fn() }),
}));
// Existing restart tests isolate observation; host cases exercise the real shared hook and selectors.
vi.mock('@/components/layout/use-runtime-apply-reconciliation', async (original) => {
  const actual = await original<typeof import('@/components/layout/use-runtime-apply-reconciliation')>();
  return { useRuntimeApplyReconciliation: (...args: Parameters<typeof actual.useRuntimeApplyReconciliation>) =>
    h.realReconciliation ? actual.useRuntimeApplyReconciliation(...args)
      : { retryPersistence: async () => undefined, checkHost: () => undefined } };
});
vi.mock('@/ipc', () => ({ api: { proxy: {
  onLifecycleReady: h.lifecycleReady,
  retryRuntimePersistence: h.retryPersistence,
  runtimeHostEffectStatus: h.hostRead,
  runtimeApplyStatus: h.coreRead,
  getPendingChanges: h.pendingRead,
  applyPendingChanges: h.freshApply,
  cancelRuntimeImpact: h.cancelImpact,
  getStatus: vi.fn(),
} } }));
vi.mock('@/store/app-store', () => ({
  normalizePendingChanges: (pending: unknown) => pending,
  useAppStore: Object.assign((selector: (state: typeof h.app) => unknown) => selector(h.app), {
    getState: () => h.app,
  }),
}));
vi.mock('@/store/staged-config-store', () => {
  const state = { entries: [], saveStatus: 'idle', conflict: null, applyNow: h.applyNow };
  return { useStagedConfigStore: Object.assign((selector: (value: typeof state) => unknown) => selector(state), {
    getState: () => state,
  }) };
});
vi.mock('@/lib/error-handler', () => ({
  toast: { info: vi.fn(), error: vi.fn(), warning: vi.fn(), success: vi.fn() },
}));

import { MobilePendingBar } from './MobilePendingBar';
import { FormSheet } from './forms/FormSheet';
import { clearDismissables, dismissTop } from './back-stack';

function unmount(): void {
  h.effects.forEach((effect) => effect.cleanup?.());
  h.effects = [];
  h.pendingEffects = [];
  clearDismissables();
}

function renderBar(): ReactElement | null {
  h.stateIndex = 0;
  h.refIndex = 0;
  h.effectIndex = 0;
  const tree = MobilePendingBar();
  h.pendingEffects.splice(0).forEach((run) => run());
  return tree;
}

function markup(node: ReactNode): string {
  const html = renderToStaticMarkup(node);
  h.pendingEffects.splice(0).forEach((run) => run());
  return html;
}

type ElementProps = { children?: ReactNode; onClick?: () => void; disabled?: boolean };
function elements(node: ReactNode): ReactElement<ElementProps>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<ElementProps>(node)) return [];
  return [node, ...elements(node.props.children)];
}

function click(label: string): void {
  const button = elements(renderBar()).find((element) => element.type === 'button' && element.props.children === label);
  expect(button, `missing action ${label}`).toBeDefined();
  expect(button!.props.disabled).not.toBe(true);
  button!.props.onClick!();
}

function restartSheet(): ReactElement<FormSheetProps> {
  const sheet = elements(renderBar()).find((element) => element.type === FormSheet);
  expect(sheet).toBeDefined();
  return sheet as ReactElement<FormSheetProps>;
}

async function openRestartSheet(): Promise<ReactElement<FormSheetProps>> {
  h.applyNow.mockResolvedValueOnce({ saved: true, status: 'restartRequired', restartConfirmation: 'restart-token',
    reasons: [{ code: 'structuralChange', path: 'tun.enabled' }, { code: 'unknown', path: 'private-path' }] });
  click('home.pendingApplyBtn');
  await vi.waitFor(() => expect(h.states[1]).toMatchObject({ token: 'restart-token' }));
  return restartSheet();
}

beforeEach(() => {
  unmount();
  vi.resetAllMocks();
  h.states = [];
  h.refs = [];
  h.lifecycleReady.mockResolvedValue(h.offLifecycle);
  h.app.config = null;
  h.app.pendingChanges = { added: ['node-1'], modified: [], removed: [], restartDeferred: false };
  h.realReconciliation = false;
  h.app.proxyStatus = { running: true, mainGeneration: 7, ownedStopContext: undefined };
  h.app.setPendingChanges.mockImplementation((pending) => { h.app.pendingChanges = pending; });
  h.pendingRead.mockImplementation(async () => h.app.pendingChanges);
  h.hostRead.mockRejectedValue(new Error('no host status reply'));
  h.coreRead.mockRejectedValue(new Error('no Core status reply'));
  h.cancelImpact.mockResolvedValue({ ok: true, status: 'cancelled' });
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});
afterEach(() => { unmount(); if (vi.isFakeTimers()) vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); });

describe('mobile pending restart and write feedback', () => {
  it('uses the mobile sheet and trusted reasons, without applying the restart while opening it', async () => {
    const sheet = await openRestartSheet();
    expect(sheet.props.compact).toBe(true);
    expect(sheet.props.cancelLabel).toBe('home.restartApplyKeepConnection');
    expect(sheet.props.submitLabel).toBe('home.restartApplyConfirm');
    const html = markup(sheet);
    expect(html).toContain('class="m-form-layer"');
    expect(html).toContain('role="dialog"');
    expect(html).toContain('home.restartApplyDescription');
    expect(html).toContain('home.restartApplyTun');
    expect(html).not.toContain('private-path');
    expect(html).not.toContain('restart-token');
    expect(html).not.toContain('class="btn');
    expect(h.applyNow).toHaveBeenCalledExactlyOnceWith(undefined);
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });

  it.each(['close/cancel', 'back'])('%s dismisses the restart decision without another write', async (route) => {
    const sheet = await openRestartSheet();
    if (route === 'back') {
      markup(sheet); // FormSheet registers its real back-stack layer.
      expect(dismissTop()).toBe(true);
      expect(dismissTop()).toBe(false);
    } else {
      sheet.props.onRequestClose(); // Cancel, close button and scrim share this callback.
    }
    expect(h.states[1]).toBeNull();
    expect(elements(renderBar()).some((element) => element.type === FormSheet)).toBe(false);
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });

  it('only explicit confirmation submits the exact token, once even on a repeated tap', async () => {
    const sheet = await openRestartSheet();
    h.applyNow.mockResolvedValueOnce({ saved: true, status: 'hotApplied', runtimeGeneration: 7 });
    sheet.props.onSubmit!();
    sheet.props.onSubmit!();
    await vi.waitFor(() => expect(h.states[0]).toMatchObject({ phase: 'idle' }));
    expect(h.applyNow).toHaveBeenCalledTimes(2);
    expect(h.applyNow).toHaveBeenNthCalledWith(2, 'restart-token');
    expect(h.states[1]).toBeNull();
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });

  it.each(['apply', 'persistence retry'])('%s waits for actual listener registration before writing', async (action) => {
    let registered!: (off: () => void) => void;
    h.lifecycleReady.mockReturnValueOnce(new Promise<() => void>((resolve) => { registered = resolve; }));
    if (action === 'persistence retry') {
      h.states = [{ phase: 'durabilityPending', reason: null, transactionId: 'same-transaction' }];
    }
    h.applyNow.mockResolvedValueOnce({ saved: true, status: 'hotApplied', runtimeGeneration: 7 });
    h.retryPersistence.mockResolvedValueOnce({ status: 'hotApplied', transactionId: 'same-transaction', runtimeGeneration: 7 });
    click(action === 'apply' ? 'home.pendingApplyBtn' : 'home.pendingRetryPersistenceBtn');
    expect(h.lifecycleReady).toHaveBeenCalledTimes(1);
    expect(h.states[0]).toMatchObject({ phase: 'applying' });
    await Promise.resolve();
    expect(h.applyNow).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
    registered(h.offLifecycle);
    await vi.waitFor(() => expect(h.states[0]).toMatchObject({ phase: 'idle' }));
    if (action === 'apply') {
      expect(h.applyNow).toHaveBeenCalledExactlyOnceWith(undefined);
      expect(h.retryPersistence).not.toHaveBeenCalled();
    } else {
      expect(h.retryPersistence).toHaveBeenCalledExactlyOnceWith('same-transaction');
      expect(h.applyNow).not.toHaveBeenCalled();
    }
    expect(h.offLifecycle).not.toHaveBeenCalled();
  });

  it.each(['apply', 'persistence retry'])('unmount before listener readiness cancels %s and releases the late listener', async (action) => {
    let registered!: (off: () => void) => void;
    h.lifecycleReady.mockReturnValueOnce(new Promise<() => void>((resolve) => { registered = resolve; }));
    if (action === 'persistence retry') {
      h.states = [{ phase: 'durabilityPending', reason: null, transactionId: 'same-transaction' }];
    }
    click(action === 'apply' ? 'home.pendingApplyBtn' : 'home.pendingRetryPersistenceBtn');
    const lastState = h.states[0];
    unmount(); // Run the component's real registered cleanup before its listener resolves.
    expect(h.offLifecycle).not.toHaveBeenCalled();
    registered(h.offLifecycle);
    await vi.waitFor(() => expect(h.offLifecycle).toHaveBeenCalledTimes(1));
    await Promise.resolve(); // Let the onApply continuation observe the cleared request owner.
    expect(h.applyNow).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.states[0]).toBe(lastState);
  });

  it('does not offer or authorize restart when the receipt omits its confirmation token', async () => {
    h.applyNow.mockResolvedValueOnce({ saved: true, status: 'restartRequired', reasons: [] });
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect(h.states[0]).toMatchObject({ phase: 'failed' }));
    expect(h.states[1]).toBeNull();
    expect(markup(renderBar())).toContain('m-pending-err');
    expect(h.applyNow).toHaveBeenCalledExactlyOnceWith(undefined);
  });

  it.each(['write', 'listener'])('%s rejection renders a persistent error and an explicit retry action', async (failure) => {
    if (failure === 'write') h.applyNow.mockRejectedValueOnce(new Error('write failed'));
    else h.lifecycleReady.mockRejectedValueOnce(new Error('listener failed'));
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect(h.states[0]).toMatchObject({ phase: 'failed' }));
    const html = markup(renderBar());
    expect(html).toContain('m-pending-err');
    expect(html).toContain('home.pendingApplyFailed');
    expect(html).toContain('home.pendingRetryBtn');
    expect(h.applyNow).toHaveBeenCalledTimes(failure === 'write' ? 1 : 0);
  });

  it('keeps a failed persistence retry visible and retries only the original transaction', async () => {
    h.applyNow.mockResolvedValueOnce({ saved: true, status: 'hotDurabilityPending', transactionId: 'same-transaction',
      runtimeGeneration: 7, selectionIntentGeneration: 0 });
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect(h.states[0]).toMatchObject({ phase: 'durabilityPending' }));
    h.retryPersistence.mockRejectedValueOnce(new Error('persistence failed'));
    click('home.pendingRetryPersistenceBtn');
    await vi.waitFor(() => expect(h.states[0]).toMatchObject({ phase: 'failed', persistenceOnly: true,
      transactionId: 'same-transaction' } satisfies Partial<ApplyState>));
    const html = markup(renderBar());
    expect(html).toContain('m-pending-err');
    expect(html).toContain('home.pendingApplyFailed');
    expect(html).toContain('home.pendingRetryPersistenceBtn');
    expect(html).not.toContain('home.pendingRetryBtn');
    expect(html).not.toContain('home.pendingApplyBtn');
    h.retryPersistence.mockResolvedValueOnce({ status: 'hotApplied', transactionId: 'same-transaction', runtimeGeneration: 7 });
    click('home.pendingRetryPersistenceBtn');
    await vi.waitFor(() => expect(h.states[0]).toMatchObject({ phase: 'idle' }));
    expect(h.retryPersistence.mock.calls).toEqual([['same-transaction'], ['same-transaction']]);
    expect(h.applyNow).toHaveBeenCalledExactlyOnceWith(undefined);
  });
});


describe('mobile independent host effect consumption', () => {
  const effect = { preview: { kind: 'systemProxy' as const, scopeDigest: 'scope', beforeDigest: 'before', targetDigest: 'target' },
    outcome: 'applied' as const, receiptId: 'host-receipt', cleanupPending: false, retryable: false, reasonCode: null };
  const owned = (status: PendingApplyResult['status']): PendingApplyResult => ({
    ok: true, status, hostExecutionId: 'host-only', runtimeGeneration: 7, selectionIntentGeneration: 9,
    coreState: 'unchanged', configurationDurable: true,
  });
  async function begin(result: PendingApplyResult): Promise<void> {
    h.realReconciliation = true;
    h.applyNow.mockResolvedValueOnce({ ...result, saved: true });
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect((h.states[0] as ApplyState).host?.executionId).toBe(result.hostExecutionId));
    h.app.pendingChanges = { added: [], modified: [], removed: [], restartDeferred: false };
    vi.useFakeTimers();
    renderBar();
  }
  async function microtasks(): Promise<void> {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  }

  it('keeps HostOnly waiting at D=R, reads only the host selector, and clears only its actual receipt', async () => {
    await begin(owned('hostEffectsPending'));
    expect((h.states[0] as ApplyState).transactionId).toBeUndefined();
    expect(markup(renderBar())).toContain('home.pendingHostEffectsPending');
    expect(elements(renderBar()).filter(e => e.type === 'button')).toHaveLength(0);
    h.hostRead.mockResolvedValueOnce({ ...owned('hotApplied'), hostEffects: [effect] });
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.hostRead).toHaveBeenCalledExactlyOnceWith('host-only');
    expect((h.states[0] as ApplyState).host?.phase).toBe('settled');
    expect(renderBar()).toBeNull();
    expect(h.coreRead).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });

  it('keeps failed host debt visible after the original Core persistence retry succeeds', async () => {
    const failed = { ...effect, outcome: 'failedRestored' as const, retryable: true };
    await begin({ ...owned('hostEffectsFailed'), transactionId: 'original-core',
      coreState: 'committed', configurationDurable: false, hostEffects: [failed] });
    expect(markup(renderBar())).toContain('home.pendingHostAndDurabilityPending');
    h.retryPersistence.mockResolvedValueOnce({ ...owned('hostEffectsFailed'), transactionId: 'original-core',
      coreState: 'committed', configurationDurable: true, hostEffects: [failed] });
    click('home.pendingRetryPersistenceBtn');
    await microtasks();
    expect(h.retryPersistence).toHaveBeenCalledExactlyOnceWith('original-core');
    expect((h.states[0] as ApplyState).coreRecovery).toBeUndefined();
    expect((h.states[0] as ApplyState).host?.phase).toBe('failed');
    expect(markup(renderBar())).toContain('home.pendingHostCoreCommittedFailed');
    expect(markup(renderBar())).not.toContain('home.pendingRetryPersistenceBtn');
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });

  it('reviews a fresh saved candidate and requires its impact confirmation before a host retry', async () => {
    await begin({ ...owned('hostEffectsFailed'),
      hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    h.freshApply.mockResolvedValueOnce({ ok: false, status: 'impactConfirmationRequired', runtimeGeneration: 7,
      impactConfirmation: 'fresh-host-plan', coreImpacts: [], hostEffectPreviews: [effect.preview] });
    click('home.pendingRetryHostBtn');
    await microtasks();
    expect(h.freshApply).toHaveBeenCalledExactlyOnceWith();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.states[2]).toMatchObject({ token: 'fresh-host-plan', hostEffects: [effect.preview] });
    const sheet = restartSheet();
    expect(sheet.props.submitLabel).toBe('home.impactApplyConfirm');
    h.applyNow.mockResolvedValueOnce({ saved: true, ...owned('hostEffectsPending'), hostExecutionId: 'host-next' });
    sheet.props.onSubmit!();
    await microtasks();
    expect(h.applyNow).toHaveBeenNthCalledWith(2, undefined, 'fresh-host-plan');
    expect((h.states[0] as ApplyState).host).toMatchObject({ executionId: 'host-next', phase: 'pending' });
    expect(h.freshApply).toHaveBeenCalledTimes(1);
  });

  it.each(['stop', 'generation', 'intent', 'execution'] as const)('a late host result cannot clear the debt after %s changes', async route => {
    await begin(owned('hostEffectsPending'));
    let finish!: (result: PendingApplyResult) => void;
    h.hostRead.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.hostRead).toHaveBeenCalledTimes(1);
    if (route === 'stop') {
      const listener = h.lifecycleReady.mock.calls[0][0] as (event: unknown) => void;
      listener({ kind: 'stop', phase: 'stopped', terminal: true, mainGeneration: 8 });
    } else if (route === 'generation') {
      h.app.proxyStatus = { running: true, mainGeneration: 8, ownedStopContext: undefined };
    } else if (route === 'intent') {
      h.app.proxyStatus = { running: true, mainGeneration: 7, ownedStopContext: { selectionIntentGeneration: 10 } };
    } else {
      const current = h.states[0] as ApplyState;
      h.states[0] = { ...current, host: { ...current.host!, executionId: 'host-newer' } };
    }
    renderBar();
    finish({ ...owned('hotApplied'), hostEffects: [effect] });
    await microtasks();
    renderBar();
    expect((h.states[0] as ApplyState).host?.phase).not.toBe('settled');
    if (route === 'execution') expect((h.states[0] as ApplyState).host?.executionId).toBe('host-newer');
    else expect((h.states[0] as ApplyState).host?.superseded).toBe(true);
    expect(markup(renderBar())).toContain('home.pendingHost');
    expect(h.retryPersistence).not.toHaveBeenCalled();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
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


describe('mobile HostOnly original persistence account', () => {
  it('discovers the original Core owner before retrying and never sends the host id to a Core endpoint', async () => {
    await begin({ ...owned('hostEffectsFailed'), persistenceTransactionId: 'original-core',
      configurationDurable: false, hostEffects: [{ ...effect, outcome: 'failedRestored', retryable: true }] });
    expect((h.states[0] as ApplyState).transactionId).toBeUndefined();
    expect((h.states[0] as ApplyState).coreRecovery?.selectionIntentGeneration).toBeUndefined();
    const retry = elements(renderBar()).find(e => e.type === 'button' && e.props.children === 'home.pendingRetryPersistenceBtn');
    expect(retry?.props.disabled).toBe(true);
    h.coreRead.mockResolvedValueOnce({ ok: true, status: 'hotDurabilityPending', transactionId: 'original-core',
      runtimeGeneration: 7, selectionIntentGeneration: 9, configurationDurable: false });
    await vi.advanceTimersByTimeAsync(2000);
    renderBar();
    expect(h.coreRead).toHaveBeenCalledExactlyOnceWith('original-core');
    expect((h.states[0] as ApplyState).coreRecovery).toEqual({ transactionId: 'original-core',
      runtimeGeneration: 7, selectionIntentGeneration: 9 });
    h.retryPersistence.mockResolvedValueOnce({ ok: true, status: 'hotApplied', transactionId: 'original-core',
      runtimeGeneration: 7, selectionIntentGeneration: 9, configurationDurable: true });
    click('home.pendingRetryPersistenceBtn');
    await microtasks();
    expect(h.retryPersistence).toHaveBeenCalledExactlyOnceWith('original-core');
    expect((h.states[0] as ApplyState).coreRecovery).toBeUndefined();
    expect((h.states[0] as ApplyState).host).toMatchObject({ executionId: 'host-only', phase: 'failed' });
    expect(markup(renderBar())).toContain('home.pendingHostEffectsFailed');
    expect(h.hostRead).not.toHaveBeenCalled();
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
  });
});

});


describe('mobile impact consent disclosure', () => {
  async function offer(classes: string[], updateClass = 2) {
    h.applyNow.mockResolvedValueOnce({ saved: true, ok: false, status: 'impactConfirmationRequired',
      impactConfirmation: 'private-impact-token', runtimeGeneration: 7, coreImpacts: [{
        path: 'private-impact-path', reason: 'private-impact-reason', updateClass,
        reconnectRequired: true, affectedSessionClasses: classes,
      }] });
    click('home.pendingApplyBtn');
    await vi.waitFor(() => expect(h.states[2]).toMatchObject({ token: 'private-impact-token' }));
    return restartSheet();
  }

  it.each([
    { name: 'TUN', classes: ['existing TUN TCP flows', 'existing TUN UDP associations', 'changed ingress admissions'],
      expected: ['home.impactApplyTunTcpReset', 'home.impactApplyTunUdpReset', 'home.impactApplyTunRollback'], absent: 'home.impactApplyApiStreams' },
    { name: 'API/dashboard', classes: ['API streams on the replaced ingress', 'api_generation_handoff_v1', 'dashboard updater'],
      expected: ['home.impactApplyApiStreams', 'home.impactApplyDashboard'], absent: 'home.impactApplyTunRollback' },
    { name: 'listener/UDP', classes: ['changed ingress admissions', 'affected UDP associations', 'dependent listener handoff'],
      expected: ['home.impactApplyListenerAdmissions', 'home.impactApplyListenerUdp', 'home.impactApplyListenerHandoff'], absent: 'home.impactApplyTunRollback' },
  ])('discloses $name in the existing sheet before confirming its opaque token', async ({ classes, expected, absent }) => {
    const sheet = await offer(classes);
    expect(sheet.props.submitDisabled).toBe(false);
    const html = markup(sheet);
    for (const key of expected) expect(html).toContain(key);
    expect(html).not.toContain(absent);
    expect(html).toContain('home.impactApplySavedPending');
    for (const raw of ['private-impact-token', 'private-impact-path', 'private-impact-reason', 'api_generation_handoff_v1']) expect(html).not.toContain(raw);
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    h.applyNow.mockResolvedValueOnce({ saved: true, ok: true, status: 'hotApplied', configurationDurable: true });
    sheet.props.onSubmit!();
    await microtasks();
    expect(h.applyNow).toHaveBeenNthCalledWith(2, undefined, 'private-impact-token');
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });

  it.each([
    { classes: ['private-session-never-render'], updateClass: 2 },
    ...[0, 4, 5, 99].map(updateClass => ({ classes: ['API streams on the replaced ingress'], updateClass })),
  ])('blocks unknown risk, including direct submit, while cancel keeps the pending changes: %j', async ({ classes, updateClass }) => {
    const sheet = await offer(classes, updateClass);
    const html = markup(sheet);
    expect(sheet.props.submitDisabled).toBe(true);
    expect(html).toContain('role="alert"');
    expect(html).toContain('home.impactApplyUnknown');
    expect(html).toContain('home.impactApplySavedPending');
    for (const raw of ['private-impact-token', 'private-impact-path', 'private-impact-reason', 'private-session-never-render']) expect(html).not.toContain(raw);
    sheet.props.onSubmit!();
    await microtasks();
    expect(h.applyNow).toHaveBeenCalledTimes(1);
    expect(h.states[2]).toMatchObject({ token: 'private-impact-token' });
    sheet.props.onRequestClose();
    expect(h.cancelImpact).toHaveBeenCalledExactlyOnceWith('private-impact-token');
    expect(h.app.pendingChanges.added).toEqual(['node-1']);
    expect(h.app.proxyStatus.mainGeneration).toBe(7);
    expect(h.freshApply).not.toHaveBeenCalled();
    expect(h.retryPersistence).not.toHaveBeenCalled();
  });
});
