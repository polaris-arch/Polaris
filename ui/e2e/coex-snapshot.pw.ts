/** Mounted App -> SettingsTun -> system wrapper -> real ipc-client -> mockIPC.
 * Browser receipts are not native IPC, platform collection or device acceptance. */
import { expect, test, type Page } from '@playwright/test';
import { LANGUAGE_STORAGE_KEY } from '../src/domain/language';
import en from '../src/i18n/locales/en-US.json' with { type: 'json' };
import zhCN from '../src/i18n/locales/zh-CN.json' with { type: 'json' };
import zhTW from '../src/i18n/locales/zh-TW.json' with { type: 'json' };
import ru from '../src/i18n/locales/ru.json' with { type: 'json' };
import fa from '../src/i18n/locales/fa.json' with { type: 'json' };
import rustFixture from '../src/contracts/coex-snapshot.rust.fixture.json' with { type: 'json' };

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
    if (platform === 'win32') await expect(card(page)).toContainText(en.settings.coex.sourceUnavailable);
    else await expect(card(page)).not.toContainText(en.settings.coex.sourceUnavailable);
    await expect(card(page)).toContainText('production source unavailable');
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


for (const [name, wire] of Object.entries(rustFixture.snapshots).filter(([name]) => name === 'linuxCollector' || name === 'optionShapes')) {
  test(`actual Rust serializer ${name} crosses mounted wrapper and renders numeric/null table facts`, async ({ page }) => {
    await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
    await page.evaluate((data) => window.__coexHarness!.reply({ success: true, data }), wire);
    await card(page).locator('summary').filter({ hasText: 'wire-fixture0' }).click();
    await expect(card(page).getByRole('alert')).toHaveCount(0);
    await expect(card(page)).toContainText(`${en.settings.coex.table}: 254`);
    await expect(card(page)).toContainText(`${en.settings.coex.lookupTable}: 100`);
    if (name === 'optionShapes') {
      await expect(card(page).getByText(en.settings.coex.noTableNumber, { exact: false })).toHaveCount(2);
      await expect(card(page)).toContainText(`${en.settings.coex.table}: 4294967295`);
      await expect(card(page)).toContainText(`${en.settings.coex.lookupTable}: 0`);
      await expect(card(page)).toContainText('table source unreadable');
    }
    expect(await calls(page)).toHaveLength(1);
  });
}

for (const name of ['macosCollector', 'macosPartialSources', 'macosUnknownScope'] as const) {
  test(`actual Rust macOS ${name} crosses mounted wrapper without platform-absence or selection inference`, async ({ page }, testInfo) => {
    await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
    await page.evaluate((name) => {
      const control = window.__coexHarness!;
      control.reply({ success: true, data: control.serializedFixture(name) });
    }, name);
    await expect(card(page).getByRole('alert')).toHaveCount(0);
    await expect(card(page)).toContainText('darwin');
    await expect(card(page)).not.toContainText(en.settings.coex.sourceUnavailable);
    await card(page).locator('summary').filter({ hasText: name === 'macosCollector' ? 'en0' : 'wire-mac0' }).click();
    await expect(card(page)).toContainText(en.settings.coex.unknown);
    await expect(card(page).locator('.good, .success, .safe')).toHaveCount(0);
    if (name === 'macosCollector') {
      await expect(card(page)).toContainText('192.168.10.142/24');
      await expect(card(page)).toContainText(en.settings.coex.noTableNumber);
      await expect(card(page)).toContainText('route role and selection not established');
    } else if (name === 'macosPartialSources') {
      await expect(card(page)).toContainText('10.77.2.9/24');
      await expect(card(page)).toContainText('injected IPv6 permission denied');
    } else {
      await expect(card(page)).toContainText('non-contiguous IPv4 mask');
      await expect(card(page)).toContainText('printed flags may truncate');
      await expect(card(page)).toContainText(en.settings.coex.noTableNumber);
    }
    expect(await calls(page)).toHaveLength(1);
    // Element screenshots extend outside the scroll container's visible clip.
    // Save actual viewports after scrolling the facts into view.
    if (name === 'macosCollector') {
      await card(page).getByText('192.168.10.142/24', { exact: false }).scrollIntoViewIfNeeded();
      await page.screenshot({ path: testInfo.outputPath('coex-macos-addresses.png') });
      await card(page).getByText(en.settings.coex.noTableNumber, { exact: false }).first().scrollIntoViewIfNeeded();
    }
    await page.screenshot({ path: testInfo.outputPath(`coex-${name}.png`) });
  });
}

const windowsReply = (page: Page, name: 'full' | 'partial' | 'empty' | 'paged' | 'busy') => page.evaluate((name) => {
  const c = window.__coexHarness!; c.reply({ success: true, data: c.serializedWindowsFixture(name) });
}, name);
const source = (page: Page, id: string) => card(page).locator(`[data-coex-source="${id}"]`);
const expandSource = async (page: Page, id: string) => { await source(page, id).locator(':scope > summary').click(); };
test('Windows actual Rust wire: five sources, lossless LUID, dual family indices, scope and masked unassociated RAS', async ({ page }, testInfo) => {
  const button = card(page).getByRole('button', { name: en.settings.coex.collect, exact: true });
  await button.evaluate((element: HTMLButtonElement) => { element.click(); element.click(); element.click(); });
  await expect.poll(() => calls(page)).toHaveLength(1);
  await windowsReply(page, 'full');
  await expect(card(page).locator('[data-coex-windows]')).toBeVisible();
  await expect(card(page).locator('[data-coex-source]')).toHaveCount(5);
  for (const id of ['adapters', 'addresses', 'routes4', 'routes6', 'ras']) await expandSource(page, id);
  await expect(source(page, 'adapters')).toContainText('18446744073709551615');
  await expect(source(page, 'addresses')).toContainText('fe80::1234');
  await expect(source(page, 'addresses')).toContainText(`${en.settings.coex.index}: 17`);
  await expect(source(page, 'addresses')).toContainText(`${en.settings.coex.index}: 42`);
  await expect(source(page, 'addresses')).toContainText(`${en.settings.coex.scopeId}: 42`);
  await expect(source(page, 'routes6')).toContainText(`${en.settings.coex.nextHopScopeId}: 42`);
  await expect(source(page, 'ras').locator('li')).toHaveCount(2);
  await expect(source(page, 'ras')).toContainText('Fixture RAS');
  await expect(source(page, 'ras')).toContainText(en.settings.coex.unknown);
  await expect(card(page)).not.toContainText('12025550123');
  await expect(card(page)).not.toContainText(en.settings.coex.sourceUnavailable);
  await expect(card(page).locator('.good, .safe, .success')).toHaveCount(0);
  await card(page).screenshot({ path: testInfo.outputPath('windows-five-sources.png') });
  await card(page).getByRole('button', { name: en.settings.coex.retry, exact: true }).click();
  await expect(card(page).locator('[data-coex-windows]')).toHaveCount(0);
  await windowsReply(page, 'full'); expect(await calls(page)).toHaveLength(2);
});
test('Windows IPv6 permission failure keeps independent sources and allows manual retry', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click(); await windowsReply(page, 'partial');
  await expandSource(page, 'addresses'); await expandSource(page, 'routes6');
  await expect(source(page, 'addresses')).toContainText('10.77.2.9');
  await expect(source(page, 'routes6')).toContainText('Windows GetIpForwardTable2 failed (code 5)');
  await expect(source(page, 'routes6')).toContainText(en.settings.coex.unknown);
  await expect(source(page, 'routes6')).not.toContainText(en.settings.coex.observedEmpty);
  await card(page).getByRole('button', { name: en.settings.coex.retry, exact: true }).click(); await windowsReply(page, 'full');
  await expect(card(page)).not.toContainText('failed (code 5)');
});
test('Windows known empty and busy are separate source facts', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click(); await windowsReply(page, 'empty');
  await expandSource(page, 'adapters');
  await expect(source(page, 'adapters')).toContainText(en.settings.coex.observedEmpty);
  await expect(source(page, 'adapters')).toContainText(en.settings.coex.unknown);
  await card(page).getByRole('button', { name: en.settings.coex.retry, exact: true }).click(); await windowsReply(page, 'busy');
  await expandSource(page, 'adapters'); await expect(source(page, 'adapters')).toContainText('snapshot worker busy');
  await expect(source(page, 'adapters')).not.toContainText(en.settings.coex.observedEmpty);
});
test('Windows source row pagination retains all 41 rows with explicit pages', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click(); await windowsReply(page, 'paged');
  await expandSource(page, 'addresses'); const panel = source(page, 'addresses');
  await expect(panel.locator('li')).toHaveCount(20); await expect(panel).toContainText('1–20 / 41');
  await panel.getByRole('button', { name: en.settings.coex.next, exact: true }).click();
  await expect(panel).toContainText('21–40 / 41'); await expect(panel.locator('li')).toHaveCount(20);
  await panel.getByRole('button', { name: en.settings.coex.next, exact: true }).click();
  await expect(panel).toContainText('41–41 / 41'); await expect(panel.locator('li')).toHaveCount(1);
  await expect(panel.getByRole('button', { name: en.settings.coex.next, exact: true })).toBeDisabled();
  await panel.getByRole('button', { name: en.settings.coex.previous, exact: true }).click(); await expect(panel).toContainText('21–40 / 41');
});
test('Windows unsafe wire rejection is private and retryable on the actual wrapper', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await page.evaluate(() => {
    const c = window.__coexHarness!; const wire = c.serializedWindowsFixture('full') as typeof import('../src/contracts/coex-windows.rust.fixture.json').snapshots.full;
    Object.assign(wire.sources.ras.rows.value[0].name, { status: 'known', value: '.12025550123' });
    delete (wire.sources.ras.rows.value[0].name as {reason?: string}).reason;
    c.reply({ success: true, data: wire });
  });
  await expect(card(page).getByRole('alert')).toContainText(en.settings.coex.malformed);
  await expect(card(page)).not.toContainText('12025550123'); await expect(card(page).locator('[data-coex-windows]')).toHaveCount(0);
  await card(page).getByRole('button', { name: en.settings.coex.retry, exact: true }).click(); await windowsReply(page, 'full');
  await expect(card(page).locator('[data-coex-windows]')).toBeVisible();
});
test('Windows delayed reply after leaving does not replace a new manual report', async ({ page }) => {
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click();
  await page.getByRole('button', { name: en.settings.nav.general, exact: true }).click(); await openTun(page);
  await windowsReply(page, 'full'); await expect(card(page).locator('[data-coex-windows]')).toHaveCount(0);
  await card(page).getByRole('button', { name: en.settings.coex.collect, exact: true }).click(); await windowsReply(page, 'partial');
  await expandSource(page, 'routes6'); await expect(source(page, 'routes6')).toContainText('failed (code 5)');
  expect(await calls(page)).toHaveLength(2);
});

for (const [language, locale] of Object.entries({ 'en-US': en, 'zh-CN': zhCN, 'zh-TW': zhTW, ru, fa })) {
  test(`Windows five-source labels and retry remain usable in ${language}`, async ({ page }) => {
    if (language === 'fa') await page.setViewportSize({ width: 390, height: 844 });
    await page.evaluate((language) => window.__coexHarness!.setLanguage(language), language);
    await card(page).getByRole('button', { name: locale.settings.coex.collect, exact: true }).click(); await windowsReply(page, 'full');
    for (const id of ['adapters', 'addresses', 'routes4', 'routes6', 'ras']) await expandSource(page, id);
    await expect(source(page, 'ras')).toContainText(locale.settings.coex.rasName);
    await expect(source(page, 'ras')).toContainText(locale.settings.coex.association);
    await expect(source(page, 'addresses')).toContainText(locale.settings.coex.scopeId);
    await expect(source(page, 'adapters')).toContainText('18446744073709551615');
    await expect(card(page)).not.toContainText('12025550123');
    const retry = card(page).getByRole('button', { name: locale.settings.coex.retry, exact: true });
    await expect(retry).toBeEnabled(); await retry.click(); await windowsReply(page, 'empty');
    await expect(card(page).locator('[data-coex-source]')).toHaveCount(5);
  });
}
