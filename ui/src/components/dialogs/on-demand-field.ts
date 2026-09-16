/**
 * 「按需连接」（sing-box 1.15 endpoint `on_demand`）—— 字段定义 + 草稿/提交两端的**唯一**一份。
 *
 * # 为什么收成一个模块
 *
 * 这个开关落在 `ServerConfig` **顶层**（与 `detour` / `bindInterface` 同处），因为它是端点
 * 生命周期属性、与协议无关；而暴露它的表单有三个（Tailscale / WireGuard / WARP）。
 * 三处各写一份 FieldSpec + 各写一遍读写逻辑 = 三份会各自漂移的同义实现，
 * 且漂了不会红（每处单独看都自洽）。同型先例：`detour-options.ts`。
 *
 * # 语义（判据在 Rust，此处不复述细节）
 *
 * 权威说明在 `crates/config-engine/src/user_config/server_config.rs` 的 `ServerConfig::on_demand`
 * 文档注释。一句话：**未被任何路由规则/选择器引用时，该端点断开连接**；Tailscale 侧等价
 * `tailscale down`（交出 tailnet 地址、停 MagicDNS），不清登录状态，恢复后身份不变。
 *
 * # 缺省即删键
 *
 * `false` / 未设置 ⇒ **删键**而不是写 `false`。Rust 侧 `skip_serializing_if` 决定了"不发射 =
 * 内核默认"，前端写一个显式 `false` 只会在磁盘上留下一个与默认等价的噪声键，
 * 并让日后"默认变了"这件事追不上存量配置。同 `buildTsSettings` 里那批"缺省即默认"的字段。
 */

import type { ServerConfig } from '@/contracts/types';
import type { FieldSpec } from './FieldSpec';

/** 三个组网表单共用的字段描述符（放各自的「高级」分组，与 `bindInterface` 同处）。 */
export const ON_DEMAND_FIELD: FieldSpec = {
  t: 'switch',
  k: 'onDemand',
  label: 'node.onDemand',
  hint: 'node.onDemandHint',
};

/** 存量节点 → 草稿值。缺席/`false` 都回显为关。 */
export function onDemandDraftValue(server?: { onDemand?: boolean } | null): boolean {
  return server?.onDemand === true;
}

/** 草稿值 → 节点（就地改并返回同一对象，签名对齐 `applyDetour`）。 */
export function applyOnDemand<T extends ServerConfig | Omit<ServerConfig, 'id'>>(
  server: T,
  value: unknown
): T {
  if (value === true) server.onDemand = true;
  else delete server.onDemand;
  return server;
}
