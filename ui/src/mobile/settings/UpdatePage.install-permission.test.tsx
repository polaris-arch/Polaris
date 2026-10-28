import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { isValidElement, useEffect, type ReactElement, type ReactNode } from 'react';
import type { MobileSettingsPageProps } from './settings-pages';

// Run the real Install button handler in the existing node test environment.
const h = vi.hoisted(() => ({
  states: [] as unknown[],
  refs: [] as Array<{ current: unknown }>,
  stateIndex: 0,
  refIndex: 0,
  effectIndex: 0,
  effects: [] as Array<{ deps: readonly unknown[] | undefined; cleanup?: () => void }>,
  pendingEffects: [] as Array<() => void>,
  operations: [] as Promise<unknown>[],
}));
vi.mock('react', async (importOriginal) => {
  const real = await importOriginal<typeof import('react')>();
  return {
    ...real,
    useState: (initial: unknown) => {
      const index = h.stateIndex++;
      if (h.states[index] === undefined) h.states[index] = typeof initial === 'function' ? initial() : initial;
      return [h.states[index], (value: unknown) => {
        h.states[index] = typeof value === 'function' ? value(h.states[index]) : value;
      }];
    },
    useRef: (initial: unknown) => h.refs[h.refIndex++] ??= { current: initial },
    useEffect: (effect: () => (() => void) | void, deps?: readonly unknown[]) => {
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
    useSyncExternalStore: (_subscribe: unknown, snapshot: () => unknown) => snapshot(),
  };
});
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/ipc/api-client', () => ({
  updateApi: {
    requestInstallPermission: vi.fn(),
    install: vi.fn(),
    onProgress: () => () => {},
    getProgress: () => Promise.resolve(null),
  },
  versionApi: { getInfo: () => new Promise(() => {}) },
  systemApi: {},
}));

const { updateApi } = await import('@/ipc/api-client');
const { UpdatePage } = await import('./UpdatePage');

function elements(node: ReactNode): ReactElement<{ children?: ReactNode; control?: ReactNode; onClick?: () => void; disabled?: boolean }>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<{ children?: ReactNode; control?: ReactNode; onClick?: () => void; disabled?: boolean }>(node)) return [];
  return [node, ...elements(node.props.children), ...elements(node.props.control)];
}

function renderPage() {
  h.stateIndex = 0;
  h.refIndex = 0;
  h.effectIndex = 0;
  const tree = UpdatePage({
    config: { appUpdateChannel: 'stable' },
    update: vi.fn(),
    commit: (_row: string, operation: Promise<unknown>) => { h.operations.push(operation); },
  } as unknown as MobileSettingsPageProps);
  h.pendingEffects.splice(0).forEach((run) => run());
  return tree;
}

function unmount(): void {
  h.effects.forEach((slot) => slot.cleanup?.());
  h.effects = [];
  h.pendingEffects = [];
}

function installClick(): void {
  const tree = renderPage();
  const button = elements(tree).find((e) => e.props.children === 'mobileSettings.update.installAction');
  expect(button).toBeDefined();
  button!.props.onClick!();
}

beforeEach(() => {
  unmount();
  vi.clearAllMocks();
  h.states = [null, false, new Set(), {
    phase: 'downloaded', path: '/private/cache/polaris.apk', info: null,
    percentage: 100, received: 100, errorCode: null, integrity: 'verified',
  }];
  h.refs = [];
  h.stateIndex = 0;
  h.refIndex = 0;
  h.effectIndex = 0;
  h.effects = [];
  h.pendingEffects = [];
  h.operations = [];
});

afterEach(unmount);

describe('effect harness lifecycle', () => {
  it('flushes after render, compares dependencies, and cleans up before replacement and unmount', () => {
    const events: string[] = [];
    const render = (dependency: string): void => {
      h.effectIndex = 0;
      useEffect(() => { events.push(`setup:${dependency}`); return () => { events.push(`cleanup:${dependency}`); }; }, [dependency]);
      useEffect(() => { events.push('mount'); return () => { events.push('unmount'); }; }, []);
      useEffect(() => { events.push('every'); });
    };
    render('first');
    expect(events).toEqual([]);
    h.pendingEffects.splice(0).forEach((run) => run());
    expect(events).toEqual(['setup:first', 'mount', 'every']);
    render('first');
    h.pendingEffects.splice(0).forEach((run) => run());
    expect(events).toEqual(['setup:first', 'mount', 'every', 'every']);
    render('second');
    h.pendingEffects.splice(0).forEach((run) => run());
    expect(events.slice(-3)).toEqual(['cleanup:first', 'setup:second', 'every']);
    unmount();
    expect(events.slice(-2)).toEqual(['cleanup:second', 'unmount']);
  });
});

describe('Android Install button permission sequencing', () => {
  it('withdraws the old package intent when the downloaded package changes', async () => {
    let grant!: (value: { granted: boolean }) => void;
    vi.mocked(updateApi.requestInstallPermission).mockReturnValueOnce(new Promise((resolve) => { grant = resolve; }));
    vi.mocked(updateApi.install).mockResolvedValue({ ok: true, awaitingSystemInstaller: true });
    installClick();
    h.states[3] = { ...(h.states[3] as Record<string, unknown>), path: '/private/cache/new.apk' };
    renderPage(); // Commit the identity-change effect without a new install click.
    grant({ granted: true });
    await h.operations[0];
    expect(updateApi.requestInstallPermission).toHaveBeenCalledTimes(1);
    expect(updateApi.install).not.toHaveBeenCalled();
    expect(h.states[4]).toBe(false);
    expect(h.states[5]).toBe(false);
    vi.mocked(updateApi.requestInstallPermission).mockResolvedValue({ granted: true });
    installClick();
    await h.operations[1];
    expect(updateApi.requestInstallPermission).toHaveBeenCalledTimes(2);
    expect(updateApi.install).toHaveBeenCalledExactlyOnceWith('/private/cache/new.apk');
  });

  it('requests permission once, waits for it, and then hands off the captured package once', async () => {
    let grant!: (value: { granted: boolean }) => void;
    vi.mocked(updateApi.requestInstallPermission).mockReturnValue(new Promise((resolve) => { grant = resolve; }));
    vi.mocked(updateApi.install).mockResolvedValue({ ok: true, awaitingSystemInstaller: true });
    installClick();
    installClick();
    expect(updateApi.requestInstallPermission).toHaveBeenCalledTimes(1);
    expect(updateApi.install).not.toHaveBeenCalled();
    grant({ granted: true });
    await h.operations[0];
    expect(updateApi.install).toHaveBeenCalledExactlyOnceWith('/private/cache/polaris.apk');
    expect(h.states[4]).toBe(true);
    expect(h.states[5]).toBe(false);
  });

  it.each([
    { granted: false, reason: 'unknown-sources-denied' as const },
    { granted: true, reason: 'unknown-sources-denied' as const },
  ])('never installs after a denied or contradictory permission result: %j', async (permission) => {
    vi.mocked(updateApi.requestInstallPermission).mockResolvedValue(permission);
    installClick();
    await expect(h.operations[0]).rejects.toThrow('unknown-sources-denied');
    expect(updateApi.install).not.toHaveBeenCalled();
    expect(h.states[4]).toBe(false);
    expect(h.states[5]).toBe(false);
  });

  it('does not install after navigation even if the old permission request later succeeds', async () => {
    let grant!: (value: { granted: boolean }) => void;
    vi.mocked(updateApi.requestInstallPermission).mockReturnValue(new Promise((resolve) => { grant = resolve; }));
    installClick();
    unmount();
    grant({ granted: true });
    await h.operations[0];
    expect(updateApi.install).not.toHaveBeenCalled();
    expect(h.states[4]).toBe(false);
  });

  it('clears the busy guard after a permission failure so the user can retry', async () => {
    vi.mocked(updateApi.requestInstallPermission).mockRejectedValueOnce(new Error('timeout'));
    installClick();
    await expect(h.operations[0]).rejects.toThrow('timeout');
    expect(h.states[5]).toBe(false);
    vi.mocked(updateApi.requestInstallPermission).mockResolvedValue({ granted: true });
    vi.mocked(updateApi.install).mockResolvedValue({ ok: true, awaitingSystemInstaller: true });
    installClick();
    await h.operations[1];
    expect(updateApi.requestInstallPermission).toHaveBeenCalledTimes(2);
    expect(updateApi.install).toHaveBeenCalledTimes(1);
  });
});
