import { test, expect, type Page } from '@playwright/test';

// Existing browser-only App harness and real production CSS/components. No native processes.
declare global {
  interface Window {
    __uiLayoutHarness: {
      settings(screen: string): Promise<void>;
      main(screen: string): Promise<void>;
      language(language: string): Promise<void>;
      connection(running: boolean): Promise<void>;
      stats(): Promise<void>;
      exitIdentity(running: boolean, actualServerId: string | null, savedServerId: string, auto?: boolean): Promise<void>;
      selection(): string | null;
    };
  }
}
async function open(page: Page, unsupported = true) {
  await page.route('**/*', route => {
    const url = new URL(route.request().url());
    return url.hostname === '127.0.0.1' ? route.continue() : route.abort();
  });
  await page.goto(`/harness.html?ui-layout-fixture=1${unsupported ? '&admission=unsupported' : ''}`);
  await page.waitForFunction(() => !!window.__uiLayoutHarness);
  await page.evaluate(() => window.__uiLayoutHarness.language('zh-CN'));
  await expect(page.locator('.statusbar')).toBeVisible();
}

test('warning upgrade action reaches a real actionable Helper state', async ({ page }) => {
  await open(page, false);
  const toast = page.locator('.toast.warn');
  await expect(toast).toBeVisible();
  const cdp = await page.context().newCDPSession(page);
  const color = await cdp.send('Runtime.evaluate', { expression: `(() => {
    const toast=document.querySelector('.toast.warn'); const probe=document.createElement('span');
    probe.style.color='hsl(var(--warn))'; toast.append(probe);
    const result={actual:getComputedStyle(toast).color,expected:getComputedStyle(probe).color};probe.remove();return result;
  })()`, returnByValue: true });
  expect(color.result.value.actual).toBe(color.result.value.expected);
  await cdp.detach();
  await toast.locator('.toast-action').click();
  await expect(page.locator('[data-s="needs-upgrade"]')).toBeVisible();
  await expect(page.locator('[data-s="needs-upgrade"] button').first()).toBeEnabled();
  // Never click installation/migration: this gate only checks the truthful available action.
});

test('unsupported state uses compact warning, spaced actions, original diagnostics and stable rate slots', async ({ page }, info) => {
  await open(page);
  await expect(page.locator('.toast.warn')).toHaveCount(0);
  await page.evaluate(() => window.__uiLayoutHarness.settings('helper'));
  const notice = page.locator('[data-s="admission-unavailable"] .feedback-notice.warning');
  await expect(notice).toBeVisible();
  await expect(notice.getByRole('button', { name: '重新检测' })).toBeVisible();
  const cdp = await page.context().newCDPSession(page);
  const measured = await cdp.send('Runtime.evaluate', { expression: `(() => {
    const notice=document.querySelector('.helper-state .feedback-notice');
    const title=notice.querySelector('.feedback-notice-title');
    return {titleFont:getComputedStyle(title).fontSize, color:getComputedStyle(title).color,
      actionsGap:getComputedStyle(notice.querySelector('.feedback-notice-actions')).gap};
  })()`, returnByValue: true });
  expect(measured.result.value.titleFont).toBe('13px');
  expect(measured.result.value.actionsGap).toBe('8px');
  await notice.getByRole('button', { name: '检查更新' }).click();
  const core = page.locator('#core-info-card');
  await expect(core).toBeVisible();
  const version = core.locator('[data-core-info="bundled"]');
  await expect(version).toHaveText('1.15.0-polaris-alpha.12.7');
  expect(await version.evaluate(el => getComputedStyle(el).fontSize)).toBe('13px');
  await core.getByTestId('core-diagnostics').locator('summary').click();
  await expect(core.getByTestId('core-diagnostics')).toContainText('选定内容标识');
  await expect(core).not.toContainText('fixture-patch-set');
  await core.getByTestId('core-diagnostics').locator('summary').click();
  await core.scrollIntoViewIfNeeded();
  // Disconnected slots stay visible, including after a late historical stats frame.
  await page.evaluate(() => window.__uiLayoutHarness.stats());
  await expect(page.locator('.sb-count .rate-dn')).toHaveText('↓ —');
  await expect(page.locator('.sb-count .rate-up')).toHaveText('↑ —');
  await expect(page.locator('.sb-count .sb-conns b')).toHaveText('—');
  await page.evaluate(() => window.__uiLayoutHarness.connection(true));
  await expect(page.locator('.sb-count .rate-dn')).toHaveText('↓ —');
  await page.evaluate(() => window.__uiLayoutHarness.stats());
  await expect(page.locator('.sb-count[data-stats-available="true"]')).toBeVisible();
  await expect(page.locator('.sb-count .sb-conns b')).toHaveText('3');
  await page.evaluate(() => window.__uiLayoutHarness.connection(false));
  await expect(page.locator('.sb-count .rate-dn')).toHaveText('↓ —');
  await page.evaluate(() => window.__uiLayoutHarness.connection(true));
  await expect(page.locator('.sb-count .rate-dn')).toHaveText('↓ —');
  await page.evaluate(() => window.__uiLayoutHarness.connection(false));
  await info.attach('cdp-compact-warning', { body: JSON.stringify({ version: await cdp.send('Browser.getVersion'), measurements: measured.result.value, layout: await cdp.send('Page.getLayoutMetrics') }), contentType: 'application/json' });
  await cdp.detach();
  await page.screenshot({ path: info.outputPath('core-warning-dark.png') });
});

test('natural default order and sticky locate clear filters and reveal the selected deep batch', async ({ page }, info) => {
  await open(page);
  await page.evaluate(() => window.__uiLayoutHarness.main('nodes'));
  await expect(page.locator('.nd-card').first()).toBeVisible();
  await expect(page.locator('.nd-card .nd-name').nth(0)).toHaveText('JP01');
  await expect(page.locator('.nd-card .nd-name').nth(1)).toHaveText('JP01-H3');
  await expect(page.locator('.nd-card .nd-name').nth(2)).toHaveText('Jp01-MASQUE');
  const locate = page.getByRole('button', { name: '定位当前', exact: true });
  await expect(locate).toBeEnabled();
  const search = page.locator('.node-toolbar input');
  await search.fill('no-such-fixture-node');
  await expect(page.locator('.nd-card')).toHaveCount(0);
  await locate.click();
  await expect(search).toHaveValue('');
  const target = page.locator('[data-node-id="layout-180"]');
  await expect(target).toBeVisible();
  await expect(target).toBeFocused();
  await expect(target).toBeInViewport();
  await expect.poll(async () => {
    const box=await target.boundingBox(), parent=await page.locator('.main-scroll').boundingBox();
    return !!box && !!parent && box.y >= parent.y && box.y + box.height <= parent.y + parent.height + 1;
  }).toBe(true);
  const rect = await target.boundingBox();
  expect(rect).not.toBeNull();
  expect(rect!.y).toBeGreaterThanOrEqual(0);
  expect(rect!.y).toBeLessThan(page.viewportSize()!.height);
  expect(await page.locator('.nd-card').count()).toBeLessThan(320);
  const position = await page.locator('.node-toolbar').evaluate(el => {
    const parent=el.closest('.main-scroll')!.getBoundingClientRect();
    return { top:el.getBoundingClientRect().top,parentTop:parent.top,position:getComputedStyle(el).position };
  });
  expect(position.position).toBe('sticky');
  expect(position.top).toBeGreaterThanOrEqual(position.parentTop - 1);
  // A second locate still works after the first highlight has ended.
  await locate.click();
  await expect(target).toBeFocused();
  await expect.poll(async () => {
    const box=await target.boundingBox(), parent=await page.locator('.main-scroll').boundingBox();
    return !!box && !!parent && box.y >= parent.y && box.y + box.height <= parent.y + parent.height + 1;
  }).toBe(true);
  await page.screenshot({ path: info.outputPath('nodes-locate.png') });
});

test('all desktop screens retain their layout across light/dark and RTL with no page errors', async ({ page }, info) => {
  const errors: string[]=[];
  page.on('pageerror', error => errors.push(error.message));
  await open(page);
  for (const theme of ['light', 'dark']) {
    await page.evaluate(theme => { document.documentElement.dataset.theme=theme; }, theme);
    for (const screen of ['home','nodes','rules','dnsrules','apppolicy','resources','connections','logs']) {
      await page.evaluate(screen => window.__uiLayoutHarness.main(screen), screen);
      await expect(page.locator('.statusbar')).toBeVisible();
      await expect(page.locator('.main-scroll h1').first()).toBeVisible();
      await page.screenshot({ path: info.outputPath(`${theme}-main-${screen}.png`) });
    }
    for (const screen of ['general','display','network','dns','tun','update','backup','helper','about']) {
      await page.evaluate(screen => window.__uiLayoutHarness.settings(screen), screen);
      await expect(page.locator('.main-scroll h1').first()).toBeVisible();
      await page.screenshot({ path: info.outputPath(`${theme}-settings-${screen}.png`) });
    }
    await page.evaluate(() => window.__uiLayoutHarness.settings('update'));
    await expect(page.locator('#core-info-card')).toBeVisible();
    await page.screenshot({ path: info.outputPath(`update-${theme}.png`) });
  }
  await page.setViewportSize({ width:1024,height:768 });
  await page.evaluate(() => window.__uiLayoutHarness.language('fa'));
  await expect(page.locator('#core-info-card')).toBeVisible();
  const overflow=await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
  expect(overflow).toBe(false);
  await page.screenshot({ path: info.outputPath('update-fa-1024.png') });
  expect(errors).toEqual([]);
});


test('locate uses the actual running identity and keeps unknown/Auto exits disabled', async ({ page }) => {
  await open(page);
  await page.evaluate(() => window.__uiLayoutHarness.main('nodes'));
  const locate = page.getByRole('button', { name: '定位当前', exact: true });
  await expect(page.locator('[data-node-id="layout-190"]')).toHaveCount(0);
  await page.evaluate(() => window.__uiLayoutHarness.exitIdentity(true, 'layout-190', 'layout-180'));
  const search = page.locator('.node-toolbar input');
  await search.fill('no-such-fixture-node');
  await locate.click();
  await expect(search).toHaveValue('');
  await expect(page.locator('[data-node-id="layout-190"]')).toBeFocused();
  expect(await page.evaluate(() => window.__uiLayoutHarness.selection())).toBe('layout-180');
  await page.evaluate(() => window.__uiLayoutHarness.exitIdentity(true, null, 'layout-180'));
  await expect(locate).toBeDisabled();
  expect(await page.evaluate(() => document.activeElement?.getAttribute('data-node-id'))).not.toBe('layout-180');
  await page.evaluate(() => window.__uiLayoutHarness.exitIdentity(false, null, 'layout-180'));
  await expect(locate).toBeEnabled();
  await locate.click();
  await expect(page.locator('[data-node-id="layout-180"]')).toBeFocused();
  await page.evaluate(() => window.__uiLayoutHarness.exitIdentity(false, null, 'layout-180', true));
  await expect(locate).toBeDisabled();
  expect(await page.evaluate(() => window.__uiLayoutHarness.selection())).toBe('layout-180');
});
