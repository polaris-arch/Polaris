use super::selected_file_label;
use std::path::Path;

#[test]
fn taildrop_diagnostics_do_not_include_parent_directories() {
    assert_eq!(
        selected_file_label(Path::new("/Users/alice/private/report.txt")),
        "report.txt"
    );
    assert_eq!(selected_file_label(Path::new("/")), "<selected-file>");
}

/* ══════════════ 取件 URI 支：提交那一跳写到一半失败时，回执报的是什么 ══════════════
 *
 * `taildrop_save` 的 URI 支拿不回「提交是原子的」（SAF 无 rename、插件不暴露 deleteDocument，
 * 见 `taildrop.rs` 调用点那张表）。能兑现的只剩两件事：错误不被吃成成功；回执/诊断里报的是
 * **真灌进目标的**字节数，而不是下载下来的字节数。下面驱动的是生产函数
 * `commit_staged_into_picked` 本体（`save_stream_to_uri` 经 `with_picked_gateway` 调它，
 * 接线由末尾的源码级判据钉住），失败由 `RecordingGateway::failing_write_after` 在第 N 字节注入。
 *
 * 射程：替身是本机普通文件；真机 SAF fd 在什么条件下写失败（provider 被杀 / 配额 / 管道对端
 * 关闭）一条都没验，归真机。本组证明的是「失败发生时本仓这一侧怎么报」。
 */

use super::{commit_staged_into_picked, open_selected_files, open_uri_for_send, ERR_WRITE};
use crate::commands::guard_scan::top_level_fn_body;
use crate::commands::picked_file::tests::{saf_file_path, RecordingGateway, SAF_URI};
use crate::commands::picked_file::{file_name_of, FilePath};
use crate::test_support::{crate_code, TestDir};
use std::str::FromStr as _;

/// 跨过 64 KiB 拷贝缓冲的边界，失败点才可能落在块中间。
fn staged_payload(dir: &TestDir) -> (std::path::PathBuf, Vec<u8>) {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let staged = dir.path().join("taildrop-save-0.part");
    std::fs::write(&staged, &payload).expect("预置临时文件");
    (staged, payload)
}

#[test]
fn a_commit_that_fails_midway_reports_the_bytes_that_really_reached_the_document() {
    // 0：一个字节都没进；5：首块中间；65_543：跨过首个 64 KiB 块；199_999：只差最后一个字节。
    for fail_at in [0u64, 5, 65_536 + 7, 199_999] {
        let dir = TestDir::new("polaris-taildrop-commit-fail");
        let (staged, payload) = staged_payload(&dir);
        let total = payload.len() as u64;
        let gateway = RecordingGateway::failing_write_after(dir.path().join("saf-doc"), fail_at);

        let resp =
            commit_staged_into_picked(&gateway, &saf_file_path(), &staged, total, "report.pdf");

        // 前提：注入真的打上了 —— 目标上独立量出来的长度恰是注入点，且短于应灌总量。
        let landed = gateway.backing_len().expect("替身文件必须已被打开并截断过");
        assert_eq!(landed, fail_at, "注入点没打上，本条测的不是半截写");
        assert!(landed < total);

        // ② 错误向上可见，不被吃成成功。
        assert!(!resp.success, "fail_at={fail_at}：半截写不许报成功");
        assert!(resp.data.is_none(), "失败时不许带一份成功回执");
        assert_eq!(resp.code.as_deref(), Some(ERR_WRITE));

        // ① 报的字节数 == 目标上真实留下的字节数（而不是下载下来的 total）。
        let error = resp.error.expect("失败必须带诊断");
        assert!(
            error.contains(&format!("wrote {landed} of {total} staged bytes")),
            "fail_at={fail_at}：诊断必须报目标上真实留下的 {landed} 字节，实得：{error}"
        );

        // ③ 本地临时源按既有约定：失败路径也删。
        assert!(!staged.exists(), "失败路径上临时文件必须删掉");
        assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);
    }
}

#[test]
fn a_commit_that_succeeds_reports_the_bytes_that_reached_the_document() {
    // 正向对照：同一个函数在不注入失败时报成功，且回执字节数 == 目标长度 == 内容逐字相等。
    let dir = TestDir::new("polaris-taildrop-commit-ok");
    let (staged, payload) = staged_payload(&dir);
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));

    // 回执来源探针：生产上 `staged_bytes` 与临时文件长度相等，于是「报下载字节数」与「报灌入
    // 字节数」在成功路径上同值、测不出来。这里故意把 `staged_bytes` 报多一截，两个来源才分得开。
    let claimed = payload.len() as u64 + 1000;
    let resp =
        commit_staged_into_picked(&gateway, &saf_file_path(), &staged, claimed, "report.pdf");

    assert!(resp.success, "未注入失败时必须成功：{:?}", resp.error);
    let data = resp.data.expect("成功回执");
    assert!(!data.canceled);
    assert_eq!(data.path.as_deref(), Some(SAF_URI));
    assert_eq!(data.bytes, gateway.backing_len());
    assert_eq!(data.bytes, Some(payload.len() as u64));
    assert_eq!(
        std::fs::read(dir.path().join("saf-doc")).unwrap(),
        payload,
        "字节必须逐字落进目标"
    );
    assert!(!staged.exists(), "成功路径上临时文件同样要删");
}

/* ══════════════ 发件 URI 支：对端收到的名字从哪来 ══════════════
 *
 * 名字只许来自 `file_name_of`（取 URI 路径段里最后一个**非空**段、不解码），空串落 `taildrop-file`。
 * 用例刻意挑「朴素取 URI 原文末节」会给出不同答案的形态（带 query、尾斜杠），好让名字来源被换掉时变红。
 *
 * 射程：真机 SAF 交回的 URI 末节长什么样（document id 还是文件名）取决于 provider，归真机；
 * 本组钉住的是「本仓这一侧拿哪个值当名字」。
 */

fn seeded_gateway(dir: &TestDir) -> RecordingGateway {
    let gateway = RecordingGateway::new(dir.path().join("picked-doc"));
    gateway.seed("abc");
    gateway
}

#[test]
fn the_name_sent_to_the_peer_comes_from_file_name_of() {
    let cases = [
        (
            "content://com.android.providers.downloads.documents/document/report.pdf",
            "report.pdf",
        ),
        // 退化形态：百分号编码的 document id 原样发出去（不解码，见 `file_name_of` 文档）。
        (
            "content://com.android.externalstorage.documents/document/primary%3ADownload%2Freport.pdf",
            "primary%3ADownload%2Freport.pdf",
        ),
        // query 不是名字的一部分 —— 朴素取原文末节会得到 `report.pdf?token=1`。
        (
            "content://com.example.provider/document/report.pdf?token=1",
            "report.pdf",
        ),
        // 尾斜杠：朴素取原文末节得空串（进而落占位），`file_name_of` 跳过空段。
        ("content://com.example.provider/tree/photos/", "photos"),
    ];
    for (raw, expected) in cases {
        let dir = TestDir::new("polaris-taildrop-send-name");
        let gateway = seeded_gateway(&dir);
        let uri = FilePath::from_str(raw).expect("FilePath::from_str 是 Infallible");
        assert!(matches!(uri, FilePath::Url(_)), "{raw} 必须落在 Url 变体上");

        let selected = open_uri_for_send(&gateway, &uri)
            .unwrap_or_else(|(msg, code)| panic!("{raw} 必须开得出来：[{code}] {msg}"));

        assert_eq!(selected.name, expected, "{raw}：对端收到的名字");
        assert_eq!(
            selected.name,
            file_name_of(&uri),
            "名字必须就是 file_name_of 的值"
        );
        assert_eq!(selected.size, 3, "声明长度必须是目标真实长度");
        assert_eq!(gateway.calls(), vec![uri.to_string()], "必须经网关打开");
    }
}

#[test]
fn an_empty_file_name_of_falls_back_to_the_placeholder() {
    let dir = TestDir::new("polaris-taildrop-send-placeholder");
    let gateway = seeded_gateway(&dir);
    let uri = FilePath::from_str("content://com.example.provider/").expect("Infallible");
    assert_eq!(
        file_name_of(&uri),
        "",
        "本条的前提：file_name_of 取不到名字"
    );

    let selected = open_uri_for_send(&gateway, &uri).expect("开得出来");
    assert_eq!(
        selected.name, "taildrop-file",
        "空名字必须落占位，不许发一个无名文件"
    );
}

#[tokio::test]
async fn a_local_path_sends_its_real_basename() {
    // 路径支（桌面，逐字未改）：名字取真 basename，与 `file_name_of` 的路径臂同值。
    let dir = TestDir::new("polaris-taildrop-send-path");
    let path = dir.path().join("report.txt");
    std::fs::write(&path, "abc").unwrap();

    let selected = open_selected_files(vec![path.clone()])
        .await
        .unwrap_or_else(|(msg, code)| panic!("[{code}] {msg}"));

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "report.txt");
    assert_eq!(selected[0].name, file_name_of(&FilePath::Path(path)));
    assert_eq!(selected[0].size, 3);
}

/* ══════════════ 接线：上面驱动的两个同步函数就是命令真正走的那两段 ══════════════ */

#[test]
fn the_uri_legs_are_wired_through_the_tested_functions() {
    let code = crate_code("commands/taildrop.rs");

    let send = top_level_fn_body(&code, "async fn open_selected_targets<R: tauri::Runtime>(");
    assert!(
        send.contains("open_uri_for_send(gateway, &picked)"),
        "发件 URI 支必须经 open_uri_for_send 开文件定名字"
    );
    assert!(
        !send.contains("SelectedFile {"),
        "发件 URI 支不许在别处另拼一个 SelectedFile（名字会绕开被测函数）"
    );
    // 被测函数交回的 SelectedFile 必须原样入列：命令体里一处 `.name` 都不许碰
    // （变异实测：只查上两条时，调用之后改写 `s.name` 照样全绿）。
    assert!(
        send.contains("selected.push(opened);"),
        "open_uri_for_send 的结果必须原样入列"
    );
    assert!(
        !send.contains(".name"),
        "发件命令体不许改写名字 —— 名字只许由 open_uri_for_send 定"
    );

    let save = top_level_fn_body(&code, "async fn save_stream_to_uri<R: tauri::Runtime>(");
    assert!(
        save.contains("commit_staged_into_picked(gateway, &picked, &source, written, &name_owned)"),
        "取件 URI 支的提交那一跳必须经 commit_staged_into_picked"
    );
    assert!(
        !save.contains("TaildropSaveResult {"),
        "取件 URI 支的成功回执只许由 commit_staged_into_picked 出"
    );
    // 切点自检：取到的是生产函数体本身，不是空片或别处。
    assert!(
        save.contains("app_cache_dir()"),
        "save 切片没取到生产函数体"
    );
    assert!(
        send.contains("open_selected_files("),
        "send 切片没取到生产函数体"
    );

    let command = top_level_fn_body(&code, "pub async fn taildrop_send(");
    assert!(
        command.contains("name: file.name.clone(),"),
        "声明给对端的名字必须就是 SelectedFile.name"
    );
}

#[test]
fn concurrent_staging_owns_unique_files_and_never_touches_a_part_neighbor() {
    use super::TaildropStagedFile;
    use std::io::Write as _;
    let dir = TestDir::new("polaris-taildrop-exclusive-");
    let destination = dir.path().join("report.txt");
    let neighbor = dir.path().join("report.txt.part");
    std::fs::write(&neighbor, b"unrelated document").unwrap();
    let mut first = TaildropStagedFile::create(&destination).unwrap();
    let mut second = TaildropStagedFile::create(&destination).unwrap();
    assert_ne!(first.path, second.path);
    first.file.as_mut().unwrap().write_all(b"first").unwrap();
    first.publish(&destination).unwrap();
    second.file.as_mut().unwrap().write_all(b"second").unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"first");
    let abandoned = second.path.clone();
    drop(second);
    assert!(!abandoned.exists());
    assert_eq!(std::fs::read(&neighbor).unwrap(), b"unrelated document");
    assert_eq!(std::fs::read(&destination).unwrap(), b"first");
}

#[test]
fn failed_rename_and_dropped_unfinished_transfer_clean_only_owned_staging() {
    use super::TaildropStagedFile;
    let dir = TestDir::new("polaris-taildrop-rename-failure-");
    let destination = dir.path().join("nonempty-directory");
    std::fs::create_dir(&destination).unwrap();
    let keep = destination.join("keep");
    std::fs::write(&keep, b"keep").unwrap();
    let staged = TaildropStagedFile::create(&destination).unwrap();
    let path = staged.path.clone();
    assert!(staged.publish(&destination).is_err());
    assert!(!path.exists());
    assert_eq!(std::fs::read(&keep).unwrap(), b"keep");
    let staged = TaildropStagedFile::create(&dir.path().join("unfinished")).unwrap();
    let path = staged.path.clone();
    // Dropping a transfer future drops the same owner, including read/write error paths.
    let never_polled = async move {
        let _owned = staged;
        std::future::pending::<()>().await;
    };
    drop(never_polled);
    assert!(!path.exists());
}

#[test]
fn streamed_path_saves_use_exclusive_raii_staging() {
    let code = crate_code("commands/taildrop.rs");
    let body = top_level_fn_body(&code, "async fn write_stream_to(");
    assert!(body.contains("TaildropStagedFile::create(dest)"));
    assert!(body.contains("staged.publish(dest)"));
    assert!(!body.contains("with_extension"));
    assert!(code.contains(".create_new(true)"));
}

#[tokio::test]
async fn cancellation_watch_joins_while_opening_and_on_owner_drop() {
    for owner_drop in [false, true] {
        let (sender, mut receiver) = tokio::sync::watch::channel(false);
        let mut waiter = tokio::spawn(async move {
            super::cancellation_requested(&mut receiver).await;
        });
        sender.send(false).unwrap();
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "false updates do not cancel an opening"
        );
        if owner_drop {
            drop(sender);
        } else {
            sender.send(true).unwrap();
        }
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), &mut waiter).await;
        if joined.is_err() {
            waiter.abort();
            let _ = waiter.await;
        }
        joined.expect("opening cancellation must join").unwrap();
    }
    let (_sender, mut receiver) = tokio::sync::watch::channel(true);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::cancellation_requested(&mut receiver),
    )
    .await
    .expect("a cancellation predating opening is immediately visible");
}

#[test]
fn opening_cancellation_encloses_transfer_and_preserves_real_file_boundaries() {
    let code = crate_code("commands/taildrop.rs");
    let opening = top_level_fn_body(&code, "async fn perform_managed_send(");
    let header = opening.find(".start_taildrop_send(").unwrap();
    let sending = opening.find("runtime.mark_sending(task_id, sink)").unwrap();
    let split = opening.find("session.into_parts()").unwrap();
    assert!(header < sending && sending < split);
    assert!(opening.contains("let send = write_taildrop_input(input, request.files);"));
    assert!(
        opening.contains("let receive = drain_taildrop_output(output, runtime, task_id, sink);")
    );
    assert!(opening.contains("tokio::select!"));
    assert!(opening.contains("Ok(_) => receive.await"));
    assert!(opening.contains("Ok(()) => send.await.map(drop)"));
    assert_eq!(opening.matches("Err(error) => Err(error)").count(), 2);

    let managed = top_level_fn_body(&code, "async fn run_managed_send(");
    assert!(managed
        .contains("let transfer = perform_managed_send(request, &runtime, &task_id, &sink);"));
    assert!(managed.contains("tokio::pin!(transfer)"));
    let biased = managed.find("biased;").unwrap();
    let cancel = managed
        .find("() = cancellation_requested(&mut cancel) => None")
        .unwrap();
    let transfer = managed
        .find("result = &mut transfer => Some(result)")
        .unwrap();
    assert!(biased < cancel && cancel < transfer);
    assert!(managed.contains("None => runtime.canceled(&task_id, &sink)"));

    let writer = top_level_fn_body(&code, "async fn write_taildrop_input(");
    assert!(code.contains("const SEND_CHUNK_SIZE: usize = 64 * 1024;"));
    assert!(writer.contains("vec![0u8; SEND_CHUNK_SIZE]"));
    assert!(writer.contains(".send_chunk(buffer[..read].to_vec())"));
    assert!(
        writer.find("if file_bytes != selected.size").unwrap()
            < writer.find(".finish_file()").unwrap()
    );
    assert!(writer.contains("ERR_READ"));
    assert!(writer.contains("Ok(total)"));
}
