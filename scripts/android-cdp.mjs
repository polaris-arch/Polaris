import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

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

export async function until(read, test, timeout = 15_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const value = await read();
    if (test(value)) return value;
    await delay(250);
  }
  throw new Error('等待预期状态超时');
}

/** Attach to the selected installed WebView. Call close() to remove the ADB forward. */
export async function connectAndroidWebView(serial, app = DEBUG_APP) {
  assert.ok(serial && !serial.startsWith('-'), 'Explicit authorized ADB serial required');
  const adbPath = `${process.env.ANDROID_HOME ?? `${process.env.HOME}/Android/Sdk`}/platform-tools/adb`;
  const adb = (...args) => execFileSync(adbPath, ['-s', serial, ...args], { encoding: 'utf8', timeout: 20_000 });
  assertInstalledAndroidPackage(adb, app);
  adb('shell', 'am', 'start', '-W', '-n', `${app}/${MAIN_ACTIVITY}`);
  const pid = await until(() => { try { return adb('shell', 'pidof', app).trim(); } catch { return ''; } }, Boolean);
  const port = adb('forward', 'tcp:0', `localabstract:webview_devtools_remote_${pid}`).trim();
  let ws;
  try {
    const pages = await until(async () => {
      try { return await (await fetch(`http://127.0.0.1:${port}/json`, { signal: AbortSignal.timeout(3000) })).json(); }
      catch { return []; }
    }, value => value.some(page => page.url.includes('mobile.html')));
    ws = new WebSocket(pages.find(page => page.url.includes('mobile.html')).webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      ws.addEventListener('open', resolve, { once: true });
      ws.addEventListener('error', reject, { once: true });
    });
  } catch (error) {
    ws?.close();
    adb('forward', '--remove', `tcp:${port}`);
    throw error;
  }

  let nextId = 0;
  const waiting = new Map();
  ws.addEventListener('message', event => {
    const reply = JSON.parse(event.data);
    const pending = waiting.get(reply.id);
    if (!pending) return;
    waiting.delete(reply.id);
    if (reply.error) pending.reject(new Error(JSON.stringify(reply.error)));
    else pending.resolve(reply.result);
  });
  async function call(method, params = {}) {
    const id = ++nextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        waiting.delete(id);
        reject(new Error(`CDP ${method} 回执超时`));
      }, 20_000);
      waiting.set(id, {
        resolve: result => { clearTimeout(timer); resolve(result); },
        reject: error => { clearTimeout(timer); reject(error); },
      });
      ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async function evaluate(expression) {
    const reply = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    assert.ok(!reply.exceptionDetails, JSON.stringify(reply.exceptionDetails));
    return reply.result.value;
  }
  const invoke = (command, args = {}) => evaluate(
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
  const close = () => {
    ws.close();
    adb('forward', '--remove', `tcp:${port}`);
  };
  return { adb, pid, port, call, evaluate, invoke, close };
}
