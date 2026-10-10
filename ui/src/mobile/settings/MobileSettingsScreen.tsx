/**
 * 移动端「设置」屏（F4）。接屏的缝：`ui/src/mobile/Screens.tsx` 里 `SCREENS.settings` 那一行。
 *
 * 内容契约：`~/docs/polaris/design/mobile-kit/component-specs/information-architecture.md` §2.4 +
 * §4 的能力缺席登记；产品裁定：`~/docs/polaris/design/polaris-mobile-ia-adjudications-2026-09-04.md`。
 * 落地说明：`~/docs/polaris/design/polaris-mobile-screen-settings-2026-09-04.md`。
 *
 * # 根页 + 压栈二级页，**不做 split view**（收敛 §2.3 / 裁定 #6）
 *
 * 理由不是「平板没做完」：D-4 已定「底部导航三档都保留」「三档内容完全一致」。split view 会让
 * 「返回」在 compact 下是弹栈、在 expanded 下是换右栏 —— **同一个屏两套导航模型**。
 *
 * # 栈状态住在外壳，不在本屏、也不进 `nav-store`
 *
 * `nav-store` 的 `settingsScreen` 是**桌面侧栏**的选中项，那边九项常驻、没有「栈」这个概念，
 * 故这条路一开始就排除了。此前它是本屏的一个 `useState`，当时的登记条件写得很清楚：
 * 「等第二个消费者出现（底部导航要读该栈）再谈提升」。**第二个消费者已经出现** ——
 * spec「Variants」与 IA §2.4 都要求压栈页上不重复画底部导航，而导航归外壳，屏内的 `useState`
 * 外壳看不见。故本轮把它提到 `MobileShell` 的 `usePushedPage` / `setPushedPage`（按目的地分格），
 * 本屏改为读写那一格。
 * 顺带消掉「切 tab 回来栈位复位」：栈不再随本屏卸载而丢。
 *
 * # Android 系统返回键：**已接**（此前登记在案的那条偏离已经消掉）
 *
 * spec「Interaction」要求响应系统返回，今天由 `MobileApp.tsx` 的三档处理器兑现（弹层/二级页 →
 * 回落点 → 交还系统），本屏的二级页在下面 `useSettingsStack` 里向 `back-stack.ts` 登记一层。
 * 别在本屏再接一个 `back-button` 监听：`Plugin.trigger` 对所有 channel 广播，两个监听会让
 * 一次返回同时退两层。
 * 页头仍然必须 sticky（见 `SettingsChrome` 的 `SettingsHeader`）：返回键不再是唯一的回退路径，
 * 但它是**看不见**的那条 —— 可发现性整个压在页头那颗按钮上，跟着内容滚走就等于没有。
 *
 * # 写失败回显：屏内自解，不往外壳造宿主（裁定 #14）
 *
 * `useConfig` 的写腿失败时做的是「回滚 + `toast.error`」。立项时移动入口没有 toast 宿主，
 * 门面落 console ⇒ 原样照抄的话真机上就是「开关自己弹回去，一句话都没有」。
 * （宿主 2026-09-06 已补上，见 `mobile/MobileToaster.tsx`；本屏仍走行内，理由见 `write-feedback.ts`。）
 *
 * 本屏的解法是**行内**：屏级持一张 `行 id → 失败提示` 表（`write-feedback.ts`），经 context 下发，
 * `SettingsRow` 按自己的 id 自取自渲染 —— 各页不必逐行接，也就没有「忘一处静默一处」的形态。
 * 交给各页的 `update` 是**会抛的那一版**（下面把 `{ throwOnError: true }` 绑死），不抛就没有失败可接。
 * 全局宿主要不要加是四条线合并后的事，本屏一行都不往外壳里加。
 *
 * ⚠️ 同一条根因的另外两个出口**本轮已收**，都是就地锚点、都复用上面那张表：
 *  · `useConfig` 的「需重启 App」确认弹窗走 `dialog-store`，而 `DialogHost` 只挂在桌面 `AppShell` 上
 *    ⇒ 弹窗永不出现。本屏把栈上那条 confirm **截下来**（`deferredNoticeOf`）记进表里，渲染成
 *    页头下面的一行；截下来之后把那个实例关掉并调它的 `onCancel`（= 桌面「稍后」那一支，
 *    也是唯一方向安全的默认：不替用户重启）。
 *  · 关于页四条外链打开失败此前只剩 `console.error` —— 已改成 `commit('<行 id>', …)`，与本屏其它
 *    写腿走同一张表、同一格红字（见 `AboutPage.tsx`）。
 *
 * # 「有新版本」入口行（W-20）：桌面常驻横幅的移动端对位物
 *
 * 桌面把它做成 `components/layout/AppUpdateBanner`，挂在 `AppShell` 的滚动区**之外** ——「常驻」
 * 的字面意思。移动端没有等价的常驻位（外壳的停靠区已经被待应用条与 toast 宿主占着，再塞一条
 * 会把两件更急的东西挤下去），故落成**设置根页最上面一行**：与桌面那条横幅的两颗按钮同义
 * （进更新页 / 关掉它），只是可发现性从「任何一屏都看得见」降到「进设置就看得见」。
 *
 * 判定**整条借用桌面那份纯逻辑**（`app-update-banner.ts` 的 `appUpdateBannerState` +
 * `shouldBannerCheckUpdate`）：四道闸（查到了 / 版本号非空 / 本会话没跳过 / 本会话没关掉）与
 * 「用户关掉了启动检查就不该偷偷发一次请求」这条门控都在那边由单测锁死。移动端重写一遍
 * 必然与它漂移，而漂移的表现是「桌面不提示了，手机还在提示同一个版本」。
 *
 * 检查在**本屏挂载**时发一次而不是在 `MobileApp` 上：这一行只在设置根页可见，把请求提前到每次
 * 冷启动等于为一个多数时候没人看的位置付一次网络往返。
 * 🔴 「每会话一次」那把闸住在 `app-update-check.ts` 的**模块态**（`runAutoAppUpdateCheck`），
 * 不在本屏的 `useRef` 上（2026-09-06 复审 minor）：本屏随底部导航切换整个卸载重挂，
 * 组件里的 ref 跟着复位 ⇒ 那把闸的实际语义会退化成「每次进设置查一次」。
 *
 * # VPN 授权那一行：状态是**读来的**，不是编的
 *
 * 根页那颗状态芯片过去恒显示「待接线」。后端今天已经有了（Kotlin `vpnAuthStatus` → Rust
 * `vpn_auth_status`），故改成读真值；读不到时显示「读不到」而**不是**「未授权」——
 * 把 `unknown` 折成 `denied` 会把用户指去系统设置里授予一个可能已经给过的权限。
 */

import { useCallback, useEffect, useState, useSyncExternalStore, type ReactElement } from 'react';
import { useDismissableLayer } from '../back-stack';
import { useTranslation } from 'react-i18next';
import type { UserConfig } from '@/contracts/types';
import { useDialogStore } from '@/components/dialogs/dialog-store';
import { useConfig } from '@/components/screens/settings/use-config';
import {
  dismissedAppVersions,
  markAppVersionDismissed,
  shouldBannerCheckUpdate,
  skippedAppVersions,
  subscribeAppVersionSkipped,
} from '@/components/layout/app-update-banner';
import { vpnApi, type VpnAuthState } from '@/ipc/api/vpn';
import { setPushedPage, usePushedPage } from '../MobileShell';
import { AboutPage } from './AboutPage';
import { BackupPage } from './BackupPage';
import { DisplayPage } from './DisplayPage';
import { DnsPage } from './DnsPage';
import { GeneralPage } from './GeneralPage';
import { NetworkPage } from './NetworkPage';
import {
  MobileButton,
  SettingsGroup,
  SettingsHeader,
  SettingsPushRow,
  SettingsRow,
  SettingsStatusRow,
} from './SettingsChrome';
import { TunPage } from './TunPage';
import { UpdatePage } from './UpdatePage';
import './settings.css';
import {
  DEFERRED_NOTICE_ROW,
  deferredNoticeOf,
  useWriteFeedback,
  WriteErrorsContext,
  type DeferredNotice,
} from './write-feedback';
import {
  appUpdateEntryVersion,
  checkAppUpdateViaIpc,
  readAppUpdateCheck,
  runAutoAppUpdateCheck,
  subscribeAppUpdateCheck,
} from './app-update-check';
import {
  isSettingsPageId,
  isTitleKey,
  MOBILE_SETTINGS_GROUPS,
  MOBILE_SETTINGS_PAGE_TITLE,
  type MobileSettingsPageId,
  type MobileSettingsPageProps,
} from './settings-pages';

/**
 * 二级页路由表。
 *
 * 写成 `Record<MobileSettingsPageId, …>` 的**字面量**而不是一串 `page === 'x' && <X/>`：
 * 后者少写一支只会让那一页**点进去是空白**，编译器与判据都看不见；字面量形态下少一个键
 * **直接编译不过**（同 `ui/src/mobile/Screens.tsx` 的 `SCREENS`，理由逐字相同）。
 * 编译期不变式不进产物，故判据另在源码层对拍一次键集。
 *
 * `BackupPage` 与 `AboutPage` 都只吃 `commit`（导出/导入与外链的失败都要落进本屏那张表）——
 * 形参更少/更窄的函数对本签名依然可赋值，不必为它们造空壳包装。
 */
const SETTINGS_PAGES: Record<MobileSettingsPageId, (props: MobileSettingsPageProps) => ReactElement> = {
  general: GeneralPage,
  display: DisplayPage,
  network: NetworkPage,
  dns: DnsPage,
  tun: TunPage,
  update: UpdatePage,
  backup: BackupPage,
  about: AboutPage,
};

export function MobileSettingsScreen(): ReactElement {
  const { t } = useTranslation();
  const { page, setPage } = useSettingsStack();
  const { config, loading, error, update, reload } = useConfig();
  const { errors, commit, report } = useWriteFeedback(t);
  const vpnAuth = useVpnAuthState();
  const appUpdate = useAppUpdateEntry(config);
  const notice = useDeferredDialogNotice(report);
  /**
   * 交给各页的写腿：**绑死 `throwOnError`**。
   *
   * `useConfig().update` 缺省吞掉保存失败（只回滚 + 发一条在移动端看不见的 toast），
   * 那正是裁定 #14 要消掉的静默。这里只加选项、不改 patch —— `update({ … })` 的字面形态仍留在各页，
   * `lib/config-write-wiring.test.ts` 的 W-a 照常扫得到那六个文件。
   */
  const strictUpdate = useCallback(
    (patch: Parameters<typeof update>[0]) => update(patch, { throwOnError: true }),
    [update],
  );

  const titleOf = (id: MobileSettingsPageId): string => {
    const title = MOBILE_SETTINGS_PAGE_TITLE[id];
    return isTitleKey(title) ? t(title) : title;
  };

  const header = (
    <>
      <SettingsHeader
        title={page === null ? t('mobileNav.settings') : titleOf(page)}
        backLabel={t('mobileSettings.back')}
        onBack={page === null ? undefined : () => setPage(null)}
      />
      {/* 被截下来的弹窗。跟着页头走（根页与压栈页都在）：触发它的那次配置变更可能在任何一页发生，
          只画在根页 = 用户正停在 DNS 页时照样什么都看不到。文案取弹窗自己的 title/message，
          不另起一句 —— 另起一句就是同一条消息的第二份真值。 */}
      {notice !== null && (
        <SettingsGroup>
          <SettingsRow first id={DEFERRED_NOTICE_ROW} label={notice.label} />
        </SettingsGroup>
      )}
    </>
  );

  /* Provider 罩住**整屏**（含页头下面那条被截弹窗），不只罩压栈页：错误表现在还挂着一条
     不属于任何一页的行（`DEFERRED_NOTICE_ROW`），它在根页/加载态/错误屏上同样要能显示。
     缺省值是空表，故多罩几屏没有行为差别。 */
  const body = ((): ReactElement => {
    /* 加载态是**冷启动回落腿**：`useConfig` 在 app-store 已 hydrate 时首帧就有种子，日常进设置页
       不会出现这一屏。删掉它 = 未 hydrate 的那条路上一片空白。 */
    if (loading) {
      return (
        <SettingsGroup>
          <span style={{ fontSize: '0.875rem', color: 'hsl(var(--fg-dim))' }}>{t('common.loading')}</span>
        </SettingsGroup>
      );
    }

    /* 只有**加载**失败才塌成错误屏：保存失败走行内回显（`useConfig` 已把 `error` 收窄成 load-only），
       否则一次瞬时保存失败会把用户正在编辑的页整个卸载，且文案还说错了原因。 */
    if (error !== null || !config) {
      return (
        <SettingsGroup>
          <span style={{ fontSize: '0.875rem', color: 'hsl(var(--err))' }}>
            {t('common.configLoadFail')}
          </span>
          <div style={{ paddingTop: '12px' }}>
            <MobileButton onClick={() => void reload()}>{t('common.retry')}</MobileButton>
          </div>
        </SettingsGroup>
      );
    }

    if (page !== null) {
      const Page = SETTINGS_PAGES[page];
      return <Page config={config} update={strictUpdate} commit={commit} />;
    }

    return <SettingsRoot onOpen={setPage} vpnAuth={vpnAuth} appUpdate={appUpdate} />;
  })();

  return (
    <WriteErrorsContext.Provider value={errors}>
      {header}
      {/* 列流容器（`./settings.css`）：compact 是 flex 纵列，medium/expanded 起两列 multicol。
          页头不进来 —— 它是 sticky 的，卷进列流会跟着分列。 */}
      <div className="ms-flow">{body}</div>
    </WriteErrorsContext.Provider>
  );
}

/**
 * 本屏的栈 = 外壳那张表里 `settings` 那一格（`MobileShell` 的 `usePushedPage` / `setPushedPage`）。
 *
 * 收窄在读回处做：外壳那格存的是裸 `string`（它不认识、也不该认识本屏的页 id 枚举），
 * 不在表里的值一律当「停在根页」而不是渲染出一页空白。
 *
 * 抽成 hook 是为了让**接线**本身可判：判据把栈写成 `dns` 之后渲染整棵生产树，导航该消失、
 * DNS 页该出现 —— 两件事都是行为断言，不是「源码里出现过 `usePushedPage` 这个词」。
 */
function useSettingsStack(): {
  page: MobileSettingsPageId | null;
  setPage: (next: MobileSettingsPageId | null) => void;
} {
  const raw = usePushedPage('settings');
  const setPage = useCallback((next: MobileSettingsPageId | null) => {
    setPushedPage('settings', next);
  }, []);
  const page = raw !== null && isSettingsPageId(raw) ? raw : null;
  /* 系统返回键在二级页上 = 回根页，与页头那颗返回按钮同一个动作、同一个闭包。
     登记在**栈这一层**而不是页头组件上：返回的语义是「退掉这一层」，与页头画不画得出来无关。
     弹层比它登记得晚（子树后渲染）⇒ LIFO 保证先关弹层、再退页。 */
  useDismissableLayer(page !== null, () => setPage(null));
  return { page, setPage };
}

/**
 * 「有新版本」入口行的接线（W-20）：每会话查一次 + 读会话快照 + 折算成「画不画、画哪个版本」。
 *
 * 三件事都在这一个 hook 里，但**判定一件都不在这里**：门控走 `shouldBannerCheckUpdate`、
 * 四道闸走 `appUpdateEntryVersion`（内部即桌面的 `appUpdateBannerState`），本 hook 只负责
 * 「什么时候查」与「订阅哪几份会话态」。理由见屏头注那一节。
 *
 * 订阅**两份**会话态：检查快照（`subscribeAppUpdateCheck`）与「已跳过/已关掉」集合
 * （`subscribeAppVersionSkipped`，桌面横幅的两颗按钮也写它）。少订后者 ⇒ 用户按「知道了」之后
 * 这一行原样杵着，直到切走再回来才消失。
 */
function useAppUpdateEntry(config: UserConfig | null): string | null {
  const check = useSyncExternalStore(subscribeAppUpdateCheck, readAppUpdateCheck, readAppUpdateCheck);
  const [, bumpSessions] = useState(0);

  useEffect(() => subscribeAppVersionSkipped(() => bumpSessions((n) => n + 1)), []);

  useEffect(() => {
    /* config 未载入前不查（`shouldBannerCheckUpdate(null)` 恒 false）；载入后按 `autoCheckUpdate` 判。
       「本会话只发一次」由 `runAutoAppUpdateCheck` 的**模块态**闸兑现 —— StrictMode 的双跑、
       本屏切走再回来的重挂，两档都被它挡住（组件里的 ref 只挡得住前者，见屏头注那条 🔴）。
       失败**不弹任何东西**：这是一次后台检查，没有用户在等它的回执（手动那次在更新页上，
       走的是 `commit` 那条行内红字）。 */
    if (!shouldBannerCheckUpdate(config)) return;
    void runAutoAppUpdateCheck(checkAppUpdateViaIpc, config).catch(() => undefined);
  }, [config]);

  return appUpdateEntryVersion(check, skippedAppVersions(), dismissedAppVersions());
}

/** 状态芯片的文案键。三态各有一句；IPC 的异常回值在读侧收敛为 unknown。 */
const VPN_AUTH_LABEL: Readonly<Record<VpnAuthState, string>> = {
  authorized: 'mobileSettings.vpnAuth.granted',
  denied: 'mobileSettings.vpnAuth.denied',
  unknown: 'mobileSettings.vpnAuth.unknown',
};

/**
 * [`watchVpnAuth`] 需要的最小事件靶子。生产传 `document`；判据传一个假的。
 *
 * 收窄到这三样（而不是吃整个 `Document`）是为了让判据**驱动得动它** —— 本仓 vitest 跑
 * `environment: 'node'`，没有 jsdom，全局 `document` 压根不存在。
 */
export interface VisibilityTarget {
  readonly visibilityState: string;
  addEventListener(type: 'visibilitychange', listener: () => void): void;
  removeEventListener(type: 'visibilitychange', listener: () => void): void;
}

/**
 * 系统 VPN 授权状态的**重读接线**：挂上读一次 + 每次回到前台再读一次；返回退订闭包。
 *
 * # 为什么不是「挂载读一次」
 *
 * 这个状态在**应用之外**被改：用户去系统设置 → VPN → 撤销本应用，或在起核时的系统弹窗里按下允许。
 * 挂载读一次的话，用户撤销完回来看到的还是「已授权」—— 一个说假话的芯片比一个说「待接线」的更糟。
 *
 * # 为什么不是轮询
 *
 * 那两条路径**都要经过「应用回到前台」**（系统 VPN 设置与系统授权弹窗都是别的 Activity）。
 * 故 `visibilitychange → visible` 就是状态可能变化的**充分覆盖时刻**，定时器只是在没有事件源的
 * 前提下瞎猜，还要付后台唤醒的电量。Android 也没有可订阅的变更事件：`VpnService.onRevoke()` 只在
 * **本应用的隧道被顶掉**时回调，「从未起过核时被撤销」压根不触发它。
 *
 * 本仓已有同型先例：`store/use-system-proxy-live.ts` 的「隐藏即停摆、`visibilitychange` 唤醒补一发」。
 *
 * # 🔴 为什么抽成这个函数，而不是把这段留在 hook 里
 *
 * 留在 hook 里的那一版**没有任何门守得住**：`renderToStaticMarkup` 不跑 effect，node 环境又没有
 * jsdom，于是「把重读整段删掉、只留首次 read」这个变异 `tsc` rc=0、`src/mobile/` 336/336 全绿
 * （协调者实测）。上面那三段论证得再好，也只是注释。抽出来之后判据能拿一个假靶子直接驱动它：
 * 派一次 `visibilitychange` 看有没有第二次读 —— 这是行为断言，不是「源码里有没有出现那个词」。
 * 这也不是为可测而复杂化：hook 因此从二十行缩到三行，接线本身一行没多。
 *
 * 同型先例：`write-feedback.ts` 的 `createCommit` / `deferredNoticeOf`（纯工厂 + 行为对照）。
 */
export function watchVpnAuth(
  read: () => Promise<VpnAuthState>,
  apply: (next: VpnAuthState) => void,
  target: VisibilityTarget,
): () => void {
  let alive = true;
  let revision = 0;
  const pull = (): void => {
    const request = ++revision;
    void read().then(
      (next) => {
        if (alive && request === revision) apply(next === 'authorized' || next === 'denied' ? next : 'unknown');
      },
      // 读不到就是 `unknown`（后端已经把三种桥失败折成它；这里兜的是 IPC 本身不通）。
      // 🔴 绝不回落成 `denied`：那会把用户指去授予一个可能已经给过的权限。
      () => {
        if (alive && request === revision) apply('unknown');
      },
    );
  };
  pull();
  const onVisibility = (): void => {
    if (target.visibilityState === 'visible') pull();
  };
  target.addEventListener('visibilitychange', onVisibility);
  return () => {
    alive = false;
    target.removeEventListener('visibilitychange', onVisibility);
  };
}

/** 屏级接线：把 [`watchVpnAuth`] 挂到真的 `document` 与真的 IPC 上。逻辑全在那边，这里只接线。 */
function useVpnAuthState(): VpnAuthState {
  const [state, setState] = useState<VpnAuthState>('unknown');
  useEffect(() => watchVpnAuth(() => vpnApi.getAuthStatus(), setState, document), []);
  return state;
}

/**
 * 截下 `dialog-store` 里那条移动端无宿主可渲染的 confirm，记进写失败表并关掉它。
 *
 * 关掉是承重的：不关的话，栈上那条会一直在，下一轮 effect 又截一次（同一条消息重复记）。
 * 关的同时调 `onCancel` —— `ConfirmPayload` 的契约是「所有离开路径都要落定」，
 * 而「取消」正是这里唯一方向安全的默认（我们不替用户重启应用）。`promptAppRestart` 没挂
 * `onCancel` ⇒ 对它是 no-op；将来别的 confirm 若把自己包成 `Promise<boolean>`，也不会永不落定。
 */
function useDeferredDialogNotice(report: (rowId: string, text: string) => void): DeferredNotice | null {
  const stack = useDialogStore((s) => s.stack);
  const closeInstance = useDialogStore((s) => s.closeInstance);
  const [notice, setNotice] = useState<DeferredNotice | null>(null);
  useEffect(() => {
    const next = deferredNoticeOf(stack);
    if (next === null) return;
    const entry = stack.find((e) => e.instanceId === next.instanceId);
    setNotice(next);
    report(DEFERRED_NOTICE_ROW, next.text);
    closeInstance(next.instanceId);
    if (entry?.kind === 'confirm') entry.payload.onCancel?.();
  }, [stack, report, closeInstance]);
  return notice;
}

/**
 * 根页那张列表。**单独导出**是为了判据：它不碰 `useConfig`（也就不碰 IPC 与 zustand），
 * 于是可以在 node 环境里直接 `renderToStaticMarkup` 出来逐行对拍。
 * 屏组件那一层只多了「配置加载态」与「压栈状态」两件事，两者都不是本列表的内容契约。
 */
export function SettingsRoot({
  onOpen,
  vpnAuth,
  appUpdate,
}: {
  onOpen: (id: MobileSettingsPageId) => void;
  /** 系统 VPN 授权状态。**由屏组件读来**（`useVpnAuthState`），本列表保持无 IPC 依赖。 */
  vpnAuth: VpnAuthState;
  /**
   * 有新版本时的版本号，`null` = 不画那一行（W-20）。同样**由屏组件折算好**再传进来
   * （`useAppUpdateEntry`）—— 本列表不碰 IPC、不碰会话态，判据才能把它单独渲染出来逐行对拍。
   */
  appUpdate: string | null;
}): ReactElement {
  const { t } = useTranslation();
  const titleOf = (id: MobileSettingsPageId): string => {
    const title = MOBILE_SETTINGS_PAGE_TITLE[id];
    return isTitleKey(title) ? t(title) : title;
  };
  return (
    <>
      {/* 「有新版本」入口（W-20）：桌面常驻横幅的对位物，故摆在**全部分组之上**。
          它是一行「进更新页」的入口，不是第九个二级页 —— `data-push` 用的是自己的 id，
          与 `MOBILE_SETTINGS_PAGES` 那八项不同名（判据按那八项逐值对拍根页的进入行）。 */}
      {appUpdate !== null && (
        <SettingsGroup>
          <SettingsPushRow
            first
            id="app-update-available"
            label={t('settings.update.bannerTitle', { version: appUpdate })}
            desc={t('mobileSettings.update.entryDesc')}
            onOpen={() => onOpen('update')}
          />
          {/* 「忽略」= 桌面横幅那颗关闭按钮（同一条键 `settings.update.bannerDismiss`）：
              **本会话**闭嘴，不写后端（跨会话仍提示）。与桌面相邻的「跳过此版本」语义强度差一个
              数量级 —— 那一颗是持久的，移动端只给这一颗弱的：强的那颗要在能读到版本说明的地方
              才说得清，而这里只是一行入口。 */}
          <div style={{ display: 'flex', justifyContent: 'flex-end', paddingTop: '8px' }}>
            <MobileButton onClick={() => markAppVersionDismissed(appUpdate)}>
              {t('settings.update.bannerDismiss')}
            </MobileButton>
          </div>
        </SettingsGroup>
      )}
      {MOBILE_SETTINGS_GROUPS.map((group) => (
        <SettingsGroup key={group.id} header={group.headerKey === null ? undefined : t(group.headerKey)}>
          {group.pages.map((id, index) => (
            <SettingsPushRow
              key={id}
              id={id}
              first={index === 0}
              label={titleOf(id)}
              onOpen={() => onOpen(id)}
            />
          ))}
          {/* VPN 授权：桌面「提权助手」那一行的移动端**替代**，不是它的移植（IA §4.4）——
              同一个用户问题（我给过权限了吗），完全不同的对象（系统 VPN 服务）与动作（系统弹窗）。
              状态是**读来的**（Kotlin `vpnAuthStatus` → Rust `vpn_auth_status`）：`unknown` 显示
              「读不到」而不是「未授权」—— 两者不是同义词，折过去就是编一个事实。 */}
          {group.statusRows?.map((rowId) => (
            <SettingsStatusRow
              key={rowId}
              id={rowId}
              label={t('mobileSettings.vpnAuth.title')}
              desc={t('mobileSettings.vpnAuth.desc')}
              status={t(VPN_AUTH_LABEL[vpnAuth])}
              tone={vpnAuth === 'authorized' ? 'ok' : vpnAuth === 'denied' ? 'warn' : 'neutral'}
            />
          ))}
        </SettingsGroup>
      ))}
    </>
  );
}
