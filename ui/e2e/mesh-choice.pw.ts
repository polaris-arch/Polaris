import { expect, test } from '@playwright/test';
import { LANGUAGE_STORAGE_KEY } from '../src/domain/language';

// Real production dialog/CSS, opt-in mock-data harness. No native account mutation.
const scenarios = [
  { width: 1200, language: 'zh-CN', state: 'configured', columns: 3 },
  { width: 1200, language: 'en-US', state: 'empty', columns: 3 },
  { width: 640, language: 'en-US', state: 'configured', columns: 2 },
  { width: 390, language: 'zh-CN', state: 'signed-out', columns: 1 },
  { width: 1200, language: 'ru', state: 'long', columns: 3 },
  { width: 390, language: 'fa', state: 'long', columns: 1 },
];
for (const { width, language, state, columns } of scenarios) {
      test(`access cards ${width}px ${language} ${state}`, async ({ page }) => {
        await page.setViewportSize({ width, height: width === 390 ? 500 : 1000 });
        await page.addInitScript(({ key, language }) => localStorage.setItem(key, language),
          { key: LANGUAGE_STORAGE_KEY, language });
        await page.goto(`/harness.html?mesh-choice-fixture=${state}`);
        const cards = page.locator('.mesh-choice > .mesh-col');
        await expect(cards).toHaveCount(state === 'empty' ? 6 : state === 'long' ? 8 : 7);
        await expect(page.locator('.access-picker-dlg')).toBeVisible();
        await expect(cards.first()).toBeVisible();
        const geometry = await page.locator('.mesh-choice-grid').evaluateAll(grids => grids.map(grid =>
          Array.from(grid.querySelectorAll('.mesh-choice')).map(choice => {
            const card = choice.querySelector('.mesh-col')!;
            const rect = card.getBoundingClientRect();
            const actions = choice.querySelector('.mesh-choice-actions');
            return { x: rect.x, y: rect.y, width: rect.width, height: rect.height,
              actionsY: actions?.getBoundingClientRect().y,
              overflow: Array.from(choice.querySelectorAll<HTMLElement>('.mesh-col, .mesh-tx, .mesh-col-sub, .btn'))
                .some(el => el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1),
            };
          })));
        const all = geometry.flat();
        expect(all.every(card => card.width > 0 && card.height > 0)).toBe(true);
        const tunnelCards = geometry[1];
        expect(tunnelCards.filter(card => Math.abs(card.y - tunnelCards[0].y) < 1)).toHaveLength(columns);
        expect(all.every(card => Math.abs(card.width - all[0].width) < 1)).toBe(true);
        for (const group of geometry) {
          for (const card of group) {
            expect(card.overflow).toBe(false);
            expect(card.height).toBeGreaterThanOrEqual(100);
            for (const peer of group.filter(peer => Math.abs(peer.y - card.y) < 1)) {
              expect(Math.abs(peer.height - card.height)).toBeLessThan(1);
              if (card.actionsY !== undefined && peer.actionsY !== undefined)
                expect(Math.abs(peer.actionsY - card.actionsY)).toBeLessThan(1);
            }
          }
        }
        if (state !== 'long' && language === 'zh-CN')
          expect(all.every(card => Math.abs(card.height - all[0].height) < 1)).toBe(true);
        const body = page.locator('.access-picker-dlg .dlg-body');
        expect(await body.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
        await cards.last().scrollIntoViewIfNeeded();
        await expect(cards.last()).toBeInViewport();
        const lastAction = page.locator('.mesh-choice-actions button').last();
        if (await lastAction.count()) {
          await lastAction.scrollIntoViewIfNeeded();
          await expect(lastAction).toBeInViewport();
        }
        await cards.first().focus();
        await expect(cards.first()).toBeFocused();
        // All wrapped action buttons remain focusable.
        const actions = page.locator('.mesh-choice-actions button');
        for (let i = 0; i < await actions.count(); i++) {
          await actions.nth(i).focus();
          await expect(actions.nth(i)).toBeFocused();
        }
        if (state === 'configured' && width === 1200) {
          const buttons = page.locator('.mesh-choice button');
          await buttons.first().focus();
          for (let i = 1; i < await buttons.count(); i++) {
            await page.keyboard.press('Tab');
            await expect(buttons.nth(i)).toBeFocused();
          }
        }
      });
}
