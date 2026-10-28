#!/usr/bin/env node
// 真 Android 系统弹窗回归：默认使用已安装的 Debug APK；Release 须显式传 --release。
// node scripts/test-android-vpn-permission.mjs emulator-5554 [--release]
// 仅接受模拟器。临时选内置直连节点，结束后还原选中项和 VPN app-op；不清应用数据。
// Node >= 22（内置 WebSocket），ANDROID_HOME 指向 SDK，无额外依赖。
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { connectAndroidWebView, parseAndroidQaTarget, until } from './android-cdp.mjs';

const { serial, app } = parseAndroidQaTarget(process.argv.slice(2));
assert.match(serial ?? '', /^emulator-\d+$/, '必须显式指定 Android 模拟器序列号');
let priorOp;
const { adb, evaluate, invoke, close } = await connectAndroidWebView(serial, app, {
  beforeLaunch(adb) {
    priorOp = /ACTIVATE_VPN: (\w+)/.exec(adb('shell', 'appops', 'get', app, 'ACTIVATE_VPN'))?.[1] ?? 'default';
  },
});
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
let testError;
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
  testError = error;
  console.error('FAIL:', error.message);
  throw error;
} finally {
  const cleanupErrors = [];
  const restore = async (step, action) => {
    try { await action(); }
    catch (error) {
      cleanupErrors.push(error);
      console.error(`测试状态还原失败 (${step}):`, error.message);
    }
  };
  // Each restoration is independent: a lost CDP session must not skip ADB cleanup.
  await restore('dialog', async () => {
    if (/topResumedActivity=.*com\.android\.vpndialogs\//.test(adb('shell', 'dumpsys', 'activity', 'activities'))) {
      await answer(false);
    }
  });
  if (selectionRead) {
    await restore('VPN stop', async () => assert.equal((await invoke('proxy_stop')).success, true));
    await restore('selection', async () => assert.equal(
      (await invoke('config_patch', { patch: { selectedServerId: selected } })).success, true));
  }
  await restore('app-op', () => adb('shell', 'appops', 'set', app, 'ACTIVATE_VPN', priorOp));
  await restore('UI dump', () => adb('shell', 'rm', '-f', '/sdcard/polaris-vpn-permission-test.xml'));
  const cleanupError = cleanupErrors.length ? new AggregateError(cleanupErrors, '测试状态还原失败') : undefined;
  if (testError && cleanupError) testError.cleanupErrors = [...(testError.cleanupErrors ?? []), cleanupError];
  close(testError ?? cleanupError);
  if (!testError && cleanupError) throw cleanupError;
}
