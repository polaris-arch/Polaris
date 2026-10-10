import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import type { CoreVersionInfo } from '@/contracts/types/update';

const locale = JSON.parse(readFileSync(fileURLToPath(new URL('../../../i18n/locales/zh-CN.json', import.meta.url)), 'utf8')) as Record<string, unknown>;
function translate(key: string): string {
  let value: unknown = locale;
  for (const part of key.split('.')) value = (value as Record<string, unknown> | undefined)?.[part];
  if (typeof value !== 'string') throw new Error(`missing locale key: ${key}`);
  return value;
}

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: translate, i18n: { language: 'zh-CN' } }),
}));

const { default: CoreInfoCard } = await import('./CoreInfoCard');
const cardSource = readFileSync(fileURLToPath(new URL('./CoreInfoCard.tsx', import.meta.url)), 'utf8');
const pageSource = readFileSync(fileURLToPath(new URL('./SettingsUpdate.tsx', import.meta.url)), 'utf8');

const PATCH_SET = '0123456789abcdef0123456789abcdef01234567';
const FULL: CoreVersionInfo = { packagedVersion: '1.14.0-polaris.3', bundledVersion: '1.14.0-polaris.4', patchSet: PATCH_SET };
const render = (info: CoreVersionInfo | null) => renderToStaticMarkup(<CoreInfoCard info={info} />);
/** 取某一格渲染出来的文本；格不在场返回 `null`（与「在场但为空」区分开）。 */
const cell = (markup: string, name: string): string | null =>
  new RegExp(`data-core-info="${name}"[^>]*>([^<]*)<`).exec(markup)?.[1] ?? null;
/** 可操作控件：原生表单/按钮/链接元素，或带交互 role / tabindex 的任意元素。 */
const INTERACTIVE = /<(?:button|input|select|textarea|a)\b|role="(?:button|switch|radio|radiogroup|combobox|listbox|checkbox|link|tab)"|tabindex=/i;

describe('内核只读信息卡', () => {
  it('三格都渲染，且各自给的是自己那一个字段', () => {
    const markup = render(FULL);
    expect(markup).toContain('sing-box 内核');
    expect(markup).toContain('随包内核自报版本');
    // 第一格不得再叫「当前版本」：经提权助手运行时执行的是助手目录里的副本。
    expect(markup).not.toContain('当前版本');
    expect(markup).toContain('配套版本');
    expect(markup).toContain('补丁集');
    expect(cell(markup, 'packaged')).toBe('1.14.0-polaris.3');
    expect(cell(markup, 'bundled')).toBe('1.14.0-polaris.4');
    expect(cell(markup, 'patch-set')).toBe(PATCH_SET);
  });

  it('随包内核版本读不到（空串）时显示占位，不拿清单声明的版本或任何版本串顶替', () => {
    const markup = render({ ...FULL, packagedVersion: '' });
    expect(cell(markup, 'packaged')).toBe('—');
    expect(cell(markup, 'bundled')).toBe('1.14.0-polaris.4');
    // 清单声明的版本只应出现在它自己那一格：第一格若回落到它，这里会数出两次。
    expect(markup.split('1.14.0-polaris.4').length - 1).toBe(1);
  });

  it('清单不含补丁集标识时不画那一行；还没读到时两个版本格都是占位', () => {
    const noPatch = render({ ...FULL, patchSet: null });
    expect(cell(noPatch, 'patch-set')).toBeNull();
    expect(noPatch).not.toContain('补丁集');
    expect(cell(noPatch, 'packaged')).toBe('1.14.0-polaris.3');

    const pending = render(null);
    expect(cell(pending, 'packaged')).toBe('—');
    expect(cell(pending, 'bundled')).toBe('—');
    expect(cell(pending, 'patch-set')).toBeNull();
  });

  it('卡片里没有任何可操作控件', () => {
    // 谓词自检：它认得出按钮、开关与下拉（否则下面的否定断言没有信息量）。
    expect(INTERACTIVE.test('<button type="button">x</button>')).toBe(true);
    expect(INTERACTIVE.test('<span role="switch" tabindex="0"></span>')).toBe(true);
    expect(INTERACTIVE.test('<select><option>a</option></select>')).toBe(true);
    // 信息提示图标是说明文字的载体，不是卡片的动作；剥掉它再判，其余部分必须零控件。
    for (const info of [FULL, { ...FULL, packagedVersion: '' }, { ...FULL, patchSet: null }, null]) {
      const markup = render(info);
      const tips = markup.match(/<span class="info-i"[\s\S]*?<\/span>/g) ?? [];
      expect(tips.length, '被剥掉的只应是那一颗信息提示').toBe(1);
      const withoutTip = markup.replace(tips[0] ?? '', '');
      expect(INTERACTIVE.test(withoutTip), withoutTip).toBe(false);
    }
    expect(cardSource).not.toMatch(/onClick|onChange|<Button|<Switch|<Select|<Segmented|<TextInput/);
  });

  it('更新页挂的是这张卡，数据来自 core_get_version_info 那一条只读命令', () => {
    expect(pageSource).toContain('<CoreInfoCard info={coreInfo} />');
    expect(pageSource).toContain('coreApi\n      .getVersionInfo()');
    expect(pageSource).toContain("console.error('[SettingsUpdate] core version info unavailable:', error)");
  });
});
