/**
 * TUN 默认 MTU —— Rust 侧 `crates/config-engine/src/user_config/tun_config.rs` 的 `DEFAULT_TUN_MTU`
 * 的渲染端副本。
 *
 * # 为什么渲染端需要一份
 *
 * MTU 设置项的「自动」态要**当场告诉用户自动是多少**（占位符里的那个数），不算出来就只能写一句
 * 「自动」让用户去猜。
 *
 * # 为什么是一个常量，不再是「栈 × 平台」的函数
 *
 * sing-box 1.15.0-alpha.3 起 TUN `stack` 弃用，Polaris 已整体移除栈的概念（不下发、无设置项），
 * 一律走 sing-tun 新栈。此前按栈与平台分三档（65535 / 9000 / 4064）的依据全是旧栈上的实测，
 * 自变量没了，默认值收敛成上游桌面默认 65535，与平台无关。判据（2026-09-13 两平台实测）见 Rust 侧常量的文档注释。
 *
 * # 两侧同值怎么保证
 *
 * 生成期的真值**始终在 Rust**（本文件只影响显示，改坏了也不会让内核拿到别的 MTU）。
 * `tun-mtu.test.ts` 直接读 Rust 源码里的常量字面量与本常量对拍 —— 任一侧改值而另一侧没跟即红。
 */

/** 用户未填 MTU 时内核实际拿到的值（= Rust `DEFAULT_TUN_MTU`）。 */
export const DEFAULT_TUN_MTU = 65535;

/** 内核接受的 MTU 区间（与 `polaris-store` 的 `validate_config` 同值）。 */
export const MTU_MIN = 1280;
export const MTU_MAX = 65535;

/**
 * 输入框文本 → 待持久化的 MTU。
 *
 * - 空白 → `{ mtu: undefined }`（自动）
 * - 合法整数且在区间内 → `{ mtu: n }`
 * - 其余（非数字 / 越界 / 小数）→ `{ invalid: true }`，调用方**不落库**并标红
 *
 * 越界不做钳制：把 70000 悄悄改成 65535 与「设了但不生效」是同一类缺陷，用户看到的框里是自己填的
 * 数，生效的是另一个。宁可当场报错。
 */
export function parseMtuInput(text: string): { mtu?: number; invalid?: true } {
  const s = text.trim();
  if (s === '') return { mtu: undefined };
  if (!/^\d+$/.test(s)) return { invalid: true };
  const n = Number(s);
  if (!Number.isSafeInteger(n) || n < MTU_MIN || n > MTU_MAX) return { invalid: true };
  return { mtu: n };
}
