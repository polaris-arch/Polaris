#!/usr/bin/env node
// Run after `pnpm exec vite build`: inspect the shipped mobile document in Chromium.
// This checks layout and input focus; native pinch/double-tap behavior needs the next APK.
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { chromium } from '@playwright/test';
import { preview } from 'vite';

const root = path.resolve(import.meta.dirname, '..');
const output = process.env.POLARIS_LAYOUT_OUTPUT || '/tmp/polaris-mobile-zoom-layout';
const variants = [
  { width: 320, height: 740, fontScale: 1 },
  { width: 390, height: 844, fontScale: 1 },
  { width: 768, height: 1024, fontScale: 1 },
  { width: 768, height: 440, fontScale: 1 },
  { width: 320, height: 480, fontScale: 2 },
  { width: 390, height: 844, fontScale: 2 },
  { width: 768, height: 600, fontScale: 2 },
];

assert.ok(existsSync(path.join(root, 'dist/mobile.html')), 'Run `pnpm exec vite build` first');
await mkdir(output, { recursive: true });
const server = await preview({ root, preview: { host: '127.0.0.1', port: 0 } });
const address = server.httpServer.address();
assert.ok(address && typeof address === 'object', 'Vite preview did not bind');
const chrome = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ||
  (existsSync('/usr/bin/google-chrome') ? '/usr/bin/google-chrome' : undefined);
let browser;
const results = [];

async function inspect(page, variant, screen, local = []) {
  const geometry = await page.evaluate((selectors) => {
    const doc = document.documentElement;
    return {
      width: innerWidth,
      document: { client: doc.clientWidth, scroll: doc.scrollWidth },
      body: { client: document.body.clientWidth, scroll: document.body.scrollWidth },
      font: getComputedStyle(doc).fontSize,
      local: selectors.map((selector) => {
        const el = document.querySelector(selector);
        if (!el) return { selector, missing: true };
        const rect = el.getBoundingClientRect();
        return { selector, client: el.clientWidth, scroll: el.scrollWidth,
          left: rect.left, right: rect.right, overflowX: getComputedStyle(el).overflowX };
      }),
    };
  }, local);
  const tag = `${variant.width}x${variant.height}-font${variant.fontScale}-${screen}`;
  assert.equal(geometry.width, variant.width, `${tag}: layout viewport changed`);
  assert.equal(geometry.font, `${16 * variant.fontScale}px`, `${tag}: system-font simulation not applied`);
  assert.ok(geometry.document.scroll - geometry.document.client <= 1, `${tag}: document overflows ${JSON.stringify(geometry)}`);
  assert.ok(geometry.body.scroll - geometry.body.client <= 1, `${tag}: body overflows ${JSON.stringify(geometry)}`);
  for (const item of geometry.local) {
    assert.ok(!item.missing, `${tag}: ${item.selector} missing`);
    assert.ok(item.left >= -1 && item.right <= variant.width + 1,
      `${tag}: ${item.selector} outside viewport ${JSON.stringify(item)}`);
    if (item.selector !== '.mn-segs') {
      assert.ok(item.scroll - item.client <= 1, `${tag}: ${item.selector} overflows ${JSON.stringify(item)}`);
    } else {
      assert.equal(item.overflowX, 'auto', `${tag}: node tabs must remain a local horizontal scroller`);
    }
  }
  await page.screenshot({ path: path.join(output, `${tag}.png`) });
  results.push({ tag, ...geometry });
  console.log(`PASS ${tag}`);
}

try {
  browser = await chromium.launch({ headless: true, ...(chrome ? { executablePath: chrome } : {}) });
  for (const variant of variants) {
    const page = await browser.newPage({ viewport: { width: variant.width, height: variant.height },
      isMobile: true, hasTouch: true, deviceScaleFactor: 2, locale: 'zh-CN' });
    try {
      await page.goto(`http://127.0.0.1:${address.port}/mobile.html`);
      await page.locator('.m-nav-item').first().waitFor();
      const viewport = await page.locator('meta[name="viewport"]').getAttribute('content');
      assert.match(viewport ?? '', /maximum-scale=1\.0.*user-scalable=no.*viewport-fit=cover/);
      await page.evaluate((scale) => document.documentElement.style.setProperty('--font-scale', String(scale)), variant.fontScale);
      await inspect(page, variant, 'home', ['.m-scroll']);

      for (const [label, screen] of [['节点', 'nodes'], ['分流', 'rules'], ['活动', 'activity'], ['设置', 'settings']]) {
        await page.locator('.m-nav-item').filter({ hasText: label }).click();
        await inspect(page, variant, screen, screen === 'nodes' ? ['.m-scroll', '.mn-segs'] : ['.m-scroll']);
        if (screen === 'nodes') {
          await page.locator('.mn-seg-label').first().evaluate((el) => {
            el.textContent = 'very_long_subscription_name_without_breaks_'.repeat(30);
          });
          const overflow = await page.locator('.mn-segs').evaluate((el) => el.scrollWidth - el.clientWidth);
          assert.ok(overflow > 1, 'long node tab did not exercise its local horizontal scroller');
          await inspect(page, variant, 'nodes-long-tab', ['.m-scroll', '.mn-segs']);
        }
      }

      await page.locator('.m-nav-item').filter({ hasText: '节点' }).click();
      await page.locator('.mn-act.primary').click();
      await page.locator('.mn-sheet-btn').filter({ hasText: '添加组网接入' }).click();
      await page.locator('.m-info-summary').first().evaluate((el) => {
        el.textContent = '一个很长且没有空格的提示示例'.repeat(12);
      });
      await inspect(page, variant, 'long-hint', ['.m-form-panel', '.m-form-body']);
      await page.locator('.m-form-choice-btn').filter({ hasText: 'Tailscale' }).click();
      await page.locator('.m-form-hint').first().evaluate((el) => {
        el.textContent = 'long_unbroken_translation_or_host_name_'.repeat(12);
      });
      const name = page.locator('.m-form-panel input').first();
      await name.fill('long-input-value.example.test'.repeat(8));
      assert.equal(await name.evaluate((el) => el === document.activeElement), true, 'form input lost focus');
      await inspect(page, variant, 'ts-form', ['.m-form-panel', '.m-form-body']);
    } finally {
      await page.close();
    }
  }
  await writeFile(path.join(output, 'geometry.json'), `${JSON.stringify(results, null, 2)}\n`);
  console.log(`Geometry and screenshots: ${output}`);
} finally {
  await browser?.close();
  await new Promise((resolve, reject) => server.httpServer.close((error) => error ? reject(error) : resolve()));
}
