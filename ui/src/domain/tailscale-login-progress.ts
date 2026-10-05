import { validatedTailscaleAuthUrl } from './tailscale-auth-url';

export type TailscaleLoginPhase = 'starting' | 'preparingConnection' | 'waitingForReady' | 'awaitingAuth' | 'mainCore' | 'authorized' | 'failed' | 'timedOut' | 'cancelled';

export interface TailscaleLoginProgress {
  serverId: string;
  attemptId: string;
  phase: TailscaleLoginPhase;
  reason?: string | null;
  url?: string | null;
  mainGeneration?: number | null;
  identityEpoch?: string | null;
}

/** A panel may display progress only for the request it started. */
export function progressForLoginRequest(
  progress: TailscaleLoginProgress | undefined,
  request: Pick<TailscaleLoginProgress, 'serverId' | 'attemptId'> | null,
): TailscaleLoginProgress | undefined {
  if (!request || !progress) return undefined;
  return progress.serverId === request.serverId && progress.attemptId === request.attemptId
    ? progress : undefined;
}

export function loginAttemptActive(phase: TailscaleLoginPhase): boolean {
  return phase === 'starting' || phase === 'preparingConnection' || phase === 'waitingForReady'
    || phase === 'awaitingAuth' || phase === 'mainCore';
}

/** Late URL/terminal events from an old request cannot replace the current request. */
export function acceptLoginProgress(current: TailscaleLoginProgress | undefined, next: TailscaleLoginProgress): boolean {
  return current?.serverId === next.serverId && current.attemptId === next.attemptId
    && loginAttemptActive(current.phase)
    && (current.mainGeneration == null || current.mainGeneration === next.mainGeneration)
    && (current.identityEpoch == null || current.identityEpoch === next.identityEpoch);
}

const REASON_KEYS: Record<string, string> = {
  ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED: 'errors.androidNativeCapacityClosed',
  TAILSCALE_IDENTITY_RETIREMENT_REQUIRED: 'ts.identityRetirementRequired',
  IOS_FOREGROUND_REQUIRED: 'prerequisite.foregroundRequired',
  IOS_VPN_PERMISSION_DENIED: 'prerequisite.permissionDenied',
  IOS_READY_UNKNOWN: 'prerequisite.readyUnknown',
  IOS_START_CANCELLED: 'prerequisite.cancelled',
  readyUnknown: 'prerequisite.readyUnknown',
  cancelled: 'prerequisite.cancelled',
  superseded: 'ts.reasonMainCoreChanged',
  configurationPending: 'prerequisite.saveConfiguration',
  unsavedConfiguration: 'prerequisite.saveConfiguration',
  targetNotInMain: 'ts.reasonMainCoreChanged',
  coreUnavailable: 'ts.reasonCoreUnavailable', configurationCheckFailed: 'ts.reasonConfigurationCheck',
  configWriteFailed: 'ts.reasonConfigWrite', processStartFailed: 'ts.reasonProcessStart',
  statusSubscriptionFailed: 'ts.reasonStatusSubscription', statusStreamEnded: 'ts.reasonStatusSubscription',
  processExited: 'ts.reasonProcessExited', authorizationTimedOut: 'ts.reasonTimeout',
  mainCoreChanged: 'ts.reasonMainCoreChanged', mainCoreInUse: 'ts.reasonMainCoreInUse', stateQueryFailed: 'ts.reasonStateQuery',
  saveFailed: 'ts.reasonSave', configurationRefreshFailed: 'ts.reasonRefresh',
  tooManyLogins: 'ts.reasonTooManyLogins', invalidAuthUrl: 'ts.reasonInvalidAuthUrl',
};

export function loginFailureReasonKey(reason: string | null | undefined): string {
  return (reason && REASON_KEYS[reason]) || 'ts.reasonAuthorization';
}

/** Clipboard success is reported only after the write promise resolves. */
export async function copyLoginUrl(url: string, clipboard: Pick<Clipboard, 'writeText'> | undefined): Promise<void> {
  if (!clipboard?.writeText) throw new Error('CLIPBOARD_UNAVAILABLE');
  await clipboard.writeText(url);
}

/** Bare main AUTH has no request binding and serves only the legacy flow without an attempt. */
export function mainAuthUrlOwner(current: TailscaleLoginProgress | undefined): string | null {
  return current ? null : 'legacy';
}

/** Keep one entry per node, while allowing a new request to reuse the same URL. */
export function claimLoginUrl(seen: Map<string, string>, serverId: string, owner: string, url: string): boolean {
  const value = `${owner}\0${url}`;
  if (seen.get(serverId) === value) return false;
  seen.set(serverId, value);
  return true;
}

/** Browser failure leaves authorization progress and the copyable URL intact. */
export async function openLoginUrl(url: string, open: (url: string) => Promise<void>, failed: () => void): Promise<boolean> {
  try {
    if (!validatedTailscaleAuthUrl(url)) throw new Error('INVALID_AUTH_URL');
    await open(url);
    return true;
  } catch {
    failed();
    return false;
  }
}
