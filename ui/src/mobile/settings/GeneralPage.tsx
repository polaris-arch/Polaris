/**
 * 设置 → 通用（移动端）。桌面对照：`components/screens/settings/SettingsGeneral.tsx`。
 *
 * # 两块，与桌面同序（IA §2.4）
 *
 *  1. 启动：**启动即连接** + **启动即检查更新** 原样保留；**静默启动**去掉（没有窗口可以隐藏地启动）；
 *     **开机自启**（`autoStart`）换了对象：Android 上是「开机自动连接」开关，另有系统的
 *     「始终开启的 VPN」跳转项与之并列，见下。
 *
 * # 开机自动连接：应用自己的开关（`autoStart`，2026-09-25）
 *
 * 与桌面同一条写腿：`autoStartApi.set`（执行侧真值：桌面是 OS launch agent，Android 是 Kotlin 侧
 * 标记文件，开机时由 `BootReceiver` 读 —— 那一刻 Rust 不在）+ `update({ autoStart })`（配置落盘）。
 * 语义是「开机后，若上次没有主动断开、已授权 VPN、有可用配置，就连上」；它**不**做掉线重连、
 * **不**拦截未走 VPN 的流量 —— 那两件是下面系统「始终开启的 VPN」的事，两行的说明各自写清区别。
 *
 * 🔴 **显示值读执行侧真值，不读 `config.autoStart`**（2026-09-25，同备份页「系统备份」开关的口径）。
 * user config 会随系统备份走：备份开关打开后换机 / 恢复，副本里的 `autoStart` 说「开」，而本机
 * Kotlin 那份标记文件（`noBackupFilesDir`，不随备份走）说「关」—— 界面与开机时的实际行为分叉。
 * 故开关挂载时经 `autoStartApi.getStatus()`（Rust `auto_start_get_status` → 桥 → Kotlin）读真值；
 * 没读到时**禁用**，读失败在这一行落红字（「关着」与「不知道」是两句话）。`config.autoStart`
 * 只作写入镜像（与桌面同一个字段，四屏对差门按这条写腿算「已接」）。
 *  2. 隐私与安全：**自动隐私锁 + 锁屏密码 + 关闭日志写盘**。前两行 2026-09-06（W-16）落地，
 *     此前是一句缺席说明，理由见下。
 *
 * # 始终开启的 VPN：跳转项，**不是开关**（IA 裁定 #8）
 *
 * Android 上「始终开启的 VPN」是**系统设置**里的一格，应用无权代为打开。做成一个看起来像开关的
 * 东西就是骗：用户拨了它，回来发现没生效，且没有任何反馈。裁定给的形态是「去系统设置开启 ›」的
 * 跳转项，而跳转要一条原生腿 —— **那条腿 2026-09-06（W-17）接上了**
 * （`system-settings-bridge.ts` ⇄ `MainActivity.installVpnSettingsBridge`）。
 *
 * 三件事必须一起看，少一件这一行就会重新变成骗人的东西：
 *  · **桥不在就不画成可点的**（桌面调试档 / 浏览器直开 / WebView < 88）。一个点了必然失败的跳转项
 *    比一行说明更坏，故 `hasVpnSettingsBridge()` 为假时退化成不可点的说明行。
 *  · **常驻路径那一行留着**（`bootConnectPath`）。跳转的落点是系统的 **VPN 列表页** ——
 *    `android.provider.Settings` 里没有任何 always-on VPN 的 action 常量（实测 android-34 的
 *    `android.jar` 常量池），列表页就是公开面的最深处。剩下「Polaris → 始终开启的 VPN」两跳
 *    只能由那行路径承担，它不是跳转没做完的补丁。
 *  · **跳不过去要看得见**。原生侧两个分支各发一条回执，失败经本屏那条 `commit` 落成行内红字
 *    （裁定 #14：点了没反应 = 静默失败）。
 * 判据钉的仍是「它不是 switch」这条不变量（`MobileSettings.test.tsx`），外加桥在/不在两档的形态。
 *
 * # 隐私锁整组：从「如实标注的缺席」变成「真的在跑」（W-16）
 *
 * 本页此前有一句常驻说明：闲置计时与锁屏遮罩两条腿都不在移动入口上，故整组不画 ——
 * 拨开一个锁不住任何东西的开关，是比缺席更坏的安全承诺。三条腿现在都有了：
 *  · 闲置计时 → `app-wiring.ts` 的 `armIdlePrivacyLock`（挂在 `useMobileAppWiring`）；
 *  · 锁屏遮罩 → `MobileLockOverlay.tsx`（挂在 `MobileApp`，外壳的兄弟节点）；
 *  · 后端进隐私态的触发点 → 那条闲置计时调的 `config_set_privacy_mode(true)`
 *    （桌面那侧是托盘菜单，移动端没有托盘 —— 这不是缺一条腿，是同一件事的另一个触发点）。
 *
 * # 🔴 本页**只碰密码**，不碰隐私态的翻转
 *
 * 锁屏密码走 `privacyApi.setPassword`（后端在锁屏中会以 `PRIVACY_LOCKED` 拒绝，`config.rs:1371`）。
 * **进/出隐私态的唯一收敛点在别处**：闲置计时与解锁腿各自调 `config_set_privacy_mode`，
 * 由后端 emit 收敛进 store。本页结构上不含任何 `setPrivacyMode` —— 判据有一条负向断言钉着它
 * （裁定 #10：直接翻标志位会绕过 `privacy_unlock` 的整条校验链）。
 *
 * 密码行**仅在隐私锁开启时显示**（契约 L111，同桌面 `SettingsGeneral.tsx:146`）：
 * 锁关着时密码不参与任何判定，恒显只会让人以为「设了密码就有保护」。
 */

import { useEffect, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { autoCheckUpdateChecked } from '@/components/screens/settings/settings-logic';
import { autoStartApi, privacyApi } from '@/ipc/api-client';
import {
  MobileButton,
  MobileSwitch,
  MobileTextInput,
  SettingsGroup,
  SettingsNote,
  SettingsPushRow,
  SettingsRow,
} from './SettingsChrome';
import { hasVpnSettingsBridge, openSystemVpnSettings } from './system-settings-bridge';
import { failureText } from './write-feedback';
import type { MobileSettingsPageProps } from './settings-pages';

/*
 * ⚠️ 行 id 一律**写成字面量**（不抽常量）。不是风格问题：⑩ 组那条「每一处写腿都挂在 `commit(` 上、
 * 且登记表与源码逐值对差」的判据是按 `commit('<id>'` 这个**字面**形态扫的，抽成常量会让这两条
 * 写腿整条离开它的取材面 —— 于是「忘了接回显」这件事在这两行上没有任何门看得见。
 * 同一条口径下本屏其余六页也全是字面量。
 */

export function GeneralPage({ config, update, commit }: MobileSettingsPageProps): ReactElement {
  const { t } = useTranslation();
  const autoPrivacyMode = !!config.autoPrivacyMode;
  /* 「跳转项」还是「说明行」由桥定，且**在渲染期读一次**：桥由 `MainActivity` 在 document-start
     注入，本组件挂载时它该在就一定已经在（同 `MobileApp.hasBackNavBridge` 的那条论证）。 */
  const canJump = hasVpnSettingsBridge();
  /** 开机自动连接的执行侧真值。`null` = 还没读到（或读失败）。 */
  const [autoStart, setAutoStart] = useState<boolean | null>(null);
  useEffect(() => {
    commit('auto-start', autoStartApi.getStatus().then(setAutoStart), (err) =>
      failureText(t, err, {
        withReason: 'mobileSettings.general.autoStartReadFailedWithReason',
        plain: 'mobileSettings.general.autoStartReadFailed',
      }),
    );
  }, [commit, t]);

  return (
    <>
      <SettingsGroup header={t('settings.general.groupStartup')}>
        <SettingsRow
          first
          id="auto-connect"
          label={t('settings.general.autoConnect')}
          desc={t('settings.general.autoConnectDesc')}
          control={
            <MobileSwitch
              checked={!!config.autoConnect}
              ariaLabel={t('settings.general.autoConnect')}
              onChange={(v) => commit('auto-connect', update({ autoConnect: v }))}
            />
          }
        />
        <SettingsRow
          id="auto-check-update"
          label={t('settings.general.autoCheckUpdate')}
          desc={t('settings.general.autoCheckUpdateDesc')}
          control={
            <MobileSwitch
              checked={autoCheckUpdateChecked(config)}
              ariaLabel={t('settings.general.autoCheckUpdate')}
              onChange={(v) => commit('auto-check-update', update({ autoCheckUpdate: v }))}
            />
          }
        />
        {/* 应用自己的开机自动连接（`autoStart` 的 Android 腿）。显示值 = 执行侧真值（见头注）；
            先写执行侧、再落配置镜像：执行侧失败时两者都不动。无论写成败，结束后**重读一次执行侧**
            作显示值 —— 镜像落盘失败时开机行为已经变了，显示必须跟执行侧走，不跟这次写的结局走。
            重读失败 ⇒ 回到「不知道」（禁用）；这次写自己的成败照常由 commit 落在这一行。 */}
        <SettingsRow
          id="auto-start"
          label={t('mobileSettings.general.autoStartTitle')}
          desc={t('mobileSettings.general.autoStartDesc')}
          control={
            <MobileSwitch
              checked={autoStart === true}
              disabled={autoStart === null}
              ariaLabel={t('mobileSettings.general.autoStartTitle')}
              onChange={(v) =>
                commit(
                  'auto-start',
                  autoStartApi.set(v).then(() => update({ autoStart: v })).finally(() =>
                    autoStartApi.getStatus().then(setAutoStart, () => setAutoStart(null)),
                  ),
                )
              }
            />
          }
        />
        {/* 裁定 #8：跳转项形态，绝不是 switch。桥不在时退化成不可点的说明行，见头注。 */}
        {canJump ? (
          <SettingsPushRow
            id="boot-auto-connect"
            label={t('mobileSettings.general.bootConnectTitle')}
            desc={t('mobileSettings.general.bootConnectDesc')}
            onOpen={() => {
              commit('boot-auto-connect', openSystemVpnSettings(), (err) =>
                failureText(t, err, {
                  withReason: 'mobileSettings.saveFailedWithReason',
                  plain: 'mobileSettings.general.bootConnectOpenFailed',
                }),
              );
            }}
          />
        ) : (
          <SettingsRow
            id="boot-auto-connect"
            label={t('mobileSettings.general.bootConnectTitle')}
            desc={t('mobileSettings.general.bootConnectDesc')}
          />
        )}
        {/* 常驻路径：跳转的落点只能到 VPN 列表页，剩下两跳由这一行承担（见头注）。 */}
        <SettingsNote id="boot-auto-connect-path">
          {t('mobileSettings.general.bootConnectPath')}
        </SettingsNote>
      </SettingsGroup>

      <SettingsGroup header={t('settings.general.groupPrivacy')}>
        <SettingsRow
          first
          id="auto-privacy-mode"
          label={t('settings.general.autoPrivacyMode')}
          desc={t('settings.general.autoPrivacyModeDesc')}
          control={
            <MobileSwitch
              checked={autoPrivacyMode}
              ariaLabel={t('settings.general.autoPrivacyMode')}
              onChange={(v) => commit('auto-privacy-mode', update({ autoPrivacyMode: v }))}
            />
          }
        />
        {/* 契约 L111：仅隐私锁开启时显示（同桌面）。锁关着时密码不参与任何判定。 */}
        {autoPrivacyMode && <PrivacyPasswordRow commit={commit} />}
        <SettingsRow
          id="disable-log-file"
          label={t('settings.advanced.disableLogFile')}
          desc={t('mobileHelp.disableLogs')}
          descDetails={t('settings.advanced.disableLogFileDescFull')}
          control={
            <MobileSwitch
              checked={!!config.disableLogFile}
              ariaLabel={t('settings.advanced.disableLogFile')}
              onChange={(v) => commit('disable-log-file', update({ disableLogFile: v }))}
            />
          }
        />
      </SettingsGroup>
    </>
  );
}

/**
 * 锁屏密码行：收起态显状态文案 + 「设置/修改密码」按钮，展开态显密码框 + 保存/取消。
 *
 * 与桌面同一套语义（`SettingsGeneral.tsx:146-190`）：**留空 = 清除密码**，对齐后端
 * `set_password_core` 对空串的处理（删哈希文件 + 抹 legacy 键）。
 *
 * 写腿走本屏那条 `commit`（不是自己 catch 落 console）：后端在锁屏中会以 `PRIVACY_LOCKED` 拒绝，
 * 那句拒绝必须落到这一行下面的红字上 —— 一个「点了保存、什么都没发生」的密码框，用户会以为设好了。
 * 抽成独立组件是因为它自带三个 state，塞进 `GeneralPage` 会让那三个 state 在密码行不显示时
 * 仍然活着（关掉隐私锁再打开，上一次没保存的草稿还在框里）。
 */
function PrivacyPasswordRow({ commit }: { commit: MobileSettingsPageProps['commit'] }): ReactElement {
  const { t } = useTranslation();
  const [hasPassword, setHasPassword] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');

  useEffect(() => {
    void privacyApi
      .hasPassword()
      .then(setHasPassword)
      .catch(() => undefined);
  }, []);

  function save(): void {
    /* `setPassword` 返 `{success}`，而 `commit` 吃的是 `Promise<void>` —— 这一跳把「后端说没成」
       也折成 reject，否则 `success:false` 会被当成保存成功（红字不出，草稿被清掉）。 */
    commit(
      'privacy-password',
      privacyApi.setPassword(draft).then((r) => {
        if (!r.success) throw new Error(t('common.saveFailed'));
        setHasPassword(draft.length > 0);
        setEditing(false);
        setDraft('');
      }),
    );
  }

  return (
    <SettingsRow
      stacked={editing}
      id="privacy-password"
      label={t('settings.general.privacyPassword')}
      desc={
        hasPassword
          ? t('settings.general.privacyPasswordSet')
          : t('settings.general.privacyPasswordUnset')
      }
      control={
        editing ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', width: '100%' }}>
            <MobileTextInput
              type="password"
              value={draft}
              ariaLabel={t('settings.general.privacyPassword')}
              placeholder={t('settings.general.privacyPasswordPlaceholder')}
              onChange={setDraft}
              onCommit={() => undefined}
            />
            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
              <MobileButton tone="primary" onClick={save}>
                {t('common.save')}
              </MobileButton>
              <MobileButton
                onClick={() => {
                  setEditing(false);
                  setDraft('');
                }}
              >
                {t('common.cancel')}
              </MobileButton>
            </div>
          </div>
        ) : (
          <MobileButton
            onClick={() => {
              setDraft('');
              setEditing(true);
            }}
          >
            {hasPassword
              ? t('settings.general.privacyPasswordChange')
              : t('settings.general.privacyPasswordSetBtn')}
          </MobileButton>
        )
      }
    />
  );
}
