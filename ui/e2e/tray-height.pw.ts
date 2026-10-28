/** Mounted real tray/CSS + mocked transport. Browser layout proof, not native/macOS acceptance. */
import { expect, test, type Page } from '@playwright/test';
import { LANGUAGE_STORAGE_KEY } from '../src/domain/language';

const reported = (page: Page) => page.evaluate(() => window.__trayReportedHeight);
async function natural(page: Page) {
  return page.locator('.tray-menu').evaluate(menu => {
    const content = menu.querySelector('.tray-menu-content')!;
    const style = getComputedStyle(menu);
    return Math.ceil(content.getBoundingClientRect().height + menu.getBoundingClientRect().top
      + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
      + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth) + parseFloat(style.marginBottom));
  });
}
async function fitHost(page: Page, maximum = 720, width = 268) {
  const height = await reported(page);
  await page.setViewportSize({ width, height: Math.min(maximum, Math.max(80, height)) });
  await expect.poll(() => reported(page)).toBe(height);
}
async function expectNoTail(page: Page) {
  await page.locator('.tray-menu').evaluate(menu => { menu.scrollTop = menu.scrollHeight; });
  const gap = await page.locator('.tray-menu').evaluate(menu => {
    const quit = menu.querySelector('.tray-view > .tray-i.danger')!;
    const style = getComputedStyle(menu);
    return menu.getBoundingClientRect().bottom - quit.getBoundingClientRect().bottom
      - parseFloat(style.paddingBottom) - parseFloat(style.borderBottomWidth);
  });
  expect(Math.abs(gap)).toBeLessThanOrEqual(1);
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(key => localStorage.setItem(key, 'zh-CN'), LANGUAGE_STORAGE_KEY);
  await page.setViewportSize({ width: 268, height: 420 });
});

for (const os of ['mac', 'win', 'lin']) {
  test(`${os}: initial 420 grows, then long→short removes the tail without clipping Quit`, async ({ page }) => {
    await page.goto(`/tray-harness.html?os=${os}&nodes=3&recent=3`);
    await expect(page.locator('.tray-node-name')).toHaveCount(3);
    await expect.poll(() => reported(page)).toBeGreaterThan(420);
    await expect.poll(async () => (await reported(page)) - (await natural(page))).toBe(0);
    const long = await reported(page);
    await fitHost(page, os === 'win' ? 700 : 720);
    await page.evaluate(() => window.__trayHeightHarness.setRecent(0));
    await expect(page.locator('.tray-node-name')).toHaveCount(1);
    await expect.poll(() => reported(page)).toBeLessThan(long);
    await expect.poll(async () => (await reported(page)) - (await natural(page))).toBe(0);
    await fitHost(page, os === 'win' ? 700 : 720);
    await expectNoTail(page);
    // Repeat expansion after shrink; a one-time/monotonic lock would fail one direction.
    await page.evaluate(() => window.__trayHeightHarness.setRecent(3));
    await expect(page.locator('.tray-node-name')).toHaveCount(3);
    await expect.poll(() => reported(page)).toBe(long);
  });
}

test('node expand/collapse holds the card; returning to a shortened main refreshes it', async ({ page }) => {
  await page.goto('/tray-harness.html?os=mac&nodes=30&recent=3');
  await expect(page.locator('.tray-node-name')).toHaveCount(3);
  await expect.poll(async () => (await reported(page)) - (await natural(page))).toBe(0);
  const main = await reported(page);
  await fitHost(page);
  await page.getByRole('button', { name: '全部节点', exact: true }).click();
  await expect(page.locator('.tray-node-name')).toHaveCount(30);
  expect(await natural(page)).toBeGreaterThan(main);
  await expect.poll(() => reported(page)).toBe(main);
  await page.locator('.tray-group-h[aria-expanded="true"]').click();
  await expect(page.locator('.tray-node-name')).toHaveCount(0);
  expect(await natural(page)).toBeLessThan(main);
  await expect.poll(() => reported(page)).toBe(main);
  await page.evaluate(() => window.__trayHeightHarness.setRecent(0));
  await page.locator('.tray-back').click();
  await expect(page.locator('.tray-node-name')).toHaveCount(1);
  await expect.poll(() => reported(page)).toBeLessThan(main);
  await expect.poll(async () => (await reported(page)) - (await natural(page))).toBe(0);
  await fitHost(page);
  await expectNoTail(page);
});

test('real font metrics and available width reflow both directions without an outer-height feedback loop', async ({ page }) => {
  await page.goto('/tray-harness.html?os=mac&nodes=3&recent=0');
  await expect(page.locator('.tray-node-name')).toHaveCount(1);
  await expect.poll(async () => (await reported(page)) - (await natural(page))).toBe(0);
  const before = await reported(page);
  await fitHost(page);
  const font = await page.addStyleTag({ content: '.tray-menu .tray-i { font-size: 18px; }' });
  await expect.poll(() => reported(page)).toBeGreaterThan(before);
  await expect.poll(async () => (await reported(page)) - (await natural(page))).toBe(0);
  await fitHost(page);
  await font.evaluate(el => el.remove());
  await expect.poll(() => reported(page)).toBe(before);
  for (const width of [300, 240, 268]) {
    await page.setViewportSize({ width, height: 420 });
    await expect.poll(async () => (await reported(page)) - (await natural(page))).toBe(0);
  }
  await fitHost(page);
  await expectNoTail(page);
});
