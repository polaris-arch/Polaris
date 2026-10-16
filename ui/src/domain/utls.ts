import type { TlsSettings } from '@/contracts/types/protocol-settings';

/** Intent controls only. Profiles always come from the actual core report. */
export const NODE_UTLS_OPTIONS: readonly (readonly [string, string])[] = [
  ['', 'node.utls.legacy'],
  ['@default', 'node.utls.default'],
  ['none', 'node.utls.disabled'],
];
export const SHADOW_UTLS_OPTIONS = NODE_UTLS_OPTIONS.filter(([v]) => v !== 'none');
export const GLOBAL_UTLS_OPTIONS: readonly (readonly [string, string])[] = [
  ['follow', 'settings.utls.follow'],
  ['default', 'node.utls.default'],
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


export interface TlsFingerprintProfile {
  id: string;
  displayName: string;
  client: string;
  templateVersion: string;
  selection: 'profile' | 'process-browser' | 'process-seeded-generator';
  templateOffersX25519MLKEM768: boolean | null;
}
export interface TlsFingerprintCapabilities {
  schema: 'polaris-tls-fingerprint-capabilities-v1';
  enabled: boolean;
  coreVersion: string;
  sourceCommit: string;
  sourceModified: boolean | null;
  goVersion: string;
  requiredGoVersion: string;
  buildTags: string;
  os: string;
  arch: string;
  modules: readonly { path: string; version: string; sum: string; replace?: unknown }[];
  defaultFingerprint: string;
  profiles: readonly TlsFingerprintProfile[];
  legacyChromeAliases: readonly string[];
  shadowTLSSessionIDWithECHSupported: false;
  curvePreferencesSupported: false;
  realityX25519MLKEM768: string;
  randomSemantics: string;
  randomizedSemantics: string;
  negotiationEvidence: string;
}
export interface TlsFingerprintSnapshot {
  /** CLI random-template facts belong to its probe process; never a live handshake. */
  observationScope: 'cli-probe-process' | 'loaded-libbox' | 'unavailable';
  byteIdentityScope: 'cli-executable' | 'loaded-jni-so' | 'packet-tunnel-executable' | 'loaded-native-container' | 'unavailable';
  identity: string | null;
  binarySha256: string | null;
  document: TlsFingerprintCapabilities | null;
  unavailableReason: string | null;
}

type Option = readonly [string, string];
function advertisedProfiles(snapshot?: TlsFingerprintSnapshot): readonly Option[] {
  const doc = snapshot?.document;
  if (!snapshot?.identity || !doc?.enabled || doc.schema !== 'polaris-tls-fingerprint-capabilities-v1') return [];
  const priority = (id: string) => id === 'random' ? 0 : id === 'randomized' ? 1 : 2;
  return [...doc.profiles].sort((a, b) => priority(a.id) - priority(b.id)
    || a.displayName.localeCompare(b.displayName, 'en', { numeric: true, sensitivity: 'base' })
    || a.id.localeCompare(b.id, 'en', { numeric: true }))
    .map(p => [p.id, p.templateVersion && p.selection === 'profile'
      ? `${p.displayName} (${p.templateVersion})` : p.displayName] as const);
}

/** Shared by desktop/mobile fields. No discovered report means no invented presets.
 * Existing/imported values remain visible through normalizeSelectOptions. */
export function nodeUtlsOptions(shadow: boolean, snapshot?: TlsFingerprintSnapshot): readonly Option[] {
  const base: Option[] = [['', 'node.utls.legacy']];
  if (snapshot?.document?.enabled !== false) base.push(['@default', 'node.utls.default']);
  if (!shadow) base.push(['none', 'node.utls.disabled']);
  return [...base, ...advertisedProfiles(snapshot)];
}
export function isUtlsField(key: string): boolean { return key === 'fp' || key === 'stlsFp'; }

/** Keep an unsupported current policy visible; never rewrite persisted intent. */
export function globalUtlsOptions(current: string | undefined, snapshot?: TlsFingerprintSnapshot): readonly Option[] {
  const options: Option[] = [['follow', 'settings.utls.follow']];
  if (snapshot?.document?.enabled !== false) options.push(['default', 'node.utls.default']);
  options.push(...advertisedProfiles(snapshot));
  return current && !options.some(([value]) => value === current)
    ? [...options, [current, current] as const] : options;
}
