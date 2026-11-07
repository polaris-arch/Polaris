import type { ServerConfig } from '@/contracts/types';
import type { TlsSettings } from '@/contracts/types/protocol-settings';

/** Intent controls only. Profiles come from reviewed selected support or a native report. */
export const NODE_UTLS_OPTIONS: readonly (readonly [string, string])[] = [
  ['', 'node.utls.legacy'],
  ['none', 'node.utls.disabled'],
];
export const SHADOW_UTLS_OPTIONS = NODE_UTLS_OPTIONS;
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

/** Reviewed data for the expected compiled Core, never an installed/running-byte proof. */
export interface SelectedTlsSupportDirectory {
  evidenceKind: 'compiled-selected-support-v1';
  selectedSpec: {
    platform: string;
    target: string;
    sourceCommit: string;
    sourceTree: string;
    version: string;
    contractSha256: string;
    coreBytes: number;
    coreSha256: string;
    dataOrigin: 'observed-cli-metadata' | 'source-derived-stable-catalog';
    observationStdoutSha256: string | null;
    sourceArchiveSha256: string;
    sourceIndexSha256: string;
    goVersion: string;
    requiredGoVersion: string;
    buildTags: string;
  } | null;
  document: {
    schema: 'polaris-tls-support-directory-v1';
    enabled: boolean;
    os: string;
    arch: string;
    defaultFingerprint: string;
    profiles: readonly (Pick<TlsFingerprintProfile, 'id' | 'displayName' | 'selection'> & {
      client: null;
      templateVersion: null;
      templateOffersX25519MLKEM768: null;
    })[];
    legacyChromeAliases: readonly string[];
    randomSemantics: string;
    randomizedSemantics: string;
    negotiationEvidence: string;
  } | null;
  unavailableReason: string | null;
}
export type TlsFingerprintPresentation = TlsFingerprintSnapshot | SelectedTlsSupportDirectory;

/** Keep the desktop support directory distinct from loaded/mobile observations. */
function presentationDocument(view?: TlsFingerprintPresentation) {
  if (!view || view.unavailableReason) return undefined;
  if ('evidenceKind' in view) {
    return view.evidenceKind === 'compiled-selected-support-v1' && view.selectedSpec
      && view.document?.schema === 'polaris-tls-support-directory-v1' ? view.document : undefined;
  }
  return view.identity && view.document?.schema === 'polaris-tls-fingerprint-capabilities-v1'
    ? view.document : undefined;
}

type Option = readonly [string, string, boolean?];
/** Presentation only: unversioned IDs follow the core's browser template.
 * Pinned and unknown profiles retain the reported name and version. */
function profileLabel(profile: NonNullable<ReturnType<typeof presentationDocument>>['profiles'][number]): string {
  const browserLabels: Readonly<Record<string, string>> = {
    chrome: 'Chrome', firefox: 'Firefox', edge: 'Edge', safari: 'Safari',
    '360': '360', qq: 'QQ', ios: 'iOS',
  };
  if (profile.id === 'random') return 'node.utls.random';
  if (Object.prototype.hasOwnProperty.call(browserLabels, profile.id)) return browserLabels[profile.id];
  return profile.templateVersion && profile.selection === 'profile'
    ? `${profile.displayName} (${profile.templateVersion})` : profile.displayName;
}

function advertisedProfiles(snapshot?: TlsFingerprintPresentation): readonly Option[] {
  const doc = presentationDocument(snapshot);
  if (!doc?.enabled) return [];
  const priority = (id: string) => id === 'random' ? 0 : 1;
  return doc.profiles.filter(p => p.id !== 'randomized').sort((a, b) => priority(a.id) - priority(b.id)
    || a.displayName.localeCompare(b.displayName, 'en', { numeric: true, sensitivity: 'base' })
    || a.id.localeCompare(b.id, 'en', { numeric: true }))
    .map(p => [p.id, profileLabel(p)] as const);
}

/** Shared by desktop/mobile fields. No support document means no invented presets.
 * Existing/imported values remain visible through normalizeSelectOptions. */
export function nodeUtlsOptions(_shadow: boolean, snapshot?: TlsFingerprintPresentation, reality = false, current?: string, engine?: unknown): readonly Option[] {
  const base: Option[] = [['', 'node.utls.legacy']];
  base.push(['none', 'node.utls.disabled', reality]);
  const incompatible = !_shadow && nativeUtlsUnsupported(engine, snapshot, reality);
  return [...base, ...[...advertisedProfiles(snapshot), ...savedUtlsOption(current)]
    .map(([id, label]) => incompatible ? [id, label, true] as const : [id, label] as const)];
}
/** Mirror the backend's platform gate; dormant foreign-engine values are inert. */
export function nativeUtlsUnsupported(engine: unknown, snapshot?: TlsFingerprintPresentation, reality = false): boolean {
  const doc = presentationDocument(snapshot);
  if (reality || !doc) return false;
  const os = doc.os;
  return engine === 'windows' && (os === 'windows' || os === 'win32')
    || engine === 'apple' && os === 'darwin';
}
/** Retain historical explicit settings visibly; never silently rewrite them. */
function savedUtlsOption(current?: string): readonly Option[] {
  if (current === '@default') return [[current, 'node.utls.savedCoreDefault']];
  if (current === 'randomized') return [[current, 'node.utls.savedRandomized']];
  return [];
}
export function isSavedUtlsSelection(current: unknown): boolean {
  return current === '@default' || current === 'randomized';
}
export function isUtlsField(key: string): boolean { return key === 'fp' || key === 'stlsFp'; }

/** Keep an unsupported current policy visible; never rewrite persisted intent. */
export function globalUtlsOptions(current: string | undefined, snapshot?: TlsFingerprintPresentation): readonly Option[] {
  const options: Option[] = [['follow', 'settings.utls.follow']];
  if (presentationDocument(snapshot)?.enabled !== false || current === 'default') options.push(['default', 'node.utls.default']);
  options.push(...advertisedProfiles(snapshot));
  if (current === 'randomized') options.push(...savedUtlsOption(current));
  return current && !options.some(([value]) => value === current)
    ? [...options, [current, current] as const] : options;
}

export function realityUtlsError(server: Pick<ServerConfig, 'security' | 'tlsSettings'>): string | null {
  const tls = server.tlsSettings;
  return server.security === 'reality' && (tls?.utlsMode === 'disabled'
    || tls?.utlsMode !== 'default' && tls?.fingerprint?.trim().toLowerCase() === 'none')
    ? 'node.utls.realityRequired' : null;
}
export function tlsFingerprintStatus(snapshot: TlsFingerprintPresentation | undefined, t: (key: string) => string): string | null {
  if (!snapshot) return t('node.utls.loading');
  // Keep native refusal details in the shared snapshot for diagnostics, rather
  // than presenting executor internals as instructions under every selector.
  if (!presentationDocument(snapshot)) return t('node.utls.unavailable');
  return 'evidenceKind' in snapshot ? t('node.utls.selectedSupport') : null;
}
