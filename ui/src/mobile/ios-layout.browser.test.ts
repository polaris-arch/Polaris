/** Real Chromium/CSS fixtures exercise the native-data consumer; these are not iPhone Duo runtime claims. */
import { beforeAll, afterAll, describe, expect, it } from 'vitest';
import { chromium, type Browser, type Page } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { stripTypeScriptTypes } from 'node:module';
import { fileURLToPath } from 'node:url';
import { cssChain, resolveChromeBinary } from '@/styles/css-oracle.test-support';
import * as ts from '@/test/ts-compiler';
import type { ReservedRegion } from './ios-layout';

let browser: Browser, css: string;
const measurements: unknown[] = [];
let cssInputs: unknown[] = [];
const sha256 = (text: string): string => createHash('sha256').update(text).digest('hex');
const runtime = stripTypeScriptTypes(readFileSync(new URL('./ios-layout.ts', import.meta.url), 'utf8'));
// Execute the real entry's native layout block, including its keyboard/focus
// scheduling. Only the Tauri platform providers are fixture bindings.
const entryPath = fileURLToPath(new URL('./MobileMain.tsx', import.meta.url));
const entrySource = readFileSync(entryPath, 'utf8');
const entryBlocks = ts.parseSourceFile(entryPath, entrySource).statements.filter((node) =>
  ts.isIfStatement(node) && entrySource.slice(node.expression.pos, node.expression.end).trim() === "isTauri() && platform() === 'ios'");
if (entryBlocks.length !== 1) throw new Error('Expected exactly one production iOS layout initializer');
const entry = stripTypeScriptTypes(entrySource.slice(entryBlocks[0].pos, entryBlocks[0].end));
beforeAll(async () => {
  const chain = await cssChain('mobile');
  css = chain.map((unit) => unit.css).join('\n');
  cssInputs = chain.map((unit) => ({ file: unit.file, compiledSha256: sha256(unit.css) }));
  browser = await chromium.launch({ executablePath: resolveChromeBinary(), headless: true });
});
afterAll(async () => {
  await browser?.close();
  if (process.env.POLARIS_IOS_LAYOUT_EVIDENCE) writeFileSync(process.env.POLARIS_IOS_LAYOUT_EVIDENCE,
    JSON.stringify({ scope: 'Chromium fixtures: synthetic markup/native regions/visual viewport, actual production JS blocks and CSS chain; not Duo/WKWebView acceptance',
      browser: browser.version(), moduleSha256: sha256(runtime), entryBlockSha256: sha256(entry), cssInputs, measurements }, null, 2) + '\n');
});

const shell = `<div id="root"><div class="m-shell" data-navigation="visible">
  <div class="m-screen-viewport"><div class="m-scroll"><div class="m-page"><section><div style="height:180px"></div>
    <header class="h-header"><div class="h-brandline"><span class="h-wordmark">Polaris</span></div><div class="h-header-actions"><span class="h-runchip">Stopped</span><button class="h-connect-action" data-write-control="connect">Connect</button></div></header>
    <div style="height:1800px">Continuous list content</div></section></div></div></div>
  <div class="m-dock"><div class="m-toast-host"><div class="m-toast">Feedback</div></div>
    <div class="m-pending-slot"><div class="m-pending"><span class="m-pending-text">Pending choice</span><button class="m-pending-btn">Apply</button></div></div>
    <nav class="m-nav">${['Home','Nodes','Rules','Connections','Settings'].map((name) => `<button class="m-nav-item"><span class="m-nav-label">${name}</span></button>`).join('')}</nav>
  </div></div></div>`;
const form = `<div class="m-form-layer"><button class="m-form-scrim"></button><section class="m-form-panel">
  <div class="m-form-grip"></div><div class="m-sheet-heading m-form-head"><h2 class="m-sheet-title m-form-title">Edit draft</h2><button class="m-sheet-close m-form-x">Close</button></div>
  <div class="m-form-body"><input id="draft" class="m-form-input" value="original"><div style="height:140px">Fields</div></div>
  <footer class="m-form-foot"><button class="m-form-btn">Cancel</button><button class="m-form-btn primary">Save</button></footer></section></div>`;

async function fixture(options: { side?: string; dir?: string; platform?: string; withForm?: boolean } = {}): Promise<Page> {
  const page = await browser.newPage({ viewport: { width: 800, height: 800 } });
  await page.setContent(`<html dir="${options.dir ?? 'ltr'}" data-mobile-os="${options.platform ?? 'ios'}" data-ios-layout-source="uikit-viewport" data-ios-vertical-bar-side="${options.side ?? 'unspecified'}" data-ios-horizontal-size-class="compact" data-ios-vertical-size-class="regular"><head><style>${css}</style></head><body>${shell}${options.withForm ? form : ''}</body></html>`);
  await page.evaluate(() => {
    for (const edge of ['t','r','b','l']) document.documentElement.style.setProperty(`--safe-${edge}`, '0px');
    (window as unknown as { __polarisIosViewport: unknown }).__polarisIosViewport = { sequence: 1, value: { width: 800, height: 800, verticalBarSide: 'unspecified', reservedRegions: [] } };
  });
  await page.addScriptTag({ type: 'module', content: `${runtime}\nimport.meta.hot = { dispose: (callback) => { window.__disposeLayout = callback; } };\nconst isTauri = () => true; const platform = () => ${JSON.stringify(options.platform ?? 'ios')};\n${entry}` });
  await settle(page);
  return page;
}

async function settle(page: Page): Promise<void> {
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(() => resolve())))));
}

async function regions(page: Page, value: ReservedRegion[]): Promise<void> {
  await page.evaluate((reservedRegions) => {
    const snapshot = (window as unknown as { __polarisIosViewport: { sequence: number; value: { reservedRegions: unknown[] } } }).__polarisIosViewport;
    snapshot.value.reservedRegions = reservedRegions;
    snapshot.sequence += 1;
    window.dispatchEvent(new Event('polaris-ios-viewport-change'));
  }, value);
  await settle(page);
}

async function overlaps(page: Page, selector: string, value: ReservedRegion[]): Promise<boolean> {
  return page.locator(selector).evaluate((el, items) => {
    const a = el.getBoundingClientRect();
    return items.some((r) => (r.width === 0 ? a.left < r.x && a.right > r.x : a.left < r.x + r.width && a.right > r.x) &&
      (r.height === 0 ? a.top < r.y && a.bottom > r.y : a.top < r.y + r.height && a.bottom > r.y));
  }, value);
}

async function observe(page: Page, name: string, selectors: string[], extra: Record<string, unknown> = {}): Promise<void> {
  measurements.push({ name, ...extra, ...await page.evaluate((items) => ({
    native: window.__polarisIosViewport,
    viewport: window.visualViewport && { x: window.visualViewport.offsetLeft, y: window.visualViewport.offsetTop,
      width: window.visualViewport.width, height: window.visualViewport.height, scale: window.visualViewport.scale },
    focus: { id: document.activeElement?.id, value: document.activeElement instanceof HTMLInputElement ? document.activeElement.value : null },
    elements: items.map((selector) => {
      const el = document.querySelector<HTMLElement>(selector);
      if (!el) throw new Error(`No observed fixture element: ${selector}`);
      return { selector, rect: el.getBoundingClientRect().toJSON(), scrollTop: el.scrollTop,
        overflowY: getComputedStyle(el).overflowY, offsetParent: el.offsetParent?.className ?? null, dataset: { ...el.dataset } };
    }),
  }), selectors) });
}

describe('local UIKit reserved-region consumption', () => {
  it('keeps compact UIKit navigation at the bottom even across the Android desktop breakpoint', async () => {
    const page = await fixture();
    try {
      await page.setViewportSize({ width: 1100, height: 800 });
      await settle(page);
      expect(await page.locator('.m-nav').evaluate(el => getComputedStyle(el).position)).toBe('static');
      expect(await page.locator('.m-nav').evaluate(el => getComputedStyle(el).flexDirection)).toBe('row');
      const nav = (await page.locator('.m-nav').boundingBox())!;
      const content = (await page.locator('.m-screen-viewport').boundingBox())!;
      expect(nav.y).toBeGreaterThanOrEqual(content.y + content.height);
      const geometry = await page.locator('.m-nav').evaluate(el => ({
        width: el.clientWidth, scrollWidth: el.scrollWidth,
        bounds: el.getBoundingClientRect().toJSON(),
        items: [...el.querySelectorAll('.m-nav-item')].map(item => item.getBoundingClientRect().toJSON()),
      }));
      measurements.push({ name: 'compact-wide-nav-items', geometry });
      expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.width);
      expect(await page.locator('.m-nav-item').first().evaluate(el => {
        const indicator = getComputedStyle(el, '::before');
        return { width: indicator.width, height: indicator.height, top: indicator.top };
      })).toEqual({ width: '28px', height: '2px', top: '0px' });
      for (const item of geometry.items) {
        expect(item.left).toBeGreaterThanOrEqual(geometry.bounds.left);
        expect(item.right).toBeLessThanOrEqual(geometry.bounds.right);
      }
    } finally { await page.close(); }
  });
  it('preserves the 554 Android left navigation at its existing breakpoint', async () => {
    const page = await fixture({ platform: 'android' });
    try {
      await page.setViewportSize({ width: 1100, height: 800 });
      await settle(page);
      expect(await page.locator('.m-nav').evaluate(el => getComputedStyle(el).position)).toBe('fixed');
      expect((await page.locator('.m-nav').boundingBox())!.x).toBe(0);
      expect(await page.locator('.m-shell').evaluate(el => getComputedStyle(el).paddingLeft)).toBe('64px');
    } finally { await page.close(); }
  });
  it('keeps the wordmark, status and empty chart readable at maximum text scaling', async () => {
    const page = await fixture();
    try {
      await page.setViewportSize({ width: 390, height: 844 });
      await page.evaluate(() => {
        document.documentElement.style.setProperty('--font-scale', '3.12');
        const chart = document.createElement('div');
        chart.className = 'h-plotwrap empty';
        chart.innerHTML = '<div class="h-empty">No traffic samples are available while disconnected</div>';
        document.querySelector('.m-page')!.append(chart);
      });
      await settle(page);
      const brand = (await page.locator('.h-brandline').boundingBox())!;
      const actions = (await page.locator('.h-header-actions').boundingBox())!;
      expect(actions.y).toBeGreaterThanOrEqual(brand.y + brand.height);
      const chart = (await page.locator('.h-plotwrap').boundingBox())!;
      const message = (await page.locator('.h-empty').boundingBox())!;
      expect(message.y).toBeGreaterThanOrEqual(chart.y);
      expect(message.y + message.height).toBeLessThanOrEqual(chart.y + chart.height);
    } finally { await page.close(); }
  });

  it.each(['ltr','rtl'])('physical bar side is identical under %s text direction', async (dir) => {
    for (const side of ['left','right']) {
      const page = await fixture({ dir, side });
      try {
        await page.evaluate(() => {
          document.documentElement.style.setProperty('--safe-l', '12px');
          document.documentElement.style.setProperty('--safe-r', '28px');
        });
        await settle(page);
        const nav = (await page.locator('.m-nav').boundingBox())!, scroll = (await page.locator('.m-scroll').boundingBox())!;
        expect(side === 'left' ? nav.x + nav.width <= scroll.x + 1 : nav.x >= scroll.x + scroll.width - 1).toBe(true);
        expect(await page.locator('.m-scroll').evaluate((el) => getComputedStyle(el).direction)).toBe(dir);
        const padding = await page.locator('.m-nav').evaluate((el) => [getComputedStyle(el).paddingLeft, getComputedStyle(el).paddingRight]);
        expect(padding).toEqual(side === 'left' ? ['12px','0px'] : ['0px','28px']);
        await observe(page, `physical-side-${side}-${dir}`, ['.m-nav','.m-scroll']);
      } finally { await page.close(); }
    }
  });

  it('a horizontal division shortens only the rail and preserves the same scroll node and position', async () => {
    const page = await fixture({ side: 'right' });
    try {
      const before = await page.locator('.m-scroll').boundingBox();
      await page.evaluate(() => { const el = document.querySelector<HTMLElement>('.m-scroll')!; el.scrollTo({ top: 250, behavior: 'instant' }); (window as unknown as { savedScroll: Element }).savedScroll = el; });
      const value: ReservedRegion[] = [{ kind: 'division', x: 0, y: 400, width: 800, height: 0 }];
      await regions(page, value);
      expect(await overlaps(page, '.m-nav', value)).toBe(false);
      expect(await page.locator('.m-scroll').boundingBox()).toEqual(before);
      expect(await page.evaluate(() => (window as unknown as { savedScroll: Element }).savedScroll === document.querySelector('.m-scroll'))).toBe(true);
      expect(await page.locator('.m-scroll').evaluate((el) => el.scrollTop)).toBe(250);
    } finally { await page.close(); }
  });

  it('multiple regions move the same form panel, retaining draft, focus and its scroll DOM', async () => {
    const page = await fixture({ withForm: true });
    try {
      await page.locator('#draft').fill('unsaved draft');
      await page.evaluate(() => { (window as unknown as { savedDraft: Element }).savedDraft = document.querySelector('#draft')!; });
      const value: ReservedRegion[] = [
        { kind: 'division', x: 400, y: 0, width: 0, height: 800 },
        { kind: 'occlusion', x: 0, y: 400, width: 400, height: 400 },
      ];
      await regions(page, value);
      expect(await overlaps(page, '.m-form-panel', value)).toBe(false);
      expect(await page.locator('#draft').inputValue()).toBe('unsaved draft');
      expect(await page.evaluate(() => document.activeElement === (window as unknown as { savedDraft: Element }).savedDraft)).toBe(true);
      await observe(page, 'multi-regions-retain-draft-focus', ['.m-form-layer','.m-form-panel','#draft','.m-scroll']);
      await regions(page, []);
      expect(await page.locator('.m-form-panel').evaluate((el) => el.hasAttribute('data-ios-reserved-adjusted'))).toBe(false);
      expect(await page.locator('#draft').inputValue()).toBe('unsaved draft');
      expect(await page.evaluate(() => (window as unknown as { savedDraft: Element }).savedDraft === document.querySelector('#draft'))).toBe(true);
    } finally { await page.close(); }
  });

  it('an impossible region remains explicitly unresolved without hiding or replacing controls', async () => {
    const page = await fixture({ withForm: true });
    try {
      await regions(page, [{ kind: 'occlusion', x: 0, y: 0, width: 800, height: 800 }]);
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-unresolved')).toBe('true');
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-adjusted')).toBeNull();
      expect(await page.locator('.m-nav-item').count()).toBe(5);
      expect(await page.locator('.m-form-foot button').count()).toBe(2);
      expect(await page.locator('#draft').isVisible()).toBe(true);
      await regions(page, [{ kind: 'occlusion', x: 0, y: 0, width: 800, height: 760 }]);
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-unresolved')).toBe('true');
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-adjusted')).toBeNull();
      await observe(page, 'under-48px-unresolved', ['.m-form-panel','.m-nav','.m-pending-slot']);
    } finally { await page.close(); }
  });

  it('a native transition repairs the connect action without hijacking later user scrolling', async () => {
    const page = await fixture();
    try {
      const target = (await page.locator('[data-write-control="connect"]').boundingBox())!;
      const value: ReservedRegion[] = [{ kind: 'division', x: 0, y: target.y + target.height / 2, width: 800, height: 0 }];
      await regions(page, value);
      expect(await overlaps(page, '[data-write-control="connect"]', value)).toBe(false);
      await page.locator('.m-scroll').evaluate((el) => { el.scrollTo({ top: 420, behavior: 'instant' }); });
      await settle(page);
      expect(await page.locator('.m-scroll').evaluate((el) => el.scrollTop)).toBe(420);
      await observe(page, 'critical-repair-then-user-scroll', ['.m-scroll','[data-write-control="connect"]']);
      await page.locator('.m-scroll').evaluate((el) => el.scrollTo({ top: 0, behavior: 'instant' }));
      await settle(page);
      await regions(page, [{ kind: 'occlusion', x: 0, y: 0, width: 800, height: 800 }]);
      expect(await page.locator('[data-write-control="connect"]').getAttribute('data-ios-reserved-unresolved')).toBe('true');
      await regions(page, []);
      expect(await page.locator('[data-write-control="connect"]').getAttribute('data-ios-reserved-unresolved')).toBeNull();
    } finally { await page.close(); }
  });

  it.each(['ltr','rtl'])('a vertical division uses local action-row flow under %s instead of translating the button over siblings', async (dir) => {
    const page = await fixture({ dir });
    try {
      const target = (await page.locator('[data-write-control="connect"]').boundingBox())!;
      const value: ReservedRegion[] = [{ kind: 'division', x: target.x + target.width / 2, y: 0, width: 0, height: 800 }];
      await regions(page, value);
      await observe(page, `action-row-${dir}`, ['.h-header-actions', '[data-write-control="connect"]'], { regions: value });
      expect(await overlaps(page, '[data-write-control="connect"]', value)).toBe(false);
      expect(await page.locator('.h-header-actions').evaluate((el) => getComputedStyle(el).flexDirection)).toBe('column');
      expect(await page.locator('[data-write-control="connect"]').evaluate((el) => getComputedStyle(el).transform)).toBe('none');
    } finally { await page.close(); }
  });

  it('an RTL panel uses the physical left free region without changing text direction', async () => {
    const page = await fixture({ dir: 'rtl', withForm: true });
    try {
      const value: ReservedRegion[] = [
        { kind: 'division', x: 400, y: 0, width: 0, height: 800 },
        { kind: 'occlusion', x: 400, y: 400, width: 400, height: 400 },
      ];
      await regions(page, value);
      expect(await overlaps(page, '.m-form-panel', value)).toBe(false);
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-unresolved')).toBeNull();
      expect(await page.locator('.m-form-panel').evaluate((el) => getComputedStyle(el).direction)).toBe('rtl');
      const panel = (await page.locator('.m-form-panel').boundingBox())!;
      expect(panel.x + panel.width).toBeLessThanOrEqual(400);
      await observe(page, 'rtl-form-mixed-regions', ['.m-form-layer','.m-form-panel']);
    } finally { await page.close(); }
  });

  it('an RTL node sheet uses the actual scroll containing block and physical left free region', async () => {
    const page = await fixture({ dir: 'rtl' });
    try {
      await page.evaluate(() => document.querySelector('.m-scroll')!.insertAdjacentHTML('beforeend',
        '<div class="mn-sheet-layer"><div class="mn-sheet"><div class="mn-sheet-body" style="height:200px">Node actions</div><button class="mn-sheet-btn">Edit</button></div></div>'));
      await settle(page);
      const value: ReservedRegion[] = [
        { kind: 'division', x: 400, y: 0, width: 0, height: 800 },
        { kind: 'occlusion', x: 400, y: 400, width: 400, height: 400 },
      ];
      await regions(page, value);
      expect(await overlaps(page, '.mn-sheet', value)).toBe(false);
      expect(await page.locator('.mn-sheet').getAttribute('data-ios-reserved-unresolved')).toBeNull();
      const panel = (await page.locator('.mn-sheet').boundingBox())!;
      const owner = (await page.locator('.m-scroll').boundingBox())!;
      expect(panel.x + panel.width).toBeLessThanOrEqual(400);
      expect(panel.y + panel.height).toBeLessThanOrEqual(owner.y + owner.height);
      await observe(page, 'rtl-node-sheet-mixed-regions', ['.m-scroll','.mn-sheet-layer','.mn-sheet']);
    } finally { await page.close(); }
  });

  it('a fixed home sheet derives offsets from its actual CSS anchor and stays above flow navigation', async () => {
    const page = await fixture({ dir: 'rtl' });
    try {
      await page.evaluate(() => document.querySelector('.m-scroll')!.insertAdjacentHTML('beforeend',
        '<section class="h-sheet"><div style="height:200px">Node picker</div><button class="h-sheetclose">Close</button></section>'));
      await settle(page);
      const value: ReservedRegion[] = [
        { kind: 'division', x: 400, y: 0, width: 0, height: 800 },
        { kind: 'occlusion', x: 400, y: 400, width: 400, height: 400 },
      ];
      await regions(page, value);
      expect(await overlaps(page, '.h-sheet', value)).toBe(false);
      expect(await page.locator('.h-sheet').getAttribute('data-ios-reserved-unresolved')).toBeNull();
      const panel = (await page.locator('.h-sheet').boundingBox())!;
      const owner = (await page.locator('.m-scroll').boundingBox())!;
      expect(panel.x + panel.width).toBeLessThanOrEqual(400);
      expect(panel.y + panel.height).toBeLessThanOrEqual(owner.y + owner.height);
      await observe(page, 'fixed-home-sheet-actual-anchor', ['.m-scroll','.h-sheet']);
    } finally { await page.close(); }
  });

  it('Android ignores the same native-looking payload', async () => {
    const page = await fixture({ platform: 'android', withForm: true });
    try {
      const before = await page.locator('.m-form-panel').boundingBox();
      await regions(page, [{ kind: 'division', x: 400, y: 0, width: 0, height: 800 }]);
      expect(await page.locator('.m-form-panel').boundingBox()).toEqual(before);
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-adjusted')).toBeNull();
    } finally { await page.close(); }
  });

  it('the production keyboard initializer and region consumer share scaled coordinates and retain the focused draft', async () => {
    const page = await fixture({ withForm: true });
    try {
      await page.locator('#draft').fill('keyboard draft');
      await page.evaluate(() => {
        for (const [key, value] of Object.entries({ offsetTop: 20, offsetLeft: 0, width: 400, height: 350, scale: 2 }))
          Object.defineProperty(window.visualViewport!, key, { configurable: true, value });
        const style = document.documentElement.style;
        style.setProperty('--ios-keyboard-visible-bottom', '600px');
        style.setProperty('--safe-t', '32px');
        style.setProperty('--safe-b', '40px');
      });
      // Native points => half-size CSS coordinates at this visual scale.
      const native: ReservedRegion[] = [{ kind: 'division', x: 400, y: 0, width: 0, height: 800 }];
      const cssRegions: ReservedRegion[] = [{ kind: 'division', x: 200, y: 20, width: 0, height: 400 }];
      await regions(page, native);
      expect(await overlaps(page, '.m-form-panel', cssRegions)).toBe(false);
      const panel = (await page.locator('.m-form-panel').boundingBox())!;
      expect(panel.y).toBeGreaterThanOrEqual(36); // offset + safe inset, each converted once
      expect(panel.y + panel.height).toBeLessThanOrEqual(320); // offset + keyboard-visible height
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-unresolved')).toBeNull();
      expect(await page.locator('#draft').inputValue()).toBe('keyboard draft');
      expect(await page.locator('#draft').evaluate((el) => document.activeElement === el)).toBe(true);
      const draft = (await page.locator('#draft').boundingBox())!;
      expect(draft.y).toBeGreaterThanOrEqual(panel.y);
      expect(draft.y + draft.height).toBeLessThanOrEqual(panel.y + panel.height);
      await observe(page, 'keyboard-scale-offset-draft-focus', ['.m-form-layer','.m-form-panel','#draft']);
    } finally { await page.close(); }
  });

  it('resamples the same native snapshot after rotation with stale innerWidth and keeps the 110px footer and focus outline visible', async () => {
    const page = await fixture({ withForm: true });
    try {
      await page.locator('#draft').fill('rotation draft');
      await page.setViewportSize({ width: 874, height: 402 });
      await page.evaluate(() => {
        window.__polarisIosViewport = { sequence: 5, value: { width: 874, height: 402,
          verticalBarSide: 'unspecified', reservedRegions: [],
          top: 0, right: 62, bottom: 20, left: 62, keyboardVisibleBottom: 110 } };
        Object.defineProperty(window, 'innerWidth', { configurable: true, value: 402 });
        Object.defineProperty(window, 'innerHeight', { configurable: true, value: 377 });
        for (const [key, value] of Object.entries({ offsetTop: 245.8125, width: 402, height: 31.453125, scale: 1 }))
          Object.defineProperty(window.visualViewport!, key, { configurable: true, value });
        const style = document.documentElement.style;
        style.setProperty('--ios-keyboard-visible-bottom', `${110 * 402 / 874}px`);
        style.setProperty('--safe-l', `${62 * 402 / 874}px`);
        window.dispatchEvent(new Event('polaris-ios-viewport-change'));
      });
      await settle(page);
      await page.evaluate(() => {
        for (const [key, value] of Object.entries({ offsetTop: 53.65625, width: 874, height: 110 }))
          Object.defineProperty(window.visualViewport!, key, { configurable: true, value });
        Object.defineProperty(window, 'innerHeight', { configurable: true, value: 348 });
        // No new native sequence or native event; a layout resize must resample.
        window.dispatchEvent(new Event('resize'));
      });
      await settle(page);
      const result = await page.evaluate(() => {
        const root = document.documentElement, panel = document.querySelector('.m-form-panel')!;
        const field = document.querySelector('#draft')!, footer = document.querySelector('.m-form-foot')!;
        const css = getComputedStyle(root), fieldCss = getComputedStyle(field);
        const outline = Number.parseFloat(fieldCss.outlineWidth) + Number.parseFloat(fieldCss.outlineOffset);
        return { sequence: window.__polarisIosViewport!.sequence, clientWidth: root.clientWidth, innerWidth,
          keyboard: css.getPropertyValue('--ios-keyboard-visible-bottom'), left: css.getPropertyValue('--safe-l'),
          gap: css.getPropertyValue('--ios-form-viewport-bottom-gap'), height: css.getPropertyValue('--ios-form-viewport-height'),
          panel: panel.getBoundingClientRect().toJSON(), field: field.getBoundingClientRect().toJSON(), outline,
          footer: footer.getBoundingClientRect().toJSON(), position: getComputedStyle(footer).position,
          header: panel.querySelector('.m-form-head')!.getBoundingClientRect().toJSON(),
          headerPosition: getComputedStyle(panel.querySelector('.m-form-head')!).position,
          footerButtons: [...footer.querySelectorAll('button')].map(button => button.getBoundingClientRect().height),
          hitChecks: (() => {
            const r = field.getBoundingClientRect();
            return [[r.x + r.width / 2, r.y + r.height / 2], [r.x + 1, r.y + r.height / 2],
              [r.right - 1, r.y + r.height / 2], [r.x + r.width / 2, r.y + 1], [r.x + r.width / 2, r.bottom - 1]]
              .map(([x, y]) => document.elementFromPoint(x, y) === field);
          })() };
      });
      expect(result).toMatchObject({ sequence: 5, clientWidth: 874, innerWidth: 402,
        keyboard: '110px', left: '62px', gap: '292px', height: '110px', position: 'sticky' });
      measurements.push({ name: '110px-focused-field-paint-occlusion', result });
      expect(result.hitChecks).toEqual([true, true, true, true, true]);
      expect(result.headerPosition).toBe('static');
      expect(result.panel.height).toBe(110);
      expect(result.footerButtons).toEqual([48, 48]);
      expect(result.field.y - result.outline).toBeGreaterThanOrEqual(result.panel.y - 0.1);
      expect(result.field.bottom + result.outline).toBeLessThanOrEqual(result.footer.y + 0.1);
      expect(result.footer.bottom).toBeLessThanOrEqual(53.65625 + 110 + 0.1);
      expect(await page.locator('#draft').inputValue()).toBe('rotation draft');
      expect(await page.locator('#draft').evaluate((el) => document.activeElement === el)).toBe(true);
      await observe(page, 'same-sequence-stale-innerwidth-110px', ['.m-form-layer','.m-form-panel','#draft','.m-form-foot'], result);
    } finally { await page.close(); }
  });

  it('uses layout width for raw insets and half-region mapping during automatic focus zoom', async () => {
    const page = await fixture({ withForm: true });
    try {
      await page.setViewportSize({ width: 402, height: 874 });
      await page.locator('#draft').fill('zoom draft');
      const scale = 1.0671641826629639;
      await page.evaluate((scale) => {
        window.__polarisIosViewport = { sequence: 3, value: { width: 402, height: 874,
          verticalBarSide: 'unspecified', reservedRegions: [{ kind: 'division', x: 201, y: 0, width: 0, height: 874 }],
          top: 62, right: 0, bottom: 34, left: 0, keyboardVisibleBottom: 477 } };
        Object.defineProperty(window, 'innerWidth', { configurable: true, value: 377 });
        Object.defineProperty(window, 'innerHeight', { configurable: true, value: 628 });
        for (const [key, value] of Object.entries({ offsetTop: 245.5, offsetLeft: 12.796875,
          width: 376.6875, height: 446.96875, scale }))
          Object.defineProperty(window.visualViewport!, key, { configurable: true, value });
        window.dispatchEvent(new Event('polaris-ios-viewport-change'));
      }, scale);
      await settle(page);
      const mapped: ReservedRegion[] = [{ kind: 'division', x: 12.796875 + 201 / scale,
        y: 245.5, width: 0, height: 874 / scale }];
      expect(await overlaps(page, '.m-form-panel', mapped)).toBe(false);
      const result = await page.evaluate(() => {
        const css = getComputedStyle(document.documentElement);
        return { top: css.getPropertyValue('--safe-t'), keyboard: css.getPropertyValue('--ios-keyboard-visible-bottom'),
          height: Number.parseFloat(css.getPropertyValue('--ios-form-viewport-height')),
          gap: Number.parseFloat(css.getPropertyValue('--ios-form-viewport-bottom-gap')) };
      });
      expect(result.top).toBe('62px');
      expect(result.keyboard).toBe('477px');
      expect(result.height).toBeCloseTo(Math.min(446.96875, 477 / scale), 5);
      expect(result.gap).toBeCloseTo(874 / scale - result.height, 5);
      const panel = (await page.locator('.m-form-panel').boundingBox())!;
      expect(panel.width).toBeLessThanOrEqual(201 / scale + 0.1);
      expect(panel.y).toBeGreaterThanOrEqual(245.5 + 62 / scale - 0.1);
      expect(await page.locator('#draft').inputValue()).toBe('zoom draft');
      expect(await page.locator('#draft').evaluate((el) => document.activeElement === el)).toBe(true);
      await observe(page, 'focus-zoom-layout-width-half-region', ['.m-form-layer','.m-form-panel','#draft'], result);
    } finally { await page.close(); }
  });

  it('a focus-only division collision includes the connect outline outside the button border', async () => {
    const page = await fixture();
    try {
      await page.locator('[data-write-control="connect"]').focus();
      const border = (await page.locator('[data-write-control="connect"]').boundingBox())!;
      const value: ReservedRegion[] = [{ kind: 'division', x: 0, y: border.y + border.height + 1, width: 800, height: 0 }];
      expect(await overlaps(page, '[data-write-control="connect"]', value)).toBe(false);
      await regions(page, value);
      const paintBottom = await page.locator('[data-write-control="connect"]').evaluate((el) => {
        const style = getComputedStyle(el);
        return el.getBoundingClientRect().bottom + Number.parseFloat(style.outlineWidth) + Number.parseFloat(style.outlineOffset);
      });
      expect(paintBottom).toBeLessThanOrEqual(value[0].y);
      expect(await page.locator('[data-write-control="connect"]').getAttribute('data-ios-reserved-unresolved')).toBeNull();
    } finally { await page.close(); }
  });

  it('a panel inserted after the native event is observed, and older sequences cannot replace its geometry', async () => {
    const page = await fixture();
    try {
      const value: ReservedRegion[] = [{ kind: 'division', x: 400, y: 0, width: 0, height: 800 }];
      await regions(page, value);
      await page.evaluate((markup) => document.body.insertAdjacentHTML('beforeend', markup), form);
      await settle(page);
      expect(await overlaps(page, '.m-form-panel', value)).toBe(false);
      const before = await page.locator('.m-form-panel').boundingBox();
      await page.evaluate(() => {
        const snapshot = (window as unknown as { __polarisIosViewport: { sequence: number; value: { reservedRegions: unknown[] } } }).__polarisIosViewport;
        snapshot.sequence = 1;
        snapshot.value.reservedRegions = [];
        window.dispatchEvent(new Event('polaris-ios-viewport-change'));
      });
      await settle(page);
      expect(await page.locator('.m-form-panel').boundingBox()).toEqual(before);
    } finally { await page.close(); }
  });

  it('old iOS without the native region source preserves its panel geometry', async () => {
    const page = await fixture({ withForm: true });
    try {
      await page.evaluate(() => { document.documentElement.dataset.iosLayoutSource = 'legacy'; });
      const before = await page.locator('.m-form-panel').boundingBox();
      await regions(page, [{ kind: 'division', x: 400, y: 0, width: 0, height: 800 }]);
      expect(await page.locator('.m-form-panel').boundingBox()).toEqual(before);
      expect(await page.locator('.m-form-panel').getAttribute('data-ios-reserved-adjusted')).toBeNull();
    } finally { await page.close(); }
  });

  it('flow chrome reserves space before a toast measures its content-row containing block', async () => {
    const page = await fixture();
    try {
      const value: ReservedRegion[] = [
        { kind: 'division', x: 0, y: 740, width: 800, height: 0 },
        { kind: 'division', x: 0, y: 650, width: 800, height: 0 },
        { kind: 'occlusion', x: 0, y: 500, width: 800, height: 100 },
      ];
      await regions(page, value);
      for (const selector of ['.m-nav','.m-pending-slot','.m-toast-host']) {
        expect(await overlaps(page, selector, value), selector).toBe(false);
        expect(await page.locator(selector).getAttribute('data-ios-reserved-unresolved'), selector).toBeNull();
      }
      const toast = (await page.locator('.m-toast-host').boundingBox())!;
      const content = (await page.locator('.m-scroll').boundingBox())!;
      expect(toast.x).toBeGreaterThanOrEqual(content.x);
      expect(toast.y + toast.height).toBeLessThanOrEqual(content.y + content.height);
    } finally { await page.close(); }
  });

  it('removing a tracked panel clears its constraints; disposal cancels pending work and prevents later writes', async () => {
    const page = await fixture({ withForm: true });
    try {
      await regions(page, [{ kind: 'division', x: 400, y: 0, width: 0, height: 800 }]);
      await page.evaluate(() => {
        const el = document.querySelector('.m-form-panel')!;
        (window as unknown as { removedPanel: Element }).removedPanel = el;
        el.remove();
      });
      await settle(page);
      expect(await page.evaluate(() => (window as unknown as { removedPanel: Element }).removedPanel.getAttribute('data-ios-reserved-adjusted'))).toBeNull();
      await page.evaluate((markup) => {
        document.body.insertAdjacentHTML('beforeend', markup);
        window.dispatchEvent(new Event('polaris-ios-viewport-change'));
        (window as unknown as { __disposeLayout: () => void }).__disposeLayout();
        window.dispatchEvent(new Event('polaris-ios-viewport-change'));
      }, form);
      await settle(page);
      expect(await page.locator('[data-ios-reserved-adjusted]').count()).toBe(0);
      expect(await page.locator('.m-form-panel').getAttribute('style')).toBeNull();
      await regions(page, [{ kind: 'division', x: 200, y: 0, width: 0, height: 800 }]);
      expect(await page.locator('[data-ios-reserved-adjusted]').count()).toBe(0);
    } finally { await page.close(); }
  });

  it.each([300,200])('same geometry preserves blurred form/body and list bottom positions with %ipx local panels', async (height) => {
    const page = await fixture({ withForm: true });
    try {
      await page.locator('.m-form-body').evaluate((el) => el.insertAdjacentHTML('beforeend', '<div style="height:1400px;flex:none">Long fields</div>'));
      await regions(page, [...(height === 300 ? [300,600,740] : [200,400,600,740])].map((y) => ({ kind: 'division', x: 0, y, width: 800, height: 0 })));
      const before = await page.evaluate(() => {
        (document.activeElement as HTMLElement)?.blur();
        return ['.m-form-body','.m-form-panel','.m-scroll'].map((selector) => {
          const el = document.querySelector<HTMLElement>(selector)!;
          el.scrollTo({ top: el.scrollHeight, behavior: 'instant' });
          return el.scrollTop;
        });
      });
      expect(before[height === 300 ? 0 : 1]).toBeGreaterThan(1000);
      expect(before[2]).toBeGreaterThan(1000);
      await page.evaluate(() => {
        window.dispatchEvent(new Event('polaris-ios-viewport-change'));
        document.querySelector('.m-page')!.append(document.createElement('span'));
      });
      await settle(page);
      const after = await page.evaluate(() => ['.m-form-body','.m-form-panel','.m-scroll'].map((selector) => document.querySelector(selector)!.scrollTop));
      expect(after).toEqual(before);
      await observe(page, `bottom-scroll-continuity-${height}`, ['.m-form-panel','.m-form-body','.m-scroll'], { before, after });
    } finally { await page.close(); }
  });
});
