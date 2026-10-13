import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import type { CoreVersionInfo } from '@/contracts/types/update';
import type { HelperStatus } from '@/contracts/types/runtime';

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

/** Only two plain text spans bound to the supplied native identities may focus
 * to expose their full explanatory tooltip. A marker alone grants no exception. */
function verifiedIdentityNotes(markup: string, selectedCore: HelperStatus['selectedCore']) {
  const found = new Set<string>();
  if (!selectedCore) return { markup, found };
  const identities: Record<string, string> = {
    'selected-id': selectedCore.id,
    provider: selectedCore.providerFingerprint,
  };
  const cleaned = markup.replace(/<span\b([^>]*)>([^<]*)<\/span>/g, (whole, rawAttributes: string, text: string) => {
    const pairs = [...rawAttributes.matchAll(/([A-Za-z_:][\w:.-]*)="([^"]*)"/g)];
    if (rawAttributes.replace(/([A-Za-z_:][\w:.-]*)="([^"]*)"/g, '').trim() !== '') return whole;
    const attrs = Object.fromEntries(pairs.map(pair => [pair[1], pair[2]]));
    if (Object.keys(attrs).length !== pairs.length || Object.keys(attrs).sort().join(',') !== 'aria-label,class,data-core-info,data-tip,tabindex') return whole;
    const key = attrs['data-core-info'];
    if (!Object.prototype.hasOwnProperty.call(identities, key) || found.has(key)) return whole;
    const identity = identities[key];
    if (!/^[a-f0-9]{64}$/.test(identity) || attrs.class !== 'mono' || attrs.tabindex !== '0'
      || attrs['data-tip'] !== identity || attrs['aria-label'] !== identity || text !== identity.slice(0, 16)) return whole;
    found.add(key);
    return '';
  });
  return { markup: cleaned, found };
}

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

  it.each([
    ['present', '已核验'], ['absent', '未安装'], ['invalid', '核验失败'], ['unavailable', '不可用'],
  ] as const)('selected content %s remains a read-only native observation', (state, label) => {
    const selectedCore = { id: 'a'.repeat(64), version: '1.15.0-alpha.12', providerFingerprint: 'b'.repeat(64), state };
    const markup = renderToStaticMarkup(<CoreInfoCard info={FULL} selectedCore={selectedCore} />);
    expect(cell(markup, 'selected-id')).toBe(selectedCore.id.slice(0, 16));
    expect(cell(markup, 'provider')).toBe(selectedCore.providerFingerprint.slice(0, 16));
    expect(cell(markup, 'installed-state')).toBe(label);
    const withoutTip = markup.replace(/<span class="info-i"[\s\S]*?<\/span>/g, '');
    const notes = verifiedIdentityNotes(withoutTip, selectedCore);
    expect([...notes.found].sort()).toEqual(['provider', 'selected-id']);
    expect(INTERACTIVE.test(notes.markup)).toBe(false);
  });

  it('身份说明的窄豁免拒绝伪 marker、错 hash、额外属性、嵌套或真实动作', () => {
    const selectedCore = { id: 'a'.repeat(64), version: 'fixture', providerFingerprint: 'b'.repeat(64), state: 'present' as const };
    const note = `<span class="mono" data-core-info="selected-id" data-tip="${selectedCore.id}" tabindex="0" aria-label="${selectedCore.id}">${selectedCore.id.slice(0, 16)}</span>`;
    expect(verifiedIdentityNotes(note, selectedCore).found.size).toBe(1);
    expect(INTERACTIVE.test(verifiedIdentityNotes(note, selectedCore).markup)).toBe(false);
    for (const untrusted of [
      note.replace('data-core-info="selected-id"', 'data-core-info="fake-readonly"'),
      note.replace(`data-tip="${selectedCore.id}"`, `data-tip="${'c'.repeat(64)}"`),
      note.replace(`aria-label="${selectedCore.id}"`, `aria-label="${'c'.repeat(64)}"`),
      note.replace(selectedCore.id.slice(0, 16) + '</span>', 'c'.repeat(16) + '</span>'),
      note.replace(' tabindex=', ' role="button" tabindex='),
      note.replace(' tabindex=', ' onclick="write()" tabindex='),
      note.replace('tabindex="0"', 'tabindex="1"'),
      note.replace(selectedCore.id.slice(0, 16) + '</span>', '<button>write</button></span>'),
      note.replace('<span ', '<button ').replace('</span>', '</button>'),
      note + note,
      '<span tabindex="0">unbound note</span>',
    ]) {
      expect(INTERACTIVE.test(verifiedIdentityNotes(untrusted, selectedCore).markup), untrusted).toBe(true);
    }
    expect(INTERACTIVE.test(verifiedIdentityNotes(note, undefined).markup)).toBe(true);
    expect(INTERACTIVE.test(verifiedIdentityNotes(note, { ...selectedCore, id: 'c'.repeat(64) }).markup)).toBe(true);
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
    expect(pageSource).toContain('<CoreInfoCard info={coreInfo} selectedCore={selectedCore} />');
    expect(pageSource).toContain('coreApi\n      .getVersionInfo()');
    expect(pageSource).toContain("console.error('[SettingsUpdate] core version info unavailable:', error)");
  });
});
