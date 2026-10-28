import { invoke } from '@/ipc/ipc-client';
import { IPC_CHANNELS } from './ipc-channels';

/** Only failures for which the existing Helper page offers a relevant recovery action. */
export function needsHelperRecovery(code: unknown): boolean {
  return code === 'HELPER_CORE_MISMATCH' || code === 'HELPER_CORE_ABSENT' ||
    code === 'HELPER_CORE_INVALID';
}

/** Reveal/recreate the main window and reuse its Helper page. Never install or elevate here.
 * The native command persists the intent across hidden, destroyed and booting renderers.
 * No main-window stores are imported: this module also runs in the independent tray webview.
 */
export async function revealHelperRecovery(): Promise<boolean> {
  try {
    await invoke(IPC_CHANNELS.TRAY_SHOW_MAIN, { screen: 'settings-helper' });
    return true;
  } catch (error) {
    console.error('[helper-recovery] could not reveal Helper settings:', error);
    return false;
  }
}
