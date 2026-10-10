import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import { CoexSnapshotDecodeError } from '@/contracts/coex-snapshot';
import rustFixture from '@/contracts/coex-snapshot.rust.fixture.json';
const transport = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: transport }));
const { systemApi } = await import('./system');
const { IpcError } = await import('../ipc-client');
const unknown = { status: 'unknown', reason: 'fixture source unavailable' };
const snapshot = {
  schemaVersion: 1, platform: 'darwin', objects: unknown, observation: unknown, commandCleanup: unknown,
  context: { observationPhase: unknown, ownInterfaces: unknown, criteria: unknown, repairHistory: unknown }, classification: unknown,
};
describe('actual system wrapper through IPC envelope decoder', () => {
  beforeEach(() => { transport.mockReset(); vi.stubGlobal('window', { __TAURI_INTERNALS__: {} }); });
  afterEach(() => vi.unstubAllGlobals());
  it('invokes the registered no-argument command once and unwraps once', async () => {
    transport.mockResolvedValue({ success: true, data: snapshot });
    expect(await systemApi.coexReadonlySnapshot()).toEqual(snapshot);
    expect(transport).toHaveBeenCalledTimes(1);
    expect(transport).toHaveBeenCalledWith(IPC_CHANNELS.COEX_READONLY_SNAPSHOT, {});
  });
  it.each(Object.entries(rustFixture.snapshots))('decodes the Rust-generated %s through the real wrapper and envelope', async (_name, wire) => {
    transport.mockResolvedValueOnce({ success: true, data: wire });
    expect(await systemApi.coexReadonlySnapshot()).toEqual(wire);
    expect(transport).toHaveBeenCalledWith(IPC_CHANNELS.COEX_READONLY_SNAPSHOT, {});
  });
  it('propagates denied-window envelope and permits a later explicit retry', async () => {
    transport.mockResolvedValueOnce({ success: false, code: 'coex_window_denied', error: 'main window required' });
    await expect(systemApi.coexReadonlySnapshot()).rejects.toBeInstanceOf(IpcError);
    transport.mockResolvedValueOnce({ success: true, data: snapshot });
    expect(await systemApi.coexReadonlySnapshot()).toEqual(snapshot);
  });
  it('propagates transport failure without manufacturing an empty snapshot', async () => {
    const error = new Error('fixture transport unavailable'); transport.mockRejectedValueOnce(error);
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    try { await expect(systemApi.coexReadonlySnapshot()).rejects.toBe(error); } finally { spy.mockRestore(); }
  });
  it.each([{ success: true }, { success: true, data: { ...snapshot, schemaVersion: 2 } },
    { success: true, data: { ...snapshot, objects: [] } }, undefined])('fails closed on malformed returned data %j', async (raw) => {
    transport.mockResolvedValueOnce(raw);
    await expect(systemApi.coexReadonlySnapshot()).rejects.toBeInstanceOf(CoexSnapshotDecodeError);
  });
  it('fails closed on the non-Tauri browser fallback', async () => {
    vi.stubGlobal('window', {});
    await expect(systemApi.coexReadonlySnapshot()).rejects.toBeInstanceOf(CoexSnapshotDecodeError);
    expect(transport).not.toHaveBeenCalled();
  });
  it('retains macOS partial-source Unknown through the registered no-argument wrapper', async () => {
    transport.mockResolvedValueOnce({ success: true, data: rustFixture.snapshots.macosPartialSources });
    const wire = await systemApi.coexReadonlySnapshot();
    expect(wire.platform).toBe('darwin');
    if (wire.objects.status !== 'known') throw Error('fixture roster unavailable');
    expect(wire.objects.value[0].addresses.status).toBe('known');
    expect(wire.objects.value[0].routes.status).toBe('unknown');
    expect(wire.classification.status).toBe('unknown');
    expect(transport).toHaveBeenCalledTimes(1);
  });

});
