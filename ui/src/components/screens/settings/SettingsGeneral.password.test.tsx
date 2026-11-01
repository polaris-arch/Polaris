import { isValidElement, type ReactElement } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';

const h = vi.hoisted(() => ({ state: [] as unknown[], refs: [] as Array<{ current: unknown }>, si: 0, ri: 0,
  effects: [] as Array<() => void | (() => void)> }));
vi.mock('react', async (original) => ({
  ...await original<typeof import('react')>(),
  useState: (initial: unknown) => {
    const i = h.si++;
    if (!(i in h.state)) h.state[i] = initial;
    return [h.state[i], (value: unknown) => { h.state[i] = value; }];
  },
  useRef: (initial: unknown) => { const i = h.ri++; return h.refs[i] ?? (h.refs[i] = { current: initial }); },
  useEffect: (effect: () => void | (() => void)) => { h.effects.push(effect); },
}));
vi.mock('react-i18next', async (original) => ({ ...await original<typeof import('react-i18next')>(),
  useTranslation: () => ({ t: (key: string) => key }), }));
vi.mock('@/ipc/api-client', () => ({ autoStartApi: { getStatus: vi.fn() },
  privacyApi: { hasPassword: vi.fn(), setPassword: vi.fn() }, }));
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn() } }));
await (await import('@/i18n')).i18nReady;
const { autoStartApi, privacyApi } = await import('@/ipc/api-client');
const { default: SettingsGeneral } = await import('./SettingsGeneral');

function render() { h.si = h.ri = 0; h.effects = [];
  return SettingsGeneral({ config: { autoPrivacyMode: true } as never, update: vi.fn(async () => {}) }); }
function nodes(node: unknown): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!isValidElement<Record<string, unknown>>(node)) return [];
  return [node, ...nodes(node.props.children)];
}
function text(node: unknown): string {
  if (Array.isArray(node)) return node.map(text).join('');
  if (isValidElement<Record<string, unknown>>(node)) return text(node.props.children);
  return typeof node === 'string' ? node : '';
}
function passwordRow() { return nodes(render()).find(node => node.props.label === 'settings.general.privacyPassword')!; }
function action(label: string) { return nodes(passwordRow()).find(node => text(node.props.children) === label && typeof node.props.onClick === 'function')!; }
async function settle() { await Promise.resolve(); await Promise.resolve(); }
beforeEach(() => { h.state = []; h.refs = []; h.effects = []; vi.resetAllMocks(); vi.mocked(autoStartApi.getStatus).mockResolvedValue(false); });

it('unknown and failed password reads cannot open a clear/set editor; retry reads actual unset and permits it', async () => {
  let fail!: (error: Error) => void;
  vi.mocked(privacyApi.hasPassword).mockImplementationOnce(() => new Promise((_resolve, reject) => { fail = reject; }));
  render(); h.effects.forEach(effect => effect());
  expect(passwordRow().props.desc).toBe('settings.general.privacyPasswordUnknown');
  expect(action('settings.general.privacyPasswordSetBtn').props.disabled).toBe(true);
  (action('settings.general.privacyPasswordSetBtn').props.onClick as () => void)();
  expect(nodes(render()).some(node => node.props.type === 'password')).toBe(false);
  fail(new Error('read failed')); await settle();
  expect(passwordRow().props.desc).toBe('settings.general.privacyPasswordReadFailed');
  expect(action('settings.general.privacyPasswordSetBtn').props.disabled).toBe(true);
  expect(privacyApi.setPassword).not.toHaveBeenCalled();

  vi.mocked(privacyApi.hasPassword).mockResolvedValueOnce(false);
  (action('common.retry').props.onClick as () => void)();
  expect(passwordRow().props.desc).toBe('settings.general.privacyPasswordUnknown');
  await settle();
  expect(passwordRow().props.desc).toBe('settings.general.privacyPasswordUnset');
  expect(action('settings.general.privacyPasswordSetBtn').props.disabled).toBe(false);
  (action('settings.general.privacyPasswordSetBtn').props.onClick as () => void)();
  expect(nodes(render()).some(node => node.props.type === 'password')).toBe(true);
  expect(privacyApi.setPassword).not.toHaveBeenCalled();
});

it('an existing password is reported as set; a late read after cleanup cannot change unknown state', async () => {
  vi.mocked(privacyApi.hasPassword).mockResolvedValueOnce(true);
  render(); h.effects.forEach(effect => effect()); await settle();
  expect(passwordRow().props.desc).toBe('settings.general.privacyPasswordSet');
  expect(action('settings.general.privacyPasswordChange').props.disabled).toBe(false);

  h.state = []; h.refs = [];
  let done!: (value: boolean) => void;
  vi.mocked(privacyApi.hasPassword).mockImplementationOnce(() => new Promise(resolve => { done = resolve; }));
  render(); const cleanups = h.effects.map(effect => effect());
  cleanups.forEach(cleanup => { if (typeof cleanup === 'function') cleanup(); });
  done(true); await settle();
  expect(passwordRow().props.desc).toBe('settings.general.privacyPasswordUnknown');
});

it.each([undefined, null, 'false'])('a malformed password status %s remains unknown without authorizing writes', async (value) => {
  vi.mocked(privacyApi.hasPassword).mockResolvedValueOnce(value as never);
  render(); h.effects.forEach(effect => effect()); await settle();
  expect(passwordRow().props.desc).toBe('settings.general.privacyPasswordReadFailed');
  expect(action('settings.general.privacyPasswordSetBtn').props.disabled).toBe(true);
  expect(privacyApi.setPassword).not.toHaveBeenCalled();
});
