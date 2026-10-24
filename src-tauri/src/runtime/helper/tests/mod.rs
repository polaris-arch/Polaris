use super::*;
use crate::commands::guard_scan::impl_method_body;
use crate::test_support::{crate_code, TestDir};

use std::sync::atomic::Ordering;
use std::sync::Mutex;

struct SequenceConnector {
    streams: Mutex<Vec<polaris_helper_client::MockStream>>,
    connects: Arc<std::sync::atomic::AtomicUsize>,
    frames: Arc<Mutex<Vec<String>>>,
}

impl Connector for SequenceConnector {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let mut streams = self.streams.lock().unwrap();
        if streams.is_empty() {
            return Err(ClientError::Connect("测试连接已耗尽".to_owned()));
        }
        Ok(Box::new(RecordingStream {
            inner: streams.remove(0),
            frames: Arc::clone(&self.frames),
        }))
    }
}

fn stop_test_client(
    streams: Vec<polaris_helper_client::MockStream>,
) -> (HelperClient, Arc<std::sync::atomic::AtomicUsize>) {
    let (client, connects, _frames) = stop_test_client_with_frames(Platform::Win, streams);
    (client, connects)
}

fn stop_test_client_with_frames(
    platform: Platform,
    streams: Vec<polaris_helper_client::MockStream>,
) -> (
    HelperClient,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<Mutex<Vec<String>>>,
) {
    let connects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let connector = SequenceConnector {
        streams: Mutex::new(streams),
        connects: Arc::clone(&connects),
        frames: Arc::clone(&frames),
    };
    (
        HelperClient::new(Box::new(connector), platform, "TOK"),
        connects,
        frames,
    )
}

fn runtime() -> (HelperRuntime, TestDir) {
    let dir = TestDir::new("polaris-helper-test-");
    // **不用 `HelperRuntime::new`**：那条走 StdSysOps + 系统路径，会把门的绿绑在「跑测的机器上
    // 没装过 Polaris」这个宿主前提上（且装了就会真连特权 daemon）。见 `never_installed_for_tests`。
    (HelperRuntime::never_installed_for_tests(dir.clone()), dir)
}

fn birth_target(pid: u32, wire: &str) -> HelperBirthTarget {
    HelperBirthTarget::parse_wire(&pid.to_string(), wire).unwrap()
}

#[test]
fn pinned_receipt_family_is_explicit_for_every_platform() {
    use polaris_helper_proto::core_protocol::PinnedBirthStart;
    let target = birth_target(4242, "00112233445566778899aabbccddeeff");
    let linux = LinuxBirthStart::Already { target };
    let native = NativeBirthStart::Already { target };
    for &platform in Platform::ALL {
        let linux_result = pinned_receipt_response(platform, PinnedBirthStart::Linux(linux));
        let native_result = pinned_receipt_response(platform, PinnedBirthStart::Native(native));
        assert_eq!(linux_result.is_ok(), platform == Platform::Linux);
        assert_eq!(
            native_result.is_ok(),
            matches!(platform, Platform::Mac | Platform::Win)
        );
        for result in [linux_result, native_result] {
            if let Err(error) = result {
                assert!(error.starts_with("HELPER_CORE_MISMATCH:"));
            }
        }
    }
}

#[test]
fn exact_start_accepts_only_birth_receipts_and_never_promotes_already() {
    let target = birth_target(4242, "00112233445566778899aabbccddeeff");
    assert_eq!(
        classify_linux_birth_start_response(Response::parse(
            "OK birth-started 4242 00112233445566778899aabbccddeeff"
        )),
        Ok(HelperStartResult::BirthStarted(target))
    );
    assert_eq!(
        classify_linux_birth_start_response(Response::parse(
            "OK birth-already 4242 00112233445566778899aabbccddeeff"
        )),
        Ok(HelperStartResult::BirthAlready(target))
    );
    assert_eq!(
        classify_linux_birth_start_response(Response::parse(
            "OK birth-start-not-admitted pending 4242 00112233445566778899aabbccddeeff"
        )),
        Ok(HelperStartResult::BirthNotAdmitted {
            target: Some(target),
            pending: true
        })
    );
    assert_eq!(
        classify_linux_birth_start_response(Response::parse("OK birth-start-not-admitted unknown")),
        Ok(HelperStartResult::BirthNotAdmitted {
            target: None,
            pending: false
        })
    );
    for wire in [
        "OK started 4242",
        "OK birth-started 4242 11112222333344445555666677778888 extra=identity",
        "ERR unknown",
    ] {
        assert!(classify_linux_birth_start_response(Response::parse(wire)).is_err());
    }
}

#[test]
fn exact_stop_retries_the_same_birth_after_lost_ack_and_pending() {
    let target = birth_target(4242, "00112233445566778899aabbccddeeff");
    let (client, connects, frames) = stop_test_client_with_frames(
        Platform::Linux,
        vec![
            polaris_helper_client::MockStream::with_response(Vec::new()),
            polaris_helper_client::MockStream::with_response(
                b"OK birth-stop-pending 4242 00112233445566778899aabbccddeeff\n".to_vec(),
            ),
            polaris_helper_client::MockStream::with_response(
                b"OK birth-stopped 4242 00112233445566778899aabbccddeeff\n".to_vec(),
            ),
        ],
    );
    stop_birth_with_client_budget(
        &client,
        target,
        Duration::from_millis(200),
        Duration::from_millis(1),
    )
    .unwrap();
    assert_eq!(connects.load(Ordering::SeqCst), 3);
    assert_eq!(
        *frames.lock().unwrap(),
        vec!["stop-birth-safe\n4242\n00112233445566778899aabbccddeeff\n".to_owned(); 3]
    );
}

#[test]
fn exact_stop_rejects_same_pid_other_birth_and_legacy_ack() {
    let target = birth_target(4242, "00112233445566778899aabbccddeeff");
    for wire in [
        b"OK birth-stopped 4242 11112222333344445555666677778888\n".to_vec(),
        b"OK stopped 4242\n".to_vec(),
        b"OK birth-stop-unknown 4242 00112233445566778899aabbccddeeff\n".to_vec(),
        b"OK birth-stop-mismatch 4242 00112233445566778899aabbccddeeff 4242 11112222333344445555666677778888\n".to_vec(),
        b"ERR unknown\n".to_vec(),
    ] {
        let (client, connects, _) = stop_test_client_with_frames(
            Platform::Linux,
            vec![polaris_helper_client::MockStream::with_response(wire)],
        );
        assert!(stop_birth_with_client_budget(
            &client,
            target,
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .is_err());
        assert_eq!(connects.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn exact_pending_exhausts_one_budget_without_releasing_birth() {
    let target = birth_target(4242, "00112233445566778899aabbccddeeff");
    let streams = (0..32)
        .map(|_| {
            polaris_helper_client::MockStream::with_response(
                b"OK birth-stop-pending 4242 00112233445566778899aabbccddeeff\n".to_vec(),
            )
        })
        .collect();
    let (client, _, _) = stop_test_client_with_frames(Platform::Linux, streams);
    let started = std::time::Instant::now();
    let error = stop_birth_with_client_budget(
        &client,
        target,
        Duration::from_millis(20),
        Duration::from_millis(5),
    )
    .unwrap_err();
    assert!(error.contains("未取得 native reap"), "{error}");
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn exact_status_does_not_accept_legacy_or_invent_no_owner() {
    let (client, _, frames) = stop_test_client_with_frames(
        Platform::Linux,
        vec![polaris_helper_client::MockStream::with_response(
            b"OK birth-status running 4242 00112233445566778899aabbccddeeff\n".to_vec(),
        )],
    );
    assert_eq!(
        managed_core_status_with_client(&client),
        Ok(ManagedCoreStatus::BirthRunning {
            target: birth_target(4242, "00112233445566778899aabbccddeeff")
        })
    );
    assert_eq!(*frames.lock().unwrap(), ["status-birth-safe\n".to_owned()]);
    let (client, _, _) = stop_test_client_with_frames(
        Platform::Linux,
        vec![polaris_helper_client::MockStream::with_response(
            b"OK birth-status empty\n".to_vec(),
        )],
    );
    assert_eq!(
        managed_core_status_with_client(&client),
        Ok(ManagedCoreStatus::BirthEmpty),
        "an empty daemon-local status is not a no-owner proof"
    );
    for wire in [b"OK running 4242\n".to_vec(), b"ERR unknown\n".to_vec()] {
        let (client, _, _) = stop_test_client_with_frames(
            Platform::Linux,
            vec![polaris_helper_client::MockStream::with_response(wire)],
        );
        assert!(managed_core_status_with_client(&client).is_err());
    }
}

#[test]
fn linux_exact_capability_probe_is_read_only_and_old_helper_fails_before_start() {
    for (wire, capable) in [
        (b"OK birth-status empty\n".to_vec(), true),
        (b"ERR unknown\n".to_vec(), false),
        (b"OK stopped\n".to_vec(), false),
        // 新助手认识探测命令、但这一次没答上来：带尾文的 `ERR unknown` 不是「助手太旧」。
        (b"ERR unknown birth-custody-unidentified\n".to_vec(), false),
        (
            b"ERR unknown poisoned lock: another task failed inside\n".to_vec(),
            false,
        ),
        (b"ERR unauthorized\n".to_vec(), false),
    ] {
        let unknown_command = wire == b"ERR unknown\n";
        let (client, connects, frames) = stop_test_client_with_frames(
            Platform::Linux,
            vec![polaris_helper_client::MockStream::with_response(wire)],
        );
        let probed = require_linux_birth_capability_with_client(&client);
        assert_eq!(probed.is_ok(), capable);
        // 只有「不认识这条命令」的确定回答算助手太旧；别的不通过只是此刻不可用。
        assert_eq!(
            matches!(probed, Err(BirthCapabilityError::HelperOutdated(_))),
            unknown_command
        );
        assert_eq!(connects.load(Ordering::SeqCst), 1);
        assert_eq!(*frames.lock().unwrap(), ["status-birth-safe\n".to_owned()]);
    }
}

#[test]
fn native_platform_capability_and_stop_require_their_own_exact_receipt_family() {
    for platform in [Platform::Mac, Platform::Win] {
        for (wire, capable) in [
            ("OK native-birth-status empty", true),
            ("OK native-birth-status unknown", false),
            (
                "OK native-birth-status unknown 4242 00112233445566778899aabbccddeeff",
                false,
            ),
            (
                "OK native-birth-status stopping 4242 00112233445566778899aabbccddeeff",
                false,
            ),
            ("OK stopped", false),
            ("OK birth-status empty", false),
            ("ERR unknown", false),
            // Windows 收到行数不足的帧、协议层折进来的任意错误：都带尾文，都不是「助手太旧」。
            ("ERR unknown bad-frame", false),
            ("ERR unknown pipe closed before reply", false),
            ("ERR auth", false),
        ] {
            let (client, _, frames) = stop_test_client_with_frames(
                platform,
                vec![polaris_helper_client::MockStream::with_response(
                    format!("{wire}\n").into_bytes(),
                )],
            );
            let probed = require_native_birth_capability_with_client(&client);
            assert_eq!(probed.is_ok(), capable);
            // 同上：`ERR unknown` 才是「助手太旧」；应答了别的协议族、custody 未清都不是。
            assert_eq!(
                matches!(probed, Err(BirthCapabilityError::HelperOutdated(_))),
                wire == "ERR unknown",
                "{wire}"
            );
            assert_eq!(*frames.lock().unwrap(), ["TOK\nstatus-native-birth-safe\n"]);
        }
        let target = birth_target(4242, "00112233445566778899aabbccddeeff");
        for (wire, confirmed) in [
            (
                "OK native-birth-stopped 4242 00112233445566778899aabbccddeeff",
                true,
            ),
            (
                "OK birth-stopped 4242 00112233445566778899aabbccddeeff",
                false,
            ),
            (
                "OK native-birth-stopped 4242 ffeeddccbbaa99887766554433221100",
                false,
            ),
            (
                "OK native-birth-stop-unknown 4242 00112233445566778899aabbccddeeff",
                false,
            ),
            ("OK notrunning", false),
        ] {
            let (client, _, frames) = stop_test_client_with_frames(
                platform,
                vec![polaris_helper_client::MockStream::with_response(
                    format!("{wire}\n").into_bytes(),
                )],
            );
            assert_eq!(
                stop_birth_with_client_budget(
                    &client,
                    target,
                    Duration::from_millis(100),
                    Duration::from_millis(1)
                )
                .is_ok(),
                confirmed
            );
            assert_eq!(
                *frames.lock().unwrap(),
                ["TOK\nstop-native-birth-safe\n4242\n00112233445566778899aabbccddeeff\n"]
            );
        }
    }
}

#[test]
fn stop_retries_once_when_the_first_roundtrip_loses_its_response() {
    let (client, connects) = stop_test_client(vec![
        polaris_helper_client::MockStream::broken(std::io::ErrorKind::BrokenPipe),
        polaris_helper_client::MockStream::with_response(b"OK notrunning\n".to_vec()),
    ]);

    stop_core_with_client(&client, Some(4242))
        .expect_err("legacy notrunning cannot attest this owned birth after lost response");
    assert_eq!(connects.load(Ordering::SeqCst), 2, "通信错误后只应补发一次");
}

#[test]
fn legacy_stop_polls_pending_but_plain_stopped_is_not_birth_exit_proof() {
    let (client, connects, frames) = stop_test_client_with_frames(
        Platform::Linux,
        vec![
            polaris_helper_client::MockStream::with_response(b"OK stop-pending 4242\n".to_vec()),
            polaris_helper_client::MockStream::with_response(b"OK stop-pending 4242\n".to_vec()),
            polaris_helper_client::MockStream::with_response(b"OK stopped 4242\n".to_vec()),
        ],
    );
    stop_core_with_client_budget(
        &client,
        Some(4242),
        Duration::from_millis(200),
        Duration::from_millis(1),
    )
    .expect_err("plain stopped only acknowledges the legacy request");
    assert_eq!(connects.load(Ordering::SeqCst), 3);
    assert_eq!(
        *frames.lock().unwrap(),
        vec!["stop-reap-safe\n4242\n".to_owned(); 3],
        "every Linux Pending retry must retain the atomic capability command"
    );
}

#[test]
fn legacy_mac_windows_stop_ack_and_empty_status_never_confirm_native_exit() {
    for platform in [Platform::Mac, Platform::Win] {
        for wire in [
            b"OK stopped 4242\n".to_vec(),
            b"OK notrunning\n".to_vec(),
            b"OK stopped\n".to_vec(),
        ] {
            let (client, connects, frames) = stop_test_client_with_frames(
                platform,
                vec![polaris_helper_client::MockStream::with_response(wire)],
            );
            assert!(stop_core_with_client(&client, Some(4242)).is_err());
            assert_eq!(connects.load(Ordering::SeqCst), 1);
            assert_eq!(*frames.lock().unwrap(), ["TOK\nstop\n4242\n".to_owned()]);
        }
    }
}

#[test]
fn stop_unknown_fails_closed_without_retry() {
    let (client, connects) =
        stop_test_client(vec![polaris_helper_client::MockStream::with_response(
            b"OK stop-unknown 4242\n".to_vec(),
        )]);
    let error = stop_core_with_client_budget(
        &client,
        Some(4242),
        Duration::from_millis(200),
        Duration::from_millis(1),
    )
    .unwrap_err();
    assert!(error.contains("未知") && error.contains("4242"));
    assert_eq!(connects.load(Ordering::SeqCst), 1);
}

#[test]
fn old_linux_helper_refusal_does_not_fall_back_or_claim_reap() {
    let (client, connects, frames) = stop_test_client_with_frames(
        Platform::Linux,
        vec![polaris_helper_client::MockStream::with_response(
            b"ERR unknown\n".to_vec(),
        )],
    );

    let error = stop_core_with_client(&client, Some(4242)).unwrap_err();

    assert!(error.contains("原生 birth 安全协议"), "{error}");
    assert!(error.contains("升级或修复"), "{error}");
    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "unknown is a capability refusal, not a retry/fallback trigger"
    );
    assert_eq!(
        *frames.lock().unwrap(),
        ["stop-reap-safe\n4242\n".to_owned()],
        "Linux must not fall back to the legacy stop token"
    );
}

#[test]
fn old_linux_helper_start_refusal_has_an_upgrade_path() {
    let error = format_helper_mutation_error(
        Platform::Linux,
        "起核",
        &polaris_helper_proto::Error::new(polaris_helper_proto::ErrorCode::Unknown),
    );
    assert!(error.contains("已拒绝起核"), "{error}");
    assert!(error.contains("升级或修复"), "{error}");
    // 带尾文的 `ERR unknown` 是这一次的故障，不引导用户去升级助手。
    let error = format_helper_mutation_error(
        Platform::Linux,
        "起核",
        &polaris_helper_proto::Error::with_detail(
            polaris_helper_proto::ErrorCode::Unknown,
            "poisoned lock",
        ),
    );
    assert!(!error.contains("升级或修复"), "{error}");
    assert!(error.contains("poisoned lock"), "{error}");
}

/// 🔴 只有**裸的** `ERR unknown` 说明助手不认识这条命令；每一种来源各钉一行。
///
/// 左列是服务端实际写出的那一行（Linux `dispatch_locked` / `handle` / `handle_birth_status`，
/// macOS `server.rs` 的解码失败臂，Windows `handle_frame`，`core_install::to_response`，以及
/// 协议层的 `From<Box<dyn Error>>`），各自在 helper 侧另有用例钉住字面。
///
/// **变异探针**：把判据退回只比错误码 ⇒ 带尾文的五行全部转红。
#[test]
fn only_a_bare_unknown_reply_means_the_helper_lacks_the_command() {
    for (wire, lacks_command, source) in [
        ("ERR unknown", true, "三平台：命令名不在解码表 / 分派表里"),
        ("ERR unknown ", true, "尾随空白不算尾文"),
        (
            "ERR unknown poisoned lock: another task failed inside",
            false,
            "Linux：处理器的状态锁中毒",
        ),
        (
            "ERR unknown birth-custody-unidentified",
            false,
            "Linux：状态探测遇到认不出归属的 custody",
        ),
        ("ERR unknown bad-frame", false, "Windows：帧行数不足"),
        (
            "ERR unknown install-result",
            false,
            "install-core：结果行解析不出",
        ),
        (
            "ERR unknown connection reset by peer",
            false,
            "协议层把任意错误折成 unknown",
        ),
        ("ERR auth", false, "别的错误码"),
        ("ERR bad-args", false, "别的错误码"),
    ] {
        let error = polaris_helper_proto::Error::parse(wire).expect("ERR 行");
        assert_eq!(
            is_unknown_command_reply(&error),
            lacks_command,
            "{source}：{wire}"
        );
    }
    // 协议层的折叠确实带尾文（否则上面那一行钉的是一个不存在的形态）。
    let folded: polaris_helper_proto::Error =
        (Box::<dyn std::error::Error + Send + Sync>::from("connection reset by peer")).into();
    assert_eq!(
        folded.to_wire_line(),
        "ERR unknown connection reset by peer"
    );
    assert!(!is_unknown_command_reply(&folded));
}

#[test]
fn pending_stop_respects_one_total_budget() {
    let streams = (0..32)
        .map(|_| {
            polaris_helper_client::MockStream::with_response(b"OK stop-pending 4242\n".to_vec())
        })
        .collect();
    let (client, _) = stop_test_client(streams);
    let started = std::time::Instant::now();
    let error = stop_core_with_client_budget(
        &client,
        Some(4242),
        Duration::from_millis(20),
        Duration::from_millis(5),
    )
    .unwrap_err();
    assert!(error.contains("物理 reap"), "{error}");
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "each Pending response must not receive a fresh timeout budget"
    );
}

#[test]
fn stop_does_not_retry_a_structured_pid_mismatch() {
    let (client, connects) =
        stop_test_client(vec![polaris_helper_client::MockStream::with_response(
            b"OK stop-mismatch 4242 9001\n".to_vec(),
        )]);

    let error = stop_core_with_client(&client, Some(4242)).unwrap_err();
    assert!(error.contains("4242") && error.contains("9001"));
    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "helper 已明确拒杀新会话时不得重发"
    );
}

#[test]
fn stop_requires_an_exact_stop_receipt() {
    for wire in [b"OK running 4242\n".to_vec(), b"OK stopped 9001\n".to_vec()] {
        let (client, connects) =
            stop_test_client(vec![polaris_helper_client::MockStream::with_response(wire)]);
        assert!(stop_core_with_client(&client, Some(4242)).is_err());
        assert_eq!(connects.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn managed_core_status_parses_native_birth_metadata_and_empty_view() {
    let target = birth_target(4242, "00112233445566778899aabbccddeeff");
    for (wire, expected) in [
        (b"OK native-birth-status running 4242 00112233445566778899aabbccddeeff\n".to_vec(), ManagedCoreStatus::NativeBirthRunning { target, created: None, image: None }),
        (b"OK native-birth-status running 4242 00112233445566778899aabbccddeeff created=133600000000000000 image=433a5c50726f6772616d2046696c65735c506f6c617269735c73696e672d626f782e657865\n".to_vec(), ManagedCoreStatus::NativeBirthRunning { target, created: Some(133_600_000_000_000_000), image: Some(r"C:\Program Files\Polaris\sing-box.exe".into()) }),
        (b"OK native-birth-status empty\n".to_vec(), ManagedCoreStatus::BirthEmpty),
        (b"OK native-birth-status unknown\n".to_vec(), ManagedCoreStatus::BirthUnidentified),
    ] {
        let (client, connects) = stop_test_client(vec![polaris_helper_client::MockStream::with_response(wire)]);
        assert_eq!(managed_core_status_with_client(&client), Ok(expected));
        assert_eq!(connects.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn managed_core_status_rejects_an_unrelated_success_response() {
    let (client, _) = stop_test_client(vec![polaris_helper_client::MockStream::with_response(
        b"OK notrunning\n".to_vec(),
    )]);
    let error = managed_core_status_with_client(&client).unwrap_err();
    assert!(error.contains("有效回执"));
}

#[test]
fn platform_supported_maps_all_platforms() {
    // 三平台均有提权 helper → 卡片可达（deriveState 的 `!s.supported` 不再恒真，前端落 installed/
    // none/needs-* 真实态而非 unsupported）。全变体断言给 mac/win/linux 值变异牙齿：任何漏平台的
    // 变异（如 `Mac | Win`、`Mac | Linux`、`Win | Linux`）在本机 Linux gate 即失败，无需真机跨平台。
    assert!(
        platform_supported(Platform::Mac),
        "macOS 有提权 helper（LaunchDaemon）"
    );
    assert!(
        platform_supported(Platform::Win),
        "Windows 有提权 helper（SCM）"
    );
    assert!(
        platform_supported(Platform::Linux),
        "Linux 有提权 helper（systemd + AmbientCaps；对齐 should_start_via_helper）"
    );
    assert!(
        !platform_supported(Platform::Other),
        "未知平台无 helper 实现 → unsupported 正确"
    );
    assert!(
        !platform_supported(Platform::Android),
        "Android 无 helper：核在应用进程内跑，没有 daemon、没有提权通道；\
         tun fd 由 VpnService 授予，不需要提权动作。这条 false 是整个 helper 子系统在\
         该平台不可达的**唯一**入口闸（安装脚本 / 提权 / LinuxStart 帧全挂在它后面）。"
    );

    // 全变体穷举：有 helper 的就是那三个，多一个少一个都红。
    let supported: Vec<Platform> = Platform::ALL
        .iter()
        .copied()
        .filter(|p| platform_supported(*p))
        .collect();
    assert_eq!(
        supported,
        vec![Platform::Mac, Platform::Win, Platform::Linux],
        "helper 支持面变了。它必须与 `runtime/proxy::should_start_via_helper` 的平台集合逐值\
         相同（两处是同一个问题的两份写法），改一处就要对差另一处。"
    );
}

/// **跨谓词不变式**：有受保护核目录 ⇒ 该平台有 helper。
///
/// 受保护核目录是 helper 的产物（root 安装脚本建、helper 的 `install-core` 写）。两个谓词住在
/// 两个模块里、各自有各自的全变体测试，但**在此之前没有任何东西断言它们同向** —— 而 2026-09-05
/// 之前它们确实是矛盾的：`platform_has_protected_core(Other) == true` 且
/// `platform_supported(Other) == false`，即「这个平台没有 helper，但它有一个由 helper 锁定的核目录」。
///
/// 判据是**蕴含**而不是相等：P4 之前 Windows 有 helper 但核走 app 侧（无受保护目录）。P4 起 Win
/// 也有受保护核目录，两谓词在全部变体上相等 —— 仍只断言蕴含，因为「有 helper 而无受保护目录」
/// 本身不矛盾，将来某个平台合法地落进那一格时不该被本门拦下。
///
/// 正面断言（不能只写「不许出现矛盾」）：蕴含成立的那一侧必须真的非空 —— 若两个谓词同时坏成
/// 全 false，蕴含会平凡成立，那种绿没有信息量。
#[test]
fn protected_core_platforms_are_a_subset_of_helper_platforms() {
    let platform_has_protected_core =
        |platform| matches!(platform, Platform::Linux | Platform::Mac | Platform::Win);

    let mut both = Vec::new();
    for platform in Platform::ALL.iter().copied() {
        if platform_has_protected_core(platform) {
            assert!(
                platform_supported(platform),
                "{platform:?}：判了「有受保护核目录」却判「无 helper」—— 受保护核目录由 helper \
                 创建并锁定，两者不可能一边有一边没有。改动方须先决定哪一边错了。"
            );
            both.push(platform);
        }
    }
    assert_eq!(
        both,
        vec![Platform::Mac, Platform::Win, Platform::Linux],
        "蕴含的非空侧必须是 mac/win/linux。两个谓词同时坏成全 false 时蕴含会平凡成立，\
         这条相等断言就是那种无信息量绿的正面对照。"
    );
}

#[test]
fn helper_install_success_requires_compatible_protocol_and_selected_content() {
    let current = HelperStatusSnapshot {
        selected_core: Some(SelectedCoreSnapshot {
            id: "a".repeat(64),
            version: "1.15.0-alpha.12".into(),
            provider_fingerprint: "b".repeat(64),
            state: SelectedCoreState::Present,
        }),
        ready: true,
        upgradeable: false,
        ..Default::default()
    };
    assert!(helper_install_reached_expected_build(&current));
    let mut unknown = current.clone();
    unknown.selected_core = None;
    assert!(!helper_install_reached_expected_build(&unknown));
    for state in [
        SelectedCoreState::Absent,
        SelectedCoreState::Invalid,
        SelectedCoreState::Unavailable,
    ] {
        let mut bad = current.clone();
        bad.selected_core.as_mut().unwrap().state = state;
        assert!(!helper_install_reached_expected_build(&bad));
    }

    let old_same_proto = HelperStatusSnapshot {
        ready: true,
        upgradeable: true,
        ..Default::default()
    };
    assert!(
        !helper_install_reached_expected_build(&old_same_proto),
        "旧 helper 虽可用，也不能被安装动作谎报为已升级"
    );

    assert!(!helper_install_reached_expected_build(
        &HelperStatusSnapshot::default()
    ));
}

#[test]
fn status_supported_reflects_platform() {
    let (rt, _d) = runtime();
    // supported 随平台；未装（替身恒报不存在）→ 其余全 false（compute_status 先判 is_installed
    // 短路，不连 socket）。**未装态由注入的替身给定，不再取决于跑测机器装没装过 Polaris。**
    let s = rt.status();
    assert_eq!(s.supported, cfg!(any(unix, windows)));
    assert!(!s.installed, "替身报不存在 → not installed");
    assert!(!s.ready);
    assert!(!s.needs_repair, "未安装 ≠ needs_repair");
}

#[test]
fn mac_proxy_connect_failure_is_the_only_transport_fallback() {
    use polaris_system_integration::proxy_ops::MacProxyWriterError;

    assert!(matches!(
        classify_mac_proxy_client_error(ClientError::Connect("missing".into())),
        MacProxyWriterError::Unavailable(_)
    ));
    for error in [ClientError::Timeout, ClientError::EmptyResponse] {
        assert!(matches!(
            classify_mac_proxy_client_error(error),
            MacProxyWriterError::Failed(_)
        ));
    }
    assert!(matches!(
        classify_mac_proxy_client_error(ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "written then closed",
        ))),
        MacProxyWriterError::Failed(_)
    ));
}

#[test]
fn mac_proxy_old_helper_falls_back_but_transaction_errors_do_not() {
    use polaris_helper_proto::{Error, ErrorCode};
    use polaris_system_integration::proxy_ops::MacProxyWriterError;

    assert!(classify_mac_proxy_response(Response::Ok(ResponseKind::MacProxyTransaction)).is_ok());
    for code in [ErrorCode::Unknown, ErrorCode::Auth] {
        assert!(matches!(
            classify_mac_proxy_response(Response::Err(Error::new(code))),
            Err(MacProxyWriterError::Unavailable(_))
        ));
    }
    assert!(matches!(
        classify_mac_proxy_response(Response::Err(Error::with_detail(
            ErrorCode::SystemProxy,
            "commit failed",
        ))),
        Err(MacProxyWriterError::Failed(_))
    ));
    assert!(matches!(
        classify_mac_proxy_response(Response::Ok(ResponseKind::Cleaned)),
        Err(MacProxyWriterError::Failed(_))
    ));
}

#[test]
fn mac_proxy_compare_capability_probe_is_read_only_and_fail_closed_on_transport() {
    use polaris_system_integration::proxy_ops::MacProxyWriterError;

    for wire in [b"ERR unknown\n".to_vec(), b"ERR auth\n".to_vec()] {
        let (client, connects) =
            stop_test_client(vec![polaris_helper_client::MockStream::with_response(wire)]);
        assert!(matches!(
            mac_proxy_compare_capable_with_client(&client),
            Ok(false)
        ));
        assert_eq!(connects.load(Ordering::SeqCst), 1);
    }

    let (client, _) = stop_test_client(vec![polaris_helper_client::MockStream::with_response(
        b"OK system-proxy\n".to_vec(),
    )]);
    assert!(matches!(
        mac_proxy_compare_capable_with_client(&client),
        Ok(true)
    ));

    let (empty, _) = stop_test_client(vec![polaris_helper_client::MockStream::with_response(
        Vec::new(),
    )]);
    assert!(matches!(
        mac_proxy_compare_capable_with_client(&empty),
        Err(MacProxyWriterError::Failed(_))
    ));

    let (io, _) = stop_test_client(vec![polaris_helper_client::MockStream::broken(
        std::io::ErrorKind::BrokenPipe,
    )]);
    assert!(matches!(
        mac_proxy_compare_capable_with_client(&io),
        Err(MacProxyWriterError::Failed(_))
    ));
}

#[test]
fn status_serializes_frontend_keys() {
    let (rt, _d) = runtime();
    let json = serde_json::to_value(rt.status()).unwrap();
    // 前端 deriveState 消费的键必须在位（camelCase）。
    for key in [
        "supported",
        "installed",
        "ready",
        "upgradeable",
        "expectedProtocolVersion",
        "expectedBuildId",
        "needsRepair",
        "backgroundDisabled",
        "pathMismatch",
    ] {
        assert!(json.get(key).is_some(), "缺前端契约键 {key}: {json}");
    }
}

#[test]
fn install_missing_binary_returns_failure_without_escalation() {
    // 无 bundled polaris-helper（且未设 POLARIS_HELPER_PATH）→ install 早返失败，绝不弹提权框。
    // 尾部 `r.status.installed` 由替身给定（见 `runtime()`），不再赖「跑测机器没装过」。
    std::env::remove_var("POLARIS_HELPER_PATH");
    let (rt, _d) = runtime();
    let r = rt.install();
    assert!(!r.success, "缺二进制必失败");
    assert!(r.error_code.is_some(), "失败结果必须携带稳定错误码");
    assert!(
        r.diagnostic.is_some(),
        "失败结果必须保留泛化诊断供日志/契约使用"
    );
    // 状态仍是真探测（未安装）。
    assert!(!r.status.installed);
}

#[test]
fn action_result_serializes_stable_code_diagnostic_and_status() {
    let (rt, _d) = runtime();
    let json = serde_json::to_value(rt.install()).unwrap();
    assert!(json.get("success").is_some());
    assert!(json["errorCode"].is_string());
    assert!(json["diagnostic"].is_string());
    assert!(
        json.get("status").is_some(),
        "install 结果须含 status（前端 r.status 消费）"
    );
}

#[test]
fn helper_action_error_codes_serialize_as_frontend_contract() {
    let cases = [
        (HelperActionErrorCode::Cancelled, "cancelled"),
        (HelperActionErrorCode::PartialCleanup, "partialCleanup"),
        (
            HelperActionErrorCode::AuthorizationIncomplete,
            "authorizationIncomplete",
        ),
        (HelperActionErrorCode::ProxyRunning, "proxyRunning"),
        (HelperActionErrorCode::Unsupported, "unsupported"),
        (HelperActionErrorCode::MissingAsset, "missingAsset"),
        (HelperActionErrorCode::BundleMismatch, "bundleMismatch"),
        (HelperActionErrorCode::NotReady, "notReady"),
        (
            HelperActionErrorCode::InvalidInstallation,
            "invalidInstallation",
        ),
        (HelperActionErrorCode::TokenWriteFailed, "tokenWriteFailed"),
        (
            HelperActionErrorCode::ScriptWriteFailed,
            "scriptWriteFailed",
        ),
        (
            HelperActionErrorCode::AuthorizationLaunchFailed,
            "authorizationLaunchFailed",
        ),
        (HelperActionErrorCode::InstallerFailed, "installerFailed"),
        (HelperActionErrorCode::UpgradeDeferred, "upgradeDeferred"),
        (HelperActionErrorCode::Failed, "failed"),
    ];
    for (code, wire) in cases {
        assert_eq!(
            serde_json::to_value(code).unwrap(),
            serde_json::Value::String(wire.to_owned())
        );
    }
}

#[test]
fn pkexec_authorization_incomplete_has_its_own_stable_error_code() {
    assert_eq!(
        escalation_failure_code(Platform::Linux, 127),
        HelperActionErrorCode::AuthorizationIncomplete
    );
    assert_eq!(
        escalation_failure_code(Platform::Linux, 126),
        HelperActionErrorCode::Failed,
        "126 已在 EscalationOutcome 层归为 Cancelled，不得在失败映射里冒充 127"
    );
    assert_eq!(
        escalation_failure_code(Platform::Mac, 127),
        HelperActionErrorCode::Failed,
        "127 的授权未完成语义只属于 pkexec"
    );
}

// ── 卸载前置停核（契约 §93「卸载前零提权停核」）─────────────────────────────

#[test]
fn uninstall_preflight_truth_table() {
    use UninstallPreflight::{ProceedDirectly, StopCoreFirst};
    // 唯一该停的组合：跑着 **且** 经 helper 起。
    assert_eq!(decide_uninstall_preflight(true, true), StopCoreFirst);
    // 没跑 → 无核可停。
    assert_eq!(decide_uninstall_preflight(false, true), ProceedDirectly);
    // 跑着但 app 直起（非 TUN）→ 不归 daemon 管，停它等于无故断网。
    assert_eq!(decide_uninstall_preflight(true, false), ProceedDirectly);
    assert_eq!(decide_uninstall_preflight(false, false), ProceedDirectly);
}

/// 🟡 **变异锁：TUN 跑着时卸 helper，stop 必须真被调。**
///
/// 把 `commands::helper_uninstall` 里的 `uninstall_preflight_stop` 删掉 ⇒
/// `commands::helper` 的调用点守卫转红；把本函数体里的 `stop().await` 删掉 ⇒ 本条转红。
#[test]
fn preflight_calls_stop_only_when_core_runs_via_helper() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let rt = tokio::runtime::Runtime::new().unwrap();

    let calls = AtomicUsize::new(0);
    let r = rt.block_on(uninstall_preflight_stop(true, true, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    assert!(r.attempted(), "TUN 经 helper 跑着 → 必须先停核");
    assert_eq!(r, PreflightStopResult::Stopped);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 未运行 → 一次都不调。
    let calls = AtomicUsize::new(0);
    let r = rt.block_on(uninstall_preflight_stop(false, true, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    assert!(!r.attempted());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // 非经 helper 起 → 一次都不调（不无故断用户的网）。
    let calls = AtomicUsize::new(0);
    rt.block_on(uninstall_preflight_stop(true, false, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// 停核失败**如实上报原因**，且本函数不替调用方做「中止还是继续」的决定。
///
/// 与 `update_install` 的停代理腿刻意相反：helper 单卸载停不掉也继续卸（见函数文档的表），
/// 「继续」在此表达为函数正常返回、不 panic、不把失败当中止信号往上抛。而完全卸载腿会读
/// [`PreflightStopResult::error`] 把它映射成硬失败 —— 两种政策共用同一份判定与停核动作。
///
/// **变异探针**：把 `StopFailed(e)` 腿改回吞掉错误（返 `Stopped`）⇒ 本条转红，且
/// `runtime::uninstall` 的 `stop_core_failure_blocks_every_delete` 会失去输入源。
#[test]
fn preflight_stop_failure_is_reported_not_swallowed() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let r = rt.block_on(uninstall_preflight_stop(true, true, || async {
        Err("mock: helper socket 已消失".to_string())
    }));
    assert!(
        r.attempted(),
        "停核失败仍须算「已尝试」—— helper 单卸载据此照常推进"
    );
    assert_eq!(
        r.error(),
        Some("mock: helper socket 已消失"),
        "原因必须原样带出：完全卸载腿要拿它当中止理由报给用户"
    );
}

#[test]
fn resolve_helper_binary_env_override_rejects_missing() {
    std::env::set_var("POLARIS_HELPER_PATH", "/nonexistent/polaris-helper-xyz");
    let r = resolve_helper_binary();
    std::env::remove_var("POLARIS_HELPER_PATH");
    assert!(r.is_err());
}

/// Query, tray polling, action replies and uninstall preflight all use the same
/// side-effect-free status method. Exact production-body checks reject a hidden
/// recovery/start call even when a comment or another method mentions the probe.
#[test]
fn status_wiring_is_read_only_for_all_callers() {
    let source = crate_code("runtime/helper.rs");
    let body = impl_method_body(
        &source,
        "    pub fn status(&self) -> HelperStatusSnapshot {",
    );
    assert!(body.contains(concat!("compute_status_with_client", "(&client)")));
    assert!(!body.contains(".start("));
    assert!(!source.contains(concat!("status_with_", "recovery")));
    let failure = impl_method_body(&source, "    fn action_failed(");
    assert!(failure.contains("status: self.status()"));
    assert!(!failure.contains(".start("));
}

/// B1：`start` 回传的 `created=` 必须**存下来**当崩溃监测的初始基线，且拿不到时不留陈值。
///
/// 三条各锁一格：
/// - 拿到 `created` ⇒ 存 `(pid, created)`（正面断言：D2/D3(4) 的「start 拿初值」有东西可取）；
/// - 换一次核（新 pid、新 created）⇒ 基线整体换新，不是只换一半；
/// - 无 timing 形态 / 旧 helper ⇒ **落 `None`**，而不是留着上一次的值 —— 留着的话，新核万一拿到
///   同一个 pid，复核就会拿旧进程的创建时间去比新进程，判出一次假复用（=一次假崩溃自愈）。
///
/// 变异锁：把 `Started | Already` 那条腿的 `remember_start_identity(pid, None)` 删掉 → 第三条转红；
/// 把存储改成「只在 `created` 有值时写」→ 第三条转红。
#[test]
fn start_identity_baseline_is_remembered_and_never_goes_stale() {
    let (runtime, _dir) = runtime();
    assert_eq!(runtime.managed_start_identity(), None, "初值应为空");

    runtime.remember_start_identity(4242, Some(133_600_000_000_000_000));
    assert_eq!(
        runtime.managed_start_identity(),
        Some((4242, 133_600_000_000_000_000))
    );

    runtime.remember_start_identity(9001, Some(133_600_000_000_000_777));
    assert_eq!(
        runtime.managed_start_identity(),
        Some((9001, 133_600_000_000_000_777)),
        "换核后基线必须整体换新"
    );

    runtime.remember_start_identity(9001, None);
    assert_eq!(
        runtime.managed_start_identity(),
        None,
        "没拿到 created 就必须落 None —— 留着上一次的值会在同号新核上判出假复用"
    );
}

/// **接线门**（B1）：`start_core` 的**两条** Start 响应腿都必须落一次基线。
///
/// 行为侧由 `start_identity_baseline_is_remembered_and_never_goes_stale` 覆盖，但那条证明的是
/// 「存得对」，不是「start 真的去存了」—— 真起核是真机门，本机编译器拦不住有人把这两行删掉。
///
/// 两条都钉，缺一条都是一种真缺陷：
/// - 少了 timed 腿那行 ⇒ 基线永远取不到，D2/D3(4)「start 拿初值」整条落空；
/// - 少了 `Started | Already` 腿那行 ⇒ 陈值留存，同号新核上判出假复用。
#[test]
fn start_core_records_the_identity_baseline_on_both_response_legs() {
    const HEAD: &str = concat!(
        "    pub fn start_core(\n",
        "        &self,\n",
        "        cfg: &Path,\n",
        "        log: &Path,\n",
        "        fwd: bool,\n",
        "        ppid: Option<u32>,\n",
        "    ) -> Result<HelperStartResult, String> {"
    );
    let body = impl_method_body(&crate_code("runtime/helper.rs"), HEAD);
    assert!(
        body.contains(concat!("remember_start_identity", "(pid, created)")),
        "timed 腿没记基线 —— 崩溃监测起手取不到初值，退回「首次 status 取初值」的旧口径"
    );
    assert!(
        body.contains(concat!("remember_start_identity", "(pid, None)")),
        "无 timing 腿没清基线 —— 上一次 start 的陈值会被拿去比同号新核"
    );
}

#[test]
fn production_start_selects_compiled_id_on_linux_and_native_desktop_birth_wire() {
    let body = impl_method_body(
        &crate_code("runtime/helper.rs"),
        concat!(
            "    pub fn start_core(\n",
            "        &self,\n",
            "        cfg: &Path,\n",
            "        log: &Path,\n",
            "        fwd: bool,\n",
            "        ppid: Option<u32>,\n",
            "    ) -> Result<HelperStartResult, String> {"
        ),
    );
    assert!(body.contains("Platform::Mac | Platform::Win => Request::NativeStartPinned(params)"));
    assert!(body.contains("Platform::Linux => Request::LinuxStartPinned(params)"));
    assert!(body.contains("crate::runtime::core_identity::selected_id("));
    assert!(body.contains("id: id.clone()"));
    assert!(body.contains("if actual == id"));
    assert!(!body.contains("Request::LinuxStartBirth("));
    assert!(!body.contains("Request::NativeStartBirth("));
    assert!(body.contains("if self.platform == Platform::Linux"));
    assert!(body.contains("return classify_linux_birth_start_response(resp)"));
}

struct RecordingStream {
    inner: polaris_helper_client::MockStream,
    frames: Arc<Mutex<Vec<String>>>,
}

impl ConnectionStream for RecordingStream {
    fn read_until_timeout(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize> {
        self.inner.read_until_timeout(buf)
    }

    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(data)?;
        self.frames
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(data).into_owned());
        Ok(())
    }

    fn shutdown(&mut self) -> std::io::Result<()> {
        self.inner.shutdown()
    }
}

#[test]
fn core_status_has_only_an_id_frame_and_does_not_upload_source_bytes() {
    let id = polaris_helper_proto::core_manifest::CoreId::parse(&"a".repeat(64)).unwrap();
    let reply = format!("OK core-present {} 1.15.0-alpha.12\n", id.as_str());
    let (client, connects, frames) = stop_test_client_with_frames(
        Platform::Win,
        vec![polaris_helper_client::MockStream::with_response(
            reply.into_bytes(),
        )],
    );
    assert!(matches!(
        query_core_status(&client, &id),
        Ok(polaris_helper_proto::core_protocol::CoreStatus::Present { .. })
    ));
    assert_eq!(connects.load(Ordering::SeqCst), 1);
    let frames = frames.lock().unwrap();
    assert_eq!(
        frames[0].lines().collect::<Vec<_>>(),
        ["TOK", "core-status", id.as_str()]
    );
}

#[test]
fn absent_invalid_unknown_empty_and_wrong_id_never_attest_a_selected_core() {
    let id = polaris_helper_proto::core_manifest::CoreId::parse(&"a".repeat(64)).unwrap();
    for reply in [
        format!("OK core-absent {}\n", id.as_str()),
        format!("OK core-invalid {} hash-differs\n", id.as_str()),
    ] {
        let (client, _) = stop_test_client(vec![polaris_helper_client::MockStream::with_response(
            reply.into_bytes(),
        )]);
        assert!(!matches!(
            query_core_status(&client, &id),
            Ok(polaris_helper_proto::core_protocol::CoreStatus::Present { .. })
        ));
    }
    for reply in [
        "ERR unknown\n".to_owned(),
        String::new(),
        format!("OK core-present {} 1.0.0\n", "b".repeat(64)),
    ] {
        let (client, _) = stop_test_client(vec![polaris_helper_client::MockStream::with_response(
            reply.into_bytes(),
        )]);
        assert!(query_core_status(&client, &id).is_err());
    }
}

#[test]
fn windows_cache_request_never_selects_the_old_helpers_console_command() {
    let request = flush_dns_request(Platform::Win);
    assert_eq!(request, Request::WindowsFlushDnsNative);
    let raw = String::from_utf8(polaris_helper_proto::codec::encode(
        Platform::Win,
        "fixture-token",
        &request,
    ))
    .unwrap();
    assert_eq!(raw.lines().nth(1), Some("flush-dns-native"));
    assert_eq!(flush_dns_request(Platform::Mac), Request::FlushDns);
}

#[test]
fn native_partial_cleanup_is_only_an_uninstall_failure_code() {
    assert_eq!(
        uninstall_failure_code(Platform::Win, 3),
        HelperActionErrorCode::PartialCleanup
    );
    assert_eq!(
        escalation_failure_code(Platform::Win, 3),
        HelperActionErrorCode::Failed
    );
    assert_eq!(
        uninstall_failure_code(Platform::Win, 2),
        HelperActionErrorCode::Failed
    );
    assert_eq!(
        uninstall_failure_code(Platform::Linux, 3),
        HelperActionErrorCode::Failed
    );
    assert_eq!(
        uninstall_failure_code(Platform::Linux, 127),
        HelperActionErrorCode::AuthorizationIncomplete
    );
}

#[test]
fn selected_core_status_projection_keeps_identity_and_native_state_read_only() {
    let status = HelperStatusSnapshot {
        selected_core: Some(SelectedCoreSnapshot {
            id: "a".repeat(64),
            version: "1.15.0-alpha.12".into(),
            provider_fingerprint: "b".repeat(64),
            state: SelectedCoreState::Absent,
        }),
        supported: true,
        installed: true,
        upgradeable: true,
        ..Default::default()
    };
    let wire = serde_json::to_value(&status).unwrap();
    assert_eq!(wire["selectedCore"]["id"], "a".repeat(64));
    assert_eq!(wire["selectedCore"]["providerFingerprint"], "b".repeat(64));
    assert_eq!(wire["selectedCore"]["state"], "absent");
    assert!(!helper_install_reached_expected_build(&status));
}

#[test]
fn native_slot_denial_is_not_an_authentication_or_node_configuration_hint() {
    use polaris_helper_proto::{Error, ErrorCode};
    for error in [
        Error::with_detail(ErrorCode::Auth, "slot-owned-by-other-user"),
        Error::with_detail(ErrorCode::Auth, "slot-owned-or-core-occupied"),
        Error::with_detail(ErrorCode::Start, "core-occupied"),
        Error::parse("ERR slot-owned-by-other-user").unwrap(),
    ] {
        assert!(is_helper_slot_occupied(&error));
        assert!(format_helper_mutation_error(Platform::Win, "start", &error)
            .starts_with("HELPER_CORE_OCCUPIED:"));
    }
    for error in [
        Error::new(ErrorCode::Auth),
        Error::with_detail(ErrorCode::Other, "arbitrary native error"),
        Error::new(ErrorCode::Unknown),
    ] {
        assert!(!is_helper_slot_occupied(&error));
    }
    let (client, _) = stop_test_client(vec![polaris_helper_client::MockStream::with_response(
        b"ERR auth slot-owned-by-other-user\n".to_vec(),
    )]);
    assert!(matches!(
        require_native_birth_capability_with_client(&client),
        Err(BirthCapabilityError::SlotOccupied(_))
    ));
}

#[cfg(unix)]
#[test]
fn installer_uid_matches_the_native_nonroot_app_user() {
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .unwrap();
    assert!(output.status.success());
    let observed: u32 = std::str::from_utf8(&output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(current_uid(), observed);
    let identity = polaris_helper_proto::core_protocol::CallerIdentity::Unix(current_uid());
    assert_eq!(
        identity,
        polaris_helper_proto::core_protocol::CallerIdentity::Unix(observed)
    );
    #[cfg(target_os = "macos")]
    assert_ne!(
        observed, 0,
        "Mac App/admin-CLI regression must run as the actual nonroot runner"
    );
}

#[test]
fn helper_install_errors_preserve_preparation_stage_without_guessing_cause() {
    use std::io::{Error, ErrorKind};
    let cases = [
        (
            ManagerError::InvalidInstallation("private"),
            HelperActionErrorCode::InvalidInstallation,
        ),
        (
            ManagerError::HelperBinaryMissing(PathBuf::from("private")),
            HelperActionErrorCode::MissingAsset,
        ),
        (
            ManagerError::TokenWrite(Error::from(ErrorKind::PermissionDenied)),
            HelperActionErrorCode::TokenWriteFailed,
        ),
        (
            ManagerError::ScriptWrite(Error::from(ErrorKind::StorageFull)),
            HelperActionErrorCode::ScriptWriteFailed,
        ),
        (
            ManagerError::Escalation(ClientError::Io(Error::from(ErrorKind::NotFound))),
            HelperActionErrorCode::AuthorizationLaunchFailed,
        ),
        (
            ManagerError::UnsupportedPlatform(Platform::Ios),
            HelperActionErrorCode::Unsupported,
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(manager_install_failure_code(&error), expected);
    }
    for platform in [Platform::Mac, Platform::Win, Platform::Linux] {
        assert_eq!(
            install_command_failure_code(platform, 1, ""),
            HelperActionErrorCode::InstallerFailed
        );
    }
    assert_eq!(
        install_command_failure_code(Platform::Linux, 127, ""),
        HelperActionErrorCode::AuthorizationIncomplete
    );
    assert_eq!(
        install_command_failure_code(Platform::Mac, 127, ""),
        HelperActionErrorCode::InstallerFailed
    );
}

#[test]
fn helper_upgrade_deferral_recognizes_the_observed_macos_marker_not_exit_code_or_prose() {
    let actual = "0:174: execution error: helper administration: helper busy, unknown or legacy upgrade admission unsupported\nHELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY (75)";
    assert_eq!(
        install_command_failure_code(Platform::Mac, 1, actual),
        HelperActionErrorCode::UpgradeDeferred
    );
    assert_eq!(
        install_command_failure_code(
            Platform::Linux,
            75,
            "HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY"
        ),
        HelperActionErrorCode::UpgradeDeferred
    );
    for diagnostic in [
        "",
        "helper busy",
        "helper busy, unknown or legacy upgrade admission unsupported",
        "path/HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY",
        "HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY (1)",
    ] {
        assert_eq!(
            install_command_failure_code(Platform::Mac, 75, diagnostic),
            HelperActionErrorCode::InstallerFailed
        );
    }
}

#[test]
fn helper_install_mode_is_typed_and_defaults_to_ordinary() {
    assert_eq!(HelperInstallMode::default(), HelperInstallMode::Ordinary);
    assert_eq!(
        serde_json::from_str::<HelperInstallMode>("\"stopAndUpgrade\"").unwrap(),
        HelperInstallMode::StopAndUpgrade
    );
    assert_eq!(
        serde_json::from_str::<HelperInstallMode>("\"ordinary\"").unwrap(),
        HelperInstallMode::Ordinary
    );
    for invalid in ["true", "1", "\"force\"", "\"stop-and-upgrade\"", "{}"][..].iter() {
        assert!(serde_json::from_str::<HelperInstallMode>(invalid).is_err());
    }
    for &platform in Platform::ALL {
        assert!(HelperInstallMode::Ordinary.supported_on(platform));
        assert_eq!(
            HelperInstallMode::StopAndUpgrade.supported_on(platform),
            platform == Platform::Mac
        );
        if platform != Platform::Mac {
            let dir = TestDir::new("polaris-helper-explicit-mode-");
            let runtime = HelperRuntime::with_platform_for_tests(dir.clone(), platform);
            let result = runtime.install_with_mode(HelperInstallMode::StopAndUpgrade);
            assert!(!result.success);
            assert_eq!(result.error_code, Some(HelperActionErrorCode::Unsupported));
        }
    }
}

#[test]
fn windows_installer_stage_codes_are_presentation_only_and_platform_scoped() {
    assert_eq!(
        install_command_failure_code(Platform::Win, 75, ""),
        HelperActionErrorCode::UpgradeDeferred
    );
    assert_eq!(
        install_command_failure_code(Platform::Mac, 75, ""),
        HelperActionErrorCode::InstallerFailed
    );
    for code in 81..=88 {
        assert_eq!(
            install_command_failure_code(Platform::Win, code, "secret details"),
            HelperActionErrorCode::InstallerFailed
        );
        assert!(install_command_diagnostic(Platform::Win, code).contains("Windows"));
        assert_eq!(
            install_command_diagnostic(Platform::Linux, code),
            "helper installer exited unsuccessfully"
        );
    }
    assert_eq!(
        install_command_diagnostic(Platform::Win, 1),
        "helper installer exited unsuccessfully"
    );
}

#[test]
fn execution_diagnostic_is_fixed_non_authoritative_and_bounded() {
    for (text, marker) in [
        (
            "pkexec must be setuid root",
            HelperExecutionMarker::NotSetuidRoot,
        ),
        (
            "Error executing command as another user: No authentication agent found.",
            HelperExecutionMarker::NoAuthenticationAgent,
        ),
        (
            "Error executing command as another user: Not authorized",
            HelperExecutionMarker::NotAuthorized,
        ),
        (
            "Error creating textual authentication agent: PRIVATE",
            HelperExecutionMarker::TextAgentCreationFailed,
        ),
        (
            "Error registering local authentication agent: PRIVATE",
            HelperExecutionMarker::TextAgentRegistrationFailed,
        ),
        (
            "Error checking for authorization PRIVATE",
            HelperExecutionMarker::AuthorizationCheckFailed,
        ),
    ] {
        let value = helper_execution_diagnostic(Platform::Linux, 127, text).unwrap();
        assert_eq!(value.marker, marker);
        assert_eq!(value.exit_code, 127);
        assert_eq!(value.scope, "child-stderr-marker-not-verified-emitter");
        assert!(!serde_json::to_string(&value).unwrap().contains("PRIVATE"));
        assert_eq!(
            helper_execution_diagnostic(Platform::Linux, 126, text)
                .unwrap()
                .marker,
            HelperExecutionMarker::Unknown
        );
    }
    for text in [
        "PRIVATE password",
        "pkexec must be setuid root suffix",
        "translated message",
        "pkexec must be setuid root\nError executing command as another user: Not authorized",
    ] {
        assert_eq!(
            helper_execution_diagnostic(Platform::Linux, 127, text)
                .unwrap()
                .marker,
            HelperExecutionMarker::Unknown
        );
    }
    assert_eq!(
        helper_execution_diagnostic(Platform::Linux, 127, &"x".repeat(16385))
            .unwrap()
            .marker,
        HelperExecutionMarker::Unknown
    );
    assert!(helper_execution_diagnostic(Platform::Mac, 127, "PRIVATE").is_none());
    assert!(helper_execution_diagnostic(Platform::Win, 127, "PRIVATE").is_none());
}

#[test]
fn helper_execution_optional_field_does_not_change_legacy_action_json() {
    let result = HelperActionResult {
        success: false,
        execution_diagnostic: None,
        error_code: Some(HelperActionErrorCode::Cancelled),
        diagnostic: None,
        status: HelperStatusSnapshot::default(),
    };
    let value = serde_json::to_value(result).unwrap();
    assert!(value.get("executionDiagnostic").is_none());
    assert_eq!(value["errorCode"], "cancelled");
    assert_eq!(value["success"], false);
}

#[test]
fn installer_failure_stage_is_bounded_non_authorizing_and_preserves_recovery_unknown() {
    let early = "POLARIS_HELPER_FAILURE_V1:manager-property:recovery-unconfirmed";
    let finished = format!("{early}\nPOLARIS_HELPER_FAILURE_V1:manager-property:rollback-returned");
    for (text, expected) in [
        (early, "recovery-unconfirmed"),
        (finished.as_str(), "rollback-returned"),
    ] {
        let diagnostic = helper_execution_diagnostic(Platform::Linux, 1, text).unwrap();
        assert_eq!(diagnostic.scope, "child-stderr-marker-not-verified-emitter");
        let failure = diagnostic.installer_failure.unwrap();
        assert_eq!(failure.stage, "manager-property");
        assert_eq!(failure.recovery, expected);
    }
    for text in ["POLARIS_HELPER_FAILURE_V1:secret-path:cleanup-returned",
        "POLARIS_HELPER_FAILURE_V1:manager-property:SECRET",
        "POLARIS_HELPER_FAILURE_V1:manager-property:cleanup-returned:secret",
        "POLARIS_HELPER_FAILURE_V1:lease:recovery-unconfirmed\nPOLARIS_HELPER_FAILURE_V1:manager-property:rollback-returned"] {
        assert!(helper_installer_failure(text).is_none());
    }
    assert!(helper_installer_failure(&"x".repeat(16385)).is_none());
    assert!(helper_execution_diagnostic(Platform::Linux, 0, early)
        .unwrap()
        .installer_failure
        .is_none());
}
