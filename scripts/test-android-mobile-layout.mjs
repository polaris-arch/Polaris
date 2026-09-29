#!/usr/bin/env node
// Read-only WebView layout matrix on the installed Debug APK. No system display/font settings change.
// ANDROID_HOME=... node scripts/test-android-mobile-layout.mjs 192.168.x.x:port
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import { connectAndroidWebView, until } from './android-cdp.mjs';

const serial = process.argv[2];
const output = '/tmp/polaris-device-20260925/final-ui';
const viewports = [[320, 740], [400, 869], [600, 960], [840, 900], [1024, 768], [869, 400]];
const { call, evaluate, close } = await connectAndroidWebView(serial);
let originalFont;
let originalNav;
let overrideSet = false;
let runError;
let screenshotMode = 'cdp';
const adbPath = `${process.env.ANDROID_HOME ?? `${process.env.HOME}/Android/Sdk`}/platform-tools/adb`;

async function click(selector, label) {
  await until(() => evaluate(`(() => {
    const items = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const item = ${label === undefined
      ? 'items[0]'
      : `items.find(el => el.textContent?.trim() === ${JSON.stringify(label)})`};
    if (!item || item.disabled) return false;
    item.click(); return true;
  })()`), Boolean);
  await delay(100);
}

async function nav(label) {
  await until(() => evaluate(`(() => {
    const item = [...document.querySelectorAll('.m-nav-item')]
      .find(el => el.querySelector('.m-nav-label')?.textContent?.trim() === ${JSON.stringify(label)});
    if (!item) return false;
    item.click(); return true;
  })()`), Boolean);
  await delay(100);
}

async function snapshot(name) {
  if (screenshotMode === 'cdp') {
    try {
      const result = await call('Page.captureScreenshot', {
        format: 'png', fromSurface: false, captureBeyondViewport: false,
      });
      assert.ok(result.data, `No CDP screenshot for ${name}`);
      await writeFile(`${output}/${name}.png`, Buffer.from(result.data, 'base64'));
      return;
    } catch (error) {
      screenshotMode = 'native';
      console.warn(`CDP screenshot unavailable (${error.message}); subsequent .native.png files show the physical screen, not the emulated viewport. Matrix assertions still use CDP DOM geometry.`);
    }
  }
  const png = execFileSync(adbPath, ['-s', serial, 'exec-out', 'screencap', '-p'], {
    timeout: 20_000, maxBuffer: 32 * 1024 * 1024,
  });
  assert.equal(png.subarray(0, 8).toString('hex'), '89504e470d0a1a0a', `No native PNG for ${name}`);
  await writeFile(`${output}/${name}.native.png`, png);
}

async function check(screen, selectors, scopes = [], blocks = []) {
  const result = await evaluate(`(() => {
    const selectors = ${JSON.stringify(selectors)};
    const scopes = ${JSON.stringify(scopes)};
    const blocks = ${JSON.stringify(blocks)};
    const targets = selectors.map(selector => {
      const buttons = [...document.querySelectorAll(selector)]
        .filter(el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; });
      return { selector, count: buttons.length,
        small: buttons.map(el => el.getBoundingClientRect().height)
          .filter(height => height < 47.5).slice(0, 4).map(height => Math.round(height * 10) / 10) };
    });
    return { width: innerWidth, height: innerHeight,
      documentOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      scopeOverflow: scopes.map(selector => {
        const el = document.querySelector(selector);
        return { selector, overflow: el ? el.scrollWidth - el.clientWidth : null };
      }),
      blocks: blocks.map(selector => {
        const el = document.querySelector(selector);
        if (!el) return { selector, missing: true };
        const bounds = el.getBoundingClientRect();
        return { selector, left: bounds.left, right: bounds.right,
          width: bounds.width, height: bounds.height,
          overflow: el.scrollWidth - el.clientWidth };
      }), targets };
  })()`);
  assert.ok(result.documentOverflow <= 1, `${screen}: document overflow ${JSON.stringify(result)}`);
  for (const scope of result.scopeOverflow) {
    assert.ok(scope.overflow !== null && scope.overflow <= 1,
      `${screen}: ${scope.selector} overflow ${JSON.stringify(result)}`);
  }
  for (const block of result.blocks) {
    assert.ok(!block.missing && block.width > 0 && block.height > 0 &&
      block.left >= -1 && block.right <= result.width + 1 && block.overflow <= 1,
    `${screen}: ${block.selector} outside viewport or overflows ${JSON.stringify(result)}`);
  }
  for (const target of result.targets) {
    assert.ok(target.count > 0 && target.small.length === 0,
      `${screen}: touch target ${JSON.stringify(target)}`);
  }
  return result;
}

async function menu(screen, selector, optionSelector, minimumCount) {
  const result = await evaluate(`(() => {
    const pane = document.querySelector(${JSON.stringify(selector)});
    const options = [...document.querySelectorAll(${JSON.stringify(optionSelector)})];
    if (!pane) return null;
    const bounds = pane.getBoundingClientRect();
    const first = options[0]?.getBoundingClientRect();
    return { left: bounds.left, right: bounds.right, width: bounds.width,
      optionCount: options.length, firstVisible: !!first &&
        Math.min(first.bottom, innerHeight) - Math.max(first.top, 0) >= Math.min(first.height, 48) - 1 };
  })()`);
  assert.ok(result && result.left >= -1 && result.right <= (await evaluate('innerWidth')) + 1 &&
    result.width > 0 && result.optionCount >= minimumCount && result.firstVisible,
  `${screen}: menu geometry ${JSON.stringify(result)}`);
}

async function checkSelect(screen) {
  const result = await evaluate(`(() => {
    const panel = document.querySelector('.m-select-panel');
    const selected = panel?.querySelector('[role="option"][aria-selected="true"]');
    const options = [...(panel?.querySelectorAll('[role="option"]') ?? [])];
    if (!panel) return null;
    const bounds = panel.getBoundingClientRect();
    const selectedBounds = selected?.getBoundingClientRect();
    return {
      left: bounds.left, right: bounds.right, top: bounds.top, bottom: bounds.bottom,
      width: bounds.width, overflow: panel.scrollWidth - panel.clientWidth,
      selectedVisible: !!selectedBounds &&
        selectedBounds.top >= Math.max(bounds.top, 0) - 1 &&
        selectedBounds.bottom <= Math.min(bounds.bottom, innerHeight) + 1,
      optionCount: options.length,
      small: options.map(option => option.getBoundingClientRect().height)
        .filter(height => height < 47.5).slice(0, 4).map(height => Math.round(height * 10) / 10),
    };
  })()`);
  assert.ok(result && result.left >= -1 && result.right <= (await evaluate('innerWidth')) + 1 &&
    result.width > 0 && result.top >= -1 && result.bottom <= (await evaluate('innerHeight')) + 1 &&
    result.overflow <= 1 && result.selectedVisible && result.optionCount >= 2 && result.small.length === 0,
  `${screen}: select geometry ${JSON.stringify(result)}`);
}

try {
  await call('Page.enable');
  await mkdir(output, { recursive: true });
  originalFont = await evaluate(`(() => {
    const style = document.documentElement.style;
    return { value: style.getPropertyValue('--font-scale'), priority: style.getPropertyPriority('--font-scale') };
  })()`);
  originalNav = await evaluate('document.querySelector(\'.m-nav-item[aria-current="page"] .m-nav-label\')?.textContent?.trim() ?? "首页"');

  for (const scale of [1, 2]) {
    for (const [width, height] of viewports) {
      const tag = `${width}x${height}-font${scale}`;
      try {
        await call('Emulation.setDeviceMetricsOverride', {
          width, height, deviceScaleFactor: 1, mobile: true, screenWidth: width, screenHeight: height,
        });
        overrideSet = true;
      } catch (error) {
        throw new Error(`WebView CDP 不支持 Emulation.setDeviceMetricsOverride：${error.message}`);
      }
      await evaluate(`document.documentElement.style.setProperty('--font-scale', ${JSON.stringify(String(scale))})`);
      await until(() => evaluate('({ width: innerWidth, font: getComputedStyle(document.documentElement).fontSize })'),
        value => value.width === width && Math.abs(parseFloat(value.font) - 16 * scale) < 0.5);

      await nav('首页');
      await until(() => evaluate('!!document.querySelector(".h-rates")'), Boolean);
      await snapshot(`${tag}-home`);
      await check(`${tag} home`, ['.m-nav-item', '.h-act'], ['.m-scroll']);
      await click('.h-act[data-write-control="switch-node"]');
      await snapshot(`${tag}-home-picker`);
      await menu(`${tag} home picker`, '.h-sheet', '.h-pickrow', 2);
      await click('.h-sheetclose');

      await nav('节点');
      const subscriptionCount = await until(() => evaluate(`(() => {
        const groups = document.querySelectorAll('.mn-seg');
        if (groups.length < 3) return 0;
        groups[2].click();
        return Number(groups[2].querySelector('.mn-seg-count')?.textContent?.trim() ?? 0);
      })()`), count => count >= 100);
      await until(() => evaluate('document.querySelectorAll(".mn-row").length'), count => count >= 100);
      await snapshot(`${tag}-subscription`);
      await check(`${tag} subscription (${subscriptionCount} nodes)`,
        ['.m-nav-item', '.mn-act.primary', '.mn-row-main', '.mn-row-more'], ['.m-scroll'],
        ['.mn-top', '.mn-sub', '.mn-toolbar', '.mn-list']);
      await click('.mn-act.primary');
      await snapshot(`${tag}-add-menu`);
      await menu(`${tag} add menu`, '.mn-sheet', '.mn-sheet-btn', 2);
      await click('.mn-sheet-btn', '添加代理节点');
      await until(() => evaluate('!!document.querySelector(".m-form-panel")'), Boolean);
      await snapshot(`${tag}-node-form`);
      await check(`${tag} node form`, ['.m-nav-item', '.m-form-x'], ['.m-form-panel', '.m-form-body']);
      await click('.m-form-panel button[role="combobox"]');
      await until(() => evaluate('!!document.querySelector(".m-select-panel")'), Boolean);
      await snapshot(`${tag}-node-select`);
      await checkSelect(`${tag} node select`);
      await click('.m-select-close');
      assert.equal(await evaluate('!!document.querySelector(".m-select-panel")'), false,
        `${tag}: select panel remained open`);
      assert.equal(await evaluate('!!document.querySelector(".m-form-panel")'), true,
        `${tag}: node form closed with select`);
      await click('.m-form-x');

      await nav('设置');
      await until(() => evaluate('document.querySelectorAll("[data-push]").length'), count => count >= 4);
      await snapshot(`${tag}-settings`);
      await check(`${tag} settings`, ['.m-nav-item', '[data-push]'], ['.m-scroll']);

      await nav('连接');
      await click('.mc-seg-item', '日志');
      await until(() => evaluate('!!document.querySelector("[data-segment=logs]")'), Boolean);
      await snapshot(`${tag}-logs`);
      await check(`${tag} logs`, ['.m-nav-item', '.mc-seg-item'], ['.m-scroll'],
        ['.mc-top', '.mc-body', '.mc-logwrap', '.mc-logview']);
      await click('.mc-head button[aria-label="更多"]');
      await snapshot(`${tag}-logs-menu`);
      await menu(`${tag} logs menu`, '.mc-sheet', '.mc-sheet-item', 2);
      await click('.mc-sheet-item:last-child');
      console.log(`PASS ${tag}: home, ${subscriptionCount} nodes, form, settings, logs, menus`);
    }
  }
} catch (error) {
  runError = error;
  throw error;
} finally {
  const cleanupErrors = [];
  const restore = async (step, action) => {
    try { await action(); }
    catch (error) {
      cleanupErrors.push(error);
      console.error(`CDP cleanup ${step} failed: ${error.message}`);
    }
  };
  if (originalFont) await restore('font', () => evaluate(`(() => {
    const style = document.documentElement.style;
    if (${JSON.stringify(originalFont.value)} === '') style.removeProperty('--font-scale');
    else style.setProperty('--font-scale', ${JSON.stringify(originalFont.value)}, ${JSON.stringify(originalFont.priority)});
  })()`));
  if (overrideSet) await restore('viewport', () => call('Emulation.clearDeviceMetricsOverride'));
  if (originalNav) await restore('navigation', () => nav(originalNav));
  await restore('connection', () => close());
  if (!runError && cleanupErrors.length) throw new AggregateError(cleanupErrors, 'CDP cleanup failed');
}
