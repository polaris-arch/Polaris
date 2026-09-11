//! `config:get` 边界的**生效值注入** —— 前端默认值的唯一替代品。
//!
//! # 这个模块存在的根因（不是"再补一个字段"）
//!
//! `bypassLANList` 曾有一个已修的缺陷：磁盘缺该键时前端用自己的三条兜底
//! （`['localhost','127.0.0.1','192.168.0.0/16']`）顶上，而内核的生效默认是 27 条
//! `DEFAULT_BYPASS_LAN`；`ListEditor` 逐字符 `onChange` ⇒ **首个按键就把那三条错误兜底
//! 当成用户清单持久化**，静默丢弃 24 条真实默认。修法是 [`ensure_bypass_lan_list`]：
//! 在读取边界把字段补成生效值，使前端兜底成为死代码。
//!
//! **然后同一个形态在 `tunConfig.inboundExcludeCidrs` 上原样复现**：Rust 默认
//! `inbound_exclude_cidrs: None`（`tun_config.rs`）、生成侧 `unwrap_or(&[])` ⇒ **内核实际排除 0 条**，
//! 而前端写着 `?? ['100.64.0.0/10']` ⇒ **UI 的折叠计数显示 1 条**。UI 报了一个不存在的排除，
//! 且首个按键会把这条幻影变成真的。
//!
//! 两次是同一个 bug。上一次修的是**一个字段**，不是**一个形态** —— 补丁没有传染性，所以复发。
//! 本模块把它升级成机制：
//!
//!  1. **值的唯一真值源在 Rust**：边界保证 [`INJECTED_FIELD_PATHS`] 里的每个字段在下发时必然在场；
//!  2. **前端一条兜底都不许写**：由 `tests/frontend_sot_guard.rs` 的
//!     `frontend_holds_no_config_default_fallback` 守住，取材面直接读 [`INJECTED_FIELD_PATHS`]
//!     —— 本表加一行，守卫自动覆盖该字段，不必改守卫。
//!
//! # 注入 ≠ 发明默认值
//!
//! 每一条注入的值都必须**逐字等于生成侧实际使用的生效值**，本模块不持有任何独立的默认。
//! 这条不变量由 `tests` 里的镜像测试逐字段钉住（`ensure_* == effective_*` / `unwrap_or` 的那个值）。
//! 注入 `[]` 不是"给它一个空默认"，是**把"缺席"这件事显式化**，好让前端既不必、也不能自己编一个。
//!
//! # 与 `config_version` 的关系
//!
//! 本函数在 `commands::config::apply_frontend_view` 里被调用，而那个投影同时是乐观并发
//! 版本号的**定义域**（两侧各算 hash）。前端 hash 的是它收到的这一份、后端 hash 的是
//! 磁盘经同一投影后的那一份 ⇒ 加字段不会让两侧分叉。**新增注入必须放进这个投影里**，
//! 放到别处（例如只在某个 command 里补）就会制造 conflict 恒真。

use serde_json::Value;

use crate::user_config::system_proxy_bypass::ensure_bypass_lan_list;
use crate::user_config::tun_config::TunModeConfig;

/// 本边界保证「前端拿到时必然在场」的字段路径（`.` 分隔嵌套）。
///
/// **这是前端 SoT 守卫的取材面**（`tests/frontend_sot_guard.rs`）：守卫按本表逐字段扫 `ui/src`，
/// 禁止任何读取点自带兜底。新增一条注入 ⇒ 在此登记 ⇒ 守卫自动开始管它。
///
/// 反过来也成立：**从表里删一行 = 声明"前端可以自己兜底这个字段了"**，那正是本模块要杜绝的事，
/// 删之前必须先说明为什么它不再由 Rust 拥有。
pub const INJECTED_FIELD_PATHS: &[&str] = &[
    "bypassLANList",
    "tunConfig",
    "tunConfig.inboundExcludeCidrs",
];

/// 把配置补成**渲染端可直接消费的生效形**。幂等；已是具体值的字段一律不覆盖。
pub fn ensure_effective_config(cfg: &mut Value) {
    ensure_bypass_lan_list(cfg);
    ensure_tun_config(cfg);
    ensure_inbound_exclude_cidrs(cfg);
}

/// `tunConfig` 缺省 → 注入 `TunModeConfig::default()` 的序列化形。
///
/// 生效值来源是 Rust 的 `Default` 实现本身（不是抄一份字面量）：`skip_serializing_if` 会把
/// 全部 `None` 字段略去，故实得 `{"stack":"auto","autoRoute":true,"strictRoute":true}`
/// —— 与 `polaris-store` 新装播种写的那三键逐字相同，也与前端此前的 `?? {…}` 兜底相同。
/// 三处相同**正是问题**：那意味着有三份默认在各自维护。此后只剩 `Default` 一份，另两处由守卫禁止。
///
/// `mtu` 刻意仍然缺席（`Option::is_none` 略去）：缺席即"自动"，写一个具体数会把当时的默认冻在磁盘上。
fn ensure_tun_config(cfg: &mut Value) {
    let Some(obj) = cfg.as_object_mut() else {
        return;
    };
    // **只补"缺席/null"，不修"畸形"**（与 [`ensure_bypass_lan_list`] 的判据刻意不同）：
    // 那边写的是「不是数组就注入」，于是手改成字符串的 `bypassLANList` 会被静默改写成默认。
    // 本边界是**投影**不是**修复**：前端把收到的这一份原样回传保存，覆盖畸形值 = 悄悄把用户
    // 那份坏配置改写成默认并落盘，既毁掉了排障证据、也可能连带丢掉同对象里的其它键。
    // 畸形值一律原样透出，由 `polaris-store` 的校验去报错——让坏配置**说话**，而不是被抹平。
    if !matches!(obj.get("tunConfig"), None | Some(Value::Null)) {
        return;
    }
    let Ok(default) = serde_json::to_value(TunModeConfig::default()) else {
        return;
    };
    obj.insert("tunConfig".to_string(), default);
}

/// `tunConfig.inboundExcludeCidrs` 缺省 → 注入 `[]`。
///
/// **生效默认就是空**：`TunModeConfig::default()` 给 `None`，而生成侧
/// `builder::inbounds` 取 `.and_then(|t| t.inbound_exclude_cidrs.as_deref()).unwrap_or(&[])`。
/// 此前前端兜底成 `['100.64.0.0/10']`（官方 Tailscale 的 CGNAT 段），于是：
///
///  - **UI 与内核分叉**：折叠计数显示 1 条，内核一条都没排；
///  - **对自建控制面是错的**：headscale 的 `prefixes.v4` 可任意配（现场实例是 `32.0.0.0/24`），
///    这条兜底既排了用不上的段、又漏了真正在用的段，而 UI 上看起来"已经配好了"。
///
/// 官方 CGNAT 段改由输入框 placeholder 承担 —— **建议值不是默认值**：前者用户看得见、
/// 要动手才生效；后者悄悄进内核。这个字段的排除是**双向**的（`builder::inbounds` 的 ⚠️ 注释：
/// 排除一个段会让该段出/入两个方向都绕过 TUN），默认打开一个双向绕过属于危险默认。
fn ensure_inbound_exclude_cidrs(cfg: &mut Value) {
    let Some(obj) = cfg.as_object_mut() else {
        return;
    };
    // `tunConfig` 由 `ensure_tun_config` 先行保证在场；此处仍做在场判断而不 unwrap ——
    // 磁盘上 `tunConfig` 可能是**非对象**（手改成字符串/数字），那种情况本函数不该 panic，
    // 交由既有的 store 校验去报错。
    let Some(tun) = obj.get_mut("tunConfig").and_then(Value::as_object_mut) else {
        return;
    };
    // 判据同 [`ensure_tun_config`]：只补缺席/null，畸形值原样透出。
    if !matches!(tun.get("inboundExcludeCidrs"), None | Some(Value::Null)) {
        return;
    }
    tun.insert("inboundExcludeCidrs".to_string(), Value::Array(vec![]));
}

#[cfg(test)]
mod tests;
