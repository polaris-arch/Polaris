/**
 * 便携版「手动覆盖」卡的渲染：五种语言下，说明里两个位置都被真的填进去了，按钮也在。
 *
 * 文案经**真的 i18next 插值**（不是手写替换），取的是仓里那五份 locale 文件本身；
 * 只把状态 owner `useAppUpdate` 换成定值 —— 本条看的是呈现层拿到 manual 态后画出了什么。
 * 按钮按下去调什么由 `settings-logic.test.ts` 的接线守卫钉（静态渲染点不了按钮）。
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { createInstance, type i18n as I18n } from 'i18next';
import { renderToStaticMarkup } from 'react-dom/server';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import type { AppUpdateState } from './use-app-update';

const LOCALES = ['zh-CN', 'en-US', 'zh-TW', 'ru', 'fa'] as const;
const ARCHIVE = 'C:\\Users\\u\\AppData\\Local\\Polaris\\updates\\Polaris_1.2.3_x64-win-Portable.zip';
const PROGRAM_DIR = 'D:\\Tools\\Polaris';

const view = vi.hoisted(() => ({
  i18n: null as unknown as { t: (key: string, options?: Record<string, unknown>) => string },
  us: 'manual' as string,
  errMsg: '',
  installing: false,
  manualCompletionUnverified: false,
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string, options?: Record<string, unknown>) => view.i18n.t(key, options) }),
}));
// 卡片下半的通道下拉框用 portal 挂到 `document.body`，node 环境没有 document；它与本条无关，换成空壳。
vi.mock('./Primitives', async (original) => ({
  ...(await original<typeof import('./Primitives')>()),
  Select: () => null,
}));
vi.mock('./use-app-update', () => ({
  useAppUpdate: () => ({
    appVersionInfo: null,
    us: view.us as AppUpdateState,
    updateInfo: { version: 'v1.2.3', isPrerelease: false },
    progress: 0,
    receivedBytes: null,
    errMsg: view.errMsg,
    downloadIntegrity: 'verified',
    installing: view.installing,
    manualCompletionUnverified: view.manualCompletionUnverified,
    checkUpdate: vi.fn(),
    reinstallCurrent: vi.fn(),
    skipVersion: vi.fn(),
    downloadUpdate: vi.fn(),
    installUpdate: vi.fn(),
  }),
}));

function unescapeHtml(markup: string): string {
  return markup
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&amp;/g, '&');
}

describe('AppUpdateCard · 便携版手动覆盖卡', () => {
  const instances = new Map<string, I18n>();
  let AppUpdateCard: typeof import('./AppUpdateCard')['default'];

  beforeAll(async () => {
    for (const lng of LOCALES) {
      const file = fileURLToPath(new URL(`../../../i18n/locales/${lng}.json`, import.meta.url));
      const instance = createInstance();
      await instance.init({
        lng,
        resources: { [lng]: { translation: JSON.parse(readFileSync(file, 'utf8')) } },
        interpolation: { escapeValue: false },
      });
      instances.set(lng, instance);
    }
    ({ default: AppUpdateCard } = await import('./AppUpdateCard'));
  });

  function render(lng: string, us: string, errMsg: string): string {
    view.i18n = instances.get(lng)!;
    view.us = us;
    view.errMsg = errMsg;
    return unescapeHtml(
      renderToStaticMarkup(<AppUpdateCard config={{} as never} update={async () => undefined} />),
    );
  }

  it.each(LOCALES)('%s：说明里填进了压缩包与程序目录两个位置，并画出退出按钮', (lng) => {
    const t = instances.get(lng)!.t;
    const message = t('settings.update.portableManualReplace', { path: ARCHIVE, dir: PROGRAM_DIR });
    const button = t('settings.update.portableQuitAndOpen');
    // 键缺失时 i18next 原样回吐键名 —— 那不是文案。
    expect(message).not.toContain('settings.update.');
    expect(button).not.toContain('settings.update.');

    const html = render(lng, 'manual', message);
    expect(html).toContain('data-us="manual"');
    expect(html).toContain(ARCHIVE);
    expect(html).toContain(PROGRAM_DIR);
    expect(html, '插值没有被填上').not.toContain('{{');
    expect(html).toContain(t('settings.update.downloadedManual'));
    expect(html).toContain(`<span>${button}</span>`);
    // 这是一张「下一步怎么做」的卡，不是失败卡。
    expect(html).not.toContain('data-us="error"');
    expect(html).not.toContain(t('settings.update.failed'));
  });

  it.each(LOCALES)('%s：恢复同版本交接时显示未确认，并明确沿重新检查清除提示', (lng) => {
    const t = instances.get(lng)!.t;
    const message = t('settings.update.portableCompletionUnverified') + '\n' + t('settings.update.portableManualReplace', { path: ARCHIVE, dir: PROGRAM_DIR });
    view.manualCompletionUnverified = true;
    const html = render(lng, 'manual', message);
    view.manualCompletionUnverified = false;
    expect(html).toContain(t('settings.update.portableCompletionUnknown'));
    expect(html).toContain(t('settings.update.portableClearAndCheck'));
    expect(html).not.toContain(t('settings.update.downloadedManual'));
    expect(html).not.toContain('settings.update.portableCompletion');
    expect(html).toContain(ARCHIVE);
  });

  it('安装调用在飞时退出按钮置灰；落定后恢复可点', () => {
    const t = instances.get('zh-CN')!.t;
    const label = `<span>${t('settings.update.portableQuitAndOpen')}</span>`;
    const buttonOf = (html: string) => {
      const end = html.indexOf(label);
      expect(end, '退出按钮没有画出来').toBeGreaterThan(0);
      return html.slice(html.lastIndexOf('<button', end), end);
    };
    view.installing = true;
    const busy = buttonOf(render('zh-CN', 'manual', 'x'));
    view.installing = false;
    const idle = buttonOf(render('zh-CN', 'manual', 'x'));
    expect(busy).toMatch(/\sdisabled(=""|\s|>)/);
    expect(idle).not.toMatch(/\sdisabled(=""|\s|>)/);
  });

  it('对照：error 态不画那颗退出按钮（按钮只属于 manual 卡）', () => {
    const t = instances.get('zh-CN')!.t;
    const html = render('zh-CN', 'error', t('settings.update.formMismatch'));
    expect(html).toContain('data-us="error"');
    expect(html).not.toContain(t('settings.update.portableQuitAndOpen'));
    expect(html).not.toContain('data-us="manual"');
  });
});
