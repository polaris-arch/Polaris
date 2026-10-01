/**
 * 节点「编辑」入口的表单路由（纯逻辑，桌面 `NodesScreen` 与移动 `MobileNodesScreen` 共用 + 单测）。
 *
 * 抽成独立模块而非留在 NodesScreen.tsx 里：这条分流是**数据损坏的防线**，值得被单测钉住
 * （vitest 走 node 环境，不引 jsdom，故纯逻辑必须与组件文件分离——同 csel-logic/wg-logic 先例）。
 *
 * # 两个客户端一条判据（2026-09-06，移动端组网三张表落地时定的形态）
 *
 * 判据本体是 [`nodeEditForm`]，它答的是**「这个节点该由哪一张表编辑」**，只认四个抽象答案，
 * 不认任何一端的弹窗/面板类型。两端各自有一个**只做名字翻译**的薄映射：
 * 桌面 [`editDialogFor`] → `DialogDesc`，移动 `mobile/forms/form-store#mobileEditFormFor`
 * → `MobileFormDesc`。两个映射都以 `never` 兜底 ⇒ 这里加一支而某一端没跟上，那一端编译不过。
 *
 * 为什么不让移动端直接消费 `editDialogFor`：`DialogDesc` 是**桌面 21 个弹窗**的 union，
 * 移动端拿到它就得为剩下 17 支写分支（或写一个会静默吃掉它们的 `default`），
 * 而那 17 支在移动端一个都不该存在。翻译层比共用一个过宽的 union 便宜得多。
 */

import type { DialogDesc } from '@/components/dialogs/dialog-store';
import type { ServerConfig } from '@/contracts/types';
import { isAccountBasedProtocol } from '@/domain/endpoint-routes';
import { isWarpServer } from '@/domain/warp';

/**
 * 「该由哪一张表编辑」的四个答案（与平台无关）。
 *
 * 通用节点表单只认 `NodeProto`（见 node-spec.ts），**不含 wireguard/tailscale**。
 * 把组网节点丢给它的后果不是「显示不对」而是**丢数据**：`isNodeProto` 判假 → 协议回落 vless →
 * 保存时 `base.protocol !== proto` 让 codecBase 退化成裸 meta → `wireguardSettings` /
 * `tailscaleSettings`（私钥、对端公钥、tailnet 设置）被整体丢弃后覆盖写回。
 */
export type NodeEditForm =
  /** WARP 专用表（单例槽，表自查现有节点，故不带 id）。 */
  | { readonly form: 'warp' }
  /** 通用 WireGuard 表。 */
  | { readonly form: 'wg'; readonly serverId: string }
  /** Tailscale 设置表。**必须携 id**：Tailscale 已不是单例（`meshSingletonConflict` 只剩 WARP 支），
      不带 id 时表只会自查到任意一个 TS 节点（通常是第一个），编辑第二个节点会打开/写坏第一个。 */
  | { readonly form: 'ts-settings'; readonly serverId: string }
  /** 通用节点表（`ND_SPEC` 建模的那批协议）。 */
  | { readonly form: 'node'; readonly serverId: string };

/**
 * 按协议把编辑请求分流到对应的表 —— **全库唯一判据**，两端共用。
 *
 * WARP 必须先判：它的 `protocol` 也是 `'wireguard'`，但它有自己那张表（注册腿 + 许可证档位），
 * 而通用 WG 表对它只会呈现一堆读不懂的原始字段。
 *
 * # ⚠️ 协议比较**归一化**（2026-09-06 批 3 起，桌面 `editDialogFor` 的一处行为变更）
 *
 * 拆出本函数之前，桌面那两句是大小写敏感的裸 `===`。归一化是**有意**的，不是搬运时的手滑：
 * 同一次分流里第一句 `isWarpServer` 早就归一化（`domain/warp`），单例槽判据
 * `tailscaleSlotTaken` / `meshSingletonConflict` 也归一化 —— 三者口径不一致时，
 * 「这个节点归哪张表编辑」与「槽位占没占」会对同一个节点给出相反答案。
 * Tailscale 那一句因此直接消费共用谓词 [`isAccountBasedProtocol`]，不在这里再写一份字面量比较。
 *
 * 今天这条差异**不可达**：Rust 侧 `Protocol` 是 `#[serde(rename_all = "lowercase")]`，
 * `server_config.rs` 明确要求反序列化保持严格小写并有绊线单测，混合大小写进不到 UI。
 */
export function nodeEditForm(server: ServerConfig): NodeEditForm {
  if (isWarpServer(server)) return { form: 'warp' };
  if (server.protocol?.toLowerCase() === 'wireguard') return { form: 'wg', serverId: server.id };
  if (isAccountBasedProtocol(server.protocol)) return { form: 'ts-settings', serverId: server.id };
  return { form: 'node', serverId: server.id };
}

/**
 * 桌面侧的名字翻译（`NodeEditForm` → `DialogDesc`）。**不含任何判据** —— 判据在
 * [`nodeEditForm`]，这里只把四个答案换成桌面弹窗的 kind。
 */
export function editDialogFor(server: ServerConfig): DialogDesc {
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
      /* 穷尽兜底：`NodeEditForm` 加一支而这里没补 case ⇒ 这一行编译不过。 */
      const never: never = target;
      throw new Error(`unhandled NodeEditForm: ${JSON.stringify(never)}`);
    }
  }
}
