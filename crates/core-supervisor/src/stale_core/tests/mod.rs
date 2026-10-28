use super::*;
use std::path::Path;

const CORE: &str = "sing-box";
const CONFIG_DIR: &str = "/home/u/.config/com.polaris.app/polaris";

/// 本用户起的进程（argv 载荷）。属主另有专门的用例，其余用例里的候选一律是本用户的。
fn argv(pid: u32, parts: &[&str]) -> CoreProcess {
    CoreProcess {
        pid,
        cmdline: parts.iter().map(|s| (*s).to_string()).collect(),
        owner: ProcessOwner::SameUser,
        ..CoreProcess::default()
    }
}

/// 本用户起的进程（`ps` 整串载荷）。
fn raw(pid: u32, line: &str) -> CoreProcess {
    CoreProcess {
        pid,
        raw: line.to_owned(),
        owner: ProcessOwner::SameUser,
        ..CoreProcess::default()
    }
}

/// `ps` 输出里代表本进程自己的那一行：uid 501，与下面样例行的属主相同。
const SELF_PID: u32 = 4_000_001;
const SELF_PS_LINE: &str = "4000001   501 /Applications/Polaris.app/Contents/MacOS/Polaris";

/// 解析一段 `ps` 输出，去掉代表本进程自己的那一行。
fn ps(lines: &str) -> Vec<CoreProcess> {
    parse_ps_output(&format!("{SELF_PS_LINE}\n{lines}"), SELF_PID)
        .into_iter()
        .filter(|process| process.pid != SELF_PID)
        .collect()
}

fn ours(process: &CoreProcess) -> bool {
    is_our_core(process, CORE, Path::new(CONFIG_DIR))
}

/// 主核的运行配置路径。
fn runtime_config() -> String {
    format!("{CONFIG_DIR}/singbox-runtime.json")
}

#[test]
fn our_core_is_recognised_by_the_config_it_runs() {
    let config = runtime_config();
    let core = argv(
        1,
        &[
            "/opt/polaris/resources/linux/sing-box",
            "run",
            "-c",
            &config,
        ],
    );
    assert!(ours(&core));
    assert_eq!(
        app_run_config_name(&core, CORE, Path::new(CONFIG_DIR)),
        Some("singbox-runtime.json")
    );
    // 尾随参数（`--disable-color` 等）不影响判定。
    assert!(ours(&argv(
        1,
        &[
            "/opt/polaris/resources/linux/sing-box",
            "run",
            "-c",
            &config,
            "--disable-color"
        ],
    )));
}

/// 🔴 **判据不依赖安装位置**：同一个配置目录下，核二进制住在哪儿都认得出来。
///
/// AppImage 每次挂载点不同、macOS 转移运行时路径随机、升级可能换安装目录；升级当次，上一版
/// 留下的孤儿还在旧的用户目录路径上。这些都必须命中，否则孤儿占着端口与 TUN，新核起不来。
///
/// **变异探针**：把判据改回「argv[0] 等于本次解析出的核路径」⇒ 除第一条外全部转红。
#[test]
fn orphans_are_recognised_wherever_the_binary_lived() {
    let config = runtime_config();
    for program in [
        "/tmp/.mount_PolarisAbC123/usr/lib/Polaris/_up_/resources/linux/sing-box",
        "/tmp/.mount_PolarisZyX987/usr/lib/Polaris/_up_/resources/linux/sing-box",
        "/usr/lib/Polaris/_up_/resources/linux/sing-box",
        "/private/var/folders/ab/T/AppTranslocation/1F2E/d/Polaris.app/Contents/Resources/_up_/resources/mac-arm64/sing-box",
        // 上一版模型的固定位置：升级当次的旧孤儿。
        "/home/u/.config/com.polaris.app/polaris/core_update/sing-box",
        // 提权助手起的那一份。
        "/var/lib/polaris/core/sing-box",
    ] {
        assert!(
            ours(&argv(7, &[program, "run", "-c", &config])),
            "{program} 起的核用的是本 app 的运行配置，必须认得出来"
        );
    }
}

/// 🔴 **核心安全点**：不是本 app 起的 sing-box 绝不能被选中。
///
/// **变异探针**：去掉「配置的父目录 == 配置目录」这一条 ⇒ 前三条转红；去掉核文件名判定 ⇒
/// `not-sing-box` 那条转红；去掉 `run` / `-c` 判定 ⇒ `check` 与 `-D` 两条转红。
#[test]
fn foreign_processes_are_never_ours() {
    let config = runtime_config();
    let foreign: Vec<(&str, CoreProcess)> = vec![
        (
            "用户系统装的 sing-box，跑他自己的配置",
            argv(
                1,
                &[
                    "/usr/bin/sing-box",
                    "run",
                    "-c",
                    "/etc/sing-box/config.json",
                ],
            ),
        ),
        (
            "另一个用户的 Polaris（另一个配置目录）",
            argv(
                1,
                &[
                    "/usr/lib/Polaris/_up_/resources/linux/sing-box",
                    "run",
                    "-c",
                    "/home/other/.config/com.polaris.app/polaris/singbox-runtime.json",
                ],
            ),
        ),
        (
            "配置在本 app 配置目录的**子目录**里（不是直接子项）",
            argv(
                1,
                &[
                    "/usr/bin/sing-box",
                    "run",
                    "-c",
                    &format!("{CONFIG_DIR}/backups/singbox-runtime.json"),
                ],
            ),
        ),
        (
            "名字只是以配置目录为前缀的兄弟目录",
            argv(
                1,
                &[
                    "/usr/bin/sing-box",
                    "run",
                    "-c",
                    &format!("{CONFIG_DIR}-evil/a.json"),
                ],
            ),
        ),
        (
            "程序文件名不是核文件名",
            argv(1, &["/usr/bin/not-sing-box", "run", "-c", &config]),
        ),
        (
            "程序不是绝对路径",
            argv(1, &["sing-box", "run", "-c", &config]),
        ),
        (
            "一次性的 check，不是常驻的 run",
            argv(1, &["/usr/bin/sing-box", "check", "-c", &config]),
        ),
        (
            "参数形状不同（`-c` 不在第三位）",
            argv(
                1,
                &["/usr/bin/sing-box", "run", "-D", "/tmp", "-c", &config],
            ),
        ),
        (
            "只是打开了核文件的进程",
            argv(
                1,
                &["/usr/bin/less", "/opt/polaris/resources/linux/sing-box"],
            ),
        ),
        (
            "任意 `xxx run`",
            argv(1, &["/usr/bin/docker", "run", "hello-world"]),
        ),
        (
            "配置目录下的非 json 文件",
            argv(
                1,
                &[
                    "/usr/bin/sing-box",
                    "run",
                    "-c",
                    &format!("{CONFIG_DIR}/notes.txt"),
                ],
            ),
        ),
        ("空候选", CoreProcess::default()),
    ];
    for (why, process) in &foreign {
        assert!(!ours(process), "{why}");
    }
}

#[test]
fn stale_pids_selects_only_our_cores_excluding_managed() {
    let config = runtime_config();
    let candidates = vec![
        argv(
            100,
            &[
                "/tmp/.mount_old/usr/lib/Polaris/_up_/resources/linux/sing-box",
                "run",
                "-c",
                &config,
            ],
        ),
        // 系统 sing-box → 必须存活。
        argv(
            200,
            &[
                "/usr/bin/sing-box",
                "run",
                "-c",
                "/etc/sing-box/config.json",
            ],
        ),
        // 无关进程。
        argv(300, &["/bin/sleep", "30"]),
        // 本 app 的核，但正是当前受管 pid → 排除，不自杀。
        argv(
            424_242,
            &[
                "/opt/polaris/resources/linux/sing-box",
                "run",
                "-c",
                &config,
            ],
        ),
    ];
    assert_eq!(
        stale_pids(&candidates, CORE, Path::new(CONFIG_DIR), &[424_242]),
        vec![100],
        "只清本 app 起的孤儿（100），排除系统 sing-box（200）/ 无关进程（300）/ 当前受管（424242）"
    );
    assert!(stale_pids(&[], CORE, Path::new(CONFIG_DIR), &[]).is_empty());
}

// ─── macOS `ps` 腿（真机卡死链的扫描侧）────────────────────────────────────────────

const MAC_CONFIG_DIR: &str = "/Users/sway/Library/Application Support/com.polaris.app/polaris";
/// 真机现场那条命令行（程序路径与配置目录都含空格，逐字取自现场的 `pgrep` 输出）。
const MAC_PS_LINE: &str = "  6439   501 /Library/Application Support/Polaris/core/sing-box run -c /Users/sway/Library/Application Support/com.polaris.app/polaris/singbox-runtime.json";

fn mac_ours(process: &CoreProcess) -> bool {
    is_our_core(process, CORE, Path::new(MAC_CONFIG_DIR))
}

/// **真机卡死的扫描侧根因门**：ps 行必须被解析出 pid，且含空格的路径必须仍能匹配。
/// 打断（把 `raw` 按空白切分后走 argv 比对）→ 路径被劈成两段 → 匹配失败 → 本测转红。
#[test]
fn mac_ps_line_with_spaces_in_path_is_recognized_as_our_core() {
    let procs = ps(MAC_PS_LINE);
    assert_eq!(procs.len(), 1, "单行 ps 输出必须解析出一个候选");
    assert_eq!(procs[0].pid, 6439, "pid 必须从行首取出");
    assert!(mac_ours(&procs[0]));
    assert_eq!(
        stale_pids(&procs, CORE, Path::new(MAC_CONFIG_DIR), &[]),
        vec![6439],
        "该孤儿必须进清理名单"
    );
    // 转移运行时给的随机路径、带尾随参数：同样命中。
    assert!(mac_ours(&raw(
        1,
        &format!(
            "/private/var/folders/ab/T/AppTranslocation/1F2E/d/Polaris.app/Contents/Resources/_up_/resources/mac-arm64/sing-box run -c {MAC_CONFIG_DIR}/speedtest-core.json --disable-color"
        ),
    )));
}

/// 安全契约在 raw 腿上同样成立。
#[test]
fn raw_leg_keeps_the_same_safety_contract() {
    let config = format!("{MAC_CONFIG_DIR}/singbox-runtime.json");
    for (why, line) in [
        (
            "系统装的 sing-box 跑自己的配置",
            "/usr/local/bin/sing-box run -c /etc/sing-box/config.json".to_owned(),
        ),
        (
            "只是打开核文件的进程",
            "less /Library/Application Support/Polaris/core/sing-box".to_owned(),
        ),
        (
            "`less` 把一整串当参数（程序不是绝对路径）",
            format!("less /opt/x/sing-box run -c {config}"),
        ),
        (
            "程序文件名不是核文件名",
            format!("/usr/local/bin/sing-box-wrapper run -c {config}"),
        ),
        (
            "一次性的 check",
            format!("/usr/local/bin/sing-box check -c {config}"),
        ),
        (
            "配置在子目录里",
            format!("/usr/local/bin/sing-box run -c {MAC_CONFIG_DIR}/sub/a.json"),
        ),
        (
            "另一个用户的配置目录",
            "/usr/local/bin/sing-box run -c /Users/other/Library/Application Support/com.polaris.app/polaris/singbox-runtime.json".to_owned(),
        ),
        ("空 raw（Linux 侧的候选）", String::new()),
    ] {
        assert!(!mac_ours(&raw(1, &line)), "{why}");
    }
}

// ─── 非 ASCII 路径与读不准的命令行 ──────────────────────────────────────────────

const CJK_CONFIG_DIR: &str = "/Users/张 三/Library/Application Support/com.polaris.app/polaris";

/// 配置目录含中文与空格：`ps` 行解析与判据两层都命中（前提是 `ps` 在 UTF-8 locale 下输出，
/// 见 [`macos_ps`]）；精确 argv 腿同样命中。
#[test]
fn non_ascii_config_dir_is_recognised_on_both_legs() {
    let line = format!(
        "  812   501 /Applications/Polaris.app/Contents/Resources/_up_/resources/mac-arm64/sing-box run -c {CJK_CONFIG_DIR}/singbox-runtime.json --disable-color"
    );
    let procs = ps(&line);
    assert_eq!(procs.len(), 1);
    assert_eq!(
        app_run_config_name(&procs[0], CORE, Path::new(CJK_CONFIG_DIR)),
        Some("singbox-runtime.json")
    );
    assert_eq!(
        stale_pids(&procs, CORE, Path::new(CJK_CONFIG_DIR), &[]),
        vec![812]
    );
    let config = format!("{CJK_CONFIG_DIR}/tailscale-login-ts1-3.json");
    assert!(is_our_core(
        &argv(
            9,
            &[
                "/usr/lib/Polaris/_up_/resources/linux/sing-box",
                "run",
                "-c",
                &config
            ]
        ),
        CORE,
        Path::new(CJK_CONFIG_DIR)
    ));
    // 读得准的行不算「读不准」。
    assert_eq!(unreadable_core_rows(&procs, CORE), 0);
}

/// 🔴 命令行读不准时不认领、也不静默：判据不命中（不能凭一串对不上的路径去杀进程），
/// 但这一行被数出来交调用方留痕。
///
/// 第一行是 C locale 下 `ps` 的输出形态：「张」的三个 UTF-8 字节被 vis 编码成 `M-eM-<M-`。
/// 第二行是不合法 UTF-8 经有损解码后的形态（替换字符）。
///
/// **变异探针**：让 `unreadable_core_rows` 恒返 0 ⇒ 本条转红；把它的内核形状条件去掉
/// （任何含转义的行都算）⇒ 末尾的反例转红。
#[test]
fn garbled_command_lines_are_not_claimed_but_are_counted() {
    let escaped = "  901   501 /Applications/Polaris.app/Contents/Resources/_up_/resources/mac-arm64/sing-box run -c /Users/M-eM-<M- M-dM-8M-^I/Library/Application Support/com.polaris.app/polaris/singbox-runtime.json";
    let lossy =
        "  902   501 /Applications/Polaris.app/Contents/Resources/_up_/resources/mac-arm64/sing-box run -c /Users/\u{FFFD}\u{FFFD} x/Library/Application Support/com.polaris.app/polaris/singbox-runtime.json";
    let procs = ps(&format!("{escaped}\n{lossy}"));
    assert_eq!(
        procs.len(),
        2,
        "读不准的行也必须被解析成候选，不得在解析层丢掉"
    );
    assert!(
        stale_pids(&procs, CORE, Path::new(CJK_CONFIG_DIR), &[]).is_empty(),
        "路径对不上就不认领"
    );
    assert_eq!(unreadable_core_rows(&procs, CORE), 2);

    // Linux 腿：`/proc/<pid>/cmdline` 里的非 UTF-8 字节经有损解码后带替换字符。
    let lossy_argv = argv(
        903,
        &[
            "/usr/lib/Polaris/_up_/resources/linux/sing-box",
            "run",
            "-c",
            "/home/\u{FFFD}\u{FFFD}/.config/com.polaris.app/polaris/singbox-runtime.json",
        ],
    );
    assert!(!is_our_core(&lossy_argv, CORE, Path::new(CONFIG_DIR)));
    assert_eq!(unreadable_core_rows(&[lossy_argv], CORE), 1);

    // 反例：不是内核在跑配置的行，即使带转义也不计；读得准的内核行不计。
    let unrelated = [
        raw(1, "/bin/cat /Users/M-eM-<M- /notes.txt"),
        raw(
            2,
            "/usr/local/bin/sing-box check -c /Users/M-eM-<M- /a.json",
        ),
        raw(
            3,
            "/usr/local/bin/sing-box run -c /etc/sing-box/config.json",
        ),
        argv(4, &["/usr/bin/vim", "/home/\u{FFFD}/a.txt"]),
        argv(
            5,
            &[
                "/usr/bin/sing-box",
                "run",
                "-c",
                "/etc/sing-box/config.json",
            ],
        ),
    ];
    assert_eq!(unreadable_core_rows(&unrelated, CORE), 0);
}

/// 全仓 `ps` 调用的 locale 前缀：两个变量都设成 UTF-8，且 `ps` 用绝对路径。
#[test]
fn macos_ps_prefix_forces_a_utf8_locale() {
    assert_eq!(macos_ps::PROGRAM, "/usr/bin/env");
    assert_eq!(
        macos_ps::ARGV_PREFIX,
        ["LC_ALL=en_US.UTF-8", "LANG=en_US.UTF-8", "/bin/ps"]
    );
}

/// ps 输出的噪声行（表头残留 / 无命令行 / pid 或 uid 非数字 / 缺 uid 列）不得产出候选，也不得 panic。
#[test]
fn ps_parser_skips_malformed_lines() {
    let out = "  PID   UID ARGS\n\n  123\n  124   501\nnotapid   501 /Library/Application Support/Polaris/core/sing-box run\n  125 root /bin/sleep 30\n  126 /bin/sleep 30\n  456   501 /bin/sleep 30\n";
    let procs = ps(out);
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].pid, 456);
    assert!(stale_pids(&procs, CORE, Path::new(MAC_CONFIG_DIR), &[]).is_empty());
}

/// 当前受管 pid 在 raw 腿上同样必须被排除（否则启动期清扫会杀掉自己刚起的核）。
#[test]
fn raw_leg_excludes_currently_managed_pid() {
    let procs = ps(MAC_PS_LINE);
    assert!(
        stale_pids(&procs, CORE, Path::new(MAC_CONFIG_DIR), &[6439]).is_empty(),
        "受管 pid 必须被排除，raw 腿不得绕过 exclude"
    );
}

#[test]
fn proc_status_uid_is_the_effective_one_and_unknown_when_malformed() {
    let status = "Name:\tsing-box\nPid:\t42\nUid:\t1000\t1001\t1000\t1000\nGid:\t7\t7\t7\t7\n";
    assert_eq!(parse_proc_status_uid(status), Some(1001));
    assert_eq!(parse_proc_status_uid("Name:\tx\nGid:\t7\t7\t7\t7\n"), None);
    assert_eq!(parse_proc_status_uid("Uid:\t1000\n"), None);
    assert_eq!(parse_proc_status_uid("Uid:\troot\troot\n"), None);
}

#[test]
fn ps_uid_output_is_one_number_or_unknown() {
    assert_eq!(parse_ps_uid("  501\n"), Some(501));
    assert_eq!(parse_ps_uid(""), None);
    assert_eq!(parse_ps_uid("501 502\n"), None);
}

// ─── Windows 腿：命令行切分 + 路径按 Windows 写法的同一判据（纯函数，各平台同跑）─────────────

const WIN_CORE: &str = "sing-box.exe";
const WIN_CONFIG_DIR: &str = r"C:\Users\A B\AppData\Roaming\com.polaris.app\polaris";
const WIN_PROGRAM: &str = r"C:\Program Files\Polaris\resources\win\sing-box.exe";

/// 一条 Windows 命令行 → 它运行的本 app 配置文件名（走切分 + 判定的完整纯链路）。
fn win_config_name(line: &str) -> Option<String> {
    let argv = split_windows_command_line(line);
    windows_run_config_name(&argv, WIN_CORE, WIN_CONFIG_DIR).map(str::to_owned)
}

fn win_ours(line: &str) -> bool {
    win_config_name(line).is_some_and(|name| name.ends_with(".json"))
}

/// 主核在 Windows 上的命令行：程序与配置路径都含空格，各自被引号包住。
fn win_runtime_line() -> String {
    format!(r#""{WIN_PROGRAM}" run -c "{WIN_CONFIG_DIR}\singbox-runtime.json" --disable-color"#)
}

/// 切分规则逐条对拍（期望值即 C 运行时对同一串的解析结果）。
///
/// **变异探针**：去掉「引号内的空白不分隔」⇒ 第 1、2 条转红；去掉反斜杠折半 ⇒ 第 5、6 条转红。
#[test]
fn windows_command_line_splits_like_the_c_runtime() {
    let cases: &[(&str, &[&str])] = &[
        (
            r#""C:\Program Files\P\sing-box.exe" run -c "C:\Users\A B\x.json" --disable-color"#,
            &[
                r"C:\Program Files\P\sing-box.exe",
                "run",
                "-c",
                r"C:\Users\A B\x.json",
                "--disable-color",
            ],
        ),
        // 程序名未加引号：到第一个空白为止。
        (
            r"C:\P\sing-box.exe run -c C:\cfg\x.json",
            &[r"C:\P\sing-box.exe", "run", "-c", r"C:\cfg\x.json"],
        ),
        // 多个空白与制表符只算一次分隔；首尾空白不产生参数。
        ("  a.exe \t run   -c\tx  ", &["a.exe", "run", "-c", "x"]),
        // 空参数保留。
        (r#"a.exe "" b"#, &["a.exe", "", "b"]),
        // 引号前的反斜杠：偶数个折半后引号照常开闭；奇数个折半后是字面引号。
        (r#"a.exe "C:\dir\\" next"#, &["a.exe", r"C:\dir\", "next"]),
        (r#"a.exe "say \"hi\"" x"#, &["a.exe", r#"say "hi""#, "x"]),
        // 不跟引号的反斜杠是字面量。
        (r"a.exe C:\a\\b\c", &["a.exe", r"C:\a\\b\c"]),
        // 引号内连写两个引号 → 一个字面引号。
        (r#"a.exe "a""b""#, &["a.exe", r#"a"b"#]),
        // 引号贴在参数中间。
        (r#"a.exe pre"mid dle"post"#, &["a.exe", "premid dlepost"]),
        ("", &[]),
        ("   ", &[]),
    ];
    for (line, want) in cases {
        assert_eq!(split_windows_command_line(line), *want, "{line:?}");
    }
}

/// 🔴 Windows 上的本 app 核：含空格的安装目录与用户目录、大小写、两种分隔符都认得出来。
///
/// **变异探针**：把目录比较改成区分大小写 ⇒ 大小写两条转红；把核文件名比较改成区分大小写 ⇒
/// `SING-BOX.EXE` 那条转红；不把 `/` 当分隔符 ⇒ 正斜杠那条转红。
#[test]
fn windows_core_is_recognised_by_the_config_it_runs() {
    assert_eq!(
        win_config_name(&win_runtime_line()).as_deref(),
        Some("singbox-runtime.json")
    );
    for line in [
        win_runtime_line(),
        // 不带尾随参数、都不加引号的最简形态（路径不含空格时 `Command` 仍给程序名加引号，
        // 但别的启动方未必）。
        format!(
            r"C:\P\sing-box.exe run -c {}\x.json",
            WIN_CONFIG_DIR.replace(' ', "")
        ),
        // 安装位置任意：升级前的旧安装目录、提权助手的受保护目录、UNC 路径。
        format!(r#""D:\Old Install\sing-box.exe" run -c "{WIN_CONFIG_DIR}\speedtest-core.json""#),
        format!(r#""C:\ProgramData\Polaris\core\sing-box.exe" run -c "{WIN_CONFIG_DIR}\a.json""#),
        format!(r#""\\server\share\sing-box.exe" run -c "{WIN_CONFIG_DIR}\a.json""#),
        // 文件系统不区分大小写。
        format!(r#""C:\P\SING-BOX.EXE" run -c "{WIN_CONFIG_DIR}\a.json""#),
        format!(
            r#""C:\P\sing-box.exe" run -c "{}\a.json""#,
            WIN_CONFIG_DIR.to_ascii_uppercase()
        ),
        // 正斜杠与反斜杠等价。
        format!(
            r#""C:/P/sing-box.exe" run -c "{}/a.json""#,
            WIN_CONFIG_DIR.replace('\\', "/")
        ),
    ] {
        let dir = if line.contains(&WIN_CONFIG_DIR.replace(' ', "")) && !line.contains("A B") {
            WIN_CONFIG_DIR.replace(' ', "")
        } else {
            WIN_CONFIG_DIR.to_owned()
        };
        let argv = split_windows_command_line(&line);
        assert!(
            windows_run_config_name(&argv, WIN_CORE, &dir).is_some_and(|n| n.ends_with(".json")),
            "{line}"
        );
    }
    // 配置目录带尾随分隔符时同样命中。
    let argv = split_windows_command_line(&win_runtime_line());
    assert!(windows_run_config_name(&argv, WIN_CORE, &format!(r"{WIN_CONFIG_DIR}\")).is_some());
}

/// 🔴 安全契约在 Windows 腿上同样成立：不是本 app 起的进程绝不能被选中。
///
/// **变异探针**：去掉目录相等 ⇒ 前四条转红；去掉核文件名判定 ⇒ `sing-box-wrapper.exe` 转红；
/// 去掉 `run` / `-c` ⇒ `check` 与 `-D` 两条转红；去掉绝对路径判定 ⇒ 相对路径两条转红。
#[test]
fn windows_leg_keeps_the_same_safety_contract() {
    let config = format!(r"{WIN_CONFIG_DIR}\singbox-runtime.json");
    for (why, line) in [
        (
            "用户自己装的 sing-box 跑他自己的配置",
            r#""C:\Tools\sing-box.exe" run -c "C:\Tools\config.json""#.to_owned(),
        ),
        (
            "另一个用户的配置目录",
            r#""C:\P\sing-box.exe" run -c "C:\Users\Other\AppData\Roaming\com.polaris.app\polaris\singbox-runtime.json""#.to_owned(),
        ),
        (
            "配置在本 app 配置目录的子目录里",
            format!(r#""C:\P\sing-box.exe" run -c "{WIN_CONFIG_DIR}\backups\a.json""#),
        ),
        (
            "名字只是以配置目录为前缀的兄弟目录",
            format!(r#""C:\P\sing-box.exe" run -c "{WIN_CONFIG_DIR}-evil\a.json""#),
        ),
        (
            "程序文件名不是核文件名",
            format!(r#""C:\P\sing-box-wrapper.exe" run -c "{config}""#),
        ),
        (
            "核文件名少了扩展名",
            format!(r#""C:\P\sing-box" run -c "{config}""#),
        ),
        (
            "程序不是绝对路径",
            format!(r#"sing-box.exe run -c "{config}""#),
        ),
        (
            "程序是相对路径",
            format!(r#"bin\sing-box.exe run -c "{config}""#),
        ),
        (
            "盘符相对路径（没有根）",
            format!(r#"C:sing-box.exe run -c "{config}""#),
        ),
        (
            "一次性的 check",
            format!(r#""C:\P\sing-box.exe" check -c "{config}""#),
        ),
        (
            "参数形状不同（`-c` 不在第三位）",
            format!(r#""C:\P\sing-box.exe" run -D C:\tmp -c "{config}""#),
        ),
        (
            "子命令大小写不同（本 app 只会写小写）",
            format!(r#""C:\P\sing-box.exe" RUN -c "{config}""#),
        ),
        (
            "只是打开了核文件的进程",
            r#""C:\Windows\notepad.exe" "C:\P\sing-box.exe""#.to_owned(),
        ),
        (
            "别的程序把整串当成一个带引号的参数",
            format!(r#""C:\Windows\System32\cmd.exe" /c "C:\P\sing-box.exe run -c {config}""#),
        ),
        (
            "配置目录下的非 json 文件",
            format!(r#""C:\P\sing-box.exe" run -c "{WIN_CONFIG_DIR}\notes.txt""#),
        ),
        (
            "配置路径就是配置目录本身（没有文件名）",
            format!(r#""C:\P\sing-box.exe" run -c "{WIN_CONFIG_DIR}\\""#),
        ),
        ("没有参数", r#""C:\P\sing-box.exe""#.to_owned()),
        ("空命令行", String::new()),
    ] {
        assert!(!win_ours(&line), "{why}: {line}");
    }
}

/// 应答解码：头里的指针指向同一块缓冲区内部；越界、奇数长度、指针在缓冲区之外一律读不到。
///
/// **变异探针**：去掉「起点 + 长度不越界」检查 ⇒ 越界那条 panic；去掉 `base` 换算 ⇒ 第一条转红。
#[test]
fn command_line_reply_is_decoded_only_when_it_points_inside_itself() {
    const POINTER: usize = std::mem::size_of::<usize>();
    const BASE: usize = 0x1000_0000;
    let text: Vec<u16> =
        r#""C:\P\sing-box.exe" run -c "C:\U\配置\x.json""#.encode_utf16().collect();
    let reply = |length: u16, pointer: usize| {
        let mut bytes = vec![0u8; 2 * POINTER];
        bytes[..2].copy_from_slice(&length.to_ne_bytes());
        bytes[POINTER..].copy_from_slice(&pointer.to_ne_bytes());
        bytes.extend(text.iter().flat_map(|unit| unit.to_ne_bytes()));
        bytes
    };
    let byte_len = (text.len() * 2) as u16;
    let data = BASE + 2 * POINTER;

    assert_eq!(
        command_line_from_reply(&reply(byte_len, data), BASE).as_deref(),
        Some(r#""C:\P\sing-box.exe" run -c "C:\U\配置\x.json""#)
    );
    // 空命令行（系统进程）是合法应答。
    assert_eq!(
        command_line_from_reply(&reply(0, data), BASE).as_deref(),
        Some("")
    );
    for (why, bytes, base) in [
        ("长度越过缓冲区末尾", reply(byte_len + 2, data), BASE),
        ("奇数长度", reply(byte_len - 1, data), BASE),
        ("指针在缓冲区之前", reply(byte_len, BASE - 8), BASE),
        ("指针在缓冲区之后", reply(byte_len, BASE + 0x10_0000), BASE),
        ("空指针", reply(byte_len, 0), BASE),
        ("应答短于头", vec![0u8; POINTER], BASE),
    ] {
        assert_eq!(command_line_from_reply(&bytes, base), None, "{why}");
    }
}

/// 公开入口按**配置目录的写法**选腿，与宿主无关：给一个 Windows 写法的配置目录，在任何宿主上
/// 走的都是 Windows 腿。
///
/// **变异探针**：把选腿条件改回 `cfg!(windows)` ⇒ 本条在非 Windows 宿主上转红，
/// POSIX 样例的那些用例在 Windows 宿主上转红。
#[test]
fn public_criterion_picks_the_leg_from_the_config_dir_spelling() {
    let process = CoreProcess {
        pid: 1,
        cmdline: split_windows_command_line(&win_runtime_line()),
        owner: ProcessOwner::SameUser,
        ..CoreProcess::default()
    };
    assert_eq!(
        app_run_config_name(&process, WIN_CORE, Path::new(WIN_CONFIG_DIR)),
        Some("singbox-runtime.json")
    );
    assert!(is_our_core(
        &process,
        "SING-BOX.EXE",
        Path::new(WIN_CONFIG_DIR)
    ));
    assert_eq!(
        stale_pids(&[process], WIN_CORE, Path::new(WIN_CONFIG_DIR), &[1]),
        Vec::<u32>::new()
    );
}

// ─── 属主：形状之外的第二条判据 ───────────────────────────────────────────────────

/// uid → 属主归类。先比「是不是自己」，再比 root；读不到的归未知。
///
/// **变异探针**：把 `(Some(0), _)` 那一臂挪到最前 ⇒ 「本进程自己是 root」那条转红；
/// 把兜底改成 `SameUser` ⇒ 后三条转红。
#[test]
fn unix_owner_is_classified_relative_to_this_process() {
    use ProcessOwner::{Elevated, Other, SameUser, Unknown};
    for (owner, me, want) in [
        (Some(1000), Some(1000), SameUser),
        (Some(0), Some(1000), Elevated),
        // 本进程自己以 root 跑：root 的进程就是同一用户。
        (Some(0), Some(0), SameUser),
        (Some(1001), Some(1000), Other),
        // 读不到对方的属主。
        (None, Some(1000), Unknown),
        // 读不到自己的：没有「自己」可比，只有 root 还认得出来。
        (Some(1000), None, Unknown),
        (Some(0), None, Elevated),
        (None, None, Unknown),
    ] {
        assert_eq!(owner_from_uid(owner, me), want, "{owner:?} vs {me:?}");
    }
    assert!(
        SameUser.sweepable() && Elevated.sweepable() && !Other.sweepable() && !Unknown.sweepable()
    );
    assert_eq!(
        ProcessOwner::default(),
        Unknown,
        "没有属主事实的候选不是清扫对象"
    );
}

/// Windows 令牌事实 → 属主归类。令牌打不开（普通权限看别的用户或 SYSTEM 的进程）归未知。
///
/// **变异探针**：把 `None` 归成 `SameUser` ⇒ 末条转红；交换前两臂的先后 ⇒ 第三条转红。
#[test]
fn windows_owner_is_classified_from_token_facts() {
    use ProcessOwner::{Elevated, Other, SameUser, Unknown};
    let facts = |same_user, local_system| {
        Some(TokenUserFacts {
            same_user: Some(same_user),
            local_system,
        })
    };
    assert_eq!(owner_from_token(facts(true, false)), SameUser);
    assert_eq!(owner_from_token(facts(false, true)), Elevated);
    // 本进程自己以 SYSTEM 跑。
    assert_eq!(owner_from_token(facts(true, true)), SameUser);
    assert_eq!(owner_from_token(facts(false, false)), Other);
    assert_eq!(owner_from_token(None), Unknown);
    assert_eq!(
        owner_from_token(Some(TokenUserFacts {
            same_user: None,
            local_system: false,
        })),
        Unknown,
        "自身令牌不可读，不能证明候选是其他用户"
    );
    assert_eq!(
        owner_from_token(Some(TokenUserFacts {
            same_user: None,
            local_system: true,
        })),
        Elevated,
        "SYSTEM 身份独立可验证，不依赖自身令牌"
    );
}

/// `ps` 的 uid 列相对本进程那一行归类；输出里没有本进程时只认得出 root。
#[test]
fn ps_rows_carry_the_owner_relative_to_this_process() {
    let lines = "  10   501 /bin/a\n  11     0 /bin/b\n  12   502 /bin/c\n";
    let owners = |procs: &[CoreProcess]| -> Vec<(u32, ProcessOwner)> {
        procs.iter().map(|p| (p.pid, p.owner)).collect()
    };
    assert_eq!(
        owners(&ps(lines)),
        [
            (10, ProcessOwner::SameUser),
            (11, ProcessOwner::Elevated),
            (12, ProcessOwner::Other),
        ]
    );
    assert_eq!(
        owners(&parse_ps_output(lines, SELF_PID)),
        [
            (10, ProcessOwner::Unknown),
            (11, ProcessOwner::Elevated),
            (12, ProcessOwner::Unknown),
        ],
        "输出里没有本进程 ⇒ 没有「自己」可比"
    );
}

/// 🔴 **别的用户照着写一条同形命令行，不是清扫对象**。三种路径写法各验一遍，判据是同一条。
///
/// 同机另一个用户常驻 `<…/核名> run -c <本用户配置目录>/<x>.json`（配置路径只是参数，不需要
/// 那个文件可读）。它不进清扫名单，只出现在 `foreign` 里供调用方留痕；本用户的孤儿与提权助手
/// 起的核照常进名单。
///
/// **变异探针**：去掉 `stale_cores` 里的属主分拨（全部进 `sweep`）⇒ 三种写法全部转红。
#[test]
fn a_lookalike_owned_by_another_user_is_never_swept() {
    let with_owner = |mut process: CoreProcess, owner| {
        process.owner = owner;
        process
    };
    let posix = runtime_config();
    let mac = format!(
        "/Library/Application Support/Polaris/core/sing-box run -c {MAC_CONFIG_DIR}/singbox-runtime.json"
    );
    let win = split_windows_command_line(&win_runtime_line());
    let shapes: [(&str, &str, &str, CoreProcess); 3] = [
        (
            "argv（POSIX）",
            CORE,
            CONFIG_DIR,
            argv(0, &["/usr/bin/sing-box", "run", "-c", &posix]),
        ),
        ("ps 整串", CORE, MAC_CONFIG_DIR, raw(0, &mac)),
        (
            "argv（Windows）",
            WIN_CORE,
            WIN_CONFIG_DIR,
            CoreProcess {
                cmdline: win,
                ..CoreProcess::default()
            },
        ),
    ];
    for (leg, core, dir, shape) in shapes {
        let candidates: Vec<CoreProcess> = [
            (1, ProcessOwner::SameUser),
            (2, ProcessOwner::Elevated),
            (3, ProcessOwner::Other),
            (4, ProcessOwner::SameUser),
            (5, ProcessOwner::Unknown),
            (6, ProcessOwner::Unknown),
        ]
        .into_iter()
        .map(|(pid, owner)| {
            let mut process = with_owner(shape.clone(), owner);
            process.pid = pid;
            process
        })
        .collect();
        assert!(
            candidates
                .iter()
                .all(|c| is_our_core(c, core, Path::new(dir))),
            "{leg}：六个候选的命令行形状都命中（前提）"
        );
        let selected = stale_cores(&candidates, core, Path::new(dir), &[4, 6]);
        assert_eq!(
            selected.sweep.iter().map(|p| p.pid).collect::<Vec<_>>(),
            [1, 2],
            "{leg}：本用户的与 root / SYSTEM 的进名单；别人的不进；排除表里的不进"
        );
        assert_eq!(selected.foreign, [3], "{leg}：已确认别人的那个只供留痕");
        assert_eq!(
            selected.unknown,
            [5],
            "{leg}：不可读者单列，不误报外来或已清扫"
        );
        assert_eq!(
            stale_pids(&candidates, core, Path::new(dir), &[4, 6]),
            [1, 2]
        );
    }
}

/// 结束前的身份复核：扫描时与结束前两次创建时间都读到且相等才算同一个进程。
///
/// **变异探针**：把判据改成「任一没读到就放行」⇒ 后三条转红。
#[test]
fn a_process_is_the_same_only_when_both_creation_times_agree() {
    assert!(same_birth(
        Some(0x01DB_0000_1234_5678),
        Some(0x01DB_0000_1234_5678)
    ));
    assert!(!same_birth(Some(1), Some(2)), "pid 上换了进程");
    assert!(!same_birth(None, Some(2)), "扫描时没读到");
    assert!(!same_birth(Some(1), None), "结束前没读到");
    assert!(!same_birth(None, None));
}

/// POSIX 目录比较不经 `std::path`：连写与尾随的分隔符不计，其余逐段全等。
#[test]
fn posix_config_dir_is_compared_segment_by_segment() {
    let core = |config: &str| argv(1, &["/usr/bin/sing-box", "run", "-c", config]);
    let dir = Path::new(CONFIG_DIR);
    assert!(is_our_core(
        &core(&format!("{CONFIG_DIR}//a.json")),
        CORE,
        dir
    ));
    assert!(is_our_core(
        &core(&format!("{CONFIG_DIR}/a.json")),
        CORE,
        Path::new(&format!("{CONFIG_DIR}/"))
    ));
    for (why, config) in [
        (
            "相对路径",
            "home/u/.config/com.polaris.app/polaris/a.json".to_owned(),
        ),
        ("配置路径就是配置目录本身", format!("{CONFIG_DIR}/")),
        ("没有目录部分", "a.json".to_owned()),
        (
            "大小写不同（POSIX 区分大小写）",
            format!("{}/a.json", CONFIG_DIR.to_uppercase()),
        ),
        ("反斜杠不是 POSIX 分隔符", format!("{CONFIG_DIR}\\a.json")),
    ] {
        assert!(!is_our_core(&core(&config), CORE, dir), "{why}");
    }
    // 整串腿同理：配置目录带尾随分隔符时照样命中。
    assert!(is_our_core(
        &raw(1, &format!("/usr/bin/sing-box run -c {CONFIG_DIR}/a.json")),
        CORE,
        Path::new(&format!("{CONFIG_DIR}/"))
    ));
}

/// An unreadable owner is lack of evidence, not proof of a foreign user.
#[test]
fn unreadable_owner_is_not_evidence_for_another_user() {
    assert_ne!(owner_from_token(None), ProcessOwner::Other);
    assert_ne!(owner_from_uid(None, Some(1000)), ProcessOwner::Other);
    assert_ne!(owner_from_uid(Some(1000), None), ProcessOwner::Other);
}
