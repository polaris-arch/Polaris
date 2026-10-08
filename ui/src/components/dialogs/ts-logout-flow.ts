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

/** The ready timestamp identifies this run; PID alone can belong to a replacement core. */
export function sameRunningCore(before: ProxyStatus, now: ProxyStatus): boolean {
  if (!before.running || !now.running || before.starting || now.starting) return false;
  const readyAt = before.startTime;
  return typeof readyAt === 'number' && Number.isSafeInteger(readyAt) && readyAt > 0
    && readyAt === now.startTime
    && before.pid === now.pid;
}

export type TsNodeHold = 'held' | 'notHeld' | 'unknown';

/**
 * Whether the running proxy holds this node, for backends that stop such a proxy themselves.
 * `notHeld` needs positive proof: the proxy is neither running nor starting, or this run's
 * status frame has arrived and does not list the node. A failed read (`null`), a stream that
 * is not live, and a first frame that has not arrived are all `unknown`, never `notHeld`.
 */
export function tsNodeHoldByRunningCore(
  proxy: ProxyStatus | null,
  snapshot: { connected: boolean; statuses: readonly { serverId: string }[] } | null,
  serverId: string,
): TsNodeHold {
  if (proxy && !proxy.running && !proxy.starting) return 'notHeld';
  // The cache is cleared when the core stops, so a non-empty live list is this run's frame.
  if (snapshot?.connected && snapshot.statuses.length > 0) {
    return snapshot.statuses.some((status) => status.serverId === serverId) ? 'held' : 'notHeld';
  }
  return 'unknown';
}

/** The run to stop is captured before asking; a core without a ready receipt cannot be named. */
export function captureMainCoreOwner(
  serverId: string,
  selectedId: string | null,
  status: ProxyStatus | null,
): MainCoreLogoutOwner | null {
  return status && sameRunningCore(status, status) ? { serverId, selectedId, status } : null;
}

export interface OwnedCoreStopIo {
  selectedId: () => string | null;
  serverPresent: (id: string) => boolean;
  status: () => Promise<ProxyStatus>;
  stop: () => Promise<void>;
}

/** Stops only the captured run; a replacement core or a changed selection is left alone. */
export async function stopOwnedCore(
  owner: MainCoreLogoutOwner,
  io: OwnedCoreStopIo,
): Promise<'stopped' | 'changed' | 'stopFailed'> {
  if (io.selectedId() !== owner.selectedId || !io.serverPresent(owner.serverId)) return 'changed';
  let current: ProxyStatus;
  try { current = await io.status(); } catch { return 'changed'; }
  if (!sameRunningCore(owner.status, current) || io.selectedId() !== owner.selectedId
    || !io.serverPresent(owner.serverId)) return 'changed';
  try { await io.stop(); } catch { return 'stopFailed'; }
  let stopped: ProxyStatus;
  try { stopped = await io.status(); } catch { return 'stopFailed'; }
  if (stopped.running || stopped.starting || io.selectedId() !== owner.selectedId
    || !io.serverPresent(owner.serverId)) return 'stopFailed';
  return 'stopped';
}

/** Only a second, explicit confirmation may stop the core; native logout remains the final gate. */
export async function stopOwnedCoreThenLogout(
  owner: MainCoreLogoutOwner,
  io: OwnedCoreStopIo & { logout: (id: string) => Promise<void> },
): Promise<MainCoreLogoutResult> {
  const stop = await stopOwnedCore(owner, io);
  if (stop !== 'stopped') return { kind: stop };
  try { await io.logout(owner.serverId); } catch (error) {
    return { kind: 'logoutFailed', code: error && typeof error === 'object' && 'code' in error
      && typeof error.code === 'string' ? error.code : undefined };
  }
  return { kind: 'loggedOut' };
}
