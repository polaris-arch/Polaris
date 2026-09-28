import { expect, test } from '@playwright/test';
import { LANGUAGE_STORAGE_KEY } from '../src/domain/language';
import enUS from '../src/i18n/locales/en-US.json' with { type: 'json' };

const url = '/harness.html?mesh-inbound-fixture=1';
const savedPolicy = (page: import('@playwright/test').Page) => page.evaluate(() => {
  const raw = window.sessionStorage.getItem('polaris-harness-mesh-inbound');
  if (!raw) return null;
  const config = JSON.parse(raw) as { servers: Array<{ id: string; meshInboundPolicy?: unknown }> };
  return config.servers.find((server) => server.id === 'mesh-ingress-ts')?.meshInboundPolicy ?? null;
});
const routing = async (page: import('@playwright/test').Page) => {
  await page.locator('#ts-settings-form-tab-routing').click();
  await expect(page.locator('#ts-mesh-inbound-mode')).toBeVisible();
};
const mode = async (page: import('@playwright/test').Page, value: 'legacy' | 'block' | 'allowlist') => {
  await page.locator('#ts-mesh-inbound-mode').click();
  await page.getByRole('option', { name: enUS.meshInbound[value], exact: true }).click();
};
const save = async (page: import('@playwright/test').Page) => {
  await page.getByRole('button', { name: enUS.common.save, exact: true }).click();
};

test('real Tailscale form saves, reopens, edits and removes ingress policy', async ({ page }) => {
  await page.addInitScript(({ key }) => window.localStorage.setItem(key, 'en-US'), { key: LANGUAGE_STORAGE_KEY });
  await page.goto(url);
  await routing(page);
  await mode(page, 'allowlist');
  await page.getByRole('button', { name: enUS.meshInbound.add, exact: true }).click();
  await page.locator('#ts-mesh-source-0').fill('10.77.0.2/32');
  await page.locator('#ts-mesh-ports-0').fill('8080');
  await save(page);
  await expect.poll(() => savedPolicy(page)).toEqual({
    mode: 'allowlist',
    rules: [{ sourceCidrs: ['10.77.0.2/32'], network: 'tcp', ports: ['8080'], target: 'local' }],
  });

  await page.reload();
  await routing(page);
  await expect(page.locator('#ts-mesh-source-0')).toHaveValue('10.77.0.2/32');
  await expect(page.locator('#ts-mesh-ports-0')).toHaveValue('8080');
  await mode(page, 'block');
  await save(page);
  await expect.poll(() => savedPolicy(page)).toEqual({ mode: 'block' });

  await page.reload();
  await routing(page);
  await mode(page, 'legacy'); // remove the field rather than writing a permissive object
  await save(page);
  await expect.poll(() => savedPolicy(page)).toBeNull();
});
