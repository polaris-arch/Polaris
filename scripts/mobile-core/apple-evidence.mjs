// Collect and retire originals for the hosted, unsigned Apple delivery.
import { createHash } from 'node:crypto';
import {
  appendFileSync, chmodSync, closeSync, constants, fstatSync, fsyncSync, lchmodSync, linkSync, lstatSync,
  mkdirSync, mkdtempSync, openSync, readdirSync, readlinkSync, readSync, realpathSync,
  rmSync, statSync, statfsSync, symlinkSync, utimesSync, writeFileSync, writeSync,
} from 'node:fs';
import { dirname, isAbsolute, join, relative, sep } from 'node:path';
import { Readable, Writable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { crc32, createInflateRaw } from 'node:zlib';
import { readZipEntries } from '../lib/zip-archive.mjs';
import { ROOT, canonical, decode, load, require, sha } from './fork-input.mjs';
import { filePin, readReceipt, receiptChunks } from './component-admission.mjs';
import { pinned, receiptPolicy } from './source-admission.mjs';
import { privateDir, writeJson } from './producer.mjs';
import { context, location, requiredSlots, scopeFields, transportPin, writeArchiveZip } from './apple-handoff.mjs';
import { CARGO_TIMING_BYTES, targetFor } from './apple-rust.mjs';

const CONTENT_BYTES = 4 * 1024 ** 3;
const ENTRIES = 16384;
const PATH_BYTES = 4096;
const INDEX_BYTES = receiptPolicy.receipt.controlBytes;
const ARTIFACT_BYTES = receiptPolicy.receipt.artifactBytes;
// Reserve the original worst-case unscoped ZIP size, including metadata and
// deflate expansion. Transported per-slot archives retain the 4 GiB cap.
const zipBytes = (content, names) => Math.floor(content * 9 / 8) + names * (2 * PATH_BYTES + 1024) + 1024;
const ZIP_BYTES = zipBytes(CONTENT_BYTES + INDEX_BYTES, ENTRIES + 1);
const RESERVE_BYTES = ZIP_BYTES + ARTIFACT_BYTES + 8 * INDEX_BYTES;
const INDEX = 'EVIDENCE-INDEX.json';
const CHUNK_BYTES = 1024 ** 2;
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const opened = path => openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
const statOptions = { bigint: true };
const stat = path => lstatSync(path, { ...statOptions, throwIfNoEntry: false });
const exists = path => stat(path) !== undefined;
const equal = (left, right) => canonical(left).equals(canonical(right));
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const regular = path => stat(path)?.isFile() === true;
const target = slot => targetFor(slot === 'device-arm64' ? 'iphoneos' : 'iphonesimulator', 'arm64', 'release');
const mode = info => Number(info.mode & 0o7777n);

function sameFile(before, after) {
  return after?.isFile() && ['dev', 'ino', 'mode', 'size', 'mtimeNs', 'ctimeNs']
    .every(name => before[name] === after[name]);
}

function beneath(root, path) {
  const member = relative(root, path);
  return member !== '' && !isAbsolute(member) && member !== '..' && !member.startsWith('..' + sep);
}

function memberName(root, path) {
  require(beneath(root, path), 'original Apple evidence escaped its fixed slots');
  const name = relative(root, path).split(sep).join('/');
  require(Buffer.byteLength(name) <= PATH_BYTES, 'original evidence ZIP path budget exceeded');
  return name;
}

function checkFixedPath(path) {
  for (let current = path; ; current = dirname(current)) {
    if (!exists(current)) continue;
    require(realpathSync(current) === current, 'linked archive path cannot be redirected');
    return;
  }
}

function readAt(fd, offset, length) {
  const bytes = Buffer.allocUnsafe(length);
  for (let received = 0; received < length;) {
    const count = readSync(fd, bytes, received, length - received, offset + received);
    require(count > 0, 'truncated original evidence bytes');
    received += count;
  }
  return bytes;
}

function* chunks(read, offset, length) {
  for (let received = 0; received < length;) {
    const size = Math.min(CHUNK_BYTES, length - received);
    yield read(offset + received, size);
    received += size;
  }
}

function copyPinnedEvidence(path, output, expected) {
  const fd = opened(path);
  try {
    const before = fstatSync(fd, statOptions);
    require(before.isFile() && before.size === BigInt(expected.bytes) && expected.bytes <= ARTIFACT_BYTES,
      'bounded regular original evidence required');
    const out = openSync(output, 'wx', mode(before));
    const digest = createHash('sha256');
    try {
      for (const raw of chunks((at, size) => readAt(fd, at, size), 0, expected.bytes)) {
        for (let written = 0; written < raw.length;) {
          const count = writeSync(out, raw, written, raw.length - written);
          require(count > 0, 'original evidence copy made no progress');
          written += count;
        }
        digest.update(raw);
      }
      fsyncSync(out);
    } finally { closeSync(out); }
    require(digest.digest('hex') === expected.sha256 && sameFile(before, fstatSync(fd, statOptions)) &&
      sameFile(before, stat(path)), 'original evidence changed during preservation');
    chmodSync(output, mode(before));
    utimesSync(output, before.atime, before.mtime);
  } finally { closeSync(fd); }
}

function preserveLinkedArchive(root, slot, receiptPath) {
  const original = join(ROOT, 'src-tauri/gen/apple/Externals/arm64/release/libapp.a');
  checkFixedPath(original);
  const work = privateDir(join(root, slot)), observation = join(work, 'linked-rust-preservation.json');
  if (!exists(original)) {
    writeJson(observation, { slot, originalPath: original, originalAbsent: true,
      preservationComplete: true, acceptedByReceipt: false, receiptClaimedArchive: receiptPath !== undefined,
      scope: 'fixed safe staging path absent; no accepted archive claimed' });
    require(receiptPath === undefined, 'claimed original linked Rust archive missing');
    return;
  }
  // Freeze the actual bytes before interpreting any receipt about them.
  const actual = filePin(original, { allowEmpty: true });
  const copy = join(work, 'original-linked-rust-archive.a');
  copyPinnedEvidence(original, copy, actual);
  const preservedPin = filePin(copy, { allowEmpty: true });
  require(equal(filePin(original, { allowEmpty: true }), actual) && equal(preservedPin, actual),
    'linked Rust archive changed during preservation');
  let expected = null, accepted = false, reason = 'accepted Rust receipt unavailable';
  if (receiptPath !== undefined) {
    try {
      const receipt = readReceipt(receiptPath);
      expected = receipt.archive ?? null;
      accepted = receipt.target === target(slot) && receipt.linkedArchive === original && equal(expected, actual);
      reason = 'original linked Rust archive differs before preservation';
    } catch { reason = 'accepted Rust receipt unreadable'; }
  }
  writeJson(observation, { schema: 'polaris-apple-linked-rust-preservation-v1', slot, originalPath: original,
    preservedPath: copy, originalPin: actual, preservedPin, expectedPin: expected, preservationComplete: true,
    acceptedByReceipt: accepted, scope: 'actual staging bytes retained, accepted or unaccepted; not a new producer receipt' });
  require(accepted, reason);
  return copy;
}

async function readMember(entry, read, name, maximum, retain = false) {
  require(entry && !(entry.flags & 1) && [0, 8].includes(entry.method) &&
    entry.uncompressedSize <= maximum && (entry.method !== 0 || entry.compressedSize === entry.uncompressedSize),
  'missing or unsupported original evidence ZIP member: ' + name);
  const digest = createHash('sha256'), retained = retain ? [] : null;
  let bytes = 0, crc = 0;
  const source = Readable.from(chunks(read, entry.dataOffset, entry.compressedSize));
  const sink = new Writable({ write(chunk, _encoding, done) {
    bytes += chunk.length;
    if (bytes > entry.uncompressedSize) return done(new Error('preserved evidence exceeds expected bytes: ' + name));
    crc = crc32(chunk, crc);
    digest.update(chunk);
    if (retained) retained.push(Buffer.from(chunk));
    done();
  } });
  if (entry.method === 8) {
    const inflate = createInflateRaw();
    await pipeline(source, inflate, sink);
    require(inflate.bytesWritten === entry.compressedSize, 'original evidence deflate bounds differ: ' + name);
  } else await pipeline(source, sink);
  require(bytes === entry.uncompressedSize && crc === entry.crc32, 'ZIP member size or CRC differs: ' + name);
  return { pin: { bytes, sha256: digest.digest('hex') }, ...(retained ? { bytes: Buffer.concat(retained, bytes) } : {}) };
}

// Hold the same descriptor for the transport hash, central directory and all
// requested ZIP member reads. Never accumulate compressed/native leaf bytes.
async function withEvidenceZip(path, maximum, expected, verify) {
  const fd = opened(path);
  try {
    const before = fstatSync(fd, statOptions);
    require(before.isFile() && sameFile(before, stat(path)) && before.size > 0n && before.size <= BigInt(maximum),
      'bounded regular original evidence ZIP required');
    const length = Number(before.size), read = (offset, bytes) => readAt(fd, offset, bytes);
    const digest = createHash('sha256');
    for (const raw of chunks(read, 0, length)) digest.update(raw);
    const pin = { bytes: length, sha256: digest.digest('hex') };
    require(expected === undefined || equal(pin, expected), 'slot originals are incomplete or changed; intermediates retained');
    const members = readZipEntries(length, read, { zip64: true, maxEntries: ENTRIES + 1, strictMetadata: true,
      decodeName: raw => utf8.decode(raw) });
    const index = decode((await readMember(members.get(INDEX), read, INDEX, INDEX_BYTES, true)).bytes, INDEX_BYTES);
    require(index?.schema === 'polaris-apple-original-evidence-v2' && object(index.files) &&
      members.size === Object.keys(index.files).length + 1 && Object.keys(index.files).every(name => name !== INDEX && members.has(name)),
    'original evidence archive index or membership differs');
    await verify(index, members, read);
    require(sameFile(before, fstatSync(fd, statOptions)) && sameFile(before, stat(path)),
      'original evidence ZIP changed during readback');
    return pin;
  } finally { closeSync(fd); }
}

async function verifyArchiveRows(index, members, read, modes) {
  let total = 0;
  for (const [name, row] of Object.entries(index.files)) {
    const member = members.get(name);
    const link = typeof row.link === 'string';
    const expected = link ? { bytes: Buffer.byteLength(row.link), sha256: sha(Buffer.from(row.link)) } :
      { bytes: row.bytes, sha256: row.sha256 };
    require(member && member.uncompressedSize === expected.bytes && (member.mode & 0o170000) === (link ? 0o120000 : 0o100000) &&
      (modes === undefined || (member.mode & 0o7777) === modes.get(name)), 'preserved original evidence member metadata differs: ' + name);
    require(Buffer.byteLength(name) <= PATH_BYTES && (total += expected.bytes) <= CONTENT_BYTES,
      'preserved original evidence exceeds its path or 4GiB budget');
    require(equal((await readMember(member, read, name, link ? PATH_BYTES : ARTIFACT_BYTES)).pin, expected),
      'preserved original evidence bytes differ: ' + name);
  }
}

function children(path, select, visit) {
  if (!exists(path)) return;
  for (const info of readdirSync(path, { withFileTypes: true })) {
    const entry = join(path, info.name);
    if (select(info)) visit(entry);
  }
}

function descendants(path, select, visit) {
  children(path, () => true, entry => {
    const info = stat(entry);
    if (info && select(entry, info)) visit(entry);
    if (info?.isDirectory()) descendants(entry, select, visit);
  });
}

const endsWith = (...suffixes) => entry => suffixes.some(suffix => entry.name.endsWith(suffix));
const frameworkPaths = root => [
  join(root, 'component/Libbox.xcframework'), join(root, 'component/staging/Libbox.xcframework'),
];

function gatherOriginals(root, scopedSlot) {
  const paths = new Set(), errors = Object.create(null), timings = [];
  const add = path => {
    paths.add(path);
    require(paths.size <= ENTRIES, 'selected original evidence entry budget exceeded');
  };
  const addRegular = path => { if (regular(path)) add(path); };
  const describe = (receipt, error) => { errors[memberName(root, receipt)] = error.message; };
  const reports = endsWith('.json', '.log', '.raw');
  if (!scopedSlot) children(root, reports, add);
  else for (const name of [
    'tools.json', 'component-handoff.json', 'component-transport.json', 'import-observation.json',
    'runner-tool-admission.json', 'original-' + scopedSlot + '.log',
    'original-' + scopedSlot + '.log.stdout.raw', 'original-' + scopedSlot + '.log.stderr.raw',
  ]) addRegular(join(root, name));
  const component = join(root, 'component');
  children(component, reports, add);
  if (!scopedSlot) {
    for (const framework of frameworkPaths(root)) {
      descendants(framework, (_entry, info) => info.isFile() || info.isSymbolicLink(), add);
    }
  }
  const receipt = join(component, 'component-receipt.json');
  let origins = null;
  if (exists(receipt)) {
    try {
      const facts = readReceipt(receipt).nativeInputs;
      require(object(facts) && object(facts.files), 'original native input inventory differs');
      origins = facts;
    } catch (error) { describe(receipt, error); }
  }
  if (origins && !scopedSlot) {
    for (const path of Object.keys(origins.files)) {
      if (exists(path)) add(path);
      else errors['native-input:' + Object.keys(errors).length] = 'original input missing in failed partial work';
    }
    if (origins.generatorOutputRoot !== undefined) {
      const generated = origins.generatorOutputRoot;
      if (exists(generated)) {
        require(beneath(root, realpathSync(generated)), 'original Apple evidence escaped its fixed slots');
        descendants(generated, (path, info) =>
          info.isFile() || info.isSymbolicLink() && statSync(path, { throwIfNoEntry: false })?.isFile(), add);
      }
    }
  }
  for (const slot of requiredSlots()) {
    if (scopedSlot && slot !== scopedSlot) continue;
    const work = join(root, slot), selected = target(slot);
    children(work, entry => endsWith('.json', '.log', '.raw', '.ipa')(entry) ||
      entry.name.startsWith('wire-') && entry.name.endsWith('.a'), add);
    for (const name of ['PolarisTLSFingerprintLinker.args', 'PolarisTLSFingerprintLinker.json']) {
      addRegular(join(work, 'tls-link-input', name));
    }
    descendants(join(work, 'derived-data'), path => /LinkMap.*\.txt$/.test(path), add);
    timings.push(join(work, 'diagnostic-cargo-timing-' + selected + '.html'));
    const nativeArchive = target => join(root, 'cache/rust-target', target, 'release/libpolaris_lib.a');
    addRegular(nativeArchive(selected));
    children(work, entry => entry.name.startsWith('rust-') && entry.name.endsWith('.json'), path => {
      try {
        const { target } = readReceipt(path);
        require(['aarch64-apple-ios', 'aarch64-apple-ios-sim'].includes(target), 'original Rust target differs');
        const native = nativeArchive(target);
        if (regular(native)) add(native);
        else errors[memberName(root, path) + ':archive'] = 'original Rust archive missing';
      } catch (error) { describe(path, error); }
    });
  }
  return { paths, errors, origins, timings };
}

function stageOriginals(root, view, paths) {
  const rows = Object.create(null), modes = new Map();
  const frameworks = frameworkPaths(root);
  let total = 0;
  for (const path of [...paths].sort()) {
    require(beneath(root, realpathSync(path)), 'original Apple evidence escaped its fixed slots');
    const name = memberName(root, path), info = stat(path);
    let row;
    if (info?.isSymbolicLink()) {
      require(frameworks.some(framework => beneath(framework, path)),
        'only actual component/staged framework links may be retained');
      row = { originalPath: path, link: readlinkSync(path) };
      total += Buffer.byteLength(row.link);
    } else {
      row = { originalPath: path, ...filePin(path, { allowEmpty: true }) };
      total += row.bytes;
    }
    require(total <= CONTENT_BYTES, 'selected original evidence exceeds 4GiB budget');
    const staged = join(view, name);
    mkdirSync(dirname(staged), { recursive: true, mode: 0o700 });
    // Darwin rejects hard links to framework aliases. Keep regular originals
    // on the same inode and recreate only the symbolic links in the view.
    if (row.link === undefined) linkSync(path, staged);
    else {
      symlinkSync(row.link, staged);
      if (stat(staged).mode !== info.mode) lchmodSync(staged, mode(info));
    }
    const observed = stat(staged), original = row.link === undefined ? observed : stat(path);
    require(original && original.ino === info.ino && original.dev === info.dev && original.mode === info.mode &&
      observed?.mode === info.mode && (row.link === undefined || readlinkSync(staged) === row.link),
    'original archive view differs: ' + name);
    rows[name] = row;
    modes.set(name, mode(info));
  }
  return { rows, modes, total };
}

function indexSize(index) {
  let size = 0;
  for (const raw of receiptChunks(index, { pretty: true })) size += raw.length;
  require(size <= INDEX_BYTES, 'original evidence index exceeds bound');
  return size;
}

function addDiagnostics(root, view, state, index, timings, maximum) {
  for (const path of timings) {
    let diagnostic;
    try {
      const name = memberName(root, path), size = Object.keys(index.files).length, info = stat(path);
      require(size < ENTRIES && info?.isFile() && realpathSync(path) === path, 'no safe diagnostic entry budget');
      const { bytes, pin } = pinned(path, CARGO_TIMING_BYTES);
      require(sameFile(info, stat(path)) && state.total + bytes.length <= CONTENT_BYTES, 'no stable diagnostic content budget');
      const row = { originalPath: path, ...pin, diagnosticOnly: true };
      const proposed = { ...index, files: { ...index.files, [name]: row } };
      require(zipBytes(state.total + bytes.length + indexSize(proposed), size + 2) <= maximum, 'no diagnostic index or ZIP budget');
      diagnostic = { name, bytes, row, mode: mode(info) };
    } catch { console.log('Cargo timing diagnostic omitted; mandatory originals unchanged'); }
    if (!diagnostic) continue;
    const { name, bytes, row, mode: permissions } = diagnostic, staged = join(view, name);
    mkdirSync(dirname(staged), { recursive: true, mode: 0o700 });
    writeFileSync(staged, bytes, { flag: 'wx', mode: permissions });
    chmodSync(staged, permissions);
    index.files[name] = row;
    state.modes.set(name, permissions);
    state.total += bytes.length;
  }
}

export async function evidence({ slot, publishEnv = false }, runner) {
  const root = location();
  require(slot === undefined || requiredSlots().includes(slot), 'unrequested Apple evidence slot');
  const scopedSlot = slot ?? process.env.POLARIS_APPLE_CARRIER_SLOT;
  require(scopedSlot === undefined || requiredSlots().includes(scopedSlot), 'unrequested Apple evidence slot');
  const output = join(root, slot ? 'apple-original-' + slot + '-evidence.zip' : 'apple-original-evidence.zip');
  if (publishEnv) appendFileSync(process.env.GITHUB_ENV, 'POLARIS_FORMAL_APPLE_ROOT=' + root + '\n', { flag: 'a' });
  const work = privateDir(join(root, 'evidence-work')), cache = privateDir(join(root, 'cache'));
  return runner.withLease(work, cache, load(), async () => {
    require(!exists(output), 'previous original evidence must not be overwritten');
    const marker = slot ? join(root, 'staging-preservation-' + slot + '.json') : undefined;
    const originalContext = marker ? await context(runner) : undefined;
    if (marker) writeJson(marker, { slot, context: originalContext, complete: false });
    const importObservation = join(root, 'import-observation.json');
    const imported = regular(importObservation) && readReceipt(importObservation).completed === true;
    const { paths, errors, origins, timings } = gatherOriginals(root, scopedSlot);
    let stagingSafe = false;
    if (slot) {
      const carrier = join(root, slot), accepted = join(carrier, 'rust-' + target(slot) + '.json');
      try {
        const preserved = preserveLinkedArchive(root, slot, regular(accepted) ? accepted : undefined);
        if (preserved) paths.add(preserved);
      } catch (error) { errors[slot + ':linked-preservation'] = error.message; }
      for (const name of ['original-linked-rust-archive.a', 'linked-rust-preservation.json']) {
        const path = join(carrier, name);
        if (regular(path)) paths.add(path);
      }
      const observation = join(carrier, 'linked-rust-preservation.json');
      stagingSafe = regular(observation) && readReceipt(observation).preservationComplete === true;
    }
    require(paths.size <= ENTRIES, 'selected original evidence entry budget exceeded');
    const view = mkdtempSync(join(work, 'archive-view-'));
    try {
      const state = stageOriginals(root, view, paths);
      const index = {
        schema: 'polaris-apple-original-evidence-v2', ...scopeFields(), files: state.rows,
        completeUnsignedChain: regular(join(root, 'apple-chain.json')) && Object.keys(errors).length === 0,
        componentAccepted: origins !== null && Object.keys(errors).length === 0 && (!scopedSlot || imported),
        componentOriginalsInThisArchive: !scopedSlot, componentOriginalsReferencedByTransport: Boolean(scopedSlot),
        slot: scopedSlot ?? null,
        sameRunComponentArtifact: 'apple-component-original-' + (process.env.GITHUB_RUN_ID ?? '') +
          '-' + (process.env.GITHUB_RUN_ATTEMPT ?? ''),
        receiptReadErrors: errors,
        receiptBytes: Object.fromEntries(Object.entries(state.rows).filter(([name, row]) =>
          name.endsWith('.json') && Object.hasOwn(row, 'bytes')).map(([name, row]) => [name, row.bytes])),
        stagedComponentIsAccepted: false, devicePassed: false, publicationEligible: false,
      };
      indexSize(index);
      const maximum = slot ? CONTENT_BYTES : ZIP_BYTES;
      addDiagnostics(root, view, state, index, timings, maximum);
      writeJson(join(view, INDEX), index);
      await writeArchiveZip(view, output, [...Object.keys(index.files), INDEX], runner, { maxBytes: maximum });
      const pin = await withEvidenceZip(output, maximum, undefined, async (readback, members, read) => {
        require(equal(readback, index), 'original evidence index changed during ZIP write');
        await verifyArchiveRows(readback, members, read, state.modes);
      });
      if (slot) {
        require(equal(pin, transportPin(output)) && equal(originalContext, await context(runner)),
          'original Apple evidence or hosted context changed');
        writeJson(join(root, 'evidence-' + slot + '-pin.json'), {
          slot, context: originalContext, archive: pin, receiptReadErrors: errors, devicePassed: false,
        });
        writeJson(marker, { slot, context: originalContext, complete: stagingSafe, archive: pin });
      }
    } finally { rmSync(view, { recursive: true }); }
    console.log('Original selected bytes/paths preserved; unsigned chain is not device or signing acceptance');
  }, { reserveBytes: RESERVE_BYTES });
}

export async function retireSlot(slot, runner) {
  require(requiredSlots().includes(slot), 'unrequested Apple retirement slot');
  const root = location(), archive = join(root, 'apple-original-' + slot + '-evidence.zip');
  const work = privateDir(join(root, 'retirement-' + slot)), cache = privateDir(join(root, 'cache'));
  return runner.withSlotLocks(work, cache, async () => {
    const expectedContext = await context(runner);
    const scope = scopeFields();
    const pin = readReceipt(join(root, 'evidence-' + slot + '-pin.json'));
    require(pin.slot === slot && equal(pin.context, expectedContext) && object(pin.archive) &&
      object(pin.receiptReadErrors) && Object.keys(pin.receiptReadErrors).length === 0,
    'slot originals are incomplete or changed; intermediates retained');
    const targetDir = join(root, 'cache/rust-target', target(slot));
    const native = join(targetDir, 'release/libpolaris_lib.a'), nativeName = memberName(root, native);
    const expectedNative = exists(native) ? filePin(native) : undefined;
    await withEvidenceZip(archive, CONTENT_BYTES, pin.archive, async (index, members, read) => {
      require(index.slot === slot && index.carrierScope === scope.carrierScope &&
        equal(index.requiredSlots, scope.requiredSlots) && object(index.receiptReadErrors) &&
        Object.keys(index.receiptReadErrors).length === 0,
        'slot archive scope differs');
      if (expectedNative) {
        const row = index.files[nativeName];
        require(row && equal({ bytes: row.bytes, sha256: row.sha256 }, expectedNative),
          'native archive missing from preserved originals');
      }
      await verifyArchiveRows(index, members, read);
    });
    require(equal(expectedContext, await context(runner)), 'Apple retirement context changed during readback');
    require(expectedNative === undefined ? !exists(native) : equal(filePin(native), expectedNative),
      'native archive changed during retirement readback');
    const before = statfsSync(root), directories = [targetDir, join(root, slot, 'derived-data')].filter(exists);
    for (const path of directories) {
      const info = stat(path);
      require(info?.isDirectory() && realpathSync(path) === path && info.uid === BigInt(process.getuid()),
        'retirement directory identity differs');
    }
    for (const path of directories) rmSync(path, { recursive: true });
    writeJson(join(root, 'retirement-' + slot + '.json'), {
      slot, context: expectedContext, preservedArchive: pin.archive, removedRebuildableDirectories: directories,
      filesystem: Number(stat(root).dev), freeBytesBefore: before.bavail * before.bsize,
      freeBytesAfter: statfsSync(root).bavail * before.bsize, freeSpaceDeltaIsNotExclusiveReleaseMeasurement: true,
      scope: 'uploaded and byte-verified originals retained; native descendants excluded by cache lock',
    });
  });
}
