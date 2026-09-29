/**
 * 移动端根组件：路由（五个目的地之间切换）+ 外壳装配 + 系统返回键接线。
 *
 * # 为什么没有路由库
 *
 * 需要的全部语义是"五选一 + 记住当前项"。仓里已有的 zustand 也没上：目前**只有外壳一个消费者**，
 * 一个 `useState` 就是全部真相；把它提到 store 里只会多一份要同步的状态而不多任何能力。
 * 子路由（节点 → 订阅、设置 → 各二级页）落地时，这里换成"每个目的地一个栈"的形态，
 * 届时再判要不要 store —— 那时它才有第二个消费者（底部导航要读"该栈能不能回顶"）。
 *
 * 路由表 = `SCREENS`，目的地表 = `DESTINATIONS`，两者共用 `DestinationId`；
 * 本文件**不含**任何目的地字面量（除冷启动落点常量本身），门为此断言。
 *
 * # 它同时是移动端的「应用级接线点」（对应桌面 `App.tsx`）
 *
 * app-store 那份配置副本的水合腿挂在这里，理由见 `config-sync.ts` 头注：外壳
 * （`MobileShell`）只管安全区/滚动/断点/导航停靠，不认识 store 也不认识 `api`；
 * `MobileMain` 是渲染入口不是组件，挂不住需要卸载清理的订阅。本文件是这棵树上
 * **唯一**既是组件、又在五个屏之上的位置。系统返回键那条腿同理挂在这里。
 *
 * 同一条理由下，2026-09-06 又挂上两样：
 *  · [`useMobileAppWiring`]（`app-wiring.ts`）—— 连接态事件订阅与首帧水合、Tailscale / OpenVPN /
 *    订阅进度 / 待应用差集 / 代理错误。**逐条判定「桌面哪些该接、哪些是桌面专属」写在那份头注的表里**；
 *  · 停靠区那两件 chrome（`MobilePendingBar` / `MobileToaster`）—— 它们要 store 与 `api`，
 *    外壳不认识那两样，故在这里装配、传给外壳摆位。
 *
 * 2026-09-06 再挂一样：[`MobileLockOverlay`]（W-16）。它是外壳的**兄弟**而不是子节点 ——
 * 外壳内部任何一层的 `z-index` 都在它自己的层叠上下文里，压不过一个 fixed 的兄弟；
 * 桌面的对位物同样挂在 chrome 容器（`AppShell.tsx:247`）而不是某一屏里。
 *
 * # 系统返回键（Android）：三档，逐档都必须自己兑现
 *
 * ```
 * 系统返回 → OnBackPressedDispatcher → tauri AppPlugin 的唯一 enabled 回调
 *   → hasListener("back-button") ?
 *       false : canGoBack() ? goBack() : onBackPressed() → finish() → 退出 App
 *       true  : 只 trigger 事件，两条原生腿**都不走**
 * ```
 * （`tauri-2.11.5/mobile/android/src/main/java/app/tauri/AppPlugin.kt:28-46`）
 *
 * ⇒ 注册这个监听之后，**原生兜底整条失效**：不自己兑现根页那一跳，表现就是「按返回完全没反应」，
 * 比接线之前的「直接退出」更糟。故 [`handleBackPress`] 的三条分支**每一条都得有出口**：
 *
 *  1. 有可关闭层（弹层 / 二级页 / 批选态等）⇒ 关掉最上面那一层（`back-stack.ts`）；
 *  2. 不在冷启动落点 ⇒ 回落点；
 *  3. 落点的根页 ⇒ [`exitToBackground`] 把应用交还系统。
 *
 * 🔴 **隐私锁定态排在这三档之前**（W-16）。锁屏遮罩**刻意不登记**进 `back-stack`：登记了按一下
 * 返回就把它 dismiss 掉 —— 返回键成了解锁键，密码形同虚设（`MobileLockOverlay.tsx` 头注不变量 1）。
 * 但「不登记」只解决了第 1 档，第 2 档会在遮罩底下**换屏**、第 3 档才是想要的那个出口。故
 * [`handleBackPress`] 收一个 `locked`，为真时**在调 `dismissTop()` 之前**就早退到「交还系统」：
 *  · 不调 `dismissTop()` —— 否则会顺手弹掉底下某个真的还开着的弹层（用户解锁回来发现它没了）；
 *  · 不 `goTo` —— 锁定期间界面状态一格都不该动；
 *  · 出口仍然是「收起界面」，与 Android 上按 Home 同义，**不解锁**。
 *
 * 同一条推论还给出了**接管的前提**：第 3 档的出口是原生桥，桥不在就没有出口 ⇒
 * [`hasBackNavBridge`] 为假时**根本不注册监听**，把返回键整条留给原生兜底（退化成「按一下退出」，
 * 与接线之前一样），而不是接管之后哑掉。两侧的开关必须是同一个真值，不能一侧有条件一侧无条件。
 *
 * 第 3 档选 `moveTaskToBack` 而不是 `finish()`：VPN 客户端按返回的语义是「收起界面」，
 * 任务栈与前台服务都该留着（`finish()` 会让用户下次进来重走冷启动，隧道状态另说）。
 * 走 web → 原生桥而不是 `invoke('plugin:app|exit')`：Kotlin 侧虽有 `@Command fun exit`，
 * 但 `exit` **不在** `tauri-2.11.5/build.rs` 的 `core:app` 命令表里 ⇒ 不存在 `allow-exit`
 * 这个权限标识符，ACL 闸会直接拒掉。桥的对端见 `MainActivity.kt` 的 `BACK_NAV_BRIDGE`。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { addPluginListener, type PluginListener } from '@tauri-apps/api/core';
import { useMobileAppWiring } from './app-wiring';
import { useMobileVisibleViewport } from './visible-viewport';
import { dismissTop } from './back-stack';
import { MobileLockOverlay } from './MobileLockOverlay';
import { useMobileConfigSync } from './config-sync';
import { installMobileNavigator } from './navigate';
import { MobilePendingBar } from './MobilePendingBar';
import { MobileToaster } from './MobileToaster';
import { DEFAULT_DESTINATION, type DestinationId } from './destinations';
import { MobileShell } from './MobileShell';
import { useAppStore } from '@/store/app-store';
import { SCREENS } from './Screens';
import { useExactRuleFilter } from './connections/exact-rule-filter';
/* 🔴 **这一行的位置是有意的，别按字母序挪上去。** 进包顺序按模块图的深度优先走：`./Screens`
   把五个屏的 CSS 拖进包，本行排在它之后 ⇒ `forms/forms.css` 落在五份屏 CSS **之后**、
   外壳 `mobile.css` 之前。表单层盖在屏之上、又不压外壳，正是它在视觉层叠上该在的位置。
   `styles/css-oracle.test.ts` 那条「进包单元与顺序」的恰等快照钉着这个次序。 */
import { MobileFormHost } from './forms/MobileFormHost';

/**
 * 原生桥的 JS 对象名 —— `MainActivity.kt` 用 `addWebMessageListener` 把它注入到 `window` 上。
 * 两侧各写一份字面量的话，一侧改名而另一侧没改会让 `postMessage` 打在 `undefined` 上，
 * 表现是根页按返回**静默无反应**、运行期零报错。门按这个常量与 Kotlin 侧逐字对拍。
 */
export const BACK_NAV_BRIDGE = 'polarisBackNav';

/** 桥上唯一的载荷。原生侧不解析内容（收到即 `moveTaskToBack`），留一个字面量是为了两侧对拍时有得比。 */
export const BACK_NAV_EXIT = 'exit';

/** 返回键这一下最终落在哪一档。判据按它对差，而不是靠"有没有报错"。 */
export type BackOutcome = 'dismissed' | 'to-root' | 'exit';

interface BackNavBridge {
  postMessage: (message: string) => void;
}

/**
 * 原生收信口在不在。**接管返回键的前提条件**，见 [`MobileApp`] 里那条早退。
 *
 * 桥由 `MainActivity.addWebMessageListener` 在 **document-start** 注入，而本组件在
 * `i18nReady` 之后才渲染 ⇒ effect 跑到时它该在就一定已经在，这个判据不会误报「不在」。
 */
export function hasBackNavBridge(): boolean {
  return backNavBridge() !== undefined;
}

function backNavBridge(): BackNavBridge | undefined {
  return (globalThis as unknown as Record<string, BackNavBridge | undefined>)[BACK_NAV_BRIDGE];
}

/**
 * 把应用交还系统（第 3 档）。
 *
 * 桥不在时**什么都不做**：桌面 / 浏览器调试档上根本没有系统返回键。
 * Android 上「桥该在却不在」这一档不靠这里兜 —— 那时监听压根不会注册（见 [`MobileApp`]），
 * 本函数走不到。
 */
export function exitToBackground(): void {
  backNavBridge()?.postMessage(BACK_NAV_EXIT);
}

/**
 * 返回键的**全部**决策。三条分支各有一个出口，一条都不许 return 掉（见文件头注）。
 *
 * 抽成不吃 React 的纯函数是为了判据驱动得动它：喂一个空栈 + 落点目的地，
 * 就该看见 `exit` 被调用 —— 那正是「根页不许变成哑巴」这条门的断言面。
 */
export function handleBackPress(deps: {
  active: DestinationId;
  goTo: (id: DestinationId) => void;
  exit: () => void;
  /** 隐私锁定态。为真时上面那三档一档都不走，见文件头注那条 🔴。 */
  locked: boolean;
}): BackOutcome {
  if (deps.locked) {
    deps.exit();
    return 'exit';
  }
  if (dismissTop()) return 'dismissed';
  if (deps.active !== DEFAULT_DESTINATION) {
    deps.goTo(DEFAULT_DESTINATION);
    return 'to-root';
  }
  deps.exit();
  return 'exit';
}

export function MobileApp(): ReactElement {
  useMobileVisibleViewport();
  // 配置副本的首拉 + 广播重拉。少这一行，五个屏读到的 `config` 恒为 null（见 `config-sync.ts`）。
  useMobileConfigSync();
  /* 其余 app 级腿：连接态事件 + 首帧水合 + 30s 兜底、Tailscale 状态与登录 URL、OpenVPN 状态、
     订阅进度、待应用差集、代理错误 → toast/系统通知、配置对账、通知总开关同步。
     少这一行，首页按下连接之后**永远停在「未连接」**（`refreshProxyStatus` 全仓再无别的调用点），
     测速按钮随之恒置灰。逐条与「桌面哪几条刻意不接」见 `app-wiring.ts` 头注。 */
  useMobileAppWiring();
  const [active, setActive] = useState<DestinationId>(DEFAULT_DESTINATION);
  /* 隐私锁定态。返回键处理器要它（见头注那条 🔴），遮罩自己另读一次 —— 两处读同一个 store 字段，
     不是两份真值。 */
  const locked = useAppStore((s) => s.privacyMode);

  /* 当前目的地喂给返回处理器。用 ref 而不是把 `active` 放进 effect 依赖：后者会在每次切换目的地时
     退订再重订一个异步注册的原生监听，中间那一小段窗口里 `hasListener` 为假 ⇒ 恰好那时按返回
     会走原生兜底直接退出。监听只注册一次，读到的永远是最后一帧的目的地。 */
  const activeRef = useRef(active);
  activeRef.current = active;
  /* 同上：监听只注册一次，锁定态必须经 ref 读到**最后一帧**的值，否则遮罩挂上之后返回键仍按
     「未锁」那三档走（监听是在冷启动那一帧注册的，闭包里捕获的 `locked` 永远是 false）。 */
  const lockedRef = useRef(locked);
  lockedRef.current = locked;

  useEffect(() => {
    /* 🔴 两侧共用同一个真值：原生收信口不在就**不接管**返回键。
       `installBackNavBridge` 在 `WEB_MESSAGE_LISTENER` 不支持（WebView < 88）时直接 return，
       而注册 `back-button` 监听会让 `AppPlugin` 的两条原生腿一条都不走 ⇒ 若照常注册，
       那一档上第三档投递不出去、原生兜底又已废，表现是**返回键永久失灵**，比接线之前更糟。
       早退之后该档退化成「和今天一样：按一下退出 App」——降级，不是新缺陷。 */
    if (!hasBackNavBridge()) {
      console.warn(
        '[mobile] 原生返回桥缺席（WebView 不支持 WEB_MESSAGE_LISTENER？），返回键沿用系统缺省行为',
      );
      return undefined;
    }
    let live = true;
    let handle: PluginListener | null = null;
    void addPluginListener('app', 'back-button', () => {
      handleBackPress({
        active: activeRef.current,
        goTo: setActive,
        exit: exitToBackground,
        locked: lockedRef.current,
      });
    })
      .then((listener) => {
        if (live) handle = listener;
        else void listener.unregister();
      })
      .catch((err: unknown) => {
        /* 注册失败 = 返回键退回原生兜底（按一下退出 App）。非 Tauri 档（浏览器调试）本来就没有
           这个事件，故只报不抛；真机上这行出现就是 ACL 或插件面出了问题，值得看见。 */
        console.warn('[mobile] back-button 监听注册失败，返回键退回系统缺省行为:', err);
      });
    return () => {
      live = false;
      void handle?.unregister();
    };
  }, []);

  /* 跨屏跳转的装载点（`navigate.ts`）。今天唯一的消费者是订阅「更多」里的「更新间隔」——
     Polaris 没有 per-sub 间隔字段，那一项要落到设置→更新那唯一一处真开关上。
     装在这里的理由与返回键那条腿相同：本组件是这棵树上唯一既是组件、又在五个屏之上的位置。 */
  useEffect(() => installMobileNavigator(setActive), []);

  const onSelect = (destination: DestinationId) => {
    // Ordinary navigation must not inherit a one-off Home rule drill-down.
    useExactRuleFilter.getState().setExactRule(null);
    setActive(destination);
  };

  const Screen = SCREENS[active];
  return (
    <>
      <MobileShell
        active={active}
        onSelect={onSelect}
        /* 两件停靠区 chrome 在这里装配、由外壳只管摆位：外壳不认识 store / `api` / `error-handler`。
           `MobileToaster` 一挂上，移动端生产源码里那 24 处早就写好的 `toast.*` 调用当场从静音变成
           可见（此前 `setToastImpl` 全仓唯一注入点在桌面 `Toaster.tsx`）。 */
        pendingBar={<MobilePendingBar />}
        toastHost={<MobileToaster />}
      >
        <Screen />
      </MobileShell>
      {/* 表单层挂在外壳**之外**（同级兄弟）：它是 `position: fixed` 的全屏层，不参与外壳的
          滚动区/停靠区排版；放进外壳会被 `overflow` 裁掉，也会逼外壳多认识一个概念。
          与两件停靠区 chrome 同理由挂在这里：它要 store 与 `api`，而外壳不认识那两样。 */}
      <MobileFormHost />
      {/* 锁屏遮罩：外壳的**兄弟**（fixed 层压不过它自己所在的层叠上下文，故不能塞进外壳里）。
          锁定态由这里喂进去而不是它自己读 store，理由见 `MobileLockOverlay.tsx` 头注那条 🔴。 */}
      <MobileLockOverlay locked={locked} />
    </>
  );
}
