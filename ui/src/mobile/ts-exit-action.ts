import { validatedTailscaleAuthUrl } from '@/domain/tailscale-auth-url';
import type { TsExitWarning } from '@/domain/tailscale-exit-warning';
import type { TailscaleLoginProgress } from '@/domain/tailscale-login-progress';

export type TsExitAction =
  | { kind: 'login-url'; url: string }
  | { kind: 'login-panel' }
  | { kind: 'exit-panel' };

/** Display STATUS can explain a warning; only request progress can supply an active login URL. */
export function tsExitAction(
  warning: TsExitWarning,
  source: {
    liveUrl?: string | null;
    storeUrl?: string | null;
    attempt?: TailscaleLoginProgress;
    normalMainRequired?: boolean;
  },
): TsExitAction {
  if (warning !== 'needs-auth') return { kind: 'exit-panel' };
  const { attempt, normalMainRequired } = source;
  if (attempt) {
    const bound = !normalMainRequired || (Number.isSafeInteger(attempt.mainGeneration)
      && typeof attempt.identityEpoch === 'string' && attempt.identityEpoch.length > 0);
    const url = bound && (attempt.phase === 'awaitingAuth' || attempt.phase === 'mainCore')
      ? validatedTailscaleAuthUrl(attempt.url) : null;
    return url ? { kind: 'login-url', url } : { kind: 'login-panel' };
  }
  // A fresh iOS panel registers an action before the normal-main/system-permission producer.
  if (normalMainRequired) return { kind: 'login-panel' };
  const url = validatedTailscaleAuthUrl(source.liveUrl) ?? validatedTailscaleAuthUrl(source.storeUrl);
  return url ? { kind: 'login-url', url } : { kind: 'login-panel' };
}
