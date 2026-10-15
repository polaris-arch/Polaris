import { describe, expect, it, vi } from 'vitest';
import type { ExpectedMainCoreStop, ExpectedMainCoreStopResult } from '@/contracts/types';
import { captureMainCoreOwner, isMainCoreLogoutError, sameRunningCore, stopOwnedCore, stopOwnedCoreThenLogout, tsNodeHoldByRunningCore } from './ts-logout-flow';

const context = { selectedServerId: 'other-node', selectionIntentGeneration: 0 };
const running = { running: true, pid: 123, startTime: 456, mainGeneration: 7, ownedStopContext: context };
const stopped = { running: false, mainGeneration: 8, ownedStopContext: context };
const owner = { serverId: 'ts-a', selectedId: 'other-node', status: running };

function fixture() {
  let status = running;
  let selected: string | null = 'other-node';
  let present = true;
  const stop = vi.fn(async (_expected: ExpectedMainCoreStop): Promise<ExpectedMainCoreStopResult> => {
    status = stopped as typeof running;
    return { kind: 'stopped' as const, expectedMainGeneration: 7, stopMainGeneration: 8 };
  });
  const logout = vi.fn(async (_id: string) => {});
  return { stop, logout,
    setStatus: (next: typeof running) => { status = next; },
    setSelected: (next: string | null) => { selected = next; },
    setPresent: (next: boolean) => { present = next; },
    io: { selectedId: () => selected, serverPresent: () => present,
      status: async () => status, stopExpected: stop, logout },
  };
}

describe('mobile Tailscale logout with a main-core owner', () => {
  it('recognizes only the structured native ownership refusal', () => {
    expect(isMainCoreLogoutError({ code: 'TAILSCALE_LOGOUT_MAIN_CORE' })).toBe(true);
    expect(isMainCoreLogoutError({ message: 'MAIN_CORE' })).toBe(false);
    expect(isMainCoreLogoutError({ code: 'TAILSCALE_LOGOUT_FAILED' })).toBe(false);
  });

  it('stops the same running core and logs out the captured TS id once', async () => {
    const f = fixture();
    expect(await stopOwnedCoreThenLogout(owner, f.io)).toEqual({ kind: 'loggedOut' });
    expect(f.stop).toHaveBeenCalledExactlyOnceWith({ expectedMainGeneration: 7, expectedSelectedServerId: 'other-node', expectedSelectionIntentGeneration: 0 });
    expect(f.logout).toHaveBeenCalledExactlyOnceWith('ts-a');
  });

  it('refuses a changed selection, missing node, replaced core, or unknown core identity', async () => {
    for (const change of [
      (f: ReturnType<typeof fixture>) => f.setSelected('new-node'),
      (f: ReturnType<typeof fixture>) => f.setPresent(false),
      (f: ReturnType<typeof fixture>) => f.setStatus({ ...running, startTime: 789 }),
      (f: ReturnType<typeof fixture>) => f.setStatus(stopped as typeof running),
    ]) {
      const f = fixture(); change(f);
      expect(await stopOwnedCoreThenLogout(owner, f.io)).toEqual({ kind: 'changed' });
      expect(f.stop).not.toHaveBeenCalled();
      expect(f.logout).not.toHaveBeenCalled();
    }
    expect(sameRunningCore({ running: true }, { running: true })).toBe(false);
    // The native status normally includes startTime. A missing receipt must not fall back to
    // PID: the OS can reuse it for a replacement core after the first confirmation.
    expect(sameRunningCore({ running: true, pid: 123 }, { running: true, pid: 123 })).toBe(false);
    expect(sameRunningCore(running, { running: true, pid: 123 })).toBe(false);
  });

  it('rechecks the target node after an asynchronous status read before stopping the core', async () => {
    const f = fixture();
    const status = f.io.status;
    f.io.status = async () => {
      const result = await status();
      f.setPresent(false);
      return result;
    };
    expect(await stopOwnedCoreThenLogout(owner, f.io)).toEqual({ kind: 'changed' });
    expect(f.stop).not.toHaveBeenCalled();
    expect(f.logout).not.toHaveBeenCalled();
  });

  it('never logs out when stop fails, a new core appears, or the native writer still refuses', async () => {
    const failedStop = fixture();
    failedStop.stop.mockRejectedValueOnce(new Error('stop failed'));
    expect(await stopOwnedCoreThenLogout(owner, failedStop.io)).toEqual({ kind: 'stopFailed' });
    expect(failedStop.logout).not.toHaveBeenCalled();

    const restarted = fixture();
    restarted.stop.mockImplementationOnce(async () => {
      restarted.setStatus({ ...running, startTime: 789, mainGeneration: 9 });
      return { kind: 'stopped', expectedMainGeneration: 7, stopMainGeneration: 8 };
    });
    expect(await stopOwnedCoreThenLogout(owner, restarted.io)).toEqual({ kind: 'changed' });
    expect(restarted.logout).not.toHaveBeenCalled();

    const refused = fixture();
    refused.logout.mockRejectedValueOnce({ code: 'TAILSCALE_LOGOUT_MAIN_CORE' });
    expect(await stopOwnedCoreThenLogout(owner, refused.io)).toEqual({
      kind: 'logoutFailed', code: 'TAILSCALE_LOGOUT_MAIN_CORE',
    });
    expect(refused.stop).toHaveBeenCalledOnce();
    expect(refused.logout).toHaveBeenCalledOnce();
  });
});

describe('shared pre-notice helpers for a node held by the running core', () => {
  const status = (serverId: string) => ({ serverId });

  const live = (...ids: string[]) => ({ connected: true, statuses: ids.map(status) });

  it('skips the notice only on positive proof that the proxy does not hold the node', () => {
    // Certain: the proxy is neither running nor starting, whatever a late status read says.
    expect(tsNodeHoldByRunningCore(stopped, live('ts-a'), 'ts-a')).toBe('notHeld');
    expect(tsNodeHoldByRunningCore(stopped, null, 'ts-a')).toBe('notHeld');
    // Certain: this run's status frame has arrived and does not list the node.
    expect(tsNodeHoldByRunningCore(running, live('ts-b'), 'ts-a')).toBe('notHeld');
    expect(tsNodeHoldByRunningCore(null, live('ts-b'), 'ts-a')).toBe('notHeld');
  });

  it('a live frame that lists the node is a holder', () => {
    expect(tsNodeHoldByRunningCore(running, live('ts-b', 'ts-a'), 'ts-a')).toBe('held');
    expect(tsNodeHoldByRunningCore(null, live('ts-a'), 'ts-a')).toBe('held');
  });

  it('a failed read, a stream that is not live and a missing first frame are never "not held"', () => {
    // Both reads failed.
    expect(tsNodeHoldByRunningCore(null, null, 'ts-a')).toBe('unknown');
    // The proxy runs but its status read failed, or the stream is reconnecting (stale cache).
    expect(tsNodeHoldByRunningCore(running, null, 'ts-a')).toBe('unknown');
    expect(tsNodeHoldByRunningCore(running, { connected: false, statuses: [status('ts-b')] }, 'ts-a')).toBe('unknown');
    expect(tsNodeHoldByRunningCore(null, { connected: false, statuses: [] }, 'ts-a')).toBe('unknown');
    // The proxy runs (or is still starting) and no frame of this run has arrived yet.
    expect(tsNodeHoldByRunningCore(running, live(), 'ts-a')).toBe('unknown');
    expect(tsNodeHoldByRunningCore({ running: false, starting: true }, live(), 'ts-a')).toBe('unknown');
    expect(tsNodeHoldByRunningCore({ running: false, starting: true }, null, 'ts-a')).toBe('unknown');
  });

  it('captures an owner only for a run that can be named again later', () => {
    expect(captureMainCoreOwner('ts-a', 'other-node', running)).toEqual(owner);
    expect(captureMainCoreOwner('ts-a', 'other-node', null)).toBeNull();
    expect(captureMainCoreOwner('ts-a', 'other-node', stopped)).toBeNull();
    expect(captureMainCoreOwner('ts-a', 'other-node', { running: true, pid: 123 })).toBeNull();
    expect(captureMainCoreOwner('ts-a', 'other-node', { ...running, starting: true })).toBeNull();
  });

  it('stops exactly the captured run and nothing else', async () => {
    const same = fixture();
    expect(await stopOwnedCore(owner, same.io)).toBe('stopped');
    expect(same.stop).toHaveBeenCalledOnce();

    const replaced = fixture();
    replaced.setStatus({ ...running, startTime: 789 });
    expect(await stopOwnedCore(owner, replaced.io)).toBe('changed');
    expect(replaced.stop).not.toHaveBeenCalled();

    const reselected = fixture();
    reselected.setSelected('new-node');
    expect(await stopOwnedCore(owner, reselected.io)).toBe('changed');
    expect(reselected.stop).not.toHaveBeenCalled();

    const stillRunning = fixture();
    stillRunning.stop.mockResolvedValueOnce({ kind: 'stopped', expectedMainGeneration: 7, stopMainGeneration: 8 });
    expect(await stopOwnedCore(owner, stillRunning.io)).toBe('changed');
  });
});


describe('atomic expected-generation stop receipt', () => {
  it('does not log out when replacement happens after the final UI status read', async () => {
    const f = fixture();
    f.stop.mockImplementationOnce(async () => {
      f.setStatus({ ...running, mainGeneration: 9 });
      return { kind: 'changed' };
    });
    expect(await stopOwnedCoreThenLogout(owner, f.io)).toEqual({ kind: 'changed' });
    expect(f.stop).toHaveBeenCalledExactlyOnceWith({ expectedMainGeneration: 7, expectedSelectedServerId: 'other-node', expectedSelectionIntentGeneration: 0 });
    expect(f.logout).not.toHaveBeenCalled();
  });

  it('refuses same-PID replacements and missing/invalid generation receipts', async () => {
    for (const mainGeneration of [undefined, 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, 8]) {
      const f = fixture();
      f.setStatus({ ...running, mainGeneration } as typeof running);
      expect(await stopOwnedCoreThenLogout(owner, f.io)).toEqual({ kind: 'changed' });
      expect(f.stop).not.toHaveBeenCalled();
      expect(f.logout).not.toHaveBeenCalled();
    }
  });
});

describe('owned stop generation and selector binding after retirement', () => {
  it.each([undefined, NaN, 0, 1.5, 10, 9_007_199_254_740_992])('rejects supplementary stopped generation %s', async (generation) => {
    const f = fixture();
    f.stop.mockImplementationOnce(async () => {
      // A successor may reuse the old PID and selection and stop again.
      f.setStatus({ ...running, running: false, mainGeneration: generation } as typeof running);
      return {kind:'stopped',expectedMainGeneration:7,stopMainGeneration:8};
    });
    expect(await stopOwnedCoreThenLogout(owner, f.io)).toEqual({kind:'changed'});
    expect(f.logout).not.toHaveBeenCalled();
  });
  it('requires the exact stopped generation and same captured selection epoch', async () => {
    const matching = fixture();
    expect(await stopOwnedCoreThenLogout(owner, matching.io)).toEqual({kind:'loggedOut'});
    const aba = fixture();
    aba.stop.mockImplementationOnce(async () => {
      aba.setStatus({ ...running, running:false, mainGeneration:8,
        ownedStopContext:{...context,selectionIntentGeneration:2} });
      return {kind:'stopped',expectedMainGeneration:7,stopMainGeneration:8};
    });
    expect(await stopOwnedCoreThenLogout(owner, aba.io)).toEqual({kind:'changed'});
    expect(aba.logout).not.toHaveBeenCalled();
  });
  it('cannot capture or submit an owner with missing or changed backend selection context', async () => {
    expect(captureMainCoreOwner('ts-a','other-node',{...running,ownedStopContext:undefined})).toBeNull();
    expect(captureMainCoreOwner('ts-a','new-node',running)).toBeNull();
    const f=fixture(); f.setStatus({...running,ownedStopContext:{...context,selectionIntentGeneration:1}});
    expect(await stopOwnedCoreThenLogout(owner,f.io)).toEqual({kind:'changed'});
    expect(f.stop).not.toHaveBeenCalled();
  });
});
