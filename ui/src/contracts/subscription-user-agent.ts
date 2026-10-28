/** Read-only Rust SubscriptionUserAgentInfo mirror; no frontend version/UA synthesis. */
export type SubscriptionUserAgentInfo = {
  appVersion: string;
  /** Selected bundled kernel build, not proof of a running core. */
  coreVersion: string;
  defaultUserAgent: string;
} & ({
  effectiveStatus: 'ready';
  /** Effective value when this subscription's optional override is empty. */
  fallbackUserAgent: string;
  effectiveUserAgent: string;
  source: 'subscription' | 'global' | 'default';
} | {
  /** Saved global policy is cold/busy: known app default must NOT be shown as effective. */
  effectiveStatus: 'unavailable';
  fallbackUserAgent: null;
  effectiveUserAgent: null;
  source: null;
});

/** Validate the independent build/effective states; never fabricate identity or a fallback. */
export function isSubscriptionUserAgentInfo(value: unknown): value is SubscriptionUserAgentInfo {
  if (typeof value !== 'object' || value === null) return false;
  const info = value as Record<string, unknown>;
  if (!['appVersion', 'coreVersion', 'defaultUserAgent']
    .every(key => typeof info[key] === 'string' && info[key].trim() !== '')
    || info.appVersion === 'unknown' || info.coreVersion === 'unknown') return false;
  if (info.effectiveStatus === 'unavailable') {
    return info.fallbackUserAgent === null && info.effectiveUserAgent === null && info.source === null;
  }
  return info.effectiveStatus === 'ready'
    && ['fallbackUserAgent', 'effectiveUserAgent']
      .every(key => typeof info[key] === 'string' && info[key].trim() !== '')
    && ['subscription', 'global', 'default'].includes(info.source as string);
}
