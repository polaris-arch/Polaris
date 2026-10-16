import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { GLOBAL_UTLS_OPTIONS, NODE_UTLS_OPTIONS, UTLS_PROFILE_OPTIONS, utlsDraftValue, utlsPatch } from './utls';

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
  it('puts Random before browsers and sorts supported browser labels naturally', () => {
    expect(GLOBAL_UTLS_OPTIONS.slice(0, 3).map(([value]) => value)).toEqual(['follow', 'default', 'random']);
    expect(NODE_UTLS_OPTIONS.slice(0, 4).map(([value]) => value)).toEqual(['', '@default', 'none', 'random']);
    const names = UTLS_PROFILE_OPTIONS.slice(2).map(([, label]) => label);
    expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b, 'en', { numeric: true, sensitivity: 'base' })));
    expect(UTLS_PROFILE_OPTIONS.some(([value]) => value === 'chrome_133' || value === 'safari_26_3')).toBe(false);
  });
});

it('all visible profiles exist in the backend admission list', () => {
  const source = readFileSync(new URL('../../../crates/config-engine/src/user_config/utls.rs', import.meta.url), 'utf8');
  const block = source.match(/pub const SUPPORTED_PROFILES:[^=]+=\s*&\[([\s\S]*?)\];/);
  expect(block, 'backend capability list must remain resolvable').not.toBeNull();
  const actual = new Set([...block![1].matchAll(/"([^"]+)"/g)].map((match) => match[1]));
  for (const [profile] of UTLS_PROFILE_OPTIONS) expect(actual.has(profile), profile).toBe(true);
});
