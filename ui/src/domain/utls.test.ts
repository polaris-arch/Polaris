import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { globalUtlsOptions, nodeUtlsOptions, nativeUtlsUnsupported, realityUtlsError, tlsFingerprintStatus, type TlsFingerprintSnapshot, GLOBAL_UTLS_OPTIONS, NODE_UTLS_OPTIONS, utlsDraftValue, utlsPatch } from './utls';

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
  it('does not advertise browser profiles without an actual core report', () => {
    expect(GLOBAL_UTLS_OPTIONS.map(([value]) => value)).toEqual(['follow', 'default']);
    expect(NODE_UTLS_OPTIONS.map(([value]) => value)).toEqual(['', 'none']);
  });
});

it('all visible profiles exist in the backend admission list', () => {
  const source = readFileSync(new URL('../../../crates/config-engine/src/user_config/utls.rs', import.meta.url), 'utf8');
  const block = source.match(/pub const SUPPORTED_PROFILES:[^=]+=\s*&\[([\s\S]*?)\];/);
  expect(block, 'backend capability list must remain resolvable').not.toBeNull();
  const actual = new Set([...block![1].matchAll(/"([^"]+)"/g)].map((match) => match[1]));
  for (const profile of ['random', 'randomized', 'chrome_155', 'chrome_133', 'firefox_148', 'firefox_120', 'safari_26_3', 'safari_16_0']) expect(actual.has(profile), profile).toBe(true);
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
  expect(tlsFingerprintStatus({ ...report([]), unavailableReason: 'old API' }, t)).toContain('old API');
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
    for (const key of ['legacy', 'disabled', 'random', 'description', 'savedCoreDefault', 'savedRandomized', 'savedHint', 'engineUnsupported']) {
      expect(text.node.utls[key], `${locale}:${key}`).toBeTruthy();
    }
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

it('keeps global Follow distinct from the initial Default selection in both settings screens', () => {
  for (const file of ['../components/screens/settings/SettingsNetwork.tsx', '../mobile/settings/NetworkPage.tsx']) {
    const source = readFileSync(new URL(file, import.meta.url), 'utf8');
    expect(source).toContain("config.utlsPolicy ?? 'default'");
    expect(source).not.toContain("config.utlsPolicy ?? 'follow'");
  }
  expect(globalUtlsOptions(undefined).map(([id]) => id)).toEqual(['follow', 'default']);
});
