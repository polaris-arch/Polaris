//! 节点（server）类 command（上游 `server-handlers.ts`）。
//!
//! 映射 channel：
//! - `server:add` → [`server_add`]
//! - `server:addBulk` → [`server_add_bulk`]
//! - `server:update` → [`server_update`]
//! - `server:delete` → [`server_delete`]
//! - `server:deleteBatch` → [`server_delete_batch`]
//! - `server:switch` → [`server_switch`]
//! - `server:generateUrl` → [`server_generate_url`]（config-engine ProtocolParser 等价）
//! - `tailcat_keypair` → [`tailcat_keypair`]（Tailcat 客户端密钥对：生成 / 由私钥推导公钥）
//! - `warp:register` / `warp:applyLicense` → [`warp_register`] / [`warp_apply_license`]（mesh crate）
//! - `tailscale:login` / `loginCancel` / `logout` / `stateExists` / `getStatus` → tailscale_* （mesh crate）
//!
//! 节点 CRUD 经 config 的 load/save（servers 数组原地改 + 原子写）+ 广播 event:configChanged。
//! DIRECT_SERVER_ID 哨兵 + 删选中节点的兜底出口逻辑对齐 Polaris（D4/F-1）。

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, State};

use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::dns_constants::BLOCK_SERVER_ID;
use polaris_config_engine::user_config::server_config::ServerConfig;
use polaris_config_engine::user_config::ProxyMode;
use polaris_mesh::warp_http::RegisterOptions;

use crate::commands::config::{broadcast_config_changed, emit_config_changed_signal};
use crate::response::{ok_void, ApiResponse};
use crate::runtime::config::{ConfigManager, Decision};
use crate::runtime::proxy::{code, SwitchOutcome};
use crate::runtime::tailscale_login_core::StartLoginOutcome;
use crate::runtime::unlock::{selected_exit_changed, BroadcastSink};
use crate::runtime::AppRuntime;

/// Polaris 直接选择哨兵（`shared/direct-selection.ts DIRECT_SERVER_ID`）。
const DIRECT_SERVER_ID: &str = "__direct__";

/// id 缺失 / 空 → mint uuid（镜像 [`server_add_bulk`] 的 `s["id"]=new_uuid()`）。
///
/// 此前 `server_add` 直接 push 原值不补 id → `store::sanitize` 丢弃 id 缺失/空的节点（要求 id 非空字符串）
/// → 克隆 / 手动加的节点产出不可用、不持久。非对象入参不动（随后由新增校验拒绝）。
fn ensure_server_id(mut server: Value) -> Value {
    if let Some(obj) = server.as_object_mut() {
        let has_id = obj
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        if !has_id {
            obj.insert("id".to_string(), json!(new_uuid()));
        }
    }
    server
}

/// 用 ConfigStore 同一套规则校验/规范化新增记录，避免被清洗掉却向调用者报成功。
fn normalized_add_server(server: Value) -> Result<Value, String> {
    let cleaned = polaris_store::sanitize_config(&json!({"servers": [server]}).to_string())
        .map_err(|_| "新增节点校验失败".to_string())?;
    let normalized = cleaned["servers"]
        .as_array()
        .and_then(|servers| servers.first())
        .cloned()
        .ok_or_else(|| "新增节点缺少必填字段或协议配置无效".to_string())?;
    // The store deliberately preserves unknown/optional fields. A wrong-typed known field
    // (for example tlsSettings as a string) must still be rejected before a successful add.
    serde_json::from_value::<ServerConfig>(normalized.clone())
        .map_err(|_| "新增节点协议配置字段类型无效".to_string())?;
    Ok(normalized)
}

/// 补 id + 校验 + 原子落盘。同 id 同规范化内容重试不再写入；冲突不覆盖既有节点。
fn server_add_core(config: &ConfigManager, server: Value) -> Result<Value, String> {
    let server = normalized_add_server(ensure_server_id(server))?;
    let id = server["id"].as_str().expect("sanitize 保证非空 id");
    let (result, saved) = config
        .update(|cfg| {
            let Some(servers) = cfg.get("servers").and_then(Value::as_array) else {
                return Decision::Skip(Err("节点列表不可用".to_string()));
            };
            if let Some(existing) = servers.iter().find(|node| node["id"].as_str() == Some(id)) {
                return if normalized_add_server(existing.clone()).as_ref() == Ok(&server) {
                    Decision::Skip(Ok(cfg.clone()))
                } else {
                    Decision::Skip(Err("节点标识已存在且内容不同".to_string()))
                };
            }
            // 检查集合级清洗约束；仅在副本上验证，不把清洗后的集合写回旧节点。
            let mut proposed = servers.clone();
            proposed.push(server.clone());
            let cleaned =
                match polaris_store::sanitize_config(&json!({"servers": proposed}).to_string()) {
                    Ok(cleaned) => cleaned,
                    Err(_) => return Decision::Skip(Err("新增节点校验失败".to_string())),
                };
            if !cleaned["servers"]
                .as_array()
                .is_some_and(|nodes| nodes.iter().any(|node| node["id"].as_str() == Some(id)))
            {
                return Decision::Skip(Err("新增节点与现有节点配置冲突".to_string()));
            }
            cfg["servers"].as_array_mut().unwrap().push(server.clone());
            Decision::Write(Ok(Value::Null))
        })
        .map_err(|e| format!("{e}"))?;
    result.map(|unchanged| saved.unwrap_or(unchanged))
}

/// 上游 `SERVER_ADD`：新增节点（id 缺失/空则 mint，防 sanitize 丢弃）。
///
/// DESIGN-REVIEW(mesh-singleton-guard-renderer-only)：WARP 的 UI 单例槽判据在渲染端
/// （`ui/src/domain/endpoint-routes.ts#meshSingletonConflict`）。上游存储允许多个独立的
/// Tailscale 节点，本命令不得把旧 TS 单例 UX 误写成持久化约束。本命令不另造 WARP 启发式守卫；
/// 但 `server:add` 会复用 ConfigStore 既有清洗策略，确保新增节点能保留，不能把策略拒绝报成成功。
/// [`server_add_bulk`] 仍沿用原批量导入路径。UI 判据不在 Rust 侧重写，理由：
///  1. 判定谓词 `isWarpServer` 在 Rust 侧无对应物（`domain/warp.ts` 头注已登记此边界：输入均在前端
///     store，漂移后果止于 UI）。在此复刻一份「端点域名兜底 + warpDevice 标记」的启发式 = 造第二真值源，
///     无 codegen 约束，日后必然与渲染端的 WARP 识别分叉。
///  2. 误判方向不可接受：本命令同时是备份恢复 / 导入的落盘substrate，Rust 侧启发式误拒一个合法节点，
///     用户在 UI 上无从修复；而渲染端误拒最多是弹一次错、用户改地址重来。
///  3. 威胁模型：`server:add` 只被本应用自己的 webview 调用，不接受外部不可信输入。
///
/// 若日后新增**非渲染端**的写入方（CLI / 深链接 / 远程配置下发），本决定即失效，须在此补守卫。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn server_add(app: AppHandle, state: State<'_, AppRuntime>, server: Value) -> ApiResponse<()> {
    match server_add_core(state.config(), server) {
        Ok(cfg) => {
            broadcast_config_changed(&app, &cfg);
            ok_void()
        }
        Err(e) => ApiResponse::err(e),
    }
}

/// `server:addBulk` 核心（注入 `ConfigManager`，便于真实 ConfigStore 驱动测试）：强制重生成 id +
/// 剥离订阅归属 + 补时间戳，落盘，返回 `(新 config, 实际新增数)`。
///
/// `added` 须在**消费 `servers` 之前**快照——下方两分支（`for` / `init` move）都会吃掉 `servers`；
/// 对齐 上游 `SERVER_ADD_BULK`（`server-handlers.ts`：`added = list.length`，入参全部入库、无逐项过滤）。
/// 此前命令层硬编码 `"added":0` → 契约破（消费方若按 added 判成功恒见 0）。
fn server_add_bulk_core(
    config: &ConfigManager,
    servers: Vec<Value>,
) -> Result<(Value, usize), String> {
    let added = servers.len();
    let now = current_iso();
    let (_, saved) = config
        .update(|cfg| {
            if let Some(arr) = cfg.get_mut("servers").and_then(Value::as_array_mut) {
                for mut s in servers {
                    // 强制重生成 id（防撞）+ 剥离订阅归属（自建节点恒可编辑可删除）。
                    s["id"] = json!(new_uuid());
                    if let Some(obj) = s.as_object_mut() {
                        obj.remove("subscriptionId");
                        obj.remove("providerName");
                    }
                    s["createdAt"] = s.get("createdAt").cloned().unwrap_or_else(|| json!(now));
                    s["updatedAt"] = json!(now);
                    arr.push(s);
                }
            } else {
                // servers 字段缺失/非数组 → 用 servers 入参（已补全 id/时间）初始化。
                let mut init: Vec<Value> = servers;
                for s in &mut init {
                    s["id"] = json!(new_uuid());
                    s["updatedAt"] = json!(now);
                }
                if let Some(obj) = cfg.as_object_mut() {
                    obj.insert("servers".to_string(), Value::Array(init));
                }
            }
            Decision::Write(())
        })
        .map_err(|e| format!("{e}"))?;
    Ok((
        saved.expect("server_add_bulk 的 Write 腿必须返回已落盘配置"),
        added,
    ))
}

/// 上游 `SERVER_ADD_BULK`：批量新增自建节点（强制重生成 id + 剥离 subscriptionId/providerName）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn server_add_bulk(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    servers: Vec<Value>,
) -> ApiResponse<Value> {
    if servers.is_empty() {
        return ApiResponse::ok(json!({ "added": 0 }));
    }
    match server_add_bulk_core(state.config(), servers) {
        Ok((cfg, added)) => {
            broadcast_config_changed(&app, &cfg);
            ApiResponse::ok(json!({ "added": added }))
        }
        Err(e) => ApiResponse::err(e),
    }
}

/// 上游 `SERVER_UPDATE`：更新节点（按 id）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn server_update(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    server: Value,
) -> ApiResponse<()> {
    let id = server.get("id").and_then(Value::as_str).map(str::to_string);
    let id = match id {
        Some(i) => i,
        None => return ApiResponse::err("server.id required"),
    };
    match state.config().update(|cfg| {
        let found = cfg
            .get_mut("servers")
            .and_then(Value::as_array_mut)
            .and_then(|arr| {
                arr.iter()
                    .position(|s| s.get("id").and_then(Value::as_str) == Some(&id))
                    .map(|idx| {
                        arr[idx] = server;
                    })
            })
            .is_some();
        if found {
            Decision::Write(Ok(()))
        } else {
            Decision::Skip(Err(format!("服务器不存在: {id}")))
        }
    }) {
        Ok((Ok(()), Some(cfg))) => {
            broadcast_config_changed(&app, &cfg);
            ok_void()
        }
        Ok((Err(error), None)) => ApiResponse::err(error),
        Ok(_) => unreachable!("server_update decision and persistence must agree"),
        Err(e) => ApiResponse::err(format!("{e}")),
    }
}

/// 删选中节点后的新 `selectedServerId`（D4）：兜底出口须**存活于删除后的 `cfg.servers`** 才采用，否则回落直连
/// 哨兵（**不可置 null**：0 节点重启会 throw）。
///
/// 单删 / 批删共用同一份 viable 校验——此前批删压根没有 `fallback_selected_id` 形参、恒落直连，与单删口径分叉。
fn resolve_fallback_selected(cfg: &Value, fallback_selected_id: Option<&str>) -> String {
    let viable = fallback_selected_id.is_some_and(|fb| {
        cfg.get("servers")
            .and_then(Value::as_array)
            .is_some_and(|arr| {
                arr.iter()
                    .any(|s| s.get("id").and_then(Value::as_str) == Some(fb))
            })
    });
    match fallback_selected_id {
        Some(fb) if viable => fb.to_string(),
        _ => DIRECT_SERVER_ID.to_string(),
    }
}

/// 删节点后更新选中出口（单删 / 批删共用的纯 `Value` 变换）：选中节点落在删除集内（`selected_removed`）→
/// 兜底出口（存活候选或直连哨兵），否则不动。`cfg` 须为**已剔除被删节点后**的配置（viable 校验看删后 servers）。
///
/// A7：出口只可能从被删的旧 id 变为 viable 兜底 / `__direct__` 哨兵（**恒不等于被删 id**）——故 `selected_removed`
/// 为真即出口变；命令层落盘后再用 `selected_exit_changed(old, new)` 统一判定失效（此腿走哨兵，不产生 →null）。
fn apply_selection_fallback(
    cfg: &mut Value,
    selected_removed: bool,
    fallback_selected_id: Option<&str>,
) {
    if selected_removed {
        let next = resolve_fallback_selected(cfg, fallback_selected_id);
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("selectedServerId".to_string(), json!(next));
        }
    }
}

/// 上游 `SERVER_DELETE`：删除单节点（删选中 → 兜底出口）+ tailscale/warp 副作用。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn server_delete(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    server_id: String,
    fallback_selected_id: Option<String>,
) -> ApiResponse<()> {
    let result = state.config().update_deferred_cleanup(|cfg| {
        let old_selected = cfg
            .get("selectedServerId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let removed = cfg
            .get_mut("servers")
            .and_then(Value::as_array_mut)
            .and_then(|arr| {
                arr.iter()
                    .position(|s| s.get("id").and_then(Value::as_str) == Some(&server_id))
                    .map(|idx| arr.remove(idx))
            });
        let Some(_removed) = removed else {
            return Decision::Skip(Err(format!("服务器不存在: {server_id}")));
        };
        let was_selected = old_selected.as_deref() == Some(&server_id);
        apply_selection_fallback(cfg, was_selected, fallback_selected_id.as_deref());
        prune_recent_server_ids_to_existing(cfg);
        Decision::Write(Ok(old_selected))
    });
    match result {
        Ok((Ok(old_selected), Some(cfg))) => {
            broadcast_config_changed(&app, &cfg);
            // A7：删当前选中 → selectedServerId 回落 viable/`__direct__`（恒 != 旧 id）= 出口变 →
            // 作废旧出口解锁探测缓存（否则解锁角标最长陈旧 30min）。删非选中节点 → 出口不动、不失效。
            if selected_exit_changed(
                old_selected.as_deref(),
                cfg.get("selectedServerId").and_then(Value::as_str),
            ) {
                let sink = BroadcastSink::new(&app);
                let running = state.proxy().status().running;
                state.unlock().invalidate(&sink, running, false);
            }
            ok_void()
        }
        Ok((Err(error), None)) => ApiResponse::err(error),
        Ok(_) => unreachable!("server_delete decision and persistence must agree"),
        Err(e) => ApiResponse::err(format!("{e}")),
    }
}

/// 上游 `SERVER_DELETE_BATCH`：批量删除（返回实际删除数）。
///
/// `fallback_selected_id`：选中节点落在删除集内时的兜底出口（渲染端按「剩余节点里最快」算，见 `pickFallbackExit`）。
/// 此前**无此形参** → 前端传的 key 被 Tauri 静默丢弃、批删掉当前出口恒落直连（流量裸奔）；现与单删同口径。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn server_delete_batch(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    server_ids: Vec<String>,
    fallback_selected_id: Option<String>,
) -> ApiResponse<u32> {
    let id_set: std::collections::HashSet<&str> = server_ids.iter().map(String::as_str).collect();
    let result = state.config().update_deferred_cleanup(|cfg| {
        let old_selected = cfg
            .get("selectedServerId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let removed: Vec<Value> = match cfg.get_mut("servers").and_then(Value::as_array_mut) {
            Some(arr) => {
                let (keep, drop) = arr.drain(..).partition(|s| {
                    !id_set.contains(s.get("id").and_then(Value::as_str).unwrap_or(""))
                });
                *arr = keep;
                drop
            }
            None => Vec::new(),
        };
        if removed.is_empty() {
            return Decision::Skip((old_selected, 0u32));
        }
        let selected_in_set = old_selected
            .as_deref()
            .is_some_and(|sid| id_set.contains(sid));
        apply_selection_fallback(cfg, selected_in_set, fallback_selected_id.as_deref());
        prune_recent_server_ids_to_existing(cfg);
        Decision::Write((old_selected, u32::try_from(removed.len()).unwrap_or(0)))
    });
    match result {
        Ok(((_old_selected, 0), None)) => ApiResponse::ok(0u32),
        Ok(((old_selected, removed_count), Some(cfg))) => {
            broadcast_config_changed(&app, &cfg);
            // A7：选中落删除集 → selectedServerId 回落 viable/`__direct__`（恒 != 旧 id）= 出口变 →
            // 作废旧出口解锁探测缓存。选中不在删除集 → 出口不动、不失效。
            if selected_exit_changed(
                old_selected.as_deref(),
                cfg.get("selectedServerId").and_then(Value::as_str),
            ) {
                let sink = BroadcastSink::new(&app);
                let running = state.proxy().status().running;
                state.unlock().invalidate(&sink, running, false);
            }
            ApiResponse::ok(removed_count)
        }
        Ok(_) => unreachable!("server_delete_batch decision and persistence must agree"),
        Err(e) => ApiResponse::err(format!("{e}")),
    }
}

/// 最近连接节点入队（托盘「节点·最近」MRU，原型 `pickNode`：
/// `st.mru = [name, ...st.mru.filter(x=>x!==name)].slice(0,3)` 同款语义）：去重后插入队首，上限 3
/// （与原型的 `.slice(0,3)` 对齐，前端再叠加「当前节点置顶」渲染，见 TrayMenu.tsx recentItems）。
///
/// 只在真实节点切换（[`server_switch`]）时调用；直连哨兵切换走 `config_save` 全量保存，不经此路径 ——
/// 与原型 `pickDirectExit` 不碰 `st.mru` 对齐。存量配置无 `recentServerIds` 字段 → 视作空历史，非结构错误。
fn push_recent_server_id(obj: &mut Map<String, Value>, server_id: &str) {
    let mut recent: Vec<String> = obj
        .get("recentServerIds")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    recent.retain(|id| id != server_id);
    recent.insert(0, server_id.to_string());
    recent.truncate(3);
    obj.insert("recentServerIds".to_string(), json!(recent));
}

/// 从托盘 MRU 历史剔除已删除节点的 id（单删 / 批删共用的纯 `Value` 变换）。
///
/// # 为什么必须剔
///
/// `TrayMenu` 渲染「节点·最近」时按 id 反查 `ServerConfig`，**查不到即跳过且不回填**
/// （`ids.map(id => servers.find(...)).filter(Boolean)`）⇒ 一个指向已删节点的死 id 会永久占住
/// `truncate(3)` 的三个槽位之一，表现为「最近节点恒少显示一条」。删除节点是这份 MRU 的所有者
/// （后端）唯一能观察到「该 id 已失效」的时机 —— 前端对该字段无写入权，剔不了也不该剔。
///
/// 无 `recentServerIds` 键（存量配置）→ 空操作，不凭空建键。剔后为空 → 落空数组（不删键，
/// 保持形状稳定；空数组与缺键对读侧 `?? []` 等价）。
fn prune_recent_server_ids(cfg: &mut Value, is_removed: impl Fn(&str) -> bool) {
    let Some(obj) = cfg.as_object_mut() else {
        return;
    };
    let Some(arr) = obj.get_mut("recentServerIds").and_then(Value::as_array_mut) else {
        return;
    };
    arr.retain(|v| v.as_str().is_some_and(|id| !is_removed(id)));
}

/// 按当前 `servers` 真值清理 MRU。供专用删除命令与全量 `config:save` 共用：后者是暂存删除的落盘腿，
/// 不经过 `server_delete`，若不在保存边界复用同一不变量，普通节点暂存删除后会留下永久占槽的死 id。
pub(crate) fn prune_recent_server_ids_to_existing(cfg: &mut Value) {
    let Some(servers) = cfg.get("servers").and_then(Value::as_array) else {
        // 没有节点真值时不能把「未知」解释成「空集合」：最小补丁/迁移配置可能只带后端权威 MRU，
        // 此时清空它会反向破坏 `enforce_backend_authoritative_fields` 的保留契约。
        return;
    };
    let existing: std::collections::HashSet<String> = servers
        .iter()
        .filter_map(|server| server.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    prune_recent_server_ids(cfg, |id| !existing.contains(id));
}

/// `server:switch` 的原子读改写核心。快速连点会并发到达 command 线程；必须复用
/// [`ConfigManager::update`] 把「验证节点 → 取旧出口 → 改选中/MRU → 落盘」圈成一个动作，
/// 否则两个请求都基于同一份旧配置写回，较慢者会覆盖较新的选择与 MRU。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerSwitchError {
    InterfaceUnavailable(String),
    Other(String),
}

impl std::fmt::Display for ServerSwitchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InterfaceUnavailable(message) | Self::Other(message) => f.write_str(message),
        }
    }
}

fn server_switch_core<F>(
    config: &ConfigManager,
    server_id: &str,
    validate_candidate: impl FnOnce(&UserConfig) -> Result<(), String>,
    register_intent: F,
) -> Result<(Value, bool, u64), ServerSwitchError>
where
    F: FnOnce() -> u64,
{
    let mut validate_candidate = Some(validate_candidate);
    let mut register_intent = Some(register_intent);
    let (exit_changed, saved) = config
        .update(|cfg| {
            let exists = cfg
                .get("servers")
                .and_then(Value::as_array)
                .is_some_and(|arr| {
                    arr.iter()
                        .any(|s| s.get("id").and_then(Value::as_str) == Some(server_id))
                });
            if !exists && server_id != DIRECT_SERVER_ID && server_id != BLOCK_SERVER_ID {
                return Decision::Skip(Err(ServerSwitchError::Other(format!(
                    "服务器不存在: {server_id}"
                ))));
            }
            // A7：出口节点 identity 是否真变（用于切换后作废旧出口的解锁缓存）。取覆盖前的旧值比对。
            let exit_changed = selected_exit_changed(
                cfg.get("selectedServerId").and_then(Value::as_str),
                Some(server_id),
            );
            if let Some(obj) = cfg.as_object_mut() {
                obj.insert("selectedServerId".to_string(), json!(server_id));
            }
            let candidate = match serde_json::from_value::<UserConfig>(cfg.clone()) {
                Ok(candidate) => candidate,
                Err(error) => {
                    return Decision::Skip(Err(ServerSwitchError::Other(format!(
                        "配置解析失败（UserConfig）: {error}"
                    ))));
                }
            };
            // Block is emitted as reject routes, not as a selector member. A running
            // direct-mode core cannot honor a saved block selection.
            if server_id == BLOCK_SERVER_ID && candidate.proxy_mode == ProxyMode::Direct {
                return Decision::Skip(Err(ServerSwitchError::Other(
                    "直连模式下不能选择阻断".to_string(),
                )));
            }
            if let Err(message) =
                validate_candidate
                    .take()
                    .expect("server_switch 的候选校验只能执行一次")(&candidate)
            {
                return Decision::Skip(Err(ServerSwitchError::InterfaceUnavailable(message)));
            }
            // 必须在 ConfigManager 的写事务内取得 selector 所有权：若先写 D、解锁后才 bump，auto
            // rollback 可在间隙内看到“D 仍等于候选”并覆盖一次同目标的用户新意图。
            let intent_generation = register_intent
                .take()
                .expect("server_switch 的 Write 腿只能执行一次")(
            );
            if let Some(obj) = cfg.as_object_mut() {
                if server_id != DIRECT_SERVER_ID && server_id != BLOCK_SERVER_ID {
                    push_recent_server_id(obj, server_id);
                }
            }
            Decision::Write(Ok((exit_changed, intent_generation)))
        })
        .map_err(|e| ServerSwitchError::Other(format!("{e}")))?;
    let (exit_changed, intent_generation) = exit_changed?;
    let cfg = saved.expect("server_switch 的 Write 腿必须返回已落盘配置");
    Ok((cfg, exit_changed, intent_generation))
}

/// 保存选择与实际运行态分开回执；只有严格读回才报告 applied。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSwitchReceipt {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

impl ServerSwitchReceipt {
    pub(crate) fn status(&self) -> &'static str {
        self.status
    }

    fn from_outcome(outcome: Option<SwitchOutcome>) -> Self {
        let (status, reason) = match outcome {
            Some(SwitchOutcome::HotSwitched | SwitchOutcome::NoOp | SwitchOutcome::Unchanged) => {
                ("applied", None)
            }
            Some(SwitchOutcome::Pending | SwitchOutcome::Restarting) => ("pending", None),
            Some(SwitchOutcome::NotRunning) => ("notRunning", None),
            Some(SwitchOutcome::Deferred) => ("deferred", Some("nodeRequiresApply")),
            None => ("superseded", None),
        };
        Self { status, reason }
    }
}

/// 上游 `SERVER_SWITCH`：切换选中节点。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub async fn server_switch(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    server_id: String,
) -> Result<ApiResponse<ServerSwitchReceipt>, ()> {
    match server_switch_core(
        state.config(),
        &server_id,
        |candidate| {
            state
                .proxy()
                .validate_selected_server_candidate_blocking(candidate)
        },
        || state.proxy().register_selector_intent(),
    ) {
        Ok((_cfg, _exit_changed, intent_generation)) => {
            // 只发磁盘变更信号：显式选择在下方等待受限 R 投影的结果，不能再让普通广播
            // 后台把完整 D（含此前保存未 Apply 的 DNS/规则）送入运行核。
            emit_config_changed_signal(&app);
            let starting_generation = state.proxy().core_generation();
            match state
                .proxy
                .switch_selected_server_if_current(&server_id, intent_generation)
                .await
            {
                Ok(outcome) => {
                    let outcome = state.proxy().settle_selected_switch_receipt(
                        outcome,
                        starting_generation,
                        intent_generation,
                    );
                    // 保存 D 本身未改变出口；热切成功由 runtime 按 R 作废，重启就绪由
                    // start/reassert 续延作废。Pending/Deferred 时旧核仍跑旧出口，缓存仍有效。
                    Ok(ApiResponse::ok(ServerSwitchReceipt::from_outcome(outcome)))
                }
                Err(error) => Ok(ApiResponse::err(error)),
            }
        }
        Err(ServerSwitchError::InterfaceUnavailable(message)) => Ok(ApiResponse::err_with_code(
            message,
            code::OUTBOUND_INTERFACE_UNAVAILABLE,
        )),
        Err(ServerSwitchError::Other(message)) => Ok(ApiResponse::err(message)),
    }
}

/// 上游 `SERVER_GENERATE_URL`：节点 → 真实 share URL（`ProtocolParser.generateUrl` 等价）。
///
/// 反序列化 `ServerConfig` 后走 net-stack [`encode_share_url`](polaris_net_stack::share_link::encode_share_url)
/// —— 即解析器 `parse_share_url` 的**逆**（`vless://…`/`vmess://base64(json)`/`ss://…`/`trojan://…` 等，
/// round-trip 金样测试逐协议锁死）。此前返回的 `polaris://name/<uuid>` 是**假链**（无法被任何客户端导入，
/// 也无法 round-trip）——已替换为真实协议 URI。
///
/// 结构非法（缺字段/端口越界）或**无标准分享链接形态的协议**（WireGuard/Tailscale/SSH/Custom）→ err 信封，
/// 前端据此提示，不吐假链。
#[tauri::command]
pub fn server_generate_url(_state: State<'_, AppRuntime>, server: Value) -> ApiResponse<String> {
    let cfg: ServerConfig = match serde_json::from_value(server) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("节点结构非法: {e}")),
    };
    match polaris_net_stack::share_link::encode_share_url(&cfg) {
        Ok(url) => ApiResponse::ok(url),
        Err(e) => ApiResponse::err(e),
    }
}

/// `tailcat_keypair`：Tailcat 客户端密钥对（设计 D5）。`private_key` 缺省/空 ⇒ 生成新私钥；给了 ⇒ 只推导它的公钥。
/// 返回 `{privateKey, publicKey}`（标准 base64）。
///
/// 推导照 sing-box `protocol/tailscale/tailcat_key.go`：公钥 = X25519(私钥, 基点 9)；新私钥落盘前按 X25519
/// 裁剪（`clampTailcatPrivateKey`），与 `sing-box generate tailcat-keypair` 打印的私钥同形。disco 密钥由私钥
/// 派生、只在客户端内部使用，服务端 `users` 只认公钥 ⇒ 不返回。**私钥不进日志**：错误文案不回显输入。
#[tauri::command]
pub fn tailcat_keypair(private_key: Option<String>) -> ApiResponse<Value> {
    match tailcat_keypair_core(private_key.as_deref()) {
        Ok((private, public)) => {
            ApiResponse::ok(json!({ "privateKey": private, "publicKey": public }))
        }
        Err(e) => ApiResponse::err(e),
    }
}

fn tailcat_keypair_core(private_key: Option<&str>) -> Result<(String, String), String> {
    use crate::runtime::mesh::{base64_encode, generate_warp_seed};
    let private = match private_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => decode_tailcat_key(k)
            .ok_or_else(|| "私钥须为 32 字节的标准 base64（44 个字符）".to_string())?,
        None => {
            let mut k =
                generate_warp_seed().map_err(|_| "系统随机源不可用，无法生成密钥".to_string())?;
            k[0] &= 248;
            k[31] = (k[31] & 127) | 64;
            k
        }
    };
    let public = crate::runtime::x25519::x25519_base(&private);
    Ok((base64_encode(&private), base64_encode(&public)))
}

/// 标准 base64 的 32 字节 key（44 字符、恰一个 `=`）→ 字节。形态判据与 config-engine
/// `tailcat_emit_check` 相同：它拒的，这里也拒。
fn decode_tailcat_key(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 44 || b[43] != b'=' {
        return None;
    }
    let mut out = [0u8; 32];
    let (mut acc, mut bits, mut i) = (0u32, 0u32, 0usize);
    for &c in &b[..43] {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out[i] = (acc >> bits) as u8;
            i += 1;
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// 上游 `WARP_REGISTER`：注册匿名 WARP 设备 → WireGuard 草稿（mesh crate WarpService）。
///
/// 装配见 `runtime::mesh::warp_service`：注入 [`HttpRuntime`](crate::runtime::http::HttpRuntime) 的
/// `WarpHttp`（reqwest+rustls）+ ring CSPRNG 种子 + RFC 7748 X25519 公钥（`runtime::x25519`）。
///
/// ⚠️ **TLS 指纹风险 / 本机不可验（留真机）**：CF WAF 校验 TLS 指纹，rustls ClientHello ≠ okhttp
/// → 真实注册可能 1020/403（`runtime/http.rs` WarpHttp impl 文档已登记）。且真实注册有副作用（在 CF 建真
/// 设备）。本批只用 mock 验编排；真实 WARP 可用性未验。失败形态是明确的 `WARP_REGISTER_FAILED` error code
/// （body 携 CF `1020` 时不伪装成功），前端据此提示，而非静默降级。
#[tauri::command]
pub async fn warp_register(
    state: State<'_, AppRuntime>,
    license_key: Option<String>,
) -> Result<ApiResponse<Value>, ()> {
    let seed = match crate::runtime::mesh::generate_warp_seed() {
        Ok(s) => s,
        Err(e) => {
            log::error!("WARP 注册未开始：系统随机源不可用");
            return Ok(ApiResponse::err_with_code(e, "WARP_RNG_UNAVAILABLE"));
        }
    };
    let svc = crate::runtime::mesh::warp_service(state.http().clone(), seed);
    match svc.register(RegisterOptions { license_key }).await {
        Ok(draft) => match serde_json::to_value(&draft) {
            Ok(v) => Ok(ApiResponse::ok(v)),
            Err(e) => Ok(ApiResponse::err_with_code(
                {
                    log::error!("WARP 注册成功但草稿序列化失败");
                    format!("WARP 草稿序列化失败: {e}")
                },
                "WARP_DRAFT_SERIALIZE",
            )),
        },
        Err(e) => {
            log::error!("WARP 注册命令失败");
            Ok(ApiResponse::err_with_code(e, "WARP_REGISTER_FAILED"))
        }
    }
}

/// 上游 `WARP_APPLY_LICENSE`：对已注册 WARP 节点原地应用 WARP+ license（升级免重建）。
///
/// token/deviceId 服务端按 `server_id` 从 `wireguardSettings.warpDevice` 取，不经前端回传。无凭据的旧节点
/// 返 `{ok:false, error:"no-credentials"}`（真实业务结果，非 stub 假成功——对齐 上游 handler）。
/// 网络/许可失败返 `{ok:false, error}`；成功返 `{ok:true, warpPlus}`。信封恒 success（业务态在 data.ok）。
#[tauri::command]
pub async fn warp_apply_license(
    state: State<'_, AppRuntime>,
    server_id: String,
    license: String,
) -> Result<ApiResponse<Value>, ()> {
    let cfg = match state.config().load_full() {
        Ok(c) => c,
        Err(e) => return Ok(ApiResponse::err(format!("{e}"))),
    };
    // 取该节点已注册的 warpDevice 凭据（deviceId+token）；转 owned 以免借用跨 await。
    let dev = cfg
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter()
                .find(|s| s.get("id").and_then(Value::as_str) == Some(server_id.as_str()))
        })
        .and_then(|s| s.get("wireguardSettings"))
        .and_then(|w| w.get("warpDevice"));
    let device_id = dev
        .and_then(|d| d.get("deviceId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let token = dev
        .and_then(|d| d.get("token"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if device_id.is_empty() || token.is_empty() {
        log::warn!("WARP+ license 未应用：节点缺少既有设备凭据");
        return Ok(ApiResponse::ok(
            json!({ "ok": false, "error": "no-credentials" }),
        ));
    }
    // applyLicense 路径不触碰 keypair（`WarpService::apply_license` 从不调 generate_keypair）→ 占位种子。
    let svc = crate::runtime::mesh::warp_service(state.http().clone(), [0u8; 32]);
    match svc.apply_license(&device_id, &token, &license).await {
        Ok(acct) => Ok(ApiResponse::ok(
            json!({ "ok": true, "warpPlus": acct.warp_plus }),
        )),
        Err(e) => {
            log::error!("WARP+ license 命令失败");
            Ok(ApiResponse::ok(json!({ "ok": false, "error": e })))
        }
    }
}

/// Register the renderer-minted identity before save/check/subscribe can be cancelled.
/// Registration never starts a process; cancelled identities remain bounded tombstones.
#[tauri::command]
pub async fn tailscale_login_prepare(
    state: State<'_, AppRuntime>,
    server_id: String,
    attempt_id: String,
) -> Result<ApiResponse<()>, ()> {
    if polaris_mesh::tailscale_state::tailscale_state_dir(std::path::Path::new("."), &server_id)
        .is_err()
    {
        return Ok(ApiResponse::err_with_code(
            "Invalid Tailscale node identity",
            "TAILSCALE_LOGIN_BAD_SERVER",
        ));
    }
    Ok(
        match state
            .mesh()
            .prepare_tailscale_login(&server_id, &attempt_id)
            .await
        {
            Ok(()) => ok_void(),
            Err(reason) => ApiResponse::err_with_code(reason, "TAILSCALE_LOGIN_BAD_ATTEMPT"),
        },
    )
}

/// Recover the last native receipt for this exact request after the renderer resumes.
/// Absence is unknown, never evidence that a TS state directory means authorization succeeded.
#[tauri::command]
pub async fn tailscale_login_progress(
    state: State<'_, AppRuntime>,
    server_id: String,
    attempt_id: String,
) -> Result<ApiResponse<Option<crate::runtime::tailscale_login_core::LoginProgressReceipt>>, ()> {
    Ok(ApiResponse::ok(
        state
            .mesh()
            .tailscale_login_progress(&server_id, &attempt_id),
    ))
}

/// Authorize the persisted node with an explicit browser/AuthKey mode and prepared attempt identity.
/// `started` denotes process startup only; request-scoped STATUS progress confirms Running after reap.
/// A matching primary owner is queried once for fresh STATUS; a changed configuration remains pending.
/// Real control-plane authorization is outside the mock/check-only test boundary.
#[tauri::command]
pub async fn tailscale_login(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    server: Value,
    request: crate::runtime::tailscale_login_core::LoginRequest,
) -> Result<ApiResponse<Value>, ()> {
    let Ok(requested) = serde_json::from_value::<ServerConfig>(server) else {
        return Ok(ApiResponse::err_with_code(
            "Invalid Tailscale node",
            "TAILSCALE_LOGIN_BAD_SERVER",
        ));
    };
    if requested.id.is_empty()
        || requested.protocol
            != polaris_config_engine::user_config::server_config::Protocol::Tailscale
    {
        return Ok(ApiResponse::err_with_code(
            "Invalid Tailscale node",
            "TAILSCALE_LOGIN_BAD_SERVER",
        ));
    }
    // Resolve only after the registry takes the TS state gate. A renderer request may have
    // waited through an identity retirement after this command was dispatched.
    let saved_server = || -> Result<ServerConfig, String> {
        let saved = state
            .config()
            .current()
            .map_err(|_| "Cannot read the saved Tailscale node".to_string())?;
        let nodes = saved
            .get("servers")
            .and_then(Value::as_array)
            .ok_or_else(|| "Saved Tailscale nodes are unavailable".to_string())?;
        let matching: Vec<_> = nodes
            .iter()
            .filter(|node| node.get("id").and_then(Value::as_str) == Some(&requested.id))
            .collect();
        let &[node] = matching.as_slice() else {
            return Err("Saved Tailscale node is absent or duplicated".into());
        };
        let current: ServerConfig = serde_json::from_value(node.clone())
            .map_err(|_| "Saved Tailscale node is invalid".to_string())?;
        if current.protocol
            != polaris_config_engine::user_config::server_config::Protocol::Tailscale
        {
            return Err("Saved Tailscale identity changed".into());
        }
        if let Some(raw_state) = saved.get("meshRouteState") {
            use polaris_config_engine::user_config::mesh_route_state::{
                MeshBindingState, MeshRouteState,
            };
            let state: MeshRouteState = serde_json::from_value(raw_state.clone())
                .map_err(|_| "Saved mesh identity ledger is invalid".to_string())?;
            let active: Vec<_> = state
                .identities
                .iter()
                .filter(|identity| {
                    identity.server_id == requested.id
                        && matches!(
                            identity.binding_state,
                            MeshBindingState::Bound | MeshBindingState::Unbound
                        )
                })
                .collect();
            if active.len() != 1 {
                return Err("Saved Tailscale identity epoch is unavailable".into());
            }
            let authority = current
                .tailscale_settings
                .as_ref()
                .and_then(|settings| settings.control_url.as_deref())
                .filter(|url| !url.is_empty())
                .unwrap_or("https://controlplane.tailscale.com");
            let canonical = polaris_config_engine::user_config::mesh_identity_reconcile::canonical_control_authority(authority)
                .map_err(|_| "Saved Tailscale authority is invalid".to_string())?;
            if canonical != active[0].control_authority {
                return Err("Saved Tailscale identity authority changed".into());
            }
        }
        Ok(current)
    };
    let main_core = || {
        let cfg = state
            .proxy()
            .running_config_snapshot()
            .and_then(|v| serde_json::from_value::<UserConfig>(v).ok());
        crate::runtime::tailscale_login_core::MainLoginSnapshot {
            alive: state.proxy().tailscale_writer_alive(),
            generation: state.proxy().core_generation(),
            api_secret: cfg
                .as_ref()
                .and_then(|c| c.clash_api_secret.clone())
                .unwrap_or_default(),
            api_port: state.proxy().status().clash_api_port,
            http_port: cfg.as_ref().and_then(|c| c.http_port),
            mixed_port: cfg.as_ref().and_then(|c| c.mixed_port),
        }
    };
    match state
        .mesh()
        .start_tailscale_login(app, &requested, request, &saved_server, &main_core)
        .await
    {
        StartLoginOutcome::Started => Ok(ApiResponse::ok(json!({"started": true}))),
        StartLoginOutcome::InMainCore => Ok(ApiResponse::ok(
            json!({"started": false, "reason": "inMainCore"}),
        )),
        StartLoginOutcome::InMainCorePending => Ok(ApiResponse::ok(
            json!({"started": false, "reason": "inMainCore", "configurationPending": true}),
        )),
        StartLoginOutcome::Cancelled => Ok(ApiResponse::ok(
            json!({"started": false, "reason": "cancelled"}),
        )),
        StartLoginOutcome::Failed(reason) => {
            Ok(ApiResponse::err_with_code(reason, "TAILSCALE_LOGIN_FAILED"))
        }
    }
}

/// 上游 `TAILSCALE_LOGIN_CANCEL`：取消一个请求，等待其核收割后注销。
/// 幂等：取消一个不存在的登录不算错（对齐 Polaris handler 的静默 ok）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub async fn tailscale_login_cancel(
    state: State<'_, AppRuntime>,
    server_id: String,
    attempt_id: String,
) -> Result<ApiResponse<()>, ()> {
    Ok(
        match state
            .mesh()
            .cancel_tailscale_login_attempt(&server_id, &attempt_id)
            .await
        {
            Ok(()) => ok_void(),
            Err(reason) => ApiResponse::err_with_code(reason, "TAILSCALE_LOGIN_CANCEL_FAILED"),
        },
    )
}

/// Logout cancels and reaps transient writers before deleting state. A live primary owner is
/// rejected without stopping the connection. An optional unclaimed attempt survives AuthKey replacement.
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub async fn tailscale_logout(
    state: State<'_, AppRuntime>,
    server_id: String,
    keep_attempt_id: Option<String>,
) -> Result<ApiResponse<Value>, ()> {
    match state
        .mesh()
        .logout_tailscale_safely(
            &server_id,
            &|| state.proxy().tailscale_writer_alive(),
            keep_attempt_id.as_deref(),
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return Ok(ApiResponse::err_with_code(
                "Stop the main connection before signing this node out",
                "TAILSCALE_LOGOUT_MAIN_CORE",
            ))
        }
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::InvalidInput {
                "TAILSCALE_LOGOUT_INVALID_SERVER_ID"
            } else {
                "TAILSCALE_LOGOUT_FAILED"
            };
            return Ok(ApiResponse::err_with_code(
                "Cannot clear Tailscale state",
                code,
            ));
        }
    }
    // Compatibility response field: a running owner is rejected above, so success never needs restart.
    Ok(ApiResponse::ok(json!({ "runningNeedsRestart": false })))
}

/// 上游 `TAILSCALE_STATE_EXISTS`：批量查 TS 节点 state 目录存在性。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn tailscale_state_exists(
    state: State<'_, AppRuntime>,
    server_ids: Vec<String>,
) -> ApiResponse<Value> {
    let mut map = serde_json::Map::new();
    for id in server_ids {
        let Ok(path) = state.mesh().tailscale_state_dir(&id) else {
            return ApiResponse::err_with_code(
                "Cannot verify Tailscale state",
                "TAILSCALE_STATE_QUERY_FAILED",
            );
        };
        let exists = match std::fs::metadata(path) {
            Ok(metadata) => metadata.is_dir(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => {
                return ApiResponse::err_with_code(
                    "Cannot verify Tailscale state",
                    "TAILSCALE_STATE_QUERY_FAILED",
                )
            }
        };
        map.insert(id, Value::Bool(exists));
    }
    ApiResponse::ok(Value::Object(map))
}

/// 上游 `TAILSCALE_GET_STATUS`：拉各 TS 节点状态末帧（sing-box 管理 API STATUS 流缓存）。
///
/// **A3 已接线**：读 `MeshRuntime` 的 STATUS 末帧缓存（由 `runtime/proxy::spawn_tailscale_status_relay` 在核就绪后
/// 订阅 `SubscribeTailscaleStatus` 流逐帧更新），`connected` = 主核是否在运行（= 状态流是否 live）。
/// - 核在跑且已收帧 → `{connected:true, statuses:[各在册 TS 节点末帧]}`（幽灵端点已在解码层过滤）。
/// - 核在跑但未收首帧 → `{connected:true, statuses:[]}`（流 live、尚无数据，诚实）。
/// - 核未跑 / 已停（缓存已清）→ `{connected:false, statuses:[]}`（renderer 据 connected=false 灰显动态位）。
///
/// `connected` 取 `proxy().status().running`（live 真值），缓存取 `mesh()`——二者在此合成（缓存本身不含 running）。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn tailscale_get_status(state: State<'_, AppRuntime>) -> ApiResponse<Value> {
    let running = state.proxy().status().running;
    let snapshot = state.mesh().tailscale_status_snapshot(running);
    ApiResponse::ok(
        serde_json::to_value(&snapshot)
            .unwrap_or_else(|_| json!({ "connected": false, "statuses": [] })),
    )
}

/// ISO8601 当前时间（上游 `new Date().toISOString()`）。
fn current_iso() -> String {
    // 复用 stats-engine 的 created_at_to_rfc3339（无 chrono/time 依赖的 civil 算法）；旧实现把整个 epoch 秒
    // 塞进秒字段产出非法 ISO（前端 Invalid Date），改为真 epoch-millis→RFC3339。时钟异常 → 空串，不 panic。
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .and_then(polaris_stats_engine::created_at_to_rfc3339)
        .unwrap_or_default()
}

/// UUID v4 生成（无 uuid crate 依赖时用伪随机占位——满足 id 唯一性语义）。
fn new_uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("pol-{nanos:032x}")
}

#[cfg(test)]
mod tests;
