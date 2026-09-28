use std::collections::BTreeSet;
use std::str::FromStr as _;

use serde_json::{json, Value};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use super::support::{node_platform, now_iso8601, today_yyyy_mm_dd};
use crate::commands::picked_file::{
    display_of, read_picked_to_string, with_picked_gateway, write_picked, FileGateway, FilePath,
};
use crate::i18n::{key, t};
use crate::response::ApiResponse;
use crate::runtime::AppRuntime;
use polaris_store::backup::{
    build_backup_info, count_category, detect_categories, parse_backup_content, pick_categories,
    sanitize_unavailable_interface_bindings, BackupCategory, BACKUP_CATEGORIES,
    BACKUP_FILE_VERSION,
};

/// 把前端传来的类别串解析成枚举；空 / None → 全选。
/// 未知类别忽略，以兼容版本差异。
fn parse_categories(raw: Option<Vec<String>>) -> Vec<BackupCategory> {
    let picked: Vec<BackupCategory> = raw
        .unwrap_or_default()
        .iter()
        .filter_map(|s| BackupCategory::from_wire(s))
        .collect();
    if picked.is_empty() {
        BACKUP_CATEGORIES.to_vec()
    } else {
        picked
    }
}

// ── 数据备份 / 恢复 ── 上游 `backup-handlers.ts` ──

/// 弹「保存文件」框，返回用户选定的**目标**（取消 → None）。
///
/// 用**回调式** API + oneshot，而非 `blocking_save_file` —— 后者禁止在主线程调用（会死锁）；
/// 本 command 是 `async fn`，回调式是官方推荐路径。
///
/// 🔴 返回的是 `FilePath` 而不是 `PathBuf`（W-18）：Android SAF 交回的是
/// `FilePath::Url(content://…)`，改动前那句 `.and_then(|p| p.into_path().ok())` 对它恒得 `None`，
/// 于是「选好了保存位置」与「按了取消」在调用方眼里一模一样 —— 用户导出备份，什么都没发生。
/// 两种形态各自怎么读写见 `commands::picked_file`。
async fn ask_save_path(app: &AppHandle, default_name: &str) -> Option<FilePath> {
    let lang = crate::i18n::app_lang(app);
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(t(lang, key::NATIVE_BACKUP_EXPORT_TITLE))
        .set_file_name(default_name)
        .add_filter(t(lang, key::NATIVE_BACKUP_FILE_TYPE), &["polaris-backup"])
        .add_filter(t(lang, key::NATIVE_ALL_FILES), &["*"])
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    rx.await.ok().flatten()
}

/// 弹「打开文件」框，返回用户选定的**目标**（取消 → None）。理由同 [`ask_save_path`]。
async fn ask_open_path(app: &AppHandle) -> Option<FilePath> {
    let lang = crate::i18n::app_lang(app);
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(t(lang, key::NATIVE_BACKUP_IMPORT_TITLE))
        .add_filter(t(lang, key::NATIVE_BACKUP_FILE_TYPE), &["polaris-backup"])
        .add_filter(t(lang, key::NATIVE_JSON_FILE_TYPE), &["json"])
        .add_filter(t(lang, key::NATIVE_ALL_FILES), &["*"])
        .pick_file(move |p| {
            let _ = tx.send(p);
        });
    rx.await.ok().flatten()
}

/// 导出腿的**落盘段** —— 保存框回话之后的全部逻辑。
///
/// 抽出来的唯一理由是**可测**：整条命令带 `AppHandle` / `State<AppRuntime>`，本仓未在 lib 单测里
/// 引 `tauri::test`，单测一行都调不动。有了这道缝，content-URI 往返门才能把
/// `FilePath::Url("content://…")` 真的喂进导出路径，断言它**不落 `cancelled`**、字节真的写出去了；
/// 负侧同理喂 `file:` 目标，断言网关一次都没被碰过（桌面仍走 `std::fs`）。
///
/// `picked` 为 `None` **只在用户真的按了取消时出现** —— 那正是本批修掉的那条歧义。
fn finish_export(gateway: &dyn FileGateway, picked: Option<FilePath>, body: &str) -> Value {
    let Some(target) = picked else {
        return backup_failure("cancelled");
    };
    if let Err(e) = write_picked(gateway, &target, body.as_bytes()) {
        log::warn!("[backup] export write failed: {e}");
        return backup_failure("writeFailed");
    }
    json!({
        "success": true,
        "filePath": display_of(&target),
    })
}

/// 导入腿**读源文件**那一段的结果。
#[derive(Debug, PartialEq, Eq)]
enum ImportSource {
    /// 用户按了取消。**只有这一种情形**才是取消（改动前 content URI 也会落到这里）。
    Cancelled,
    /// 选中了，但读不出来。
    ReadFailed,
    /// 读到了正文，外加回给前端的目标标识（`backup_import_apply` 拿它二次打开同一个文档）。
    Loaded { raw: String, file_path: String },
}

/// 导入腿的**读取段**。抽出来的理由与 [`finish_export`] 相同。
fn read_import_source(gateway: &dyn FileGateway, picked: Option<FilePath>) -> ImportSource {
    let Some(target) = picked else {
        return ImportSource::Cancelled;
    };
    match read_picked_to_string(gateway, &target) {
        Ok(raw) => ImportSource::Loaded {
            raw,
            file_path: display_of(&target),
        },
        Err(e) => {
            log::warn!("[backup] import read failed: {e}");
            ImportSource::ReadFailed
        }
    }
}

/// 导入腿的**二次打开段**：`backup_import_apply` 拿前端回传的那个字符串重新打开同一个文档。
///
/// 抽成缝的理由与 [`finish_export`] / [`read_import_source`] 相同，但它多守一件本批别处守不到
/// 的事：**`display_of` → 前端 → `FilePath::from_str` 这条往返**。它是五条腿里唯一真正执行
/// 整类替换的那条 —— pick / 预览成功而 apply 失败，用户看到的是「选好了、类目也列出来了、
/// 按恢复没反应」。第一版把这条腿留在命令体里、一条判据都没有：实测把入参改成
/// `file_path.trim_start_matches("content://")` 全仓门**照绿**，而真机上每一次恢复都会 100% 失败
/// （剥掉 scheme 后 `Url::parse` 失败 ⇒ 落 `Path` ⇒ `std::fs::read_to_string` 一个相对路径 ⇒ ENOENT）。
///
/// 入参刻意是 **`&str` 而不是已解析的 [`FilePath`]**：解析本身就是判据的一部分，缝外先解析
/// 一次只是为了决定派不派线程（见 [`with_picked_gateway`]）。
///
/// 这道缝同时钉住桌面侧唯一没被断言过的分流点：`FilePath::from_str` 把**单字母 scheme** 留给
/// Windows 盘符（`C:\…` ⇒ `Path`，`tauri-plugin-fs-2.5.1/src/file_path.rs:189-196`），
/// Unix 绝对路径则因 `Url::parse` 直接失败而落 `Path`。
fn read_apply_source(gateway: &dyn FileGateway, file_path: &str) -> Result<String, ()> {
    let source = FilePath::from_str(file_path).expect("FilePath::from_str 是 Infallible");
    read_picked_to_string(gateway, &source).map_err(|e| {
        log::warn!("[backup] import apply read failed: {e}");
    })
}

/// 上游 `BACKUP_EXPORT`：选择性导出（按 categories）。
///
/// `categories` 缺省 / 空 → 全 8 类。1.2 新增独立 DNS 规则类别，仍兼容导入 1.0/1.1 / 裸配置。
/// **clashApiSecret / privacyPassword 恒不入备份**（由 `pick_categories` 的排除表保证，见 store::backup）。
#[tauri::command]
pub async fn backup_export(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    categories: Option<Vec<String>>,
) -> Result<ApiResponse<Value>, ()> {
    let config = match state.config().load_full() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[backup] export failed to load config: {e}");
            return Ok(ApiResponse::ok(backup_failure("configLoadFailed")));
        }
    };
    let selected = parse_categories(categories);
    let picked = pick_categories(&config, &selected);

    let backup = json!({
        "version": BACKUP_FILE_VERSION,
        "appVersion": app.package_info().version.to_string(),
        "platform": node_platform(),
        "exportedAt": now_iso8601(),
        "config": picked,
    });

    let default_name = format!("polaris-backup-{}.polaris-backup", today_yyyy_mm_dd());
    let target = ask_save_path(&app, &default_name).await;
    // 序列化仍在选完之后：用户按了取消就不必白跑一趟 pretty-print（与改动前次序一致）。
    // 但 `target` 为 None **现在只意味着真取消**，不再兼任「content URI 转不出路径」。
    if target.is_none() {
        return Ok(ApiResponse::ok(backup_failure("cancelled")));
    }
    let body = match serde_json::to_string_pretty(&backup) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[backup] export serialization failed: {e}");
            return Ok(ApiResponse::ok(backup_failure("serializeFailed")));
        }
    };
    // URI 支的插件 I/O 是同步阻塞的、且上游拿不到 fd 时会 panic ⇒ 经 `with_picked_gateway`
    // 派线程（桌面那一支仍原地跑）。`JoinError` 不许 `unwrap()`：那会把 panic 变回一个永不
    // settle 的 promise，`BackupPage.tsx` 的 `setBusy(false)` 写在 `finally` 里，一起冻住。
    let payload = with_picked_gateway(&app, target, move |gateway, target| {
        finish_export(gateway, target, &body)
    })
    .await
    .unwrap_or_else(|e| {
        log::warn!("[backup] export task join failed: {e}");
        backup_failure("writeFailed")
    });
    Ok(ApiResponse::ok(payload))
}

/// Android only knows currently available physical Networks, not the full interface inventory.
/// Preserve imported explicit exits when a cellular Network is dormant; native binding fails closed.
async fn import_interface_names() -> Option<BTreeSet<String>> {
    import_interface_names_from(
        polaris_helper_proto::Platform::current(),
        crate::commands::system::list_network_interfaces(),
    )
    .await
}

async fn import_interface_names_from(
    platform: polaris_helper_proto::Platform,
    enumerate: impl std::future::Future<
        Output = Result<Vec<crate::commands::system::NetworkInterfaceInfo>, String>,
    >,
) -> Option<BTreeSet<String>> {
    if platform == polaris_helper_proto::Platform::Android {
        return None;
    }
    match enumerate.await {
        Ok(rows) => Some(rows.into_iter().map(|row| row.name).collect()),
        Err(_) => {
            log::warn!("[backup] interface observation failed; preserving imported bindings");
            None
        }
    }
}

/// 上游 `BACKUP_IMPORT_PICK`：弹文件框 + 解析 → 返回含哪些类 + 各类数量（**不 apply**）。
#[tauri::command]
pub async fn backup_import_pick(app: AppHandle) -> Result<ApiResponse<Value>, ()> {
    let picked = ask_open_path(&app).await;
    // 同 `backup_export`：URI 支派线程 + `JoinError` 收成 readFailed，不留永不 settle 的 promise。
    let source = with_picked_gateway(&app, picked, read_import_source)
        .await
        .unwrap_or_else(|e| {
            log::warn!("[backup] import pick task join failed: {e}");
            ImportSource::ReadFailed
        });
    let (raw, file_path) = match source {
        ImportSource::Cancelled => return Ok(ApiResponse::ok(json!({ "canceled": true }))),
        ImportSource::ReadFailed => {
            return Ok(ApiResponse::ok(
                json!({ "canceled": false, "errorCode": "readFailed" }),
            ))
        }
        ImportSource::Loaded { raw, file_path } => (raw, file_path),
    };
    let parsed = match parse_backup_content(&raw) {
        Ok(p) => p,
        Err(code) => {
            log::warn!("[backup] import preview parse rejected: {code}");
            return Ok(ApiResponse::ok(
                json!({ "canceled": false, "errorCode": "invalidFormat" }),
            ));
        }
    };
    let available = detect_categories(&parsed.config);
    let interface_names = import_interface_names().await;
    let mut counts = serde_json::Map::new();
    let mut unavailable_interface_bindings = serde_json::Map::new();
    for cat in &available {
        counts.insert(
            cat.as_str().to_string(),
            json!(count_category(&parsed.config, *cat)),
        );
        let mut preview = parsed.config.clone();
        let missing = interface_names.as_ref().map_or(0, |names| {
            sanitize_unavailable_interface_bindings(&mut preview, names, std::slice::from_ref(cat))
        });
        if missing > 0 {
            unavailable_interface_bindings.insert(cat.as_str().to_string(), json!(missing));
        }
    }
    Ok(ApiResponse::ok(json!({
        "canceled": false,
        // 桌面上这仍是路径字符串（逐字不变）；Android 上是 content URI ——
        // 它是那个文档唯一能被二次打开的句柄，`backup_import_apply` 拿的就是它。
        "filePath": file_path,
        "available": available,
        "counts": counts,
        "unavailableInterfaceBindings": unavailable_interface_bindings,
    })))
}

/// 上游 `BACKUP_IMPORT_APPLY`：按所选类**整类替换 + 空跳过** + 跨平台 sanitize + 保存。
///
/// 失效 `selectedServerId` 已在 `merge_categories` 末尾归零（`validate_config` 对失效引用是 Err、非归零，
/// 不兜底会令整份导入失败）。保存走 `save_full`（内部再跑 sanitize + validate）。
///
/// 存盘成功后必须走 `broadcast_config_changed`：那是本仓配置变更的唯一汇流点（前端 store 对账 +
/// `switch_mode` 热切换/重启判定 + `set_level` 跟随 logLevel）。本命令的落盘腿
/// （[`crate::commands::config::backup_import_save_core`]）不含广播 → 少了这一步，导入的备份只落磁盘、
/// 运行核与前端一无所知（一份含 logLevel/节点变更的备份导入后静默不生效）。
#[tauri::command]
pub async fn backup_import_apply(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    file_path: String,
    categories: Vec<String>,
) -> Result<ApiResponse<Value>, ()> {
    let selected: Vec<BackupCategory> = categories
        .iter()
        .filter_map(|s| BackupCategory::from_wire(s))
        .collect();
    if file_path.is_empty() || selected.is_empty() {
        return Ok(ApiResponse::ok(backup_failure("invalidArgs")));
    }
    // 前端回传的是 `backup_import_pick` 给出的那个字符串。分流与判据都住在 [`read_apply_source`]；
    // 这里先解析一次，只为让 `with_picked_gateway` 知道该不该把它派到阻塞线程池。
    let target = FilePath::from_str(&file_path).expect("FilePath::from_str 是 Infallible");
    let raw = match with_picked_gateway(&app, Some(target), move |gateway, _| {
        read_apply_source(gateway, &file_path)
    })
    .await
    {
        Ok(Ok(raw)) => raw,
        Ok(Err(())) => return Ok(ApiResponse::ok(backup_failure("readFailed"))),
        Err(e) => {
            log::warn!("[backup] import apply task join failed: {e}");
            return Ok(ApiResponse::ok(backup_failure("readFailed")));
        }
    };
    let parsed = match parse_backup_content(&raw) {
        Ok(p) => p,
        Err(code) => {
            log::warn!("[backup] import apply parse rejected: {code}");
            return Ok(ApiResponse::ok(backup_failure("invalidFormat")));
        }
    };

    // 保留 configLoadFailed 语义；事务内会以最新盘值再次读取并按类别合并。
    if let Err(e) = state.config().load_full() {
        log::warn!("[backup] import failed to load config: {e}");
        return Ok(ApiResponse::ok(backup_failure("configLoadFailed")));
    }

    // 网卡名属于设备本地资源。桌面端用实际库存检查导入绑定；Android
    // 只有当前可绑定的 Network 列表，不能以其缺席清空用户指定的接口。
    // 具体清理在 backup_import_save_core 的事务内、按真正导入类别执行。
    let interface_names = import_interface_names().await;

    // 合并、清洗及落盘前的三条策略收口在 [`config::backup_import_save_core`] 的同一事务中：
    // 回填隐私 hash（备份导出侧脱敏，不回填 = 导入即拆锁）、以本机磁盘回正后端权威字段（外机 MRU /
    // geo 元数据不得灌进本机）、全局 UA 变更时作废受影响订阅的条件 GET 验证器（不清 = 换 UA 后恒 304）。
    let platform = node_platform();
    let saved = match crate::commands::config::backup_import_save_core(
        state.config(),
        &parsed.config,
        &selected,
        parsed.platform.as_deref(),
        platform,
        interface_names.as_ref(),
    ) {
        Ok(saved) => saved,
        Err(e) => {
            log::warn!("[backup] import save failed: {e}");
            return Ok(ApiResponse::ok(backup_failure("saveFailed")));
        }
    };
    if saved.cross_platform_disabled_rules > 0 {
        log::info!(
            "[backup] 跨平台导入（{:?}→{}）：禁用 {} 条进程规则（保留供重映射）",
            parsed.platform,
            platform,
            saved.cross_platform_disabled_rules
        );
    }
    if saved.unavailable_interface_bindings > 0 {
        log::warn!(
            "[backup] 导入配置引用了本机不存在的网卡：已将 {} 处绑定回退为自动/继承",
            saved.unavailable_interface_bindings
        );
    }
    // 恢复后二次 load_full 重走完整迁移链（migrate_all）再广播：备份可能来自旧版本（上游/旧 Polaris），含旧 shape
    // 字段（legacy DomainRule / subscriptionUpdateViaProxy / 遗留 tunConfig.stack 等）。`save_full` 只 sanitize+validate、
    // **不跑迁移链**，直接广播已保存配置会让旧 shape 未迁移即入核/下发前端。二次 load_full 触发 migrate_all，
    // 广播迁移后配置。load 异常（刚存的合法配置几乎不可能）→ 回落广播保存结果（仍带回填后的私密字段）。
    // 广播**回填后**（saved.config / 其迁移形）配置；导出侧脱敏的配置不可直接入核。
    let broadcast_cfg = state.config().load_full().unwrap_or(saved.config);
    crate::commands::config::broadcast_config_changed(&app, &broadcast_cfg);
    crate::commands::config::invalidate_unlock_on_exit_change(
        state.unlock(),
        &crate::runtime::unlock::BroadcastSink::new(&app),
        state.proxy().status().running,
        saved.old_selected.as_deref(),
        broadcast_cfg
            .get("selectedServerId")
            .and_then(Value::as_str),
    );

    let info = build_backup_info(&broadcast_cfg, saved.cross_platform_disabled_rules);
    let skipped: Vec<&str> = saved.skipped.iter().map(|c| c.as_str()).collect();
    let mut out = json!({ "success": true, "info": info });
    if saved.unavailable_interface_bindings > 0 {
        out["unavailableInterfaceBindings"] = json!(saved.unavailable_interface_bindings);
    }
    if !skipped.is_empty() {
        out["skipped"] = json!(skipped);
    }
    Ok(ApiResponse::ok(out))
}

/// 备份失败的用户面只带稳定码。原始 OS/路径错误只记日志，不能越过 IPC 变成跨语种 UI 文案。
fn backup_failure(code: &str) -> Value {
    json!({ "success": false, "errorCode": code })
}

/// 上游 `BACKUP_GET_INFO`：当前配置摘要。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn backup_get_info(state: State<'_, AppRuntime>) -> ApiResponse<Value> {
    match state.config().current() {
        Ok(c) => ApiResponse::ok(json!(build_backup_info(&c, 0))),
        Err(e) => ApiResponse::err(format!("{e}")),
    }
}

#[cfg(test)]
mod tests;
