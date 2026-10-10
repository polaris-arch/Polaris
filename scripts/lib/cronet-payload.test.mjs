import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { chmodSync, mkdtempSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { CRONET_REQUIRED_EXPORTS, inspectCronetBytes, verifyCronetPayload } from './cronet-payload.mjs';

// Inert, structured file fixtures shared by the CLI/SquashFS tests. Never executed.
export function cronetFixture(platform, options = {}) {
  const names = CRONET_REQUIRED_EXPORTS.filter((name) => name !== options.missingExport).sort();
  if (options.unsortedNames) [names[0], names[1]] = [names[1], names[0]];
  const bytes = Buffer.alloc(platform === 'linux' ? 0xc00 : 0x1400);
  if (platform === 'linux') {
    bytes.set([0x7f, 69, 76, 70, 2, 1, 1]);
    bytes.writeUInt16LE(3, 16); bytes.writeUInt16LE(options.machine ?? 62, 18); bytes.writeUInt32LE(1, 20);
    bytes.writeBigUInt64LE(64n, 32); bytes.writeUInt16LE(56, 54); bytes.writeUInt16LE(3, 56);
    bytes.writeBigUInt64LE(0xa00n, 40); bytes.writeUInt16LE(64, 52); bytes.writeUInt16LE(64, 58); bytes.writeUInt16LE(6, 60);
    let str = 0x301;
    const strings = [options.dependency ?? 'libc.so.6', ...names];
    const indices = strings.map((value) => { const index = str - 0x300; bytes.write(value + '\0', str); str += value.length + 1; return index; });
    const section = (index, type, offset, size, { link = 0, entry = 0, flags = 0, address = 0 } = {}) => {
      const at = 0xa00 + index * 64;
      bytes.writeUInt32LE(type, at + 4); bytes.writeBigUInt64LE(BigInt(flags), at + 8); bytes.writeBigUInt64LE(BigInt(address), at + 16);
      bytes.writeBigUInt64LE(BigInt(offset), at + 24); bytes.writeBigUInt64LE(BigInt(size), at + 32); bytes.writeUInt32LE(link, at + 40); bytes.writeBigUInt64LE(BigInt(entry), at + 56);
    };
    section(1, 1, 0x200, 32, { flags: 6, address: 0x1000 });
    section(2, 3, 0x300, str - 0x300, { address: 0x2000 });
    section(3, 6, 0x900, 96, { link: 2, entry: 16, address: 0x2600 });
    bytes.writeBigUInt64LE(1n, 0x900); bytes.writeBigUInt64LE(BigInt(indices[0]), 0x908);
    section(4, 11, 0x600, (names.length + 1) * 24, { link: 2, entry: 24, address: 0x2300 });
    for (const [i, tag, value] of [[1, 5, 0x2000], [2, 10, str - 0x300], [3, 6, 0x2300], [4, 11, 24]]) {
      bytes.writeBigUInt64LE(BigInt(tag), 0x900 + i * 16); bytes.writeBigUInt64LE(BigInt(value), 0x908 + i * 16);
    }
    const program = (i, type, flags, offset, address, size) => {
      const at = 64 + i * 56; bytes.writeUInt32LE(type, at); bytes.writeUInt32LE(flags, at + 4);
      bytes.writeBigUInt64LE(BigInt(offset), at + 8); bytes.writeBigUInt64LE(BigInt(address), at + 16);
      bytes.writeBigUInt64LE(BigInt(size), at + 32); bytes.writeBigUInt64LE(BigInt(size), at + 40);
    };
    program(0, 1, 5, 0x200, 0x1000, 32); program(1, 1, 6, 0x300, 0x2000, 0x660); program(2, 2, 6, 0x900, 0x2600, 96);
    names.forEach((_, i) => {
      const at = 0x600 + (i + 1) * 24;
      bytes.writeUInt32LE(indices[i + 1], at); bytes[at + 4] = options.exportMode === 'data' ? 0x11 : 0x12;
      bytes[at + 5] = options.exportMode === 'hidden' ? 2 : 0;
      bytes.writeUInt16LE(options.exportMode === 'undefined' ? 0 : 1, at + 6); bytes.writeBigUInt64LE(BigInt(0x1000 + i), at + 8);
    });
    // Real GNU build-id note; appending/tampering elsewhere does not change it.
    section(5, 7, 0x980, 36); bytes.writeBigUInt64LE(4n, 0xa00 + 5 * 64 + 48);
    bytes.writeUInt32LE(4, 0x980); bytes.writeUInt32LE(20, 0x984); bytes.writeUInt32LE(3, 0x988);
    bytes.write('GNU\0', 0x98c); bytes.fill(0x42, 0x990, 0x9a4);
    if (options.rpath) { bytes.writeBigUInt64LE(29n, 0x900); }
  } else {
    bytes.write('MZ'); bytes.writeUInt32LE(0x80, 60); bytes.write('PE\0\0', 0x80);
    bytes.writeUInt16LE(options.machine ?? 0x8664, 0x84); bytes.writeUInt16LE(1, 0x86); bytes.writeUInt16LE(240, 0x94); bytes.writeUInt16LE(0x2000, 0x96);
    const optional = 0x98;
    bytes.writeUInt16LE(0x20b, optional); bytes.writeUInt32LE(16, optional + 108);
    bytes.writeUInt32LE(0x1100, optional + 112); bytes.writeUInt32LE(0x200, optional + 116);
    bytes.writeUInt32LE(0x1800, optional + 120); bytes.writeUInt32LE(40, optional + 124);
    const section = optional + 240;
    bytes.writeUInt32LE(0x1000, section + 8); bytes.writeUInt32LE(0x1000, section + 12); bytes.writeUInt32LE(0x1000, section + 16); bytes.writeUInt32LE(0x200, section + 20);
    bytes.writeUInt32LE(options.exportMode === 'data' ? 0x40000040 : 0x60000020, section + 36);
    bytes.writeUInt32LE(names.length, 0x300 + 20); bytes.writeUInt32LE(names.length, 0x300 + 24);
    bytes.writeUInt32LE(0x1140, 0x300 + 28); bytes.writeUInt32LE(0x1180, 0x300 + 32); bytes.writeUInt32LE(0x11c0, 0x300 + 36);
    let str = 0x600;
    names.forEach((name, i) => {
      bytes.writeUInt32LE(options.exportMode === 'forwarded' ? 0x1150 : 0x1e00 + i, 0x340 + i * 4);
      bytes.writeUInt32LE(str + 0xe00, 0x380 + i * 4); bytes.writeUInt16LE(i, 0x3c0 + i * 2);
      bytes.write(name + '\0', str); str += name.length + 1;
    });
    bytes.writeUInt32LE(0x1900, 0xa00 + 12); bytes.write(options.dependency ?? 'KERNEL32.dll', 0xb00);
  }
  return bytes;
}

export const fixtureManifest = (bytes) => {
  const sha = createHash('sha256').update(bytes).digest('hex');
  return { cronetLibrarySha256: { linux: sha, win: sha } };
};

// Other contract tests import only the inert fixture constructors above.
if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  for (const platform of ['linux', 'win']) {
    test(`${platform}: actual structural tables expose architecture, dependencies and direct C APIs`, () => {
      const result = inspectCronetBytes(cronetFixture(platform), platform);
      assert.equal(result.architecture, 'amd64'); assert.deepEqual(result.requiredExports, [...CRONET_REQUIRED_EXPORTS]);
    });
    for (const [label, options] of [
      ['wrong architecture', { machine: platform === 'linux' ? 183 : 0xaa64 }],
      ['unknown dependency', { dependency: 'injected-library' }],
      ['missing C API', { missingExport: CRONET_REQUIRED_EXPORTS[0] }],
      ['data/string bait instead of defined function', { exportMode: 'data' }],
      ...(platform === 'linux' ? [['undefined function', { exportMode: 'undefined' }], ['hidden function', { exportMode: 'hidden' }], ['rpath mutation', { rpath: true }]] : [['forwarded export', { exportMode: 'forwarded' }]]),
    ]) test(`${platform}: rejects ${label}`, () => assert.throws(() => inspectCronetBytes(cronetFixture(platform, options), platform)));
    for (const name of ['Cronet_Engine_Create', platform === 'linux' ? 'libc.so.6' : 'KERNEL32.dll']) {
      test(`${platform}: rejects high-bit byte alias in ${name}`, () => {
        const bytes = cronetFixture(platform), at = bytes.indexOf(Buffer.from(name + '\0'));
        assert.ok(at >= 0); bytes[at] |= 0x80;
        assert.throws(() => inspectCronetBytes(bytes, platform));
      });
    }
    if (platform === 'win') {
      for (const at of [0, 1, 0x80, 0x81]) test(`PE rejects high-bit magic alias at ${at}`, () => {
        const bytes = cronetFixture('win'); bytes[at] |= 0x80;
        assert.throws(() => inspectCronetBytes(bytes, 'win'));
      });
      test('PE rejects unsorted export name table used by loader binary search', () => {
        assert.throws(() => inspectCronetBytes(cronetFixture('win', { unsortedNames: true }), 'win'));
      });
    } else {
      for (const fault of ['missing program headers', 'missing LOAD', 'non-executable LOAD', 'conflicting PT_DYNAMIC', 'conflicting DT_STRTAB', 'conflicting DT_SYMTAB', 'conflicting DT_STRSZ', 'section-only executable symbol']) {
        test(`ELF rejects loader/section inconsistency: ${fault}`, () => {
          const bytes = cronetFixture('linux');
          if (fault === 'missing program headers') bytes.writeUInt16LE(0, 56);
          if (fault === 'missing LOAD') { bytes.writeUInt32LE(0, 64); bytes.writeUInt32LE(0, 120); }
          if (fault === 'non-executable LOAD') bytes.writeUInt32LE(4, 68);
          if (fault === 'conflicting PT_DYNAMIC') bytes.writeBigUInt64LE(0x910n, 176 + 8);
          if (fault === 'conflicting DT_STRTAB') bytes.writeBigUInt64LE(0x2001n, 0x918);
          if (fault === 'conflicting DT_SYMTAB') bytes.writeBigUInt64LE(0x2301n, 0x938);
          if (fault === 'conflicting DT_STRSZ') bytes.writeBigUInt64LE(1n, 0x928);
          if (fault === 'section-only executable symbol') { bytes.writeBigUInt64LE(0x3000n, 0xa00 + 64 + 16); for (let i = 1; i <= CRONET_REQUIRED_EXPORTS.length; i++) bytes.writeBigUInt64LE(BigInt(0x3000 + i), 0x600 + i * 24 + 8); }
          assert.throws(() => inspectCronetBytes(bytes, 'linux'));
        });
      }
    }
    test(`${platform}: truncated/malformed ranges fail closed`, () => {
      const bytes = cronetFixture(platform);
      for (const length of [0, 63, 100, bytes.length / 2]) assert.throws(() => inspectCronetBytes(bytes.subarray(0, length), platform));
      if (platform === 'linux') bytes.writeBigUInt64LE(0xffffffffffffffffn, 40); else bytes.writeUInt32LE(0xffffffff, 60);
      assert.throws(() => inspectCronetBytes(bytes, platform));
    });
    test(`${platform}: source and final hash, permissions, symlinks and pin prerequisites are independent gates`, (t) => {
      const root = mkdtempSync(join(tmpdir(), 'polaris-cronet-bytes-')); t.after(() => rmSync(root, { recursive: true, force: true }));
      const source = join(root, 'source'), payload = join(root, 'payload'), bytes = cronetFixture(platform), manifest = fixtureManifest(bytes);
      writeFileSync(source, bytes, { mode: 0o755 }); writeFileSync(payload, bytes, { mode: 0o755 });
      assert.equal(verifyCronetPayload(source, payload, platform, manifest).bytes, bytes.length);
      const wrong = Buffer.from(bytes); wrong[0x210] ^= 1;
      writeFileSync(payload, wrong); assert.throws(() => verifyCronetPayload(source, payload, platform, manifest), /final SHA/);
      writeFileSync(payload, bytes); writeFileSync(source, wrong); assert.throws(() => verifyCronetPayload(source, payload, platform, manifest), /source SHA/);
      writeFileSync(source, bytes); chmodSync(payload, 0o700); assert.throws(() => verifyCronetPayload(source, payload, platform, manifest), /permissions/);
      chmodSync(payload, 0o755); assert.throws(() => verifyCronetPayload(source, payload, platform, {}), /cronetLibrarySha256/);
      renameSync(payload, payload + '.real'); symlinkSync(payload + '.real', payload); assert.throws(() => verifyCronetPayload(source, payload, platform, manifest), /regular file/);
      const dir = join(root, 'linked'); symlinkSync(root, dir); assert.throws(() => verifyCronetPayload(join(dir, 'source'), payload + '.real', platform, manifest), /ancestor/);
    });
  }
}
