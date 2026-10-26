import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PendingApplyResult } from '@/contracts/types';
import { runtimeApplyPollDisposition, startRuntimeApplyPolling, RUNTIME_APPLY_POLL_INTERVAL_MS } from './runtime-apply-polling';

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
