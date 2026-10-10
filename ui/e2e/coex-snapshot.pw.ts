/** Mounted App -> SettingsTun -> system wrapper -> real ipc-client -> mockIPC.
 * Browser receipts are not native IPC, platform collection or device acceptance. */
import { expect, test, type Page } from '@playwright/test';
import { LANGUAGE_STORAGE_KEY } from '../src/domain/language';
import en from '../src/i18n/locales/en-US.json' with { type: 'json' };
import zhCN from '../src/i18n/locales/zh-CN.json' with { type: 'json' };
import zhTW from '../src/i18n/locales/zh-TW.json' with { type: 'json' };
import ru from '../src/i18n/locales/ru.json' with { type: 'json' };
import fa from '../src/i18n/locales/fa.json' with { type: 'json' };

const card = (page: Page) => page.locator('[data-coex-snapshot]');
const calls = (page: Page) => page.evaluate(() => window.__coexHarness!.calls.filter((call) => call.command === 'coex_readonly_snapshot'));
const openTun = async (page: Page) => {
  await page.getByRole('button', { name: 'TUN', exact: true }).click();
  await expect(card(page)).toBeVisible();
};
const replyFixture = (page: Page) => page.evaluate(() => {
  const control = window.__coexHarness!;
  control.reply({ success: true, data: control.fixture() });
});
test.beforeEach(async ({ page }) => {
  await page.addInitScript(({ key }) => localStorage.setItem(key, 'en-US'), { key: LANGUAGE_STORAGE_KEY });
  await page.goto('/harness.html?coex-snapshot-fixture=1');
  await page.getByRole('button', { name: en.sidebar.settings, exact: true }).click();
  await openTun(page);
  await expect(card(page)).toContainText(en.settings.coex.notCollected);
  expect(await calls(page)).toHaveLength(0);
  await page.evaluate(() => { window.__coexHarness!.calls.length = 0; });
});
test.afterEach(async ({ page }) => {
  const commands = await page.evaluate(() => window.__coexHarness!.calls.map((call) => call.command));
  expect(commands.filter((cmd) => /config_(save|set_value)|proxy_(start|stop|restart|clear)|helper_(install|uninstall)|tun_.*(write|set)/.test(cmd))).toEqual([]);
});

test('manual single flight and actual mounted facts, then explicit refresh clears the prior report', async ({ page }, testInfo) => {
  const button = card(page).getByRole('button', { name: en.settings.coex.collect, exact: true });
  await button.evaluate((element: HTMLButtonElement) => { element.click(); element.click(); element.click(); });
  await expect(card(page).getByRole('button', { name: en.settings.coex.loading })).toBeDisabled();
  await expect.poll(() => calls(page)).toHaveLength(1);
  expect((await calls(page))[0].payload).toEqual({});
  await replyFixture(page);
  await expect(card(page)).toContainText(en.settings.coex.classification);
  await expect(card(page)).toContainText(en.settings.coex.unknown);
  await card(page).locator('summary').filter({ hasText: 'fixture-tun0' }).click();
  await expect(card(page)).toContainText('10.77.2.9/24');
  await expect(card(page)).toContainText('0.0.0.0/1');
  await expect(card(page)).toContainText(en.settings.coex.noIdentity);
  await expect(card(page).locator('.good, .success, .safe')).toHaveCount(0);
  await button.click();
  await expect(card(page).locator('[data-coex-facts]')).toHaveCount(0);
  await expect.poll(() => calls(page)).toHaveLength(2);
  await replyFixture(page);
  await expect(button).toBeEnabled();
  await card(page).screenshot({ path: testInfo.outputPath('coex-normal-en.png') });
});

test('envelope permission failure, transport failure, malformed format and explicit retry', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await page.evaluate(() => window.__coexHarness!.reply({ success: false, code: 'coex_window_denied', error: '<permission denied>' }));
  await expect(card(page).getByRole('alert')).toContainText(en.settings.coex.failed);
  await expect(card(page).locator('[data-coex-facts]')).toHaveCount(0);
  const retry = card(page).getByRole('button', { name: en.settings.coex.retry, exact: true });
  await retry.click();
  await page.evaluate(() => window.__coexHarness!.reject('fixture transport failed'));
  await expect(card(page).getByRole('alert')).toContainText(en.settings.coex.failed);
  await retry.click();
  await page.evaluate(() => window.__coexHarness!.reply({ success: true, data: { schemaVersion: 2, objects: [] } }));
  await expect(card(page).getByRole('alert')).toContainText(en.settings.coex.malformed);
  await retry.click(); await replyFixture(page);
  await expect(card(page).getByRole('alert')).toHaveCount(0);
  expect(await calls(page)).toHaveLength(4);
});

test('leaving and returning isolates a delayed old reply, with no automatic recollection', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await expect.poll(() => calls(page)).toHaveLength(1);
  await page.getByRole('button', { name: en.settings.nav.general, exact: true }).click();
  await expect(card(page)).toHaveCount(0);
  await openTun(page);
  await expect(card(page)).toContainText(en.settings.coex.notCollected);
  await replyFixture(page);
  await expect(card(page).locator('[data-coex-facts]')).toHaveCount(0);
  expect(await calls(page)).toHaveLength(1);
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await replyFixture(page);
  await expect(card(page).locator('[data-coex-facts]')).toBeVisible();
  expect(await calls(page)).toHaveLength(2);
});

for (const platform of ['darwin', 'win32'] as const) {
  test(`${platform} unavailable remains Unknown rather than an empty or safe result`, async ({ page }) => {
    await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
    await page.evaluate((platform) => {
      const c = window.__coexHarness!; const data = c.fixture();
      const unknown = { status: 'unknown' as const, reason: 'production source unavailable' };
      data.platform = platform; data.objects = unknown; data.observation = unknown; data.commandCleanup = unknown;
      c.reply({ success: true, data });
    }, platform);
    await expect(card(page)).toContainText(en.settings.coex.sourceUnavailable);
    await expect(card(page)).toContainText(en.settings.coex.unknown);
    await expect(card(page)).not.toContainText(en.settings.coex.observedEmpty);
    await expect(card(page).locator('[data-coex-interface]')).toHaveCount(0);
  });
}

test('busy/quarantine unknown and partial sources preserve independent evidence', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await page.evaluate(() => {
    const c = window.__coexHarness!; const data = c.fixture();
    data.objects = { status: 'unknown', reason: 'snapshot busy; retained cleanup unknown' };
    data.observation = { status: 'unknown', reason: 'no new collection' };
    data.commandCleanup = { status: 'unknown', reason: 'retained original command' };
    c.reply({ success: true, data });
  });
  await expect(card(page)).toContainText(en.settings.coex.unknown);
  await expect(card(page)).not.toContainText(en.settings.coex.observedEmpty);
  expect(await calls(page)).toHaveLength(1);
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await page.evaluate(() => {
    const c = window.__coexHarness!; const data = c.fixture();
    if (data.objects.status !== 'known') throw Error('fixture');
    data.objects.value[0].routes = { status: 'unknown', reason: 'route read denied' };
    c.reply({ success: true, data });
  });
  await card(page).locator('summary').filter({ hasText: 'fixture-tun0' }).click();
  await expect(card(page)).toContainText('10.77.2.9/24');
  await expect(card(page)).not.toContainText('0.0.0.0/1');
});

test('large raw lists paginate honestly and one expanded interface wraps long diagnostics in narrow RTL', async ({ page }, testInfo) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await page.evaluate(() => {
    const c = window.__coexHarness!; const data = c.fixture();
    if (data.objects.status !== 'known') throw Error('fixture');
    const object = data.objects.value[0];
    object.interface = 'fixture-long-' + 'alias'.repeat(140);
    object.virtualization = { status: 'unknown', reason: 'reason'.repeat(600) };
    object.addresses = { status: 'known', value: Array.from({ length: 1001 }, (_, i) => ({ address: `row-${i}`, prefixLen: 24 })) };
    data.objects.value = Array.from({ length: 128 }, (_, index) => ({ ...object, interface: index ? `fixture-other-${index}` : object.interface }));
    c.reply({ success: true, data });
  });
  await expect(card(page).locator('[data-coex-interface]')).toHaveCount(128);
  await card(page).locator('[data-coex-interface]').first().locator('summary').first().click();
  await expect(card(page)).toContainText('1–20 / 1001');
  await expect(card(page)).not.toContainText('row-20/24');
  await card(page).getByRole('button', { name: en.settings.coex.next, exact: true }).click();
  await expect(card(page)).toContainText('21–40 / 1001');
  await expect(card(page)).toContainText('row-20/24');
  await page.setViewportSize({ width: 720, height: 900 });
  await page.evaluate(() => window.__coexHarness!.setLanguage('fa'));
  await expect(card(page)).toContainText(fa.settings.coex.title);
  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await card(page).locator('[data-coex-interface]').first().locator('details summary').first().click();
  expect(await card(page).evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  await card(page).scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('coex-narrow-rtl.png') });
  await card(page).locator('[data-coex-interface]').nth(1).locator('summary').first().click();
  await expect(card(page).locator('[data-coex-interface][open]')).toHaveCount(1);
});


test('all five loaded locales display the mounted card and Unknown without translation-key fallback', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await replyFixture(page);
  for (const [language, locale] of [['en-US', en], ['zh-CN', zhCN], ['zh-TW', zhTW], ['ru', ru], ['fa', fa]] as const) {
    await page.evaluate((language) => window.__coexHarness!.setLanguage(language), language);
    await expect(card(page)).toContainText(locale.settings.coex.title);
    await expect(card(page)).toContainText(locale.settings.coex.classification);
    await expect(card(page)).toContainText(locale.settings.coex.unknown);
    await expect(card(page)).not.toContainText('settings.coex.');
    await expect(card(page).getByRole('button', { name: locale.settings.coex.collect, exact: true })).toBeEnabled();
  }
  expect(await calls(page)).toHaveLength(1);
});
