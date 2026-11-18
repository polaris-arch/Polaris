#!/usr/bin/env node
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, constants, fstatSync, lstatSync, mkdirSync, mkdtempDisposableSync, openSync,
  readFileSync, readSync, realpathSync, readdirSync, renameSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const formalProfile = fileURLToPath(new URL('./ndk-r30-linux.json', import.meta.url));
const formalProfileSha256 = 'db468c29e87569f09ca04dc3b3d2137554124cf755ebdf68371cfdff466f6a30';
const archivePath = relative(fileURLToPath(new URL('../..', import.meta.url)), formalProfile);
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

function realPath(input, directory = false) {
  const path = resolve(input);
  assert.equal(realpathSync(path), path, 'formal NDK input or slot cannot be redirected');
  const stat = lstatSync(path);
  assert.ok(directory ? stat.isDirectory() : stat.isFile(), 'formal NDK input type differs: ' + path);
  return path;
}

function ndkProperties(raw) {
  const values = new Map();
  for (const line of utf8.decode(raw).split(/\r\n?|\n/)) {
    if (!line.trim() || /^[#!]/.test(line.trimStart())) continue;
    const assignment = line.indexOf('=');
    assert.ok(assignment >= 0, 'NDK source.properties field malformed');
    const key = line.slice(0, assignment).trim();
    assert.ok(key && !values.has(key), 'NDK source.properties duplicate/empty key');
    values.set(key, line.slice(assignment + 1).trim());
  }
  const revision = values.get('Pkg.Revision');
  assert.ok(revision && /^[0-9]+\.[0-9]+\.[0-9]+(?:-(?:alpha|beta|rc)[0-9]+)?$/.test(revision),
    'NDK source.properties revision malformed/missing');
  return { revision, baseRevision: values.get('Pkg.BaseRevision') ?? null, releaseName: values.get('Pkg.ReleaseName') };
}

function validateNdkProperties({ revision, baseRevision, releaseName }, profile) {
  assert.ok(revision === profile.ndkSourceRevision && baseRevision === profile.ndkRevision
    && releaseName === profile.ndkPackage.slice('android-ndk-'.length), 'actual pinned NDK revision/base/release differs');
}

function retain(path, raw) {
  const existing = lstatSync(path, { throwIfNoEntry: false });
  if (existing) {
    assert.ok(existing.isFile() && readFileSync(path).equals(raw), 'previous NDK source observation differs: ' + path);
  } else {
    writeFileSync(path, raw, { flag: 'wx' });
  }
}

export function observeNdkInput(root, ndk, profile, profileBytes) {
  const path = realPath(join(realpathSync(ndk), 'source.properties'));
  const raw = readFileSync(path);
  retain(join(root, 'ndk-source.properties'), raw);
  const parsed = ndkProperties(raw);
  const { revision, baseRevision } = parsed;
  // Preserve the compact sorted UTF-8 bytes used in hosted tool receipts.
  const canonical = value => JSON.stringify(value, (_key, item) => item && typeof item === 'object'
    ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
  const facts = { schema: 'polaris-mobile-ndk-input-observation-v1', path,
    sourceProperties: { bytes: raw.length, sha256: sha256(raw) }, revision, requiredRevision: profile.ndkSourceRevision,
    baseRevision, requiredBaseRevision: profile.ndkRevision, package: profile.ndkPackage, archivePath,
    archiveContract: { bytes: profileBytes.length, sha256: sha256(profileBytes) },
    devicePassed: false, publicationEligible: false };
  facts.fingerprint = sha256(canonical(facts));
  retain(join(root, 'sdk-input-observation.json'), Buffer.from(canonical(facts)));
  assert.ok(readFileSync(path).equals(raw), 'NDK source.properties changed during observation');
  validateNdkProperties(parsed, profile);
  return facts;
}

function privateSlot(path, message) {
  const info = lstatSync(path);
  assert.ok(info.isDirectory() && info.uid === process.getuid() && (info.mode & 0o7777) === 0o700, message);
}

function sdkLink(link, target) {
  const info = lstatSync(link, { throwIfNoEntry: false });
  if (!info) symlinkSync(target, link, 'dir');
  else assert.ok(info.isSymbolicLink() && realpathSync(link) === target, 'retained SDK link differs: ' + link);
}

export function prepareSdkView(work, sdk, ndk, profile, bootclasspath) {
  [work, sdk, ndk] = [work, sdk, ndk].map(path => realpathSync(path));
  privateSlot(work, 'fixed private caller-owned work slot required');
  validateNdkProperties(ndkProperties(readFileSync(join(ndk, 'source.properties'))), profile);
  const jar = join(sdk, 'platforms', bootclasspath.platform, 'android.jar');
  assert.ok(statSync(jar, { throwIfNoEntry: false })?.isFile() && sha256(readFileSync(jar)) === bootclasspath.sha256,
    'actual Android bootclasspath differs');
  // The caller still holds the exclusive producer lease on this fixed view.
  const view = join(work, 'android-sdk-view');
  if (!lstatSync(view, { throwIfNoEntry: false })) mkdirSync(view, { mode: 0o700 });
  privateSlot(view, 'SDK view must be a private real directory');
  const sdkDirs = ['platforms', 'build-tools', 'platform-tools'];
  assert.ok(readdirSync(view).every(name => name === 'ndk' || sdkDirs.includes(name)), 'unexpected SDK view entry');
  for (const name of sdkDirs) {
    const target = join(sdk, name);
    assert.ok(statSync(target, { throwIfNoEntry: false })?.isDirectory(), 'actual SDK directory missing: ' + name);
    sdkLink(join(view, name), target);
  }
  const ndks = join(view, 'ndk');
  if (!lstatSync(ndks, { throwIfNoEntry: false })) mkdirSync(ndks, { mode: 0o700 });
  assert.ok(lstatSync(ndks).isDirectory() && readdirSync(ndks).every(name => name === profile.ndkRevision),
    'SDK view may contain only the pinned NDK; ambient NDK28 is forbidden');
  sdkLink(join(ndks, profile.ndkRevision), ndk);
  return [view, jar];
}

function unpackArchive(download, temp, profile) {
  const archive = profile.archive;
  const fd = openSync(realPath(download), constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const before = fstatSync(fd, { bigint: true });
    assert.ok(before.isFile() && before.size === BigInt(archive.bytes), 'formal NDK archive size/type differs');
    const sha1 = createHash('sha1'), sha256Hasher = createHash('sha256');
    const buffer = Buffer.alloc(1024 * 1024);
    let offset = 0;
    while (offset < archive.bytes) {
      const n = readSync(fd, buffer, 0, Math.min(buffer.length, archive.bytes - offset), offset);
      assert.ok(n > 0, 'formal NDK archive truncated');
      const bytes = buffer.subarray(0, n);
      sha1.update(bytes);
      sha256Hasher.update(bytes);
      offset += n;
    }
    assert.ok(sha1.digest('hex') === archive.sha1 && sha256Hasher.digest('hex') === archive.sha256,
      'formal NDK archive hashes differ');

    // Authorize the original object before invoking unzip, retaining official Unix modes and links.
    const opened = '/proc/self/fd/3';
    const options = { stdio: ['ignore', 'pipe', 'pipe', fd], maxBuffer: 32 * 1024 ** 2 };
    const listing = utf8.decode(execFileSync('unzip', ['-Z1', opened], options));
    const names = listing.replace(/\n$/, '').split('\n');
    assert.ok(new Set(names).size === names.length, 'formal NDK ZIP inventory contains duplicates');
    for (const name of names) {
      assert.ok(name.startsWith(archive.archiveRoot + '/') && !name.split('/').includes('..') && !/[\\\0\r]/.test(name),
        'formal NDK ZIP escapes its root');
    }
    execFileSync('unzip', ['-q', opened, '-d', temp], options);
    const after = fstatSync(fd, { bigint: true });
    assert.ok(after.size === before.size && after.mtimeNs === before.mtimeNs && after.ctimeNs === before.ctimeNs,
      'formal NDK archive changed during preparation');
  } finally {
    closeSync(fd);
  }
  const unpacked = realPath(join(temp, archive.archiveRoot), true);
  assert.equal(lstatSync(unpacked).uid, process.getuid(), 'formal NDK extraction ownership differs');
  const properties = realPath(join(unpacked, 'source.properties'));
  assert.ok(lstatSync(properties).size === profile.sourcePropertiesBytes
    && sha256(readFileSync(properties)) === profile.sourcePropertiesSha256, 'formal NDK source.properties bytes differ');
  return unpacked;
}

function main() {
  const { values, positionals } = parseArgs({
    options: { profile: { type: 'string' }, sdk: { type: 'string' }, ndk: { type: 'string' },
      'check-license-only': { type: 'boolean' }, 'prepare-view': { type: 'boolean' } },
    allowPositionals: true,
  });
  assert.ok(values.profile && values.sdk, 'explicit formal NDK profile and SDK required');
  const profilePath = realPath(values.profile);
  assert.equal(profilePath, formalProfile, 'exact reviewed formal NDK profile required');
  const profileBytes = readFileSync(profilePath);
  assert.equal(sha256(profileBytes), formalProfileSha256, 'formal NDK profile bytes differ');
  // This byte pin includes the exact revision, package root and source.properties digest.
  const profile = JSON.parse(utf8.decode(profileBytes));
  const [root, localArchive] = positionals;
  const licenseOnly = values['check-license-only'];
  if (values['prepare-view']) {
    assert.ok(!licenseOnly && values.ndk && positionals.length === 1, 'SDK view requires a private work slot and explicit NDK');
    const { android } = JSON.parse(readFileSync(new URL('./source-contract.json', import.meta.url)));
    console.log(JSON.stringify(prepareSdkView(root, values.sdk, values.ndk, profile, android.bootclasspath)));
    return;
  }
  assert.ok(!values.ndk && process.platform === 'linux', 'formal NDK preparation requires Linux');
  assert.ok(licenseOnly ? positionals.length === 0 : root && positionals.length <= 2,
    'license check cannot prepare a slot; extraction requires an explicit private runner slot');
  const sdk = realPath(values.sdk, true);
  const accepted = realPath(join(sdk, 'licenses', profile.licenseId));
  assert.ok(utf8.decode(readFileSync(accepted)).split(/\r\n?|\n/).includes(profile.licenseSha1),
    'standard Android SDK license is not already accepted');
  if (licenseOnly) return;

  const work = realPath(root, true);
  privateSlot(work, 'formal NDK root must be current-user private');
  const target = join(work, 'polaris-mobile-ndk-' + profile.ndkRevision);
  assert.ok(!lstatSync(target, { throwIfNoEntry: false }),
    'retained formal NDK slot refused; fresh private runner slot required');
  mkdirSync(target, { mode: 0o700 });
  const ndk = join(target, profile.archive.archiveRoot);
  {
    using scratch = mkdtempDisposableSync(join(target, 'verified-archive-'));
    const download = localArchive ?? join(scratch.path, 'ndk.zip');
    if (!localArchive) {
      execFileSync('curl', ['--disable', '-f', '--proto', '=https', '--retry', '3', '--retry-all-errors',
        '--connect-timeout', '20', '--max-time', '900', '--max-filesize', String(profile.archive.bytes),
        '-sS', '-o', download, profile.archive.url]);
    }
    const unpacked = unpackArchive(download, join(scratch.path, 'unpacked'), profile);
    renameSync(unpacked, ndk);
  }
  observeNdkInput(work, ndk, profile, profileBytes);
  console.log(ndk);
}

if (import.meta.main) {
  try {
    main();
  } catch (error) {
    console.error('libbox NDK preparation failed: ' + error.message);
    process.exitCode = 1;
  }
}
