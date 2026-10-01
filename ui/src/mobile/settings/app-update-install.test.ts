/**
 * 移动端「下载 → 交系统安装器」那一跳的**纯折算**判据。
 *
 * 三组，各守一件独立的事：
 *  ① 交付结局的判别 —— 判据是 `awaitingSystemInstaller === true`，不是 `ok`；
 *  ② 五个 `REASON_*` 码逐码一句话，且**与 Kotlin 侧那份常量表对得上**（跨语言命名契约）；
 *  ③ 可下载目标的判据有牙 —— 三个资产字段任一缺席就不许出下载按钮。
 *
 * 🔴 ② 的取材面是 **Kotlin 源码原文**，不是本仓另抄的一张表：两处各写一份的下场是
 * 「Kotlin 改了码、前端还在按老码取文」——症状是一句兜底的「没能把安装包交给系统安装器」，
 * 全绿、不报错、用户拿不到「按一下开关就能继续」那句话。
 */
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import {
  ANDROID_INSTALL_REASONS,
  androidInstallFailureKey,
  classifyInstallHandoff,
} from './app-update-install';
import { appUpdateDownloadTarget, reinstallTarget } from './app-update-check';
import zhCN from '@/i18n/locales/zh-CN.json';
import enUS from '@/i18n/locales/en-US.json';
import zhTW from '@/i18n/locales/zh-TW.json';
import ru from '@/i18n/locales/ru.json';
import fa from '@/i18n/locales/fa.json';

const REPO = fileURLToPath(new URL('../../../..', import.meta.url));
const PLUGIN = join(
  REPO,
  'src-tauri/gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt',
);

const LOCALES = { 'zh-CN': zhCN, 'en-US': enUS, 'zh-TW': zhTW, ru, fa } as const;

/** 按 `a.b.c` 取一条文案；取不到返 `undefined`（缺键与空串必须可分辨）。 */
function lookup(bundle: unknown, key: string): string | undefined {
  let node: unknown = bundle;
  for (const seg of key.split('.')) {
    if (typeof node !== 'object' || node === null) return undefined;
    node = (node as Record<string, unknown>)[seg];
  }
  return typeof node === 'string' ? node : undefined;
}

describe('① 交付结局：成功的判据是那个只属于本条腿的键', () => {
  it('`awaitingSystemInstaller: true` = 交出去了', () => {
    expect(classifyInstallHandoff({ ok: true, awaitingSystemInstaller: true })).toEqual({
      kind: 'handed-off',
    });
  });

  it('交不出去时把 Kotlin 的原因码原样带出来（不吞、不翻译、不兜一个「其他」）', () => {
    expect(
      classifyInstallHandoff({
        ok: false,
        awaitingSystemInstaller: false,
        reason: 'unknown-sources-denied',
      }),
    ).toEqual({ kind: 'refused', reason: 'unknown-sources-denied' });
  });

  /**
   * 🔴 **不许按 `ok` 判**。后端在**形态错配**那条失败路径上回的是
   * `{ok:false, handedToSystem, reason:'form-mismatch'}`，而在桌面脚本腿上回的是
   * `{ok:true}` 且**没有** `awaitingSystemInstaller` —— 按 `ok` 判会把后者当成「交出去了」，
   * 于是界面显示一句「已交给系统安装器」，而系统安装器根本没被叫起来。
   */
  it('字段整个缺席 ⇒ 不算成功（即使 `ok:true`）', () => {
    expect(classifyInstallHandoff({ ok: true }).kind).toBe('refused');
    expect(classifyInstallHandoff({ ok: true, handedToSystem: true, reason: 'form-mismatch' })).toEqual(
      { kind: 'refused', reason: 'form-mismatch' },
    );
  });
});

describe('② 五个原因码逐码一句话，且与 Kotlin 侧那份常量表对得上', () => {
  /**
   * 跨语言命名契约：Kotlin 的 `REASON_* = "…"` 字面量集合 ≡ 本侧登记的码集合。
   *
   * 取材自检承重：取不到任何一条就当场红，而不是在一个空集上让下面的相等断言恒真。
   */
  it('Kotlin 的 REASON_* 字面量集合 ≡ 本侧登记的码集合', () => {
    const kotlin = readFileSync(PLUGIN, 'utf8');
    const codes = [...kotlin.matchAll(/const val REASON_[A-Z_]+ = "([a-z-]+)"/g)].map((m) => m[1]);
    expect(codes.length, 'Kotlin 侧一条 REASON_* 都没抓到 —— 取材面塌了，下面那条相等断言会恒真').toBe(
      5,
    );
    expect([...codes].sort()).toEqual([...ANDROID_INSTALL_REASONS].sort());
  });

  it('每个码取到的是自己那一句（五个码五句话，一句都不重复）', () => {
    const keys = ANDROID_INSTALL_REASONS.map((r) => androidInstallFailureKey(r));
    expect(new Set(keys).size, '有两个码折到了同一句话上 —— 那正是这五个常量分开的理由').toBe(
      keys.length,
    );
  });

  it('不认识的码**不编原因**，落到那句只说「没交出去」的兜底上', () => {
    // 空串（后端没给原因）与一个本侧从没见过的码，都走兜底 —— 绝不把机器码贴进界面。
    expect(androidInstallFailureKey('')).toBe('mobileSettings.update.installHandoffFailed');
    expect(androidInstallFailureKey('something-new-from-kotlin')).toBe(
      'mobileSettings.update.installHandoffFailed',
    );
    // 反向对照：真登记过的码**不能**落到兜底上，否则上面两条什么都证明不了。
    expect(androidInstallFailureKey('unknown-sources-denied')).not.toBe(
      'mobileSettings.update.installHandoffFailed',
    );
  });

  it('五个码 + 兜底 + 交付成功那句预告，五个语种一条不缺', () => {
    const keys = [
      ...ANDROID_INSTALL_REASONS.map((r) => androidInstallFailureKey(r)),
      'mobileSettings.update.installHandoffFailed',
      'mobileSettings.update.handedOffNote',
      'mobileSettings.update.downloadingPercent',
      'mobileSettings.update.downloadReady',
      'mobileSettings.update.downloadingAction',
      'mobileSettings.update.installAction',
    ];
    for (const [locale, bundle] of Object.entries(LOCALES)) {
      for (const key of keys) {
        const text = lookup(bundle, key);
        expect(text, `${locale} 缺 ${key}`).toBeTypeOf('string');
        expect((text ?? '').length, `${locale} 的 ${key} 是空串`).toBeGreaterThan(0);
      }
    }
  });
});

describe('③ 可下载目标：三个资产字段任一缺席就不许出下载按钮', () => {
  const full = {
    version: 'v1.2.3',
    downloadUrl: 'https://x/polaris-1.2.3-android-arm64.apk',
    fileName: 'polaris-1.2.3-android-arm64.apk',
    fileSize: 210,
    publishedAt: '2026-09-13T00:00:00Z',
    isPrerelease: false,
    sha256: 'a'.repeat(64),
  };

  it('齐全 ⇒ 原样成为下载目标（摘要一路带到底：下载侧的强校验判据就是它）', () => {
    expect(appUpdateDownloadTarget(full)).toEqual(full);
  });

  /**
   * 🔴 这一条是这一批最要紧的判据：后端在「有新版本但那个 release 没发 APK」那一档
   * **如实回空的三个资产字段**（`check_app_update_release_only`），而 2026-09-13 之前的每一个
   * release 都是这一档。少了它，那一档会得到一颗点下去必然失败的下载按钮。
   */
  it('空资产字段（那一档真实存在）⇒ null，界面退回「打开发布页」', () => {
    expect(
      appUpdateDownloadTarget({
        version: 'v1.2.3',
        downloadUrl: '',
        fileName: '',
        fileSize: 0,
        publishedAt: '2026-09-13T00:00:00Z',
        isPrerelease: false,
      }),
    ).toBeNull();
  });

  it('逐格缺席都判 null（含**整个字段不存在**那一形，不只是空串/0）', () => {
    for (const patch of [
      { downloadUrl: '' },
      { fileName: '' },
      { fileSize: 0 },
      { fileSize: -1 },
    ] as const) {
      expect(appUpdateDownloadTarget({ ...full, ...patch }), JSON.stringify(patch)).toBeNull();
    }
    for (const missing of ['downloadUrl', 'fileName', 'fileSize'] as const) {
      const partial: Record<string, unknown> = { ...full };
      delete partial[missing];
      expect(
        appUpdateDownloadTarget(partial as unknown as typeof full),
        `${missing} 整个缺席时仍被当成可下载 —— 否定式判据挡不住「字段不存在」`,
      ).toBeNull();
    }
    expect(appUpdateDownloadTarget(undefined)).toBeNull();
  });

  it('「重装当前版本」三条早退各有牙，且命中那一档真的给出目标', () => {
    // ① 真有新版本 ⇒ 不替用户去下另一个目标（逐字同桌面的取向）。
    expect(reinstallTarget({ hasUpdate: true, isCurrentVersion: true, updateInfo: full })).toBeNull();
    // ② 通道最新版与已装版本对不上 ⇒ 继续下去就是一次静默降级。
    expect(reinstallTarget({ hasUpdate: false, updateInfo: full })).toBeNull();
    expect(
      reinstallTarget({ hasUpdate: false, isCurrentVersion: false, updateInfo: full }),
    ).toBeNull();
    // ③ 那个 release 没有可下载的资产。
    expect(
      reinstallTarget({
        hasUpdate: false,
        isCurrentVersion: true,
        updateInfo: { ...full, downloadUrl: '' },
      }),
    ).toBeNull();
    // 正面：三条都不命中时给出目标（否则上面三条只是在一个恒 null 的函数上全绿）。
    expect(reinstallTarget({ hasUpdate: false, isCurrentVersion: true, updateInfo: full })).toEqual(
      full,
    );
  });
});
