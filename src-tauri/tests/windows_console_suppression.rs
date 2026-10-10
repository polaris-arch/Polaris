//! Windows 控制台窗口抑制的接线门。
//!
//! # 守的是什么
//!
//! 宿主是 GUI 子系统进程（`src-tauri/src/main.rs` 的 `windows_subsystem = "windows"`）⇒ 自身无控制台。
//! 无控制台的父进程起 **console 子系统**程序时，`CreateProcess` 会新分配一个控制台窗口（黑框）。
//! std 与 tokio 都**没有**隐含抑制 —— tokio 的 `creation_flags` 只是往 std 透传
//! （实测 tokio-1.53.1 `src/process/mod.rs:675-677`）。
//!
//! # 为什么必须是源码级门
//!
//! 这件事**在 Linux 上没有任何运行期表征**：`#[cfg(windows)]` 的分支根本不参与编译，
//! 纯函数单测测不到「有没有挂标志」，而唯一能观察到黑框的地方是 Windows 真机。
//! 三份现成教训都指向同一形状：`spawner.rs` 曾写着「tokio::process 在 Windows 默认不显示控制台窗口」
//! —— 一句**错误的注释**让这条缺陷在起核路径上潜伏了整个迁移期，没有任何门会红。
//!
//! # 与既有门的分工
//!
//! `core_build_matrix`（编了什么）/ `core_schema_surface`（配置形状与取值域）/ 起核 `check`（这份配置收不收）
//! 三道门都不看**进程怎么被创建**。本门只管这一格。

use std::collections::BTreeMap;

/// 仓库根（`src-tauri/` 的上一级）。
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 必有上级目录")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读不到 {}: {e}", p.display()))
}

/// 一个被守的调用点：锚点串之后的窗口内，必须同时出现 `Command::new(` 与抑制标记。
struct Guarded {
    file: &'static str,
    /// 唯一定位串。同名函数有多个 cfg 变体时**连 `#[cfg(...)]` 一起写**，否则锚点不唯一。
    anchor: &'static str,
    /// 抑制形态（可执行形，不是裸标识符）。
    suppressor: &'static str,
    /// 窗口自检串：窗口里必须先有它，否则说明窗口没盖住要守的东西 ⇒ 抑制断言恒真。
    /// 多数点是 `Command::new(`；`win_console.rs` 那两个函数**接收**已构造好的 `Command`，
    /// 它们自己不构造 ⇒ 自检改钉 cfg 门（抑制必须只在 Windows 生效，别在 Linux 上编不过）。
    self_check: &'static str,
    /// 从锚点起看多少行。各函数都远短于此；放宽只会让门更松，故取够用的最小值。
    ///
    /// 仅在 `before` 为 `None` 时生效。
    window: usize,
    /// `Some(forms)` = **判据不数行**：抑制标志必须早于 `forms` 里最早出现的那一处
    /// （= 早于进程真正被创建）。窗口就是「锚点 → 那一处」，宽度由源码结构给，不由人数行给。
    ///
    /// # 为什么要有这一格（行数窗口的失效方式与它守的属性无关）
    ///
    /// `spawner.rs` 的 spawn 方法体本来就要写清「这一路管道为什么这么开」，注释只会越来越长；
    /// [`strip_comments`] 只把 `//` 之后清空、**不删行**，注释照样占窗口。整改前那一条的 delta 是
    /// 23 / 窗口 30，余量 7 行、其中 6 行是注释 —— 下一个在方法开头补几句注释的人就会把门顶红，
    /// 而顶红的原因（「离锚点第 31 行」）与门要守的属性（「标志在进程创建之前设上」）毫无关系。
    /// 更糟的另一头：真把 `creation_flags` 挪到 `.spawn()` 之后（Windows 上照样弹黑窗），
    /// 只要它还落在 30 行内，行数窗口是**绿**的。
    ///
    /// `Command` 的构建器调用顺序对 `creation_flags` 语义无关紧要，唯一要紧的是它在 `spawn()`
    /// 之前 —— 那才是这道门真正要守的东西，所以判据就写成它。
    before: Option<&'static [&'static str]>,
}

/// 既有构造点的较早边界；`.stdout(` 是构建器调用，`.spawn()` 才创建进程。
/// CheckCustody 在配置 stdout/stderr 后挂 Windows 标志，单独以真实 `.spawn()` 为边界。
const CREATION_FORMS: &[&str] = &[".stdout(", ".spawn()"];

/// 全部「Windows 可达 + 目标是 console 程序」的子进程构造点。
///
/// **不在表里 = 声称该调用点在 Windows 上不可达**。目前的豁免全部有 cfg 佐证：
/// `/bin/ps`（macos/linux 腿）、`pgrep`（`cfg(unix)`）、`route -n monitor`（仅 mac 守卫会调，
/// 见 `dns_watcher_loop` 文档）、`mesh.rs::run_command_stdout`（mac `ifconfig` 反查）、
/// `uninstall.rs::spawn_uninstaller`（拉起的是 Windows 卸载程序**自己的 GUI**，抑制窗口反而不对）。
const GUARDED: &[Guarded] = &[
    // ---- 本 crate：经 runtime/win_console.rs 收口 ----
    Guarded {
        file: "src-tauri/src/runtime/win_console.rs",
        anchor: "pub(crate) fn no_console_window(",
        suppressor: "creation_flags(0x0800_0000)",
        self_check: "#[cfg(windows)]",
        window: 14,
        before: None,
    },
    // `no_console_window_async`（tokio 版）已随它仅有的两个调用点一起删除：那两处是自己写了一遍的
    // `sing-box check`，已折叠进 `core-supervisor::config_gate::run_check_raw`（本表下方单独守）。
    // 本 crate 于是不再有 Windows 可达的 tokio 子进程构造点。
    // Phase 2 拆分：`core_version_first_line` 进 `proxy/process_supervision.rs`。同文件原先还有
    // Windows 版 `send_signal`（起 `taskkill`）：孤儿清扫改为经扫描时留住的句柄结束进程后，
    // 那个子进程构造点已不存在，条目随之删除。
    Guarded {
        file: "src-tauri/src/runtime/proxy/process_supervision.rs",
        anchor: "fn core_version_first_line(",
        suppressor: "no_console_window(",
        self_check: "Command::new(",
        window: 6,
        before: None,
    },
    Guarded {
        file: "src-tauri/src/runtime/updater.rs",
        anchor: "pub fn read_core_version_line(",
        suppressor: "no_console_window(",
        self_check: "Command::new(",
        window: 10,
        before: None,
    },
    // 这里曾有两条 `sing-box check` 的构造点（`tailscale_login_core.rs::SingBoxConfigChecker` 与
    // `commands/proxy.rs::run_probe_check`）。两处已不再自己构造子进程，改调
    // `core-supervisor::config_gate::run_check_raw`（本表下方那条守着它的 `creation_flags`），
    // 故条目随构造点一起消失。**替代判据见
    // [`only_one_production_site_spawns_sing_box_check`]**：本表是手写清单，
    // [`no_new_console_program_spawn_escapes_the_suppression`] 只按程序名字面量反查，`sing-box`
    // 的路径是变量、两者都盖不住「有人在这两个文件里重新写一份 check」。那条新门把「这两处挂了
    // 抑制标志」换成了更强的「全仓只允许有一处 check 构造点」。
    // ---- 另外三个 crate：与本 crate 无共同依赖，各自持等价实现 ----
    Guarded {
        file: "crates/system-integration/src/exec.rs",
        // 构造点搬进了固有方法 `run_observed`；`CommandRunner::run` 只转调它，不再有 `Command::new(`。
        anchor: "impl StdCommandRunner {",
        suppressor: "creation_flags(CREATE_NO_WINDOW)",
        self_check: "Command::new(",
        window: 20,
        before: None,
    },
    Guarded {
        file: "crates/core-supervisor/src/spawner.rs",
        anchor: "impl SingBoxSpawner for TokioSpawner {",
        suppressor: "creation_flags(0x0800_0000)",
        self_check: "Command::new(",
        // 这一条**不数行**（见 `Guarded::before`）：本方法体是全仓最会长注释的地方（三条核腿共用
        // 的起核入口），行数窗口的余量迟早被注释吃光，而顶红的原因与门守的属性无关。
        window: 0,
        before: Some(CREATION_FORMS),
    },
    Guarded {
        file: "crates/core-supervisor/src/config_gate/check_custody.rs",
        anchor: "fn spawn(",
        suppressor: "creation_flags(0x0800_0000)",
        self_check: "Command::new(",
        window: 0,
        before: Some(&[".spawn()"]),
    },
    Guarded {
        file: "crates/helper-client/src/manager.rs",
        anchor: "fn sc_command(",
        suppressor: "creation_flags(0x0800_0000)",
        self_check: "Command::new(",
        window: 8,
        before: None,
    },
    Guarded {
        file: "crates/helper-client/src/privilege.rs",
        anchor: "impl Executor for StdExecutor {",
        suppressor: "creation_flags(0x0800_0000)",
        self_check: "Command::new(",
        window: 18,
        before: None,
    },
];

/// 去掉行注释（`//` 之后）—— 判据必须落在**可执行形态**上。
///
/// 本文件自己的模块头就反复写着 `creation_flags` 与 `CREATE_NO_WINDOW`，被守文件的文档注释同理；
/// 不剥注释的话，把生产调用整个删掉、注释留下，门照样绿（本仓 2026-08-07 起同型撞过四次）。
fn strip_comments(src: &str) -> String {
    src.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_windows_reachable_spawn_suppresses_the_console() {
    let mut sources: BTreeMap<&str, String> = BTreeMap::new();
    for g in GUARDED {
        let src = sources
            .entry(g.file)
            .or_insert_with(|| strip_comments(&read(g.file)));
        let at = src.find(g.anchor).unwrap_or_else(|| {
            panic!(
                "{}：锚点 `{}` 消失（改名/删除？）——门已失去判据，不是「通过」",
                g.file, g.anchor
            )
        });
        assert!(
            src[at + g.anchor.len()..].find(g.anchor).is_none(),
            "{}：锚点 `{}` 不唯一，窗口可能落在另一个 cfg 变体上",
            g.file,
            g.anchor
        );
        let rest = &src[at..];
        // 窗口两种取法：数行（`before: None`），或者取到「进程真被创建」那一处为止。
        // 后者的宽度由源码结构给 —— 它守的属性就是「标志早于创建」，判据于是与行数无关。
        let (window, scope) = match g.before {
            None => (
                rest.lines().take(g.window).collect::<Vec<_>>().join("\n"),
                format!("之后 {} 行内", g.window),
            ),
            Some(forms) => {
                let creation = forms.iter().filter_map(|f| rest.find(f)).min();
                // 切点自检：找不到创建点 = 锚点漂了 / 构造搬走了 ⇒ 窗口无从界定，必须红。
                // 没有这一条，`min()` 为 `None` 时无论怎么退化都是一次静默失效。
                let creation = creation.unwrap_or_else(|| {
                    panic!(
                        "{}：`{}` 之后找不到 {forms:?} 里的任何一处 —— \
                         这个块里没有子进程被创建（锚点漂了 / 构造搬走了），窗口无从界定，\
                         抑制断言在这种状态下恒真",
                        g.file, g.anchor
                    )
                });
                (
                    rest[..creation].to_owned(),
                    format!("与首个 {forms:?} 之间"),
                )
            }
        };
        // 自检：窗口里必须真有子进程构造，否则说明窗口太小 / 锚点漂了，下面那条断言就没有意义。
        assert!(
            window.contains(g.self_check),
            "{}：`{}` {scope}没有 `{}` —— 窗口没盖住要守的东西，抑制断言恒真",
            g.file,
            g.anchor,
            g.self_check
        );
        assert!(
            window.contains(g.suppressor),
            "{}：`{}` 的子进程构造没挂 `{}`（{scope}）—— Windows 上会弹控制台窗口。\
             `before` 形态下这句话的意思是：标志要么没了，要么被挪到了进程创建**之后** —— \
             那时候再设已经不起作用",
            g.file,
            g.anchor,
            g.suppressor
        );
    }
}

/// 已知会在 Windows 上执行的 console 程序名（字面量形态）。按**程序名反查**，与上面的清单互补：
/// 清单防「已守的被删」，本条防「新增一个 console 程序调用却忘了挂标志」。
const CONSOLE_PROGRAMS: &[&str] = &[
    "\"tasklist\"",
    "\"taskkill\"",
    "\"sc\"",
    "\"netsh\"",
    "\"reg\"",
];

/// 允许出现裸调用的位置（测试夹具 / 纯字符串常量表）。
fn is_scannable(rel: &str) -> bool {
    rel.ends_with(".rs") && !rel.contains("/tests/") && !rel.contains("target/")
}

#[test]
fn no_new_console_program_spawn_escapes_the_suppression() {
    let root = repo_root();
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    let mut hits = 0usize;
    let mut sighted: Vec<String> = Vec::new();
    for dir in ["src-tauri/src", "crates"] {
        for entry in walk(&root.join(dir)) {
            let rel = entry
                .strip_prefix(&root)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            if !is_scannable(&rel) {
                continue;
            }
            // helper 是 Windows **服务**（session 0，无交互桌面）⇒ 它起的子进程本就无窗口可弹，
            // 且那边已自带 `CREATE_NO_WINDOW`（`winproc/win.rs:296`）。不纳入本门射程。
            if rel.starts_with("crates/helper/") {
                continue;
            }
            let raw = std::fs::read_to_string(&entry).unwrap_or_default();
            scanned += 1;
            // **不切 `#[cfg(test)]`**：`proxy.rs` 里生产码与测试模块交替出现（实测顶层 5 处），
            // 切第一处会把后面全部真调用点一起丢掉 —— 实测本门第一版就是这么静默漏掉 4 处的。
            // 测试夹具起的是 `powershell` / `sleep`，都不在 [`CONSOLE_PROGRAMS`] 里，故无需切。
            let prod = strip_comments(&raw);
            for (i, line) in prod.lines().enumerate() {
                // 只认 `process::Command::new(`（std / tokio 都带这个前缀）。
                // `polaris_system_integration::exec::Command::new(program, args)` 是两参数的**命令描述**，
                // 真正的 spawn 在 `StdCommandRunner::run` 里、已在 GUARDED 表中单独守着。
                if !line.contains("process::Command::new(") {
                    continue;
                }
                if !CONSOLE_PROGRAMS.iter().any(|p| line.contains(p)) {
                    continue;
                }
                hits += 1;
                sighted.push(format!("{rel}: {}", line.trim()));
                let lo = i.saturating_sub(6);
                let ctx: String = prod
                    .lines()
                    .skip(lo)
                    .take(i - lo + 14)
                    .collect::<Vec<_>>()
                    .join("\n");
                let guarded = ctx.contains("no_console_window")
                    || ctx.contains("creation_flags")
                    || ctx.contains("sc_command(");
                if !guarded {
                    offenders.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        scanned > 50,
        "只扫到 {scanned} 个文件 —— 遍历坏了，绿没有信息量"
    );
    // 具名自检比数量更有信息量：锁住仍真实存在的 console 调用点，防遍历/匹配坏掉后「零命中也绿」。
    // 旧 1Hz `tasklist` 探活与孤儿清扫的 `taskkill` 都已改为 Win32 原生 API，扫描面里按程序名
    // 起 console 程序的只剩下面这一处。
    let (file, program) = ("crates/helper-client/src/manager.rs", "\"sc\"");
    assert!(
        sighted
            .iter()
            .any(|s| s.starts_with(file) && s.contains(program)),
        "扫描面里没有 `{file}` 的 {program} 调用——遍历或匹配坏了。实际命中 {hits} 处：\n{}",
        sighted.join("\n")
    );
    assert!(
        offenders.is_empty(),
        "以下 console 程序调用点没有窗口抑制（Windows 上会弹黑框）：\n{}",
        offenders.join("\n")
    );
}

/// Windows 可达的 `sing-box check` 构造点只有 CheckCustody 一处；另有 Linux x86_64 sealed-fd 检查。
///
/// # 这条门补的是哪个缝
///
/// [`GUARDED`] 曾用两条登记守着 `tailscale_login_core.rs::SingBoxConfigChecker` 与
/// `commands/proxy.rs::run_probe_check` 的 `CREATE_NO_WINDOW`。两处折叠进
/// `core-supervisor::config_gate::run_check_raw` 之后，那两条登记失去对象、必须删掉 —— 而删掉之后
/// 这两个文件就**不在任何判据的射程里**了：`GUARDED` 是手写清单（只守写进去的东西），
/// [`no_new_console_program_spawn_escapes_the_suppression`] 按**程序名字面量**反查
/// （`tasklist` / `sc` / `netsh`…），而 sing-box 的路径是个变量。于是「有人在这两个文件里重新写一份
/// `Command::new(binary).arg("check")`」既不会被清单抓到，也不会被字面量扫描抓到。
///
/// 常规 check 已移至 CheckCustody。新 mesh candidate 的另一处仅在 Linux x86_64 模块编译，
/// 且必须使用 sealed fd、空输出管道、kill_on_drop 与预算托管。本门逐个锁住两处精确 argv、
/// 次数和 Linux cfg；新拷贝仍要求归入已有托管入口，不能只补一个窗口标志。
///
/// # 判据形态
///
/// 针是 argv 里的字面量 `"check"`（含引号），比 `.arg("check")` 宽：`.args(["check", …])` 之类的
/// 写法同样落网。取材面先过 [`strip_comments`]，否则被守文件的文档注释里那些讲 `check` 的句子会
/// 让本门恒红。正向对照是「两处各恰好命中一次」；缺失、新增或失去 Linux 闭集条件都变红。
#[test]
fn only_one_production_site_spawns_sing_box_check() {
    /// argv 里的子命令字面量（含引号）。
    const CHECK_ARG: &str = "\"check\"";
    /// Windows 可达的常规 check 构造点，与仅 Linux x86_64 的 sealed-fd check 各一处。
    const HOME: &str = "crates/core-supervisor/src/config_gate/check_custody.rs";
    const LINUX: &str = "src-tauri/src/runtime/proxy/mesh_apply/candidate/check.rs";

    let root = repo_root();
    let mut sites: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for dir in ["src-tauri/src", "crates"] {
        for entry in walk(&root.join(dir)) {
            let rel = entry
                .strip_prefix(&root)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            if !is_scannable(&rel) {
                continue;
            }
            scanned += 1;
            let prod = strip_comments(&std::fs::read_to_string(&entry).unwrap_or_default());
            for (i, line) in prod.lines().enumerate() {
                if line.contains(CHECK_ARG) {
                    sites.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        scanned > 50,
        "只扫到 {scanned} 个文件 —— 遍历坏了，绿没有信息量"
    );
    assert_eq!(
        sites.len(),
        2,
        "`sing-box check` 的构造点必须是 Windows 可达的 `{HOME}` 与仅 Linux x86_64 的 `{LINUX}` 各一处。\
         缺任一处 = 针或遍历坏了；>2 = 又出现了一份各自漂的拷贝，\
         而每一份拷贝都得自己记得超时与 `kill_on_drop` —— 折叠之前的三份里就有两份没记住。\
         实际命中：\n{}",
        sites.join("\n")
    );
    for (file, argv) in [
        (HOME, ".arg(\"check\")"),
        (
            LINUX,
            ".args([\"--disable-color\", \"check\", \"-c\", \"/proc/self/fd/0\"])",
        ),
    ] {
        assert_eq!(
            sites
                .iter()
                .filter(|site| site.starts_with(&format!("{file}:")) && site.ends_with(argv))
                .count(),
            1,
            "{file} 必须持有其唯一精确 check argv；其它构造点不受豁免：{sites:?}"
        );
    }
    let linux = strip_comments(&read(LINUX));
    let module = braced_block(
        &linux,
        "#[cfg(all(target_os = \"linux\", target_arch = \"x86_64\"))]\nmod linux {",
    );
    let run = braced_block(module, "pub(super) async fn run_check(");
    for required in [
        ".args([\"--disable-color\", \"check\", \"-c\", \"/proc/self/fd/0\"])",
        "/proc/self/fd/",
        ".env_clear()",
        ".current_dir(\"/\")",
        ".stdout(Stdio::null())",
        ".stderr(Stdio::null())",
        ".kill_on_drop(true)",
        "supervise(command, binary, config, timeout, None).await",
    ] {
        assert!(
            run.contains(required),
            "Linux-only protected check 失去严格构造条件：{required}"
        );
    }
    // fd 0 的接线不在 `run_check` 里了：sealed config 的原子 CLOEXEC 复制与 `.stdin(…)` 随 Child 的
    // 保管一起搬进了 CheckCustody 的入队处。沿「run_check → supervise → 入队」逐跳钉住，中间任何
    // 一跳改道，上面那串 argv 里的 `/proc/self/fd/0` 就指向一个没人接的 fd。
    let relay = braced_block(module, "pub(super) async fn supervise(");
    assert!(
        relay.contains(
            "supervise_owned_sealed_check(command, binary, config, timeout, spawned).await"
        ),
        "Linux-only protected check 不再把命令与两份 sealed 输入交给 CheckCustody"
    );
    let home = strip_comments(&read(HOME));
    for (anchor, required) in [
        (
            "pub async fn supervise_owned_sealed_check(",
            ".supervise_owned_sealed(command, binary, config, timeout, spawned)",
        ),
        (
            "async fn supervise_owned_sealed(",
            "self.queue_owned_sealed(command, binary, config, Arc::new(NativeIo))",
        ),
        ("fn queue_owned_sealed(", "validate_sealed_file(&binary)?"),
        ("fn queue_owned_sealed(", "validate_sealed_file(&config)?"),
        (
            "fn queue_owned_sealed(",
            "rustix::io::fcntl_dupfd_cloexec(&config, 3)",
        ),
        (
            "fn queue_owned_sealed(",
            ".stdin(std::process::Stdio::from(stdin))",
        ),
    ] {
        assert!(
            braced_block(&home, anchor).contains(required),
            "Linux-only protected check 在 `{anchor}` 失去严格构造条件：{required}"
        );
    }
}

/// 在已有词法净化面配平块，保证 check 的构造位于 Linux cfg 的实际作用域中。
fn braced_block<'a>(source: &'a str, anchor: &str) -> &'a str {
    assert_eq!(
        source.matches(anchor).count(),
        1,
        "块锚点必须唯一：{anchor}"
    );
    let start = source.find(anchor).unwrap();
    let masked = polaris_source_probe::mask_comments_and_strings(source);
    let bytes = masked.as_bytes();
    let open = start + masked[start..].find('{').expect("块没有起始括号");
    let mut depth = 0usize;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=offset];
                }
            }
            _ => {}
        }
    }
    panic!("块没有闭合：{anchor}");
}

#[test]
fn linux_check_scope_does_not_borrow_a_function_outside_its_cfg() {
    const MODULE: &str =
        "#[cfg(all(target_os = \"linux\", target_arch = \"x86_64\"))]\nmod linux {";
    const RUN: &str = "pub(super) async fn run_check(";
    let inside = format!("{MODULE}\n{RUN}) {{ let message = \"}}\"; }}\n}}");
    assert!(braced_block(braced_block(&inside, MODULE), RUN).contains("let message"));
    let outside = format!("{MODULE}}}\n{RUN}) {{}}");
    assert!(
        std::panic::catch_unwind(|| braced_block(braced_block(&outside, MODULE), RUN)).is_err()
    );
    assert!(std::panic::catch_unwind(|| braced_block("mod linux {", "mod linux {")).is_err());
}

/// 主程序直起的核必须被纳入「主程序消失即结束」的作业对象 —— 同样是只有 Windows 才编得到的一行。
///
/// # 为什么也放在这里
///
/// 与控制台抑制是同一种形状：`#[cfg(windows)]` 的一行接线，Linux 编译单元里根本没有它，删掉之后
/// 本机全绿。行为门在 `crates/core-supervisor/tests/windows_process.rs`（真起进程、真杀父进程），
/// 但它只在 Windows 上跑；这一条让「接线还在」在任何平台上都看得见。
///
/// 判据三条：纳入调用在 `TokioSpawner::spawn` 里、位于进程创建**之后**与返回**之前**；它确实受
/// `#[cfg(windows)]` 管着（否则别的平台编不过，或者有人为了编过把它整个删了）；作业确实带
/// 「句柄关闭即结束」那个限制，且主程序没有把自己放进去（放进去会连带结束它起的所有子进程）。
#[test]
fn direct_spawned_cores_join_the_kill_on_close_job() {
    let spawner = strip_comments(&read("crates/core-supervisor/src/spawner.rs"));
    let block = braced_block(&spawner, "impl SingBoxSpawner for TokioSpawner {");
    let created = block
        .find("cmd.spawn()")
        .expect("spawn 方法里找不到进程创建点");
    let returned = block
        .find("Ok(SpawnedChild { child })")
        .expect("spawn 方法里找不到返回点");
    const ENROLL: &str = "#[cfg(windows)]\n        crate::job_object::enroll_spawned_core(&child);";
    assert_eq!(
        spawner.matches("enroll_spawned_core(").count(),
        1,
        "纳入作业的调用点应恰有一处"
    );
    let enrolled = block
        .find(ENROLL)
        .expect("TokioSpawner::spawn 不再把子进程纳入作业对象（或那一行不再受 cfg(windows) 管）");
    assert!(
        created < enrolled && enrolled < returned,
        "纳入必须在进程创建之后、返回之前"
    );

    let job = strip_comments(&read("crates/core-supervisor/src/job_object.rs"));
    assert!(
        job.contains("LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;"),
        "作业不再带「句柄关闭即结束」的限制"
    );
    assert!(
        !job.contains("GetCurrentProcess"),
        "主程序不得把自己放进这个作业：那会连带结束安装脚本、卸载程序等它起的其它子进程"
    );
    let lib = strip_comments(&read("crates/core-supervisor/src/lib.rs"));
    assert!(lib.contains("#[cfg(windows)]\npub mod job_object;"));
}

/// 四份实现散在四个无共同依赖的 crate 里 —— 值必须逐字一致，否则「改了一处以为全改了」。
#[test]
fn the_four_crates_agree_on_the_flag_value() {
    let bearers = [
        "src-tauri/src/runtime/win_console.rs",
        "crates/system-integration/src/exec.rs",
        "crates/core-supervisor/src/spawner.rs",
        "crates/helper-client/src/manager.rs",
    ];
    for f in bearers {
        let src = strip_comments(&read(f));
        assert!(
            src.contains("0x0800_0000"),
            "{f}：`CREATE_NO_WINDOW` 的值不见了（或被写成了别的字面形态）"
        );
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
