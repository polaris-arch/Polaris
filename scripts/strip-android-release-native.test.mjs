import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runtimeFingerprint, stripLibrary, verifyStrippedRuntime } from './strip-android-release-native.mjs';

function elfFixture({ cls = 64, statics = true, changed = null } = {}) {
  const is64 = cls === 64;
  const headerSize = is64 ? 64 : 52;
  const programSize = is64 ? 56 : 32;
  const sectionSize = is64 ? 64 : 40;
  const entries = [
    ['', 0, 0n, Buffer.alloc(0)],
    ['.text', 1, 6n, Buffer.from('native-code')],
    ['.dynamic', 6, 3n, Buffer.alloc(16, 1)],
    ['.dynsym', 11, 2n, Buffer.alloc(24, 2)],
    ['.dynstr', 3, 2n, Buffer.from('JNI_OnLoad\0Java_native_entry\0')],
    ['.go.buildinfo', 1, 3n, Buffer.from('Go runtime metadata')],
    ['.bss', 8, 3n, Buffer.alloc(64)],
    ...(statics ? [['.symtab', 2, 0n, Buffer.alloc(24)], ['.strtab', 3, 0n, Buffer.from('C++ static symbols\0')]] : []),
  ];
  const strings = Buffer.from([...entries.map(([name]) => name), '.shstrtab'].join('\0') + '\0');
  entries.push(['.shstrtab', 3, 0n, strings]);
  let position = headerSize + programSize;
  const offsets = entries.map(([, type, , bytes]) => {
    const offset = position;
    if (type !== 8) position += bytes.length;
    return offset;
  });
  const sectionStart = position;
  const buf = Buffer.alloc(sectionStart + entries.length * sectionSize);
  const word = (p, value) => is64 ? buf.writeBigUInt64LE(BigInt(value), p) : buf.writeUInt32LE(Number(value), p);
  buf.set([0x7f, 0x45, 0x4c, 0x46, is64 ? 2 : 1, 1, 1]);
  buf.writeUInt16LE(3, 16);
  buf.writeUInt16LE(is64 ? 0xb7 : 0x28, 18);
  word(is64 ? 32 : 28, headerSize);
  word(is64 ? 40 : 32, sectionStart);
  buf.writeUInt16LE(programSize, is64 ? 54 : 42);
  buf.writeUInt16LE(1, is64 ? 56 : 44);
  buf.writeUInt16LE(sectionSize, is64 ? 58 : 46);
  buf.writeUInt16LE(entries.length, is64 ? 60 : 48);
  buf.writeUInt16LE(entries.length - 1, is64 ? 62 : 50);
  buf.writeUInt32LE(1, headerSize);
  const runtimeSize = offsets[6];
  word(headerSize + (is64 ? 32 : 16), runtimeSize);
  word(headerSize + (is64 ? 40 : 20), runtimeSize + 64);
  word(headerSize + (is64 ? 48 : 28), 16384);
  let nameOffset = 0;
  entries.forEach(([name, type, flags, bytes], index) => {
    const p = sectionStart + index * sectionSize;
    buf.writeUInt32LE(nameOffset, p);
    nameOffset += Buffer.byteLength(name) + 1;
    buf.writeUInt32LE(type, p + 4);
    word(p + 8, flags);
    word(p + (is64 ? 16 : 12), flags & 2n ? offsets[index] : 0);
    word(p + (is64 ? 24 : 16), offsets[index]);
    word(p + (is64 ? 32 : 20), bytes.length);
    word(p + (is64 ? 48 : 32), 1);
    if (type !== 8) bytes.copy(buf, offsets[index]);
    if (name === changed) buf[offsets[index]] ^= 1;
  });
  return buf;
}

test('strip can remove non-allocated static tables while preserving ELF32/64 runtime and Go metadata', () => {
  for (const cls of [32, 64]) {
    const before = elfFixture({ cls });
    const after = elfFixture({ cls, statics: false });
    assert.deepEqual(runtimeFingerprint(before), runtimeFingerprint(after));
    assert.equal(verifyStrippedRuntime(before, after).ok, true);
    assert.throws(() => verifyStrippedRuntime(before, before), /unstripped sections/);
  }
});

test('stripping rejects changed code, dynamic exports, JNI names and Go runtime metadata', () => {
  const before = elfFixture();
  for (const changed of ['.text', '.dynamic', '.dynsym', '.dynstr', '.go.buildinfo']) {
    assert.throws(() => verifyStrippedRuntime(before, elfFixture({ statics: false, changed })),
      /strip changed runtime sections/, changed);
  }
});

test('compressed packaging still requires 16 KB ELF LOAD segments and intact program headers', () => {
  const before = elfFixture();
  const after = elfFixture({ statics: false });
  after.writeBigUInt64LE(4096n, 64 + 48);
  assert.throws(() => verifyStrippedRuntime(before, after), /not 16 KB aligned/);
  after.writeBigUInt64LE(16384n, 64 + 48);
  after[64 + 4] = 1;
  assert.throws(() => verifyStrippedRuntime(before, after), /strip changed runtime sections/);
});

test('already stripped libraries keep exact bytes and libbox rejects an unstripped source', () => {
  const directory = mkdtempSync(join(tmpdir(), 'polaris-native-source-proof-'));
  try {
    const clean = elfFixture({ statics: false });
    for (const name of ['libbox.so', 'libpolaris_lib.so']) {
      const file = join(directory, name);
      writeFileSync(file, clean);
      assert.equal(stripLibrary(file, '/nonexistent/llvm-strip').removedBytes, 0);
      assert.deepEqual(readFileSync(file), clean);
    }
    const file = join(directory, 'libbox.so');
    const unstripped = elfFixture();
    writeFileSync(file, unstripped);
    assert.throws(() => stripLibrary(file, '/nonexistent/llvm-strip'), /libbox must arrive stripped/);
    assert.deepEqual(readFileSync(file), unstripped);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
