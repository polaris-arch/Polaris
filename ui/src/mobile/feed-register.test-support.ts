/**
 * **喂数腿登记表** —— 移动端整棵树读的每一格 store 状态，逐格登记它在移动树里的写入方。
 *
 * # 它守的是哪一档缺陷（W-23 / W-24 就死在这一档）
 *
 * 接线之前那批缺陷的形态不是「控件少了」，而是**「读点在、写入方一个都没有」**：
 * 首页与节点屏读 `tailscaleLoginStates` / `tailscaleStatuses`（W-23）、节点屏读订阅进度（W-24），
 * 而三张表在移动端没有任何写入方 ⇒ 角标恒「未登录」、进度条恒不出现。控件都在、界面在说假话。
 *
 * 这种缺陷**在所有既有的面上都是隐形的**：
 *  · i18n 面看不见（没有任何一句「这里没有」的文案 —— 界面以为自己在正常工作）；
 *  · 控件面看不见（没有任何一颗写死禁用的控件）；
 *  · 屏级对差表看不见（那颗控件桌面移动端都在，逐颗对差全绿）；
 *  · `app-wiring.test.tsx` ⑫ 只问「桌面那条订阅移动端接没接 / 登不登记」，
 *    **不问「移动端读的那格状态有没有人写」** —— 一条被登记成「不接」的订阅，
 *    只要没人注意到有屏在读它写的那格，⑫ 就照绿。
 *
 * 本表补的正是最后那一格：取材面在**移动端这一侧**（读点），判据是**写入方存在且被机器打开核对**。
 * 桌面那一侧改成什么样都不影响它 —— 两道门方向相反，故不重叠。
 *
 * # 取材面（全称否定的前提，写在这里）
 *
 * `ui/src/mobile/**` 全部非测试 `.ts/.tsx`，剥注释与字符串后认三种词法形态：
 *  ① `useXStore((s) => … s.<字段> …)` —— 选择器体里出现的**每一个** `s.<字段>`（一次调用可产出多条）；
 *  ② `useXStore.getState().<字段>`；
 *  ③ `useXStore()` / `useEffectiveConfig()` 这类**无选择器**的整体读，记作 `useXStore.*`。
 * 只收名字像 store 的 hook（`*Store` 结尾，加上 `useEffectiveConfig` / `useEffectiveServers` /
 * `useSystemProxyLive` 三个具名的派生读）。
 *
 * 🔴 **抓不到的形态，逐条写在这里**（不许把它说成「覆盖全部状态读」）：
 *  · 把 store 传成参数、在别的模块里解构再读（本仓今天零处，门每次自己报这个数）；
 *  · `const s = useAppStore.getState(); s.foo` 这种先取快照再点属性的两步写法；
 *  · 计算属性 `s[key]`。
 * 这三种一旦出现，读点会掉在面外 ⇒ 它们各自的写入方也就无人核对。兜底是 ①–③ 之外**没有第二条
 * 便捷路径** —— 本仓四百多处 store 读全是上面三种形态，门的 ⓪ 自检把总数钉住，
 * 有人换写法会让总数掉下去、当场红。
 *
 * # 三种登记，处置码与屏级表共用一套（`parity-registry.ts`）
 *
 *  · `role: 'action'` + `ported` —— 这一格读的是**写腿本身**（`startProxy` / `stage` / `retainServerIds`）。
 *    锚指 store 模块里那条声明：函数改名 / 被删，当场红。
 *  · `role: 'state'` + `ported` —— 有写入方，锚指移动树里那条真的写调用。
 *  · `role: 'state'` + `platform-absent` —— **刻意没有写入方**，且「恒初值」正是对的答案。
 *    🔴 这个码的锚必须指一条**平台判定**（`#[cfg(` / `target_os` / `platform != "android"` /
 *    平台专属源码树），不是一句我们自己写的「这里没有」——`screen-parity.test.ts ④` 逐条核对。
 *    2026-09-06 因此把 `privacyMode` 从这一档改判成 `absent`：Android 上隐私锁并非不可实现。
 *  · `role: 'state'` + `absent` —— **没有写入方，而恒初值是错的**：界面读着一格永远不动的状态。
 *    这一档进债务，因为它就是 W-23 的形状。
 *
 * # 本表落地当天抓到的一条活的（`useAppStore.invalidNodes`）—— 2026-09-13（批 18）已销
 *
 * 节点屏 `nodes/MobileNodesScreen.tsx` 读 `invalidNodes`、拿它算 `invalidNodeIndex`，
 * 而 `api.proxy.onInvalidNodes` 在 `app-wiring.test.tsx` ⑫ 的 `NOT_PORTED` 里登记着「不接」，
 * 理由写的是「`setInvalidNodes` 在移动可达面上零读点」—— **那句话是假的**。
 * 后果是「无效节点」那一格恒不出现：不是坏了，是永远不会亮。逐字同 W-23 的形状，
 * 而在本表之前没有任何一面看得见它。批 18 把那条订阅接进 `app-wiring.ts`，
 * ⑫ 那条登记随之删掉，本条翻成 `ported`、锚指那条真的写调用。
 *
 * 🔴 它留下的那条判据教训**比这条账本身重要**：写入方必须落在本表的取材面上。
 * `app-wiring.ts` 里那条写调用刻意写成 `useAppStore.getState().setInvalidNodes(…)` 而不是
 * 本地的 `store()` 速记 —— 后者不在 ①–③ 三种词法形态里，于是写入方在本表上隐形，
 * 「摘掉订阅」这次变异不会让任何东西红。一条自己看不见的写入方，等于没有写入方。
 */
import type { Anchor, ParityDisposition } from './parity-registry.test-support';

export interface FeedEntry {
  /** `<hook>.<字段>`，由门从移动端源码算出来（`.*` = 无选择器的整体读）。 */
  readonly id: string;
  /** `action` = 这一格读的是写腿本身；`state` = 这一格是被读的状态。 */
  readonly role: 'state' | 'action';
  readonly disposition: ParityDisposition;
  readonly note: string;
}

/* ───────────── 写入方锚（多格共用的写在这里，改一处就够）───────────── */

/** 配置镜像的唯一写入方：冷启动一次 + `event:configChanged` 广播重拉。 */
const CONFIG_FED: Anchor = { file: 'ui/src/mobile/config-sync.ts', mustContain: 'loadConfig(true)' };
/** 连接态：事件三条腿 + 30s 兜底轮询都收敛到这一次重拉。 */
const PROXY_STATUS_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'store().refreshProxyStatus()' };
/** 🔴 W-23 的那条腿。摘掉它，`tailscaleStatuses` / `tailscaleLoginStates` 两格立刻没有写入方。 */
const TS_STATUS_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'api.proxy.onTailscaleStatus(' };
const TS_AUTH_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'api.proxy.onTailscaleAuth(' };
const TS_PROGRESS_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'api.proxy.onTailscaleLoginProgress(' };
const STORE_TS_PROGRESS: Anchor = { file: 'ui/src/store/use-tailscale-login-progress-store.ts', mustContain: 'export const useTailscaleLoginProgressStore' };
/** 🔴 W-24 的那条腿。后台 scheduler 自跑，没有任何前端调用点可挂 —— 只能走事件。 */
const SUB_PROGRESS_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'subscribeSubscriptionProgressEvents()' };
const VPN_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'api.vpn.onOpenVpnStatus(' };
/** 🔴 本表落地当天抓到的那条（批 18 接上）。摘掉它，`invalidNodes` 立刻退回「读点在、没有写入方」。 */
const INVALID_NODES_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'api.proxy.onInvalidNodes(' };
const PENDING_FED: Anchor = { file: 'ui/src/mobile/app-wiring.ts', mustContain: 'setPendingChanges(normalizePendingChanges(data))' };
const STAGED_FED: Anchor = { file: 'ui/src/mobile/MobilePendingBar.tsx', mustContain: 'useStagedConfigStore.getState().save()' };
/** 隐私锁在移动端没有生产端 —— 设置页那一行如实写着，且那句话是用户可见的。 */


const STORE_APP: Anchor = { file: 'ui/src/store/app-store.ts', mustContain: 'export const useAppStore' };
const STORE_SPEED: Anchor = { file: 'ui/src/mobile/use-mobile-speed-test.ts', mustContain: 'export const useMobileSpeedTestStore' };
const STORE_NODE_SORT: Anchor = { file: 'ui/src/store/use-node-sort-store.ts', mustContain: 'export const useNodeSortStore' };
const SPEED_FED: Anchor = { file: 'ui/src/mobile/use-mobile-speed-test.ts', mustContain: '.progress(event)' };
const STORE_LATENCY: Anchor = { file: 'ui/src/store/use-latency-store.ts', mustContain: 'export const useLatencyStore' };
const STORE_STAGED: Anchor = { file: 'ui/src/store/staged-config-store.ts', mustContain: 'export const useStagedConfigStore' };
const STORE_VPN: Anchor = { file: 'ui/src/store/use-vpn-status-store.ts', mustContain: 'export const useVpnStatusStore' };
const STORE_TSCACHE: Anchor = { file: 'ui/src/store/use-tailscale-login-cache-store.ts', mustContain: 'export const useTailscaleLoginCacheStore' };
const STORE_SUBPROG: Anchor = { file: 'ui/src/store/use-subscription-progress-store.ts', mustContain: 'export const useSubscriptionProgressStore' };
const STORE_PRESETS: Anchor = { file: 'ui/src/store/use-app-presets-store.ts', mustContain: 'export const useAppPresetsStore' };
const STORE_REDACT: Anchor = { file: 'ui/src/store/use-log-redact-store.ts', mustContain: 'export const useLogRedactStore' };
const STORE_DIALOG: Anchor = { file: 'ui/src/components/dialogs/dialog-store.ts', mustContain: 'export const useDialogStore' };

const act = (declaredIn: Anchor): ParityDisposition => ({ kind: 'ported', mobile: declaredIn });
const fed = (writer: Anchor): ParityDisposition => ({ kind: 'ported', mobile: writer });

export const MOBILE_FEEDS: readonly FeedEntry[] = [
  { id: 'useNodeSortStore.sortByLatency', role: 'state', disposition: fed({ file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'onClick={toggleSortByLatency}' }), note: '首页排序开关更新同一份持久偏好，节点页和托盘也读它。' },
  { id: 'useNodeSortStore.toggleSortByLatency', role: 'action', disposition: act(STORE_NODE_SORT), note: '首页选择 sheet 的按延迟排序动作；组内排序复用既有比较器。' },
  /* 测速轮次为窗口级内存状态；真实事件/回执写入，静默只切 waiting。 */
  { id: 'useMobileSpeedTestStore.task', role: 'state', disposition: fed(SPEED_FED), note: '真实轮次与本地请求锁由窗口事件 feed 和 invoke 回执更新；跨屏读同一状态。' },
  { id: 'useMobileSpeedTestStore.request', role: 'state', disposition: fed({ file: 'ui/src/mobile/use-mobile-speed-test.ts', mustContain: '.begin(ids, kind, acceptLatency)' }), note: '真实轮次与本地请求锁由窗口事件 feed 和 invoke 回执更新；跨屏读同一状态。' },
  { id: 'useMobileSpeedTestStore.begin', role: 'action', disposition: act(STORE_SPEED), note: '测速 coordinator 的 begin 写腿；runId高水位与本地token保护已由行为测试核对。' },
  { id: 'useMobileSpeedTestStore.progress', role: 'action', disposition: act(STORE_SPEED), note: '测速 coordinator 的 progress 写腿；runId高水位与本地token保护已由行为测试核对。' },
  { id: 'useMobileSpeedTestStore.result', role: 'action', disposition: act(STORE_SPEED), note: '测速 coordinator 的 result 写腿；runId高水位与本地token保护已由行为测试核对。' },
  { id: 'useMobileSpeedTestStore.done', role: 'action', disposition: act(STORE_SPEED), note: '测速 coordinator 的 done 写腿；runId高水位与本地token保护已由行为测试核对。' },
  { id: 'useMobileSpeedTestStore.settle', role: 'action', disposition: act(STORE_SPEED), note: '测速 coordinator 的 settle 写腿；runId高水位与本地token保护已由行为测试核对。' },
  { id: 'useMobileSpeedTestStore.waiting', role: 'action', disposition: act(STORE_SPEED), note: '测速 coordinator 的 waiting 写腿；runId高水位与本地token保护已由行为测试核对。' },

  /* ── 应用规则预设（规则屏的应用分流）─────────────────────────────────── */
  { id: 'useAppPresetsStore.loadPresets', role: 'action', disposition: act(STORE_PRESETS), note: '预设加载腿本身（规则屏挂载时跑一次）。' },
  { id: 'useAppPresetsStore.presets', role: 'state', disposition: fed({ file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'void loadPresets();' }), note: '应用分流预设表。写入方就是本屏那次 `loadPresets()`。' },
  { id: 'useAppPresetsStore.loaded', role: 'state', disposition: fed({ file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'void loadPresets();' }), note: '预设是否已加载过（决定空态与加载态怎么画）。' },
  { id: 'useAppPresetsStore.loading', role: 'state', disposition: fed({ file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'void loadPresets();' }), note: '预设加载中。' },
  { id: 'useAppPresetsStore.failed', role: 'state', disposition: fed({ file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'void loadPresets();' }), note: '预设加载失败（失败要能看见，否则空列表读起来像「一个应用都没有」）。' },

  /* ── app-store：配置镜像 ─────────────────────────────────────────────── */
  { id: 'useAppStore.config', role: 'state', disposition: fed(CONFIG_FED), note: '磁盘配置镜像。' },
  { id: 'useAppStore.servers', role: 'state', disposition: fed(CONFIG_FED), note: '节点扁平镜像，只由 loadConfig / saveConfig 写。' },
  { id: 'useAppStore.rules', role: 'state', disposition: fed(CONFIG_FED), note: '分流规则扁平镜像。' },
  { id: 'useAppStore.dnsRules', role: 'state', disposition: fed(CONFIG_FED), note: 'DNS 规则扁平镜像。' },
  { id: 'useAppStore.selectedServerId', role: 'state', disposition: fed(CONFIG_FED), note: '当前出口 id。' },
  { id: 'useEffectiveConfig.*', role: 'state', disposition: fed(CONFIG_FED), note: '磁盘配置叠加暂存差集后的**生效**视图。两个上游（配置镜像 + 暂存表）各自的写入方都在本表里。' },
  { id: 'useEffectiveConfig.desktopNotifications', role: 'state', disposition: fed(CONFIG_FED), note: '通知总开关的生效值（`app-wiring` 拿它同步给通知出口）。' },
  { id: 'useEffectiveServers.*', role: 'state', disposition: fed(CONFIG_FED), note: '节点的生效视图，同上。' },

  /* ── app-store：运行态 ───────────────────────────────────────────────── */
  { id: 'useAppStore.proxyStatus', role: 'state', disposition: fed(PROXY_STATUS_FED), note: '连接态。**唯一**写入口是 refreshProxyStatus（startProxy/stopProxy 故意不写，托盘等别的入口也能启停）。' },
  { id: 'useAppStore.pendingChanges', role: 'state', disposition: fed(PENDING_FED), note: '待应用差集。PUSH 帧写，另有三档由 pull 兜底。' },
  /* ── 2026-09-06 合并态补登：批 2（表单宿主）与批 5（隐私锁）新增的读点。
        b6 分叉在那两批合入之前，故它的表里没有它们 —— 门抓到的正是这个差。 ── */
  { id: 'useMobileFormStore.stack', role: 'state', disposition: fed({ file: 'ui/src/mobile/forms/form-store.ts', mustContain: 'set((s) =>' }), note: '表单栈。宿主整叠都渲染（只渲染栈顶会把底下那张表的草稿卸载掉）。' },
  { id: 'useMobileFormStore.open', role: 'action', disposition: fed({ file: 'ui/src/mobile/forms/form-store.ts', mustContain: 'open:' }), note: '开一张表(入栈)。宿主整叠都渲染,只渲染栈顶会把底下那张表的草稿卸载掉。' },
  { id: 'useMobileFormStore.closeInstance', role: 'action', disposition: fed({ file: 'ui/src/mobile/forms/form-store.ts', mustContain: 'closeInstance:' }), note: '按实例关（不是关栈顶）—— 异步提交回来时栈顶可能已经不是自己。' },
  { id: 'useMobileFormStore.hasInstance', role: 'action', disposition: fed({ file: 'ui/src/mobile/forms/form-store.ts', mustContain: 'hasInstance:' }), note: '判自己还在不在栈上，异步回执落地前先问一次。' },
  { id: 'useStagedConfigStore.revert', role: 'action', disposition: fed({ file: 'ui/src/mobile/nodes/node-deletion.ts', mustContain: 'splitStagedOnly' }), note: '删除编排里的暂存回退那一段（复用桌面同一条腿）。' },
  { id: 'useSubscriptionCreateOperationStore.start', role: 'action', disposition: fed({ file: 'ui/src/mobile/forms/SubFormPanel.tsx', mustContain: 'startCreate(' }), note: '发起订阅创建。' },
  { id: 'useSubscriptionCreateOperationStore.snapshots', role: 'state', disposition: fed({ file: 'ui/src/mobile/app-wiring.ts', mustContain: 'void subscribeAndHydrateSubscriptionCreateOperations()' }), note: '创建进度快照。2026-09-25 ζ 批 A8 起写入方是**应用级**订阅 + list 水合（此前表单在自己生命期内订阅，表单关掉 / 进程被回收之后就没人写了）；表单与恢复面 SubCreateTaskPanel 都只读。' },
  { id: 'useSubscriptionCreateOperationStore.handledTerminalRevisions', role: 'state', disposition: fed({ file: 'ui/src/mobile/forms/SubFormPanel.tsx', mustContain: 'markTerminalHandled(operationId, snapshot.revision)' }), note: '终态回执已播报到哪一版（跨 WebView 重建持久化）。恢复面据此决定成功 toast 只播一次（ζ 批 A8）。' },
  { id: 'useSubscriptionCreateOperationStore.markTerminalHandled', role: 'action', disposition: fed({ file: 'ui/src/store/subscription-create-operation-store.ts', mustContain: 'markTerminalHandled:' }), note: '记下某次终态已播报。恢复面在发布确认之后才记（面板没了就不记，留给下次水合）。' },
  { id: 'useSubscriptionCreateOperationStore.cancel', role: 'action', disposition: fed({ file: 'ui/src/store/subscription-create-operation-store.ts', mustContain: 'cancel: async' }), note: '恢复面的「取消添加」（叠确认）。只有终态 cancelled 帧才关面板。' },
  { id: 'useTaildropTaskStore.tasks', role: 'state', disposition: fed({ file: 'ui/src/mobile/app-wiring.ts', mustContain: 'offs.push(subscribeTaildropTaskEvents())' }), note: 'Taildrop 发件任务（ζ 批 A12）。写入方是应用级事件订阅 + 冷启动水合，TaildropPanel 只是视图 —— 面板关掉再打开进度仍是当下那一帧。' },
  { id: 'useTaildropTaskStore.applySnapshot', role: 'action', disposition: fed({ file: 'ui/src/store/use-taildrop-task-store.ts', mustContain: 'applySnapshot:' }), note: '取消发件的回执本身就是一帧快照，当场并进 store。' },
  { id: 'useTaildropTaskStore.hydrateSnapshots', role: 'action', disposition: fed({ file: 'ui/src/store/use-taildrop-task-store.ts', mustContain: 'hydrateSnapshots:' }), note: '面板打开 / 操作后那次 pull 并进 store（按 revision 合并，不覆盖更新的事件帧）。' },
  { id: 'useSubscriptionCreateOperationStore.clearTerminal', role: 'action', disposition: fed({ file: 'ui/src/mobile/forms/SubFormPanel.tsx', mustContain: 'clearTerminal(' }), note: '清掉终态记录；刻意排在 hasInstance 之后（见该处注释）。' },
  { id: 'useEffectiveConfig.autoPrivacyMode', role: 'state', disposition: fed({ file: 'ui/src/mobile/app-wiring.ts', mustContain: 'armIdlePrivacyLock' }), note: '闲置隐私锁的武装条件（批 5 接上）。' },
  { id: 'useAppStore.ipInfo', role: 'state', disposition: fed({ file: 'ui/src/mobile/app-wiring.ts', mustContain: 'api.ipInfo.onUpdated(' }), note: '出口 IP 结果由 Rust 连接后长热身探测推送，应用级接线在切页后仍接收终态。' },
  { id: 'useAppStore.unlock', role: 'state', disposition: fed({ file: 'ui/src/mobile/app-wiring.ts', mustContain: 'unlockApi.onUpdated(' }), note: '解锁结果由 Rust 失效后自跑，应用级订阅跨页接收增量与终态。' },
  { id: 'useAppStore.tailscaleStatuses', role: 'state', disposition: fed(TS_STATUS_FED), note: '🔴 W-23 的那一格：组网节点的原始 STATUS 帧。没有这条腿，节点屏的隧道健康与出口警示恒空。' },
  { id: 'useAppStore.tailscaleLoginStates', role: 'state', disposition: fed(TS_STATUS_FED), note: '🔴 W-23 的另一半：折叠后的登录态。没有写入方时角标恒「未登录」，而「完成登录授权」点了必然落到「要去桌面端」。' },
  { id: 'useAppStore.tailscaleAuthUrls', role: 'state', disposition: fed(TS_AUTH_FED), note: '登录 URL（按 serverId 存最后一个）。节点屏拿它渲染「完成登录授权」。' },
  { id: 'useTailscaleLoginProgressStore.attempts', role: 'state', disposition: fed(TS_PROGRESS_FED), note: '本窗口的登录 attempt 状态，由应用级进度订阅按 attemptId 喂入；面板只显示自己发起的请求。' },
  { id: 'useTailscaleLoginProgressStore.begin', role: 'action', disposition: act(STORE_TS_PROGRESS), note: '新登录请求先登记 attemptId，旧 URL 和终态不能覆盖新请求。' },
  { id: 'useTailscaleLoginProgressStore.apply', role: 'action', disposition: act(STORE_TS_PROGRESS), note: '状态帧按当前 attemptId 接受，取消和授权都写同一 store。' },
  // 🔴 2026-09-06 改判：上一版写的是 `platform-absent`，锚指移动端设置页那句「隐私锁缺席」的文案。
  // 那不是一条平台判定，是一句我们自己写的话 —— Android 上隐私锁并非不可实现（BiometricPrompt +
  // Activity 生命周期就是它的落点），拿不出「平台上没有这个对象」的证据就不许用那个码。
  // 同屏的 `field:autoPrivacyMode` 早就按 `absent` 记着债，两条本来就该是同一个判定。
  // 恒 false 今天确实与界面自洽，但那是「这条腿没做」的自洽，不是「这个对象不存在」。
  { id: 'useAppStore.privacyMode', role: 'state', disposition: fed({ file: 'ui/src/mobile/app-wiring.ts', mustContain: 'onEnterPrivacyMode' }), note: '隐私模式。2026-09-06 批 5 接上写入方：闲置计时 `armIdlePrivacyLock` 翻真值、`onEnterPrivacyMode`/`onExitPrivacyMode` 两条订阅收敛，冷启动还会按「开关开着且已设密码」补锁。上一版这里写着「移动端没有任何写入方」——那句话现在是假的。' },
  {
    id: 'useAppStore.invalidNodes',
    role: 'state',
    disposition: fed(INVALID_NODES_FED),
    note: '🔴 本表落地当天抓到的那条活的，2026-09-13（批 18）接上写入方。此前节点屏读它、还拿它算 `invalidNodeIndex`，而移动端**没有任何写入方**（`api.proxy.onInvalidNodes` 登记着不接，理由写的是「没人读」这句假话）⇒ 那一格恒空，「这个节点已失效」永远不会亮。恒初值在这里**不是**对的答案：核确实会报无效节点，只是没人接。逐字同 W-23 的形状。',
  },

  /* ── app-store：写腿（读到的是函数本身）──────────────────────────────── */
  { id: 'useAppStore.loadConfig', role: 'action', disposition: act(STORE_APP), note: '重拉配置（冷启动一次 + 广播重拉；非 force 会被在飞的那次合并）。' },
  { id: 'useAppStore.saveConfig', role: 'action', disposition: act(STORE_APP), note: '落盘配置。写完由 `event:configChanged` 广播回来，两条腿收敛到同一份镜像。' },
  { id: 'useAppStore.mutateConfigEntities', role: 'action', disposition: act(STORE_APP), note: '按实体路径改配置（规则屏那批增删改走它，暂存开着时先落暂存表）。' },
  { id: 'useAppStore.updateProxyMode', role: 'action', disposition: act(STORE_APP), note: '改分流模式（首页模式芯片与规则屏「切回智能」共用这一条）。' },
  { id: 'useAppStore.startProxy', role: 'action', disposition: act(STORE_APP), note: '起核。它**故意不写** `proxyStatus` —— 托盘等别的入口也能启停，只有走事件才让多处收敛。' },
  { id: 'useAppStore.stopProxy', role: 'action', disposition: act(STORE_APP), note: '停核。同起核：不写连接态，等 `onStopped` 事件把真值推回来。' },
  { id: 'useAppStore.switchServer', role: 'action', disposition: act(STORE_APP), note: '换出口节点（首页选择面与节点屏整行主动作共用这一条）。' },
  { id: 'useAppStore.setUnlock', role: 'action', disposition: act(STORE_APP), note: '检测成功或失败后结束进行态。' },
  { id: 'useAppStore.setUnlockProgress', role: 'action', disposition: act(STORE_APP), note: '应用级 progress 事件逐个更新服务徽章。' },
  { id: 'useAppStore.resetUnlock', role: 'action', disposition: act(STORE_APP), note: 'Rust 失效事件确认已停核或出口无效时复位检测态。' },
  { id: 'useAppStore.setIpInfo', role: 'action', disposition: act(STORE_APP), note: '写出口 IP 探测结果（首页自持那条腿的落库口）。' },
  { id: 'useAppStore.setPendingChanges', role: 'action', disposition: act(STORE_APP), note: '写待应用差集（PUSH 帧与 pull 兜底两个入口共用同一份归一化）。' },
  { id: 'useAppStore.setInvalidNodes', role: 'action', disposition: act(STORE_APP), note: '写启动 gate 剔除的无效节点表（批 18 接上的那条订阅的落点）。它出现在本表上这件事本身就是判据的一部分：`app-wiring.ts` 那条写调用刻意不走本地的 `store()` 速记，否则写入方在本表的取材面上是隐形的，摘掉订阅不会有任何东西红。' },
  { id: 'useAppStore.beginUnlockCheck', role: 'action', disposition: act(STORE_APP), note: '标记解锁检测开始（进行态要能看见，否则用户会以为没反应再点一次）。' },
  { id: 'useAppStore.applyUnlockSnapshot', role: 'action', disposition: act(STORE_APP), note: '写解锁检测终态（应用级事件与手动 `unlockApi.run` 返回值共用）。' },
  { id: 'useAppStore.applyTailscaleStateExists', role: 'action', disposition: act(STORE_APP), note: '写「登录过没」的挂载兜底结果 —— 代理没跑的整段时间里，角标除它之外只剩 localStorage 一条来源。' },
  /* ── 2026-09-13：`resolveStoreAliases` 把 `app-wiring.ts` 里经 `store()` 别名的读点还原之后，
     下面四条**第一次**出现在这张面上。它们一直都在（十余条应用级订阅的主流写法就是 `store().<写腿>`），
     只是取材面看不见 —— 「有一格状态没人问过谁写它」正是本表要抓的那一档，而它们自己躲在写法后面。 */
  { id: 'useAppStore.refreshProxyStatus', role: 'action', disposition: act(STORE_APP), note: '重拉连接态。`startProxy`/`stopProxy` **故意不写** `proxyStatus`（`app-store.ts:396`）—— 托盘等别的入口也能启停，只有走事件才让多处收敛，故这条重拉腿是连接态唯一的落库口。' },
  { id: 'useAppStore.setPrivacyMode', role: 'action', disposition: act(STORE_APP), note: '写隐私锁开关态。挂载时从后端回读一次（Android 会非自愿回收进程，进程内 `AtomicBool` 不足以还原），此后由进入/退出两条事件维持。' },
  { id: 'useAppStore.setTailscaleStatus', role: 'action', disposition: act(STORE_APP), note: '写 Tailscale 原始状态帧。整帧落库是刻意的：`deriveTsExitWarning` 要的 `peers` / `backendState` 都在里面，丢了它「选中的出口设备在线但没广告出口」这类形态在产品里永不可达。' },
  { id: 'useAppStore.setTailscaleLoginState', role: 'action', disposition: act(STORE_APP), note: '写折叠后的登录态。写的值先过 `isDefinitiveTsLoginFrame` 那道判决门（核启动早期的 NoState/Stopped 帧会把 loggedIn 折成 false，无条件写下去会经双写穿进 localStorage 缓存，下次冷启动显示「需登录」）。' },
  { id: 'useAppStore.setTailscaleAuthUrl', role: 'action', disposition: act(STORE_APP), note: '写这台节点的 Tailscale 授权链接。移动端登录面板（`forms/TsLoginPanel.tsx`）两个入口共用它：`tailscale_up` 那次返回值里带 `authUrl` 时直接写，以及后到的 `onTailscaleAuth` 事件补写。' },
  { id: 'useAppStore.setTailscaleLoginInitiated', role: 'action', disposition: act(STORE_APP), note: '写「这台节点的登录流程已经发起」。它决定角标画成「登录中」还是「未登录」—— 拿到授权链接或用户放弃时由同一块面板落回 false。' },
  { id: 'useAppStore.retainServerIds', role: 'action', disposition: act(STORE_APP), note: '配置对账时驱逐已删节点的派生缓存（长会话里这些表只增不减就是泄漏）。' },

  /* ── 延迟 ───────────────────────────────────────────────────────────── */
  { id: 'useLatencyStore.latencyMap', role: 'state', disposition: fed({ file: 'ui/src/mobile/use-mobile-speed-test.ts', mustContain: 'applyLatencyResult(event.serverId, event.latency)' }), note: '窗口 coordinator 接主动带 runId 结果与 passive RTT；旧轮次不写延迟，invoke 仅补遗漏，切页仍继续。' },
  { id: 'useLatencyStore.testedAt', role: 'state', disposition: fed({ file: 'ui/src/mobile/use-mobile-speed-test.ts', mustContain: 'applyLatencyResult(event.serverId, event.latency)' }), note: '测速时刻（陈旧判定靠它，陈旧要画成「陈旧」而不是一个数）。' },
  { id: 'useLatencyStore.applyLatencyResult', role: 'action', disposition: act(STORE_LATENCY), note: '已接受主动 RESULT 或无 ID passive RTT 的单项延迟写入口。' },
  { id: 'useLatencyStore.applyLatencyResults', role: 'action', disposition: act(STORE_LATENCY), note: '延迟落库腿（一次测速的结果整批写进去）。' },
  { id: 'useLatencyStore.retainServerIds', role: 'action', disposition: act(STORE_LATENCY), note: '延迟表的对账驱逐（节点删了它的延迟也要跟着走）。' },

  /* ── 暂存（待应用条）─────────────────────────────────────────────────── */
  { id: 'useStagedConfigStore.entries', role: 'state', disposition: fed({ file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'stage({' }), note: '暂存条目。写入方是各屏那些 `stage({ … })` 调用。' },
  { id: 'useStagedConfigStore.saveStatus', role: 'state', disposition: fed(STAGED_FED), note: '保存态（保存失败时条上要说话，静默失败是本仓修过的形态）。' },
  { id: 'useStagedConfigStore.conflict', role: 'state', disposition: fed(STAGED_FED), note: '保存冲突（盘上被别处改过）。冲突时 save 把 saveStatus 退回 idle 并 return false —— 不报冲突就等于「什么都没发生」。' },
  { id: 'useStagedConfigStore.stage', role: 'action', disposition: act(STORE_STAGED), note: '压一条暂存（Class B 配置命中暂存腿时只记意图，真正生效留到 Apply）。' },
  { id: 'useStagedConfigStore.save', role: 'action', disposition: act(STORE_STAGED), note: '保存暂存但不重启核（冲突时把 saveStatus 退回 idle 并返回 false）。' },
  { id: 'useStagedConfigStore.applyNow', role: 'action', disposition: act(STORE_STAGED), note: '保存并立即应用，可能连带重启核 —— 待应用条上那颗主动作。' },
  { id: 'useStagedConfigStore.reset', role: 'action', disposition: act(STORE_STAGED), note: '丢弃全部暂存（二次确认，条上那颗「重置」）。' },
  { id: 'useStagedConfigStore.dismissConflict', role: 'action', disposition: act(STORE_STAGED), note: '关掉冲突提示（关掉不等于解决，条上另有逐条解决那一路）。' },
  { id: 'useStagedConfigStore.resolveConflict', role: 'action', disposition: act(STORE_STAGED), note: '按条解决冲突（用暂存值覆盖盘上被别处改过的那几条）。' },

  /* ── VPN / 订阅进度 / 组网缓存 ───────────────────────────────────────── */
  { id: 'useVpnStatusStore.openVpn', role: 'state', disposition: fed(VPN_FED), note: 'OpenVPN 每个节点的状态帧。节点屏的 `deriveMeshTunnelHealth` 读的就是它。' },
  { id: 'useVpnStatusStore.replace', role: 'action', disposition: act(STORE_VPN), note: '整帧替换 VPN 快照。首帧水合与停核清空都走它 —— 留着旧帧会让「核已停」那一屏还画着上一秒的隧道健康。' },
  { id: 'useVpnStatusStore.setOpenVpn', role: 'action', disposition: act(STORE_VPN), note: '增量写一条 OpenVPN 状态（事件腿的落点，与整帧替换是两个入口）。' },
  { id: 'useVpnStatusStore.retainServerIds', role: 'action', disposition: act(STORE_VPN), note: 'VPN 快照的对账驱逐（节点删了它的隧道状态也要跟着走）。' },
  { id: 'useSubscriptionProgressStore.progress', role: 'state', disposition: fed(SUB_PROGRESS_FED), note: '🔴 W-24 的那一格：订阅更新进度。它由后台 scheduler 自跑，**没有任何前端调用点可挂** ⇒ 不订阅事件就永远是空表，节点屏的订阅进度恒不出现。' },
  { id: 'useSubscriptionProgressStore.retainSubscriptionIds', role: 'action', disposition: act(STORE_SUBPROG), note: '订阅进度表的对账驱逐（订阅删了它的失败进度也要跟着走）。' },
  { id: 'useTailscaleLoginCacheStore.retainServerIds', role: 'action', disposition: act(STORE_TSCACHE), note: '持久登录缓存的对账驱逐（代理关着时唯一的登录态来源，误清会让角标退回「未登录」）。' },

  /* ── 日志脱敏 / 弹窗栈 ───────────────────────────────────────────────── */
  { id: 'useLogRedactStore.redactLogs', role: 'state', disposition: fed({ file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: 'onSelect: toggleRedactLogs' }), note: '日志脱敏开关。写入方就在同屏那颗开关上。' },
  { id: 'useLogRedactStore.toggleRedactLogs', role: 'action', disposition: act(STORE_REDACT), note: '翻转脱敏开关（与隐私模式取或：任一为真就脱敏）。' },
  { id: 'useDialogStore.stack', role: 'state', disposition: fed({ file: 'ui/src/components/screens/settings/use-config.ts', mustContain: 'function promptAppRestart(' }), note: '弹窗栈。移动端没有弹窗宿主，但共享的 `use-config` 写腿会往栈上压一条「要重启才生效」的 confirm —— 设置屏把它截下来落成行内提示。写入方在共享模块里，不在 `mobile/` 下，这正是**取材面按读点取、锚可以指仓里任何一处**的理由。' },
  { id: 'useDialogStore.closeInstance', role: 'action', disposition: act(STORE_DIALOG), note: '关掉栈上那条（不关的话下一轮 effect 会把同一条消息再记一次）。' },
];
