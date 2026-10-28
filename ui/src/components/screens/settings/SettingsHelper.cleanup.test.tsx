import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { clearHelperInstallFeedback } from './helper-install-session';
import { isValidElement, type ReactElement } from 'react';

// Exercise JSX handlers with controlled hooks and already-unwrapped API payloads.
// Real SettingsPage + raw transport envelopes are covered by the browser fixture.
// This is the existing node harness pattern; it does not emulate a real UAC window.
const h = vi.hoisted(() => ({
  state: [] as unknown[], refs: [] as Array<{ current: unknown }>,
  stateIndex: 0, refIndex: 0, effect: null as null | (() => (() => void)),
  refresh: null as null | (() => void), cleanup: null as null | (() => void),
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
vi.mock('react-i18next', async (importOriginal) => ({
  ...await importOriginal<typeof import('react-i18next')>(), useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/ipc/api-client', () => ({ helperApi: {
  getStatus: vi.fn(), install: vi.fn(), uninstall: vi.fn(),
  onUpgradeable: (f: () => void) => { h.refresh = f; return () => {}; },
} }));
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn() } }));
vi.mock('@/lib/confirm-twice', () => ({ useConfirmTwice: () => ({
  armed: null, confirmTwice: (_key: string, action: () => void) => action(),
}) }));
// Initialization must settle (or fail this suite), never leak an unhandled rejection.
await (await import('@/i18n')).i18nReady;

const { helperApi } = await import('@/ipc/api-client');
const { toast } = await import('@/lib/error-handler');
const { default: Screen } = await import('./SettingsHelper');
const absent = { supported: true, installed: false, ready: false, upgradeable: false, needsRepair: false };
const installed = { ...absent, installed: true, ready: true };
function render() { h.stateIndex = h.refIndex = 0; return Screen(); }
function nodes(node: unknown): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!isValidElement<Record<string, unknown>>(node)) return [];
  return [node, ...nodes(node.props.children), ...nodes(node.props.actions)];
}
function text(node: unknown): string {
  if (Array.isArray(node)) return node.map(text).join('');
  if (isValidElement<Record<string, unknown>>(node)) return text(node.props.children);
  return typeof node === 'string' ? node : '';
}
function click(label: string) {
  const button = nodes(render()).find((n) => typeof n.props.onClick === 'function' && text(n.props.children) === label);
  expect(button, label).toBeDefined();
  expect(button!.props.disabled).toBe(false);
  (button!.props.onClick as () => void)();
}
async function mounted(status = installed) {
  vi.mocked(helperApi.getStatus).mockResolvedValue(status as never);
  render(); h.cleanup = h.effect!();
  await vi.waitFor(() => expect(h.state[1]).toBe(status.installed ? 'installed' : 'none'));
}
beforeEach(() => { h.state = []; h.refs = []; h.effect = h.refresh = null; clearHelperInstallFeedback(); vi.clearAllMocks(); });
it('partial plus installed:false preserves helper-only cleanup retry through status refresh and success', async () => {
  await mounted();
  vi.mocked(helperApi.uninstall).mockResolvedValueOnce({ success: false, errorCode: 'partialCleanup', status: absent } as never);
  click('helper.uninstall');
  await vi.waitFor(() => expect(h.state[1]).toBe('cleanup-pending'));
  expect(toast.error).toHaveBeenCalledWith('helper.uninstallFail', 'helper.actionError.partialCleanup');
  vi.mocked(helperApi.getStatus).mockResolvedValue(absent as never);
  click('helper.recheck');
  await vi.waitFor(() => expect(h.state[2]).toBe(false));
  expect(h.state[1]).toBe('cleanup-pending');
  h.refresh!(); await Promise.resolve();
  expect(h.state[1]).toBe('cleanup-pending');
  vi.mocked(helperApi.uninstall).mockResolvedValueOnce({ success: true, status: absent } as never);
  click('helper.uninstall');
  await vi.waitFor(() => expect(h.state[1]).toBe('none'));
  expect(helperApi.uninstall).toHaveBeenCalledTimes(2);
  expect(helperApi.install).not.toHaveBeenCalled();
});
it('cancelled cleanup retry does not erase the pending recovery action', async () => {
  await mounted();
  vi.mocked(helperApi.uninstall).mockResolvedValueOnce({ success: false, errorCode: 'partialCleanup', status: absent } as never);
  click('helper.uninstall'); await vi.waitFor(() => expect(h.state[1]).toBe('cleanup-pending'));
  vi.mocked(helperApi.uninstall).mockResolvedValueOnce({ success: false, errorCode: 'cancelled', status: absent } as never);
  click('helper.uninstall'); await vi.waitFor(() => expect(h.state[2]).toBe(false));
  expect(h.state[1]).toBe('cleanup-pending');
});
it('reopened helper page retains a cleanup action when service status says absent', async () => {
  await mounted(absent);
  vi.mocked(helperApi.uninstall).mockResolvedValue({ success: true, status: absent } as never);
  click('helper.uninstall'); await vi.waitFor(() => expect(helperApi.uninstall).toHaveBeenCalledTimes(1));
  expect(helperApi.install).not.toHaveBeenCalled();
});

it.each([
  ['none', absent],
  ['needs-repair', { ...absent, needsRepair: true }],
  ['needs-upgrade', { ...absent, upgradeable: true }],
  ['installed', installed],
  ['needs-btm', { ...installed, backgroundDisabled: true }],
] as const)('failed install in %s exposes exactly one status-only recovery action', async (expectedState, resultStatus) => {
  await mounted(absent);
  // The terminal subscriber now performs an authoritative, read-only refresh.
  vi.mocked(helperApi.getStatus).mockResolvedValue(resultStatus as never);
  vi.mocked(helperApi.install).mockResolvedValueOnce({ success: false, errorCode: 'installerFailed', status: resultStatus } as never);
  click('helper.installAction');
  await vi.waitFor(() => expect(h.state[3]).toBe('installerFailed'));
  expect(h.state[1]).toBe(expectedState);
  const rechecks = nodes(render()).filter(n => typeof n.props.onClick === 'function' && text(n.props.children) === 'helper.recheck');
  expect(rechecks).toHaveLength(1);
  vi.mocked(helperApi.getStatus).mockResolvedValue(resultStatus as never);
  click('helper.recheck');
  await vi.waitFor(() => expect(h.state[2]).toBe(false));
  expect(helperApi.getStatus).toHaveBeenLastCalledWith(true);
  expect(helperApi.install).toHaveBeenCalledTimes(1);
  expect(helperApi.uninstall).not.toHaveBeenCalled();
  expect(h.state[3]).toBe('installerFailed');
});
it('failed status recovery retains the install cause and never retries installation', async () => {
  await mounted(absent);
  vi.mocked(helperApi.install).mockResolvedValueOnce({ success: false, errorCode: 'installerFailed', status: absent } as never);
  click('helper.installAction');
  await vi.waitFor(() => expect(h.state[3]).toBe('installerFailed'));
  vi.mocked(helperApi.getStatus).mockRejectedValueOnce(new Error('synthetic status failure'));
  click('helper.recheck');
  await vi.waitFor(() => expect(h.state[2]).toBe(false));
  expect(h.state[3]).toBe('installerFailed');
  expect(h.state[1]).toBe('none');
  expect(toast.error).toHaveBeenLastCalledWith('helper.statusCheckFail', 'helper.actionError.failed');
  expect(helperApi.install).toHaveBeenCalledTimes(1);
  expect(helperApi.uninstall).not.toHaveBeenCalled();
});

afterEach(() => { h.cleanup?.(); h.cleanup = null; clearHelperInstallFeedback(); });
