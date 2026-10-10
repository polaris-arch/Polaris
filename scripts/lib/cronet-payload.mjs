// Static file inspection only: never dlopen or execute a kernel/library.
import { createHash } from 'node:crypto';
import { lstatSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { validateCronetLibraryPins } from './cronet-contract.mjs';

export const CRONET_REQUIRED_EXPORTS = Object.freeze([
  'Cronet_Engine_Create', 'Cronet_EngineParams_Create', 'Cronet_Engine_StartWithParams',
  'Cronet_UrlRequest_Create', 'Cronet_UrlRequest_InitWithParams', 'Cronet_UrlRequest_Start',
]);
const ELF_DEPENDENCIES = new Set(['libdl.so.2', 'libpthread.so.0', 'libm.so.6', 'libgcc_s.so.1', 'libc.so.6', 'ld-linux-x86-64.so.2']);
const PE_DEPENDENCIES = new Set([
  'kernel32.dll', 'advapi32.dll', 'dbghelp.dll', 'ws2_32.dll', 'shell32.dll', 'iphlpapi.dll',
  'winmm.dll', 'shlwapi.dll', 'ole32.dll', 'winhttp.dll', 'user32.dll', 'secur32.dll',
  'crypt32.dll', 'api-ms-win-core-winrt-l1-1-0.dll', 'ntdll.dll',
]);
const demand = (condition, message) => { if (!condition) throw new Error(`Cronet payload: ${message}`); };
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

function reader(bytes) {
  demand(Buffer.isBuffer(bytes), 'expected binary bytes');
  const range = (offset, length) => {
    demand(Number.isSafeInteger(offset) && Number.isSafeInteger(length) && offset >= 0 && length >= 0
      && offset <= bytes.length - length, 'binary range outside file');
    return offset;
  };
  const u16 = (at) => bytes.readUInt16LE(range(at, 2));
  const u32 = (at) => bytes.readUInt32LE(range(at, 4));
  const u64 = (at) => { const value = Number(bytes.readBigUInt64LE(range(at, 8))); demand(Number.isSafeInteger(value), 'unsafe 64-bit offset'); return value; };
  const string = (at, end) => {
    range(at, end - at);
    const zero = bytes.indexOf(0, at);
    demand(zero >= at && zero < end, 'unterminated binary string');
    const value = bytes.subarray(at, zero).toString('ascii');
    demand(value.length > 0 && /^[\x21-\x7e]+$/.test(value), 'invalid binary string');
    return value;
  };
  return { range, u16, u32, u64, string };
}

function inspectElf(bytes) {
  const r = reader(bytes);
  r.range(0, 64);
  demand(bytes.subarray(0, 4).equals(Buffer.from([0x7f, 69, 76, 70])) && bytes[4] === 2 && bytes[5] === 1
    && bytes[6] === 1 && r.u32(20) === 1 && r.u16(16) === 3 && r.u16(18) === 62, 'expected ELF64 little-endian x86-64 shared library');
  const offset = r.u64(40), count = r.u16(60);
  demand(r.u16(58) === 64 && count > 0, 'missing ELF section table');
  r.range(offset, count * 64);
  const sections = Array.from({ length: count }, (_, i) => {
    const at = offset + i * 64;
    const section = { type: r.u32(at + 4), flags: r.u64(at + 8), address: r.u64(at + 16),
      offset: r.u64(at + 24), size: r.u64(at + 32), link: r.u32(at + 40), entrySize: r.u64(at + 56) };
    if (section.type !== 8) r.range(section.offset, section.size); // NOBITS has no file bytes
    return section;
  });
  const one = (type) => { const found = sections.filter((s) => s.type === type); demand(found.length === 1, `expected one ELF section type ${type}`); return found[0]; };
  const strings = (section) => {
    const linked = sections[section.link];
    demand(linked?.type === 3, 'ELF section lacks linked string table');
    return (index) => { demand(index < linked.size, 'ELF string index outside table'); return r.string(linked.offset + index, linked.offset + linked.size); };
  };
  const dynamic = one(6), dynString = strings(dynamic);
  demand(dynamic.entrySize === 16 && dynamic.size % 16 === 0, 'malformed ELF dynamic table');
  const dependencies = []; let terminated = false;
  for (let at = dynamic.offset; at < dynamic.offset + dynamic.size; at += 16) {
    const tag = r.u64(at), value = r.u64(at + 8);
    if (tag === 0) { terminated = true; break; }
    if (tag === 1) dependencies.push(dynString(value));
    demand(tag !== 15 && tag !== 29, 'RPATH/RUNPATH transform not permitted by canonical-byte policy');
  }
  demand(terminated, 'unterminated ELF dynamic table');
  const sym = one(11), symString = strings(sym);
  demand(sym.entrySize === 24 && sym.size % 24 === 0, 'malformed ELF dynsym');
  const exports = new Set();
  for (let at = sym.offset; at < sym.offset + sym.size; at += 24) {
    const name = r.u32(at), info = bytes[at + 4], visibility = bytes[at + 5] & 3, index = r.u16(at + 6);
    const section = sections[index], address = r.u64(at + 8);
    if (name && (info >> 4 === 1 || info >> 4 === 2) && (info & 15) === 2 && (visibility === 0 || visibility === 3)
        && index !== 0 && section && section.type !== 8 && (section.flags & 4)
        && address >= section.address && address < section.address + section.size) exports.add(symString(name));
  }
  return { format: 'ELF64', architecture: 'amd64', dependencies, exports };
}

function inspectPe(bytes) {
  const r = reader(bytes);
  r.range(0, 64);
  demand(bytes.toString('ascii', 0, 2) === 'MZ', 'expected DOS/PE header');
  const pe = r.u32(60); r.range(pe, 24);
  demand(bytes.toString('ascii', pe, pe + 4) === 'PE\0\0' && r.u16(pe + 4) === 0x8664
    && (r.u16(pe + 22) & 0x2000), 'expected PE AMD64 DLL');
  const optional = pe + 24, size = r.u16(pe + 20), count = r.u16(pe + 6);
  r.range(optional, size);
  demand(size >= 224 && r.u16(optional) === 0x20b && r.u32(optional + 108) >= 14 && count > 0, 'expected PE32+ data directories');
  demand(r.u32(optional + 112 + 13 * 8) === 0, 'delay imports require separate ABI policy');
  const sectionsAt = optional + size; r.range(sectionsAt, count * 40);
  const sections = Array.from({ length: count }, (_, i) => {
    const at = sectionsAt + i * 40;
    const section = { address: r.u32(at + 12), size: r.u32(at + 16), offset: r.u32(at + 20), executable: !!(r.u32(at + 36) & 0x20000000) };
    r.range(section.offset, section.size); return section;
  });
  const locate = (address, length = 1) => {
    const matches = sections.filter((s) => address >= s.address && address - s.address <= s.size - length);
    demand(matches.length === 1, 'PE RVA outside one file-backed section');
    const section = matches[0]; return { at: section.offset + address - section.address, end: section.offset + section.size, section };
  };
  const string = (address) => { const p = locate(address); return r.string(p.at, p.end); };
  const expAddress = r.u32(optional + 112), expSize = r.u32(optional + 116);
  demand(expAddress > 0 && expSize >= 40, 'missing PE exports');
  const exp = locate(expAddress, 40).at;
  const functions = r.u32(exp + 20), names = r.u32(exp + 24);
  demand(names > 0 && names <= functions, 'malformed PE export count');
  const addresses = locate(r.u32(exp + 28), functions * 4).at;
  const nameTable = locate(r.u32(exp + 32), names * 4).at;
  const ordinals = locate(r.u32(exp + 36), names * 2).at;
  const exports = new Set();
  for (let i = 0; i < names; i++) {
    const name = string(r.u32(nameTable + i * 4)), ordinal = r.u16(ordinals + i * 2);
    demand(ordinal < functions, 'PE export ordinal outside table');
    const address = r.u32(addresses + ordinal * 4);
    if (address && !(address >= expAddress && address < expAddress + expSize) && locate(address).section.executable) exports.add(name);
  }
  const importAddress = r.u32(optional + 120), importSize = r.u32(optional + 124);
  demand(importAddress > 0 && importSize >= 20, 'missing PE import descriptors');
  const imp = locate(importAddress, importSize).at;
  const dependencies = []; let terminated = false;
  for (let at = imp; at + 20 <= imp + importSize; at += 20) {
    if ([0, 4, 8, 12, 16].every((off) => r.u32(at + off) === 0)) { terminated = true; break; }
    dependencies.push(string(r.u32(at + 12)).toLowerCase());
  }
  demand(terminated, 'unterminated PE import descriptors');
  return { format: 'PE32+', architecture: 'amd64', dependencies, exports };
}

export function inspectCronetBytes(bytes, platform) {
  demand(platform === 'linux' || platform === 'win', 'unsupported dynamic-library platform');
  const result = platform === 'linux' ? inspectElf(bytes) : inspectPe(bytes);
  const allowed = platform === 'linux' ? ELF_DEPENDENCIES : PE_DEPENDENCIES;
  const required = platform === 'linux' ? 'libc.so.6' : 'kernel32.dll';
  demand(result.dependencies.includes(required) && result.dependencies.every((name) => allowed.has(name))
    && new Set(result.dependencies).size === result.dependencies.length, 'unexpected or missing system dependencies');
  for (const name of CRONET_REQUIRED_EXPORTS) demand(result.exports.has(name), `missing directly defined C API ${name}`);
  return { format: result.format, architecture: result.architecture, dependencies: result.dependencies.sort(), requiredExports: [...CRONET_REQUIRED_EXPORTS] };
}

function regularBytes(path) {
  let parent = dirname(resolve(path));
  for (;;) {
    demand(lstatSync(parent).isDirectory(), `non-directory/symlink ancestor: ${parent}`);
    const next = dirname(parent); if (next === parent) break; parent = next;
  }
  demand(lstatSync(path).isFile(), `not a regular file: ${path}`);
  return readFileSync(path);
}

// Both input and final payload bind to the manifest's source-library digest.
// Same size, GNU Build ID and an already-processed AppDir are never authority.
export function verifyCronetPayload(source, payload, platform, manifest) {
  const pin = validateCronetLibraryPins(manifest.cronetLibrarySha256)[platform];
  const sourceBytes = regularBytes(source), payloadBytes = regularBytes(payload);
  demand(sha256(sourceBytes) === pin, 'source SHA-256 differs from manifest pin');
  demand(sha256(payloadBytes) === pin, 'final SHA-256 differs from manifest pin');
  demand((lstatSync(source).mode & 0o7777) === (lstatSync(payload).mode & 0o7777), 'final permissions differ from source');
  return { sha256: pin, mode: lstatSync(source).mode & 0o7777, bytes: sourceBytes.length, ...inspectCronetBytes(payloadBytes, platform) };
}
