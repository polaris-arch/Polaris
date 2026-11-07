import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PendingApplyResult } from '@/contracts/types';
import { runtimeHostPollDisposition, runtimeNativePollDisposition, startRuntimeExecutionPolling, runtimeApplyPollDisposition, startRuntimeApplyPolling, RUNTIME_APPLY_POLL_INTERVAL_MS } from './runtime-apply-polling';

afterEach(() => vi.useRealTimers());
const receipt = (status: PendingApplyResult['status'], transactionId = 'original'): PendingApplyResult => ({ok:true,status,transactionId});

describe('transaction-bound status-only polling', () => {
  it.each(['conflict', 'rejected', 'skipped'] as const)('stops on matching ownerless %s without clearing as success', async (status) => {
    vi.useFakeTimers();
    const read = vi.fn().mockResolvedValue(receipt(status));
    const accept = vi.fn((result: PendingApplyResult) =>
      runtimeApplyPollDisposition(result, 'original', 7, 9) === 'failure');
    startRuntimeApplyPolling({ transactionId: 'original', read, current: () => true, accept });
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 3);
    expect(read).toHaveBeenCalledTimes(1);
    expect(accept).toHaveReturnedWith(true);
    expect(runtimeApplyPollDisposition(receipt(status, 'other'), 'original', 7, 9)).toBe('ignore');
  });
  it.each(['hotApplied', 'hotDurabilityPending', 'unconfirmed'] as const)('requires owner evidence for %s', (status) => {
    expect(runtimeApplyPollDisposition(receipt(status), 'original', 7, 9)).toBe('ignore');
    expect(runtimeApplyPollDisposition(receipt(status), 'original', undefined, undefined)).toBe('ignore');
    const owned = { ...receipt(status), runtimeGeneration: 7, selectionIntentGeneration: 9 };
    expect(runtimeApplyPollDisposition(owned, 'original', 7, 10)).toBe('ignore');
    expect(runtimeApplyPollDisposition(owned, 'original', 7, 9)).toBe('owned');
  });
  it('serializes reads, retains ambiguity, and stops only on accepted authoritative result', async () => {
    vi.useFakeTimers();
    const read = vi.fn().mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce(receipt('hotApplied', 'other'))
      .mockResolvedValueOnce(receipt('unconfirmed'))
      .mockResolvedValueOnce(receipt('hotApplied'));
    const accept = vi.fn((result: PendingApplyResult) => result.status === 'hotApplied');
    const cancel = startRuntimeApplyPolling({transactionId:'original',read,current:()=>true,accept});
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 5);
    expect(read).toHaveBeenCalledTimes(4);
    expect(read.mock.calls.every(([id]) => id === 'original')).toBe(true);
    expect(accept.mock.calls.map(([result]) => result.status)).toEqual(['unconfirmed','hotApplied']);
    cancel();
  });
  it('retains read failures and ignores a host-only receipt with the same opaque id as its Core transaction', async () => {
    vi.useFakeTimers();
    const read = vi.fn().mockRejectedValueOnce(new Error('RUNTIME_APPLY_CORE_REFERENCE_REQUIRED'))
      .mockResolvedValueOnce({ ok: true, status: 'hostEffectsFailed', hostExecutionId: 'original',
        runtimeGeneration: 7, selectionIntentGeneration: 9 })
      .mockResolvedValueOnce(receipt('hotApplied'));
    const accept = vi.fn(() => true);
    const cancel = startRuntimeApplyPolling({ transactionId: 'original', read, current: () => true, accept });
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 4);
    expect(read).toHaveBeenCalledTimes(3);
    expect(read.mock.calls.every(([id]) => id === 'original')).toBe(true);
    expect(accept).toHaveBeenCalledExactlyOnceWith(receipt('hotApplied'));
    cancel();
  });
  it('does not overlap a stalled read and ignores its result after unmount/cancel', async () => {
    vi.useFakeTimers();
    let release!: (result: PendingApplyResult) => void;
    const read = vi.fn(() => new Promise<PendingApplyResult>(resolve => {release=resolve;}));
    const accept = vi.fn(() => true);
    const cancel = startRuntimeApplyPolling({transactionId:'original',read,current:()=>true,accept});
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 10);
    expect(read).toHaveBeenCalledTimes(1);
    cancel(); release(receipt('hotApplied'));
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 2);
    expect(accept).not.toHaveBeenCalled();
    expect(read).toHaveBeenCalledTimes(1);
  });
  it('discards late read when a newer attempt, generation, intent or stop supersedes it', async () => {
    vi.useFakeTimers();
    let valid = true;
    let release!: (result: PendingApplyResult) => void;
    const read = vi.fn(() => new Promise<PendingApplyResult>(resolve => {release=resolve;}));
    const accept = vi.fn(() => true);
    const cancel = startRuntimeApplyPolling({transactionId:'original',read,current:()=>valid,accept});
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS);
    valid = false; release(receipt('hotApplied'));
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 2);
    expect(accept).not.toHaveBeenCalled();
    expect(read).toHaveBeenCalledTimes(1);
    cancel();
  });
});


describe('typed independent host execution polling', () => {
  const hostReceipt = (status: PendingApplyResult['status']): PendingApplyResult => ({
    ok: true, status, hostExecutionId: 'host-only', runtimeGeneration: 7, selectionIntentGeneration: 9,
  });

  it.each(['hostEffectsPending', 'hostEffectsFailed', 'hotApplied', 'conflict', 'rejected'] as const)(
    'requires exact host execution, generation and intent even for %s', status => {
      const result = hostReceipt(status);
      expect(result.transactionId).toBeUndefined();
      expect(runtimeHostPollDisposition(result, 'host-only', 7, 9)).toBe('owned');
      expect(runtimeHostPollDisposition(result, 'other-host', 7, 9)).toBe('ignore');
      expect(runtimeHostPollDisposition(result, 'host-only', 8, 9)).toBe('ignore');
      expect(runtimeHostPollDisposition(result, 'host-only', 7, 10)).toBe('ignore');
      expect(runtimeHostPollDisposition({ ...result, runtimeGeneration: undefined }, 'host-only', 7, 9)).toBe('ignore');
      expect(runtimeHostPollDisposition({ ...result, selectionIntentGeneration: undefined }, 'host-only', 7, 9)).toBe('ignore');
    });

  it('reads a HostOnly selector without manufacturing a Core transaction and stops on its accepted receipt', async () => {
    vi.useFakeTimers();
    const read = vi.fn().mockResolvedValueOnce({ ...hostReceipt('hotApplied'), hostExecutionId: 'older' })
      .mockResolvedValueOnce(hostReceipt('hostEffectsPending')).mockResolvedValueOnce(hostReceipt('hotApplied'));
    const accept = vi.fn((result: PendingApplyResult) => result.status === 'hotApplied');
    const cancel = startRuntimeExecutionPolling({ execution: { kind: 'host', hostExecutionId: 'host-only' },
      read, current: () => true, accept });
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 5);
    expect(read.mock.calls).toEqual(Array.from({ length: 3 }, () => [{ kind: 'host', hostExecutionId: 'host-only' }]));
    expect(accept.mock.calls.map(([result]) => result.status)).toEqual(['hostEffectsPending', 'hotApplied']);
    cancel();
  });

  it.each(['stop', 'generation', 'intent', 'execution'] as const)('a stalled host read is single-inflight and cannot land after %s changes', async () => {
    vi.useFakeTimers();
    let current = true;
    let finish!: (result: PendingApplyResult) => void;
    const read = vi.fn(() => new Promise<PendingApplyResult>(resolve => { finish = resolve; }));
    const accept = vi.fn(() => true);
    const cancel = startRuntimeExecutionPolling({ execution: { kind: 'host', hostExecutionId: 'host-only' },
      read, current: () => current, accept });
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 8);
    expect(read).toHaveBeenCalledTimes(1);
    current = false;
    finish(hostReceipt('hotApplied'));
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 3);
    expect(accept).not.toHaveBeenCalled();
    expect(read).toHaveBeenCalledTimes(1);
    cancel();
  });

  it('lets independent Core durability polling finish while host polling reports a failure', async () => {
    vi.useFakeTimers();
    const coreRead = vi.fn().mockResolvedValue(receipt('hotApplied'));
    const hostRead = vi.fn().mockResolvedValue(hostReceipt('hostEffectsFailed'));
    const coreAccept = vi.fn(() => true), hostAccept = vi.fn(() => true);
    const stopCore = startRuntimeExecutionPolling({ execution: { kind: 'core', transactionId: 'original' },
      read: coreRead, current: () => true, accept: coreAccept });
    const stopHost = startRuntimeExecutionPolling({ execution: { kind: 'host', hostExecutionId: 'host-only' },
      read: hostRead, current: () => true, accept: hostAccept });
    await vi.advanceTimersByTimeAsync(RUNTIME_APPLY_POLL_INTERVAL_MS * 2);
    expect(coreRead).toHaveBeenCalledExactlyOnceWith({ kind: 'core', transactionId: 'original' });
    expect(hostRead).toHaveBeenCalledExactlyOnceWith({ kind: 'host', hostExecutionId: 'host-only' });
    expect(coreAccept).toHaveBeenCalledExactlyOnceWith(receipt('hotApplied'));
    expect(hostAccept).toHaveBeenCalledExactlyOnceWith(hostReceipt('hostEffectsFailed'));
    stopCore(); stopHost();
  });
});


describe('original native retirement status selector', () => {
  const native = { transactionId: 'native-A', runtimeGeneration: 7, selectionIntentGeneration: 9,
    retirementState: 'UNKNOWN' as const, reason: { code: 'LISTENER_RETIREMENT_FAILED' as const, path: 'inbounds' as const } };
  it.each(['nativeCleanupUnknown', 'hotApplied', 'conflict', 'rejected'] as const)('requires the exact original tuple even for %s', status => {
    const result = { ok: false, status, transactionId: 'native-A', runtimeGeneration: 7, selectionIntentGeneration: 9 };
    expect(runtimeNativePollDisposition(result, native)).toBe('owned');
    for (const other of [
      { transactionId: 'host-B' }, { transactionId: undefined, hostExecutionId: 'native-A' },
      { runtimeGeneration: 8 }, { selectionIntentGeneration: 10 }, { selectionIntentGeneration: undefined },
    ]) expect(runtimeNativePollDisposition({ ...result, ...other }, native)).toBe('ignore');
  });
});
