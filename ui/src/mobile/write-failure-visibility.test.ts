/**
 * 跨屏门：**移动端没有一次写失败是静默的**（IA 裁定 #14）。
 *
 * # 为什么是一道跨屏门，而不是把四套实现并成一套
 *
 * 五个移动屏各自实现了失败回显，四套机制、各自都有屏级门、且都在工作：
 * 首页 `home/write-errors.ts` 的 `createRunWrite`、设置 `settings/write-feedback.ts` 的
 * `createCommit`、节点与连接各自屏内的 `runWrite`、规则屏的 `reportWriteError`。
 * 把它们重构成一套，是拿五个屏的回归风险换一次抽象收益 —— 不做。
 *
 * 真正缺的东西是另一样：**「没有一次写失败是静默的」是一条性质，不是一种实现**，而这条性质
 * 此前没有任何一道门在守。屏级门各守各的取材面，于是：
 *  - 第六个屏落地时不带任何回显，**没有一道门会转红**（各屏的门只看自己那个目录）；
 *  - 某一屏的取材面比另一屏窄，窄的那一格谁也不知道。
 *
 * 本门只断言那条性质：登记表记 (屏, 失败回显机制)，然后用**同一套**写腿判据面扫遍
 * `src/mobile/**`，要求每一处写调用都被它那个屏的已登记机制罩住。机制长什么样是各屏自己的事。
 *
 * # 判据不许只钉「有 catch」
 *
 * 裁定 #14 原文点名：「有 `catch` 而 `catch` 里什么都不做，正是要抓的那个形态」。故除了「写腿在
 * 机制辖区内」，本门还逐个断言**机制自身的失败腿真的把错误送进了用户看得见的通道**：两个纯工厂
 * 直接拿一个必然 reject 的 op 驱动（行为对照），两处屏内 `runWrite` 与规则屏的 reporter 走源码断言。
 *
 * # 与 `lib/config-write-wiring.test.ts` 的关系
 *
 * 那道门的判据面是「**写 config** 的调用点」，本门的是「**会失败、且用户在等回执**的调用点」——
 * 后者严格更宽：`switchServer` / `startProxy` / `api.connections.close` / 剪贴板都会失败、都要回显，
 * 但一个字节的 config 都不写。移动端「节点」屏在那张登记表里**一行都没有**，直接复用它会让
 * 五个屏里的两个在本门下无门可守。故这里自带判据面，并对齐各屏门已经验证过的形态。
 *
 * # 本轮（2026-09-05）补的两处，各修一类**不同的**失明
 *
 *  1. **判据面漏了 `<域>Api.<方法>(` 这个门面**（见 `WRITE_CALLS` 头注）。`systemApi.openExternal`
 *     里 `api` 前后都是词字符，`\bapi` 的词边界不成立 ⇒ 关于页四条外链从来没进过取材面。
 *     旧面对照：把那处锚点摘回 `console.error`（裁定 #14 原文点名的形态）、`tsc` rc=0，旧面 13/13 全绿。
 *  2. **不是「一次调用」的那一条腿另立断言**（下面 ④）。`useConfig` 的「需重启 App」确认弹窗走
 *     `dialog-store`，而 `DialogHost` 只挂在桌面 `AppShell` 上 —— 写**成功**了，但「没生效」这句话
 *     在移动端没有宿主可说。它不是写调用，任何基于调用点的取材面都看不见它。
 *
 * # 仍未收口的一条（登记，不假装覆盖）
 *
 * **订阅刷新的 detail 被 toast 吞掉**：`domain/subscription-refresh.ts` 的
 * `refreshSubscriptionWithToast` 只返 boolean，后端给的 `subscriptionErrorDetail` 留在了一个移动端
 * 看不见的 toast 里；唯一的移动端消费点是 `mobile/nodes/MobileNodesScreen.tsx` 的 `refreshSub`。
 * 那处**写腿本身在本门辖区内**（`refreshSubscriptionWithToast(` 在 `WRITE_CALLS` 里、且落在
 * `runWrite` 的实参区间内 ⇒ ② 绿），本门守的是「有没有回显」，守不到「回显里丢了多少信息」——
 * 后者要改的是那个 domain 函数的返回契约，不在本门的判据类别里。
 */
import { describe, it, expect, vi } from 'vitest';
import { api } from '@/ipc';
import { runMobileSpeedTest, useMobileSpeedTestStore } from './use-mobile-speed-test';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import * as ts from '@/test/ts-compiler';
import { aliasWriteLegs, ALIAS_FIXTURE, ALIAS_FIXTURE_LEGS } from '@/test/write-leg-aliases';

import type { TFunction } from 'i18next';
import { createRunWrite } from './home/write-errors';
import type { WriteErrors as HomeWriteErrors } from './home/view-model';
import { createCommit, deferredNoticeOf } from './settings/write-feedback';
import type { WriteErrors as SettingsWriteErrors } from './settings/write-feedback';

const SRC = fileURLToPath(new URL('..', import.meta.url));
const MOBILE = join(SRC, 'mobile');

/** 去注释但保留行号与偏移（注释体换成等量空白）—— 与 `lib/config-write-wiring.test.ts` 同一口径。 */
function code(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/(^|[^:])\/\/.*$/gm, (m, p1: string) => p1 + ' '.repeat(m.length - p1.length));
}

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) sourceFiles(p, out);
    // 同 `contracts/test-only-modules.ts` 头注定的那一个概念：`.test-support.` 也不进产物。
    // 四屏对差登记表（`*/absence-register.test-support.ts`）里那些 `update({ … })` 是**锚文本**，
    // 不是写腿 —— 它们连一次 IPC 都发不出去，纳进面里只会逼人给一张数据表登记「失败回显机制」。
    else if (/\.tsx?$/.test(e.name) && !IS_TEST_ONLY_MODULE.test(e.name)) out.push(p);
  }
  return out;
}

const rel = (p: string): string => p.slice(SRC.length).split(/[\\/]/).join('/');

// ════════════════════════════════════════════════════════════════════════════
// 判据面：会失败、且用户在等回执的调用
// ════════════════════════════════════════════════════════════════════════════

/**
 * 写腿形态。**容忍换行的点号**（`api.logs\n  .search(…)` 与 `api.logs.search(…)` 是同一次调用），
 * 否则一次重排就能静默绕过整道门。
 *
 * # 🔴 `<域>Api.<方法>(` 这一条是本轮补的，它此前是判据面上的一个洞
 *
 * 本仓的 IPC 有**两个**门面：聚合的 `api.<域>.<方法>` 与各域自己的具名导出
 * （`systemApi` / `versionApi` / `vpnApi` / `configApi` / `windowApi`，`ipc/api-client.ts` 两者都导出）。
 * 上面第一条只认前者：`systemApi.openExternal` 里 `api` 前后都是词字符，`\bapi` 的词边界压根不成立
 * ⇒ **整类调用点从来没进过取材面**。关于页四条外链走的正是它，于是「点了 LICENSE 什么都没发生」
 * 这条腿在本门下无门可守（旧面对照：把那处锚点摘成 `console.error`，旧面 13/13 全绿）。
 *
 * `unlockApi` 那一条因此变成本条的子集 —— **不删**：删了就等于用一次重写把一条已验过的判据换掉，
 * 而两条同时在只是同一处写腿被匹配两次，下面按偏移去重本就要处理这件事。
 */
const WRITE_CALLS: readonly RegExp[] = [
  /\bapi\s*\.\s*[a-zA-Z]+\s*\.\s*[a-zA-Z]+(?=\s*\()/g,
  /\bunlockApi\s*\.\s*[a-zA-Z]+(?=\s*\()/g,
  /\b[a-zA-Z]+Api\s*\.\s*[a-zA-Z]+(?=\s*\()/g,
  /(?<![.\w$])switchServer\s*\(/g,
  /(?<![.\w$])startProxy\s*\(/g,
  /(?<![.\w$])stopProxy\s*\(/g,
  /(?<![.\w$])saveConfig\s*\(/g,
  /(?<![.\w$])mutateConfigEntities\s*\(/g,
  /(?<![.\w$])refreshSubscriptionWithToast\s*\(/g,
  /(?<![.\w$])runMobileSpeedTest\s*\(/g,
  /\bnavigator\s*\.\s*clipboard\s*\.\s*[a-zA-Z]+(?=\s*\()/g,
];

/**
 * 本门关心的 store 写方法（喂 `aliasWriteLegs` 的选择器别名那一支）。
 *
 * 比配置门的集合宽：`switchServer` / `startProxy` / `stopProxy` 一个字节的 config 都不写，但它们
 * 会失败、且用户在等回执 —— 那正是本门的判据面定义。
 */
const STORE_WRITES = [
  'saveConfig',
  'mutateConfigEntities',
  'switchServer',
  'startProxy',
  'stopProxy',
] as const;

/**
 * `useConfig()` 漏斗腿 + 全部绑定别名腿。
 *
 * 别名解析与 `lib/config-write-wiring.test.ts` **共用同一份实现**（`@/test/write-leg-aliases`）：
 * 两道门各抄一份的下场是口径漂移，而漂移后没有任何东西会红 —— 协调者的 P2 变异
 * （`const h = useConfig(); h.update({…})` 脱开 `runWrite`）当时就是三道门一起绿的。
 */
const updateLegs = (src: string): RegExp[] => [
  /(?<![.\w$])update\s*\(\s*\{/g,
  ...aliasWriteLegs(src, STORE_WRITES),
];

/**
 * **读腿白名单**：订阅、水合、状态回读。它们不是「用户按下了一个按钮」，失败没有可归因的控件，
 * 故不占用户的行内通道（留 console 证据）。按**具体方法名**登记而不是整片放行某个域：
 * `api.stats.*` 里的 `clearClosed` 是真的写，整片放行会给它开一条绕过唯一出口的路。
 * 下面有一条僵尸自检钉着这张表 —— 登记了一个已经没人调的读，也红。
 */
const READ_CALLS: readonly string[] = [
  'api.ipInfo.onUpdated',
  'api.ipInfo.peek',
  'unlockApi.onProgress',
  'unlockApi.onUpdated',
  'unlockApi.onInvalidated',
  'unlockApi.get',
  'api.stats.subscribe',
  'api.stats.unsubscribe',
  'api.stats.onStatsUpdated',
  'api.stats.onConnectionsDetail',
  'api.stats.onConnectionsClosed',
  'api.stats.onConnectionsAggregate',
  'api.logs.get',
  'api.logs.search',
  'api.logs.unsubscribe',
  'api.logs.runtimeLevel',
  'api.logs.diagnosticState',
  /*
   * W26 前遗留的无界 `singbox.log` 的**只读探测**（批 13，`mobile/connections/`）。
   * 与 `api.logs.diagnosticState` 同类：进日志段时问一次「有没有这个文件」，
   * 没有用户在等回执、也没有可归因的控件 —— 读不到的正确表现是**那块提示不出现**
   * （而不是画一块说「没有旧日志」的提示，那会把 1.4 GB 的历史资产说成不存在）。
   * 真正「用户按下按钮、在等回执」的那两条是 `archiveLegacy` / `deleteLegacy`，
   * 它们**不在**这张白名单里，两条都罩在连接屏的 `runWrite(` 里。
   */
  'api.logs.legacyInfo',
  'api.logs.onReceivedBatchReady',
  // 配置广播的订阅登记（`mobile/config-sync.ts`）：它是一次**订阅**，同步返回退订闭包、
  // 不落盘、也没有用户在等回执 —— 与 `api.stats.onStatsUpdated` 同类。真正会失败的那件事是
  // 回调里的 `loadConfig(true)`（一次读），失败落 console，没有可归因的控件。
  'api.config.onChanged',
  'api.proxy.onLifecycle',
  'api.proxy.getPendingChanges',
  'api.ipInfo.get',
  'api.ruleResources.list',
  /*
   * 组网 force-route 结算报告（节点屏的「被覆盖网段」角标、规则屏的重叠角标）。
   *
   * 只读命令，**没有用户在等回执**：它不是按下某颗控件触发的，而是随 servers/规则/核起停变化
   * 自己重拉。拉不到的正确表现就是角标不出现（面板里两处都写着 `setForceRouteReport(null)`），
   * 而不是弹一条错误 —— 「这条规则有没有被组网段吃掉」这件事本身就允许答不出来。
   * 反面是**不许**退回渲染端重算：那正是本轮把判据搬进引擎要终结的形态。
   */
  'api.config.endpointForceRouteReport',
  /*
   * 外来隧道冲突报告（设置 → TUN 页那块只读报告，本批接线）。
   *
   * 与上一条同族、同理由：只读命令，挂载时拉一次，**没有用户在等回执**。拉不到的正确表现是
   * 那一块自己说「读取失败，暂时拿不到这份报告」（`TunReports.tsx` 的 `report === null` 那一支），
   * 而不是弹一条错误 —— 报告答不出来是这四态判别联合本来就允许的一档。
   * 🔴 要紧的是它**不许**被折成「无冲突」：那一支只属于 `status === 'probed'` 且 `conflicts` 为空。
   */
  'api.config.tunnelConflictReport',
  /*
   * 已装应用枚举（自定义应用表单的包名选择器，批 16）。只读命令，挂载时拉一次，
   * **没有用户在等回执**：它不是按下某颗控件触发的。读不到的正确表现是那一格自己说
   * 「读不到这台设备的已安装应用清单」（`AppAddPanel` 的 `appsFailed` 那一支）+ 表照样能提交
   * （包名是可选的一格）—— 而**不是**把整张表的红字通道占掉，那会盖住真正卡住提交的
   * 那条（geosite 必填）。
   * 🔴 要紧的是它**不许**被折成空表：空表与「这台设备上真的一个应用都没有」是两回事，
   * 后端为此刻意不返空表（`system_list_installed_apps` 一律 `success:false` + 原因）。
   */
  'api.system.listInstalledApps',
  /*
   * Taildrop 收件箱 / 发件任务的**只读**快照（批 16，`mobile/forms/TaildropPanel.tsx`）。
   * 面板打开时拉、STATUS 计数变化时重拉、脚上那颗刷新也是它 —— 失败时面板里仍落一句红字
   * （那条 catch 就在 `refresh` 里），登记在这张白名单上只是因为它们不是「用户按下按钮改了什么」。
   * 真正的五条写腿（save / delete / send / cancel / taskCancel）**不在**这张表里，
   * 五条各自罩在自己的 `try/catch { setNotice }` 内。
   */
  'api.server.taildropList',
  'api.server.taildropTasks',
  /*
   * 清未读角标。名义上是一次写，但它**没有可归因的控件**：打开面板就自动发一次，
   * 与 `api.config.onChanged` 同类。失败的正确表现是角标还在（那是真话 —— 确实没清掉），
   * 而不是在一张刚打开的面板上先弹一条错误。桌面 `TaildropDialog` 同样 `.catch(() => {})`，
   * 头注写着「失败静默：标记已读纯属体验，不应盖住真正的内容错误」。
   */
  'api.server.taildropMarkRead',
  'api.ruleResources.onProgress',
  /*
   * 全局接线层（`mobile/app-wiring.ts`，批 1 / W-22~W-24）的订阅与状态回读面。
   *
   * 全部是「登记一个监听 / 拉一次当前状态」，同 `api.config.onChanged` 那一类：**没有用户在等回执**，
   * 也没有可归因的控件可以贴错误。真正会失败又有人等的那一件，是同一份接线里的
   * `api.system.openExternal`（自动开登录页）—— 它**没有**被放进本白名单，而是罩在
   * `reportIfFails(` 里落全局 toast，见下面 FEEDBACK 表那一行。这条分界就是这批放行的边界。
   */
  'api.proxy.getStatus',
  'api.proxy.onStarted',
  'api.proxy.onStopped',
  'api.proxy.onPendingChanges',
  'api.proxy.onError',
  'api.proxy.onTailscaleStatus',
  'api.proxy.onTailscaleAuth',
  'api.proxy.onTailscaleLoginProgress',
  'api.vpn.getStatus',
  'api.vpn.onOpenVpnStatus',
  'api.subscription.onAutoUpdate',
  // 2026-09-06 复审 major 补接的两条桌面订阅（登录期出口让位 / 自动换节点）。同一类：登记监听，
  // 没有用户在等回执；它们**自己就是**回执 —— 回调体里落的是全局 toast（后端自驱改了出口，
  // 没有可归因的控件可以贴）。
  'api.proxy.onMeshLoginFallback',
  'api.proxy.onAutoNodeSwitched',
  /*
   * 批 18 接上的无效节点订阅。同一类：登记一个监听，没有用户在等回执 —— 没有任何控件按下去会
   * 触发它（后端在起核的 gate 阶段自己发）。**它自己就是回执**：回调体把整表写进
   * `store.invalidNodes`，节点屏那条常驻 `.mn-note` 就是用户看得见的那一面。
   */
  'api.proxy.onInvalidNodes',
  'api.server.tailscaleStateExists',
  /*
   * 表单宿主（批 2 / W-00）新纳入的一条**读**，与上面那一类同形：
   *  · `api.server.tailscaleGetStatus` —— 无参调用，拉整机 TS 状态快照喂出口候选下拉
   *    （`forms/TsExitPanel.tsx`）。失败 = 候选为空，面板**如实降级**成手填（`ts.exitEmptyHint`），
   *    不是一次失败的写；桌面 `TsSettingsDialog` 那处的 catch 同样只吞不报。
   * （同批还登记过 `api.subscription.onCreateProgressReady`：表单在自己生命期内订阅创建进度。
   *  2026-09-25 ζ 批 A8 把那条订阅挪到应用级 —— 由 store 的
   *  `subscribeAndHydrateSubscriptionCreateOperations` 登记，移动树里不再有这次直调，条目随之删掉。）
   */
  'api.server.tailscaleGetStatus',
  /*
   * 隐私锁（W-16）的三条**读/订阅**腿。与上面那批同类：登记一个监听、拉一次当前状态，
   * 没有用户在等回执，也没有可归因的控件可以贴错误。
   *
   * ⚠️ 同一批里**真的会失败又有人等**的那两条并**不**在这张表里：
   *  · `api.config.setPrivacyMode(true)`（闲置计时那条）罩在 `reportIfFails(` 里落全局 toast；
   *  · 解锁那两跳住在 `mobile/privacy-lock.ts`，由下面 FEEDBACK 表里「隐私锁」那一行罩着。
   * 这条分界就是这批放行的边界。
   */
  // Window-lifetime subscriptions are reads; user speedTest invocation remains a guarded write.
  'api.server.onSpeedTestProgress',
  'api.server.onSpeedTestResult',
  'api.server.onSpeedTestDone',
  'api.config.getPrivacyMode',
  'api.config.onEnterPrivacyMode',
  'api.config.onExitPrivacyMode',
  // 「设没设过密码」的状态回读（遮罩进入锁定态时查一次，决定空密码放不放行）。
  // 它自带兜底（读不到按「已设密码」处理），失败不需要用户做任何事。
  'api.privacy.hasPassword',
  // `<域>Api.<方法>` 门面上的读腿（判据面新纳入的那一类，见 `WRITE_CALLS` 头注）。
  'versionApi.getInfo',
  'vpnApi.getAuthStatus',
  // 锁屏密码那一行的状态回读（「设没设过密码」）。**写**那一侧（`privacyApi.setPassword`）
  // 不在这张表里 —— 它罩在设置屏的 `commit('privacy-password', …)` 里。
  'privacyApi.hasPassword',
  /*
   * 应用更新检查的 IPC 绑定（`settings/app-update-check.ts` 的 `checkAppUpdateViaIpc`）。
   *
   * 放行的是这个**绑定点**，不是那次用户动作：真正「用户按下按钮、等一个回执」的那一跳是
   * `runAppUpdateCheck(checkAppUpdateViaIpc, config)`，它写在 `UpdatePage.tsx` 的
   * `commit('app-update', …)` 实参里 —— 那条腿的登记与红字由 `MobileSettings.test.tsx`
   * ⑩ 的 `WRITE_ROWS.update` 与 ⑱ 一起钉着（⑱ 那条断言逐字对拍了 `commit(` 里的整次调用）。
   * 另一条调用点是根页那次**后台**自动检查（挂载时每会话一次），它按定义没有人在等回执。
   */
  'updateApi.check',
  /*
   * 下载腿的**订阅 / 快照回读**（`UpdatePage.tsx` 的 `wireUpdateProgress` 接线，批 15）。
   *
   * 与上面那批订阅同类：登记一个监听、拉一次当前进度，没有用户在等它们的回执 ——
   * 回读失败按定义就是「退回只有事件的老行为」，那是 `wireUpdateProgress` 自己写着的语义。
   *
   * ⚠️ 同一批里**真的会失败又有人等**的两条并**不**在这张表里：`updateApi.download` 与
   * `updateApi.install`，两者都内联在 `UpdatePage.tsx` 的 `commit('app-update', …)` /
   * `commit('app-reinstall', …)` 实参里。这条分界就是这批放行的边界。
   */
  'updateApi.onProgress',
  'updateApi.getProgress',
];
const READ_SET = new Set(READ_CALLS);

interface WriteSite {
  readonly file: string;
  readonly text: string;
  readonly line: number;
  readonly index: number;
}

function writeSitesIn(file: string, src: string): WriteSite[] {
  const seen = new Set<number>();
  return [...WRITE_CALLS, ...updateLegs(src)]
    .flatMap((re) =>
      [...src.matchAll(re)].map((m) => ({
        file,
        text: m[0].replace(/\s+/g, ''),
        line: src.slice(0, m.index).split('\n').length,
        index: m.index,
      }))
    )
    .filter((s) => !READ_SET.has(s.text))
    // A named async owner declaration is not its callers; only call expressions enter this new leg.
    .filter(s => s.text !== 'runMobileSpeedTest(' || !/function\s+$/.test(src.slice(Math.max(0,s.index-30),s.index)))
    // 同一个偏移只算一次：静态判据面与别名腿按构造不重叠（改名了静态就不匹配、句柄形态有 `.`
    // 前缀被 lookbehind 挡掉），但重复一旦发生，同一处写腿会在错误清单里出现两遍 —— 那种噪音
    // 会让人整体重刷清单。按偏移去重是防它的最便宜一手。
    .filter((s) => (seen.has(s.index) ? false : (seen.add(s.index), true)))
    .sort((a, b) => a.index - b.index);
}

const FILES = sourceFiles(MOBILE).map((p) => ({ file: rel(p), abs: p, src: code(readFileSync(p, 'utf8')) }));
const SITES: readonly WriteSite[] = FILES.flatMap((f) => writeSitesIn(f.file, f.src));

// ════════════════════════════════════════════════════════════════════════════
// 登记表：(屏, 失败回显机制)
// ════════════════════════════════════════════════════════════════════════════

/**
 * - `wrapper` —— 写腿必须落在机制调用的**实参区间**内（唯一写出口形态）。
 * - `reporter` —— 写腿必须落在一个 `try` 里，且那个 `try` 的 `catch` 真的调了 reporter
 *   （行内回显形态；「有 catch 但 catch 是空的」在这条下面当场红）。
 */
type Shape = 'wrapper' | 'reporter' | 'delegate';

interface ScreenFeedback {
  readonly screen: string;
  /** 取材面前缀（相对 `src/`）。 */
  readonly dir: string;
  readonly shape: Shape;
  /** 罩住写腿的那个符号。 */
  readonly mechanism: string;
  /**
   * 机制自身的「有牙」判据：
   * - `behavioral` —— 机制是个纯工厂，下面直接拿一个必然 reject 的 op 驱动它（最强的一档）。
   * - 否则 = 机制声明所在文件 + 它的失败腿必须调进的那条用户可见通道。
   */
  readonly teeth: 'behavioral' | { readonly file: string; readonly decl: string; readonly channel: string; readonly needsCatch: boolean };
  readonly why: string;
}

const FEEDBACK: readonly ScreenFeedback[] = [
  {
    screen: '窗口测速 coordinator',
    dir: 'mobile/use-mobile-speed-test.ts',
    shape: 'delegate',
    mechanism: 'runMobileSpeedTest',
    teeth: 'behavioral',
    why: '共享 IPC 失败清本地 token 后重抛原异常；caller 的 runMobileSpeedTest 写腿同样进本门，首页 catch/setError 与节点 runWrite 回显均需在场，不能只核共享清理函数',
  },
  {
    screen: '首页测速',
    dir: 'mobile/home/use-home-speed-test.ts',
    shape: 'reporter',
    mechanism: 'setError',
    teeth: { file: 'mobile/home/use-home-speed-test.ts', decl: 'runAll', channel: 'setError', needsCatch: true },
    why: '当前和全部测速的请求失败进入可见错误行，批量被中断不能报成功',
  },
  {
    screen: 'Debug 故障报告',
    dir: 'mobile/DebugReportButton.tsx',
    shape: 'reporter',
    mechanism: 'setStatus',
    teeth: { file: 'mobile/DebugReportButton.tsx', decl: 'exportReport', channel: 'setStatus', needsCatch: true },
    why: '报告生成或分享失败时，按钮下方显示失败提示，并允许重试',
  },
  {
    screen: '首页',
    dir: 'mobile/home/',
    shape: 'wrapper',
    mechanism: 'runWrite',
    teeth: 'behavioral',
    why: '`home/write-errors.ts` 的 `createRunWrite`：错误按控件 id 落表，呈现层渲染在那颗控件下面一行',
  },
  {
    screen: '设置',
    dir: 'mobile/settings/',
    shape: 'wrapper',
    mechanism: 'commit',
    teeth: 'behavioral',
    why: '`settings/write-feedback.ts` 的 `createCommit`：错误挂行 id，由 `SettingsRow` 经 context 自取',
  },
  {
    screen: '节点',
    dir: 'mobile/nodes/',
    shape: 'wrapper',
    mechanism: 'runWrite',
    teeth: { file: 'mobile/nodes/MobileNodesScreen.tsx', decl: 'runWrite', channel: 'setNotice', needsCatch: true },
    why: '屏内唯一写出口 `runWrite`，失败落屏级 notice（tone: err）',
  },
  {
    screen: '连接',
    dir: 'mobile/connections/',
    shape: 'wrapper',
    mechanism: 'runWrite',
    teeth: { file: 'mobile/connections/MobileConnectionsScreen.tsx', decl: 'runWrite', channel: 'setWriteErrors', needsCatch: true },
    why: '屏内唯一写出口 `runWrite`，失败落行内/屏级 `writeErrors`（另含 `ok:false` 腿，屏级门 ⑧ 钉）',
  },
  /*
   * 第六条**不是屏**，是全局接线层（批 1 / W-22~W-28）。它照样要在这张表里表态 ——
   * 本门守的性质是「移动端没有一次写失败是静默的」，那条性质不因为「它不是一个屏」而豁免。
   *
   * 与五个屏的差别在**通道**：这一层的写腿由后端事件触发，用户可能正在别的屏、甚至已经把应用
   * 切到后台 ⇒ 没有可归因的控件可以贴行内错误。故它落全局 toast，而全局 toast 宿主正是同一批
   * 补上的（`mobile/MobileToaster.tsx`，W-25）—— 在此之前这条通道根本不存在，这也是为什么
   * 「事件层的失败回显」只能与宿主同批落地。
   */
  {
    screen: '全局接线层',
    dir: 'mobile/app-wiring.ts',
    shape: 'wrapper',
    mechanism: 'reportIfFails',
    teeth: {
      file: 'mobile/app-wiring.ts',
      decl: 'reportIfFails',
      channel: 'warning',
      needsCatch: true,
    },
    why: '事件驱动的副作用没有可归因的控件 ⇒ 失败落全局 toast（`toast.warning`），另留 console.error 作诊断',
  },
  /*
   * 第七条同样不是屏：隐私锁的解锁链（W-16，`mobile/privacy-lock.ts`）。
   *
   * 它的用户可见通道是**遮罩自己那行提示** —— `submitPrivacyUnlock` 不返回错误、也不 throw，
   * 而是把四种结局逐一交给调用方传进来的 `showOutcome`，由遮罩渲染成那一行字。
   * 故形态是 `reporter`：两跳写腿都在 `try` 里，`catch` 真的把失败送进它。
   *
   * 为什么不是 `wrapper`：这一层没有「把一个 op 交给机制」的形状 —— 机制就是这个函数本身。
   */
  {
    screen: '隐私锁',
    dir: 'mobile/privacy-lock.ts',
    shape: 'reporter',
    mechanism: 'showOutcome',
    teeth: {
      file: 'mobile/privacy-lock.ts',
      decl: 'submitPrivacyUnlock',
      channel: 'showOutcome',
      needsCatch: true,
    },
    why: '解锁的四种结局（缺密码 / 密码错 / 没问成 / 成功）逐一交给遮罩渲染成那一行提示；`failed` 与 `rejected` 分开，免得用户一遍遍重打一个正确的密码',
  },
  {
    screen: '规则',
    dir: 'mobile/screens/rules/',
    shape: 'reporter',
    mechanism: 'reportWriteError',
    teeth: { file: 'mobile/screens/rules/RulesScreen.tsx', decl: 'reportWriteError', channel: 'setWriteErrors', needsCatch: false },
    why: '行内回显：每条写腿自带 try/catch，catch 里 `reportWriteError(key, …)` 贴着那一行显示',
  },
  /*
   * 第八条同样**不是屏**，是**表单宿主**（批 2 / W-00）—— `mobile/forms/**` 那一叠面板。
   *
   * 它的通道与五个屏都不同，而不同是**结构性**的：一张表单在提交失败时，用户的下一步动作就在
   * 这张表里（改一个字段再提交），错误必须留在**这张表上**、跟着表一起活着。落屏级 notice
   * 会在表单关掉的那一刻消失（而失败恰恰意味着表单没关），落全局 toast 则 2 秒就散。
   * 故每个面板自持一格 `notice`，渲染在脚上方（`FormSheet` 的 `notice` 槽）。
   *
   * 形态取 `reporter`：每条写腿自带 try/catch，catch 里调 `setNotice`。这与「唯一写出口」
   * 不冲突 —— 一张表单只有一条提交腿，出口本来就唯一，包一层 wrapper 只会多一个间接层。
   */
  {
    screen: '表单宿主',
    dir: 'mobile/forms/',
    shape: 'reporter',
    mechanism: 'setNotice',
    teeth: {
      file: 'mobile/forms/NodeFormPanel.tsx',
      decl: 'submit',
      channel: 'setNotice',
      needsCatch: true,
    },
    why: '每张表单自持一格 `notice`（渲染在 `FormSheet` 的脚上方）：失败时用户的下一步就在这张表里，错误必须跟着表活着',
  },
];

/**
 * 裁定 #14 登记在案的三条无锚点失败里，**不是一次写调用**的那一条。
 *
 * 上面那张 `FEEDBACK` 表守的是「写调用落在机制辖区内」，而这一条的形态不同：**写成功了**，
 * 但「还要做一件事才生效」的那句话在移动端没有宿主可说 —— `use-config.ts` 的 `promptAppRestart`
 * 走 `dialog-store`，`DialogHost` 只挂在桌面 `AppShell` 上 ⇒ 那个弹窗永不出现，用户改完设置
 * 界面上一个字都没有。它不是一次调用，`WRITE_CALLS` 按定义看不见它，故另立一组断言。
 *
 * 两条腿缺一不可（少任一条就是「门在但没牙」的两种形态）：
 *  · **行为**：纯工厂真的把弹窗折成了一条可显示的文本，且不认的形态返 `null`（不瞎显示）；
 *  · **接线**：屏内真的有人调它、把结果送进那张写失败表、并且**画出来**。
 *    只测工厂 = 工厂对了但没人调；只测接线 = 调了但折出来是空的。
 */
const DEFERRED_ANCHORS = [
  {
    what: '设置：`useConfig` 的「需重启 App」确认弹窗',
    /** 屏内消费它的那个局部函数。 */
    decl: 'useDeferredDialogNotice',
    /** 它必须真的去读 `dialog-store` 的栈（否则截的是空气）。 */
    source: 'useDialogStore',
    /** 它必须把折出来的文本送进的那条用户可见通道（= 本屏那张写失败表）。 */
    channel: 'report',
    /** 屏文件里必须真的把这一行画出来 —— 记进表而不渲染，用户照样什么都看不到。 */
    rowConst: 'DEFERRED_NOTICE_ROW',
    file: 'mobile/settings/MobileSettingsScreen.tsx',
  },
] as const;

// ════════════════════════════════════════════════════════════════════════════
// AST 小工具（同 `renderer-recovery.test.ts` / `lib/config-write-wiring.test.ts` 的先例）
// ════════════════════════════════════════════════════════════════════════════

function collectNodes(root: ts.Node, pred: (n: ts.Node) => boolean): ts.Node[] {
  const out: ts.Node[] = [];
  const walk = (n: ts.Node): void => {
    if (pred(n)) out.push(n);
    ts.forEachChild(n, walk);
  };
  walk(root);
  return out;
}

function calleeNameOf(call: ts.CallExpression): string | null {
  const e = call.expression;
  if (ts.isIdentifier(e)) return e.text;
  if (ts.isPropertyAccessExpression(e) && ts.isIdentifier(e.name)) return e.name.text;
  return null;
}

/** 本文件里「名字 + 形参 + 函数体」的局部函数（`function f(){}` 与 `const f = (…) => …` 两形态）。 */
interface LocalFn {
  readonly name: string;
  readonly params: readonly string[];
  readonly body: ts.Node;
}

function localFns(sf: ts.SourceFile): LocalFn[] {
  const out: LocalFn[] = [];
  const paramsOf = (n: { parameters: readonly ts.ParameterDeclaration[] }): string[] =>
    n.parameters.filter((p) => ts.isIdentifier(p.name)).map((p) => (p.name as ts.Identifier).text);
  for (const n of collectNodes(sf, ts.isFunctionDeclaration) as ts.FunctionDeclaration[]) {
    if (n.name) out.push({ name: n.name.text, params: paramsOf(n), body: n });
  }
  for (const d of collectNodes(sf, ts.isVariableDeclaration) as ts.VariableDeclaration[]) {
    if (!ts.isIdentifier(d.name) || !d.initializer) continue;
    // `const f = (…) => …` 与 `const f = useCallback((…) => …, [])` 都算（后者是本仓的常态）。
    const arrows = collectNodes(d.initializer, ts.isArrowFunction) as ts.ArrowFunction[];
    if (arrows.length === 0) continue;
    out.push({ name: d.name.text, params: paramsOf(arrows[0]), body: d.initializer });
  }
  return out;
}

/**
 * 机制的**传递闭包**。
 *
 * 直接查「写腿在不在 `commit(` 的实参里」会误报：设置页的 `NetworkPage` 把七个开关的写腿做成
 * thunk 交给本页的 `toggleRow(id, …, patch)`，由 `toggleRow` 统一挂 `commit(id, patch(v))` ——
 * 那是**更强**的接法（七行不可能有哪一行忘了接），却不是字面上的 `commit(update({…}))`。
 *
 * 故：局部函数 F 若在体内调了一个已知包装器、**且那次调用的实参里出现了 F 自己的形参**
 * （＝F 收到的那个 thunk 确实被送进了机制），F 本身也算包装器。取到不动点为止。
 * 「实参里出现自己的形参」这一条是必要的：少了它，一个恰好也调过 `commit` 的函数会把它
 * 全部实参一并漂白。
 */
function wrapperNames(sf: ts.SourceFile, seed: string): Set<string> {
  const known = new Set([seed]);
  const fns = localFns(sf);
  for (let pass = 0; pass < fns.length + 1; pass += 1) {
    let grew = false;
    for (const f of fns) {
      if (known.has(f.name) || f.params.length === 0) continue;
      const routed = (collectNodes(f.body, ts.isCallExpression) as ts.CallExpression[]).some(
        (c) =>
          known.has(calleeNameOf(c) ?? '') &&
          c.arguments.some(
            (a) => collectNodes(a, (n) => ts.isIdentifier(n) && f.params.includes(n.text)).length > 0
          )
      );
      if (routed) {
        known.add(f.name);
        grew = true;
      }
    }
    if (!grew) break;
  }
  return known;
}

/** `wrapper` 形态：写腿必须落在某个包装器调用的实参区间内。 */
function uncoveredByWrapper(sf: ts.SourceFile, mechanism: string, offsets: readonly number[]): number[] {
  const known = wrapperNames(sf, mechanism);
  const spans = (collectNodes(sf, ts.isCallExpression) as ts.CallExpression[])
    .filter((c) => known.has(calleeNameOf(c) ?? ''))
    .map((c) => ({ start: c.getStart(sf), end: c.end }));
  return offsets.filter((at) => !spans.some((s) => at > s.start && at < s.end));
}

/** `reporter` 形态：写腿必须落在一个 `try` 里，且那个 `try` 的 `catch` 真的调了 reporter。 */
function uncoveredByReporter(sf: ts.SourceFile, reporter: string, offsets: readonly number[]): number[] {
  const guarded = (collectNodes(sf, ts.isTryStatement) as ts.TryStatement[])
    .filter((t) => {
      const c = t.catchClause;
      return (
        c !== undefined &&
        (collectNodes(c, ts.isCallExpression) as ts.CallExpression[]).some(
          (call) => calleeNameOf(call) === reporter
        )
      );
    })
    .map((t) => ({ start: t.tryBlock.getStart(sf), end: t.tryBlock.end }));
  return offsets.filter((at) => !guarded.some((s) => at > s.start && at < s.end));
}

/** Extracted async owner must reject with the same caught error; callers remain separately in WRITE_CALLS. */
function uncoveredByDelegate(sf: ts.SourceFile, offsets: readonly number[]): number[] {
  const guarded = (collectNodes(sf, ts.isTryStatement) as ts.TryStatement[]).filter(statement => {
    const clause = statement.catchClause;
    const name = clause?.variableDeclaration?.name;
    return clause && name && ts.isIdentifier(name) &&
      (collectNodes(clause.block, ts.isThrowStatement) as ts.ThrowStatement[]).some(thrown =>
        thrown.expression && ts.isIdentifier(thrown.expression) && thrown.expression.text === name.text);
  }).map(statement => ({start: statement.tryBlock.getStart(sf), end: statement.tryBlock.end}));
  return offsets.filter(at => !guarded.some(span => at > span.start && at < span.end));
}

const parsedCache = new Map<string, ts.SourceFile>();
function parse(file: string): ts.SourceFile {
  let sf = parsedCache.get(file);
  if (!sf) {
    const abs = join(SRC, file);
    sf = ts.parseSourceFile(abs, readFileSync(abs, 'utf8'));
    parsedCache.set(file, sf);
  }
  return sf;
}

const screenOf = (file: string): ScreenFeedback | undefined => FEEDBACK.find((s) => file.startsWith(s.dir));

// ════════════════════════════════════════════════════════════════════════════
// 断言
// ════════════════════════════════════════════════════════════════════════════

describe('自检：判据面非空且抓得到（扫空 ⇒ 下面每条恒绿，那才是真危险）', () => {
  it('整个 src/mobile 树上扫得到一批写腿，且每个登记屏都有自己的那一批', () => {
    expect(FILES.length, `只收到 ${FILES.length} 个移动端源文件`).toBeGreaterThan(20);
    expect(SITES.length, `只扫到 ${SITES.length} 处写腿`).toBeGreaterThan(40);
    for (const s of FEEDBACK) {
      expect(
        SITES.filter((x) => x.file.startsWith(s.dir)).length,
        `${s.screen}（${s.dir}）一处写腿都没扫到 —— 它这一格的断言会空跑`
      ).toBeGreaterThan(0);
    }
  });

  it('扫描器认得换行写法的点号，且读腿白名单真的在过滤（两件事一条断言）', () => {
    // `api.logs.search` 是登记过的读腿 ⇒ 必须被滤掉；另两条换行写法必须照样抓到。
    const sample =
      'void api.logs\n  .search(q);\nvoid api.rules\n  .update(r);\nawait api.connections.close(id);';
    expect(writeSitesIn('x.ts', sample).map((s) => s.text)).toEqual([
      'api.rules.update',
      'api.connections.close',
    ]);
  });

  /**
   * 绑定别名判据面的正反对照。四种形态里**句柄成员访问**与**选择器改名**在真实代码上零实例 ——
   * 判据在真实代码上空跑，是绿的还是死的分不出来，故用合成夹具证明它报得出、且不是恒报。
   * 夹具与预期都取自 `@/test/write-leg-aliases`（与配置门同一份，两处各抄一份会各自漂移）。
   */
  it('绑定别名判据面：句柄成员访问 / 解构改名 / 选择器改名三种都抓得到', () => {
    const hit = updateLegs(ALIAS_FIXTURE)
      .slice(1) // 第 0 条是静态的 `update({`，这里只对照别名腿
      .flatMap((re) => [...ALIAS_FIXTURE.matchAll(re)].map((m) => ({ t: m[0].replace(/\s+/g, ''), i: m.index })))
      .sort((a, b) => a.i - b.i)
      .map((x) => x.t);
    expect(hit, '别名解析没抓全 —— P2 那类「脱开唯一写出口且判据面看不见」的洞会重新张开').toEqual([
      ...ALIAS_FIXTURE_LEGS,
    ]);
  });

  it('反向对照：不改名的解构与同名选择器绑定**不**产生别名腿（否则同一处写腿会被重复计数）', () => {
    const same = ["const { update } = useConfig();", "const switchServer = useAppStore((s) => s.switchServer);"].join(
      '\n'
    );
    expect(aliasWriteLegs(same, STORE_WRITES)).toEqual([]);
  });

  it('抽出的测速函数声明不是调用；真实caller仍进入写失败面', () => {
    const source = 'export async function runMobileSpeedTest(ids) {}\nawait runMobileSpeedTest(ids);';
    expect(writeSitesIn('fixture.ts', source).map(site => site.text)).toEqual(['runMobileSpeedTest(']);
    expect(writeSitesIn('fixture.ts', source)[0].line).toBe(2);
  });

  it('读腿白名单不留僵尸（登记了一个已经没人调的读 = 白名单在腐烂）', () => {
    const all = new Set(
      FILES.flatMap((f) =>
        [...WRITE_CALLS].flatMap((re) => [...f.src.matchAll(re)].map((m) => m[0].replace(/\s+/g, '')))
      )
    );
    for (const name of READ_CALLS) {
      expect([...all], `白名单里的 ${name} 已经没人调了 —— 该把它删掉`).toContain(name);
    }
  });
});

describe('① 每一条移动端写路径都落在一个已登记的屏下（第六个屏没回显 ⇒ 上不了车）', () => {
  it('登记的屏目录与「树上真的有写腿的屏目录」逐字相等', () => {
    const discovered = [
      ...new Set(
        SITES.map((s) => {
          const dir = FEEDBACK.find((f) => s.file.startsWith(f.dir));
          return dir ? dir.dir : `未登记：${s.file}`;
        })
      ),
    ].sort();
    expect(
      discovered,
      '树上出现了写腿却没有失败回显机制的屏（或登记了一个已经没有写腿的屏）—— ' +
        '裁定 #14 要求每一条写路径都有可见回执，新屏落地时必须在本表登记它的机制'
    ).toEqual([...new Set(FEEDBACK.map((f) => f.dir))].sort());
  });
});

describe('② 每一处写调用都被它那个屏的已登记机制罩住', () => {
  it('没有一处写腿逃出机制的辖区', () => {
    const escaped: string[] = [];
    for (const f of FILES) {
      const sites = SITES.filter((s) => s.file === f.file);
      if (sites.length === 0) continue;
      const screen = screenOf(f.file);
      if (!screen) continue; // 由 ① 负责报，这里不重复
      const offsets = sites.map((s) => s.index);
      const bad = new Set(
        screen.shape === 'wrapper'
          ? uncoveredByWrapper(parse(f.file), screen.mechanism, offsets)
          : screen.shape === 'delegate'
            ? uncoveredByDelegate(parse(f.file), offsets)
            : uncoveredByReporter(parse(f.file), screen.mechanism, offsets)
      );
      for (const s of sites) if (bad.has(s.index)) escaped.push(`${s.file}:${s.line} ${s.text}`);
    }
    expect(
      escaped.sort(),
      '这些写调用不在本屏失败回显机制的辖区内 —— 它们失败时用户看不到任何东西：' +
        '控件自己弹回原位，一句话都没有（IA 裁定 #14 要抓的正是这个形态）'
    ).toEqual([]);
  });
});

describe('③ 机制自身有牙：失败真的送进了用户看得见的通道（不是空 catch）', () => {
  it('共享测速失败确实拒绝给caller并释放自己的请求；本门同时核caller可见失败腿', async () => {
    useMobileSpeedTestStore.setState({task:null,request:null,nextToken:0});
    const cause = new Error('fixture-speed-failure');
    const call = vi.spyOn(api.server,'speedTest').mockRejectedValue(cause);
    try {
      await expect(runMobileSpeedTest(['fixture'],'nodes')).rejects.toBe(cause);
      expect(useMobileSpeedTestStore.getState().request).toBeNull();
    } finally { call.mockRestore(); }
  });

  it('首页：`createRunWrite` 的失败腿把错误记进了表，成功腿清掉它', async () => {
    let errors: HomeWriteErrors = {};
    const t = ((k: string, o?: Record<string, unknown>) =>
      o?.reason !== undefined ? `${k}:${String(o.reason)}` : k) as unknown as TFunction;
    const runWrite = createRunWrite((up) => {
      errors = up(errors);
    }, t);
    await runWrite('connect', () => Promise.reject(new Error('boom')));
    expect(Object.keys(errors), '写失败后错误表仍是空的 —— catch 里什么都没做').toEqual(['connect']);
    expect(errors.connect).toContain('boom');
    await runWrite('connect', () => Promise.resolve(undefined));
    expect(errors, '写成功没有清掉上一次的错误（修好的失败会永久挂在控件下面）').toEqual({});
  });

  it('设置：`createCommit` 的失败腿把错误记进了表，成功腿清掉它', async () => {
    let errors: SettingsWriteErrors = {};
    const t = ((k: string, o?: Record<string, unknown>) =>
      o?.reason !== undefined ? `${k}:${String(o.reason)}` : k) as unknown as TFunction;
    const commit = createCommit((up) => {
      errors = up(errors);
    }, t);
    commit('theme', Promise.reject(new Error('nope')));
    await new Promise((r) => setTimeout(r, 0));
    expect(Object.keys(errors), '写失败后错误表仍是空的 —— 拒绝腿什么都没做').toEqual(['theme']);
    expect(errors.theme).toContain('nope');
    commit('theme', Promise.resolve());
    await new Promise((r) => setTimeout(r, 0));
    expect(errors, '写成功没有清掉上一次的错误').toEqual({});
  });

  it('屏内机制：失败腿必须调进那条用户可见的通道（只有 catch 不算数）', () => {
    for (const s of FEEDBACK) {
      if (s.teeth === 'behavioral') continue;
      const { file, decl, channel, needsCatch } = s.teeth;
      const sf = parse(file);
      const owner = localFns(sf).find((f) => f.name === decl);
      expect(owner, `${file} 里找不到 \`${decl}\` —— ${s.screen}屏的机制不见了（改名了？）`).toBeDefined();
      const calls = collectNodes(owner!.body, ts.isCallExpression) as ts.CallExpression[];
      if (needsCatch) {
        const clauses = collectNodes(owner!.body, ts.isCatchClause);
        expect(clauses.length, `${s.screen}屏的 \`${decl}\` 里没有 catch`).toBeGreaterThan(0);
        const wired = clauses.some(
          (c) =>
            (collectNodes(c, ts.isCallExpression) as ts.CallExpression[]).some(
              (call) => calleeNameOf(call) === channel
            )
        );
        expect(
          wired,
          `${s.screen}屏的 \`${decl}\` 的 catch 里没有把失败送进 \`${channel}\` —— 空 catch 与没有 catch 等价`
        ).toBe(true);
      } else {
        expect(
          calls.some((call) => calleeNameOf(call) === channel),
          `${s.screen}屏的 \`${decl}\` 没有写进 \`${channel}\` —— 它只是个不做事的函数`
        ).toBe(true);
      }
    }
  });
});

describe('④ 弹窗宿主缺席那一条腿也有锚点（写成功了、但「没生效」这句话没人说）', () => {
  it('自检：登记表非空（表空 ⇒ 下面每条恒绿）', () => {
    expect(DEFERRED_ANCHORS.length, '登记表是空的 —— 本组断言在空跑').toBeGreaterThan(0);
  });

  it('行为：`deferredNoticeOf` 把 confirm 折成一条可显示的文本（正面断言，不是「没出现 X」）', () => {
    const got = deferredNoticeOf([
      { kind: 'confirm', instanceId: 'd1', payload: { title: '需要重启', message: '甲已保存\n\n乙仍按旧值跑' } },
    ]);
    expect(got, '栈上有一条 confirm，却什么都没折出来 —— 用户还是一个字都看不到').not.toBeNull();
    expect(got?.instanceId).toBe('d1');
    expect(got?.label).toBe('需要重启');
    // 段间空行折成空格：行内那一格是一行文本，`\n\n` 在那里会被折叠成什么都看不见。
    expect(got?.text).toBe('甲已保存 乙仍按旧值跑');
  });

  it('反向对照：空栈与不认识的 kind 都返 `null`（不是恒报，也不瞎显示一个渲染不了的弹窗）', () => {
    expect(deferredNoticeOf([])).toBeNull();
    expect(
      deferredNoticeOf([{ kind: 'node', instanceId: 'd2' }]),
      '把一个要渲染整张表单的弹窗折成一行文本 = 一句没头没尾的话',
    ).toBeNull();
  });

  it('接线：屏内真的调了它、送进了写失败表、并且把那一行画出来了', () => {
    for (const a of DEFERRED_ANCHORS) {
      const sf = parse(a.file);
      const owner = localFns(sf).find((f) => f.name === a.decl);
      expect(owner, `${a.file} 里找不到 \`${a.decl}\` —— ${a.what}那条锚点的接线不见了（改名了？）`).toBeDefined();
      const called = new Set(
        (collectNodes(owner!.body, ts.isCallExpression) as ts.CallExpression[]).map(
          (c) => calleeNameOf(c) ?? ''
        )
      );
      expect(called.has(a.source), `\`${a.decl}\` 没有去读 \`${a.source}\` —— 它截的是空气`).toBe(true);
      expect(called.has('deferredNoticeOf'), `\`${a.decl}\` 没有调折叠工厂`).toBe(true);
      expect(
        called.has(a.channel),
        `\`${a.decl}\` 没有把结果送进 \`${a.channel}\` —— 截下来了却没人显示，与没截等价`
      ).toBe(true);
      const src = readFileSync(join(SRC, a.file), 'utf8');
      expect(
        new RegExp(`id=\\{${a.rowConst}\\}`).test(src),
        `${a.file} 里没有 \`id={${a.rowConst}}\` 的那一行 —— 记进了表却没画出来，用户照样看不到`
      ).toBe(true);
    }
  });
});

/* ── 判据自证：它报得出错，且报的正是那一处 ────────────────────────────────── */
describe('⑤ 正反对照（判据既不恒绿也不恒红）', () => {
  const WRAP_HEAD = `
function Screen(runWrite: (op: () => Promise<void>) => void) {
  const onA = () => {
    void runWrite(async () => {
      await api.connections.close('a');
    });
  };
  const onB = () => {`;
  const WRAP_TAIL = `
  };
  return { onA, onB };
}
`;
  const at = (text: string, needle: string): number => text.indexOf(needle);

  it('wrapper：一处写腿没被机制罩住 ⇒ 点名它（且只点名它）', () => {
    const text = `${WRAP_HEAD}
    void api.logs.clear();${WRAP_TAIL}`;
    const sf = ts.parseSourceFile('synthetic-wrapper-escape.ts', text);
    const inside = at(text, 'api.connections.close');
    const outside = at(text, 'api.logs.clear');
    expect(uncoveredByWrapper(sf, 'runWrite', [inside, outside])).toEqual([outside]);
  });

  it('wrapper：包进机制后不再报（证明不是恒红），且经由本地 thunk 中转的也认', () => {
    const text = `${WRAP_HEAD}
    void runWrite(async () => {
      await api.logs.clear();
    });${WRAP_TAIL}`;
    const sf = ts.parseSourceFile('synthetic-wrapper-ok.ts', text);
    expect(
      uncoveredByWrapper(sf, 'runWrite', [at(text, 'api.connections.close'), at(text, 'api.logs.clear')])
    ).toEqual([]);

    // 设置页 `NetworkPage.toggleRow` 那种「thunk 交给本地装配函数，由它统一挂机制」的形态。
    const relay = `
function Page(commit: (id: string, p: Promise<void>) => void, update: (p: object) => Promise<void>) {
  function row(id: string, patch: () => Promise<void>) {
    return { onChange: () => commit(id, patch()) };
  }
  return row('a', () => update({ blockQuic: true }));
}
`;
    const relaySf = ts.parseSourceFile('synthetic-wrapper-relay.ts', relay);
    expect(uncoveredByWrapper(relaySf, 'commit', [at(relay, 'update({ blockQuic')])).toEqual([]);
  });

  it('reporter：**有 catch 但 catch 里什么都不做** ⇒ 照样点名（裁定 #14 点名的那个形态）', () => {
    const empty = `
async function onDelete(id: string) {
  try {
    await api.rules.delete(id);
  } catch (err) {
    console.error(err);
  }
}
`;
    const sfEmpty = ts.parseSourceFile('synthetic-reporter-empty.ts', empty);
    const off = at(empty, 'api.rules.delete');
    expect(uncoveredByReporter(sfEmpty, 'reportWriteError', [off])).toEqual([off]);

    const wired = empty.replace('console.error(err);', "reportWriteError('r', 'failed');");
    const sfWired = ts.parseSourceFile('synthetic-reporter-wired.ts', wired);
    expect(uncoveredByReporter(sfWired, 'reportWriteError', [at(wired, 'api.rules.delete')])).toEqual([]);
  });
});
