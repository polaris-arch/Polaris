/**
 * 规则屏的**与桌面逐块对差登记表**。形态与口径见 `../../parity-registry.ts`；
 * 门在 `../../screen-parity.test.ts`。
 *
 * 桌面取材面 = `components/screens/rules/` 整个目录（`RulesScreen` / `RuleItem` / `GeoCard` /
 * `PriorityFlow` / `DnsPolicyWorkspace`）。
 *
 * # 这张表第一次让一件事可见
 *
 * **DNS 服务器 / DNS 组的删除在移动端整条没有**。桌面那两颗（`DnsPolicyWorkspace.tsx` 里
 * 二次确认式的 danger 按钮）在移动端的 DNS 二级页上一个字都没提：那一页只有开关与
 * 「编辑不可用」两件事（`RulesScreen.tsx:1206` 那条 `editUnavailableReason`），
 * 删除既不在场、也不置灰、也没有理由行 ⇒ 此前不在 i18n 面、控件面、任何登记表上。
 * 它与「编辑」**不是同一条腿**：删一条 DNS 服务器不需要表单层，所以不能挂在
 * `mobileRules.formUnavailable` 底下混过去。
 */
import type { ScreenParityRegister } from '../../parity-registry.test-support';

export const RULES_PARITY: ScreenParityRegister = {
  screen: 'rules',
  // 🔴 三个桌面目录：移动规则屏用 `SegmentedTabs` 把桌面的 `rules` / `app-policy` / `resources`
  // 三整屏折成四段（`RulesScreen.tsx` 的 `<AppsSegment>` / `<ResourcesSegment>`）。
  // 上一版这里只有第一个，后两个（合计 20 颗动作）不在任何一面上。
  desktopDirs: [
    'ui/src/components/screens/rules',
    'ui/src/components/screens/app-policy',
    'ui/src/components/screens/resources',
  ],
  mobileDir: 'ui/src/mobile/screens/rules',

  actionFaces: ['controls'],
  primitiveModules: [],
  // 逐条规则 / 逐条应用规则 / 逐条资源的**详情面**住在 hover-cards：那是这一屏的内容，不是通用原语
  // （`rules.noConditions` 那句「（无条件）」就是从这条缝漏掉的）。
  extraBlockSources: [
    'ui/src/components/hover-cards/RuleHoverCard.tsx',
    'ui/src/components/hover-cards/AppRuleHoverCard.tsx',
    'ui/src/components/hover-cards/ResourceRefsHoverCard.tsx',
    // 网络场景表单里的清单编辑器（地址段 / 搜索域）。它**不是**原语：自己渲染屏级文案
    // （`settings.listImportHint`，批量导入的格式说明）⇒ 按 ⓪ 组判据进内容块面，块在 blocks 面逐个判。
    'ui/src/components/screens/settings/ListEditor.tsx',
  ],
  sharedPrimitives: [
    'ui/src/components/Fold.tsx',
    // 统一信息提示图标（网络场景面板的字段说明挂在它上面）：只渲染传进来的 tip，一句自有文案都没有。
    'ui/src/components/InfoIcon.tsx',
    'ui/src/components/dialogs/Csel.tsx',
    // 桌面弹窗外壳（网络场景面板的两层弹窗）：自有文案只有 `common.close`，它是容器不是内容。
    'ui/src/components/dialogs/Modal.tsx',
    'ui/src/components/hover-cards/HoverCard.tsx',
    'ui/src/components/screens/settings/Primitives.tsx',
  ],
  slotHandoffGates: [
    { file: 'ui/src/mobile/screens/rules/rules-screen.test.tsx', mustContain: '④ 内容顺序与 IA §2.2 对拍（DOM 顺序即重要性顺序）' },
    { file: 'ui/src/mobile/screens/rules/rules-screen.test.tsx', mustContain: '⑥ 写操作失败必须有**可见**回显（行内，贴着那颗控件）' },
  ],
  actions: [
    /* ── DnsPolicyWorkspace（DNS 服务器 / 组 的工作区）─────────────────────── */
    {
      id: 'DnsPolicyWorkspace.tsx|k:common.edit',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/DnsSegment.tsx', mustContain: "{t('common.edit')}" },
      },
      note: '编辑一条 DNS 服务器。二级页每一行一颗「编辑」，开表单宿主的 `dns-server` 那一层（`forms/DnsResourceFormPanel`）；校验与 Hosts 记录往返复用 `dialogs/dns-resource-logic`，写腿走同一条 `useConfig().update` 漏斗。',
    },
    {
      id: 'DnsPolicyWorkspace.tsx|f:requestDeleteDnsServer',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'const requestDeleteDnsServer = useCallback(' },
      },
      note: '删除一条 DNS 服务器。三道闸逐条同桌面：内置保护（`isProtectedDnsServer`）→ 被引用则拒绝并说清被谁占着（`dnsServerReferences`，与行上那个引用数**同一份**判据）→ `confirmTwice` 二次确认。',
    },
    {
      id: 'DnsPolicyWorkspace.tsx|k:common.edit#2',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "openMobileForm(isServers ? { kind: 'dns-server', serverId: id } : { kind: 'dns-group', groupId: id })" },
      },
      note: '编辑一条 DNS 组。与编辑服务器是同一颗按钮的另一支（同一页的两个 tab 共用一份 `DnsResourcePage`），按当前在哪一页分派到两张表。',
    },
    {
      id: 'DnsPolicyWorkspace.tsx|f:requestDeleteDnsGroup',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'const requestDeleteDnsGroup = useCallback(' },
      },
      note: '删除一条 DNS 组。与删除服务器同族、同一条护栏（`dnsGroupReferences` + `confirmTwice`），少的只是内置保护那一闸（组没有内置项）。',
    },

    /* ── GeoCard（地区分流）────────────────────────────────────────────────── */
    {
      id: 'GeoCard.tsx|k:rules.region.cn+rules.region.ir+rules.region.ru',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/TrafficSegment.tsx', mustContain: "rules.region." },
      },
      note: '三个地区档（CN / IR / RU）。移动端在流量分段的地区分流卡上，键逐字同源。',
    },
    {
      id: 'GeoCard.tsx|k:rules.backHome',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/TrafficSegment.tsx', mustContain: "t('rules.backHome')" },
      },
      note: '「回国」反向开关。它是首页那枚反向指示器的**写腿**：两端由同一个 `regionRouting.reverse` 驱动。',
    },

    /* ── RuleItem（一条规则的行动作）──────────────────────────────────────── */
    {
      id: 'RuleItem.tsx|k:rules.moveTop',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "t('rules.moveTop')" },
      },
      note: '置顶。移动端在行动作 sheet 里，四颗移动动作同批移植。',
    },
    {
      id: 'RuleItem.tsx|k:rules.moveUp',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "t('rules.moveUp')" },
      },
      note: '上移一位。四颗移动动作在移动端合成一张行动作 sheet，顺序与桌面一致（顺序即优先级，重排会让熟练用户重学一遍）。',
    },
    {
      id: 'RuleItem.tsx|k:rules.moveDown',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "t('rules.moveDown')" },
      },
      note: '下移一位。与上移共用同一条 `onMove` 写腿，一并移植，避免只搬一半留下一个只能往上走的列表。',
    },
    {
      id: 'RuleItem.tsx|k:rules.moveBottom',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "t('rules.moveBottom')" },
      },
      note: '置底。四颗里最容易被顺手省掉的一颗（长列表里它才是真正省事的那个），逐颗登记正是为了挡住这种省法。',
    },
    {
      id: 'RuleItem.tsx|k:rules.duplicate',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "t('rules.duplicate')" },
      },
      note: '复制一条规则。不经表单层（直接克隆一条已有规则），故移植得动。',
    },
    {
      id: 'RuleItem.tsx|k:common.edit',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "onEdit: (r) => openMobileForm({ kind: 'rule', ruleId: r.id, initialPlane: plane })" },
      },
      note: '编辑一条规则。行动作面里那一颗开表单宿主的 `rule` 那一层（`forms/RuleFormPanel`）：条件草稿走 `rule-cond`、候选池走 `use-rule-pools`、提交走 `rule-submit#submitRule` —— 判据一条都没有第二份实现。',
    },
    {
      id: 'RuleItem.tsx|k:common.confirmAgain+common.delete',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: 'confirming: deleteConfirming' },
      },
      note: '删除一条规则（二次确认）。移动端连 `.confirming` 这个与 confirm-twice 的契约类名一起搬了 —— 漏掉它这条删除永远确认不了。',
    },

    /* ── RulesScreen（屏级动作）───────────────────────────────────────────── */
    {
      id: 'RulesScreen.tsx|c:btn flow',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "openMobileForm({ kind: 'rule', initialPlane: segment === 'dns' ? 'dns' : 'route' })" },
      },
      note: '新建。桌面那颗 `+` 按当前视图分派三条腿；移动端把服务器 / 分组做成了二级页 ⇒ 分派落在「你现在在哪」上：根页的流量 / DNS 段开规则表（带对应 plane），两个二级页各自开 DNS 服务器 / 分组表。',
    },
    {
      id: 'RulesScreen.tsx|f:setDnsView',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "tr(isServers ? 'rules.dnsWorkspace.serversTab' : 'rules.dnsWorkspace.groupsTab')" },
      },
      note: 'DNS 平面的三个视图切换。移动端把「规则」留在分段里、把「服务器 / 组」各做成一张二级页，切换这一维仍在。',
    },
    {
      id: 'RulesScreen.tsx|k:rules.backToSmart',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "tr('rules.backToSmartOk')" },
      },
      note: '非智能分流模式下的「切回智能」。移动端同一条写腿，成功/失败都有行内回显。',
    },
    /* ── 行内开关（`role="switch"` 的 `<span>`，上一版只认 `<button>` 时整批不可见）──── */
    {
      id: 'GeoCard.tsx|k:rules.regionRouting',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/TrafficSegment.tsx', mustContain: "label={t('rules.regionRouting')}" } },
      note: '地区分流总开关。移动端在流量段同名一行，写腿与暂存回显同源。',
    },
    {
      id: 'RuleItem.tsx|k:rules.toggleEnabled',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "rules.toggleEnabled" } },
      note: '逐条规则的启停开关。移动端在规则行动作面里同名一颗。',
    },
    {
      id: 'Primitives.tsx|f:disabled',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function Switch' } },
      note: '开关原语本身（`screens/settings/Primitives.tsx` 的 `<Switch>`，规则屏 import 它画地区分流那颗）。渲染一跳把它带进动作面；移动端规则屏有自己的 `Switch` 原语，命中盒与行为由 `rules-screen.test.tsx` 守。',
    },

    /* ── 自定义下拉（渲染一跳；桌面规则屏用它选规则类型 / DNS 目标）────────────── */
    {
      id: 'Csel.tsx|f:preventDefault',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function ActionSheet' } },
      note: '自定义下拉的触发器。移动端换成从底部升起的 sheet（`select-sheet` / `action-sheet` 两套词汇按 IA §3.3 仍是两套）。',
    },
    {
      id: 'Csel.tsx|f:choose',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function CheckIcon' } },
      note: '在下拉里选中一项（带选中勾）。移动端 sheet 的选中勾逐字照抄桌面 `.csel-ck` 的路径与线宽（IA §3.1 #20 明写它要跟过来）。',
    },
    {
      id: 'Csel.tsx|f:header',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'mr-sel-grp mr-sel-grp-t' },
      },
      note: '下拉里的分组组头。`SelectSheetPanel` 现在吃 `CselGroup[]`，行的摊平走**同一个** `csel-logic#buildCselRows`：不带 id 的组是纯标签（规则类型 15×5），带 id 的组是可点的展开钮（节点按订阅分组）。这一维在 390 宽的屏上比桌面更要紧 —— 几十个节点平铺等于一条读不完的滚动条。',
    },

    /* ── 应用分流段（桌面 `components/screens/app-policy/` 整屏折进本屏）────────── */
    {
      id: 'AppPolicyScreen.tsx|k:appPolicy.masterTip+appPolicy.masterToggleFail',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: "t('appPolicy.masterTip')" } },
      note: '应用分流总开关（`role="switch"` 的 `<span>`）。移动端在应用段顶部同名一颗，失败回显走本屏统一的行内错误位。',
    },
    {
      id: 'AppPolicyScreen.tsx|k:appPolicy.backToSmart',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: "t('appPolicy.backToSmart')" } },
      note: '「切回智能分流」（非智能模式下应用策略不生效时出现的补救键）。移动端同名一颗，写腿同源。',
    },
    {
      id: 'AppPolicyScreen.tsx|k:appPolicy.view.cards',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: "t('appPolicy.view.cards')" },
      },
      note: '切到卡片视图。上一版不移植，理由是「compact 下卡片墙就是一行一张，两种排布收敛到同一个东西」—— 那句话只在 compact 成立：`Cards` 在 medium / expanded 下是双列（平板与折叠屏展开态），与单列的行不是同一个东西。两种排布承载的第二格也不同（卡片给分类、行给进程名），同桌面。',
    },
    {
      id: 'AppPolicyScreen.tsx|k:appPolicy.view.list',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: "t('appPolicy.view.list')" },
      },
      note: '切到列表视图。与上一条是同一颗二选一的另一半，走同一个 `Choice`（桌面那颗二选一按钮组的对位）。',
    },
    {
      id: 'AppPolicyScreen.tsx|k:appPolicy.addCustom',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'onClick={onAddCustom}' },
      },
      note: '添加自定义应用。开表单宿主的 `app-add` 那一层（`forms/AppAddPanel`）：名称 + 资源标签（geosite 必填，那是这台设备上唯一有效的匹配面）+ emoji + 进程名，写腿走 `mutateConfigEntities` + `customAppPresets` 暂存闸门。',
    },
    {
      id: 'AppPolicyScreen.tsx|f:onRemove',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'const removeButton = (row: AppRowModel)' },
      },
      note: '列表视图里删除一个自定义应用。与卡片视图共用同一颗 `removeButton` —— 两个渲染位一份实现，不会出现「一个视图删得掉另一个删不掉」。',
    },
    {
      id: 'AppPolicyScreen.tsx|f:onRemove#2',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: '{removeButton(row)}' },
      },
      note: '卡片视图里删除一个自定义应用（同一条腿的第二个渲染位）。',
    },
    {
      id: 'AppPolicyScreen.tsx|f:onClick',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'const requestRemoveCustomApp = useCallback(' },
      },
      note: '删除键本体的写腿。**预设与它的 appRule 在同一个事务里删**（同桌面）：只删预设会在盘上留一条指向不存在应用的孤儿规则，它不报错、不显示，而 route builder 照样按它发规则。',
    },
    {
      id: 'AppPolicyScreen.tsx|f:toggleMenu',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'const policyTrigger = (row: AppRowModel)' },
      },
      note: '逐应用的策略选择器触发器（`role="button"` 的 `<span>`）。移动端是同一行 / 同一张卡上的一颗策略芯片（`SelectSheetTrigger`），点开 sheet 选档；两个视图共用同一颗，不会出现「一个视图点得开另一个点不开」。',
    },
    {
      id: 'AppPolicyScreen.tsx|f:pick',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'const handlePick = (appId: string, value: string)' },
      },
      note: '在策略菜单里落一档。2026-09-13 起四档**全部**可写（此前「指定节点 / 阻断」两档置灰，那两条债在控件面上记着）：面板给回一个值编码，`handlePick` 解回「哪一档 + 哪个节点」再交给容器，写腿与桌面同源（`appRuleForPick`）。',
    },
    {
      id: 'AppPolicyScreen.tsx|c:ns-grp',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'groups={policyGroups}' },
      },
      note: '策略菜单里按订阅分组的可折叠组头。分组与默认展开集复用 `domain/server-grouping` 的 `groupServersBySubscription` / `defaultOpenGroupIds`（桌面三处选择器共用的同一份判据），组头本体由 `SelectSheetPanel` 渲染。',
    },
    {
      id: 'AppPolicyScreen.tsx|c:mi ns-node',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'value: `node:${server.id}`' },
      },
      note: '在策略菜单里给这个应用指定一个具体节点。**「指定节点」本来就不是一个档位**，它是节点列表本身 —— 上一版把它压成一个点不动的档位，那一压正是这一跳整条消失的原因。选中后落 `{ action:\'proxy\', targetServerId }`（`appRuleForPick` 的第三参）。',
    },

    /* ── 网络场景（spec §6.1 / §6.2，桌面 `NetworkProfilePanel.tsx` 两层弹窗 + 规则页页头入口）────────
     * 形态差一句话说完：桌面「页头入口 → 列表弹窗 → 编辑弹窗叠在上面」，移动端「屏头溢出 → 二级页
     * （`NetworkProfilesPage`）→ 表单宿主一层（`forms/NetworkProfileFormPanel`）」，与 DNS 服务器 / 分组同形。
     * 判据（校验 / 落盘形态 / 引用计数 / 探测显示态）与后端探测结果两端同一份，见两个文件的头注。
     * 同一段 JSX 在桌面被数成两颗（`<ConfigShell render={…}>` 的开标签里含 `onClick`，外壳与里面那层
     * `<Modal>` 各算一颗，后者带 `#2`），两颗各自落在移动端对应的那一格上。 */
    {
      id: 'RulesScreen.tsx|k:rules.networkProfile.entry',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "label: tr('rules.networkProfile.entry')," } },
      note: '规则页的「网络场景」入口（两个平面共用，spec §6.1）。移动端屏头动作封顶两颗（§3.1 #10），入口进溢出面板：流量段与 DNS 段各一颗溢出、进同一张二级页。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close+rules.networkProfile.add+rules.networkProfile.intro',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "if (page === 'network-profiles') {" } },
      note: '场景列表弹窗的读取外壳（`<ConfigShell>` 那一颗）。移动端是规则屏的二级页分支：读取态（加载中 / 读失败 / 就绪）由容器从同一个 `useConfig()` 算出来交给页面。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close+rules.networkProfile.add+rules.networkProfile.intro#2',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/NetworkProfilesPage.tsx', mustContain: "{t('mobileRules.networkProfile.intro')}" } },
      note: '场景列表弹窗本体（说明 + 列表）。移动端二级页顶上同一句说明 —— 措辞换成「手机连着该网络时」（桌面那句写的是「电脑」，在手机上是错的），其余同源。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close#2',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'onClick={() => setPage(null)}' } },
      note: '关掉场景列表。移动端是二级页屏头的返回键（与系统返回键同一个动作，`useRulesStack` 登记的那一层）。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:rules.networkProfile.add',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "aria-label={tr('rules.networkProfile.add')}" } },
      note: '新建场景。二级页屏头的 `+`（与 DNS 服务器 / 分组二级页同一位置），开表单宿主的 `network-profile` 那一层。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.edit',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: "openMobileForm({ kind: 'network-profile', profileId: id })" } },
      note: '编辑一个场景。二级页每一行一颗「编辑」，开同一张表单（带 `profileId`）。',
    },
    {
      id: 'NetworkProfilePanel.tsx|f:confirmTwice',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'const requestDeleteNetworkProfile = useCallback(' } },
      note: '删除一个场景（二次确认）。桌面是原地二次点击、武装态里写「N 条规则将停止生效」；移动端叠一层确认面板（`ConfirmPanel`），同一句后果写在正文里 —— 触屏上武装态那一行在拇指底下被手指挡住（`destructive-confirm-wiring` 清册移动端段头注同一条理由）。引用计数走同一个 `profileRefCounts`。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close+common.configLoadFail+common.retry',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: 'if (cfg.config === null) {' } },
      note: '配置读取失败 / 加载中的外壳（桌面 `ConfigShell` 的非就绪分支）。移动端两处都有：表单宿主那层（锚）与二级页（`NetworkProfilesPage` 的 `loadState`）。读失败时**不**画空表 —— 空表一提交会拿「空集合 + 新场景」覆盖盘上全部场景。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: 'onRequestClose={close}' } },
      note: '读取外壳上的「关闭」。移动端表单宿主的关闭出口（返回键 / 遮罩 / 关闭键三条路径共用 `onRequestClose`）。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.retry',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: '() => void cfg.reload()' } },
      note: '读取失败后的「重试」。移动端表单那层与二级页各一颗（后者 `NetworkProfilesPage` 的 `onRetry`），都落到 `useConfig().reload`。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.add+common.bulkImport+common.cancel+rules.networkProfile.cidrs+rules.networkProfile.cidrsHint+rules.networkProfile.cidrsPh+rules.networkProfile.criteriaHint+rules.networkProfile.domains+rules.networkProfile.domainsHint+rules.networkProfile.domainsPh+rules.networkProfile.enabled+rules.networkProfile.enabledHint+rules.networkProfile.name+rules.networkProfile.namePh+rules.networkProfile.probe+rules.networkProfile.probeAuto+rules.networkProfile.probeDhcp+rules.networkProfile.probeHint+rules.networkProfile.probeSystem',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: 'function ProfileForm(' } },
      note: '场景编辑表单整块（名称 / 地址段 / 搜索域 / 探测方式 / 启用）。移动端同五格；三格说明从 InfoIcon 落成常驻行（§4.12）；探测方式是 `m-form-seg` 三段，DHCP 那一档在本机不可用时置灰并写原因（后端 `builtinDhcpStatus`，与 dhcp 源场景同一个判据）。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.cancel',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: "cancelLabel={tr('common.cancel')}" } },
      note: '表单取消（脏表单先叠一层放弃确认，同桌面）。',
    },
    {
      id: 'NetworkProfilePanel.tsx|c:btn flow',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: "submitLabel={tr(isEdit ? 'common.save' : 'common.add')}" } },
      note: '表单提交。校验与落盘形态走同一组 `validateNetworkProfileDraft` / `buildNetworkProfile`，写腿同一条 `useConfig().update`；带 `onSaved` 打开时（规则表单「新建场景…」）建完回填。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close+rules.networkProfile.missing',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: 'if (profileId && !base) {' } },
      note: '编辑的场景已不在（外壳那一颗）。移动端同一分支：说清「已被删除」，不悄悄变成新建。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close+rules.networkProfile.missing#2',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: "{t('rules.networkProfile.missing')}" } },
      note: '「这个网络场景已被删除」那张卡本体。移动端同一句（同键）。',
    },
    {
      id: 'NetworkProfilePanel.tsx|k:common.close#3',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: "cancelLabel={t('common.close')}" } },
      note: '「已被删除」那张卡的关闭。移动端脚上唯一一颗（没有提交键）。',
    },
    /* ── 渲染一跳：桌面弹窗外壳 `Modal` 与清单编辑器 `ListEditor`（网络场景面板 import 进来）──── */
    {
      id: 'Modal.tsx|k:common.close',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/FormSheet.tsx', mustContain: 'closeClassName="m-form-x"' } },
      note: '弹窗右上角的关闭键。移动端每一层表单都是 `FormSheet`，同一位置同一颗（`m-form-x`），且与返回键 / 遮罩共用同一个关闭出口。',
    },
    {
      id: 'ListEditor.tsx|f:add',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: "<MobileButton onClick={() => commit([...draft, ''])}>{addLabel}</MobileButton>" } },
      note: '清单里加一条（场景表单的地址段 / 搜索域）。移动端表单用设置屏的 `MobileListEditor`（去重与草稿规则在 `@/domain/list-entries`，与桌面同一份）；这颗在那个组件身上。',
    },
    {
      id: 'ListEditor.tsx|k:common.delete',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'onClick={() => commit(draft.filter((_, i) => i !== index))}' } },
      note: '删掉清单里的一条。`MobileListEditor` 每行尾那颗叉。',
    },
    {
      id: 'ListEditor.tsx|f:setImportOpen',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: '<MobileButton onClick={() => setImportOpen((v) => !v)}>{importLabel}</MobileButton>' } },
      note: '批量导入（一次粘贴多行）。就地展开一块 `textarea`，不另起弹层。',
    },
    {
      id: 'ListEditor.tsx|k:common.confirm',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: 'commit(parseBulkEntries(importDraft, [...draft]));' } },
      note: '批量导入的确认。解析与去重走 `@/domain/list-entries#parseBulkEntries`，与桌面同一个函数。',
    },
    {
      id: 'ListEditor.tsx|k:common.cancel',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/settings/SettingsChrome.tsx', mustContain: '{cancelLabel}' } },
      note: '批量导入的取消（收起并丢掉草稿）。',
    },

    /* ── 规则资源段（桌面 `components/screens/resources/` 整屏折进本屏）────────── */
    {
      id: 'ResourcesScreen.tsx|k:resources.resetBuiltin+resources.resetBuiltinConfirm',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.resetBuiltin')" } },
      note: '恢复内置资源（二次确认）。移动端同名一颗，确认态用同一对文案键。',
    },
    {
      id: 'ResourcesScreen.tsx|k:resources.catalog',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.catalog')" } },
      note: '打开资源目录（从内置清单里挑一份加进来）。移动端同名一颗。',
    },
    {
      id: 'ResourcesScreen.tsx|k:resources.urlDownload',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.urlDownload')" } },
      note: '按 URL 下载一份外部资源（自建规则集的入口）。移动端同名一颗。',
    },
    {
      id: 'ResourcesScreen.tsx|k:resources.src.all+resources.src.builtin+resources.src.external',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.srcFilter')" } },
      note: '来源筛选（全部 / 内置 / 外部）三档轮转。移动端同三档，键同源。',
    },
    {
      id: 'ResourcesScreen.tsx|k:resources.updateAll',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.updateAll')" } },
      note: '一键更新全部资源。移动端同名一颗，后端命令与进度事件同源。',
    },
    {
      id: 'ResourcesScreen.tsx|k:resources.cancel',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.cancel')" } },
      note: '取消在途下载（后端用 select! 丢弃传输）。移动端同名一颗，后端命令同源。',
    },
    {
      id: 'ResourcesScreen.tsx|k:resources.retryNow+resources.update',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.retryNow')" } },
      note: '单条资源的更新 / 失败后重试（同一颗按状态换文案）。移动端同一颗、同两档文案。',
    },
    {
      id: 'ResourcesScreen.tsx|f:onDelete',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.deleteConfirmPlain')" } },
      note: '删除一条外部资源（被规则引用时确认文案不同）。移动端两档确认文案都在。',
    },
  ],

  blocks: [
    {
      id: 'DnsPolicyWorkspace',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RulesScreen.tsx', mustContain: 'dnsServerRows' },
      },
      note: 'DNS 服务器 / 组的工作区。移动端拆成两张二级页（同一份 `DnsResourcePage` 渲染），承担同一件事。',
    },
    {
      id: 'GeoCard',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/TrafficSegment.tsx', mustContain: "t('rules.regionRouting')" },
      },
      note: '地区分流卡（选地区 + 回国反向）。移动端在流量分段里。',
    },
    {
      id: 'PriorityFlow',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/TrafficSegment.tsx', mustContain: "t('rules.routingPriority')" },
      },
      note: '分流优先级流程图（哪一层先命中）。移动端落成一条纵向的优先级说明，回答同一个问题。',
    },
    {
      id: 'RuleItem',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: 'export function RuleRow' },
      },
      note: '一条规则的行。移动端是 `RuleRow`（整行 + 行动作 sheet），逐颗动作的处置见上面的 actions 面。',
    },
    /* ── 规则行内部的块（定义在 `RuleItem.tsx` 文件里，上一版按文件名取块时整批不可见）──── */
    {
      id: 'RuleMetaCounts',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "t(`rules.types.${c.type}.name`)" } },
      note: '规则行上的条件摘要（逐类型 + 每类几个值）。2026-08-03 真机实测后定下的产品形态，桌面另有 `RuleItem.meta-counts.test.tsx` 守它。移动端在行副标题上画同一串。',
    },
    {
      id: 'CondCount',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: '×{c.values.length}' } },
      note: '摘要里单个条件的「类型 ×N」。移动端逐字同形（`×` + 值个数）。',
    },
    {
      id: 'TypeIcon',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function TypeIcon' },
      },
      note: '规则行首的类型分类图标（五档）。五档字形逐字取自桌面 `RuleItem.tsx:90-129`，分类判据走 `RULE_TYPE_CATEGORY`（派生自 15 份描述符，不是第二张表）。上一版「摘要那行已经说了同一件事」的理由只对**摘要在场**的行成立：零条件规则的行上一个字都没有，这枚图标是仅剩的类型线索。',
    },
    {
      id: 'GripIcon',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function GripIcon' } },
      note: '拖拽排序手柄（六个圆点）。移动端逐字照抄桌面路径，并把长按拖拽接在同一颗上。',
    },
    {
      id: 'EditIcon',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function EditIcon' } },
      note: '编辑图标。移动端在行动作面里同一颗（那颗动作本身在场置灰，见 actions 面的 `common.edit`）。',
    },

    /* ── GeoCard 内部 ─────────────────────────────────────────────────────── */
    {
      id: 'InfoIcon',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/InfoIcon.tsx', mustContain: 'export function InfoIcon' } },
      note: '地区分流卡上那颗说明图标。移动端共享 InfoIcon 原语由规则 Primitives re-export，MobileInfo 同源消费，完整说明通过独立详情层查看。',
    },

    /* ── 应用分流段的块 ───────────────────────────────────────────────────── */
    {
      id: 'ApRow',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'AppRowModel' } },
      note: '列表视图里的一行应用（图标 + 名字 + 进程名 + 策略芯片）。移动端的应用段就是这一行的移植（`AppRowModel` 逐字段对应），进程名那一格另有一条通告条说明手机上永不命中。',
    },
    {
      id: 'AppCard',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'mr-card mr-app-card' },
      },
      note: '卡片视图里的一张应用卡（图标 + 名字 + 分类 + 策略选择器 + 自定义项的删除键）。与列表行的差别同桌面：第二格是**分类**而不是进程名。',
    },
    {
      id: 'AppIcon',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'brandIcon(row.id) ?? row.emoji' } },
      note: '应用图标（三态：图标代理 → 本地缓存 → emoji 兜底）。移动端**只用第三态**，不走图标代理的网络腿 —— 换了取值，不是缺了这一格；理由与取舍逐字写在 `AppRowModel.emoji` 的字段注释里。',
    },
    {
      id: 'PolicySelector',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'groups={policyGroups}' },
      },
      note: '逐应用的策略选择器整块（芯片 + 弹出菜单）。移动端是芯片 + 底部 sheet，结构与桌面同形：一组不可折叠的策略 + 若干组按订阅折叠的节点。',
    },
    {
      id: 'RemoveButton',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'removeConfirmingId === row.id' },
      },
      note: '自定义应用的删除键整块（含二次确认态）。只在 `isCustom` 的行/卡上渲染 —— 内置预设删不掉，画一颗点不动的删除键比不画更糟。武装态落 `.confirming`（与 `confirm-twice` 的跨文件契约）。',
    },
    {
      id: 'CheckMark',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function CheckIcon' } },
      note: '菜单里「当前就是这一档」的选中勾。移动端 sheet 的选中勾逐字照抄桌面 `.csel-ck`（IA §3.1 #20 明写它要跟过来）。',
    },
    {
      id: 'AppRuleHoverCardContent',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/AppsSegment.tsx', mustContain: 'policyText' } },
      note: '悬停在应用行上弹出的策略详情（当前动作 + 目标节点）。触屏没有悬停这一跳，移动端把同两格常驻在行上（`policyText` + `policyTone`）—— 换了触发方式，答的是同一个问题。',
    },

    /* ── 网络场景（`NetworkProfilePanel.tsx` 与 `RuleItem.tsx` 里的块）────────────────── */
    {
      id: 'ProfileList',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/NetworkProfilesPage.tsx', mustContain: 'export function NetworkProfilesPage(' } },
      note: '场景列表（判据摘要 · 引用计数 · 本机探测源 · 告警 · 启用开关 · 编辑 · 删除）。移动端二级页逐格同；停用的场景只显示「已停用」、不显示探测结果（同桌面 `profileRowStatus`）。',
    },
    {
      id: 'ProfileForm',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: 'function ProfileForm(' } },
      note: '场景编辑表单。见 actions 面同名那一条。',
    },
    {
      id: 'ConfigShell',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: 'if (cfg.config === null) {' } },
      note: '读取态外壳（加载中 / 读失败带重试）。移动端表单那层与二级页各有一份同形分支。',
    },
    {
      id: 'Field',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: '<span className="m-form-req" aria-hidden>' } },
      note: '字段行（标签 + 必填星 + 说明）。移动端 `m-form-row` 同形；说明不挂 InfoIcon，落成常驻 `m-form-hint`（§4.12）。',
    },
    {
      id: 'ProbeLine',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: '{probeDisplayText(display, t, mobileReasonKey)}' } },
      note: '「本机将使用：… / 不可用（原因）」一行。文案函数与桌面同一个（`network-profile-probes#probeDisplayText`），只把 `dhcpNeedsPrivilege` 的措辞换成手机上那一句（`network-profile-copy`）；二级页每行同一行。',
    },
    {
      id: 'ProbeWarn',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: '{tr(warningKey)}' } },
      note: '后端告警（`dhcpIpv6Only`：规则照常生成但地址段永不命中）。判据只在后端，移动端按码渲染；二级页每行同一句。',
    },
    {
      id: 'MatchDot',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: '<MatchDot match={badge.match} label={matchText} />' } },
      note: '命中态圆点（N4，内核 canary 的结果；命中 / 未命中 / 未知三态，未知是空心圆）。组件本身两端共用（`components/screens/rules/MatchDot.tsx`），移动端画在规则行徽标里与场景二级页每行的命中态行首；桌面的 `.dot` 样式不进移动链，`rules-screen.css` 限定 `.mr-row` 同值重画。',
    },
    {
      id: 'MatchLine',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/NetworkProfilesPage.tsx', mustContain: '<MatchDot match={row.match.state} label={row.match.text} />' } },
      note: '场景列表行的命中态一行（圆点 + 文案）。移动端二级页每行同一行（`mr-note mr-np-match`），取值同桌面 `profileRowStatus(...).match`：只在探测源可用时出现；拿不到后端结果 / 核未运行 / 网络刚变化 ⇒「未知」，不猜。',
    },
    {
      id: 'ProfileIcon',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/Primitives.tsx', mustContain: 'export function NetworkProfileIcon' } },
      note: '网络场景字形（桌面画在两层弹窗标题与页头入口上）。移动端表单宿主没有标题图标位，字形落在入口那一格（溢出面板里「网络场景」那一行），路径逐字同桌面。',
    },
    {
      id: 'NetworkProfilePill',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "t('rules.networkProfile.badge', { name: badge.name })" } },
      note: '规则行上的场景徽标（角标⑤，判据同一个 `ruleProfileBadge`）。五档逐档同色；桌面挂在 tip 上的说明，不生效的四档落成行下常驻说明（§4.12），正常那一档跟着溢出面板的详情走。',
    },
    {
      id: 'ListEditor',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/forms/NetworkProfileFormPanel.tsx', mustContain: '<MobileListEditor' } },
      note: '清单编辑器整块（地址段 / 搜索域两处）。移动端是设置屏的 `MobileListEditor`（添加 / 删除 / 批量导入 / 确认 / 取消五条腿齐），去重与草稿规则两端同一份。',
    },

    /* ── 规则资源段的块 ───────────────────────────────────────────────────── */
    {
      id: 'ResRow',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: 'ResourceRowModel' } },
      note: '一行规则资源（名字 + 来源 + 大小 + 更新时间 + 引用数）。移动端同一行，大小与更新时间两格 2026-09-06 已接（`data-contract.json#sources.rule-resources` 已登记）。',
    },
    {
      id: 'RefBadge',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.refCountAria'" } },
      note: '资源行上的「被几条规则引用」角标（0 引用时是「未被引用」）。移动端同一格，无障碍标签同键。',
    },
    {
      id: 'ResourceRefsHoverCardContent',
      disposition: { kind: 'ported', mobile: { file: 'ui/src/mobile/screens/rules/ResourcesSegment.tsx', mustContain: "t('resources.refSystemBaseline'" } },
      note: '悬停在引用角标上弹出的「被哪几条规则引用」明细。移动端把同一份信息落在行内（含「系统基线」那一档），不另开悬浮层。',
    },

    /* ── 逐条规则的详情面（住在 `components/hover-cards/`，靠 `extraBlockSources` 进面）──── */
    {
      id: 'RuleHoverCardContent',
      disposition: {
        kind: 'ported',
        mobile: { file: 'ui/src/mobile/screens/rules/RuleRow.tsx', mustContain: "{t('rules.noConditions')}" },
      },
      note: '逐条规则的详情面。触屏没有悬停 ⇒ 它跟着行尾那颗溢出键被点开。四格逐条补齐（此前缺）：零条件那一档（`rules.noConditions`，此前渲染出一片空白）、与/或标记（行上那个 `∧` / `·` 连接符读不出「全部满足」还是「满足任一」）、停用角标（判据是**裸 `rule.enabled`**，与行上那个按执行平面算的 `enabled` 不是一回事）、路由 / DNS 生效摘要（行上的药丸没有标签，说不出哪条是路由哪条是 DNS）。',
    },
  ],

  /**
   * 桌面规则屏**三个目录**（rules + app-policy + resources）的具名数据槽基线。
   * **变更探测器，不是对差面**。2026-09-06 从 7 涨到 13：涨的六个（`ap-*` / `s-apppolicy` /
   * `s-resources`）是折进来的那两屏本来就有的槽，此前整批不在任何一面上。
   */
  slotBaseline: [
    'ap-body',
    'ap-cat-filter',
    'ap-content',
    'ap-wfp-note',
    'rule-count',
    'rule-list',
    'rules-body',
    'rules-manual-note',
    'rules-mode-note',
    'rules-mode-warn',
    'rules-mode-warn-tx',
    's-apppolicy',
    's-resources',
  ],
};
