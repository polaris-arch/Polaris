/**
 * 移动端**表单宿主**的栈（zustand，与 `dialog-store` 同模式）。
 *
 * # 为什么不复用桌面 `components/dialogs/dialog-store.ts`
 *
 * 那个 store 的 `DialogDesc` 是**桌面 21 个弹窗**的 union，而 `DialogHost` 的穷尽 switch 会为
 * 每一个 kind 渲染一个桌面弹窗组件 —— 那整条链的外观住在 `prototype.css` / `components.css`
 * = 桌面层叠链，契约 A1 禁入移动端（`mobile/mobile-entry.test.ts` ③ 守着 CSS 集合恰等）。
 * 复用 store 就必须复用它的 union，而 union 的每一支在移动端都得有一个 case；今天移动端只做得出
 * 其中四支，剩下 17 支要么渲染成空（用户点了什么也没发生），要么在穷尽 switch 里写 17 个
 * `return null` —— 那时「这条腿没接」这件事就再也没有任何东西看得见了。
 *
 * ⇒ **规格与逻辑复用（`field-spec.ts` / `node-spec.ts` / `proto-codec.ts` / `mesh-form-layout.ts`
 * / `ts-settings-logic.ts` 全是平台无关纯数据与纯函数），呈现层与栈重写。**
 *
 * # union 与 `never` 兜底**一次建对**（这条是本文件存在的主要理由）
 *
 * 本批只落四个 kind（`node` / `sub` / `import` / `mesh-join`），外加两个**地基**
 * （`confirm` 破坏性二次确认 / `ts-exit` 出口设备选择器）。但形状是按「将来还会有十几支」定的：
 *  · 每一支自带**它自己的**载荷字段，不设 `payload: unknown` 这种一刀切口袋 ——
 *    口袋一旦存在，新增 kind 的类型错误就全都退化成运行期 undefined；
 *  · `MobileFormHost` 的 switch 以 `const never: never = entry` 兜底 ⇒ **加一支 union 而不加 case
 *    时编译不过**。这是「后续每加一个 kind 都要重构宿主」与「加一行 case」之间的全部差别；
 *  · 关闭走 `closeInstance(instanceId)` 而不是 `close()`（pop 顶层）：异步 continuation
 *    （表单提交回来时用户可能已经又叠了一层确认框）必须关**自己**那一层，按栈顶猜必然关错。
 *    这条语义逐字取自 `dialog-store.ts` 的同名方法，不是新发明的。
 *
 * # 栈而不是单值
 *
 * 「节点表单 → 放弃更改确认」「删除 → 二次确认」都是**叠**在前一层上的：单值会把下面那层的
 * 草稿整个卸载掉（React 卸载 = state 丢失），用户点「取消」回到的是一张空表单。
 */

import { create } from 'zustand';
import type { ServerConfig } from '@/contracts/types';
import type { RulePreset } from '@/domain/rules';
import type { NodeProto } from '@/components/dialogs/node-spec';
import { nodeEditForm } from '@/components/screens/nodes/node-edit-routing';

/**
 * 破坏性动作的二次确认载荷。
 *
 * `onConfirm` **由回调自行负责关闭**（同 `dialog-store.ts#ConfirmPayload` 的既定语义）：
 * 不自动 pop，避免「确认框里又开了一层」时 pop 顶层 ≠ pop 目标的歧义。
 */
export interface MobileConfirmPayload {
  readonly title: string;
  readonly message: string;
  /** 确认按钮文案。缺省走 `common.confirm`。 */
  readonly confirmLabel?: string;
  /** 危险语气（确认键红）。删除类恒为真。 */
  readonly danger?: boolean;
  readonly onConfirm: () => void | Promise<void>;
}

/**
 * 表单描述符（discriminated union）。**加一支就要在 `MobileFormHost` 里加一个 case**，
 * 否则那里的 `never` 兜底编译不过 —— 这正是本 union 的设计目的。
 */
export type MobileFormDesc =
  /**
   * 节点表单。三种打开方式共用一支：
   *  · 新增 —— 两者皆空；
   *  · 编辑 —— `serverId` 指向存量节点；
   *  · 组网隧道新增 —— `initialProto` 预选 OpenConnect / OpenVPN（入口是 `mesh-join`）。
   * 克隆**不走本表单**：它是一次直调 `server_add` 的无表单动作（桌面
   * `use-node-actions.ts:60 cloneServer` 同形），故这里没有 `cloneFrom`。
   */
  | { kind: 'node'; serverId?: string; initialProto?: NodeProto }
  /**
   * 订阅表单。`focus` 区分订阅「更多」里的「重命名」与「编辑 URL」——Polaris 只有一个订阅表单，
   * 两个菜单项落到同一张表的不同字段（口径逐字取自 `dialog-store.ts` 的同名字段）。
   */
  | { kind: 'sub'; subId?: string; focus?: 'name' | 'url'; onAdded?: (subId: string) => void }
  /**
   * 订阅**创建操作恢复面**（2026-09-25 ζ 批 A8）。只由 `mobile/app-wiring.ts` 在应用级水合时为
   * 「本机发起过、进程被回收 / WebView 重建后找回来」的那几次创建打开（见
   * `openMobileSubscriptionCreateRecovery`），与桌面 `{ kind:'sub-create-task', operationId }`
   * 逐字同形。
   */
  | { kind: 'sub-create-task'; operationId: string }
  /**
   * 手动导入（粘贴文本）。恒新增。
   *
   * `onAdded` 与 `sub` 那一支同形（桌面 `NodesHeader.tsx:111` 逐字如此：
   * `openDialog({ kind:'import', onAdded: () => setActiveTab('manual') })`）——
   * 导入的产物一律落**自建**分组（`server-grouping.ts:53`），当前 tab 若停在某条订阅上，
   * 表单一关屏上零变化，用户读作「点了没反应」。
   */
  | { kind: 'import'; onAdded?: () => void }
  /** 组网接入选择器。 */
  | { kind: 'mesh-join' }
  /**
   * WARP 接入表（批 3）。`edit` 真 = 编辑现有那一个（单例槽，面板自查 `findWarpNode`，
   * 故不带 id —— 与桌面 `{ kind:'warp', edit }` 逐字同形）；假/缺省 = 注册一个新的。
   */
  | { kind: 'warp'; edit?: boolean }
  /** Tailscale **登录**表：浏览器登录 / Auth Key 两条腿。带 `serverId` = 给该既有节点换 key /
      换控制面；不带 = 新建（Tailscale 已不是单例，没有 id 时**不猜**，直接走新建路径）。 */
  | { kind: 'ts-login'; serverId?: string }
  /** Tailscale **设置**表：`ts-spec` 那张 20 余项的表。**必须带 id** —— Tailscale 已不是单例，
      面板自查「任意一个」会让编辑第二个节点打开/写坏第一个。 */
  | { kind: 'ts-settings'; serverId: string }
  /** WireGuard 字段表（批 3）。`serverId` 指向存量节点 = 编辑；缺省 = 新增（WG 允许多实例）。 */
  | { kind: 'wg'; serverId?: string }
  /** Tailscale 出口设备选择器（W-05）。`serverId` = 那个 tailscale 节点。 */
  | { kind: 'ts-exit'; serverId: string }
  /**
   * Taildrop 收件箱（批 16）。**必须带 id**，理由与 `ts-settings` 那一支逐字相同：
   * Tailscale 已不是单例，按协议 `.find()` 取「任意一个」会让用户翻到别人的收件箱。
   * 入口在 `TsSettingsPanel` 的账号级动作那一行（判断与依据见 `TaildropPanel` 头注）。
   */
  | { kind: 'taildrop'; serverId: string }
  /**
   * 规则表单（批 10）。新建与编辑**是同一支**，只由 `ruleId` 在不在分叉 —— 与桌面
   * `dialog-store.ts` 的 `{ kind:'rule' }` 逐字同形，三个字段的含义也逐字同：
   * `preset` 预填首条件（从连接记录「为它建条规则」进来时用），
   * `initialPlane` 决定这张表是流量面还是 DNS 面（两面**不共存**，见 `rule-submit.ts`）。
   */
  | { kind: 'rule'; ruleId?: string; preset?: RulePreset; initialPlane?: 'route' | 'dns' }
  /** DNS 服务器表单。带 id = 编辑，不带 = 新建（同桌面 `{ kind:'dns-server'; serverId? }`）。 */
  | { kind: 'dns-server'; serverId?: string }
  /** DNS 服务器组表单。带 id = 编辑，不带 = 新建。 */
  | { kind: 'dns-group'; groupId?: string }
  /**
   * 网络场景表单（spec §6.2）。带 `profileId` = 编辑，不带 = 新建 —— 与桌面
   * `{ kind:'network-profile'; profileId?; onSaved? }` 逐字同形。`onSaved` 给规则表单「生效网络」
   * 里那一项「新建场景…」用：建完直接选中新场景（表单叠在规则表单之上，关掉即回到那里）。
   */
  | { kind: 'network-profile'; profileId?: string; onSaved?: (profileId: string) => void }
  /** 添加自定义应用。恒新增 —— 桌面那张表同样没有编辑态。 */
  | { kind: 'app-add' }
  /** 从内置目录挑规则资源（多选下载）。 */
  | { kind: 'res-catalog' }
  /** 按 URL 下载一份规则资源。 */
  | { kind: 'res-url' }
  /** 破坏性动作二次确认（地基，被删除 / 放弃更改 / 删订阅共用）。 */
  | { kind: 'confirm'; payload: MobileConfirmPayload };

export type MobileFormKind = MobileFormDesc['kind'];

/**
 * 入栈后的不可变实例身份。
 *
 * `kind` 只说明要渲染什么，不能说明「是哪一次打开」：同类表单可嵌套/重开，异步回调必须按此
 * 身份关闭自己的实例，绝不能再按"当前栈顶"猜测。
 */
export type MobileFormEntry = MobileFormDesc & { readonly instanceId: string };

let nextInstance = 0;

function newInstanceId(): string {
  // 单测环境可能没有 crypto；进程内单调 id 对实例隔离已经充分（同 dialog-store）。
  return globalThis.crypto?.randomUUID?.() ?? `mform-${++nextInstance}`;
}

interface MobileFormStore {
  /** 栈底 → 栈顶。末尾 = 最上面那张表。 */
  stack: readonly MobileFormEntry[];
  /** push 一张表，返回这一次打开的稳定身份。 */
  open: (desc: MobileFormDesc) => string;
  /** pop 顶层。 */
  close: () => void;
  /** 移除指定实例；异步 continuation 只准走此入口。 */
  closeInstance: (instanceId: string) => void;
  /** 供异步回调在执行 UI 后续动作前确认其宿主仍在场。 */
  hasInstance: (instanceId: string) => boolean;
  /** 清空。 */
  closeAll: () => void;
}

export const useMobileFormStore = create<MobileFormStore>((set, get) => ({
  stack: [],
  open: (desc) => {
    const instanceId = newInstanceId();
    set((s) => ({ stack: [...s.stack, { ...desc, instanceId } as MobileFormEntry] }));
    return instanceId;
  },
  close: () => set((s) => ({ stack: s.stack.slice(0, -1) })),
  closeInstance: (instanceId) =>
    set((s) => ({ stack: s.stack.filter((e) => e.instanceId !== instanceId) })),
  hasInstance: (instanceId) => get().stack.some((e) => e.instanceId === instanceId),
  closeAll: () => set({ stack: [] }),
}));

/**
 * 屏级调用点的开表入口 —— **组件外也能调**（`openMobileForm`），因为节点屏的行动作面是在
 * `SheetRow.onSelect` 里开表的，那不是渲染期。
 */
export function openMobileForm(desc: MobileFormDesc): string {
  return useMobileFormStore.getState().open(desc);
}

/** 关掉某一层（异步 continuation 的唯一出口）。 */
export function closeMobileForm(instanceId: string): void {
  useMobileFormStore.getState().closeInstance(instanceId);
}

/**
 * 给一次**本机追踪着**的订阅创建操作打开恢复面，同一个 `operationId` 只开一层。
 * 返回是否真的开了。
 *
 * 桌面对位是 `store/subscription-create-recovery.ts#openTrackedSubscriptionCreateRecovery`，
 * 它开的是桌面 `dialog-store` 的弹窗（移动端没有那个宿主），故这里是同一条判据落到移动端栈上：
 * 去重键同为 `kind + operationId` —— 水合与表单交接都可能为同一次操作各开一次。
 */
export function openMobileSubscriptionCreateRecovery(operationId: string): boolean {
  const { stack, open } = useMobileFormStore.getState();
  if (stack.some((e) => e.kind === 'sub-create-task' && e.operationId === operationId)) return false;
  open({ kind: 'sub-create-task', operationId });
  return true;
}

/**
 * 编辑一个存量节点该开哪张表。
 *
 * **判据不在这里** —— 在 `components/screens/nodes/node-edit-routing.ts#nodeEditForm`，
 * 与桌面 `editDialogFor` 是**同一个函数**（那份文件是零渲染的 `.ts`，契约 A1 的两条风险
 * 一条都不成立）。本函数只做名字翻译：四个平台无关答案 → 四支 `MobileFormDesc`。
 *
 * 🔴 **2026-09-06（批 3）起不再返回 `null`。** 上一版对 wireguard / tailscale（含 WARP）返 `null`，
 * 调用方据此在场置灰并给一句「那三张表还没移植」的理由 —— 那三张表本批已经落地
 * （`WarpPanel` / `WgPanel` / `TsSettingsPanel`），理由随之作废，`null` 那一档也没有了对应的产品状态。
 * 保留一个恒不发生的 `null` 分支会留下一条**没有牙的**判据（门测得到函数、测不到任何真实入口）。
 *
 * `switch` 以 `const never: never` 兜底：`NodeEditForm` 加一支而这里没跟上 ⇒ **编译不过**。
 * 那正是「桌面新增一类组网协议时移动端不会静默漏掉」的全部保障。
 */
export function mobileEditFormFor(server: ServerConfig): MobileFormDesc {
  const target = nodeEditForm(server);
  switch (target.form) {
    case 'warp':
      return { kind: 'warp', edit: true };
    case 'wg':
      return { kind: 'wg', serverId: target.serverId };
    case 'ts-settings':
      return { kind: 'ts-settings', serverId: target.serverId };
    case 'node':
      return { kind: 'node', serverId: target.serverId };
    default: {
      const never: never = target;
      throw new Error(`unhandled NodeEditForm: ${JSON.stringify(never)}`);
    }
  }
}
