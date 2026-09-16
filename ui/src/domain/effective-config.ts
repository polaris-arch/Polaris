/**
 * 读取「由 `config:get` 边界保证在场」的配置字段 —— **前端唯一允许的缺席处理点**。
 *
 * # 为什么不能在调用点写 `?? 默认值`
 *
 * 这个仓里同一个 bug 犯过两次：
 *
 *  1. `bypassLANList`：前端兜底三条（`localhost` / `127.0.0.1` / `192.168.0.0/16`），
 *     内核生效默认是 27 条 `DEFAULT_BYPASS_LAN`。`ListEditor` 逐字符 `onChange` ⇒
 *     **首个按键就把那三条错误兜底当用户清单持久化**，静默丢弃 24 条真实默认。
 *  2. `tunConfig.inboundExcludeCidrs`：前端兜底 `['100.64.0.0/10']`，而 Rust 默认是 `None`、
 *     生成侧 `unwrap_or(&[])` ⇒ **内核实际排除 0 条**，UI 的折叠计数却显示 1 条。
 *     UI 报了一个不存在的排除，且首个按键会把这条幻影变成真的。
 *     对自建 Tailscale 控制面（`prefixes.v4` 可任意配）它还是**错的**段。
 *
 * 第一次的修法是给那一个字段在边界补齐 —— 补丁没有传染性，所以第二个字段原样复发。
 * 现在的机制是：**值的唯一真值源在 Rust**（`user_config::effective_view::ensure_effective_config`，
 * 登记在 `INJECTED_FIELD_PATHS`），前端一条兜底都不写。由 Rust 侧
 * `tests/frontend_sot_guard.rs::frontend_holds_no_config_default_fallback` 守住：
 * 那道门按 `INJECTED_FIELD_PATHS` 逐字段扫 `ui/src`，见到读取点自带兜底就转红。
 *
 * # 本模块回落的空值不是"默认值"
 *
 * `injectedList` 缺席时回落空数组、`injectedRecord` 回落空对象，**同时打一条 error**。
 * 那不是在给字段挑默认，是把"边界没注入"这件事变成**可见的故障**：
 * 静默兜底会让"边界坏了"与"用户就是配了空"长得一模一样，而空清单/空对象在界面上是显然的异常，
 * 配上 error 日志就能被排查抓到。选默认值的权力留在 Rust，这里只负责让缺席说话。
 */

/** 稳定引用：每次回落都返回同一个数组，避免把 re-render churn 混进故障现场。 */
const EMPTY_LIST: readonly string[] = Object.freeze([]);
const EMPTY_RECORD = Object.freeze({});

function reportMissing(path: string): void {
  console.error(
    `[effective-config] ${path} 缺席：config:get 边界未注入。` +
      ` 请在 user_config::effective_view::ensure_effective_config 里补上并登记进 INJECTED_FIELD_PATHS，` +
      ` 不要在调用点写兜底。`
  );
}

/**
 * 读一个清单字段。缺席 ⇒ 报错 + 空清单（**故障标记，非默认值**，见模块头）。
 */
export function injectedList(
  value: readonly string[] | undefined | null,
  path: string
): readonly string[] {
  if (Array.isArray(value)) return value;
  reportMissing(path);
  return EMPTY_LIST;
}

/**
 * 读一个对象字段。缺席 ⇒ 报错 + 空对象（**故障标记，非默认值**，见模块头）。
 *
 * 空对象会让下游控件渲染成"未选中/空"，那是刻意的：边界坏掉时界面必须看起来就是坏的，
 * 而不是显示一份看似正常、实则与内核不符的配置。
 */
export function injectedRecord<T extends object>(value: T | undefined | null, path: string): T {
  if (value != null && typeof value === 'object') return value;
  reportMissing(path);
  return EMPTY_RECORD as T;
}
