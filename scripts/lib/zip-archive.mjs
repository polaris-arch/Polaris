export function parseZipEntries(buf, { zip64 = false, maxEntries = 65535 } = {}) {
  const need = (ok, why) => { if (!ok) throw new Error(`ZIP: ${why}`); };
  const integer64 = (at) => {
    need(at >= 0 && at + 8 <= buf.length, 'truncated ZIP64 integer');
    const value = buf.readBigUInt64LE(at);
    need(value <= BigInt(Number.MAX_SAFE_INTEGER), 'ZIP64 integer exceeds supported bounds');
    return Number(value);
  };
  let eocd = -1;
  for (let p = buf.length - 22; p >= Math.max(0, buf.length - 22 - 65535); p--) {
    if (buf.readUInt32LE(p) === 0x06054b50 && p + 22 + buf.readUInt16LE(p + 20) === buf.length) { eocd = p; break; }
  }
  need(eocd >= 0, 'EOCD missing or truncated');
  need(buf.readUInt16LE(eocd + 4) === 0 && buf.readUInt16LE(eocd + 6) === 0, 'split ZIP is unsupported');
  let count = buf.readUInt16LE(eocd + 10), size = buf.readUInt32LE(eocd + 12), start = buf.readUInt32LE(eocd + 16);
  need(count === buf.readUInt16LE(eocd + 8), 'central directory bounds differ');
  let directoryEnd = eocd;
  if (count === 0xffff || size === 0xffffffff || start === 0xffffffff
    || zip64 && eocd >= 20 && buf.readUInt32LE(eocd - 20) === 0x07064b50) {
    need(zip64, 'ZIP64 is unsupported for APK');
    need(eocd >= 20 && buf.readUInt32LE(eocd - 20) === 0x07064b50
      && buf.readUInt32LE(eocd - 16) === 0 && buf.readUInt32LE(eocd - 4) === 1, 'ZIP64 locator differs');
    const at = integer64(eocd - 12);
    need(at + 56 <= eocd - 20 && buf.readUInt32LE(at) === 0x06064b50 && integer64(at + 4) >= 44
      && buf.readUInt32LE(at + 16) === 0 && buf.readUInt32LE(at + 20) === 0, 'ZIP64 directory bounds differ');
    count = integer64(at + 32); need(count === integer64(at + 24), 'split ZIP64 is unsupported');
    size = integer64(at + 40); start = integer64(at + 48); directoryEnd = at;
  }
  need(count <= maxEntries && start + size <= directoryEnd, 'central directory bounds or entry limit differ');
  let base = zip64 ? directoryEnd - size - start : 0;
  if (base > 0 && start + 4 <= buf.length && buf.readUInt32LE(start) === 0x02014b50) base = 0;
  start += base;
  const entries = new Map();
  let p = start;
  for (let i = 0; i < count; i++) {
    need(p + 46 <= start + size && buf.readUInt32LE(p) === 0x02014b50, 'central entry missing or truncated');
    const nameLength = buf.readUInt16LE(p + 28), extraLength = buf.readUInt16LE(p + 30);
    const length = 46 + nameLength + extraLength + buf.readUInt16LE(p + 32);
    need(p + length <= start + size, 'central entry extends beyond directory');
    const nameBytes = buf.subarray(p + 46, p + 46 + nameLength), name = nameBytes.toString('utf8');
    need(!entries.has(name), `duplicate entry ${name}`);
    let local = buf.readUInt32LE(p + 42), compressedSize = buf.readUInt32LE(p + 20), uncompressedSize = buf.readUInt32LE(p + 24);
    if ([local, compressedSize, uncompressedSize].includes(0xffffffff)) {
      need(zip64, 'ZIP64 is unsupported for APK');
      let found = false;
      for (let extra = p + 46 + nameLength; extra + 4 <= p + 46 + nameLength + extraLength;) {
        const tag = buf.readUInt16LE(extra), bytes = buf.readUInt16LE(extra + 2), end = extra + 4 + bytes;
        need(end <= p + 46 + nameLength + extraLength, 'ZIP64 extra field bounds differ');
        if (tag === 1) {
          let at = extra + 4;
          const value = () => { need(at + 8 <= end, 'truncated ZIP64 field'); const n = integer64(at); at += 8; return n; };
          if (uncompressedSize === 0xffffffff) uncompressedSize = value();
          if (compressedSize === 0xffffffff) compressedSize = value();
          if (local === 0xffffffff) local = value();
          found = true; break;
        }
        extra = end;
      }
      need(found, 'ZIP64 extra field missing');
    }
    local += base;
    const method = buf.readUInt16LE(p + 10), flags = buf.readUInt16LE(p + 8);
    need(local + 30 <= start && buf.readUInt32LE(local) === 0x04034b50, `${name}: local header missing`);
    const localNameLength = buf.readUInt16LE(local + 26);
    const dataOffset = local + 30 + localNameLength + buf.readUInt16LE(local + 28);
    need(dataOffset + compressedSize <= start, `${name}: data extends beyond central directory`);
    need(buf.readUInt16LE(local + 8) === method && localNameLength === nameLength &&
      buf.subarray(local + 30, local + 30 + localNameLength).equals(nameBytes), `${name}: local and central headers differ`);
    entries.set(name, { method, compressedSize, uncompressedSize, dataOffset, localOffset: local,
      flags, crc32: buf.readUInt32LE(p + 16), mode: buf.readUInt32LE(p + 38) >>> 16 });
    p += length;
  }
  need(p === start + size, 'central directory length differs');
  if (zip64) {
    const ordered = [...entries.values()].sort((a, b) => a.localOffset - b.localOffset);
    for (let i = 0; i < ordered.length; i++) need(ordered[i].dataOffset + ordered[i].compressedSize
      <= (ordered[i + 1]?.localOffset ?? start), 'overlapping ZIP members');
  }
  return entries;
}
