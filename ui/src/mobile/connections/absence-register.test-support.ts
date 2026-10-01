/**
 * 连接屏的**与桌面逐块对差登记表**。形态与口径见 `../parity-registry.ts`；
 * 门在 `../screen-parity.test.ts`。
 *
 * 桌面取材面 = `components/screens/connections/` 整个目录（今天只有 `ConnectionsScreen.tsx`
 * 一个 `.tsx` 加四个纯逻辑 `.ts`）。
 *
 * # 这一屏的对差结果：动作面**全部有落点**，数据块面为空
 *
 * 桌面这一屏没有把任何呈现块拆成自己目录里的组件（`ConnectionsScreen.tsx` 一个文件画完），
 * 所以数据块面**恰好是空集**。空集不是「没测」——门对它有一条专门的正面断言：
 * 桌面这一屏的块数必须等于 `blocks.length`（= 0），桌面哪天把表格拆成一个组件、
 * 或新加一张卡，这条当场红。四屏里另外三屏的块面都非空，那是「块面本身没塌」的对照。
 *
 * 逐颗对差把一处**假缺口**也钉住了：桌面的「拓扑」tab 在移动端叫 `mobileConnections.seg.overview`，
 * 键名不同、腿是同一条。靠「桌面这句文案在移动端出现过没有」这类字面信号判，会把它误报成缺席；
 * 靠人读又会漏掉真缺席。所以这张表**逐颗都要登记**，不接受「没命中就算缺」的自动判定。
 */
import type { ScreenParityRegister } from '../parity-registry.test-support';

export const CONNECTIONS_PARITY: ScreenParityRegister = {
  screen: 'connections',
  // 🔴 两个桌面目录：移动连接屏的第四段就是桌面的日志屏（`ConnectionsView.tsx` 的
  // `{ id: 'logs', … }`）。上一版这里只有第一个，日志屏那 12 颗动作不在任何一面上 ——
  // `logs.archiveLegacy` / `logs.deleteLegacy` 因此既没接、也没登记、也没人看得见。
  desktopDirs: ['ui/src/components/screens/connections', 'ui/src/components/screens/logs'],
  mobileDir: 'ui/src/mobile/connections',

  actionFaces: ['controls'],
  primitiveModules: [],
  // 同首页：右键面板里那两颗「加规则」渲染 `rules.*`，是内容不是原语。
  extraBlockSources: ['ui/src/components/RuleSubjectMenuItems.tsx'],
  sharedPrimitives: [
    'ui/src/components/HorizontalScroll.tsx',
    'ui/src/components/InfoIcon.tsx',
    'ui/src/components/ListPager.tsx',
    'ui/src/components/dialogs/Csel.tsx',
  ],
  slotHandoffGates: [
    { file: 'ui/src/mobile/connections/connections-screen.test.tsx', mustContain: '① IA §2.3 内容序：四个分段，默认概览，每段的块按重要性序出现' },
    { file: 'ui/src/mobile/connections/connections-screen.test.tsx', mustContain: '④ IA §2.3：四个破坏性/控制动作进 header overflow，不摆成一行工具栏' },
  ],
  actions: [
    {
      id: 'HorizontalScroll.tsx|f:navigation',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: "<DetailRow label={t('connections.colDest')}" } },
      note: '桌面横向表格的左右列导航；移动端活动连接用竖向详情逐项展示同列内容，页面纵向滚动即可到达目的地等字段。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.topologyTab',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: "{ id: 'overview', label: t('mobileConnections.seg.overview') }" },
      },
      note: '「拓扑」视图（Top 主机 / Top 出站两张榜）。移动端叫「概览」段，装的是同两张榜 —— 换了名字不是换了腿。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.activeTab',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: "{ id: 'active', label: t('connections.activeTab') }" },
      },
      note: '「活动连接」视图。移动端同名同键。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.closedTab',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: "{ id: 'closed', label: t('connections.closedTab') }" },
      },
      note: '「已关闭」视图。移动端同名同键。',
    },
    {
      id: 'ConnectionsScreen.tsx|#conn-pause-btn',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "paused ? tr('connections.resume') : tr('connections.pause')" },
      },
      note: '暂停 / 继续刷新。移动端在屏级动作 sheet 里，且暂停期间列表不清空（守卫与桌面同源）。',
    },
    {
      id: 'ConnectionsScreen.tsx|#conn-close-filtered',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "onSelect: () => confirmTwice(CLOSE_FILTERED_KEY, onCloseFiltered)" },
      },
      note: '关闭当前筛选出的全部连接（二次确认）。移动端连 confirm-twice 一起搬。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.closeAll+connections.closeAllTitle+connections.confirm',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "tr('connections.closeAll')" },
      },
      note: '关闭全部连接（二次确认）。移动端在屏级动作 sheet 里，`armed` 那一档同样要走两次点击才真的执行。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.clearClosed+connections.clearClosedTitle+connections.confirmClearClosed',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: 'await api.stats.clearClosed()' },
      },
      note: '清空已关闭历史（二次确认）。锚取的是那条真的 IPC 调用，不是按钮文案 —— 文案在、腿没了正是本仓栽过的形态。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.close',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: 'onClick={() => onClose(row.id)}' },
      },
      note: '行尾那颗关闭单条连接。移动端在活动连接行上同样有一颗（已关闭那一段没有，那一段的连接本来就不在了）。',
    },
    {
      id: 'ConnectionsScreen.tsx|f:setMenu',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: 'id: `copy-${subject.kind}`' },
      },
      note: '右键面板里切换「拿哪一维当规则主体」（域名 / IP / 进程）。移动端不做切换，而是把每一维**各铺一行**到行动作 sheet 里 —— 触屏没有「先选维再点动作」这一跳，铺开反而少一步。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.copySubject',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "tr('connections.copySubject'" },
      },
      note: '复制选中的规则主体。移动端逐维一行，键同源。',
    },
    {
      id: 'ConnectionsScreen.tsx|k:connections.close#2',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "id: 'close'," },
      },
      note: '右键面板里的「关闭这条连接」。与行尾那颗是同一条腿的第二个入口，移动端在行动作 sheet 里。',
    },
    {
      id: 'ConnectionsScreen.tsx|f:setTopN',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: 'TOP_N_OPTIONS.map((n) =>' },
      },
      note: 'Top 榜展示条数（5 / 10 / 15）。移动端同一张表 `TOP_N_OPTIONS`。',
    },

    /* ── 列排序（`<th>` 上的 onClick，不是 `<button>`）──────────────────────── */
    {
      id: 'ConnectionsScreen.tsx|f:onSort',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: tr('mobileConnections.sort')" },
      },
      note: '按列排序（点表头切换排序键与方向）。移动端没有表头这一层，排序是屏级动作 sheet 里的一项 + 一张九键排序表。上一版的动作面只认 `<button>`，这颗挂在 `<th>` 上的排序整条不可见。',
    },

    /* ── 渲染一跳带进来的：分页器 / 右键面板的加规则片段 / 自定义下拉 ────────── */
    {
      id: 'ListPager.tsx|k:common.previousPage',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: "t('common.previousPage')" } },
      note: '列表分页的上一页。组件住在 `components/ListPager.tsx`（这一屏与日志段共用），由渲染一跳带进动作面；移动端 `ConnectionsView` 自己画了同一对翻页键。',
    },
    {
      id: 'ListPager.tsx|k:common.nextPage',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: "t('common.nextPage')" } },
      note: '列表分页的下一页，与上一条同一条腿的另一半。',
    },
    {
      id: 'RuleSubjectMenuItems.tsx|k:rules.addNew',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx',
          mustContain: "openMobileForm({ kind: 'rule', preset: { type: subject.type, value: subject.value } });",
        },
      },
      note: '右键面板里的「为这条连接新建规则」。桌面是 `navigate(\'rules\')` + `openDialog({kind:\'rule\',preset})`；移动端开的是同一条语义的表单层（`mobile/forms/RuleFormPanel.tsx`，恒新建 / 恒单条件 / 恒 route 面，类型表与校验复用 `domain/rules`，暂存分流复用同一个 `trafficRules` 闸门）。锚取的是**真的把预设交出去**那一行 —— 按钮在、preset 丢了，规则就建到一个空条件上。',
    },
    {
      id: 'RuleSubjectMenuItems.tsx|k:rules.addExisting+rules.subjectAlreadyInRule',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx',
          mustContain: "await api.rules.update(next, 'route');",
        },
      },
      note: '右键面板里的「合并进已有规则」。选择器（桌面 `RulePickDialog`）在移动端是一张底部面板，判据整层复用 `dialogs/rule-append.ts`（`ruleAppendTargets` / `sortAppendTargets` / `analyzeRuleCoverage` / `isShadowedTarget` / `appendSubjectToRule`）。锚取的是那条真的 IPC 追加调用，不是选择器的标题 —— 面板在、腿没了正是本仓栽过的形态。',
    },
    {
      id: 'Csel.tsx|f:preventDefault',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "title: tr('mobileConnections.logs.filter')" } },
      note: '自定义下拉的触发器（日志段用它选级别 / 来源）。移动端换成从底部升起的 ActionSheet，回答同一个问题。',
    },
    {
      id: 'Csel.tsx|f:choose',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: `${tr('logs.currentLevel')} · ${opt.toUpperCase()}`" } },
      note: '在下拉里选中一项。移动端是 ActionSheet 里的一行。',
    },
    {
      id: 'Csel.tsx|f:header',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/connections/ConnectionsView.tsx',
          mustContain: 'className={`mc-sheet-grp mc-sheet-grp-t${row.collapsed ? \'\' : \' open\'}`}',
        },
      },
      note: '下拉里**可折叠的分组组头**（`Csel` 支持分组选项）。移动端的 ActionSheet 现在同样有这一维：组头带 `aria-expanded` + 组内条数，「有没有 `id`」决定可不可折叠（不带 id 的组恒展开，主路径不许被折进去），折叠行序复用**同一个** `buildCselRows`。真消费方是「加入已有规则」那张面板：可追加的一组恒展开，点不下去的一组默认折起。',
    },

    /* ── 日志段（桌面 `components/screens/logs/` 整屏折进本屏的 `logs` 段）──────── */
    {
      id: 'LogsScreen.tsx|k:logs.archiveLegacy',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx',
          mustContain: 'const res = await api.logs.archiveLegacy();',
        },
      },
      note: '把 W26 前遗留的无界 `singbox.log` 归档到用户选定的位置。移动端在日志段的「更多」面板里，**与桌面同源同条件**（`legacyInfo().exists` 为真才在场；没有那个文件时两端都一行不画）。射程如实记：归档事务要的是目标**所在的目录**（同目录临时文件 + `rename` 提交），SAF 交回的 content URI 上表达不出来 ⇒ 后端对 URI 目标**显式报错**（`logs.rs` 的 `PickedTarget::Path` 那一支），红字经 `runWrite` 落在面板里，不假装成功也不假装取消。',
    },
    {
      id: 'LogsScreen.tsx|k:common.confirmAgain+logs.deleteLegacy',
      disposition: {
        kind: 'ported',
        mobile: {
          file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx',
          mustContain: 'const res = await api.logs.deleteLegacy();',
        },
      },
      note: '二次确认后删除那份遗留日志。桌面用原地 `confirmTwice`（按钮翻红 + 「再点一次」），移动端叠一层 `forms/ConfirmPanel`：这一颗与本屏另外四颗破坏性动作的差别是**不可逆且不可重来**（删的是用户的历史资产），触屏上「再点一次」的第二击与误触只差 40 ms。**不用 `window.confirm`** —— 它被 dialog 插件的 init 覆写成 `plugin:dialog|confirm`，漏授 ACL 时整条腿抛 rejection。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.openDir+logs.openDirTip',
      disposition: {
        kind: 'platform-absent',
        /* 锚指**平台判定本身** —— Android 上本应用能把私有存储里的东西交给别的应用的**唯一**机制，
           而它交的是「一个文件的 content URI + 一次性读授权」。「可浏览的目录」这个对象在这条路上
           不存在，也不在另一条路（SAF）上：`tauri-plugin-dialog` 的 Android 面只有
           `ACTION_GET_CONTENT` / `ACTION_CREATE_DOCUMENT`，没有 `ACTION_OPEN_DOCUMENT_TREE`，
           而 `commands/picked_file.rs` 的射程自曝第 2 条逐字写着「需要目标**所在目录**的动作在
           content URI 上结构性不成立」。 */
        evidence: {
          file: 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt',
          mustContain:
            'val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", apk)',
        },
      },
      note: '在系统文件管理器里打开日志目录。日志目录 = Tauri 的 `configDir`，在 Android 上是 `activity.dataDir`（应用私有内部存储）—— 没有任何 DocumentsProvider 暴露它，文件管理器里根本不存在这个目录可打开；后端那条腿在 Android 上落到 `ShellPlugin` 的 `Intent(ACTION_VIEW, Uri.parse(裸路径))`，一个无 scheme 的 URI 没有任何组件能处理。用户「拿到日志」这件事在本屏由**导出**兑现（`logs.export` / 诊断报告，两条都已移植），那不是这一颗的替代品，是这一颗在本平台上唯一存在的形态。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.diagnosticMode+logs.diagnosticTipOff+logs.diagnosticTipOn',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: tr('logs.diagnosticMode')" } },
      note: '临时开 DEBUG 诊断（不改持久配置）。移动端在日志段的屏级动作 sheet 里，开关文案两态同源。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.export',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: tr('mobileActions.exportOptions')" } },
      note: '导出入口（点开是「诊断报告 / 纯日志」二选一）。移动端同名一行，附一句「要经系统分享面板」的现状说明（那句话本身已在债务表上）。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.exportReport+logs.exportReportDesc+logs.recommended',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: tr('mobileActions.report')" } },
      note: '导出 Markdown 诊断报告（推荐档）。移动端在导出 sheet 的第一行。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.exportLogsOnly+logs.exportLogsOnlyDesc',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: tr('mobileActions.saveLogs')" } },
      note: '只导出纯日志。移动端在导出 sheet 的第二行，键同源。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.follow+logs.followTipOff+logs.followTipOn',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: tr('logs.follow')" } },
      note: '跟随新日志（自动滚到底）。移动端同名一行，两态说明同源。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.followTipOff+logs.scrollToBottom',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/ConnectionsView.tsx', mustContain: "t('logs.scrollToBottom')" } },
      note: '「跳到最新」那颗浮动键（跟随关掉、又滚上去看历史时出现）。移动端在日志视图里画同一颗。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.redact+logs.redactTip',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "label: tr('logs.redact')" } },
      note: '脱敏显示（域名 / IP 打码）。移动端同名一行。',
    },
    {
      id: 'LogsScreen.tsx|k:common.copy+logs.copyTip',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "description: tr('logs.copyTip')" } },
      note: '复制当前可见日志。移动端同名一行。',
    },
    {
      id: 'LogsScreen.tsx|k:logs.clear+logs.clearConfirm+logs.clearTip',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "armed === LOGS_CLEAR_KEY ? tr('logs.clearConfirm') : tr('logs.clear')" } },
      note: '清空日志（二次确认）。移动端同名一行，确认态用同一对文案键。',
    },
  ],

  /**
   * 桌面连接屏与日志屏都没有把呈现块拆成自己目录里的组件（各自一个 `*Screen.tsx` 画完），
   * 所以这一面上唯一的一块是渲染一跳带进来的那个内容片段。
   */
  blocks: [
    {
      id: 'RuleSubjectMenuItems',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/connections/MobileConnectionsScreen.tsx', mustContain: "id: 'add-rule'," },
      },
      note: '右键面板里「拿这条连接去加规则」那一整块。它住在 `components/` 下却渲染 `rules.*` 屏级文案 ⇒ 按 ⓪ 组那条判据是这一屏的内容，进 `extraBlockSources`。移动端把它落成行动作 sheet 里的一行入口 + 一张逐维铺开两颗（新建 / 加入已有）的二级面板；逐颗的处置见 actions 面同名两条。',
    },
  ],

  /**
   * 桌面连接屏**两个目录**（connections + logs）的具名数据槽基线。
   * **变更探测器，不是对差面**。2026-09-06 从 12 涨到 19：涨的七个（`log-*` / `s-logs`）
   * 是折进来的日志屏本来就有的槽，此前整批不在任何一面上。
   */
  slotBaseline: [
    'conn-close-filtered',
    'conn-filtered-lbl',
    'conn-pause-btn',
    'conn-pause-lbl',
    'conn-search',
    'conn-scroll',
    'conn-table-view',
    'conn-tbody',
    'conn-top-view',
    'log-count',
    'log-follow-state',
    'log-legacy-note',
    'log-privacy-note',
    'log-search',
    'log-view',
    's-connections',
    's-logs',
    'top-host-n',
    'top-hosts',
    'top-outbounds',
  ],
};
