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


describe('native-only observation is distinct from original persistence recovery', () => {
  beforeEach(() => { transport.mockReset(); vi.stubGlobal('window', { __TAURI_INTERNALS__: {} }); });
  afterEach(() => vi.unstubAllGlobals());
  const native = { transactionId: 'native-A', runtimeGeneration: 7, selectionIntentGeneration: 9,
    retirementState: 'UNKNOWN', reason: { code: 'LISTENER_RETIREMENT_FAILED', path: 'inbounds' } };
  const pending = { ok: false, status: 'nativeCleanupUnknown', transactionId: 'native-A', runtimeGeneration: 7,
    selectionIntentGeneration: 9, coreState: 'committed', configurationDurable: false, nativeRetirements: [native] };

  it('native check alone sets the discriminator and preserves the exact nondurable original proof', async () => {
    transport.mockResolvedValueOnce({ success: true, data: pending });
    const result = await proxyApi.runtimeNativeRetirementStatus('native-A');
    expect(result).toEqual(pending);
    expect(result.nativeRetirements).toBe(pending.nativeRetirements);
    expect(result.configurationDurable).toBe(false);
    expect(transport).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.PROXY_RUNTIME_APPLY_STATUS,
      { transactionId: 'native-A', nativeRetirementOnly: true });
  });

  it('ordinary original D status remains flag-absent and can advance D without settling native UNKNOWN', async () => {
    const durable = { ...pending, configurationDurable: true };
    transport.mockResolvedValueOnce({ success: true, data: pending })
      .mockResolvedValueOnce({ success: true, data: durable });
    expect(await proxyApi.runtimeApplyStatus('native-A')).toEqual(pending);
    expect(await proxyApi.runtimeApplyStatus('native-A')).toEqual(durable);
    expect(transport.mock.calls).toEqual([
      [IPC_CHANNELS.PROXY_RUNTIME_APPLY_STATUS, { transactionId: 'native-A' }],
      [IPC_CHANNELS.PROXY_RUNTIME_APPLY_STATUS, { transactionId: 'native-A' }],
    ]);
    expect(durable.nativeRetirements).toEqual([native]);
  });

  it('HostOnly keeps its own selector and current owner beside older A debt without the native discriminator', async () => {
    const host = { ...pending, transactionId: undefined, hostExecutionId: 'host-B', selectionIntentGeneration: 10,
      coreState: 'unchanged', configurationDurable: true };
    transport.mockResolvedValueOnce({ success: true, data: host });
    const result = await proxyApi.runtimeHostEffectStatus('host-B');
    expect(result).toEqual(host);
    expect(result.nativeRetirements?.[0].selectionIntentGeneration).toBe(9);
    expect(result.selectionIntentGeneration).toBe(10);
    expect(transport).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.PROXY_RUNTIME_APPLY_STATUS, { hostExecutionId: 'host-B' });
  });

  it.each(['envelope rejection', 'transport rejection'] as const)('%s of native observation is not retried or sent through Apply', async route => {
    const failure = new Error('synthetic native-only transport failure');
    if (route === 'envelope rejection') {
      // Synthetic error envelope exercises the actual public IPC rejection path; Rust tests own its native selector classification.
      transport.mockResolvedValueOnce({ success: false, error: 'synthetic rejected reference', code: 'SYNTHETIC_REFERENCE_REJECTED' });
    } else transport.mockRejectedValueOnce(failure);
    const error = await proxyApi.runtimeNativeRetirementStatus('native-A').catch(error => error);
    if (route === 'envelope rejection') {
      expect(error).toBeInstanceOf(IpcError);
      expect(error.code).toBe('SYNTHETIC_REFERENCE_REJECTED');
    } else expect(error).toBe(failure);
    expect(transport).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.PROXY_RUNTIME_APPLY_STATUS,
      { transactionId: 'native-A', nativeRetirementOnly: true });
  });
});
