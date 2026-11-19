// Build or revalidate exactly one formal mobile component in its owned slots.
import {
  closeSync, fsyncSync, fstatSync, lstatSync, mkdtempSync, openSync, readlinkSync,
  realpathSync, renameSync, rmSync, statfsSync, unlinkSync, writeFileSync, writeSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';
import {
  ROOT, SOURCE_ADMISSION, bindCommand, canonical, decode, declaredPlan, gitEnvironment,
  load, require, toolEnvironment, verifySource,
} from './fork-input.mjs';
import {
  collectNativeInputs, filePin, inspectAar, inspectFramework, readReceipt,
  receiptChunks, receiptDigest, receiptFieldBytes, receiptLimit, treeInventory, verifyNativeInputs,
} from './component-admission.mjs';
import { decodeDocuments, pinned, receiptPolicy } from './source-admission.mjs';
import { createNativeRunner, nativeCapacity, nativeEnvironment, privateDir, progressWrite, withNativeSignals } from './native-runner.mjs';
import { install, recover } from './transaction.mjs';

export { createNativeRunner, nativeCapacity, nativeEnvironment, privateDir, progressWrite };

const exists = path => lstatSync(path, { throwIfNoEntry: false }) !== undefined;
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const hex = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const same = (left, right) => canonical(left).equals(canonical(right));
const output = value => console.log(JSON.stringify(value));
const without = (value, field) => Object.fromEntries(Object.entries(value).filter(([key]) => key !== field));
const command = async (run, argv, options) => (await run(argv, options)).trim();

export function writeJson(path, value) {
  const maximumBytes = receiptLimit(value);
  let size = 0;
  try {
    for (const chunk of receiptChunks(value, { pretty: true })) size += chunk.length;
  } catch (error) {
    console.error(JSON.stringify({ schema: 'polaris-receipt-size-observation-v1', phase: 'bounded-serialization-rejected',
      path, maximumBytes, error: error.message, scope: 'no full serialized bytes produced or installed' }));
    throw new Error('receipt rejected before installation: ' + error.message, { cause: error });
  }
  console.error(JSON.stringify({ schema: 'polaris-receipt-size-observation-v1', phase: 'serialize-before-install',
    path, bytes: size, maximumBytes, receiptSchema: value.schema,
    canonicalValueBytesByTopLevelField: receiptFieldBytes(value), scope: 'actual serialized size; not installation or carrier acceptance' }));
  const temporary = path + '.tmp';
  const fd = openSync(temporary, 'wx', 0o600);
  try {
    try {
      let written = 0;
      for (const chunk of receiptChunks(value, { pretty: true })) {
        let at = 0;
        while (at < chunk.length) {
          const count = writeSync(fd, chunk, at, chunk.length - at);
          require(count > 0, 'receipt write made no progress');
          at += count;
        }
        written += at;
      }
      require(written === size, 'receipt changed during serialization');
      fsyncSync(fd);
    } finally { closeSync(fd); }
    renameSync(temporary, path);
  } catch (error) {
    unlinkSync(temporary);
    throw error;
  }
}

export const PRODUCER_FILES = Object.freeze([...receiptPolicy.sourceFiles.producer]);
export const ANDROID_BUILD_CODE_POLICY = 'polaris-android-build-code-v2';
const appleCode = new Set([
  'apple-cache.py', 'apple_rust.py', 'apple_tls_fingerprint.py', 'ci_apple.py', 'build_apple_app.py',
  'apple-admission.mjs', 'apple_admission.py', 'apple_handoff.py', 'apple-cache.mjs',
  'apple-rust.mjs', 'ci-apple.mjs', 'build-apple-app.mjs', 'apple-handoff.mjs',
  'apple-evidence.mjs',
].map(name => 'scripts/mobile-core/' + name));
appleCode.add('scripts/apple-carrier.py');
appleCode.add('scripts/apple-carrier.mjs');

// Recorded names from older receipts establish that their original inventory
// was complete. They never stand in for the current executable or its digest.
const priorProducerFiles = new Map(Object.entries({
  'source-admission.mjs': ['source_policy.py'],
  'prepare-ndk-linux.mjs': ['prepare-ndk-linux.py'],
  'apple-admission.mjs': ['apple_admission.py'],
  'fork-input.mjs': ['fork_input.py'],
  'producer.mjs': ['producer.py'],
  'native-runner.mjs': ['budget.py', 'producer.py'],
  'native-reservation.mjs': ['budget.py'],
  'native-capacity.mjs': ['budget.py'],
  'native-fd.rs': ['budget.py', 'producer.py'],
  'transaction.mjs': ['transaction.py'],
  'component-admission.mjs': ['artifacts.py', 'native_inputs.py'],
  'mobile-consumption.mjs': ['consume.py', 'carriers.py'],
  'android-ci.mjs': ['ci_android.py'],
  'apple-cache.mjs': ['apple-cache.py'],
  'apple-handoff.mjs': ['apple_handoff.py'],
  'ci-apple.mjs': ['ci_apple.py'],
  'apple-evidence.mjs': ['ci_apple.py'],
  'apple-rust.mjs': ['apple_rust.py'],
  'build-apple-app.mjs': ['build_apple_app.py'],
}).map(([name, alternatives]) => ['scripts/mobile-core/' + name, alternatives.map(item => 'scripts/mobile-core/' + item)]));
priorProducerFiles.set('scripts/apple-carrier.mjs', ['scripts/apple-carrier.py']);

export async function producerCode(run) {
  const options = { cwd: ROOT, env: gitEnvironment() };
  require(!await run(['git', 'status', '--porcelain=v1', '--untracked-files=all'], options), 'App author source must be frozen');
  const candidate = await command(run, ['git', 'rev-parse', 'HEAD'], options);
  const files = Object.fromEntries([...PRODUCER_FILES, ...receiptPolicy.sourceFiles.shared]
    .map(name => [name, filePin(join(ROOT, name))]));
  return { candidate, files };
}

export function androidBuildCode(identity, policy = ANDROID_BUILD_CODE_POLICY) {
  require(policy === ANDROID_BUILD_CODE_POLICY, 'unknown Android build code policy');
  const files = identity?.files;
  require(object(files), 'original producer code inventory is missing');
  const missing = new Set(PRODUCER_FILES.filter(name => !Object.hasOwn(files, name)));
  for (const name of missing) {
    if (priorProducerFiles.get(name)?.every(previous => Object.hasOwn(files, previous))) missing.delete(name);
  }
  // The three jointly introduced Android ELF verifier files may all be absent in an old receipt.
  const verifier = ['scripts/verify-apk.mjs', 'scripts/lib/zip-archive.mjs', 'scripts/release-assets.mjs'];
  if (verifier.every(name => missing.has(name))) for (const name of verifier) missing.delete(name);
  require(missing.size === 0, 'complete original producer code inventory required');
  require(Object.values(files).every(pin => object(pin) && Object.keys(pin).length === 2 &&
    Object.hasOwn(pin, 'bytes') && Object.hasOwn(pin, 'sha256') &&
    Number.isSafeInteger(pin.bytes) && pin.bytes > 0 && hex(pin.sha256)), 'producer code file pin differs');
  return { policy, files: Object.fromEntries(Object.entries(files).filter(([name]) => !appleCode.has(name))) };
}

function comparisonBinding(value) {
  require(object(value) && object(value.producer ?? {}), 'component input/producer structure differs');
  const producer = without(value.producer ?? {}, 'candidate');
  if (value.platform === 'android') {
    const projected = androidBuildCode(value.producer);
    Object.assign(producer, { files: projected.files, comparisonPolicy: projected.policy });
  }
  return { ...value, producer };
}

export const equivalentBinding = (original, current) => same(comparisonBinding(original), comparisonBinding(current));

function componentRevalidationObservation(receipt, binding, receiptPin, reused) {
  const observation = {
    schema: 'polaris-mobile-component-revalidation-v1', derivedObservation: true, reused,
    comparisonPolicy: binding.platform === 'android' ? ANDROID_BUILD_CODE_POLICY : 'polaris-full-producer-code-v1',
    currentVerifier: binding.producer, originalProducerCandidate: receipt.input.producer.candidate,
    originalReceipt: receiptPin, originalComponentFingerprint: receipt.fingerprint,
    originalBuildID: 'polaris-mobile-' + receiptDigest(receipt.input),
    currentBuildInputFingerprint: receiptDigest(comparisonBinding(binding)),
    scope: 'current byte/origin revalidation; original receipt/BuildID remain unchanged; no App/device acceptance',
  };
  return { ...observation, fingerprint: receiptDigest(observation) };
}

function cacheBindingObservation(receipt, binding, fingerprint) {
  const left = comparisonBinding(receipt.input), right = comparisonBinding(binding);
  const known = ['schema', 'platform', 'sourceObservation', 'sourceContract', 'moduleGraph', 'tools', 'producer', 'sourceOverlay'];
  const graph = rows => Array.isArray(rows)
    ? { rows: rows.length, directories: rows.filter(row => row?.Dir).length, sums: rows.filter(row => row?.Sum).length }
    : { rows: null, directories: null, sums: null };
  require(hex(fingerprint) && hex(receipt.fingerprint), 'actual binding/receipt fingerprint required');
  return { schema: 'polaris-mobile-cache-binding-observation-v1', derivedObservation: true,
    changedFields: known.filter(name => !same(left[name] ?? null, right[name] ?? null)),
    unknownFieldCount: new Set([...Object.keys(left), ...Object.keys(right)].filter(name => !known.includes(name))).size,
    retainedReceiptFingerprint: receipt.fingerprint, currentInputFingerprint: fingerprint,
    originalModuleGraph: graph(left.moduleGraph), currentModuleGraph: graph(right.moduleGraph),
    inputValuesExported: false, fileContentsExported: false,
    scope: 'actual comparison before refusal; not component, recovery or App acceptance' };
}

const downloadLimits = Object.freeze({
  stdoutBytes: 1024 ** 2, stderrBytes: 4096, lineBytes: 8192, lines: 32768,
  stringBytes: 4096, depth: 8, tokens: 65536, seconds: 3600, cleanupSeconds: 2,
});
const downloadFields = new Set(['Path', 'Version', 'Query', 'Info', 'GoMod', 'Zip', 'Dir', 'Sum', 'GoModSum', 'Origin', 'Reuse']);
const originFields = new Set(['VCS', 'URL', 'Subdir', 'Hash', 'TagPrefix', 'TagSum', 'Ref', 'RepoSum']);

function downloadStructure(row) {
  return object(row) && Object.keys(row).every(key => downloadFields.has(key)) &&
    ['Path', 'Version', 'Sum', 'Dir'].every(key => typeof row[key] === 'string') &&
    Object.entries(row).every(([key, value]) => {
      if (key === 'Reuse') return typeof value === 'boolean';
      if (key === 'Origin') return object(value) && Object.entries(value)
        .every(([field, text]) => originFields.has(field) && typeof text === 'string');
      return typeof value === 'string';
    });
}

export async function boundedModuleDownload(runBounded, argv, { cwd, env, expectedCount, observation }) {
  require(Number.isSafeInteger(expectedCount) && expectedCount > 0 && expectedCount <= 1024,
    'bounded module download item count required');
  require(!exists(observation), 'retained module download observation');
  const limits = { ...downloadLimits, items: expectedCount };
  const deadline = performance.now() + limits.seconds * 1000;
  const chunks = [];
  let lines = 0, lineBytes = 0, stringBytes = 0, depth = 0, tokens = 0, items = 0;
  let quoted = false, escaped = false;
  function scan(chunk) {
    chunks.push(Buffer.from(chunk));
    for (const byte of chunk) {
      lineBytes += 1;
      if (byte === 10) { lines += 1; lineBytes = 0; }
      if (lines > limits.lines || lineBytes > limits.lineBytes) return 'line-budget';
      if (quoted) {
        if (++stringBytes > limits.stringBytes) return 'string-budget';
        if (escaped) escaped = false;
        else if (byte === 92) escaped = true;
        else if (byte === 34) quoted = false;
      } else if (byte === 34) {
        quoted = true;
        stringBytes = 0;
      } else if (byte === 123 || byte === 91) {
        depth += 1;
        tokens += 1;
        if (depth === 1) {
          items += 1;
          if (byte !== 123) return 'download-json-structure';
        }
        if (depth > limits.depth || items > expectedCount) return 'structure-or-item-budget';
      } else if (byte === 125 || byte === 93) depth -= 1;
      else if (byte === 44 || byte === 58) tokens += 1;
      if (tokens > limits.tokens || depth < 0) return 'structure-budget';
    }
  }
  let result;
  try {
    result = await runBounded(argv, {
      cwd, env, stdoutLimit: limits.stdoutBytes, stderrLimit: limits.stderrBytes,
      timeoutSeconds: limits.seconds, cleanupSeconds: limits.cleanupSeconds, onStdout: scan,
    });
  } catch {
    result = { complete: false, reason: 'collection-error', cleanupComplete: false,
      stdoutBytesObserved: chunks.reduce((total, chunk) => total + chunk.length, 0), stderrBytesObserved: 0 };
  }
  const reasons = {
    'exit-or-live-group': 'download-exit-or-live-group', 'interrupted': 'download-interrupted',
    'cleanup-incomplete': 'download-cleanup-incomplete', 'collection-error': 'download-collection-or-json-error',
  };
  const knownReasons = new Set(['deadline', 'stdout-budget', 'stderr-budget', 'line-budget', 'string-budget',
    'structure-or-item-budget', 'structure-budget', 'download-json-structure']);
  let reason = result.complete ? null : (Object.hasOwn(reasons, result.reason) ? reasons[result.reason] :
    knownReasons.has(result.reason) ? result.reason : 'download-collection-or-json-error');
  const rows = [];
  if (!reason) {
    try {
      const documents = decodeDocuments(Buffer.concat(chunks), limits.stdoutBytes);
      for (const row of documents) {
        if (performance.now() >= deadline) { reason = 'deadline'; break; }
        if (rows.length >= expectedCount) { reason = 'item-budget'; break; }
        if (!object(row) || Object.hasOwn(row, 'Error')) { reason = 'download-json-error'; break; }
        if (!downloadStructure(row)) { reason = 'download-json-structure'; break; }
        rows.push(row);
      }
      if (!reason && rows.length !== expectedCount) reason = 'download-item-count';
    } catch { reason = 'download-collection-or-json-error'; }
  }
  try {
    writeJson(observation, {
      schema: 'polaris-module-download-observation-v1', derivedObservation: true, complete: reason === null, reason,
      cleanupComplete: result.cleanupComplete, stdoutBytesObserved: result.stdoutBytesObserved,
      stderrBytesObserved: result.stderrBytesObserved, linesObserved: lines, itemsObserved: items, itemsDecoded: rows.length,
      limits, rawOutputRetained: false, inputValuesExported: false, processRSS: null,
      scope: 'bounded pipe/JSON collection only; module checksum/cache admission separately required',
    });
  } catch { throw new Error('module download observation could not be saved'); }
  require(!reason, 'module download rejected: ' + reason);
  return rows;
}

async function actualModuleList(run, source, go, env, scratch) {
  const directory = mkdtempSync(join(scratch, 'module-list-'));
  try {
    const log = join(directory, 'go-list.log'), graphLog = join(directory, 'graph.log');
    await run([go, 'list', '-m', '-json', 'all'], { cwd: source, env, log, capture: 'none' });
    await run([process.execPath, SOURCE_ADMISSION, '--module-list', log + '.stdout.raw'],
      { log: graphLog, capture: 'none', timeoutSeconds: 30 });
    const maximum = receiptPolicy.receipt.controlBytes;
    return decode(pinned(graphLog + '.stdout.raw', maximum).bytes, maximum);
  } finally { rmSync(directory, { recursive: true }); }
}

async function admitModuleCache(runBounded, source, graph, go, env, work) {
  const candidates = [];
  for (const row of graph) {
    const node = Object.hasOwn(row, 'Replace') ? row.Replace : row;
    require(object(node), 'module replacement structure differs');
    if (!node.Main && !node.Dir && node.Version) candidates.push(node);
  }
  if (!candidates.length) return;
  const sourceBytes = name => pinned(join(source, name), 1024 ** 2, false, { allowEmpty: true }).bytes;
  const originals = Object.fromEntries(['go.mod', 'go.sum'].map(name => [name, sourceBytes(name)]));
  const key = (path, version) => JSON.stringify([path, version]);
  const sums = new Map();
  const lines = new TextDecoder('utf8', { fatal: true }).decode(originals['go.sum']).split(/\r?\n/);
  if (lines.at(-1) === '') lines.pop();
  for (const line of lines) {
    const fields = line.trim().split(/\s+/);
    require(fields.length === 3, 'source sum record differs');
    const [path, version, checksum] = fields;
    if (version.endsWith('/go.mod')) continue;
    const id = key(path, version);
    require(!sums.has(id) || sums.get(id) === checksum, 'conflicting source module checksum');
    sums.set(id, checksum);
  }
  const pending = new Map();
  for (const { Path: path, Version: version } of candidates) {
    const id = key(path, version);
    if (sums.has(id)) pending.set(id, { path, version, checksum: sums.get(id) });
  }
  if (!pending.size) return;
  require(pending.size <= 1024 && [...pending.values()].every(({ path, version }) =>
    typeof path === 'string' && typeof version === 'string' && Buffer.byteLength(path) <= 512 &&
    Buffer.byteLength(version) <= 128 && !/[\s@]/.test(path + version)),
  'bounded exact source-summed module arguments required');
  const compare = (a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b));
  const modules = [...pending.values()].sort((left, right) => compare(left.path, right.path) || compare(left.version, right.version))
    .map(({ path, version }) => path + '@' + version);
  require(modules.reduce((size, item) => size + Buffer.byteLength(item) + 1, 0) <= 128 * 1024,
    'module download argument byte budget exceeded');
  const view = privateDir(join(work, 'module-cache-admission'));
  const modfile = join(view, 'admission.mod'), sumfile = join(view, 'admission.sum');
  require(!exists(modfile) && !exists(sumfile), 'prior module admission inputs retained; explicit recovery required');
  writeFileSync(modfile, originals['go.mod'], { flag: 'wx', mode: 0o600 });
  writeFileSync(sumfile, originals['go.sum'], { flag: 'wx', mode: 0o600 });
  const downloaded = await boundedModuleDownload(runBounded,
    [go, 'mod', 'download', '-modfile=' + modfile, '-json', ...modules], {
      cwd: source, env, expectedCount: pending.size, observation: join(work, 'module-cache-admission-observation.json'),
    });
  require(downloaded.length === pending.size &&
    new Set(downloaded.map(row => key(row.Path, row.Version))).size === pending.size &&
    downloaded.every(row => row.Dir && row.Sum === pending.get(key(row.Path, row.Version))?.checksum),
  'actual downloaded module identity differs from frozen source sum');
  require(Object.entries(originals).every(([name, raw]) => raw.equals(sourceBytes(name))),
    'source module bytes changed during cache admission');
}

export function executableDriverIdentity(path) {
  // LLVM dispatches on argv[0]; retain the driver name and pin its physical bytes.
  const driverPath = resolve(path), driver = lstatSync(driverPath);
  require(driver.isFile() || driver.isSymbolicLink(), 'compiler driver must be a file or explicit symlink');
  const physical = realpathSync(driverPath);
  return { path: physical, ...filePin(physical), driverPath, driverMode: driver.mode & 0o7777,
    driverLinkTarget: driver.isSymbolicLink() ? readlinkSync(driverPath) : null };
}

export async function compilerFact(path, env, run) {
  const identity = executableDriverIdentity(path);
  const version = await run([identity.driverPath, '--version'], { env });
  require(same(executableDriverIdentity(path), identity), 'compiler driver/physical bytes changed during observation');
  return { ...identity, version };
}

function generatorBuildInfo(raw, name, value) {
  const [header, ...lines] = raw.split(/\r?\n/);
  require(header?.endsWith(': go' + value.goVersion) && header.length > value.goVersion.length + 4,
    'actual mobile tool Go version differs: ' + name);
  const rows = lines.filter(line => line.trim()).map(line => line.trim().split(/\s+/));
  const paths = rows.filter(row => row[0] === 'path');
  const modules = rows.filter(row => row[0] === 'mod');
  require(same(paths, [['path', 'github.com/sagernet/gomobile/cmd/' + name]]) &&
    modules.length === 1 && modules[0].length === 4 &&
    modules[0][1] === 'github.com/sagernet/gomobile' && modules[0][2] === value[name + 'Version'] &&
    /^h1:[A-Za-z0-9+/]{43}=$/.test(modules[0][3]) && !rows.some(row => row[0] === '=>'),
  'actual mobile tool module differs: ' + name);
}

export async function toolFacts(args, value, env, run) {
  const names = ['go', 'gomobile', 'gobind'];
  const paths = Object.fromEntries(names.map(name => [name, realpathSync(args[name])]));
  const originals = () => Object.fromEntries(names.map(name => [name, filePin(paths[name])]));
  const executables = originals(), mobileBuildInfo = {};
  const tool = argv => run(argv, { env });
  require((await tool([paths.go, 'version'])).trim().startsWith('go version go' + value.goVersion + ' '),
    'actual Go tool version differs');
  for (const name of ['gomobile', 'gobind']) {
    const raw = await tool([paths.go, 'version', '-m', paths[name]]);
    generatorBuildInfo(raw, name, value);
    mobileBuildInfo[name] = raw;
  }
  const account = { executables, mobileBuildInfo };
  if (args.platform === 'android') {
    const jdk = realpathSync(args.jdk), ndk = realpathSync(args.ndk);
    const javaPath = join(jdk, 'bin/java'), java = await tool([javaPath, '--version']);
    require(/^(?:openjdk|java) 17[. ]/.test(java), 'formal JNI requires actual Java17');
    const javac = realpathSync((await tool(['which', 'javac'])).trim());
    require(javac === realpathSync(join(jdk, 'bin/javac')), 'actual javac resolution differs from pinned JDK');
    const javacVersion = (await tool([javac, '--version'])).trim();
    require(/^javac 17(?:[.][0-9]+)*(?:[-+].*)?$/.test(javacVersion), 'actual javac must be Java17');
    paths.javap = join(jdk, 'bin/javap');
    const binaries = join(ndk, 'toolchains/llvm/prebuilt', process.platform === 'linux' ? 'linux-x86_64' : 'darwin-x86_64', 'bin');
    const readelfPath = join(binaries, 'llvm-readelf'), readelf = executableDriverIdentity(readelfPath);
    paths.readelf = readelf.driverPath;
    const compilers = {};
    for (const name of ['clang', 'clang++', 'ld.lld', 'llvm-ar']) {
      compilers[name] = await compilerFact(join(binaries, name), env, run);
    }
    Object.assign(account, {
      java: { path: realpathSync(javaPath), ...filePin(realpathSync(javaPath)), version: java },
      javac: { path: javac, ...filePin(javac), version: javacVersion },
      ndkProperties: pinned(join(ndk, 'source.properties'), 1024 ** 2).bytes.toString('utf8').replace(/\r\n?/g, '\n'),
      readelf: { bytes: readelf.bytes, sha256: readelf.sha256 }, readelfDriver: readelf,
      javap: filePin(paths.javap), compilers,
    });
    require(same(executableDriverIdentity(readelfPath), readelf), 'readelf driver/physical bytes changed during observation');
  } else {
    const xcode = await tool(['xcodebuild', '-version']);
    require(xcode.includes('Xcode ' + value.apple.xcodeVersion) && xcode.includes('Build version ' + value.apple.xcodeBuild),
      'selected Xcode version/build differs');
    const sdk = {};
    for (const [name, key] of [['iphoneos', 'iPhoneSDK'], ['iphonesimulator', 'simulatorSDK']]) {
      const xcrun = async (...argv) => (await tool(['xcrun', '--sdk', name, ...argv])).trim();
      const version = await xcrun('--show-sdk-version');
      require(version === value.apple[key], 'actual Apple SDK version differs');
      const compiler = await xcrun('--find', 'clang'), path = realpathSync(await xcrun('--show-sdk-path'));
      sdk[name] = { version, path, settings: filePin(join(path, 'SDKSettings.json')),
        compiler: filePin(realpathSync(compiler)), compilerVersion: await tool([compiler, '--version']) };
    }
    Object.assign(account, { xcode, sdk });
  }
  require(same(originals(), executables), 'mobile tool bytes changed during observation');
  return { tools: paths, identities: account };
}

export async function appleNativeEnvironment(run, go, env, source) {
  const keys = ('CGO_CFLAGS CGO_CPPFLAGS CGO_CXXFLAGS CGO_FFLAGS CGO_LDFLAGS CC CXX AR PKG_CONFIG ' +
    'CGO_ENABLED GOEXPERIMENT GOAMD64 GOARM GOARM64 GO386 GOMIPS GOMIPS64 GOPPC64 GORISCV64 ' +
    'GO_EXTLINK_ENABLED GOFIPS140 GOFLAGS GOTOOLCHAIN').split(' ');
  const effective = JSON.parse(await run([go, 'env', '-json', ...keys], { cwd: source, env }));
  require(object(effective) && Object.keys(effective).length === keys.length &&
    keys.every(key => typeof effective[key] === 'string'), 'effective Go compilation inputs differ');
  const ambient = ('CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH OBJC_INCLUDE_PATH LIBRARY_PATH COMPILER_PATH ' +
    'CCC_OVERRIDE_OPTIONS CLANG_CONFIG_FILE_SYSTEM_DIR CLANG_CONFIG_FILE_USER_DIR GCC_EXEC_PREFIX PKG_CONFIG_PATH ' +
    'PKG_CONFIG_LIBDIR PKG_CONFIG_SYSROOT_DIR PKG_CONFIG_TOP_BUILD_DIR PKG_CONFIG_ALLOW_SYSTEM_CFLAGS ' +
    'PKG_CONFIG_ALLOW_SYSTEM_LIBS PKG_CONFIG_SYSTEM_INCLUDE_PATH PKG_CONFIG_SYSTEM_LIBRARY_PATH ' +
    'PKG_CONFIG_DISABLE_UNINSTALLED').split(' ');
  for (const kind of ['CFLAGS', 'CPPFLAGS', 'CXXFLAGS', 'FFLAGS', 'LDFLAGS']) {
    for (const mode of ['ALLOW', 'DISALLOW']) ambient.push('CGO_' + kind + '_' + mode);
  }
  return { go: effective, ambient: Object.fromEntries(ambient.map(key => [key, env[key] ?? null])), GODEBUG: 'gocacheverify=1' };
}

function under(parent, child) {
  const rel = relative(parent, child);
  return !isAbsolute(rel) && rel !== '..' && !rel.startsWith('..' + sep);
}

async function admittedBinding(args, value, work, cache, producer, initialSource, { run, runBounded }) {
  const scratch = privateDir(join(work, 'scratch')), home = homedir();
  // Go's pinned generator writes source/build even with TMPDIR set.
  require(under(join(home, 'Code'), work) || !under(home, work), 'producer Git build view must be under Code or outside home');
  const source = join(work, 'build-view');
  if (!exists(source)) {
    await run(['git', 'clone', '--no-checkout', '--shared', realpathSync(args.source), source], { env: gitEnvironment() });
    await run(['git', 'checkout', '--detach', value.sourceCommit], { cwd: source, env: gitEnvironment() });
  }
  require(realpathSync(source) === source, 'producer Git build view redirected');
  const verify = () => verifySource(source, value, { generatorOutputs: true, run });
  const sourceObservation = await verify();
  require(same(sourceObservation, initialSource), 'source changed during admission');
  let env, jar;
  if (args.platform === 'android') {
    const [sdk, bootclasspath] = JSON.parse(await run([process.execPath, join(ROOT, 'scripts/mobile-core/prepare-ndk-linux.mjs'),
      '--prepare-view', '--profile', join(ROOT, value.android.ndkArchivePath), '--sdk', args.sdk, '--ndk', args.ndk, work]));
    jar = bootclasspath;
    env = toolEnvironment(args.go, args.gomobile, args.gobind, cache, scratch,
      { sdk, ndk: realpathSync(args.ndk), jdk: realpathSync(args.jdk) });
    env.CGO_LDFLAGS = '-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384';
    env.CGO_LDFLAGS_ALLOW = '^-l:libcronet\\.a$';
  } else {
    require(process.platform === 'darwin', 'formal Apple bind requires a Mac host');
    env = toolEnvironment(args.go, args.gomobile, args.gobind, cache, scratch, { developer: realpathSync(args.developer) });
  }
  output({ phase: 'component-validation-resources', platform: args.platform, capacity: nativeCapacity(),
    compilerAdmissionRequired: false, budget: { filesystemPeakReserveBytes: value.budget.buildPeakReserveBytes,
      cacheMaxBytes: value.budget.cacheMaxBytes } });
  const { tools, identities } = await toolFacts(args, value, env, run);
  // gomobile init would install gobind@latest; only the original private marker is needed.
  privateDir(join(env.GOPATH, 'pkg/gomobile'));
  const verifyModules = () => run([tools.go, 'mod', 'verify'],
    { cwd: source, env, log: join(work, 'original-module-verify.log'), capture: 'none' });
  await verifyModules();
  const originalGraph = await actualModuleList(run, source, tools.go, env, scratch);
  await admitModuleCache(runBounded, source, originalGraph, tools.go, env, work);
  require(same(await verify(), sourceObservation), 'source changed during module cache admission');
  const moduleGraph = await actualModuleList(run, source, tools.go, env, scratch);
  await verifyModules();
  const binding = { schema: 'polaris-mobile-producer-input-v1', platform: args.platform,
    sourceObservation, sourceContract: value, moduleGraph, tools: identities, producer, sourceOverlay: 0 };
  if (args.platform === 'apple') binding.nativeEnvironment = await appleNativeEnvironment(run, tools.go, env, source);
  require(same(await producerCode(run), producer), 'producer source changed during tool admission');
  return { source, scratch, env, tools, identities, binding, jar };
}

function inspectComponent(component, value, binding, tools, run, scratch, expectedId,
  { originalOutput, inspectionProgress } = {}) {
  const graph = binding.moduleGraph;
  return binding.platform === 'android'
    ? inspectAar({ aar: component, source: value, graph, tools, run, scratch, expectedId, originalOutput })
    : inspectFramework({ framework: component, source: value, graph, expectedId, run, scratch, inspectionProgress });
}

async function verifyReceipt(component, receipt, value, binding, tools, run, scratch, inspectionProgress) {
  require(receipt.schema === 'polaris-mobile-component-v1' && receipt.platform === binding.platform &&
    same(receipt.input, binding) && receipt.sourceOverlay === 0 &&
    receipt.devicePassed === false && receipt.AppAccepted === false,
  'old/CLI/diagnostic component receipt rejected');
  require(receiptDigest(without(receipt, 'fingerprint')) === receipt.fingerprint, 'component receipt fingerprint differs');
  await verifyNativeInputs({ facts: receipt.nativeInputs, log: join(dirname(component), 'original-bind.log'),
    output: receipt.output, run });
  const expectedId = 'polaris-mobile-' + receiptDigest(binding);
  const observed = await inspectComponent(component, value, binding, tools, run, scratch, expectedId,
    { originalOutput: receipt.output, inspectionProgress });
  require(same(observed, receipt.output), 'cached component native/content predicate differs');
}

export function inspectionProgress(fd, stage, elapsedSeconds, fileReads, readBytes) {
  require(['verify-original', 'ipa-parse', 'static-linkmap', 'framework-inspection'].includes(stage),
    'unknown inspection progress stage');
  require(Number.isFinite(elapsedSeconds) && elapsedSeconds >= 0 && elapsedSeconds < 86400 &&
    [fileReads, readBytes].every(value => value === null || (Number.isSafeInteger(value) && value >= 0)),
  'invalid inspection progress measurement');
  const row = { schema: 'polaris-apple-inspection-observation-v1', stage,
    elapsedSeconds: Math.round(elapsedSeconds * 1000) / 1000, fileReads, readBytes,
    scope: 'local reads and elapsed only; no acceptance' };
  if (fd === undefined) console.error('APPLE_INSPECTION_PROGRESS=' + JSON.stringify(row));
  else progressWrite(fd, 'APPLE_INSPECTION_PROGRESS=', row);
}

export async function execute(args, runner) {
  const { run, withLease } = runner;
  const value = load();
  const producer = await producerCode(run);
  const initialSource = await verifySource(args.source, value, { run });
  await withLease(args.work, args.cache, value, async (work, cache) => {
    const { source, scratch, env, tools, identities, binding, jar } =
      await admittedBinding(args, value, work, cache, producer, initialSource, runner);
    const fingerprint = receiptDigest(binding);
    const apple = args.platform === 'apple';
    const component = join(work, apple ? 'Libbox.xcframework' : value.android.outputName);
    const receiptPath = join(work, 'component-receipt.json');
    const stageRoot = privateDir(join(work, 'staging'));
    const stagedComponent = join(stageRoot, apple ? 'Libbox.xcframework' : 'stage.aar');
    const stagedReceipt = join(work, 'stage-component-receipt.json');
    const log = join(work, 'stage-original-bind.log');
    const pairs = [[stagedComponent, component], [stagedReceipt, receiptPath], [log, join(work, 'original-bind.log')]];
    recover(pairs, work);
    const runTool = (argv, options) => run(argv, { cwd: source, env, ...options });
    const progress = (...args) => inspectionProgress(runner.borrowedProgressFd, ...args);
    const previous = join(work, 'previous-pair');
    let rebuild = !exists(component), receipt;
    if (!rebuild || exists(receiptPath)) {
      require(!rebuild && lstatSync(receiptPath, { throwIfNoEntry: false })?.isFile(),
        'incomplete retained output; preserve for explicit recovery');
      receipt = readReceipt(receiptPath);
      if (equivalentBinding(receipt.input ?? {}, binding)) {
        await verifyReceipt(component, receipt, value, receipt.input, tools, runTool, scratch, progress);
      } else {
        require(!exists(previous), 'prior component recovery slot is occupied');
        require(receipt.fingerprint === receiptDigest(without(receipt, 'fingerprint')),
          'retained old receipt is corrupt; preserve without replacement');
        if (apple) {
          require(same(treeInventory(component), receipt.output.inventory), 'retained old framework bytes differ');
        } else require(same(filePin(component), receipt.output.artifact), 'retained old component bytes differ');
        const observation = join(work, 'cache-binding-observation.json');
        require(!exists(observation), 'prior cache binding observation retained; explicit recovery required');
        writeJson(observation, cacheBindingObservation(receipt, binding, fingerprint));
        rebuild = true;
      }
    }
    if (rebuild) {
      const generatorOutput = join(source, 'build');
      require(!exists(generatorOutput), 'original generator outputs retained; explicit recovery required before rebinding');
      require(!exists(stagedComponent), 'unaccepted prior stage retained; inspect before recovery');
      const observation = nativeCapacity();
      output({ phase: 'component-native-capacity-sample', platform: args.platform, capacity: observation });
      const native = nativeEnvironment(env, observation);
      output({ phase: 'component-native-capacity', platform: args.platform, capacity: native.observation,
        compilerCapacityApplied: true });
      const declared = declaredPlan(args.platform, value).facts;
      const argv = bindCommand(args.platform, value, tools.gomobile, stagedComponent, fingerprint, jar);
      require(!exists(log), 'original bind log retained; archive its identity before another bind');
      // A new compiler object has to be visible even when the Go cache was populated.
      const buildEnv = { ...native.env, GODEBUG: 'gocacheverify=1' };
      await run(argv, { cwd: source, env: buildEnv, log, capture: 'none' });
      require(same(await verifySource(source, value, { generatorOutputs: true, run }), binding.sourceObservation),
        'source changed during bind');
      require(same((await toolFacts(args, value, buildEnv, run)).identities, identities) &&
        same(await producerCode(run), binding.producer), 'producer/tool input changed');
      if (apple) require(same(await appleNativeEnvironment(run, tools.go, buildEnv, source), binding.nativeEnvironment),
        'effective Go/native inputs changed during bind');
      const buildRun = (argv, options) => run(argv, { cwd: source, env: buildEnv, ...options });
      const observed = await inspectComponent(stagedComponent, value, binding, tools, buildRun, scratch,
        'polaris-mobile-' + fingerprint, { inspectionProgress: progress });
      const origins = await collectNativeInputs({ scratch, log, platform: args.platform, output: observed, cache, run: buildRun });
      if (exists(generatorOutput)) {
        origins.generatorOutputRoot = generatorOutput;
        origins.generatorOutputInventory = treeInventory(generatorOutput);
      }
      await verifyNativeInputs({ facts: origins, scratch, log, output: observed, run: buildRun });
      receipt = {
        schema: 'polaris-mobile-component-v1', platform: args.platform, input: binding, command: argv,
        declaredPlan: declared, sourceOverlay: 0, output: observed, nativeInputs: origins,
        AppAccepted: false, devicePassed: false, publicationEligible: false,
        scope: 'actual component bytes/API/source/tool identity only; final App/carrier/device pending',
      };
      receipt.fingerprint = receiptDigest(receipt);
      writeJson(stagedReceipt, receipt);
      await install(pairs, work,
        () => verifyReceipt(component, receipt, value, binding, tools, buildRun, scratch, progress));
    }
    const space = statfsSync(work, { bigint: true });
    require(space.bavail * space.frsize >= 1024n ** 3n, 'post-build filesystem reserve exhausted');
    require(same(await producerCode(run), producer), 'verifier source changed during component validation');
    output({ component, receipt: receiptPath, fingerprint: receipt.fingerprint, AppAccepted: false, devicePassed: false,
      revalidation: componentRevalidationObservation(receipt, binding, filePin(receiptPath), !rebuild) });
  });
}

async function main() {
  const required = ['source', 'go', 'gomobile', 'gobind', 'work', 'cache'];
  const options = [...required, 'sdk', 'ndk', 'jdk', 'developer', 'progress-fd'];
  if (process.argv.length === 3 && process.argv[2] === '--help') {
    console.log('usage: producer.mjs android|apple ' + required.map(name => '--' + name + ' PATH').join(' ') +
      ' (--sdk PATH --ndk PATH --jdk PATH | --developer PATH [--progress-fd FD])');
    return;
  }
  const { positionals, values } = parseArgs({
    allowPositionals: true, options: Object.fromEntries(options.map(name => [name, { type: 'string' }])),
  });
  require(positionals.length === 1 && ['android', 'apple'].includes(positionals[0]) &&
    required.every(name => values[name]), 'formal platform and all source/tool/work/cache inputs required');
  const [platform] = positionals;
  require(platform === 'android' ? values.sdk && values.ndk && values.jdk && values.developer === undefined :
    values.developer && ['sdk', 'ndk', 'jdk'].every(name => values[name] === undefined), 'platform tools differ');
  let borrowedProgressFd;
  if (values['progress-fd'] !== undefined) {
    borrowedProgressFd = Number(values['progress-fd']);
    require(platform === 'apple' && Number.isSafeInteger(borrowedProgressFd) && borrowedProgressFd > 2,
      'dedicated Apple progress descriptor required');
    require(borrowedProgressFd !== Number(process.env.POLARIS_BUILD_RESERVATION_FD),
      'progress descriptor cannot own a native reservation');
    fstatSync(borrowedProgressFd);
  }
  await withNativeSignals(async signal => {
    const runner = createNativeRunner({ signal, borrowedProgressFd,
      borrowedProgressPhase: borrowedProgressFd === undefined ? undefined : 'component-build-or-revalidate' });
    await execute({ ...values, platform }, { ...runner, borrowedProgressFd });
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { await main(); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
