import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createHash } from 'node:crypto';
import { performance } from 'node:perf_hooks';
import { requireGraph as demand } from '../desktop-core/source-graph.mjs';

export const MAX_LISTING = 8 * 1024 ** 2;
export const MAX_STDERR = 256 * 1024;
// libuv's monotonic clock uses the same host counter in parent and capture.
export const now = () => Number(process.hrtime.bigint() / 1000000n);

// Keep full FileIdInfo, FILETIME, and nanosecond fields as JSON integers.
export function parse(text) {
  return JSON.parse(text, (_key, value, context) => typeof value === 'number' && !Number.isSafeInteger(value)
    && /^-?\d+$/.test(context.source) ? BigInt(context.source) : value);
}

export function wire(value, indent) {
  return JSON.stringify(value, (_key, item) => {
    if (typeof item === 'bigint') return JSON.rawJSON(String(item));
    if (item && !Array.isArray(item) && typeof item === 'object' && indent === undefined) {
      return Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]]));
    }
    return item;
  }, indent).replace(/[\x7f-\uffff]/g, ch => '\\u' + ch.charCodeAt(0).toString(16).padStart(4, '0'));
}

export function startCustody(executable) {
  const child = spawn(executable, [], { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  const ended = once(child, 'close');
  // The process can fail before its first request. Each caller observes the
  // same failure through its pending/native request or the close operation.
  ended.catch(() => {});
  const pending = [];
  let failure, line = '', diagnostic = '';
  function reject(cause) {
    failure ??= cause;
    while (pending.length) pending.shift().reject(failure);
  }
  child.once('error', reject);
  child.stdin.on('error', reject);
  child.on('close', (code) => {
    if (code !== 0 || pending.length) reject(new Error('native Windows custody closed: ' + diagnostic.trim()));
  });
  child.stderr.setEncoding('utf8').on('data', block => { diagnostic = (diagnostic + block).slice(-4096); });
  child.stdout.setEncoding('utf8').on('data', block => {
    try {
      line += block;
      while (line.includes('\n')) {
        const separator = line.indexOf('\n');
        demand(separator <= MAX_LISTING && pending.length, 'invalid native custody reply');
        const row = JSON.parse(line.slice(0, separator));
        line = line.slice(separator + 1);
        const waiter = pending.shift();
        if (row.error !== undefined) waiter.reject(new Error(row.error));
        else waiter.resolve(row.value);
      }
      demand(line.length <= MAX_LISTING, 'native custody reply budget exceeded');
    } catch (cause) {
      reject(cause);
      child.kill();
    }
  });
  function request(value) {
    if (failure) return Promise.reject(failure);
    const raw = JSON.stringify(value) + '\n';
    demand(Buffer.byteLength(raw) <= MAX_LISTING, 'native custody request budget exceeded');
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        child.kill();
        reject(new Error('native Windows custody request timed out'));
      }, 30_000);
      pending.push({
        resolve(value) { clearTimeout(timeout); resolve(value); },
        reject(cause) { clearTimeout(timeout); reject(cause); },
      });
      child.stdin.write(raw);
    });
  }
  const opened = async (op, files) => (await request({ op, files }));
  return {
    pid: child.pid,
    async inspect(files) {
      return (await opened('inspect', files)).map(value => value?.map(BigInt) ?? null);
    },
    async hold(files) {
      return (await opened('hold', files)).map(({ handle, identity }) => ({ handle, identity: identity.map(BigInt) }));
    },
    async transfer(pid, birth, handle, file) {
      const result = await request({ op: 'transfer', pid, birth: String(birth), handle, file });
      return { handle: result.handle, identity: result.identity.map(BigInt) };
    },
    async check({ handle }, file) {
      return (await request({ op: 'check', handle, file })).map(BigInt);
    },
    release: files => request({ op: 'release', handles: files.map(file => file.handle) }),
    async birth(pid) { return BigInt(await request({ op: 'process', pid })); },
    async close() {
      child.stdin.end();
      const [code] = await ended;
      if (failure) throw failure;
      demand(code === 0, 'native Windows custody did not exit normally');
    },
  };
}

export async function withHolds(custody, paths, fn) {
  const files = await custody.hold(paths);
  try { return await fn(files); }
  finally { if (files.length) await custody.release(files); }
}

// The real child and its two pipes are owned by this call. EOF does not end
// the watchdog: named extraction can continue writing until the primary exits.
export function streamChild(argv, consume, stdoutLimit, {
  deadline, cwd, env = process.env, birth, guard, diagnosticContext, cargo = false, diagnostics = true,
} = {}) {
  demand(Number.isSafeInteger(stdoutLimit) && stdoutLimit >= 0 && deadline > now(), 'invalid child output/deadline budget');
  let [command, ...args] = argv;
  let windowsVerbatimArguments = false;
  if (process.platform === 'win32' && /\.cmd$/i.test(command)) {
    const values = [command, ...args];
    demand(values.every(value => !/["\0\r\n]/.test(value)), 'unsupported CMD child argument');
    command = process.env.ComSpec ?? 'C:\\Windows\\System32\\cmd.exe';
    env = { ...env, ...Object.fromEntries(values.map((value, index) => ['POLARIS_CMD_ARG_' + index, value])) };
    args = ['/d', '/s', '/v:off', '/c', '"' + values.map((_value, index) => '"%POLARIS_CMD_ARG_' + index + '%"').join(' ') + '"'];
    windowsVerbatimArguments = true;
  }
  return new Promise((resolve, reject) => {
    const started = performance.now(), startedUnixSeconds = Date.now() / 1000, startTick = now();
    const child = spawn(command, args, { cwd, env, windowsHide: true, windowsVerbatimArguments, stdio: ['ignore', 'pipe', 'pipe'] });
    const controller = new AbortController(), { signal } = controller;
    const aborted = new Promise(resolve => signal.addEventListener('abort', resolve, { once: true }));
    // A stopped callback cannot postpone settlement after the child has closed.
    async function wait(work) {
      const result = await Promise.race([work, aborted]);
      signal.throwIfAborted();
      return result;
    }
    const channels = {
      stdout: { hash: diagnostics ? createHash('sha256') : undefined, bytes: 0, eof: false, last: null },
      stderr: { hash: diagnostics ? createHash('sha256') : undefined, bytes: 0, eof: false, last: null },
    };
    const stderr = [], deliveredHash = diagnostics ? createHash('sha256') : undefined;
    const decoder = cargo ? new TextDecoder('utf8', { fatal: true }) : null;
    const event = { counts: {}, lastEvents: [], invalidJsonLines: 0, oversizedLines: 0,
      freshTrue: 0, freshFalse: 0, buildFinishedSeen: 0, lastBuildSuccess: null, lastEventElapsed: null };
    let line = Buffer.alloc(0), lineBytes = 0, oversized = false, error, checking, consuming, birthResult, stdoutBytes = 0, stderrBytes = 0;
    const elapsed = () => (performance.now() - started) / 1000;
    const summary = (row) => {
      const reason = row?.reason;
      if (!['compiler-artifact', 'compiler-message', 'build-script-executed', 'build-finished'].includes(reason)) return;
      const info = { reason };
      if (typeof row.package_id === 'string') info.packageId = row.package_id.slice(0, 512);
      if (typeof row.target?.name === 'string') info.targetName = row.target.name.slice(0, 256);
      if (Array.isArray(row.target?.kind)) info.targetKind = row.target.kind.slice(0, 8).map(item => String(item).slice(0, 64));
      if (typeof row.fresh === 'boolean') {
        info.fresh = row.fresh;
        event[row.fresh ? 'freshTrue' : 'freshFalse']++;
      }
      if (reason === 'build-finished') {
        event.buildFinishedSeen++;
        info.success = event.lastBuildSuccess = typeof row.success === 'boolean' ? row.success : null;
      }
      if (reason === 'compiler-message' && typeof row.message?.level === 'string') info.level = row.message.level.slice(0, 64);
      event.counts[reason] = (event.counts[reason] ?? 0) + 1;
      event.lastEvents = [...event.lastEvents, info].slice(-16);
      event.lastEventElapsed = elapsed();
    };
    function observeCargo(block) {
      decoder.decode(block, { stream: true });
      for (let offset = 0; offset < block.length;) {
        const end = block.indexOf(10, offset);
        const part = block.subarray(offset, end < 0 ? undefined : end);
        lineBytes += part.length;
        if (!oversized && lineBytes <= 65536) line = Buffer.concat([line, part]);
        else { oversized = true; line = Buffer.alloc(0); }
        if (end < 0) break;
        if (oversized) event.oversizedLines++;
        else if (line.toString('utf8').trim()) {
          try { summary(JSON.parse(line)); }
          catch (cause) { if (!(cause instanceof SyntaxError)) throw cause; event.invalidJsonLines++; }
        }
        offset = end + 1;
        line = Buffer.alloc(0); lineBytes = 0; oversized = false;
      }
    }
    function report(stage) {
      if (!diagnostics) return;
      const bytes = Buffer.concat(stderr), tail = bytes.subarray(Math.max(0, bytes.length - 4096));
      const streams = Object.fromEntries(Object.entries(channels).map(([name, row]) => [name, {
        totalReadBytes: row.bytes, readSha256: row.hash.copy().digest('hex'), eofRead: row.eof,
        eofConsumed: row.eof && (name === 'stderr' || stdoutBytes === row.bytes), lastReadElapsed: row.last,
      }]));
      Object.assign(streams.stdout, { retainedBytes: stdoutBytes, retainedSha256: deliveredHash.copy().digest('hex'),
        truncated: streams.stdout.totalReadBytes > stdoutBytes, retentionBasis: 'blocks delivered successfully to consumer; failed callback retention unknown' });
      Object.assign(streams.stderr, { retainedBytes: stderrBytes, retainedSha256: createHash('sha256').update(bytes).digest('hex'),
        truncated: streams.stderr.totalReadBytes > stderrBytes, tail: tail.toString('utf8'), tailTruncated: stderrBytes > tail.length,
        tailBytes: tail.length, tailOmittedBytes: stderrBytes - tail.length,
        tailStartsAtLineBoundary: !tail.length || tail.length === bytes.length || bytes[bytes.length - tail.length - 1] === 10,
        tailEndsAtLineBoundary: !bytes.length || bytes.at(-1) === 10 });
      const row = { schema: 'polaris-owned-child-diagnostic-v1', diagnosticsOnly: true, artifactAuthority: false,
        allDescendantsObserved: false, stage, argv: argv.slice(0, 32).map(value => value.slice(0, 512)),
        argvTruncated: argv.length > 32 || argv.some(value => value.length > 512),
        cwd: (cwd ?? process.cwd()).slice(0, 2048), tool: argv[0].slice(0, 2048), pid: child.pid,
        startedUnixSeconds, startedMonotonic: startTick / 1000, deadlineMonotonic: deadline / 1000,
        timeoutSeconds: (deadline - startTick) / 1000, elapsedSeconds: elapsed(), primaryPoll: child.exitCode, streams,
        limits: { stdoutBytes: stdoutLimit, stderrBytes: MAX_STDERR, blockBytes: 65536 },
        cause: error ? { type: error.name, message: error.message.slice(0, 1024) } : null,
        cleanup: { primaryExited: child.exitCode !== null || child.signalCode !== null,
          killed: child.killed, pipesClosed: child.stdout.destroyed && child.stderr.destroyed },
        ...(cargo && { cargo: { ...event, incompleteLineBytes: lineBytes, retainedIncompleteLineBytes: line.length } }),
        ...(diagnosticContext && { context: diagnosticContext }),
      };
      const text = wire(row);
      demand(Buffer.byteLength(text) <= 256 * 1024, 'child diagnostic record byte limit exceeded');
      process.stderr.write('POLARIS_OWNED_CHILD_DIAGNOSTIC ' + text + '\n');
    }
    function stop(cause) {
      if (error) return;
      error = cause;
      controller.abort(cause);
      try { report('failure-before-cleanup'); }
      finally {
        child.kill('SIGKILL');
        child.stdout.destroy();
        child.stderr.destroy();
      }
    }
    const interrupted = () => stop(new DOMException('owned child interrupted', 'AbortError'));
    const expires = setTimeout(() => stop(new Error('owned child deadline exceeded')), deadline - now());
    const check = () => Promise.resolve().then(() => guard(signal));
    const poll = guard && setInterval(() => {
      if (checking || error) return;
      checking = check().catch(stop).finally(() => { checking = null; });
    }, 50);
    process.once('SIGINT', interrupted);
    process.once('SIGTERM', interrupted);
    child.once('spawn', () => {
      if (birth) birthResult = Promise.resolve().then(() => birth(child.pid))
        .then(value => ({ value }), cause => { stop(cause); return {}; });
    });
    child.once('error', stop);
    for (const label of ['stdout', 'stderr']) {
      const stream = child[label], channel = channels[label];
      stream.once('error', stop);
      stream.once('end', () => { channel.eof = true; });
      stream.on('data', async block => {
        if (error) return;
        stream.pause();
        try {
          channel.bytes += block.length;
          channel.hash?.update(block);
          channel.last = elapsed();
          if (label === 'stderr') {
            demand(channel.bytes <= MAX_STDERR, 'owned child stderr byte limit exceeded');
            stderr.push(block); stderrBytes += block.length;
          } else {
            demand(channel.bytes <= stdoutLimit, 'owned child stdout byte limit exceeded');
            if (cargo) observeCargo(block);
            consuming = (async () => {
              await consume(block, signal);
              signal.throwIfAborted();
              stdoutBytes += block.length; deliveredHash?.update(block);
            })();
            await consuming;
          }
        } catch (cause) { stop(cause); }
        finally { if (!error) stream.resume(); }
      });
    }
    child.once('close', async code => {
      try {
        clearInterval(poll);
        if (error) throw error;
        const stderrText = Buffer.concat(stderr).toString('utf8');
        demand(code === 0, 'owned child failed (' + code + '): ' + stderrText);
        const identity = await wait(birthResult);
        await wait(checking);
        await wait(consuming);
        if (guard) await wait(check());
        demand(now() < deadline && channels.stdout.eof && channels.stderr.eof, 'incomplete owned child exit or pipe evidence');
        decoder?.decode();
        if (diagnosticContext?.candidate) report('transport-complete');
        resolve({ stdoutBytes, stderrBytes, pid: child.pid, birth: identity?.value ?? null, exitCode: code,
          stdoutSha256: deliveredHash?.digest('hex'), pipeEOF: ['stderr', 'stdout'], actualWaitSeconds: elapsed(), stderr: stderrText });
      } catch (cause) {
        error ??= cause;
        controller.abort(error);
        try { report('failure-after-cleanup'); } finally { reject(error); }
      } finally {
        clearTimeout(expires);
        process.removeListener('SIGINT', interrupted);
        process.removeListener('SIGTERM', interrupted);
      }
    });
  });
}
