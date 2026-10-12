import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, copyFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { coreIdentityId, canonicalCoreIdentity, selectedCoreManifest, writeSelectedCoreManifest, verifySelectedCoreManifest } from './lib/core-identity.mjs';

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

test('final installer hashes reject changed helper, payload and self-consistent replacement manifests', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'polaris-installed-core-identity-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = join(root, 'source'), final = join(root, 'final'); mkdirSync(source); mkdirSync(final);
  const hash = (value) => createHash('sha256').update(value).digest('hex');
  const data = { 'sing-box': Buffer.from('core bytes'), 'libcronet.so': Buffer.from('library bytes'), 'polaris-helper': Buffer.from('helper bytes') };
  for (const [name, raw] of Object.entries(data)) { writeFileSync(join(source, name), raw); writeFileSync(join(final, name), raw); }
  const file = (member) => ({ member, bytes: data[member].length, sha256: hash(data[member]) });
  const contract = { version: '1.15.0-alpha.12', platforms: { linux: { providerFingerprint: 'a'.repeat(64), core: file('sing-box'), sidecar: file('libcronet.so') } } };
  writeSelectedCoreManifest(source, contract, 'linux', '1.0.0');
  copyFileSync(join(source, 'core-manifest.json'), join(final, 'core-manifest.json'));
  assert.equal(verifySelectedCoreManifest(final, source, 'linux').core.helperSha256, hash(data['polaris-helper']));
  for (const name of ['polaris-helper', 'sing-box', 'libcronet.so']) {
    writeFileSync(join(final, name), 'altered bytes');
    assert.throws(() => verifySelectedCoreManifest(final, source, 'linux'));
    writeFileSync(join(final, name), data[name]);
  }
  const replacement = JSON.parse(readFileSync(join(final, 'core-manifest.json')));
  replacement.core.helperSha256 = hash('altered bytes');
  writeFileSync(join(final, 'polaris-helper'), 'altered bytes');
  writeFileSync(join(final, 'core-manifest.json'), JSON.stringify(replacement));
  assert.throws(() => verifySelectedCoreManifest(final, source, 'linux'), /compiled installation input/);
});
