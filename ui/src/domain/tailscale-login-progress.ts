import { validatedTailscaleAuthUrl } from './tailscale-auth-url';

export type TailscaleLoginPhase = 'starting' | 'awaitingAuth' | 'mainCore' | 'authorized' | 'failed' | 'timedOut' | 'cancelled';

export interface TailscaleLoginProgress {
  serverId: string;
  attemptId: string;
  phase: TailscaleLoginPhase;
  reason?: string | null;
  url?: string | null;
}

export function loginAttemptActive(phase: TailscaleLoginPhase): boolean {
  return phase === 'starting' || phase === 'awaitingAuth' || phase === 'mainCore';
}

/** Late URL/terminal events from an old request cannot replace the current request. */
export function acceptLoginProgress(current: TailscaleLoginProgress | undefined, next: TailscaleLoginProgress): boolean {
  return current?.attemptId === next.attemptId && loginAttemptActive(current.phase);
}

/** A live main-core frame may confirm only a request for the configuration it actually owns. */
export function authorizeFromMainFrame(current: TailscaleLoginProgress | undefined, frame: {
  serverId: string; backendState: string; expired: boolean;
}): TailscaleLoginProgress | null {
  return current?.serverId === frame.serverId && current.phase === 'mainCore' &&
    !current.reason && frame.backendState === 'Running' && !frame.expired
    ? { ...current, phase: 'authorized', url: null } : null;
}

const REASON_KEYS: Record<string, string> = {
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

/** Main AUTH and STATUS events use the same request identity; completed requests ignore residual URLs. */
export function mainAuthUrlOwner(current: TailscaleLoginProgress | undefined): string | null {
  if (!current) return 'legacy';
  return current.phase === 'mainCore' && !current.reason ? current.attemptId : null;
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
