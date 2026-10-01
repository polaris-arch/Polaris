import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactElement } from 'react';
import type { BackupCategory } from '@/domain/backup-categories';
import type { ConfirmPayload } from './dialog-store';

// Vitest runs in node without a DOM. Call the component's real footer handler with
// controlled hook values, then exercise the real confirmation callback and IPC result.
const h = vi.hoisted(() => ({
  state: [] as unknown[],
  refs: [] as Array<{ current: string | boolean | null }>,
  stateIndex: 0,
  refIndex: 0,
  confirms: [] as Array<{ id: string; kind: string; payload: unknown }>,
  stack: [] as string[],
  closed: [] as string[],
  nextId: 1,
}));

vi.mock('react', async (importOriginal) => {
  const real = await importOriginal<typeof import('react')>();
  return {
    ...real,
    useState: (initial: unknown) => {
      const index = h.stateIndex++;
      if (h.state[index] === undefined) {
        h.state[index] = typeof initial === 'function' ? initial() : initial;
      }
      return [h.state[index], (value: unknown) => {
        h.state[index] = typeof value === 'function' ? value(h.state[index]) : value;
      }];
    },
    useRef: (initial: string | boolean | null) => h.refs[h.refIndex++] ?? { current: initial },
  };
});

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) =>
      vars?.categories ? `${key}: ${vars.categories}` : key,
  }),
}));
vi.mock('./dialog-store', () => ({
  useDialogStore: (selector: (s: unknown) => unknown) => selector({
    open: (desc: { kind: string; payload: unknown }) => {
      const id = `confirm-${h.nextId++}`;
      h.stack.push(id);
      h.confirms.push({ id, ...desc });
      return id;
    },
    closeInstance: (id: string) => {
      h.stack = h.stack.filter((entry) => entry !== id);
      h.closed.push(id);
    },
    hasInstance: (id: string) => h.stack.includes(id),
  }),
}));
vi.mock('@/ipc', () => ({ api: { backup: { importPick: vi.fn(), importApply: vi.fn() } } }));
vi.mock('@/lib/error-handler', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

const { api } = await import('@/ipc');
const { toast } = await import('@/lib/error-handler');
const { BackupImportDialog } = await import('./BackupImportDialog');

const picked = {
  filePath: '/tmp/backup.json',
  available: ['manualNodes', 'subscriptions', 'generalSettings'] as BackupCategory[],
  blockedCategories: [] as BackupCategory[],
  counts: { manualNodes: 2, subscriptions: 1, generalSettings: 1 },
  unavailableInterfaceBindings: {},
};

function restoreClick() {
  const modal = renderDialog();
  const [, button] = modal.props.footer.props.children;
  button.props.onClick();
}

function renderDialog() {
  h.stateIndex = 0;
  h.refIndex = 0;
  const modal = BackupImportDialog({ instanceId: 'backup-1' }) as ReactElement<{
    footer: ReactElement<{ children: ReactElement<{ onClick: () => void }>[] }>;
    onClose: () => void;
  }>;
  return modal;
}

function confirmation(): ConfirmPayload {
  const last = h.confirms[h.confirms.length - 1];
  expect(last?.kind).toBe('confirm');
  return last!.payload as ConfirmPayload;
}

beforeEach(() => {
  h.state = [picked, new Set<BackupCategory>(['subscriptions', 'manualNodes']), false];
  h.refs = [{ current: null }, { current: false }];
  h.confirms = [];
  h.stack = ['backup-1'];
  h.closed = [];
  h.nextId = 1;
  vi.clearAllMocks();
});

describe('BackupImportDialog restore feedback', () => {
  it('asks about the selected categories and overwrite before applying; cancel preserves the preview', () => {
    restoreClick();
    restoreClick();
    expect(h.confirms).toHaveLength(1);
    expect(api.backup.importApply).not.toHaveBeenCalled();
    const first = confirmation();
    expect(first.title).toBe('backupImport.confirmTitle');
    expect(first.message).toContain('settings.advanced.backup.manualNodes');
    expect(first.message).toContain('settings.backup.catSubscriptions');
    expect(first.message).not.toContain('settings.advanced.backup.generalSettings');
    expect(first.message).toContain('backupImport.confirmMsg');
    first.onCancel?.();
    expect(api.backup.importApply).not.toHaveBeenCalled();
    expect(h.state[0]).toBe(picked);
    h.stack = h.stack.filter((id) => id !== h.confirms[0].id); // ConfirmDialog closes before onCancel
    restoreClick();
    expect(h.confirms).toHaveLength(2);
  });

  it('retains the separate discard-preview confirmation', () => {
    renderDialog().props.onClose();
    expect(confirmation().title).toBe('backupImport.discardTitle');
    expect(api.backup.importApply).not.toHaveBeenCalled();
  });

  it('ignores an old confirmation after cancel and permits a new one', async () => {
    vi.mocked(api.backup.importApply).mockResolvedValue({ success: true });
    restoreClick();
    const old = confirmation();
    h.stack = h.stack.filter((id) => id !== 'confirm-1');
    old.onCancel?.();
    restoreClick();
    await old.onConfirm();
    expect(api.backup.importApply).not.toHaveBeenCalled();
    await confirmation().onConfirm();
    expect(api.backup.importApply).toHaveBeenCalledTimes(1);
  });

  it('does not submit when its backup dialog disappeared before confirmation', async () => {
    restoreClick();
    h.stack = h.stack.filter((id) => id !== 'backup-1');
    await confirmation().onConfirm();
    expect(api.backup.importApply).not.toHaveBeenCalled();
    expect(h.closed).toEqual(['confirm-1']);
  });

  it('applies the captured selection once and gives a short success toast only after success', async () => {
    vi.mocked(api.backup.importApply).mockResolvedValue({ success: true });
    restoreClick();
    const confirm = confirmation();
    const first = confirm.onConfirm();
    const duplicate = confirm.onConfirm();
    await Promise.all([first, duplicate]);
    expect(api.backup.importApply).toHaveBeenCalledTimes(1);
    expect(api.backup.importApply).toHaveBeenCalledWith('/tmp/backup.json', [
      'manualNodes', 'subscriptions',
    ]);
    expect(toast.success).toHaveBeenCalledExactlyOnceWith('backupImport.restoreSuccess');
    expect(toast.warning).not.toHaveBeenCalled();
    expect(h.closed).toEqual(['confirm-1', 'backup-1']);
  });

  it('keeps the existing network interface fallback warning instead of claiming normal success', async () => {
    vi.mocked(api.backup.importApply).mockResolvedValue({
      success: true,
      unavailableInterfaceBindings: 2,
    });
    restoreClick();
    await confirmation().onConfirm();
    expect(toast.warning).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it('closes only its own backup if another dialog opens while the restore is pending', async () => {
    let resolve!: (value: { success: boolean }) => void;
    vi.mocked(api.backup.importApply).mockReturnValue(new Promise((done) => { resolve = done; }));
    restoreClick();
    const pending = confirmation().onConfirm();
    h.stack.push('other-dialog');
    resolve({ success: true });
    await pending;
    expect(h.closed).toEqual(['confirm-1', 'backup-1']);
    expect(h.stack).toEqual(['other-dialog']);
  });

  it('does not publish a late result after the backup dialog disappears', async () => {
    let resolve!: (value: { success: boolean }) => void;
    vi.mocked(api.backup.importApply).mockReturnValue(new Promise((done) => { resolve = done; }));
    restoreClick();
    const pending = confirmation().onConfirm();
    h.stack = ['other-dialog'];
    resolve({ success: true });
    await pending;
    expect(toast.success).not.toHaveBeenCalled();
    expect(h.closed).toEqual(['confirm-1']);
    expect(h.stack).toEqual(['other-dialog']);
  });

  it.each([
    ['backend failure', { success: false, errorCode: 'saveFailed' as const }],
    ['transport failure', new Error('transport')],
  ])('reports %s without success and retains the preview', async (_name, result) => {
    if (result instanceof Error) vi.mocked(api.backup.importApply).mockRejectedValue(result);
    else vi.mocked(api.backup.importApply).mockResolvedValue(result);
    restoreClick();
    await confirmation().onConfirm();
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
    expect(h.closed).toEqual(['confirm-1']);
    expect(h.stack).toEqual(['backup-1']);
    expect(h.state[0]).toBe(picked);
  });
});
