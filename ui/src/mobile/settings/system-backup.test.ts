/**
 * 「系统备份」开关（设置 → 备份，Android Auto Backup 的运行期闸门，默认关）的前端写/读路径。
 *
 * 行为层：`systemBackupApi` 真的打到 `system_backup_set` / `system_backup_get_status`，参数袋是
 * `{ enabled }`（Rust 形参名）；后端 `success:false` 一律 reject —— 读侧不许折成 `false`，写侧不许
 * 静默成功。再用本屏真正的 `createCommit` 串一遍页面那条写链：执行侧拒绝时开关状态不动、错误落在
 * `system-backup` 那一行。
 *
 * 页面源码形态（写链挂在 `commit('system-backup', …)` 上、只在备份页、不进 user config）由
 * `MobileSettings.test.tsx` ⑤c 按字面钉住；Kotlin 侧闸门由 `scripts/check-android-bridge.mjs` A14 钉住。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TFunction } from 'i18next';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import { createCommit, type WriteErrors } from './write-feedback';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

beforeEach(() => {
  invokeMock.mockReset();
  // ipc-client 只在 Tauri 宿主里真的发（`__TAURI_INTERNALS__ in window`），否则返 mock —— 那会让下面每条空跑绿。
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function loadApi() {
  return (await import('@/ipc/api/system')).systemBackupApi;
}

describe('systemBackupApi', () => {
  it('写：打 system_backup_set，参数袋 { enabled }（开、关两态）', async () => {
    invokeMock.mockResolvedValue({ success: true });
    const api = await loadApi();
    await api.set(true);
    await api.set(false);
    expect(IPC_CHANNELS.SYSTEM_BACKUP_SET).toBe('system_backup_set');
    expect(invokeMock.mock.calls).toEqual([
      ['system_backup_set', { enabled: true }],
      ['system_backup_set', { enabled: false }],
    ]);
  });

  it('写：后端拒绝 ⇒ reject（不静默成功）', async () => {
    invokeMock.mockResolvedValue({ success: false, error: '写系统备份开关失败：x' });
    const api = await loadApi();
    await expect(api.set(true)).rejects.toThrow('写系统备份开关失败');
  });

  it('读：打 system_backup_get_status，返回执行侧真值', async () => {
    invokeMock.mockResolvedValue({ success: true, data: true });
    const api = await loadApi();
    await expect(api.getStatus()).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith('system_backup_get_status', {});
    invokeMock.mockResolvedValue({ success: true, data: false });
    await expect(api.getStatus()).resolves.toBe(false);
  });

  it('读：后端读不到 ⇒ reject，不折成 false', async () => {
    invokeMock.mockResolvedValue({ success: false, error: '读系统备份开关失败：x' });
    const api = await loadApi();
    await expect(api.getStatus()).rejects.toThrow('读系统备份开关失败');
  });
});

describe('页面写链（与 BackupPage 同形：commit(\'system-backup\', set(v).then(() => setEnabled(v)))）', () => {
  function harness() {
    let errors: WriteErrors = {};
    const t = ((key: string) => key) as unknown as TFunction;
    const commit = createCommit((updater) => {
      errors = updater(errors);
    }, t);
    return { commit, errors: () => errors };
  }
  const flush = () => new Promise((r) => setTimeout(r, 0));

  it('执行侧成功 ⇒ 开关状态跟着变、该行无错误', async () => {
    invokeMock.mockResolvedValue({ success: true });
    const api = await loadApi();
    const h = harness();
    let enabled: boolean | null = false;
    h.commit('system-backup', api.set(true).then(() => { enabled = true; }));
    await flush();
    expect(enabled).toBe(true);
    expect(h.errors()['system-backup']).toBeUndefined();
  });

  it('执行侧拒绝 ⇒ 开关状态不动、错误落在 system-backup 那一行', async () => {
    invokeMock.mockResolvedValue({ success: false, error: '写系统备份开关失败：磁盘满' });
    const api = await loadApi();
    const h = harness();
    let enabled: boolean | null = false;
    h.commit('system-backup', api.set(true).then(() => { enabled = true; }));
    await flush();
    expect(enabled).toBe(false);
    expect(h.errors()['system-backup']).toBeTruthy();
  });
});
