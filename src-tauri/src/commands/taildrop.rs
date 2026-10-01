//! Taildrop 收件箱命令（sing-box 1.14.0-beta.15 起）。
//!
//! # 这组命令为什么必须存在
//!
//! 核从 beta.15 起在 `Start(StartStateInitialize)` 里**无条件**建收件目录并注册收件 handler
//! （`protocol/tailscale/endpoint.go:253-263`）⇒ 只要 tailnet 授了 `cap/file-sharing`，对端发来的
//! 文件**已经在往盘上落**。没有这组命令，用户拥有的是一个看不见、也清不掉的收件箱。
//!
//! # 为什么是一次性快照而不是常驻订阅
//!
//! 收件箱面板的生命周期以分钟计；而**角标要的三个计数**（未读 / 待处理 / 接收中）本来就随
//! `SubscribeTailscaleStatus` 每帧下发（`TailscaleStatusEvent` 的 `unreadFileCount` 等），
//! 走的是已有的 STATUS relay，不需要新流。故这里只做「打开面板时读一次、操作后再读一次」，
//! 判据见 [`SingBoxApiClient::first_taildrop_inbox_snapshot`]（上游在等待信号前先发一帧）。
//!
//! # 错误一律回稳定 code，不回中文
//!
//! `error` 字段里的串会被直接显示，写中文就等于把文案钉死在 Rust 侧、绕开 i18n。故本模块的失败
//! 一律 [`ApiResponse::err_with_code`]：`error` 放**给日志看的英文诊断**，`code` 放前端查表用的
//! 稳定 token（对照表见 `ui/src/domain/taildrop.ts` 的 `TAILDROP_ERROR_KEY`）。

use polaris_singbox_grpc::{
    Endpoint, SingBoxApiClient, TaildropDownload, TaildropOutgoingFile, TaildropSendInput,
    TaildropSendOutput, TaildropSendUpdate,
};
use serde::Serialize;
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncReadExt;

use crate::commands::picked_file::{
    classify, display_of, file_name_of, open_picked_for_read, stream_into_picked,
    with_picked_gateway, PickedTarget,
};
use crate::response::{ok_void, ApiResponse};
use crate::runtime::taildrop::{
    BroadcastTaildropTaskSink, TaildropRuntime, TaildropTaskEventSink, TaildropTaskSnapshot,
    TaildropTaskStartError, MAX_TAILDROP_FILES_PER_TASK,
};
use crate::runtime::AppRuntime;

/// 核没在跑，或该节点不在运行核吃进去的那份配置里（刚加未重启 / 已删）。
const ERR_UNAVAILABLE: &str = "TAILDROP_ENDPOINT_UNAVAILABLE";
/// 连管理 API 失败（核刚起还没 bind / 端口被占）。
const ERR_API: &str = "TAILDROP_API_UNREACHABLE";
/// RPC 本身失败（核拒绝 / 超时 / 文件已不在）。
const ERR_CALL: &str = "TAILDROP_CALL_FAILED";
/// 落盘失败（目标路径不可写 / 空间不足）。
const ERR_WRITE: &str = "TAILDROP_WRITE_FAILED";
/// 待读取的本地文件打不开、读取失败或发送期间大小变化。
const ERR_READ: &str = "TAILDROP_READ_FAILED";
/// 同时发送任务已到资源上限。
const ERR_BUSY: &str = "TAILDROP_BUSY";
/// 一次选择的文件数超过有界任务快照上限。
const ERR_TOO_MANY_FILES: &str = "TAILDROP_TOO_MANY_FILES";
/// 取消时 taskId 已被有界终态缓存驱逐，或从未存在。
const ERR_TASK_NOT_FOUND: &str = "TAILDROP_TASK_NOT_FOUND";

/// 单个请求块。64 KiB 足以摊平 IPC/HTTP2 帧开销，又不会让取消/背压响应粗到以 MiB 为单位。
const SEND_CHUNK_SIZE: usize = 64 * 1024;

/// 收件箱里已落盘、等待处理的一个文件（前端 `contracts/taildrop.ts` 镜像）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaildropFile {
    pub name: String,
    pub size: i64,
    pub sender_name: String,
    /// Unix 秒。前端负责按本地时区与语言格式化 —— Rust 侧不产生任何面向用户的时间文案。
    pub modified_at: i64,
}

/// 正在接收中的一个文件。`sender_id` + `name` 是取消操作的定位键（缺一不可：
/// 两个发件人可以同时发同名文件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaildropReceiving {
    pub name: String,
    pub size: i64,
    pub received_bytes: i64,
    #[serde(rename = "senderID")]
    pub sender_id: String,
    pub sender_name: String,
}

/// 一次收件箱快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaildropInbox {
    pub files: Vec<TaildropFile>,
    pub receiving: Vec<TaildropReceiving>,
}

/// 建一条到运行核管理 API 的连接 + 解出该节点的 endpoint tag。
///
/// 两段失败分别给不同 code：**「拿不到落点」与「连不上」不是一回事** —— 前者是「现在做不了」
/// （核没跑 / 节点没进核），后者是「本该能做但连不上」，用户的下一步动作不同。
fn management_target_for(
    state: &State<'_, AppRuntime>,
    server_id: &str,
) -> Result<(u16, String, String), (String, &'static str)> {
    state
        .proxy()
        .management_target_for(server_id)
        .ok_or_else(|| {
            (
                format!("no running endpoint for server {server_id}"),
                ERR_UNAVAILABLE,
            )
        })
}

async fn connect_for(
    state: &State<'_, AppRuntime>,
    server_id: &str,
) -> Result<(SingBoxApiClient, String), (String, &'static str)> {
    let (port, secret, tag) = management_target_for(state, server_id)?;
    let client = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret)
        .await
        .map_err(|e| (format!("management api connect failed: {e}"), ERR_API))?;
    Ok((client, tag))
}

/// 读一次该节点的 Taildrop 收件箱。
///
/// 🔴 **空结果不等于「tag 正确且没有文件」**：核对未知 endpointTag 回的是一帧空收件箱而非错误。
/// 本命令用 [`crate::runtime::proxy::ProxyRuntime::management_target_for`] 解 tag，解不到就直接
/// 报 `TAILDROP_ENDPOINT_UNAVAILABLE` 而**不猜**，正是为了让「空」只剩一种含义。
#[tauri::command]
pub async fn taildrop_list(
    state: State<'_, AppRuntime>,
    server_id: String,
) -> Result<ApiResponse<TaildropInbox>, ()> {
    let (client, tag) = match connect_for(&state, &server_id).await {
        Ok(v) => v,
        Err((msg, code)) => return Ok(ApiResponse::err_with_code(msg, code)),
    };
    match client.first_taildrop_inbox_snapshot(tag).await {
        Ok(inbox) => Ok(ApiResponse::ok(TaildropInbox {
            files: inbox
                .files
                .into_iter()
                .map(|f| TaildropFile {
                    name: f.name,
                    size: f.size,
                    sender_name: f.sender_name,
                    modified_at: f.modified_at,
                })
                .collect(),
            receiving: inbox
                .receiving
                .into_iter()
                .map(|r| TaildropReceiving {
                    name: r.name,
                    size: r.size,
                    received_bytes: r.received_bytes,
                    sender_id: r.sender_id,
                    sender_name: r.sender_name,
                })
                .collect(),
        })),
        Err(e) => Ok(ApiResponse::err_with_code(
            format!("SubscribeTaildropInbox failed: {e}"),
            ERR_CALL,
        )),
    }
}

/// 把收件箱标记为已读（清未读角标）。**不删文件** —— 待处理数不变。
#[tauri::command]
pub async fn taildrop_mark_read(
    state: State<'_, AppRuntime>,
    server_id: String,
) -> Result<ApiResponse<()>, ()> {
    let (client, tag) = match connect_for(&state, &server_id).await {
        Ok(v) => v,
        Err((msg, code)) => return Ok(ApiResponse::err_with_code(msg, code)),
    };
    match client.mark_taildrop_inbox_read(tag).await {
        Ok(()) => Ok(ok_void()),
        Err(e) => Ok(ApiResponse::err_with_code(
            format!("MarkTaildropInboxRead failed: {e}"),
            ERR_CALL,
        )),
    }
}

/// 删除收件箱里的一个文件。
#[tauri::command]
pub async fn taildrop_delete(
    state: State<'_, AppRuntime>,
    server_id: String,
    name: String,
) -> Result<ApiResponse<()>, ()> {
    let (client, tag) = match connect_for(&state, &server_id).await {
        Ok(v) => v,
        Err((msg, code)) => return Ok(ApiResponse::err_with_code(msg, code)),
    };
    match client.delete_taildrop_file(tag, name).await {
        Ok(()) => Ok(ok_void()),
        Err(e) => Ok(ApiResponse::err_with_code(
            format!("DeleteTaildropFile failed: {e}"),
            ERR_CALL,
        )),
    }
}

/// 取消一个**接收中**的文件。定位键是 `sender_id` + `name` 两个一起。
#[tauri::command]
pub async fn taildrop_cancel(
    state: State<'_, AppRuntime>,
    server_id: String,
    sender_id: String,
    name: String,
) -> Result<ApiResponse<()>, ()> {
    let (client, tag) = match connect_for(&state, &server_id).await {
        Ok(v) => v,
        Err((msg, code)) => return Ok(ApiResponse::err_with_code(msg, code)),
    };
    match client.cancel_taildrop_receiving(tag, sender_id, name).await {
        Ok(()) => Ok(ok_void()),
        Err(e) => Ok(ApiResponse::err_with_code(
            format!("CancelTaildropReceiving failed: {e}"),
            ERR_CALL,
        )),
    }
}

struct SelectedFile {
    file: tokio::fs::File,
    name: String,
    size: u64,
}

type SendFailure = (String, &'static str);

/// Taildrop 诊断只保留用户已经会在任务清单里看到的文件名，不记录完整本地目录。
fn selected_file_label(path: &std::path::Path) -> String {
    path.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<selected-file>".to_owned())
}

async fn open_selected_files(
    paths: Vec<std::path::PathBuf>,
) -> Result<Vec<SelectedFile>, SendFailure> {
    let mut selected = Vec::with_capacity(paths.len());
    for path in paths {
        let label = selected_file_label(&path);
        let file = tokio::fs::File::open(&path).await.map_err(|e| {
            (
                format!("open selected file {label:?} failed: {e}"),
                ERR_READ,
            )
        })?;
        let metadata = file.metadata().await.map_err(|e| {
            (
                format!("stat selected file {label:?} failed: {e}"),
                ERR_READ,
            )
        })?;
        if !metadata.is_file() {
            return Err((
                format!("selected item {label:?} is not a regular file"),
                ERR_READ,
            ));
        }
        let name = path
            .file_name()
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| (format!("{} has no file name", path.display()), ERR_READ))?;
        if metadata.len() > i64::MAX as u64 {
            return Err((
                format!("selected file {label:?} is too large for Taildrop"),
                ERR_READ,
            ));
        }
        selected.push(SelectedFile {
            file,
            name,
            size: metadata.len(),
        });
    }
    Ok(selected)
}

/// 打开一批**选中目标**（本机路径 + SAF content URI 两种形态）供发送。
///
/// # 这条腿 2026-09-13（批 16）才接上 URI 那一支
///
/// 此前这里写的是 `p.into_path().map_err(…)` —— content URI 上恒 `Err`，于是「Android 上发件
/// 不支持」。那是「还没接」而不是平台做不到：`commands/picked_file` 批 4 就把机器造好了
/// （`open_picked_for_read` 拿 fd、`file_name_of` 取名），这条腿从没用过它。
///
/// # 两处**真实的**射程边界（不是省事，是形态冲突，逐条写清）
///
/// 1. **文件名在 SAF 目标上会退化。** Taildrop 把名字发给对端，对端按它落盘。路径侧取的是
///    真 basename；URI 侧取的是 URI 最后一节（`file_name_of` 的既定口径，它明说这个值只当展示用、
///    且刻意不做百分号解码）。`ACTION_GET_CONTENT` 的 URI 末节**常常**就是文件名，但也可能是
///    document id（`…/document/msf%3A42`）⇒ 对端可能收到一个不像文件名的名字。
///    拿真名要 `ContentResolver.query(DISPLAY_NAME)`，`tauri-plugin-fs` 不暴露它，本仓也不为此
///    自建一条 JNI 腿。**文件内容一个字节都不受影响**，降的只是名字的可读性。
/// 2. **非 regular-file 的 fd 发不了，且必须当场说。** Taildrop 协议要求**先声明每个文件的长度**
///    （`TaildropOutgoingFile.size`），而云盘类 DocumentsProvider 交回的常常是
///    `openPipeHelper` 造的**管道**：`metadata().len()` 在管道上没有意义。声明一个错的长度会让
///    对端收到一个「传完了却损坏」的文件 —— 那比拒绝发送坏得多。故这一档保留 `ERR_READ`，
///    报的是「这个目标给不出确定长度」，不是「Android 不支持发件」。
async fn open_selected_targets<R: tauri::Runtime>(
    app: &AppHandle<R>,
    picked: Vec<crate::commands::picked_file::FilePath>,
) -> Result<Vec<SelectedFile>, SendFailure> {
    // 路径形态整批走原来那条腿：桌面**一行不变**（连它跑在哪个线程上都不改）。
    if picked
        .iter()
        .all(|p| matches!(classify(p), PickedTarget::Path(_)))
    {
        let paths = picked
            .into_iter()
            .map(|p| match classify(&p) {
                PickedTarget::Path(path) => path,
                PickedTarget::Uri(_) => unreachable!("上面刚判过全是路径"),
            })
            .collect();
        return open_selected_files(paths).await;
    }

    let mut selected = Vec::with_capacity(picked.len());
    for target in picked {
        match classify(&target) {
            PickedTarget::Path(path) => {
                selected.extend(open_selected_files(vec![path]).await?);
            }
            PickedTarget::Uri(uri) => {
                // URI 支的插件 I/O 是同步阻塞的、且上游拿不到 fd 时会 panic ⇒ 经
                // `with_picked_gateway` 派线程（`picked_file` 模块文档射程自曝 5 / 6）。
                let opened = with_picked_gateway(app, Some(uri.clone()), move |gateway, picked| {
                    let picked = picked.expect("with_picked_gateway 收到的目标不会为 None");
                    open_uri_for_send(gateway, &picked)
                })
                .await
                .map_err(|e| {
                    (
                        format!("open {:?} worker failed: {e}", file_name_of(&uri)),
                        ERR_READ,
                    )
                })??;
                selected.push(opened);
            }
        }
    }
    Ok(selected)
}

/// 发件 URI 支的同步一段：经网关开只读句柄、定长度、**定发给对端的名字**。
///
/// 从 [`open_selected_targets`] 里拆出来的理由只有一个：可测。那边要真 `AppHandle`
/// （`with_picked_gateway`），这里只要一个 [`FileGateway`](crate::commands::picked_file::FileGateway) —— 于是「对端收到的名字从哪来」
/// 能拿记账网关在**生产函数本体**上驱动，而不是在一份复制出来的逻辑上。
/// 跑在 `with_picked_gateway` 派出的阻塞线程上，故 `metadata` 用同步的 `std` 版。
///
/// 名字的口径（射程边界 1，见 [`open_selected_targets`]）：**只取 [`file_name_of`]**，
/// 它取 URI 路径段里最后一个非空段、不做百分号解码；取不到（空串）落 `taildrop-file` 占位 ——
/// 空名字会让对端落一个无名文件。
fn open_uri_for_send(
    gateway: &dyn crate::commands::picked_file::FileGateway,
    uri: &crate::commands::picked_file::FilePath,
) -> Result<SelectedFile, SendFailure> {
    let label = file_name_of(uri);
    let opened = open_picked_for_read(gateway, uri).map_err(|e| {
        (
            format!("open selected document {label:?} failed: {e}"),
            ERR_READ,
        )
    })?;
    let metadata = opened.metadata().map_err(|e| {
        (
            format!("stat selected document {label:?} failed: {e}"),
            ERR_READ,
        )
    })?;
    // 射程边界 2：管道 / 非 regular file 给不出确定长度，而协议要先声明它。
    if !metadata.is_file() {
        return Err((
            format!(
                "selected document {label:?} has no fixed length (not a regular file) \
                 — Taildrop must declare each file's size up front"
            ),
            ERR_READ,
        ));
    }
    if metadata.len() > i64::MAX as u64 {
        return Err((
            format!("selected document {label:?} is too large for Taildrop"),
            ERR_READ,
        ));
    }
    // 射程边界 1：名字退化成 URI 末节。空串会让对端落一个无名文件 ⇒ 兜一个占位。
    let name = if label.is_empty() {
        "taildrop-file".to_owned()
    } else {
        label
    };
    Ok(SelectedFile {
        file: tokio::fs::File::from_std(opened),
        name,
        size: metadata.len(),
    })
}

async fn write_taildrop_input(
    input: TaildropSendInput,
    files: Vec<SelectedFile>,
) -> Result<u64, SendFailure> {
    let mut total = 0u64;
    let mut buffer = vec![0u8; SEND_CHUNK_SIZE];
    for mut selected in files {
        let mut file_bytes = 0u64;
        loop {
            let read = selected.file.read(&mut buffer).await.map_err(|e| {
                (
                    format!(
                        "read {} failed after {file_bytes} bytes: {e}",
                        selected.name
                    ),
                    ERR_READ,
                )
            })?;
            if read == 0 {
                break;
            }
            input
                .send_chunk(buffer[..read].to_vec())
                .await
                .map_err(|e| (format!("SendTaildropFiles request failed: {e}"), ERR_CALL))?;
            file_bytes = file_bytes.saturating_add(read as u64);
        }
        // Start 帧先声明了长度；发送途中被别的进程改短/改长时继续提交会让核把下一文件的边界读错。
        if file_bytes != selected.size {
            return Err((
                format!(
                    "{} changed size while sending: declared {}, read {file_bytes}",
                    selected.name, selected.size
                ),
                ERR_READ,
            ));
        }
        input
            .finish_file()
            .await
            .map_err(|e| (format!("SendTaildropFiles request failed: {e}"), ERR_CALL))?;
        total = total.saturating_add(file_bytes);
    }
    // input 在返回时 drop，关闭请求流；这是服务端完成本次 RPC 的终止信号。
    Ok(total)
}

async fn drain_taildrop_output(
    mut output: TaildropSendOutput,
    runtime: &std::sync::Weak<TaildropRuntime>,
    task_id: &str,
    sink: &BroadcastTaildropTaskSink,
) -> Result<(), SendFailure> {
    while let Some(update) = output
        .next_update()
        .await
        .map_err(|e| (format!("SendTaildropFiles response failed: {e}"), ERR_CALL))?
    {
        let Some(runtime) = runtime.upgrade() else {
            return Ok(());
        };
        match update {
            TaildropSendUpdate::Progress {
                file_index,
                sent_bytes,
                file_completed,
            } => runtime.record_progress(task_id, file_index, sent_bytes, file_completed, sink),
            TaildropSendUpdate::ReceivedBytes(bytes) => {
                runtime.record_acknowledged(task_id, bytes, sink);
            }
        }
    }
    Ok(())
}

struct ManagedTaildropSend {
    port: u16,
    secret: String,
    endpoint_tag: String,
    peer_stable_id: String,
    declarations: Vec<TaildropOutgoingFile>,
    files: Vec<SelectedFile>,
}

async fn perform_managed_send(
    request: ManagedTaildropSend,
    runtime: &std::sync::Weak<TaildropRuntime>,
    task_id: &str,
    sink: &BroadcastTaildropTaskSink,
) -> Result<(), SendFailure> {
    let client =
        SingBoxApiClient::connect(Endpoint::new("127.0.0.1", request.port), request.secret)
            .await
            .map_err(|e| (format!("management api connect failed: {e}"), ERR_API))?;
    let session = client
        .start_taildrop_send(
            request.endpoint_tag,
            request.peer_stable_id,
            request.declarations,
        )
        .await
        .map_err(|e| (format!("SendTaildropFiles failed: {e}"), ERR_CALL))?;
    if let Some(runtime) = runtime.upgrade() {
        runtime.mark_sending(task_id, sink);
    } else {
        return Ok(());
    }

    let (input, output) = session.into_parts();
    let send = write_taildrop_input(input, request.files);
    let receive = drain_taildrop_output(output, runtime, task_id, sink);
    tokio::pin!(send);
    tokio::pin!(receive);

    // 双向流必须并发驱动。任一半先失败就 drop 另一半，取消核侧 RPC；正常先完成的一半等待另一半收尾。
    tokio::select! {
        sent = &mut send => match sent {
            Ok(_) => receive.await,
            Err(error) => Err(error),
        },
        received = &mut receive => match received {
            Ok(()) => send.await.map(drop),
            Err(error) => Err(error),
        },
    }
}

async fn cancellation_requested(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    loop {
        // sender 被 AppRuntime Drop 一并释放，也等价于 owner 要求退出。
        if cancel.changed().await.is_err() || *cancel.borrow() {
            return;
        }
    }
}

async fn run_managed_send(
    request: ManagedTaildropSend,
    runtime: std::sync::Weak<TaildropRuntime>,
    task_id: String,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    sink: BroadcastTaildropTaskSink,
) {
    let transfer = perform_managed_send(request, &runtime, &task_id, &sink);
    tokio::pin!(transfer);
    let outcome = tokio::select! {
        biased;
        () = cancellation_requested(&mut cancel) => None,
        result = &mut transfer => Some(result),
    };

    let Some(runtime) = runtime.upgrade() else {
        return;
    };
    match outcome {
        None => runtime.canceled(&task_id, &sink),
        Some(Ok(())) => runtime.complete(&task_id, &sink),
        Some(Err((message, code))) => {
            log::warn!("Taildrop send task {task_id} failed [{code}]: {message}");
            runtime.fail(&task_id, code, &sink);
        }
    }
}

/// Taildrop 发件结果。`canceled=true` 仅表示用户关闭了原生文件选择框，不是失败。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaildropSendResult {
    pub canceled: bool,
    pub file_count: usize,
    /// 声明总字节（任务此刻只是已受理，不代表已经传完）。
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// 选择一个或多个本地文件，经当前 Tailscale endpoint 发给指定 peer stableID。
#[tauri::command]
pub async fn taildrop_send(
    state: State<'_, AppRuntime>,
    window: WebviewWindow,
    server_id: String,
    peer_stable_id: String,
) -> Result<ApiResponse<TaildropSendResult>, ()> {
    if peer_stable_id.trim().is_empty() {
        return Ok(ApiResponse::err_with_code(
            "Taildrop peer stable ID is empty",
            ERR_UNAVAILABLE,
        ));
    }
    if !state.taildrop().can_start() {
        return Ok(ApiResponse::err_with_code(
            "too many active Taildrop send tasks",
            ERR_BUSY,
        ));
    }

    // 先选文件、打开句柄并锁定声明长度，再连 RPC；用户在选择框里停留时不占一条管理 API 双向流。
    let lang = crate::i18n::app_lang(window.app_handle());
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_title(crate::i18n::t(
            lang,
            crate::i18n::key::NATIVE_TAILDROP_SEND_TITLE,
        ))
        .pick_files(move |paths| {
            let _ = tx.send(paths);
        });
    let Some(picked) = rx.await.ok().flatten() else {
        return Ok(ApiResponse::ok(TaildropSendResult {
            canceled: true,
            ..Default::default()
        }));
    };
    if picked.is_empty() {
        return Ok(ApiResponse::ok(TaildropSendResult {
            canceled: true,
            ..Default::default()
        }));
    }
    if picked.len() > MAX_TAILDROP_FILES_PER_TASK {
        return Ok(ApiResponse::err_with_code(
            format!(
                "too many selected files: {} (max {MAX_TAILDROP_FILES_PER_TASK})",
                picked.len()
            ),
            ERR_TOO_MANY_FILES,
        ));
    }
    let files = match open_selected_targets(window.app_handle(), picked).await {
        Ok(files) => files,
        Err((msg, code)) => return Ok(ApiResponse::err_with_code(msg, code)),
    };
    let declarations = files
        .iter()
        .map(|file| TaildropOutgoingFile {
            name: file.name.clone(),
            size: file.size as i64,
        })
        .collect();
    let file_count = files.len();
    let bytes = files
        .iter()
        .fold(0u64, |sum, file| sum.saturating_add(file.size));
    let task_files = files
        .iter()
        .map(|file| (file.name.clone(), file.size))
        .collect();

    let (port, secret, endpoint_tag) = match management_target_for(&state, &server_id) {
        Ok(v) => v,
        Err((msg, code)) => return Ok(ApiResponse::err_with_code(msg, code)),
    };
    let started = match state
        .taildrop()
        .start_task(server_id, peer_stable_id.clone(), task_files)
    {
        Ok(started) => started,
        Err(TaildropTaskStartError::Busy) => {
            return Ok(ApiResponse::err_with_code(
                "too many active Taildrop send tasks",
                ERR_BUSY,
            ));
        }
        Err(TaildropTaskStartError::TooManyFiles) => {
            return Ok(ApiResponse::err_with_code(
                format!("too many selected files (max {MAX_TAILDROP_FILES_PER_TASK})"),
                ERR_TOO_MANY_FILES,
            ));
        }
    };
    let task_id = started.snapshot.task_id.clone();
    let sink = BroadcastTaildropTaskSink::new(window.app_handle().clone());
    sink.updated(&started.snapshot);
    let runtime = std::sync::Arc::downgrade(state.taildrop());
    tauri::async_runtime::spawn(run_managed_send(
        ManagedTaildropSend {
            port,
            secret,
            endpoint_tag,
            peer_stable_id,
            declarations,
            files,
        },
        runtime,
        task_id.clone(),
        started.cancel,
        sink,
    ));

    Ok(ApiResponse::ok(TaildropSendResult {
        canceled: false,
        file_count,
        bytes,
        task_id: Some(task_id),
    }))
}

/// 拉取有界任务快照。`server_id=None` 用于主窗口重建水合全部任务。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn taildrop_tasks(
    state: State<'_, AppRuntime>,
    server_id: Option<String>,
) -> Result<ApiResponse<Vec<TaildropTaskSnapshot>>, ()> {
    Ok(ApiResponse::ok(
        state.taildrop().snapshots(server_id.as_deref()),
    ))
}

/// 取消一个在途发件任务。终态重复取消幂等返回原快照。
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri IPC command owns its deserialized payload across the call"
)]
#[tauri::command]
pub fn taildrop_task_cancel(
    state: State<'_, AppRuntime>,
    app: AppHandle,
    task_id: String,
) -> Result<ApiResponse<TaildropTaskSnapshot>, ()> {
    let sink = BroadcastTaildropTaskSink::new(app);
    Ok(match state.taildrop().cancel(&task_id, &sink) {
        Some(snapshot) => ApiResponse::ok(snapshot),
        None => ApiResponse::err_with_code(
            format!("Taildrop task not found: {task_id}"),
            ERR_TASK_NOT_FOUND,
        ),
    })
}

/// 取件：把收件箱里的一个文件写到用户选定的路径。
///
/// # 为什么先写 `.part` 再改名
///
/// 下载流**不重连**（见 [`SingBoxApiClient::download_taildrop_file`]）：中途断开会让已写出的字节
/// 成为半截文件。直接写目标路径的话，用户在文件管理器里看到的是一个大小对不上、却完全像回事的
/// 文件；写 `.part` + 成功才改名，则失败路径上目标位置**从来没有出现过**这个名字。
/// 临时文件与目标**同目录**（同卷），改名才是原子的；失败路径必删。
async fn write_stream_to(
    mut stream: TaildropDownload,
    dest: &std::path::Path,
) -> std::io::Result<u64> {
    use std::io::Write;

    let part = dest.with_extension(format!(
        "{}part",
        dest.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let mut file = std::fs::File::create(&part)?;
    let mut written = 0u64;
    let outcome = async {
        // 首帧只带 size（总字节、data 空）—— 把它当数据块写进去会在文件头多出内容。
        while let Some(chunk) = stream
            .message()
            .await
            .map_err(|e| std::io::Error::other(format!("DownloadTaildropFile stream: {e}")))?
        {
            if chunk.data.is_empty() {
                continue;
            }
            file.write_all(&chunk.data)?;
            written += chunk.data.len() as u64;
        }
        file.flush()?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    drop(file);
    match outcome {
        Ok(()) => {
            std::fs::rename(&part, dest)?;
            Ok(written)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// 取件结果。`canceled` = 用户在原生保存框里按了取消（**不是错误**）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaildropSaveResult {
    pub canceled: bool,
    /// 实际写入的目标路径（取消时缺省）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 实际写出的字节数（取消时缺省）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// 取件：选一个保存位置，把收件箱里的该文件写过去。
///
/// **保存框开在 Rust 侧**，与 `local_import_pick_file` 同一范式 —— 前端因此不需要
/// `@tauri-apps/plugin-dialog` 这个 JS 依赖（本仓 UI 至今没有它，为一个按钮引进来不划算），
/// 框的标题也就自然走 Rust 侧 i18n 表（`native.taildropSaveTitle`，五语齐备由
/// `rust-i18n-coverage.test.ts` 守）。
///
/// 默认文件名取 `name`（收件箱里的原名），用户可改。
#[tauri::command]
pub async fn taildrop_save(
    state: State<'_, AppRuntime>,
    window: WebviewWindow,
    server_id: String,
    name: String,
) -> Result<ApiResponse<TaildropSaveResult>, ()> {
    let (client, tag) = match connect_for(&state, &server_id).await {
        Ok(v) => v,
        Err((msg, code)) => return Ok(ApiResponse::err_with_code(msg, code)),
    };

    // 先问路径再开流：反过来的话，用户在保存框里犹豫的这几十秒里流一直挂着，
    // 而取消之后那条流还得额外收一次尾。
    let lang = crate::i18n::app_lang(window.app_handle());
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_title(crate::i18n::t(
            lang,
            crate::i18n::key::NATIVE_TAILDROP_SAVE_TITLE,
        ))
        .set_file_name(&name)
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    let Some(picked) = rx.await.ok().flatten() else {
        return Ok(ApiResponse::ok(TaildropSaveResult {
            canceled: true,
            ..Default::default()
        }));
    };
    let stream = match client.download_taildrop_file(tag, &name).await {
        Ok(s) => s,
        Err(e) => {
            return Ok(ApiResponse::err_with_code(
                format!("DownloadTaildropFile failed: {e}"),
                ERR_CALL,
            ))
        }
    };

    /* ── 两支落盘（2026-09-13 批 16 接上 URI 那一支）──────────────────────────────────
     *
     * 本条腿要的不是「那个文件」，而是**那个文件所在的目录**：`write_stream_to` 先写同目录
     * `.part`、`flush` 之后才 `rename` 提交。SAF 的 content URI 上既没有目录也没有 rename，
     * 此前的处置是**显式报错** ——「Android 上取件不支持」。那是「还没接」，不是平台做不到：
     * 批 4 已经把这套机器造好了（`commands/picked_file`），只是这条腿从没用过它。
     *
     * 🔴 **原子性：形态换了，而且有一条保证真的拿不回来 —— 逐条写清，不许默默换掉。**
     *
     * | 保证 | 路径支（桌面，逐字不变） | URI 支（SAF） |
     * |---|---|---|
     * | 下载中断不留半截 | ✅ `.part` 删掉，目标名从未出现 | ✅ 私有临时文件删掉，用户选的文档**一个字节没写过** |
     * | 提交是原子的 | ✅ 同卷 `rename` | ❌ **拿不回来**：`write` 中途失败会在用户选的文档上留下半截 |
     * | 失败时目标不存在 | ✅ | ❌ `ACTION_CREATE_DOCUMENT` 在交回 URI **之前**就已经把（空）文档建好了 |
     *
     * 拿不回来的是**提交那一跳的原子性**：SAF 没有 rename，`tauri-plugin-fs` 也不暴露
     * `DocumentsContract.deleteDocument` ⇒ 没有任何办法把一次半截写回滚掉。
     * 能做的是把**窗口缩到最小**：先把整份内容流式落进应用私有目录（那里有真正的文件系统），
     * **完整之后**再一次性灌进用户选的文档 —— 于是「网络中断」这个主要风险不再碰得到目标，
     * 剩下的只有「本地 fd 写到一半失败」。这不是等价替换，是一条**更小但仍非零**的窗口。
     * 这一跳失败时怎么报（错误不被吃成成功、诊断报**真灌进目标的**字节数）有判据：
     * `taildrop/tests` 用 `RecordingGateway::failing_write_after` 在第 N 字节注入失败，
     * 驱动的是 `commit_staged_into_picked` 本体。
     *
     * 临时文件落**应用私有缓存目录**而不是目标旁边：URI 上根本没有「旁边」这个位置，
     * 而私有目录在 Android 上一定可写、不需要任何权限，且失败路径上必删。
     */
    match classify(&picked) {
        // 桌面原路：与接 URI 支之前**逐字相同**（同目录 `.part` → `rename`）。
        PickedTarget::Path(dest) => match write_stream_to(stream, &dest).await {
            Ok(n) => Ok(ApiResponse::ok(TaildropSaveResult {
                canceled: false,
                path: Some(dest.to_string_lossy().into_owned()),
                bytes: Some(n),
            })),
            Err(e) => Ok(ApiResponse::err_with_code(
                format!(
                    "write selected file {:?} failed: {e}",
                    selected_file_label(&dest)
                ),
                ERR_WRITE,
            )),
        },
        PickedTarget::Uri(uri) => save_stream_to_uri(window.app_handle(), stream, uri, &name).await,
    }
}

/// 取件的 **URI 支**：先完整落到应用私有临时文件，再一次性灌进用户选中的文档。
///
/// 两段刻意分开（原子性的取舍逐字写在调用点那张表里）：
///  ① 下载 → 私有临时文件。中断即删，用户选的文档这一步**一个字节都没碰**；
///  ② 临时文件 → 文档。`stream_into_picked` 流式拷（不吞内存），经 `with_picked_gateway`
///     派到阻塞线程池 —— URI 支的插件 I/O 是同步 JNI 往返，且上游拿不到 fd 时会 panic
///     （`picked_file` 模块文档射程自曝 5 / 6）。在 async command 自己的 task 上直接跑，
///     轻则占住一个 worker 到远端收完，重则让前端那个 `await` 永不 settle。
///
/// 临时文件**两条路径上都删**：成功后删（它已经没有用了）、失败后删（不许在缓存里留下一份
/// 用户以为没保存成的文件）。删不掉只 warn —— 那不该让一次已经成功的取件报成失败。
async fn save_stream_to_uri<R: tauri::Runtime>(
    app: &AppHandle<R>,
    stream: TaildropDownload,
    uri: crate::commands::picked_file::FilePath,
    name: &str,
) -> Result<ApiResponse<TaildropSaveResult>, ()> {
    let dir = match app.path().app_cache_dir() {
        Ok(dir) => dir,
        Err(e) => {
            return Ok(ApiResponse::err_with_code(
                format!("no app cache dir for Taildrop staging: {e}"),
                ERR_WRITE,
            ))
        }
    };
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return Ok(ApiResponse::err_with_code(
            format!("create Taildrop staging dir failed: {e}"),
            ERR_WRITE,
        ));
    }
    // 文件名不进临时名（它来自对端，可能含路径分隔符 / 非法字符）—— 用进程内单调计数。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let staged = dir.join(format!(
        "taildrop-save-{}.part",
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));

    // ① 完整落地。失败 ⇒ 删掉临时文件并报错，用户选的文档一个字节没写过。
    let written = match write_stream_to(stream, &staged).await {
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&staged).await;
            return Ok(ApiResponse::err_with_code(
                format!("stage Taildrop file {name:?} failed: {e}"),
                ERR_WRITE,
            ));
        }
    };

    // ② 灌进用户选的文档（连同删临时文件、出回执，收在 `commit_staged_into_picked` 里）。
    let source = staged.clone();
    let name_owned = name.to_owned();
    match with_picked_gateway(app, Some(uri), move |gateway, picked| {
        let picked = picked.expect("with_picked_gateway 收到的目标不会为 None");
        commit_staged_into_picked(gateway, &picked, &source, written, &name_owned)
    })
    .await
    {
        Ok(response) => Ok(response),
        // 阻塞线程里炸了（上游 `unimplemented!()`）⇒ 翻成稳定错误码，命令照常回话。
        // 线程没跑完 ⇒ 临时文件那一步可能没轮到，在这里补删。
        Err(e) => {
            let _ = tokio::fs::remove_file(&staged).await;
            Ok(ApiResponse::err_with_code(
                format!("Taildrop save worker failed: {e}"),
                ERR_WRITE,
            ))
        }
    }
}

/// 取件 URI 支的第 ② 段（同步，跑在 `with_picked_gateway` 派出的阻塞线程上）：
/// 私有临时文件 → 用户选的文档，删临时文件，出回执。
///
/// 从 [`save_stream_to_uri`] 拆出来只为可测（那边要真 `AppHandle` 与真下载流）：
/// 「写到一半失败时回执报什么」这条降级要在**生产函数本体**上用注入失败的网关驱动。
///
/// 回执口径（这一跳拿不回原子性，回执不许替它圆场）：
///  · 成功：`bytes` = **灌进目标的**字节数，不是下载下来的 `staged_bytes`；
///  · 失败：报错（`ERR_WRITE`），诊断里写明**失败前已灌进目标多少字节**、共应灌多少 ——
///    目标上此刻就留着那么长的半截，没有任何接口能删掉它（SAF 无 rename、插件不暴露
///    `deleteDocument`），这个数是用户唯一能拿到的事实。
///
/// 临时文件两条路径上都删（理由见 [`save_stream_to_uri`]）。
fn commit_staged_into_picked(
    gateway: &dyn crate::commands::picked_file::FileGateway,
    uri: &crate::commands::picked_file::FilePath,
    staged: &std::path::Path,
    staged_bytes: u64,
    name: &str,
) -> ApiResponse<TaildropSaveResult> {
    let copied = std::fs::File::open(staged)
        .map_err(|error| crate::commands::picked_file::StreamIntoPickedError { written: 0, error })
        .and_then(|mut file| stream_into_picked(gateway, uri, &mut file));
    if let Err(e) = std::fs::remove_file(staged) {
        log::warn!("remove Taildrop staging file failed: {e}");
    }

    match copied {
        Ok(n) => ApiResponse::ok(TaildropSaveResult {
            canceled: false,
            path: Some(display_of(uri)),
            // 报**灌进去的**字节数，不是下载下来的。
            bytes: Some(n),
        }),
        Err(e) => ApiResponse::err_with_code(
            format!(
                "write Taildrop file {name:?} to picked document failed: wrote {} of {staged_bytes} \
                 staged bytes into the document (left partial): {}",
                e.written, e.error
            ),
            ERR_WRITE,
        ),
    }
}

#[cfg(test)]
mod tests;
