//! 线协议解码器 ⇔ 分派表 的结构性对齐门，以及 `handle_frame` 的顺序判据。
//!
//! 为什么要有：`windows/helper/tests/mod.rs` 其余测试全部直接把 [`Request`] 结构体喂给
//! [`WinHelper::handle`]，**绕过了解码器**。批一 `flush-dns`、批三 `install-core` 两次都是
//! 「分派加了、解码没加」，测试全绿、真机 0 字节 / 233。解码器与分派表之间的缝就是生产路径。

use super::*;
use crate::platform::windows::logic;
use polaris_helper_proto::codec;
use polaris_helper_proto::{ErrorCode, InstallCoreParams, Platform, RouteParams};

const TOKEN: &str = "real-token";

/// 同一张表同时生成「每个变体一份样本」与「无 `_` 臂的穷举 match」。
///
/// 新增 [`Request`] 变体而不在下表补一行 ⇒ `variant_name` 的 match 不穷举 ⇒ E0004 编译错。
/// 样本与名字同源于一行，标签错配由门里的 `variant_name(sample) == label` 自检兜住。
macro_rules! request_samples {
    ($($variant:ident => $sample:expr,)*) => {
        fn request_samples() -> Vec<(&'static str, Request)> {
            vec![$((stringify!($variant), $sample),)*]
        }

        fn variant_name(req: &Request) -> &'static str {
            match req {
                $(Request::$variant { .. } => stringify!($variant),)*
            }
        }
    };
}

request_samples! {
    Ping => Request::Ping,
    Version => Request::Version,
    Status => Request::Status,
    Stop => Request::Stop { pid: Some(4242) },
    Cleanup => Request::Cleanup,
    FreePort => Request::FreePort { port: 9090 },
    Start => Request::Start(StartParams {
        cfg: r"C:\Users\polaris\config\config.json".to_owned(),
        log: r"C:\Users\polaris\config\core.log".to_owned(),
        fwd: true,
        parent_pid: Some(4242),
    }),
    LinuxStart => Request::LinuxStart(polaris_helper_proto::LinuxStartParams {
        singbox_path: "/usr/lib/polaris/core/sing-box".to_owned(),
        common: StartParams {
            cfg: "/home/u/.config/polaris/config.json".to_owned(),
            log: String::new(),
            fwd: false,
            parent_pid: None,
        },
    }),
    RouteAdd => Request::RouteAdd(RouteParams {
        iface: "polaris-tun0".to_owned(),
        cidrs: vec!["10.0.0.0/8".to_owned(), "fd00::/8".to_owned()],
    }),
    RouteDel => Request::RouteDel(RouteParams {
        iface: "polaris-tun0".to_owned(),
        cidrs: vec!["172.16.0.0/12".to_owned()],
    }),
    // src 指向不存在的目录：分派会在读 sing-box.exe 那步失败，一个字节都不写（且 support 是 tempdir）。
    InstallCore => Request::InstallCore(InstallCoreParams {
        src_dir: r"C:\Users\polaris\AppData\Roaming\Polaris\core_promote".to_owned(),
        want_hash: "ab".repeat(32),
    }),
    LinuxDnsSet => Request::LinuxDnsSet(polaris_helper_proto::LinuxDnsSetParams {
        interface_name: "polaris-tun".to_owned(),
        server_ip: "172.19.0.2".to_owned(),
    }),
    LinuxDnsRevert => Request::LinuxDnsRevert {
        interface_name: "polaris-tun".to_owned(),
    },
    DefaultRestore => Request::DefaultRestore {
        gateway_ipv4: "192.168.1.1".to_owned(),
    },
    FlushDns => Request::FlushDns,
    MacProxyTransaction => Request::MacProxyTransaction {
        payload_hex: "7b7d".to_owned(),
    },
    MacProxyCompareTransaction => Request::MacProxyCompareTransaction {
        payload_hex: "7b7d".to_owned(),
    },
    MacProxyCompareCapability => Request::MacProxyCompareCapability,
    IfaceMetric => Request::IfaceMetric {
        iface: "polaris-tun0".to_owned(),
        metric: 999,
    },
    Uninstall => Request::Uninstall,
}

/// 每次调用一个全新 helper：start 会记账 pid，留着会让后面的 install-core 回 busy。
fn fresh_helper(
    support: &std::path::Path,
) -> WinHelper<StaticTokenStore, MockProcOps, MockNetTableOps> {
    make_helper_with_support(MockProcOps::new(), &support.to_string_lossy())
}

fn is_unknown(out: &HandleOutcome) -> bool {
    matches!(out, HandleOutcome::Respond(Response::Err(e)) if e.code == ErrorCode::Unknown)
}

/// **判据**：对 `Request` 的每一个变体，「经 `codec::encode(Platform::Win, …)` 编码的真实字节，
/// 走生产同一条切行 → 解码流程后**无损**还原成原请求」当且仅当「`WinHelper::handle` 对它不回
/// `ErrorCode::Unknown`」。两个方向都红：分派了但解不出（批一/批三），解得出但分派回 unknown。
///
/// 「解得出」取**无损往返**（`parse_request(..) == Some(原请求)`），不取 `is_some()`：
/// `LinuxStart` 的 wire 命令也是 `start`，Windows 解码器会把它解成（字段错位的）`Start` ——
/// 那不叫认识 `LinuxStart`；而一个本该支持的变体若往返有损（丢行、错位），同样应当红。
///
/// 第三条腿：同一份字节喂生产入口 [`WinHelper::handle_frame`]，其是否回 `ERR unknown` 必须与
/// 「解码结果 + 分派」的结论一致 —— 证明门测的切行/解码就是 `handle_frame` 用的那一份。
#[test]
fn decoder_and_dispatch_agree_on_every_request_variant() {
    let support = tempfile::tempdir().unwrap();
    let samples = request_samples();
    let mut failures: Vec<String> = Vec::new();
    let mut both_supported = 0usize;
    let mut both_unsupported = 0usize;

    for (label, req) in &samples {
        // 判据自检：样本表的标签与样本本身是同一个变体。
        assert_eq!(variant_name(req), *label, "样本表标签错配");

        let dispatched = !is_unknown(&fresh_helper(support.path()).handle(TOKEN, req.clone()));

        let raw = String::from_utf8(codec::encode(Platform::Win, TOKEN, req))
            .expect("编码帧必须是 UTF-8");
        let frame = logic::split_frame(&raw)
            .unwrap_or_else(|| panic!("{label}: 编码帧切不出 token/命令行：{raw:?}"));
        assert_eq!(frame.token, TOKEN, "{label}: 切行取错了 token 行");
        assert_eq!(
            frame.command,
            req.command_name(),
            "{label}: 切行取错了命令行"
        );
        let decoded = logic::parse_request(frame.command, &frame.args);
        let lossless = decoded.as_ref() == Some(req);

        if lossless != dispatched {
            failures.push(format!(
                "{label}: 解码无损={lossless}（解出 {decoded:?}）但分派{}",
                if dispatched {
                    "支持它 ⇒ 生产上永远够不着这条分支（客户端读到 0 字节 / 233）"
                } else {
                    "回 ErrorCode::Unknown ⇒ 解码器认了一条分派不认的命令"
                }
            ));
        }
        match (lossless, dispatched) {
            (true, true) => both_supported += 1,
            (false, false) => both_unsupported += 1,
            _ => {}
        }

        // 生产入口与上面的拆解结论一致。
        let expect_unknown = match &decoded {
            None => true,
            Some(d) => is_unknown(&fresh_helper(support.path()).handle(TOKEN, d.clone())),
        };
        let reply = fresh_helper(support.path()).handle_frame(&raw);
        if (reply.line == "ERR unknown\n") != expect_unknown {
            failures.push(format!(
                "{label}: handle_frame 回 {:?}，与「解码 {decoded:?} + 分派」的结论（unknown={expect_unknown}）不一致",
                reply.line
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "解码器与分派表不对齐：\n{}",
        failures.join("\n")
    );
    // 正面断言：两侧都真有样本落进判定，不是「全部恰好不支持」这种空绿。
    assert_eq!(
        both_supported + both_unsupported,
        samples.len(),
        "每个变体都必须落进一致的一格"
    );
    assert!(
        both_supported >= 12,
        "Windows 两侧都支持的变体只有 {both_supported} 个：取材面塌了"
    );
    assert!(
        both_unsupported >= 1,
        "没有任何两侧都不支持的变体：反向格失去对照"
    );
}

// ===== handle_frame 的顺序判据（先验 token，再判命令）=====

/// token 不对时，对已知 / 未知 / 参数解不出的命令，响应必须逐字节相同（且就是鉴权失败那条腿）。
///
/// 旧顺序（先解码、再验 token）下这三格分别是 `ERR auth` / `ERR unknown` / `ERR unknown`：
/// 未鉴权对端只要换命令名就能探出 helper 认识哪些命令。
#[test]
fn bad_token_reply_is_indistinguishable_across_known_and_unknown_commands() {
    let h = make_helper_defaults();
    let install = String::from_utf8(codec::encode(
        Platform::Win,
        "wrong-token",
        &Request::InstallCore(InstallCoreParams {
            src_dir: r"C:\x".to_owned(),
            want_hash: "ab".repeat(32),
        }),
    ))
    .unwrap();
    let frames = [
        "wrong-token\nping\n".to_owned(),
        "wrong-token\nno-such-command\n".to_owned(),
        "wrong-token\nfreeport\nnot-a-port\n".to_owned(),
        "wrong-token\nsystem-proxy-transaction\n7b7d\n".to_owned(),
        install,
        "\nping\n".to_owned(),
        "\nno-such-command\n".to_owned(),
    ];
    let expected = FrameReply {
        line: "ERR auth\n".to_owned(),
        flush: FlushMode::NoWait,
        exit_after: false,
    };
    for raw in &frames {
        assert_eq!(
            h.handle_frame(raw),
            expected,
            "token 不对时响应随命令而变：{raw:?}"
        );
    }
}

/// 已鉴权而命令未知（或参数连类型都解不出）⇒ `ERR unknown` + **WaitPeer**（可靠送达）。
///
/// 这一行是 app 判「这个 helper 不支持某命令」的唯一依据；NoWait 下它会被随后的
/// `DisconnectNamedPipe` 丢掉，app 只看到 0 字节 / 233。
#[test]
fn authed_unknown_command_is_delivered_with_wait_peer() {
    let h = make_helper_defaults();
    for raw in [
        "real-token\nno-such-command\n",
        "real-token\nfreeport\nnot-a-port\n",
        "real-token\nsystem-proxy-transaction\n7b7d\n",
    ] {
        assert_eq!(
            h.handle_frame(raw),
            FrameReply {
                line: "ERR unknown\n".to_owned(),
                flush: FlushMode::WaitPeer,
                exit_after: false,
            },
            "{raw:?}"
        );
    }
}

/// 帧行数不足（连 token/命令都凑不齐）维持现状：`ERR unknown` + NoWait。
#[test]
fn short_frame_keeps_the_no_wait_unknown_reply() {
    let h = make_helper_defaults();
    for raw in ["real-token\n", "real-token", ""] {
        assert_eq!(
            h.handle_frame(raw),
            FrameReply {
                line: "ERR unknown\n".to_owned(),
                flush: FlushMode::NoWait,
                exit_after: false,
            },
            "{raw:?}"
        );
    }
}

/// 正面：已鉴权的已知命令照常分派、WaitPeer；uninstall 带上自退标志（其余不带）。
#[test]
fn authed_known_commands_are_dispatched_with_wait_peer() {
    let h = make_helper_defaults();
    let ping = h.handle_frame("real-token\nping\n");
    assert!(ping.line.starts_with("OK pong uid=0 v"), "{ping:?}");
    assert!(ping.line.ends_with('\n'));
    assert_eq!(ping.flush, FlushMode::WaitPeer);
    assert!(!ping.exit_after);

    let uninstall = make_helper_defaults().handle_frame("real-token\nuninstall\n");
    assert_eq!(uninstall.line, "OK uninstalling\n");
    assert_eq!(uninstall.flush, FlushMode::WaitPeer);
    assert!(uninstall.exit_after, "uninstall 必须让 service 层自退");
}

/// 生产的 `handle_connection` 真的走 `handle_frame`，且没有第二条自己切行 / 解码 / 分派的旁路。
///
/// `service/win.rs` 整模块 `cfg(windows)`，本机不编译；`crate_source!` 读磁盘文本不受 cfg 影响。
/// 编译面由 msvc 交叉 clippy 兜。
#[test]
fn production_handle_connection_goes_through_handle_frame() {
    let src = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    let code = polaris_source_probe::mask_comments_and_strings(&src);
    assert_eq!(
        code.matches("fn handle_connection<").count(),
        1,
        "handle_connection 切点不唯一 / 消失"
    );
    let at = code.find("fn handle_connection<").unwrap();
    let end = code[at..]
        .find("\nfn read_frame(")
        .map(|i| at + i)
        .expect("handle_connection 之后的 read_frame 切点消失");
    let body = &code[at..end];
    // 切片自检：窗口确实盖住了读帧与写回。
    assert!(body.contains("read_frame(h, &mut buf)"), "窗口没盖住读帧");
    assert!(body.contains("cleanup_pipe(h)"), "窗口没盖住断开");

    assert_eq!(
        body.matches("helper.handle_frame(&raw)").count(),
        1,
        "handle_connection 不再经 handle_frame 处理整帧"
    );
    assert!(
        body.contains("write_response(h, reply.line.as_bytes(), reply.flush)"),
        "写回没有照 handle_frame 给的行与 flush 档位"
    );
    for bypass in [
        "parse_request",
        "split_frame",
        ".lines()",
        ".handle(",
        "FlushMode::",
    ] {
        assert!(
            !body.contains(bypass),
            "handle_connection 里出现旁路 `{bypass}`：顺序判据会被绕开"
        );
    }
    // 整个文件不得再留一份解码器（两份解码器 = 下一次只改了一份）。
    assert!(
        !code.contains("fn parse_request"),
        "service/win.rs 又长出了自己的 parse_request"
    );
}
