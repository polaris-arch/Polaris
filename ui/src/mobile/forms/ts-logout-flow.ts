import type { ProxyStatus } from '@/contracts/types';

export interface MainCoreLogoutOwner {
  readonly serverId: string;
  readonly selectedId: string | null;
  readonly status: ProxyStatus;
}

export type MainCoreLogoutResult =
  | { kind: 'loggedOut' }
  | { kind: 'changed' }
  | { kind: 'stopFailed' }
  | { kind: 'logoutFailed'; code?: string };

export function isMainCoreLogoutError(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error
    && error.code === 'TAILSCALE_LOGOUT_MAIN_CORE';
}

/** PID alone may be reused. Prefer the core's ready timestamp, and reject unknown identity. */
export function sameRunningCore(before: ProxyStatus, now: ProxyStatus): boolean {
  if (!before.running || !now.running || before.starting || now.starting) return false;
  if (typeof before.startTime === 'number' && before.startTime > 0) {
    return before.startTime === now.startTime
      && (before.pid === undefined || now.pid === undefined || before.pid === now.pid);
  }
  return typeof before.pid === 'number' && before.pid > 0 && before.pid === now.pid;
}

/** Only a second, explicit confirmation may stop the core; native logout remains the final gate. */
export async function stopOwnedCoreThenLogout(
  owner: MainCoreLogoutOwner,
  io: {
    selectedId: () => string | null;
    serverPresent: (id: string) => boolean;
    status: () => Promise<ProxyStatus>;
    stop: () => Promise<void>;
    logout: (id: string) => Promise<void>;
  },
): Promise<MainCoreLogoutResult> {
  if (io.selectedId() !== owner.selectedId || !io.serverPresent(owner.serverId)) return { kind: 'changed' };
  let current: ProxyStatus;
  try { current = await io.status(); } catch { return { kind: 'changed' }; }
  if (!sameRunningCore(owner.status, current) || io.selectedId() !== owner.selectedId) return { kind: 'changed' };
  try { await io.stop(); } catch { return { kind: 'stopFailed' }; }
  let stopped: ProxyStatus;
  try { stopped = await io.status(); } catch { return { kind: 'stopFailed' }; }
  if (stopped.running || stopped.starting || io.selectedId() !== owner.selectedId
    || !io.serverPresent(owner.serverId)) return { kind: 'stopFailed' };
  try { await io.logout(owner.serverId); } catch (error) {
    return { kind: 'logoutFailed', code: error && typeof error === 'object' && 'code' in error
      && typeof error.code === 'string' ? error.code : undefined };
  }
  return { kind: 'loggedOut' };
}
