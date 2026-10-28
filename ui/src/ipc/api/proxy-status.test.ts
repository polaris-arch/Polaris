import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
const transport = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: transport }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const { proxyApi } = await import('./proxy');
const { IpcError } = await import('../ipc-client');

describe('shared desktop/mobile public status error envelope', () => {
  beforeEach(() => { transport.mockReset(); vi.stubGlobal('window', { __TAURI_INTERNALS__: {} }); });
  afterEach(() => vi.unstubAllGlobals());
  it.each(['PROXY_STATUS_WRITER_BUSY', 'PROXY_STATUS_CACHE_BUSY',
    'PROXY_STATUS_WRITER_POISONED', 'PROXY_STATUS_CACHE_POISONED',
    'PROXY_STATUS_CACHE_COLD', 'PROXY_STATUS_UNAVAILABLE'])('keeps %s rejected with no fabricated status or replay', async code => {
    // Exact ApiResponse shape is also serialized by the real Rust command test.
    transport.mockResolvedValueOnce({ success:false, error:code, code });
    const error = await proxyApi.getStatus().catch(error => error);
    expect(error).toBeInstanceOf(IpcError);
    expect(error.code).toBe(code);
    expect(error.command).toBe(IPC_CHANNELS.PROXY_GET_STATUS);
    expect(error.data).toBeUndefined();
    expect(transport).toHaveBeenCalledTimes(1);
    expect(transport).toHaveBeenCalledWith(IPC_CHANNELS.PROXY_GET_STATUS, {});
  });
  it('preserves complete successful current status and transport failure separately', async () => {
    const status = { running:true, mainGeneration:7, pid:123,
      ownedStopContext:{selectedServerId:'node-a',selectionIntentGeneration:2} };
    transport.mockResolvedValueOnce({success:true,data:status});
    expect(await proxyApi.getStatus()).toEqual(status);
    const failure = new Error('synthetic transport failure');
    transport.mockRejectedValueOnce(failure);
    await expect(proxyApi.getStatus()).rejects.toBe(failure);
    expect(transport).toHaveBeenCalledTimes(2);
  });
});
