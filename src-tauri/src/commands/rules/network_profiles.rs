//! 网络场景的只读查询：「本机解析后的探测源」（spec §4.4 末条 / §6.1 D8 第一期）。
//!
//! 场景本身的增删改沿用配置写入路径（`config_patch` 写 `networkProfiles` 整表，store 写入层做逐值
//! 校验与保留 id 检查）；规则挂场景沿用规则 IPC（`networkProfileId` 字段，`rules_add/update` 校验引用存在）。

use tauri::State;

use crate::response::ApiResponse;
use crate::runtime::AppRuntime;
use polaris_config_engine::builder::network_env::{BuiltinDhcpStatus, ResolvedProbe};

/// 每个网络场景在本机实际使用的探测源、是否可用、不可用/告警原因码。
///
/// 真值与生成侧同一判据（`ProxyRuntime::network_profile_resolved_sources` → `network_env::resolved_probe`），
/// 渲染端只显示、不重算（两处各算一份会漂移）。读的是磁盘上的当前配置（下次起核会用的那份）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn network_profile_resolved_sources(
    state: State<'_, AppRuntime>,
) -> ApiResponse<Vec<ResolvedProbe>> {
    let cfg = match state.config().load_full() {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("{e}")),
    };
    match state.proxy().network_profile_resolved_sources(&cfg) {
        Ok(list) => ApiResponse::ok(list),
        Err(e) => ApiResponse::err(e),
    }
}

/// 内置解析器「当前网络 DHCP 下发的 DNS」（`builtin-netenv-dhcp`，D5）在本机是否可用。
///
/// 不可用时引用它的 DNS 规则整条不生成（生成侧 B，同一判据），UI 据此在 DNS 动作选择处提示。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn network_profile_builtin_dhcp_status(
    state: State<'_, AppRuntime>,
) -> ApiResponse<BuiltinDhcpStatus> {
    let cfg = match state.config().load_full() {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("{e}")),
    };
    match state.proxy().network_profile_builtin_dhcp_status(&cfg) {
        Ok(status) => ApiResponse::ok(status),
        Err(e) => ApiResponse::err(e),
    }
}
