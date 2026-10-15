import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { AsyncLocalStorage } from 'node:async_hooks';

export const DEBUG_APP = 'com.polaris2.app.debug';
export const RELEASE_APP = 'com.polaris2.app';
export const MAIN_ACTIVITY = 'com.polaris2.app.MainActivity';

export function parseAndroidQaTarget(args) {
  const [serial, ...options] = args;
  assert.ok(serial && !serial.startsWith('-'), 'Explicit authorized ADB serial required');
  assert.ok(options.length === 0 || (options.length === 1 && options[0] === '--release'),
    'Usage: <adb-serial> [--release]');
  return { serial, app: options.length ? RELEASE_APP : DEBUG_APP };
}

export function assertInstalledAndroidPackage(adb, app) {
  assert.ok(app === DEBUG_APP || app === RELEASE_APP, `Unknown Android QA package: ${app}`);
  const installed = new Set(adb('shell', 'pm', 'list', 'packages', 'com.polaris2.app')
    .split(/\r?\n/).map(line => /^package:(.+)$/.exec(line)?.[1]).filter(Boolean));
  if (installed.has(app)) return;
  const other = app === DEBUG_APP ? RELEASE_APP : DEBUG_APP;
  const hint = installed.has(other)
    ? ` ${other} is installed; ${app === DEBUG_APP ? 'pass --release to target it explicitly' : 'omit --release to target it'}.`
    : ' Neither Debug nor Release package is installed.';
  throw new Error(`Android QA package ${app} is not installed.${hint}`);
}

// The observation scope lets existing until(() => evaluate(...)) / adb(...) callers
// share a deadline without changing their call signatures. Known I/O consumes its
// cancellation signal; arbitrary callbacks passed to until must do so themselves.
const observation = new AsyncLocalStorage();
const defaults = {
  now: () => performance.now(),
  setTimeout: (...args) => setTimeout(...args),
  clearTimeout: timer => clearTimeout(timer),
  execFileSync,
  fetch: (...args) => fetch(...args),
  WebSocket: (...args) => new WebSocket(...args),
  reportFailure: error => {
    console.error('Android CDP cleanup/session failed:', error);
    process.exitCode = 1;
  },
};
const asError = error => error instanceof Error ? error : new Error(String(error));
const expired = label => new Error(`${label} deadline exceeded`);

function checkBudget(scope = observation.getStore()) {
  if (scope?.signal.aborted) throw scope.signal.reason;
  if (scope && scope.remaining() <= 0) throw expired(scope.label);
}

function abortable(promise, signal) {
  return new Promise((resolve, reject) => {
    const aborted = () => reject(signal.reason);
    const settle = callback => value => {
      signal.removeEventListener('abort', aborted);
      callback(value);
    };
    // Always observe late rejection, including an operation started just before abort.
    Promise.resolve(promise).then(settle(resolve), settle(reject));
    if (signal.aborted) aborted();
    else signal.addEventListener('abort', aborted, { once: true });
  });
}

async function deadline(action, timeout, runtime, label) {
  assert.ok(Number.isFinite(timeout) && timeout > 0, 'Positive finite timeout required');
  const parent = observation.getStore();
  checkBudget(parent);
  const budget = Math.min(timeout, parent?.remaining() ?? Infinity);
  const end = runtime.now() + budget;
  const controller = new AbortController();
  const scope = { signal: controller.signal, remaining: () => end - runtime.now(), label };
  const timer = runtime.setTimeout(() => controller.abort(expired(label)), budget);
  const aborted = () => controller.abort(parent.signal.reason);
  parent?.signal.addEventListener('abort', aborted, { once: true });
  try {
    return await observation.run(scope, async () => {
      const value = await abortable(Promise.resolve().then(() => action(scope)), scope.signal);
      checkBudget(scope);
      return value;
    });
  } finally {
    runtime.clearTimeout(timer);
    parent?.signal.removeEventListener('abort', aborted);
  }
}

function pause(ms, runtime, scope) {
  return new Promise((resolve, reject) => {
    checkBudget(scope);
    const aborted = () => { runtime.clearTimeout(timer); reject(scope.signal.reason); };
    const timer = runtime.setTimeout(() => {
      scope.signal.removeEventListener('abort', aborted);
      resolve();
    }, Math.min(ms, scope.remaining()));
    scope.signal.addEventListener('abort', aborted, { once: true });
  });
}

export async function until(read, test, timeout = 15_000, dependencies = {}) {
  const runtime = { ...defaults, ...dependencies };
  return deadline(async scope => {
    for (;;) {
      checkBudget(scope);
      const value = await read(scope);
      checkBudget(scope);
      if (test(value)) return value;
      await pause(250, runtime, scope);
    }
  }, timeout, runtime, '等待预期状态');
}

function addCleanupErrors(primary, errors) {
  if (errors.length) primary.cleanupErrors = [...(primary.cleanupErrors ?? []), ...errors];
  return primary;
}

function mobilePage(pages) {
  assert.ok(Array.isArray(pages), 'CDP discovery must return a page array');
  return pages.find(page => {
    if (page?.type !== 'page' || typeof page.url !== 'string') return false;
    let url;
    try { url = new URL(page.url); } catch { return false; }
    return url.pathname.endsWith('/mobile.html');
  });
}

/**
 * Attach to the selected installed mobile WebView. The entire connection gets one
 * 15 s budget; each ADB command uses its remaining budget. Cleanup gets a separate
 * 3 s budget. beforeLaunch(adb) is synchronous and runs after package preflight.
 * close(primaryError?) remains synchronous/idempotent. It reports cleanup-only
 * failure through stderr + exitCode, so an unchanged caller's finally cannot
 * replace its original business exception with a secondary cleanup exception.
 */
export async function connectAndroidWebView(serial, app = DEBUG_APP, options = {}) {
  assert.ok(serial && !serial.startsWith('-'), 'Explicit authorized ADB serial required');
  const { beforeLaunch, timeout = 15_000, dependencies = {} } = options;
  const runtime = { ...defaults, ...dependencies };
  const adbPath = `${process.env.ANDROID_HOME ?? `${process.env.HOME}/Android/Sdk`}/platform-tools/adb`;
  const runAdb = (args, limit, checkAfter = true) => {
    const scope = observation.getStore();
    checkBudget(scope);
    const output = runtime.execFileSync(adbPath, ['-s', serial, ...args], {
      encoding: 'utf8', timeout: Math.max(1, Math.ceil(Math.min(limit, scope?.remaining() ?? Infinity))),
      killSignal: 'SIGKILL',
    });
    if (checkAfter) checkBudget(scope);
    return output;
  };
  const adb = (...args) => runAdb(args, 20_000);
  let pid, port, ws;
  let forwardOwned = false;
  let released = false;
  let closed = false;
  let failure;
  let failureReported = false;
  let nextId = 0;
  const waiting = new Map();
  const listeners = [];

  function release(primary) {
    if (released) return [];
    released = true;
    const errors = [];
    for (const [event, handler] of listeners) ws.removeEventListener(event, handler);
    listeners.length = 0;
    try { ws?.close(); } catch (error) { errors.push(asError(error)); }
    if (forwardOwned) {
      // Clear ownership before attempting removal. Never remove unrelated forwards,
      // and never retry a port that could have been reassigned after an uncertain remove.
      forwardOwned = false;
      try {
        runtime.execFileSync(adbPath, ['-s', serial, 'forward', '--remove', `tcp:${port}`],
          { encoding: 'utf8', timeout: 3_000, killSignal: 'SIGKILL' });
      } catch (error) { errors.push(asError(error)); }
    }
    if (primary) addCleanupErrors(primary, errors);
    return errors;
  }

  function rejectPending(error) {
    for (const pending of waiting.values()) pending.reject(error);
    waiting.clear();
  }

  function fail(error) {
    if (failure) return failure;
    failure = asError(error);
    rejectPending(failure);
    release(failure);
    return failure;
  }

  const onError = event => fail(event.error ?? new Error('CDP WebSocket error'));
  const onClose = event => fail(new Error(`CDP WebSocket closed (${event.code ?? 'unknown'})`));
  const onMessage = event => {
    try {
      const reply = JSON.parse(event.data);
      assert.ok(reply && typeof reply === 'object' && !Array.isArray(reply), 'Invalid CDP frame');
      if (!Object.hasOwn(reply, 'id')) {
        assert.equal(typeof reply.method, 'string', 'Invalid CDP event');
        return;
      }
      assert.ok(Number.isSafeInteger(reply.id) && waiting.has(reply.id), 'Unknown CDP response ID');
      const hasError = Object.hasOwn(reply, 'error');
      assert.notEqual(hasError, Object.hasOwn(reply, 'result'), 'Invalid CDP response');
      if (hasError) {
        assert.ok(reply.error && typeof reply.error === 'object' && !Array.isArray(reply.error)
          && Number.isSafeInteger(reply.error.code) && typeof reply.error.message === 'string',
          'Invalid CDP command error');
        // Unsupported commands (e.g. screenshot capture) have caller fallbacks.
        // A valid command error does not invalidate an otherwise usable transport.
        const pending = waiting.get(reply.id);
        waiting.delete(reply.id);
        pending.reject(new Error(`CDP command error: ${JSON.stringify(reply.error)}`));
      } else {
        assert.ok(reply.result && typeof reply.result === 'object' && !Array.isArray(reply.result),
          'Invalid CDP result');
        const pending = waiting.get(reply.id);
        waiting.delete(reply.id);
        pending.resolve(reply.result);
      }
    } catch (error) { fail(error); }
  };

  function handshake(scope) {
    return new Promise((resolve, reject) => {
      const cleanup = () => {
        ws.removeEventListener('open', opened);
        ws.removeEventListener('error', errored);
        ws.removeEventListener('close', closedDuringHandshake);
        scope.signal.removeEventListener('abort', aborted);
      };
      const rejectHandshake = error => { cleanup(); reject(fail(error)); };
      const errored = event => rejectHandshake(event.error ?? new Error('CDP WebSocket handshake error'));
      const closedDuringHandshake = () => rejectHandshake(new Error('CDP WebSocket closed before open'));
      const aborted = () => rejectHandshake(scope.signal.reason);
      const opened = () => {
        cleanup();
        if (ws.readyState !== 1) return rejectHandshake(new Error('CDP WebSocket did not stay open'));
        for (const entry of [['message', onMessage], ['error', onError], ['close', onClose]]) {
          listeners.push(entry);
          ws.addEventListener(...entry);
        }
        resolve();
      };
      ws.addEventListener('open', opened);
      ws.addEventListener('error', errored);
      ws.addEventListener('close', closedDuringHandshake);
      scope.signal.addEventListener('abort', aborted, { once: true });
      if (scope.signal.aborted) aborted();
    });
  }

  try {
    await deadline(async scope => {
      assertInstalledAndroidPackage(adb, app);
      const hookResult = beforeLaunch?.(adb);
      assert.ok(!hookResult || typeof hookResult.then !== 'function', 'beforeLaunch must be synchronous');
      checkBudget(scope);
      adb('shell', 'am', 'start', '-W', '-n', `${app}/${MAIN_ACTIVITY}`);
      pid = await until(() => {
        try { return adb('shell', 'pidof', app).trim(); }
        catch (error) {
          // pidof's empty exit 1 means not started yet; transport/unknown failures do not.
          if (error.status === 1 && !String(error.stdout ?? '').trim()
            && !String(error.stderr ?? '').trim() && !error.signal) return '';
          throw error;
        }
      }, value => /^\d+$/.test(value), timeout, runtime);
      try {
        // Record the allocated port before the post-command deadline check, so a
        // completed allocation that used the last budget is still owned and removed.
        port = runAdb(['forward', 'tcp:0', `localabstract:webview_devtools_remote_${pid}`], 20_000, false).trim();
      } catch (error) {
        const allocated = String(error.stdout ?? '').trim();
        if (/^\d+$/.test(allocated) && +allocated > 0 && +allocated <= 65535) {
          port = allocated;
          forwardOwned = true;
        }
        throw error;
      }
      assert.ok(/^\d+$/.test(port) && +port > 0 && +port <= 65535, 'Invalid allocated ADB forward port');
      forwardOwned = true;
      checkBudget(scope);
      const page = await until(async readScope => {
        const controller = new AbortController();
        const abort = () => controller.abort(readScope.signal.reason);
        const timer = runtime.setTimeout(() => controller.abort(expired('CDP discovery request')),
          Math.min(3_000, readScope.remaining()));
        readScope.signal.addEventListener('abort', abort, { once: true });
        try {
          let response;
          try {
            response = await runtime.fetch(`http://127.0.0.1:${port}/json`, { signal: controller.signal });
          } catch (error) {
            checkBudget(readScope);
            return null; // DevTools may not be listening yet; no permission result is inferred.
          }
          assert.ok(response.ok, `CDP discovery HTTP ${response.status}`);
          const pages = await response.json(); // Body read is cancelled by the same fetch signal.
          checkBudget(readScope);
          return mobilePage(pages);
        } finally {
          runtime.clearTimeout(timer);
          readScope.signal.removeEventListener('abort', abort);
          // Cancel an unread/error response body as well as an in-flight request.
          controller.abort();
        }
      }, Boolean, timeout, runtime);
      const endpoint = new URL(page.webSocketDebuggerUrl);
      assert.ok(['ws:', 'wss:'].includes(endpoint.protocol) && endpoint.hostname,
        'Invalid mobile WebView WebSocket URL');
      ws = runtime.WebSocket(endpoint.href);
      await handshake(scope);
    }, timeout, runtime, 'Android CDP connection');
  } catch (error) {
    throw fail(error);
  }

  async function call(method, params = {}) {
    if (failure) throw failure;
    if (closed) throw new Error('CDP session is closed');
    if (ws.readyState !== 1) throw fail(new Error('CDP WebSocket is not open'));
    const scope = observation.getStore();
    try { checkBudget(scope); } catch (error) { throw fail(error); }
    const id = ++nextId;
    return new Promise((resolve, reject) => {
      const abort = () => fail(scope.signal.reason);
      const timer = runtime.setTimeout(() => fail(new Error(`CDP ${method} 回执超时`)),
        Math.min(20_000, scope?.remaining() ?? Infinity));
      const settle = callback => value => {
        runtime.clearTimeout(timer);
        scope?.signal.removeEventListener('abort', abort);
        callback(value);
      };
      waiting.set(id, { resolve: settle(resolve), reject: settle(reject) });
      scope?.signal.addEventListener('abort', abort, { once: true });
      try { ws.send(JSON.stringify({ id, method, params })); }
      catch (error) { fail(error); }
    });
  }
  async function evaluate(expression) {
    const reply = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (reply.exceptionDetails) throw fail(new Error(`Runtime.evaluate failed: ${JSON.stringify(reply.exceptionDetails)}`));
    if (!reply.result || typeof reply.result.type !== 'string'
      || (!Object.hasOwn(reply.result, 'value') && reply.result.type !== 'undefined')) {
      throw fail(new Error('Runtime.evaluate did not return a by-value result'));
    }
    return reply.result.value;
  }
  const invoke = (command, args = {}) => evaluate(
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
  const close = primaryError => {
    if (closed) return;
    closed = true;
    rejectPending(primaryError ?? failure ?? new Error('CDP session closed'));
    const errors = release(primaryError);
    if (primaryError && failure && failure !== primaryError) addCleanupErrors(primaryError, [failure]);
    const diagnostic = failure ?? (errors.length ? new AggregateError(errors, 'CDP cleanup failed') : null);
    if (diagnostic && !failureReported) {
      failureReported = true;
      runtime.reportFailure(diagnostic);
    }
  };
  return { adb, pid, port, call, evaluate, invoke, close };
}
