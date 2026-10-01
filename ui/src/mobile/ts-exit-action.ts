import { validatedTailscaleAuthUrl } from '@/domain/tailscale-auth-url';
import type { TsExitWarning } from '@/domain/tailscale-exit-warning';

export type TsExitAction =
  | { kind: 'login-url'; url: string }
  | { kind: 'login-panel' }
  | { kind: 'exit-panel' };

/** The warning's cause decides the action; an unrelated cached URL cannot hijack exit setup. */
export function tsExitAction(
  warning: TsExitWarning,
  liveUrl: string | null | undefined,
  ownedStoreUrl?: string | null,
): TsExitAction {
  if (warning !== 'needs-auth') return { kind: 'exit-panel' };
  const url = validatedTailscaleAuthUrl(liveUrl) ?? validatedTailscaleAuthUrl(ownedStoreUrl);
  return url ? { kind: 'login-url', url } : { kind: 'login-panel' };
}
