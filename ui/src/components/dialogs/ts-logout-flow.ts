import type { TailscaleMainCoreHold } from '@/contracts/tailscale-status';
import type { ProxyStatus, ExpectedMainCoreStop, ExpectedMainCoreStopResult } from '@/contracts/types';

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
  const generation = before.mainGeneration;
  return typeof generation === 'number' && Number.isSafeInteger(generation) && generation > 0
    && generation === now.mainGeneration
    && typeof readyAt === 'number' && Number.isSafeInteger(readyAt) && readyAt > 0
    && readyAt === now.startTime
    && before.pid === now.pid;
}

export type TsNodeHold = 'held' | 'notHeld' | 'unknown';

/**
 * Only a same-generation applied endpoint set can prove a Ready core does not hold a node.
 * STATUS may be empty before the first frame, or never subscribed for a no-TS core; neither
 * an empty nor a nonempty unbound cache proves custody. Unknown always keeps the notice.
 */
export function tsNodeHoldByRunningCore(
  proxy: ProxyStatus | null,
  snapshot: { connected: boolean; statuses: readonly { serverId: string }[]; mainCoreHold?: TailscaleMainCoreHold } | null,
  serverId: string,
): TsNodeHold {
  const generation = proxy?.mainGeneration;
  const proof = snapshot?.mainCoreHold;
  // These reads are parallel: a stopped proxy read may precede a new Ready core's snapshot.
  // Connected or Ready proof contradicts the stopped read, even if the proof lists no target.
  if (proxy && !proxy.running && !proxy.starting) {
    return snapshot?.connected || proof != null ? 'unknown' : 'notHeld';
  }
  if (!proxy?.running || proxy.starting || !snapshot?.connected
    || typeof generation !== 'number' || !Number.isSafeInteger(generation) || generation <= 0
    || !proof || proof.mainGeneration !== generation
    || !Array.isArray(proof.serverIds)
    || !proof.serverIds.every((id) => typeof id === 'string' && id.length > 0)) return 'unknown';
  return proof.serverIds.includes(serverId) ? 'held' : 'notHeld';
}

/** The run to stop is captured before asking; a core without a ready receipt cannot be named. */
export function captureMainCoreOwner(
  serverId: string,
  selectedId: string | null,
  status: ProxyStatus | null,
): MainCoreLogoutOwner | null {
  return status && sameRunningCore(status, status) && sameSelectionContext(status, status, selectedId)
    ? { serverId, selectedId, status } : null;
}

function sameSelectionContext(before: ProxyStatus, now: ProxyStatus, selectedId: string | null): boolean {
  const a = before.ownedStopContext;
  const b = now.ownedStopContext;
  return a != null && b != null && typeof a === 'object' && typeof b === 'object'
    && Number.isSafeInteger(a.selectionIntentGeneration) && a.selectionIntentGeneration >= 0
    && Number.isSafeInteger(b.selectionIntentGeneration) && b.selectionIntentGeneration >= 0
    && a.selectedServerId === selectedId && b.selectedServerId === selectedId
    && a.selectionIntentGeneration === b.selectionIntentGeneration;
}

export interface OwnedCoreStopIo {
  selectedId: () => string | null;
  serverPresent: (id: string) => boolean;
  status: () => Promise<ProxyStatus>;
  stopExpected: (expected: ExpectedMainCoreStop) => Promise<ExpectedMainCoreStopResult>;
}

/** Stops only the captured run; a replacement core or a changed selection is left alone. */
export async function stopOwnedCore(
  owner: MainCoreLogoutOwner,
  io: OwnedCoreStopIo,
): Promise<'stopped' | 'changed' | 'stopFailed'> {
  const expected = owner.status.mainGeneration;
  if (typeof expected !== 'number' || !Number.isSafeInteger(expected) || expected <= 0) return 'changed';
  if (io.selectedId() !== owner.selectedId || !io.serverPresent(owner.serverId)) return 'changed';
  let current: ProxyStatus;
  try { current = await io.status(); } catch { return 'changed'; }
  if (!sameRunningCore(owner.status, current) || !sameSelectionContext(owner.status, current, owner.selectedId)
    || io.selectedId() !== owner.selectedId
    || !io.serverPresent(owner.serverId)) return 'changed';
  let stopGeneration: number;
  const context = owner.status.ownedStopContext;
  if (!context) return 'changed';
  try {
    const receipt = await io.stopExpected({ expectedMainGeneration: expected,
      expectedSelectedServerId: context.selectedServerId,
      expectedSelectionIntentGeneration: context.selectionIntentGeneration });
    if (receipt.kind === 'changed') return 'changed';
    if (receipt.kind !== 'stopped') return 'stopFailed';
    if (receipt.expectedMainGeneration !== owner.status.mainGeneration
      || !Number.isSafeInteger(receipt.stopMainGeneration) || receipt.stopMainGeneration <= expected) return 'stopFailed';
    stopGeneration = receipt.stopMainGeneration;
  } catch { return 'stopFailed'; }
  let stopped: ProxyStatus;
  try { stopped = await io.status(); } catch { return 'stopFailed'; }
  if (!Number.isSafeInteger(stopped.mainGeneration) || stopped.mainGeneration !== stopGeneration
    || !sameSelectionContext(owner.status, stopped, owner.selectedId)
    || io.selectedId() !== owner.selectedId || !io.serverPresent(owner.serverId)) return 'changed';
  if (stopped.running || stopped.starting) return 'stopFailed';
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
