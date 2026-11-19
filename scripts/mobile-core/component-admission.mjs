import { createHash } from 'node:crypto';
import { lstatSync, mkdtempSync, readdirSync, readlinkSync, realpathSync, rmSync,
  statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

import { openIndexedZip, readZipEntry, zipName } from '../lib/zip-archive.mjs';
import { aarElfFacts, aarInterfaceClasses, verifyAarInterfaces } from '../verify-apk.mjs';
import { sdkFrameworkLibraries, sdkHeaderContracts } from './apple-admission.mjs';
import { aarGoInvocationIdentity, canonical, canonicalChunks, checkMobileBuildInfo,
  codepointOrder, decode, parseMobileBuildInfo, pinned, pinnedDigest, receiptLimit, receiptPolicy,
  validateMobileSource } from './source-admission.mjs';

export { canonical, canonicalChunks, receiptLimit, receiptPolicy };
export const RECEIPT_POLICY = receiptPolicy;
export const ROOT = resolve(fileURLToPath(new URL('../..', import.meta.url)));
export const MAX_ARTIFACT = receiptPolicy.receipt.artifactBytes;
export const MAX_RECEIPT = receiptPolicy.receipt.controlBytes;
const ABI = new Map([['arm64-v8a', 'arm64'], ['armeabi-v7a', 'arm'],
  ['x86', '386'], ['x86_64', 'amd64']]);
export const APPLE = {
  'ios-arm64': [0x100000c, 2, 'arm64', null],
  'ios-arm64-simulator': [0x100000c, 7, 'arm64', 'simulator'],
};
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const sha = raw => createHash('sha256').update(raw).digest('hex');
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const digestPattern = /^[0-9a-f]{64}$/;
const maxEntries = 65536;

export const equal = (left, right) => isDeepStrictEqual(left, right, { skipPrototype: true });

export function demand(condition, message) {
  if (!condition) throw new Error(message);
}

export function within(root, path) {
  const remainder = relative(root, path);
  return !isAbsolute(remainder) && remainder !== '..' && !remainder.startsWith('..' + sep);
}

export function receiptChunks(value, { pretty = false } = {}) {
  return canonicalChunks(value, { maximum: receiptLimit(value), pretty });
}

function lengthOf(chunks) {
  let bytes = 0;
  for (const chunk of chunks) bytes += chunk.length;
  return bytes;
}

export function receiptFieldBytes(value) {
  const maximum = receiptLimit(value);
  return Object.fromEntries(Object.entries(value).map(([key, field]) =>
    [key, lengthOf(canonicalChunks(field, { maximum }))]));
}

export function receiptDigest(value) {
  const hash = createHash('sha256');
  for (const chunk of receiptChunks(value)) hash.update(chunk);
  return hash.digest('hex');
}

export function readReceipt(path) {
  const maximum = receiptPolicy.receipt.appleComponentBytes;
  return decode(pinned(path, maximum, true).bytes, maximum, receiptLimit);
}

export function readPinned(path, { allowEmpty = false, observations } = {}) {
  const result = pinned(path, MAX_ARTIFACT, false, { allowEmpty });
  if (observations) {
    observations.fileReads = (observations.fileReads ?? 0) + 1;
    observations.readBytes = (observations.readBytes ?? 0) + result.pin.bytes;
  }
  return result;
}

export function filePin(path, options) {
  const pin = pinnedDigest(path, MAX_ARTIFACT, options);
  const observations = options?.observations;
  if (observations) {
    observations.fileReads = (observations.fileReads ?? 0) + 1;
    observations.readBytes = (observations.readBytes ?? 0) + pin.bytes;
  }
  return pin;
}

export function failureNote(error, message) {
  if (!object(error)) return;
  try {
    if (Array.isArray(error.notes)) error.notes.push(message);
    else if (!Object.hasOwn(error, 'notes')) error.notes = [message];
  } catch {
    // Keep the primary failure when diagnostics are unwritable.
  }
}

export async function inspectionStage(stage, reads, progress, inspect) {
  const started = performance.now();
  let primary;
  try {
    return await inspect();
  } catch (error) {
    primary = error;
    throw error;
  } finally {
    try {
      await progress?.(stage, (performance.now() - started) / 1000,
        reads?.fileReads ?? (reads ? 0 : undefined), reads?.readBytes ?? (reads ? 0 : undefined));
    } catch (error) {
      if (!primary) throw error;
      failureNote(primary, 'Apple inspection observation failed; original failure preserved');
    }
  }
}

export function treeInventory(root, { pinned: observedPins, observations, metadataOnly = false } = {}) {
  demand(lstatSync(root).isDirectory(), 'component root redirected/missing');
  root = realpathSync(root);
  const rows = [];
  function visit(current) {
    for (const name of readdirSync(current)) {
      const path = join(current, name);
      const info = lstatSync(path);
      const row = { path: relative(root, path).split(sep).join('/'), mode: info.mode & 0o777 };
      if (info.isSymbolicLink()) {
        const target = readlinkSync(path);
        demand(!isAbsolute(target) && within(root, realpathSync(path)),
          'framework link must remain inside the same component');
        Object.assign(row, { type: 'symlink', target });
      } else if (info.isFile()) {
        row.type = 'file';
        if (!metadataOnly) Object.assign(row, observedPins?.get(realpathSync(path)) ??
          filePin(path, { allowEmpty: true, observations }));
      } else {
        demand(info.isDirectory(), 'unsupported component object');
        row.type = 'directory';
        visit(path);
      }
      rows.push(row);
    }
  }
  visit(root);
  demand(rows.length > 0, 'empty component tree');
  return rows.sort((left, right) => codepointOrder(left.path, right.path));
}

export function localReplacementAliases(replacement, sourceDir, observed, name) {
  const { Path: literal, Dir: directory } = replacement;
  const diagnostic = 'native replacement differs: ' + name + '; observed=' + JSON.stringify(observed) +
    '; declaredPath=' + JSON.stringify(literal) + '; declaredDir=' + JSON.stringify(directory);
  demand(typeof literal === 'string' && typeof directory === 'string' && typeof sourceDir === 'string' &&
    literal.startsWith('./') && !/[\\\0]/.test(literal), diagnostic + '; exact source-owned relative replacement required');
  const parts = literal.slice(2).split('/');
  demand(parts.every(part => part && part !== '.' && part !== '..'),
    diagnostic + '; relative replacement escapes or is not canonical');
  demand(isAbsolute(sourceDir) && isAbsolute(directory) &&
    !sourceDir.split(sep).includes('..') && !directory.split(sep).includes('..'),
  diagnostic + '; absolute source graph roots required');
  demand(resolve(directory) === join(sourceDir, ...parts), diagnostic + '; source graph Path/Dir disagree');
  const root = realpathSync(sourceDir), child = realpathSync(directory);
  demand(statSync(child).isDirectory() && within(root, child), diagnostic + '; replacement escaped verified source root');
  return [new Set([literal, directory]), diagnostic];
}

export function verifyNativeBuildInfo({ raw, source, graph, target, expectedId, actualId, original }) {
  const evidence = { raw, source, graph, target, expectedId, actualId };
  if (original !== undefined) evidence.originalAarGoBuildInfo = original.goBuildInfo;
  lengthOf(canonicalChunks(evidence, { maximum: MAX_RECEIPT }));
  const facts = checkMobileBuildInfo(evidence);
  if (original !== undefined) {
    const abi = source.android.abis[source.android.targets.indexOf('android/' + target.arch)];
    const filename = 'jni_' + abi + '_libbox.so';
    demand(aarGoInvocationIdentity(raw, facts.goVersion, filename) ===
      aarGoInvocationIdentity(original.goBuildInfo, facts.goVersion, filename),
    'original AAR Go BuildInfo body differs');
  }
  return facts;
}

export class AppleBuildInfoBatch {
  #observations = new Map();
  #bytes = 0;

  observe = info => {
    const raw = info.parserInput;
    this.#bytes += Buffer.byteLength(raw);
    demand(this.#bytes <= MAX_RECEIPT, 'Apple BuildInfo batch exceeds control byte budget');
    this.#observations.set(info, { raw });
  };

  bind(info, { arch, simulator, expectedId, actualId }) {
    const row = this.#observations.get(info);
    demand(row, 'bound Apple BuildInfo was not observed');
    Object.assign(row, { target: { osName: 'ios', arch, mode: 'c-archive', simulator }, expectedId, actualId });
  }

  async verify({ source, graph } = {}) {
    if (this.#observations.size === 0) return;
    const evidence = { appleObservations: [...this.#observations.values()] };
    if (source !== undefined) Object.assign(evidence, { source, graph });
    lengthOf(canonicalChunks(evidence, { maximum: MAX_RECEIPT }));
    for (const [info, row] of this.#observations) {
      const facts = row.target ? checkMobileBuildInfo({ ...row, source, graph }) : parseMobileBuildInfo(row.raw);
      info.parsedBuildInfo = facts;
      info.buildInfoFingerprint = sha(canonical(facts, { ascii: true }));
    }
  }
}

export function openComponentZip(path) {
  const { entries, read, close } = openIndexedZip(path, {
    maximum: MAX_ARTIFACT, subject: 'mobile component', decodeName: zipName,
  });
  try {
    demand(entries.size > 0, 'duplicate/empty/large ZIP inventory');
    let expanded = 0;
    for (const [name, row] of entries) {
      demand(!name.startsWith('/') && !/[\\\0]/.test(name) && !name.split('/').includes('..') &&
        !(row.flags & 1) && (row.mode & 0o170000) !== 0o120000, 'unsafe ZIP member');
      expanded += row.uncompressedSize;
      demand(expanded <= 2 * MAX_ARTIFACT, 'expanded ZIP too large');
    }
    return { entries, close, member: name => readZipEntry(entries.get(name), read, { name, maximum: MAX_ARTIFACT }) };
  } catch (error) {
    close();
    throw error;
  }
}

export function withComponentZip(path, inspect) {
  const archive = openComponentZip(path);
  try {
    return inspect(archive);
  } finally {
    archive.close();
  }
}

export async function withInspectionDirectory(scratch, prefix, action) {
  const work = mkdtempSync(join(scratch ?? tmpdir(), prefix));
  try { return await action(work); }
  finally { rmSync(work, { recursive: true, force: true }); }
}

export async function inspectAar({ aar, source, graph, tools, run, scratch, expectedId, originalOutput }) {
  validateMobileSource(source);
  const artifact = filePin(aar);
  return withInspectionDirectory(scratch, 'inspect-aar-', async work => {
    const classes = join(work, 'classes.jar');
    const { names, libraries, manifestSha256 } = withComponentZip(aar, archive => {
      const names = [...archive.entries.keys()];
      const libs = names.filter(name => /^jni\/[^/]+\/[^/]+\.so$/.test(name)).sort(codepointOrder);
      const abis = new Set(libs.map(name => name.split('/')[1]));
      demand(names.filter(name => name.endsWith('.so')).length === libs.length, 'unbound AAR native library location');
      demand(abis.size === ABI.size && [...ABI.keys()].every(abi => abis.has(abi) &&
        archive.entries.has('jni/' + abi + '/libbox.so')), 'formal AAR must contain all four ABIs');
      demand(archive.entries.has('classes.jar') && archive.entries.has('AndroidManifest.xml'), 'AAR binding/manifest missing');
      const manifest = archive.member('AndroidManifest.xml');
      demand(/minSdkVersion\s*=\s*["']24["']/.test(utf8.decode(manifest)), 'AAR minimumAPI must be24');
      const libraries = libs.map(name => {
        const abi = name.split('/')[1], libbox = name.endsWith('/libbox.so');
        const bytes = archive.member(name), path = join(work, name.replaceAll('/', '_'));
        const elf = aarElfFacts(bytes, abi, libbox);
        writeFileSync(path, bytes, { flag: 'wx' });
        return { name, abi, libbox, path, elf, pin: { bytes: bytes.length, sha256: sha(bytes) } };
      });
      writeFileSync(classes, archive.member('classes.jar'), { flag: 'wx' });
      return { names: new Set(names), libraries, manifestSha256: sha(manifest) };
    });
    const nativeLibraries = Object.create(null), jni = Object.create(null);
    const system = new Set(['liblog.so', 'libdl.so', 'libm.so', 'libandroid.so', 'libc.so']);
    const decodedInteger = value => value <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(value) : value;
    for (const { name, abi, libbox, path, elf: { ptLoad, androidAPI }, pin } of libraries) {
      const dynamic = await run([tools.readelf, '-d', path]);
      const needed = [...dynamic.matchAll(/Shared library: \[([^\]]+)\]/g)].map(match => match[1]);
      demand(needed.every(library => system.has(library) || names.has('jni/' + abi + '/' + library)),
        'unpackaged Android runtime dependency');
      // Match the historical JSON parser's integer types without rounding a 64-bit ELF field.
      const row = { ...pin, ptLoad: ptLoad.map(({ virtualAddress, alignment, ...segment }) => ({
        ...segment, virtualAddress: decodedInteger(virtualAddress), alignment: decodedInteger(alignment),
      })), needed };
      if (libbox) {
        const raw = await run([tools.go, 'version', '-m', path]);
        const actualId = (await run([tools.go, 'tool', 'buildid', path])).trim();
        const original = originalOutput?.nativeLibraries[name];
        demand(originalOutput === undefined || original, 'original AAR library observation missing');
        const parsedBuildInfo = verifyNativeBuildInfo({ raw, source, graph, expectedId, actualId, original,
          target: { osName: 'android', arch: ABI.get(abi), mode: 'c-shared', simulator: false } });
        jni[abi] = await run([tools.readelf, '--dyn-syms', '--wide', path]);
        Object.assign(row, { goBuildInfo: original?.goBuildInfo ?? raw, parsedBuildInfo, buildID: actualId, androidAPI });
      }
      demand(equal(filePin(path), pin), 'staged native library changed during tool inspection');
      nativeLibraries[name] = row;
    }
    const plan = aarInterfaceClasses(), observations = { jni };
    for (const [field, option] of [['hotJavaInterfaces', '-s'], ['javaInterfaces', '-constants']]) {
      observations[field] = {};
      for (const name of plan[field]) {
        observations[field][name] = await run([tools.javap, option, '-classpath', classes, 'io.nekohasekai.libbox.' + name]);
      }
    }
    lengthOf(canonicalChunks(observations, { maximum: MAX_RECEIPT }));
    const exports = verifyAarInterfaces(observations);
    for (const [abi, symbols] of Object.entries(exports)) {
      nativeLibraries['jni/' + abi + '/libbox.so'].jniExports = symbols;
    }
    demand(equal(artifact, filePin(aar)), 'original AAR changed during native inspection');
    return { format: 'AAR', artifact, nativeLibraries, classes: filePin(classes),
      javaInterfaces: observations.javaInterfaces, hotJavaInterfaces: observations.hotJavaInterfaces,
      manifestSha256, minimumAPI: 24 };
  });
}

export async function parsePlist(bytes, { run, scratch }) {
  return withInspectionDirectory(scratch, 'plist-', async work => {
    const source = join(work, 'original.plist');
    writeFileSync(source, bytes, { flag: 'wx' });
    return JSON.parse(await run(['/usr/bin/plutil', '-convert', 'json', '-o', '-', '--', source]));
  });
}

export async function inspectFramework({ framework, source, graph, expectedId, run, scratch, inspectionProgress }) {
  validateMobileSource(source);
  const reads = {};
  return inspectionStage('framework-inspection', reads, inspectionProgress, async () => {
    const { container, FAT } = await import('../apple-carrier.mjs');
    const root = realpathSync(framework), layout = treeInventory(framework, { metadataOnly: true });
    const pins = new Map(), buildInfo = new AppleBuildInfoBatch(), slices = {};
    const read = path => {
      const physical = realpathSync(path);
      demand(within(root, physical), 'static binary or header escaped framework');
      const { bytes, pin } = readPinned(path, { observations: reads });
      pins.set(physical, pin);
      return bytes;
    };
    const info = await parsePlist(read(join(framework, 'Info.plist')), { run, scratch });
    const declared = sdkFrameworkLibraries(info.AvailableLibraries);
    for (const { identifier, path, target, architectures } of declared) {
      const [cpu, platform, arch, simulator] = APPLE[target];
      const library = join(framework, identifier, path);
      demand(statSync(library).isDirectory() && library.endsWith('.framework'), 'expected a framework component');
      await parsePlist(read(join(library, 'Resources/Info.plist')), { run, scratch });
      const binary = realpathSync(join(library, 'Libbox'));
      const raw = read(binary);
      let parsed;
      try { parsed = container(raw, buildInfo.observe); }
      catch (error) {
        const count = raw.length >= 8 && raw.subarray(0, 4).equals(FAT) ? raw.readUInt32BE(4) : null;
        throw new Error('actual framework input refused: path=' + binary + '; header=' + raw.subarray(0, 16).toString('hex') +
          '; fatCount=' + count + '; declaredArchitectures=' + JSON.stringify(architectures) + '; ' + error.message,
        { cause: error });
      }
      const archives = parsed.kind === 'fat-ar' ? parsed.slices : [{ ordinal: 0, archive: parsed }];
      demand(archives.length === 1 && archives[0].archive.kind === 'bsd-ar',
        'Libbox static slice inventory differs from framework declaration');
      const headers = {};
      for (const name of readdirSync(join(library, 'Headers')).filter(name => name.endsWith('.h'))) {
        const header = realpathSync(join(library, 'Headers', name));
        headers[name] = utf8.decode(read(header)).replace(/\r\n?/g, '\n');
      }
      const contracts = sdkHeaderContracts(Object.values(headers).join('\n'));
      const members = archives[0].archive.members.filter(member => member.classification === 'code');
      demand(members.length && !Object.hasOwn(slices, target) && members.every(member => member.machO.cpu === cpu),
        'static archive has no code, or mixed/unknown/duplicate Apple architecture slice');
      const infos = [], ids = [];
      for (const member of members) {
        const { sections, platformCommands } = member.machO;
        const go = sections.some(section => Object.hasOwn(section, 'buildInfo'));
        demand((!go || platformCommands.length > 0) && platformCommands.every(command =>
          command.platform === platform && command.minOSRaw > 0 && command.minOSRaw <= 15 << 16 &&
          (!go || command.minOSRaw === 15 << 16)), 'Apple archive deployment/platform differs');
        for (const section of sections) {
          if (section.buildInfo) infos.push(section.buildInfo);
          ids.push(...(section.buildIDs ?? []).map(note => note.value));
        }
      }
      demand(infos.length === 1 && ids.length === 1, 'one original Go carrier per Apple slice required');
      buildInfo.bind(infos[0], { arch, simulator: simulator === 'simulator', expectedId, actualId: ids[0] });
      slices[target] = { libraryIdentifier: identifier, binary: pins.get(binary),
        headerSha256: Object.fromEntries(Object.entries(headers).map(([name, text]) => [name, sha(text)])), ...contracts,
        goBuildInfo: infos[0], buildID: ids[0],
        linkerOptions: members.flatMap(member => member.machO.loadCommands.filter(command => command.command === 0x2d)
          .map(command => ({ memberOrdinal: member.ordinal, memberName: member.name, memberSha256: member.sha256, command }))),
        members: members.map(({ machO, ...member }) => ({ ...member,
          goCarrier: machO.sections.some(section => Object.hasOwn(section, 'buildInfo')) })),
      };
    }
    demand(Object.keys(slices).length === Object.keys(APPLE).length, 'formal Apple inventory must contain exactly two arm64 platform slices');
    await buildInfo.verify({ source, graph });
    const inventory = treeInventory(framework, { pinned: pins, observations: reads });
    demand(equal(inventory.map(({ bytes, sha256, ...row }) => row), layout),
      'framework layout changed during original-byte inspection');
    demand(equal(treeInventory(framework, { observations: reads }), inventory),
      'framework changed during original-byte inspection');
    return { format: 'static-XCFramework', inventory, inventorySha256: sha(canonical(inventory)), slices };
  });
}

export function referenceLayout(output) {
  demand(output?.format === 'static-XCFramework' && object(output.slices), 'actual static output required for origins');
  const slices = Object.entries(output.slices);
  demand(slices.length > 0 && slices.length <= maxEntries, 'actual static slices required for origins');
  let count = 0;
  return Object.fromEntries(slices.map(([target, row]) => {
    demand(target.length > 0 && target.length <= 4096 && Array.isArray(row?.members), 'invalid static target/member layout');
    const refs = [], ordinals = new Set();
    for (const member of row.members) {
      demand(++count <= maxEntries, 'static member reference count budget');
      demand(object(member) && Number.isSafeInteger(member.ordinal) && member.ordinal >= 0 && !ordinals.has(member.ordinal),
        'duplicate/invalid static member ordinal');
      ordinals.add(member.ordinal);
      demand(typeof member.goCarrier === 'boolean' && typeof member.sha256 === 'string' && digestPattern.test(member.sha256),
        'invalid static member digest/classification');
      if (!member.goCarrier) refs.push({ memberOrdinal: member.ordinal, sha256: member.sha256 });
    }
    return [target, refs];
  }));
}

export function bindOrigins(mapping, origins) {
  const usedDigests = new Set();
  for (const rows of Object.values(mapping)) {
    const used = new Map();
    for (const { sha256: digest } of rows) {
      const count = (used.get(digest) ?? 0) + 1;
      demand(origins[digest]?.length >= count, 'static member lacks actual compiler/runtime origin');
      used.set(digest, count);
      usedDigests.add(digest);
    }
  }
  return Object.fromEntries([...usedDigests].sort().map(digest => [digest, origins[digest]]));
}

export async function sourceTable(files, { reader = readPinned } = {}) {
  const entries = Object.entries(files).sort(([left], [right]) => codepointOrder(left, right));
  demand(entries.length > 0 && entries.length <= maxEntries, 'native source file count budget');
  const buildInfo = new AppleBuildInfoBatch(), origins = Object.create(null);
  let rows = 0, characters = 0;
  function add(row) {
    characters += row.path.length;
    demand(++rows <= maxEntries && row.path.length <= 4096 && characters <= 16 * 1024 ** 2,
      'native source table row/path budget');
    (origins[row.sha256] ??= []).push(row);
  }
  let container;
  for (const [path, pin] of entries) {
    if (pin.kind === 'compiler-object') {
      add({ kind: pin.kind, path, sha256: pin.sha256 });
      continue;
    }
    demand(pin.kind === 'external-static-archive', 'unknown native origin kind');
    container ??= (await import('../apple-carrier.mjs')).container;
    const { bytes, pin: actual } = await reader(path);
    demand(equal(actual, { bytes: pin.bytes, sha256: pin.sha256 }), 'static origin changed during inspection');
    const parsed = container(bytes, buildInfo.observe);
    const slices = parsed.kind === 'fat-ar' ? parsed.slices : [{ ordinal: 0, archive: parsed }];
    for (const row of slices) {
      demand(row.archive.kind === 'bsd-ar', 'external runtime must be a static archive');
      for (const member of row.archive.members) {
        if (member.classification === 'code') add({ kind: pin.kind, path, archiveSha256: pin.sha256,
          sliceOrdinal: row.ordinal, memberOrdinal: member.ordinal, sha256: member.sha256 });
      }
    }
  }
  await buildInfo.verify();
  return origins;
}

function validateOrigin(row) {
  const external = row?.kind === 'external-static-archive';
  const keys = ['kind', 'path', 'sha256', ...(external ? ['archiveSha256', 'sliceOrdinal', 'memberOrdinal'] : [])];
  demand(object(row) && Object.keys(row).length === keys.length && keys.every(key => Object.hasOwn(row, key)) &&
    (external || row.kind === 'compiler-object') && typeof row.path === 'string' && row.path.length > 0 &&
    row.path.length <= 4096 && typeof row.sha256 === 'string' && digestPattern.test(row.sha256),
    'invalid static origin identity');
  if (external) demand(typeof row.archiveSha256 === 'string' && digestPattern.test(row.archiveSha256) &&
    [row.sliceOrdinal, row.memberOrdinal].every(ordinal => Number.isSafeInteger(ordinal) && ordinal >= 0),
  'invalid legacy/static archive origin identity');
}

// Tokenize only the pinned generator's logged argv. Never evaluate shell text.
function shellWords(line) {
  const words = [];
  let word = '', quote = '', started = false;
  for (let i = 0; i < line.length; i += 1) {
    const char = line[i];
    if (char === '\\' && quote !== "'") {
      const next = line[++i];
      demand(next !== undefined, 'incomplete quoted generator command');
      word += quote === '"' && next !== '"' && next !== '\\' ? '\\' + next : next;
      started = true;
    } else if (quote) {
      if (char === quote) quote = '';
      else word += char;
    } else if (char === "'" || char === '"') {
      quote = char;
      started = true;
    } else if (/[ \t\r\n]/.test(char)) {
      if (started) words.push(word);
      word = '';
      started = false;
    } else {
      word += char;
      started = true;
    }
  }
  demand(!quote, 'incomplete quoted generator command');
  if (started) words.push(word);
  return words;
}

function* walkPaths(root) {
  for (const item of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, item.name);
    yield path;
    if (item.isDirectory()) yield* walkPaths(path);
  }
}

export async function collectNativeInputs({ scratch, log, platform, output, cache }) {
  scratch = realpathSync(scratch);
  cache = realpathSync(cache);
  const originalLog = readPinned(log), text = utf8.decode(originalLog.bytes).replace(/\r\n?/g, '\n');
  const workRoots = [...new Set([...text.matchAll(/^WORK=(.+)$/gm)].map(match => realpathSync(match[1])))].sort(codepointOrder);
  demand(workRoots.length > 0 && workRoots.length <= 32 &&
    workRoots.every(path => statSync(path).isDirectory() && within(scratch, path)), 'actual compiler WORK roots must remain in the fixed scratch');
  const files = Object.create(null);
  let count = 0;
  function record(path, kind) {
    if (!Object.hasOwn(files, path)) demand(++count <= maxEntries, 'native source file count budget');
    files[path] = { kind, ...filePin(path) };
  }
  for (const root of workRoots) {
    for (const path of walkPaths(root)) {
      if (!path.endsWith('.o')) continue;
      demand(!lstatSync(path).isSymbolicLink() && within(scratch, realpathSync(path)), 'compiler object escaped scratch');
      record(path, 'compiler-object');
    }
  }
  if (platform === 'apple') {
    const mobile = workRoots.filter(root => ['ios', 'iossimulator', 'src'].some(target =>
      statSync(join(root, target), { throwIfNoEntry: false })?.isDirectory()));
    demand(mobile.length === 1, 'one actual gomobile workspace required');
    for (const line of text.split('\n')) {
      if (!line.includes('libtool -static -o ')) continue;
      const words = shellWords(line.replaceAll('$WORK', mobile[0]).replaceAll('$GOPATH', join(cache, 'go-path'))
        .replaceAll('$GOMOBILE', join(cache, 'go-path/pkg/gomobile')));
      for (const token of words.slice(words.indexOf('-o') + 3)) {
        const path = realpathSync(token);
        demand(path.endsWith('.a') && within(cache, path), 'external static origin outside verified module cache');
        record(path, 'external-static-archive');
      }
    }
  }
  demand(count > 0, 'actual compiler/static input bytes are missing');
  const facts = { schema: 'polaris-mobile-native-inputs-v1', scratch, cache, workRoots,
    originalBindLog: originalLog.pin, files };
  if (platform === 'apple') {
    const staticMemberOrigins = referenceLayout(output);
    Object.assign(facts, { schema: 'polaris-mobile-native-inputs-v2', staticMemberOrigins,
      originsByDigest: bindOrigins(staticMemberOrigins, await sourceTable(files)) });
  }
  return facts;
}

export async function verifyNativeInputs({ facts, scratch, log, output }) {
  demand(['polaris-mobile-native-inputs-v1', 'polaris-mobile-native-inputs-v2'].includes(facts?.schema),
    'actual native input receipt required');
  const staticOutput = output?.format === 'static-XCFramework' || facts.schema === 'polaris-mobile-native-inputs-v2' ||
    Object.hasOwn(facts, 'staticMemberOrigins');
  const refs = staticOutput ? referenceLayout(output) : null;
  if (refs) {
    demand(facts.schema === 'polaris-mobile-native-inputs-v2' && equal(facts.staticMemberOrigins, refs),
      'static source table/member references differ');
    demand(object(facts.originsByDigest), 'actual source table/member origins required');
    for (const choices of Object.values(facts.originsByDigest)) {
      demand(Array.isArray(choices), 'actual source table/member origins required');
      for (const row of choices) validateOrigin(row);
    }
  }
  if (Object.hasOwn(facts, 'generatorOutputRoot')) {
    const generated = realpathSync(facts.generatorOutputRoot);
    demand(basename(generated) === 'build' && basename(dirname(generated)) === 'build-view',
      'generator output escaped the fixed build view');
    demand(equal(treeInventory(generated), facts.generatorOutputInventory), 'original generator output bytes differ');
  }
  const before = realpathSync(facts.scratch), cache = realpathSync(facts.cache);
  if (scratch !== undefined) demand(before === realpathSync(scratch), 'native scratch input changed');
  if (log !== undefined) demand(equal(filePin(log), facts.originalBindLog), 'original compiler log changed');
  demand(object(facts.files), 'actual native input inventory required');
  const files = Object.entries(facts.files);
  demand(files.length > 0 && files.length <= maxEntries, 'native input inventory bounds');
  for (const [path, pin] of files) {
    const external = pin.kind === 'external-static-archive';
    demand((external || pin.kind === 'compiler-object') && !lstatSync(path).isSymbolicLink() &&
      within(external ? cache : before, realpathSync(path)), 'native input escaped owned roots');
    if (!refs || !external) {
      demand(equal(filePin(path), { bytes: pin.bytes, sha256: pin.sha256 }), 'actual native input bytes changed');
    }
  }
  if (refs) demand(equal(facts.originsByDigest, bindOrigins(refs, await sourceTable(facts.files))),
    'actual source table/member references/counts differ');
}
