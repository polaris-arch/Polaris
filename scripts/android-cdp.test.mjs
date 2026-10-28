import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  assertInstalledAndroidPackage,
  connectAndroidWebView,
  DEBUG_APP,
  parseAndroidQaTarget,
  RELEASE_APP,
  until,
} from './android-cdp.mjs';

test('Android QA defaults to Debug and requires an explicit Release flag', () => {
  assert.deepEqual(parseAndroidQaTarget(['emulator-5554']),
    { serial: 'emulator-5554', app: DEBUG_APP });
  assert.deepEqual(parseAndroidQaTarget(['emulator-5554', '--release']),
    { serial: 'emulator-5554', app: RELEASE_APP });
  for (const args of [[], ['--release'], ['emulator-5554', '--debug'],
    ['emulator-5554', '--release', '--release'], ['emulator-5554', 'another-device']]) {
    assert.throws(() => parseAndroidQaTarget(args));
  }
});

test('installed-package preflight checks exact IDs before an Activity can launch', () => {
  const fakeAdb = output => (...args) => {
    assert.deepEqual(args, ['shell', 'pm', 'list', 'packages', 'com.polaris2.app']);
    return output;
  };
  const both = fakeAdb(`package:${RELEASE_APP}\npackage:${DEBUG_APP}\n`);
  assert.doesNotThrow(() => assertInstalledAndroidPackage(both, DEBUG_APP));
  assert.doesNotThrow(() => assertInstalledAndroidPackage(both, RELEASE_APP));
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(`package:${RELEASE_APP}\n`), DEBUG_APP),
    /com\.polaris2\.app\.debug is not installed.*pass --release/);
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(`package:${DEBUG_APP}\n`), RELEASE_APP),
    /com\.polaris2\.app is not installed.*omit --release/);
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(''), DEBUG_APP),
    /Neither Debug nor Release package is installed/);
  assert.throws(() => assertInstalledAndroidPackage(fakeAdb(`package:${DEBUG_APP}.other\n`), DEBUG_APP),
    /com\.polaris2\.app\.debug is not installed/);
  assert.throws(() => assertInstalledAndroidPackage(both, 'com.example.other'),
    /Unknown Android QA package/);
});

test('Android QA entrypoints reject the wrong variant and launch the native Activity class', () => {
  const sdk = mkdtempSync(join(tmpdir(), 'polaris-qa-adb-'));
  const calls = join(sdk, 'calls');
  try {
    mkdirSync(join(sdk, 'platform-tools'));
    writeFileSync(join(sdk, 'platform-tools', 'adb'),
      '#!/bin/sh\nprintf "%s\\n" "$*" >> "$ANDROID_QA_CALLS"\n'
      + 'if [ "$4" = "am" ] && [ "$5" = "start" ]; then exit 66; fi\n'
      + 'printf "%s\\n" "$ANDROID_QA_PACKAGES"\n',
      { mode: 0o755 });
    for (const script of ['test-android-mobile.mjs', 'test-android-mobile-layout.mjs',
      'test-android-vpn-permission.mjs']) {
      for (const { option, installed, missing } of [
        { option: [], installed: RELEASE_APP, missing: DEBUG_APP },
        { option: ['--release'], installed: DEBUG_APP, missing: RELEASE_APP },
      ]) {
        writeFileSync(calls, '');
        const result = spawnSync(process.execPath,
          [fileURLToPath(new URL(script, import.meta.url)), 'emulator-5554', ...option], {
            encoding: 'utf8',
            env: { ...process.env, ANDROID_HOME: sdk, ANDROID_QA_CALLS: calls,
              ANDROID_QA_PACKAGES: `package:${installed}` },
          });
        assert.equal(result.status, 1, `${script}: ${result.stderr}`);
        assert.ok(result.stderr.includes(`${missing} is not installed`), `${script}: ${result.stderr}`);
        assert.equal(readFileSync(calls, 'utf8'),
          '-s emulator-5554 shell pm list packages com.polaris2.app\n');

        const selected = option.length ? RELEASE_APP : DEBUG_APP;
        writeFileSync(calls, '');
        const launch = spawnSync(process.execPath,
          [fileURLToPath(new URL(script, import.meta.url)), 'emulator-5554', ...option], {
            encoding: 'utf8',
            env: { ...process.env, ANDROID_HOME: sdk, ANDROID_QA_CALLS: calls,
              ANDROID_QA_PACKAGES: `package:${selected}` },
          });
        assert.equal(launch.status, 1, `${script}: ${launch.stderr}`);
        const observed = readFileSync(calls, 'utf8').trim().split('\n');
        assert.deepEqual(observed, [
          '-s emulator-5554 shell pm list packages com.polaris2.app',
          ...(script === 'test-android-vpn-permission.mjs'
            ? [`-s emulator-5554 shell appops get ${selected} ACTIVATE_VPN`] : []),
          `-s emulator-5554 shell am start -W -n ${selected}/com.polaris2.app.MainActivity`,
        ]);
      }
    }
  } finally {
    rmSync(sdk, { recursive: true, force: true });
  }
});

// Inert transport controls: fake time, ADB, fetch and WebSocket only. These tests
// never contact a device, use a real socket, or sleep for the permission waits.
const flush = async () => {
  for (let i = 0; i < 40; i++) await Promise.resolve();
};

class FakeClock {
  time = 0;
  next = 0;
  timers = new Map();
  now = () => this.time;
  setTimeout = (callback, ms) => {
    const id = ++this.next;
    this.timers.set(id, { at: this.time + ms, callback });
    return id;
  };
  clearTimeout = id => this.timers.delete(id);
  async advance(ms) {
    const target = this.time + ms;
    for (;;) {
      await flush();
      const due = [...this.timers].filter(([, timer]) => timer.at <= target)
        .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!due) break;
      this.time = due[1].at;
      this.timers.delete(due[0]);
      due[1].callback();
    }
    this.time = target;
    await flush();
  }
}

class FakeSocket {
  readyState = 0;
  listeners = new Map();
  sent = [];
  closes = 0;
  sendError;
  closeError;
  addEventListener(type, handler) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type).add(handler);
  }
  removeEventListener(type, handler) { this.listeners.get(type)?.delete(handler); }
  emit(type, event = {}) {
    if (type === 'open') this.readyState = 1;
    if (type === 'close') this.readyState = 3;
    for (const handler of [...(this.listeners.get(type) ?? [])]) handler(event);
  }
  send(value) {
    if (this.sendError) throw this.sendError;
    this.sent.push(JSON.parse(value));
  }
  close() {
    this.closes++;
    this.readyState = 3;
    if (this.closeError) throw this.closeError;
  }
  reply(result, id = this.sent.at(-1).id) {
    this.emit('message', { data: JSON.stringify({ id, result }) });
  }
  get listenerCount() { return [...this.listeners.values()].reduce((n, set) => n + set.size, 0); }
}

const page = (changes = {}) => ({
  type: 'page', url: 'http://tauri.localhost/mobile.html',
  webSocketDebuggerUrl: 'ws://127.0.0.1:41234/devtools/page/mobile', ...changes,
});

function transport(options = {}) {
  const clock = new FakeClock();
  const calls = [];
  const sockets = [];
  const reports = [];
  const signals = [];
  const dependencies = {
    now: clock.now, setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    execFileSync(path, args, settings) {
      assert.ok(path.endsWith('/platform-tools/adb'));
      assert.deepEqual(args.slice(0, 2), ['-s', 'emulator-5554']);
      assert.equal(settings.killSignal, 'SIGKILL');
      assert.ok(settings.timeout > 0);
      const command = args.slice(2);
      calls.push({ command, settings });
      const override = options.adb?.(command, settings, clock);
      if (override !== undefined) return override;
      if (command[1] === 'pm') return `package:${DEBUG_APP}\n`;
      if (command[1] === 'pidof') return '1234\n';
      if (command[0] === 'forward' && command[1] === 'tcp:0') return '41234\n';
      return '';
    },
    async fetch(url, { signal }) {
      assert.equal(url, 'http://127.0.0.1:41234/json');
      signals.push(signal);
      if (options.fetch) return options.fetch(signal);
      return { ok: true, status: 200, json: async () => options.pages ?? [page()] };
    },
    WebSocket(url) {
      const socket = new FakeSocket();
      socket.url = url;
      sockets.push(socket);
      if (options.autoOpen !== false) queueMicrotask(() => socket.emit('open'));
      return socket;
    },
    reportFailure: error => reports.push(error),
  };
  const connect = (extra = {}) => connectAndroidWebView('emulator-5554', DEBUG_APP,
    { dependencies, ...extra });
  const removals = () => calls.filter(({ command }) => command[0] === 'forward' && command[1] === '--remove');
  const clean = () => {
    assert.equal(clock.timers.size, 0, 'all owned timers cleared');
    for (const socket of sockets) assert.equal(socket.listenerCount, 0, 'all owned listeners removed');
    assert.ok(calls.every(({ command }) => !command.includes('--remove-all')));
    for (const { command, settings } of removals()) {
      assert.deepEqual(command, ['forward', '--remove', 'tcp:41234']);
      assert.equal(settings.timeout, 3_000);
    }
  };
  return { clock, calls, sockets, signals, reports, dependencies, connect, removals, clean };
}

function waitForAbort(signal) {
  return new Promise((resolve, reject) => {
    const aborted = () => reject(signal.reason);
    if (signal.aborted) aborted();
    else signal.addEventListener('abort', aborted, { once: true });
  });
}

test('shared helper preserves preflight, synchronous app-op hook, launch and selected mobile page order', async () => {
  const fake = transport({ pages: [
    page({ type: 'worker' }), page({ url: 'http://tauri.localhost/other.html' }), page(),
  ] });
  const session = await fake.connect({ beforeLaunch: adb => adb('shell', 'appops', 'get', DEBUG_APP, 'ACTIVATE_VPN') });
  assert.deepEqual(fake.calls.slice(0, 4).map(({ command }) => command), [
    ['shell', 'pm', 'list', 'packages', 'com.polaris2.app'],
    ['shell', 'appops', 'get', DEBUG_APP, 'ACTIVATE_VPN'],
    ['shell', 'am', 'start', '-W', '-n', `${DEBUG_APP}/com.polaris2.app.MainActivity`],
    ['shell', 'pidof', DEBUG_APP],
  ]);
  assert.equal(fake.sockets[0].url, page().webSocketDebuggerUrl);
  assert.equal(session.close(), undefined, 'close stays synchronous');
  session.close();
  assert.equal(fake.removals().length, 1);
  assert.equal(fake.sockets[0].closes, 1);
  fake.clean();
});

test('hook failure never launches or allocates a forward', async () => {
  const fake = transport();
  const primary = new Error('prior app-op observation failed');
  await assert.rejects(fake.connect({ beforeLaunch() { throw primary; } }), error => error === primary);
  assert.equal(fake.calls.length, 1);
  assert.equal(fake.removals().length, 0);
  fake.clean();
});

test('connection ADB commands consume the one deadline and an expired allocation is still removed', async () => {
  const fake = transport({ adb(command, settings, clock) {
    if (command[0] === 'forward' && command[1] === '--remove') return '';
    if (command[1] === 'pm') clock.time += 2_000;
    if (command[1] === 'am') clock.time += 2_000;
    if (command[1] === 'pidof') clock.time += 2_000;
    if (command[0] === 'forward') clock.time += 10_000;
  } });
  await assert.rejects(fake.connect(), /connection deadline exceeded/);
  assert.deepEqual(fake.calls.slice(0, 4).map(({ settings }) => settings.timeout),
    [15_000, 13_000, 11_000, 9_000]);
  assert.equal(fake.removals().length, 1);
  assert.equal(fake.sockets.length, 0);
  fake.clean();
});

test('disconnected ADB observation fails immediately rather than becoming an empty pid', async () => {
  const primary = Object.assign(new Error('device disconnected'), { status: 1, stdout: '', stderr: 'device offline' });
  const fake = transport({ adb(command) { if (command[1] === 'pidof') throw primary; } });
  await assert.rejects(fake.connect(), error => error === primary);
  assert.equal(fake.calls.filter(({ command }) => command[1] === 'pidof').length, 1);
  assert.equal(fake.removals().length, 0);
  fake.clean();
});

test('hung discovery fetch is cancelled at the hard deadline and owned forward is removed', async () => {
  const fake = transport({ fetch: waitForAbort });
  const rejected = assert.rejects(fake.connect(), /deadline exceeded/);
  await fake.clock.advance(15_000);
  await rejected;
  assert.ok(fake.signals.length > 0 && fake.signals.every(signal => signal.aborted));
  assert.equal(fake.removals().length, 1);
  assert.equal(fake.sockets.length, 0);
  fake.clean();
});

test('hung discovery response body uses the fetch cancellation signal', async () => {
  const fake = transport({ fetch: signal => ({ ok: true, status: 200, json: () => waitForAbort(signal) }) });
  const rejected = assert.rejects(fake.connect(), /discovery request deadline exceeded/);
  await fake.clock.advance(3_000);
  await rejected;
  assert.equal(fake.signals[0].aborted, true);
  assert.equal(fake.removals().length, 1);
  fake.clean();
});

test('failed HTTP discovery cancels an unread body before releasing its forward', async () => {
  let bodyRead = false;
  const fake = transport({ fetch: () => ({ ok: false, status: 503,
    json: async () => { bodyRead = true; return []; } }) });
  await assert.rejects(fake.connect(), /HTTP 503/);
  assert.equal(bodyRead, false);
  assert.equal(fake.signals[0].aborted, true);
  assert.equal(fake.removals().length, 1);
  fake.clean();
});

for (const [label, pages] of [
  ['non-array discovery', {}], ['wrong page type', [page({ type: 'worker' })]],
  ['unrelated page', [page({ url: 'http://tauri.localhost/not-mobile.html' })]],
  ['invalid page URL', [page({ url: 'mobile.html' })]],
  ['invalid WebSocket URL', [page({ webSocketDebuggerUrl: 'https://example.invalid' })]],
]) {
  test(`${label} never falls back to another WebView`, async () => {
    const fake = transport({ pages });
    const rejected = assert.rejects(fake.connect());
    await fake.clock.advance(15_000);
    await rejected;
    assert.equal(fake.sockets.length, 0);
    assert.equal(fake.removals().length, 1);
    fake.clean();
  });
}

for (const event of ['timeout', 'error', 'close']) {
  test(`WebSocket handshake ${event} clears its timer/listeners and forward`, async () => {
    const fake = transport({ autoOpen: false });
    const rejected = assert.rejects(fake.connect(), /deadline exceeded|handshake error|closed before open/);
    await flush();
    assert.equal(fake.sockets.length, 1);
    if (event === 'timeout') await fake.clock.advance(15_000);
    else fake.sockets[0].emit(event);
    await rejected;
    assert.equal(fake.sockets[0].closes, 1);
    assert.equal(fake.removals().length, 1);
    fake.clean();
  });
}

test('connection cleanup failure remains secondary to the primary handshake failure', async () => {
  const removeError = new Error('remove failed');
  const closeError = new Error('socket close failed');
  const primary = new Error('handshake failed');
  const fake = transport({ autoOpen: false, adb(command) {
    if (command[1] === '--remove') throw removeError;
  } });
  const rejected = assert.rejects(fake.connect(), error => {
    assert.equal(error, primary);
    assert.deepEqual(error.cleanupErrors, [closeError, removeError]);
    return true;
  });
  await flush();
  fake.sockets[0].closeError = closeError;
  fake.sockets[0].emit('error', { error: primary });
  await rejected;
  assert.equal(fake.removals().length, 1);
  fake.clean();
});

for (const [label, trigger] of [
  ['close', socket => socket.emit('close', { code: 1006 })],
  ['error', socket => socket.emit('error', { error: new Error('transport lost') })],
  ['invalid JSON', socket => socket.emit('message', { data: '{' })],
  ['unknown response', socket => socket.reply({}, 999)],
  ['invalid result', socket => socket.reply(null)],
  ['malformed command error', socket => socket.emit('message', { data: JSON.stringify({ id: socket.sent[0].id, error: { message: 'missing code' } }) })],
]) {
  test(`session ${label} rejects every pending request and all subsequent observation`, async () => {
    const fake = transport();
    const session = await fake.connect();
    const first = session.call('First');
    const second = session.call('Second');
    const results = Promise.allSettled([first, second]);
    trigger(fake.sockets[0]);
    const failed = await results;
    assert.ok(failed.every(result => result.status === 'rejected'));
    assert.equal(failed[0].reason, failed[1].reason);
    await assert.rejects(session.invoke('vpn_auth_status'), error => error === failed[0].reason);
    assert.equal(fake.sockets[0].sent.length, 2, 'no request is sent after failure');
    session.close(failed[0].reason);
    session.close();
    assert.equal(fake.removals().length, 1);
    assert.equal(fake.reports.length, 1);
    fake.clean();
  });
}

test('valid screenshot command error permits fallback and later CDP evaluation', async () => {
  const fake = transport();
  const session = await fake.connect();
  const unrelated = session.call('Unrelated');
  const screenshot = assert.rejects(session.call('Page.captureScreenshot'), /unsupported screenshot/);
  const screenshotId = fake.sockets[0].sent.at(-1).id;
  fake.sockets[0].emit('message', { data: JSON.stringify({ id: screenshotId,
    error: { code: -32601, message: 'unsupported screenshot' } }) });
  await screenshot;
  assert.equal(fake.clock.timers.size, 1, 'only the unrelated call remains pending');
  assert.equal(fake.removals().length, 0);
  fake.sockets[0].reply({}, fake.sockets[0].sent[0].id);
  assert.deepEqual(await unrelated, {});
  const geometry = session.evaluate('document.body.clientWidth');
  fake.sockets[0].reply({ result: { type: 'number', value: 400 } });
  assert.equal(await geometry, 400);
  assert.equal(fake.clock.timers.size, 0);
  session.close();
  assert.equal(fake.reports.length, 0);
  fake.clean();
});

test('synchronous send failure clears both the current and other pending requests', async () => {
  const fake = transport();
  const session = await fake.connect();
  const earlier = session.call('Earlier');
  const primary = new Error('send failed');
  fake.sockets[0].sendError = primary;
  const failed = await Promise.allSettled([earlier, session.call('Broken')]);
  assert.ok(failed.every(result => result.status === 'rejected' && result.reason === primary));
  session.close(primary);
  assert.equal(fake.removals().length, 1);
  fake.clean();
});

test('request timeout invalidates the session and releases all pending timers', async () => {
  const fake = transport();
  const session = await fake.connect();
  const rejected = assert.rejects(session.call('NeverReplies'), /NeverReplies.*回执超时/);
  await fake.clock.advance(20_000);
  await rejected;
  await assert.rejects(session.call('Later'), /NeverReplies.*回执超时/);
  session.close();
  assert.equal(fake.removals().length, 1);
  fake.clean();
});

test('until cancellation closes a blocked CDP read instead of only racing its promise', async () => {
  const fake = transport();
  const session = await fake.connect();
  const rejected = assert.rejects(until(() => session.evaluate('neverSettles()'), Boolean,
    1_000, fake.dependencies), /deadline exceeded/);
  await fake.clock.advance(1_000);
  await rejected;
  assert.equal(fake.sockets[0].closes, 1);
  assert.equal(fake.removals().length, 1);
  await assert.rejects(session.call('Later'), /deadline exceeded/);
  session.close();
  fake.clean();
});

test('until caps synchronous ADB time and refuses a result observed after its budget', async () => {
  let expire = false;
  const fake = transport({ adb(command, settings, clock) {
    if (expire && command[1] === 'dumpsys') {
      assert.equal(settings.timeout, 500);
      clock.time += 501;
      return 'expected';
    }
  } });
  const session = await fake.connect();
  expire = true;
  await assert.rejects(until(() => session.adb('shell', 'dumpsys'), value => value === 'expected',
    500, fake.dependencies), /deadline exceeded/);
  session.close();
  fake.clean();
});

test('successful response clears its timer and Runtime exceptions never become business values', async () => {
  const fake = transport();
  const session = await fake.connect();
  const good = session.evaluate('1');
  fake.sockets[0].reply({ result: { type: 'number', value: 1 } });
  assert.equal(await good, 1);
  assert.equal(fake.clock.timers.size, 0);
  const bad = session.evaluate('throw Error()');
  const rejected = assert.rejects(bad, /Runtime.evaluate failed/);
  fake.sockets[0].reply({ exceptionDetails: { text: 'Uncaught' }, result: { type: 'undefined' } });
  await rejected;
  await assert.rejects(session.invoke('vpn_auth_status'), /Runtime.evaluate failed/);
  session.close();
  fake.clean();
});

test('missing Runtime by-value observation cannot be interpreted as permission denial', async () => {
  const fake = transport();
  const session = await fake.connect();
  const rejected = assert.rejects(session.invoke('vpn_auth_status'), /by-value result/);
  fake.sockets[0].reply({ result: { type: 'object', objectId: 'unavailable' } });
  await rejected;
  session.close();
  fake.clean();
});

test('explicit synchronous close rejects in-flight work and is idempotent', async () => {
  const fake = transport();
  const session = await fake.connect();
  const rejected = assert.rejects(session.call('Pending'), /session closed/);
  assert.equal(session.close(), undefined);
  await rejected;
  await assert.rejects(session.call('Later'), /session is closed/);
  session.close();
  assert.equal(fake.removals().length, 1);
  assert.equal(fake.reports.length, 0);
  fake.clean();
});

test('cleanup-only failure sets nonzero exit status without masking an existing finally exception', async () => {
  const primary = new Error('business assertion failed');
  const removeError = new Error('forward remove failed');
  const fake = transport({ adb(command) { if (command[1] === '--remove') throw removeError; } });
  delete fake.dependencies.reportFailure;
  const session = await fake.connect();
  const priorExit = process.exitCode;
  const priorError = console.error;
  const reports = [];
  try {
    console.error = (...args) => reports.push(args);
    assert.doesNotThrow(() => session.close(primary));
    assert.equal(process.exitCode, 1);
    assert.deepEqual(primary.cleanupErrors, [removeError]);
    assert.equal(reports.length, 1);
    session.close(primary);
    assert.equal(reports.length, 1);
  } finally {
    process.exitCode = priorExit;
    console.error = priorError;
  }
  assert.equal(fake.removals().length, 1);
  fake.clean();
});

test('permission entrypoint retains every negative control and the mandatory long waits', () => {
  const source = readFileSync(new URL('test-android-vpn-permission.mjs', import.meta.url), 'utf8');
  for (const label of [
    'PASS: 未授权弹窗；取消后保持断开',
    'PASS: 等待超过 30 秒，同意后自动连接',
    'PASS: 已授权直接连接',
    'PASS: 停止后迟到的同意不能启动 VPN',
    'PASS: 重复请求不叠窗；重试可连接',
    'PASS: 授权超时后迟到回调不崩溃、不起核；再次连接成功',
  ]) assert.ok(source.includes(label), label);
  assert.equal((source.match(/console\.log\('PASS:/g) ?? []).length, 6);
  assert.ok(source.includes('await delay(32_000);'));
  assert.ok(source.includes('await until(() => result(), value => value !== null, 130_000);'));
  assert.ok(source.includes('beforeLaunch(adb)'));
  assert.ok(!/new WebSocket|execFileSync|fetch\(/.test(source));
  for (const step of ['dialog', 'VPN stop', 'selection', 'app-op', 'UI dump']) {
    assert.ok(source.includes(`await restore('${step}'`), step);
  }
  assert.ok(source.includes('close(testError ?? cleanupError)'));
});

test('permission restoration attempts app-op and XML cleanup after independent CDP and dialog failures', () => {
  const directory = mkdtempSync(join(tmpdir(), 'polaris-permission-restore-'));
  const log = join(directory, 'calls.jsonl');
  try {
    // Execute the unchanged entrypoint against an inert sibling helper. The first
    // revoke fails before any long wait or Android interaction can be reached.
    writeFileSync(join(directory, 'permission.mjs'),
      readFileSync(new URL('test-android-vpn-permission.mjs', import.meta.url), 'utf8'));
    writeFileSync(join(directory, 'android-cdp.mjs'), `
      import { appendFileSync } from 'node:fs';
      const record = value => appendFileSync(process.env.RESTORE_CALLS, JSON.stringify(value) + '\\n');
      export const parseAndroidQaTarget = () => ({ serial: 'emulator-5554', app: '${DEBUG_APP}' });
      export const until = async read => read();
      export async function connectAndroidWebView(serial, app, { beforeLaunch }) {
        let patches = 0;
        let stops = 0;
        const adb = (...args) => {
          record(['adb', ...args]);
          if (args[1] === 'appops' && args[2] === 'get') return 'ACTIVATE_VPN: allow';
          if (args[1] === 'dumpsys') throw new Error('dialog observation failed');
          if (args[1] === 'appops') throw new Error('app-op restore failed');
          if (args[1] === 'rm') throw new Error('XML removal failed');
          throw new Error('unexpected fake ADB operation');
        };
        beforeLaunch(adb);
        return {
          adb, evaluate: async () => 'function',
          invoke: async (command, args) => {
            record(['invoke', command, args]);
            if (command === 'proxy_get_status') return { success: true, data: { running: false } };
            if (command === 'config_get') return { success: true, data: { selectedServerId: 'original-node' } };
            if (command === 'config_patch') return { success: ++patches === 1 };
            if (command === 'proxy_stop') throw new Error(++stops === 1 ? 'primary business failure' : 'stop restore failed');
            throw new Error('unexpected fake CDP operation');
          },
          close: error => record(['close', error?.message, error?.cleanupErrors?.[0]?.errors?.length]),
        };
      }
    `);
    const result = spawnSync(process.execPath, [join(directory, 'permission.mjs'), 'emulator-5554'], {
      encoding: 'utf8', timeout: 5_000, env: { ...process.env, RESTORE_CALLS: log },
    });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /Error: primary business failure/);
    const calls = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line));
    assert.deepEqual(calls.slice(-6), [
      ['adb', 'shell', 'dumpsys', 'activity', 'activities'],
      ['invoke', 'proxy_stop', null],
      ['invoke', 'config_patch', { patch: { selectedServerId: 'original-node' } }],
      ['adb', 'shell', 'appops', 'set', DEBUG_APP, 'ACTIVATE_VPN', 'allow'],
      ['adb', 'shell', 'rm', '-f', '/sdcard/polaris-vpn-permission-test.xml'],
      ['close', 'primary business failure', 5],
    ]);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
