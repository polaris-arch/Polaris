import type { TsExitWarning } from '@/domain/tailscale-exit-warning';

/** Presentation only: the shared domain remains the sole source of the warning state. */
export const TS_EXIT_SUMMARY_KEY: Record<Exclude<TsExitWarning, 'none'>, string> = {
  'needs-auth': 'mobileHelp.tsExitNeedsAuth',
  'no-exit-device': 'mobileHelp.tsExitNoDevice',
  'exit-device-offline': 'mobileHelp.tsExitOffline',
  'exit-device-not-advertised': 'mobileHelp.tsExitNotAdvertised',
};
