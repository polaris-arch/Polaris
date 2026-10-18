//! 启动期 stale-core 清扫 —— 父进程（Polaris）异常退出时，其 spawn 的 sing-box 子进程会存活
//! （§K2 边界声明：`Child` 无 `kill_on_drop`，panic 时 `rt.stop()` 不执行 → 孤儿核）。下次启动前
//! 须清掉这些孤儿核，否则它们占着端口/TUN 设备致新核起不来（Linux "resource busy"）。
//!
//! # 安全第一性：只杀「本 app 起的」核，且判据不依赖安装位置
//!
//! **绝不按进程名 `pkill sing-box`**：用户机器上可能装有无关的 sing-box，误杀是安全事故。
//! 判据有两条，缺一不可：命令行的**确切形状**（[`app_run_config_name`]）与进程的**属主**
//! （[`ProcessOwner`]）。形状是：
//!
//! ```text
//! <绝对路径，文件名恰为核文件名> run -c <本 app 配置目录>/<一个文件名> …
//! ```
//!
//! 四个条件缺一不可：
//! - 程序是绝对路径且文件名是核文件名 → 排除 `less <核路径> run …` 这类只是提到核的进程；
//! - 第二、三个参数恰为 `run`、`-c` → 排除 `check`、`version` 等一次性子命令；
//! - 配置文件是**本 app 配置目录的直接子项** → 用户另跑的 sing-box 用的是他自己的配置路径；
//!   另一个用户的 Polaris 用的是另一个配置目录。
//!
//! # 为什么形状之外还要看属主
//!
//! 命令行是进程自己说的，谁都能照着写一份：同机另一个用户常驻一个形状相同的进程（配置路径只是
//! 一串参数，不需要那个文件真的可读），本用户每次起核都会命中它 —— 杀不动、升级到提权清扫也
//! 碰不到，于是任何模式都起不来。故只有属主是**本用户**或 **root / SYSTEM**（提权助手起的核）
//! 的命中才是清扫对象；别的属主、以及读不到属主的，只留一行日志，不结束、不升级、不阻断起核。
//!
//! # 为什么不按「核二进制的路径」认
//!
//! 核二进制住在安装包里，而安装位置在两次会话之间并不稳定：AppImage 每次挂载到不同的
//! `/tmp/.mount_*`，macOS 转移运行时（App Translocation）给的是随机路径，升级安装也可能换目录。
//! 按路径认，上一会话留下的孤儿就永远认不出来 —— 它继续占着端口与 TUN，新核起不来。
//! 配置目录则由系统的用户数据目录约定决定，跨会话、跨安装位置不变；提权助手起的核也用同一份
//! 配置路径，故同样认得出来（杀不动的由调用方走提权清扫）。
//!
//! # 三个平台，一个判据
//!
//! 取材各不相同（Linux 读 `/proc`，macOS 读 `ps`，Windows 向系统要每个进程的命令行与令牌），
//! 判的是同一个形状、同一条属主规则。路径一律按字符串处理而不经 `std::path`：后者按**宿主**
//! 平台解析，Linux 上处理 `C:\a\b` 得到空的父目录，Windows 上不认 `/opt/x` 是绝对路径。走哪一种
//! 路径写法由**配置目录自己的写法**决定（`/` 开头即 POSIX，否则 Windows），与宿主无关，所以三种
//! 写法的样例串在任何宿主上都验得到。
//!
//! 纯判定（[`app_run_config_name`] / [`is_our_core`] / [`stale_cores`] / [`stale_pids`]）与 I/O
//! 扫描（[`scan_running_cores`]）分离：前者可零进程单测。

use std::path::Path;

#[cfg(windows)]
mod windows;

/// 扫描到的一个候选进程。
///
/// **两种载荷按平台二选一**（另一个留空），因为各平台能拿到的保真度不同：
/// - `cmdline`：argv。Linux 取自 `/proc/<pid>/cmdline`（NUL 分隔 → 无歧义）；Windows 取自进程的
///   整串命令行，按 Windows 的引号规则切开（[`split_windows_command_line`]）。
/// - `raw`：空格拼接的原始命令行（macOS `ps -axo args=`）。**ps 的输出是有损的**——argv 已被
///   空格拼死，无法还原。而 macOS 真实核路径恰恰含空格
///   （`/Library/Application Support/Polaris/core/sing-box`），按空白切分会把它劈成两段 →
///   路径全等比对必然失配 → 孤儿一个都扫不出来。故 macOS 只能按原始串匹配（同 上游 `pgrep -f`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreProcess {
    pub pid: u32,
    /// 完整命令行 argv（Linux 取自 `/proc/<pid>/cmdline`；Windows 由整串命令行切出）。macOS 为空。
    pub cmdline: Vec<String>,
    /// 原始命令行整串（macOS 取自 `ps -axo args=`）。Linux / Windows 为空。
    pub raw: String,
    /// 进程属主相对本进程的归类，扫描时与命令行一并读出。
    pub owner: ProcessOwner,
    /// 扫描时留住的进程把柄（仅 Windows 有内容，见 [`ProcessHold`]）。
    pub hold: ProcessHold,
}

/// 进程属主相对本进程的归类。
///
/// 默认值是 [`Unknown`](Self::Unknown)：没有属主事实，既不能清扫，也不能证明是其他用户。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProcessOwner {
    /// 与本进程同一用户。
    SameUser,
    /// root（Unix）或 SYSTEM（Windows）：提权助手起的核。
    Elevated,
    /// 已读到属主且能确认是其他用户。
    Other,
    /// 候选属主或本用户身份不可读，无法完成归属判定；不是清扫对象。
    #[default]
    Unknown,
}

impl ProcessOwner {
    /// 该属主的命中是否归本 app 清扫（见模块文档「为什么形状之外还要看属主」）。
    #[must_use]
    pub fn sweepable(self) -> bool {
        matches!(self, Self::SameUser | Self::Elevated)
    }
}

/// Unix 上的归类：`owner` 是候选进程的有效 uid，`me` 是本进程的；读不到即 `None`。
///
/// 先比「是不是自己」再比 root：本进程自己以 root 跑时，root 的进程就是同一用户。
#[must_use]
pub fn owner_from_uid(owner: Option<u32>, me: Option<u32>) -> ProcessOwner {
    match (owner, me) {
        (Some(owner), Some(me)) if owner == me => ProcessOwner::SameUser,
        (Some(0), _) => ProcessOwner::Elevated,
        (Some(_), Some(_)) => ProcessOwner::Other,
        _ => ProcessOwner::Unknown,
    }
}

/// Windows 上从进程令牌读到的两件事实；令牌打不开时整个没有。
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TokenUserFacts {
    /// 与本进程 SID 的比较；本进程令牌不可读时没有比较事实。
    same_user: Option<bool>,
    /// 令牌的用户 SID 是 SYSTEM（`S-1-5-18`）。
    local_system: bool,
}

/// Windows 上的归类。普通权限的进程打不开别的用户（含 SYSTEM）的令牌，那时 `facts` 是 `None`，
/// 归 [`ProcessOwner::Unknown`]：不当成自己的，也不把不可读误报成其他用户。
#[cfg(any(windows, test))]
fn owner_from_token(facts: Option<TokenUserFacts>) -> ProcessOwner {
    match facts {
        Some(TokenUserFacts {
            same_user: Some(true),
            ..
        }) => ProcessOwner::SameUser,
        Some(TokenUserFacts {
            local_system: true, ..
        }) => ProcessOwner::Elevated,
        Some(TokenUserFacts {
            same_user: Some(false),
            ..
        }) => ProcessOwner::Other,
        _ => ProcessOwner::Unknown,
    }
}

/// 扫描时留住的进程把柄。
///
/// **只在 Windows 上有内容**：一把仅带查询权限的进程句柄，连同当时读到的创建时间。句柄开着期间
/// 这个 pid 不会被系统分给别的进程，所以宽限期前后说的是同一个进程；结束时另开一把只带结束权限
/// 的句柄，创建时间对上才动手（`ProcessHold::terminate`，仅 Windows 有）。别的平台上是个空壳 —— 那里结束进程
/// 用信号，按 pid 发。
///
/// 克隆共享同一把句柄，最后一份丢弃时关闭。相等按「是不是同一把」判，空壳之间恒等。
#[derive(Clone, Default)]
pub struct ProcessHold {
    #[cfg(windows)]
    held: Option<std::sync::Arc<windows::HeldProcess>>,
}

impl std::fmt::Debug for ProcessHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProcessHold")
    }
}

impl PartialEq for ProcessHold {
    #[cfg(windows)]
    fn eq(&self, other: &Self) -> bool {
        match (&self.held, &other.held) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    #[cfg(not(windows))]
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ProcessHold {}

#[cfg(windows)]
impl ProcessHold {
    /// 结束把柄所指的那个进程，**不连带它的子进程**。没有把柄、创建时间对不上或系统拒绝
    /// （别的用户 / SYSTEM 的进程）时什么都不做并返回 `false`。
    #[must_use]
    pub fn terminate(&self) -> bool {
        self.held
            .as_deref()
            .is_some_and(windows::HeldProcess::terminate)
    }

    /// 把柄所指的进程是否还在跑；没有把柄即 `None`。问的是句柄而不是 pid，不受 pid 复用影响。
    #[must_use]
    pub fn is_running(&self) -> Option<bool> {
        self.held.as_deref().map(windows::HeldProcess::is_running)
    }
}

/// 扫描时与结束前各读一次的创建时间是否指同一个进程：两次都读到且相等。任何一次没读到都不算。
#[cfg(any(windows, test))]
fn same_birth(held: Option<u64>, target: Option<u64>) -> bool {
    matches!((held, target), (Some(held), Some(target)) if held == target)
}

/// 该进程若是本 app 起的核（形状见模块文档），返回它 `-c` 指向的配置**文件名**。
///
/// 两种载荷各走各的解析：
/// - `cmdline`（Linux / Windows，argv）：`argv[0]` 是绝对路径且文件名 == `core_filename`，
///   `argv[1..3] == ["run", "-c"]`，`argv[3]` 的父目录 == `config_dir`。
/// - `raw`（macOS，`ps` 把 argv 用空格拼死，而程序路径与配置目录都可能含空格）：在整串里找
///   ` run -c <config_dir>/`，其前是程序、其后到下一个空格是配置文件名。本 app 生成的配置文件名
///   都不含空格。
///
/// 路径按哪种写法比，由 `config_dir` 自己决定：`/` 开头是 POSIX 写法，否则是 Windows 写法
/// （分隔符两种等价、盘符、不区分大小写）。不看宿主平台，也不经 `std::path`（理由见模块文档）。
///
/// 目录按**字面**比较，不 canonicalize：起核时传给核的就是 `config_dir.join(name)` 的字面串。
#[must_use]
pub fn app_run_config_name<'a>(
    process: &'a CoreProcess,
    core_filename: &str,
    config_dir: &Path,
) -> Option<&'a str> {
    let dir = config_dir.to_str()?;
    if !dir.starts_with('/') {
        return windows_run_config_name(&process.cmdline, core_filename, dir);
    }
    if let [program, run, flag, config, ..] = process.cmdline.as_slice() {
        let (config_parent, name) = config.rsplit_once('/')?;
        let matches = program.starts_with('/')
            && program.rsplit('/').next() == Some(core_filename)
            && run == "run"
            && flag == "-c"
            && same_posix_dir(config_parent, dir)
            && !name.is_empty();
        return matches.then_some(name);
    }
    let marker = format!(" run -c {}/", dir.trim_end_matches('/'));
    let at = process.raw.find(&marker)?;
    let program = &process.raw[..at];
    if !program.starts_with('/') || program.rsplit('/').next() != Some(core_filename) {
        return None;
    }
    let name = process.raw[at + marker.len()..].split(' ').next()?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// 两个 POSIX 目录串是否指同一个目录：都是绝对路径，按 `/` 切开后逐段相等（连写的 `/` 与
/// 尾随的 `/` 不计）。不解析 `.` / `..`，不跟随链接。
fn same_posix_dir(a: &str, b: &str) -> bool {
    fn parts(path: &str) -> impl Iterator<Item = &str> {
        path.split('/').filter(|part| !part.is_empty())
    }
    a.starts_with('/') && b.starts_with('/') && parts(a).eq(parts(b))
}

/// [`app_run_config_name`] 的 argv 腿在 Windows 路径写法下的版本：同一个形状，路径按 Windows 的
/// 规则比。
///
/// 与 POSIX 腿的差别只有三处，都来自平台事实：
/// - 分隔符是 `\`，`/` 也被系统接受 → 两者等价；
/// - 文件系统不区分大小写 → 核文件名与配置目录按 ASCII 忽略大小写比较；
/// - 绝对路径是盘符形式（`C:\…`）或 `\\` 开头（UNC / 设备路径）。
///
/// 全部按字符串处理，不经 `std::path`（理由见模块文档）。
fn windows_run_config_name<'a>(
    argv: &'a [String],
    core_filename: &str,
    config_dir: &str,
) -> Option<&'a str> {
    let [program, run, flag, config, ..] = argv else {
        return None;
    };
    let (_, program_name) = split_windows_path(program)?;
    if !is_windows_absolute(program)
        || !program_name.eq_ignore_ascii_case(core_filename)
        || run != "run"
        || flag != "-c"
    {
        return None;
    }
    let (dir, name) = split_windows_path(config)?;
    (same_windows_dir(dir, config_dir) && !name.is_empty()).then_some(name)
}

fn is_windows_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// 在最后一个分隔符处切成（目录，文件名）；没有分隔符即 `None`。
fn split_windows_path(path: &str) -> Option<(&str, &str)> {
    let at = path.rfind(is_windows_separator)?;
    Some((&path[..at], &path[at + 1..]))
}

/// 盘符形式（`X:\…`）或 `\\` 开头。
fn is_windows_absolute(path: &str) -> bool {
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(drive), Some(':'), Some(sep)) => {
            drive.is_ascii_alphabetic() && is_windows_separator(sep)
        }
        (Some(a), Some(b), _) => is_windows_separator(a) && is_windows_separator(b),
        _ => false,
    }
}

/// 两个目录串是否指同一个目录：逐字符比较，分隔符等价、ASCII 忽略大小写、不计尾随分隔符。
fn same_windows_dir(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches(is_windows_separator);
    let b = b.trim_end_matches(is_windows_separator);
    let same = |x: char, y: char| {
        (is_windows_separator(x) && is_windows_separator(y)) || x.eq_ignore_ascii_case(&y)
    };
    a.chars().count() == b.chars().count() && a.chars().zip(b.chars()).all(|(x, y)| same(x, y))
}

/// 把 Windows 的整串命令行切成参数，规则同 C 运行时（也即 Rust 的 `Command` 拼命令行时所逆的那套）。
///
/// - 程序名（第一个参数）：引号成对包住含空格的路径，其中的反斜杠是字面量；
/// - 其余参数：空白分隔；引号开闭一段可含空白的内容；`2n` 个反斜杠后跟引号 → `n` 个反斜杠 + 引号
///   开闭；`2n+1` 个反斜杠后跟引号 → `n` 个反斜杠 + 字面引号；引号内连写两个引号 → 一个字面引号；
///   不跟引号的反斜杠是字面量。
#[must_use]
pub fn split_windows_command_line(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let blank = |c: char| c == ' ' || c == '\t';
    let mut args = Vec::new();
    let mut i = 0;
    while i < chars.len() && blank(chars[i]) {
        i += 1;
    }
    if i == chars.len() {
        return args;
    }

    let mut program = String::new();
    let mut quoted = false;
    while i < chars.len() && (quoted || !blank(chars[i])) {
        if chars[i] == '"' {
            quoted = !quoted;
        } else {
            program.push(chars[i]);
        }
        i += 1;
    }
    args.push(program);

    loop {
        while i < chars.len() && blank(chars[i]) {
            i += 1;
        }
        if i == chars.len() {
            return args;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while i < chars.len() && (quoted || !blank(chars[i])) {
            let mut backslashes = 0;
            while i < chars.len() && chars[i] == '\\' {
                backslashes += 1;
                i += 1;
            }
            if i < chars.len() && chars[i] == '"' {
                arg.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes % 2 == 1 {
                    arg.push('"');
                } else if quoted && chars.get(i + 1) == Some(&'"') {
                    arg.push('"');
                    i += 1;
                } else {
                    quoted = !quoted;
                }
                i += 1;
            } else {
                arg.extend(std::iter::repeat_n('\\', backslashes));
                if i < chars.len() && (quoted || !blank(chars[i])) {
                    arg.push(chars[i]);
                    i += 1;
                }
            }
        }
        args.push(arg);
    }
}

/// `NtQueryInformationProcess(ProcessCommandLineInformation)` 的应答 → 命令行。
///
/// 应答是一个 `UNICODE_STRING` 头（`u16` 字节长度、`u16` 容量、一个指针）后跟字符数据，头里的
/// 指针指向**同一块缓冲区**内部。`base` 是这块缓冲区在本进程里的地址，用来把指针换成偏移；
/// 指针落在缓冲区之外、长度越界或为奇数一律当作读不到。
#[cfg(any(windows, test))]
fn command_line_from_reply(reply: &[u8], base: usize) -> Option<String> {
    const POINTER: usize = std::mem::size_of::<usize>();
    let length = usize::from(u16::from_ne_bytes(reply.get(..2)?.try_into().ok()?));
    // 指针字段按自身大小对齐：两个 `u16` 之后补齐到一个指针宽度。
    let pointer = usize::from_ne_bytes(reply.get(POINTER..2 * POINTER)?.try_into().ok()?);
    let start = pointer.checked_sub(base)?;
    let units = reply.get(start..start.checked_add(length)?)?;
    if length % 2 != 0 {
        return None;
    }
    let wide: Vec<u16> = units
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_ne_bytes(*pair))
        .collect();
    Some(String::from_utf16_lossy(&wide))
}

/// 该进程是否是「本 app 起的」sing-box 核：形状匹配，且配置是一份 `.json`
/// （主核、测速临时核、Tailscale 登录核的运行配置都是配置目录下的 `.json`）。
#[must_use]
pub fn is_our_core(process: &CoreProcess, core_filename: &str, config_dir: &Path) -> bool {
    app_run_config_name(process, core_filename, config_dir)
        .is_some_and(|name| name.ends_with(".json"))
}

/// 命令行里带着**还原不出来的字节**、因而判不了归属的内核进程数。
///
/// 判据比对的是路径字面串。两种情形下进程表给出的串与真实命令行不是同一串：
/// - 字节不是合法 UTF-8，解码时被换成了替换字符（U+FFFD）；
/// - macOS 的 `ps` 在非 UTF-8 locale 下把非 ASCII 字节转义成 `M-x` 形态（见 [`macos_ps`]）。
///
/// 这样的行在 [`is_our_core`] 眼里只是「路径对不上」，会被当成别人的进程静默放过。本函数把
/// 其中**确是内核在跑某份配置**的那些（`<…/核文件名> run -c …`）数出来，交调用方留痕：清扫
/// 因为读不准命令行而没清，与「没有孤儿」必须分得开。它不改变任何判定，只是让失效自曝。
#[must_use]
pub fn unreadable_core_rows(candidates: &[CoreProcess], core_filename: &str) -> usize {
    fn garbled(text: &str) -> bool {
        // `M-` 后跟一个可见字符，或 `M-^` 后跟一个字符，是 vis(3) 的 meta 转义。
        text.contains('\u{FFFD}')
            || text.match_indices("M-").any(|(at, _)| {
                text[at + 2..]
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_whitespace())
            })
    }
    let run_marker = format!("{core_filename} run -c ");
    candidates
        .iter()
        .filter(|process| {
            if let [program, run, flag, rest @ ..] = process.cmdline.as_slice() {
                let named_like_core = program
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|name| name == core_filename);
                return named_like_core
                    && run == "run"
                    && flag == "-c"
                    && (garbled(program) || rest.iter().any(|arg| garbled(arg)));
            }
            process.raw.contains(&run_marker) && garbled(&process.raw)
        })
        .count()
}

/// 命令行形状命中、且不在排除表里的候选，分为可清扫、已确认外来与归属未知三拨。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaleCores {
    /// 属主是本用户或 root / SYSTEM：清扫对象。
    pub sweep: Vec<CoreProcess>,
    /// 已确认其他用户的候选，只供调用方留痕。
    pub foreign: Vec<u32>,
    /// 同形候选的属主无法确认：未清扫，归属结论仍未决。
    pub unknown: Vec<u32>,
}

/// 从候选进程中挑出形状命中的那些，排除 `exclude`（自身 / 当前受管 pid），再按属主分拨。
#[must_use]
pub fn stale_cores(
    candidates: &[CoreProcess],
    core_filename: &str,
    config_dir: &Path,
    exclude: &[u32],
) -> StaleCores {
    let mut out = StaleCores::default();
    for process in candidates
        .iter()
        .filter(|c| is_our_core(c, core_filename, config_dir))
        .filter(|c| !exclude.contains(&c.pid))
    {
        match process.owner {
            ProcessOwner::SameUser | ProcessOwner::Elevated => out.sweep.push(process.clone()),
            ProcessOwner::Other => out.foreign.push(process.pid),
            ProcessOwner::Unknown => out.unknown.push(process.pid),
        }
    }
    out
}

/// 从候选进程中挑出「本 app 起的、需清理的」pid（[`stale_cores`] 的清扫对象那一拨）。
#[must_use]
pub fn stale_pids(
    candidates: &[CoreProcess],
    core_filename: &str,
    config_dir: &Path,
    exclude: &[u32],
) -> Vec<u32> {
    stale_cores(candidates, core_filename, config_dir, exclude)
        .sweep
        .iter()
        .map(|process| process.pid)
        .collect()
}

/// 扫描系统内在跑的候选进程。
///
/// Linux：直接读 `/proc/<pid>/cmdline` 与 `/proc/<pid>/status`（无外部命令、无新依赖）。macOS 与
/// Windows 各有一版，见下。
#[cfg(target_os = "linux")]
#[must_use]
pub fn scan_running_cores() -> Vec<CoreProcess> {
    let me = process_owner_uid(std::process::id());
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        // 只看纯数字 pid 目录（跳过 /proc/self、/proc/cpuinfo 等）。
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) else {
            continue; // 进程已退 / 无权限读 → 跳过。
        };
        if bytes.is_empty() {
            continue; // 内核线程 cmdline 为空。
        }
        // /proc/<pid>/cmdline 是 NUL 分隔的 argv；末尾可能有多余 NUL。
        let cmdline: Vec<String> = bytes
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        if cmdline.is_empty() {
            continue;
        }
        out.push(CoreProcess {
            pid,
            cmdline,
            owner: owner_from_uid(process_owner_uid(pid), me),
            ..CoreProcess::default()
        });
    }
    out
}

/// macOS 上起 `ps` 的唯一写法：经 `/usr/bin/env` 只给 `ps` 子进程注入 UTF-8 locale。
///
/// 从 Finder / LaunchServices 启动的 GUI 应用常处于 C locale。`ps` 输出命令行与可执行路径时
/// 经 `vis(3)` 编码，C locale 下非 ASCII 字节不算可见字符，会被逐个转义成 `M-x` 形态，事后
/// 按 UTF-8 解码也还原不了。用户名或路径含非 ASCII 字符时，凡是拿 `ps` 输出里的路径去比对的
/// 判据都会静默失配。故全仓对 `ps` 的调用都从这里取程序与前缀，不各写各的。
///
/// 不带 cfg：应用侧的进程选择器在任何宿主上都要能构造并测试这条命令。
pub mod macos_ps {
    /// 被执行的程序。
    pub const PROGRAM: &str = "/usr/bin/env";
    /// 排在 `ps` 自己的参数之前的固定前缀（两个 locale 变量 + `ps` 的绝对路径）。
    pub const ARGV_PREFIX: [&str; 3] = ["LC_ALL=en_US.UTF-8", "LANG=en_US.UTF-8", "/bin/ps"];

    /// `ps <args…>` 的命令（stdin 置空：继承来的终端会让 `ps` 按终端宽度截断输出列）。
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn command(ps_args: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new(PROGRAM);
        command
            .args(ARGV_PREFIX)
            .args(ps_args)
            .stdin(std::process::Stdio::null());
        command
    }
}

/// `ps -axo pid=,uid=,args=` 输出 → 候选进程（纯解析，可零进程单测）。
///
/// 每行 = 前导空格 + pid + 空白 + uid + 空白 + 命令行余部。**余部整串留作 `raw` 不再切分**——见
/// [`CoreProcess`] 的说明：macOS 核路径含空格，切分即失配。
///
/// 属主相对 `self_pid` 那一行的 uid 归类：本进程自己就在这份输出里，于是「我是谁」与「它们是谁」
/// 出自同一次 `ps`。输出里找不到本进程时没有「自己」可比，只有 uid 0 还认得出来（root）。
#[cfg(any(target_os = "macos", test))]
#[must_use]
pub fn parse_ps_output(stdout: &str, self_pid: u32) -> Vec<CoreProcess> {
    let rows: Vec<(u32, u32, &str)> = stdout
        .lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
            let (uid, rest) = rest.trim_start().split_once(char::is_whitespace)?;
            let raw = rest.trim();
            (!raw.is_empty()).then_some((pid.parse().ok()?, uid.parse().ok()?, raw))
        })
        .collect();
    let me = rows
        .iter()
        .find(|(pid, ..)| *pid == self_pid)
        .map(|(_, uid, _)| *uid);
    rows.into_iter()
        .map(|(pid, uid, raw)| CoreProcess {
            pid,
            raw: raw.to_owned(),
            owner: owner_from_uid(Some(uid), me),
            ..CoreProcess::default()
        })
        .collect()
}

/// macOS 孤儿扫描：`ps -axo pid=,uid=,args=` 全量列进程（`-a` 含他人进程 → **root 起的核也在列**，
/// 这正是 helper 提权路径遗留的孤儿；`-x` 含无控制终端的后台进程）。
///
/// **为什么必须有**：此前本函数在非 Linux 恒返空 ⇒ macOS 的启动期清扫是彻底的 no-op ⇒
/// helper 以 root 遗留的孤儿核永不被发现、永不被清理，一直占着 `cache.db` ⇒ 之后每次起核
/// 都 `initialize cache-file: timeout`，连切回 systemProxy 也起不来（真机实证的卡死链）。
#[cfg(target_os = "macos")]
#[must_use]
pub fn scan_running_cores() -> Vec<CoreProcess> {
    let Ok(out) = macos_ps::command(&["-axo", "pid=,uid=,args="]).output() else {
        return Vec::new();
    };
    parse_ps_output(&String::from_utf8_lossy(&out.stdout), std::process::id())
}

/// Windows 孤儿扫描：枚举进程，逐个向系统要命令行（切成 argv）与令牌的用户，并留住句柄。
///
/// 命令行经 `NtQueryInformationProcess(ProcessCommandLineInformation)` 取得（Windows 8.1 起）：
/// 只要 `PROCESS_QUERY_LIMITED_INFORMATION` 权限，由系统代读，不涉及读对方内存，也就没有
/// 32 / 64 位进程互读的问题。打不开或读不到的进程（别的会话、受保护进程、已退出）跳过 ——
/// 认不出就不碰。没走 WMI / `wmic` / PowerShell：要起外部进程，慢，且 `wmic` 在新系统上已不随附。
///
/// 每个候选带着扫描时那把句柄（[`ProcessHold`]）：命令行、属主、创建时间都从同一把句柄读出，
/// 之后结束与探活也认它，中间不再按 pid 重新找人。
#[cfg(windows)]
#[must_use]
pub fn scan_running_cores() -> Vec<CoreProcess> {
    let me = windows::TokenUserSid::of_current_process();
    windows::process_ids()
        .into_iter()
        .filter_map(|pid| {
            let held = windows::HeldProcess::open(pid)?;
            let cmdline = split_windows_command_line(&held.command_line()?);
            (!cmdline.is_empty()).then(|| CoreProcess {
                pid,
                cmdline,
                raw: String::new(),
                owner: owner_from_token(held.token_user_facts(me.as_ref())),
                hold: ProcessHold {
                    held: Some(std::sync::Arc::new(held)),
                },
            })
        })
        .collect()
}

/// 其余平台没有可扫的进程形态（移动端的核在进程内），恒返空。
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
#[must_use]
pub fn scan_running_cores() -> Vec<CoreProcess> {
    Vec::new()
}

/// Effective uid from `/proc/<pid>/status` (`Uid:` lists real, effective, saved, filesystem).
#[cfg(any(target_os = "linux", test))]
#[must_use]
pub fn parse_proc_status_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// `ps -o uid= -p <pid>` output → uid. Anything but one number is unknown.
#[cfg(any(target_os = "macos", test))]
#[must_use]
pub fn parse_ps_uid(stdout: &str) -> Option<u32> {
    stdout.trim().parse().ok()
}

/// The user a process runs as; `None` when it cannot be read. Callers that end processes by
/// identity use it to leave another user's processes alone.
#[cfg(target_os = "linux")]
#[must_use]
pub fn process_owner_uid(pid: u32) -> Option<u32> {
    parse_proc_status_uid(&std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?)
}

#[cfg(target_os = "macos")]
#[must_use]
pub fn process_owner_uid(pid: u32) -> Option<u32> {
    let out = macos_ps::command(&["-o", "uid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    parse_ps_uid(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[must_use]
pub fn process_owner_uid(_pid: u32) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests;
