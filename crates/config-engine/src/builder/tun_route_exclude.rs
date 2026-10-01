//! TUN 排除段计算（上游 `shared/tun-route-exclude.ts` 1:1 移植）。
//!
//! computeUserTunExclude（连入来源排除，减 mesh/fakeip/macOS 物理 LAN）+
//! computeWinBypassExclude（Windows bypassLAN carve，算术差集挖 engaged mesh 段）。

#![forbid(unsafe_code)]

use crate::user_config::cidr::{partition_cidrs_by_overlap, subtract_cidrs};
use crate::user_config::collections::dedupe;
// **必须**用 rule_validate 的严格校验（上游 `rules.isValidIpCidr` 的对位移植）：八位组 ≤255 / 禁前导零 /
// 掩码 v4≤32·v6≤128 / IPv6 结构合法。system_proxy_bypass::is_ip_cidr 是形状粗判（对位 上游 `isIpCidr`，
// 只数点分段数与位数），`256.1.1.1/24`、`10.0.0.0/40` 都能过——这些串进 route_exclude_address 会让
// sing-box `netip.ParsePrefix` 启动 FATAL，正是本函数存在的理由。
use crate::user_config::rule_validate::is_valid_ip_cidr;

const V4_MIN_PREFIX: u32 = 8;
const V6_MIN_PREFIX: u32 = 7;

/// 规范化 + 校验排除条目（裸 IP 补掩码，拒 catch-all/过宽/非法）。上游 `normalizeTunExcludeCidr`。
pub fn normalize_tun_exclude_cidr(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let is_v6 = t.contains(':');
    let cidr = if t.contains('/') {
        t.to_string()
    } else {
        format!("{t}/{}", if is_v6 { 128 } else { 32 })
    };
    if !is_valid_ip_cidr(&cidr) {
        return None;
    }
    let prefix_str = cidr.split('/').nth(1).unwrap_or("32");
    let prefix: u32 = prefix_str.parse().ok()?;
    if prefix < if is_v6 { V6_MIN_PREFIX } else { V4_MIN_PREFIX } {
        return None;
    }
    Some(cidr)
}

/// 用户排除输入。上游 `UserTunExcludeInput`。
pub struct UserTunExcludeInput<'a> {
    pub platform: &'a str,
    pub user_cidrs: &'a [String],
    pub mesh_cidrs: &'a [String],
    pub fakeip_ranges: &'a [String],
    pub own_lan_cidrs: &'a [String],
}

/// 用户排除结果。上游 `UserTunExcludeResult`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserTunExcludeResult {
    pub extra: Vec<String>,
    pub dropped_invalid: usize,
    pub dropped_mesh_overlap: Vec<String>,
    pub dropped_fakeip_overlap: Vec<String>,
    pub dropped_own_lan_mac: Vec<String>,
}

/// 计算用户声明 TUN 排除段的最终生效集。上游 `computeUserTunExclude`。
pub fn compute_user_tun_exclude(input: &UserTunExcludeInput) -> UserTunExcludeResult {
    let mut dropped_invalid = 0;
    let normalized: Vec<String> = input
        .user_cidrs
        .iter()
        .filter_map(|raw| match normalize_tun_exclude_cidr(raw) {
            Some(c) => Some(c),
            None => {
                dropped_invalid += 1;
                None
            }
        })
        .collect();
    let valid = dedupe(normalized);

    let (mesh_overlap, mesh_disjoint) = partition_cidrs_by_overlap(&valid, input.mesh_cidrs);
    let (fakeip_overlap, fakeip_disjoint) =
        partition_cidrs_by_overlap(&mesh_disjoint, input.fakeip_ranges);

    let (extra, dropped_own_lan_mac) = if input.platform == "darwin" {
        let (lan_overlap, lan_disjoint) =
            partition_cidrs_by_overlap(&fakeip_disjoint, input.own_lan_cidrs);
        (lan_disjoint, lan_overlap)
    } else {
        (fakeip_disjoint, vec![])
    };

    UserTunExcludeResult {
        extra,
        dropped_invalid,
        dropped_mesh_overlap: mesh_overlap,
        dropped_fakeip_overlap: fakeip_overlap,
        dropped_own_lan_mac,
    }
}

/// 落在 `base` 某条目内（= 与之相交）的 carve 候选段，去重后保序。
///
/// 与 `base` 不相交的候选段对差集**没有任何作用**，却会把下面那道「无可 carve ⇒ 原样返回」的
/// 短路打掉 —— 而短路正是「该 carve 时才 carve」这条不变量的落点（理由见 [`carve_out`]）。
/// 两个 carve 调用点（Windows 内核排除表、`builder::route` 块 1 的 bypass 直连表）共用本函数，
/// 免得一处过滤、另一处不过滤，同一份输入在两张表上给出不同形态。
pub(crate) fn carve_candidates_within(base: &[String], carve: &[String]) -> Vec<String> {
    dedupe(carve.iter().cloned())
        .into_iter()
        .filter(|c| crate::user_config::cidr::cidr_overlaps_any(c, base))
        .collect()
}

/// 从 `base` 里算术挖掉 `carve`；**无可 carve ⇒ 原样返回**。
///
/// 短路不是省一次计算：[`subtract_cidrs`] 会把每条能解析的段按 `(网络号, 前缀)` 重新格式化
/// （`10.0.0.1/8` → `10.0.0.0/8`），于是「carve 集为空」这一格若也走差集，产物里的字面量会被
/// 无声改写一遍 —— 对内核语义无影响，但会让「没有组网节点时这张表与 carve 之前**逐字节相同**」
/// 这条负向对照失去判别力（它本来就是用来证明 carve 只在该发生时发生的）。
pub(crate) fn carve_out(base: Vec<String>, carve: &[String]) -> Vec<String> {
    if carve.is_empty() {
        return base;
    }
    subtract_cidrs(&base, carve)
}

/// Windows bypassLAN carve 保护段（回环/链路本地/多播）。
const WIN_BYPASS_CARVE_GUARD: &[&str] = &[
    "127.0.0.0/8",
    "::1/128",
    "169.254.0.0/16",
    "fe80::/10",
    "224.0.0.0/4",
];

/// Windows bypassLAN 输入。上游 `WinBypassExcludeInput`。
pub struct WinBypassExcludeInput<'a> {
    pub bypass_cidrs: &'a [String],
    pub engaged_mesh_cidrs: &'a [String],
    pub own_lan_cidrs: &'a [String],
    pub fakeip_ranges: &'a [String],
}

/// Windows bypassLAN 结果。上游 `WinBypassExcludeResult`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WinBypassExcludeResult {
    pub exclude: Vec<String>,
    pub carved_mesh_cidrs: Vec<String>,
    pub mesh_skipped_own_lan: Vec<String>,
}

/// Windows bypassLAN 内核排除表 carve。上游 `computeWinBypassExclude`。
pub fn compute_win_bypass_exclude(input: &WinBypassExcludeInput) -> WinBypassExcludeResult {
    // 1. fakeip 整条剔除。
    let (_fakeip_overlap, after_fakeip) =
        partition_cidrs_by_overlap(input.bypass_cidrs, input.fakeip_ranges);

    // 2. 只考虑落在某 bypass 条目内的 engaged mesh 段。
    let relevant_mesh = carve_candidates_within(&after_fakeip, input.engaged_mesh_cidrs);

    // 3. 分流：与保护段（物理子网 + guard）相交的段不 carve。
    let mut guard_with_lan: Vec<String> = input.own_lan_cidrs.to_vec();
    guard_with_lan.extend(WIN_BYPASS_CARVE_GUARD.iter().map(|s| s.to_string()));
    let (mesh_skipped_own_lan, carve_mesh) =
        partition_cidrs_by_overlap(&relevant_mesh, &guard_with_lan);

    // 4. 算术差集（无可 carve ⇒ `carve_out` 原样返回，不经重格式化）。
    WinBypassExcludeResult {
        exclude: carve_out(after_fakeip, &carve_mesh),
        carved_mesh_cidrs: carve_mesh,
        mesh_skipped_own_lan,
    }
}

#[cfg(test)]
mod tests;
