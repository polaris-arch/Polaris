#!/usr/bin/env node
// Authorized-device regression. Explicit serial required; preserves selection and requires VPN stopped.
// Uses the installed Debug APK; does not upload reports or save test nodes/subscriptions.
// ANDROID_HOME=... node scripts/test-android-mobile.mjs 192.168.x.x:port
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { connectAndroidWebView, until } from './android-cdp.mjs';
const serial = process.argv[2];
const app = 'com.polaris2.app';
const { adb, pid, call, evaluate, invoke, close } = await connectAndroidWebView(serial);

const status = async () => (await invoke('proxy_get_status')).data;
const click = async (text, selector = 'button') => {
  await until(() => evaluate(`(() => {const b = [...document.querySelectorAll(${JSON.stringify(selector)})].find(b => b.innerText.trim() === ${JSON.stringify(text)}); if (!b) return false; b.click(); return true;})()`), Boolean);
  await delay(150);
};
const clickSheet = async text => {
  await until(() => evaluate(`(() => {
    const b = [...document.querySelectorAll('.mc-sheet-item')]
      .find(b => b.querySelector('.mc-sheet-item-tx > span')?.textContent?.trim() === ${JSON.stringify(text)});
    if (!b || b.disabled) return false;
    b.click(); return true;
  })()`), Boolean);
  await delay(150);
};
const home = async () => {
  await evaluate('if (!document.querySelector(".m-nav-item")) document.querySelector("button[aria-label=返回]")?.click()');
  await click('首页', '.m-nav-item');
};
const alive = () => assert.equal(adb('shell', 'pidof', app).trim(), pid, 'App process restarted/crashed');
let selected, restore = false, focusEmulated = false;
try {
  assert.equal((await invoke('version_get_info')).data.debugReportAvailable, true);
  assert.equal((await status()).running, false, 'Start with the device VPN stopped');
  const config = await invoke('config_get');
  assert.equal(config.success, true);
  selected = config.data.selectedServerId ?? null;
  restore = true;
  assert.equal((await invoke('config_patch', {patch: {selectedServerId: '__direct__'}})).success, true);
  await home();
  const empty = await evaluate(`(() => {
    const text = document.querySelector('.h-plotwrap.empty .h-empty');
    const rate = document.querySelector('.h-rates');
    return {visible: !!text, gap: text ? text.getBoundingClientRect().top - rate.getBoundingClientRect().bottom : -1,
      axes: document.querySelectorAll('.h-xaxis').length,
      width: innerWidth, scroll: document.documentElement.scrollWidth};
  })()`);
  assert.ok(empty.visible && empty.gap > 0 && empty.axes === 0 && empty.width === empty.scroll, JSON.stringify(empty));
  console.log('PASS stopped traffic layout', JSON.stringify(empty));

  // This WebView can have a focused Activity while CDP reports document.hasFocus() false.
  // Keep the document focused only while checking real keyboard focus on form fields.
  await call('Emulation.setFocusEmulationEnabled', { enabled: true });
  focusEmulated = true;
  await until(() => evaluate('document.hasFocus()'), Boolean);
  await click('节点', '.m-nav-item');
  for (const kind of ['添加代理节点', '添加订阅']) {
    await click('添加'); await click(kind);
    await until(() => evaluate('!!document.querySelector(".m-form-panel")'), Boolean);
    // Programmatic focus after a pointer click does not necessarily activate :focus-visible.
    // A real keyboard event changes Chromium's input modality without editing user data.
    await call('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    await call('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    const form = await evaluate(`(() => {
      const panels = [...document.querySelectorAll('.m-form-panel')]
        .filter(panel => panel.getBoundingClientRect().width > 0);
      const panel = panels.at(-1);
      const field = [...(panel?.querySelectorAll('input.m-form-input, textarea.m-form-input') ?? [])]
        .find(el => {
          const rect = el.getBoundingClientRect();
          const style = getComputedStyle(el);
          return !el.disabled && !el.hidden && rect.width > 0 && rect.height > 0 &&
            style.display !== 'none' && style.visibility === 'visible';
        });
      if (!field) return { found: false };
      field.focus();
      const body = field.closest('.m-form-body');
      const rect = field.getBoundingClientRect();
      const clip = body.getBoundingClientRect();
      const style = getComputedStyle(field);
      return {width: innerWidth, scroll: document.documentElement.scrollWidth,
        found: true, tag: field.tagName, id: field.id,
        focused: document.activeElement === field, focusVisible: field.matches(':focus-visible'),
        formWidth: body.clientWidth, formScroll: body.scrollWidth,
        focusOffset: style.outlineOffset, outlineWidth: style.outlineWidth,
        outlineStyle: style.outlineStyle,
        fieldWidth: rect.width, fieldHeight: rect.height,
        ringEdgesVisible: [
          rect.left >= Math.max(clip.left, 0) - 1,
          rect.right <= Math.min(clip.right, innerWidth) + 1,
          rect.top >= Math.max(clip.top, 0) - 1,
          rect.bottom <= Math.min(clip.bottom, innerHeight) + 1,
        ]};
    })()`);
    assert.ok(form.found && form.focused && form.focusVisible,
      `${kind}: visible text field did not receive keyboard focus ${JSON.stringify(form)}`);
    assert.ok(form.width === form.scroll && form.formWidth === form.formScroll, JSON.stringify(form));
    assert.ok(form.outlineStyle === 'solid' && form.outlineWidth === '2px' &&
      form.focusOffset === '-2px' && form.fieldWidth > 0 && form.fieldHeight > 0 &&
      form.ringEdgesVisible.every(Boolean), `${kind}: focus ring clipped ${JSON.stringify(form)}`);
    await evaluate('document.activeElement.blur(); document.querySelector(".m-form-x").click()');
    await delay(150);
    console.log('PASS', kind, JSON.stringify(form));
  }
  await call('Emulation.setFocusEmulationEnabled', { enabled: false });
  focusEmulated = false;

  await home();
  for (let cycle = 0; cycle < 5; cycle++) {
    assert.equal((await invoke('proxy_start')).success, true, `direct start ${cycle}`);
    assert.equal((await status()).running, true);
    for (let hop = 0; hop < 5; hop++) {
      await click('连接', '.m-nav-item'); await home();
      await click('设置', '.m-nav-item'); await home();
    }
    assert.equal(await evaluate('!!document.querySelector(".h-rates")'), true, 'Home render failed');
    assert.equal((await invoke('proxy_stop')).success, true);
    alive();
    console.log('PASS direct start/stop and navigation cycle', cycle + 1);
  }
  await click('连接', '.m-nav-item');
  await click('日志', '.mc-seg-item');
  await until(() => evaluate(`(() => {
    const b = document.querySelector('.mc-head button[aria-label="更多"]');
    if (!b) return false;
    b.click(); return true;
  })()`), Boolean);
  await clickSheet('导出选项');
  await clickSheet('导出故障报告');
  await until(() => adb('shell', 'dumpsys', 'activity', 'activities'), s => /topResumedActivity=.*(?:ChooserActivity|ResolverActivity)/.test(s), 20000);
  const reports = adb('shell', 'run-as', app, 'ls', 'cache/debug-reports');
  assert.match(reports, /polaris-debug-.*\.md/);
  console.log('PASS native debug report generated and system share chooser opened');
  // Some Xiaomi builds block injected keys; bringing our own Activity forward
  // avoids requesting broad input permissions just to leave the share chooser.
  adb('shell', 'am', 'start', '-n', `${app}/.MainActivity`);
  alive();
} finally {
  try {
    if (restore) {
      await evaluate('document.querySelector(".m-form-x")?.click()');
      await invoke('proxy_stop');
      assert.equal((await invoke('config_patch', {patch: {selectedServerId: selected}})).success, true);
      await home();
    }
  } finally {
    try {
      if (focusEmulated) await call('Emulation.setFocusEmulationEnabled', { enabled: false });
    } finally {
      close();
    }
  }
}
