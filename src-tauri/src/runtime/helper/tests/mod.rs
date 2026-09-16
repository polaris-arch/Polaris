use super::*;
use crate::commands::guard_scan::impl_method_body;
use crate::test_support::{crate_code, TestDir};

use std::sync::atomic::Ordering;
use std::sync::Mutex;

struct SequenceConnector {
    streams: Mutex<Vec<polaris_helper_client::MockStream>>,
    connects: Arc<std::sync::atomic::AtomicUsize>,
}

impl Connector for SequenceConnector {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let mut streams = self.streams.lock().unwrap();
        if streams.is_empty() {
            return Err(ClientError::Connect("测试连接已耗尽".to_owned()));
        }
        Ok(Box::new(streams.remove(0)))
    }
}

fn stop_test_client(
    streams: Vec<polaris_helper_client::MockStream>,
) -> (HelperClient, Arc<std::sync::atomic::AtomicUsize>) {
    let connects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connector = SequenceConnector {
        streams: Mutex::new(streams),
        connects: Arc::clone(&connects),
    };
    (
        HelperClient::new(Box::new(connector), Platform::Win, "TOK"),
        connects,
    )
}

fn runtime() -> (HelperRuntime, TestDir) {
    let dir = TestDir::new("polaris-helper-test-");
    // **不用 `HelperRuntime::new`**：那条走 StdSysOps + 系统路径，会把门的绿绑在「跑测的机器上
    // 没装过 Polaris」这个宿主前提上（且装了就会真连特权 daemon）。见 `never_installed_for_tests`。
    (HelperRuntime::never_installed_for_tests(dir.clone()), dir)
}

#[test]
fn stop_retries_once_when_the_first_roundtrip_loses_its_response() {
    let (client, connects) = stop_test_client(vec![
        polaris_helper_client::MockStream::broken(std::io::ErrorKind::BrokenPipe),
        polaris_helper_client::MockStream::with_response(b"OK notrunning\n".to_vec()),
    ]);

    stop_core_with_client(&client, Some(4242))
        .expect("同一 pid 的第二次 stop 应以 notrunning 幂等收口");
    assert_eq!(connects.load(Ordering::SeqCst), 2, "通信错误后只应补发一次");
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
fn managed_core_status_parses_running_and_stopped() {
    for (wire, expected) in [
        (
            // 旧 helper 的 wire：无身份 token ⇒ 两个字段 None（消费方按「不可观测」处理）。
            b"OK running 4242\n".to_vec(),
            ManagedCoreStatus::Running {
                pid: 4242,
                created: None,
                image: None,
            },
        ),
        (
            // 新 Windows helper：created 十进制、image 是 hex 编码的路径（含空格）。
            b"OK running 4242 created=133600000000000000 image=433a5c50726f6772616d2046696c65735c506f6c617269735c73696e672d626f782e657865\n".to_vec(),
            ManagedCoreStatus::Running {
                pid: 4242,
                created: Some(133_600_000_000_000_000),
                image: Some(r"C:\Program Files\Polaris\sing-box.exe".to_owned()),
            },
        ),
        (b"OK stopped\n".to_vec(), ManagedCoreStatus::Stopped),
    ] {
        let (client, connects) =
            stop_test_client(vec![polaris_helper_client::MockStream::with_response(wire)]);
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
    assert!(error.contains("非预期响应"));
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
}

#[test]
fn helper_install_success_requires_the_expected_build() {
    let current = HelperStatusSnapshot {
        ready: true,
        upgradeable: false,
        ..Default::default()
    };
    assert!(helper_install_reached_expected_build(&current));

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
        (
            HelperActionErrorCode::AuthorizationUnavailable,
            "authorizationUnavailable",
        ),
        (HelperActionErrorCode::ProxyRunning, "proxyRunning"),
        (HelperActionErrorCode::Unsupported, "unsupported"),
        (HelperActionErrorCode::MissingAsset, "missingAsset"),
        (HelperActionErrorCode::NotReady, "notReady"),
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
fn pkexec_without_auth_agent_has_its_own_stable_error_code() {
    assert_eq!(
        escalation_failure_code(Platform::Linux, 127),
        HelperActionErrorCode::AuthorizationUnavailable
    );
    assert_eq!(
        escalation_failure_code(Platform::Linux, 126),
        HelperActionErrorCode::Failed,
        "126 已在 EscalationOutcome 层归为 Cancelled，不得在失败映射里冒充 127"
    );
    assert_eq!(
        escalation_failure_code(Platform::Mac, 127),
        HelperActionErrorCode::Failed,
        "127 的认证代理语义只属于 pkexec"
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

/// W20 防回潮：`status()` 必须走带恢复腿的探测。直连 `compute_status_with_client` 会把
/// 「装了但停着」误报成修复态（UI 弹「修复助手」），而那只是 `sc start` 一把的事。
/// 行为已在 helper-client 单测覆盖（recovery_* 五条），这里源码级钉住接线不被回退——
/// 本机 Linux gate 走不到 win 分身，编译器拦不住这行被改回去。
///
/// 钉法：走共用取材器 [`impl_method_body`]（`impl` 块内方法，按四空格 `}` 封顶），
/// 只断言生产方法体；断言串再经 `concat!` 打断——否则测试自身的字符串字面量就是取材面里的一个
/// 命中点，改回生产代码后测试照样绿（评审 F1 实证过的假绿形态）。
///
/// # 手写切片器换成共用器，换掉的是三处会静默放水的地方
///
/// 旧写法 `find("pub fn status(")` → 封顶到 `"\n    /// "`（下一个兄弟文档注释）：
/// - **封顶锚在注释上**：把 `status` 与下一个方法之间那段文档注释删掉（或改成 `//` 行注释），
///   `unwrap_or(rest.len())` 直接把切片放到**文件尾** —— 射程从「这个方法」变成「这个文件」，
///   而 `status_with_recovery(&client)` 在别处出现一次就能替它作证，门不会喊；
/// - **锚点不校验唯一性**：`find` 取首次命中，取材面里再出现一个 `pub fn status(`（另一个类型的
///   同名方法）时，切的是哪一个全凭书写顺序；
/// - **不剥注释**：本条是正面 `contains`，把那行调用整行注释掉，注释里那份副本照样喂饱它。
///
/// 共用器三处都反过来：锚点必须带 `impl` 方法缩进、必须**恰好**出现一次、内部再剥一道；
/// 取材先过 [`crate_code`]，行尾与块注释也一并剥掉。
#[test]
fn status_wiring_uses_recovery_probe() {
    let body = impl_method_body(
        &crate_code("runtime/helper.rs"),
        "    pub fn status(&self) -> HelperStatusSnapshot {",
    );
    assert!(
        body.contains(concat!("status_with_recovery", "(&client)")),
        "HelperRuntime::status 必须调 status_with_recovery（W20 恢复腿）"
    );
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
        "    ) -> Result<u32, String> {"
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

/// **S-2 记号语义**：失效键取「探到的那次身份」，`Unreachable` 一律不记（也不留旧值）。
///
/// 「探不到就不记」这条以前写在 `reconcile_protected_core` 的闭包里，够不着任何行为测试；
/// 搬到 `HelperRuntime` 之后它有了直接判据。**第三条最要紧**：探不到时若只是「不覆盖」，
/// 旧记号会原地留着 —— 那正是「没有失效键的缓存」的另一种长相。
#[test]
fn install_core_note_keys_on_the_probed_build_and_drops_on_an_unreachable_probe() {
    let (rt, _dir) = runtime();
    assert!(
        rt.install_core_unsupported_note().is_none(),
        "初始必须无记号，否则首次起核直接跳过对账 = 加固整条不生效"
    );

    rt.note_install_core_unsupported(&HelperBuildProbe::Reported(Some("build-A".to_owned())));
    assert_eq!(
        rt.install_core_unsupported_note(),
        Some(InstallCoreUnsupportedRecord {
            helper_build_id: Some("build-A".to_owned())
        }),
        "记号必须带上探到的构建身份（那是它唯一的失效键）"
    );

    // 探不到 ≠ 还是那个 helper：不记新的，**也不许把旧的留着**。
    rt.note_install_core_unsupported(&HelperBuildProbe::Unreachable);
    assert!(
        rt.install_core_unsupported_note().is_none(),
        "探不到身份时留着旧记号 = 一个永不失效的缓存"
    );

    // 旧 helper 如实不带 build_identity：`Reported(None)` 本身是稳定身份，照记。
    rt.note_install_core_unsupported(&HelperBuildProbe::Reported(None));
    assert_eq!(
        rt.install_core_unsupported_note(),
        Some(InstallCoreUnsupportedRecord {
            helper_build_id: None
        }),
        "`Reported(None)` 与 `Unreachable` 必须分得开"
    );
}

/// 🔴 **S-2 反向对照：失败的 install 绝不清记号。**
///
/// 清记号挂的是「提权脚本真跑成功 ⇒ 磁盘上的 helper 已换一份」，不是「`install()` 被调了一次」。
/// 没有这一条，把清除挪到方法开头（每次点一下就清，哪怕缺二进制早返）也照样全绿 ——
/// 那等于把能力缓存变成「用户每点一次修复就白跑一轮 80MB 对账」。
#[test]
fn a_failed_install_never_clears_the_capability_note() {
    std::env::remove_var("POLARIS_HELPER_PATH");
    let (rt, _dir) = runtime();
    rt.note_install_core_unsupported(&HelperBuildProbe::Reported(Some("build-A".to_owned())));

    let r = rt.install();
    assert!(!r.success, "无 bundled helper 必须早返失败（不弹提权框）");
    assert_eq!(
        rt.install_core_unsupported_note(),
        Some(InstallCoreUnsupportedRecord {
            helper_build_id: Some("build-A".to_owned())
        }),
        "install 失败却把记号清了 —— 清除被挂在了「调用发生」而不是「二进制被替换」上"
    );
}

/// 🔴 **S-2 接线：`install()` 的成功分支必须清掉能力记号。**
///
/// # 为什么这条非有不可
///
/// 记号的失效键是 helper 自报的 `build_identity`，而
/// `polaris_helper_proto::build_identity::current()` 在 `POLARIS_BUILD_ID` 未注入时落
/// `CARGO_PKG_VERSION` —— **开源项目的源码构建、CI 的非发布产物都不注入**。于是这条链上失效键
/// 恒不变：旧 helper 回 `ERR unknown` → 记号 `{build=0.x.y}` → 拉新源码重编 helper → 设置页点
/// 「升级/修复」（同一个 app 进程）→ 新 helper 仍自报 `0.x.y` → 缓存**命中** → 本会话内
/// install-core 永不再试，受保护目录停在旧核，现场只有一句 warn 可查。
/// app 自己刚把 helper 装了一遍，这是能拿到的最精确的失效事件。
///
/// # 为什么是源码级
///
/// `EscalationOutcome::Success` 那一臂要真跑一次提权脚本（UAC/osascript/pkexec）才到得了，
/// 属真机门；本机能测的只有失败腿（上面那条反向对照）。
///
/// 变异锁：删掉臂里的 `clear_install_core_unsupported()` → 第一条红；
/// 挪到 `wait_until_ready` 之后 → 第三条红；挪出 Success 臂（例如方法开头）→ 第一条红
/// 且上面那条行为对照同时红。
#[test]
fn a_successful_install_clears_the_install_core_capability_note() {
    let body = impl_method_body(
        &crate_code("runtime/helper.rs"),
        "    pub fn install(&self) -> HelperActionResult {",
    );
    let arm = crate::commands::guard_scan::match_arm_body(
        &body,
        "Ok(EscalationOutcome::Success) => {",
        "Ok(EscalationOutcome::",
    );
    // 切点自检：臂体不是空的、且确实是那条成功臂（含就绪轮询）。空切片上的正面断言必然假红，
    // 但「臂体里混进了别的臂」会造成**假绿** —— 后者由 `match_arm_body` 自己的输出自检兜。
    assert!(
        arm.contains("wait_until_ready"),
        "切点自检失败：切出来的不是 Success 臂（里面没有就绪轮询）"
    );
    assert!(
        arm.contains(concat!("clear_install_core_unsupported", "()")),
        "install 成功后没清「helper 不支持 install-core」的记号 —— 源码构建下 build_identity \
         不变，失效键指望不上，重装 helper 也唤不醒受保护核对账"
    );
    // 只有一个调用点：散在多处会让「射程到底覆盖哪条腿」取决于书写顺序。
    assert_eq!(
        body.matches(concat!("clear_install_core_unsupported", "()"))
            .count(),
        1,
        "install() 里出现了不止一个清除调用点"
    );
    // 顺序：清除挂在**二进制已被替换**这个事实上，不是挂在「新 helper 就绪」上 ——
    // 回退粒度要等于故障粒度。就绪轮询可能超时（NotReady），而那时二进制早就换过了。
    let clear_at = arm
        .find(concat!("clear_install_core_unsupported", "()"))
        .expect("上面刚断言过");
    let ready_at = arm.find("wait_until_ready").expect("上面刚断言过");
    assert!(
        clear_at < ready_at,
        "清记号必须先于就绪轮询：轮询超时走 NotReady 分支，而那时 helper 二进制已经换了一份"
    );
}
