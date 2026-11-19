// Inspect raw Apple carriers; framework, bundle identity and signing are
// independently admitted by consumers of these original-byte observations.
import { createHash } from 'node:crypto';

const MACHO = Buffer.from('cffaedfe', 'hex');
const AR = Buffer.from('!<arch>\n');
export const FAT = Buffer.from('cafebabe', 'hex');
const INFO = Buffer.from('\xff Go buildinf:', 'latin1');
const INFO_ENCODING = Buffer.from('080200000000000000000000000000000000', 'hex');
const INFO_START = Buffer.from('3077af0c9274080241e1c107e6d618e6', 'hex');
const INFO_END = Buffer.from('f932433186182072008242104116d8f2', 'hex');
const ID_START = Buffer.from('\xff Go build ID: "', 'latin1');
const ID_END = Buffer.from('"\n \xff', 'latin1');
const CPU_SUBTYPE = new Map([[0x100000c, 0], [0x1000007, 3]]);
const SECTION_TYPES = new Set([0, 1, 2, 3, 4, 5, 6, 8, 9, 11, 14, 17, 18, 19, 22]);
const LITERAL_TLV_TYPES = new Set([3, 4, 5, 14, 17, 18, 19]);
const ELEMENT_WIDTHS = new Map([[3, 4n], [4, 8n], [5, 8n], [14, 16n], [19, 24n]]);
const FIXED_COMMAND_BYTES = new Map([[27, 24], [42, 16], [0x80000028, 24], [44, 24]]);
const FRAMEWORKS = new Set(['Foundation', 'CFNetwork', 'Security', 'CoreFoundation']);
const RPATHS = new Set(['/usr/lib/swift', '@executable_path/Frameworks', '@executable_path/../../Frameworks']);
const UTF8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

function require(condition, reason) {
  if (!condition) throw new Error('Apple carrier: ' + reason);
}

const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const startsWith = (bytes, prefix) => bytes.subarray(0, prefix.length).equals(prefix);
const allZero = bytes => bytes.every(byte => byte === 0);
const printable = bytes => bytes.every(byte => byte >= 32 && byte < 127);
const compare = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const orderRanges = (a, b) => compare(a[0], b[0]) || compare(a[1], b[1]);

function bounds(offset, size, total, label) {
  require(offset >= 0 && offset <= total && size >= 0 && size <= total - offset, label + ' bounds');
}

function disjoint(ranges, label) {
  let end = 0;
  for (const [start, stop] of ranges.sort(orderRanges)) {
    if (start === stop) continue;
    require(start >= end, label + ' overlap');
    end = stop;
  }
}

function utf8(bytes, label) {
  try {
    return UTF8.decode(bytes);
  } catch (cause) {
    throw new Error('Apple carrier: ' + label + ' UTF-8', { cause });
  }
}

function nameBytes(raw, reason) {
  const nul = raw.indexOf(0);
  if (nul === -1) return raw;
  require(allZero(raw.subarray(nul)), reason);
  return raw.subarray(0, nul);
}

function fixedName(raw) {
  const name = nameBytes(raw, 'name padding');
  require(name.length && printable(name), 'invalid section/segment name');
  return name.toString('ascii');
}

function inlineString(bytes, offset) {
  let value = 0n;
  for (let index = 0; index < 10; index++) {
    bounds(offset + index, 1, bytes.length, 'inline varint');
    const byte = bytes[offset + index];
    require(index < 9 || byte <= 1, 'inline varint overflow');
    value |= BigInt(byte & 127) << BigInt(7 * index);
    if (byte < 128) {
      require(index === 0 || byte !== 0, 'noncanonical inline varint');
      const start = offset + index + 1;
      bounds(BigInt(start), value, BigInt(bytes.length), 'inline string');
      const end = start + Number(value);
      return [bytes.subarray(start, end), end];
    }
  }
  require(false, 'inline varint overflow');
}

export function buildInfo(payload) {
  require(payload.length >= 32 && startsWith(payload, INFO), 'BuildInfo header');
  require(payload.subarray(14, 32).equals(INFO_ENCODING), 'unsupported BuildInfo encoding');
  const [versionBytes, framingStart] = inlineString(payload, 32);
  const [framed, end] = inlineString(payload, framingStart);
  require(payload.length % 16 === 0 && payload.length - end < 16 && allZero(payload.subarray(end)),
    'BuildInfo padding');
  require(framed.length > 32 && startsWith(framed, INFO_START) && framed.subarray(-16).equals(INFO_END),
    'BuildInfo module framing');
  const version = utf8(versionBytes, 'BuildInfo');
  const raw = utf8(framed.subarray(16, -16), 'BuildInfo');
  require(/^go\p{Nd}+\.\p{Nd}+\.\p{Nd}+(?![\s\S])/u.test(version)
    && raw.endsWith('\n') && !raw.includes('\0') && !raw.includes('\r'), 'BuildInfo raw framing');
  // Keep Python splitlines semantics for the original section adapter.
  const lines = raw.slice(0, -1).split(/[\n\u000b\u000c\u001c-\u001e\u0085\u2028\u2029]/u);
  const parserInput = 'observed-section: ' + version + '\n' + lines.map(line => '\t' + line + '\n').join('');
  return { goVersionRaw: version, moduleRaw: raw, parserInput, headerHex: payload.subarray(0, 32).toString('hex'),
    provenance: 'validated __DATA,__go_buildinfo inline section' };
}

function readWords(data, offset, count, bigEndian = false) {
  return Array.from({ length: count }, (_, index) => bigEndian
    ? data.readUInt32BE(offset + index * 4) : data.readUInt32LE(offset + index * 4));
}

function stringEnd(data, limit, start, label) {
  const length = data.subarray(start, limit).indexOf(0);
  require(length >= 0, label);
  return start + length;
}

const exactInteger = value => value <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(value) : value;

export function macho(data, observeBuildInfo) {
  bounds(0, 32, data.length, 'Mach-O header');
  require(startsWith(data, MACHO), 'unsupported Mach-O format');
  const [cpu, subCPU, filetype, count, commandBytes, flags, reserved] = readWords(data, 4, 7);
  require(CPU_SUBTYPE.get(cpu) === subCPU && (filetype === 1 || filetype === 2) && reserved === 0,
    'unsupported Mach-O CPU/subCPU/filetype');
  bounds(32, commandBytes, data.length, 'load-command region');
  require(count > 0 && count <= Math.floor(commandBytes / 8), 'load-command count');
  const limit = 32 + commandBytes;
  const sections = [], segments = [], vmSegments = [], platformCommands = [], loadCommands = [], auxiliary = [];
  const rpaths = new Set(), linkeditSegments = [];
  let symbolCount, dynamic, optimizationHint, codeSignature, offset = 32;
  for (let ordinal = 0; ordinal < count; ordinal++) {
    bounds(offset, 8, limit, 'load-command header');
    const [command, size] = readWords(data, offset, 2);
    require(size >= 8 && size % 8 === 0, 'load-command size');
    bounds(offset, size, limit, 'load-command');
    const commandEnd = offset + size;
    const row = { ordinal, command, offset, bytes: size };
    loadCommands.push(row);
    switch (command) {
      case 0x19: {
        require(size >= 72, 'segment command size');
        const rawName = data.subarray(offset + 8, offset + 24);
        // MH_OBJECT can contain one anonymous segment and named sections.
        const name = allZero(rawName) ? '' : fixedName(rawName);
        const address = data.readBigUInt64LE(offset + 24);
        const vmSize = data.readBigUInt64LE(offset + 32);
        const fileOffset = data.readBigUInt64LE(offset + 40);
        const fileBytes = data.readBigUInt64LE(offset + 48);
        const [maximum, initial, sectionCount] = readWords(data, offset + 56, 3);
        require(size === 72 + sectionCount * 80 && vmSize >= fileBytes && (maximum & ~7) === 0
          && (initial & ~maximum) === 0, 'segment layout');
        bounds(fileOffset, fileBytes, BigInt(data.length), 'segment file');
        require(address + vmSize <= 1n << 64n, 'segment address overflow');
        const fileRange = [Number(fileOffset), Number(fileOffset + fileBytes)];
        segments.push(fileRange);
        vmSegments.push([address, address + vmSize]);
        if (name === '__LINKEDIT') linkeditSegments.push(fileRange);
        for (let index = 0; index < sectionCount; index++) {
          const start = offset + 72 + index * 80;
          const section = fixedName(data.subarray(start, start + 16));
          const segment = fixedName(data.subarray(start + 16, start + 32));
          const sectionAddress = data.readBigUInt64LE(start + 32);
          const bytes = data.readBigUInt64LE(start + 40);
          const [off, align, relocationOffset, relocationCount, sectionFlags, r1, r2, r3] = readWords(data, start + 48, 8);
          require(!name || name === segment, 'section segment differs');
          require(sectionAddress >= address && bytes <= address + vmSize - sectionAddress && align <= 31
            && sectionAddress % (1n << BigInt(align)) === 0n, 'section address/alignment');
          const sectionType = sectionFlags & 255;
          require(SECTION_TYPES.has(sectionType), 'unsupported section type');
          if (LITERAL_TLV_TYPES.has(sectionType)) {
            require(r1 === 0 && r2 === 0 && r3 === 0, 'literal/TLV reserved fields');
            const width = ELEMENT_WIDTHS.get(sectionType);
            require(!width || bytes % width === 0n, 'literal/TLV element width');
            if (sectionType === 18) require(relocationCount === 0, 'TLV zero-fill relocation');
          }
          const zeroFill = sectionType === 1 || sectionType === 18;
          const observation = { segment, section, address: sectionAddress, offset: off, bytes,
            flags: sectionFlags, zeroFill, relocationOffset, relocationCount };
          bounds(relocationOffset, relocationCount * 8, data.length, 'section relocation');
          require(relocationCount || relocationOffset === 0, 'section relocation offset without entries');
          if (relocationCount) {
            require(relocationOffset >= limit, 'section relocation in headers');
            auxiliary.push([relocationOffset, relocationOffset + relocationCount * 8]);
          }
          if (zeroFill) {
            require(off === 0, 'zero-fill file offset');
          } else {
            bounds(BigInt(off), bytes, BigInt(data.length), 'section file');
            require(off >= limit && off >= fileRange[0] && Number(bytes) <= fileRange[1] - off,
              'section outside segment/header');
            observation.sha256 = sha(data.subarray(off, off + Number(bytes)));
          }
          sections.push(observation);
        }
        break;
      }
      case 0x32: {
        require(size >= 24, 'platform command size');
        const [platform, minOSRaw, sdkRaw, tools] = readWords(data, offset + 8, 4);
        require(size === 24 + tools * 8 && (platform === 2 || platform === 7), 'platform command layout');
        platformCommands.push({ platform, minOSRaw, sdkRaw });
        break;
      }
      case 0x2d: {
        // Only observed MH_OBJECT framework pairs, with exact NUL padding.
        require(filetype === 1 && (size === 32 || size === 40), 'linker option object/size');
        const optionCount = data.readUInt32LE(offset + 8);
        require(optionCount === 2, 'linker option count');
        const options = [];
        let cursor = offset + 12;
        for (let index = 0; index < optionCount; index++) {
          const stop = stringEnd(data, commandEnd, cursor, 'linker option string termination');
          require(stop > cursor, 'linker option string termination');
          options.push(utf8(data.subarray(cursor, stop), 'linker option'));
          cursor = stop + 1;
        }
        require(options[0] === '-framework' && FRAMEWORKS.has(options[1]), 'unsupported linker option pair');
        require(commandEnd - cursor < 8 && allZero(data.subarray(cursor, commandEnd)), 'linker option padding');
        Object.assign(row, { count: optionCount, options, paddingBytes: commandEnd - cursor,
          payloadHex: data.subarray(offset, commandEnd).toString('hex') });
        break;
      }
      case 2: {
        require(size === 24 && symbolCount === undefined, 'symbol command size/duplicate');
        const [symoff, nsyms, stroff, strsize] = readWords(data, offset + 8, 4);
        symbolCount = nsyms;
        for (const [start, bytes] of [[symoff, nsyms * 16], [stroff, strsize]]) {
          bounds(start, bytes, data.length, 'symbol table');
          if (bytes) {
            require(start >= limit, 'symbol table in headers');
            auxiliary.push([start, start + bytes]);
          }
        }
        break;
      }
      case 11: {
        require(size === 80 && dynamic === undefined, 'dynamic symbol command size/duplicate');
        dynamic = readWords(data, offset + 8, 18);
        for (const [index, width] of [[6, 8], [8, 56], [10, 4], [12, 4], [14, 8], [16, 8]]) {
          const start = dynamic[index], bytes = dynamic[index + 1] * width;
          bounds(start, bytes, data.length, 'dynamic symbol table');
          if (bytes) {
            require(start >= limit, 'dynamic symbol table in headers');
            auxiliary.push([start, start + bytes]);
          }
        }
        break;
      }
      case 0x2e:
      case 0x1d:
      case 0x80000033:
      case 0x80000034:
      case 0x26:
      case 0x29: {
        const optimization = command === 0x2e, signature = command === 0x1d;
        if (optimization) require(filetype === 1 && size === 16 && !optimizationHint,
          'optimization hint object/size/duplicate');
        else if (signature) require(filetype === 2 && size === 16 && !codeSignature,
          'code signature executable/size/duplicate');
        else require(size === 16, 'linkedit command size');
        const label = optimization ? 'optimization hint' : signature ? 'code signature' : 'linkedit';
        const [start, bytes] = readWords(data, offset + 8, 2);
        bounds(start, bytes, data.length, label + ' data');
        const range = [start, start + bytes];
        if (signature) require(bytes > 0 && start >= limit, 'code signature empty/in headers');
        else require(!bytes || start >= limit, label + (optimization ? ' in headers' : ' data in headers'));
        if (bytes) auxiliary.push(range);
        if (optimization || signature) {
          Object.assign(row, { dataOffset: start, dataBytes: bytes, payloadSha256: sha(data.subarray(...range)) });
          if (optimization) optimizationHint = range;
          else {
            // Preserve opaque signature bytes. No CodeDirectory or trust assertion.
            codeSignature = range;
            Object.assign(row, { payloadKind: 'opaque-code-signature', trustValidated: false });
          }
        }
        break;
      }
      case 0x80000018:
      case 0x8000001c: {
        require(filetype === 2, 'weak dylib/rpath requires linked executable');
        const weak = command === 0x80000018, base = weak ? 24 : 12;
        require(size >= base + 1 && size <= 160, 'weak dylib/rpath command size');
        const nameOffset = data.readUInt32LE(offset + 8);
        require(nameOffset === base && nameOffset < size, 'weak dylib/rpath string offset/prefix');
        const start = offset + nameOffset;
        const stop = stringEnd(data, commandEnd, start, 'weak dylib/rpath string termination/length');
        require(stop > start && stop <= start + 128, 'weak dylib/rpath string termination/length');
        const paddingBytes = commandEnd - stop - 1;
        require(paddingBytes < 8 && allZero(data.subarray(stop + 1, commandEnd)), 'weak dylib/rpath string padding');
        const name = utf8(data.subarray(start, stop), 'weak dylib/rpath');
        Object.assign(row, { name, nameOffset, paddingBytes, payloadHex: data.subarray(offset, commandEnd).toString('hex') });
        if (weak) {
          require(/^\/usr\/lib\/swift\/libswift[A-Za-z0-9_]{1,64}\.dylib(?![\s\S])/u.test(name),
            'weak dylib must be an observed system Swift library');
          const [timestamp, currentVersionRaw, compatibilityVersionRaw] = readWords(data, offset + 12, 3);
          require(currentVersionRaw >= compatibilityVersionRaw, 'weak dylib version relationship');
          Object.assign(row, { weak, timestamp, currentVersionRaw, compatibilityVersionRaw });
        } else {
          require(RPATHS.has(name) && !rpaths.has(name), 'unobserved or duplicate executable rpath');
          rpaths.add(name);
        }
        break;
      }
      case 12:
      case 14: {
        const base = command === 12 ? 24 : 12;
        require(size >= base, 'dylib/dylinker command size');
        const start = data.readUInt32LE(offset + 8);
        require(start >= base && start < size, 'load-command string bounds');
        stringEnd(data, commandEnd, offset + start, 'load-command string bounds');
        break;
      }
      case 27:
      case 42:
      case 0x80000028:
      case 44: {
        require(size === FIXED_COMMAND_BYTES.get(command), 'fixed load-command size');
        if (command === 0x80000028) {
          require(data.readBigUInt64LE(offset + 8) < BigInt(data.length), 'entry point bounds');
        }
        if (command === 44) {
          const [start, bytes, cryptId, padding] = readWords(data, offset + 8, 4);
          bounds(start, bytes, data.length, 'encryption region');
          require(cryptId === 0 && padding === 0, 'encrypted Mach-O unsupported');
        }
        break;
      }
      default:
        require(false, 'unsupported load command 0x' + command.toString(16));
    }
    offset = commandEnd;
  }
  require(offset === limit, 'load-command total differs');
  if (dynamic) {
    require(symbolCount !== undefined, 'dynamic symbols without symbol table');
    for (const index of [0, 2, 4]) bounds(dynamic[index], dynamic[index + 1], symbolCount, 'dynamic symbol index');
  }
  require(platformCommands.length <= 1, 'duplicate platform command');
  if (codeSignature) {
    require(linkeditSegments.length === 1 && linkeditSegments[0][0] <= codeSignature[0]
      && codeSignature[0] < codeSignature[1] && codeSignature[1] <= linkeditSegments[0][1],
    'code signature outside unique __LINKEDIT segment');
  }
  require(new Set(sections.map(row => row.segment + '\0' + row.section)).size === sections.length, 'duplicate section');
  disjoint(segments, 'segment file');
  disjoint(vmSegments, 'segment address');
  disjoint(sections.filter(row => !row.zeroFill).map(row => [row.offset, row.offset + Number(row.bytes)])
    .concat(auxiliary), 'section/auxiliary file');
  disjoint(sections.map(row => [row.address, row.address + row.bytes]), 'section address');

  const infos = [];
  let identities = 0;
  for (const row of sections) {
    const payload = row.zeroFill ? Buffer.alloc(0)
      : data.subarray(row.offset, row.offset + Number(row.bytes));
    if (row.section === '__go_buildinfo') {
      require(row.segment === '__DATA' && (row.flags & 255) === 0, 'BuildInfo locus');
      row.buildInfo = buildInfo(payload);
      infos.push(row.buildInfo);
    }
    if (row.segment === '__TEXT' && row.section === '__text') {
      row.buildIDs = [];
      let cursor = 0;
      for (let found; (found = payload.indexOf(ID_START, cursor)) !== -1;) {
        const end = payload.indexOf(ID_END, found + ID_START.length);
        require(end !== -1, 'truncated Go BuildID note');
        const value = payload.subarray(found + ID_START.length, end);
        require(value.length > 0 && value.every(byte => byte > 32 && byte < 127 && byte !== 34 && byte !== 92),
          'Go BuildID value');
        cursor = end + ID_END.length;
        row.buildIDs.push({ value: value.toString('ascii'), sectionOffset: found, fileOffset: row.offset + found,
          bytes: cursor - found, sha256: sha(payload.subarray(found, cursor)) });
        identities++;
      }
    }
    row.address = exactInteger(row.address);
    row.bytes = exactInteger(row.bytes);
  }
  require(infos.length <= 1 && identities <= 1 && Boolean(infos.length) === Boolean(identities), 'missing/extra Go identity');
  if (infos.length) observeBuildInfo(infos[0]);
  return { kind: 'Mach-O64', cpu, subCPU, filetype, sha256: sha(data), bytes: data.length,
    flags, platformCommands, loadCommands, sections };
}

function symbolTable(payload, members) {
  bounds(0, 8, payload.length, 'BSD symbol table');
  const tableBytes = payload.readUInt32LE(0);
  require(tableBytes % 8 === 0, 'BSD symbol table entry size');
  bounds(4, tableBytes + 4, payload.length, 'BSD symbol table entries');
  const stringsBytes = payload.readUInt32LE(4 + tableBytes), stringsOffset = 8 + tableBytes;
  bounds(stringsOffset, stringsBytes, payload.length, 'BSD symbol strings');
  const tail = payload.subarray(stringsOffset + stringsBytes);
  require(tail.length < 8 && allZero(tail), 'BSD symbol table padding');
  const codeOffsets = new Set(members.filter(row => row.classification === 'code').map(row => row.headerOffset));
  for (let offset = 4; offset < 4 + tableBytes; offset += 8) {
    const [nameOffset, memberOffset] = readWords(payload, offset, 2);
    require(nameOffset < stringsBytes && codeOffsets.has(memberOffset), 'BSD symbol/member binding');
    stringEnd(payload, stringsOffset + stringsBytes, stringsOffset + nameOffset, 'BSD symbol/member binding');
  }
}

export function archive(data, observeBuildInfo) {
  require(startsWith(data, AR), 'unsupported archive format');
  const members = [];
  let position = 8, symbols;
  while (position < data.length) {
    bounds(position, 60, data.length, 'ar header');
    const header = data.subarray(position, position + 60);
    require(header[58] === 96 && header[59] === 10, 'ar header trailer');
    require(printable(header.subarray(0, 58)), 'ar header ASCII');
    const rawName = header.toString('ascii', 0, 16).trimEnd();
    require(/^#1\/[1-9][0-9]*$/u.test(rawName), 'unsupported BSD ar name');
    for (const [start, end, digits] of [[16, 28, /^[0-9]+$/], [28, 34, /^[0-9]+$/],
      [34, 40, /^[0-9]+$/], [40, 48, /^[0-7]+$/], [48, 58, /^[0-9]+$/]]) {
      require(digits.test(header.toString('ascii', start, end).trim()), 'ar numeric field');
    }
    const length = Number(header.toString('ascii', 48, 58));
    const start = position + 60, nameLength = Number(rawName.slice(3));
    bounds(start, length, data.length, 'ar member');
    require(nameLength <= length, 'BSD ar name length');
    const rawMemberName = nameBytes(data.subarray(start, start + nameLength), 'BSD ar name/padding');
    const name = rawMemberName.toString('ascii');
    require(rawMemberName.length && printable(rawMemberName) && !/[\\/]/u.test(name)
      && name !== '.' && name !== '..', 'BSD ar name/padding');
    const payloadOffset = start + nameLength, end = start + length;
    const payload = data.subarray(payloadOffset, end);
    const row = { ordinal: members.length, headerOffset: position, rawName, name, nameBytes: nameLength,
      payloadOffset, bytes: payload.length, sha256: sha(payload) };
    if (name === '__.SYMDEF' || name === '__.SYMDEF SORTED') {
      require(!members.length && !startsWith(payload, MACHO), 'ar symbol table position/payload');
      row.classification = 'symbol-table';
      symbols = payload;
    } else {
      require(startsWith(payload, MACHO), 'unsupported code member format');
      row.classification = 'code';
      row.machO = macho(payload, observeBuildInfo);
      require(row.machO.filetype === 1, 'ar member must be MH_OBJECT');
    }
    if (length % 2) {
      bounds(end, 1, data.length, 'ar padding');
      require(data[end] === 10, 'ar padding byte');
    }
    members.push(row);
    position = end + length % 2;
  }
  require(members.some(row => row.classification === 'code'), 'empty code archive');
  if (symbols) symbolTable(symbols, members);
  return { kind: 'bsd-ar', sha256: sha(data), bytes: data.length, members };
}

export function container(data, observeBuildInfo) {
  if (startsWith(data, FAT)) {
    bounds(0, 8, data.length, 'fat header');
    const count = data.readUInt32BE(4);
    require(count === 1 || count === 2, 'unsupported fat slice count');
    const headerEnd = 8 + count * 20;
    bounds(8, count * 20, data.length, 'fat table');
    const slices = [], ranges = [];
    for (let ordinal = 0; ordinal < count; ordinal++) {
      const [cpu, subCPU, offset, bytes, alignment] = readWords(data, 8 + ordinal * 20, 5, true);
      require(alignment <= 31 && offset >= headerEnd && offset % (2 ** alignment) === 0 && bytes > 0,
        'fat slice alignment/header');
      bounds(offset, bytes, data.length, 'fat slice');
      ranges.push([offset, offset + bytes]);
      require(data.subarray(offset, offset + AR.length).equals(AR), 'unsupported fat payload; expected ar');
      slices.push({ ordinal, offset, bytes, cpu, subCPU, alignment });
    }
    disjoint(ranges, 'fat slice');
    require(new Set(slices.map(row => row.cpu + ':' + row.subCPU)).size === count, 'duplicate fat CPU/subCPU');
    let cursor = headerEnd;
    for (const [start, end] of ranges) {
      require(allZero(data.subarray(cursor, start)), 'fat padding');
      cursor = end;
    }
    require(cursor === data.length, 'fat trailing bytes');
    for (const row of slices) {
      row.archive = archive(data.subarray(row.offset, row.offset + row.bytes), observeBuildInfo);
      const code = row.archive.members.filter(member => member.classification === 'code');
      require(code.length && code.every(({ machO }) => machO.cpu === row.cpu && machO.subCPU === row.subCPU),
        'fat header/payload CPU differs');
    }
    return { kind: 'fat-ar', sha256: sha(data), bytes: data.length, slices };
  }
  if (startsWith(data, AR)) return archive(data, observeBuildInfo);
  if (startsWith(data, MACHO)) return macho(data, observeBuildInfo);
  require(false, 'unsupported container format');
}
