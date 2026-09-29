import { describe, expect, it, vi } from 'vitest';
import { isMainCoreLogoutError, sameRunningCore, stopOwnedCoreThenLogout } from './ts-logout-flow';

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
