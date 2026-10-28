import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { assetNames, assetMatches, verifyAbiNames, verifyAndroidDigests } from './release-assets.mjs';

const expected = [
  'Polaris_1.0.0_x64-win-setup.exe', 'Polaris_1.0.0_x64-win-Portable.zip',
  'Polaris_1.0.0_x64-mac.dmg', 'Polaris_1.0.0_aarch64-mac.dmg',
  'Polaris_1.0.0_amd64-linux.AppImage', 'Polaris_1.0.0_amd64-linux.deb',
  'Polaris_1.0.0_arm64-v8a-android.apk', 'Polaris_1.0.0_armeabi-v7a-android.apk',
  'Polaris_1.0.0_universal-android.apk',
];
const libraries = (abis) => abis.flatMap((abi) => ['libbox.so', 'libpolaris_lib.so', 'libc++_shared.so'].map((lib) => `lib/${abi}/${lib}`));
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

test('public asset inventory is exactly the nine agreed names, with no iOS or x86 package', () => {
  assert.deepEqual(assetNames('1.0.0', 'release-all'), expected);
  assert.deepEqual(assetNames('1.0.0', 'release'), expected.slice(0, 6));
  assert.deepEqual(assetNames('1.0.0', 'android'), expected.slice(6));
});
test('version and label admission rejects path injection, run labels and unknown platforms', () => {
  for (const version of ['', 'v1.0.0', '1.0', '../1.0.0', 'run-123']) assert.throws(() => assetNames(version, 'release-all'));
  assert.throws(() => assetNames('1.0.0', 'ios'));
});
test('selector mirror requires exact version, case, suffix and architecture order', () => {
  assert.ok(assetMatches('Polaris_1.0.0-rc.1_x64-win-Portable.zip', 'x64-win-Portable.zip'));
  for (const name of ['polaris-portable-v1.0.0.zip', 'Polaris_1.0.0_x64-win-portable.zip',
    'Polaris__x64-win-Portable.zip', 'Polaris_1.0.0_x64-win-Portable.zip.1',
    'Polaris_1.0.0_x64-mac-x64.dmg']) assert.equal(assetMatches(name, 'x64-win-Portable.zip'), false);
});
test('each Android flavor accepts its actual native ABI set', () => {
  verifyAbiNames(libraries(['arm64-v8a']), 'arm64-v8a');
  verifyAbiNames(libraries(['armeabi-v7a']), 'armeabi-v7a');
  verifyAbiNames(libraries(['arm64-v8a', 'armeabi-v7a']), 'universal');
});
test('Android universal rejects either missing ARM ABI and any x86 or x86_64 payload', () => {
  for (const abis of [['arm64-v8a'], ['armeabi-v7a'], ['arm64-v8a', 'armeabi-v7a', 'x86'], ['arm64-v8a', 'armeabi-v7a', 'x86_64']]) {
    assert.throws(() => verifyAbiNames(libraries(abis), 'universal'), /ABI set/);
  }
  assert.throws(() => verifyAbiNames(libraries(['arm64-v8a', 'armeabi-v7a']), 'arm64-v8a'), /ABI set/);
});
test('ABI inventory rejects absent native runtime libraries, duplicate members and unknown flavors', () => {
  const names = libraries(['arm64-v8a', 'armeabi-v7a']);
  for (const member of names) assert.throws(() => verifyAbiNames(names.filter((name) => name !== member), 'universal'));
  assert.throws(() => verifyAbiNames([...names, 'assets/duplicate', 'assets/duplicate'], 'universal'), /duplicate/);
  assert.throws(() => verifyAbiNames([], 'x86_64'));
});
test('final manifest binds all three APKs to their fixed previously verified SHA values', () => {
  const verified = Object.fromEntries(expected.slice(6).map((name) => [name, digest(name)]));
  const sums = Object.entries(verified).map(([name, sha]) => `${sha}  ${name}`).join('\n');
  verifyAndroidDigests('1.0.0', verified, sums);
  for (const name of Object.keys(verified)) {
    assert.throws(() => verifyAndroidDigests('1.0.0', verified, sums.replace(verified[name], 'b'.repeat(64))));
    assert.throws(() => verifyAndroidDigests('1.0.0', verified, sums + `\n${verified[name]}  ${name}`));
    assert.throws(() => verifyAndroidDigests('1.0.0', Object.fromEntries(Object.entries(verified).filter(([key]) => key !== name)), sums));
  }
  assert.throws(() => verifyAndroidDigests('1.0.0', { ...verified, 'extra.apk': 'a'.repeat(64) }, sums));
});
test('complete packaging gate rejects old names, stale versions, extra files, missing APK and stale sums', () => {
  const directory = mkdtempSync(join(tmpdir(), 'polaris-release-contract-'));
  try {
    for (const name of expected) writeFileSync(join(directory, name), `fixture asset: ${name}`);
    const sums = expected.map((name) => `${digest(readFileSync(join(directory, name)))}  ${name}`).join('\n') + '\n';
    writeFileSync(join(directory, 'SHA256SUMS'), sums);
    const run = () => spawnSync(process.execPath, ['scripts/verify-packaging.mjs', 'assets', '--label', 'release-all', '--dir', directory], { encoding: 'utf8' });
    const good = run();
    assert.equal(good.status, 0, good.stdout + good.stderr);
    for (const extra of ['polaris-portable-v1.0.0.zip', 'Polaris_0.9.0_x64-mac.dmg', 'extra.txt']) {
      writeFileSync(join(directory, extra), 'extra');
      assert.equal(run().status, 1, extra);
      rmSync(join(directory, extra));
    }
    for (const name of expected.slice(6)) {
      rmSync(join(directory, name));
      assert.equal(run().status, 1, name);
      writeFileSync(join(directory, name), `fixture asset: ${name}`);
    }
    writeFileSync(join(directory, expected[8]), 'tampered APK bytes');
    assert.equal(run().status, 1);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
