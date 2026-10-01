/**
 * TUN MTU 输入的渲染端纯逻辑（区间 + 解析）。
 *
 * # 这里**没有**默认 MTU（2026-09-25 起）
 *
 * 用户留空 = 生成期**不下发** `mtu` 键，由 sing-box 内核按运行环境取默认（上游
 * `protocol/tun/inbound.go` 的 `options.MTU == 0` 分支）。Polaris 不持有平台 → MTU 表：
 * 抄一份只会在上游改值时静默分叉，且渲染端看到的平台与内核看到的运行环境未必同一口径。
 * 故设置页占位符只写「自动（内核默认）」、不写数字。Rust 侧依据见 `TunModeConfig::mtu` 的文档注释。
 */

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
