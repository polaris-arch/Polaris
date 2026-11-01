import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import type { TlsFingerprintPresentation } from '@/domain/utls';

const nativeInvoke = vi.hoisted(() => vi.fn<(...args: unknown[]) => Promise<unknown>>());
vi.mock('@tauri-apps/api/core', () => ({ invoke: nativeInvoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const { coreApi, updateApi } = await import('./updater');

beforeEach(() => {
  nativeInvoke.mockReset();
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
});
afterEach(() => { vi.unstubAllGlobals(); });

it('requests only capability presentation and preserves its distinct desktop or mobile evidence', async () => {
  const views: TlsFingerprintPresentation[] = [
    { evidenceKind: 'compiled-selected-support-v1', selectedSpec: null, document: null,
      unavailableReason: 'No reviewed directory for the expected compiled selection' },
    { observationScope: 'unavailable', byteIdentityScope: 'loaded-jni-so', identity: null,
      binarySha256: 'a'.repeat(64), document: null, unavailableReason: 'Loaded native report unavailable' },
  ];
  for (const view of views) {
    nativeInvoke.mockClear().mockResolvedValue({ success: true, data: { tlsFingerprints: view } });
    expect(await coreApi.getTlsFingerprints()).toEqual(view);
    expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.CORE_GET_VERSION_INFO, { capabilitiesOnly: true });
  }
});

it('keeps the legacy version probe outside capability presentation requests', async () => {
  const info = { packagedVersion: '', bundledVersion: 'fixture', patchSet: null };
  nativeInvoke.mockResolvedValue({ success: true, data: info });
  expect(await coreApi.getVersionInfo()).toEqual(info);
  expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.CORE_GET_VERSION_INFO, {});
});

it('treats persisted update skips as a void command and preserves save errors', async () => {
  nativeInvoke.mockResolvedValueOnce({ success: true });
  await expect(updateApi.skip('v1.2.3')).resolves.toBeUndefined();
  expect(nativeInvoke).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.UPDATE_SKIP, { version: 'v1.2.3' });

  nativeInvoke.mockResolvedValueOnce({ success: false, error: 'storage unavailable' });
  await expect(updateApi.skip('v1.2.3')).rejects.toThrow('storage unavailable');
});
