//! 测试专用夹具：临时目录守卫 + **源码级门的取材锚点**。
//!
//! # 临时目录守卫（[`TestDir`]）
//!
//! 手工在测试尾部 `remove_dir_all` 遇到 `assert!` / `#[should_panic]` 时必然失效；本守卫把清理
//! 绑定到栈展开，并保留 `PathBuf` 的调用手感，避免每个测试模块各写一份不完整生命周期。
//!
//! # 取材锚点（[`crate_source`] / [`crate_file`] / [`repo_file`] / [`module_source`]）
//!
//! 本 crate 的源码级门此前一律用 `include_str!` 取材，而 `include_str!` 的相对路径锚在
//! **含它的那个文件**：测试实体从 `foo.rs` 搬进 `foo/tests/mod.rs` 的那一刻，143 处锚点全部
//! 平移一层，失败模式是「解析到另一个真实存在的文件 ⇒ 编译通过、门继续绿、扫的却是别的东西」。
//!
//! 下面四个 wrapper 把锚点换成 `CARGO_MANIFEST_DIR`（crate 根，不随测试位置动）。**`env!` 必须
//! 写在本 crate 里**——写进 `polaris-source-probe` 内部就会解析成 `crates/source-probe`，
//! 全部取材当场跑偏；实现单点在那个 crate，锚点单点在这里。语义与边界见该 crate 的模块文档。

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TestDir(PathBuf);

impl TestDir {
    pub(crate) fn new(prefix: &str) -> Self {
        Self::new_in(prefix, &std::env::temp_dir())
    }

    pub(crate) fn new_in(prefix: &str, parent: &Path) -> Self {
        assert!(
            !prefix.is_empty() && !prefix.contains(['/', '\\']),
            "测试临时目录前缀必须是单个安全路径段"
        );
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}{}-{nonce}-{sequence}", std::process::id()));
        std::fs::create_dir(&path).expect("测试临时目录必须唯一且可创建");
        let dir = Self(path);
        // Resolve Unix temp aliases such as macOS /var only for our owned root.
        // Windows canonicalize adds a verbatim prefix, which breaks file URLs
        // and rule-file paths assembled with '/'; keep the created normal path.
        // Artifact/source readers retain their own no-follow component checks.
        #[cfg(unix)]
        let dir = {
            let mut dir = dir;
            dir.0 = std::fs::canonicalize(&dir.0).expect("测试自有临时目录必须可解析为真实路径");
            dir
        };
        dir
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Deref for TestDir {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ── 源码级门的取材锚点 ────────────────────────────────────────────────────
//
// 这几个 wrapper 都只做一件事：把**本 crate** 的 `CARGO_MANIFEST_DIR` 传给单点实现。

/// `src-tauri/src/<rel>` 的全文。取代 `include_str!("tray.rs")` / `include_str!("runtime/stats.rs")`。
pub(crate) fn crate_source(rel: &str) -> String {
    polaris_source_probe::crate_source_in(env!("CARGO_MANIFEST_DIR"), rel)
}

/// `src-tauri/<rel>` 的全文（`Cargo.toml` / `tauri.conf.json` / `capabilities/*.json`）。
/// 取代 `include_str!("../Cargo.toml")`。
pub(crate) fn crate_file(rel: &str) -> String {
    polaris_source_probe::crate_file_in(env!("CARGO_MANIFEST_DIR"), rel)
}

/// 仓库根下 `<rel>` 的全文（`ui/**` / `scripts/**`）。取代
/// `include_str!("../../../../scripts/verify-packaging.mjs")` —— 那串 `../` 的**个数**本身
/// 就是随调用方深度变化的锚点。
pub(crate) fn repo_file(rel: &str) -> String {
    polaris_source_probe::repo_file_in(env!("CARGO_MANIFEST_DIR"), rel)
}

/// 仓库根下 `<dir_rel>` 里全部 `.<extension>` 文件（递归，跳过依赖缓存 / 构建产物 / 隐藏目录），
/// 按**仓库内路径**升序返回 `(路径, 全文)`。
///
/// 相对逐个 [`repo_file`] 列清单的优势与 [`module_source`] 那条一样：**新增文件自动进取材面**。
/// 跨语言门此前把被扫的入口写死成三行文件名，于是「`ui/` 下多了一个 renderer 入口」这件事
/// 门看不见、也不报错 —— 覆盖面由判据定，不由夹具定。
pub(crate) fn repo_dir_files(dir_rel: &str, extension: &str) -> Vec<(String, String)> {
    polaris_source_probe::repo_dir_files_in(env!("CARGO_MANIFEST_DIR"), dir_rel, extension)
}

/// `src-tauri/<rel>` 的**原始字节**（`icons/*.png` 等随包二进制资产）。
/// 取代 `include_bytes!("../../../icons/tray-on-black.png")` —— 那串 `..` 的个数随测试位置变。
pub(crate) fn crate_bytes(rel: &str) -> Vec<u8> {
    polaris_source_probe::crate_bytes_in(env!("CARGO_MANIFEST_DIR"), rel)
}

/// `src-tauri/src/<dir_rel>` 下全部生产 `.rs`（递归，排除 `tests/`）的拼接。
///
/// 相对逐个 `include_str!` 列清单的优势是**新增子模块自动进取材面**——手写清单的门在
/// 「有人新加了一个文件」时静默失去覆盖，而这是本仓已经吃过的亏。
pub(crate) fn module_source(dir_rel: &str) -> String {
    polaris_source_probe::module_source_in(env!("CARGO_MANIFEST_DIR"), dir_rel)
}

/// 取材面的**字面量面**：注释按字节抹成空格（偏移与行号不变），字符串/字符字面量原样保留。
///
/// 实现单点在 `polaris-source-probe`（与 `crates/system-integration` 的 `literal_face` 同一份），
/// 射程与已知边界见该函数的文档。[`module_code`] / [`crate_code`] 是它的两个取材形态。
pub(crate) use polaris_source_probe::mask_comments as literal_face;

/// **HTML 面**的取材净化：只剥 `<!-- -->` 注释，属性值原样保留。
///
/// 与 [`literal_face`] 不可互换 —— 定界符不同（`<!-- -->` vs `//` / `/* */`），且 HTML 里的
/// 「字符串」就是属性值本身（CSP 策略整条住在 `content="…"` 里），连它一起抹判据就没有对象了。
/// 射程与已知边界见 `polaris_source_probe::mask_html_comments` 的文档。
pub(crate) use polaris_source_probe::mask_html_comments;

/// [`module_source`] 的**剥注释**形态（[`literal_face`]）。
///
/// # 什么时候必须用它
///
/// 判据是**全文正面** `contains` / `matches().count()` 时。这类腿的针只要在注释里出现过一次，
/// 就有一份与生产调用点**无关**的证据源：生产侧删光、注释留着 ⇒ 门照绿。本仓实测：
/// `tray.rs:25` 的模块文档写着 `window::TRAY_IDLE_RECLAIM_SECS`，把常量的全部代码位改名，
/// `overlay_lifecycle_gate.rs` 那条正面断言仍然绿。
///
/// # 射程（如实登记）
///
/// **剥**：行注释 `//`（含 `///` / `//!`）—— **行首与行尾同等对待**；块注释 `/* */`（含嵌套），
/// 行中间起笔的也剥。**保留**：字符串 / 原始字符串 / 字符与字节字面量，故字面量内部的 `//`
/// （`"http://a // b"`）不会被误当成注释起笔。**不做**条件编译求值：`#[cfg(test)]` 关掉的代码
/// 仍在面上 —— 排除测试代码是 [`module_source`] 那一层的职责（它剔 `tests/`），不是这里。
///
/// # 为什么不是符号面（连字符串一起抹）
///
/// 本 crate 的消费方里有多条针**就是字符串字面量**（`"phase": "fetching"` / `"subscriptionId"` /
/// `"keepTrayMenuWarm"`）。连字符串一起抹，这些针在净化面上永远命中不到 —— 判据不是变弱，
/// 是消失。要按符号扫时用 `polaris_source_probe::mask_comments_and_strings`。
///
/// 切片型取材器（[`crate::commands::guard_scan::top_level_fn_body`] /
/// `impl_method_body` / [`method_body`]）内部还会再剥一道整行注释，幂等无副作用；
/// 但它们**只**剥整行，故喂它们的取材仍应先过这里，行尾/块注释才有人管。
pub(crate) fn module_code(dir_rel: &str) -> String {
    literal_face(&module_source(dir_rel))
}

/// [`crate_source`] 的**剥注释**形态。取舍与射程同 [`module_code`]。
pub(crate) fn crate_code(rel: &str) -> String {
    literal_face(&crate_source(rel))
}

/// **crate 根**的取材面：`main.rs` + `lib.rs` 两份拼接（各自已剥注释）。
///
/// # 为什么取材面是「两个文件」而不是其中某一个
///
/// 应用装配（18 个 `mod`、插件注册、`setup`、command 注册表、`RunEvent` 循环）已从 `main.rs`
/// 下沉进 `lib.rs`（移动端加载的是 cdylib、根本没有 `main()`，理由见 `lib.rs` 模块文档），
/// `main.rs` 收成薄壳。把取材写死成其中任何**一个**文件，都会在下一次同类搬迁时静默失效 ——
/// 这正是本仓吃过一次的亏，[`module_source`] 的文档记着原话：「测试实体从 `foo.rs` 搬进
/// `foo/tests/mod.rs` 的那一刻，143 处锚点全部平移一层，失败模式是**解析到另一个真实存在的
/// 文件** ⇒ 编译通过、门继续绿、扫的却是别的东西」。crate 根横跨 `main.rs` 与 `lib.rs`，
/// 与「模块横跨 `foo.rs` 与 `foo/`」是同一件事的另一个面，修法也同款：取整个面。
///
/// [不选「逐处改成 `crate_code("lib.rs")`」：改动确实更小，但那是把「装配住在哪个文件」这个
///  **实现细节**重新焊进十来处判据里，下一次搬迁再来一遍；而且负面断言（「不许出现 X」）
///  取到空文件时恒真 —— 失效方向是静默的]
///
/// # 顺序断言为什么仍然成立
///
/// 拼接序固定 `main.rs` 在前。薄壳里只剩 `windows_subsystem` 属性与一句 `run()`，不含任何
/// 判据锚点 ⇒ 全部 `find(A) < find(B)` 的相对位置与只取 `lib.rs` 时逐字节相同。哪天薄壳里
/// 真长出与锚点同名的文本，[`crate::commands::guard_scan::top_level_fn_body`] 的「恰好命中
/// 一次」是硬断言（不是取第一处），会当场把这件事报出来。
pub(crate) fn crate_root_code() -> String {
    // 中间垫一个换行：`crate_source` 返回的是文件全文、末尾未必带换行，首尾直接相接会把薄壳
    // 最后一行与 `lib.rs` 第一行粘成同一行 —— 按行工作的取材器（整行注释剥离）会跟着错一行。
    format!("{}\n{}", crate_code("main.rs"), crate_code("lib.rs"))
}

/// 逐文件形态，用于让失败信息说得出「哪个文件」。
pub(crate) fn module_files(dir_rel: &str) -> Vec<(String, String)> {
    polaris_source_probe::module_files_in(env!("CARGO_MANIFEST_DIR"), dir_rel)
}

pub(crate) use polaris_source_probe::expect_marker;

// ── 假管道回压（核腿的「灌满 stderr」行为门共用）────────────────────────────

/// 假 stderr 的内存管道容量：取 **64 KiB = Linux 匿名管道默认容量**（`F_GETPIPE_SZ` 实测值，与
/// `man 7 pipe` 官方值一致）。取这个数是为了让「没人读就写不动」在内存里与真管道**逐字同构** ——
/// 2026-09-02 测速临时核卡死的缺陷本体就是这个回压。
pub(crate) const FAKE_PIPE_CAPACITY: usize = 64 * 1024;

/// 假核往 stderr 灌多少字节（1 MiB = 管道容量的 16 倍）。没人排空时它必然在 64 KiB 处永久卡住。
pub(crate) const STDERR_FLOOD_BYTES: usize = 1024 * 1024;

/// 造一条**会把自己写堵死**的假 stderr：返回读端，写手在后台逐行灌 `total` 字节，已写字节数经
/// `progress` 播报。
///
/// # 它为什么是判据本身，不是脚手架
///
/// 「排空真的发生了」这件事没法靠源码级 grep 证明（词法判据看得见文本、看不见谁在读）。能证明它的
/// 只有回压：管道满了之后写手推不动，`progress` 就停在容量附近。故灌的量必须**远大于**容量
/// （[`STDERR_FLOOD_BYTES`] 是 16 倍），否则「全写完了」也可能只是因为压根没越过容量 —— 那种绿
/// 没有信息量。
///
/// 三条核腿共用一份：它们共用同一个 spawner、同一份排空实现（`proxy::core_log::pipe_to_log`），
/// 缺陷形态完全同构。各写一份的代价不是重复几行，而是两份会漂 —— 容量取值一旦分叉，其中一份的
/// 「绿」就悄悄变成「没越过容量」。
pub(crate) fn flooding_stderr(
    total: usize,
    progress: tokio::sync::watch::Sender<usize>,
) -> polaris_core_supervisor::ChildStream {
    let (mut writer, reader) = tokio::io::duplex(FAKE_PIPE_CAPACITY);
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        // 逐行灌（行长 1 KiB，与真核那种「每节点几行」的形态同构）。**没人读就会在
        // `FAKE_PIPE_CAPACITY` 处永久阻塞** —— 这正是要被排空接线消掉的那个回压。
        let mut line = vec![b'x'; 1023];
        line.push(b'\n');
        let mut sent = 0usize;
        while sent < total {
            if writer.write_all(&line).await.is_err() {
                return; // 读端已关（= 没人排空 / 会话已收尾）→ 写手就此打住，进度停在此处
            }
            sent += line.len();
            let _ = progress.send(sent);
        }
    });
    Box::new(reader)
}

// ── 子进程探针 ────────────────────────────────────────────────────────────

/// 探针的睡眠时长。见证文件在睡满之后才写，故它同时是「等多久才敢说文件不会出现了」的下界。
#[cfg(unix)]
pub(crate) const PROBE_SLEEP_MILLIS: u64 = 400;

/// 写一个冒充内核子命令的 shell 探针：先睡 [`PROBE_SLEEP_MILLIS`]，**然后**建见证文件、rc=0。
/// 返回脚本路径，直接当二进制路径传给被测函数。
///
/// # 为什么是脚本，而不是一个 Rust bin 目标
///
/// `core-supervisor` 里有一个同款的 Rust 探针（`src/bin/check_probe.rs`），三平台恒在；但
/// `CARGO_BIN_EXE_*` 只在**同一个包**的集成测试里由 cargo 注入，本包（`polaris`）拿不到它。
/// 而本包这两条腿要验的恰恰是「子进程真的起起来了、超时之后真的被杀掉」，只能真起一个进程。
/// 脚本是最小实现：不新增 bin 目标（本包的 bin 会进发布产物）、不引依赖、不写任何平台专属代码。
///
/// 只有 unix 一版：Windows 上没有同样零依赖的等价物（批处理要么得靠 `timeout` 拿控制台、要么
/// 得 `ping` 回环，后者触碰宿主网络）。跨平台的那一半——超时与 `kill_on_drop` 本身——由
/// `crates/core-supervisor/tests/config_gate_process.rs` 用 Rust 探针在三平台各跑一遍；本处只验
/// 「本包这两个调用点确实走了那条共用实现」，而这件事与平台无关。
///
/// # 见证文件凭什么能证明「子进程死了」
///
/// 它在睡满之后才写。超时短于睡眠 ⇒ 文件永不出现 ⇒ 进程确实没跑完；超时长于睡眠 ⇒ 文件出现
/// （正向对照：证明这条腿本身是活的，不是路径压根没传对所以永远没有文件）。不去扫进程表，
/// 是因为那要按平台各写一套，且在 CI 容器里未必看得见。
///
/// 探针**完全忽略 argv**：调用方发的是固定的一串参数，脚本一个都不需要读。
#[cfg(unix)]
pub(crate) fn write_sleeping_probe(dir: &Path, witness: &Path) -> PathBuf {
    let script = dir.join("sleeping-probe.sh");
    write_executable_stand_in(&script, sleeping_probe_script(witness));
    script
}

/// [`write_sleeping_probe`] 的脚本正文。要在探针里再插一行的测试取它改写后自己落盘，
/// 不得先落盘再覆写（覆写同样会在本进程里开出写句柄，见 [`write_executable_stand_in`]）。
#[cfg(unix)]
pub(crate) fn sleeping_probe_script(witness: &Path) -> String {
    let seconds = PROBE_SLEEP_MILLIS as f64 / 1000.0;
    format!("#!/bin/sh\nsleep {seconds}\n: > '{}'\n", witness.display())
}

/// 落一个**随后要被直接执行**的替身文件（0o755）。测试里凡是「写出一个文件再把它当程序起」
/// 都必须走这里，`src-tauri/tests/executable_stand_in_discipline.rs` 在源码层面强制。
///
/// # 它消除的竞态
///
/// `execve` 对一个**仍有写句柄开着**的文件返回 `ETXTBSY`。「写完、关掉、再执行」在单线程里
/// 没有问题，但测试进程是多线程的：本线程持有写句柄的那一小段时间里，别的测试线程若正好
/// `fork` 出子进程，那个子进程会**继承**这枚写句柄，直到它自己 `exec`（`O_CLOEXEC` 这时才
/// 生效）或退出为止。本线程随后关掉自己的那一份并去执行文件，内核看到的写句柄计数仍不为零。
/// 这扇窗与本线程做了什么无关，只取决于旁边的线程何时 `fork`，所以表现为高负载下的偶发失败。
///
/// # 为什么这样写就没有了
///
/// 目标文件的写句柄从头到尾**只存在于一个专门起的 `cp` 子进程里**：本进程把内容写进旁边的
/// 暂存文件（它从不被执行，有没有人继承它的写句柄都无所谓），再让 `cp` 去打开、写入、关闭
/// 目标文件。本进程从未打开过目标文件，于是旁边的线程无论何时 `fork`，都没有它的写句柄可以
/// 继承；`cp` 自己不是多线程的，它退出（下面 `status` 等到）时写句柄计数归零，此后再没有人
/// 以写方式打开它。加执行位用的是按路径的 `chmod`，不开文件。
///
/// 不是重试：重试只是把撞窗的概率再乘一次，而且会把「替身真的起不来」也一并吞掉。
///
/// # 平台
///
/// 只有 unix 一版。替身是 shell 脚本，调用点本就全部 `#[cfg(unix)]`；`cp` 与这些脚本里用到的
/// `sleep` / `touch` 一样从 `PATH` 取，没有引入新的前提。macOS 的 `execve` 同样返回
/// `ETXTBSY`，机制与修法一致。Windows 没有这类调用点：那里的对应故障是「映像文件正被占用」，
/// 而 std 打开的句柄默认不可继承，别的线程起子进程带不走它。
///
/// 已有内容的可执行文件（例如把真核复制到另一路径）同样走这里：读出字节再交给它，
/// 不要用 `std::fs::copy` —— 那也是在本进程里开目标文件的写句柄。
#[cfg(unix)]
pub(crate) fn write_executable_stand_in(path: &Path, contents: impl AsRef<[u8]>) {
    use std::os::unix::fs::PermissionsExt;

    let mut staged = path.as_os_str().to_owned();
    staged.push(".staged");
    let staged = PathBuf::from(staged);
    std::fs::write(&staged, contents).expect("替身暂存文件必须可写");
    let status = std::process::Command::new("cp")
        .arg(&staged)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .status()
        .expect("替身写入子进程必须起得来");
    assert!(status.success(), "替身写入子进程失败：{status}");
    std::fs::remove_file(&staged).expect("替身暂存文件必须可删");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("替身必须可加执行位");
}

/// 把一条真起 `sing-box check` 探针的测试原样放进**独占子进程**里跑；返回 `true` 表示本进程是
/// 父进程、子进程已跑完并通过。`module` 传调用处的 `module_path!()`，`name` 传测试函数名。
///
/// 这些测试经 `run_check_raw` 记进**进程级**那本 check 账，与同进程里并行的原生起核测试共用同一道
/// 完整准入。超时那几条在「请求关闭 → 原生 wait → 两个读端返回 → 快照清理」之间背的是**真债**
/// （探针的 `sleep` 继承着管道，读端要等它睡完才 EOF），这段时间里完整准入按设计一律拒绝 ——
/// 旁边那些直接起核的测试于是成批拿到 cleanup debt。债是对的，要隔开的是账本：不串行、不另造
/// 测试专用账本、不绕准入，子进程里跑的仍是调用处逐字未动的测试体。
///
/// 子进程必须**恰好跑了这一条且通过**：`--exact` 过滤成 0 条时退出码同样是 0，只看退出码会把
/// 「什么都没跑」当成通过。
#[cfg(all(test, unix))]
pub(crate) fn ran_in_isolated_worker(module: &str, name: &str) -> bool {
    const MARKER: &str = "POLARIS_ISOLATED_CHECK_WORKER";
    let test = format!("{}::{name}", module.split_once("::").unwrap().1);
    if std::env::var(MARKER).is_ok_and(|bound| bound == test) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test, "--nocapture"])
        .env(MARKER, &test)
        .env("POLARIS_NO_KERNEL_RUN", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed; 0 failed"),
        "独占子进程没有恰好跑过 `{test}`（{}）：\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

/// 按花括号配对取**方法体**（`impl` 块里的方法也截得准）。
///
/// [`crate::commands::guard_scan::top_level_fn_body`] 只认**列 0** 的 `\n}\n` 作结束锚 —— 对
/// `impl` 块里的方法不成立：切片会一路吃到整个 `impl` 结束，把后续所有方法都囊括进来，于是
/// 「把被守的调用从这个方法删掉、在同一个 impl 的下一个方法里加一句」就能骗过顺序守卫。
/// 本函数精确截到该方法**自己**的闭合括号，并跳过字符串字面量与 `//` 行注释（否则
/// `format!("{x}")`、注释里的括号都会算进深度）。
///
/// **已知边界**：不处理块注释 `/* */` 与裸字符字面量 `'{'`（本仓被守的几个函数体里都没有）。
///
/// 取方法体源码（从签名锚点起，到与签名后第一个 `{` 配对的 `}` 止，含两端），
/// **整行注释已剥**（与 `commands::guard_scan::{top_level_fn_body, impl_method_body}` 同口径）。
///
/// 锚点缺失 / 括号不配对一律 panic —— 守卫**失去判据时必须转红**，而不是静默退化成
/// 「扫了个空串、断言恒真」。
///
/// # 为什么必须剥注释（本函数此前只在**配对深度**上跳过注释，返回的文本仍含注释）
///
/// 跳过注释里的花括号只解决「切到哪里」，不解决「切出来的文本喂谁」。消费者全是
/// `contains` / `find` / `matches().count()` 型判据 ⇒ 方法体内注释里的同名文本会**替生产
/// 调用点作证**：实测 `runtime/stats/gate.rs::spawn_visibility_refresh` 的体内注释写着
/// 「主线程调用时 `run_on_main_thread` 内联执行该闭包」，把生产的
/// `app.run_on_main_thread(...)` 整段删掉，`stats/tests/mod.rs` 那条正面断言照样绿。
///
/// 剥法与 [`crate::commands::guard_scan::strip_line_comments`] 共用一份实现（整行注释换成
/// 空行，保留行数与行序）⇒ `find()` 比大小的顺序断言语义不变。行尾注释不剥，理由见那边的
/// doc（要剥就得先分辨字符串字面量里的 `//`）。
///
/// 返回 `String` 而非 `&'a str`：剥注释必然产生新串。调用点全是取完即断言，无借用需求。
pub(crate) fn method_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("锚点消失，守卫已失去判据: {signature}"));
    let rest = &src[start..];
    let open = rest
        .find('{')
        .unwrap_or_else(|| panic!("{signature} 之后找不到左花括号（守卫已失去判据）"));
    // 逐字节扫：UTF-8 续字节恒 >= 0x80，绝不会与下面这几个 ASCII 记号相等，故按字节比较安全。
    let bytes = rest.as_bytes();
    let (mut depth, mut i) = (0usize, open);
    let (mut in_str, mut in_line_comment) = (false, false);
    while i < bytes.len() {
        let c = bytes[i];
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
        } else if in_str {
            match c {
                b'\\' => i += 1, // 跳过被转义的那个字节
                b'"' => in_str = false,
                _ => {}
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    in_line_comment = true;
                    i += 1;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        // `}` 是单字节 ASCII ⇒ i+1 必是 char 边界
                        return crate::commands::guard_scan::strip_line_comments(&rest[..=i]);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    panic!("{signature} 的花括号不配对（守卫已失去判据）")
}

#[cfg(test)]
mod tests;
