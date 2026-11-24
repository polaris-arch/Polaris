import { AsyncLocalStorage } from 'node:async_hooks';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  closeSync, constants, fsyncSync, fstatSync, ftruncateSync, lstatSync, mkdirSync, mkdtempSync, openSync,
  readdirSync, readSync, realpathSync, renameSync, rmSync, statfsSync, writeFileSync, writeSync,
} from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { exactJSONInteger } from './native-capacity.mjs';
import { decode, receiptPolicy } from './source-admission.mjs';

export const RESERVATION_FD = 'POLARIS_BUILD_RESERVATION_FD';
const SCHEMA = 'polaris-mobile-filesystem-reservation-v1';
const BUDGET_MARGIN = 1024n ** 3n;
const ROW_BYTES = 8192;
const BIG = { bigint: true };
const source = fileURLToPath(new URL('./native-fd.rs', import.meta.url));
const rootRepo = fileURLToPath(new URL('../../', import.meta.url));
let nativeTool;

export function requireNative(condition, reason) {
  if (!condition) throw new Error(reason);
}

function nativeExecutable() {
  if (nativeTool !== undefined) return nativeTool;
  requireNative(['linux', 'darwin'].includes(process.platform), 'Unix native subprocess custody required');
  const directory = mkdtempSync(join(realpathSync(tmpdir()), 'polaris-mobile-native-fd-'));
  const executable = join(directory, 'native-fd');
  const result = spawnSync('rustc', ['--edition=2024', '-C', 'strip=symbols', '-o', executable, source], {
    cwd: rootRepo, encoding: 'utf8', maxBuffer: 16_384, timeout: 30_000, killSignal: 'SIGKILL',
  });
  if (result.status !== 0) {
    rmSync(directory, { recursive: true, force: true });
    throw new Error('could not build mobile native descriptor helper with the pinned Rust toolchain',
      { cause: result.error ?? new Error(result.stderr?.trim()) });
  }
  // macOS assesses a new executable on its first launch; take that once here so
  // each descriptor operation's 2 s bound only covers a steady-state launch.
  spawnSync(executable, [], { stdio: 'ignore', timeout: 30_000, killSignal: 'SIGKILL' });
  process.once('exit', () => rmSync(directory, { recursive: true, force: true }));
  nativeTool = executable;
  return executable;
}

// The POSIX lock is attached to the inherited open file description. The
// temporary invocation closing its copy leaves Node and native child copies held.
function descriptorOperation(fd, operation, input) {
  const result = spawnSync(nativeExecutable(), [operation], {
    input, stdio: [input === undefined ? 'ignore' : 'pipe', 'ignore', 'pipe', fd],
    timeout: 2000, killSignal: 'SIGKILL', maxBuffer: 4096,
  });
  if (result.status === 0) return true;
  if (result.status === 2 && operation !== 'progress') return false;
  throw new Error(operation === 'progress' ? 'tool progress output incomplete' : 'native lease operation failed', {
    cause: result.error ?? new Error(result.stderr?.toString('utf8').trim()),
  });
}

export function progressWrite(fd, prefix, payload) {
  requireNative(Number.isSafeInteger(fd) && fd >= 0, 'invalid progress descriptor');
  // The event payload is a small fixed observation; keep JSON wire-compatible
  // with callers that open the original descriptor in text mode.
  const json = JSON.stringify(payload, (key, value) => {
    if (value && typeof value === 'object' && !Array.isArray(value) && !JSON.isRawJSON(value)) {
      return Object.fromEntries(Object.entries(value).sort(([left], [right]) => left < right ? -1 : Number(left > right)));
    }
    return value;
  });
  const row = Buffer.from((prefix + json + '\n').replace(/[^\x00-\x7f]/g,
    char => '\\u' + char.charCodeAt(0).toString(16).padStart(4, '0')), 'ascii');
  requireNative(row.length <= 4096, 'progress event exceeds fixed bound');
  // Run a bounded write on the delegated description. A stalled observation
  // reader cannot block the event loop indefinitely while detached tools run.
  descriptorOperation(fd, 'progress', row);
}

export function groupAlive(group) {
  requireNative(Number.isSafeInteger(group) && group > 0, 'native process group identity invalid');
  try {
    process.kill(-group, 0);
    return true;
  } catch (error) {
    if (error.code === 'ESRCH') return false;
    if (error.code === 'EPERM') return true;
    throw error;
  }
}

function privateDirectory(input, message) {
  let path = isAbsolute(input) ? input : process.cwd() + '/' + input;
  for (let current = path; ; current = dirname(current)) {
    requireNative(!lstatSync(current, { throwIfNoEntry: false })?.isSymbolicLink(), message + ' redirected');
    if (dirname(current) === current) break;
  }
  path = resolve(path);
  mkdirSync(path, { recursive: true, mode: 0o700 });
  const info = lstatSync(path);
  requireNative(info.isDirectory() && info.uid === process.getuid() && (info.mode & 0o7777) === 0o700,
    message + ' must be current-user private');
  return path;
}

export function privateDir(input) {
  return privateDirectory(input, 'producer slot');
}

function openLock(path, budget) {
  const fd = openSync(path, constants.O_RDWR | constants.O_CREAT | constants.O_NOFOLLOW | constants.O_NONBLOCK, 0o600);
  try {
    const info = fstatSync(fd);
    requireNative(info.isFile() && info.uid === process.getuid() && info.nlink === 1 &&
      (!budget || (info.mode & 0o7777) === 0o600), budget ? 'budget lock identity differs' : 'producer lock identity differs');
    return fd;
  } catch (error) {
    closeSync(fd);
    throw error;
  }
}

function readRow(fd) {
  const { size } = fstatSync(fd);
  requireNative(size > 0 && size <= ROW_BYTES, 'reservation record bounds');
  const raw = Buffer.alloc(size);
  let at = 0;
  while (at < raw.length) {
    const count = readSync(fd, raw, at, raw.length - at, at);
    requireNative(count > 0, 'reservation record changed during read');
    at += count;
  }
  const row = decode(raw, ROW_BYTES);
  requireNative(row?.schema === SCHEMA && typeof row.slot === 'string' &&
    /^[a-f0-9]{64}\.reservation$/.test(row.slot) && typeof row.work === 'string' && isAbsolute(row.work) &&
    (Number.isSafeInteger(row.filesystem) || typeof row.filesystem === 'bigint') &&
    (Number.isSafeInteger(row.reserveBytes) || typeof row.reserveBytes === 'bigint') && row.reserveBytes > 0 &&
    Array.isArray(row.groups) && row.groups.length <= 32 &&
    row.groups.every(group => Number.isSafeInteger(group) && group > 0), 'reservation record identity differs');
  return row;
}

function writeRow(fd, { filesystem, groups, reserveBytes, schema, slot, work }) {
  const raw = Buffer.from(JSON.stringify({
    filesystem: exactJSONInteger(BigInt(filesystem)), groups, reserveBytes: exactJSONInteger(BigInt(reserveBytes)), schema, slot, work,
  }));
  requireNative(raw.length <= ROW_BYTES, 'reservation record bounds');
  let at = 0;
  while (at < raw.length) {
    const count = writeSync(fd, raw, at, raw.length - at, at);
    requireNative(count > 0, 'reservation record write incomplete');
    at += count;
  }
  ftruncateSync(fd, raw.length);
  fsyncSync(fd);
}

function freeBytes(path) {
  const { bavail, frsize } = statfsSync(path, BIG);
  requireNative(typeof frsize === 'bigint' && frsize > 0n && bavail >= 0n, 'filesystem space observation unavailable');
  return bavail * frsize;
}

async function withGate(root, mode, operation, signal) {
  const gate = openLock(join(root, '.admission.lock'), true);
  const deadline = performance.now() + 20_000;
  try {
    for (;;) {
      signal?.throwIfAborted();
      if (descriptorOperation(gate, mode)) return operation();
      requireNative(performance.now() < deadline, 'native reservation admission lock remained busy');
      await delay(20);
    }
  } finally {
    closeSync(gate);
  }
}

function checkInherited(root, work, reserveBytes, fd) {
  const row = readRow(fd);
  const path = join(root, row.slot);
  const held = fstatSync(fd, BIG), visible = lstatSync(path, BIG);
  requireNative(visible.isFile() && held.dev === visible.dev && held.ino === visible.ino &&
    held.dev === lstatSync(work, BIG).dev && held.uid === BigInt(process.getuid()) &&
    held.nlink === 1n && (held.mode & 0o7777n) === 0o600n &&
    BigInt(row.filesystem) === held.dev && lstatSync(row.work, BIG).dev === held.dev &&
    BigInt(row.reserveBytes) >= reserveBytes, 'inherited reservation identity differs');
  // Check that the delegated description already owns an exclusive lease,
  // including when a stale descriptor names today's exact slot file.
  const independent = openLock(path, true);
  try {
    requireNative(!descriptorOperation(independent, 'exclusive') && descriptorOperation(fd, 'exclusive'),
      'inherited reservation lock differs');
  } finally {
    closeSync(independent);
  }
}

function acquire(root, work, reserveBytes) {
  const slot = createHash('sha256').update(work).digest('hex') + '.reservation';
  const fd = openLock(join(root, slot), true);
  try {
    requireNative(descriptorOperation(fd, 'exclusive'), 'same work slot already owns a native reservation');
    if (fstatSync(fd).size) {
      requireNative(!readRow(fd).groups.some(groupAlive), 'same work slot has a retained live/unknown native session');
    }
    let outstanding = 0n;
    const rows = readdirSync(root).filter(name => name.endsWith('.reservation'));
    requireNative(rows.length <= 128, 'reservation ledger slot budget exceeded');
    for (const name of rows) {
      if (name === slot) continue;
      const other = openLock(join(root, name), true);
      try {
        const idle = descriptorOperation(other, 'exclusive');
        const row = !idle || fstatSync(other).size ? readRow(other) : null;
        if (row && (!idle || row.groups.some(groupAlive))) outstanding += BigInt(row.reserveBytes);
      } finally {
        closeSync(other);
      }
    }
    requireNative(freeBytes(work) >= outstanding + reserveBytes + BUDGET_MARGIN,
      'shared filesystem peak reservations exhaust free space');
    writeRow(fd, { filesystem: lstatSync(work, BIG).dev, groups: [], reserveBytes, schema: SCHEMA, slot, work });
    return fd;
  } catch (error) {
    closeSync(fd);
    throw error;
  }
}

function pathBytes(root) {
  let total = 0n;
  const seen = new Set();
  const directories = [root];
  while (directories.length) {
    const parent = directories.pop();
    for (const name of readdirSync(parent)) {
      const path = join(parent, name);
      const info = lstatSync(path, BIG);
      if (info.isDirectory()) directories.push(path);
      if (!info.isFile()) continue;
      const key = `${info.dev}:${info.ino}`;
      if (!seen.has(key)) total += info.size;
      seen.add(key);
    }
  }
  return total;
}

function reportBudget(work, cache, cacheMaxBytes) {
  const cacheBytes = pathBytes(cache);
  const report = {
    cacheBytes: exactJSONInteger(cacheBytes), cacheMaxBytes, cleanupPerformed: false,
    filesystem: exactJSONInteger(lstatSync(work, BIG).dev), freeBytes: exactJSONInteger(freeBytes(work)),
    retainedReason: 'original components, native inputs and recovery evidence', workBytes: exactJSONInteger(pathBytes(work)),
  };
  const temporary = join(work, '.budget-observation.tmp');
  writeFileSync(temporary, JSON.stringify(report), { flag: 'wx', mode: 0o600 });
  renameSync(temporary, join(work, 'budget-observation.json'));
  requireNative(cacheBytes <= BigInt(cacheMaxBytes), 'fixed mobile cache budget exceeded');
}

export function createNativeBudget(env, signal) {
  const owner = new AsyncLocalStorage();
  function active(entry) {
    for (let current = entry; current !== undefined; current = current.parent) {
      if (!current.active) return false;
    }
    return true;
  }
  function state() {
    const current = owner.getStore();
    requireNative(active(current), 'native reservation scope already closed');
    return current ?? { slots: [] };
  }
  function withOwner(entry, operation) {
    entry.parent = owner.getStore();
    entry.active = true;
    return owner.run(entry, async () => {
      try {
        return await operation();
      } finally {
        entry.active = false;
      }
    });
  }
  function hasReservation() {
    const current = owner.getStore();
    return current?.reservation !== undefined && active(current);
  }
  function transport() {
    const { reservation, slots } = state();
    return { reservationFd: reservation?.fd, reservationRoot: reservation?.root, slots };
  }
  async function withReservation(input, contract, operation, { reserveBytes = contract.budget.buildPeakReserveBytes } = {}) {
    const prior = state();
    requireNative(Number.isSafeInteger(reserveBytes) && reserveBytes > 0, 'reservation byte requirement invalid');
    const work = privateDirectory(input, 'budget directory');
    const root = privateDirectory(env.POLARIS_BUILD_BUDGET_ROOT ?? join(homedir(), '.cache/polaris-build-budget'), 'budget directory');
    requireNative(lstatSync(root, BIG).dev === lstatSync(work, BIG).dev, 'budget and work must share a filesystem');
    const inherited = prior.reservation?.fd ?? env[RESERVATION_FD];
    if (inherited !== undefined) {
      requireNative(/^[0-9]+$/.test(String(inherited)) && Number.isSafeInteger(Number(inherited)), 'reservation descriptor invalid');
      const fd = Number(inherited);
      await withGate(root, 'shared', () => {
        state();
        checkInherited(root, work, BigInt(reserveBytes), fd);
      }, signal);
      return withOwner({ ...prior, reservation: { root, fd } }, operation);
    }
    const fd = await withGate(root, 'exclusive', () => acquire(root, work, BigInt(reserveBytes)), signal);
    try {
      return await withOwner({ ...prior, reservation: { root, fd } }, operation);
    } finally {
      closeSync(fd);
    }
  }
  async function withSlotLocks(workPath, cachePath, operation) {
    const prior = state(), handles = [];
    const work = privateDir(workPath), cache = privateDir(cachePath);
    try {
      for (const root of new Set([work, cache].sort())) {
        const fd = openLock(join(root, '.mobile-producer.lock'), false);
        handles.push(fd);
        requireNative(descriptorOperation(fd, 'exclusive'), 'mobile producer slot already in use');
      }
      return await withOwner({ ...prior, slots: [...prior.slots, ...handles] }, () => operation(work, cache));
    } finally {
      for (const fd of handles.reverse()) closeSync(fd);
    }
  }
  function withLease(work, cache, contract, operation, options) {
    return withReservation(work, contract, () => withSlotLocks(work, cache, async (work, cache) => {
      requireNative(lstatSync(work, BIG).dev === lstatSync(cache, BIG).dev, 'work/cache must use one reserved filesystem');
      try {
        return await operation(work, cache);
      } finally {
        reportBudget(work, cache, contract.budget.cacheMaxBytes);
      }
    }), options);
  }
  async function registerGroup(group, finished = false, signal) {
    const { reservation } = state();
    if (!reservation) return;
    const { root, fd } = reservation;
    await withGate(root, 'exclusive', () => {
      state();
      const row = readRow(fd), groups = new Set(row.groups);
      if (finished && !groupAlive(group)) groups.delete(group);
      else groups.add(group);
      requireNative(groups.size <= 32, 'retained native process-group budget exceeded');
      row.groups = [...groups].sort((left, right) => left - right);
      writeRow(fd, row);
    }, signal);
  }
  return { hasReservation, registerGroup, transport, withReservation, withSlotLocks, withLease };
}
