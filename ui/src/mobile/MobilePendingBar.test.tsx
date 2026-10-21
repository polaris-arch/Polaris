import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
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
  lifecycleReady: vi.fn(),
  offLifecycle: vi.fn(),
  app: {
    pendingChanges: { added: ['node-1'], modified: [], removed: [], restartDeferred: false },
    proxyStatus: { running: true, mainGeneration: 7 },
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
// Poll ownership has separate tests; these cases cover the mobile write/confirmation boundary.
vi.mock('@/components/layout/use-runtime-apply-reconciliation', () => ({
  useRuntimeApplyReconciliation: () => undefined,
}));
vi.mock('@/ipc', () => ({ api: { proxy: {
  onLifecycleReady: h.lifecycleReady,
  retryRuntimePersistence: h.retryPersistence,
  getStatus: vi.fn(),
} } }));
vi.mock('@/store/app-store', () => ({
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
  h.app.pendingChanges = { added: ['node-1'], modified: [], removed: [], restartDeferred: false };
  h.app.proxyStatus = { running: true, mainGeneration: 7 };
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});
afterEach(() => { unmount(); vi.restoreAllMocks(); });

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
