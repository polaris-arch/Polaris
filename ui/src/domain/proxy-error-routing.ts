/**
 * `event:proxyError` 的**分腿路由** —— 桌面与移动**共用同一张处置表**。
 *
 * # 为什么它住在 `domain/` 而不是 `App.tsx`（2026-09-06 从 `App.tsx` 整段迁出）
 *
 * 迁出前它是 `App.tsx` 的导出函数，而 `App.tsx` 的模块图带着整条桌面 CSS 层叠链
 * （`AppShell` → `styles/index.css` …）⇒ 移动入口结构上引不进来（`mobile/mobile-entry.test.ts`
 * 的契约 A1 是**集合恰等**，引一次当场红）。于是移动端要么没有这条腿（后端把码发出来、
 * 前端一个字不显，正是本轮在修的那类假接线），要么照抄一份 —— 而照抄意味着
 * 「哪个码算核已停、哪个码只是走错路」从此有两份会各自漂移的答案。
 *
 * 整段迁到这里之后两个入口读同一份：桌面 `App.tsx` 与移动 `mobile/app-wiring.ts` 各自
 * 在 `api.proxy.onError` 的订阅体里调它。判据面（`contracts/proxy-error-key-coverage.test.ts`
 * 的 G3）随之从 `App.tsx` 改钉本文件，并额外正面断言**两个入口都真的接着它** ——
 * 一个被抽出来却没人调的纯函数，单测再全绿也没有任何生产意义。
 *
 * # Android 上有几条分腿结构上不可达，仍然保留
 *
 * `HELPER_*`（提权助手）/ `ROOT_ORPHAN_BLOCKED`（root 孤儿核）/ `SYSTEM_PROXY_FAILED`
 * （移动端恒 TUN，没有系统代理这个对象）在 Android 上不会被发出来。**不为此在本文件里分平台**：
 * 这张表回答的是「后端发了这个码该怎么处置」，那个答案与平台无关；按平台裁剪只会让两端的处置
 * 重新分叉，而不可达的分腿在运行期的代价恰好是零。同一条取向在
 * `mobile/home/MobileHomeScreen.tsx` 的 `MOBILE_TAKEOVER` 处已登记过一次
 * （`proxy-degraded` 那一态在移动端同样结构上不可达，呈现层照样保留）。
 */

import { toast } from '../lib/error-handler';
import { notifyDesktop } from '../lib/desktop-notify';
import { isProxyStartClaimed } from '../lib/proxy-start-claim';
import { proxyErrorText } from './proxy-error-text';

/**
 * `event:proxyError` 分腿路由——从 onError 订阅体抽出为具名函数，供 `.test.ts` 直接断言副作用调用
 * （toast/notifyDesktop/refreshProxyStatus 是否被真调），不必整棵渲染 App。
 *
 * # 文案一律经 `proxyErrorText`，**不得**直接用 `data.message`
 *
 * 载荷里的 `message` 是 Rust 侧写死的中文串。此前六处都写成 `data.message || t(key)`，而
 * `emit_proxy_error(message, error_code)` 两参皆非可选 ⇒ `||` 右边永远短路不到，那些 key 是死键、
 * 俄语/波斯语用户在最高频的错误路径上看到中文。现改为只按稳定 `errorCode` 取 locale，
 * 诊断 `message` 只进日志（见 `domain/proxy-error-text.ts` 头注）。**本函数每条腿的分类判据
 * 仍是 `errorCode`，文案不再参与分腿** —— 两者混在一句 `||` 里正是上面那个缺陷的形状。
 *
 * - **崩溃腿**（PROCESS_EXITED / AUTO_RESTART_FAILED）：核不再运行 → 刷连接态 + 「已断开」toast +
 *   桌面通知（C8，对齐 上游 index.ts:1900 notifyUser(proxyError*)：崩溃常发生在窗口已收进托盘/隐藏时，
 *   应用内 toast 看不到，系统通知是唯一送达路径；desktopNotifications 关/无权限则 notifyDesktop 内部静默不发）。
 * - **出口误导腿**（SYSTEM_PROXY_FAILED / EXIT_MISMATCH）：核仍在运行，只是流量未按预期经代理 ——
 *   **不**刷连接态、**不**用「已断开」文案（都会把活核错误地标成已停）。报 `home.proxyMisdirected`
 *   警告 toast；同样发桌面通知——窗口常被收进托盘，用户可能正以为在用代理、实则明文直连，
 *   应用内 toast 送不到。
 * - **能力降级腿**（RULE_RESOURCES_MISSING）：核仍在运行，只是引用缺失 `.srs` 的分流规则整段被跳过
 *   → 智能分流失效。后端剪枝是 fail-closed：剪枝把 `final` 压成 `direct` 时会回退成用户出口
 *   （`builder/route.rs` T2 fail-safe），即**未命中规则的流量兜底走代理，不会因剪枝静默转直连**。
 *   但这**不等于「全量经代理」**，两个例外仍在：① 不依赖 rule_set 的显式 direct 规则不受剪枝影响
 *   （bypass-LAN 的 ip_cidr、ICMP 兜底、DoH 泄漏拦截、系统进程放行等）；② 组网出口回退时
 *   （`mesh_selected_exit_falls_back_to_direct` 为真）用户出口本身就是 `direct`，fail-safe 回退后
 *   `final` 仍是 `direct`。与出口误导腿同形（**不**刷连接态、warning toast + 桌面通知——同样常发生在
 *   窗口已收进托盘时），但文案指向「规则资源」页下载而非直连风险：两者的用户下一步动作完全不同，
 *   共用一条文案等于把可操作指引冲掉。
 * - **自动换节点落空腿**（AUTO_SWITCH_NEEDS_RESTART）：核仍在运行 —— 当前节点已连不上、自动换节点
 *   真的触发了、候选也规划出来了，但可用节点都要整核重启才能切过去 ⇒ 这轮实际什么也没做。
 *   与出口误导腿同形（**不**刷连接态、warning toast + 桌面通知），但文案指向用户能做的两件事：
 *   手动换一个节点，或重启代理让新配置入核。**桌面通知在这条腿上尤其必要**：整件事全程后台发生，
 *   用户对此完全无感，只会感到「网怎么突然不通了」。后端按世代锁存（`AutoSwitchMachine`），
 *   同一个运行核最多发一次、核重启后自然复位，故此处不另做去重。
 * - **root 孤儿阻断腿**（ROOT_ORPHAN_BLOCKED）：残留的 root 孤儿核用户态杀不动、独占 `cache.db`，
 *   任何模式都起不来 → 核**未起**（终态），故必须 `refreshProxyStatus`。与 HELPER 两码同为后端
 *   **双出口**，同样纳入认领闸门去重（此前**完全不路由**，托盘/自动连接等无人 await 的入口静默丢弃，
 *   有人 await 时又落到 HomeScreen 的「检查服务器配置」通用兜底——对残留进程这码是错的指引）。
 *   用户文案走 `errors.rootOrphanBlocked`；具体 pid 与 helper/OS 原文留日志诊断。
 *   toast.error + 桌面通知（窗口常已收进托盘）。
 * - **随包内核不可执行腿**（CORE_NOT_EXECUTABLE）：起核前即被拒绝，核**未起**（终态）。与上一条同为
 *   后端**双出口**、同一套处置（刷连接态 + 认领闸门去重 + toast.error + 桌面通知）；文案走
 *   `errors.coreNotExecutable`（下一步是重新安装应用，与服务器配置无关）。
 * - **提权助手内核不配套腿**（HELPER_CORE_MISMATCH）：确认不了提权助手里的内核就是本应用配套的
 *   那一份 → 拒绝以 TUN 起核；或起核后自证发现助手实跑的版本不同 → 已停核。两种都是核**不在跑**
 *   （终态），处置同上一条；文案走 `errors.helperCoreMismatch`（下一步是到「设置 › Helper」重装或
 *   升级助手）。起核后停核那一种没有任何 await 腿在等，只有这里会报。
 * - 其余码（如 STARTUP_FAILED）：忽略。它必然伴随某次 proxy.start 的 reject，发起方（Home 连接按钮）
 *   自己会 toast，此处重报 = 同一次失败弹两遍。
 */
export function handleProxyErrorEvent(
  data: { errorCode?: string; message?: string },
  deps: { t: (key: string, fallback?: string) => string; refreshProxyStatus: () => Promise<void> }
): void {
  const { t, refreshProxyStatus } = deps;
  if (data.errorCode === 'ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED') {
    void refreshProxyStatus();
    if (isProxyStartClaimed()) return;
    toast.error(proxyErrorText(data, t));
    return;
  }
  if (data.errorCode === 'PROCESS_EXITED' || data.errorCode === 'AUTO_RESTART_FAILED') {
    void refreshProxyStatus();
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(t('notify.proxyError.title'), t('notify.proxyError.body'));
    return;
  }
  if (data.errorCode === 'SYSTEM_PROXY_FAILED' || data.errorCode === 'EXIT_MISMATCH') {
    toast.warning(proxyErrorText(data, t));
    void notifyDesktop(t('notify.proxyMisdirected.title'), t('notify.proxyMisdirected.body'));
    return;
  }
  if (data.errorCode === 'RULE_RESOURCES_MISSING') {
    toast.warning(proxyErrorText(data, t));
    void notifyDesktop(
      t('notify.ruleResourcesMissing.title'),
      t('notify.ruleResourcesMissing.body')
    );
    return;
  }
  // 网络场景规则降级（后端 `set_nonfatal_error`，核仍在跑）：部分场景规则本次没生成或可能永不命中。
  // **不**刷连接态；warning toast 指向「路由」页检查场景。不发桌面通知：它只在起核那一刻产生，
  // 用户刚做完「连接 / 保存场景」这一步，应用内 toast 送得到。
  if (data.errorCode === 'NETWORK_PROFILE_RULES_PRUNED') {
    toast.warning(proxyErrorText(data, t));
    return;
  }
  if (data.errorCode === 'SYSTEM_DNS_TAKEOVER_FAILED') {
    toast.warning(proxyErrorText(data, t));
    void notifyDesktop(
      t('notify.systemDnsTakeoverFailed.title'),
      t('notify.systemDnsTakeoverFailed.body')
    );
    return;
  }
  // 自动换节点落空（有候选节点要整核重启才能切过去）：后端走 `set_nonfatal_error`，核仍在跑
  // ⇒ **不**刷连接态、**不**用「已断开」文案（都会把活核错误地标成已停）。桌面通知不可省：
  // 这条腿整件事都发生在后台，窗口常已收进托盘，应用内 toast 送不到，而用户此刻正卡在一条
  // 连不上的代理里。后端按世代锁存，同一个运行核最多发一次，故这里不另做去重。
  //
  // 文案里**没有**「或重启代理」：走到上报点时后端已确证 D 与 R 的选中节点相同，同配置重启只会
  // 世代 +1 → 锁存复位 → 同情形再报一次，用户照做就进循环。
  if (data.errorCode === 'AUTO_SWITCH_NEEDS_RESTART') {
    toast.warning(proxyErrorText(data, t));
    void notifyDesktop(
      t('notify.autoSwitchNeedsRestart.title'),
      t('notify.autoSwitchNeedsRestart.body')
    );
    return;
  }
  if (data.errorCode === 'OUTBOUND_INTERFACE_UNAVAILABLE') {
    // 起核腿是终态，热切换腿则保留旧核；统一回拉一次真实状态，避免前端猜当前属于哪一腿。
    void refreshProxyStatus();
    if (isProxyStartClaimed()) return;
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(
      t('notify.outboundInterfaceUnavailable.title'),
      t('notify.outboundInterfaceUnavailable.body')
    );
    return;
  }
  // TUN 出口未夺到（其他 VPN 占默认路由）：核已被硬闸 `kill_core`、**未起**（终态），故必须
  // `refreshProxyStatus` —— 否则 UI 停在假「已连接」。同 HELPER 两码是后端**双出口**（既 emit 事件、
  // 又让 start reject），故认领期内让位给 await 腿（Home 连接按钮），无人认领（托盘/自动连接）时照常报。
  if (data.errorCode === 'TUN_ROUTE_NOT_CAPTURED') {
    void refreshProxyStatus();
    if (isProxyStartClaimed()) return;
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(
      t('notify.tunRouteNotCaptured.title'),
      t('notify.tunRouteNotCaptured.body')
    );
    return;
  }
  // TUN 网卡从未建出来（#327，Windows）：核已被逐腿硬闸 `kill_core`、**未起**（终态），处置形态与上一支
  // 完全相同（刷连接态 + 认领去重 + 报）。**文案与通知刻意分开**：上一支叫用户「断开其他 VPN」，本支
  // 叫用户「查 wintun 驱动是否被安全软件拦截」——共用一条会把两边的下一步动作都指错。
  if (data.errorCode === 'TUN_ADAPTER_MISSING') {
    void refreshProxyStatus();
    if (isProxyStartClaimed()) return;
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(t('notify.tunAdapterMissing.title'), t('notify.tunAdapterMissing.body'));
    return;
  }
  // TUN 提权门两码：核**未起**（终态），故必须 `refreshProxyStatus` —— 与崩溃腿同理，
  // 不刷则 UI 停在假「已连接」。这两条腿的发起方常常**没人在 await**：托盘切档位 / 启动自动连接 /
  // switchMode 去抖重启都不经 Home 的连接按钮，此前后端已发码、前端整条 else 落空 = 静默丢弃
  // （真机反馈「点了没反应」的直接成因）。
  if (data.errorCode === 'HELPER_GATE_ABORTED' || data.errorCode === 'HELPER_NOT_INSTALLED') {
    // 刷连接态**在认领判定之外**：核未起是终态，与「谁负责提示」无关，两条路径都得刷。
    // 挪进下面的 return 之后 = 认领期内 UI 停在假「已连接」（await 腿并不刷）。
    void refreshProxyStatus();

    // 这两码是后端**双出口**（既 emit 事件、又让 `api.proxy.start` reject），故须与 await 腿去重 ——
    // 同本文件 §STARTUP_FAILED 写明的既有约定「两处都报 = 同一次失败弹两遍」。但不能照搬那条的
    // 「整条忽略」：忽略会把上面说的「没人 await 的入口」重新变成静默。故由发起方显式认领
    // （Home 连接按钮把 startProxy 包进 `withProxyStartClaim`）：认领期内让位给 await 腿，
    // 无人认领（托盘/自动连接）时照常报。认领期为何带宽限尾巴见 proxy-start-claim.ts 头注。
    if (isProxyStartClaimed()) return;

    // **两码分开报，不合并**（同 §RULE_RESOURCES_MISSING 的分家理由）：
    // - GATE_ABORTED = 用户刚亲口拒绝安装 → 中性 info，**不发桌面通知**（自己点的取消，再推一条是噪音）；
    // - NOT_INSTALLED = 装不上/被抑制 → error + 桌面通知（窗口可能已收进托盘，用户正等着它连上）。
    if (data.errorCode === 'HELPER_GATE_ABORTED') {
      toast.info(proxyErrorText(data, t));
      return;
    }
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(t('notify.helperNotInstalled.title'), t('notify.helperNotInstalled.body'));
    return;
  }

  // root 孤儿阻断起核：核**未起**（终态，与 HELPER 两码同理），必须 refreshProxyStatus。
  // 与 HELPER 两码同为后端**双出口**（emit 事件 + 让 `api.proxy.start` reject），同样纳入认领闸门
  // 去重（见 proxy-start-claim.ts 头注）：认领期内让位给 Home 连接按钮的 await 腿，无人认领
  // （托盘/自动连接/switchMode 去抖重启）时照常报——此前这里**没有这一支**，两类入口分别是
  // 「静默丢弃」与「落到 STARTUP_FAILED 兜底的错误指引」，见本函数头注对应小节。
  if (data.errorCode === 'ROOT_ORPHAN_BLOCKED') {
    void refreshProxyStatus();
    if (isProxyStartClaimed()) return;
    // 具体 pid 留日志；可见面只给本地化的处置指引。
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(t('notify.rootOrphanBlocked.title'), t('notify.rootOrphanBlocked.body'));
    return;
  }

  // 随包内核文件缺少可执行权限：起核前即被拒绝，核**未起**（终态）⇒ 必须 refreshProxyStatus。
  // 后端同样是**双出口**（`set_error` 发事件 + `proxy_start` 把码带回让 start reject），处置与上一支
  // 逐条相同：认领期内让位给发起方的 await 腿（Home 连接按钮自己报 `errors.coreNotExecutable`），
  // 无人认领（托盘 / 启动自动连接 / switchMode 去抖重启）时由这里报。此前这里**没有这一支**：
  // 那些入口撞上它时落到函数末尾被静默丢弃，用户一个字都看不到。
  if (data.errorCode === 'CORE_NOT_EXECUTABLE') {
    void refreshProxyStatus();
    if (isProxyStartClaimed()) return;
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(
      t('notify.coreNotExecutable.title'),
      t('notify.coreNotExecutable.body')
    );
    return;
  }

  // 提权助手里的内核与本应用不配套（或确认不了）：起核前被拒绝，或起核后被自证停掉 —— 两种都是
  // 核**不在跑**（终态）⇒ 必须 refreshProxyStatus。起核前那一种是后端**双出口**，处置与上一支逐条
  // 相同（认领期内让位给发起方的 await 腿）；起核后停核那一种发生在后台，没有人 await，只有这里报。
  if (data.errorCode === 'HELPER_CORE_MISMATCH') {
    void refreshProxyStatus();
    if (isProxyStartClaimed()) return;
    toast.error(proxyErrorText(data, t));
    void notifyDesktop(
      t('notify.helperCoreMismatch.title'),
      t('notify.helperCoreMismatch.body')
    );
  }
}
