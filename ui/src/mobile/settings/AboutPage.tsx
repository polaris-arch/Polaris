/**
 * 设置 → 关于（移动端）。桌面对照：`components/screens/settings/SettingsAbout.tsx`。
 *
 * # 🔴 许可文案必须按平台给对
 *
 * 桌面那句写的是「LICENSE（MIT，Polaris 本体）」。对**源码**它仍然准确，但 **Android 产物整体按
 * GPLv3 分发** —— 内核在 Android 上不是独立 sidecar 子进程，而是以 libbox 形态载入应用进程
 * （`PlatformInterface.OpenTun` 是拿 tun 描述符的唯一通道，`TunInboundOptions` 没有接收已开 fd 的
 * 字段），mere-aggregation 在这里不成立；且本仓 Android 侧的 Kotlin 实现移植自 GPLv3 的
 * sing-box-for-android。口径的真值源是**仓根 `NOTICE` 的「按平台的许可形态」一节**，本页照它写。
 *
 * 照抄桌面那一句 = 在产物里说了一句假话（对拿到 APK 的人，他拿到的是 GPLv3）。故这里拆成两行：
 * **源码许可**（MIT）与**本产物许可**（GPLv3），两件事本来就不是一件事。
 * `MobileSettings.test.tsx` 对这两行有一条与 `NOTICE` 对账的判据：产物那行必须含 GPLv3、
 * 且**不得**含 MIT。
 *
 * # 完全卸载：一句话，不是一个动作（IA §4.9）
 *
 * 桌面的危险区会去删助手、用户数据与内核，并逐项报结果（含「本平台做不到」）。移动端没有这个动作 ——
 * 卸载经系统的应用管理进行。但**不能就这么静默丢掉**：丢掉会留下「我的数据到底删没删」这个没人回答
 * 的问题，而那正是能力缺席登记要防的那类缺陷。故换成一句常驻说明。
 */

import { useEffect, useState, type ReactElement, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { AppVersionInfo } from '@/contracts/types/update';
import { systemApi, versionApi } from '@/ipc/api-client';
import { SettingsGroup, SettingsNote, SettingsRow } from './SettingsChrome';
import { failureText, type CommitWrite } from './write-feedback';

/** 仓库根的 LICENSE（MIT，Polaris 本体源码）与 NOTICE（第三方组件 + 按平台的分发条款）。 */
const LICENSE_URL = 'https://github.com/polaris-arch/Polaris/blob/main/LICENSE';
const NOTICE_URL = 'https://github.com/polaris-arch/Polaris/blob/main/NOTICE';
const RELEASES_URL = 'https://github.com/polaris-arch/Polaris/releases';
const ISSUES_URL = 'https://github.com/polaris-arch/Polaris/issues';

/**
 * 外链只发一次 IPC，不用 `<a href>`：两者都有导航权，WebView 上一次点击会开两个标签。
 *
 * # 打开失败必须看得见（裁定 #14）
 *
 * 这条腿此前是 `catch` → `console.error` —— 也就是裁定 #14 原文点名的那个形态：有 catch，
 * 而 catch 里对用户什么都没做。真机上表现为「点了 LICENSE，什么都没发生」，用户会一直点。
 * `systemApi.openExternal` 在 Android 上要经 Intent 找一个能开 http(s) 的 Activity，**没有浏览器
 * 时它会真的失败**，不是理论风险。
 *
 * 修法是复用本屏那一套（`write-feedback.ts` 的 `commit`）而不是另造：错误落进同一张表、渲染在
 * 同一格红字里。取文换成外链那两句 —— 这条腿一个字节的配置都不写，套「保存失败」的壳会让人去查配置。
 */
function ExternalRow({
  id,
  url,
  label,
  commit,
  first,
}: {
  id: string;
  url: string;
  label: ReactNode;
  commit: CommitWrite;
  first?: boolean;
}): ReactElement {
  const { t } = useTranslation();
  return (
    <SettingsRow
      id={id}
      first={first}
      label={
        <button
          type="button"
          data-external={id}
          onClick={() => {
            commit(id, systemApi.openExternal(url), (err) =>
              failureText(t, err, {
                withReason: 'mobileSettings.about.openLinkFailedWithReason',
                plain: 'mobileSettings.about.openLinkFailed',
              }),
            );
          }}
          style={{
            margin: 0,
            padding: 0,
            border: 0,
            background: 'none',
            color: 'hsl(var(--flow))',
            font: 'inherit',
            textAlign: 'start',
            cursor: 'pointer',
            touchAction: 'manipulation',
          }}
        >
          {label}
        </button>
      }
    />
  );
}

/**
 * `version_get_info` 的载荷 → `AppVersionInfo` 的**运行时**窄化守卫。
 *
 * # 为什么不是 `as unknown as AppVersionInfo`
 *
 * `version_get_info` 的真实载荷是 `{ appVersion, coreVersion, coreBaseline }`
 * （`src-tauri/src/commands/updater/app_update.rs:428-436`，权威声明 `contracts/types/update.ts:96`）。
 * 而 `ipc/api/updater.ts` 的 `VersionInfo` 还是 Electron 遗留形状（`singBoxVersion` / `appName` /
 * `buildDate` … 后端一个都不返回）—— 桌面 `SettingsAbout.tsx:171` 读 `info.singBoxVersion` 因此是条
 * 恒假分支。改那份类型会连带动桌面，属别的线的面。
 *
 * 但**双重断言不是收窄，是把编译期保护整个关掉**：`as unknown as` 之后两侧的类型关系彻底切断，
 * 后端把 `coreVersion` 改名（或 `read_core_version` 改返 `Option` 而序列化成 `null`）时 `tsc` 一个字
 * 都不会说，关于页静默退回 `—`，而那正是 K5-07 原本要修的症状。故改成**读入处逐字段检查**：
 * 形状不符就整段保持 `null`（没有版本号，好过给一个编出来的），而检查本身在运行时真的执行。
 *
 * ⚠️ 面外的两条姊妹腿仍未处理，登记在此免得它们无人认领：
 *  ① `ipc/api/updater.ts` 的 `VersionInfo` 与 `contracts/types/update.ts` 的 `AppVersionInfo` 不一致；
 *  ② 桌面 `SettingsAbout.tsx:171` 的 `info.singBoxVersion` 是恒假分支。
 */
export function narrowVersionInfo(payload: unknown): AppVersionInfo | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const p = payload as Record<string, unknown>;
  if (typeof p.appVersion !== 'string' || typeof p.coreVersion !== 'string') return null;
  return {
    appVersion: p.appVersion,
    coreVersion: p.coreVersion,
    // 基线只进「关于」页的展示位，缺了不该把整段版本号一起打掉。
    coreBaseline: typeof p.coreBaseline === 'string' ? p.coreBaseline : '',
  };
}

export function AboutPage({ commit }: { commit: CommitWrite }): ReactElement {
  const { t } = useTranslation();
  const [info, setInfo] = useState<AppVersionInfo | null>(null);

  useEffect(() => {
    void versionApi
      .getInfo()
      .then((payload) => setInfo(narrowVersionInfo(payload)))
      .catch(() => undefined);
  }, []);

  return (
    <>
      <SettingsGroup>
        <SettingsRow
          first
          id="about-version"
          label="Polaris"
          desc={t('settings.about.tagline')}
          control={
            /* 两个版本串都是 IA §2.4「Data positions」登记的数据位，桌面同一行给的也是这两个
               （`SettingsAbout.tsx:168-171` 的 `v{app} · sing-box {core}`）。内核版本读不到就整段不渲染
               —— 报障时没有版本号，好过给一个编出来的。 */
            <span style={{ fontSize: '0.75rem', color: 'hsl(var(--fg-dim))', fontFamily: 'var(--mono)' }}>
              {info?.appVersion ? `v${info.appVersion}` : '—'}
              {info?.coreVersion ? ` · sing-box ${info.coreVersion}` : ''}
            </span>
          }
        />
        <SettingsNote id="about-intro">{t('settings.about.intro')}</SettingsNote>
        <SettingsNote id="about-telemetry">{t('settings.about.noTelemetry')}</SettingsNote>
      </SettingsGroup>

      <SettingsGroup header={t('mobileSettings.about.licenseBlock')}>
        <SettingsRow
          first
          id="license-source"
          label={t('mobileSettings.about.licenseSource')}
          desc={t('mobileSettings.about.licenseSourceValue')}
        />
        <SettingsRow
          id="license-artifact"
          label={t('mobileSettings.about.licenseArtifact')}
          desc={t('mobileSettings.about.licenseArtifactValue')}
        />
        {/* 链接标签用**文件名**而不是桌面那句「开源许可 · MIT」：后者紧挨着上面「本产物 GPLv3」
            那一行时会读成互相打架，而它其实只是「链到哪个文件」。文件名跨语种同形，不进 locale。 */}
        <ExternalRow id="license" url={LICENSE_URL} label="LICENSE" commit={commit} />
        <ExternalRow id="notice" url={NOTICE_URL} label={t('settings.about.notice')} commit={commit} />
      </SettingsGroup>

      <SettingsGroup>
        <ExternalRow first id="releases" url={RELEASES_URL} label={t('settings.about.releases')} commit={commit} />
        <ExternalRow id="issues" url={ISSUES_URL} label={t('settings.about.reportIssue')} commit={commit} />
        <SettingsNote id="about-uninstall">{t('mobileSettings.about.uninstallNote')}</SettingsNote>
      </SettingsGroup>
    </>
  );
}
