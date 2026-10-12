import { test } from 'node:test';
import assert from 'node:assert/strict';
import { coreIdentityId, canonicalCoreIdentity, selectedCoreManifest } from './lib/core-identity.mjs';

const core = { platform: 'linux', version: '1.15.0-alpha.12.polaris.1', providerFingerprint: 'a'.repeat(64),
  files: [{ name: 'sing-box', bytes: 10, sha256: 'b'.repeat(64) }, { name: 'libcronet.so', bytes: 20, sha256: 'c'.repeat(64) }], helperSha256: null };
test('framed identity is stable across file order and helper installation metadata', () => {
  assert.equal(coreIdentityId(core), coreIdentityId({ ...core, files: [...core.files].reverse(), helperSha256: 'd'.repeat(64) }));
  const changed = structuredClone(core); changed.files[0].bytes++;
  assert.notEqual(coreIdentityId(core), coreIdentityId(changed));
});
test('unknown payload, absent sidecar and invalid id inputs are refused', () => {
  for (const name of ['../sing-box', 'polaris-helper', 'libcronet.dll.']) {
    const changed = structuredClone(core); changed.files[0].name = name;
    assert.throws(() => canonicalCoreIdentity(changed));
  }
  assert.throws(() => canonicalCoreIdentity({ ...core, files: core.files.slice(0, 1) }));
});
test('selected input preserves actual producer hashes without synthesizing helper bytes', () => {
  const row = { providerFingerprint: core.providerFingerprint, core: { member: 'sing-box', ...core.files[0] },
    sidecar: { member: 'libcronet.so', ...core.files[1] } };
  const value = selectedCoreManifest({ version: core.version, platforms: { linux: row } }, 'linux', '1.0.0');
  assert.equal(value.core.helperSha256, null);
  assert.equal(value.id, coreIdentityId(core));
});
