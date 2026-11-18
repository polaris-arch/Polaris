#!/usr/bin/env node
// The command chooses a producer operation, never a runtime trust lane.
import { spawn } from 'node:child_process';
import {
  chmodSync, closeSync, constants, existsSync, fstatSync, lstatSync, mkdirSync, mkdtempSync, openSync,
  readFileSync, readSync, realpathSync, renameSync, rmdirSync, rmSync, writeFileSync,
} from 'node:fs';
import { delimiter, isAbsolute, resolve, dirname, join, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { selectedCoreManifest, verifySelectedCoreManifest, actualCoreFile, writeSelectedCoreManifest } from '../lib/core-identity.mjs';
import { portableCanonical } from './release-consumer.mjs';
import { digest, requireGraph as demand } from './source-graph.mjs';
export const LOCAL_MARKER = Buffer.from('POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const compiledSelection = 'crates/core-supervisor/src/selected_core/development_config.rs';
const selectionSchema = 'polaris-local-development-selection-v1';
const contractSchema = 'polaris-local-development-core-contract-v1';
const inputsSchema = 'polaris-local-development-inputs-v1';
const sourceRecipe = 'scripts/desktop-core/local-development-source.json';
const receiptLimit = 2 * 1024 ** 2;
const nativeLimit = 96 * 1024 ** 2;
const sourceId = (value) => typeof value === 'string' && /^[a-f0-9]{40}$/.test(value);
const strictUtf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
export const localDirectory = (base) => join(base, 'resources/.local-development/linux');
const selectionExecutable = (path) => /^(?:polaris|Polaris|polaris-helper)(?:\.exe)?$/.test(path.replaceAll('\\', '/').split('/').at(-1));
export function rejectLocalExecutables(paths) {
  const selected = paths.filter(selectionExecutable);
  demand(selected.length <= 256, 'Distribution executable count exceeds budget');
  for (const input of selected) {
    // Resolve legitimate AppDir links, then retain/read the actual opened object.
    const path = realpathSync(input), before = lstatSync(path);
    demand(before.isFile() && !before.isSymbolicLink() && before.size <= 512 * 1024 ** 2, 'Unsafe distribution executable');
    const fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
    try {
      const opened = fstatSync(fd); demand(opened.dev === before.dev && opened.ino === before.ino, 'Distribution executable changed before scan');
      const block = Buffer.alloc(64 * 1024); let tail = Buffer.alloc(0), total = 0;
      for (let n; (n = readSync(fd, block, 0, block.length, null)) > 0;) {
        total += n; demand(total <= before.size, 'Distribution executable grew during scan');
        const data = Buffer.concat([tail, block.subarray(0, n)]);
        demand(!data.includes(LOCAL_MARKER), 'Formal package/publisher rejects a LocalDevelopmentSelection executable: ' + input);
        tail = data.subarray(Math.max(0, data.length - LOCAL_MARKER.length + 1));
      }
      const after = fstatSync(fd);
      demand(total === before.size && after.size === before.size && after.mtimeMs === before.mtimeMs && after.ctimeMs === before.ctimeMs, 'Distribution executable changed during scan');
    } finally { closeSync(fd); }
  }
}
export function rejectLocalDistribution(base, directory) {
  demand(!existsSync(join(base, 'resources/.local-development')), 'Formal distribution rejects local development compiler inputs');
  if (directory) rejectLocalExecutables(['polaris-helper', 'polaris', 'Polaris', 'polaris-helper.exe', 'polaris.exe', 'Polaris.exe'].map(name => join(directory,name)).filter(existsSync));
}

export function compilerSelectionSource(template, selectionBytes) {
  const { context, row, contractSha256 } = JSON.parse(selectionBytes);
  const pins = {
    source_commit: context.coreSource.commit,
    source_tree: context.coreSource.tree,
    producer_commit: context.producerSource.commit,
    producer_tree: context.producerSource.tree,
    target: row.target,
    go_version: row.goVersion,
    tags: row.tags,
    core_bytes: row.core.bytes,
    core_sha256: row.core.sha256,
    sidecar_bytes: row.sidecar.bytes,
    sidecar_sha256: row.sidecar.sha256,
    selection_sha256: digest(selectionBytes),
    contract_sha256: contractSha256,
  };
  const fields = Object.entries(pins).map(([name, value]) => name + ':' + portableCanonical(value)).join(',');
  return template.replace(/\r\n?/g, '\n').split('pub const COMPILED:', 1)[0] +
    'pub const COMPILED:Option<DevelopmentSelection>=Some(DevelopmentSelection{' + fields + '});\n';
}

// The pinned producer's quantities (member sizes, seals and process results)
// are exact integers. Keep the sorted two-space ASCII encoding of v1 receipts.
const receiptJson = (value) => portableCanonical(value, 2) + '\n';
const closed = (value, keys) => value && typeof value === 'object' && !Array.isArray(value) &&
  Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));

export function localReceipt(template, { recipe, recipeSha256, draft, selection, contract }) {
  const verifying = selection !== undefined;
  let document;
  try {
    document = JSON.parse(verifying ? selection : draft, (_, value, context) => {
      if (typeof value === 'number') {
        demand(Number.isSafeInteger(value) && /^(0|-?[1-9][0-9]*)$/.test(context.source),
          'local receipt quantity is not an exact integer');
      }
      return value;
    });
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    throw new Error('invalid local receipt JSON');
  }
  if (verifying) demand(selection === receiptJson(document), 'local selection is not the exact producer encoding');
  const fields = ['schema', 'configurationSha256', 'context', 'row', 'proof', 'policy'];
  if (verifying) fields.push('providerFingerprint', 'contractSha256');
  demand(closed(document, fields) && document.schema === selectionSchema, 'local selection shape differs');
  const { configurationSha256, context, row, proof, policy } = document;
  demand(configurationSha256 === recipeSha256, 'local compiler configuration changed');
  demand(closed(row, ['version', 'target', 'sourceCommit', 'sourceTree', 'goVersion', 'tags', 'variant',
    'buildGraph', 'rawBuildInfo', 'archive', 'core', 'sidecar', 'finalPackageCronetSmoke']), 'local producer row shape differs');
  demand(row.version === recipe.version && row.goVersion === recipe.goVersion && row.target === recipe.target &&
    row.tags === recipe.tags.join(','), 'local build declaration differs');
  demand(closed(context, ['schema', 'coreSource', 'producerSource', 'host']) &&
    context.schema === 'polaris-local-development-execution-context-v1' &&
    portableCanonical(context.coreSource) === portableCanonical(recipe.coreSource) &&
    portableCanonical(context.producerSource) === portableCanonical(recipe.producerSource) &&
    portableCanonical(context.host) === '{"arch":"amd64","os":"linux"}' &&
    row.sourceCommit === recipe.coreSource.commit && row.sourceTree === recipe.coreSource.tree,
  'local source context differs');
  demand(closed(row.sidecar, ['member', 'bytes', 'sha256']) && row.sidecar.member === 'libcronet.so' &&
    row.sidecar.bytes === recipe.sidecar.bytes && row.sidecar.sha256 === recipe.sidecar.sha256,
  'local sidecar is not the compiled trusted input');

  const providerFingerprint = digest(receiptJson({ configurationSha256, context, row, proof }));
  const { version, sourceCommit, sourceTree, archive, core, sidecar } = row;
  const contractBytes = receiptJson({ schema: contractSchema, version, sourceCommit, sourceTree,
    platforms: { linux: { core, sidecar, archive, sourceCommit, sourceTree, providerFingerprint } } });
  const contractSha256 = digest(contractBytes);
  if (verifying) {
    demand(document.providerFingerprint === providerFingerprint, 'local protected facts/provider differs');
    demand(contract === contractBytes && document.contractSha256 === contractSha256, 'local compiled contract differs');
    demand(template.replace(/\r\n?/g, '\n') === compilerSelectionSource(template, selection),
      'generated facts differ from the immutable Rust compiler configuration');
    return null;
  }
  const selectionBytes = receiptJson({ schema: selectionSchema, configurationSha256, context, row, proof, policy,
    providerFingerprint, contractSha256 });
  return { selection: selectionBytes, contract: contractBytes, compiled: compilerSelectionSource(template, selectionBytes),
    providerFingerprint };
}

function ownedDirectory(path) {
  demand(resolve(path) === path && realpathSync(path) === path, 'Local directory must be absolute and unredirected: ' + path);
  const info = lstatSync(path);
  demand(info.isDirectory() && info.uid === process.getuid(), 'Local directory must be owned: ' + path);
}

function readOwned(path) {
  const before = lstatSync(path, { bigint: true });
  demand(before.isFile() && before.uid === BigInt(process.getuid()) && before.nlink === 1n &&
    before.size > 0n && before.size <= BigInt(receiptLimit), 'Unsafe or oversized owned local input: ' + path);
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const same = info => ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs'].every(key => info[key] === before[key]);
    demand(same(fstatSync(fd, { bigint: true })), 'Local input replaced: ' + path);
    const bytes = Buffer.alloc(Number(before.size));
    for (let offset = 0; offset < bytes.length;) {
      const count = readSync(fd, bytes, offset, bytes.length - offset, null);
      demand(count > 0, 'Local input truncated: ' + path);
      offset += count;
    }
    demand(same(fstatSync(fd, { bigint: true })) && same(lstatSync(path, { bigint: true })), 'Local input changed: ' + path);
    return bytes;
  } finally { closeSync(fd); }
}

function configuration(base) {
  demand(process.platform === 'linux' && process.arch === 'x64', 'Local platform must be Linux amd64');
  const recipePath = join(base, sourceRecipe);
  const bytes = readOwned(recipePath);
  const recipe = JSON.parse(strictUtf8.decode(bytes), (_, value, context) => {
    if (typeof value === 'number') demand(Number.isSafeInteger(value) && /^[1-9][0-9]*$/.test(context.source), 'Invalid local recipe quantity');
    return value;
  });
  demand(closed(recipe, ['schema', 'coreSource', 'producerSource', 'target', 'goVersion', 'tags', 'version', 'sidecar']) &&
    recipe.schema === 'polaris-local-development-source-v1' && recipe.target === 'linux/amd64', 'Local compiler configuration shape differs');
  for (const source of [recipe.coreSource, recipe.producerSource]) {
    demand(closed(source, ['commit', 'tree']) && sourceId(source.commit) && sourceId(source.tree), 'Invalid compiled source pin');
  }
  demand(Array.isArray(recipe.tags) && new Set(recipe.tags).size === recipe.tags.length &&
    recipe.tags.every(tag => typeof tag === 'string' && /^[a-z0-9_]+$/.test(tag)), 'Invalid local tags');
  demand(typeof recipe.version === 'string' && /^[A-Za-z0-9.+-]{1,128}$/.test(recipe.version) && recipe.version.includes('-polaris-dev.') &&
    typeof recipe.goVersion === 'string' && /^go1\.[0-9]+\.[0-9]+$/.test(recipe.goVersion), 'Explicit developer version/toolchain required');
  demand(closed(recipe.sidecar, ['bytes', 'sha256', 'version']) && Number.isSafeInteger(recipe.sidecar.bytes) &&
    recipe.sidecar.bytes > 0 && recipe.sidecar.bytes <= nativeLimit && typeof recipe.sidecar.sha256 === 'string' &&
    /^[a-f0-9]{64}$/.test(recipe.sidecar.sha256) && typeof recipe.sidecar.version === 'string' && recipe.sidecar.version.length > 0,
  'Local sidecar configuration differs');
  return { recipePath, recipe, recipeSha256: digest(bytes) };
}

const formalVariables = ['GITHUB_ACTIONS', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'POLARIS_NATIVE_DNS_CANDIDATE', 'CORE_CANDIDATE'];
function producerEnvironment() {
  const env = {};
  for (const key of ['HOME', 'TMPDIR', 'XDG_CACHE_HOME', 'GOCACHE', 'GOMODCACHE', 'GOPATH']) {
    if (process.env[key]) env[key] = process.env[key];
  }
  env.PATH = (process.env.PATH ?? '').split(delimiter).filter(isAbsolute).join(delimiter);
  demand(env.PATH, 'Local producer requires an absolute toolchain PATH');
  return { ...env, LANG: 'C', LC_ALL: 'C', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_NO_LAZY_FETCH: '1', GIT_NO_REPLACE_OBJECTS: '1', GIT_TERMINAL_PROMPT: '0',
    GOENV: 'off', GOFLAGS: '', GOTOOLCHAIN: 'local', GOWORK: 'off', GOPROXY: 'off', GOSUMDB: 'off',
    GONOPROXY: 'none', GOVCS: '*:off', GOOS: 'linux', GOARCH: 'amd64', CGO_ENABLED: '0', GOMAXPROCS: '2' };
}

function localCommand(program, args, { cwd, env, timeout, outputLimit = receiptLimit }) {
  return new Promise((complete, reject) => {
    const child = spawn(program, args, { cwd, env, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const stdout = [], stderr = [];
    let stdoutBytes = 0, stderrBytes = 0, failure, force, retire, settled = false;
    const signal = name => {
      if (!child.pid) return;
      try { process.kill(-child.pid, name); }
      catch (error) { if (error.code !== 'ESRCH') failure ??= error; }
    };
    const stop = message => {
      if (failure) return;
      failure = new Error(message);
      signal('SIGTERM');
      force = setTimeout(() => signal('SIGKILL'), 5000);
      retire = setTimeout(() => {
        child.stdout.destroy(); child.stderr.destroy(); finish();
      }, 6000);
    };
    const interrupt = () => stop('Local producer interrupted');
    const timer = setTimeout(() => stop('Local command deadline exceeded: ' + program), timeout);
    process.on('SIGINT', interrupt); process.on('SIGTERM', interrupt);
    function finish(status) {
      if (settled) return;
      settled = true;
      clearTimeout(timer); clearTimeout(force); clearTimeout(retire);
      process.off('SIGINT', interrupt); process.off('SIGTERM', interrupt);
      if (child.pid) {
        try {
          process.kill(-child.pid, 0);
          failure ??= new Error('Local command left a live process group');
          signal('SIGKILL');
        } catch (error) { if (error.code !== 'ESRCH') failure ??= error; }
      }
      const diagnostics = Buffer.concat(stderr);
      if (failure || status !== 0) reject(failure ?? new Error('Local ' + program + ' failed (' + status + '): ' + diagnostics.toString('utf8')));
      else complete({ stdout: Buffer.concat(stdout), stderr: diagnostics });
    }
    child.stdout.on('data', chunk => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > outputLimit) stop('Local command stdout budget exceeded');
      else if (!failure) stdout.push(chunk);
    });
    child.stderr.on('data', chunk => {
      stderrBytes += chunk.length;
      if (stderrBytes > receiptLimit) stop('Local command stderr budget exceeded');
      else if (!failure) stderr.push(chunk);
    });
    child.stdout.once('error', () => stop('Local command stdout unavailable'));
    child.stderr.once('error', () => stop('Local command stderr unavailable'));
    child.once('error', error => stop(error.message));
    child.once('close', finish);
  });
}

async function cleanSource(path, expected, env) {
  ownedDirectory(path);
  const git = async (args, outputLimit) => strictUtf8.decode((await localCommand('git', [
    '--no-optional-locks', '-c', 'core.fsmonitor=false', '-c', 'core.untrackedCache=false',
    '-c', 'core.ignoreStat=false', '-c', 'core.filemode=true', '-c', 'core.trustctime=true',
    '-c', 'core.attributesFile=/dev/null', ...args,
  ], { cwd: path, env, timeout: 20_000, outputLimit })).stdout);
  const identity = await git(['rev-parse', '--show-toplevel', 'HEAD', 'HEAD^{tree}']);
  demand(identity === path + '\n' + expected.commit + '\n' + expected.tree + '\n', 'Local source commit/tree differs: ' + path);
  const config = await git(['config', '--name-only', '--null', '--list']);
  demand(!config.split('\0').some(key => key.startsWith('filter.')), 'Local source enables a Git clean filter: ' + path);
  const files = await git(['ls-files', '-v', '-z'], 32 * 1024 ** 2);
  demand(files.endsWith('\0') && files.slice(0, -1).split('\0').every(entry => entry.startsWith('H ')),
    'Local source has hidden/skip-worktree tracked entries: ' + path);
  const status = await git(['status', '--porcelain=v1', '-z', '--untracked-files=all', '--ignored=matching', '--ignore-submodules=none']);
  demand(status === '', 'Local source is dirty (tracked, untracked, or ignored files): ' + path);
}

async function admitSources(paths, recipe, env) {
  demand(closed(paths, ['schema', 'coreSourceRoot', 'producerSourceRoot']) && paths.schema === inputsSchema &&
    typeof paths.coreSourceRoot === 'string' && typeof paths.producerSourceRoot === 'string', 'Local producer paths differ');
  await cleanSource(paths.producerSourceRoot, recipe.producerSource, env);
  if (paths.coreSourceRoot !== paths.producerSourceRoot || portableCanonical(recipe.coreSource) !== portableCanonical(recipe.producerSource)) {
    await cleanSource(paths.coreSourceRoot, recipe.coreSource, env);
  }
  const { stdout } = await localCommand('go', ['version'], { cwd: paths.producerSourceRoot, env, timeout: 20_000 });
  demand(strictUtf8.decode(stdout) === 'go version ' + recipe.goVersion + ' linux/amd64\n', 'Local producer Go toolchain differs');
}

async function runNative(mode, config, paths, env, directory, args = []) {
  const { stdout, stderr } = await localCommand('go', [
    'run', '-mod=readonly', './cmd/internal/ci', 'local-native', mode,
    '--recipe', config.recipePath, '--recipe-sha256', config.recipeSha256, '--output', directory, ...args,
  ], { cwd: paths.producerSourceRoot, env, timeout: mode === 'collect' ? 20 * 60_000 : 5 * 60_000 });
  if (stderr.length && mode === 'collect') process.stderr.write(stderr);
  return strictUtf8.decode(stdout);
}

async function verifySlot(config, paths, env, directory, template) {
  ownedDirectory(dirname(directory));
  ownedDirectory(directory);
  const names = ['inputs.json', 'selection.json', 'contract.json'];
  const bytes = names.map(name => readOwned(join(directory, name)));
  const [input, selection, contract] = bytes.map(value => strictUtf8.decode(value));
  demand(portableCanonical(JSON.parse(input)) === portableCanonical(paths), 'Local producer paths changed');
  localReceipt(template, { ...config, selection, contract });
  const result = JSON.parse(await runNative('verify', config, paths, env, directory));
  demand(closed(result, ['schema', 'verified']) && result.schema === selectionSchema && result.verified === true,
    'Local producer verification result differs');
  for (const [index, name] of names.entries()) {
    demand(bytes[index].equals(readOwned(join(directory, name))), 'Local verification input changed: ' + name);
  }
  demand(config.recipeSha256 === digest(readOwned(config.recipePath)), 'Local compiler configuration changed');
  return JSON.parse(contract);
}

function verifyInstallation(base, directory, contract) {
  const appVersion = JSON.parse(readFileSync(join(base, 'src-tauri/tauri.conf.json'))).version;
  const expected = selectedCoreManifest(contract, 'linux', appVersion);
  const saved = JSON.parse(readFileSync(join(directory, 'core-manifest.json')));
  demand(saved.id === expected.id && saved.appVersion === appVersion && saved.core?.platform === 'linux', 'Local installation selected content differs');
  demand(saved.schema === expected.schema, 'Local installation schema differs');
  for (const name of ['version', 'providerFingerprint', 'files']) demand(JSON.stringify(saved.core[name]) === JSON.stringify(expected.core[name]), 'Local installation identity differs: ' + name);
  const helper = join(directory, 'polaris-helper'); const bytes = readFileSync(helper);
  demand(bytes.includes(LOCAL_MARKER), 'Local installation requires a compiled LocalDevelopmentSelection Helper');
  actualCoreFile(helper, { bytes: bytes.length, sha256: saved.core.helperSha256 });
  verifySelectedCoreManifest(directory, directory, 'linux');
}

export async function verifyLocalSelection(base = root, installation = false) {
  const config = configuration(base), env = producerEnvironment(), directory = localDirectory(base);
  ownedDirectory(dirname(directory));
  ownedDirectory(directory);
  const paths = JSON.parse(strictUtf8.decode(readOwned(join(directory, 'inputs.json'))));
  const compiler = join(base, compiledSelection);
  ownedDirectory(dirname(compiler));
  const template = readOwned(compiler);
  await admitSources(paths, config.recipe, env);
  const contract = await verifySlot(config, paths, env, directory, strictUtf8.decode(template));
  demand(template.equals(readOwned(compiler)), 'Local compiler selection changed during verification');
  if (installation) verifyInstallation(base, directory, contract);
  return contract;
}

export async function prepareLocalInstallation(base, directory, mode) {
  demand(resolve(directory) === resolve(localDirectory(base)), 'Fixed local development input directory required');
  demand(['prepare', 'verify'].includes(mode), 'Local development has no maintenance-anchor or packaging mode');
  const contract = await verifyLocalSelection(base, mode === 'verify');
  if (mode === 'verify') return contract;
  const helper = readFileSync(join(directory, 'polaris-helper'));
  demand(helper.includes(LOCAL_MARKER), 'Build the Helper with the explicit local development Cargo feature first');
  const appVersion = JSON.parse(readFileSync(join(base, 'src-tauri/tauri.conf.json'))).version;
  writeSelectedCoreManifest(directory, contract, 'linux', appVersion);
  verifyInstallation(base, directory, contract);
  return contract;
}

function publishLocal(directory, staged, compiler, template, compiled) {
  const material = mkdtempSync(join(dirname(compiler), '.local-compiler-'));
  try {
    const next = join(material, 'next'), prior = join(material, 'prior');
    const mode = lstatSync(compiler).mode & 0o777;
    for (const [path, bytes] of [[next, compiled], [prior, template]]) {
      writeFileSync(path, bytes, { flag: 'wx', mode, flush: true });
      chmodSync(path, mode);
    }
    demand(template.equals(readOwned(compiler)), 'Local compiler selection changed before publishing');
    demand(!lstatSync(directory, { throwIfNoEntry: false }), 'Local producer refuses to overwrite an existing selection');
    // The completed slot is the commit point. The compiler accepts it only
    // with the exact matching pin, and restores the previous pin on failure.
    renameSync(next, compiler);
    try { renameSync(staged, directory); }
    catch (error) {
      if (readOwned(compiler).equals(Buffer.from(compiled))) renameSync(prior, compiler);
      throw error;
    }
  } finally { rmSync(material, { recursive: true, force: true }); }
}

export async function collectLocalSelection(base, inputs) {
  demand(!formalVariables.some(key => Object.hasOwn(process.env, key)), 'Local producer refuses workflow/candidate mode');
  const config = configuration(base), env = producerEnvironment(), directory = localDirectory(base);
  demand(!lstatSync(directory, { throwIfNoEntry: false }), 'Local producer refuses to overwrite an existing selection');
  const paths = { schema: inputsSchema, coreSourceRoot: realpathSync(inputs['core-source']),
    producerSourceRoot: realpathSync(inputs['producer-source']) };
  const core = resolve(inputs.core), sidecar = resolve(inputs.sidecar), namespace = dirname(directory);
  const compiler = join(base, compiledSelection);
  ownedDirectory(dirname(compiler));
  const template = readOwned(compiler);
  await admitSources(paths, config.recipe, env);
  demand(![paths.coreSourceRoot, paths.producerSourceRoot, namespace].some(path => core === path || core.startsWith(path + sep)),
    'Local Core build output must be outside the admitted source and selection slot');
  demand(!lstatSync(core, { throwIfNoEntry: false }), 'Local Core build output already exists');
  ownedDirectory(dirname(namespace));
  let created = false, locked = false, scratch;
  const lock = join(namespace, '.collect-lock');
  try {
    try { mkdirSync(namespace, { mode: 0o700 }); created = true; }
    catch (error) { if (error.code !== 'EEXIST') throw error; }
    ownedDirectory(namespace);
    mkdirSync(lock, { mode: 0o700 });
    locked = true;
    demand(!lstatSync(directory, { throwIfNoEntry: false }), 'Local producer refuses to overwrite an existing selection');
    scratch = mkdtempSync(join(namespace, '.collect-'));
    const staged = join(scratch, 'linux');
    const draft = await runNative('collect', config, paths, env, staged, ['--core-source', paths.coreSourceRoot, '--core', core, '--sidecar', sidecar]);
    ownedDirectory(staged);
    const { selection, contract, compiled, providerFingerprint } = localReceipt(strictUtf8.decode(template), { ...config, draft });
    writeFileSync(join(staged, 'selection.json'), selection, { flag: 'wx', mode: 0o600, flush: true });
    writeFileSync(join(staged, 'contract.json'), contract, { flag: 'wx', mode: 0o600, flush: true });
    await verifySlot(config, paths, env, staged, compiled);
    publishLocal(directory, staged, compiler, template, compiled);
    return { schema: selectionSchema, coreSource: config.recipe.coreSource, producerSource: config.recipe.producerSource,
      providerFingerprint, policy: JSON.parse(selection).policy };
  } finally {
    if (scratch) rmSync(scratch, { recursive: true, force: true });
    if (locked) rmdirSync(lock);
    if (created) {
      try { rmdirSync(namespace); }
      catch (error) { if (error.code !== 'ENOTEMPTY') throw error; }
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [mode, ...args] = process.argv.slice(2);
  if (mode === 'collect') {
    const options = Object.fromEntries(['core-source', 'producer-source', 'core', 'sidecar'].map(key => [key, { type: 'string' }]));
    const parsed = parseArgs({ args, options, tokens: true });
    demand(parsed.tokens.length === 4 && Object.keys(parsed.values).length === 4 &&
      Object.values(parsed.values).every(Boolean), 'Explicit unique local producer inputs required');
    console.log(portableCanonical(await collectLocalSelection(root, parsed.values)));
  } else {
    demand(args.length === 0 && ['verify', 'verify-installation'].includes(mode), 'Expected local collect/verify/verify-installation');
    const contract = await verifyLocalSelection(root, mode === 'verify-installation');
    console.log(JSON.stringify({ schema: contract.schema, verified: true }));
  }
}
