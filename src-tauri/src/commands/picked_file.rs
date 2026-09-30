//! 原生文件框选中的目标 —— **本机路径**与 **content URI** 两种形态的统一读写。
//!
//! # 修的是什么（W-18）
//!
//! `tauri-plugin-dialog` 的保存 / 打开框把用户的选择交回来时，给的是
//! [`FilePath`]，它有两个变体：`Path`（本机路径）与 `Url`（`file:` URL，
//! **以及 Android SAF 返回的 `content:` URI**）。本仓此前一律写
//!
//! ```text
//! .save_file(…)                          // 或 .pick_file(…)
//! .and_then(|p| p.into_path().ok())      // ← 缺陷在这一行
//! ```
//!
//! 而 `into_path()` 对 `Url` 变体走的是 `url.to_file_path()`
//! （`tauri-plugin-fs-2.5.1/src/file_path.rs:54-59`），它对 `content:` **恒 `Err`** ——
//! 于是 `.ok()` 把它吃成 `None`，调用方按既有约定判成「用户取消了」。
//!
//! ⇒ 用户点「导出备份」，系统文件保存器弹出来、选好位置、按了确定，**然后什么都没发生**。
//! 这是**静默失败**：它不报错、不留日志、在 UI 上与「我自己按了取消」完全同形。
//! 改动前全仓 `FilePath::Url` 零命中 —— 这条分支从来没有被处理过。
//!
//! # 为什么不是「先转 `PathBuf` 再用 `std::fs`」
//!
//! `content://com.android.providers.downloads.documents/document/42` **没有文件系统路径**。
//! 它是一个由 `ContentResolver` 解析的句柄，只能拿到 fd。`tauri-plugin-fs` 提供的正是这条路：
//! [`tauri_plugin_fs::Fs::open`] 吃 [`FilePath`] 吐 `std::fs::File` ——
//! Android 上把 `content:` 交给 `getFileDescriptor` 再 `File::from_raw_fd`
//! （`tauri-plugin-fs-2.5.1/src/android.rs:33-63`），桌面上则退回 `std::fs::OpenOptions`
//! （同版 `desktop.rs:27-38`）。本模块按它提供的形态写，不自己造第二套。
//!
//! # 桌面必须一行不变（本批最容易出的回归）
//!
//! 故本模块的分派**不是**「按平台分」，而是 [`classify`] 按 [`FilePath`] 的**变体**分 ——
//! 与 `tauri_plugin_fs::Fs::open` 自己的口径逐字同构（三端的 `open` 都是先看变体）：
//!
//!  · `FilePath::Path` ⇒ 走**与改动前逐字相同**的 `std::fs::write` / `std::fs::read_to_string`，
//!    连回给前端的 `filePath` 字符串也仍是 `path.to_string_lossy()`（见 [`display_of`]）。
//!  · `FilePath::Url` ⇒ 经 [`FileGateway`] 交给插件。
//!
//! **桌面恒走前者**，这不是推断：`tauri-plugin-dialog-2.7.2/src/desktop.rs` 的 146 / 161 / 176 /
//! 191 / 206 五个回调一律 `p.path().to_path_buf().into()`，桌面的对话框**产生不出** `Url` 变体。
//! `backup_import_apply` 那条二次打开拿的是前端回传的字符串，`FilePath::from_str` 对 Unix 绝对
//! 路径（无 scheme，`Url::parse` 直接失败）与 Windows 盘符（单字母 scheme 留给盘符，
//! `tauri-plugin-fs-2.5.1/src/file_path.rs:189-196`）同样落 `Path`。
//! ⇒ 桌面回归的可执行判据是「**裸路径**目标一次都不碰网关」，见 `picked_file/tests/mod.rs`。
//!
//! # 为什么不按「`into_path()` 成不成」分（本批复审推翻的第一版口径）
//!
//! 第一版按 `into_path()` 是否成功分派，于是**所有 `file:` URL 都落进路径侧**。桌面上看不出
//! 差别（桌面根本产不出 `file:` URL），iOS 上则恰好相反：`saveFileDialog` 用
//! `UIDocumentPickerViewController(url:in:.exportToService)`
//! （`tauri-plugin-dialog-2.7.2/ios/Sources/DialogPlugin.swift:168`）交回的是**沙箱外**、用户选中
//! 位置的 `file:` URL，直接 `std::fs` 开它没有 security scope；而
//! `tauri-plugin-fs-2.5.1/src/ios.rs:38-66` 的 `Fs::open` 存在的唯一理由，就是先替这一支调
//! `startAccessingSecurityScopedResource()` 再开。更糟的是第一版的门把这个行为**钉死**了
//! （负侧断言「`file:` URL 一次都不许碰网关」），等于把 iOS 上的正确改法判成回归。
//!
//! 按变体分派在三端都对：桌面 `path_or_err` 仍把 `file:` 解析成路径后 `std::fs::OpenOptions`
//! （`desktop.rs:13-24`），Android 走 `getFileDescriptor`（`android.rs:35-45`），iOS 拿到安全作用域。
//!
//! # 射程自曝（这个模块**不**保证什么）
//!
//! 1. **单测里构造的 `content://…` 不是真机 SAF 返回的那个东西。** 判据能证明的是
//!    「URI 形态的目标不再被吃成取消、且真的经网关读写到了字节」；真机上
//!    `ACTION_CREATE_DOCUMENT` 返回的 URI 能不能被 `ContentResolver` 打开、
//!    权限持不持久，**归真机验收**，本模块的任何一条断言都不覆盖它。
//! 2. **只治「选中的那个目标」。** 需要目标**所在目录**的动作（同目录临时文件 + `rename`
//!    的事务性落盘）在 content URI 上结构性不成立 —— 那些调用点的处置是**显式报错**，
//!    不是假装成功，也不再是假装取消。今天有两处：`logs::logs_archive_legacy`
//!    与 `taildrop::taildrop_save`，各自在调用点写明理由。
//! 3. **不做 scope 校验。** 目标是用户刚在系统面板里亲手选的，授权来自那次选择本身；
//!    本模块不替 `fs` 插件的 scope 表做第二次判断。
//! 4. **iOS 一条都没跑过。** 上面按变体分派在 iOS 上是**照插件源码推出来的**，不是实测：本仓
//!    今天编不出 iOS app（`src-tauri/Cargo.toml` 的 crate-type、无 `gen/apple`、CI 无 iOS 腿），
//!    lib 单测里也没有 iOS 目标。能确定的只有「它是 `std::fs` 那条路的**超集**」——
//!    `ios.rs` 拿到 `file:` URL 时先 `startAccessingSecurityScopedResource()`，随后仍是
//!    `url.to_file_path()` + `std::fs::OpenOptions::from(opts).open(path)`，与改前那句
//!    `std::fs::write` 开的是同一个路径。真机行为归 iOS 那条线。
//! 5. **URI 支的插件 I/O 是同步阻塞的，本模块自己不派线程。** Android 上 `gateway.open()`
//!    内部是一次同步 JNI 往返（`run_mobile_plugin` 用 `std::sync::mpsc` 的 `rx.recv()` 阻塞调用
//!    线程，`tauri-2.11.5/src/plugin/mobile.rs:324-341`），拿到的 fd 对云盘类 DocumentsProvider
//!    还可能是 `openPipeHelper` 造的**管道** —— 随后的整文件读写会一直阻塞到远端收完。
//!    本仓的文件腿全是 `#[tauri::command] async fn`，故派线程这件事收口在
//!    [`with_picked_gateway`]，**五条腿一条不落**；本模块的读写函数保持纯同步，好让门能驱动它们。
//! 6. **上游 `unimplemented!()` 不在本模块的守备范围内。** `tauri-plugin-fs-2.5.1/src/android.rs:77-84`
//!    在 Kotlin 侧 `openAssetFileDescriptor(uri, mode)?.parcelFileDescriptor?.detachFd()`
//!    任一环为 null 时直接 `unimplemented!()` panic（`FsPlugin.kt:63-68`）。[`PluginFiles::open`]
//!    的 `try_state` 守卫只挡「插件没注册」，挡不住它 —— 别把那句守卫的注释读成「崩溃面已封住」。
//!    今天的兜底不在这里，而在 [`with_picked_gateway`]：URI 支跑在 `spawn_blocking` 里，panic
//!    收成一个 `JoinError`，调用方翻成稳定错误码，命令**照常回话** —— 而不是让前端那个 `await`
//!    永不 settle、把写在 `finally` 里的 `setBusy(false)` 一起冻住，按钮永久禁用还不报错。

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_fs::OpenOptions;

/// 对话框插件交回来的目标类型。从本模块转出一份，好让调用点只认一个导入来源
/// （`tauri_plugin_dialog::FilePath` 与 `tauri_plugin_fs::FilePath` 是同一个类型，
/// 两处各写一半会让「这两个是不是同一个东西」变成读者要自己去核的事）。
pub use tauri_plugin_fs::FilePath;

/// 把一个 [`FilePath`] 打开成真实文件句柄的能力。
///
/// 抽成 trait 的唯一理由是**可测**：生产实现要经 `tauri-plugin-fs` 的插件状态，而那要真
/// `AppHandle`（本仓未在 lib 单测里引 `tauri::test`）。有了这层缝，往返门才能拿一个
/// 记账用的替身驱动整条导出 / 导入路径，并同时断言「桌面那条路一次都没碰过网关」。
pub trait FileGateway {
    /// 按 `opts` 打开 `target`。`target` 恒为 [`classify`] 判成 URI 的那一支。
    fn open(&self, target: &FilePath, opts: OpenOptions) -> std::io::Result<std::fs::File>;

    /// 打开 `target` 供**流式写入**（[`stream_into_picked`] 的 URI 支用）。
    ///
    /// 默认实现就是 [`FileGateway::open`] 的结果原样装箱 —— 生产实现 [`PluginFiles`] **不覆盖**它，
    /// 生产路径上写的仍是插件交回的那个 fd。单列这一层的唯一理由是**失败注入**：`std::fs::File`
    /// 没法「在第 N 字节写失败」，而「本地 fd 写到一半失败」正是 `taildrop_save` URI 支唯一拿不回
    /// 原子性的那一跳（见 `taildrop.rs` 调用点那张表）。替身在这里换上一个会在第 N 字节报错的写者，
    /// 那条降级才有判据。
    fn open_for_write(
        &self,
        target: &FilePath,
        opts: OpenOptions,
    ) -> std::io::Result<Box<dyn std::io::Write>> {
        Ok(Box::new(self.open(target, opts)?))
    }
}

/// 生产实现：转交 `tauri-plugin-fs`。
pub struct PluginFiles<'a, R: Runtime> {
    app: &'a AppHandle<R>,
}

impl<'a, R: Runtime> PluginFiles<'a, R> {
    /// 绑定到一个 app 句柄。
    pub fn new(app: &'a AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> FileGateway for PluginFiles<'_, R> {
    fn open(&self, target: &FilePath, opts: OpenOptions) -> std::io::Result<std::fs::File> {
        // 用 `try_state` 而不是 `FsExt::fs()`：后者在插件未注册时 **panic**。
        // 插件今天确实在 `lib.rs` 上无门控注册着，但「没注册」必须自曝成一条能读的错误，
        // 而不是把一次导出变成一次崩溃 —— 顺带也让这条腿在裁掉插件后当场说话。
        //
        // 🔴 射程：这一句只挡「插件没注册」。URI 支真正的崩溃点在上游
        // （`android.rs:77-84` 拿不到 fd 时 `unimplemented!()`，见模块文档射程自曝 6），
        // 它挡不住，兜它的是 [`with_picked_gateway`] 的 `spawn_blocking`。
        let fs = self
            .app
            .try_state::<tauri_plugin_fs::Fs<R>>()
            .ok_or_else(|| {
                std::io::Error::other("tauri-plugin-fs 未注册：content URI 无法读写".to_string())
            })?;
        fs.inner().open(target.clone(), opts)
    }
}

/// 用户选中的目标落在哪一侧。
#[derive(Debug, Clone)]
pub enum PickedTarget {
    /// 有本机文件系统路径（桌面全部情形；Android 上少数直接给路径的选择器也走这里）。
    Path(PathBuf),
    /// 只有 URI，没有路径（Android SAF 的 `content:`）。
    Uri(FilePath),
}

// `FilePath` 自己不实现 `PartialEq`（上游 crate 的类型，本仓改不了），故手写一份：
// URI 侧按**原文串**比。判据要能写 `assert_eq!(classify(x), PickedTarget::Path(p))`，
// 而它正是「桌面仍走原路」那条负侧断言的形态；没有它就只剩 `matches!`，
// 而 `matches!` 分不出「转成了哪个路径」——转成另一个路径同样会绿。
impl PartialEq for PickedTarget {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Path(left), Self::Path(right)) => left == right,
            (Self::Uri(left), Self::Uri(right)) => left.to_string() == right.to_string(),
            _ => false,
        }
    }
}

impl Eq for PickedTarget {}

/// 判定目标形态。**唯一口径**：[`FilePath`] 的变体。
///
/// 刻意不按 `into_path()` 成不成判（本批复审推翻的第一版，理由见模块文档）：那会把**所有**
/// `file:` URL 送去 `std::fs` —— 桌面上看不出差别，iOS 上却正好绕开 `Fs::open` 那条
/// security-scope 支，而那一支是 `tauri-plugin-fs` 的 `ios.rs` 存在的唯一理由。
///
/// 也刻意不按 `scheme == "content"` 判：那会把 `content:` 之外的任何非 `file:` scheme
/// （将来的新形态）重新丢回「转不出路径 ⇒ 当成取消」那条老路上。
pub fn classify(target: &FilePath) -> PickedTarget {
    match target {
        FilePath::Path(path) => PickedTarget::Path(path.clone()),
        FilePath::Url(_) => PickedTarget::Uri(target.clone()),
    }
}

/// 回给前端的目标标识。
///
/// 路径侧**逐字保持**改动前的 `path.to_string_lossy()`（桌面回归面，桌面恒走这一支）；
/// URI 侧给 URI 原文 —— 那是这个目标在 Android 上唯一能被重新打开的句柄，
/// `backup_import_apply` 拿它回来二次打开靠的就是它。
///
/// 两侧必须与 [`classify`] 同口径：回执字符串经前端原样绕一圈再 `FilePath::from_str` 回来，
/// 若这里把 `file:` URL 降解成路径、而 I/O 侧按变体分派，那条往返就会**换一条腿**落地
/// （iOS 上等于丢掉 security scope）。代价是 iOS 的「已保存到 X」会显示成百分号编码的
/// `file:` URL 而不是路径 —— 如实登记：那是显示层的取舍，不是本函数能两全的事。
pub fn display_of(target: &FilePath) -> String {
    match classify(target) {
        PickedTarget::Path(path) => path.to_string_lossy().into_owned(),
        PickedTarget::Uri(uri) => uri.to_string(),
    }
}

/// 只读打开选项。
fn read_opts() -> OpenOptions {
    let mut opts = OpenOptions::new();
    opts.read(true);
    opts
}

/// 覆盖写打开选项。
///
/// `create` 在 Android 侧不参与（`OpenOptions::android_mode` 只映射 r/w/t/a，
/// 见 `tauri-plugin-fs-2.5.1/src/lib.rs:306-322`）——SAF 的 `ACTION_CREATE_DOCUMENT`
/// 在返回 URI 之前就已经把文档建好了。它在这里是给桌面侧那条 `std::fs::OpenOptions`
/// 转换用的，两侧共用一份选项不必分叉。
fn overwrite_opts() -> OpenOptions {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    opts
}

/// 把 `bytes` 写进用户选中的目标。
pub fn write_picked(
    gateway: &dyn FileGateway,
    target: &FilePath,
    bytes: &[u8],
) -> std::io::Result<()> {
    match classify(target) {
        // 桌面原路：与改动前逐字相同的一次 `std::fs::write`。
        PickedTarget::Path(path) => std::fs::write(path, bytes),
        PickedTarget::Uri(uri) => {
            let mut file = gateway.open(&uri, overwrite_opts())?;
            // 这里**刻意不再 `flush()`**（本批第一版有，注释还写成「显式落盘一次」——那是假的）：
            // `std::fs::File` 不做用户态缓冲，它的 `Write::flush` 在 Unix / Windows 上是**空操作**
            // （std 文档原话：“Since a File structure doesn't contain any buffers, this function is
            // currently a no-op”）；本机 strace 实测 `write_all` 之后到进程下一次输出之间零系统调用。
            // 留着那行只会让下一个读者以为耐久性已经处理过了。
            //
            // 也刻意**不**换成 `sync_all()`：SAF 的 fd 在云盘类 DocumentsProvider 上是
            // `openPipeHelper` 造的管道，对管道 `fsync` 返回 `EINVAL` —— 那会把一次**写成功**的
            // 导出报成失败。真正的交付点是 fd 关闭（`File` drop）时把它交还 provider。
            file.write_all(bytes)
        }
    }
}

/// [`stream_into_picked`] 的失败：**连同失败前已经灌进目标的字节数**一起交回。
///
/// 只回一个 `io::Error` 会把「目标上现在留着多少字节」这件事丢掉，而在没有 rename 的 URI 支上，
/// 那正是调用方唯一能如实告诉用户的东西（目标上留下的是半截，半截有多长）。
#[derive(Debug)]
pub struct StreamIntoPickedError {
    /// 失败前目标 fd 已经**接收**的字节数（`write` 返回 `Ok(n)` 的累计），不是源里读出的字节数。
    /// 打开目标就失败时为 0。
    pub written: u64,
    pub error: std::io::Error,
}

impl std::fmt::Display for StreamIntoPickedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (after {} bytes written)", self.error, self.written)
    }
}

/// 给目标写者记账：累计 `write` 真正接收的字节数。
///
/// `std::io::copy` 失败时只交回错误、不交回已拷字节数，故计数必须挂在**目标那一侧** ——
/// 挂在源那一侧数的是「读出来了多少」，读出来的一块可能只写进去一半。
struct CountingWriter<'a> {
    inner: &'a mut dyn std::io::Write,
    written: u64,
}

impl std::io::Write for CountingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// `source` → `dst` 流式拷贝，失败时带回已被 `dst` 接收的字节数。
fn copy_counted(
    source: &mut dyn std::io::Read,
    dst: &mut dyn std::io::Write,
) -> Result<u64, StreamIntoPickedError> {
    let mut counting = CountingWriter {
        inner: dst,
        written: 0,
    };
    match std::io::copy(source, &mut counting) {
        Ok(_) => Ok(counting.written),
        Err(error) => Err(StreamIntoPickedError {
            written: counting.written,
            error,
        }),
    }
}

/// 把 `source` 里的字节**流式**灌进用户选中的目标，返回写出的字节数。
///
/// # 为什么不用 [`write_picked`]
///
/// 那一支吃 `&[u8]`：备份 JSON 与日志归档都是一次性攒在内存里的整块文本，那个形态是对的。
/// Taildrop 取件不是 —— 收件箱里躺着的可能是几个 GB 的文件，把它整份读进内存再交出去，
/// 峰值内存就等于文件大小，在手机上是一次必然的 OOM。
///
/// 两侧都用 [`std::io::copy`]（64 KiB 缓冲）：
///  · 路径侧：一次 `File::create` + copy —— 与 [`write_picked`] 的桌面支同语义，只是不吞内存；
///  · URI 侧：经网关拿写者（[`FileGateway::open_for_write`]）再 copy，同样**不** `flush()` /
///    `sync_all()`（理由逐字见 [`write_picked`]：`std::fs::File` 无用户态缓冲，而 SAF 的管道 fd
///    对 `fsync` 返 `EINVAL`，会把一次写成功报成失败）。真正的交付点是 fd 关闭时把它交还 provider。
///
/// 返回的字节数（成功与失败两侧）一律是**目标接收的**字节数（见 [`StreamIntoPickedError::written`]）。
///
/// ⚠️ **这条腿不提供「要么没有、要么完整」**：`copy` 中途失败会在目标上留下半截。
/// 需要那条保证的调用方要自己在**本机**侧先落一个完整的临时文件，再拿它喂本函数
/// （`taildrop_save` 的 URI 支就是这么做的，那里逐字写清了这条保证在 SAF 上退化成什么）。
pub fn stream_into_picked(
    gateway: &dyn FileGateway,
    target: &FilePath,
    source: &mut dyn std::io::Read,
) -> Result<u64, StreamIntoPickedError> {
    let opened = |error| StreamIntoPickedError { written: 0, error };
    match classify(target) {
        PickedTarget::Path(path) => {
            let mut file = std::fs::File::create(path).map_err(opened)?;
            copy_counted(source, &mut file)
        }
        PickedTarget::Uri(uri) => {
            let mut writer = gateway
                .open_for_write(&uri, overwrite_opts())
                .map_err(opened)?;
            copy_counted(source, &mut *writer)
        }
    }
}

/// 读用户选中的目标（UTF-8 全文）。
pub fn read_picked_to_string(
    gateway: &dyn FileGateway,
    target: &FilePath,
) -> std::io::Result<String> {
    match classify(target) {
        // 桌面原路：与改动前逐字相同的一次 `std::fs::read_to_string`。
        PickedTarget::Path(path) => std::fs::read_to_string(path),
        PickedTarget::Uri(uri) => {
            let mut buf = String::new();
            gateway.open(&uri, read_opts())?.read_to_string(&mut buf)?;
            Ok(buf)
        }
    }
}

/// 打开用户选中的目标供**流式**读取，最多再多读一个字节用于判超限。
///
/// 给 `subscription::local_import_pick_file` 用：它有 10MB 上限、有超时，且明确不肯
/// 相信 `metadata().len()`（元数据会与写者赛跑）。故这里只交句柄，读多少由调用方定。
pub fn open_picked_for_read(
    gateway: &dyn FileGateway,
    target: &FilePath,
) -> std::io::Result<std::fs::File> {
    match classify(target) {
        // 桌面原路：与改动前的 `tokio::fs::File::open` 打开的是同一个东西
        //（调用方随后 `tokio::fs::File::from_std` 包回去，异步读腿一字未改）。
        PickedTarget::Path(path) => std::fs::File::open(path),
        PickedTarget::Uri(uri) => gateway.open(&uri, read_opts()),
    }
}

/// 目标的展示名（basename）。
///
/// 路径侧取文件名 —— 与改动前逐字相同。URI 侧取路径段的最后一节：SAF 的 document id
/// 常常不是人可读的文件名（`…/document/msf%3A42` 这种），故这个值**只当展示用**，
/// 取不到就空串，绝不拿它去拼路径、也不做百分号解码（那需要一个本仓没有的依赖，
/// 而多解一层只会把 `%3A` 变成 `:`，读起来并不更像文件名）。
pub fn file_name_of(target: &FilePath) -> String {
    match classify(target) {
        PickedTarget::Path(path) => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        PickedTarget::Uri(FilePath::Url(url)) => url
            .path_segments()
            .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
            .unwrap_or_default()
            .to_owned(),
        // `classify` 判成 URI 的 `Path` 变体**按构造不存在**（它只把 `FilePath::Url` 判成 URI），
        // 这一臂只是把 match 补穷尽，不是一条真实路径。
        PickedTarget::Uri(FilePath::Path(path)) => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// 拿一个生产网关跑一段文件 I/O：**URI 形态派到阻塞线程池，路径形态原地跑**。
///
/// # 为什么必须有这一层（两件事，都不是风格问题）
///
/// 1. **不许挂住 tokio worker。** URI 支的 [`FileGateway::open`] 在 Android 上是一次同步 JNI
///    往返，随后的整文件读写还可能落在云盘 provider 的管道上（模块文档射程自曝 5）。本仓的
///    文件腿全是 `#[tauri::command] async fn`，直接调等于用一次导出占住一个 worker 到远端收完。
///    `subscription::local_import_pick_file` 先落地了这条不变量（判据在
///    `subscription/tests/wiring_gate.rs`），本函数是它在其余四条腿上的**同一形态** ——
///    一条腿做对、四条腿不做，那不是取舍，是漏了。
/// 2. **不许让 panic 变成一个永不 settle 的 promise。** 上游那句 `unimplemented!()`
///    （射程自曝 6）若在 async command 自己的 task 里炸开，`resolver.respond` 不会执行：前端的
///    `await` 永远挂着，写在 `finally` 里的 `setBusy(false)` 也永远不跑 ⇒ 按钮永久禁用、
///    全程零报错 —— 正是 W-18 要消灭的那类静默。跑在 `spawn_blocking` 里它收成一个
///    `JoinError`，调用方翻成稳定错误码，命令照常回话。
///
/// # 为什么路径形态**不**派
///
/// 桌面那条路要「一行不变」，连它跑在哪个线程上都不改（改动前就是在 command 的 task 上同步
/// `std::fs::write`）。这一层只给本批新增的那一支兜底，不顺手改桌面的调度。
/// `picked` 为 `None`（用户取消）同理原地跑：那条路根本碰不到网关。
pub async fn with_picked_gateway<R, T, F>(
    app: &AppHandle<R>,
    picked: Option<FilePath>,
    work: F,
) -> Result<T, tokio::task::JoinError>
where
    R: Runtime,
    T: Send + 'static,
    F: FnOnce(&dyn FileGateway, Option<FilePath>) -> T + Send + 'static,
{
    match picked.as_ref().map(classify) {
        Some(PickedTarget::Uri(_)) => {
            let app = app.clone();
            tokio::task::spawn_blocking(move || work(&PluginFiles::new(&app), picked)).await
        }
        _ => Ok(work(&PluginFiles::new(app), picked)),
    }
}

// `pub(crate)`：往返门的记账网关替身住在这里，调用点侧的两组（`misc/logs/tests`、
// `misc/backup/tests`）复用同一个替身。三处各造一份替身 = 三份会各自漂移的判据。
// cfg 门仍在，测试专用符号照旧进不了生产构建。
#[cfg(test)]
pub(crate) mod tests;
