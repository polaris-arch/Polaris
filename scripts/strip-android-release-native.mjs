#!/usr/bin/env node
/** Operates on Gradle's independent release native output, never the source AAR or Rust SO. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readdirSync, readFileSync, renameSync, rmSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { elfIdent, elfPageAlignment, elfSectionNames, strippedReport } from './verify-apk.mjs';

const sha256 = (data) => createHash('sha256').update(data).digest('hex');
const GO_METADATA = new Set(['.go.buildinfo', '.go.fipsinfo', '.note.go.buildid', '.gopclntab', '.gosymtab']);

/** Every allocated section and program header must remain byte-for-byte identical. */
export function runtimeFingerprint(buf) {
  const ident = elfIdent(buf);
  assert.ok(ident.ok, ident.why);
  const parsed = elfSectionNames(buf);
  assert.ok(parsed.ok, parsed.why);
  const alignment = elfPageAlignment(buf);
  assert.ok(alignment.ok, alignment.why);
  const le = buf[5] === 1;
  const u16 = (p) => le ? buf.readUInt16LE(p) : buf.readUInt16BE(p);
  const u32 = (p) => le ? buf.readUInt32LE(p) : buf.readUInt32BE(p);
  const u64 = (p) => Number(le ? buf.readBigUInt64LE(p) : buf.readBigUInt64BE(p));
  assert.equal(u16(16), 3, 'release native input must be a linked shared library (ET_DYN)');
  const is64 = ident.elfClass === 2;
  const phoff = is64 ? u64(32) : u32(28);
  const phsize = u16(is64 ? 54 : 42);
  const phcount = u16(is64 ? 56 : 44);
  assert.ok(Number.isSafeInteger(phoff) && phoff >= (is64 ? 64 : 52) &&
    phsize >= (is64 ? 56 : 32) && phcount > 0 && phoff + phsize * phcount <= buf.length,
  'program headers missing or truncated');
  const sections = parsed.details.filter((s) => (s.flags & 2n) !== 0n || GO_METADATA.has(s.name)).map((s) => {
    assert.ok(Number.isSafeInteger(s.offset) && Number.isSafeInteger(s.size) && s.offset >= 0 && s.size >= 0,
      `${s.name}: section range is invalid`);
    assert.ok(s.type === 8 || s.offset + s.size <= buf.length, `${s.name}: allocated section is truncated`);
    return {
      name: s.name, type: s.type, flags: s.flags.toString(), address: s.address.toString(),
      offset: s.offset, size: s.size, alignment: s.alignment,
      sha256: s.type === 8 ? null : sha256(buf.subarray(s.offset, s.offset + s.size)),
    };
  });
  for (const name of ['.text', '.dynamic', '.dynsym', '.dynstr']) {
    assert.ok(sections.some((s) => s.name === name && BigInt(s.flags) & 2n), `${name}: required runtime section missing`);
  }
  return {
    ident, flags: u32(is64 ? 48 : 36),
    programHeadersSha256: sha256(buf.subarray(phoff, phoff + phsize * phcount)),
    sections,
  };
}

export function verifyStrippedRuntime(before, after) {
  assert.deepEqual(runtimeFingerprint(after), runtimeFingerprint(before),
    'strip changed runtime sections, dynamic exports, Go metadata or program headers');
  const report = strippedReport(after);
  assert.ok(report.ok, report.why ?? `unstripped sections: ${report.debug.join(' ')} ${report.staticSymbols.join(' ')}`);
  return report;
}

export function stripLibrary(file, stripTool) {
  const before = readFileSync(file);
  runtimeFingerprint(before); // Reject bad inputs before invoking the tool.
  const beforeReport = strippedReport(before);
  // The libbox source receipt requires its embedded bytes to match the pinned AAR.
  // It is already stripped upstream; fail closed if that contract ever changes.
  if (basename(file) === 'libbox.so') {
    assert.ok(beforeReport.ok, 'libbox must arrive stripped from its pinned source AAR');
  }
  if (beforeReport.ok) {
    return { library: file, beforeBytes: before.length, afterBytes: before.length,
      removedBytes: 0, sha256: sha256(before) };
  }
  const temporary = mkdtempSync(join(dirname(file), '.native-strip-'));
  try {
    const output = join(temporary, 'stripped.so');
    execFileSync(stripTool, ['--strip-all', '-o', output, file], { stdio: 'pipe' });
    const after = readFileSync(output);
    verifyStrippedRuntime(before, after);
    renameSync(output, file);
    return { library: file, beforeBytes: before.length, afterBytes: after.length,
      removedBytes: before.length - after.length, sha256: sha256(after) };
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

function main() {
  const [stripTool, directory, ...extra] = process.argv.slice(2);
  if (!stripTool || !directory || extra.length) throw new Error('usage: strip-android-release-native.mjs <NDK llvm-strip> <copied release native directory>');
  const libraries = [];
  const visit = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile() && entry.name.endsWith('.so')) libraries.push(path);
      else if (entry.isSymbolicLink()) throw new Error(`release native output must contain copied files, not symlinks: ${path}`);
    }
  };
  visit(directory);
  assert.ok(libraries.length > 0, 'release native output contains no shared libraries');
  for (const library of libraries) console.log(JSON.stringify(stripLibrary(library, stripTool)));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
