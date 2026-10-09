use super::*;

// ── protoVersion 契约（**前提已换**）─────────────────────────────────────────
//
// 旧测 `proto_versions_match_polaris` / `platforms_are_distinct` 断言 9/5/1 且三者互异，前提是
// 「三平台各自演进的独立谱系必须原样移植」。那个前提是 **上游的历史包袱**：9/5/1 只是三套独立
// Go module 各自加过多少次功能的计数，唯一用途是让新 client 认出机器上那代旧 helper。Polaris 是
// 全新产品 + 全新 Rust helper，**不存在任何一代旧 Polaris helper 需要被认出** ⇒ 三谱系无对象、
// 「必须互异」更是把别人的演进史写成自己的不变量（它会主动阻止本该做的统一）。
//
// 新前提：版本号只表达「wire 断代」，平台差异由 `Platform`（帧结构）+ `command`（命令集）表达。
// 故三平台共用一个 `CURRENT`，下面两测锁的是**统一**而非互异。

#[test]
fn proto_version_is_unified_v1() {
    assert_eq!(
        proto_version::CURRENT,
        1,
        "首次正式发布前的兼容命令扩展不构成协议断代"
    );
}

// 曾有一条 `proto_version_does_not_vary_by_platform`：遍历四个 `Platform` 反复断言
// `Response::Ok(Pong{ proto_version: CURRENT, .. }).to_wire_line()` 的版本段。**已删** ——
// 循环变量只出现在断言消息里，`advertised` 由常量 `CURRENT` 算出、与平台无关 ⇒ 四次迭代是同一
// 个断言的四份副本，语义等价于 `CURRENT == 1`（上面那条已覆盖）；它自称能拦「有人按 Platform
// match 返不同值」，可新增的那个函数**根本不会被它调用**，拦不住。
//
// 「不得 per-platform 分叉」的真锚点在**分叉真会发生的地方** —— 三个平台各自的 `PROTO_VERSION`
// 常量（cfg 门控模块，helper-proto 这层遍历不到），每处一条字面量断言：
//   · `platform::macos::mod.rs`   `proto_version_is_unified_current`
//   · `platform::windows::mod.rs` `proto_version_is_unified_current`
//   · `platform::linux::handler.rs` `wire_forms_match_protocol`（钉死当前 protocol）
// `to_wire_line` 的 Pong 形态另由 `crates/helper-proto/src/response::to_wire_line_matches_go_source_literals` 覆盖。

#[test]
fn platform_carries_frame_shape_not_version() {
    // 推翻旧前提的正面表述：三平台**唯一**的协议差异是帧结构（token 行有无），不是版本号。
    // 同一个 Request 在 mac/linux 下编出的帧不同 —— 差异由 Platform 承载，版本号无需分叉。
    let req = Request::Ping;
    let mac = String::from_utf8(codec::encode(Platform::Mac, "TOK", &req)).unwrap();
    let linux = String::from_utf8(codec::encode(Platform::Linux, "", &req)).unwrap();
    assert_eq!(mac, "TOK\nping\n", "mac 带 token 行");
    assert_eq!(linux, "ping\n", "linux 走 SO_PEERCRED，无 token 行");
    assert_ne!(mac, linux, "平台差异体现在帧结构上");
}

/// Windows helper 会合点的绝对值金标。helper 与 helper-client 都引 [`windows_helper`] 的同一份
/// 常量，两侧之间不可能再分叉；剩下的风险是「这一份被改了」——而已部署的 helper 服务名、管道名、
/// 目录都烧在用户机器上（SCM 注册、ImagePath、ACL），改值 = 新 app 找不到旧 helper。故判据写死
/// 字面量：引常量的话常量一改判据跟着漂。
#[test]
fn windows_helper_rendezvous_is_pinned() {
    assert_eq!(windows_helper::SERVICE_NAME, "PolarisHelper");
    assert_eq!(windows_helper::PIPE_NAME, r"\\.\pipe\polaris-helper");
    assert_eq!(
        windows_helper::DEFAULT_SUPPORT_DIR,
        r"C:\ProgramData\Polaris"
    );
}

/// 受保护核目录的文件名白名单：每个平台只有核与配套库两个**精确名**。字面量写死在这里：
/// 它们同时是打包脚本落位的文件名、安装脚本播种的文件名。
#[test]
fn core_payload_names_are_exact_per_platform() {
    use core_payload::{core_filename, is_sidecar_name, name_allowed, sidecar_filename};

    assert_eq!(core_filename(Platform::Linux), "sing-box");
    assert_eq!(core_filename(Platform::Mac), "sing-box");
    assert_eq!(core_filename(Platform::Win), "sing-box.exe");
    assert_eq!(sidecar_filename(Platform::Linux), Some("libcronet.so"));
    assert_eq!(sidecar_filename(Platform::Win), Some("libcronet.dll"));
    assert_eq!(sidecar_filename(Platform::Mac), None);
    for platform in [Platform::Android, Platform::Ios, Platform::Other] {
        assert_eq!(sidecar_filename(platform), None, "{platform:?}");
    }

    for platform in Platform::ALL.iter().copied() {
        assert!(name_allowed(core_filename(platform), platform));
        if let Some(sidecar) = sidecar_filename(platform) {
            assert!(name_allowed(sidecar, platform));
            assert!(is_sidecar_name(sidecar, platform));
        }
        // 核文件名不是配套库名。
        assert!(!is_sidecar_name(core_filename(platform), platform));

        for bad in [
            "",
            ".",
            "..",
            "libcronet",
            "libcronet.",
            "libcronet.so.",
            "libcronet.dll.",
            "libcronet.so.119",
            "libcronet.dylib",
            ".libcronet.so",
            ".sing-box",
            "sing-box.",
            "sing-box.exe.",
            "sing-box.bak",
            "sing-box.new",
            "LIBCRONET.so",
            "libcronet..so",
            "libcronet.so/..",
            "libcronet.a/b",
            r"libcronet.a\b",
            "../libcronet.so",
            "libcronet.dll:stream",
            "libcronet.so copy",
            "libcronet.so\n",
            "libcronet.s\u{f6}",
            ".core-seed.json",
        ] {
            assert!(!name_allowed(bad, platform), "{bad:?} on {platform:?}");
            assert!(!is_sidecar_name(bad, platform), "{bad:?} on {platform:?}");
        }
    }

    // 别的平台的名字在本平台就是白名单外的名字。
    assert!(!name_allowed("sing-box.exe", Platform::Linux));
    assert!(!name_allowed("sing-box", Platform::Win));
    assert!(!name_allowed("libcronet.dll", Platform::Linux));
    assert!(!name_allowed("libcronet.so", Platform::Win));
    assert!(!name_allowed("libcronet.so", Platform::Mac));
}

#[test]
fn build_identity_is_a_single_safe_wire_token() {
    assert!(build_identity::is_wire_safe(build_identity::current()));
    assert!(!build_identity::is_wire_safe(""));
    assert!(!build_identity::is_wire_safe("sha with spaces"));
    assert!(!build_identity::is_wire_safe("sha\nsecond-line"));
}

#[test]
fn platform_token_line_semantics() {
    // mac/win 带 token 行；linux 经 SO_PEERCRED 不带（helper-linux/helper.go:333-343）
    assert!(Platform::Mac.has_token_line());
    assert!(Platform::Win.has_token_line());
    assert!(!Platform::Linux.has_token_line());
    // Other 视同 Linux：未知平台无 helper 实现，保守不带 token 行。
    assert!(!Platform::Other.has_token_line());
    // Android 同理：无 helper、无 daemon 可鉴权。
    assert!(!Platform::Android.has_token_line());

    // **全变体穷举**（不是「列几个我想到的」）：ALL 是唯一取材面，新增变体自动进这条断言。
    let with_token: Vec<Platform> = Platform::ALL
        .iter()
        .copied()
        .filter(|p| p.has_token_line())
        .collect();
    assert_eq!(
        with_token,
        vec![Platform::Mac, Platform::Win],
        "带 token 行的平台集合变了。这不是风格问题：多一个平台 = 向一个**未鉴权对端**发送\
         凭据行；少一个 = 该平台的 helper 收不到 token 直接拒。改这里必须有 helper 侧的对应改动。"
    );
}

/// [`Platform::ALL`] 必须真的覆盖每一个变体。
///
/// 强制力来自下面这个**穷举 match**：新增变体时它编译不过，作者被迫回来加一行；而那一行
/// 又要求对应变体出现在 `ALL` 里，否则断言红。`ALL` 是本仓所有「全变体」判据（含
/// `src-tauri/tests/platform_dispatch_exhaustive.rs` 那道门）的取材面，它漏一个变体，
/// 那个变体就不受任何全变体判据管辖。
#[test]
fn platform_all_covers_every_variant() {
    for probe in [
        Platform::Mac,
        Platform::Win,
        Platform::Linux,
        Platform::Android,
        Platform::Ios,
        Platform::Other,
    ] {
        // 穷举 match：新增变体 ⇒ 此处 E0004 ⇒ 必须回来补。
        let named = match probe {
            Platform::Mac => "Mac",
            Platform::Win => "Win",
            Platform::Linux => "Linux",
            Platform::Android => "Android",
            Platform::Ios => "Ios",
            Platform::Other => "Other",
        };
        assert!(
            Platform::ALL.contains(&probe),
            "{named} 不在 Platform::ALL 里 —— 补上，否则它逃出所有全变体判据"
        );
    }
    assert_eq!(
        Platform::ALL.len(),
        6,
        "ALL 长度变了：新增变体请同步本断言与上面的穷举 match（两处都改才算真的加进来了）"
    );
}

#[test]
fn platform_current_matches_compile_target() {
    // current() 由编译 target 决定；CI 本机 Linux → Linux。
    let cur = Platform::current();
    if cfg!(target_os = "macos") {
        assert_eq!(cur, Platform::Mac);
    } else if cfg!(target_os = "windows") {
        assert_eq!(cur, Platform::Win);
    } else if cfg!(target_os = "android") {
        // Android 的 `target_os` 是 "android" 而非 "linux"，故必须排在 linux 之前判。
        // 这条腿在本机（Linux）跑不到，靠 `--target aarch64-linux-android` 的交叉门覆盖编译面。
        assert_eq!(cur, Platform::Android);
    } else if cfg!(target_os = "ios") {
        // iOS 的 `target_os` 是 "ios" 而非 "macos"，与上面 macos 那支互斥。
        // **本条今天在任何门里都跑不到**：本仓构不出 iOS 产物，`gate-rust.sh` 的三目标
        // cross-clippy 也不含 apple-ios。它在这里是为了让「current() 缺 ios 分支」这件事至少
        // 有一处代码写着它该是什么 —— 不是收据，是待兑现的判据。见本批报告「本批未证实」。
        assert_eq!(cur, Platform::Ios);
    } else if cfg!(target_os = "linux") {
        assert_eq!(cur, Platform::Linux);
    } else {
        assert_eq!(cur, Platform::Other);
    }
}

#[test]
fn platform_parse_maps_known_strings() {
    // 对齐 上游 `process.platform` 口径 + 兼容各处历史传参写法。
    assert_eq!(Platform::parse("darwin"), Platform::Mac);
    assert_eq!(Platform::parse("macos"), Platform::Mac);
    assert_eq!(Platform::parse("win32"), Platform::Win);
    assert_eq!(Platform::parse("windows"), Platform::Win);
    assert_eq!(Platform::parse("linux"), Platform::Linux);
    // "android" = `std::env::consts::OS` 在 Android 上的原值，config-engine 直传给本函数。
    // 它此前落进 `Other`，靠「log builder 视 Other 同 Linux」顺带得到正确的日志落盘行为；
    // 给 Android 具名后必须由本条覆盖，否则那条腿断掉（回归而非旧缺陷）。
    assert_eq!(Platform::parse("android"), Platform::Android);
    // "ios" = `std::env::consts::OS` 在 iOS 上的原值（Node 的 `process.platform` 同名，
    // 不像 darwin/win32 那样有第二套写法）。它此前落进 `Other`：那条腿在**字符串轴**上顺带
    // 给对了一部分答案（未知平台 = 桌面口径），也顺带给错了另一部分（见本批 inbounds 的
    // mixed 入站与回环排除两格）。具名之后两部分都必须逐处显式答，本条只钉桥本身。
    assert_eq!(Platform::parse("ios"), Platform::Ios);
    // 未知串 → Other（非 std FromStr，不报错）。
    assert_eq!(Platform::parse("freebsd"), Platform::Other);
    assert_eq!(Platform::parse(""), Platform::Other);
}

/// 端到端往返：Request → encode → Response::parse 应覆盖典型路径。
/// 这是「core 发、helper 收」的 wire 兼容性最关键的契约 —— 锁住编码/解码对称。
#[test]
fn end_to_end_wire_roundtrip_ping() {
    let req = Request::Ping;
    let bytes = codec::encode(Platform::Mac, "TOK", &req);
    let wire = String::from_utf8(bytes).unwrap();
    // 模拟 helper 回复 ping
    let resp = Response::parse("OK pong uid=0 v9");
    assert!(matches!(resp, Response::Ok(ResponseKind::Pong(_))));
    // wire 形态断言
    assert_eq!(wire, "TOK\nping\n");
}

/// start 完整往返：args 顺序 + fwd 字符串化 + ppid 可选行。
#[test]
fn end_to_end_start_roundtrip() {
    let req = Request::Start(StartParams {
        cfg: "/tmp/c.json".into(),
        log: "".into(),
        fwd: true,
        parent_pid: Some(999),
    });
    // mac 帧
    let mac_bytes = codec::encode(Platform::Mac, "T", &req);
    assert_eq!(
        String::from_utf8(mac_bytes).unwrap(),
        "T\nstart\n/tmp/c.json\n\n1\n999\n"
    );
    // linux 帧（无 token 行，但 LinuxStart 多 singbox 行）
    let lreq = Request::LinuxStart(LinuxStartParams {
        singbox_path: "/core/sing-box".into(),
        common: StartParams {
            cfg: "/tmp/c.json".into(),
            log: "".into(),
            fwd: false,
            parent_pid: None,
        },
    });
    let linux_bytes = codec::encode(Platform::Linux, "", &lreq);
    assert_eq!(
        String::from_utf8(linux_bytes).unwrap(),
        "start-reap-safe\n/core/sing-box\n/tmp/c.json\n\n0\n"
    );
}
