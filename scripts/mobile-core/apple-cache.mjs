#!/usr/bin/env node
// Actions cache transport moves dependency-only entries on the producer filesystem.
import {
  appendFileSync, lstatSync, mkdirSync, opendirSync, realpathSync, renameSync, rmdirSync, statfsSync, unlinkSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { context, location, requiredSlots, scopeFields, transportPin } from './apple-handoff.mjs';
import { equal, filePin, readReceipt } from './component-admission.mjs';
import { canonical, require, ROOT, sha } from './fork-input.mjs';
import { createNativeRunner, privateDir, PRODUCER_FILES, writeJson } from './producer.mjs';

const STAGE = 'target/apple-ci-cache';
const TARGETS = { 'device-arm64': 'aarch64-apple-ios', 'sim-arm64': 'aarch64-apple-ios-sim' };
const GROUPS = ['deps', 'build', '.fingerprint'];
const REGISTRY_DIRS = ['registry/index', 'registry/cache', 'git/db'];
const MAX_BYTES = 4 * 1024 ** 3, MAX_ENTRIES = 131_072, REGISTRY_MAX_BYTES = 2 * 1024 ** 3;
const HEADROOM = 1024 ** 3, SAMPLE_LIMIT = 64, SAMPLE_PATH_BYTES = 1024;
const APP_ENTRY = /^(?:lib)?polaris(?:_lib)?(?:-[0-9a-f]{16}(?:\.|$)|\.|$)/;
const BUILD_CODE_POLICY = 'polaris-apple-rust-build-code-v1';
const ANDROID_ONLY = new Set(['scripts/mobile-core/android_sdk.py', 'scripts/mobile-core/prepare-ndk-linux.mjs']);

export function appleRustBuildCode(identity, policy = BUILD_CODE_POLICY) {
  require(policy === BUILD_CODE_POLICY, 'unknown Apple Rust build code policy');
  const files = identity?.files;
  require(files && typeof files === 'object' && !Array.isArray(files)
    && [...PRODUCER_FILES].every(name => Object.hasOwn(files, name)), 'complete original producer code inventory required');
  for (const pin of Object.values(files)) {
    require(pin && Object.keys(pin).length === 2 && Number.isSafeInteger(pin.bytes) && pin.bytes > 0
      && typeof pin.sha256 === 'string' && /^[0-9a-f]{64}$/.test(pin.sha256), 'producer code file pin differs');
  }
  // Native observation/parsing shared with Apple remains in this projection.
  return { policy, files: Object.fromEntries(Object.entries(files).filter(([name]) => !ANDROID_ONLY.has(name))) };
}

function cargoSourceConfig(name) {
  return ['Cargo.toml', 'Cargo.lock'].includes(basename(name)) || /(?:^|\/)\.cargo\/config(?:\.toml)?$/.test(name);
}

export function cacheIdentities(tools, rustc, cargo, toolPins, config, env) {
  require(Object.hasOwn(config, 'Cargo.lock') && Object.hasOwn(config, 'src-tauri/Cargo.toml'), 'cache lock/config inputs missing');
  const sourceOverrides = Object.fromEntries(Object.entries(env).filter(([key]) =>
    ['CARGO_REGISTRY_DEFAULT', 'CARGO_REGISTRY_INDEX', 'CARGO_REGISTRY_PROTOCOL'].includes(key)
    || /^CARGO_REGISTRIES_.*_(?:INDEX|PROTOCOL)$/.test(key)));
  const overrides = Object.fromEntries(Object.entries(env).filter(([key]) => key !== 'CARGO_TARGET_DIR'
    && /^(?:CARGO_PROFILE_|CARGO_TARGET_|RUSTFLAGS|RUSTDOCFLAGS|CARGO_ENCODED_RUSTFLAGS|CFLAGS|CXXFLAGS|OBJC_INCLUDE_PATH)/.test(key)));
  const identity = sha(canonical({ policy: 'polaris-apple-rust-dependencies-v2',
    rustc, cargo, tools: toolPins, xcode: tools.xcode, producerCode: appleRustBuildCode(tools.App), config,
    profile: 'release', features: ['tauri/custom-protocol'], crateType: 'staticlib',
    deploymentTarget: '17.0', overrides, sourceOverrides }));
  const registryIdentity = sha(canonical({ policy: 'polaris-apple-registry-downloads-v2',
    cargo, cargoTool: toolPins.cargo, config: Object.fromEntries(Object.entries(config).filter(([name]) => cargoSourceConfig(name))),
    sourceOverrides }));
  return [identity, registryIdentity];
}

function stat(path) {
  return lstatSync(path, { bigint: true, throwIfNoEntry: false });
}

function owned(info) {
  return info?.isDirectory() && info.uid === BigInt(process.getuid());
}

function registryDirectory(path) {
  const home = homedir();
  require(isAbsolute(home) && realpathSync(home) === home, 'registry home ancestry redirected');
  require(owned(stat(home)), 'registry home ownership/type differs');
  const parts = relative(home, path).split(sep);
  require(parts[0] === '.cargo' && !parts.includes('..'), 'registry directory outside fixed Cargo home');
  let current = home;
  for (const part of parts) {
    current = join(current, part);
    const info = stat(current);
    if (!info) return false;
    require(!info.isSymbolicLink(), 'registry cache directory symlink refused');
    require(owned(info), 'registry cache directory ownership/type differs');
  }
  return true;
}

function cargoHome() {
  const home = join(homedir(), '.cargo');
  require((process.env.CARGO_HOME ?? home) === home, 'registry cache path differs from actual Cargo home');
  if (registryDirectory(home)) {
    // External configuration can carry credentials and never enters the key.
    require(!stat(join(home, 'config')) && !stat(join(home, 'config.toml')), 'unbound Cargo home configuration refused');
  }
  return home;
}

function canonicalDirectory(path, required = false) {
  require(isAbsolute(path) && resolve(path) === path, 'cache ancestry cannot be redirected');
  let existing = path;
  while (!stat(existing)) {
    require(!required, 'cache ancestry required');
    existing = dirname(existing);
  }
  require(realpathSync(existing) === existing && stat(existing).isDirectory(), 'cache ancestry cannot be redirected');
}

export function checkedTree(path, { maximum = MAX_BYTES, localExport = false } = {}) {
  const ancestry = localExport ? dirname(path) : path;
  // A strict restore can include a regular-file tree; the root is pinned here.
  require(realpathSync(ancestry) === ancestry, 'cache ancestry cannot be redirected');
  if (localExport) require(owned(stat(ancestry)), 'cache parent ownership/type differs');
  const uid = BigInt(process.getuid()), pending = [path], aliases = new Map();
  let bytes = 0, entries = 0, containsLink = false;
  while (pending.length) {
    const current = pending.pop(), info = stat(current);
    require(++entries <= MAX_ENTRIES && info?.uid === uid, 'cache ownership/count differs');
    if (info.isSymbolicLink()) {
      require(localExport, 'cache symlink refused');
      containsLink = true;
      continue;
    }
    if (info.isDirectory()) {
      const children = opendirSync(current);
      try {
        for (let child; (child = children.readSync());) {
          pending.push(join(current, child.name));
          require(entries + pending.length <= MAX_ENTRIES, 'cache entry count exceeds bound');
        }
      } finally { children.closeSync(); }
    } else {
      require(info.isFile(), 'cache regular file required');
      const key = info.dev + ':' + info.ino, currentAlias = aliases.get(key) ?? { seen: 0n, links: info.nlink };
      require(currentAlias.links === info.nlink, 'cache inode links changed during scan');
      currentAlias.seen++;
      aliases.set(key, currentAlias);
      bytes += Number(info.size);
      require(bytes <= maximum, 'cache bytes exceed transport bound');
    }
  }
  const skip = containsLink ? 'symlink'
    : [...aliases.values()].some(({ seen, links }) => seen !== links) ? 'external-alias' : null;
  require(localExport || !skip, 'cache alias leaves selected dependency entry');
  return { bytes, entries, skip };
}

function checkSpace(path, bytes) {
  const { bavail, bsize } = statfsSync(path);
  const free = bavail * bsize;
  require(free >= bytes + HEADROOM, 'cache archive/extraction headroom unavailable; no snapshot made');
  return free;
}

function targetPath(root) { return join(root, 'cache/rust-target'); }

function note(root, action, slot, facts) {
  const row = { schema: 'polaris-apple-dependency-cache-observation-v1', action, slot, ...scopeFields(),
    actualCargoTargetDir: targetPath(root), stableCachePath: slot ? join(STAGE, slot) : STAGE,
    carrierAccepted: false, devicePassed: false, spaceCheckIsReservation: false, ...facts };
  console.log('APPLE_DEPENDENCY_CACHE=' + canonical(row));
  writeJson(join(root, 'apple-cache-' + action + (slot ? '-' + slot : '') + '.json'), row);
}

function boundedChildren(folder) {
  const children = opendirSync(folder), paths = [];
  try {
    for (let entry; (entry = children.readSync());) {
      require(paths.length < MAX_ENTRIES, 'cache directory entry count exceeds bound');
      paths.push(join(folder, entry.name));
    }
  } finally { children.closeSync(); }
  return paths.sort();
}

async function state(root, stage, runner) {
  const row = readReceipt(join(stage, 'owner.json'));
  require(equal(row.context, await context(runner)) && equal(row.scope, scopeFields()), 'cache transport owner/scope differs');
  require(row.actualCargoTargetDir === targetPath(root), 'cache target path differs');
  require(stat(stage).dev === stat(join(root, 'cache')).dev, 'cache transport requires same filesystem');
  return row;
}

async function prepare(root, stage, cache, runner) {
  const { run } = runner;
  require((await run(['git', 'check-ignore', '--', STAGE], { cwd: ROOT })).trim() === STAGE,
    'cache transport must be ignored by the frozen checkout');
  require(boundedChildren(stage).every(path => basename(path) === '.mobile-producer.lock'), 'prior cache transport retained');
  require(stat(stage).dev === stat(cache).dev && stat(cache).dev === stat(process.env.RUNNER_TEMP).dev,
    'cache transport/archive temporary files require the admitted filesystem');
  const free = checkSpace(stage, (MAX_BYTES + REGISTRY_MAX_BYTES) * 9 / 4);
  const tools = readReceipt(join(root, 'tools.json')), rustc = await run(['rustc', '-vV']);
  require(rustc === tools.rustc, 'cache actual Rust toolchain differs from admission');
  const tracked = (await run(['git', 'ls-files', '-z'], { cwd: ROOT })).split('\0');
  const config = Object.fromEntries(tracked.filter(name => cargoSourceConfig(name)
    || ['rust-toolchain', 'rust-toolchain.toml'].includes(basename(name))
    || ['.github/workflows/apple.yml', 'src-tauri/tauri.conf.json', 'src-tauri/tauri.ios.conf.json'].includes(name))
    .map(name => [name, filePin(join(ROOT, name))]));
  const toolPins = {};
  for (const name of ['cargo', 'rustc']) toolPins[name] = filePin((await run(['rustup', 'which', name])).trim());
  const [identity, registryIdentity] = cacheIdentities(tools, rustc, await run(['cargo', '-V']), toolPins, config, process.env);
  const home = cargoHome();
  for (const name of REGISTRY_DIRS) registryDirectory(join(home, name));
  writeJson(join(stage, 'owner.json'), { context: await context(runner), scope: scopeFields(),
    actualCargoTargetDir: targetPath(root), identity, registryIdentity });
  appendFileSync(process.env.GITHUB_OUTPUT, 'identity=' + identity + '\nregistry_identity=' + registryIdentity + '\n');
  note(root, 'prepare', null, { identity, registryIdentity, freeBytes: free, registryPaths: REGISTRY_DIRS.map(name => '~/.cargo/' + name) });
}

function allowedRoles(slot) {
  require(requiredSlots().includes(slot), 'unrequested cache carrier slot');
  return slot === 'device-arm64' ? ['host', 'target'] : ['target'];
}

function nativeBase(root, slot, role) {
  return join(targetPath(root), ...(role === 'host' ? [] : [TARGETS[slot]]), 'release');
}

function ownedDir(path) {
  canonicalDirectory(path);
  mkdirSync(path, { mode: 0o700, recursive: true });
  require(owned(stat(path)), 'cache directory ownership differs');
  return path;
}

function transfer(entries) {
  const moved = [];
  try {
    for (const [source, destination] of entries) {
      require(!stat(destination), 'cache destination already exists');
      require(stat(source).dev === stat(dirname(destination)).dev, 'cache copy fallback forbidden');
    }
    for (const [source, destination] of entries) {
      require(!stat(destination), 'cache destination became occupied');
      renameSync(source, destination);
      moved.push([source, destination]);
    }
  } catch (cause) {
    const failures = [];
    for (const [source, destination] of moved.reverse()) {
      try {
        require(!stat(source), 'cache rollback source occupied');
        renameSync(destination, source);
      } catch (error) { failures.push(error.message); }
    }
    if (failures.length) throw new Error('cache rename failed; originals retained at reported source/stage paths; rollback: ' + failures.join('; '), { cause });
    throw cause;
  }
}

function manifest(owner, slot) {
  return { schema: 'polaris-apple-dependency-cache-v1', slot, scope: owner.scope, identity: owner.identity, profile: 'release' };
}

async function importCache(root, stage, slot, restored, runner) {
  const owner = await state(root, stage, runner), roles = allowedRoles(slot), payload = join(stage, slot);
  if (!stat(payload)) {
    require(restored === 'miss', 'reported cache hit/export has no dependency payload');
    note(root, 'import', slot, { cacheState: 'miss', restoredBytes: 0 });
    return;
  }
  require(['hit', 'staged'].includes(restored), 'unexpected payload on a cache miss');
  const { bytes } = checkedTree(payload), marker = join(payload, 'cache.json');
  require(equal(readReceipt(marker), manifest(owner, slot)), 'cache scope/tool/profile identity differs');
  require(boundedChildren(payload).every(path => [...roles, 'cache.json'].includes(basename(path))), 'unexpected cache payload role');
  const plan = [], empty = [];
  for (const role of roles) {
    const folder = join(payload, role);
    if (!stat(folder)) continue;
    require(stat(folder).isDirectory(), 'unexpected cache dependency group');
    for (const group of boundedChildren(folder)) {
      const name = basename(group);
      require(GROUPS.includes(name) && stat(group).isDirectory(), 'unexpected cache dependency group');
      const dest = ownedDir(join(nativeBase(root, slot, role), name));
      for (const source of boundedChildren(group)) {
        require(!APP_ENTRY.test(basename(source)), 'App crate artifacts are not reusable dependency cache');
        plan.push([source, join(dest, basename(source))]);
      }
      empty.push(group);
    }
    empty.push(folder);
  }
  transfer(plan);
  unlinkSync(marker);
  for (const path of empty) rmdirSync(path);
  rmdirSync(payload);
  note(root, restored === 'staged' ? 'return' : 'import', slot,
    { cacheState: restored === 'staged' ? 'local-staged-return' : 'remote-hit', restoredBytes: bytes, movedEntries: plan.length });
}

function cacheSample(samples, name) {
  if (samples.length >= SAMPLE_LIMIT) return true;
  const raw = Buffer.from(name);
  let end = Math.min(raw.length, SAMPLE_PATH_BYTES);
  while (end < raw.length && (raw[end] & 0xc0) === 0x80) end--;
  samples.push(raw.subarray(0, end).toString('utf8'));
  return end < raw.length;
}

async function exportCache(root, stage, slot, runner) {
  const owner = await state(root, stage, runner), roles = allowedRoles(slot);
  const pin = readReceipt(join(root, 'evidence-' + slot + '-pin.json'));
  require(pin.slot === slot && equal(pin.context, await context(runner)) && equal(pin.receiptReadErrors, {})
    && equal(pin.archive, transportPin(join(root, 'apple-original-' + slot + '-evidence.zip'))),
  'cache export requires intact preserved carrier originals');
  require(stat(join(root, 'summaries', slot, 'carrier-' + slot + '.json'))?.isFile(), 'cache export requires current carrier summary');
  const payload = join(stage, slot), sources = [];
  require(!stat(payload), 'prior dependency payload retained');
  const alias = { paths: [], count: 0, truncated: false }, symlinks = { paths: [], count: 0, truncated: false };
  let bytes = 0, count = 0, examined = 0;
  for (const role of roles) {
    const base = nativeBase(root, slot, role);
    canonicalDirectory(base);
    for (const group of GROUPS) {
      const folder = join(base, group), info = stat(folder);
      if (!info) continue;
      require(info.isDirectory(), 'native dependency directory redirected');
      for (const source of boundedChildren(folder)) {
        require(++examined <= MAX_ENTRIES, 'examined cache entries exceed bound');
        if (APP_ENTRY.test(basename(source))) continue;
        const inspected = checkedTree(source, { localExport: true });
        examined += inspected.entries - 1;
        require(examined <= MAX_ENTRIES, 'examined cache entries exceed bound');
        if (inspected.skip) {
          const sample = inspected.skip === 'symlink' ? symlinks : alias;
          sample.count++;
          sample.truncated = cacheSample(sample.paths, relative(targetPath(root), source)) || sample.truncated;
          continue;
        }
        bytes += inspected.bytes;
        count += inspected.entries;
        require(bytes <= MAX_BYTES && count <= MAX_ENTRIES, 'combined dependency cache exceeds bound');
        sources.push([source, role, group]);
      }
    }
  }
  require(sources.length > 0, 'no dependency entries available for cache export');
  const headroom = Math.floor(bytes * 9 / 8) + count * 4096, free = checkSpace(stage, headroom);
  privateDir(payload);
  const plan = sources.map(([source, role, group]) => [source, join(privateDir(join(payload, role, group)), basename(source))]);
  writeJson(join(payload, 'cache.json'), manifest(owner, slot));
  transfer(plan);
  note(root, 'export', slot, { dependencyBytes: bytes, movedEntries: plan.length, freeBytes: free,
    uncachedExternalAliasEntries: alias.paths, uncachedExternalAliasCount: alias.count, uncachedExternalAliasSampleLimit: SAMPLE_LIMIT,
    uncachedExternalAliasTruncated: alias.truncated, uncachedSymlinkEntries: symlinks.paths, uncachedSymlinkEntryCount: symlinks.count,
    uncachedSymlinkSampleLimit: SAMPLE_LIMIT, uncachedSymlinkTruncated: symlinks.truncated, examinedEntries: examined,
    archiveHeadroomBytes: headroom, scopeNote: 'same-filesystem dependency moves after original and summary upload; no App products' });
}

function registryReady(root) {
  const home = cargoHome();
  let bytes = 0, entries = 0;
  for (const name of REGISTRY_DIRS) {
    const path = join(home, name);
    if (!registryDirectory(path)) continue;
    const inspected = checkedTree(path, { maximum: REGISTRY_MAX_BYTES });
    bytes += inspected.bytes;
    entries += inspected.entries;
  }
  require(bytes > 0 && bytes <= REGISTRY_MAX_BYTES && entries <= MAX_ENTRIES, 'bounded nonempty registry cache required');
  const free = checkSpace(process.env.RUNNER_TEMP, Math.floor(bytes * 9 / 8) + entries * 4096);
  note(root, 'registry-save', null, { dependencyBytes: bytes, freeBytes: free,
    scopeNote: 'registry downloads only; separate from target dependency cache and carrier acceptance' });
}

async function main() {
  const { positionals: [action, ...extra], values } = parseArgs({ allowPositionals: true, options: {
    slot: { type: 'string' }, restored: { type: 'string', default: 'hit' },
  } });
  require(!extra.length && ['prepare', 'import', 'export', 'registry-ready'].includes(action)
    && (values.slot === undefined || Object.hasOwn(TARGETS, values.slot))
    && ['hit', 'miss', 'staged'].includes(values.restored), 'invalid Apple cache action/slot/restore state');
  const root = location();
  if (action === 'registry-ready') return registryReady(root);
  const stage = privateDir(join(ROOT, STAGE)), cache = privateDir(join(root, 'cache')), runner = createNativeRunner();
  await runner.withSlotLocks(stage, cache, async () => {
    if (action === 'prepare') return prepare(root, stage, cache, runner);
    require(values.slot !== undefined, 'exact cache slot required');
    if (action === 'import') return importCache(root, stage, values.slot, values.restored, runner);
    return exportCache(root, stage, values.slot, runner);
  });
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  await main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
