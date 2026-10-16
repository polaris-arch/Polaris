import type { TlsSettings } from '@/contracts/types/protocol-settings';

/** IDs accepted by the pinned desktop and mobile kernel, not library constants. */
export const UTLS_PROFILE_OPTIONS: readonly (readonly [string, string])[] = [
  ['random', 'Random'],
  ['randomized', 'Randomized'],
  ...[
    ['chrome', 'Chrome'], ['firefox', 'Firefox'], ['safari', 'Safari'], ['edge', 'Edge'],
    ['360', '360'], ['qq', 'QQ'], ['ios', 'iOS'], ['android', 'Android'],
  ].sort((a, b) => a[1].localeCompare(b[1], 'en', { numeric: true, sensitivity: 'base' }))
    .map(([value, label]) => [value, label] as const),
];

export const NODE_UTLS_OPTIONS: readonly (readonly [string, string])[] = [
  ['', 'node.utls.legacy'],
  ['@default', 'node.utls.default'],
  ['none', 'node.utls.disabled'],
  ...UTLS_PROFILE_OPTIONS,
];
export const SHADOW_UTLS_OPTIONS = NODE_UTLS_OPTIONS.filter(([v]) => v !== 'none');
export const GLOBAL_UTLS_OPTIONS: readonly (readonly [string, string])[] = [
  ['follow', 'settings.utls.follow'],
  ['default', 'node.utls.default'],
  ...UTLS_PROFILE_OPTIONS,
];

type Intent = Pick<TlsSettings, 'utlsMode' | 'fingerprint'>;

export function utlsDraftValue(base: Intent | undefined): string {
  if (base?.utlsMode === 'default') return '@default';
  if (base?.utlsMode === 'disabled') return 'none';
  return base?.fingerprint?.trim().toLowerCase() ?? '';
}

/** A no-edit round trip preserves dormant values and explicit enabled intent. */
export function utlsPatch(value: string, base: Intent | undefined): Intent {
  if (value === utlsDraftValue(base)) {
    return { utlsMode: base?.utlsMode, fingerprint: base?.fingerprint?.trim().toLowerCase() || undefined };
  }
  if (value === '') return { utlsMode: undefined, fingerprint: undefined };
  if (value === '@default') return { utlsMode: 'default', fingerprint: undefined };
  if (value === 'none') return { utlsMode: 'disabled', fingerprint: undefined };
  return { utlsMode: 'profile', fingerprint: value.trim().toLowerCase() };
}

/** Keep an imported future/unknown policy visible until explicit user selection. */
export function globalUtlsOptions(current: string | undefined): readonly (readonly [string, string])[] {
  return current && !GLOBAL_UTLS_OPTIONS.some(([value]) => value === current)
    ? [...GLOBAL_UTLS_OPTIONS, [current, current] as const] : GLOBAL_UTLS_OPTIONS;
}
