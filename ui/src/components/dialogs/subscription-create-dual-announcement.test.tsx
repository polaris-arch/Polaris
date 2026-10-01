import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SubscriptionCreateSnapshot } from '@/contracts/subscription-create-operation';

const harness = vi.hoisted(() => ({
  surface: '' as 'form' | 'task' | '',
  stateIndex: 0,
  effects: {} as Record<'form' | 'task', (() => void) | undefined>,
  refreshes: [] as Array<() => void>,
  instances: new Set<string>(),
  published: false,
  closeInstance: vi.fn<(instanceId: string) => void>(),
}));

vi.mock('react', async (importOriginal) => {
  const real = await importOriginal<typeof import('react')>();
  return {
    ...real,
    useState: (initial: unknown) => {
      const index = harness.stateIndex++;
      // The form's operation id is normally set by start(); seed that local state so its real
      // terminal effect and the recovery dialog can observe the same backend operation.
      const value = harness.surface === 'form' && index === 0
        ? 'shared-operation'
        : typeof initial === 'function' ? (initial as () => unknown)() : initial;
      return [value, vi.fn()];
    },
    useRef: (initial: unknown) => ({ current: initial }),
    useEffect: (effect: () => void) => {
      if (harness.surface) harness.effects[harness.surface] = effect;
    },
  };
});

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/ipc', () => ({ api: { subscription: {} } }));
vi.mock('@/lib/error-handler', () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock('@/store/app-store', () => {
  const state = {
    loadConfig: () => new Promise<void>((resolve) => harness.refreshes.push(resolve)),
    get config() {
      return harness.published ? { subscriptions: [{ id: 'sub-1' }] } : null;
    },
  };
  return { useAppStore: Object.assign((selector: (value: typeof state) => unknown) => selector(state), {
    getState: () => state,
  }) };
});
vi.mock('./dialog-store', () => ({
  useDialogStore: (selector: (state: unknown) => unknown) => selector({
    open: vi.fn(),
    closeInstance: harness.closeInstance,
    hasInstance: (instanceId: string) => harness.instances.has(instanceId),
  }),
}));
vi.mock('@/store/subscription-create-operation-store', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/store/subscription-create-operation-store')>();
  const store = real.useSubscriptionCreateOperationStore;
  return {
    ...real,
    useSubscriptionCreateOperationStore: Object.assign(
      (selector: (state: ReturnType<typeof store.getState>) => unknown) => selector(store.getState()),
      { getState: store.getState, setState: store.setState },
    ),
  };
});

const { toast } = await import('@/lib/error-handler');
const { useSubscriptionCreateOperationStore } = await import('@/store/subscription-create-operation-store');
const { useSubscriptionCreateDialogOperation } = await import('./use-subscription-create-dialog-operation');
const { SubscriptionCreateTaskDialog } = await import('./SubscriptionCreateTaskDialog');

const completed: SubscriptionCreateSnapshot = {
  operationId: 'shared-operation',
  revision: 8,
  phase: 'succeeded',
  terminal: true,
  result: {
    subscription: { id: 'sub-1', name: 'Example', url: 'https://example.test/sub', autoUpdate: false, createdAt: '2026-09-29' },
    nodeCount: 1,
    addedServers: 1,
  },
};

beforeEach(() => {
  harness.surface = '';
  harness.stateIndex = 0;
  harness.effects = { form: undefined, task: undefined };
  harness.refreshes = [];
  harness.instances = new Set(['form-instance', 'task-instance']);
  harness.published = false;
  harness.closeInstance.mockReset();
  vi.mocked(toast.success).mockReset();
  vi.mocked(toast.error).mockReset();
  useSubscriptionCreateOperationStore.setState({ snapshots: {}, trackedOperationIds: [], handledTerminalRevisions: {} });
  const store = useSubscriptionCreateOperationStore.getState();
  store.track(completed.operationId);
  store.accept(completed);
});

describe('desktop subscription create terminal announcement across two mounted surfaces', () => {
  it.each([
    ['form', 'task'],
    ['task', 'form'],
  ] as const)('announces once when %s publication finishes before %s', async (first, second) => {
    harness.surface = 'form';
    harness.stateIndex = 0;
    useSubscriptionCreateDialogOperation({
      instanceId: 'form-instance', requestFormClose: vi.fn(), externalCloseLocked: false,
    });
    harness.surface = 'task';
    harness.stateIndex = 0;
    SubscriptionCreateTaskDialog({ instanceId: 'task-instance', operationId: completed.operationId });

    harness.effects.form?.();
    harness.effects.task?.();
    expect(harness.refreshes).toHaveLength(2);
    expect(toast.success).not.toHaveBeenCalled();

    harness.published = true;
    harness.refreshes[first === 'form' ? 0 : 1]();
    await vi.waitFor(() => expect(harness.closeInstance).toHaveBeenCalledWith(`${first}-instance`));
    harness.refreshes[second === 'form' ? 0 : 1]();
    await vi.waitFor(() => expect(harness.closeInstance).toHaveBeenCalledTimes(2));

    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(useSubscriptionCreateOperationStore.getState().trackedOperationIds).toEqual([]);
  });
});
