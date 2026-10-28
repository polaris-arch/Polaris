import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { clearHelperInstallFeedback } from './helper-install-session';
import { isValidElement, type ReactElement } from 'react';

const h = vi.hoisted(() => ({
  state: [] as unknown[], refs: [] as Array<{ current: unknown }>,
  stateIndex: 0, refIndex: 0, effect: null as null | (() => (() => void)),
  refresh: null as null | (() => void), platform: 'mac' as string | undefined, cleanup: null as null | (() => void),
}));
vi.mock('react', async (original) => ({
  ...await original<typeof import('react')>(),
  useState: (initial: unknown) => {
    const i = h.stateIndex++;
    if (!(i in h.state)) h.state[i] = initial;
    return [h.state[i], (value: unknown) => { h.state[i] = typeof value === 'function' ? value(h.state[i]) : value; }];
  },
  useRef: (initial: unknown) => {
    const i = h.refIndex++;
    return h.refs[i] ?? (h.refs[i] = { current: initial });
  },
  useEffect: (effect: () => (() => void)) => { h.effect ??= effect; },
}));
vi.mock('react-i18next', async (original) => ({
  ...await original<typeof import('react-i18next')>(), useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock('@/ipc/api-client', () => ({ helperApi: {
  getStatus: vi.fn(), install: vi.fn(), uninstall: vi.fn(),
  onUpgradeable: (f: () => void) => { h.refresh = f; return () => {}; },
} }));
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn() } }));
vi.mock('@/lib/confirm-twice', () => ({ useConfirmTwice: () => ({ armed: null, confirmTwice: vi.fn() }) }));
vi.mock('./settings-logic', () => ({ shellPlatformFromDataOs: () => h.platform }));
await (await import('@/i18n')).i18nReady;
const { helperApi } = await import('@/ipc/api-client');
const { useDialogStore } = await import('@/components/dialogs/dialog-store');
const { default: Screen } = await import('./SettingsHelper');
const { FeedbackNotice } = await import('./Primitives');
const { toast } = await import('@/lib/error-handler');
const absent = { supported: true, installed: false, ready: false, upgradeable: false, needsRepair: false };
function render() { h.stateIndex = h.refIndex = 0; return Screen(); }
function nodes(node: unknown): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!isValidElement<Record<string, unknown>>(node)) return [];
  // The real presenter renders its actions slot; do not execute hook-bearing components here.
  return [node, ...nodes(node.props.children), ...(node.type === FeedbackNotice ? nodes(node.props.actions) : [])];
}
function text(node: unknown): string {
  if (Array.isArray(node)) return node.map(text).join('');
  if (isValidElement<Record<string, unknown>>(node)) return text(node.props.children);
  return typeof node === 'string' ? node : '';
}
function button(label: string) {
  return nodes(render()).find(n => typeof n.props.onClick === 'function' && text(n.props.children) === label);
}
function click(label: string) {
  const target = button(label);
  expect(target, label).toBeDefined();
  expect(target!.props.disabled).toBe(false);
  (target!.props.onClick as () => void)();
}
async function mounted() {
  vi.mocked(helperApi.getStatus).mockResolvedValue(absent as never);
  render(); const unmount = h.effect!(); h.cleanup = unmount;
  await vi.waitFor(() => expect(h.state[1]).toBe('none'));
  return unmount;
}
async function deferred() {
  const unmount = await mounted();
  vi.mocked(helperApi.install).mockResolvedValue({ success: false, errorCode: 'upgradeDeferred', status: absent } as never);
  click('helper.installAction');
  await vi.waitFor(() => expect(h.state[3]).toBe('upgradeDeferred'));
  expect(helperApi.install).toHaveBeenCalledExactlyOnceWith();
  return unmount;
}
function confirmation() {
  const stack = useDialogStore.getState().stack;
  const dialog = stack[stack.length - 1];
  expect(dialog?.kind).toBe('confirm');
  if (dialog?.kind !== 'confirm') throw new Error('missing confirmation');
  return dialog.payload;
}
beforeEach(() => {
  h.state = []; h.refs = []; h.effect = h.refresh = null; h.platform = 'mac';
  clearHelperInstallFeedback();
  vi.clearAllMocks(); useDialogStore.getState().closeAll();
});
it('mount, refresh, and ordinary upgradeDeferred never select or auto-start migration', async () => {
  await mounted(); h.refresh!(); await Promise.resolve();
  expect(helperApi.install).not.toHaveBeenCalled();
  expect(button('helper.stopAndUpgradeAction')).toBeUndefined();
  vi.mocked(helperApi.install).mockResolvedValue({ success: false, errorCode: 'upgradeDeferred', status: absent } as never);
  click('helper.installAction');
  await vi.waitFor(() => expect(h.state[3]).toBe('upgradeDeferred'));
  expect(button('helper.stopAndUpgradeAction')).toBeDefined();
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[]]);
  expect(useDialogStore.getState().stack).toHaveLength(0);
});
it.each(['win', 'lin', undefined])('hides the explicit action on %s even after upgradeDeferred', async platform => {
  h.platform = platform; await deferred();
  expect(button('helper.stopAndUpgradeAction')).toBeUndefined();
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[]]);
});
it.each(['failed', 'cancelled', 'proxyRunning'])('does not offer stop-and-upgrade for %s', async errorCode => {
  await mounted();
  vi.mocked(helperApi.install).mockResolvedValue({ success: false, errorCode, status: absent } as never);
  click('helper.installAction'); await vi.waitFor(() => expect(h.state[3]).toBe(errorCode));
  expect(button('helper.stopAndUpgradeAction')).toBeUndefined();
});
it('requires the scoped confirmation, deduplicates clicks, and sends the typed mode only once', async () => {
  await deferred(); click('helper.stopAndUpgradeAction'); click('helper.stopAndUpgradeAction');
  expect(useDialogStore.getState().stack).toHaveLength(1);
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[]]);
  const prompt = confirmation();
  expect(prompt).toMatchObject({ title: 'helper.stopAndUpgradeTitle', message: 'helper.stopAndUpgradeWarning', confirmLabel: 'helper.stopAndUpgradeConfirm', danger: true });
  vi.mocked(helperApi.install).mockResolvedValue({ success: true, status: absent } as never);
  prompt.onConfirm(); prompt.onConfirm();
  await vi.waitFor(() => expect(h.state[2]).toBe(false));
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[], ['stopAndUpgrade']]);
  expect(useDialogStore.getState().stack).toHaveLength(0);
  expect(button('helper.stopAndUpgradeAction')).toBeUndefined();
});
it('Cancel, external dismissal, and unmount make saved confirmation callbacks inert', async () => {
  const unmount = await deferred();
  click('helper.stopAndUpgradeAction'); const cancelled = confirmation();
  useDialogStore.getState().close(); cancelled.onCancel?.(); cancelled.onConfirm();
  click('helper.stopAndUpgradeAction'); const dismissed = confirmation();
  useDialogStore.getState().closeAll(); dismissed.onConfirm();
  click('helper.stopAndUpgradeAction'); const abandoned = confirmation();
  unmount(); abandoned.onConfirm();
  expect(useDialogStore.getState().stack).toHaveLength(0);
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[]]);
});
it('an explicit failure stays visible with no fallback or retry', async () => {
  await deferred(); click('helper.stopAndUpgradeAction');
  vi.mocked(helperApi.install).mockResolvedValue({ success: false, errorCode: 'installerFailed', status: absent } as never);
  confirmation().onConfirm(); await vi.waitFor(() => expect(h.state[3]).toBe('installerFailed'));
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[], ['stopAndUpgrade']]);
  expect(button('helper.stopAndUpgradeAction')).toBeUndefined();
});

it('the deferred primary action no longer offers an ordinary retry and a cancelled confirmation is inert', async () => {
  await deferred();
  expect(button('helper.installAction')).toBeUndefined();
  click('helper.stopAndUpgradeAction');
  const stale = confirmation();
  useDialogStore.getState().close(); stale.onCancel?.(); stale.onConfirm();
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[]]);
  expect(h.state[3]).toBe('upgradeDeferred');
});
it('revalidates the platform before a confirmed migration request', async () => {
  await deferred(); click('helper.stopAndUpgradeAction');
  h.platform = 'win'; confirmation().onConfirm();
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[]]);
  expect(useDialogStore.getState().stack).toHaveLength(0);
});
it.each(['cancelled', 'upgradeDeferred'])('an explicit %s result never triggers a retry on refresh', async errorCode => {
  await deferred(); click('helper.stopAndUpgradeAction');
  vi.mocked(helperApi.install).mockResolvedValue({ success: false, errorCode, status: absent } as never);
  confirmation().onConfirm();
  await vi.waitFor(() => expect(h.state[2]).toBe(false));
  h.refresh!(); await Promise.resolve();
  expect(h.state[3]).toBe(errorCode);
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[], ['stopAndUpgrade']]);
  expect(useDialogStore.getState().stack).toHaveLength(0);
});
it('an in-flight request deduplicates even previously captured click handlers', async () => {
  await mounted();
  let complete!: (value: never) => void;
  vi.mocked(helperApi.install).mockImplementation(() => new Promise(resolve => { complete = resolve; }));
  const install = button('helper.installAction')!.props.onClick as () => void;
  install(); install();
  expect(vi.mocked(helperApi.install).mock.calls).toEqual([[]]);
  complete({ success: true, status: absent } as never);
  await vi.waitFor(() => expect(h.state[2]).toBe(false));
});


it('classified install failure remains in the global toast after settings unmounts', async () => {
  const unmount = await mounted();
  vi.mocked(helperApi.install).mockResolvedValue({ success: false, errorCode: 'installerFailed', status: absent } as never);
  click('helper.installAction');
  await vi.waitFor(() => expect(h.state[3]).toBe('installerFailed'));
  unmount();
  expect(toast.error).toHaveBeenCalledExactlyOnceWith('helper.installFail', 'helper.actionError.installerFailed');
  expect(helperApi.install).toHaveBeenCalledExactlyOnceWith();
});

afterEach(() => { h.cleanup?.(); h.cleanup = null; clearHelperInstallFeedback(); });
