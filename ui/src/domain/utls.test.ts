import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { globalUtlsOptions, nodeUtlsOptions, nativeUtlsUnsupported, realityUtlsError, tlsFingerprintStatus, type TlsFingerprintSnapshot, type SelectedTlsSupportDirectory, utlsDraftValue, utlsPatch } from './utls';

describe('uTLS intent and shared menus', () => {
  it('preserves disabled dormant fingerprint and enabled core default on unchanged edit', () => {
    for (const base of [undefined, { fingerprint: 'firefox' },
      { utlsMode: 'disabled' as const, fingerprint: 'chrome' },
      { utlsMode: 'default' as const }, { utlsMode: 'profile' as const, fingerprint: 'safari' }]) {
      expect(utlsPatch(utlsDraftValue(base), base)).toEqual({ utlsMode: base?.utlsMode, fingerprint: base?.fingerprint });
    }
  });
  it('distinguishes unset, core default, disabled and a concrete profile', () => {
    const base = { fingerprint: 'chrome' };
    expect(utlsPatch('', base)).toEqual({ utlsMode: undefined, fingerprint: undefined });
    expect(utlsPatch('@default', base)).toEqual({ utlsMode: 'default', fingerprint: undefined });
    expect(utlsPatch('none', base)).toEqual({ utlsMode: 'disabled', fingerprint: undefined });
    expect(utlsPatch('firefox', base)).toEqual({ utlsMode: 'profile', fingerprint: 'firefox' });
  });
});

const report = (ids: string[]): TlsFingerprintSnapshot => ({
  observationScope: 'cli-probe-process', byteIdentityScope: 'cli-executable', identity: 'a'.repeat(64), binarySha256: 'b'.repeat(64), unavailableReason: null,
  document: { schema: 'polaris-tls-fingerprint-capabilities-v1', enabled: true,
    coreVersion: 'fixture', sourceCommit: '', sourceModified: null, goVersion: 'fixture', requiredGoVersion: 'fixture',
    buildTags: 'with_utls', os: 'linux', arch: 'amd64', modules: [], defaultFingerprint: 'chrome',
    legacyChromeAliases: ['chrome_pq'], curvePreferencesSupported: false, shadowTLSSessionIDWithECHSupported: false,
    realityX25519MLKEM768: 'removed', randomSemantics: 'process', randomizedSemantics: 'process seed', negotiationEvidence: 'offer only',
    profiles: ids.map(id => ({ id, displayName: id.replaceAll('_', ' '), client: id.split('_')[0],
      templateVersion: id.split('_').slice(1).join('.'), selection: id === 'random' ? 'process-browser' : 'profile',
      templateOffersX25519MLKEM768: null })),
  },
});
const directory = (): SelectedTlsSupportDirectory => ({
  evidenceKind: 'compiled-selected-support-v1', unavailableReason: null,
  selectedSpec: { platform: 'mac-arm64', target: 'darwin/arm64', sourceCommit: 'c'.repeat(40), sourceTree: 'd'.repeat(40),
    version: 'fixture', contractSha256: 'e'.repeat(64), coreBytes: 1, coreSha256: 'b'.repeat(64),
    dataOrigin: 'observed-cli-metadata', observationStdoutSha256: 'f'.repeat(64), sourceArchiveSha256: '1'.repeat(64), sourceIndexSha256: '2'.repeat(64), goVersion: 'fixture', requiredGoVersion: 'fixture', buildTags: 'with_utls' },
  document: { schema: 'polaris-tls-support-directory-v1', enabled: true, os: 'darwin', arch: 'arm64',
    defaultFingerprint: 'chrome', legacyChromeAliases: ['chrome_pq'], randomSemantics: 'process browser',
    randomizedSemantics: 'process seed', negotiationEvidence: 'not a completed handshake', profiles: [
      { id: 'random', displayName: 'Random browser', selection: 'process-browser', client: null, templateVersion: null, templateOffersX25519MLKEM768: null },
      { id: 'chrome_155', displayName: 'Chrome 155', selection: 'profile', client: null, templateVersion: null, templateOffersX25519MLKEM768: null },
    ] },
});

it.each([
  ['linux', 'linux', 'amd64', 'observed-cli-metadata'],
  ['mac-arm64', 'darwin', 'arm64', 'observed-cli-metadata'],
  ['win', 'windows', 'amd64', 'source-derived-stable-catalog'],
  ['mac-x64', 'darwin', 'amd64', 'source-derived-stable-catalog'],
] as const)('presents %s selected support without requiring installed or running byte evidence', (platform, os, arch, origin) => {
  const view = directory();
  view.selectedSpec!.platform = platform;
  view.selectedSpec!.target = `${os}/${arch}`;
  view.selectedSpec!.dataOrigin = origin;
  view.selectedSpec!.observationStdoutSha256 = origin === 'observed-cli-metadata' ? 'f'.repeat(64) : null;
  view.document!.os = os;
  view.document!.arch = arch;
  const before = structuredClone(view);
  expect(nodeUtlsOptions(false, view).map(([id]) => id)).toEqual(['', 'none', 'random', 'chrome_155']);
  expect(globalUtlsOptions('follow', view).map(([id]) => id)).toEqual(['follow', 'default', 'random', 'chrome_155']);
  expect(tlsFingerprintStatus(view, key => key)).toBe('node.utls.selectedSupport');
  expect(nativeUtlsUnsupported('apple', view)).toBe(os === 'darwin');
  expect(nativeUtlsUnsupported('windows', view)).toBe(os === 'windows');
  expect(nativeUtlsUnsupported('apple', view, true)).toBe(false);
  expect(view).not.toHaveProperty('identity');
  expect(view).not.toHaveProperty('binarySha256');
  expect(view).toEqual(before);
});

it('keeps saved intent when the directory is unavailable or its evidence kind is unknown', () => {
  for (const view of [
    { ...directory(), selectedSpec: null, document: null, unavailableReason: 'unavailable fixture' },
    { ...directory(), evidenceKind: 'future' } as unknown as SelectedTlsSupportDirectory,
    { ...directory(), document: { ...directory().document!, schema: 'future' } } as unknown as SelectedTlsSupportDirectory,
  ]) {
    expect(nodeUtlsOptions(false, view).map(([id]) => id)).toEqual(['', 'none']);
    expect(tlsFingerprintStatus(view, key => key)).toBe('node.utls.unavailable');
    expect(globalUtlsOptions('firefox_120', view).slice(-1)[0]).toEqual(['firefox_120', 'firefox_120']);
    expect(nodeUtlsOptions(false, view, false, 'randomized').slice(-1)[0]).toEqual(['randomized', 'node.utls.savedRandomized']);
    const base = { utlsMode: 'disabled' as const, fingerprint: 'firefox_120' };
    expect(utlsPatch(utlsDraftValue(base), base)).toEqual(base);
  }
});
it('menus reflect only actual profiles, preserve existing values, and change on kernel replacement', () => {
  expect(nodeUtlsOptions(false).map(([id]) => id)).toEqual(['', 'none']);
  const next = report(['chrome_155', 'safari_26_3', 'safari_16_0', 'random']);
  expect(nodeUtlsOptions(false, next).map(([id]) => id)).toEqual(['', 'none', 'random', 'chrome_155', 'safari_16_0', 'safari_26_3']);
  expect(nodeUtlsOptions(true, next).some(([id]) => id === 'none')).toBe(true);
  expect(nodeUtlsOptions(false, report(['chrome_133'])).some(([id]) => id === 'chrome_155')).toBe(false);
  expect(globalUtlsOptions('safari_26_3').slice(-1)[0]).toEqual(['safari_26_3', 'safari_26_3']);
  expect(next.document?.sourceModified).toBeNull();
  expect(nodeUtlsOptions(false, next).some(([id]) => id === 'chrome_pq')).toBe(false);
});

it('disables incompatible REALITY intent and rejects unchanged historical conflicts without rewriting them', () => {
  expect(nodeUtlsOptions(false, undefined, true).find(([id]) => id === 'none')?.[2]).toBe(true);
  for (const tlsSettings of [{ utlsMode: 'disabled' as const }, { fingerprint: 'none' }]) {
    const node = { security: 'reality' as const, tlsSettings };
    const before = structuredClone(node);
    expect(realityUtlsError(node)).toBe('node.utls.realityRequired');
    expect(node).toEqual(before);
    expect(realityUtlsError({ ...node, security: 'tls' })).toBeNull();
  }
  expect(realityUtlsError({ security: 'reality', tlsSettings: { utlsMode: 'default', fingerprint: 'none' } })).toBeNull();
});
it('distinguishes loading, explicit failure, and a supported report', () => {
  const t = (key: string, options?: { reason: string }) => options ? `${key}: ${options.reason}` : key;
  expect(tlsFingerprintStatus(undefined, t)).toBe('node.utls.loading');
  const reason = 'CORE_ADMISSION_UnsupportedExactObject: pre-execution object protection unavailable';
  const unavailable = { ...report([]), unavailableReason: reason };
  expect(tlsFingerprintStatus(unavailable, t)).toBe('node.utls.unavailable');
  expect(unavailable.unavailableReason).toBe(reason);
  expect(tlsFingerprintStatus({ ...report([]), identity: null, document: null }, t)).toBe('node.utls.unavailable');
  expect(tlsFingerprintStatus(report(['safari_26_3']), t)).toBeNull();
});


it('keeps removed choices visible only for the saved value without migrating it', () => {
  const snapshot = report(['random', 'randomized', 'chrome']);
  expect(nodeUtlsOptions(false, snapshot).map(([id]) => id)).toEqual(['', 'none', 'random', 'chrome']);
  expect(nodeUtlsOptions(false, snapshot, false, '@default').slice(-1)[0]).toEqual(['@default', 'node.utls.savedCoreDefault']);
  expect(nodeUtlsOptions(false, snapshot, false, 'randomized').slice(-1)[0]).toEqual(['randomized', 'node.utls.savedRandomized']);
  expect(globalUtlsOptions('follow', snapshot).some(([id]) => id === 'randomized')).toBe(false);
  expect(globalUtlsOptions('randomized', snapshot).slice(-1)[0]).toEqual(['randomized', 'node.utls.savedRandomized']);
  expect(globalUtlsOptions('default', snapshot).find(([id]) => id === 'default')).toEqual(['default', 'node.utls.default']);
  for (const base of [{ utlsMode: 'default' as const }, { utlsMode: 'profile' as const, fingerprint: 'randomized' }]) {
    expect(utlsPatch(utlsDraftValue(base), base)).toEqual({ utlsMode: base.utlsMode, fingerprint: base.fingerprint });
  }
});
it('uses localized Random and concise browser aliases without changing pinned labels or report data', () => {
  const snapshot = report(['chrome_155']);
  const browsers = [['chrome', 'Chrome'], ['firefox', 'Firefox'], ['edge', 'Edge'], ['safari', 'Safari'], ['360', '360'], ['qq', 'QQ'], ['ios', 'iOS']];
  snapshot.document!.profiles = [...snapshot.document!.profiles,
    ...browsers.map(([id, name]) => ({ id, displayName: `${name} (Auto)`, client: name,
      templateVersion: '155', selection: 'profile' as const, templateOffersX25519MLKEM768: null })),
    { id: 'random', displayName: 'Random browser', client: 'Chrome', templateVersion: '155',
      selection: 'process-browser', templateOffersX25519MLKEM768: null },
  ];
  const before = structuredClone(snapshot);
  for (const options of [nodeUtlsOptions(false, snapshot), nodeUtlsOptions(true, snapshot), globalUtlsOptions('follow', snapshot)]) {
    for (const [id, label] of browsers) expect(options.find(([value]) => value === id)?.[1]).toBe(label);
    expect(options.find(([id]) => id === 'random')?.[1]).toBe('node.utls.random');
    expect(options.find(([id]) => id === 'chrome_155')?.[1]).toBe('chrome 155 (155)');
  }
  expect(snapshot).toEqual(before);
});
it('ships default, off, Random and saved-value explanations in all five locales', () => {
  for (const locale of ['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa']) {
    const text = JSON.parse(readFileSync(new URL(`../i18n/locales/${locale}.json`, import.meta.url), 'utf8'));
    for (const key of ['legacy', 'disabled', 'random', 'description', 'savedCoreDefault', 'savedRandomized', 'savedHint', 'engineUnsupported', 'unavailable', 'selectedSupport']) {
      expect(text.node.utls[key], `${locale}:${key}`).toBeTruthy();
    }
    expect(text.node.utls.unavailable, `${locale}: user status must not interpolate native diagnostics`).not.toContain('{{reason}}');
    if (locale !== 'en-US') expect(text.node.utls.random).not.toBe('Random');
  }
});

it('uses actual native-engine capabilities without rejecting dormant engines or REALITY', () => {
  const snapshot = report(['random', 'chrome']);
  for (const [os, engine] of [['darwin', 'apple'], ['windows', 'windows']]) {
    snapshot.document!.os = os;
    expect(nativeUtlsUnsupported(engine, snapshot)).toBe(true);
    expect(nativeUtlsUnsupported(engine, snapshot, true)).toBe(false);
    const opts = nodeUtlsOptions(false, snapshot, false, undefined, engine);
    expect(opts.find(([id]) => id === '')?.[2]).not.toBe(true);
    expect(opts.find(([id]) => id === 'none')?.[2]).not.toBe(true);
    expect(opts.find(([id]) => id === 'chrome')?.[2]).toBe(true);
    expect(opts.find(([id]) => id === 'random')?.[2]).toBe(true);
    expect(nodeUtlsOptions(true, snapshot, false, undefined, engine).find(([id]) => id === 'chrome')?.[2]).not.toBe(true);
  }
  snapshot.document!.os = 'linux';
  expect(nativeUtlsUnsupported('apple', snapshot)).toBe(false);
  expect(nativeUtlsUnsupported('windows', snapshot)).toBe(false);
  expect(nativeUtlsUnsupported('apple')).toBe(false);
});

it('starts with Follow without replacing an explicit Default selection in both settings screens', () => {
  expect(globalUtlsOptions(undefined).map(([id]) => id)).toEqual(['follow', 'default']);
});
