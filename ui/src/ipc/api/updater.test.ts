import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import type { TlsFingerprintPresentation } from '@/domain/utls';

const nativeInvoke = vi.hoisted(() => vi.fn<(...args: unknown[]) => Promise<unknown>>());
vi.mock('@tauri-apps/api/core', () => ({ invoke: nativeInvoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const { coreApi } = await import('./updater');

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
