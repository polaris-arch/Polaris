#!/usr/bin/env node
// Own the Windows release producer and verify the complete static carriers.
import { createHash, createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
import {
  appendFileSync, chmodSync, closeSync, constants, fstatSync, lstatSync, mkdirSync, mkdtempSync,
  openSync, readdirSync, readSync, renameSync, rmSync, statSync, writeFileSync, writeSync, fsyncSync,
} from 'node:fs';
import { isDeepStrictEqual, parseArgs } from 'node:util';
import { homedir } from 'node:os';
import { basename, dirname, isAbsolute, join, parse as parsePath, relative, resolve, toNamespacedPath } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { requireGraph as demand } from './desktop-core/source-graph.mjs';
import { buildPortableZip, inspectBoundNsis, inspectPortableZip, memberKey } from './lib/formal-package-selection.mjs';
import { MAX_LISTING, MAX_STDERR, now, parse, startCustody, streamChild, wire, withHolds } from './lib/windows-package-custody.mjs';

const script = fileURLToPath(import.meta.url);
const root = dirname(dirname(script));
const MAX_FILE = 512 * 1024 ** 2, MAX_TOTAL = 2 * 1024 ** 3, MAX_MEMBERS = 10_000;
const MAX_UNINSTALLER = 64 * 1024 ** 2, EXTRACT_TIMEOUT = 180_000;
const TAURI_CLI_VERSION = '2.12.5';
const BUNDLE_UNK = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK'), BUNDLE_NSS = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_NSS');
const MARKER = 'Polaris portable edition marker - DO NOT DELETE.\n'
  + 'Polaris 便携版标记文件，请勿删除。\n\n'
  + 'This file tells Polaris it is running as the portable (zip) edition, so that\n'
  + "'Check for updates' offers the portable zip instead of the NSIS installer.\n"
  + 'Deleting it makes Polaris think it was installed, and it will then offer an\n'
  + 'installer that creates a SECOND, separate copy of the app.\n';
const CAPTURE_ENV = 'POLARIS_WINDOWS_PACKAGE_CAPTURE', STATE_ENV = 'POLARIS_WINDOWS_PACKAGE_STATE';
const runnerEnv = [CAPTURE_ENV, 'POLARIS_PACKAGE_NODE', 'POLARIS_PACKAGE_SCRIPT', 'POLARIS_PACKAGE_STATE'];
const CAPTURE_READY = 'POLARIS_WINDOWS_PACKAGE_CAPTURE_READY';
const RUNNER_TEXT = '@echo off\r\nsetlocal DisableDelayedExpansion\r\n'
  + '"%POLARIS_PACKAGE_NODE%" "%POLARIS_PACKAGE_SCRIPT%" capture --state "%POLARIS_PACKAGE_STATE%" -- %*\r\nexit /b %ERRORLEVEL%\r\n';
const legal = ['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md'];
const utf8 = new TextDecoder('utf-8', { fatal: true });
const statOptions = { bigint: true };
const file = path => ({ path, kind: 'file' });
const directory = (path, missing = false) => ({ path, kind: 'directory', ...(missing && { missing }) });
const targetApp = path => ({ path, kind: 'app' });
const tool = path => ({ path, kind: 'tool' });
const pin = bytes => ({ bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
const identical = (left, right) => isDeepStrictEqual(left, right)
  || wire(left) === wire(right); // JSON integer value is independent of its in-memory Number/BigInt representation.
const posixName = path => path.replaceAll('\\', '/');
const samePath = (left, right) => relative(left, right) === '';

function requireSame(left, right, message) {
  demand(identical(left, right), message);
}

function stat(path) {
  try { return lstatSync(toNamespacedPath(path), statOptions); }
  catch (cause) { if (cause.code !== 'ENOENT') throw cause; return null; }
}

function below(base, path) {
  const name = relative(base, path);
  return !isAbsolute(name) && name !== '..' && !name.startsWith('..\\') && !name.startsWith('../');
}

function ancestors(path) {
  const result = [];
  for (let parent = dirname(path); parent !== path; path = parent, parent = dirname(path)) result.push(parent);
  return result;
}

function spelling(path) {
  demand(isAbsolute(path) && !path.split(/[\\/]/).includes('..'), 'absolute canonical Cargo directory required');
  const prefix = parsePath(path).root;
  for (const part of path.slice(prefix.length).split(/[\\/]/).filter(Boolean)) memberKey(part);
  return path;
}

function names(rows) {
  const seen = new Set();
  demand(rows.length <= MAX_MEMBERS, 'member count budget exceeded');
  for (const name of rows) {
    const key = memberKey(name);
    demand(!seen.has(key), 'duplicate/alias member: ' + name);
    seen.add(key);
  }
  for (const key of seen) {
    const segments = key.split('/');
    for (let index = 1; index < segments.length; index++) {
      demand(!seen.has(segments.slice(0, index).join('/')), 'file/directory alias collision');
    }
  }
}

export function expectedWindowsApp(source, kind, version) {
  demand(version === TAURI_CLI_VERSION && ['nsis', 'zip'].includes(kind), 'unreviewed Tauri CLI app transform');
  if (kind === 'zip') return source;
  const offset = source.indexOf(BUNDLE_UNK);
  demand(offset >= 0 && source.indexOf(BUNDLE_UNK, offset + 1) < 0, 'NSIS source requires one unique unpatched bundle token');
  return Buffer.concat([source.subarray(0, offset), BUNDLE_NSS, source.subarray(offset + BUNDLE_UNK.length)]);
}

async function identity(custody, input) {
  const [result] = await custody.inspect([input]);
  return result;
}

function sameRead(left, right, sameView = true) {
  return left.dev === right.dev && left.ino === right.ino && left.nlink === right.nlink
    && left.size === right.size && left.mtimeNs === right.mtimeNs
    && (!sameView || left.ctimeNs === right.ctimeNs && left.mode === right.mode);
}

// Hash the actual descriptor once. Native binding covers the full object ID,
// while comparisons between libuv path and fd views catch mid-read replacement.
async function readPinned(custody, input, { capture = false, before, copy } = {}) {
  const path = toNamespacedPath(input.path);
  before ??= await identity(custody, input);
  const pathStat = input.kind === 'tool' ? statSync : lstatSync;
  const initial = pathStat(path, statOptions);
  const fd = openSync(path, constants.O_RDONLY);
  let output;
  try {
    const opened = fstatSync(fd, statOptions);
    demand(opened.isFile() && opened.size === before[4] && sameRead(opened, initial, false),
      'Windows package file changed before read: ' + input.path);
    const hash = createHash('sha256'), buffer = Buffer.allocUnsafe(1024 * 1024);
    const raw = capture ? Buffer.allocUnsafe(Number(opened.size)) : null;
    if (copy) output = openSync(toNamespacedPath(copy), 'wx');
    let at = 0;
    for (let size; (size = readSync(fd, buffer));) {
      demand(at + size <= opened.size, 'Windows package file grew during read');
      const chunk = buffer.subarray(0, size);
      hash.update(chunk);
      if (raw) chunk.copy(raw, at);
      if (output !== undefined) {
        for (let offset = 0; offset < size;) offset += writeSync(output, chunk, offset, size - offset);
      }
      at += size;
    }
    if (output !== undefined) fsyncSync(output);
    demand(at === Number(opened.size) && sameRead(opened, fstatSync(fd, statOptions))
      && sameRead(initial, pathStat(path, statOptions)), 'Windows package file changed during read');
    requireSame(await identity(custody, input), before, 'Windows package file identity changed during read');
    return { identity: before, facts: { bytes: at, sha256: hash.digest('hex') }, raw };
  } finally {
    try { if (output !== undefined) closeSync(output); }
    finally { closeSync(fd); }
  }
}

async function walk(custody, path) {
  const pending = [path], inputs = [];
  while (pending.length) {
    const where = pending.pop();
    await identity(custody, directory(where));
    for (const entry of readdirSync(toNamespacedPath(where), { withFileTypes: true })) {
      const path = join(where, entry.name);
      demand(!entry.isSymbolicLink(), 'directory link/reparse alias');
      if (entry.isDirectory()) pending.push(path);
      else inputs.push(file(path));
      demand(inputs.length <= MAX_MEMBERS, 'member count budget exceeded');
    }
  }
  const observed = await custody.inspect(inputs);
  const rows = Object.create(null);
  let total = 0;
  for (const [index, input] of inputs.entries()) {
    const bytes = observed[index][4];
    total += Number(bytes);
    demand(total <= MAX_TOTAL, 'payload budget exceeded');
    const { facts } = await readPinned(custody, input, { before: observed[index] });
    rows[posixName(relative(path, input.path))] = facts;
  }
  names(Object.keys(rows));
  return rows;
}

function bindFiles(entries) {
  demand(Array.isArray(entries), 'resource declaration list required');
  for (const name of legal) demand(entries.filter(entry => entry === '../' + name).length === 1,
    'required legal resource must occur once: ' + name);
  return entries.map(entry => {
    demand(typeof entry === 'string' && !/[*?]/.test(entry)
      && (entry === 'core-manifest.json' || entry.startsWith('../') && !entry.slice(3).startsWith('../')),
    'unreviewed resource mapping');
    const path = resolve(root, 'src-tauri', entry);
    demand(below(root, path), 'resource escaped repository');
    return [path, entry.replace(/^\.\.\//, '_up_/').replace(/\/$/, '')];
  });
}

async function output(argv, { cwd = root, limit = MAX_LISTING, timeout = 30_000, ...options } = {}) {
  const chunks = [];
  await streamChild(argv, chunk => chunks.push(chunk), limit, { cwd, deadline: now() + timeout, ...options });
  return Buffer.concat(chunks);
}

function forwardStdout(block, signal) {
  signal.throwIfAborted();
  const stdout = process.stdout;
  return new Promise((resolve, reject) => {
    const abort = () => reject(signal.reason);
    const failed = error => {
      signal.removeEventListener('abort', abort);
      reject(error);
    };
    stdout.once('error', failed);
    signal.addEventListener('abort', abort, { once: true });
    try {
      stdout.write(block, error => {
        // A failed write calls back before emitting its stream error.
        if (error) { failed(error); return; }
        stdout.removeListener('error', failed);
        signal.removeEventListener('abort', abort);
        resolve();
      });
    } catch (error) {
      stdout.removeListener('error', failed);
      failed(error);
    }
  });
}

export function selectTauriEntry(paths) {
  return paths.find(path => /\.(?:cmd|exe)$/i.test(path) && isAbsolute(path)
    && statSync(toNamespacedPath(path), { throwIfNoEntry: false })?.isFile());
}

async function locate(name, fallback, select = ([path]) => path) {
  const where = join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'where.exe');
  try {
    const paths = utf8.decode(await output([where, name], { limit: 1024 * 1024 })).trim().split(/\r?\n/);
    return select(paths) || fallback;
  } catch (cause) {
    if (!String(cause.message).startsWith('owned child failed (1):')) throw cause;
  }
  return fallback;
}

export function forPackage(custody) {
  const facts = async path => (await readPinned(custody, file(path))).facts;
  const jsonFile = async path => parse(utf8.decode((await readPinned(custody, file(path), { capture: true })).raw));
  const tree = path => walk(custody, path);
  const windowsConf = () => jsonFile(join(root, 'src-tauri/tauri.windows.conf.json'));
  const version = async () => (await jsonFile(join(root, 'src-tauri/tauri.conf.json'))).version;
  const git = (...args) => output(['git', '-C', root, ...args]);
  const candidate = async () => utf8.decode(await git('rev-parse', 'HEAD')).trim();
  const dirId = async path => (await identity(custody, directory(path))).slice(0, 2);
  const cliTool = async path => {
    path = resolve(path);
    const got = await readPinned(custody, tool(path));
    return { path, identity: got.identity, sha256: got.facts.sha256 };
  };

  async function verifiedSources() {
    await output([process.execPath, join(root, 'scripts/prepare-core-identity.mjs'), '--platform=win', '--mode=verify'], { timeout: EXTRACT_TIMEOUT });
    const config = await windowsConf();
    const resources = Object.create(null), sources = Object.create(null);
    for (const [path, target] of bindFiles(config.bundle.resources)) {
      const members = stat(path)?.isDirectory() ? await tree(path) : { '': await facts(path) };
      for (const [relative, value] of Object.entries(members)) {
        const name = relative ? target + '/' + relative : target;
        demand(!Object.hasOwn(resources, name), 'resource declaration collision');
        resources[name] = value; sources[name] = join(path, relative);
      }
    }
    names(Object.keys(resources));
    for (const name of ['core-manifest.json', ...legal.map(name => '_up_/' + name),
      ...['core-manifest.json', 'polaris-helper.exe', 'polaris-helper-setup.exe', 'sing-box.exe', 'libcronet.dll']
        .map(name => '_up_/resources/win/' + name)]) {
      demand(resources[name]?.bytes > 0, 'required source missing: ' + name);
    }
    demand(Object.values(resources).reduce((sum, row) => sum + row.bytes, 0) <= MAX_TOTAL, 'source inventory total budget exceeded');
    return { resources, sources };
  }

  async function cargoContext(target, frozen) {
    const executable = process.env.CARGO || await locate('cargo');
    demand(executable, 'normal Cargo executable missing');
    const toolPin = await cliTool(executable);
    const manifest = join(root, 'src-tauri/Cargo.toml');
    const inputs = {};
    for (const path of [join(root, 'Cargo.toml'), join(root, 'Cargo.lock'), manifest]) inputs[path] = await facts(path);
    const home = process.env.CARGO_HOME || join(homedir(), '.cargo');
    for (const parent of [join(root, 'src-tauri'), root, ...ancestors(root), home]) {
      for (const name of ['config', 'config.toml']) {
        const path = parent === home ? join(parent, name) : join(parent, '.cargo', name);
        if (stat(path)) inputs[path] = await facts(path);
      }
    }
    const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => /^(CARGO_PROFILE_|CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_)/.test(name)
      || ['CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN',
        'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'TAURI_CONFIG'].includes(name)));
    const hash = createHash('sha256');
    await streamChild(['git', '-C', root, 'diff', '--binary', 'HEAD', '--'], block => hash.update(block), Number.MAX_SAFE_INTEGER,
      { cwd: root, deadline: now() + 30_000, diagnostics: false });
    const sourceDiffSha256 = hash.digest('hex');
    const common = { tool: toolPin, inputs, sourceDiffSha256, environment };
    if (frozen) {
      demand(target === frozen.target, 'foreign readback target');
      return { ...frozen, ...common };
    }
    const cargo = async (args, cwd = root) => utf8.decode(await output([toolPin.path, ...args], { cwd, cargo: true }));
    const releaseVersion = await cargo(['-vV']);
    demand(releaseVersion.startsWith('cargo ') && /^host: x86_64-pc-windows-msvc\r?$/m.test(releaseVersion),
      'native x64 Windows Cargo required');
    target ||= process.env.CARGO_BUILD_TARGET || null;
    demand(target === null || target === 'x86_64-pc-windows-msvc', 'only the actual native Windows target is supported');
    const metadata = parse(await cargo(['metadata', '--no-deps', '--format-version=1', '--locked', '--offline',
      '--manifest-path', manifest], join(root, 'src-tauri')));
    const packages = metadata.packages.filter(row => samePath(row.manifest_path, manifest));
    demand(samePath(metadata.workspace_root, root) && packages.length === 1, 'foreign Cargo workspace/manifest');
    const [pkg] = packages;
    demand(pkg.name === 'polaris' && metadata.workspace_members.includes(pkg.id), 'foreign Cargo package');
    demand(pkg.version === await version(), 'Cargo/App version differs');
    const binaries = pkg.targets.filter(row => row.name === 'polaris' && identical(row.kind, ['bin']));
    const source = join(root, 'src-tauri/src/main.rs');
    demand(binaries.length === 1 && samePath(binaries[0].src_path, source), 'foreign Cargo binary target');
    await identity(custody, file(source));
    const targetDirectory = spelling(metadata.target_directory);
    demand(!below(targetDirectory, root), 'Cargo target overlaps source root');
    await identity(custody, directory(targetDirectory, true));
    const configured = process.env.CARGO_TARGET_DIR;
    if (configured) {
      // Walk the configured spelling before resolving "..", which could
      // erase a reparse ancestor from the eventual Cargo metadata path.
      const prefix = parsePath(configured).root;
      let path = prefix || join(root, 'src-tauri');
      demand(!prefix || isAbsolute(prefix), 'unsupported configured Cargo target prefix');
      for (const segment of configured.slice(prefix.length).split(/[\\/]/).filter(Boolean)) {
        path = resolve(path, segment);
        await identity(custody, directory(path, true));
      }
    }
    const releaseDirectory = join(targetDirectory, ...[target, 'release'].filter(Boolean));
    requireSame(await cliTool(toolPin.path), toolPin, 'Cargo tool changed during metadata');
    return { ...common, version: releaseVersion.replaceAll('\r\n', '\n'), packageId: pkg.id, manifest,
      binarySource: binaries[0].src_path, targetDirectory, target, releaseDirectory, profile: 'release' };
  }

  async function prepare() {
    const sevenzip = await locate('7zz') ?? await locate('7z', join(process.env.ProgramFiles ?? 'C:\\Program Files', '7-Zip/7z.exe'));
    const info = utf8.decode(await output([sevenzip, 'i', '-sccUTF-8'], { limit: 1024 * 1024 }));
    const sevenzipVersion = /7-Zip(?: \([^\n]*\))? ([0-9]+\.[0-9]+)/.exec(info)?.[1];
    demand(sevenzipVersion && Number(sevenzipVersion.split('.')[0]) >= 24 && /\bNsis\b/i.test(info), '7-Zip >=24 with the NSIS reader required');
    const source = await verifiedSources();
    const cargo = await cargoContext(process.env.POLARIS_WINDOWS_PACKAGE_TARGET);
    const release = spelling(cargo.releaseDirectory);
    mkdirSync(toNamespacedPath(release), { recursive: true });
    const releaseIdentity = await dirId(release);
    demand(releaseIdentity[0] === (await dirId(root))[0], 'Cargo release/publication must share a filesystem for atomic retention');
    const owner = mkdtempSync(join(release, '.polaris-package-'));
    const state = { schema: 'polaris-windows-package-input-v2', owner, root, cargo,
      targetIdentity: await dirId(cargo.targetDirectory), releaseIdentity, ownerIdentity: await dirId(owner),
      sevenzip, sevenzipVersion, ...source, version: await version(), candidate: await candidate() };
    let retained = 0;
    for (const [name, path] of [['nsis', join(release, 'bundle/nsis')], ['dist-win', join(root, 'dist-win')],
      ['evidence', join(root, 'windows-package-evidence')]]) {
      if (stat(path)) {
        retained += Object.values(await tree(path)).reduce((sum, row) => sum + row.bytes, 0);
        demand(retained <= MAX_TOTAL, 'combined old output retention budget exceeded');
        renameSync(toNamespacedPath(path), toNamespacedPath(join(owner, 'previous-' + name)));
      }
      mkdirSync(toNamespacedPath(path), { recursive: true });
    }
    const statePath = join(owner, 'state.json');
    writeFileSync(toNamespacedPath(statePath), wire({ ...state, retainedPreviousOutputBytes: retained }, 2) + '\n', { flag: 'wx' });
    if (process.env.GITHUB_ENV) appendFileSync(toNamespacedPath(process.env.GITHUB_ENV), STATE_ENV + '=' + statePath + '\n');
    console.log(STATE_ENV + '=' + statePath);
    console.log('static NSIS reader:', sevenzip, sevenzipVersion, '; fresh output namespaces; frozen resources:', Object.keys(state.resources).length);
  }

  async function loadState(readback = false, config) {
    const path = process.env[STATE_ENV];
    demand(path && isAbsolute(path), 'owned package state required');
    const state = await jsonFile(path);
    demand(state.schema === 'polaris-windows-package-input-v2' && state.cargo && state.root === root, 'foreign package state');
    const expected = config ? { ...state.cargo, environment: { ...state.cargo.environment, TAURI_CONFIG: config } } : state.cargo;
    const context = await cargoContext(expected.target, readback ? expected : undefined);
    const release = spelling(context.releaseDirectory);
    demand(samePath(dirname(path), state.owner) && samePath(dirname(state.owner), release)
      && basename(state.owner).startsWith('.polaris-package-'), 'foreign package state');
    requireSame(expected, context, 'Cargo context changed');
    requireSame(await dirId(context.targetDirectory), state.targetIdentity, 'Cargo target identity changed');
    requireSame(await dirId(release), state.releaseIdentity, 'Cargo release identity changed');
    requireSame(await dirId(state.owner), state.ownerIdentity, 'owned namespace changed');
    requireSame(await verifiedSources(), { resources: state.resources, sources: state.sources }, 'source resources changed after pre-build freeze');
    demand(await candidate() === state.candidate && await version() === state.version, 'candidate or App version changed during bundling');
    return state;
  }

  async function cargoApp(state) {
    const path = join(state.cargo.releaseDirectory, 'polaris.exe'), request = targetApp(path);
    const value = await identity(custody, request);
    if (value[3] === 2n) {
      const aliases = [];
      const build = join(dirname(path), 'build/polaris');
      for (const unit of readdirSync(toNamespacedPath(build))) {
        const alias = targetApp(join(build, unit, 'out/polaris.exe'));
        const aliasIdentity = await identity(custody, { ...alias, missing: true });
        if (aliasIdentity?.[0] === value[0] && aliasIdentity[1] === value[1]) aliases.push(aliasIdentity);
      }
      demand(aliases.length === 1 && aliases[0][3] === 2n, 'unexpected Cargo App hardlink');
    }
    return { path, request, identity: value };
  }

  async function compiledApp(state, staging) {
    const app = await cargoApp(state);
    const observed = await readPinned(custody, app.request, { before: app.identity, capture: true });
    const selected = (await readPinned(custody, file(join(root, 'resources/win/core-manifest.json')), { capture: true })).raw;
    const offset = observed.raw.indexOf(selected);
    demand(offset >= 0 && observed.raw.indexOf(selected, offset + 1) < 0, 'App must embed the exact frozen selected manifest once');
    let path = app.path;
    if (staging) {
      path = join(staging, 'polaris.exe');
      writeFileSync(toNamespacedPath(path), observed.raw, { flag: 'wx', flush: true });
      requireSame(await facts(path), observed.facts, 'private compiled App snapshot differs');
    }
    requireSame((await cargoApp(state)).identity, app.identity, 'Cargo App path changed while reading');
    return { ...observed, path };
  }

  async function tauri() {
    const version = utf8.decode((await readPinned(custody, file(join(root, 'scripts/tauri-cli.version')), { capture: true })).raw).trim();
    const entry = await locate('tauri', undefined, selectTauriEntry);
    demand(entry, 'normal Tauri CLI entry is missing from PATH');
    const actual = utf8.decode(await output([entry, '--version'])).trim();
    demand(version === TAURI_CLI_VERSION && actual === 'tauri-cli ' + version, 'unreviewed App transform tool');
    return { entry, version: actual, tool: await cliTool(entry) };
  }

  return { prepare, loadState, cargoApp, compiledApp, tauri, facts, jsonFile, tree, windowsConf, cliTool, dirId };
}

function sourceKey(consumer, physical) {
  demand(isAbsolute(consumer) && isAbsolute(physical)
    && parsePath(consumer).root.toLowerCase() === parsePath(physical).root.toLowerCase(),
  'native resource stage must share the consumer drive/share anchor');
  const name = relative(consumer, physical);
  demand(!isAbsolute(name) && !/[*?]/.test(name) && resolve(consumer, name).toLowerCase() === physical.toLowerCase(),
    'invalid relative native resource source');
  return name;
}

async function derivedConfig(api, stage) {
  const config = await api.windowsConf(), consumer = join(root, 'src-tauri');
  const entries = bindFiles(config.bundle.resources);
  config.bundle.resources = Object.fromEntries(entries.map(([, target], i) => [sourceKey(consumer, join(stage, String(i))), target]));
  demand(new Set(Object.values(config.bundle.resources)).size === entries.length, 'duplicate native resource declaration');
  return config;
}

function derivedMembers(stage, config, state) {
  const consumer = join(root, 'src-tauri');
  const members = Object.create(null), directories = new Set([stage, ...ancestors(stage)]);
  for (const [short, target] of Object.entries(config.bundle.resources)) {
    const base = resolve(consumer, short);
    demand(dirname(base) === stage && sourceKey(consumer, base) === short, 'native resource source escaped its fixed stage');
    if (!Object.hasOwn(state.sources, target)) directories.add(base);
    for (const name of Object.keys(state.sources)) {
      if (name !== target && !name.startsWith(target + '/')) continue;
      demand(!Object.hasOwn(members, name), 'ambiguous native resource role');
      const path = name === target ? base : join(base, name.slice(target.length + 1));
      demand(below(stage, path), 'derived resource escaped its owner');
      members[name] = path;
      for (const parent of ancestors(path)) if (below(stage, parent)) directories.add(parent);
    }
  }
  demand(Object.keys(members).length === Object.keys(state.resources).length, 'native resource role member set differs');
  return { members, directories: [...directories] };
}

async function selectResourceStage(api, state) {
  const paths = derivedMembers(root, await derivedConfig(api, root), state).members;
  for (const parent of ancestors(root)) {
    if (parent.length + 12 >= 260) continue;
    await api.dirId(parent);
    let stage;
    try { stage = mkdtempSync(join(parent, 'r-')); }
    catch (cause) { if (!['EACCES', 'EPERM'].includes(cause.code)) throw cause; continue; }
    if (Object.values(paths).every(path => join(stage, relative(root, path)).length < 260)) return stage;
    rmSync(stage, { recursive: true });
  }
  throw new Error('no short writable native resource stage on the consumer drive/share');
}

async function verifyCopies(custody, api, state, copies, plan) {
  requireSame(Object.keys(copies.members).sort(), Object.keys(plan.members).sort(), 'derived resource member set differs');
  requireSame(Object.keys(copies.directories).sort(), [...plan.directories].sort(), 'derived resource directory set differs');
  const directories = await custody.inspect(plan.directories.map(path => directory(path)));
  for (const [i, path] of plan.directories.entries()) {
    requireSame(copies.directories[path], directories[i].slice(0, 2), 'derived resource ancestor changed');
  }
  const tree = await api.tree(copies.root), expected = Object.create(null);
  for (const [name, path] of Object.entries(plan.members)) {
    const [original, staged] = await custody.inspect([file(state.sources[name]), file(path)]);
    requireSame(copies.members[name], { sourceID: original.slice(0, 2), path, copyID: staged.slice(0, 2) },
      'derived resource identity changed');
    expected[posixName(relative(copies.root, path))] = state.resources[name];
  }
  requireSame(tree, expected, 'derived resource bytes changed');
}

export function releaseArguments(state, args) {
  demand(args[0] === 'build', 'only the normal Tauri release build is supported');
  const flags = [], values = {};
  for (let i = 1; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--features' || arg === '--target') {
      demand(!Object.hasOwn(values, arg) && i + 1 < args.length, 'duplicate or missing Cargo value');
      values[arg] = args[++i];
    } else flags.push(arg);
  }
  demand(identical(flags.sort(), ['--bins', '--locked', '--message-format=json', '--offline', '--release'])
    && values['--features'] === 'tauri/custom-protocol' && (values['--target'] ?? null) === state.cargo.target,
  'foreign Cargo release arguments/features/target');
}

async function releaseEvent(api, state, text) {
  const events = text.split(/\r?\n/).filter(line => line.trim()).map(line => parse(line));
  demand(events.length > 0 && events.length <= MAX_MEMBERS
    && events.every(row => row && typeof row === 'object' && !Array.isArray(row)), 'bounded Cargo JSON events required');
  const artifacts = events.filter(row => row.reason === 'compiler-artifact'
    && row.target?.name === 'polaris' && identical(row.target.kind, ['bin']));
  const success = { reason: 'build-finished', success: true };
  demand(artifacts.length === 1 && identical(events.filter(row => row.reason === 'build-finished'), [success])
    && identical(events.at(-1), success), 'unique successful Cargo App artifact required');
  const artifact = artifacts[0], profile = artifact.profile, expected = await api.cargoApp(state);
  demand(artifact.package_id === state.cargo.packageId && samePath(artifact.manifest_path, state.cargo.manifest)
    && samePath(artifact.target.src_path, state.cargo.binarySource) && samePath(artifact.executable, expected.path)
    && artifact.filenames.includes(expected.path) && profile.test === false && profile.debug_assertions === false
    && ['1', '2', '3', 's', 'z'].includes(profile.opt_level), 'foreign/debug Cargo App artifact');
  return { artifact, expected };
}

function writeSigned(path, data, key) {
  const raw = wire({ data, mac: createHmac('sha256', key).update(wire(data)).digest('hex') });
  const temporary = path + '.tmp';
  demand(!stat(path), 'capture record is single use');
  writeFileSync(toNamespacedPath(temporary), raw, { flag: 'wx', flush: true });
  renameSync(toNamespacedPath(temporary), toNamespacedPath(path));
}

async function readSigned(custody, path, key) {
  const before = await identity(custody, file(path));
  demand(before[4] <= BigInt(MAX_LISTING), 'bounded capture record required');
  const { data, mac } = parse(utf8.decode((await readPinned(custody, file(path), { before, capture: true })).raw));
  const expected = createHmac('sha256', key).update(wire(data)).digest();
  demand(typeof mac === 'string' && /^[a-f0-9]{64}$/.test(mac) && timingSafeEqual(Buffer.from(mac, 'hex'), expected),
    'missing, foreign or modified live producer evidence');
  return data;
}

async function capture(custody, api, nativeProgram, args) {
  demand(typeof process.env[CAPTURE_ENV] === 'string', 'missing live producer scope');
  const { key: mac, ...scope } = parse(process.env[CAPTURE_ENV]);
  const key = Buffer.from(mac, 'hex');
  demand(key.length === 32 && now() < scope.deadline && identical(await custody.birth(scope.parent), scope.parentBirth),
    'producer scope expired');
  demand(process.env[STATE_ENV] === scope.statePath, 'input state changed');
  requireSame(await api.facts(scope.statePath), scope.stateFile, 'input state changed');
  const configText = process.env.TAURI_CONFIG;
  const state = await api.loadState(false, configText);
  releaseArguments(state, args);
  const proof = join(state.owner, 'resource-copies.json'), config = join(state.owner, 'native-windows.conf.json');
  const copies = await api.jsonFile(proof), plan = derivedMembers(copies.root, await derivedConfig(api, copies.root), state);
  const verified = async () => {
    demand(now() < scope.deadline && identical(await custody.birth(scope.parent), scope.parentBirth), 'producer scope expired');
    requireSame(await api.facts(script), scope.script, 'capture implementation changed');
    requireSame(await api.cliTool(process.execPath), scope.node, 'capture Node tool changed');
    requireSame(await api.cliTool(nativeProgram), scope.native, 'capture native owner changed');
    requireSame(await api.facts(proof), scope.copies, 'derived resource proof changed');
    requireSame(await api.facts(config), scope.config, 'native Tauri input configuration changed');
    const expectedConfig = await derivedConfig(api, copies.root);
    requireSame(await api.jsonFile(config), expectedConfig, 'native Tauri input configuration changed');
    requireSame(parse(configText), expectedConfig, 'foreign Tauri child configuration');
    demand((process.env.POLARIS_BUILD_ID ?? state.candidate) === state.candidate, 'first producer effective Cargo context changed');
    await verifyCopies(custody, api, state, copies, plan);
  };
  await verified();
  const owner = state.owner, raw = join(owner, 'first-cargo.jsonl'), chunks = [];
  const argv = [state.cargo.tool.path, ...args];
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !runnerEnv.includes(name)
    && name !== STATE_ENV && !/^POLARIS_CMD_ARG_\d+$/.test(name)));
  const fd = openSync(toNamespacedPath(raw), 'wx');
  let transport;
  try {
    transport = await streamChild(argv, (block, signal) => {
      chunks.push(block);
      for (let offset = 0; offset < block.length;) offset += writeSync(fd, block, offset, block.length - offset);
      return forwardStdout(block, signal);
    }, MAX_LISTING, { cwd: join(root, 'src-tauri'), env, deadline: scope.deadline, birth: custody.birth, cargo: true,
      diagnosticContext: { candidate: state.candidate, firstActualChildObserved: true,
        TAURI_CONFIG: { present: true, empty: configText === '', ...pin(Buffer.from(configText)) } } });
    fsyncSync(fd);
  } finally { closeSync(fd); }
  const stdout = await api.facts(raw);
  const { artifact, expected } = await releaseEvent(api, state, utf8.decode(Buffer.concat(chunks)));
  await verified();
  requireSame(await api.loadState(false, configText), state, 'Cargo inputs changed during first build');
  demand(transport.exitCode === 0 && transport.birth !== null && identical(transport.pipeEOF, ['stderr', 'stdout'])
    && transport.stdoutBytes === stdout.bytes && transport.stdoutSha256 === stdout.sha256, 'incomplete first Cargo process/pipe evidence');
  await withHolds(custody, [expected.request], async ([held]) => {
    requireSame(held.identity, expected.identity, 'first Cargo App changed before custody');
    const snapshot = join(owner, 'first-app.bin');
    const original = await readPinned(custody, expected.request, { before: held.identity, copy: snapshot });
    requireSame(await api.facts(snapshot), original.facts, 'first Cargo snapshot differs');
    requireSame((await api.cargoApp(state)).identity, expected.identity, 'first Cargo App changed during original snapshot');
    const record = { scope, argv, artifact, identity: expected.identity, firstApp: original.facts, transport, stdout,
      pid: custody.pid, birth: await custody.birth(custody.pid), handle: held.handle,
      childConfig: { present: true, ...pin(Buffer.from(configText)) } };
    const recordPath = join(owner, 'capture.json'), ack = join(owner, 'capture-ack.json');
    writeSigned(recordPath, record, key);
    await forwardStdout('\n' + CAPTURE_READY + '\n', AbortSignal.timeout(Math.max(0, scope.deadline - now())));
    while (!stat(ack)) {
      demand(now() < scope.deadline, 'live producer did not take App custody');
      await delay(10);
    }
    requireSame(await readSigned(custody, ack, key), { scope, capture: await api.facts(recordPath) },
      'foreign App custody acknowledgement');
  });
}

function carrierExpected(state, raw, kind) {
  const inventory = Object.fromEntries(Object.entries(state.resources)
    .map(([name, row]) => [kind === 'zip' && name.startsWith('_up_/') ? name.slice(5) : name, row]));
  inventory['polaris.exe'] = pin(expectedWindowsApp(raw, kind, TAURI_CLI_VERSION));
  if (kind === 'zip') inventory['portable.marker'] = pin(Buffer.from(MARKER));
  const result = kind === 'nsis' ? Object.fromEntries(Object.entries(inventory).map(([name, row]) => ['$R6/' + name, row])) : inventory;
  names(Object.keys(result));
  return result;
}

async function nsisPlugins(api) {
  const cache = join(process.env.LOCALAPPDATA, 'tauri/NSIS');
  const plugins = join(cache, 'Plugins/x86-unicode'), result = {};
  for (const name of ['System.dll', 'nsDialogs.dll', 'StartMenu.dll', 'NSISdl.dll', 'LangDLL.dll', 'nsExec.dll', 'nsis_tauri_utils.dll']) {
    result['$PLUGINSDIR/' + name] = await api.facts(join(plugins, name === 'nsis_tauri_utils.dll' ? 'additional/' + name : name));
  }
  const conf = (await api.jsonFile(join(root, 'src-tauri/tauri.conf.json'))).bundle.windows.nsis;
  demand(conf.headerImage === null && conf.sidebarImage === null && conf.template === null, 'unreviewed NSIS UI resource mapping');
  result['$PLUGINSDIR/modern-wizard.bmp'] = await api.facts(join(cache, 'Contrib/Graphics/Wizard/win.bmp'));
  return result;
}

export async function inspectNsisCarriers(custody, api, tool, archive, expected, scratch, auxiliary) {
  auxiliary = { ...auxiliary, '$PLUGINSDIR/polaris-helper-setup.exe': expected['$R6/_up_/resources/win/polaris-helper-setup.exe'] };
  const cleaner = '$PLUGINSDIR/polaris-helper-setup.exe', uninstaller = 'uninstall.exe';
  demand(auxiliary[cleaner], 'installer cleaner must bind to the frozen application input');
  const potential = { ...expected, ...auxiliary }, budget = { bytes: 0 };
  const allowed = Object.fromEntries(Object.entries(potential).map(([name, row]) => [name, row.bytes]));
  allowed[uninstaller] = MAX_UNINSTALLER;
  const options = { custody, budget, facts: api.facts, tree: api.tree };
  const { members, listing } = await inspectBoundNsis(tool, archive, join(scratch, 'installer'), allowed, potential, options);
  demand(members[uninstaller] && members[cleaner], 'required NSIS installer member absent');
  requireSame(Object.fromEntries(Object.entries(members).filter(([name]) => Object.hasOwn(expected, name))), expected,
    'complete final NSIS payload differs');
  const nested = await inspectBoundNsis(tool, join(scratch, 'installer', uninstaller), join(scratch, 'uninstaller'),
    Object.fromEntries(Object.entries(auxiliary).map(([name, row]) => [name, row.bytes])), auxiliary, options);
  requireSame(nested.members[cleaner], auxiliary[cleaner], 'embedded uninstall cleaner differs');
  return { members, uninstallerMembers: nested.members, listing, uninstallerListing: nested.listing,
    streamedBytesIncludingNestedUninstaller: budget.bytes,
    extractionLimits: { memberBytes: MAX_FILE, aggregateBytes: MAX_TOTAL, uninstallerBytes: MAX_UNINSTALLER,
      listingBytes: MAX_LISTING, stderrBytes: MAX_STDERR, perUnpackSeconds: EXTRACT_TIMEOUT / 1000 } };
}

async function generatedScript(custody, api, state, sources) {
  requireSame(Object.keys(sources).sort(), Object.keys(state.sources).sort(), 'NSIS source member set differs');
  const path = join(state.cargo.releaseDirectory, 'nsis/x64/installer.nsi');
  const script = await readPinned(custody, file(path), { capture: true });
  const text = utf8.decode(script.raw);
  const define = name => {
    const matches = [...text.matchAll(new RegExp('^!define ' + name + ' "([^"\\r\\n]+)"\\s*$', 'gm'))];
    demand(matches.length === 1, 'generated NSIS define must be unique: ' + name);
    return matches[0][1];
  };
  demand(define('VERSION') === state.version && define('MAINBINARYNAME') === 'polaris', 'NSIS source/version differs');
  demand(samePath(define('MAINBINARYSRCPATH'), (await api.cargoApp(state)).path), 'NSIS main input differs');
  const sections = [...text.matchAll(/^Section Install\s*$(.*?)^SectionEnd\s*$/gms)];
  demand(sections.length === 1, 'unique normal NSIS install section required');
  const section = sections[0][1], mapped = Object.create(null);
  demand([...section.matchAll(/^\s*File "\$\{MAINBINARYSRCPATH\}"\s*$/gm)].length === 1, 'unique NSIS App File required');
  const rows = [...section.matchAll(/^\s*File \/a "\/oname=([^"\r\n]+)" "([^"\r\n]+)"\s*$/gm)];
  for (const [, raw, source] of rows) {
    const name = posixName(raw);
    demand(!Object.hasOwn(mapped, name) && Object.hasOwn(sources, name), 'extra/duplicate NSIS File mapping: ' + name);
    const [actual, expected] = await custody.inspect([file(source), file(sources[name])]);
    requireSame(actual.slice(0, 2), expected.slice(0, 2), 'NSIS File source differs: ' + name);
    mapped[name] = (await readPinned(custody, file(source), { before: actual })).facts;
  }
  requireSame(mapped, state.resources, 'generated NSIS resource facts differ');
  demand([...section.matchAll(/^\s*File\b/gm)].length === rows.length + 1, 'unparsed NSIS File instruction');
  return script.facts;
}

async function assembleCarriers(custody, api, state, app, source, sources) {
  const cli = await api.tauri(), generatedNSISScript = await generatedScript(custody, api, state, sources);
  const folder = join(state.cargo.releaseDirectory, 'bundle/nsis');
  const candidates = readdirSync(toNamespacedPath(folder)).filter(name => /-setup\.exe$/i.test(name));
  const stem = 'Polaris_' + state.version + '_x64';
  demand(candidates.length === 1 && candidates[0] === stem + '-setup.exe', 'expected unique current raw NSIS candidate');
  const raw = join(folder, candidates[0]), original = await api.facts(raw);
  const nsisExpected = carrierExpected(state, app.raw, 'nsis'), scratch = mkdtempSync(join(state.owner, 'nsis-'));
  let nsisObserved;
  try { nsisObserved = await inspectNsisCarriers(custody, api, state.sevenzip, raw, nsisExpected, scratch, await nsisPlugins(api)); }
  finally { rmSync(toNamespacedPath(scratch), { recursive: true }); }
  requireSame(await api.facts(raw), original, 'NSIS changed during inspection');
  const destination = join(root, 'dist-win');
  demand(readdirSync(toNamespacedPath(destination)).length === 0, 'publication output namespace is not empty');
  const installer = stem + '-win-setup.exe', portable = stem + '-win-Portable.zip';
  renameSync(toNamespacedPath(raw), toNamespacedPath(join(destination, installer)));
  const portableExpected = carrierExpected(state, app.raw, 'zip');
  const { carriers, portableObserved } = await withHolds(custody,
    [directory(dirname(app.path)), file(app.path), directory(destination)], async () => {
      const observation = await buildPortableZip(join(destination, portable), portableExpected, sources, app.path, MARKER);
      await api.loadState();
      requireSame(await api.facts(app.path), app.facts, 'App changed during carrier inspection');
      requireSame((await api.cargoApp(state)).identity, source.identity, 'Cargo App changed during carrier inspection');
      requireSame((await api.compiledApp(state)).facts, state.cargoArtifact.firstApp, 'original-byte-mismatch during carrier inspection');
      requireSame((await api.cargoApp(state)).identity, source.identity, 'Cargo App changed during carrier inspection');
      const carriers = { [installer]: await api.facts(join(destination, installer)), [portable]: await api.facts(join(destination, portable)) };
      requireSame(carriers[installer], original, 'moved NSIS differs from inspected installer');
      requireSame(carriers[portable], observation.artifact, 'portable ZIP changed after inspection');
      return { carriers, portableObserved: observation.observed };
    });
  const receipt = { schema: 'polaris-windows-static-carriers-v1', candidate: state.candidate,
    ownedStage: { path: state.owner, retainedPreviousOutputBytes: state.retainedPreviousOutputBytes,
      retirement: 'preserved old outputs; fresh temporary extracts/staging removed; runner owns final retirement' },
    sevenzip: { path: state.sevenzip, version: state.sevenzipVersion },
    sourceResources: state.resources, sourceApp: app.facts, generatedNSISScript,
    compiledAppInput: { sourcePath: source.path, sourceIdentity: source.identity, privateSingleLinkSnapshot: app.facts },
    tauriCLI: cli, cargoContext: state.cargo, cargoArtifact: state.cargoArtifact, nsisExpected, nsisObserved,
    portableExpected, portableObserved, carriers, nativeInstallation: 'not performed by static carrier gate' };
  writeFileSync(toNamespacedPath(join(root, 'windows-package-evidence/static-carriers.json')), wire(receipt, 2) + '\n', { flag: 'wx' });
  console.log('Actual final NSIS + portable complete member/SHA/size inventories verified');
}

async function produce(custody, api, nativeProgram) {
  const initial = await api.loadState(), cli = await api.tauri(), owner = initial.owner;
  demand(!runnerEnv.some(name => Object.hasOwn(process.env, name)), 'nested or caller-selected capture scope');
  demand(!['runner.cmd', 'resource-copies.json', 'native-windows.conf.json', 'capture.json', 'capture-ack.json',
    'first-cargo.jsonl', 'first-app.bin', 'produced.json'].some(name => stat(join(owner, name))),
  'producer owner is single use; prepare a fresh owner after a known failure');
  const stage = await selectResourceStage(api, initial), held = [];
  try {
    const configValue = await derivedConfig(api, stage), plan = derivedMembers(stage, configValue, initial);
    for (const path of plan.directories) {
      if (below(stage, path) && path !== stage) mkdirSync(toNamespacedPath(path), { recursive: true });
    }
    const directories = await custody.hold(plan.directories.map(path => directory(path)));
    held.push(...directories);
    const copies = { root: stage, directories: Object.fromEntries(plan.directories
      .map((path, i) => [path, directories[i].identity.slice(0, 2)])), members: Object.create(null) };
    for (const [name, path] of Object.entries(plan.members)) {
      const source = file(initial.sources[name]);
      const original = await withHolds(custody, [source], ([{ identity }]) => readPinned(custody, source, { before: identity, copy: path }));
      requireSame(original.facts, initial.resources[name], 'opened source bytes changed during copy');
      const [derived] = await custody.hold([file(path)]);
      held.push(derived);
      requireSame(await api.facts(path), original.facts, 'derived resource copy bytes differ');
      copies.members[name] = { sourceID: original.identity.slice(0, 2), path, copyID: derived.identity.slice(0, 2) };
    }
    await verifyCopies(custody, api, initial, copies, plan);
    const proof = join(owner, 'resource-copies.json'), config = join(owner, 'native-windows.conf.json'), runner = join(owner, 'runner.cmd');
    writeFileSync(toNamespacedPath(proof), wire(copies), { flag: 'wx', flush: true });
    writeFileSync(toNamespacedPath(config), wire(configValue), { flag: 'wx', flush: true });
    writeFileSync(toNamespacedPath(runner), RUNNER_TEXT, { flag: 'wx' });
    held.push(...await custody.hold([proof, config, runner, script, process.env[STATE_ENV], nativeProgram,
      join(root, 'src-tauri/tauri.windows.conf.json')].map(file)));
    const runnerPin = await api.facts(runner), key = randomBytes(32);
    const scope = { parent: process.pid, parentBirth: await custody.birth(process.pid), statePath: process.env[STATE_ENV],
      stateFile: await api.facts(process.env[STATE_ENV]), script: await api.facts(script), node: await api.cliTool(process.execPath),
      native: await api.cliTool(nativeProgram), config: await api.facts(config), copies: await api.facts(proof), deadline: now() + 3600_000 };
    const env = { ...process.env, [CAPTURE_ENV]: wire({ ...scope, key: key.toString('hex') }), POLARIS_PACKAGE_NODE: process.execPath,
      POLARIS_PACKAGE_SCRIPT: script, POLARIS_PACKAGE_STATE: scope.statePath };
    demand(runnerEnv.slice(1).every(name => !/[\r\n\0"]/.test(env[name])) && !/[%\r\n\0"]/.test(runner),
      'unsafe native runner path');
    const args = [cli.entry, 'build', '--config', config, '--bundles', 'nsis', '--runner', runner];
    if (initial.cargo.target) args.push('--target', initial.cargo.target);
    args.push('--', '--locked', '--offline', '--message-format=json');
    let observed, retainedApp, pending = '', discarding = false;
    const ready = async signal => {
      demand(!observed && now() < scope.deadline, 'duplicate or expired first producer');
      requireSame(await api.facts(runner), runnerPin, 'changed first producer runner');
      const recordPath = join(owner, 'capture.json'), rawPath = join(owner, 'first-cargo.jsonl');
      signal.throwIfAborted();
      const record = await readSigned(custody, recordPath, key);
      requireSame(record.scope, scope, 'foreign first producer scope');
      releaseArguments(initial, record.argv.slice(1));
      demand(record.argv[0] === initial.cargo.tool.path, 'foreign Cargo executable');
      signal.throwIfAborted();
      const raw = await readPinned(custody, file(rawPath), { capture: true });
      requireSame(raw.facts, record.stdout, 'foreign first producer output');
      signal.throwIfAborted();
      const { artifact, expected } = await releaseEvent(api, initial, utf8.decode(raw.raw));
      requireSame(artifact, record.artifact, 'first Cargo artifact differs');
      requireSame(expected.identity, record.identity, 'first Cargo App identity differs');
      signal.throwIfAborted();
      const snapshot = join(owner, 'first-app.bin');
      const [snapshotHold] = await custody.hold([file(snapshot)]);
      signal.throwIfAborted();
      held.push(snapshotHold);
      requireSame(await api.facts(snapshot), record.firstApp, 'original-byte-mismatch: authenticated first snapshot differs');
      signal.throwIfAborted();
      const app = await custody.transfer(record.pid, record.birth, record.handle, expected.request);
      signal.throwIfAborted();
      held.push(app);
      retainedApp = app;
      requireSame(app.identity, expected.identity, 'transferred App differs');
      const capture = await api.facts(recordPath);
      signal.throwIfAborted();
      demand(now() < scope.deadline, 'first producer scope expired before custody acknowledgement');
      writeSigned(join(owner, 'capture-ack.json'), { scope, capture }, key);
      observed = record;
    };
    const run = await streamChild(args, async (block, signal) => {
      await forwardStdout(block, signal);
      signal.throwIfAborted();
      for (const [index, line] of block.toString('ascii').split('\n').entries()) {
        if (index !== 0) {
          if (!discarding && pending.replace(/\r$/, '') === CAPTURE_READY) await ready(signal);
          pending = ''; discarding = false;
        }
        if (!discarding) {
          pending += line;
          if (pending.length > 65536) { pending = ''; discarding = true; }
        }
      }
    }, MAX_LISTING, { cwd: root, env, deadline: scope.deadline, birth: custody.birth, diagnosticContext: { candidate: initial.candidate } });
    demand(observed && run.exitCode === 0, 'Tauri did not finish one captured producer');
    requireSame(await api.facts(runner), runnerPin, 'first producer runner changed');
    requireSame(await api.loadState(), initial, 'producer inputs changed');
    requireSame(await api.tauri(), cli, 'Tauri CLI changed');
    await verifyCopies(custody, api, initial, copies, plan);
    const current = await api.cargoApp(initial);
    requireSame(current.identity.slice(0, 2), observed.identity.slice(0, 2), 'Tauri replaced the actual App');
    requireSame(await custody.check(retainedApp, current.request), current.identity, 'retained Cargo App changed');
    const state = { ...initial, cargoArtifact: { argv: observed.argv, artifact: observed.artifact, identity: current.identity,
      firstApp: observed.firstApp, transport: observed.transport, nativeResourceConfig: scope.config,
      resourceCopies: copies, childConfig: observed.childConfig, tauri: run } };
    const compiled = mkdtempSync(join(owner, 'compiled-app-'));
    try {
      const app = await api.compiledApp(state, compiled);
      requireSame(app.facts, observed.firstApp, 'original-byte-mismatch: Tauri did not restore the first Cargo App');
      await assembleCarriers(custody, api, state, app, current,
        Object.fromEntries(Object.entries(copies.members).map(([name, row]) => [name, row.path])));
      await verifyCopies(custody, api, initial, copies, plan);
    } finally { rmSync(toNamespacedPath(compiled), { recursive: true }); }
    const receipt = join(root, 'windows-package-evidence/static-carriers.json'), produced = join(owner, 'produced.json');
    writeFileSync(toNamespacedPath(produced), wire({ schema: 'polaris-windows-produced-v1', candidate: state.candidate,
      receipt: await api.facts(receipt), ownerIdentity: state.ownerIdentity }), { flag: 'wx', flush: true });
    chmodSync(toNamespacedPath(produced), 0o444);
  } finally {
    try { if (held.length) await custody.release(held); }
    finally { rmSync(toNamespacedPath(stage), { recursive: true }); }
  }
}

async function assemble(custody, api) {
  const state = await api.loadState(true), produced = join(state.owner, 'produced.json');
  const receiptPath = join(root, 'windows-package-evidence/static-carriers.json');
  const original = await identity(custody, file(produced));
  demand(!(original[2] & 0o200n), 'mutable completed publication binding');
  const readback = await readPinned(custody, file(receiptPath), { capture: true });
  requireSame(await api.jsonFile(produced), { schema: 'polaris-windows-produced-v1', candidate: state.candidate,
    receipt: readback.facts, ownerIdentity: state.ownerIdentity }, 'foreign or modified publication readback');
  const receipt = parse(utf8.decode(readback.raw)), app = await api.compiledApp(state), current = await api.cargoApp(state);
  demand(receipt.candidate === state.candidate, 'completed publication candidate changed');
  requireSame(receipt.cargoContext, state.cargo, 'completed publication context changed');
  requireSame(receipt.sourceApp, app.facts, 'completed publication App changed');
  requireSame(receipt.cargoArtifact.identity, current.identity, 'completed publication App identity changed');
  const destination = join(root, 'dist-win');
  requireSame(await api.tree(destination), receipt.carriers, 'completed final carriers differ');
  const portable = join(destination, 'Polaris_' + state.version + '_x64-win-Portable.zip');
  await withHolds(custody, [file(portable)], async () => requireSame(
    await inspectPortableZip(portable, carrierExpected(state, app.raw, 'zip')), receipt.portableObserved,
    'completed portable ZIP observations differ'));
  console.log('POLARIS_WINDOWS_PACKAGE_PREVIEW_READBACK ' + wire({ candidate: state.candidate, receipt: readback.facts,
    carriers: receipt.carriers, nsisPayloadMembers: Object.keys(receipt.nsisExpected).length,
    nsisMembers: Object.keys(receipt.nsisObserved.members).length,
    uninstallerMembers: Object.keys(receipt.nsisObserved.uninstallerMembers).length,
    nsisStreamedBytes: receipt.nsisObserved.streamedBytesIncludingNestedUninstaller,
    portableMembers: Object.keys(receipt.portableObserved).length }));
}

async function main(args) {
  const separator = args.indexOf('--');
  const forwarded = separator < 0 ? [] : args.splice(separator).slice(1);
  const { values, positionals } = parseArgs({ args, allowPositionals: true,
    options: { state: { type: 'string' }, target: { type: 'string' } } });
  const [action] = positionals;
  demand(positionals.length === 1 && ['prepare', 'produce', 'capture', 'assemble'].includes(action),
    'expected prepare, produce, capture or assemble');
  demand(action === 'capture' || forwarded.length === 0, 'only the private Cargo runner accepts forwarded arguments');
  if (values.state) {
    demand(action !== 'prepare', '--state requires an owned publication action');
    process.env[STATE_ENV] = values.state;
  }
  if (values.target) {
    demand(action === 'prepare' && values.target === 'x86_64-pc-windows-msvc', '--target is only valid for native Windows prepare');
    process.env.POLARIS_WINDOWS_PACKAGE_TARGET = values.target;
  }
  demand(process.platform === 'win32' && process.arch === 'x64', 'native x64 Windows packaging required');
  const nativeProgram = join(root, 'target/x86_64-pc-windows-msvc/release/polaris-package-custody.exe');
  const custody = startCustody(nativeProgram), api = forPackage(custody);
  try {
    if (action === 'prepare') await api.prepare();
    else if (action === 'produce') await produce(custody, api, nativeProgram);
    else if (action === 'capture') await capture(custody, api, nativeProgram, forwarded);
    else await assemble(custody, api);
  } finally { await custody.close(); }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { await main(process.argv.slice(2)); }
  catch (cause) {
    console.error(cause.message);
    // An unread inherited pipe can keep Node alive after native cleanup.
    if (process.stdout.writableLength) process.exit(1);
    process.exitCode = 1;
  }
}
