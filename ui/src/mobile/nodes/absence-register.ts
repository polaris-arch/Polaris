/**
 * 节点屏的**能力缺席登记表**（IA spec §3.3 第 2 条 + §4 的登记口径）。
 *
 * 规则本体（照抄 §3.3-2，因为它是本表存在的全部理由）：
 * >「一个被移除的控件要么**缺席且有理由**，要么**在场且置灰且有理由**——绝不许悄无声息地没了。」
 * 桌面自己就守着这条线并写明了为什么（`NodeCard.tsx:371-374`：
 * 「按钮凭空少一个，用户只会以为这张卡坏了」）。
 *
 * # 为什么登记表是**代码**而不是文档里的一段话
 *
 * 写在设计文档里的表对实现没有强制力：桌面加第七颗按钮时，没有任何东西会红。
 * 本表是可执行的：`nodes-screen.test.ts` 拿它与桌面 `.nd-acts` 的按钮数逐个对差，
 * 桌面多一颗而这里没登记 ⇒ 当场红，逼一次「移动端怎么处置」的显式决定。
 *
 * # 三种处置，没有第四种
 *
 * 🔴 **这三个码回答的是「这条腿在移动端有没有」，不是「画成什么样」**（2026-09-06 改正）。
 * 上一版把 `absent` 定义成「不渲染」，而实现里 clone/edit/delete 三条 `absent` 恰恰是**在场置灰**
 * （`MobileNodesScreen.tsx:609-618` 逐条 push 带 `disabledReason` 的 SheetRow）—— 定义与实现分叉，
 * 且分叉的方向是「文档说藏了、代码其实画着」，读代码的人会照定义去找一个不存在的隐藏分支。
 *
 * `ported`  —— 已移植，移动端有等价入口。**两端由同一份状态/协议判据驱动的控件也算这一档**，
 *              哪怕它在某些状态下是灰的：那是运行期状态，不是移植缺口。
 * `disabled`—— 移动端**这条腿在**，但被移植之外的原因恒不可用，且带一句已本地化的理由。
 * `absent`  —— 移动端**没有这条腿**（「结构上永远不会好」或「那条腿还没移植」），理由登记在此。
 *
 * **渲染形态由消费方按位置决定，不由本码决定**，两种都在用、两种都不是「悄悄没了」：
 *  · 行动作面（纵向可滚的表）：**在场置灰 + 理由** —— `view-model.ts#buildRowItems` 的
 *    「登记成缺席」那一支；
 *  · 停靠条（拇指区一共放得下四五个目标，一颗永远死的会挤掉一颗活的）：**不渲染** ——
 *    `NodesScreenView.tsx` 的批量条只画 `ported` 的那几颗，缺的在 `BATCH_ACTIONS` 里逐颗登记。
 * （**不写行号**：行号会被上方任何一次无关改动推走，那是脆而不是严 —— 同
 *   `wiring-completeness.test.ts` 里 anchors 那条口径。）
 *
 * ⚠️ 无论哪种形态都**不等于**「悄悄没了」：它在这张表里、在设计文档里、在门里各出现一次。
 *    §4 那句话是本表的验收口径 —— 缺席是产品边界，缺席而不登记才是缺陷。
 */

/** 处置码。三选一，穷举 `switch` 会因新增档位而编译不过。 */
export type Disposition =
  | { readonly kind: 'ported' }
  | { readonly kind: 'disabled'; readonly reasonKey: string }
  | { readonly kind: 'absent'; readonly reasonKey: string };

/**
 * 桌面节点卡 `.nd-acts` 的六颗按钮（`NodeCard.tsx:370-494`），逐颗登记移动端处置。
 *
 * `desktopRef` 是**取材面自检**用的：门会断言这段字面量真的出现在 `NodeCard.tsx` 里，
 * 否则本表可能在给一批并不存在的控件编处置（登记表变僵尸的典型形态）。
 */
export interface RowActionEntry {
  readonly id: string;
  readonly desktopRef: string;
  readonly disposition: Disposition;
}

/*
 * 逐条的处置理由写在**注释**里而不是字段里：本仓的 `i18n-coverage` 门把源码里的裸中文字面量
 * 一律判红（新界面写死中文 ⇒ 红），而这些理由是给读代码的人看的、不该也不会进 UI。
 * 注释是它们正确的载体；进 UI 的那一句由 `reasonKey` 指向 locale。
 */
export const ROW_ACTIONS: readonly RowActionEntry[] = [
  /* 测速：**不是缺席，是 `ported`**（2026-09-06 改判）。§4.10 —— 核没跑时置灰并给理由，绝不隐藏。

     为什么从 `disabled` 挪到 `ported`：这颗按钮两端**同源同函数**，灰不灰完全由运行期状态与协议
     决定，不由「移动端还没做」决定 —— `view-model.ts:72 speedTestBlockedReason` →
     `components/screens/shared/speedtest-feedback.ts:133 speedTestBlockedMessage`，桌面
     `NodeCard.tsx:384/:396` 读的是同一份。把它登记成一条缺口会稀释本表的含义：本表回答的是
     「移动交付缺了什么」，而这一颗两端可做的动作**完全相等**。

     ⚠️ 顺带修掉一处**登记与生产不符**：上一版这里挂着 `reasonKey: 'nodes.speedTestNotApplicable'`，
     而生产的理由是逐档算出来的 —— 核停档返 `home.stubProxyStopped`，其余档走
     `speedTestBlockedMessage` 的六个具名分支，`nodes.speedTestNotApplicable` 只是它的 `default`
     那一档（`speedtest-feedback.ts:148`）。那道门只校验「该键在 zh-CN 存在」⇒ 挂一个错的键也不会红。

     § 4.10 的第三件事（“plus an action that connects”）**2026-09-05 已落地**（K3-RV03）：
     核停时行动作面里多一颗「连接」，走的是与首页**同一条**起核腿
     （`withProxyStartClaim(() => startProxy())`，不认领会让提权门那两码被事件腿再报一遍）。

     裁定的那一格是**文案**，不是接线：核停这一档的理由此前是 `nodes.mobileSpeedTestCoreStopped`，
     五语种写的都是「请先在**首页**连接后再测」—— 同一张面板里再放一颗「连接」会让那句话当场变假。
     裁定「只复用已有的五语种键，不新造键」⇒ 理由改指中性的 `home.stubProxyStopped`
     （「代理未运行」），旧键从五份 locale 里删掉（删除不是翻译，不触碰「凭空写 ru/fa 译文」那条禁令）。

     进行态那条次要项也一并收了：动作面选中即关闭、本屏没有首页那颗按钮的 `connectBusy` 内联忙态
     ⇒ 起核那几秒落一条 `notice`，用的同样是已存在的键 `home.statusStarting`（「启动中」）。

     ⚠️ **仍未量到的一跳**：行动作面要选中某一行才展开，本仓的渲染夹具（`NodesScreenView` 直渲）
     够不到那一层 DOM ⇒ 「这颗按钮按下去真的起核」归真机验收；源码面的三条腿由
     `nodes-screen.test.tsx` ⑯-b 钉着（在场分支 / 起核腿同源 / 文案不指回首页）。 */
  {
    id: 'speed-test',
    desktopRef: 'className="nd-a speed"',
    disposition: { kind: 'ported' },
  },
  /* 复制链接：纯 IPC + 剪贴板，无弹窗依赖 ⇒ 直接移植。 */
  {
    id: 'copy-link',
    desktopRef: "t('nodes.copyLink')",
    disposition: { kind: 'ported' },
  },
  /* 克隆（2026-09-06 接上，批 2 / W-02）：**没有表单**，是一次直调 `server_add` 的动作 ——
     剥 id / subscriptionId / providerName，改名后新增，与桌面 `use-node-actions.ts:60 cloneServer`
     同一形态。单例拦截走同一条纯函数 `domain/endpoint-routes#meshSingletonConflict`，文案沿用
     克隆专属的那两条（`nodes.cloneWarpSingleton` / `nodes.cloneTsSingleton`，五语已在）。
     接线点：`MobileNodesScreen.tsx#cloneNode`。 */
  {
    id: 'clone',
    desktopRef: "t('nodes.clone')",
    disposition: { kind: 'ported' },
  },
  /* 编辑（2026-09-06 接上，批 2 / W-02）：移动端有自己的表单宿主了
     （`mobile/forms/**`，字段规格复用 `node-spec.ts` + `proto-codec.ts`，呈现层重写 ⇒ 契约 A1 不破）。
     接线点：`MobileNodesScreen.tsx#rowItems` → `mobileEditFormFor` → `MobileFormHost`。

     🔴 **覆盖面 2026-09-06（批 3）补齐到全部协议。** 上一版这里写着「wireguard / tailscale
     （含 WARP）落在表外、渲染成在场置灰 + 一条理由键」——那三张表本批落地
     （`forms/WarpPanel` / `WgPanel` / `TsSettingsPanel`），`mobileEditFormFor` 随之**不再返回
     `null`**，行动作面上那一档在场置灰也没有了对应的产品状态。
     分流判据两端共用：`components/screens/nodes/node-edit-routing.ts#nodeEditForm`
     （桌面 `editDialogFor` 与移动 `mobileEditFormFor` 都只是它的名字翻译层，各以 `never` 兜底
     ⇒ 那里加一支而某一端没跟上，那一端编译不过）。 */
  {
    id: 'edit',
    desktopRef: "t('common.edit')",
    disposition: { kind: 'ported' },
  },
  /* 设为出口：移动端由**整行**承担（entity-row 的主动作），并按 IA 裁定 #3 紧贴一条 TsExitWarning。 */
  {
    id: 'use-as-exit',
    desktopRef: 'useTip',
    disposition: { kind: 'ported' },
  },
  /* 删除（2026-09-06 接上，批 2 / W-03）。桌面那条腿（`use-node-deletion.ts`）有三段，本批逐段处置：

      ① **暂存事务** —— 复用同一批纯函数：`lib/staged-config#splitStagedOnly`（staged-only 的删除
         走「撤销那条暂存条目」而不是发一次后端删除）+ `node-delete-fallback#partitionNodeDeleteRoutes`
         （按策略分流 staged / 直落盘）+ `staged-config-store#stageServerDeletions`。接线点见
         `mobile/nodes/node-deletion.ts`。
      ② **兜底改选** —— 同样接了（`fallbackExitAfterDelete`），但它**不产生用户可感的后果**：
         `src-tauri/src/commands/server.rs:212 resolve_fallback_selected` 的函数文档写着
         「**不可置 null**：0 节点重启会 throw」，兜底 id 不存活于删后 `servers` 就回落
         `:223 DIRECT_SERVER_ID` 哨兵，而 `:232 apply_selection_fallback` 在 `server_delete`（`:274`）
         与 `server_delete_batch`（`:337`）内部**无条件调用**。前端传它只是把「回落到剩下最快的那个」
         而不是「回落到直连」这个选择权拿回来。
      ③ **二次确认** —— 桌面是原地 `confirmTwice`（按钮翻红 + 再点一次），移动端换成叠一层
         确认面板（`mobile/forms/ConfirmPanel.tsx`）：触屏没有 hover，翻红那一下在拇指底下被手指挡住，
         而「再点一次」的第二击极易被读成误触重复。**WARP 注销确认**走同一层。

     ⚠️ 桌面的 `removeWarpNode`（组网接入面上那两颗「重新注册 / 注销 WARP」）仍不在射程内：
     那是**组网接入面卡片里的次动作**，逐颗登记在本文件的 `MESH_JOIN_ACTIONS`。
     节点行上删除一个 WARP 节点这条路是通的。 */
  {
    id: 'delete',
    desktopRef: "t('common.delete')",
    disposition: { kind: 'ported' },
  },
];

/**
 * 「移动到分组」——§4.14。**归因是数据模型，不是平台**（2026-09-06 写明）：Polaris 的分组由订阅归属
 * 与协议派生，没有可自由分配的分组，所以**桌面同一颗也永久灰着** ——
 * `components/screens/nodes/NodesBatchBar.tsx:83 disabled={!canMoveToGroup()}`，而
 * `components/screens/nodes/nodes-logic.ts:210-212 moveToGroupTargets()` 恒返 `[]`。
 * ⇒ 两端可做的动作相等，这不构成移动交付缺口，别把它读成「Android 上做不到」。
 *
 * 移动端取「不渲染」而非跟着桌面灰着：停靠条一共放得下四五个目标，一颗永远死的会挤掉一颗活的。
 * 理由文案 `nodes.batchMoveUnavailable` 说的也是数据模型这条，与此处一致。
 */
export const BATCH_MOVE_TO_GROUP: Disposition = {
  kind: 'absent',
  reasonKey: 'nodes.batchMoveUnavailable',
};

/**
 * 批量「删除」（2026-09-06 接上，批 2 / W-03）—— 与逐行那颗删除**同一条腿**
 * （`mobile/nodes/node-deletion.ts` 的 `deleteBatch`，判据全部复用桌面纯函数）。
 *
 * 射程上界与桌面同一条：`nodes-logic#selectedVisibleIds`（勾选集不随 tab 切换 / 筛选收窄而收缩，
 * 直接消费它会删掉用户此刻**看不见**的节点）。求交后为空 = 连确认都不该武装。
 *
 * ⚠️ 停靠条的排版预算是一道**记账门**，不是一堵墙：`styles/text-fit.test.ts` 那条判据先断言
 * 按钮数与键表，**在量折行之前**。本批把这颗画回停靠条 ⇒ 那两条基线要同批重推一次
 * （见该文件的批量条那一节），意思是「基线要显式重推」，不是「装不下」。
 */
export const BATCH_DELETE: Disposition = { kind: 'ported' };

/** 批量条的一颗按钮。与 `RowActionEntry` 同形：`desktopRef` 供门做取材面自检。 */
export interface BatchActionEntry {
  readonly id: string;
  readonly desktopRef: string;
  readonly disposition: Disposition;
}

/**
 * 桌面批量条 `.batch-bar` 的六颗按钮（`NodesBatchBar.tsx:36-115`），逐颗登记移动端处置。
 *
 * 为什么是一张**逐颗**的表而不是一句「除了 X 其余照搬」：那句话就是上一版写的，它把
 * 「删除」漏成了悄无声息的缺席，而没有任何东西会红 —— 一句自述对实现零强制力。
 * 门拿桌面 `<button` 数与本表条目数对差、再拿本表的 `ported` 数与真渲染出来的按钮数对差，
 * 桌面加一颗或移动端少画一颗都当场红（`nodes-screen.test.tsx` ⑤）。
 */
export const BATCH_ACTIONS: readonly BatchActionEntry[] = [
  { id: 'select-all', desktopRef: 'id="batch-all"', disposition: { kind: 'ported' } },
  { id: 'speed-test', desktopRef: 'id="batch-test"', disposition: { kind: 'ported' } },
  { id: 'move-to-group', desktopRef: 'id="batch-move"', disposition: BATCH_MOVE_TO_GROUP },
  { id: 'copy-links', desktopRef: "t('nodes.batchCopyLinks')", disposition: { kind: 'ported' } },
  { id: 'delete', desktopRef: "t('nodes.batchDeleteConfirmAgain')", disposition: BATCH_DELETE },
  { id: 'exit', desktopRef: "t('nodes.batchExit')", disposition: { kind: 'ported' } },
];

/**
 * 订阅摘要里**没有登记数据源**的位置（IA §2.1「Data positions with no registered source」）。
 * 口径是「标为 pending 而不是画出来」：不许填一个看起来对的假值，也不许假装这块不存在。
 *
 * 🔴 **2026-09-06 清空（批 2 / W-06）。** 用量 / 到期 / 上次更新三处的来源已登记进
 * `~/docs/polaris/design/mobile-kit/data-contract.json#sources.subscription-summary`，
 * 三处随之**真的画出来**（`MobileNodesScreen.tsx#sub` 算值 → `NodesScreenView` 渲染），
 * 判据与桌面 `SubInfoBar.tsx:191-194` 同一批：`nodes-logic#subUsage` / `fmtBytes` /
 * `relativeTimeTextIso` / `userInfo.expire`。
 *
 * 空数组不是「这张表没用了」：它是本登记表在这一格上的**当前答案**。哪天订阅摘要上又出现一个
 * 没有登记来源的位置，它要回到这里，而不是被画成一个看起来对的数。
 */
export const SUB_PENDING_FIELDS: readonly string[] = [];

/* ════════════════════════════════════════════════════════════════════════════
 * 「添加」四条 · 订阅「更多」五条 · 组网接入五个 —— 2026-09-06 新登记（批 2）。
 *
 * 🔴 **这三块此前一条都不在表里、也没有任何门。** 于是桌面菜单多一项、或移动端顺手删一项，
 *    都不会有任何东西红 —— 那正是 `BATCH_ACTIONS` 上一版把「删除」漏成悄无声息缺席的同一形态。
 *    现在三块都与 `ROW_ACTIONS` / `BATCH_ACTIONS` **同形**：逐条带 `desktopRef` 供取材面自检，
 *    并由 `nodes-screen.test.tsx` 拿桌面菜单项数与本表条目数对差。
 * ═══════════════════════════════════════════════════════════════════════════ */

/**
 * 桌面节点屏页头「添加」菜单的四项（`NodesHeader.tsx:88/:102/:117/:131`），逐项登记移动端处置。
 *
 * 四条后端腿全在且**无平台门控**：`server_add` · `server_add_bulk` ·
 * `subscription_create_start` · `local_import_parse`。本批（W-01）四条全部接通，
 * 入口是 `MobileNodesScreen.tsx#addItems` → `mobile/forms/**`。
 */
export const ADD_ACTIONS: readonly RowActionEntry[] = [
  /* 添加代理节点 → `node` 表单（`ND_SPEC` 17 协议）。 */
  { id: 'manual-add', desktopRef: "t('nodes.manualAdd')", disposition: { kind: 'ported' } },
  /* 添加组网接入 → `mesh-join` 选择器（内含五个选择，逐个登记见 `MESH_JOIN_CHOICES`）。 */
  { id: 'mesh-join', desktopRef: "t('nodes.meshAddAccess')", disposition: { kind: 'ported' } },
  /* 导入节点 → `import` 表单（粘贴文本；文件选择器那条腿见该文件头注的射程自曝）。 */
  { id: 'import', desktopRef: "t('nodes.manualImport')", disposition: { kind: 'ported' } },
  /* 添加订阅 → `sub` 表单（走 backend-owned create operation，不是一次 update）。 */
  { id: 'subscription', desktopRef: "t('nodes.addSubscription')", disposition: { kind: 'ported' } },
];

/**
 * 桌面订阅信息栏「更多」菜单的五项（`SubInfoBar.tsx:306/:316/:326/:339` 那块 `.mini-menu`），
 * 逐项登记移动端处置。本批（W-04）五条全部接通。
 *
 * 「更新间隔」那一条值得单说：Polaris **没有 per-sub 间隔字段**（调度器只读全局
 * `config.subscriptionUpdateIntervalHours`），桌面那一项做的是「跳到设置→更新」。移动端做同一件事
 * —— `mobile/navigate.ts#navigateMobile('settings', 'update')`，落到同一个真开关
 * （`mobile/settings/UpdatePage.tsx:114` 写的就是那个键）。**不做一个假的「本订阅间隔」**。
 */
export const SUB_MENU_ACTIONS: readonly RowActionEntry[] = [
  { id: 'rename', desktopRef: 'data-act="sub-rename"', disposition: { kind: 'ported' } },
  { id: 'edit-url', desktopRef: 'data-act="sub-edit-url"', disposition: { kind: 'ported' } },
  { id: 'copy-url', desktopRef: 'data-act="sub-copy-url"', disposition: { kind: 'ported' } },
  { id: 'interval', desktopRef: 'data-act="sub-interval"', disposition: { kind: 'ported' } },
  { id: 'delete', desktopRef: 'data-act="sub-del-menu"', disposition: { kind: 'ported' } },
];

/** 组网接入面的一个选择。`label` 是产品名（专有名词，不入 i18n，同桌面那侧的写法）。 */
export interface MeshJoinChoiceEntry {
  readonly id: string;
  readonly label: string;
  /** 说明文案键 —— 与桌面 `MeshJoinDialog` 同键，两端不各写一句。 */
  readonly descKey: string;
  readonly desktopRef: string;
  readonly disposition: Disposition;
}

/**
 * 桌面 `MeshJoinDialog` 的五个选择（两个托管服务 + 三条隧道），逐个登记移动端处置。
 *
 * # 五个**全部接通**（2026-09-06 / 批 3）
 *
 * 上一版有三条 `absent`（WARP / Tailscale / WireGuard），理由是「那三份字段规格还住在桌面 `.tsx` 里」。
 * 本批把那三份规格拆进零 React 的 `.ts`（`components/dialogs/warp-spec.ts` / `ts-spec.ts` /
 * `wg-spec.ts`，形态与 `field-spec.ts` 从 `FieldSpec.tsx` 拆出来逐字相同），三张移动面板随之落地：
 *  · WARP —— `mobile/forms/WarpPanel.tsx`（注册 + 编辑；单例闸走
 *    `domain/mesh-singleton-guard#registerWarpIfSlotFree`，**前置到 Cloudflare 请求之前**）；
 *  · Tailscale —— `mobile/forms/TsLoginPanel.tsx`（登录）+ `TsSettingsPanel.tsx`（设置），
 *    按槽位占没占分流，逐字同桌面 `go({ kind: tsNode ? 'ts-settings' : 'ts-login' })`；
 *  · WireGuard —— `mobile/forms/WgPanel.tsx`（手填 + 粘贴 wg-quick `.conf`）。
 *
 * ⚠️ **同一件工作的第二个影子也一起收了**：节点行上「编辑」一个 wireguard / tailscale（含 WARP）
 * 节点撞的是同样这三张表。判据在 `mobile/forms/form-store.ts#mobileEditFormFor`，它现在**不再返回
 * `null`** —— 分流本体是两端共用的
 * `components/screens/nodes/node-edit-routing.ts#nodeEditForm`，两侧各有一个以 `never` 兜底的
 * 名字翻译层。旧的理由键 `nodes.mobileMeshFormUnavailable` 随之从五份 locale 里删掉：
 * 那句话说的是「这三张表还没接」，本批之后它是**假的**，留着比没有更坏
 * （删除不是翻译，不触碰「凭空写 ru/fa 译文」那条禁令；同 `nodes.mobileSpeedTestCoreStopped` 先例）。
 */
export const MESH_JOIN_CHOICES: readonly MeshJoinChoiceEntry[] = [
  {
    id: 'warp',
    label: 'Cloudflare WARP',
    descKey: 'meshJoin.warpNew',
    desktopRef: 'title="Cloudflare WARP"',
    disposition: { kind: 'ported' },
  },
  {
    id: 'tailscale',
    label: 'Tailscale',
    descKey: 'meshJoin.tsNew',
    desktopRef: 'title="Tailscale"',
    disposition: { kind: 'ported' },
  },
  {
    id: 'openconnect',
    label: 'OpenConnect',
    descKey: 'meshJoin.oc',
    desktopRef: 'title="OpenConnect"',
    disposition: { kind: 'ported' },
  },
  {
    id: 'openvpn',
    label: 'OpenVPN',
    descKey: 'meshJoin.ovpn',
    desktopRef: 'title="OpenVPN"',
    disposition: { kind: 'ported' },
  },
  {
    id: 'wireguard',
    label: 'WireGuard',
    descKey: 'meshJoin.wg',
    desktopRef: 'title="WireGuard"',
    disposition: { kind: 'ported' },
  },
  // 2026-09-24 随主线 MASQUE 客户端进组网接入面：同 OpenConnect/OpenVPN，落共享节点表单并预选协议。
  {
    id: 'masque',
    label: 'MASQUE',
    descKey: 'meshJoin.masque',
    desktopRef: 'title="MASQUE"',
    disposition: { kind: 'ported' },
  },
];

/**
 * 桌面组网接入面上挂在**卡片里**的次动作（`MeshJoinDialog` 那两处 `actions={…}` 里的
 * `btn ghost sm`），逐颗登记移动端处置 —— **2026-09-06（批 3）新登记**。
 *
 * # 为什么现在才登记，以及它不是「凭空多出来的债」
 *
 * 上一版这五颗**一颗都不在任何面上**：那时 WARP 与 Tailscale 两张卡片本身就是灰的，
 * 次动作是「灰按钮底下的灰按钮」，看不见也点不到。本批把两张卡片接通之后它们才**露出来** ——
 * 桌面点进 Tailscale 卡片旁边能直接退出登录，移动端不能。这正是「掀开毯子之后才看见的缺口」，
 * 如实登记进来比让它继续隐形好（同 `BATCH_ACTIONS` 上一版把「删除」漏成悄无声息缺席的形态）。
 *
 * 两颗 `ported` 的落点是 `mobile/forms/TsSettingsPanel.tsx` 末尾那一行动作：
 * 移动端这张接入面是一列纵向选择（拇指区），每条挂 2–3 颗次动作会把主选择挤没，
 * 故账号级动作跟着**账号设置表**走，而不是跟着选择器走。三颗 `absent` 的理由见各自注释。
 */
export const MESH_JOIN_ACTIONS: readonly RowActionEntry[] = [
  /* WARP 重新注册（2026-09-13 接上，批 16）：桌面是「先删掉现有 WARP 节点、再打开注册表」的
     复合动作（`NodesScreen.tsx:507` → `use-node-deletion#removeWarpNode` + `afterDelete` 开注册表）。
     移动端**同一条复合动作**接在 `forms/WarpPanel.tsx` 编辑态的末尾（`reregister`）：
     删除腿复用 `mobile/nodes/node-deletion.ts#removeWarpNode`（暂存事务 / 写入路由分流 /
     兜底改选三段与逐行删除同一份），`afterDelete` 关掉本面板再开 `{ kind:'warp', edit:false }`。

     为什么落在 WarpPanel 而不是接入面上：那张接入面是一列纵向选择（拇指区），塞不下每条
     2–3 颗次动作 —— 这条口径是批 3 给 Tailscale 那两颗账号级动作定的（见 `MeshJoinPanel` 头注），
     本批让 WARP 这两颗落在同一条口径上。「已注册的那一台」的语境只有编辑态那张表有。 */
  {
    id: 'warp-reregister',
    desktopRef: "t('meshJoin.reregister')",
    disposition: { kind: 'ported' },
  },
  /* WARP 注销（2026-09-13 接上，批 16）：同上，走同一个 `removeWarpNode`（只是不 `afterDelete`
     开注册表，只关掉本面板）。确认与成功文案各用自己那三条键 —— 用户按的是「注销 WARP」，
     不是「删除节点」，套 `nodes.deleteSuccess` 会把一次远端设备注销说成删掉一行。 */
  {
    id: 'warp-deregister',
    desktopRef: "t('meshJoin.deregister')",
    disposition: { kind: 'ported' },
  },
  /* Taildrop（2026-09-13 接上，批 16）：收件箱整张面移植进 `forms/TaildropPanel.tsx`，
     入口在 `TsSettingsPanel` 的账号级动作那一行（带未读角标，判据与桌面同一函数
     `domain/taildrop#taildropBadgeCount`）。

     🔴 入口**按 `serverId` 寻址**，这正是桌面本轮把那张卡改成多节点分行要解决的同一件事：
     `{ kind:'taildrop'; serverId }` 那一支必须带 id（`form-store.ts`），而节点屏上每个
     Tailscale 节点各有一行、「编辑」都落到 `TsSettingsPanel` ⇒ 每一个节点的收件箱都到得了，
     不存在「绑在 `.find()` 取到的第一个上」。完整依据写在 `TaildropPanel` 头注。

     三个动作**各自**的状态（不打包成一条）：
      · **收件箱列表 / 删除 / 取消接收** —— 本来就通，纯 gRPC，不经任何选择框；
      · **取件保存** —— 本批在后端接上 SAF（`taildrop_save` 的 URI 支：先流式落应用私有临时文件，
        完整之后经 `stream_into_picked` 一次性灌进用户选的文档）。**一条保证拿不回来**：
        提交那一跳的原子性（SAF 没有 rename），写到一半失败会在目标文档上留下半截；
        降级表逐行写在 `taildrop.rs#taildrop_save`；
      · **发件** —— 本批在后端接上 SAF（`open_selected_targets`：`open_picked_for_read` 拿 fd、
        `file_name_of` 取名）。两条如实边界：名字退化成 URI 末节（内容不受影响）；
        云盘类 provider 的管道 fd 给不出确定长度，那一档当场拒绝（协议要先声明大小）。

     🔴 上一版这里写的是「两格照画、后端必报错」，并援引 `MobileConnectionsScreen` 的
     `archive-legacy` 当先例 —— **那个先例不成立**：那两颗由 `legacyLog?.exists` 门控，而遗留
     `singbox.log` 移动端从来没产生过 ⇒ 在 Android 上根本不渲染，用户点不到。
     「点不到」与「点了必报错」不是同一档，后者是本仓反复判过的「画一颗按下去不产生任何发射
     差异的控件，比没有更坏」。处置是把腿接上，不是找一个像的先例。 */
  {
    id: 'taildrop',
    desktopRef: "t('meshJoin.taildrop')",
    disposition: { kind: 'ported' },
  },
  /* 切换账号 → `TsSettingsPanel` 末尾那颗，开 `ts-login`（与桌面 `go({ kind:'ts-login' })` 同一条腿）。 */
  {
    id: 'ts-switch-account',
    desktopRef: "t('meshJoin.switchAccount')",
    disposition: { kind: 'ported' },
  },
  /* 退出登录 → `TsSettingsPanel` 末尾那颗，走 `api.server.tailscaleLogout` +
     `splitStagedOnly` 的 staged-only 拦截（与桌面 `TsSettingsDialog#handleLogout` 同一条腿），
     破坏性 ⇒ 叠一层 `confirm` 面板。 */
  {
    id: 'ts-logout',
    desktopRef: "t('meshJoin.logout')",
    disposition: { kind: 'ported' },
  },
];
