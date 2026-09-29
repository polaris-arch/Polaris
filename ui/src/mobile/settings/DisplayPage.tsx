/**
 * 设置 → 外观（移动端）。桌面对照：`components/screens/settings/SettingsDisplay.tsx`。
 *
 * 桌面三块，移动端只剩第一块 + 通知一行（IA §2.4 / §4.3）：
 *  · 保留：语言、主题。
 *  · **整块缺席**：窗口与通知（关闭主窗行为 / 记忆窗口大小 / 自动轻量模式 / 托盘菜单常驻）与图形
 *    （硬件加速 / 窗口特效）。缺席**不是禁用** —— 禁用读作「现在不可用」并引人等待，而它们描述的是
 *    一个在移动端**不存在的对象**（桌面窗口与托盘）。
 *  · **换载体保留**：桌面通知 → 系统通知（§4.3）。开关真写 `config.desktopNotifications`，
 *    而那份配置**2026-09-06 起真的有消费方了**（见下）。
 *
 * 🔴 **接线落点（2026-09-06，批 1 / W-28）—— 这一行的两句文案随之从条件式改回确定式。**
 *  · 缺口原样：唯一的发送出口 `lib/desktop-notify.ts` 的消费点全在桌面侧
 *    （`App.tsx` / `tray/TrayMenu.tsx` / `components/screens/home/HomeScreen.tsx`），
 *    **一个都不在移动入口的静态 import 图上** ⇒ 这个开关写的配置在 Android 上没有任何消费方。
 *  · 现在补上的是两半，缺任何一半这个开关仍然是死的：
 *      ① **总开关同步** —— `mobile/app-wiring.ts` 的 `useMobileAppWiring` 在 `desktopNotifications`
 *         变化时调 `setDesktopNotificationsEnabled`。少这一半，关掉开关照样发。
 *      ② **发送链** —— 同一份接线里 `api.proxy.onError` → `domain/proxy-error-routing.ts`
 *         的 `handleProxyErrorEvent`（崩溃 / 出口误导 / 规则资源缺失 / 自动换节点落空 … 逐码
 *         `notifyDesktop`），以及 `api.proxy.onTailscaleAuth` 的登录提示。
 *         少这一半，开关开着也没有任何东西会发。
 *  · **权限那条腿本来就是通的**（此前一度被归因成「权限未接线」，是错的）：
 *    `desktop-notify.ts:36-53 ensurePermission()` 首次发送时 invoke
 *    `plugin:notification|is_permission_granted` / `|request_permission`；插件在
 *    `src-tauri/src/lib.rs` **无 cfg 门**注册；ACL 已授到渲染端（`capabilities/default.json`）；
 *    `AndroidManifest.xml` 有 `POST_NOTIFICATIONS`。
 *  · ⚠️ **文案只承诺已经接上的那些**：`notificationsDesc` 现在写的是「代理异常与断开、
 *    Tailscale 登录提示」——**没有**「订阅失败」与「更新提醒」。前者桌面也刻意不发系统通知
 *    （后台更新只入日志，移动端另有分组标签上的常驻失败详情），后者的应用内更新腿本身还没接。
 *    把它们留在文案里就是拿一句正面断言去承诺两件没做的事。
 *
 * 🔴 **note 里保留的那一句仍然为真，且仍然必要**：Android 上装了包、一连接就能看到一条常驻通知
 *  —— `vpn/ServiceNotification.kt:53` 起的前台服务通知，`:118` `service.startForeground(...)`
 *  由 `BoxService.kt:75 notification.show(...)` 在一进 `onStartCommand`（`:66`）时就立起来，
 *  `:123`/`:197` 随状态与速率刷新。它与 `config.desktopNotifications` **毫无关系**
 *  （持有 VpnService 必须常驻一条，见该文件头注）⇒ 用户会把「关掉开关却还有一条通知」读成开关坏了。
 *  故这条 note 从 `warn`（「这项没有做/做不到」）改成 `plain`（中性说明）：它现在陈述的是权限时机
 *  与归属，不再是一条缺口。
 *
 * # 两个控件各自的生效链（都不是「写完就完」）
 *
 *  · **语言**走 `syncLanguageChoice`：它同时写 localStorage 与 `i18n.changeLanguage`，而
 *    `i18n/index.ts` 冷启动读的正是那份 localStorage ⇒ 本次即时生效、下次冷启动也对。
 *    只写 config 不调它 = 改完语言界面不变，重启还是旧语言（移动入口没有桌面 `App.tsx` 那个校正 effect）。
 *  · **主题**写 config 之外还要当场落 `<html data-theme>`：冷启动那一格由主进程的
 *    `theme_boot_script` 播种（`src-tauri/src/lib.rs:314`，它读的就是 `config.uiTheme`，移动端加载的
 *    同样是主窗 ⇒ 这条腿在移动端本来就通），运行期那一格桌面归 `AppShell` 的 effect，而移动入口没有
 *    那个 effect ⇒ 不在这里落，选浅色要重启才看得见。
 */

import type { ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type { UserConfig } from '@/contracts/types';
import { syncLanguageChoice } from '@/i18n';
import { readInitialThemeSeed, resolveTheme } from '@/components/layout/theme-state';
import { MobileSelect, MobileSwitch, SettingsGroup, SettingsNote, SettingsRow } from './SettingsChrome';
import type { MobileSettingsPageProps } from './settings-pages';

/** 系统是否深色。非浏览器环境（判据的 SSR 渲染）回落浅色，与 `AppShell` 同口径。 */
function systemDark(): boolean {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia('(prefers-color-scheme: dark)').matches
    : false;
}

export function DisplayPage({ config, update, commit }: MobileSettingsPageProps): ReactElement {
  const { t } = useTranslation();
  const language = config.language === 'auto' || !config.language ? 'system' : config.language;
  const theme = config.uiTheme ?? 'system';

  function pickTheme(next: UserConfig['uiTheme']): void {
    commit('theme', update({ uiTheme: next }));
    if (typeof document !== 'undefined') {
      document.documentElement.setAttribute(
        'data-theme',
        resolveTheme({ uiTheme: next, seed: readInitialThemeSeed(), systemDark: systemDark() }),
      );
    }
  }

  return (
    <>
      <SettingsGroup header={t('settings.nav.appearance')}>
        <SettingsRow
          first
          stacked
          id="language"
          label={t('settings.appearance.language')}
          desc={t('settings.display.languageDesc')}
          control={
            <MobileSelect
              value={language}
              ariaLabel={t('settings.appearance.language')}
              onChange={(v) => {
                const choice = v === 'system' ? 'auto' : v;
                commit('language', update({ language: choice }));
                syncLanguageChoice(choice);
              }}
            >
              {/* 语言名一律用该语言自己的写法（endonym），与桌面同一条理由：界面正显示着一门你看不懂
                  的语言时，能认出的恰恰是母语那个写法。故这六项不入 locale。 */}
              <option value="system">{t('settings.appearance.system')}</option>
              <option value="zh-CN">简体中文</option>
              <option value="zh-TW">繁體中文</option>
              <option value="en-US">English</option>
              <option value="ru">Русский</option>
              <option value="fa">فارسی</option>
            </MobileSelect>
          }
        />
        <SettingsRow
          stacked
          id="theme"
          label={t('settings.appearance.theme')}
          control={
            <MobileSelect
              value={theme}
              ariaLabel={t('settings.appearance.theme')}
              onChange={(v) => pickTheme(v as UserConfig['uiTheme'])}
            >
              <option value="system">{t('settings.appearance.system')}</option>
              <option value="dark">{t('settings.appearance.dark')}</option>
              <option value="light">{t('settings.appearance.light')}</option>
            </MobileSelect>
          }
        />
      </SettingsGroup>

      <SettingsGroup header={t('mobileSettings.display.notifyBlock')}>
        <SettingsRow
          first
          id="notifications"
          label={t('mobileSettings.display.notifications')}
          desc={t('mobileSettings.display.notificationsDesc')}
          control={
            <MobileSwitch
              checked={config.desktopNotifications !== false}
              ariaLabel={t('mobileSettings.display.notifications')}
              onChange={(v) => commit('notifications', update({ desktopNotifications: v }))}
            />
          }
        />
        <SettingsNote id="notifications-permission" title={t('mobileSettings.display.notifications')} summary={t('mobileHelp.notifications')}>
          {t('mobileSettings.display.notificationsPermission')}
        </SettingsNote>
      </SettingsGroup>
    </>
  );
}
