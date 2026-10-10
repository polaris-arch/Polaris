/** Actual mounted App/SettingsTun -> real wrapper/listenReady -> mocked transport.
 * Rust-generated states exercise source wiring; no native IPC, core or device proof. */
import { expect, test, type Page } from '@playwright/test';
import { LANGUAGE_STORAGE_KEY } from '../src/domain/language';
import en from '../src/i18n/locales/en-US.json' with { type: 'json' };
import zhCN from '../src/i18n/locales/zh-CN.json' with { type: 'json' };
import zhTW from '../src/i18n/locales/zh-TW.json' with { type: 'json' };
import ru from '../src/i18n/locales/ru.json' with { type: 'json' };
import fa from '../src/i18n/locales/fa.json' with { type: 'json' };
const card = (page: Page) => page.locator('[data-coex-snapshot]');
const commands = (page: Page) => page.evaluate(() => window.__coexHarness!.calls.filter(c => c.command.startsWith('coex_')));
async function reply(page: Page, name: 'latest' | 'staleBusy' | 'crashed' | 'windowsPartial' | 'windowsUnavailable') { await page.evaluate(name => window.__coexHarness!.reply({ success: true, data: window.__coexHarness!.runtimeState(name) }), name); }
async function emit(page: Page, name: 'latest' | 'staleBusy' | 'crashed' | 'windowsPartial' | 'windowsUnavailable') { await page.evaluate(name => window.__coexHarness!.emitRuntimeState(window.__coexHarness!.runtimeState(name)), name); }
test.beforeEach(async ({ page }) => {
  await page.addInitScript(({ key }) => localStorage.setItem(key, 'en-US'), { key: LANGUAGE_STORAGE_KEY });
  await page.goto('/harness.html?coex-snapshot-fixture=1');
  await page.getByRole('button', { name: en.sidebar.settings, exact: true }).click();
  await page.getByRole('button', { name: 'TUN', exact: true }).click();
  await expect(card(page)).toBeVisible(); await expect.poll(async () => (await commands(page)).filter(c => c.command === 'coex_runtime_get_state').length).toBe(1);
});
test('mount only reads state; it never starts native or legacy collection', async ({ page }) => {
  expect((await commands(page)).map(c => c.command)).toEqual(['coex_runtime_get_state']);
  await reply(page, 'latest'); await expect(card(page)).toContainText(en.settings.coex.latest);
  expect((await commands(page)).map(c => c.command)).toEqual(['coex_runtime_get_state']);
  await expect(card(page)).toContainText(en.settings.coex.classification);
});
test('original-hook stale/busy event and crash clear the current report', async ({ page }) => {
  await reply(page, 'latest'); await emit(page, 'staleBusy'); await expect(card(page)).toContainText(en.settings.coex.stale); await expect(card(page)).toContainText(en.settings.coex.busy); await expect(card(page)).toContainText(en.settings.coex.queued);
  await emit(page, 'crashed'); await expect(card(page)).toContainText(en.settings.coex.unavailable); await expect(card(page)).not.toContainText(en.settings.coex.observedEmpty);
  expect((await commands(page)).filter(c => c.command !== 'coex_runtime_get_state')).toHaveLength(0);
});
test('event during initial get-state wins over its late older response', async ({ page }) => {
  await emit(page, 'staleBusy'); await expect(card(page)).toContainText(en.settings.coex.stale);
  await reply(page, 'latest'); await expect(card(page)).toContainText(en.settings.coex.stale); await expect(card(page)).not.toContainText(en.settings.coex.latest);
});
test('old session event cannot overwrite a newer terminal state', async ({ page }) => {
  await reply(page, 'latest'); await emit(page, 'crashed'); await emit(page, 'latest'); await expect(card(page)).toContainText(en.settings.coex.unavailable); await expect(card(page)).not.toContainText(en.settings.coex.latest);
});
test('manual schedules with only the displayed revision; new event beats old reply', async ({ page }) => {
  await reply(page, 'latest'); await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await expect.poll(async () => (await commands(page)).filter(c => c.command === 'coex_runtime_refresh').length).toBe(1);
  const payload = (await commands(page)).find(c => c.command === 'coex_runtime_refresh')!.payload as Record<string, unknown>; expect(Object.keys(payload)).toEqual(['expectedReportRevision']); expect(typeof payload.expectedReportRevision).toBe('string');
  await emit(page, 'crashed'); await reply(page, 'latest'); await expect(card(page)).toContainText(en.settings.coex.unavailable);
});
test('late failed manual request does not cover a newer accepted event', async ({ page }) => {
  await reply(page, 'latest'); await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await expect.poll(async () => (await commands(page)).filter(c => c.command === 'coex_runtime_refresh').length).toBe(1);
  await emit(page, 'crashed'); await page.evaluate(() => window.__coexHarness!.reject('older fixture failure')); await expect(card(page)).not.toContainText('older fixture failure');
});
test('malformed state fails closed and manual retry rereads state', async ({ page }) => {
  await page.evaluate(() => window.__coexHarness!.reply({ success: true, data: { schemaVersion: 1, reportRevision: 1 } }));
  await expect(card(page)).toContainText(en.settings.coex.unavailable); await expect(card(page)).toContainText(en.settings.coex.malformed);
  await card(page).getByRole('button', { name: en.settings.coex.retry, exact: true }).click(); await expect.poll(async () => (await commands(page)).filter(c => c.command === 'coex_runtime_get_state').length).toBe(2);
  await reply(page, 'latest'); await expect.poll(async () => (await commands(page)).filter(c => c.command === 'coex_runtime_refresh').length).toBe(1); await reply(page, 'staleBusy'); await expect(card(page)).toContainText(en.settings.coex.stale);
});
test('five Unknown Windows sources remain unavailable with exact source errors', async ({ page }) => {
  await reply(page, 'windowsUnavailable'); await expect(card(page)).toContainText(en.settings.coex.unavailable); await expect(card(page).locator('[data-coex-source]')).toHaveCount(5);
  await card(page).locator('[data-coex-source="addresses"] > summary').click(); await expect(card(page)).toContainText('fixture permission unavailable'); await expect(card(page)).not.toContainText(en.settings.coex.observedEmpty);
});
test('Windows partial rows can be latest observed facts with scope/completeness Unknown', async ({ page }) => {
  await reply(page, 'windowsPartial'); await expect(card(page)).toContainText(en.settings.coex.latest); await expect(card(page).locator('[data-coex-source]')).toHaveCount(5);
  await card(page).locator('[data-coex-source="adapters"] > summary').click(); await expect(card(page)).toContainText('fixture compartment unavailable'); await expect(card(page)).toContainText('fixture completeness unavailable');
});
for (const [language, locale] of [['en-US', en], ['zh-CN', zhCN], ['zh-TW', zhTW], ['ru', ru], ['fa', fa]] as const) {
  test(`actual mounted state and refresh text in ${language}`, async ({ page }) => {
    await reply(page, 'latest'); await page.evaluate(language => window.__coexHarness!.setLanguage(language), language); await expect(card(page)).toContainText(locale.settings.coex.latest);
    await emit(page, 'staleBusy'); await expect(card(page)).toContainText(locale.settings.coex.stale); await expect(card(page)).toContainText(locale.settings.coex.busy); await emit(page, 'crashed'); await expect(card(page)).toContainText(locale.settings.coex.unavailable);
  });
}
test('narrow RTL state fits and preserves Unknown fact text', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 900 }); await reply(page, 'windowsUnavailable'); await page.evaluate(() => window.__coexHarness!.setLanguage('fa')); await expect(card(page)).toContainText(fa.settings.coex.unavailable);
  const sizes = await card(page).evaluate(el => ({ width: el.clientWidth, scroll: el.scrollWidth })); expect(sizes.scroll).toBeLessThanOrEqual(sizes.width + 1);
});
test('unmount before get-state reply removes listener without starting replacement', async ({ page }) => {
  await page.getByRole('button', { name: en.settings.nav.back, exact: true }).click(); await expect(card(page)).toHaveCount(0); await reply(page, 'latest'); await emit(page, 'staleBusy'); expect((await commands(page)).filter(c => c.command === 'coex_runtime_refresh')).toHaveLength(0);
});

test('listenReady late acknowledgement after unmount is actually unsubscribed', async ({ page }) => {
  await reply(page, 'latest'); await page.getByRole('button', { name: en.settings.nav.back, exact: true }).click();
  await page.evaluate(() => { window.__coexHarness!.holdRegistration = true; });
  await page.getByRole('button', { name: en.sidebar.settings, exact: true }).click(); await page.getByRole('button', { name: 'TUN', exact: true }).click(); await expect(card(page)).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.__coexHarness!.registrationReleases.length)).toBeGreaterThan(0);
  expect((await commands(page)).filter(c => c.command === 'coex_runtime_get_state')).toHaveLength(1);
  const before = await page.evaluate(() => window.__coexHarness!.unregisterCount);
  await page.getByRole('button', { name: en.settings.nav.back, exact: true }).click(); await expect(card(page)).toHaveCount(0); await page.evaluate(() => window.__coexHarness!.releaseRegistration());
  await expect.poll(() => page.evaluate(() => window.__coexHarness!.unregisterCount)).toBeGreaterThan(before); expect((await commands(page)).filter(c => c.command === 'coex_runtime_get_state')).toHaveLength(1);
});
test('newer event survives late initial get-state transport failure', async ({ page }) => {
  await emit(page, 'latest'); await page.evaluate(() => window.__coexHarness!.reject('older initial failure')); await expect(card(page)).toContainText(en.settings.coex.latest); await expect(card(page)).not.toContainText('older initial failure');
});
