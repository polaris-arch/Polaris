import { describe, expect, it, vi } from 'vitest';
import { captureMainCoreOwner, isMainCoreLogoutError, sameRunningCore, stopOwnedCore, stopOwnedCoreThenLogout, tsNodeHeldByRunningCore } from './ts-logout-flow';

const running = { running: true, pid: 123, startTime: 456 };
const stopped = { running: false };
const owner = { serverId: 'ts-a', selectedId: 'other-node', status: running };

function fixture() {
  let status = running;
  let selected: string | null = 'other-node';
  let present = true;
  const stop = vi.fn(async () => { status = stopped as typeof running; });
  const logout = vi.fn(async (_id: string) => {});
  return { stop, logout,
    setStatus: (next: typeof running) => { status = next; },
    setSelected: (next: string | null) => { selected = next; },
    setPresent: (next: boolean) => { present = next; },
    io: { selectedId: () => selected, serverPresent: () => present,
      status: async () => status, stop, logout },
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
    expect(f.stop).toHaveBeenCalledOnce();
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
    restarted.stop.mockImplementationOnce(async () => restarted.setStatus({ ...running, startTime: 789 }));
    expect(await stopOwnedCoreThenLogout(owner, restarted.io)).toEqual({ kind: 'stopFailed' });
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

  it('only a live status stream that lists the node counts as held', () => {
    expect(tsNodeHeldByRunningCore({ connected: true, statuses: [status('ts-b'), status('ts-a')] }, 'ts-a')).toBe(true);
    // A stopped core keeps its last frames as a stale cache; that is not a holder.
    expect(tsNodeHeldByRunningCore({ connected: false, statuses: [status('ts-a')] }, 'ts-a')).toBe(false);
    expect(tsNodeHeldByRunningCore({ connected: true, statuses: [status('ts-b')] }, 'ts-a')).toBe(false);
    expect(tsNodeHeldByRunningCore({ connected: true, statuses: [] }, 'ts-a')).toBe(false);
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
    stillRunning.stop.mockImplementationOnce(async () => {});
    expect(await stopOwnedCore(owner, stillRunning.io)).toBe('stopFailed');
  });
});
