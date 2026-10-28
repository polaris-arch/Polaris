/**
 * A1：托盘「打开设置」的主窗侧消费点（窄跨窗导航通道）。
 *
 * 通道的值域由 Rust 白名单 `tray::normalize_tray_screen` 钉死，本侧再守一道：**未登记值一律不导航**。
 * 两道白名单不是重复——Rust 那道保证「发出去的只能是登记值」，这道保证「就算通道被别的东西污染
 * （或将来有人在前端另发一次），也不会跳到没预期的屏」。宁可不跳，也不跳错。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
const { invoke, privacy } = vi.hoisted(() => ({ invoke: vi.fn(), privacy: { privacyMode: false } }));
vi.mock('@/ipc/ipc-client', () => ({ invoke }));
vi.mock('@/store/app-store', () => ({ useAppStore: { getState: () => privacy } }));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
import { useDialogStore } from '@/components/dialogs/dialog-store';
import { HelperRecoveryDialog } from '@/components/dialogs/HelperRecoveryDialog';
import { useNavStore, applyTrayScreenIntent, drainTrayPendingScreen } from './nav-store';

beforeEach(() => {
  invoke.mockReset(); privacy.privacyMode = false;
  useDialogStore.getState().closeAll();
  useNavStore.setState({ scope: 'main', mainScreen: 'home', settingsScreen: 'general' });
});

describe('applyTrayScreenIntent', () => {
  it('helper recovery opens the existing Helper subpage', () => {
    expect(applyTrayScreenIntent('settings-helper')).toBe(true);
    expect(useNavStore.getState()).toMatchObject({ scope: 'settings', settingsScreen: 'helper' });
  });

  it("'settings' → 进设置 scope", () => {
    expect(applyTrayScreenIntent('settings')).toBe(true);
    expect(useNavStore.getState().scope).toBe('settings');
  });

  it('未登记值一律忽略，且不动导航态', () => {
    for (const evil of ['home', 'nodes', '/settings', 'Settings', '', null, undefined, 42, {}]) {
      expect(applyTrayScreenIntent(evil), `${String(evil)} 不该被放行`).toBe(false);
    }
    const s = useNavStore.getState();
    expect(s.scope).toBe('main');
    expect(s.mainScreen).toBe('home');
  });

  it('复用既有 enterSettings 语义（默认落 general 子页，不另造一套导航写法）', () => {
    useNavStore.setState({ settingsScreen: 'about' });
    applyTrayScreenIntent('settings');
    expect(useNavStore.getState().settingsScreen).toBe('general');
  });
});


describe('cancellable Helper recovery intent', () => {
  function prompt() {
    applyTrayScreenIntent('helper-recovery');
    const stack = useDialogStore.getState().stack;
    const entry = stack[stack.length - 1]!;
    return { entry, payload: HelperRecoveryDialog({ instanceId: entry.instanceId }).props.payload };
  }
  it('opens one explanation without navigation, install or native mutation across duplicate deliveries', () => {
    const first = prompt();
    applyTrayScreenIntent('helper-recovery'); applyTrayScreenIntent('helper-recovery');
    expect(useDialogStore.getState().stack).toEqual([first.entry]);
    expect(useNavStore.getState().scope).toBe('main');
    expect(invoke).not.toHaveBeenCalled();
  });
  it('only a live top owner can confirm; confirm navigates once and does not invoke native', () => {
    const { entry, payload } = prompt();
    const other = useDialogStore.getState().open({ kind: 'res-url' });
    payload.onConfirm(); expect(useNavStore.getState().scope).toBe('main');
    useDialogStore.getState().closeInstance(other);
    payload.onConfirm();
    expect(useNavStore.getState()).toMatchObject({ scope: 'settings', settingsScreen: 'helper' });
    expect(useDialogStore.getState().hasInstance(entry.instanceId)).toBe(false);
    useNavStore.getState().navigate('logs'); payload.onConfirm();
    expect(useNavStore.getState()).toMatchObject({ scope: 'main', mainScreen: 'logs' });
    expect(invoke).not.toHaveBeenCalled();
  });
  it('cancel/closeAll revoke captured callbacks and preserve a newer top dialog', () => {
    const { entry, payload } = prompt();
    useDialogStore.getState().closeInstance(entry.instanceId);
    const other = useDialogStore.getState().open({ kind: 'res-url' });
    payload.onConfirm();
    expect(useDialogStore.getState().stack.map(x => x.instanceId)).toEqual([other]);
    expect(useNavStore.getState().scope).toBe('main');
    useDialogStore.getState().closeAll();
    payload.onConfirm(); expect(invoke).not.toHaveBeenCalled();
  });
  it.each(['navigate', 'enterSettings', 'setSettingsScreen', 'backToApp'] as const)('navigation %s revokes the previous owner', action => {
    const { entry, payload } = prompt();
    const nav = useNavStore.getState();
    if (action === 'navigate') nav.navigate('logs');
    else if (action === 'enterSettings') nav.enterSettings('about');
    else if (action === 'setSettingsScreen') nav.setSettingsScreen('about');
    else nav.backToApp();
    expect(useDialogStore.getState().hasInstance(entry.instanceId)).toBe(false);
    const before = useNavStore.getState(); payload.onConfirm();
    expect(useNavStore.getState()).toBe(before);
  });
  it('privacy lock rejects captured confirmation without navigating or invoking native', () => {
    const { payload } = prompt(); privacy.privacyMode = true;
    payload.onConfirm();
    expect(useNavStore.getState().scope).toBe('main');
    expect(invoke).not.toHaveBeenCalled();
  });
  it('event then cancel makes an already in-flight bootstrap drain stale', async () => {
    let release!: (screen: string) => void;
    invoke.mockImplementationOnce(() => new Promise<string>(resolve => { release = resolve; }));
    const drain = drainTrayPendingScreen(true);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledOnce());
    const { entry } = prompt(); useDialogStore.getState().closeInstance(entry.instanceId);
    release('helper-recovery'); await drain;
    expect(useDialogStore.getState().stack).toEqual([]);
    expect(useNavStore.getState().scope).toBe('main');
  });
  it('bootstrap-only delivery opens once; clear-only pending delivery never reopens it', async () => {
    invoke.mockResolvedValue('helper-recovery');
    await drainTrayPendingScreen(true);
    expect(useDialogStore.getState().stack.map(x => x.kind)).toEqual(['helper-recovery']);
    useDialogStore.getState().closeAll();
    await drainTrayPendingScreen(false);
    expect(useDialogStore.getState().stack).toEqual([]);
  });
});
