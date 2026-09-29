#!/usr/bin/env node
// 真 Android 系统弹窗回归：默认使用已安装的 Debug APK；Release 须显式传 --release。
// node scripts/test-android-vpn-permission.mjs emulator-5554 [--release]
// 仅接受模拟器。临时选内置直连节点，结束后还原选中项和 VPN app-op；不清应用数据。
// Node >= 22（内置 WebSocket），ANDROID_HOME 指向 SDK，无额外依赖。
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { assertInstalledAndroidPackage, MAIN_ACTIVITY, parseAndroidQaTarget } from './android-cdp.mjs';

const { serial, app } = parseAndroidQaTarget(process.argv.slice(2));
assert.match(serial ?? '', /^emulator-\d+$/, '必须显式指定 Android 模拟器序列号');
const adbPath = `${process.env.ANDROID_HOME ?? `${process.env.HOME}/Android/Sdk`}/platform-tools/adb`;
const adb = (...args) => execFileSync(adbPath, ['-s', serial, ...args], { encoding: 'utf8', timeout: 20_000 });
assertInstalledAndroidPackage(adb, app);
const priorOp = /ACTIVATE_VPN: (\w+)/.exec(adb('shell', 'appops', 'get', app, 'ACTIVATE_VPN'))?.[1] ?? 'default';
adb('shell', 'am', 'start', '-W', '-n', `${app}/${MAIN_ACTIVITY}`);
const pid = await until(() => {
  try { return adb('shell', 'pidof', app).trim(); } catch { return ''; }
}, value => value.length > 0);
const port = adb('forward', 'tcp:0', `localabstract:webview_devtools_remote_${pid}`).trim();
const pages = await until(async () => {
  try { return await (await fetch(`http://127.0.0.1:${port}/json`)).json(); } catch { return []; }
}, value => value.some(p => p.type === 'page'));
const ws = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.addEventListener('open', resolve, { once: true });
  ws.addEventListener('error', reject, { once: true });
});
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
async function evaluate(expression) {
  const id = ++nextId;
  const reply = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      waiting.delete(id);
      reject(new Error('CDP 回执超时'));
    }, 20_000);
    waiting.set(id, {
      resolve: r => { clearTimeout(timer); resolve(r); },
      reject: e => { clearTimeout(timer); reject(e); },
    });
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: {
      expression, awaitPromise: true, returnByValue: true,
    } }));
  });
  assert.ok(!reply.exceptionDetails, JSON.stringify(reply.exceptionDetails));
  return reply.result.value;
}
const invoke = (command, args = {}) => evaluate(
  `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
);
async function until(read, test, timeout = 15_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const value = await read();
    if (test(value)) return value;
    await delay(250);
  }
  throw new Error('等待预期状态超时');
}
async function foreground() {
  await until(() => adb('shell', 'dumpsys', 'activity', 'activities'),
    value => value.split('\n').some(line => line.includes('topResumedActivity=') && line.includes(`${app}/`)));
  await delay(350);
}
async function start(key = 'start') {
  await evaluate(`window.__vpnPermissionTest ??= {}; window.__vpnPermissionTest[${JSON.stringify(key)}] = null;
    window.__TAURI_INTERNALS__.invoke('proxy_start').then(
      r => window.__vpnPermissionTest[${JSON.stringify(key)}] = r,
      e => window.__vpnPermissionTest[${JSON.stringify(key)}] = { ipcError: String(e) });`);
}
const result = (key = 'start') => evaluate(`window.__vpnPermissionTest[${JSON.stringify(key)}]`);
const done = (key = 'start') => until(() => result(key), value => value !== null, 60_000);
const status = async () => (await invoke('proxy_get_status')).data;
async function dialog() {
  await until(() => adb('shell', 'dumpsys', 'activity', 'activities'),
    value => /topResumedActivity=.*com\.android\.vpndialogs\//.test(value));
}
async function answer(allow) {
  adb('shell', 'uiautomator', 'dump', '/sdcard/polaris-vpn-permission-test.xml');
  const xml = adb('shell', 'cat', '/sdcard/polaris-vpn-permission-test.xml');
  const button = new RegExp(`<node[^>]*resource-id="android:id/button${allow ? 1 : 2}"[^>]*bounds="\\[(\\d+),(\\d+)\\]\\[(\\d+),(\\d+)\\]"`).exec(xml);
  assert.ok(button, '系统 VPN 弹窗必须有确认/取消按钮');
  const [, x1, y1, x2, y2] = button.map(Number);
  adb('shell', 'input', 'tap', String(Math.round((x1 + x2) / 2)), String(Math.round((y1 + y2) / 2)));
  await foreground();
}
async function revoke() {
  assert.equal((await invoke('proxy_stop')).success, true);
  adb('shell', 'appops', 'set', app, 'ACTIVATE_VPN', 'default');
  assert.equal((await invoke('vpn_auth_status')).data, 'denied');
  await foreground();
}
let selected;
let selectionRead = false;
let testFailed = false;
try {
  await until(() => evaluate('typeof window.__TAURI_INTERNALS__?.invoke'), value => value === 'function');
  assert.equal((await status()).running, false, '测试开始前请先断开模拟器上的 VPN');
  console.log('已连接模拟器 WebView，开始授权回归');
  const config = await invoke('config_get');
  assert.equal(config.success, true);
  selected = config.data.selectedServerId ?? null;
  selectionRead = true;
  assert.equal((await invoke('config_patch', { patch: { selectedServerId: '__direct__' } })).success, true);

  await revoke();
  await start();
  await dialog();
  await answer(false);
  assert.equal((await done()).code, 'VPN_PERMISSION_DENIED');
  assert.equal((await status()).running, false);
  console.log('PASS: 未授权弹窗；取消后保持断开');

  await start();
  await dialog();
  await delay(32_000);
  assert.equal(await result(), null, '阅读授权弹窗不能触发 30 秒起核超时');
  await answer(true);
  const connected = await done();
  assert.equal(connected.success, true, JSON.stringify(connected));
  assert.equal((await status()).running, true);
  console.log('PASS: 等待超过 30 秒，同意后自动连接');

  await invoke('proxy_stop');
  await start();
  assert.equal((await done()).success, true);
  assert.equal((await status()).running, true);
  console.log('PASS: 已授权直接连接');

  await revoke();
  await start();
  await dialog();
  assert.equal((await invoke('proxy_stop')).success, true);
  await done();
  await answer(true);
  assert.equal((await status()).running, false);
  console.log('PASS: 停止后迟到的同意不能启动 VPN');

  await revoke();
  await start();
  await dialog();
  await start('duplicate');
  assert.equal((await done('duplicate')).code, 'VPN_PERMISSION_DENIED');
  await answer(true);
  await done();
  assert.equal((await status()).running, false);
  await start('retry');
  assert.equal((await done('retry')).success, true);
  assert.equal((await status()).running, true);
  console.log('PASS: 重复请求不叠窗；重试可连接');

  await revoke();
  await start();
  await dialog();
  console.log('检查两分钟授权超时及迟到回调…');
  await until(() => result(), value => value !== null, 130_000);
  assert.equal((await result()).code, 'VPN_PERMISSION_DENIED');
  await answer(true);
  assert.equal((await status()).running, false);
  await start('afterTimeout');
  assert.equal((await done('afterTimeout')).success, true);
  assert.equal((await status()).running, true);
  console.log('PASS: 授权超时后迟到回调不崩溃、不起核；再次连接成功');
} catch (error) {
  testFailed = true;
  console.error('FAIL:', error.message);
  throw error;
} finally {
  try {
    // 失败时可能仍有系统弹窗，先退回应用再还原测试状态。
    if (/topResumedActivity=.*com\.android\.vpndialogs\//.test(adb('shell', 'dumpsys', 'activity', 'activities'))) {
      await answer(false);
    }
    if (selectionRead) {
      await invoke('proxy_stop');
      await invoke('config_patch', { patch: { selectedServerId: selected } });
    }
    adb('shell', 'appops', 'set', app, 'ACTIVATE_VPN', priorOp);
    adb('shell', 'rm', '-f', '/sdcard/polaris-vpn-permission-test.xml');
  } catch (error) {
    console.error('测试状态还原失败:', error.message);
    if (!testFailed) throw error;
  } finally {
    ws.close();
    // 模拟器退出时 forward 已被 adb 清除，不覆盖原始失败。
    try { adb('forward', '--remove', `tcp:${port}`); } catch { /* device disconnected */ }
  }
}
