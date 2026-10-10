//! **content-URI 往返门**（机制侧）—— 与调用点侧的两组（`misc/logs/tests`、`misc/backup/tests`）
//! 合起来是一道门。
//!
//! # 这道门守的是什么
//!
//! W-18 修的缺陷是「**静默**失败」：Android SAF 交回 `FilePath::Url(content://…)`，
//! 而 `into_path().ok()` 把它吃成 `None`，调用方判成「用户取消了」。它在 UI 上与真取消同形 ——
//! 没有报错、没有日志。所以只写「不许出现 cancelled」是不够的：**「什么都没发生」也不会出现
//! cancelled**。故本门每一组都是正负成对，且正侧必须断言**字节真的走通了**：
//!
//!  · **正**：喂 `FilePath::Url`（`content://…` 与 `file:…` 两种形态），断言 ① 不落 cancelled；
//!    ② 网关真的被调用了；③ 目标里真的有那些字节（读侧则是真的把字节读了回来）；
//!    ④ 写侧还要断言**覆盖**：预置一份比新正文更长的旧内容，写完必须逐字相等而不是带着旧尾巴。
//!  · **负**：喂**裸路径**（`FilePath::Path`），断言仍走 `std::fs` 原路 —— 判据是**网关一次都
//!    没被调用**，外加落盘内容与 `filePath` 回执逐字不变。这是「桌面一行不许变」那条要求的
//!    可执行形态：桌面的对话框**只产得出**裸路径（`tauri-plugin-dialog-2.7.2/src/desktop.rs`
//!    的五个回调一律 `p.path().to_path_buf().into()`）。
//!
//! # 复审推翻过的第一版（留档，免得下一个人再走一遍）
//!
//! 第一版把 `file:` URL 放在**负**侧、断言它「一次都不许碰网关」，还把这条登记成「桌面回归的
//! 可执行形态」。两处都是错的：桌面根本产不出 `file:` URL（那条断言实际只覆盖 iOS），而 iOS 上
//! `file:` URL 恰恰**必须**经插件（`ios.rs:38-66` 的 security scope）。于是那条负侧断言把 iOS 上
//! 的错误行为钉死，正确改法反倒会让门变红。现在 `file:` URL 在**正**侧。
//!
//! # 射程自曝（不许把这道门说得比实际宽）
//!
//! 1. **单测里构造的 `content://…` 与真机 SAF 返回的不是一回事。** 本门证明的是「URI 形态的目标
//!    在本仓这一侧不再被吃成取消、并且真的经插件网关读写到了字节」。真机上
//!    `ACTION_CREATE_DOCUMENT` 返回的那个 URI 能不能被 `ContentResolver` 打开、
//!    `getFileDescriptor` 给不给 fd、权限持不持久 —— **一条都没验**，归真机验收。
//! 2. **网关的生产实现（[`super::PluginFiles`]）没有被执行到。** 它要真 `AppHandle`
//!    （本仓未在 lib 单测里引 `tauri::test`）。本门驱动的是分派逻辑与 I/O 形状；
//!    「插件状态真的在、`Fs::open` 真的能开」由那条断言不到的缝之外的东西负责。
//! 3. **不覆盖 `logs_archive_legacy`**：它要「目标所在的目录」（同目录临时文件 + `rename`），
//!    在 URI 上结构性不成立，处置是**显式报错**，判据在它自己的调用点旁边。
//!    🔴 **`taildrop_save` 2026-09-13（批 16）从这一条里移出去了**：它同样要目录，但那条腿
//!    改成了「先在应用私有目录落一个完整的临时文件、再一次性灌进用户选的文档」——
//!    形态换了，且**有一条保证真的拿不回来**（提交那一跳的原子性：SAF 没有 rename，
//!    写到一半失败会在目标文档上留下半截）。降级点逐条写在 `taildrop.rs` 那张表里，
//!    本模块只提供 [`super::stream_into_picked`]，不替调用方主张它有原子性。
//! 4. **iOS 一条都没跑过。** `file:` URL 那一支的正侧证明的是「它经了网关」，不是「iOS 上真的
//!    拿到了 security scope」——本仓编不出 iOS app、CI 无 iOS 腿（见 `picked_file` 模块文档
//!    射程自曝 4）。它是照插件源码推出来的，归 iOS 那条线实测。
//! 5. **派线程那条只有源码级判据。** [`every_uri_leg_hands_its_blocking_io_off_the_async_task`]
//!    读的是源码文本，证明五条腿都经 `with_picked_gateway`、且 `spawn_blocking` 落在 URI 那一臂
//!    **区间内**。它证不了运行期真的没挂住 worker（那需要一个本仓没有的多线程运行期夹具）。
//! 6. **`RecordingGateway` 的替身是本机普通文件。** 故 `truncate` 那条覆盖断言测的是
//!    `std::fs::OpenOptions::from(opts)` 的语义；Android 上那一位变成 `openAssetFileDescriptor`
//!    的 `"wt"` 模式，各家 DocumentsProvider 支不支持截断**归真机**。

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::Mutex;

use super::{
    classify, display_of, file_name_of, open_picked_for_read, read_picked_to_string,
    stream_into_picked, write_picked, FileGateway, FilePath, PickedTarget,
};
use crate::commands::guard_scan::top_level_fn_body;
use crate::test_support::{crate_code, TestDir};

/// 记账网关：把 URI 目标映射到一份真实的临时文件，并记下每一次被调用的目标。
///
/// 两件事同时要：**正**侧靠「真的落了字节」证明这条路是通的，**负**侧靠
/// [`RecordingGateway::calls`] 恒为空证明桌面那条路根本没绕到这里来。只记调用不真写，
/// 正侧就只能证明「被调用了」而证明不了「写成了」；只真写不记调用，负侧就没有判据。
pub(crate) struct RecordingGateway {
    /// URI 目标在本机的替身文件。
    backing: PathBuf,
    calls: Mutex<Vec<String>>,
    /// `Some(n)` ⇒ 经 [`FileGateway::open_for_write`] 拿到的写者在接收满 `n` 字节后报错。
    fail_write_after: Option<u64>,
}

impl RecordingGateway {
    pub(crate) fn new(backing: PathBuf) -> Self {
        Self {
            backing,
            calls: Mutex::new(Vec::new()),
            fail_write_after: None,
        }
    }

    /// 同 [`Self::new`]，但流式写入在**第 `n` 字节**失败：前 `n` 字节真的落进替身文件，
    /// 之后每次 `write` 都报错。模拟的是 SAF 上「本地 fd 写到一半失败」—— `taildrop_save`
    /// URI 支唯一拿不回原子性的那一跳。
    pub(crate) fn failing_write_after(backing: PathBuf, n: u64) -> Self {
        Self {
            fail_write_after: Some(n),
            ..Self::new(backing)
        }
    }

    /// 替身文件当前的字节数（不存在 → `None`）。与回执比对用：这是**独立于被测代码**量出来的
    /// 「目标上真实留下了多少」。
    pub(crate) fn backing_len(&self) -> Option<u64> {
        std::fs::metadata(&self.backing).ok().map(|m| m.len())
    }

    /// 至今被打开过的目标（URI 原文，按调用顺序）。
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("网关记账锁").clone()
    }

    /// 替身文件当前的内容（不存在 → `None`）。
    pub(crate) fn contents(&self) -> Option<String> {
        std::fs::read_to_string(&self.backing).ok()
    }

    /// 预置替身文件的内容（读侧用）。
    pub(crate) fn seed(&self, body: &str) {
        std::fs::write(&self.backing, body).expect("预置替身文件");
    }
}

impl FileGateway for RecordingGateway {
    fn open(
        &self,
        target: &FilePath,
        opts: tauri_plugin_fs::OpenOptions,
    ) -> std::io::Result<std::fs::File> {
        self.calls
            .lock()
            .expect("网关记账锁")
            .push(target.to_string());
        std::fs::OpenOptions::from(opts).open(&self.backing)
    }

    fn open_for_write(
        &self,
        target: &FilePath,
        opts: tauri_plugin_fs::OpenOptions,
    ) -> std::io::Result<Box<dyn std::io::Write>> {
        let file = self.open(target, opts)?;
        Ok(match self.fail_write_after {
            None => Box::new(file),
            Some(budget) => Box::new(FailAfter { file, budget }),
        })
    }
}

/// 接收满 `budget` 字节后每次 `write` 都报错的写者。最后一次可以**短写**（只收下剩余额度），
/// 于是失败点精确落在第 `budget` 字节上，而不是落在某个 64 KiB 缓冲块的边界上。
struct FailAfter {
    file: std::fs::File,
    budget: u64,
}

impl std::io::Write for FailAfter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.budget == 0 {
            return Err(std::io::Error::other("注入的写失败（RecordingGateway）"));
        }
        let take = buf
            .len()
            .min(usize::try_from(self.budget).unwrap_or(usize::MAX));
        let n = std::io::Write::write(&mut self.file, &buf[..take])?;
        self.budget -= n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.file)
    }
}

/// 一个形状取自真机的 SAF 目标（Downloads provider 的 document URI）。
pub(crate) const SAF_URI: &str = "content://com.android.providers.downloads.documents/document/42";

/// SAF 目标。**断言它真的落在 URL 变体上** —— 不然整条正侧会在一个「其实是路径」的输入上假绿。
pub(crate) fn saf_file_path() -> FilePath {
    let parsed = FilePath::from_str(SAF_URI).expect("FilePath::from_str 是 Infallible");
    assert!(
        matches!(parsed, FilePath::Url(_)),
        "content: URI 必须解析成 Url 变体，否则这条判据测的不是 W-18 那个形态"
    );
    parsed
}

/// 建一个 URI 目标 + 它的记账网关。
pub(crate) fn saf_target(dir: &TestDir, backing_name: &str) -> (FilePath, RecordingGateway) {
    (
        saf_file_path(),
        RecordingGateway::new(dir.path().join(backing_name)),
    )
}

/// 本机绝对路径 → `file:` URL 形态的目标。
///
/// 🔴 **这不是桌面形态。** 桌面的对话框产不出 `Url` 变体（`tauri-plugin-dialog-2.7.2/src/desktop.rs`
/// 的 146/161/176/191/206 五个回调一律 `p.path().to_path_buf().into()`）。`file:` URL 是 **iOS**
/// 的形态：`saveFileDialog` 用 `.exportToService` 交回用户选中位置（沙箱外）的 file URL。
/// 第一版拿它当桌面负侧用，等于把「iOS 上绕过 security scope」钉死成不变量 —— 复审推翻，
/// 它现在在**正**侧。桌面负侧用的是裸路径。
///
/// 本仓不直接依赖 `url` crate，也不为一条判据去引（禁引新依赖），故按 URL 规范手工拼：
/// Unix 的 `/a/b` → `file:///a/b`，Windows 的 `C:\a\b` → `file:///C:/a/b`。
/// 拼完当场自检两件事：① 落在 `Url` 变体上；② **仍然转得出**同一个本机路径 —— 拼错了在这里红，
/// 而不是让下游在一个坏 URL 上得出结论。第 ② 条同时是「本条覆盖的是第一版口径的反面」那句话的
/// 前提：正因为它转得出路径，第一版才会把它判成 `Path`。
pub(crate) fn file_url_of(path: &Path) -> FilePath {
    let text = path.to_string_lossy().replace('\\', "/");
    let joined = if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    };
    let parsed = FilePath::from_str(&joined).expect("FilePath::from_str 是 Infallible");
    assert!(
        matches!(parsed, FilePath::Url(_)),
        "{joined} 必须解析成 Url 变体"
    );
    assert_eq!(
        parsed.clone().into_path().ok().as_deref(),
        Some(path),
        "这个 file: URL 必须仍转得出同一个本机路径，否则本组判据自己就是坏的"
    );
    assert_eq!(
        classify(&parsed),
        PickedTarget::Uri(parsed.clone()),
        "file: URL 必须落进 URI 侧 —— 那正是 iOS 上 security scope 所在的那一支"
    );
    parsed
}

/// 桌面负侧的真实形态：对话框交回的裸路径。
pub(crate) fn desktop_target(path: &Path) -> FilePath {
    let parsed = FilePath::Path(path.to_path_buf());
    assert_eq!(
        classify(&parsed),
        PickedTarget::Path(path.to_path_buf()),
        "桌面形态必须落进路径侧 —— 那条路上 std::fs 一行没变"
    );
    parsed
}

/* ══════════════ ① 分派：形态判定是「哪个变体」，不是「转不转得出路径」 ══════════════ */

#[test]
fn classify_splits_on_the_variant_not_on_whether_a_path_can_be_derived() {
    // 正：content URI 没有文件系统路径。
    assert!(
        matches!(classify(&saf_file_path()), PickedTarget::Uri(_)),
        "content URI 必须落进 URI 侧"
    );

    // 负（**桌面形态**）：裸路径 —— 桌面的对话框只产得出这一种，故这才是「桌面一行不变」的
    // 可执行判据。
    let dir = TestDir::new("polaris-picked-classify");
    let local = dir.path().join("export.md");
    let _ = desktop_target(&local);

    // iOS 形态：`file:` URL 即便**转得出**本机路径，也必须落进 URI 侧。
    // 这一条是第一版口径的正反面：按 `into_path()` 分派时它落 `Path`，于是 iOS 上
    // `std::fs` 直接开一个沙箱外 URL，绕开 `ios.rs` 的 `startAccessingSecurityScopedResource`。
    let as_url = file_url_of(&local);
    assert!(
        as_url.clone().into_path().is_ok(),
        "本条的前提：这个 URL 转得出路径（转不出就测不到「转得出也仍走 URI 侧」）"
    );
    assert!(matches!(classify(&as_url), PickedTarget::Uri(_)));
}

#[test]
fn display_of_keeps_the_desktop_string_and_hands_back_every_url_verbatim() {
    let dir = TestDir::new("polaris-picked-display");
    let local = dir.path().join("a.md");

    // 负（桌面）：回执逐字不变 —— 改动前就是 `path.to_string_lossy()`。
    assert_eq!(
        display_of(&desktop_target(&local)),
        local.to_string_lossy().into_owned()
    );

    // URL 侧一律回原文，且回执必须**解析回同一支**：那个字符串会经前端绕一圈再
    // `FilePath::from_str` 回来（`backup_import_apply`）。若这里把 `file:` URL 降解成路径，
    // 往返回来的就是 `Path` 变体 —— iOS 上等于中途掉出 security scope 那一支。
    for url in [saf_file_path(), file_url_of(&local)] {
        let echoed = display_of(&url);
        assert_eq!(echoed, url.to_string(), "URL 侧回执必须是原文");
        let round_tripped = FilePath::from_str(&echoed).expect("FilePath::from_str 是 Infallible");
        assert_eq!(
            classify(&round_tripped),
            classify(&url),
            "display_of → 前端 → from_str 这条往返必须落回同一支"
        );
    }
    assert_eq!(display_of(&saf_file_path()), SAF_URI);
}

#[test]
fn file_name_of_takes_the_last_uri_segment_and_never_pretends_to_be_a_path() {
    let dir = TestDir::new("polaris-picked-name");
    assert_eq!(
        file_name_of(&FilePath::Path(dir.path().join("sub.yaml"))),
        "sub.yaml"
    );
    assert_eq!(file_name_of(&saf_file_path()), "42");
}

/* ══════════════ ② 写：正负两侧 ══════════════ */

#[test]
fn writing_to_a_content_uri_goes_through_the_gateway_and_really_lands_bytes() {
    let dir = TestDir::new("polaris-picked-write-uri");
    let (target, gateway) = saf_target(&dir, "saf-doc");

    write_picked(&gateway, &target, b"exported-body").expect("URI 目标必须写得出去");

    assert_eq!(
        gateway.calls(),
        vec![SAF_URI.to_string()],
        "URI 目标必须交给插件网关，不许自己去拼路径"
    );
    assert_eq!(
        gateway.contents().as_deref(),
        Some("exported-body"),
        "「不报错」不等于「写进去了」—— 这一条断言的是字节"
    );
}

#[test]
fn writing_to_a_content_uri_truncates_whatever_was_already_there() {
    // `overwrite_opts()` 的 `truncate` 是 Android 上**唯一**决定 `android_mode()` 出 `"wt"`
    // 而不是 `"w"` 的那一位（`tauri-plugin-fs-2.5.1/src/lib.rs:303-320` 只映射 r/w/t/a，
    // `create` 被丢弃）。上一条只在**空文件**上写一次，`truncate` 删掉它照样绿 —— 实测过。
    //
    // 真机上的失效形态很具体：允许「替换现有文件」的 DocumentsProvider 交回一份**更长的旧
    // 文档**，没有 truncate 的话新备份会带着旧尾巴落盘，导出**当场报成功**，一直到用户恢复时
    // `parse_backup_content` 判 invalidFormat 才暴露 —— 而门一声不吭。
    let dir = TestDir::new("polaris-picked-write-truncate");
    let (target, gateway) = saf_target(&dir, "saf-doc");
    gateway.seed("OLD-BACKUP-CONTENT-THAT-IS-MUCH-LONGER-THAN-THE-NEW-BODY");

    write_picked(&gateway, &target, b"{\"v\":\"1.1\"}").expect("URI 目标必须写得出去");

    assert_eq!(
        gateway.contents().as_deref(),
        Some("{\"v\":\"1.1\"}"),
        "必须**逐字相等**，不能用 contains —— 带着旧尾巴同样会让 contains 通过"
    );
}

#[test]
fn writing_to_a_file_url_also_goes_through_the_gateway() {
    // iOS 形态。经网关不是绕远路：`ios.rs` 的 `Fs::open` 先
    // `startAccessingSecurityScopedResource()`，随后仍是 `to_file_path()` +
    // `std::fs::OpenOptions`，是 `std::fs::write` 那条路的**超集**。
    // 射程：iOS 真机一条都没跑过（见头注射程自曝 4）。
    let dir = TestDir::new("polaris-picked-write-file-url");
    let (_, gateway) = saf_target(&dir, "ios-doc");
    let picked = dir.path().join("picked.md");
    let target = file_url_of(&picked);

    write_picked(&gateway, &target, b"ios-body").expect("file: URL 目标必须写得出去");

    assert_eq!(
        gateway.calls(),
        vec![target.to_string()],
        "file: URL 必须经网关 —— 直接 std::fs 开它在 iOS 上没有 security scope"
    );
    assert_eq!(gateway.contents().as_deref(), Some("ios-body"));
    assert!(
        !picked.exists(),
        "字节必须落在网关那一侧；直接落到路径上说明分派又绕回了 std::fs"
    );
}

#[test]
fn writing_to_a_local_path_never_touches_the_gateway() {
    // 桌面负侧：只喂**裸路径**（桌面的对话框只产得出这一种）。
    let dir = TestDir::new("polaris-picked-write-path");
    let out = dir.path().join("export.md");
    // 网关的替身指向另一个文件：一旦有人把桌面这条路也改道过去，下面两条断言会同时红。
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));

    write_picked(&gateway, &desktop_target(&out), b"desktop-body").expect("桌面目标必须写得出去");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "desktop-body");

    assert!(
        gateway.calls().is_empty(),
        "桌面必须仍走 std::fs 原路，一次都不该碰插件网关"
    );
    assert!(
        gateway.contents().is_none(),
        "桌面那条路不许在网关的替身上留下任何痕迹"
    );
}

/* ══════════════ ②-b 流式写（`stream_into_picked`）：正负两侧 + 一条截断 ══════════════ */

#[test]
fn streaming_into_a_content_uri_goes_through_the_gateway_and_really_lands_bytes() {
    let dir = TestDir::new("polaris-picked-stream-uri");
    let (target, gateway) = saf_target(&dir, "saf-doc");
    let source = dir.path().join("staged.bin");
    std::fs::write(&source, b"taildrop-payload").expect("预置源文件");

    let mut reader = std::fs::File::open(&source).expect("打开源文件");
    let n = stream_into_picked(&gateway, &target, &mut reader).expect("URI 目标必须写得出去");

    assert_eq!(
        n,
        b"taildrop-payload".len() as u64,
        "返回的必须是真写出的字节数"
    );
    assert_eq!(
        gateway.calls(),
        vec![SAF_URI.to_string()],
        "URI 目标必须交给插件网关，不许自己去拼路径"
    );
    assert_eq!(
        gateway.contents().as_deref(),
        Some("taildrop-payload"),
        "「不报错」不等于「写进去了」—— 这一条断言的是字节"
    );
}

#[test]
fn streaming_into_a_content_uri_truncates_whatever_was_already_there() {
    // 与 `write_picked` 那条同一个理由（`overwrite_opts()` 的 `truncate` 是 Android 上唯一
    // 决定 `android_mode()` 出 `"wt"` 的那一位）：没有它，一份**更长的旧文档**会给新内容
    // 留下旧尾巴，而取件**当场报成功**。Taildrop 上这条更要紧 —— 用户选的往往正是同名旧文件。
    let dir = TestDir::new("polaris-picked-stream-trunc");
    let (target, gateway) = saf_target(&dir, "saf-doc");
    gateway.seed("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    let source = dir.path().join("staged.bin");
    std::fs::write(&source, b"short").expect("预置源文件");

    let mut reader = std::fs::File::open(&source).expect("打开源文件");
    stream_into_picked(&gateway, &target, &mut reader).expect("URI 目标必须写得出去");

    assert_eq!(
        gateway.contents().as_deref(),
        Some("short"),
        "新内容必须完整替换旧文档 —— 留下旧尾巴的取件文件会静默损坏"
    );
}

#[test]
fn streaming_into_a_local_path_never_touches_the_gateway() {
    // 桌面负侧：`taildrop_save` 的路径支根本不走这个函数（它走 `.part` + rename），
    // 但本函数的路径臂仍必须是 `std::fs` 原路 —— 哪天有人把桌面改道过来，这两条会同时红。
    let dir = TestDir::new("polaris-picked-stream-path");
    let out = dir.path().join("saved.bin");
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));
    let source = dir.path().join("staged.bin");
    std::fs::write(&source, b"desktop-payload").expect("预置源文件");

    let mut reader = std::fs::File::open(&source).expect("打开源文件");
    stream_into_picked(&gateway, &desktop_target(&out), &mut reader).expect("桌面目标必须写得出去");

    assert_eq!(std::fs::read(&out).unwrap(), b"desktop-payload");
    assert!(
        gateway.calls().is_empty(),
        "桌面必须仍走 std::fs 原路，一次都不该碰插件网关"
    );
    assert!(
        gateway.contents().is_none(),
        "桌面那条路不许在网关的替身上留下任何痕迹"
    );
}

/* ══════════════ ③ 读：正负两侧 ══════════════ */

#[test]
fn reading_a_content_uri_goes_through_the_gateway_and_really_returns_bytes() {
    let dir = TestDir::new("polaris-picked-read-uri");
    let (target, gateway) = saf_target(&dir, "saf-doc");
    gateway.seed("{\"version\":\"1.1\"}");

    let body = read_picked_to_string(&gateway, &target).expect("URI 目标必须读得出来");

    assert_eq!(body, "{\"version\":\"1.1\"}");
    assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);
}

#[test]
fn reading_a_file_url_also_goes_through_the_gateway() {
    let dir = TestDir::new("polaris-picked-read-file-url");
    let (_, gateway) = saf_target(&dir, "ios-doc");
    gateway.seed("ios-source");
    let target = file_url_of(&dir.path().join("picked.polaris-backup"));

    assert_eq!(
        read_picked_to_string(&gateway, &target).expect("file: URL 目标必须读得出来"),
        "ios-source"
    );
    assert_eq!(gateway.calls(), vec![target.to_string()]);
}

#[test]
fn reading_a_local_path_never_touches_the_gateway() {
    let dir = TestDir::new("polaris-picked-read-path");
    let src = dir.path().join("backup.polaris-backup");
    std::fs::write(&src, "{\"version\":\"1.0\"}").unwrap();
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));

    assert_eq!(
        read_picked_to_string(&gateway, &desktop_target(&src)).unwrap(),
        "{\"version\":\"1.0\"}"
    );
    assert!(gateway.calls().is_empty(), "桌面读侧必须仍走 std::fs 原路");
}

#[test]
fn streaming_handle_follows_the_same_split() {
    let dir = TestDir::new("polaris-picked-stream");
    let (uri_target, gateway) = saf_target(&dir, "saf-doc");
    gateway.seed("stream-me");

    let mut buf = String::new();
    open_picked_for_read(&gateway, &uri_target)
        .expect("URI 目标必须开得出只读句柄")
        .read_to_string(&mut buf)
        .unwrap();
    assert_eq!(buf, "stream-me");
    assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);

    let local = dir.path().join("local.conf");
    std::fs::write(&local, "local-body").unwrap();
    let mut buf = String::new();
    open_picked_for_read(&gateway, &desktop_target(&local))
        .expect("桌面目标必须开得出只读句柄")
        .read_to_string(&mut buf)
        .unwrap();
    assert_eq!(buf, "local-body");
    assert_eq!(
        gateway.calls().len(),
        1,
        "桌面那次不许再进网关（计数仍是 URI 那一次）"
    );
}

/* ══════════════ ④ 正向对照：这套判据报得出错 ══════════════ */

#[test]
fn a_gateway_failure_is_reported_and_not_swallowed() {
    // 替身落在一个**不存在的目录**里 ⇒ 打开必失败。上面几条全是成功路径，这里喂一个已知应失败
    // 的输入，证明这套判据分得清「成功」与「失败」，而不是对任何输入都点头。
    let dir = TestDir::new("polaris-picked-negative");
    let gateway = RecordingGateway::new(dir.path().join("no-such-dir").join("doc"));

    let err =
        write_picked(&gateway, &saf_file_path(), b"x").expect_err("打不开就必须报错，不许静默成功");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);
}

/* ══════════════ ⑤ 源码级：URI 支不许在 async command 的 task 上同步跑 ══════════════ */

/// 五条腿逐条点名：`(取材文件, 顶层锚点)`。
///
/// 写死成清单而不是「扫全仓找 `PluginFiles::new`」，是为了让**新增一条腿却忘了派线程**这件事
/// 有人管：清单少一条 ⇒ 下面那条「生产侧构造网关的地方只有这几处」的负面断言会红。
const URI_LEGS: &[(&str, &str)] = &[
    ("commands/misc/logs.rs", "async fn export_off_thread("),
    ("commands/misc/backup.rs", "pub async fn backup_export("),
    (
        "commands/misc/backup.rs",
        "pub async fn backup_import_pick(",
    ),
    (
        "commands/misc/backup.rs",
        "pub async fn backup_import_apply(",
    ),
    /* Taildrop 两条腿（2026-09-13 批 16 接上 URI 支）。锚点指的是**辅助函数**而不是那两个
    `#[tauri::command]` 本体 —— 与 `export_off_thread` 同形：命令体负责分派（`classify` 的
    两臂），真正碰网关的那一段各自收在一个函数里，锚点跟着网关走。
    这两条此前**不在本清单里也不该在**：它们那时根本没有 URI 支（显式报错）。 */
    (
        "commands/taildrop.rs",
        "async fn save_stream_to_uri<R: tauri::Runtime>(",
    ),
    (
        "commands/taildrop.rs",
        "async fn open_selected_targets<R: tauri::Runtime>(",
    ),
];

#[test]
fn every_uri_leg_hands_its_blocking_io_off_the_async_task() {
    // 不变量：URI 支的插件 I/O 是**同步阻塞**的，且上游拿不到 fd 时会 panic
    //（`picked_file` 模块文档射程自曝 5 / 6）。故每条 `#[tauri::command] async fn` 文件腿都必须
    // 经 `with_picked_gateway`，而不是在自己的 task 上直接构造网关同步读写。
    //
    // `subscription::local_import_pick_file` 是第五条腿，它自带 `spawn_blocking`，判据在
    // `subscription/tests/wiring_gate.rs::local_file_picker_never_blocks_a_tokio_worker_on_std_fs`。
    for (file, anchor) in URI_LEGS {
        let body = top_level_fn_body(&crate_code(file), anchor);
        assert!(
            body.contains("with_picked_gateway("),
            "{file} 的 `{anchor}` 必须经 with_picked_gateway 派线程（URI 支是同步阻塞的）"
        );
        assert!(
            !body.contains("PluginFiles::new("),
            "{file} 的 `{anchor}` 不许自己在 async task 上构造生产网关同步读写"
        );
    }

    // 生产侧构造网关的地方只剩 `with_picked_gateway` 自己（两臂各一处）与 subscription 那条
    // 自带 `spawn_blocking` 的腿（一处）。多出一处 = 有人新开了一条没派线程的腿。
    // 取材面把**所有**碰过文件框的模块都列进来，别只数已知的那两个 —— 否则新腿写在第三个
    // 文件里时这条断言一声不吭。
    const GATEWAY_OWNERS: [&str; 2] = ["commands/picked_file.rs", "commands/subscription.rs"];
    let mut seen = 0usize;
    for file in [
        "commands/picked_file.rs",
        "commands/subscription.rs",
        "commands/misc/logs.rs",
        "commands/misc/backup.rs",
        "commands/taildrop.rs",
    ] {
        let hits = crate_code(file).matches("PluginFiles::new(").count();
        if hits > 0 {
            assert!(
                GATEWAY_OWNERS.contains(&file),
                "{file} 直接构造了生产网关（{hits} 处）—— 新腿必须经 with_picked_gateway"
            );
            seen += hits;
        }
    }
    assert_eq!(
        seen, 3,
        "生产侧构造网关的地方应恰好三处（with_picked_gateway 两臂各一 + subscription 的 \
         spawn_blocking 支一处）—— 数目变了说明有腿绕过了这道门，请连同 URI_LEGS 一起更新"
    );

    // `with_picked_gateway` 自己：`spawn_blocking` 必须落在 URI 那一臂**区间内**。
    // 只断言它「在函数里某处出现过」，等于允许把 open 写在臂外而 spawn_blocking 用在别处 ——
    // 与 wiring_gate 里那条同款的偏移比对。
    let helper = top_level_fn_body(
        &crate_code("commands/picked_file.rs"),
        "pub async fn with_picked_gateway<R, T, F>(",
    );
    let uri_arm = helper
        .find("Some(PickedTarget::Uri(_)) => {")
        .expect("URI 那一臂必须在场");
    let blocking = helper
        .find("tokio::task::spawn_blocking(move || work(")
        .expect("URI 支必须派到阻塞线程池");
    assert!(
        blocking > uri_arm,
        "spawn_blocking 必须落在 URI 那一臂里，而不是函数里随便某处"
    );
    assert!(
        !helper[..uri_arm].contains("spawn_blocking"),
        "URI 臂之前不许出现 spawn_blocking（否则上面那条偏移比对可以被前置的一处满足）"
    );
}

/// 门的自检（正向对照）：取材器真的取到了**生产**函数体，而不是本模块里的同名字面量。
#[test]
fn the_off_thread_gate_scans_production_code_not_itself() {
    let helper = top_level_fn_body(
        &crate_code("commands/picked_file.rs"),
        "pub async fn with_picked_gateway<R, T, F>(",
    );
    assert!(
        helper.contains("picked.as_ref().map(classify)"),
        "取到的必须是生产函数体（含真实实现语句），实得：{}",
        &helper[..helper.len().min(200)]
    );
    // 自指检查：测试实体住在 `picked_file/tests/mod.rs`，`crate_code("commands/picked_file.rs")`
    // 只该看到生产码 + 那句 `mod tests;`。拿本模块**独有**的符号验，而不是拿 `mod tests`
    // 这个声明本身 —— 后者本来就在生产文件里。
    for own_symbol in ["RecordingGateway", "SAF_URI", "desktop_target"] {
        assert!(
            !crate_code("commands/picked_file.rs").contains(own_symbol),
            "扫描面混进了本测试模块的 `{own_symbol}`——自指会让判据在自己的字符串常量里找到证据"
        );
    }
    for (file, _) in URI_LEGS {
        assert!(
            !crate_code(file).contains("RecordingGateway"),
            "{file} 的取材面混进了替身网关，说明扫的不是纯生产码"
        );
    }
}

/// 正向对照：锚点消失时这道门必须**红**，而不是静默拿到一段空片继续点头。
#[test]
#[should_panic(expected = "命中 0 次")]
fn the_off_thread_gate_goes_red_when_an_anchor_disappears() {
    let _ = top_level_fn_body(
        &crate_code("commands/picked_file.rs"),
        "pub async fn no_such_function_anywhere(",
    );
}

/* ══════════════ ⑥ 流式写中途失败：报的是目标**接收的**字节数 ══════════════ */

#[test]
fn a_stream_that_fails_midway_reports_what_the_target_really_received() {
    let dir = TestDir::new("polaris-picked-stream-fail");
    let gateway = RecordingGateway::failing_write_after(dir.path().join("saf-doc"), 70_000);
    let payload = vec![7u8; 200_000];

    let err = stream_into_picked(&gateway, &saf_file_path(), &mut payload.as_slice())
        .expect_err("注入失败必须报错，不许静默成功");

    assert_eq!(gateway.backing_len(), Some(70_000), "注入点没打上");
    assert_eq!(err.written, 70_000, "written 必须是目标真实接收的字节数");
    assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);
}
