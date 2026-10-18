#!/usr/bin/env node
/** Public release names and exact Android ABI inventory. No package execution. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const ASSET_SUFFIXES = Object.freeze({
  windows: ['x64-win-setup.exe', 'x64-win-Portable.zip'],
  'macos-x64': ['x64-mac.dmg'],
  'macos-arm64': ['aarch64-mac.dmg'],
  linux: ['amd64-linux.AppImage', 'amd64-linux.deb'],
  android: ['arm64-v8a-android.apk', 'armeabi-v7a-android.apk', 'universal-android.apk'],
});
export const ANDROID_ABIS = Object.freeze({
  'arm64-v8a': ['arm64-v8a'],
  'armeabi-v7a': ['armeabi-v7a'],
  universal: ['arm64-v8a', 'armeabi-v7a'],
});
export function assetNames(version, label) {
  assert.match(version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/, 'invalid release version');
  const labels = label === 'release' ? ['windows', 'macos-x64', 'macos-arm64', 'linux']
    : label === 'release-all' ? Object.keys(ASSET_SUFFIXES) : [label];
  assert.ok(labels.every((key) => ASSET_SUFFIXES[key]), `unknown asset label: ${label}`);
  return labels.flatMap((key) => ASSET_SUFFIXES[key].map((suffix) => `Polaris_${version}_${suffix}`));
}
export function assetMatches(name, suffix) {
  return name.startsWith('Polaris_') && name.endsWith(`_${suffix}`) &&
    /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(name.slice(8, -suffix.length - 1));
}
export function verifyAbiNames(names, flavor) {
  const expected = ANDROID_ABIS[flavor];
  assert.ok(expected, `unknown Android release flavor: ${flavor}`);
  const native = names.filter((name) => /^lib\/[^/]+\/[^/]+\.so$/.test(name));
  const actual = [...new Set(native.map((name) => name.split('/')[1]))].sort();
  assert.deepEqual(actual, [...expected].sort(), 'APK actual ABI set differs from release contract');
  for (const abi of expected) {
    for (const library of ['libbox.so', 'libpolaris_lib.so', 'libc++_shared.so']) {
      assert.equal(names.filter((name) => name === `lib/${abi}/${library}`).length, 1,
        `APK must contain exactly one lib/${abi}/${library}`);
    }
  }
  assert.equal(new Set(names).size, names.length, 'duplicate APK ZIP members');
  return expected;
}
export function verifyAndroidDigests(version, verified, sums) {
  assert.ok(verified && typeof verified === 'object' && !Array.isArray(verified), 'verified APK digests must be an object');
  assert.deepEqual(Object.keys(verified).sort(), assetNames(version, 'android').sort(), 'verified APK asset set differs');
  for (const [name, sha256] of Object.entries(verified)) {
    assert.match(sha256, /^[a-f0-9]{64}$/, `invalid verified digest: ${name}`);
    const rows = sums.split('\n').filter((line) => line.slice(66) === name);
    assert.deepEqual(rows, [`${sha256}  ${name}`], `final manifest differs from verified APK: ${name}`);
  }
}
function main() {
  const [mode, ...args] = process.argv.slice(2);
  if (mode === 'names' && args.length === 2) console.log(assetNames(...args).join('\n'));
  else if (mode === 'abis' && args.length === 2) {
    const names = execFileSync('unzip', ['-Z1', args[0]], { encoding: 'utf8' }).trimEnd().split('\n');
    verifyAbiNames(names, args[1]);
    console.log(`APK ABI inventory verified: ${args[1]}`);
  } else if (mode === 'android-digests' && args.length === 3) {
    verifyAndroidDigests(args[0], JSON.parse(args[1]), readFileSync(args[2], 'utf8'));
  } else throw new Error('usage: release-assets.mjs names <version> <label> | abis <apk> <flavor> | android-digests <version> <JSON> <SHA256SUMS>');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
