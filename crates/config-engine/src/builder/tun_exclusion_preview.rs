//! 「本平台这份配置下，TUN 实际排除了哪些网段」—— 给 UI 看的**生效值**，不是又一份计算。
//!
//! # 为什么要有它（本模块的存在理由就是一次真实事故）
//!
//! 2026-09-08：用户在 macOS 上跑自建 Tailscale 控制面（tailnet 前缀 `32.0.0.0/24`），
//! 与 Polaris 的 TUN 冲突。外部给的排查建议是"往 `bypassLANList` 加那两个网段"。
//! 那条建议**在 macOS 上完全无效** —— 这张表只在 `platform == "win32"` 才喂给
//! `route_exclude_address`（`builder::inbounds`），mac 分支只放回环。真正该填的是
//! `tunConfig.inboundExcludeCidrs`。
//!
//! 用户没法从界面上看出这件事：两张表长得一样、都叫"排除"，谁在本平台真的生效毫无提示。
//! 更糟的是当时 `inboundExcludeCidrs` 的**折叠计数显示 1 条**（前端兜底 `['100.64.0.0/10']`），
//! 而内核实际排除 **0 条**（Rust 默认 `None` → `unwrap_or(&[])`）。
//!
//! # 为什么不是"按平台补几条说明文案"
//!
//! 那是**表同步型补丁**：一份手写的平台→归宿映射要与 `build_inbounds` 里的 `if` 分支两处维护，
//! 漂了不会红（文案和代码各自都自洽）。判据必须由代码持有 —— 所以本模块**不解释规则，直接给结果**：
//! 跑真正的 `build_inbounds`，把它交给内核的那一份原样读回来。
//!
//! 于是没有表可同步：用户看到的就是内核要吃的那一份。上面那次事故里，用户当时会直接看到
//! 「本平台实际排除：(空)」，而不是一个显示 1 条、内核 0 条的折叠计数。
//!
//! # 实现上的唯一花招：诊断捕获
//!
//! 静默剔除的四类原因（非法/过宽、组网段重叠、fakeip 段重叠、macOS 物理 LAN 相交）只以
//! `deps.log` 的 warn 形式存在，而 [`InboundsDeps::log`] 是**裸函数指针**（`fn`，不是闭包），
//! 捕不住环境。改成 `&dyn Fn` 要动 `GenerateConfigDeps` 的所有构造点 —— 为一个预览面做那种半径
//! 的改动不划算，且会把风险塞进真正的起核路径。
//!
//! 故用线程局部缓冲 + 一个写进它的裸函数：作用域只在本模块内，生产起核路径一个字节都不动。
//! [`preview_tun_exclusion`] 是同步的、单线程内自洽（进入时清空、返回前取走）。

#![forbid(unsafe_code)]

use std::cell::RefCell;

use crate::builder::endpoint_routes::ObservedTailnetAddresses;
use crate::builder::inbounds::{build_inbounds, InboundsDeps};
use crate::user_config::app_config::UserConfig;
use crate::user_config::LogLevel;

thread_local! {
    /// 本次预览捕获到的诊断行。仅 [`preview_tun_exclusion`] 读写。
    static CAPTURED: RefCell<Vec<PreviewNote>> = const { RefCell::new(Vec::new()) };
}

/// 一条生成期诊断。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreviewNote {
    /// `warn` / `info` / …（`LogLevel` 的字符串形，前端按它选样式）。
    pub level: String,
    pub message: String,
}

/// 预览结果。
///
/// `rename_all = "camelCase"`：前端 `contracts/tun-exclusion-preview.ts` 按 camelCase 声明，
/// 漏了这行会让 `tunActive` 恒 `undefined` —— 而 `undefined` 是假值，UI 会把"TUN 正在跑"
/// 显示成"当前不是 TUN 模式"，且 tsc 与两侧单测都不会红。由 `wire_shape_is_camel_case` 钉住。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunExclusionPreview {
    /// **真正下发给内核**的 `route_exclude_address`。
    ///
    /// 空数组是**有意义的结果**，不是"没算出来"：它就是"本平台这份配置下一条都不排除"。
    /// UI 必须把空态显示成空，不许用任何兜底顶上 —— 那正是本模块要终结的那类谎。
    pub effective: Vec<String>,
    /// 生成期诊断，含四类静默剔除（非法/过宽、组网重叠、fakeip 重叠、macOS 物理 LAN）
    /// 与「Linux 恒忽略本表」那条。用户填了却没生效时，原因只在这里。
    pub notes: Vec<PreviewNote>,
    /// 该平台是否根本不产出 TUN inbound（非 TUN 模式）。此时 `effective` 恒空且无意义。
    pub tun_active: bool,
}

fn capture(level: LogLevel, message: &str) {
    CAPTURED.with(|c| {
        c.borrow_mut().push(PreviewNote {
            level: format!("{level:?}").to_lowercase(),
            message: message.to_owned(),
        });
    });
}

/// 跑一次真正的 `build_inbounds`，读回它交给内核的 `route_exclude_address` 与诊断。
///
/// `platform` / `own_lan_cidrs` / `observed_tailnet_addresses` 三个都必须来自**运行期真值**
/// （`platform_tag()` / `enumerate_own_lan_cidrs()` / `ProxyRuntime::observed_tailnet_snapshot()`），
/// 不是猜的、更不是图省事传空：
/// - darwin 分支用 `own_lan_cidrs` 做减法，传空会让预览比实际多显示几条；
/// - `observed_tailnet_addresses` 喂 `engaged_mesh`（组网段重叠剔除的减数）。A-0b 把观测面接进
///   生产的 `GenerateConfigDeps` 之后，这里传空就等于**预览又变回了另一份计算** —— 界面显示的
///   排除面与内核吃的那一份不再是同一份，正是本模块头注那次事故的形态。
///
/// 端口类入参一律给中性值 —— 它们只影响别的 inbound，与排除面无关。这条不变量由
/// `preview_matches_real_build` 钉住（同一份配置**与同一份观测面**，预览结果必须与真实生成
/// 逐条相等；该门同时带一条反向对照：只喂一侧观测必须让两边分叉）。
#[must_use]
pub fn preview_tun_exclusion(
    config: &UserConfig,
    platform: &str,
    own_lan_cidrs: Vec<String>,
    observed_tailnet_addresses: ObservedTailnetAddresses,
) -> TunExclusionPreview {
    CAPTURED.with(|c| c.borrow_mut().clear());

    let deps = InboundsDeps {
        probe_direct_port: None,
        probe_proxy_port: None,
        update_in_port: None,
        subscription_update_in_port: None,
        probe_pool_ports: vec![],
        platform: platform.to_owned(),
        own_lan_cidrs,
        log: capture,
        // A-0b 已接线：运行期观测到的 tailnet 地址由调用方（`commands::config` 取
        // `ProxyRuntime::observed_tailnet_snapshot()`）透进来，与起核时喂给
        // `GenerateConfigDeps` 的**同一份快照**。
        //
        // 此前这里硬传空 —— 那让 `preview_matches_real_build` 变成了「两边都传空的时候相等」，
        // 门还在、牙没了（测试环境比生产宽容 ⇒ 绿无信息量）。现在它由参数决定，且那道门带
        // 反向对照：只喂一侧观测必须分叉。
        observed_tailnet_addresses,
    };
    let inbounds = build_inbounds(config, None, &deps);
    let tun = inbounds.iter().find(|i| i.type_field == "tun");

    TunExclusionPreview {
        effective: tun
            .and_then(|t| t.route_exclude_address.clone())
            .unwrap_or_default(),
        notes: CAPTURED.with(|c| std::mem::take(&mut *c.borrow_mut())),
        tun_active: tun.is_some(),
    }
}

#[cfg(test)]
mod tests;
