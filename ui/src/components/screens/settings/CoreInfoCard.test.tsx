import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import { isValidElement, type ReactElement } from 'react';
import type { CoreVersionInfo } from '@/contracts/types/update';

const h = vi.hoisted(() => ({ copyState: 'idle', enterSettings: vi.fn() }));
vi.mock('react', async original => ({
  ...await original<typeof import('react')>(),
  useState: () => [h.copyState, (value: string) => { h.copyState = value; }],
}));
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
vi.mock('@/store/nav-store', () => ({ useNavStore: { getState: () => ({ enterSettings: h.enterSettings }) } }));
vi.mock('./CoreRecoveryActions', () => ({ default: () => <div data-core-recovery="current-app">
  {translate('coreRecovery.check')}<a href="#app-update-card">{translate('coreRecovery.reinstall')}</a>
</div> }));
const { default: CoreRecoveryActions } = await import('./CoreRecoveryActions');
const { default: CoreInfoCard } = await import('./CoreInfoCard');
const pageSource = readFileSync(fileURLToPath(new URL('./SettingsUpdate.tsx', import.meta.url)), 'utf8');
const PATCH_SET = '0123456789abcdef0123456789abcdef01234567';
const FULL: CoreVersionInfo = { packagedVersion: '1.14.0-polaris.3', bundledVersion: '1.14.0-polaris.4', patchSet: PATCH_SET };
const selectedCore = { id: 'a'.repeat(64), version: '1.15.0-alpha.12', providerFingerprint: 'b'.repeat(64), state: 'present' as const };
const render = (info: CoreVersionInfo | null) => renderToStaticMarkup(<CoreInfoCard info={info} />);
const cell = (markup: string, name: string): string | null =>
  new RegExp(`data-core-info="${name}"[^>]*>([^<]*)<`).exec(markup)?.[1] ?? null;
function nodes(node: unknown): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!isValidElement<Record<string, unknown>>(node)) return [];
  return [node, ...nodes(node.props.children)];
}
function copyButton(core = selectedCore) {
  const tree = CoreInfoCard({ info: FULL, selectedCore: core });
  return nodes(tree).find(n => typeof n.props.onClick === 'function')!;
}
beforeEach(() => { h.copyState = 'idle'; h.enterSettings.mockClear(); });
afterEach(() => { vi.unstubAllGlobals(); });

describe('内核信息与只读诊断', () => {
  it('分别显示随包自报、配套、选定版本，不将它们称为实际运行版本', () => {
    const markup = renderToStaticMarkup(<CoreInfoCard info={FULL} selectedCore={selectedCore} />);
    expect(cell(markup, 'packaged')).toBe('1.14.0-polaris.3');
    expect(cell(markup, 'bundled')).toBe('1.14.0-polaris.4');
    expect(cell(markup, 'selected-version')).toBe('1.15.0-alpha.12');
    expect(cell(markup, 'patch-set')).toBeNull();
    expect(markup).not.toContain(PATCH_SET);
    expect(markup).toContain('不代表当前正在运行的内核身份');
    expect(markup).not.toContain('当前版本');
  });
  it('缺失读数保留未知，不用另一份版本或选定身份顶替', () => {
    const markup = render({ ...FULL, packagedVersion: '' });
    expect(cell(markup, 'packaged')).toBe('—');
    expect(cell(markup, 'bundled')).toBe(FULL.bundledVersion);
    expect(cell(markup, 'selected-version')).toBe('—');
    expect(cell(markup, 'installed-state')).toBe('未知／未观测');
    const pending = render(null);
    expect(cell(pending, 'packaged')).toBe('—');
    expect(cell(pending, 'bundled')).toBe('—');
    expect(cell(pending, 'patch-set')).toBeNull();
    expect(cell(render({ ...FULL, patchSet: null }), 'patch-set')).toBeNull();
    expect(nodes(CoreInfoCard({ info: null })).find(n => typeof n.props.onClick === 'function')?.props.disabled).toBe(true);
  });
  it.each([
    ['present', '选定安装内容已核验'], ['absent', '选定内容未安装'],
    ['invalid', '选定内容核验失败'], ['unavailable', '未能核验选定内容'],
  ] as const)('明确 %s 核验对象，完整身份只出现在默认关闭的诊断内', (state, label) => {
    const markup = renderToStaticMarkup(<CoreInfoCard info={FULL} selectedCore={{ ...selectedCore, state }} />);
    expect(cell(markup, 'installed-state')).toBe(label);
    const details = /<details\b([^>]*)>([\s\S]*?)<\/details>/.exec(markup)!;
    expect(details[1]).not.toMatch(/\bopen\b/);
    expect(details[2]).toContain(selectedCore.id);
    expect(details[2]).toContain(selectedCore.providerFingerprint);
    expect(markup.replace(details[0], '')).not.toContain(selectedCore.id);
    expect(markup.replace(details[0], '')).not.toContain(selectedCore.providerFingerprint);
    expect(details[2]).toContain('不是 Git 提交');
    expect(details[2].match(/<button\b/g)).toHaveLength(1);
    expect(markup.replace(details[0], '')).not.toMatch(/<(?:button|a|select|input)\b/);
    expect(markup).not.toMatch(/<select|<input|role="switch"/);
  });
  it('复制真实控件的白名单快照，排除未来字段及私密内容', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { clipboard: { writeText } });
    const core = { ...selectedCore, subscription: 'https://secret.invalid/token', credential: 'private', path: '/Users/private', diagnostic: 'raw log' };
    await (copyButton(core).props.onClick as () => Promise<void>)();
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(JSON.parse(writeText.mock.calls[0][0])).toEqual({ selectedCore: {
      id: selectedCore.id, providerFingerprint: selectedCore.providerFingerprint, state: 'present',
    } });
    expect(writeText.mock.calls[0][0]).not.toContain(PATCH_SET);
    expect(h.copyState).toBe('copied');
    expect(renderToStaticMarkup(<CoreInfoCard info={FULL} selectedCore={core} />)).toContain('诊断信息已复制');
  });
  it('无效身份与未知状态显示未知，复制不会泄出被塞入身份字段的路径或原日志', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { clipboard: { writeText } });
    const core = { ...selectedCore, id: '/Users/private/key', providerFingerprint: 'raw log\nsecret', state: 'future' } as never;
    const markup = renderToStaticMarkup(<CoreInfoCard info={FULL} selectedCore={core} />);
    expect(cell(markup, 'installed-state')).toBe('未知／未观测');
    expect(markup).not.toContain('/Users/private');
    await (copyButton(core).props.onClick as () => Promise<void>)();
    expect(JSON.parse(writeText.mock.calls[0][0])).toEqual({ selectedCore: { id: null, providerFingerprint: null, state: null } });
  });
  it('剪贴板拒绝或不可用给出失败反馈，且不声称已复制', async () => {
    vi.stubGlobal('navigator', { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('private OS error')) } });
    await (copyButton().props.onClick as () => Promise<void>)();
    expect(h.copyState).toBe('failed');
    const markup = renderToStaticMarkup(<CoreInfoCard info={FULL} selectedCore={selectedCore} />);
    expect(markup).toContain('未能复制诊断信息');
    expect(markup).not.toContain('private OS error');
    vi.stubGlobal('navigator', {});
    await (copyButton().props.onClick as () => Promise<void>)();
    expect(h.copyState).toBe('failed');
  });
  it('更新页保留真实只读数据来源', () => {
    expect(pageSource).toContain('<CoreInfoCard info={coreInfo} selectedCore={selectedCore} onRecovered=');
    expect(pageSource).toContain('coreApi\n      .getVersionInfo()');
    expect(pageSource).toContain("console.error('[SettingsUpdate] core version info unavailable:', error)");
  });
});

it('内容损坏提供实际离线修复入口并保留同版受信应用重装', () => {
  const info = { ...FULL, admission: { state: 'rejected', kind: 'Mismatch', recovery: 'reinstallCurrentApp' } } as CoreVersionInfo;
  const markup=renderToStaticMarkup(<CoreInfoCard info={info}/>);
  expect(markup).toContain('href="#app-update-card"');
  expect(markup).toContain(translate('coreRecovery.reinstall'));
  expect(markup).toContain(translate('coreRecovery.check'));
  expect(markup).not.toContain(translate('coreRecovery.restore'));
  expect(markup).not.toContain('reset');
});
it('未证明的平台能力不把重装当前版本展示成可恢复保证', () => {
  const info = { ...FULL, admission: { state: 'rejected', kind: 'UnsupportedExactObject', recovery: 'reinstallCurrentApp' } } as CoreVersionInfo;
  const markup=renderToStaticMarkup(<CoreInfoCard info={info}/>);
  expect(markup).toContain('当前版本无法在此环境中安全启动内核。');
  expect(markup).not.toContain('选定内容核验失败');
  expect(markup).toContain('href="#app-update-card"');
  expect(markup).toContain(translate('coreRecovery.reinstall'));
  expect(markup).not.toContain(translate('coreRecovery.restore'));
});
it.each(['UnsupportedExactObject', 'UnsupportedCapability', 'LoaderPolicy'] as const)(
  'data recovery inspection is independent of execution rejection %s', kind => {
    const info: CoreVersionInfo = { ...FULL, admission: { state: 'rejected', kind, recovery: 'none' } };
    expect(nodes(CoreInfoCard({ info })).some(node => node.type === CoreRecoveryActions)).toBe(true);
    expect(h.enterSettings).not.toHaveBeenCalled();
  },
);
it('retains the actual data-restored notice after the refreshed admission card changes', () => {
  const markup = renderToStaticMarkup(<CoreInfoCard info={{ ...FULL, admission: { state: 'admitted', recovery: 'none' } }} recoveryComplete />);
  expect(markup).toContain(translate('coreRecovery.restored'));
  expect(markup).not.toContain(translate('coreRecovery.restore'));
});

it('缺少能力使用真实检查更新入口，即使收到错误的重装建议也不承诺同版修复', () => {
  const info: CoreVersionInfo = { ...FULL, admission: { state: 'rejected', kind: 'UnsupportedCapability', recovery: 'reinstallCurrentApp', sourceCommit: 'private-source-sha', policySha256: 'private-policy-sha' } };
  const markup=renderToStaticMarkup(<CoreInfoCard info={info}/>);
  expect(markup).toContain('内核缺少此功能所需的支持，请更新应用。');
  expect(markup).toContain('href="#app-update-card"');
  expect(markup).toContain(translate('settings.about.checkUpdate'));
  expect(markup).not.toContain(translate('settings.update.reinstallCurrent'));
  expect(markup).not.toContain('private-source');expect(markup).not.toContain('private-policy');
});
it.each(['Missing','Mismatch','Changed'] as const)('真实 %s 损坏保留可信重装并先检查离线恢复', kind => {
  const info:CoreVersionInfo={...FULL,admission:{state:'rejected',kind,recovery:'reinstallCurrentApp'}};
  const markup=renderToStaticMarkup(<CoreInfoCard info={info}/>);
  expect(markup).toContain('内核文件校验失败，请通过受信修复或重新安装应用。');
  expect(markup).toContain(translate('coreRecovery.reinstall'));
  expect(markup).toContain(translate('coreRecovery.check'));
});
it.each(['zh-CN','zh-TW','en-US','ru','fa'])('%s 有完整独立的五类人类提示', language => {
  const messages=JSON.parse(readFileSync(fileURLToPath(new URL(`../../../i18n/locales/${language}.json`,import.meta.url)),'utf8')).helper.coreAdmission as Record<string,string>;
  expect(Object.keys(messages).sort()).toEqual(['unsupportedExactObject','unsupportedCapability','damaged','busy','unavailable'].sort());
  for(const value of Object.values(messages)) {expect(value.trim().length).toBeGreaterThan(10);expect(value).not.toMatch(/cohort|sha256|CORE_ADMISSION|wire|\/proc\//i);}
  expect(new Set(Object.values(messages)).size).toBe(5);
});

it('普通环境IO不把权限或资源错误呈现为内容损坏或重装承诺', () => {
  const info: CoreVersionInfo = { ...FULL, admission: { state:'rejected', kind:'Io', recovery:'none' } };
  const markup=renderToStaticMarkup(<CoreInfoCard info={info}/>);
  expect(markup).toContain(translate('helper.coreAdmission.unavailable'));
  expect(markup).not.toContain(translate('helper.coreAdmission.damaged'));
  expect(markup).not.toContain('href="#app-update-card"');
});

it('内容损坏接入普通恢复consumer，不在信息卡自动安装或导航助手', () => {
  const info = { ...FULL, admission: { state: 'rejected', kind: 'Mismatch', recovery: 'reinstallCurrentApp' } } as CoreVersionInfo;
  expect(nodes(CoreInfoCard({ info })).some(node => node.type === CoreRecoveryActions)).toBe(true);
  expect(h.enterSettings).not.toHaveBeenCalled();
});

it.each([undefined, 'Missing', 'Mismatch', 'Changed', 'UnsupportedCapability', 'UnsupportedExactObject'] as const)('source and policy digests have no markup, aria, tooltip or copy exit (%s)', async kind => {
  const sourceTree = 'cafebabecafebabecafebabecafebabecafebabecafe';
  const policy = 'fedcba98'.repeat(8);
  const admission = kind ? { state: 'rejected' as const, kind, recovery: 'reinstallCurrentApp' as const, sourceCommit: sourceTree, sourceTree, policySha256: policy } : undefined;
  const info: CoreVersionInfo = { ...FULL, patchSet: PATCH_SET, admission };
  const markup = renderToStaticMarkup(<CoreInfoCard info={info} selectedCore={selectedCore} />);
  const writeText = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal('navigator', { clipboard: { writeText } });
  const tree = CoreInfoCard({ info, selectedCore });
  const button = nodes(tree).find(n => typeof n.props.onClick === 'function' && nodes(n.props.children).some(child => child.props.children === translate('helper.copyDiagnostics')))!;
  await (button.props.onClick as () => Promise<void>)();
  for (const forbidden of [PATCH_SET, sourceTree, policy]) {
    expect(markup).not.toContain(forbidden);
    expect(writeText.mock.calls[0][0]).not.toContain(forbidden);
  }
  const details = /<details\b[^>]*>[\s\S]*?<\/details>/.exec(markup)![0];
  expect(markup.replace(details, '')).not.toContain(selectedCore.id);
  expect(markup.replace(details, '')).not.toContain(selectedCore.providerFingerprint);
  expect(JSON.parse(writeText.mock.calls[0][0])).toEqual({ selectedCore: { id: selectedCore.id, providerFingerprint: selectedCore.providerFingerprint, state: 'present' } });
});

it.each(['Missing', 'Mismatch', 'Changed'] as const)('typed data damage %s reaches the actual offline recovery consumer', kind => {
  const info: CoreVersionInfo = { ...FULL, admission: { state: 'rejected', kind, recovery: 'reinstallCurrentApp' } };
  const tree = CoreInfoCard({ info });
  expect(nodes(tree).some(node => node.type === CoreRecoveryActions)).toBe(true);
  expect(h.enterSettings).not.toHaveBeenCalled();
});
