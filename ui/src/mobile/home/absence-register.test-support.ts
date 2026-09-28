/**
 * 首页的**与桌面逐块对差登记表**。形态与口径见 `../parity-registry.ts`；
 * 门在 `../screen-parity.test.ts`（取材面、射程自曝、反向对照都写在那份文件的头注里）。
 *
 * 桌面取材面 = `components/screens/home/` 整个目录（`HomeScreen` / `NodeMenu` /
 * `ConnectionTopology` / `TsExitWarning` / `MeshTunnelHealth` / `ReverseRoutingBadge`）。
 * 门走目录，不走写死的文件清单 —— 桌面在这个目录里新加一个文件，它的按钮与块自动进面。
 *
 * # 这张表第一次让三件事可见，三条现在都接上了（2026-09-13）
 *
 *  · **直连 / 阻断出口**。桌面的出口下拉里这两颗是一等公民（`NodeMenu.tsx` 顶上两行），
 *    移动端首页的选择面此前只列 `pickRows`（全是真节点），全树零 `DIRECT_SERVER_ID` 消费点，
 *    而界面上**一个字都没提** —— 既不在 i18n 面、也不在控件面，只有这张表看得见它。
 *    现在两行落在节点选择面顶上，写腿走 `update({ selectedServerId })`（哨兵不过 `server_switch`）。
 *  · **拓扑图上「给这个主机加一条规则」**（`ConnectionTopology` 的右键面板四颗）。Sankey 图本身
 *    没有搬（360px 上不可读，且首页八张卡是 IA 定死的），但它承载的这条能力接在了承载同一批数据的
 *    主机 Top 行上 —— 换形态不算缺席，逐颗的锚见下面各条。图上**筛选**那一维 2026-09-13（批 18）
 *    也接上了，落在同一张卡内部（`home.clear`）：此前判 `absent`，理由是「主机 Top 固定 5 行，
 *    不构成在大集合里定位的问题」—— 那句话只在「筛那 5 行」的读法下成立，而筛的应当是整个活动
 *    连接窗口（先筛后排名），于是问题是真的、能力是缺的。
 *  · **空态那两颗直达入口**（`home.addServer` / `home.addSubscription`）。它们与节点屏那张表的
 *    `nodes.mobileNeedsFormLayer` 是同一条腿，而那一层（`mobile/forms/`）批 2 已经落地 ——
 *    此前缺的只是首页这个入口。
 *
 * ⚠️ 三条都不是「桌面新加的」，是移动端从第一天起就没有、而没有任何门看得见的。
 */
import type { ScreenParityRegister } from '../parity-registry.test-support';

export const HOME_PARITY: ScreenParityRegister = {
  screen: 'home',
  desktopDirs: ['ui/src/components/screens/home'],
  mobileDir: 'ui/src/mobile/home',

  actionFaces: ['controls'],
  primitiveModules: [],
  // 「给这个主机加一条规则」的菜单项片段（拓扑图右键面板的内容）住在 `components/` 下，
  // 但它渲染的是 `rules.*` 屏级文案 —— 按 ⓪ 组那条判据它是**内容**，不是通用原语。
  extraBlockSources: ['ui/src/components/RuleSubjectMenuItems.tsx'],
  sharedPrimitives: ['ui/src/components/FlagImg.tsx'],
  slotHandoffGates: [
    { file: 'ui/src/mobile/mobile-chrome.test.tsx', mustContain: '③ 反向分流指示器（真机 2026-07-20 §1.4 的移植）' },
    { file: 'ui/src/mobile/home/home-screen.test.tsx', mustContain: '① DOM 序 = 重要性序，不许被断点重排' },
    { file: 'ui/src/mobile/home/home-screen.test.tsx', mustContain: '④ 写失败必有可见回显（IA 裁定 #14）' },
  ],
  actions: [
    /* ── ConnectionTopology（桌面首页底部的 Sankey 流量拓扑）─────────────────── */
    {
      id: 'ConnectionTopology.tsx|k:home.clear',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'data-host-filter="clear"' },
      },
      note: '桌面拓扑图**主机筛选框**的清除键。🔴 2026-09-13（批 18）接上：整条筛选维度落在主机 Top 卡内部（首页八张卡由 IA §2.1 定死，不许为它新开一张），筛的是**整个活动连接窗口**再排名取前 5 —— 判据 `filterByHostQuery` 是后端 `project_connections_topology_iter` 那段 filter 的 1:1 转写（host 或 outbound 任一含子串，大小写不敏感），桌面那个框发的 `api.stats.projectTopology` 走的就是它。上一版这条判着 `absent`，理由是「主机 Top 固定 5 行，不构成在大集合里定位的问题」—— 那句话只在「筛已经画出来的那 5 行」的读法下成立；筛完整窗口之后，排在第 30 位的域名才够得着，问题是真的。⚠️ 本屏另有一颗叫 `home.clear` 的清除键（「合并进已有规则」选择器的搜索框），**那不是这一条**：两颗只是恰好同名，服务的是两个对象。故本条的锚指 `data-host-filter="clear"` 而**不指** `aria-label={t(\'home.clear\')}` —— 后者两颗都命中，拿它当锚等于「把规则选择器的搜索删掉这条账照样绿」。正面事实断言在 `home-screen.test.tsx` ⑭（两颗同屏并存、各自认得出）。',
    },
    {
      id: 'ConnectionTopology.tsx|k:home.ruleProxy',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "props.onQuickRule('proxy')" },
      },
      note: '给这个主机直接加一条走代理的规则（零输入）。桌面挂在拓扑图的右键面板上，移动端挂在主机 Top 行开出来的同一张面板上。写腿与桌面 `addSubjectRule` 逐字同形：同一个 rule 形状、同一道 `trafficRules` 暂存闸门、同一句 `home.ruleRemarks` 默认备注。',
    },
    {
      id: 'ConnectionTopology.tsx|k:home.ruleDirect',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "props.onQuickRule('direct')" },
      },
      note: '同上，加一条直连规则。两颗共用一条腿（接线层的 `addSubjectRule`），一并登记，避免只判一半。',
    },

    /* ── HomeScreen（连接控制卡）─────────────────────────────────────────────── */
    {
      id: 'HomeScreen.tsx|k:home.addServer',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "t('home.addServer')" },
      },
      note: '空态（一个节点都没有、且没选哨兵出口）时的「添加节点」直达入口。桌面那颗落到桌面的造节点弹窗，移动端落到自己的表单宿主（`mobile/forms/` 的 `{kind:\'node\'}`）—— 同一条能力换了宿主。空态判据与桌面 `emptyState` 逐字同源，哨兵那半句不能省。',
    },
    {
      id: 'HomeScreen.tsx|k:home.addSubscription',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "t('home.addSubscription')" },
      },
      note: '空态时的「添加订阅」直达入口，与上一条同一族，落到表单宿主的 `{kind:\'sub\'}` 那一支。两颗并排在节点卡上，与桌面同一个位置语义。',
    },
    {
      id: 'HomeScreen.tsx|#node-trigger',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "t('home.switchNodeTip')" },
      },
      note: '出口节点选择器。移动端是整张节点卡按下去开一张 sheet（`.h-sheet`），复用桌面同一条 tip 键。',
    },
    {
      id: 'HomeScreen.tsx|k:home.networkCheckTip+home.unlockCooldown',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'data-write-control="network-check"' },
      },
      note: '网络检测（延迟 + 解锁重检 + 出口 IP 重探三合一）。移动端在动作行第二颗。',
    },
    {
      id: 'HomeScreen.tsx|#connect-btn',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'data-write-control="connect"' },
      },
      note: '连接 / 断开。移动端在动作行第一颗，五态与桌面同源。',
    },
    {
      id: 'HomeScreen.tsx|k:home.tunSettingsTip',
      disposition: {
        kind: 'reachable-elsewhere',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "tun: 'TUN'" },
      },
      note: '「接管方式」标签行右边那颗齿轮 —— 它只是一条通往 TUN 设置页的捷径。移动端设置屏有 TUN 二级页，能力在，缺的只是首页这条捷径。',
    },
    {
      id: 'HomeScreen.tsx|f:onInterceptChange',
      disposition: {
        kind: 'platform-absent',
        // 锚指**平台判定本身**：`effective_on` 的 Android 臂把任何存量档位都判成 `Tun` —— 那条臂的依据是
        // 「系统代理在 Android 上没有承载物」（非 root 应用无权改全局 HTTP 代理设置）。
        // 不指移动端自己那个 `MOBILE_TAKEOVER` 常量 —— 后者是这条事实的**后果**，拿它当依据等于自证。
        evidence: { file: 'crates/config-engine/src/user_config/proxy_mode.rs', mustContain: 'Platform::Android => Self::Tun,' },
      },
      note:
        '接管方式二选一（TUN / 系统代理）。🔴 2026-09-25 按主因重锚（盘点 §4.2「标签偏宽」）：' +
        '这颗二选一的另一侧是**系统代理**，而 Android 上非 root 应用无权改全局 HTTP 代理设置' +
        '（`VpnService.Builder.setHttpProxy` 只是随 VPN 存亡的建议，不是一个可切换的接管形态）—— ' +
        '这是平台事实，与我们发不发 mixed inbound 无关，故仍判 `platform-absent`，锚从 `inbounds.rs` 的 ' +
        'mixed 守卫（那是**安全裁定**，只是次因）改指 `effective_on` 的 Android 臂。' +
        '移动端把这一维整个删掉（连同那个只会有一种取值的 sheet 状态机），不是留一颗永远选不动的按钮。',
    },
    {
      id: 'HomeScreen.tsx|k:home.networkSettingsTip',
      disposition: {
        kind: 'reachable-elsewhere',
        mobile: { file: 'ui/src/mobile/settings/settings-pages.ts', mustContain: "network: 'settings.nav.network'" },
      },
      note: '「分流策略」标签行右边那颗齿轮，同样只是通往网络设置页的捷径。',
    },
    {
      id: 'HomeScreen.tsx|f:onRoutingChange',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'data-write-control="routing"' },
      },
      note: '分流策略三档。移动端落成模式芯片（`mode-chips` 卡），写腿走同一个 `updateProxyMode`。',
    },

    /* ── NodeMenu（桌面出口下拉的内部）───────────────────────────────────────── */
    {
      id: 'NodeMenu.tsx|k:home.routingDirect',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'data-exit-write="home-sentinel-direct"' },
      },
      note: '把出口设成「直连」（`DIRECT_SERVER_ID` 哨兵）。移动端在节点选择面顶上，与桌面出口下拉的 `.mi` 同一个位置语义。写腿走 `update({ selectedServerId })` 而不是 `switchServer` —— 后端 `server_switch` 只收真实节点 id，哨兵会被拒，这一条逐字同桌面 `onPickDirectExit`。',
    },
    {
      id: 'NodeMenu.tsx|k:home.routingBlock',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'data-exit-write="home-sentinel-block"' },
      },
      note: '把出口设成「阻断」（`BLOCK_SERVER_ID` 哨兵）。与上一条同一条写腿。直连模式下**在场置灰 + 写出原因**（`home.blockExitUnavailableInDirect`），判据与桌面 `blockDisabledReason` 同源：那时 route.final 恒为 direct，选中它是一次静默 no-op。',
    },
    {
      id: 'NodeMenu.tsx|c:ns-grp',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeNodePickerList.tsx', mustContain: 'groupServersBySubscription' },
      },
      note: '首页选择面复用桌面真实来源分组/defaultOpenGroupIds；搜索自动展开命中，清空后恢复手动折叠，重开只展开当前组。',
    },
    {
      id: 'NodeMenu.tsx|f:onPick',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeNodePickerList.tsx', mustContain: 'onUseAsExit(server)' },
      },
      note: '在下拉里选一个节点当出口。移动端是 sheet 里的一行，整行即动作。',
    },
    {
      id: 'NodeMenu.tsx|k:home.sortByLatency',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "t('home.sortByLatency')" },
      },
      note: '出口选择 sheet 的搜索框旁提供同一持久化开关，组内排序共用 sortServersByLatency；组次序和折叠态保持原样。',
    },
    {
      id: 'NodeMenu.tsx|k:nodes.testAll',
      disposition: {
        kind: 'reachable-elsewhere',
        mobile: { file: 'ui/src/mobile/nodes/view-model.ts', mustContain: "id: 'test-all', label: t('nodes.testAll')" },
      },
      note: '下拉底部的「全部测速」。移动端首页保留路由三格，能力在节点屏的「更多」菜单内；该菜单由 NodesScreenView 调用 buildNodeMoreItems 渲染。',
    },
    {
      id: 'NodeMenu.tsx|k:home.manageNodesArrow',
      disposition: {
        kind: 'reachable-elsewhere',
        mobile: { file: 'ui/src/mobile/destinations.ts', mustContain: "{ id: 'nodes', labelKey: 'mobileNav.nodes' }" },
      },
      note: '下拉底部的「管理节点 →」。移动端节点屏是底部导航的一等目的地，不需要从首页兜一跳。',
    },

    /* ── TsExitWarning ─────────────────────────────────────────────────────── */
    /* ── ConnectionTopology 的图上命中区 + 右键面板（`RuleSubjectMenuItems`）───────── */
    {
      id: 'ConnectionTopology.tsx|k:home.recentTarget',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'props.onOpenRuleSubject(h.key)' },
      },
      note: '拓扑图上每个主机的命中区 —— 它承载两件事：悬停出「最近目标」注脚，点击开规则面板。**后者移动端接上了**（主机 Top 的每一行就是那个命中区）；前者两条腿都不成立：触屏没有悬停这一维，而「最近」这一位的数据源是 `aggregate` topic 的 `ConnectionAggHost.recent`，本屏按 `data-contract.json` 的陷阱条明确不订那个 topic（它的 count 是连接条数，按字节排名的卡一条都不能用）。',
    },
    {
      id: 'RuleSubjectMenuItems.tsx|k:rules.addNew',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "props.onRuleSubjectView('new')" },
      },
      note: '「新建规则」。桌面那颗开的是完整规则弹窗（可改类型、加条件、选具体出站），移动端开的是同一层弹层的第二个视图：对象只读、动作三选一（代理 / 直连 / 阻断）、备注可改。**射程差异如实记在这里**：多条件与指定出站节点属于规则的完整编辑面，那一层归规则屏登记；从一个观测对象新建一条规则这条腿在移动端是完整的。',
    },
    {
      id: 'RuleSubjectMenuItems.tsx|k:rules.addExisting+rules.subjectAlreadyInRule',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "props.onRuleSubjectView('pick')" },
      },
      note: '「合并进已有规则」（把这个主机追加到某条现成规则的条件里）。判据与变换整套复用 `components/dialogs/rule-append.ts` 的纯函数（`ruleAppendTargets` / `sortAppendTargets` / `matchAppendTargets` / `appendSubjectToRule` / `analyzeRuleCoverage`），与桌面 `RulePickDialog` 同源；置灰三档各自带出路，「前面可能先命中」只提示不禁用。',
    },
    {
      id: 'TsExitWarning.tsx|f:isAuth',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "t('home.tsExitGoAuth')" },
      },
      note: '组网出口名不副实时那条注脚上的动作（去授权 / 去挑一个出口设备）。移动端两档都在，且按裁定 #3 紧贴会写 exit_node 的那颗控件。',
    },
  ],

  blocks: [
    {
      id: 'ConnectionTopology',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'className="h-hostrow act"' },
      },
      note: '桌面首页底部那张 Sankey 流量拓扑（设备 → 出站 → 目标三列）。**形态换了，不是缺席**：三列 Sankey 加连线与标签在 360px 宽上不可读，且首页八张卡是 IA §2.1 定死的集合（`HOME_CARD_ORDER` 那道门钉着「多一张就红」）。它承载的三维在移动端各有归属 —— 出站维在「流量构成」卡、规则维在「规则命中」卡、目标维在「域名 Top」卡；而**图上的动作**（给某个主机加一条规则）接在目标维那张卡的每一行上，四颗一颗不少（逐颗见 actions 面 `home.ruleProxy` / `home.ruleDirect` / `home.recentTarget` 与 `RuleSubjectMenuItems` 两条）。',
    },
    {
      id: 'MeshTunnelHealth',
      disposition: {
        kind: 'reachable-elsewhere',
        mobile: { file: 'ui/src/mobile/nodes/NodesScreenView.tsx', mustContain: 'data-mesh-tunnel-health=' },
      },
      note: '只走内网的组网节点的隧道健康注脚。移动端把它放在节点屏的组网分组里（IA 裁定 #3 后半），五档文案表逐字照抄桌面。',
    },
    {
      id: 'NodeMenu',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "title={t('home.nodesMenu')}" },
      },
      note: '出口选择面板。桌面是下拉，移动端是从底部升起的 sheet（带搜索），承担同一件事。',
    },
    {
      id: 'ReverseRoutingBadge',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "t('home.reverseRoutingBadge')" },
      },
      note: '地区分流反向（回国）时贴在分流标签行上的常驻指示器。它是 2026-07-20 那次真机事故的修复产物：没有它，用户会把「回国」读成「翻墙」，而两者的流量方向正相反。移动端同样常驻在模式芯片行上。',
    },
    {
      id: 'TsExitWarning',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "data-ts-exit-warning" },
      },
      note: '选中 Tailscale 节点当出口却出不了公网时的行内警示。跨屏义务见 IA 裁定 #3，另有 `nodes-screen.test.tsx` ② 两条腿守它与出口写控件的同 scope 关系。',
    },
    {
      id: 'UnlockBadge',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'detail.result.region.toUpperCase()' },
      },
      note: '解锁检测的逐服务徽章（品牌图标 + 状态 + 地区码）。它定义在 `HomeScreen.tsx` **文件内部**，上一版按「标签名对得上同目录文件名」取块时整条不可见。移动端在解锁卡的详情行上画同三格，地区码同样「缺省就整段不拼」。',
    },
    {
      id: 'NmCheck',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeNodePickerList.tsx', mustContain: "row.isCurrent ? ' cur' : ''" },
      },
      note: '出口下拉里「这一项就是当前出口」的对勾。移动端 sheet 用整行的 `cur` 样式承担同一件事 —— 换了呈现，回答的是同一个问题。',
    },
    {
      id: 'MiCheck',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: "props.node.kind === 'direct' ? ' cur' : ''" },
      },
      note: '哨兵出口（直连 / 阻断）那两行上的对勾。移动端这两行用整行的 `cur` 样式承担同一件事 —— 与 `NmCheck` 那条（节点行）同一个处置形态：换了呈现，回答的是同一个问题「这一项就是当前出口」。选中态取自 `node.kind`，与卡上那行身份文案同源，不另存一格。',
    },
    {
      id: 'RuleSubjectMenuItems',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/home/HomeScreenView.tsx', mustContain: 'function RuleSubjectPanel' },
      },
      note: '拓扑图右键面板整块（新建规则 / 合并进已有规则）。它住在 `components/` 下但渲染 `rules.*` 屏级文案 ⇒ 按 ⓪ 组那条判据它是这一屏的**内容**，进 `extraBlockSources`。移动端对应 `RuleSubjectPanel`：同一层弹层里的三个视图（菜单 / 新建 / 合并），判据与变换整套复用 `rule-append.ts` 的纯函数。逐颗的处置见 actions 面同名两条。',
    },
  ],

  /**
   * 桌面首页的具名数据槽基线（22 个）。器，不是对差面**（见 `parity-registry.ts` 的字段注释）。
   * 桌面加一个槽 ⇒ 这条不等 ⇒ 红 ⇒ 逼一次「移动端要不要画」的决定。
   */
  slotBaseline: [
    'connect-btn',
    'connect-ic',
    'cur-addr',
    'cur-node',
    'cur-proto',
    'exit-flag',
    'home-meta',
    'home-mode-line',
    'home-node-dd',
    'home-runtime',
    'home-uptime',
    'linkGrad',
    'nm-search-inp',
    'node-menu',
    'node-trigger',
    's-home',
    'sankey-fallback',
    'sankey-stub',
    'sankey-wrap',
    'topo-card',
    'unlock-row',
    'unlock-ts',
  ],
};
