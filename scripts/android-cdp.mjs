import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

const app = 'com.polaris2.app';

export async function until(read, test, timeout = 15_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const value = await read();
    if (test(value)) return value;
    await delay(250);
  }
  throw new Error('等待预期状态超时');
}

/** Attach to the installed Debug WebView. Call close() to remove the ADB forward. */
export async function connectAndroidWebView(serial) {
  assert.ok(serial && !serial.startsWith('-'), 'Explicit authorized ADB serial required');
  const adbPath = `${process.env.ANDROID_HOME ?? `${process.env.HOME}/Android/Sdk`}/platform-tools/adb`;
  const adb = (...args) => execFileSync(adbPath, ['-s', serial, ...args], { encoding: 'utf8', timeout: 20_000 });
  adb('shell', 'am', 'start', '-W', '-n', `${app}/.MainActivity`);
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
