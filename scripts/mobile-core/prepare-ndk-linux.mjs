#!/usr/bin/env node
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, constants, fstatSync, lstatSync, mkdirSync, mkdtempDisposableSync, openSync,
  readFileSync, readSync, realpathSync, renameSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const formalProfile = fileURLToPath(new URL('./ndk-r30-linux.json', import.meta.url));
const formalProfileSha256 = 'db468c29e87569f09ca04dc3b3d2137554124cf755ebdf68371cfdff466f6a30';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

function realPath(input, directory = false) {
  const path = resolve(input);
  assert.equal(realpathSync(path), path, 'formal NDK input or slot cannot be redirected');
  const stat = lstatSync(path);
  assert.ok(directory ? stat.isDirectory() : stat.isFile(), 'formal NDK input type differs: ' + path);
  return path;
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
    options: { profile: { type: 'string' }, sdk: { type: 'string' }, 'check-license-only': { type: 'boolean' } },
    allowPositionals: true,
  });
  assert.ok(process.platform === 'linux', 'formal NDK preparation requires Linux');
  assert.ok(values.profile && values.sdk, 'explicit formal NDK profile and SDK required');
  const profilePath = realPath(values.profile);
  assert.equal(profilePath, formalProfile, 'exact reviewed formal NDK profile required');
  const profileBytes = readFileSync(profilePath);
  assert.equal(sha256(profileBytes), formalProfileSha256, 'formal NDK profile bytes differ');
  // This byte pin includes the exact revision, package root and source.properties digest.
  const profile = JSON.parse(utf8.decode(profileBytes));
  const [root, localArchive] = positionals;
  const licenseOnly = values['check-license-only'];
  assert.ok(licenseOnly ? positionals.length === 0 : root && positionals.length <= 2,
    'license check cannot prepare a slot; extraction requires an explicit private runner slot');
  const sdk = realPath(values.sdk, true);
  const accepted = realPath(join(sdk, 'licenses', profile.licenseId));
  assert.ok(utf8.decode(readFileSync(accepted)).split(/\r\n?|\n/).includes(profile.licenseSha1),
    'standard Android SDK license is not already accepted');
  if (licenseOnly) return;

  const work = realPath(root, true), info = lstatSync(work);
  assert.ok(info.uid === process.getuid() && (info.mode & 0o7777) === 0o700,
    'formal NDK root must be current-user private');
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
  console.log(ndk);
}

try {
  main();
} catch (error) {
  console.error('libbox NDK preparation failed: ' + error.message);
  process.exitCode = 1;
}
