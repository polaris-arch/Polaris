/**
 * Settings UI 治理门。
 *
 * 这里守的是页面级规则，而不是某个页面当下的像素：
 *  1. 下拉只能经共享 Select → Csel，禁止重新引入系统原生弹层；
 *  2. 设置项与从属内容只能用语义分组组件组织，页面不得靠内联边框补缝；
 *  3. disabled 必须传到真实触发器，不能只有变灰但仍可点击的假禁用态。
 *  4. 静态帮助统一进入标题旁信息提示，常驻 desc 只承载当前状态/警告/动作。
 *  5. 设置页标题与短说明保持同行，说明文案不以句号制造段落感。
 */
import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const read = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const stripComments = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');

const settingScreens = readdirSync(here)
  .filter((name) => /^Settings.+\.tsx$/.test(name))
  .map((name) => ({ name, source: stripComments(read(`./${name}`)) }));

describe('Settings UI 使用统一组件与语义分组', () => {
  it('所有设置页都不直接渲染原生 select', () => {
    for (const { name, source } of settingScreens) {
      expect(source, `${name} 重新引入了系统原生下拉`).not.toMatch(/<\/?select\b/);
    }
  });

  it('页面不以内联上下边框拼装设置分组', () => {
    for (const { name, source } of settingScreens) {
      expect(source, `${name} 使用了内联 borderTop/borderBottom`).not.toMatch(
        /\bborder(?:Top|Bottom)\s*:/,
      );
      expect(source, `${name} 使用了 border: 0 局部消线`).not.toMatch(/\bborder\s*:\s*0\b/);
    }
  });

  it('可见标签与无障碍名称不写死自然语言', () => {
    const technicalNames = new Set(['MTU', 'CIDR', 'MAC', 'FakeIP', 'DoH URL']);
    const violations: string[] = [];
    for (const { name, source } of settingScreens) {
      for (const match of source.matchAll(/\b(?:label|aria-label|ariaLabel)="([^"]+)"/g)) {
        if (!technicalNames.has(match[1])) violations.push(`${name}: ${match[1]}`);
      }
    }
    expect(violations, '自然语言应来自 locale；这里只允许跨语言同形的技术名').toEqual([]);
  });

  it('开关行只常驻简短名称，复杂说明统一进入信息提示', () => {
    const violations: string[] = [];
    for (const { name, source } of settingScreens) {
      // The one platform warning row is covered by actual macOS/Windows/Linux render cases.
      // All other switch rows still reject permanently expanded static explanations.
      const copy = name === 'SettingsTun.tsx'
        ? source.replace(/<SetRow\s+id="set-strict-route"[\s\S]*?<\/SetRow>/, '') : source;
      if (/<SetRow\b[^>]*\bdesc=\{[^}]+\}[^>]*>\s*(?:\{\/\*[\s\S]*?\*\/\}\s*)?<Switch/.test(copy))
        violations.push(name);
    }
    expect(violations, '开关行仍永久铺开说明，应改用 SetRow tip').toEqual([]);
  });

  it('静态字段说明统一进入信息提示，常驻 desc 只保留动态上下文', () => {
    const allowedDescCounts: Record<string, number> = {
      'SettingsGeneral.tsx': 1, // 密码已设置/未设置
      'SettingsNetwork.tsx': 5, // WebRTC 模式、代理端口、网卡失败、测速并发上限及实际 TLS 指纹能力状态
      'SettingsTun.tsx': 2, // IPv6/FakeIP组合风险，以及严格路由的平台禁用/启用风险
    };
    for (const { name, source } of settingScreens) {
      const count = source.match(/\bdesc=/g)?.length ?? 0;
      expect(
        count,
        `${name} 出现新的常驻说明；静态帮助应改用 SetRow tip，动态状态需登记本门`,
      ).toBe(allowedDescCounts[name] ?? 0);
    }
  });

  it('折叠清单的静态说明也使用 Fold tip，不在内容区铺 fld-hint', () => {
    for (const { name, source } of settingScreens) {
      expect(source, `${name} 的折叠清单仍在内容区常驻静态说明`).not.toContain('fld-hint');
    }
  });

  it('共享 Select 由 Csel 实现并把 disabled 传到真实控件', () => {
    const primitives = stripComments(read('./Primitives.tsx'));
    const selectStart = primitives.indexOf('export function Select');
    const selectEnd = primitives.indexOf('export function TextInput', selectStart);
    const selectBody = selectStart >= 0 && selectEnd > selectStart
      ? primitives.slice(selectStart, selectEnd)
      : undefined;
    expect(selectBody, '找不到共享 Select 实现').toBeDefined();
    expect(selectBody).toContain('<Csel');
    expect(selectBody).toContain('disabled={disabled}');
    expect(selectBody).not.toMatch(/<\/?select\b/);
  });

  it('连接域名处理归属设置 DNS，DNS 规则页与路由规则不保留重复入口', () => {
    const network = read('./SettingsNetwork.tsx');
    const dns = read('./SettingsDns.tsx');
    const dnsWorkspace = read('../rules/DnsPolicyWorkspace.tsx');
    expect(network).not.toContain('resolveBeforeDial');
    expect(dns).not.toContain('resolveBeforeDial');
    expect(dnsWorkspace).not.toContain("t('settings.dns.connectionResolution')");
    expect(dnsWorkspace).not.toContain('effectiveDnsConnectionResolution(config)');
  });

  it('流量规则只写路由动作，不再写入 DNS 解析行为', () => {
    const dialog = stripComments(read('../../dialogs/RuleDialog.tsx'));
    const item = stripComments(read('../rules/RuleItem.tsx'));
    expect(dialog).not.toContain('destinationResolution');
    expect(dialog).not.toContain('rule-destination-resolution');
    expect(dialog).not.toContain('resolutionOnly');
    expect(item).not.toContain('resolutionOnly');
  });
});
