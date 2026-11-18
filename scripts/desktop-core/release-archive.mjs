// Read reviewed native release members before creating a private extraction tree.
import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { crc32, gunzipSync, inflateRawSync } from 'node:zlib';
import { parseZipEntries } from '../lib/zip-archive.mjs';
import { regularPath } from './release-consumer.mjs';
import { safeNativePath } from './upstream-release.mjs';
import { digest as sha256, requireGraph as demand } from './source-graph.mjs';

const require = createRequire(new URL('../../ui/package.json', import.meta.url));

export async function extractNativeArchive(archive, destination, record, root, target) {
  demand(Number.isSafeInteger(record.bytes) && record.bytes > 0 && record.bytes <= 512 * 1024 ** 2
    && statSync(archive).size === record.bytes, 'archive bytes differ');
  const data = readFileSync(archive);
  demand(data.length === record.bytes && sha256(data) === record.sha256, 'archive bytes differ');
  const expected = record.members, expectedNames = Object.keys(expected);
  demand(expectedNames.length > 0 && expectedNames.length <= 1024, 'member inventory limit');
  const hasModes = Object.hasOwn(record, 'memberModes');
  if (hasModes) demand(record.memberModes && typeof record.memberModes === 'object' && !Array.isArray(record.memberModes)
    && Object.keys(record.memberModes).length === expectedNames.length
    && expectedNames.every((name) => Object.hasOwn(record.memberModes, name)), 'member permissions differ');
  const seen = new Set(), aliases = new Set(), members = new Map();
  let expandedBytes = 0;
  const admit = (rawName, directory, mode, size) => {
    const name = directory && rawName.endsWith('/') ? rawName.slice(0, -1) : rawName;
    demand(safeNativePath(name) && name.split('/')[0] === root, 'unsafe archive path');
    demand(!seen.has(name) && !aliases.has(name.toLowerCase()), 'duplicate archive path');
    seen.add(name); aliases.add(name.toLowerCase());
    demand(!(mode & 0o7000) && !(mode & 0o022), 'unsafe member permissions');
    if (directory) {
      demand(expectedNames.some((member) => member.startsWith(name + '/')), 'unexpected directory');
    } else {
      demand(Object.hasOwn(expected, name) && Number.isSafeInteger(size) && size >= 0 && size <= 200 * 1024 ** 2,
        'unexpected/oversized member');
      expandedBytes += size;
      demand(expandedBytes <= 1024 ** 3, 'expanded size limit');
      if (hasModes) demand(record.memberModes[name] === (mode & 0o777), 'member permissions differ');
    }
    return name;
  };
  const keep = (name, mode, size, bytes) => {
    demand(bytes.length === size && sha256(bytes) === expected[name], 'member bytes differ');
    members.set(name, { bytes, mode: mode & 0o777 });
  };
  if (archive.endsWith('.zip')) {
    const entries = parseZipEntries(data, { zip64: true, maxEntries: 2048 });
    for (const [rawName, entry] of entries) {
      const directory = rawName.endsWith('/'), kind = entry.mode & 0o170000;
      demand(!rawName.includes('\0'), 'raw NUL ZIP member name');
      demand(!(entry.flags & 1)
        && (kind === 0 || kind === (directory ? 0o040000 : 0o100000)), 'encrypted/link/special ZIP member');
      let mode = entry.mode;
      if (target === 'windows/amd64') {
        if (mode === 0o100666) mode = 0o100644;
        else if (rawName === root + '/sing-box.exe' && mode === 0o100777) mode = 0o100755;
      }
      mode ||= directory ? 0o755 : 0o644;
      const name = admit(rawName, directory, mode, entry.uncompressedSize);
      if (directory) continue;
      const raw = data.subarray(entry.dataOffset, entry.dataOffset + entry.compressedSize);
      let bytes;
      if (entry.method === 0) bytes = raw;
      else {
        demand(entry.method === 8, 'unsupported ZIP compression');
        const decoded = inflateRawSync(raw, { maxOutputLength: entry.uncompressedSize + 1, info: true });
        demand(decoded.engine.bytesWritten === raw.length, 'trailing compressed ZIP bytes');
        bytes = decoded.buffer;
      }
      demand(crc32(bytes) === entry.crc32, 'ZIP member CRC differs');
      keep(name, mode, entry.uncompressedSize, bytes);
    }
  } else {
    demand(archive.endsWith('.tar.gz'), 'unsupported archive format');
    const expanded = gunzipSync(data, { maxOutputLength: 1024 ** 3 });
    demand(!(expanded[0] === 0x1f && expanded[1] === 0x8b), 'nested TAR compression');
    const { Parser } = require('tar');
    await new Promise((complete, fail) => {
      const parser = new Parser({ strict: true, zstd: false, maxMetaEntrySize: 1024 ** 3 });
      let entries = 0;
      parser.on('error', fail);
      parser.on('end', complete);
      parser.on('ignoredEntry', () => parser.abort(new Error('link/special archive entry')));
      parser.on('entry', (entry) => {
        try {
          const directory = entry.type === 'Directory';
          demand(++entries <= 2048 && (directory || ['File', 'OldFile', 'ContiguousFile'].includes(entry.type)),
            'link/special/entry limit');
          // ReadEntry normalizes Windows separators; admit the original header.
          const name = admit(entry.header.path, directory, entry.mode, entry.size);
          if (directory) { entry.resume(); return; }
          const chunks = []; let size = 0;
          entry.on('data', (chunk) => {
            size += chunk.length;
            if (size > entry.size) { parser.abort(new Error('member bytes differ')); return; }
            chunks.push(chunk);
          });
          entry.on('end', () => {
            try { keep(name, entry.mode, entry.size, chunks.length === 1 ? chunks[0] : Buffer.concat(chunks, size)); }
            catch (error) { parser.abort(error); }
          });
        } catch (error) { parser.abort(error); }
      });
      parser.end(expanded);
    });
  }
  demand(members.size === expectedNames.length && expectedNames.every((name) => members.has(name)), 'missing archive member');
  const prefixes = new Map(), files = new Set([...members.keys()].map((name) => name.toLowerCase()));
  for (const name of seen) {
    const parts = name.split('/');
    for (let i = 1; i < parts.length; i++) {
      const parent = parts.slice(0, i).join('/'), folded = parent.toLowerCase();
      demand(!files.has(folded), 'file/directory collision');
      demand(!prefixes.has(folded) || prefixes.get(folded) === parent, 'directory case alias');
      prefixes.set(folded, parent);
    }
  }
  demand(!existsSync(destination), 'extract destination already exists');
  regularPath(dirname(resolve(destination)), true);
  mkdirSync(destination);
  for (const [name, member] of members) {
    const path = join(destination, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, member.bytes, { flag: 'wx' });
    chmodSync(path, member.mode);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const request = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(readFileSync(0)));
    demand(request && !Array.isArray(request) && typeof request === 'object'
      && Object.keys(request).every((key) => ['archive', 'root', 'target'].includes(key)), 'unexpected archive request fields');
    demand(process.argv.length === 4 && !process.argv[2].startsWith('--'), 'unexpected native archive arguments');
    await extractNativeArchive(process.argv[2], process.argv[3], request.archive, request.root, request.target);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
