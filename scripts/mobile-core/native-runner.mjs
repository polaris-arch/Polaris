import { spawn } from 'node:child_process';
import { closeSync, fstatSync, openSync, readSync, realpathSync, writeSync } from 'node:fs';
import { devNull, loadavg } from 'node:os';
import { setTimeout as delay } from 'node:timers/promises';
import { createNativeBudget, groupAlive, progressWrite, requireNative, RESERVATION_FD } from './native-reservation.mjs';

export { capacityLimit, nativeCapacity, nativeEnvironment } from './native-capacity.mjs';
export { groupAlive, privateDir, progressWrite } from './native-reservation.mjs';

const TOOL_SECONDS = 3600;
const TERM_SECONDS = 10;
const DOWNSTREAM_SIGNAL_MARGIN_SECONDS = 3;
const HEARTBEAT_MS = 30_000;
const CHUNK_BYTES = 65_536;
const ERROR_CHARS = 8192;
const SIGNALS = ['SIGTERM', 'SIGINT', 'SIGHUP'];
const STREAMS = ['stdout', 'stderr'];
const phases = new Set([
  'component-build-or-revalidate', 'component-import-verification', 'component-link-input',
  'rust-target-build', 'host-swift-controls', 'app-extension-archive',
  'final-carrier-inspection', 'carrier-command', 'build-settings-admission',
  'tool-prepare', 'frontend-dashboard', 'frontend-build',
]);
const slots = new Set(['device-arm64', 'sim-arm64']);
const decoder = () => new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const newline = text => text.replace(/\r\n?/g, '\n');

export async function withNativeSignals(operation) {
  const controller = new AbortController();
  const interrupt = name => controller.abort(new Error('native operation interrupted by ' + name));
  for (const name of SIGNALS) process.on(name, interrupt);
  try {
    return await operation(controller.signal);
  } finally {
    for (const name of SIGNALS) process.off(name, interrupt);
  }
}

function gitEnvironment(env) {
  const clean = Object.fromEntries(Object.entries(env).filter(([key]) => !key.startsWith('GIT_')));
  return Object.assign(clean, {
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: devNull, GIT_NO_REPLACE_OBJECTS: '1', GIT_TERMINAL_PROMPT: '0',
  });
}

function nativeTransport(env, { reservationFd, reservationRoot, slots: held }, progress) {
  const stdio = ['inherit', 'pipe', 'pipe'];
  const childEnv = { ...env };
  delete childEnv[RESERVATION_FD];
  if (progress != null) {
    const { fd, childFd } = progress;
    requireNative(Number.isSafeInteger(fd) && fd >= 0 && Number.isSafeInteger(childFd) && childFd >= 3 &&
      childFd <= 4096 && fd !== reservationFd && !held.includes(fd), 'invalid inherited tool progress descriptor');
    stdio[childFd] = fd;
  }
  let slot = 3;
  function pass(fd) {
    while (stdio[slot] !== undefined) slot++;
    stdio[slot] = fd;
    return slot++;
  }
  if (reservationFd !== undefined) {
    childEnv[RESERVATION_FD] = String(pass(reservationFd));
    childEnv.POLARIS_BUILD_BUDGET_ROOT = reservationRoot;
  }
  for (const fd of held) pass(fd);
  for (let i = 3; i < stdio.length; i++) stdio[i] ??= 'ignore';
  return { stdio, env: childEnv };
}

function failGroup(group, signal) {
  if (!group) return;
  try {
    process.kill(-group, signal);
  } catch (error) {
    if (error.code !== 'ESRCH') throw error;
  }
}

function nativeFailure(reason, cause) {
  return { reason, error: cause };
}

// One state machine consumes both pipes, waits for Node's direct child reap,
// and tests the original PGID after close before releasing ledger custody.
async function supervise(argv, options, budget, callbacks, signal) {
  const { cwd, env, progressFd, timeoutSeconds, cleanupSeconds, downstreamCleanupSeconds } = options;
  requireNative(['linux', 'darwin'].includes(process.platform), 'Unix native subprocess custody required');
  const managed = downstreamCleanupSeconds !== undefined;
  requireNative(!managed || (Number.isFinite(downstreamCleanupSeconds) && downstreamCleanupSeconds > 0),
    'invalid managed native cleanup allowance');
  const afterKillMs = cleanupSeconds * 500;
  const termMs = managed ? Math.max(afterKillMs, (downstreamCleanupSeconds + DOWNSTREAM_SIGNAL_MARGIN_SECONDS) * 1000)
    : afterKillMs;
  const cleanupMs = termMs + afterKillMs;
  requireNative(Array.isArray(argv) && argv.length > 0 && Number.isFinite(timeoutSeconds) && timeoutSeconds > 0 &&
    timeoutSeconds * 1000 <= 2 ** 31 - 1 && Number.isFinite(cleanupSeconds) && cleanupSeconds > 0 &&
    cleanupMs <= 2 ** 31 - 1, 'invalid native subprocess timeout or command');
  if (signal?.aborted) return { failure: nativeFailure('interrupted', signal.reason), cleanupComplete: true };
  const started = performance.now();
  let child;
  try {
    child = spawn(String(argv[0]), argv.slice(1).map(String), {
      cwd, detached: true, ...nativeTransport(env, budget.transport(), progressFd),
    });
  } catch (error) {
    return { failure: nativeFailure('collection-error', error), cleanupComplete: true };
  }

  const group = child.pid;
  let closed = false, closingCode, recorded = false, mayRegister = false, cleanupComplete = true;
  let failure, cleanupStart, killTimer, heartbeat, managedForceStopped = false;
  let wake;
  const activity = new Promise(resolve => { wake = resolve; });
  function signalGroup(name) {
    try {
      failGroup(group, name);
    } catch (error) {
      failure ??= nativeFailure('collection-error', error);
    }
  }
  function stop(reason, error) {
    if (failure) return;
    failure = nativeFailure(reason, error);
    cleanupStart = performance.now();
    signalGroup('SIGTERM');
    killTimer = setTimeout(() => {
      // A managed Node child must survive long enough to stop the original
      // detached groups it owns. Forced retirement of the wrapper certifies
      // only the local group, so its reservation journal remains retained.
      if (managed && (!closed || liveGroup())) managedForceStopped = true;
      signalGroup('SIGKILL');
    }, termMs);
    wake();
  }
  function liveGroup() {
    if (group === undefined) return false;
    try {
      return groupAlive(group);
    } catch (error) {
      stop('collection-error', error);
      return true;
    }
  }
  function consume(stream, chunk) {
    try {
      const reason = callbacks.onChunk(stream, chunk, failure !== undefined);
      if (reason) stop(reason);
    } catch (error) {
      if (failure) callbacks.onSecondary?.(error);
      else stop('collection-error', error);
    }
  }
  function progress(event) {
    try {
      callbacks.onProgress?.(event, child, started);
    } catch (error) {
      stop('collection-error', error);
    }
  }
  child.once('close', code => {
    closed = true;
    closingCode = code;
    wake();
  });
  child.on('error', error => stop('collection-error', error));
  for (const stream of STREAMS) {
    child[stream].on('error', error => stop('collection-error', error));
    child[stream].on('data', chunk => consume(stream, chunk));
  }
  const cancel = () => stop('interrupted', signal.reason);
  signal?.addEventListener('abort', cancel, { once: true });
  if (signal?.aborted) cancel();
  const timer = setTimeout(() => stop('deadline'), timeoutSeconds * 1000);
  function stopWatching() {
    clearTimeout(timer);
    clearTimeout(killTimer);
    clearInterval(heartbeat);
    signal?.removeEventListener('abort', cancel);
  }
  try {
    const registration = AbortSignal.timeout(Math.min(20_000, Math.ceil(timeoutSeconds * 1000)));
    if (group !== undefined && !failure) {
      mayRegister = true;
      try {
        await budget.registerGroup(group, false, signal ? AbortSignal.any([signal, registration]) : registration);
        recorded = true;
      } catch (error) {
        stop(signal?.aborted ? 'interrupted' : registration.aborted ? 'deadline' : 'collection-error', error);
      }
    }
    if (!failure && group !== undefined) {
      progress('start');
      if (!failure) progress('waiting');
      heartbeat = setInterval(() => { if (!failure) progress('waiting'); }, HEARTBEAT_MS);
    }
    if (!closed && !failure) await activity;
    const directExitFailed = !failure && closed && closingCode !== 0;
    if (!failure && liveGroup()) stop('exit-or-live-group');
    if (failure) {
      const cutoff = cleanupStart + cleanupMs;
      while (performance.now() < cutoff && (!closed || liveGroup())) {
        await delay(Math.min(20, Math.max(1, cutoff - performance.now())));
      }
    }
    const joined = closed && !liveGroup();
    cleanupComplete = joined && !managedForceStopped;
    stopWatching();
    if (!joined) {
      signalGroup('SIGKILL');
      // The incomplete outcome preserves the group journal and the inherited
      // lease on any remaining child. It can never produce a success receipt.
      child.stdout.destroy();
      child.stderr.destroy();
      child.unref();
    } else if (mayRegister && cleanupComplete) {
      try {
        await budget.registerGroup(group, true, AbortSignal.timeout(5000));
      } catch (error) {
        cleanupComplete = false;
        if (failure) callbacks.onSecondary?.(error);
        else failure = nativeFailure('collection-error', error);
      }
    }
    if (!failure && recorded) progress('end');
    if (!failure && closingCode !== 0) failure = nativeFailure('exit-or-live-group');
    return { failure, cleanupComplete, directExitFailed };
  } finally {
    stopWatching();
  }
}

function openLogs(path, capture) {
  const files = {};
  try {
    files.combined = openSync(path, 'a', 0o600);
    for (const stream of STREAMS) {
      const fd = openSync(path + '.' + stream + '.raw', capture === 'none' ? 'a+' : 'a', 0o600);
      files[stream] = fd;
    }
    return files;
  } catch (error) {
    for (const fd of Object.values(files)) closeSync(fd);
    throw error;
  }
}

function append(fd, raw) {
  let at = 0;
  while (at < raw.length) {
    const count = writeSync(fd, raw, at, raw.length - at);
    requireNative(count > 0, 'original tool log incomplete: short write');
    at += count;
  }
}

function projectLogs(files, starts, stored) {
  let firstDecodeError, stderrTail = '';
  const bytes = Buffer.allocUnsafe(CHUNK_BYTES);
  for (const stream of STREAMS) {
    let decode = decoder();
    let at = 0;
    function decodeText(raw, streaming) {
      if (!decode) return;
      try {
        const text = decode.decode(raw, { stream: streaming });
        if (stream === 'stderr') stderrTail = (stderrTail + text).slice(-2 * ERROR_CHARS);
      } catch (error) {
        firstDecodeError ??= error;
        decode = null;
      }
    }
    while (at < stored[stream]) {
      const count = readSync(files[stream], bytes, 0, Math.min(bytes.length, stored[stream] - at), starts[stream] + at);
      requireNative(count > 0, 'original tool log incomplete: raw range ended early');
      const raw = bytes.subarray(0, count);
      append(files.combined, raw);
      decodeText(raw, true);
      at += count;
    }
    decodeText(undefined, false);
  }
  if (firstDecodeError) throw firstDecodeError;
  return newline(stderrTail).slice(-ERROR_CHARS);
}

function primaryFailure(result, argv, timeoutSeconds, stderr) {
  const { failure, cleanupComplete, directExitFailed } = result;
  if (!failure && cleanupComplete) return undefined;
  // A residual descendant still needs cleanup after the direct tool exits.
  // Its existence must not hide the tool's original nonzero result or stderr.
  const message = directExitFailed ? 'actual tool failed (' + argv[0] + '): ' + stderr.slice(-ERROR_CHARS) : ({
    interrupted: 'native operation interrupted',
    deadline: 'actual tool timed out after ' + timeoutSeconds + ' seconds (' + argv[0] + ')',
    'exit-or-live-group': 'native session still owns descendants; original reservation retained',
    'collection-error': 'native tool collection failed (' + argv[0] + ')',
  })[failure?.reason] ?? 'native tool cleanup failed';
  return new Error(message + (!cleanupComplete ? '; native cleanup incomplete; original reservation/group retained' : ''),
    { cause: failure?.error });
}

export function createNativeRunner({ env = process.env, signal, borrowedProgressFd, borrowedProgressPhase } = {}) {
  const budget = createNativeBudget(env, signal);
  const observeSignals = operation => signal ? operation(signal) : withNativeSignals(operation);
  async function runBounded(argv, {
    cwd, env: childEnv = env, stdoutLimit, stderrLimit, timeoutSeconds = TOOL_SECONDS,
    cleanupSeconds = 2, downstreamCleanupSeconds, onStdout, onStderr, onStdoutOverflow, chunkBytes = 4096,
  } = {}) {
    requireNative([stdoutLimit, stderrLimit].every(limit => Number.isSafeInteger(limit) && limit >= 0 &&
      limit < Number.MAX_SAFE_INTEGER) && Number.isSafeInteger(chunkBytes) && chunkBytes > 0 && chunkBytes <= CHUNK_BYTES,
      'bounded native output limits required');
    const limits = { stdout: stdoutLimit, stderr: stderrLimit };
    const counts = { stdout: 0, stderr: 0 };
    const handle = { stdout: onStdout, stderr: onStderr };
    const { failure, cleanupComplete } = await observeSignals(active => supervise(argv, {
      cwd, env: childEnv ?? env, timeoutSeconds, cleanupSeconds, downstreamCleanupSeconds,
    }, budget, {
      onChunk(stream, chunk, cleaning) {
        if (cleaning) return;
        for (let at = 0; at < chunk.length;) {
          const amount = Math.min(chunkBytes, limits[stream] - counts[stream] + 1, chunk.length - at);
          const part = chunk.subarray(at, at + amount);
          at += amount;
          counts[stream] += amount;
          if (counts[stream] > limits[stream]) {
            if (stream === 'stdout') onStdoutOverflow?.(part);
            return stream + '-budget';
          }
          const reason = handle[stream]?.(part);
          requireNative(reason === undefined || (typeof reason === 'string' && /^[a-z][a-z0-9-]{0,63}$/.test(reason)),
            'invalid bounded native refusal');
          if (reason) return reason;
        }
      },
    }, active));
    const reason = cleanupComplete ? failure?.reason ?? null : 'cleanup-incomplete';
    return {
      complete: reason === null, reason, cleanupComplete,
      stdoutBytesObserved: counts.stdout, stderrBytesObserved: counts.stderr,
    };
  }

  async function run(argv, {
    cwd, env: childEnv = env, log, capture = 'full', progressFd, observationFd,
    observationPhase = borrowedProgressFd == null ? undefined : borrowedProgressPhase,
    observationSlot, timeoutSeconds = TOOL_SECONDS, downstreamCleanupSeconds,
  } = {}) {
    return observeSignals(async active => {
      childEnv ??= env;
      requireNative(['full', 'none'].includes(capture), 'unknown tool capture mode');
      requireNative(capture !== 'none' || log != null, 'capture none requires complete original logs');
      requireNative(observationPhase == null || phases.has(observationPhase), 'unknown tool observation phase');
      requireNative(observationSlot == null || slots.has(observationSlot), 'unknown tool observation slot');
      const progress = typeof progressFd === 'number' ? { fd: progressFd, childFd: progressFd } : progressFd;
      const eventFd = observationFd ?? progress?.fd ?? borrowedProgressFd;
      const { reservationFd, slots: held } = budget.transport();
      requireNative(borrowedProgressFd == null || (borrowedProgressFd !== reservationFd && !held.includes(borrowedProgressFd)),
        'progress descriptor cannot own a native lease');
      if (String(argv[0]) === 'git') {
        childEnv = gitEnvironment(childEnv);
        if (cwd != null) {
          const captured = [];
          const { failure, cleanupComplete } = await supervise(
            ['git', 'rev-parse', '--show-toplevel'], { cwd, env: gitEnvironment(env), timeoutSeconds: 10, cleanupSeconds: 2 },
            budget, {
              onChunk(stream, chunk, cleaning) {
                if (cleaning || stream !== 'stdout') return;
                captured.push(chunk);
                return captured.reduce((sum, entry) => sum + entry.length, 0) > 16_384 ? 'stdout-budget' : undefined;
              },
            }, active);
          requireNative(!failure && cleanupComplete &&
            realpathSync(decoder().decode(Buffer.concat(captured)).trim()) === realpathSync(cwd),
          'Git worktree differs from actual source root');
        }
      }
      const counts = { stdout: 0, stderr: 0 }, stored = { stdout: 0, stderr: 0 };
      const buffers = { stdout: [], stderr: [] }, tails = { stdout: '', stderr: '' };
      const secondary = [];
      let files, starts, error;
      try {
        if (log != null) {
          files = openLogs(String(log), capture);
          starts = Object.fromEntries(STREAMS.map(stream => [stream, fstatSync(files[stream]).size]));
        }
        const result = await supervise(argv, {
          cwd, env: childEnv, progressFd: progress, timeoutSeconds, cleanupSeconds: TERM_SECONDS * 2, downstreamCleanupSeconds,
        }, budget, {
          onChunk(stream, chunk) {
            counts[stream] += chunk.length;
            if (capture === 'full') buffers[stream].push(chunk);
            if (files) {
              append(files[stream], chunk);
              stored[stream] += chunk.length;
            }
          },
          onSecondary(problem) { if (secondary.length < 2) secondary.push(problem); },
          onProgress(event, child, started) {
            if (eventFd == null) return;
            const load = loadavg()[0];
            progressWrite(eventFd, 'APPLE_TOOL_PROGRESS=', {
              event, phase: observationPhase ?? null, slot: observationSlot ?? null, pid: child.pid,
              elapsedSeconds: Math.round(performance.now() - started) / 1000,
              stdoutBytes: counts.stdout, stderrBytes: counts.stderr,
              processRunning: child.exitCode === null && child.signalCode === null,
              load1: Number.isFinite(load) && load >= 0 ? load : null, peakRSSBytes: null,
              scope: 'direct process/pipe bytes only; no descendant liveness or acceptance',
            });
          },
        }, active);
        try {
          if (capture === 'none') tails.stderr = projectLogs(files, starts, stored);
          else {
            if (files) for (const stream of STREAMS) for (const raw of buffers[stream]) append(files.combined, raw);
            for (const stream of STREAMS) tails[stream] = newline(decoder().decode(Buffer.concat(buffers[stream])));
          }
        } catch (problem) {
          secondary.push(problem);
        }
        error = primaryFailure(result, argv, timeoutSeconds, tails.stderr) ?? secondary[0];
        if (error) throw error;
        if (capture === 'full') return tails.stdout;
      } catch (problem) {
        error = problem;
        throw error;
      } finally {
        if (files) {
          for (const fd of Object.values(files)) {
            try { closeSync(fd); } catch (problem) { secondary.push(problem); }
          }
        }
        if (error && secondary.length) error.secondary = secondary.filter(problem => problem !== error);
        if (!error && secondary.length) throw secondary[0];
      }
    });
  }

  return {
    run, runBounded, hasReservation: budget.hasReservation,
    withReservation: budget.withReservation, withSlotLocks: budget.withSlotLocks, withLease: budget.withLease,
  };
}
