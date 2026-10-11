import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { buildWindowsCore, verifyHash } from './lib/build-windows-core.mjs';
import { classifyImpact } from './classify-ci-impact.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
// Active D intentionally has no legacy Windows overlay. Keep the retired
// builder's patch-rejection fixture complete; never reactivate it in production.
const legacyWindow = { sourceCommit: 'a'.repeat(40), goVersion: '1.25.5',
  version: `${manifest.bundledCoreVersion}.polaris.1`,
  patchSha256: createHash('sha256').update(readFileSync(join(root, 'scripts/core-patches/windows-dns-refresh.patch'))).digest('hex') };

test('managed build rejects missing output pins before downloading or replacing anything', () => {
  for (const binarySha256 of [null, '', undefined, 'abc']) {
    assert.throws(() => buildWindowsCore(root,
      { ...manifest, windowsBuild: { ...legacyWindow, binarySha256 } },
      '/must-not-be-written', false), /Invalid pinned/);
  }
});

test('patch changes require a new pin even when a cached core exists', () => {
  const work = mkdtempSync(join(tmpdir(), 'polaris-windows-patch-test-'));
  try {
    const cachedCore = Buffer.from('MZ synthetic cached Windows core');
    const dest = join(work, 'sing-box.exe');
    writeFileSync(dest, cachedCore);
    // Source-only manifests leave output pins null. This complete mock pin
    // lets the test reach the patch gate without weakening the null-pin gate.
    const pinned = { ...manifest, windowsBuild: { ...legacyWindow,
      binarySha256: createHash('sha256').update(cachedCore).digest('hex'),
      patchSha256: '0'.repeat(64) } };
    assert.throws(() => buildWindowsCore(root, pinned, dest, false,
      () => assert.fail('Patch mismatch must reject before invoking native tools')), /SHA-256 mismatch/);
    assert.deepEqual(readFileSync(dest), cachedCore);
    verifyHash(join(root, 'scripts/core-patches/windows-dns-refresh.patch'), legacyWindow.patchSha256);
  } finally { rmSync(work, { recursive: true, force: true }); }
});

test('same-size executable corruption fails SHA verification', () => {
  const work = mkdtempSync(join(tmpdir(), 'polaris-hash-test-'));
  try {
    const file = join(work, 'core');
    writeFileSync(file, 'original');
    const hash = createHash('sha256').update('original').digest('hex');
    verifyHash(file, hash);
    writeFileSync(file, 'tampered');
    assert.throws(() => verifyHash(file, hash), /SHA-256 mismatch/);
  } finally { rmSync(work, { recursive: true, force: true }); }
});

test('changing the Windows patch or builder triggers native packaging and core gates', () => {
  for (const path of ['scripts/core-patches/windows-dns-refresh.patch', 'scripts/lib/build-windows-core.mjs']) {
    const impact = classifyImpact([path]);
    assert.equal(impact.kernel, true);
    assert.ok(impact.platforms.includes('windows'));
  }
});
