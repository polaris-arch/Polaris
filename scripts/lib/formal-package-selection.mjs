// Prove App and Helper identities from actual carrier members without executing them.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  closeSync, constants, existsSync, fstatSync, lstatSync, mkdirSync, mkdtempSync, openSync,
  readFileSync, readSync, readdirSync, rmSync, writeFileSync, writeSync,
} from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, extname, isAbsolute, join, posix, resolve, toNamespacedPath } from 'node:path';
import { Readable, Writable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';
import { createInflateRaw, crc32 } from 'node:zlib';
import { LOCAL_MARKER } from '../desktop-core/local-development.mjs';
import { requireGraph as demand } from '../desktop-core/source-graph.mjs';
import { appImageFilesystems } from '../postprocess-appimage.mjs';
import { readZipEntries, zipName } from './zip-archive.mjs';
import { now, streamChild, withHolds } from './windows-package-custody.mjs';

const require = createRequire(new URL('../../ui/package.json', import.meta.url));
const MAX_FILE = 512 * 1024 ** 2;
const MAX_TOTAL = 2 * 1024 ** 3;
const MAX_MEMBERS = 10000;
const MAX_LISTING = 8 * 1024 ** 2;
const MAX_STDERR = 256 * 1024;
const DEADLINE_MS = 180_000;
const BLOCK = 64 * 1024;
const FILE_TYPE = 0o170000n;
const statOptions = { bigint: true };
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

export function rejectWindowsReparsePoints(path, label, timeout = 30_000) {
  if (process.platform !== 'win32') return;
  // lstat exposes symlinks and junctions through libuv but not all reparse tags.
  const powershell = join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'WindowsPowerShell/v1.0/powershell.exe');
  execFileSync(powershell, ['-NoProfile', '-NonInteractive', '-Command', `
    $ErrorActionPreference = 'Stop'
    [Console]::OutputEncoding = [Text.Encoding]::UTF8
    for ($path = $env:POLARIS_WINDOWS_PACKAGE_PATH; $path; $path = [IO.Path]::GetDirectoryName($path)) {
      if ([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) {
        [Console]::Error.WriteLine($env:POLARIS_WINDOWS_PACKAGE_LABEL + ' contains a reparse point: ' + $path)
        exit 1
      }
    }
  `], { env: { ...process.env, POLARIS_WINDOWS_PACKAGE_PATH: path, POLARIS_WINDOWS_PACKAGE_LABEL: label },
    stdio: ['ignore', 'pipe', 'pipe'], timeout, maxBuffer: 1024 * 1024 });
}

function identityMatches(left, right, sameView) {
  return left.dev === right.dev && left.ino === right.ino && left.nlink === right.nlink
    && left.size === right.size && left.mtimeNs === right.mtimeNs
    && (sameView ? left.mode === right.mode && left.ctimeNs === right.ctimeNs
      : (left.mode & FILE_TYPE) === (right.mode & FILE_TYPE));
}

function regularArtifactStat(path) {
  for (let parent = dirname(path);;) {
    demand(lstatSync(parent, statOptions).isDirectory(), `formal asset linked ancestor: ${parent}`);
    const next = dirname(parent);
    if (next === parent) break;
    parent = next;
  }
  const before = lstatSync(path, statOptions);
  demand(before.isFile() && before.size > 0n && before.size <= BigInt(MAX_FILE)
    && (process.platform !== 'win32' || before.nlink === 1n), 'formal asset byte budget or regular file differs');
  return before;
}

function readSource(fd, start, requested, remaining) {
  remaining();
  const data = Buffer.allocUnsafe(requested);
  let at = 0;
  while (at < requested) {
    const read = readSync(fd, data, at, requested - at, start + at);
    if (read === 0) break;
    at += read;
  }
  return data.subarray(0, at);
}

function* fileBlocks(read, start, length, probeEof = false) {
  for (let position = 0; position < length;) {
    const wanted = Math.min(BLOCK, length - position), bytes = read(start + position, wanted);
    demand(bytes.length === wanted, 'formal member or asset size differs');
    position += bytes.length;
    yield bytes;
  }
  if (probeEof) demand(read(start + length, 1).length === 0, 'formal asset snapshot byte budget exceeded');
}

function putFile(path, consume) {
  const fd = openSync(path, 'wx', 0o600);
  const write = (block) => {
    for (let at = 0; at < block.length;) {
      const written = writeSync(fd, block, at, block.length - at);
      demand(written > 0, 'formal asset scratch write failed');
      at += written;
    }
  };
  return Promise.resolve().then(() => consume(write)).finally(() => closeSync(fd));
}

function scanBytes(read, size) {
  const hash = createHash('sha256');
  for (const block of fileBlocks(read, 0, size, true)) hash.update(block);
  return hash.digest('hex');
}

function snapshot(read, path, size, remaining) {
  return putFile(path, (write) => {
    // Check the live source length before each write, including growth in
    // the last partial block, and keep all writes below the frozen size.
    for (let at = 0; ; ) {
      const block = read(at, BLOCK);
      if (block.length === 0) {
        demand(at === size, 'formal asset snapshot differs');
        break;
      }
      remaining();
      demand(block.length <= size - at, 'formal asset snapshot byte budget exceeded');
      write(block);
      at += block.length;
    }
    demand(lstatSync(path).size === size, 'formal asset snapshot differs');
  });
}

// Each read uses literal argv, bounded concurrent pipes and one shared
// absolute deadline. Resolve only when the owned primary child has closed.
async function readChild(tool, args, limit, remaining, consume) {
  const { stdoutBytes } = await streamChild([tool, ...args], block => {
    remaining();
    consume(block);
  }, limit, { deadline: now() + remaining(), diagnostics: false });
  remaining();
  return stdoutBytes;
}

async function textChild(tool, args, limit, remaining) {
  const blocks = [];
  await readChild(tool, args, limit, remaining, (block) => blocks.push(block));
  return utf8.decode(Buffer.concat(blocks));
}

// Match Unicode default case folding, independent of the reader host locale.
function caseFold(name) {
  return Array.from(name, (ch) => {
    if (ch === '\u0131') return ch;
    if (ch === '\u1e9e') return 'ss';
    if (/[\u13a0-\u13f5\u13f8-\u13fd\uab70-\uabbf]/.test(ch)) return ch.toUpperCase();
    return Array.from(ch.toUpperCase(), (letter) => letter.toLowerCase()).join('');
  }).join('');
}

export function memberKey(name, caseSensitive = false) {
  demand(name && !/[\\:]/.test(name) && !name.startsWith('/'), 'unsafe ZIP member');
  const parts = name.split('/');
  demand(parts.every(part => part && part !== '.' && part !== '..'), 'non-canonical ZIP path');
  for (const part of parts) {
    demand(!/[ .]$/.test(part) && !/[\x00-\x1f<>"|?*]/.test(part) && part.normalize('NFC') === part,
      'Windows ZIP path alias');
    demand(!/^(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])$/i.test(part.split('.')[0]), 'Windows ZIP device name');
  }
  return caseSensitive ? name : caseFold(name);
}

function insertRecord(records, name, row) {
  demand(!records.has(name) && records.size < MAX_MEMBERS, 'formal archive duplicate/member count budget');
  records.set(name, row);
}

function role(name, suffix) {
  const leaf = posix.basename(name);
  if (['polaris', 'Polaris', 'polaris.exe', 'Polaris.exe'].includes(leaf)
    || (suffix === '.apk' && leaf === 'libpolaris_lib.so')) return 'app';
  if (['polaris-helper', 'polaris-helper.exe'].includes(leaf)) return 'helper';
  return null;
}

function resolveMember(records, name) {
  let target = name;
  const visited = new Set();
  while (records.get(target).kind === 'link') {
    demand(!visited.has(target) && visited.size < 16, 'formal executable link cycle');
    visited.add(target);
    const link = records.get(target).target, parent = posix.dirname(target);
    demand(!link.startsWith('/') && !link.includes('\\'), 'formal executable link escapes archive');
    const parts = parent === '.' ? [] : parent.split('/');
    for (const part of link.split('/')) {
      if (!part || part === '.') continue;
      if (part === '..') {
        demand(parts.length > 0, 'formal executable link escapes archive');
        parts.pop();
      } else parts.push(part);
    }
    target = parts.join('/');
    demand(records.has(target), 'formal executable link target absent');
  }
  demand(records.get(target).kind === 'file', 'formal executable is not a regular member');
  return target;
}

function validateRecords(records, caseSensitive = false) {
  const normalized = new Map(), parents = new Set();
  let expanded = 0;
  for (const [name, row] of records) {
    const key = memberKey(name, caseSensitive);
    demand(!normalized.has(key), 'duplicate/alias member: ' + name);
    normalized.set(key, row.kind === 'dir');
    const parts = key.split('/');
    for (let i = 1; i < parts.length; i++) parents.add(parts.slice(0, i).join('/'));
    expanded += row.bytes ?? 0;
    demand(expanded <= MAX_TOTAL, 'formal archive expanded budget exceeded');
  }
  demand(Array.from(parents).every(parent => normalized.get(parent) !== false), 'formal archive file/directory alias collision');
}

function selectedMembers(records, suffix, caseSensitive = false) {
  validateRecords(records, caseSensitive);
  const selected = [];
  for (const [member, row] of records) {
    const memberRole = role(member, suffix);
    if (memberRole && row.kind !== 'dir') selected.push({ member, role: memberRole, target: resolveMember(records, member) });
  }
  return selected;
}

function observer(name, expected, charge, copies = 1) {
  const hash = createHash('sha256');
  let bytes = 0, tail = Buffer.alloc(0);
  return {
    consume(block) {
      bytes += block.length;
      demand(bytes <= MAX_FILE && (expected === null || bytes <= expected), 'formal executable member size or expansion budget exceeded');
      charge(block.length * copies);
      const data = Buffer.concat([tail, block]);
      demand(!data.includes(LOCAL_MARKER), 'Formal package/publisher rejects a LocalDevelopmentSelection executable: ' + name);
      tail = data.subarray(Math.max(0, data.length - LOCAL_MARKER.length + 1));
      hash.update(block);
    },
    finish() {
      demand(expected === null || bytes === expected, 'formal executable member size differs');
      return { bytes, sha256: hash.digest('hex') };
    },
  };
}

function zipRecords(read, length, strictMetadata = false) {
  const records = new Map();
  for (const [raw, entry] of readZipEntries(length, read, {
    zip64: true, maxEntries: MAX_MEMBERS, decodeName: zipName, strictMetadata,
  })) {
    const directory = raw.endsWith('/'), type = entry.mode & 0o170000;
    demand([0, 0o040000, 0o100000].includes(type), 'formal ZIP alias/link or nonregular member');
    const name = directory ? raw.slice(0, -1) : raw;
    insertRecord(records, name, { kind: directory ? 'dir' : 'file', bytes: entry.uncompressedSize, entry });
  }
  return records;
}

async function readZipMember(read, entry, consume, signal) {
  demand(!(entry.flags & 0x61) && [0, 8].includes(entry.method), 'formal ZIP encrypted/patched member or unsupported compression');
  const input = fileBlocks(read, entry.dataOffset, entry.compressedSize);
  let checksum = 0;
  const receive = block => { consume(block); checksum = crc32(block, checksum); };
  if (entry.method === 0) {
    for (const block of input) receive(block);
  } else {
    const inflater = createInflateRaw();
    await pipeline(Readable.from(input), inflater, new Writable({
      write(block, _encoding, done) {
        try { receive(block); done(); } catch (error) { done(error); }
      },
    }), { signal });
    demand(inflater.bytesWritten === entry.compressedSize, 'formal ZIP compressed size differs');
  }
  demand(checksum === entry.crc32, 'formal ZIP member CRC differs');
}

async function scanZip(read, length, suffix, charge, signal) {
  const records = zipRecords(read, length);
  const checked = [];
  // Android reads APK members case-sensitively, like the deb/AppImage consumers; AGP's
  // shortened resource paths (res/2F.xml beside res/2f.xml) are distinct there. Windows ZIP still folds.
  for (const { member, role, target } of selectedMembers(records, suffix, suffix === '.apk')) {
    const { entry, bytes } = records.get(target);
    const output = observer(member, bytes, charge);
    await readZipMember(read, entry, output.consume, signal);
    checked.push({ member, role, ...output.finish() });
  }
  return checked;
}

function validateTarNames(raw) {
  // node-tar accepts loose PAX lines and normalizes the effective Windows
  // view. Detect a name the parser could otherwise silently truncate or omit.
  const unsafe = value => /[\x00-\x1f\\\ufffd]/u.test(value);
  if (!/^\d+ [^=\n\0]+=/.test(raw) || raw.endsWith('\0')) {
    demand(!unsafe(raw.replace(/\0+$/, '')), 'unsafe TAR extended member name');
    return;
  }
  const bytes = Buffer.from(raw);
  for (let start = 0; start < bytes.length;) {
    const space = bytes.indexOf(32, start);
    demand(space > start, 'invalid TAR extended record');
    const width = bytes.toString('ascii', start, space), length = Number(width), end = start + length;
    demand(/^\d+$/.test(width) && length > space - start + 2 && end <= bytes.length && bytes[end - 1] === 10,
      'invalid TAR extended record');
    const field = bytes.toString('utf8', space + 1, end - 1), equals = field.indexOf('=');
    demand(equals > 0 && !field.includes('\n'), 'invalid TAR extended record');
    if (['path', 'linkpath'].includes(field.slice(0, equals))) {
      demand(!unsafe(field.slice(equals + 1)), 'unsafe TAR extended member name');
    }
    start = end;
  }
}

function readTar(archive, remaining, onEntry) {
  const { Parser } = require('tar');
  const fd = openSync(archive, 'r');
  try {
    const length = fstatSync(fd).size;
    const read = (at, count) => readSource(fd, at, count, remaining);
    const magic = read(0, 2);
    demand(length >= 512 && !(magic[0] === 0x1f && magic[1] === 0x8b), 'formal raw TAR data differs');
    let failure;
    const parser = new Parser({ strict: true, zstd: false, maxMetaEntrySize: MAX_LISTING });
    const abort = (error) => { failure ??= error; parser.abort(error); };
    parser.on('error', error => { failure ??= error; });
    parser.on('ignoredEntry', () => abort(new Error('formal tar nonregular member')));
    parser.on('meta', raw => {
      try { validateTarNames(raw); } catch (error) { abort(error); }
    });
    parser.on('entry', (entry) => {
      try {
        onEntry(entry, abort);
        entry.resume();
      } catch (error) { abort(error); }
    });
    for (const block of fileBlocks(read, 0, length, true)) {
      parser.write(block);
      if (failure) throw failure;
    }
    parser.end();
    if (failure) throw failure;
  } finally {
    closeSync(fd);
  }
}

function tarName(entry) {
  demand(!entry.header.path.includes('\\'), 'unsafe ZIP member');
  return (entry.extended?.path ?? entry.globalExtended?.path ?? entry.header.path).replace(/^\.\//, '').replace(/\/+$/, '');
}

async function scanDeb(retained, temporary, remaining, suffix, charge) {
  const archive = join(temporary, 'data.tar');
  await putFile(archive, write => readChild('dpkg-deb', ['--fsys-tarfile', retained], MAX_TOTAL, remaining, write));
  const records = new Map();
  readTar(archive, remaining, entry => {
    const name = tarName(entry);
    const kind = entry.type === 'Directory' ? 'dir' : ['Link', 'SymbolicLink'].includes(entry.type) ? 'link' : 'file';
    if (kind === 'dir' && (name === '' || name === '.')) return;
    demand(kind !== 'file' || ['File', 'OldFile', 'ContiguousFile'].includes(entry.type), 'formal tar nonregular member');
    // Local PAX/GNU values override global PAX and the original header.
    // In TAR only hard-link targets are archive-root relative.
    demand(!entry.header.linkpath?.includes('\\'), 'formal executable link escapes archive');
    let target = entry.extended?.linkpath ?? entry.globalExtended?.linkpath ?? entry.header.linkpath ?? '';
    if (entry.type === 'Link') {
      demand(!target.startsWith('/') && !target.includes('\\'), 'formal executable link escapes archive');
      // Use a virtual absolute archive root. The process cwd is irrelevant.
      // Keep .. in the root-relative target so traversal past root is detected.
      const parent = posix.dirname(name);
      target = (parent === '.' ? '' : '../'.repeat(parent.split('/').length)) + target;
    }
    insertRecord(records, name, { kind, bytes: entry.header.size, target });
  });
  const selected = selectedMembers(records, suffix, true);
  const targets = new Map();
  for (const row of selected) {
    if (!targets.has(row.target)) targets.set(row.target, []);
    targets.get(row.target).push(row.member);
  }
  const facts = new Map();
  readTar(archive, remaining, (entry, abort) => {
    const name = tarName(entry);
    const aliases = targets.get(name);
    if (!aliases) return;
    const output = observer(aliases[0], records.get(name).bytes, charge, aliases.length);
    entry.on('data', (block) => {
      try { output.consume(block); } catch (error) { abort(error); }
    });
    entry.once('end', () => {
      try { facts.set(name, output.finish()); } catch (error) { abort(error); }
    });
  });
  return selected.map(({ member, role, target }) => {
    demand(facts.has(target), 'formal tar executable bytes absent');
    return { member, role, ...facts.get(target) };
  });
}

async function scanSquashfs(retained, length, remaining, suffix, charge) {
  const fd = openSync(retained, 'r');
  let offsets;
  try {
    const read = (at, count) => readSource(fd, at, count, remaining);
    offsets = appImageFilesystems(length, read, at => {
      if (at + 96 > length) return false;
      const header = read(at, 96);
      demand(header.length === 96, 'formal AppImage superblock truncated');
      const blockSize = header.readUInt32LE(12), used = header.readBigUInt64LE(40);
      return header.readUInt32LE(4) > 0 && blockSize >= 4096 && blockSize <= 1024 ** 2
        && (blockSize & (blockSize - 1)) === 0
        && header.readUInt16LE(28) === 4 && header.readUInt16LE(30) === 0
        && used >= 96n && used <= BigInt(length - at);
    });
  } finally {
    closeSync(fd);
  }
  demand(offsets.length === 1, 'final AppImage lacks unique valid SquashFS superblock');
  const offset = String(offsets[0]);
  const listing = await textChild('unsquashfs', ['-lln', '-no-progress', '-offset', offset, retained], MAX_LISTING, remaining);
  const records = new Map();
  for (const line of listing.split('\n')) {
    const match = line.match(/^([dl-])[rwxstST-]{9}\s+\S+\s+(\d+)\s+\S+\s+\S+\s+(.*)$/);
    if (!/^[dl-][rwxstST-]{9}\s/.test(line)) continue;
    demand(match, 'formal SquashFS listing differs');
    const [, type, bytes, raw] = match;
    if (raw === 'squashfs-root') continue;
    demand(raw.startsWith('squashfs-root/'), 'formal SquashFS root differs');
    let name = raw.slice('squashfs-root/'.length), target = '';
    if (type === 'l') {
      const separator = name.indexOf(' -> ');
      demand(separator >= 0, 'formal SquashFS target absent');
      target = name.slice(separator + 4);
      name = name.slice(0, separator);
    }
    insertRecord(records, name, { kind: { d: 'dir', l: 'link', '-': 'file' }[type], bytes: Number(bytes), target });
  }
  const checked = [];
  for (const { member, role, target } of selectedMembers(records, suffix, true)) {
    const output = observer(member, records.get(target).bytes, charge);
    await readChild('unsquashfs', ['-cat', '-no-wildcards', '-offset', offset, retained, target], MAX_FILE, remaining, output.consume);
    checked.push({ member, role, ...output.finish() });
  }
  return checked;
}

async function sevenZip(remaining) {
  const tools = process.platform === 'win32'
    ? ['7zz', '7z', join(process.env.ProgramFiles ?? 'C:\\Program Files', '7-Zip', '7z.exe')]
    : ['7zz', '7z'];
  for (const tool of tools) {
    try {
      const capabilities = await textChild(tool, ['i', '-sccUTF-8'], 1024 ** 2, () => Math.min(30_000, remaining()));
      const version = /7-Zip(?: \([^\n]*\))? (\d+)\.\d+/.exec(capabilities);
      demand(version && Number(version[1]) >= 24 && /\bNsis\b/i.test(capabilities), '7-Zip >=24 with the NSIS reader required');
      return tool;
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
  throw new Error('7-Zip is required for static final carrier inspection');
}

function sevenZipProperties(body, newline, nsis) {
  const records = [];
  for (const block of body.split(newline + newline)) {
    const row = new Map();
    for (const line of block.split(newline)) {
      if (!line) continue;
      const split = line.indexOf(' = ');
      if (nsis && split < 0) continue;
      demand(split >= 0, 'invalid formal DMG property line');
      const key = line.slice(0, split);
      demand(!row.has(key), 'duplicate formal 7-Zip property');
      row.set(key, line.slice(split + 3));
    }
    if (row.size === 0) continue;
    demand(records.length < MAX_MEMBERS, 'formal archive member count budget');
    records.push(row);
  }
  return records;
}

function sevenZipBytes(size, optional, limit = MAX_TOTAL) {
  if (!size && optional) return null;
  demand(/^\d+$/.test(size ?? '') && Number(size) <= limit, 'formal 7-Zip member size differs');
  return Number(size);
}

export function nsisMembers(listing) {
  const boundary = listing.indexOf('----------');
  demand(boundary >= 0 && /^Type = Nsis\r?$/m.test(listing.slice(0, boundary)), 'actual artifact is not readable NSIS');
  const newline = listing.slice(0, boundary).includes('\r\n') ? '\r\n' : '\n';
  const records = new Map();
  for (const row of sevenZipProperties(listing.slice(boundary + 10), newline, true)) {
    const name = row.get('Path'), attributes = row.get('Attributes') ?? '';
    demand(name && !row.has('Symbolic Link') && !row.has('Hard Link')
      && (row.get('Folder') ?? '-') === '-' && !attributes.startsWith('D')
      && !/\b[lrwx-]{10}\b/.test(attributes), 'invalid NSIS member record');
    insertRecord(records, name.replaceAll('\\', '/'), { kind: 'file', bytes: sevenZipBytes(row.get('Size'), true, MAX_FILE) });
  }
  demand(records.size > 0, 'empty NSIS member listing');
  validateRecords(records);
  return records;
}

// The current Windows distribution has all payload/plugin bytes frozen. Only
// the generated uninstaller needs its own literal extraction before the batch.
export async function inspectBoundNsis(tool, archive, destination, allowed, expected, {
  custody, budget, facts, tree, timeout = DEADLINE_MS,
}) {
  const deadline = now() + timeout;
  const file = path => ({ path, kind: 'file' });
  const directory = path => ({ path, kind: 'directory' });
  return withHolds(custody, [file(archive)], async ([archiveFile]) => {
    const original = await facts(archive);
    const command = (args, consume, limit = MAX_LISTING, guard) =>
      streamChild([tool, ...args], consume, limit, { deadline, guard });
    const chunks = [];
    await command(['l', '-slt', '-sccUTF-8', archive], block => chunks.push(block));
    const listing = new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)).replaceAll('\r\n', '\n');
    const records = nsisMembers(listing);
    let expanded = 0;
    for (const [name, { bytes }] of records) {
      demand(Object.hasOwn(allowed, name), 'unregistered NSIS listed member');
      const limit = allowed[name], row = expected[name];
      demand(Number.isSafeInteger(limit) && limit >= 0 && limit <= MAX_FILE
        && (bytes === null || bytes <= limit), 'NSIS member exceeds registered size');
      demand(!row || Number.isSafeInteger(row.bytes) && row.bytes >= 0 && row.bytes <= limit
        && /^[a-f0-9]{64}$/.test(row.sha256) && Object.keys(row).length === 2
        && (bytes === null || bytes === row.bytes), 'listed NSIS size differs from bound member');
      expanded += bytes ?? limit;
    }
    demand(expanded <= MAX_TOTAL, 'NSIS expanded budget exceeded');
    mkdirSync(toNamespacedPath(destination));
    const exact = { ...expected }, captured = new Set();
    for (const [name, { bytes }] of records) {
      if (Object.hasOwn(expected, name)) continue;
      const limit = Math.min(allowed[name], MAX_TOTAL - budget.bytes);
      demand(limit >= 0, 'NSIS aggregate byte limit exceeded');
      const path = join(destination, name);
      mkdirSync(toNamespacedPath(dirname(path)), { recursive: true });
      const output = observer(name, null, count => { budget.bytes += count; });
      await putFile(toNamespacedPath(path), write => command(['x', '-so', '-bd', '-y', '-spd', '--', archive, name], block => {
        output.consume(block);
        write(block);
      }, limit));
      exact[name] = output.finish();
      demand(bytes === null || bytes === exact[name].bytes, 'extracted NSIS size differs');
      captured.add(name);
    }
    const selected = [...records.keys()].filter(name => !captured.has(name));
    demand(selected.reduce((sum, name) => sum + exact[name].bytes, 0) <= MAX_TOTAL - budget.bytes,
      'NSIS aggregate byte limit exceeded');
    const directories = new Set();
    for (const name of records.keys()) {
      for (let parent = posix.dirname(name); parent !== '.'; parent = posix.dirname(parent)) directories.add(parent);
    }
    demand(records.size + directories.size <= MAX_MEMBERS, 'NSIS extracted entry limit exceeded');
    if (selected.length) {
      const scratch = mkdtempSync(toNamespacedPath(join(dirname(destination), 'nsis-list-')));
      const selector = join(scratch, 'members.txt');
      const maxima = new Map();
      let charged = 0;
      const guard = async signal => {
        const pending = [destination], observed = [];
        while (pending.length) {
          const parent = pending.pop();
          for (const entry of readdirSync(toNamespacedPath(parent), { withFileTypes: true })) {
            const path = join(parent, entry.name);
            const name = path.slice(destination.length + 1).replaceAll('\\', '/');
            const stat = lstatSync(toNamespacedPath(path));
            demand(!stat.isSymbolicLink(), 'NSIS extracted reparse or link');
            if (stat.isDirectory()) {
              demand(directories.has(name), 'unregistered NSIS extracted directory');
              pending.push(path); observed.push(directory(path));
            } else {
              demand(stat.isFile() && records.has(name) && stat.size <= exact[name].bytes,
                'unregistered or oversized NSIS extracted member');
              if (!captured.has(name)) maxima.set(name, Math.max(maxima.get(name) ?? 0, stat.size));
              observed.push(file(path));
            }
            demand(observed.length <= MAX_MEMBERS, 'NSIS extracted entry limit exceeded');
          }
        }
        await custody.inspect(observed);
        signal.throwIfAborted();
        const bytes = [...maxima.values()].reduce((sum, size) => sum + size, 0);
        demand(bytes - charged <= MAX_TOTAL - budget.bytes, 'NSIS aggregate byte limit exceeded');
        budget.bytes += bytes - charged; charged = bytes;
      };
      try {
        writeFileSync(selector, selected.join('\n') + '\n', { flag: 'wx', flush: true });
        await withHolds(custody, [file(selector), directory(destination), ...[...captured].map(name => file(join(destination, name)))],
          () => command(['x', '-bd', '-y', '-spd', '-scsUTF-8', '-o' + toNamespacedPath(destination),
            '-i@' + selector, '--', archive], () => {}, MAX_LISTING, guard));
      } finally { rmSync(scratch, { recursive: true }); }
    }
    const members = await withHolds(custody, [...records.keys()].map(name => file(join(destination, name))), () => tree(destination));
    demand(Object.keys(members).length === records.size, 'final NSIS member set differs');
    for (const name of records.keys()) {
      demand(members[name]?.bytes === exact[name].bytes && members[name]?.sha256 === exact[name].sha256,
        'final member SHA/size differs: ' + name);
    }
    const [identity] = await custody.inspect([file(archive)]);
    const final = await facts(archive);
    demand(identity.every((value, index) => value === archiveFile.identity[index])
      && final.bytes === original.bytes && final.sha256 === original.sha256, 'held NSIS archive changed');
    demand(now() < deadline, 'NSIS shared extraction deadline exceeded');
    return { members, listing };
  });
}

async function scanSevenZip(tool, retained, temporary, remaining, suffix, charge, depth = 0) {
  const listing = await textChild(tool, ['l', '-slt', '-sccUTF-8', '--', retained], MAX_LISTING, remaining);
  const boundary = listing.indexOf('----------');
  demand(boundary >= 0, 'formal 7-Zip member boundary absent');
  const header = listing.slice(0, boundary), nsis = /^Type = Nsis\r?$/m.test(header);
  demand(nsis || /^Type = (Dmg|HFS|APFS)\r?$/m.test(header), 'formal archive type is not supported');
  const newline = header.includes('\r\n') ? '\r\n' : '\n';
  const list = nsis ? [] : sevenZipProperties(listing.slice(boundary + 10), newline, false);
  const records = nsis ? nsisMembers(listing) : new Map();
  // HFS/APFS -so otherwise appends extended attribute streams to the data fork.
  const args = ['x', '-so', '-spd', ...(nsis ? [] : ['-sns-']), '--', retained];
  for (const row of list) {
    let name = row.get('Path');
    demand(name, 'formal 7-Zip member path absent');
    const mode = row.get('Mode') ?? '', folder = row.get('Folder'), attributes = row.get('Attributes') ?? '';
    const hasLink = row.has('Symbolic Link') || row.has('Hard Link');
    const size = row.get('Size');
    const directory = folder === '+' || mode.startsWith('d') || attributes.startsWith('D');
    let target = row.get('Symbolic Link') || row.get('Hard Link') || '';
    const linked = Boolean(target) || mode.startsWith('l');
    demand(!(directory && linked), 'formal DMG directory/link conflict');
    demand(!mode || 'd-l'.includes(mode[0]), 'formal DMG nonregular member');
    const kind = directory ? 'dir' : linked ? 'link' : 'file';
    const bytes = sevenZipBytes(size, kind !== 'file');
    // A reserved, zero-content HFS bookkeeping directory is counted but
    // never normalized into a potentially selectable name.
    if (posix.basename(name) === '.HFS+ Private Directory Data\r') {
      demand(kind === 'dir' && (size === '' || size === undefined || size === '0'), 'invalid HFS metadata directory');
      continue;
    }
    if (linked && !target) {
      const blocks = [];
      const observed = await readChild(tool, [...args, name], 4096, remaining, (block) => {
        charge(block.length); blocks.push(block);
      });
      demand(bytes === null || bytes === observed, 'formal DMG link size differs');
      target = utf8.decode(Buffer.concat(blocks));
    }
    demand(!records.has(name), 'formal archive duplicate member');
    insertRecord(records, name, { kind, bytes: bytes ?? 0, target });
  }
  const selected = selectedMembers(records, suffix);
  if (selected.length === 0) {
    const volumes = [...records].filter(([, row]) => row.kind === 'file');
    demand(depth === 0 && volumes.length === 1 && /\.(?:hfs|apfs|img)$/i.test(volumes[0][0]),
      'formal archive contains no actual App/Helper');
    const [[name, { bytes }]] = volumes;
    const volume = join(temporary, 'volume.img');
    const observed = await putFile(volume, (write) => readChild(tool, [...args, name], MAX_TOTAL, remaining, block => {
      charge(block.length); write(block);
    }));
    demand(observed === bytes, 'formal volume bytes differ');
    return scanSevenZip(tool, volume, temporary, remaining, suffix, charge, depth + 1);
  }
  const checked = [];
  for (const { member, role, target } of selected) {
    const { bytes } = records.get(target);
    demand(nsis || bytes > 0, 'formal DMG empty or unresolved hard-linked executable unsupported');
    const output = observer(member, bytes, charge);
    await readChild(tool, [...args, target], MAX_FILE, remaining, output.consume);
    checked.push({ member, role, ...output.finish() });
  }
  return checked;
}

export async function inspectFormalArtifact(input, {
  clock = () => performance.now(),
  retainedPath = fd => `/proc/${process.pid}/fd/${fd}`,
  readFrom = readSource,
  openedStat = fd => fstatSync(fd, statOptions),
} = {}) {
  const deadline = clock() + DEADLINE_MS;
  const controller = new AbortController();
  const interrupt = () => controller.abort(new DOMException('formal archive reader interrupted', 'AbortError'));
  const remaining = () => {
    controller.signal.throwIfAborted();
    const timeout = deadline - clock();
    demand(timeout > 0, 'formal asset inspection deadline exceeded');
    return Math.ceil(timeout);
  };
  const artifact = resolve(input);
  const suffix = extname(artifact);
  rejectWindowsReparsePoints(artifact, 'Windows package artifact', Math.min(30_000, remaining()));
  const before = regularArtifactStat(artifact);
  const fd = openSync(artifact, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  let temporary;
  process.once('SIGINT', interrupt);
  process.once('SIGTERM', interrupt);
  try {
    const opened = openedStat(fd);
    const windows = process.platform === 'win32';
    demand(opened.isFile() && identityMatches(opened, before, !windows), 'formal asset replaced before open');
    const read = (at, count) => readFrom(fd, at, count, remaining);
    const length = Number(before.size);
    temporary = mkdtempSync(join(tmpdir(), 'polaris-formal-selection-'));
    let retained = retainedPath(fd, temporary);
    if (!retained || !existsSync(retained)) {
      retained = join(temporary, 'carrier' + suffix);
      await snapshot(read, retained, length, remaining);
    }
    let expandedExecutableBytes = 0;
    const charge = count => {
      expandedExecutableBytes += count;
      demand(expandedExecutableBytes <= MAX_TOTAL, 'formal executable expansion budget exceeded');
    };
    let checkedExecutables;
    if (suffix === '.zip' || suffix === '.apk') checkedExecutables = await scanZip(read, length, suffix, charge, controller.signal);
    else if (suffix === '.deb') checkedExecutables = await scanDeb(retained, temporary, remaining, suffix, charge);
    else if (suffix === '.AppImage') checkedExecutables = await scanSquashfs(retained, length, remaining, suffix, charge);
    else if (suffix === '.exe' || suffix === '.dmg') {
      checkedExecutables = await scanSevenZip(await sevenZip(remaining), retained, temporary, remaining, suffix, charge);
    } else throw new Error('unknown formal asset container');
    const roles = new Set(checkedExecutables.map(record => record.role));
    demand(roles.has('app') && (suffix === '.apk' || roles.has('helper')), 'formal asset actual App/Helper members absent');
    const artifactSha256 = scanBytes(read, length);
    demand(identityMatches(openedStat(fd), opened, true) && identityMatches(lstatSync(artifact, statOptions), before, true),
      'formal asset changed during actual object check');
    return { artifact, artifactBytes: length, artifactSha256, checkedExecutables, expandedExecutableBytes };
  } finally {
    process.removeListener('SIGINT', interrupt);
    process.removeListener('SIGTERM', interrupt);
    try { closeSync(fd); }
    finally { if (temporary) rmSync(temporary, { recursive: true, force: true }); }
  }
}

// Re-open the distribution path independently after carrier inspection.
export function verifyFormalArtifactBytes(path, checked) {
  demand(Number.isSafeInteger(checked.artifactBytes) && checked.artifactBytes > 0 && checked.artifactBytes <= MAX_FILE
    && /^[a-f0-9]{64}$/.test(checked.artifactSha256), 'Inspected artifact identity differs from distribution bytes');
  const before = lstatSync(path, statOptions);
  const fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  try {
    const opened = fstatSync(fd, statOptions);
    const deadline = performance.now() + DEADLINE_MS;
    const remaining = () => demand(performance.now() < deadline, 'formal asset checksum deadline exceeded');
    demand(before.isFile() && opened.isFile() && before.size === BigInt(checked.artifactBytes)
      && identityMatches(opened, before, process.platform !== 'win32')
      && checked.artifactSha256 === scanBytes((at, count) => readSource(fd, at, count, remaining), checked.artifactBytes)
      && identityMatches(fstatSync(fd, statOptions), opened, true)
      && identityMatches(lstatSync(path, statOptions), before, true), 'Inspected artifact object differs from distribution bytes');
  } finally { closeSync(fd); }
}

// The Windows producer guards native reparse tags and passes its frozen member map.
export async function inspectPortableZip(input, expected, {
  pinArtifact = false, deadline = performance.now() + DEADLINE_MS,
} = {}) {
  demand(expected && typeof expected === 'object' && !Array.isArray(expected), 'invalid portable ZIP inventory');
  const names = Object.keys(expected), directories = new Set();
  demand(names.length <= MAX_MEMBERS, 'portable ZIP expected member count budget exceeded');
  for (const name of names) {
    for (let parent = posix.dirname(name); parent !== '.' && parent !== '/'; parent = posix.dirname(parent)) {
      directories.add(parent);
    }
  }
  const artifact = resolve(input);
  const remaining = () => {
    const timeout = deadline - performance.now();
    demand(timeout > 0, 'portable ZIP inspection deadline exceeded');
    return Math.ceil(timeout);
  };
  const before = regularArtifactStat(artifact);
  demand(before.nlink === 1n, 'portable ZIP requires a single-link archive');
  const fd = openSync(artifact, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  try {
    const opened = fstatSync(fd, statOptions);
    demand(opened.isFile() && identityMatches(opened, before, process.platform !== 'win32'), 'portable ZIP replaced before open');
    const read = (at, count) => readSource(fd, at, count, remaining);
    const records = zipRecords(read, Number(opened.size), true);
    validateRecords(records);
    const actual = new Map();
    for (const [name, { kind, bytes, entry }] of records) {
      if (kind === 'dir') {
        demand(directories.has(name) && bytes === 0, 'unregistered ZIP directory: ' + name);
        continue;
      }
      demand(Object.hasOwn(expected, name) && bytes <= MAX_FILE, 'unregistered/oversized ZIP member: ' + name);
      const pin = expected[name];
      demand(pin && Object.keys(pin).length === 2 && bytes === pin.bytes, 'final member SHA/size differs: ' + name);
      const hash = createHash('sha256');
      let size = 0;
      await readZipMember(read, entry, block => {
        remaining();
        size += block.length;
        demand(size <= bytes, 'ZIP expanded size differs');
        hash.update(block);
      });
      const sha256 = hash.digest('hex');
      demand(size === bytes && sha256 === pin.sha256, 'final member SHA/size differs: ' + name);
      actual.set(name, { bytes: size, sha256 });
    }
    demand(actual.size === names.length, 'final member set differs');
    const pin = pinArtifact ? { bytes: Number(opened.size), sha256: scanBytes(read, Number(opened.size)) } : null;
    demand(identityMatches(fstatSync(fd, statOptions), opened, true)
      && identityMatches(regularArtifactStat(artifact), before, true), 'portable ZIP changed during inspection');
    const observed = Object.fromEntries(actual);
    return pin ? { observed, artifact: pin } : observed;
  } finally { closeSync(fd); }
}

// The producer retains the App/resource HANDLEs. PowerShell only provides the
// platform ZIP encoder for those exact input paths, without walking a tree.
export async function buildPortableZip(input, expected, sources, app, marker) {
  const paths = new Map();
  for (const [source, path] of Object.entries(sources)) {
    const name = source.startsWith('_up_/') ? source.slice(5) : source;
    demand(!paths.has(name) && name !== 'polaris.exe' && name !== 'portable.marker', 'portable ZIP source collision: ' + name);
    paths.set(name, path);
  }
  paths.set('polaris.exe', app);
  paths.set('portable.marker', null);
  const records = new Map();
  demand(paths.size <= MAX_MEMBERS && paths.size === Object.keys(expected).length, 'portable ZIP source/member set differs');
  for (const [name, path] of paths) {
    const pin = Object.hasOwn(expected, name) && expected[name];
    demand(pin && Number.isSafeInteger(pin.bytes) && pin.bytes >= 0 && pin.bytes <= MAX_FILE &&
      typeof pin.sha256 === 'string' && /^[a-f0-9]{64}$/.test(pin.sha256), 'invalid portable ZIP member pin: ' + name);
    demand(name === 'portable.marker' || (typeof path === 'string' && isAbsolute(path)), 'absolute held portable source required: ' + name);
    records.set(name, { kind: 'file', bytes: pin.bytes });
  }
  validateRecords(records);
  const markerBytes = Buffer.from(marker, 'utf8'), markerPin = expected['portable.marker'];
  demand(markerBytes.length === markerPin.bytes && createHash('sha256').update(markerBytes).digest('hex') === markerPin.sha256,
    'portable marker differs from frozen inventory');
  const archive = resolve(input), deadline = performance.now() + DEADLINE_MS;
  const members = Array.from(paths, ([name, path]) => ({ name, bytes: expected[name].bytes,
    ...(name === 'portable.marker' ? { inline: markerBytes.toString('base64') } : { path: toNamespacedPath(path) }) }));
  members.sort((left, right) => Buffer.compare(Buffer.from(left.name), Buffer.from(right.name)));
  execFileSync('pwsh', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', `
    $ErrorActionPreference = 'Stop'
    [Console]::InputEncoding = [Text.UTF8Encoding]::new($false, $true)
    $request = [Console]::In.ReadToEnd() | ConvertFrom-Json -AsHashtable
    $file = [IO.File]::Open($request.archive, 'CreateNew', 'Write', 'None')
    try {
      $zip = [IO.Compression.ZipArchive]::new($file, 'Create', $true)
      try {
        $block = [byte[]]::new(65536)
        foreach ($member in $request.members) {
          $entry = $zip.CreateEntry($member.name, 'Optimal')
          $output = $entry.Open()
          try {
            if ($member.Contains('inline')) {
              $bytes = [Convert]::FromBase64String($member.inline)
              $output.Write($bytes, 0, $bytes.Length)
              continue
            }
            $source = [IO.File]::Open($member.path, 'Open', 'Read', 'Read')
            try {
              $remaining = [long] $member.bytes
              if ($source.Length -ne $remaining) { throw 'portable ZIP held member size differs' }
              while ($remaining -gt 0) {
                $count = $source.Read($block, 0, [int] [Math]::Min($block.Length, $remaining))
                if ($count -eq 0) { throw 'portable ZIP held member truncated' }
                $output.Write($block, 0, $count)
                $remaining -= $count
              }
              if ($source.ReadByte() -ne -1) { throw 'portable ZIP held member grew' }
            } finally { $source.Dispose() }
          } finally { $output.Dispose() }
        }
      } finally { $zip.Dispose() }
      $file.Flush($true)
    } finally { $file.Dispose() }
  `], { input: Buffer.from(JSON.stringify({ archive: toNamespacedPath(archive), members })),
    stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true, timeout: DEADLINE_MS, killSignal: 'SIGKILL', maxBuffer: MAX_STDERR });
  const left = deadline - performance.now();
  demand(left > 0, 'portable ZIP build deadline exceeded');
  rejectWindowsReparsePoints(toNamespacedPath(archive), 'Windows portable ZIP artifact', Math.min(30_000, Math.ceil(left)));
  return inspectPortableZip(archive, expected, { pinArtifact: true, deadline });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const mode = process.argv[2];
    demand(process.argv.length === 3 && ['portable-zip', 'build-portable-zip'].includes(mode), 'expected portable ZIP request');
    const { archive, expected, sources, app, marker } = JSON.parse(utf8.decode(readFileSync(0)));
    const observed = mode === 'portable-zip' ? await inspectPortableZip(archive, expected) :
      await buildPortableZip(archive, expected, sources, app, marker);
    console.log(JSON.stringify(observed));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
