import { beforeEach, describe, expect, it, vi } from 'vitest';
import { isValidElement, type ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { CoreRecoveryState } from '@/contracts/types/update';

const h = vi.hoisted(() => ({
  result: null as CoreRecoveryState | null, busy: false,
  inFlight: { current: false }, armed: null as string | null,
  check: vi.fn(), restore: vi.fn(), recovered: vi.fn(), enterSettings: vi.fn(),
}));
vi.mock('react', async original => ({
  ...await original<typeof import('react')>(),
  useRef: () => h.inFlight,
  useState: (initial: unknown) => typeof initial === 'boolean'
    ? [h.busy, (value: boolean) => { h.busy = value; }]
    : [h.result, (value: CoreRecoveryState | null) => { h.result = value; }],
}));
vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock('@/ipc/api/updater', async original => ({
  ...await original<typeof import('@/ipc/api/updater')>(),
  coreApi: { checkOfflineRecovery: h.check, restoreBundledData: h.restore },
}));
vi.mock('@/store/nav-store', () => ({ useNavStore: { getState: () => ({ enterSettings: h.enterSettings }) } }));
vi.mock('@/lib/confirm-twice', () => ({
  useConfirmTwice: () => ({
    armed: h.armed,
    confirmTwice: (key: string, action: () => void) => {
      if (h.armed === key) { h.armed = null; action(); }
      else h.armed = key;
    },
  }),
}));
const { default: Actions } = await import('./CoreRecoveryActions');
function nodes(value: unknown): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(value)) return value.flatMap(nodes);
  if (!isValidElement<Record<string, unknown>>(value)) return [];
  return [value, ...nodes(value.props.children)];
}
const tree = () => Actions({ onRecovered: h.recovered });
function click(key: string) {
  const button = nodes(tree()).find(node => node.props.children === key && typeof node.props.onClick === 'function');
  expect(button, key).toBeDefined();
  (button!.props.onClick as () => void)();
}
const html = () => renderToStaticMarkup(tree());
beforeEach(() => {
  vi.resetAllMocks();
  h.result = null; h.busy = false; h.inFlight.current = false; h.armed = null;
});
describe('current-app offline recovery consumer', () => {
  it('offers only a cold check before the backend verifies a real recovery source', async () => {
    h.check.mockResolvedValue('ready');
    expect(html()).toContain('coreRecovery.check');
    expect(html()).not.toContain('coreRecovery.restore<');
    expect(h.check).not.toHaveBeenCalled(); expect(h.restore).not.toHaveBeenCalled();
    const handler = nodes(tree()).find(node => node.props.children === 'coreRecovery.check')!.props.onClick as () => void;
    handler(); handler();
    await vi.waitFor(() => expect(h.result).toBe('ready'));
    expect(h.check).toHaveBeenCalledExactlyOnceWith();
    expect(h.restore).not.toHaveBeenCalled();
    expect(html()).toContain('coreRecovery.restore');
  });

  it('restores only after Ready and explicit confirmation, never reconnects or installs Helper', async () => {
    h.result = 'ready'; h.restore.mockResolvedValue('restored');
    click('coreRecovery.restore');
    expect(h.restore).not.toHaveBeenCalled();
    click('coreRecovery.confirm');
    await vi.waitFor(() => expect(h.result).toBe('restored'));
    expect(h.restore).toHaveBeenCalledExactlyOnceWith();
    expect(h.recovered).toHaveBeenCalledOnce();
    expect(h.recovered).toHaveBeenCalledWith('restored');
    expect(h.enterSettings).not.toHaveBeenCalled();
    expect(html()).not.toContain('coreRecovery.restore<');
  });

  it.each(['reinstallRequired', 'pending', 'notNeeded', 'helperRequired'] as const)(
    'does not offer Reset for %s', state => {
      h.result = state;
      const markup = html();
      expect(markup).not.toContain('coreRecovery.restore<');
      expect(h.restore).not.toHaveBeenCalled();
      expect(h.enterSettings).not.toHaveBeenCalled();
    },
  );

  it('does not force Helper while checking an ordinary recovery source', async () => {
    h.check.mockResolvedValue('reinstallRequired');
    click('coreRecovery.check');
    await vi.waitFor(() => expect(h.result).toBe('reinstallRequired'));
    expect(html()).toContain('coreRecovery.reinstall');
    expect(h.enterSettings).not.toHaveBeenCalled();
    expect(h.restore).not.toHaveBeenCalled();
  });

  it('suppresses duplicate in-flight requests and removes a stale Ready after failure', async () => {
    let reject!: (reason: Error) => void;
    h.result = 'ready';
    h.restore.mockReturnValue(new Promise((_resolve, fail) => { reject = fail; }));
    click('coreRecovery.restore'); click('coreRecovery.confirm');
    expect(h.result).toBeNull(); expect(h.busy).toBe(true);
    expect(html()).not.toContain('coreRecovery.restore<');
    reject(new Error('private file path / policy / digest'));
    await vi.waitFor(() => expect(h.result).toBe('unavailable'));
    expect(h.restore).toHaveBeenCalledOnce();
    expect(html()).not.toContain('private file');
    expect(h.recovered).not.toHaveBeenCalled();
  });
});
