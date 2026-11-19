import { closeSync, constants, fstatSync, openSync, readSync } from 'node:fs';
import { crc32, inflateRawSync } from 'node:zlib';

export function withIndexedZip(path, { maximum, subject }, inspect) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const file = fstatSync(fd);
    if (!file.isFile() || file.size === 0 || file.size > maximum) throw new Error(`bounded regular ${subject} required`);
    const read = (offset, length) => {
      const bytes = Buffer.allocUnsafe(length);
      if (readSync(fd, bytes, 0, length, offset) !== length) throw new Error(`truncated ${subject} ZIP bytes`);
      return bytes;
    };
    return inspect(readZipEntries(file.size, read, { zip64: true, maxEntries: 65536 }), read);
  } finally {
    closeSync(fd);
  }
}

export function readZipEntry(entry, read, { name, maximum }) {
  if (!entry || (entry.flags & 1) || ![0, 8].includes(entry.method)
    || entry.uncompressedSize > maximum) throw new Error(`missing or unsupported ZIP member: ${name}`);
  const packed = read(entry.dataOffset, entry.compressedSize);
  const bytes = entry.method === 0 ? packed : inflateRawSync(packed, { maxOutputLength: entry.uncompressedSize + 1 });
  if (bytes.length !== entry.uncompressedSize || crc32(bytes) !== entry.crc32) {
    throw new Error(`ZIP member size or CRC differs: ${name}`);
  }
  return bytes;
}

export function parseZipEntries(buf, options) {
  return readZipEntries(buf.length, (start, length) => buf.subarray(start, start + length), options);
}

// The formal carrier scan reads from a retained file. APK and native-release
// callers can continue using their existing byte buffer.
export function readZipEntries(length, read, {
  zip64 = false, maxEntries = 65535, decodeName = (bytes) => bytes.toString('utf8'), strictMetadata = false,
} = {}) {
  const need = (ok, why) => { if (!ok) throw new Error(`ZIP: ${why}`); };
  const chunk = (at, size) => {
    need(at >= 0 && at + size <= length, 'truncated bytes');
    const result = read(at, size);
    need(result.length === size, 'truncated bytes');
    return result;
  };
  const integer64 = (bytes, at) => {
    need(at >= 0 && at + 8 <= bytes.length, 'truncated ZIP64 integer');
    const value = bytes.readBigUInt64LE(at);
    need(value <= BigInt(Number.MAX_SAFE_INTEGER), 'ZIP64 integer exceeds supported bounds');
    return Number(value);
  };

  const tailStart = Math.max(0, length - 22 - 65535);
  const tail = chunk(tailStart, length - tailStart);
  let eocd = -1, end;
  for (let p = tail.length - 22; p >= 0; p--) {
    if (tail.readUInt32LE(p) === 0x06054b50 && p + 22 + tail.readUInt16LE(p + 20) === tail.length) {
      eocd = tailStart + p; end = tail.subarray(p, p + 22); break;
    }
  }
  need(eocd >= 0, 'EOCD missing or truncated');
  need(end.readUInt16LE(4) === 0 && end.readUInt16LE(6) === 0, 'split ZIP is unsupported');
  let count = end.readUInt16LE(10), size = end.readUInt32LE(12), start = end.readUInt32LE(16);
  need(count === end.readUInt16LE(8), 'central directory bounds differ');
  let directoryEnd = eocd;
  const locator = zip64 && eocd >= 20 ? chunk(eocd - 20, 20) : null;
  if (count === 0xffff || size === 0xffffffff || start === 0xffffffff || locator?.readUInt32LE(0) === 0x07064b50) {
    need(zip64, 'ZIP64 is unsupported for APK');
    need(locator?.readUInt32LE(0) === 0x07064b50 && locator.readUInt32LE(4) === 0 && locator.readUInt32LE(16) === 1, 'ZIP64 locator differs');
    const at = integer64(locator, 8);
    need(at + 56 <= eocd - 20, 'ZIP64 directory bounds differ');
    const header = chunk(at, 56);
    need(header.readUInt32LE(0) === 0x06064b50 && integer64(header, 4) >= 44
      && header.readUInt32LE(16) === 0 && header.readUInt32LE(20) === 0, 'ZIP64 directory bounds differ');
    count = integer64(header, 32); need(count === integer64(header, 24), 'split ZIP64 is unsupported');
    size = integer64(header, 40); start = integer64(header, 48); directoryEnd = at;
  }
  need(count <= maxEntries && start + size <= directoryEnd, 'central directory bounds or entry limit differ');
  let base = zip64 ? directoryEnd - size - start : 0;
  if (base > 0 && start + 4 <= length && chunk(start, 4).readUInt32LE(0) === 0x02014b50) base = 0;
  start += base;

  const entries = new Map();
  let p = start;
  for (let i = 0; i < count; i++) {
    need(p + 46 <= start + size, 'central entry missing or truncated');
    const header = chunk(p, 46);
    need(header.readUInt32LE(0) === 0x02014b50, 'central entry missing or truncated');
    const nameLength = header.readUInt16LE(28), extraLength = header.readUInt16LE(30);
    const entryLength = 46 + nameLength + extraLength + header.readUInt16LE(32);
    need(p + entryLength <= start + size, 'central entry extends beyond directory');
    const nameBytes = chunk(p + 46, nameLength), flags = header.readUInt16LE(8);
    const name = decodeName(nameBytes, flags);
    need(!entries.has(name), `duplicate entry ${name}`);
    let local = header.readUInt32LE(42), compressedSize = header.readUInt32LE(20), uncompressedSize = header.readUInt32LE(24);
    const extendedSizes = [local, compressedSize, uncompressedSize].includes(0xffffffff);
    need(!extendedSizes || zip64, 'ZIP64 is unsupported for APK');
    if (strictMetadata) need(header[6] <= 63, 'unsupported ZIP extraction version');
    if (extendedSizes || strictMetadata) {
      const extras = chunk(p + 46 + nameLength, extraLength);
      let found = false;
      for (let extra = 0; extra + 4 <= extraLength;) {
        const tag = extras.readUInt16LE(extra), bytes = extras.readUInt16LE(extra + 2), end = extra + 4 + bytes;
        need(end <= extraLength, 'ZIP extra field bounds differ');
        if (tag === 1 && extendedSizes && !found) {
          let at = extra + 4;
          const value = () => { need(at + 8 <= end, 'truncated ZIP64 field'); const n = integer64(extras, at); at += 8; return n; };
          if (uncompressedSize === 0xffffffff) uncompressedSize = value();
          if (compressedSize === 0xffffffff) compressedSize = value();
          if (local === 0xffffffff) local = value();
          found = true;
          if (!strictMetadata) break;
        }
        if (tag === 0x7075 && strictMetadata) {
          const data = extras.subarray(extra + 4, end);
          need(data.length >= 5, 'truncated ZIP Unicode path');
          if (data[0] === 1 && data.readUInt32LE(1) === crc32(nameBytes) && data.length > 5) {
            need(decodeName(data.subarray(5), 0x800) === name, 'ZIP Unicode path differs from original');
          }
        }
        extra = end;
      }
      need(!extendedSizes || found, 'ZIP64 extra field missing');
    }
    local += base;
    const method = header.readUInt16LE(10);
    need(local + 30 <= start, `${name}: local header missing`);
    const localHeader = chunk(local, 30);
    need(localHeader.readUInt32LE(0) === 0x04034b50, `${name}: local header missing`);
    const localNameLength = localHeader.readUInt16LE(26);
    const dataOffset = local + 30 + localNameLength + localHeader.readUInt16LE(28);
    need(dataOffset + compressedSize <= start, `${name}: data extends beyond central directory`);
    const localName = chunk(local + 30, localNameLength);
    need(localHeader.readUInt16LE(8) === method && localName.equals(nameBytes)
      && (!strictMetadata || decodeName(localName, localHeader.readUInt16LE(6)) === name),
      `${name}: local and central headers differ`);
    entries.set(name, { method, compressedSize, uncompressedSize, dataOffset, localOffset: local,
      flags, crc32: header.readUInt32LE(16), mode: header.readUInt32LE(38) >>> 16 });
    p += entryLength;
  }
  need(p === start + size, 'central directory length differs');
  if (zip64) {
    const ordered = [...entries.values()].sort((a, b) => a.localOffset - b.localOffset);
    for (let i = 0; i < ordered.length; i++) need(ordered[i].dataOffset + ordered[i].compressedSize
      <= (ordered[i + 1]?.localOffset ?? start), 'overlapping ZIP members');
  }
  return entries;
}
