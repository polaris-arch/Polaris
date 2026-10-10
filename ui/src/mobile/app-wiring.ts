import { subscribeMobileSpeedTestProgress } from './use-mobile-speed-test';
/**
 * 移动端的**全局接线层** —— 对应桌面 `App.tsx` 那批 app 级 effect。
 *
 * # 缺陷原样：控件都在、数字不动
 *
 * 接线之前，移动入口这棵树上只挂着一条 `useMobileConfigSync()`。后果不是「少几个功能」，
 * 而是**界面在说假话**：
 *
 *  · 按下连接 → 后端真的起了核 → 首页永远停在「未连接」。`store.proxyStatus` 的**唯一**刷新入口是
 *    `refreshProxyStatus()`（`app-store.ts:396` 写明 `startProxy`/`stopProxy` **故意不写**
 *    `proxyStatus`，因为托盘浮层也能触发启停，只有走事件才能让多个窗口收敛），而全仓调它的地方
 *    在接线之前只有桌面 `App.tsx`。连带塌掉的还有：连接按钮的三态、测速按钮的可用性
 *    （`speedTestCaps.mainCorePool = proxyRunning`）、流量图的清空腿、出口国旗。
 *  · 首页与节点屏读 `tailscaleLoginStates` / `tailscaleStatuses` / `tailscaleAuthUrls`，
 *    三张表在移动端**没有任何写入方** ⇒ 组网卡角标恒「未登录」、`deriveTsExitWarning` 恒空、
 *    「完成登录授权」那颗按钮点了必然落到「要去桌面端」。
 *  · 节点屏读 `useVpnStatusStore.openVpn[…]` 喂 `deriveMeshTunnelHealth`，同样零写入方。
 *  · 外壳里那个 `.m-pending-slot` 槽位恒空：差集的 pull 与 push 两条腿一条都没挂。
 *
 * # 为什么挂在这里，而不是各屏内部
 *
 * 逐字同桌面 `App.tsx` 的头注：各屏是条件渲染（`SCREENS[active]`，无 keep-alive），组件卸载后订阅
 * 即失效，而连接态 / 配置变更 / 订阅进度在用户不在该屏时同样会发生（订阅更新更是**后台
 * scheduler** 自跑，压根没有前端调用点）。`MobileApp` 是这棵树上唯一既是组件、又在五个屏之上的位置。
 *
 * # 为什么拆成 [`startMobileAppWiring`] + [`useMobileAppWiring`]
 *
 * 同 `config-sync.ts` 的理由：本仓 vitest 跑 `environment: 'node'`、刻意不装 jsdom ⇒ `useEffect`
 * 在 `renderToStaticMarkup` 下**不执行**。effect 体若直接写在 `MobileApp` 里，「订阅 / 水合 / 退订」
 * 三件事就一条行为判据都写不出来，只剩源码级断言。拆开之后 effect 体是普通函数，能在 node 里喂假
 * IPC 真跑（`app-wiring.test.tsx` ①–⑦），而 hook 挂没挂由一次真渲染断言（⑨）。
 *
 * # `t` 走 getter，不走闭包捕获
 *
 * 这些订阅活一整个应用生命周期，闭包捕获的 `t` 在用户切界面语言之后就是陈旧的（桌面靠把 `t`
 * 放进 effect 依赖来重挂订阅，代价是每次切语言退订重订一整批）。这里改成传一个**读当下 i18n**
 * 的 getter，依赖恒空、订阅只建一次 —— 同 `lib/speedtest-progress-toast.ts` 已定的形态。
 *
 * # 桌面那 ~25 条 effect 里**没有**照搬的，以及为什么（判完才知道射程在哪）
 *
 * | 桌面 effect | 移动端 | 理由 |
 * |---|---|---|
 * | `useSystemProxyLivePolling()` | 不接 | 移动端接管方式恒为 TUN（`MobileHomeScreen.MOBILE_TAKEOVER`），`systemProxy` 档在 Android 上连入站都不发；那条轮询每轮 exec `networksetup`/`gsettings`/`reg`，在这里既无对象也无消费者 |
 * | `initTooltips()` | 不接 | 引擎是 `data-tip` 的 hover 委托；触屏没有 hover，移动端按 IA §4.12 把桌面的 `data-tip` 一律落成常驻行（各屏头注逐条登记过），全树零 `data-tip` |
 * | `useIdlePrivacyLock()` + 隐私态水合 / 进出事件 | **已接**（2026-09-06，W-16） | ⚠️ 这一行**曾经**写着「不接」，理由是「`privacyMode` 在 Android 上没有生产端 ⇒ 接了恒收不到帧」。那条判定当时是对的，射程也只到「**订阅桌面发的隐私态事件**」这一格：三个来源（托盘「立即锁定」/ 桌面 idle 计时 / 桌面窗解锁）确实一个都不在移动入口上。W-16 做的是另一件事 —— 让移动端**自己**产生这个状态（本文件下面那条闲置计时 + `MobileLockOverlay` 的解锁腿），生产端于是有了，水合与两条事件订阅随之成为真接线而不是假接线 |
 * | 界面语言水合（`syncLanguageChoice`） | 不接 | 移动端的写腿自带这一跳（`settings/DisplayPage.tsx` 改语言时当场 `syncLanguageChoice`），冷启动那一格由 `i18n/index.ts` 读 localStorage 兑现。缺的只是「config 被别处改了」这一档（备份恢复 / 桌面同步），不构成会说假话的界面，登记为后续线 |
 * | 订阅**创建**操作水合（`subscribeAndHydrateSubscriptionCreateOperations`） | **已接**（2026-09-25，ζ 批 A8） | ⚠️ 这一行**曾经**写着「不接」，最后一版理由是「找回来之后要交给一个可见的重试面，移动端没有那个面」。面现在有了：`forms/SubFormPanel.tsx#SubCreateTaskPanel`（进度文案与表单同一份），经 `forms/form-store.ts#openMobileSubscriptionCreateRecovery` 打开（桌面 `openTrackedSubscriptionCreateRecovery` 开的是桌面弹窗栈，移动端用自己的栈、同一条去重判据）。Android 上进程被回收 / Activity 重建是日常，这一格此前的代价是「表单关掉 / 应用被回收之后那次创建仍在后端跑、进度与结局都看不见」 |
 * | Taildrop 任务（`subscribeTaildropTaskEvents` + 水合） | **已接**（2026-09-25，ζ 批 A12） | ⚠️ 这一行**曾经**写着「不接」，理由是「唯一的消费者是一张短命面板，面板自持进度 + 一颗刷新」。代价是发件进行中进度不会自己跳，面板关掉再打开也只是再拉一次当时的快照。现在事件逐帧落 `useTaildropTaskStore`（跨面板存活的消费者就是这张 store），`forms/TaildropPanel.tsx` 只是它的视图（`visibleTaildropTasks`，与桌面 `TaildropDialog` 同一份） |
 * | 测速**进度** | **已接** | 窗口级 runId 会话 store 消费 PROGRESS/DONE；跨页保留，计数仅取真实事件与回执。桌面 sticky toast 不复用。 |
 * | 测速**结果**事件（`subscribeMobileSpeedTestProgress`） | **已接** | Rust 在出口 IP 探测成功后自动 fire-and-forget 伴测活跃节点；它没有页面级 await 返回值。必须在窗口生命周期持续接事件，切页仍更新同一 latency store。手动批量仍以 invoke 返回兜底。 |
 * | 助手可升级（`handleHelperUpgradeable`） | 不接 | 提权助手是桌面对象，Android 上没有这条腿 |
 * | 系统代理残留 | 不接 | 同 `useSystemProxyLivePolling`（无系统代理这个对象） |
 * | 无效节点（`onInvalidNodes`） | **已接**（2026-09-13，批 18） | ⚠️ 这一行**曾经**写着「不接」，理由是「移动端节点屏没有『标灰无效节点』这一格呈现（`setInvalidNodes` 在移动可达面上零读点）」—— 那句话是假的：`nodes/MobileNodesScreen.tsx` 一直在读 `invalidNodes` 并拿它算 `invalidNodeIndex`，`nodes/NodesScreenView.tsx` 也一直把理由渲染成常驻 `.mn-note`。真实形态是 W-23 那一档：**读点在、写入方一个都没有** ⇒「这个节点已失效」那一格恒不出现，不是坏了，是永远不会亮。这条账 2026-09-06 记在 `feed-register.test-support.ts` 的 `useAppStore.invalidNodes` 上，本批销掉 |
 * | 出口 IP（`onIpInfoUpdated` + peek） | **已接** | Rust 在起核后排程长热身检测；移动端在 app 级持续接事件，切页也不丢终态。手动按钮才调用 force |
 * | 解锁检测三条订阅 + 冷水合 | **已接** | Rust 的 invalidate 会去抖自跑；app 级持续接 progress / updated / invalidated，切页也不丢终态 |
 * | `event:configChanged` → `loadConfig(true)` | **已在别处** | `config-sync.ts` |
 *
 * 接的那些逐条见下面每个小节的注释。
 *
 * 🔴 **这张表由门对差，不靠人记得来补**（2026-09-06 复审 major，已修）。上一版这张表自称覆盖
 * 桌面全部 ~25 条，实测漏了两条 —— `api.proxy.onMeshLoginFallback`（登录期出口让位）与
 * `api.proxy.onAutoNodeSwitched`（自动换节点）**既没移植、也不在表里**，而两条的开关都露在移动端
 * 设置页上、后端 emit 无任何 `#[cfg]` 门控 ⇒ 在 Android 上真会发。本批把这两条接上（见下），
 * 并补了 `app-wiring.test.tsx` ⑫：从桌面 `App.tsx` 抓出全部 `api.<域>.on<X>(` 与
 * `subscribe<X>(`，与「移动树里真有调用点的那些」＋测试里那张 `DESKTOP_SUBSCRIPTIONS_NOT_PORTED`
 * 求并集对差 —— 桌面下次再加一条 effect，这里少一行就红。
 */

import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { api } from '@/ipc';
import type { UserConfig } from '@/contracts/types';
import type { VpnStatusSnapshot } from '@/contracts/vpn-status';
import { shouldArmIdleLock } from '@/domain/privacy';
import { handleProxyErrorEvent } from '@/domain/proxy-error-routing';
import { isDefinitiveTsLoginFrame } from '@/domain/tailscale-conn-state';
import { validatedTailscaleAuthUrl } from '@/domain/tailscale-auth-url';
import {
  claimLoginUrl, loginAttemptActive, mainAuthUrlOwner, openLoginUrl,
} from '@/domain/tailscale-login-progress';
import { toast } from '@/lib/error-handler';
import { notifyDesktop, setDesktopNotificationsEnabled } from '@/lib/desktop-notify';
import { unlockApi } from '@/ipc';
import { normalizePendingChanges, useAppStore, useEffectiveConfig } from '@/store/app-store';
import { useLatencyStore } from '@/store/use-latency-store';
import { useSubscriptionProgressStore, subscribeSubscriptionProgressEvents } from '@/store/use-subscription-progress-store';
import {
  stopSubscriptionCreateOperationSubscription,
  subscribeAndHydrateSubscriptionCreateOperations,
} from '@/store/subscription-create-operation-store';
import { hydrateTaildropTasks, subscribeTaildropTaskEvents } from '@/store/use-taildrop-task-store';
import { useTailscaleLoginCacheStore } from '@/store/use-tailscale-login-cache-store';
import { useTailscaleLoginProgressStore } from '@/store/use-tailscale-login-progress-store';
import { useVpnStatusStore } from '@/store/use-vpn-status-store';
import { openMobileSubscriptionCreateRecovery } from './forms/form-store';
import { armIdlePrivacyLock, shouldLockAfterColdStart } from './privacy-lock';
import { applySecureScreen } from './secure-screen-bridge';

/**
 * 取当下语种的翻译函数。**不是**捕获来的 `t`（见文件头「`t` 走 getter」）。
 * 第二参既吃插值对象、也吃 `t(key, fallback)` 那种字符串兜底 —— 桌面路由函数用的就是后者。
 */
export type WiringT = (key: string, vars?: Record<string, unknown> | string) => string;

/** 没有 serverId 的 Tailscale 登录 URL 归到这个哨兵键下（同桌面 `App.tsx`）。 */
const UNOWNED_TAILSCALE_AUTH_KEY = '__unowned__';

/**
 * 30s 低频兜底轮询的间隔。**不能降到零**：事件面盖不住的边缘态在 Android 上只多不少 ——
 * 崩溃腿只走 `set_error`、**不发** `proxyStopped`（全仓无任何崩溃路径发它），
 * 而 Android 上应用长时间在后台、WebView 被系统回收重建后订阅也会有一小段空窗。
 * 代价是每半分钟一次本地 IPC。逐字同桌面 `App.tsx`（系统设计 §B.3.7：事件为主、轮询兜底）。
 */
export const MOBILE_STATUS_POLL_MS = 30_000;

/**
 * 待应用差集 pull 兜底：拉当下差集写 store。
 *
 * 覆盖 PUSH 盖不住的三档（清差集 / 冷启动 / 订阅自动更新），触发点与桌面同一套。
 * 缺字段的降级走 store 里那**一份** `normalizePendingChanges`（pull 与 push 同构，
 * 两个入口共四条腿读同一口径）。失败静默 —— 差集是锦上添花，不该因拉取失败弹错。
 */
function pullPendingChanges(): void {
  void api.proxy
    .getPendingChanges()
    .then((p) => useAppStore.getState().setPendingChanges(normalizePendingChanges(p)))
    .catch(() => {});
}

/**
 * 一帧 VPN 快照写进 store。
 *
 * **与桌面的唯一差别：不开认证弹窗。** 桌面在 `authChallenge` / `challenge` 到达时
 * `dialogs.open({kind:'vpn-auth'})`，而弹窗层住在 `components/dialogs/**`（契约 A1 禁入移动端）。
 * 这里只落 store —— 移动端的消费者是节点屏的 `deriveMeshTunnelHealth`，它读的是隧道健康，不是挑战。
 * ⚠️ 连带结论，如实登记：**OpenVPN 的交互式认证挑战在移动端今天没有出口**，属表单层那一批，
 * 不在本批射程内；这里不为它造一个点了没有下一步的入口。
 */
function applyVpnStatusSnapshot(snapshot: VpnStatusSnapshot): void {
  useVpnStatusStore
    .getState()
    .replace(snapshot.connected, snapshot.openConnect ?? [], snapshot.openVpn ?? []);
}

function clearVpnStatusSnapshot(): void {
  useVpnStatusStore.getState().replace(false, [], []);
}

/**
 * 事件驱动的副作用的**唯一写出口**（裁定 #14 在本文件这一侧的落法）。
 *
 * 这一层的写腿没有「用户刚按下的那颗控件」可以贴 —— 它们由后端事件触发，用户可能正在别的屏、
 * 甚至已经把应用切到后台。故失败落**全局 toast**：那正是全局宿主（`MobileToaster`，W-25）存在的
 * 理由，也是各屏行内回显够不着的那一格。**不是**吞掉 —— 静默失败在这里的表现是
 * 「通知说该去登录了，点开却什么都没发生」。
 *
 * `console.error` 与 toast 两条都要：前者留诊断（含原始错误），后者是用户可见回执。
 */
async function reportIfFails(op: () => Promise<unknown>, message: string): Promise<void> {
  try {
    await op();
  } catch (err) {
    console.error('[mobile] 事件驱动的副作用失败:', err);
    toast.warning(message);
  }
}

/**
 * **配置对账**（W-24）：配置实体是所有 per-node / per-subscription 派生缓存的共同所有者。
 * 一次对账覆盖全部 store，避免只清延迟、却让登录缓存 / STATUS / 订阅失败进度在长会话里只增不减。
 *
 * `config === null` 是「尚未水合」，不是「用户删光了配置」—— 此时对账会把冷启动秒显的持久登录
 * 缓存整个清掉。故必须先有权威配置才允许做所有权对账（逐字同桌面）。
 *
 * 🔴 喂进来的必须是**磁盘镜像**（`useAppStore((s) => s.config)`），不是 `useEffectiveConfig()`。
 * 按 `store/app-store.ts` 那张读点口径表：本函数问的是「**后端此刻能按 id 找到 / 正在使用**的
 * 实体集合」，那一栏读 disk。读 effective 会踩上一条真的会咬人的形态（2026-09-06 复审 minor）：
 * 暂存一条**尚未保存**的节点删除 → `effectiveConfig` 少掉该 id → 这里把它那份**持久化**的
 * Tailscale 登录缓存清掉（代理关着时唯一的登录态来源）→ 用户在待应用条上点「重置」，节点回来了、
 * 角标却显示「未登录」，而 `hydrateTailscaleStates` 跟的是磁盘镜像 `s.servers`（没变过）⇒ 不重探，
 * 只能等起核的 STATUS 流或下次冷启动。表单/删除那一批落地后这条就是活的。
 *
 * 抽成导出的纯函数：判据可以直接喂一份配置跑它，不必渲染整棵树。
 */
export function reconcileEntityCaches(config: UserConfig | null | undefined): void {
  if (!config) return;
  const serverIds = (config.servers ?? []).map((server) => server.id);
  const subscriptionIds = (config.subscriptions ?? []).map((subscription) => subscription.id);
  useLatencyStore.getState().retainServerIds(serverIds);
  useAppStore.getState().retainServerIds(serverIds);
  useTailscaleLoginCacheStore.getState().retainServerIds(serverIds);
  useVpnStatusStore.getState().retainServerIds(serverIds);
  useSubscriptionProgressStore.getState().retainSubscriptionIds(subscriptionIds);
}

/**
 * Tailscale 登录态的**挂载兜底**（三条 feed 之二，见 `domain/tailscale-conn-state.ts` 头注）：
 * 不起核、只读当前持久会话（不证明授权仍有效）。缺它时代理没跑的整段时间里角标只有 localStorage 缓存一条来源。
 */
export function hydrateTailscaleStates(serverIds: readonly string[]): void {
  if (serverIds.length === 0) return;
  void api.server
    .tailscaleStateExists([...serverIds], true)
    .then((states) => useAppStore.getState().applyTailscaleStateExists(states))
    .catch((err: unknown) => console.error('[mobile] tailscaleStateExists failed:', err));
}

/**
 * 挂上全部**与配置无关**的 app 级腿，返回退订闭包。
 *
 * 分三类，每一类都在下面就地注明「不挂会怎样」：
 *  ① 首帧水合 —— 事件只推增量，初值必须自取（冷启动时核可能已经在跑）；
 *  ② 事件订阅 —— 后端状态跃迁的推送面；
 *  ③ 30s 兜底轮询 —— 事件面盖不住的边缘态的最后一道网。
 */
export function startMobileAppWiring(t: WiringT): () => void {
  const store = () => useAppStore.getState();

  /* ── ① 首帧水合 ─────────────────────────────────────────────────────────────
     少这一拍，冷启动进来看到的是「未连接」，直到用户手动点一次启停或等满 30s 兜底。
     Android 上这一档特别常见：VPN 服务是前台服务，进程被系统回收后 WebView 重建，核还在跑。 */
  void store().refreshProxyStatus();
  pullPendingChanges();
  void api.vpn.getStatus().then(applyVpnStatusSnapshot).catch(() => {});
  /* 隐私态首帧水合（W-16）。读的是 Rust 进程内的 `PRIVACY_MODE` atomic，**跨 WebView 重建存活** ——
     Android 上这一格比桌面更要紧：应用长期在后台，WebView 被系统回收重建是常态，而 enter/exit
     两条事件只推**增量**。少这一拍，「锁着的时候界面被重建」会得到一个没有遮罩的解锁界面，
     那正是这把锁要挡的场景。逐字同桌面 `App.tsx:269`。 */
  void api.config
    .getPrivacyMode()
    .then((on) => store().setPrivacyMode(on))
    .catch(() => {});

  const offs: Array<() => void> = [];

  // The backend owns connection-triggered probes. force=true here would cancel its
  // longer post-connect IP warmup and start a second unlock run before debounce.
  let live = true;
  offs.push(api.ipInfo.onUpdated((snapshot) => store().setIpInfo(snapshot)));
  void api.ipInfo.peek().then((snapshot) => {
    // The same backend revision orders cold hydration against any live event,
    // including stop/restart frames with identical wall-clock timestamps.
    if (live) store().setIpInfo(snapshot);
  }).catch(() => {});
  let unlockFeedRevision = 0;
  offs.push(
    unlockApi.onProgress((progress) => {
      unlockFeedRevision += 1;
      store().setUnlockProgress(progress.serviceId, progress.result);
    }),
    unlockApi.onUpdated((snapshot) => {
      unlockFeedRevision += 1;
      store().applyUnlockSnapshot(snapshot);
    }),
    unlockApi.onInvalidated((payload) => {
      unlockFeedRevision += 1;
      if (payload.running && !payload.exitBlocked) store().beginUnlockCheck();
      else store().resetUnlock();
    }),
  );
  const unlockHydrationRevision = unlockFeedRevision;
  void unlockApi.get().then((snapshot) => {
    if (!live || snapshot === null || unlockFeedRevision !== unlockHydrationRevision) return;
    const current = store().unlock;
    if (current.running || current.checkedAt !== null || Object.keys(current.results).length > 0) return;
    store().applyUnlockSnapshot(snapshot);
  }).catch(() => {});


  /* ── ② 连接态：proxyStarted / proxyStopped → 重拉真值 ──────────────────────
     不在 store 的 `startProxy`/`stopProxy` 里直接写 `proxyStatus`（`app-store.ts:396` 的既定结论），
     故这里是移动端 `proxyStatus` 的**唯一**事件驱动刷新点。顺带 pull 一次差集：重启落地后
     起核快照刷新，PUSH 的重启腿留下的 `added` 由这一拍清掉。 */
  offs.push(
    api.proxy.onStarted(() => {
      void store().refreshProxyStatus();
      void api.vpn.getStatus().then(applyVpnStatusSnapshot).catch(() => {});
      pullPendingChanges();
    }),
  );
  offs.push(
    api.proxy.onStopped(() => {
      void store().refreshProxyStatus();
      clearVpnStatusSnapshot();
      pullPendingChanges();
    }),
  );

  /* runtime 生命周期结局（`event:proxyLifecycle`）：后端在**真状态跃迁点**发，覆盖上面那对
     started/stopped 盖不住的全部**后端自驱**路径 —— 去抖重启 /「立即应用」/ drain 排空 / 崩溃自愈。
     这三条在移动端全都可达：改一次设置就会走 `switch_mode` 的去抖重启，而那条路径**不发**
     started/stopped。不接它，首页的 pid / 已运行时长最多陈旧 30s（等下一次兜底轮询）。
     **不再 pull 差集**：后端在同一跃迁点已经 PUSH 过（`push_pending_changes` 与 `push_lifecycle`
     严格配对），再拉一次是纯重复往返。 */
  offs.push(api.proxy.onLifecycle(() => void store().refreshProxyStatus()));

  /* 待应用差集 PUSH：后端 `switch_mode` 末尾 emit，载荷与 pull 腿同构。 */
  offs.push(api.proxy.onPendingChanges((data) => store().setPendingChanges(normalizePendingChanges(data))));

  /* 代理错误：**分腿的全部判定在 `domain/proxy-error-routing.ts`，与桌面同一份**
     （迁出理由见该文件头注）。这条订阅是移动端 `notifyDesktop` 的主消费点 —— Android 上应用长期
     在后台，应用内 toast 送不到，系统通知是崩溃/出口误导这类事实的唯一送达路径。
     不接它 = 后端把码发出来、前端一个字不显（起核腿还有连接按钮的 await 兜底，其余码没有）。 */
  offs.push(
    api.proxy.onError((data) =>
      handleProxyErrorEvent(data, {
        t: (key: string, fallback?: string) => t(key, fallback),
        refreshProxyStatus: () => store().refreshProxyStatus(),
      }),
    ),
  );

  /* 启动 gate 剔除的无效节点（`EVENT_PROXY_INVALID_NODES`）→ `store.invalidNodes`。
     消费者一直都在：`nodes/MobileNodesScreen.tsx` 读它算 `invalidNodeIndex`，再经
     `domain/invalid-node-reason.ts` 把机器 token 翻成人话，落成节点行上那条常驻 `.mn-note`
     （桌面把同一句挂在 `data-tip` 上，触屏摸不到 ⇒ IA §4.12 改成常驻行）。
     不接它，那一格**恒不出现** —— 核确实会报无效节点，只是没人接；这正是 W-23 的形状。

     写成长形还是 `store()` 速记都可以：喂数腿门的取材面 2026-09-13 起会先还原本模块的 store 别名
     （`screen-parity.test.ts#resolveStoreAliases`），两种写法在那张面上等价。此前只认长形，
     于是经 `store()` 的写入方整批隐形 —— 放宽当天就浮出四条从没被问过「谁写它」的写腿
     （`refreshProxyStatus` / `setPrivacyMode` / `setTailscaleStatus` / `setTailscaleLoginState`）。
     这里保留长形只是与桌面 `App.tsx:490` 逐字一致，不再是判据的要求。 */
  offs.push(api.proxy.onInvalidNodes((nodes) => useAppStore.getState().setInvalidNodes(nodes)));

  const authSeen = new Map<string, string>();
  const openAuthUrl = (serverId: string, owner: string, url: string): void => {
    if (!claimLoginUrl(authSeen, serverId, owner, url)) return;
    void openLoginUrl(url, api.system.openExternal, () => toast.warning(t('mobileSettings.about.openLinkFailed')));
    void notifyDesktop(t('notify.tsLogin.title'), t('notify.tsLogin.body'));
  };

  /* Tailscale STATUS 实时校正。登录成功只接受属于当前 attempt 的新鲜 Running 帧。
     折叠登录态仍过下面的 definitive 判决门，避免启动过渡帧污染持久缓存。 */
  offs.push(
    api.proxy.onTailscaleStatus((data) => {
      store().setTailscaleStatus(data);
      if (!isDefinitiveTsLoginFrame(data)) return;
      store().setTailscaleLoginState(data.serverId, data.loggedIn);
    }),
  );

  /* 主核 AUTH 帧不带 attemptId，仅供没有请求记录的旧主核流。
     当前或终态请求都只能消费带绑定的 producer progress。 */
  offs.push(
    api.proxy.onTailscaleAuth((data) => {
      if (data.transient || !validatedTailscaleAuthUrl(data.url)) return;
      const current = data.serverId ? useTailscaleLoginProgressStore.getState().attempts[data.serverId] : undefined;
      const owner = mainAuthUrlOwner(current);
      if (!owner) return;
      if (data.serverId) store().setTailscaleAuthUrl(data.serverId, data.url);
      openAuthUrl(data.serverId || UNOWNED_TAILSCALE_AUTH_KEY, owner, data.url);
    }),
  );

  /* 瞬态与主核登录进度只接受本窗口 begin 的 attempt。晚到的旧 URL/终态不覆盖新请求。 */
  offs.push(api.proxy.onTailscaleLoginProgress((data) => {
    if ((data.phase === 'awaitingAuth' || (data.phase === 'mainCore' && data.url))
      && !validatedTailscaleAuthUrl(data.url)) {
      if (useTailscaleLoginProgressStore.getState().apply({ ...data, phase: 'failed', reason: 'invalidAuthUrl', url: null })) {
        store().setTailscaleAuthUrl(data.serverId, null);
        store().setTailscaleLoginInitiated(data.serverId, false);
        void reportIfFails(
          () => api.server.tailscaleLoginCancel(data.serverId, data.attemptId),
          t('ts.loginCancelFailed'),
        );
      }
      return;
    }
    if (!useTailscaleLoginProgressStore.getState().apply(data)) return;
    store().setTailscaleLoginInitiated(data.serverId, loginAttemptActive(data.phase));
    store().setTailscaleAuthUrl(data.serverId,
      data.phase === 'awaitingAuth' || data.phase === 'mainCore' ? data.url ?? null : null);
    if (data.phase === 'authorized') store().setTailscaleLoginState(data.serverId, true);
    if ((data.phase === 'awaitingAuth' || (data.phase === 'mainCore' && !data.reason)) && data.url) {
      openAuthUrl(data.serverId, data.attemptId, data.url);
    }
  }));

  /* OpenVPN 原生状态增量 → `use-vpn-status-store`。消费者是节点屏组网分组的隧道健康
     （`deriveMeshTunnelHealth`），不接它那格恒为 undefined。认证挑战没有移动端出口，见
     `applyVpnStatusSnapshot` 头注。 */
  offs.push(api.vpn.onOpenVpnStatus((status) => useVpnStatusStore.getState().setOpenVpn(status)));

  /* 订阅更新逐阶段进度 → `use-subscription-progress-store` → 节点屏分组标签上的失败详情。
     这条腿有一个本条独有的理由：**后台 scheduler 也会更新订阅**，那条腿没有任何前端调用点
     可以挂 state，只能靠事件流。 */
  offs.push(subscribeSubscriptionProgressEvents());

  /* 订阅**创建**操作（ζ 批 A8）：后端持有的「创建订阅并拉节点」操作，WebView 重建 / 进程被回收
     之后仍在后端跑（或已经跑完）。`subscribeAndHydrateSubscriptionCreateOperations` 自持顺序：
     **先 await 事件登记、再 list 水合**，登记之后到 list 返回之间的新帧不会被旧快照盖掉（按
     operation revision 合并）；它还起一条低频 status 兜底，丢帧也不会把任务卡死在 committing。
     水合找回来的只有「本机发起过的」那几次（localStorage 追踪），逐个交给移动端恢复面
     （`SubCreateTaskPanel`）；list 里只有历史终态的那些**不开**，否则每次重建都重播一次旧的成功。
     退订与异步登记的竞态同桌面 `App.tsx`：先退订、后登记完成的那一档当场收掉。 */
  let createOpsDisposed = false;
  let createOps: { off: () => void; stopReconcile: () => void } | null = null;
  void subscribeAndHydrateSubscriptionCreateOperations()
    .then(({ off, stopReconcile, recovered }) => {
      if (createOpsDisposed) {
        stopSubscriptionCreateOperationSubscription({ off, stopReconcile });
        return;
      }
      createOps = { off, stopReconcile };
      for (const snapshot of recovered) openMobileSubscriptionCreateRecovery(snapshot.operationId);
    })
    .catch((err: unknown) => console.error('[mobile] subscription create hydrate failed:', err));
  offs.push(() => {
    createOpsDisposed = true;
    if (createOps) stopSubscriptionCreateOperationSubscription(createOps);
    createOps = null;
  });

  /* Taildrop 发件任务（ζ 批 A12）。任务属于后端，不属于面板：先挂事件再 pull 有界快照（顺序同桌面
     `App.tsx`，旧 pull 与新事件的竞态由 per-task revision 在 store 内拒绝）。消费者是
     `useTaildropTaskStore` —— 它活一整个应用生命周期，`TaildropPanel` 关掉再打开读到的是事件推过来的
     当下那一帧，发大文件时进度自己跳，不用按刷新。水合失败静默：非 Tauri 预览 / 核没跑时本就没有任务。 */
  offs.push(subscribeTaildropTaskEvents());
  void hydrateTaildropTasks().catch(() => {});

  /* 订阅后台自动更新结果 → 只 pull 一次差集。
     **刻意不弹 toast**（桌面弹）：移动端节点屏的分组标签上挂着一条**常驻**失败详情
     （`groupTabs[].failureDetail`，来自上面那条进度流的 `failed` 帧），它活到下一次更新为止 ——
     比 2.2s 的 toast 更适合「这条订阅现在是停更的」这种持续为真的状态，再加一条 toast 是同一事实
     的第二份低精度副本。这里要的是差集：自动更新会改节点集，而 PUSH 与本事件异步并行。 */
  offs.push(api.subscription.onAutoUpdate(() => pullPendingChanges()));

  /* 登录期出口让位（桌面 `App.tsx` 那条，逐字同口径）。选中账号制 TS 全隧道出口而登录尚未就绪时，
     后端把默认路由**零重启热切成 direct** 并发 `engaged:true`；就绪或关开关后切回发 `engaged:false`。
     不接它，Android 上的表现是：首页照旧「智能 · 已连接」+ 该组网节点为出口，**而流量此刻是明文
     直连** —— 与 W-27 移植的那条 2026-07-20 事故（分流语义反转、首页零呈现）同一形态。
     可达性不是推测：开关 `meshLoginFallbackDirect` 就在移动端设置页上
     （`settings/NetworkPage.tsx`，缺省为开），后端 `runtime/proxy/login_fallback.rs` 的三个
     emit 点没有任何 `#[cfg]` 门控。
     第三档（`engaged=false` 且不带 `serverName`）是**切走出口的静默复位**，不弹 —— 弹了会误报
     「已切回 XXX」。 */
  offs.push(
    api.proxy.onMeshLoginFallback((data) => {
      if (data.engaged) {
        toast.info(t('nodes.meshLoginFallbackTitle'));
        return;
      }
      if (!data.serverName) return;
      toast.success(t('nodes.meshLoginFallbackRestored', { name: data.serverName }));
    }),
  );

  /* 自动换节点（桌面 `App.tsx` 那条）：当前节点连不上、后端切到候选节点成功后 emit。开关
     `autoSwitchNode` 同样在移动端设置页上、后端 `runtime/proxy/auto_switch.rs` 的 emit 无 `#[cfg]`
     ⇒ 可达。两件事：**重拉连接态**（出口名 / 国旗要跟着换，否则得等下一次 30s 兜底轮询），
     以及一条 toast 说清「节点为什么自己变了」——没有它，用户看到的是首页节点名静默换成了另一个。
     **刻意不发系统通知**（与上面 `onAutoUpdate` 同一条口径）：换节点的结果本身是**常驻**呈现
     （首页那行节点名 + 出口国旗），回到前台一眼可见；而自动切换开着时这条事件可能连着来好几次。
     系统通知留给「不看就会错过」的那类（起核失败 / TS 登录）。 */
  offs.push(
    api.proxy.onAutoNodeSwitched((data) => {
      void store().refreshProxyStatus();
      toast.success(t('home.autoSwitched', { name: data.newServerName, latency: data.latency }));
    }),
  );

  /* 隐私态进/出（W-16）。后端 `config_set_privacy_mode` 只在**真状态跃迁**时 emit
     （`commands/config.rs:1327-1341`：`prev != value` 才发），故这两条是遮罩的唯一收敛点：
     闲置计时（下面那条 hook）与解锁腿（`privacy-lock.ts`）都只调 `setPrivacyMode`，谁都不写 store。
     单一收敛点是刻意的 —— 两个写入方会在「后端拒了但界面已经翻过去」时分叉。 */
  offs.push(api.config.onEnterPrivacyMode(() => store().setPrivacyMode(true)));
  offs.push(api.config.onExitPrivacyMode(() => store().setPrivacyMode(false)));

  /* ── ③ 30s 兜底轮询 ──────────────────────────────────────────────────────── */
  const poll = setInterval(() => void store().refreshProxyStatus(), MOBILE_STATUS_POLL_MS);
  const onForeground = () => {
    if (document.visibilityState === 'visible') void store().refreshProxyStatus();
  };
  if (typeof document !== 'undefined') document.addEventListener('visibilitychange', onForeground);

  return () => {
    live = false;
    clearInterval(poll);
    if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', onForeground);
    for (const off of offs) off();
  };
}

/**
 * 移动端根组件挂这一条即可（`MobileApp`）。
 *
 * 七条 effect 的依赖各不相同，**不能合成一条**：订阅面依赖恒空（活一整个应用生命周期），
 * 而对账 / 通知开关 / 隐私锁冷启动补锁 / `FLAG_SECURE` / 隐私锁闲置计时 / TS 兜底水合
 * 各自跟着一份会变的输入走。
 */
export function useMobileAppWiring(): void {
  const { i18n } = useTranslation();
  /* 订阅活一辈子，故不把 `t` 放进依赖（那会在每次切语言时退订重订一整批）。ref 让回调读到当下语种。 */
  const i18nRef = useRef(i18n);
  i18nRef.current = i18n;

  // Backend IP probe schedules warm RTT once for the active exit. The result is
  // event-only, so this subscription belongs to the app lifetime, not Home.
  useEffect(() => subscribeMobileSpeedTestProgress(), []);

  useEffect(
    () =>
      startMobileAppWiring((key, vars) =>
        typeof vars === 'string' ? i18nRef.current.t(key, vars) : i18nRef.current.t(key, vars ?? {}),
      ),
    [],
  );

  /* 配置对账。读**磁盘镜像**（与桌面 `App.tsx` 同源），理由见 `reconcileEntityCaches` 头注那条
     🔴：暂存里的删除**尚未落盘**，提前驱逐它的持久缓存会让「重置」之后角标变成「未登录」。 */
  const config = useAppStore((s) => s.config);
  useEffect(() => {
    reconcileEntityCaches(config);
  }, [config]);

  /* 系统通知总开关同步（缺省视为开）：config 变即同步给通知出口，`notifyDesktop` 据此静默门控。
     不接这条，设置页那个开关写进 config 之后没有任何消费方 —— 关掉它照样发通知。
     这一条**刻意读 effective**，与上面那条对账相反：它问的不是「后端认得哪些实体」，而是
     「用户现在把这个开关拨到哪一档」—— 用户刚关掉通知、还没落盘的那段时间里就不该再发。
     它也没有对账那条的风险：开关是个布尔，落盘失败会由设置页的行内回显说话，不驱逐任何持久状态。 */
  const desktopNotifications = useEffectiveConfig((c) => c?.desktopNotifications);
  useEffect(() => {
    setDesktopNotificationsEnabled(desktopNotifications);
  }, [desktopNotifications]);

  /* 自动隐私锁的闲置计时（W-16）。桌面同一条腿挂在 `App.tsx`（`useIdlePrivacyLock`），
     移动端挂这里，理由与本文件其余几条一致：各屏是条件渲染，而闲置发生在用户不在任何一屏操作时。

     武装条件走**共享**的 `shouldArmIdleLock`（开了自动隐私锁 且 当前未锁）：已锁再武装 =
     每 10 分钟重复触发一次 `setPrivacyMode(true)`，而那条命令在同值时不 emit ⇒ 症状是纯浪费、
     还看不出来。两个平台读同一份判定，不各写一遍。

     失败落全局 toast 而不是静默（桌面那份是静默的，这里刻意不照抄）：**没锁上**是一条用户会
     照着它行动的安全承诺 —— 他以为把手机递出去界面会锁。静默失败在这里等于骗人。 */
  const autoPrivacyMode = useEffectiveConfig((c) => c?.autoPrivacyMode ?? false);
  const privacyMode = useAppStore((s) => s.privacyMode);

  /* 🔴 冷启动补锁（2026-09-06 复审 blocker）。上面那条水合只问后端「现在锁着吗」，而后端那个
     真值是**进程内的** atomic：Android 上系统回收整个进程是日常，回收之后它复位成 false，
     闲置计时也随进程一起死 ⇒ 用户再打开时得到一个没有遮罩的解锁界面。**判定**住在
     `privacy-lock.ts` 的 `shouldLockAfterColdStart`（本进程一次的闸与三条早退的理由都在那里），
     **翻转**留在这里 —— 与下面闲置计时那条同一个出口，失败一并落全局 toast。

     依赖是 `[autoPrivacyMode]` 而不是 `[]`：`useEffectiveConfig` 在 config 载入前恒返 false，
     依赖为空的话这条 effect 只在挂载那一帧跑一次、那时必然早退，于是**一次都不会补锁**。 */
  useEffect(() => {
    if (!autoPrivacyMode) return;
    void reportIfFails(async () => {
      if (await shouldLockAfterColdStart()) await api.config.setPrivacyMode(true);
    }, i18nRef.current.t('mobileSettings.general.privacyLockFailed'));
  }, [autoPrivacyMode]);

  /* 🔴 `FLAG_SECURE` 跟着**开关**走（2026-09-06 复审 major）。锁上那一刻才置 flag 是没用的：
     Android 的任务快照在 Activity 停止那一刻就拍完了，而闲置锁的典型发生时刻正是「已经在后台」
     ⇒ 最近任务里那张缩略图仍是锁定前的一屏，节点名/订阅名可读、全程不要密码。理由与两条腿
     （置上/清掉）的必要性见 `secure-screen-bridge.ts` 头注。
     桥不在（浏览器调试档）时它返 false，只落一行日志：这一档本就没有系统快照这个对象。 */
  useEffect(() => {
    if (!applySecureScreen(autoPrivacyMode) && autoPrivacyMode) {
      console.warn('[mobile] FLAG_SECURE 桥缺席，隐私锁开着但系统快照/截图没有被禁掉');
    }
  }, [autoPrivacyMode]);

  useEffect(() => {
    if (!shouldArmIdleLock(autoPrivacyMode, privacyMode)) return undefined;
    return armIdlePrivacyLock(window, () => {
      void reportIfFails(
        () => api.config.setPrivacyMode(true),
        i18nRef.current.t('mobileSettings.general.privacyLockFailed'),
      );
    });
  }, [autoPrivacyMode, privacyMode]);

  /* TS 登录态的挂载兜底。跟着节点集走：新增一个 TS 节点后也要查一次。 */
  const servers = useAppStore((s) => s.servers);
  useEffect(() => {
    hydrateTailscaleStates(
      servers.filter((s) => s.protocol?.toLowerCase() === 'tailscale').map((s) => s.id),
    );
  }, [servers]);
}
