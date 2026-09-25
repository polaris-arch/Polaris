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

test('managed build rejects missing output pins before downloading or replacing anything', () => {
  for (const binarySha256 of ['', undefined, 'abc']) {
    assert.throws(() => buildWindowsCore(root,
      { ...manifest, windowsBuild: { ...manifest.windowsBuild, binarySha256 } },
      '/must-not-be-written', false), /Invalid pinned/);
  }
});

test('patch changes require a new pin even when a cached core exists', () => {
  assert.throws(() => buildWindowsCore(root,
    { ...manifest, windowsBuild: { ...manifest.windowsBuild, patchSha256: '0'.repeat(64) } },
    join(root, 'resources/win/sing-box.exe'), false), /SHA-256 mismatch/);
  verifyHash(join(root, 'scripts/core-patches/windows-dns-refresh.patch'), manifest.windowsBuild.patchSha256);
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
