use super::*;
use crate::transport::MockStream;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// mock SysOps：可预设路径存在性 + 服务状态。
#[derive(Default)]
struct MockSysOps {
    exists_paths: HashSet<PathBuf>,
    loaded: bool,
    /// SCM/launchd/systemd 里**已注册**的服务标识（可停着）。与 `loaded`（正在运行）正交：
    /// Windows 的 `is_installed` 判的是注册而非运行。
    registered_services: HashSet<String>,
    /// W20：拉起是否失败（模拟二进制被删 / DACL 无 SERVICE_START）。
    start_fails: bool,
    start_calls: Arc<Mutex<Vec<String>>>,
    stop_calls: Arc<Mutex<Vec<String>>>,
    /// `is_loaded` 收到的 label —— **必须记**：早先它只返 `self.loaded`、丢掉 label，
    /// 于是「`is_loaded` 硬传 SERVICE_LABEL」这个变异在 Windows 上无人能杀
    /// （`start`/`stop` 有门、`is_loaded` 没有 = 三缺一）。
    loaded_calls: Arc<Mutex<Vec<String>>>,
}

impl SysOps for MockSysOps {
    fn exists(&self, path: &Path) -> bool {
        self.exists_paths.contains(path)
    }
    fn start_service(&self, label: &str) -> Result<(), String> {
        self.start_calls.lock().unwrap().push(label.to_owned());
        if self.start_fails {
            return Err("mock: 拉起失败（模拟二进制被删 / 无 SERVICE_START）".to_owned());
        }
        Ok(())
    }
    fn stop_service(&self, label: &str) -> Result<(), String> {
        self.stop_calls.lock().unwrap().push(label.to_owned());
        Ok(())
    }
    fn is_loaded(&self, label: &str) -> bool {
        self.loaded_calls.lock().unwrap().push(label.to_owned());
        self.loaded
    }
    fn service_exists(&self, label: &str) -> bool {
        self.registered_services.contains(label)
    }
}

/// mock connector：返回预置 MockStream。
#[derive(Clone)]
struct MockConnector {
    streams: Arc<Mutex<Vec<MockStream>>>,
}

impl Connector for MockConnector {
    fn connect(&self) -> Result<Box<dyn crate::transport::ConnectionStream>, ClientError> {
        let mut g = self.streams.lock().unwrap();
        if g.is_empty() {
            return Err(ClientError::Connect("no mock".into()));
        }
        Ok(Box::new(g.remove(0)))
    }
}

fn manager(platform: Platform, sysops: MockSysOps) -> HelperManager {
    HelperManager::new(
        platform,
        PathBuf::from("/tmp/helper-client.token"),
        Box::new(sysops),
    )
}

/// 构造 manager + 写好 token 文件（让 compute_status 跑到 ping 探测阶段）。
fn manager_with_token(
    platform: Platform,
    sysops: MockSysOps,
) -> (HelperManager, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("token");
    crate::token::write_token_content(&token_path, "TOK").unwrap();
    let m = HelperManager::new(platform, token_path, Box::new(sysops));
    (m, dir)
}

/// 当前源码 helper 的真实 pong wire；测试不要再手拼一份漏掉 build identity 的“半握手”。
fn current_pong_wire() -> Vec<u8> {
    format!(
        "{}\n",
        Response::Ok(ResponseKind::Pong(Pong::current(0))).to_wire_line()
    )
    .into_bytes()
}

#[test]
fn install_paths_mac_match_polaris() {
    // Polaris HelperManager.ts:30,33-35
    let p = InstallPaths::mac();
    assert_eq!(
        p.binary,
        PathBuf::from("/Library/PrivilegedHelperTools/com.polaris.helper")
    );
    assert_eq!(
        p.descriptor,
        Some(PathBuf::from(
            "/Library/LaunchDaemons/com.polaris.helper.plist"
        ))
    );
    assert!(p.socket.to_string_lossy().contains("helper.sock"));
}

/// win 的 client 连接目标必须是 daemon 创建的那条管道、`sc` 起停的必须是 daemon 注册的那个服务。
/// 两侧共引 `polaris_helper_proto::windows_helper`；本条钉的是**本 crate 真的在用它**——
/// 早先这里是内联字面量 `PathBuf::from(r"\\.\pipe\...")`，连常量都不是，与 daemon 零对账，
/// 改错一个字符的后果是 app 恒判 helper 未运行、每次起核都弹 UAC 重装。字面量写死：引常量的话
/// 常量一改判据跟着漂。
#[test]
fn win_install_paths_rendezvous_with_the_daemon() {
    let p = InstallPaths::win();
    assert_eq!(p.socket, PathBuf::from(r"\\.\pipe\polaris-helper"));
    assert_eq!(p.service_label, "PolarisHelper");
    assert_eq!(
        p.binary,
        PathBuf::from(r"C:\ProgramData\Polaris\polaris-helper.exe")
    );
}

#[test]
fn install_paths_for_platform() {
    assert_eq!(
        InstallPaths::for_platform(Platform::Mac),
        InstallPaths::mac()
    );
    assert_eq!(
        InstallPaths::for_platform(Platform::Linux),
        InstallPaths::linux()
    );
    assert_eq!(
        InstallPaths::for_platform(Platform::Win),
        InstallPaths::win()
    );
}

/// W17 防回潮（2026-08-20 订正为 Platform 键控形态）：is_installed 的 **Win 平台**必须
/// 「SCM 单证据」，不得回到文件 stat——安装脚本的 ACL 锁下未提权 app 恒 false
/// （2026-08-19 .207 实测 Test-Path=False 而 sc query=0）。证据集按 Platform 枚举分派
/// 而非编译目标 cfg（cfg 形态曾让 CI win 腿六测全红：win 目标上 Mac 平台 mock 也被拽进
/// SCM 分支；push 只跑 ubuntu 腿不可见，全矩阵 dispatch 才暴露）。
#[test]
fn win_is_installed_uses_scm_evidence_not_the_acl_locked_file() {
    let src = polaris_source_probe::crate_source!("manager.rs");
    let at = src.find("pub fn is_installed(").expect("is_installed 消失");
    // 从函数起点向后找**下一个兄弟文档注释**作切片终点（文件前部有同名文案，全局 find 会倒挂）
    let end = src[at..].find("\n    /// ").map_or(src.len(), |i| at + i);
    let body = &src[at..end];
    let win_at = body
        .find("Platform::Win => self.sysops.service_exists")
        .expect("is_installed 缺 Win 平台 SCM 单证据臂");
    assert!(
        !body[..win_at].contains("sysops.exists"),
        "Win 臂早退之前不得有文件 stat（W17 复发：ACL 锁下未提权恒 false）"
    );
    assert!(
        !body.contains("#[cfg("),
        "证据分派不得再按编译目标 cfg（Platform 键控，测试须宿主无关）"
    );
}

/// W10 跟进项钉扎：装后就绪轮询窗 ≥ 10s（20×500ms）——.207 首装实测 3s 窗口
/// 把快照定格在未就绪、卡片停在安装前旧态。收紧需带新的真机计时依据。
#[test]
fn ready_poll_window_covers_scm_cold_start() {
    assert_eq!(READY_POLL_ATTEMPTS, 20);
    assert_eq!(READY_POLL_DELAY, Duration::from_millis(500));
}

#[test]
fn is_installed_requires_both_binary_and_descriptor() {
    // Polaris filesPresent: HELPER_DEST && PLIST_PATH（HelperManager.ts:202）
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    // 只放 binary，不放 descriptor → 未装
    sysops.exists_paths.insert(paths.binary.clone());
    let m = manager(Platform::Mac, sysops);
    assert!(!m.is_installed());
}

#[test]
fn is_installed_true_when_both_present() {
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.clone().expect("mac/linux 有描述符文件"));
    let m = manager(Platform::Mac, sysops);
    assert!(m.is_installed());
}

/// Windows 的「第二条证据」是 SCM 服务已注册，不是描述符文件。
///
/// 早先 `InstallPaths::win()` 塞了个从不创建的 `helper-service.yml` 且被 stat，
/// 导致 Windows 上 `is_installed` 恒 false → `compute_status_with_client` 短路成全 false、
/// 连管道都不 ping → helper 卡片恒显示未安装、TUN 起核门每次弹提权引导且装完复检仍判未装。
///
/// **变异**：把 `is_installed` 的 `None` 腿改回 stat `descriptor` → 首条断言转红。
#[test]
fn is_installed_on_windows_reads_scm_service_not_a_descriptor_file() {
    let paths = InstallPaths::win();
    assert!(
        paths.descriptor.is_none(),
        "Windows 服务定义在 SCM，磁盘无描述符文件"
    );

    // exe 在位 + 服务已注册 → 已安装
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    assert!(manager(Platform::Win, sysops).is_installed());

    // exe 在位但服务没注册（装了一半 / 服务被删）→ 未安装
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    assert!(!manager(Platform::Win, sysops).is_installed());

    // 服务在但 exe 没了 → Win 平台 SCM 单证据 ⇒ 判已装（宿主无关，2026-08-20 订正为
    // Platform 键控后不再 cfg 拆分）。ping 随后挂 → needs_repair →「点一下修复」而非
    // 「从未装过」——与 is_installed 头注「不得把可修复态误报成未安装态」同一条设计
    // 原则；重装脚本 Copy-Item -Force 覆盖缺失文件。
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    assert!(manager(Platform::Win, sysops).is_installed());
}

/// 「已注册」与「正在运行」正交：服务装了但停着，仍须判已安装。
///
/// 若这里图省事复用 `is_loaded`（要求 RUNNING），一台 helper 停着的机器会被判成从没装过，
/// 直接丢掉 `needs_repair` 可修复态 —— 用户看到的是「未安装」而非「点一下修复」。
#[test]
fn is_installed_on_windows_true_even_when_service_stopped() {
    let paths = InstallPaths::win();
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    sysops.loaded = false; // 停着
    let m = manager(Platform::Win, sysops);
    assert!(m.is_installed(), "装了但停着仍算已安装");
    assert!(!m.is_loaded(), "但不算正在运行");
}

/// 生命周期操作必须用**本平台**的服务标识。
///
/// 早先 `is_loaded`/`start`/`stop` 一律硬传 `SERVICE_LABEL`（`com.polaris.helper`），
/// 而 Windows 装出来的服务叫 `PolarisHelper` ⇒ `sc query/start/stop` 全部打在不存在的服务上。
///
/// **变异**：把这三个方法改回硬传 `SERVICE_LABEL` → 本条转红。
#[test]
fn lifecycle_uses_platform_service_label() {
    for (platform, want) in [
        (Platform::Win, WIN_SERVICE_NAME),
        (Platform::Mac, SERVICE_LABEL),
        (Platform::Linux, SERVICE_LABEL),
    ] {
        let start_calls = Arc::new(Mutex::new(vec![]));
        let stop_calls = Arc::new(Mutex::new(vec![]));
        let loaded_calls = Arc::new(Mutex::new(vec![]));
        let sysops = MockSysOps {
            exists_paths: HashSet::new(),
            loaded: true,
            registered_services: HashSet::new(),
            start_fails: false,
            start_calls: start_calls.clone(),
            stop_calls: stop_calls.clone(),
            loaded_calls: loaded_calls.clone(),
        };
        let m = manager(platform, sysops);
        m.start().unwrap();
        m.stop().unwrap();
        let _ = m.is_loaded();
        // 三个方法都要断言，缺一即留逃逸面（实测：只断 start/stop 时，
        // 「is_loaded 硬传 SERVICE_LABEL」这个变异存活）。
        assert_eq!(
            (*start_calls.lock().unwrap()).clone(),
            vec![want.to_owned()],
            "{platform:?} 的 start 应作用于 {want}"
        );
        assert_eq!(
            (*stop_calls.lock().unwrap()).clone(),
            vec![want.to_owned()],
            "{platform:?} 的 stop 应作用于 {want}"
        );
        assert_eq!(
            (*loaded_calls.lock().unwrap()).clone(),
            vec![want.to_owned()],
            "{platform:?} 的 is_loaded 应作用于 {want}"
        );
    }
}

#[test]
fn status_not_installed_returns_empty() {
    // Polaris computeStatus: !filesPresent → 全 false（HelperManager.ts:174-184）
    let m = manager(Platform::Mac, MockSysOps::default());
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![])),
    };
    let status =
        m.compute_status_with_client(&HelperClient::new(Box::new(connector), Platform::Mac, ""));
    assert!(!status.installed);
    assert!(!status.ready);
}

#[test]
fn status_installed_but_no_token_needs_repair() {
    // installed 但 token 缺失 → needsRepair（token 文件读不到）
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.clone().expect("mac/linux 有描述符文件"));
    let m = HelperManager::new(
        Platform::Mac,
        PathBuf::from("/nonexistent/path/token"), // 不存在
        Box::new(sysops),
    );
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![])),
    };
    let status =
        m.compute_status_with_client(&HelperClient::new(Box::new(connector), Platform::Mac, ""));
    assert!(status.installed);
    assert!(!status.ready);
    assert!(status.needs_repair);
}

#[test]
fn status_ready_when_proto_above_min_usable() {
    // helper 广告统一的 CURRENT → ready（不再是 上游的 9 ≥ 4）。
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("token");
    crate::token::write_token_content(&token_path, "TOK").unwrap();
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.clone().expect("mac/linux 有描述符文件"));
    let m = HelperManager::new(Platform::Mac, token_path, Box::new(sysops));
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::with_response(
            current_pong_wire(),
        )])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Mac, "TOK");
    let status = m.compute_status_with_client(&client);
    assert!(status.installed);
    assert!(status.ready);
    assert_eq!(
        status.version,
        Some(polaris_helper_proto::proto_version::CURRENT)
    );
    // 自家 helper == 本 build 期望版本 → 不 upgradeable、不 needs_repair。
    assert!(!status.upgradeable);
    assert!(!status.needs_repair);
}

#[test]
fn status_same_proto_without_build_identity_is_ready_but_upgradeable() {
    // .207 现场旧 helper：protocol v1 与随包 helper 相同，但 pong 没 build 字段。它仍可用，
    // 不能误报 needsRepair；同时必须进入既有五语种升级流，否则部署漂移会永久滞留。
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.expect("mac 有描述符文件"));
    let (m, _dir) = manager_with_token(Platform::Mac, sysops);
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::with_response(
            format!(
                "OK pong uid=0 v{}\n",
                polaris_helper_proto::proto_version::CURRENT
            )
            .into_bytes(),
        )])),
    };
    let status = m.compute_status_with_client(&HelperClient::new(
        Box::new(connector),
        Platform::Mac,
        "TOK",
    ));
    assert!(status.ready, "同 proto 旧 helper 仍可用");
    assert!(status.upgradeable, "缺 build identity 必须识别为旧 helper");
    assert!(!status.needs_repair, "升级态不是损坏态");
    assert_eq!(status.build_id, None);
}

#[test]
fn status_same_proto_with_different_build_identity_is_upgradeable() {
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.expect("mac 有描述符文件"));
    let (m, _dir) = manager_with_token(Platform::Mac, sysops);
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::with_response(
            format!(
                "OK pong uid=0 v{} build=older-package\n",
                polaris_helper_proto::proto_version::CURRENT
            )
            .into_bytes(),
        )])),
    };
    let status = m.compute_status_with_client(&HelperClient::new(
        Box::new(connector),
        Platform::Mac,
        "TOK",
    ));
    assert!(status.ready);
    assert!(status.upgradeable);
    assert_eq!(status.build_id.as_deref(), Some("older-package"));
    assert!(!status.needs_repair);
}

#[test]
fn upgradeable_window_is_empty_while_min_usable_equals_current() {
    // 旧测 `status_upgradeable_when_proto_between_min_and_expected` 靠 上游的 MIN_USABLE(4)
    // < EXPECTED(9) 撑出一个「够用但偏旧」的窗口（v5 落在里面）。统一 v1 后 MIN_USABLE ==
    // CURRENT ⇒ 窗口为空——这不是缺陷，是「Polaris 尚无第二代 helper」的事实。
    // `CURRENT` 一旦 +1，窗口自动打开，届时本测该改回「窗口内的版本判 upgradeable」。
    for v in 0..=(polaris_helper_proto::proto_version::CURRENT + 2) {
        let ready = v >= MIN_USABLE_PROTO;
        let upgradeable = ready && v < expected_proto();
        assert!(!upgradeable, "v{v}：当前无更新代次，不该判可升级");
    }
}

#[test]
fn status_needs_repair_when_proto_below_min_usable() {
    // proto=0 < MIN_USABLE(1) → !ready → needsRepair（唯一低于门槛的取值：解析失败也落 0）
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.clone().expect("mac/linux 有描述符文件"));
    let (m, _dir) = manager_with_token(Platform::Mac, sysops);
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::with_response(
            b"OK pong uid=0 v0\n".to_vec(),
        )])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Mac, "TOK");
    let status = m.compute_status_with_client(&client);
    assert!(status.installed);
    assert!(!status.ready);
    assert!(status.needs_repair);
}

// ── W20：status_with_recovery（「装了但停着」分型拉起，Windows 手杀 helper 的自愈腿）──

/// Win 分身：注册 + 二进制在 + token 在 + 停着 + ping 挂 → 拉起一次 + 复核 ready。
/// 变异锁：删恢复腿 / 删 is_loaded 分型 / 拉起后不复核，本条或下两条之一必转红。
/// （is_installed 2026-08-20 起按 Platform 键控证据集：Win 平台在任何宿主都走 SCM 单证据，
/// 故本组测试宿主无关地驱动完整恢复逻辑。）
#[test]
fn recovery_pulls_up_stopped_service_and_becomes_ready() {
    let paths = InstallPaths::win();
    let start_calls = Arc::new(Mutex::new(Vec::new()));
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    sysops.start_calls = start_calls.clone();
    let (m, _dir) = manager_with_token(Platform::Win, sysops);
    // 第一次 ping 挂（broken 流）→ 拉起后复核 ping 通（pong 流）。
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![
            MockStream::broken(std::io::ErrorKind::ConnectionAborted),
            MockStream::with_response(current_pong_wire()),
        ])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Win, "TOK");
    let status = m.status_with_recovery_poll(&client, 3, Duration::from_millis(1));
    assert_eq!(
        *start_calls.lock().unwrap(),
        vec![paths.service_label.to_owned()]
    );
    assert!(status.installed);
    assert!(status.ready, "拉起后复核应就绪");
    assert!(!status.needs_repair);
    assert_eq!(
        status.version,
        Some(polaris_helper_proto::proto_version::CURRENT)
    );
}

/// 分型：跑着（is_loaded）仍 ping 不通 = 结构性问题 → 不拉服务，交回修复流。
#[test]
fn recovery_skips_start_when_running_but_unreachable() {
    let paths = InstallPaths::win();
    let start_calls = Arc::new(Mutex::new(Vec::new()));
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    sysops.loaded = true;
    sysops.start_calls = start_calls.clone();
    let (m, _dir) = manager_with_token(Platform::Win, sysops);
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::broken(
            std::io::ErrorKind::ConnectionAborted,
        )])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Win, "TOK");
    let status = m.status_with_recovery_poll(&client, 2, Duration::from_millis(1));
    assert!(
        start_calls.lock().unwrap().is_empty(),
        "跑着仍不通是结构问题，拉服务是误动作"
    );
    assert!(status.needs_repair);
}

/// 分型：token 缺失 → 拉起也过不了鉴权，不白拉。
#[test]
fn recovery_skips_start_when_token_missing() {
    let paths = InstallPaths::win();
    let start_calls = Arc::new(Mutex::new(Vec::new()));
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    sysops.start_calls = start_calls.clone();
    let m = manager(Platform::Win, sysops); // 不写 token
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::broken(
            std::io::ErrorKind::ConnectionAborted,
        )])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Win, "");
    let status = m.status_with_recovery_poll(&client, 2, Duration::from_millis(1));
    assert!(start_calls.lock().unwrap().is_empty(), "无 token 不白拉");
    assert!(status.needs_repair);
}

/// 拉起失败（二进制被删 / DACL 无 SERVICE_START）→ 如实维持 needs_repair，不粉饰。
///
/// 预置一条 pong 流作「诱饵」：正确的失败路径**不该消费它**（start Err 即返，不轮询）；
/// 若有人把 start 失败吞成 Ok（或删掉 MockSysOps 的失败旋钮），轮询会吃到 pong 变 ready，
/// 本条转红——否则它会退化成与 never_binds 用例不可区分的弱断言（变异电池 M9 实证）。
#[test]
fn recovery_maintains_repair_when_start_fails() {
    let paths = InstallPaths::win();
    let start_calls = Arc::new(Mutex::new(Vec::new()));
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    sysops.start_fails = true;
    sysops.start_calls = start_calls.clone();
    let (m, _dir) = manager_with_token(Platform::Win, sysops);
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![
            MockStream::broken(std::io::ErrorKind::ConnectionAborted),
            MockStream::with_response(current_pong_wire()),
        ])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Win, "TOK");
    let status = m.status_with_recovery_poll(&client, 2, Duration::from_millis(1));
    assert_eq!(start_calls.lock().unwrap().len(), 1, "确实试拉了一次");
    assert!(status.needs_repair, "拉不起 = 真坏，该修不该粉饰");
}

/// 拉起成功但管道始终不绑（如起即崩）→ 轮询耗尽后维持 needs_repair。
#[test]
fn recovery_maintains_repair_when_service_never_binds() {
    let paths = InstallPaths::win();
    let mut sysops = MockSysOps::default();
    sysops
        .registered_services
        .insert(paths.service_label.to_owned());
    let (m, _dir) = manager_with_token(Platform::Win, sysops);
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![
            MockStream::broken(std::io::ErrorKind::ConnectionAborted),
            MockStream::broken(std::io::ErrorKind::ConnectionAborted),
            MockStream::broken(std::io::ErrorKind::ConnectionAborted),
        ])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Win, "TOK");
    let status = m.status_with_recovery_poll(&client, 2, Duration::from_millis(1));
    assert!(status.needs_repair, "复核仍不通 → 维持修复态");
}

#[test]
fn status_needs_repair_when_ping_fails() {
    // ping 连接失败（helper 未跑）→ version None → needsRepair
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.clone().expect("mac/linux 有描述符文件"));
    let (m, _dir) = manager_with_token(Platform::Mac, sysops);
    // connector 返回空（连接失败 → send 报错 → version None）
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Mac, "TOK");
    let status = m.compute_status_with_client(&client);
    assert!(status.installed);
    assert!(!status.ready);
    assert!(status.needs_repair);
}

#[test]
fn start_stop_delegate_to_sysops() {
    let start_calls = Arc::new(Mutex::new(vec![]));
    let stop_calls = Arc::new(Mutex::new(vec![]));
    let sysops = MockSysOps {
        exists_paths: HashSet::new(),
        loaded: true,
        registered_services: HashSet::new(),
        start_calls: start_calls.clone(),
        stop_calls: stop_calls.clone(),
        ..Default::default()
    };
    let m = manager(Platform::Mac, sysops);
    m.start().unwrap();
    m.stop().unwrap();
    assert_eq!(
        (*start_calls.lock().unwrap()).clone(),
        vec![SERVICE_LABEL.to_owned()]
    );
    assert_eq!(
        (*stop_calls.lock().unwrap()).clone(),
        vec![SERVICE_LABEL.to_owned()]
    );
}

#[test]
fn is_loaded_delegates() {
    let sysops = MockSysOps {
        loaded: true,
        ..Default::default()
    };
    let m = manager(Platform::Mac, sysops);
    assert!(m.is_loaded());
}

#[test]
fn prepare_token_reuses_existing() {
    // Polaris install 复用已有 token（HelperManager.ts:478-482）
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let token_path = dir.path().join("token");
    // 预置 token
    token::write_token_content(&token_path, "existing-tok").unwrap();
    let m = HelperManager::new(Platform::Mac, token_path, Box::new(MockSysOps::default()));
    let t = m.prepare_token().unwrap();
    assert_eq!(t, "existing-tok");
}

#[test]
fn prepare_token_generates_when_missing() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let token_path = dir.path().join("token");
    let m = HelperManager::new(
        Platform::Mac,
        token_path.clone(),
        Box::new(MockSysOps::default()),
    );
    let t = m.prepare_token().unwrap();
    assert_eq!(t.len(), 32, "新 token 须 32 hex 字符");
    assert!(token_path.exists());
}

#[test]
fn clear_token_removes_file() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let token_path = dir.path().join("token");
    token::write_token_content(&token_path, "tok").unwrap();
    assert!(token_path.exists());
    let m = HelperManager::new(
        Platform::Mac,
        token_path.clone(),
        Box::new(MockSysOps::default()),
    );
    m.clear_token();
    assert!(!token_path.exists());
}

// ── protoVersion 期望值（前提已换：不再是 mac=9 的三谱系）────────────────────
// 旧测 `expected_proto_mac_is_9` / `expected_proto_matches_helper_proto_constants` 锁的是 上游
// 的 9/5/1。那前提对 Polaris 不成立（无旧版 helper 需被认出），已随常量统一一并推翻。

#[test]
fn expected_proto_is_unified_current() {
    assert_eq!(
        expected_proto(),
        polaris_helper_proto::proto_version::CURRENT
    );
}

// `black_box` 挡住常量折叠：两个阈值都是 const，直写会被 clippy::assertions_on_constants 判为
// 恒真断言。用 const 块断言也能通过，但那样失败形态是**编译不过**、丢掉下面这段解释性 message；
// 这里要的正是「门红了并告诉你为什么」。
#[test]
fn min_usable_not_above_current() {
    // 这条守的是「统一版本号」最凶的连带：门槛留在 上游的 4、而 helper 广告 1 → 每台机器都
    // ready=false + needs_repair=true，TUN 全线不可用。门槛必须 ≤ 当前广告版本。
    let min = std::hint::black_box(MIN_USABLE_PROTO);
    let cur = std::hint::black_box(polaris_helper_proto::proto_version::CURRENT);
    assert!(
        min <= cur,
        "MIN_USABLE_PROTO({min}) > CURRENT({cur}) → 自家 helper 会被判为需修复，TUN 全线不可用"
    );
}

#[test]
fn own_helper_is_ready_and_not_upgradeable() {
    // 端到端语义：本 build 的 helper 广告 CURRENT → 必须判 ready 且不提示可升级。
    // 这是 min_usable / expected_proto 两个阈值的**汇合断言**，任一改坏都会红。
    let v = std::hint::black_box(polaris_helper_proto::proto_version::CURRENT);
    assert!(v >= MIN_USABLE_PROTO, "自家 helper 必须判 ready");
    assert!(v >= expected_proto(), "自家 helper 不该被判为可升级");
}

// ===== 装卸流程（install/uninstall）=====
// ClientError / EscalationOutcome / Executor 经 `use super::*` 从模块级引入。
use crate::privilege::EscalationOutcome;

type CapturedCalls = Arc<Mutex<Vec<Vec<String>>>>;
type CapturedScript = Arc<Mutex<Option<String>>>;

/// 捕获提权 argv + 读脚本文件内容（execute 时脚本尚未清理）的 mock executor。
struct CapturingExecutor {
    calls: CapturedCalls,
    /// pkexec argv[2] 即脚本路径 → 读其内容供断言（linux 全路径可跑）。
    script_content: CapturedScript,
    result: (String, i32),
}
impl Executor for CapturingExecutor {
    fn execute(&self, argv: &[String]) -> Result<(String, i32), ClientError> {
        self.calls.lock().unwrap().push(argv.to_vec());
        // pkexec: [/usr/bin/pkexec, /bin/bash, <scriptPath>]。读脚本内容（清理前）。
        if let Some(path) = argv.get(2) {
            if let Ok(c) = std::fs::read_to_string(path) {
                *self.script_content.lock().unwrap() = Some(c);
            }
        }
        Ok(self.result.clone())
    }
}
fn capturing(result: (String, i32)) -> (CapturingExecutor, CapturedCalls, CapturedScript) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let content = Arc::new(Mutex::new(None));
    let e = CapturingExecutor {
        calls: calls.clone(),
        script_content: content.clone(),
        result,
    };
    (e, calls, content)
}

fn install_params(script_dir: PathBuf, src: PathBuf) -> InstallParams {
    InstallParams {
        src_binary: src,
        bundled_core: PathBuf::from("/app/resources/sing-box"),
        conf_dir: PathBuf::from("/home/user/.config/polaris"),
        uid: 1000,
        script_dir,
    }
}

/// 构造已「装好源二进制」的 manager + 可写 token_path（tempdir）。
fn install_manager(platform: Platform, src: &Path) -> (HelperManager, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("helper-client.token");
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(src.to_path_buf());
    let m = HelperManager::new(platform, token_path, Box::new(sysops));
    (m, dir)
}

// ── 脚本内容（移植保真度 —— mutation 相关：删任一关键步骤即挂）──
#[test]
fn mac_install_script_has_all_steps() {
    let paths = InstallPaths::mac();
    let p = install_params(PathBuf::from("/x"), PathBuf::from("/app/helper"));
    let s = build_mac_install_script(&paths, &p, "TOKEN123");
    // 同目录暂存，旧 job 完全退出后才原子替换。
    assert!(
        s.contains("cp \"$SRC\" \"$NEW_HELPER\"") && s.contains("mv -f \"$NEW_HELPER\" \"$DEST\""),
        "缺 helper 暂存/原子替换"
    );
    assert!(
        s.contains("DEST='/Library/PrivilegedHelperTools/com.polaris.helper'"),
        "DEST 路径错"
    );
    // 写 root 侧 token（600）
    assert!(
        s.contains("printf '%s' 'TOKEN123' > \"$SUPPORT/helper.token\""),
        "缺写 token"
    );
    assert!(
        s.contains("chmod 600 \"$SUPPORT/helper.token\""),
        "token 权限须 600"
    );
    // 播种核（守卫 + codesign）
    assert!(
        s.contains("if [ ! -x \"$COREDIR/sing-box\" ]; then"),
        "缺核播种守卫"
    );
    assert!(s.contains("codesign --force --sign -"), "缺 codesign");
    // 写 plist（含 daemon flag）
    assert!(
        s.contains("<key>Label</key><string>com.polaris.helper</string>"),
        "plist Label 错"
    );
    assert!(
        s.contains("<string>--singbox</string>"),
        "plist 缺 --singbox flag"
    );
    assert!(
        s.contains("<string>--coredir</string>"),
        "plist 缺 --coredir flag"
    );
    // bootstrap
    assert!(
        s.contains("launchctl bootstrap system \"$PLIST\""),
        "缺 launchctl bootstrap"
    );
    assert!(s.contains("echo installed-ok"));
}

#[test]
fn mac_helper_upgrade_stops_before_replace_and_rolls_back_on_failure() {
    let paths = InstallPaths::mac();
    let p = install_params(PathBuf::from("/x"), PathBuf::from("/app/helper"));
    let script = build_mac_install_script(&paths, &p, "TOKEN123");

    let armed = script
        .find("trap rollback_install EXIT")
        .expect("缺破坏性阶段回滚闸");
    let stop = script[armed..]
        .find("launchctl bootout \"system/$LABEL\"")
        .map(|at| at + armed)
        .expect("缺硬 bootout");
    let wait = script[stop..]
        .find("while launchctl print \"system/$LABEL\"")
        .map(|at| at + stop)
        .expect("缺旧 job 退出确认");
    let replace = script[wait..]
        .find("mv -f \"$NEW_HELPER\" \"$DEST\"")
        .map(|at| at + wait)
        .expect("缺 helper 原子替换");
    let bootstrap = script[replace..]
        .find("launchctl bootstrap system \"$PLIST\"")
        .map(|at| at + replace)
        .expect("缺新 job bootstrap");
    let verify = script[bootstrap..]
        .find("launchctl print \"system/$LABEL\" >/dev/null")
        .map(|at| at + bootstrap)
        .expect("缺新 job 注册态确认");

    assert!(
        armed < stop && stop < wait && wait < replace && replace < bootstrap && bootstrap < verify,
        "mac 升级必须按 回滚武装→停旧→确认退出→原子替换→启动→确认 的顺序执行"
    );
    assert!(
        !script[armed..replace]
            .contains("launchctl bootout \"system/$LABEL\" >/dev/null 2>&1 || true"),
        "破坏性阶段不得吞掉 bootout 失败"
    );
    for needle in [
        "cp \"$DEST\" \"$TXDIR/helper.rollback\"",
        "cp \"$PLIST\" \"$TXDIR/plist.rollback\"",
        "cp \"$TXDIR/helper.rollback\" \"$NEW_HELPER\"",
        "cp \"$TXDIR/plist.rollback\" \"$NEW_PLIST\"",
        "rollback incomplete; recovery files kept at $TXDIR",
    ] {
        assert!(script.contains(needle), "mac 回滚腿缺：{needle}");
    }
}

#[test]
fn mac_plist_flags_match_daemon_parse_args() {
    // daemon macos/daemon.rs:parse_args 认 --singbox/--confdir/--support/--coredir；plist argv 须逐一对上。
    let plist = render_mac_plist(
        "/Library/PrivilegedHelperTools/com.polaris.helper",
        "/Library/Application Support/Polaris/core",
        "/home/u/conf",
        "/Library/Application Support/Polaris",
    );
    assert!(plist.contains("<string>--singbox</string><string>/Library/Application Support/Polaris/core/sing-box</string>"));
    assert!(plist.contains("<string>--confdir</string><string>/home/u/conf</string>"));
    assert!(plist.contains(
        "<string>--support</string><string>/Library/Application Support/Polaris</string>"
    ));
    assert!(plist.contains(
        "<string>--coredir</string><string>/Library/Application Support/Polaris/core</string>"
    ));
    assert!(plist.contains("<key>RunAtLoad</key><true/>"));
    assert!(plist.contains("<key>KeepAlive</key><true/>"));
}

#[test]
fn linux_install_script_has_all_steps() {
    let paths = InstallPaths::linux();
    let p = install_params(PathBuf::from("/x"), PathBuf::from("/app/helper"));
    let s = build_linux_install_script(&paths, &p);
    // 同目录暂存后原子替换二进制
    assert!(
        s.contains("DEST='/usr/local/lib/polaris/helper'")
            && s.contains("NEW_HELPER=\"$DEST.new.$$\"")
            && s.contains("install -D -o root -g root -m 0755 '/app/helper' \"$NEW_HELPER\"")
            && s.contains("mv -f \"$NEW_HELPER\" \"$DEST\""),
        "缺 helper 同目录暂存/原子替换"
    );
    // 播种核守卫
    assert!(
        s.contains("if [ ! -x '/usr/local/lib/polaris/core/sing-box' ]; then"),
        "缺核播种守卫"
    );
    // 授权 uid 合并追加（不覆写）
    assert!(
        s.contains("grep -qxF '1000' '/var/lib/polaris/authorized-uids'"),
        "缺 uid 授权"
    );
    // 装 unit（含 daemon flag）
    assert!(s.contains("ExecStart=/usr/local/lib/polaris/helper --socket=/run/polaris/helper.sock --authfile=/var/lib/polaris/authorized-uids --coredir=/usr/local/lib/polaris/core"), "ExecStart flag 须对齐 daemon parse_args");
    assert!(s.contains("RuntimeDirectory=polaris"));
    // reload + enable + restart：restart 在首装 inactive 时会启动，在升级 active 时必换进程。
    assert!(s.contains("systemctl daemon-reload"), "缺 daemon-reload");
    assert!(
        s.contains("systemctl enable \"$SERVICE\"") && s.contains("systemctl restart \"$SERVICE\""),
        "缺 enable + restart"
    );
    assert!(
        !s.contains("enable --now"),
        "enable --now 不会替换已 active 的旧进程"
    );
    assert!(
        !s.contains("try-restart"),
        "try-restart 在首装 inactive 时不会启动服务"
    );
    assert!(s.contains("echo polaris-helper-install-ok"));
}

#[test]
fn linux_helper_upgrade_is_transactional_and_verifies_the_running_inode() {
    let paths = InstallPaths::linux();
    let p = install_params(PathBuf::from("/x"), PathBuf::from("/app/helper"));
    let script = build_linux_install_script(&paths, &p);

    let snapshot_state = script
        .find("systemctl is-active --quiet \"$SERVICE\"")
        .expect("缺旧服务运行态快照");
    let live_binary_backup = script
        .find("cp \"/proc/$OLD_PID/exe\" \"$TXDIR/helper.rollback\"")
        .expect("active 服务必须从 /proc/MainPID/exe 备份最后已知可用进程");
    let stage_helper = script
        .find("install -D -o root -g root -m 0755 '/app/helper' \"$NEW_HELPER\"")
        .expect("缺新 helper 暂存");
    let stop = script[snapshot_state..]
        .find("systemctl stop \"$SERVICE\"; fi")
        .map(|at| at + snapshot_state)
        .expect("缺破坏性写入前停旧服务");
    let replace = script[stop..]
        .find("mv -f \"$NEW_HELPER\" \"$DEST\"")
        .map(|at| at + stop)
        .expect("缺原子替换");
    let restart = script[replace..]
        .find("systemctl restart \"$SERVICE\"")
        .map(|at| at + replace)
        .expect("缺新服务启动");
    let verify = script
        .find("while [ \"$TAKEOVER_ATTEMPTS\" -gt 0 ]; do")
        .expect("缺有界接管等待");
    let commit = script
        .find("trap - EXIT HUP INT TERM\nrm -rf \"$TXDIR\"\necho polaris-helper-install-ok")
        .expect("缺验证后的事务提交");

    assert!(
        snapshot_state < live_binary_backup
            && live_binary_backup < stage_helper
            && stage_helper < stop
            && stop < replace
            && replace < restart
            && restart < verify
            && verify < commit,
        "快照/暂存/停服/替换/启动/验证/提交顺序错误"
    );
    for needle in [
        "rollback_install() {",
        "install -o root -g root -m 0755 \"$TXDIR/helper.rollback\" \"$NEW_HELPER\"",
        "install -o root -g root -m 0644 \"$TXDIR/unit.rollback\" \"$NEW_UNIT\"",
        "if [ \"$WAS_ACTIVE\" -eq 1 ] && ! systemctl restart \"$SERVICE\"",
        "rollback incomplete; recovery files kept at $TXDIR",
        "TAKEOVER_ATTEMPTS=50",
        "systemctl is-active --quiet \"$SERVICE\" 2>/dev/null",
        "NEW_PID=$(systemctl show -p MainPID --value \"$SERVICE\")",
        "cmp -s \"/proc/$NEW_PID/exe\" \"$DEST\"",
        "TAKEOVER_ATTEMPTS=$((TAKEOVER_ATTEMPTS - 1))",
        "sleep 0.1",
        "HELPER_TAKEOVER_TIMEOUT",
    ] {
        assert!(script.contains(needle), "回滚腿缺：{needle}");
    }
}

#[cfg(unix)]
fn assert_shell_syntax(shell: &str, script: &str) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new(shell)
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("无法启动 {shell} 做脚本语法检查：{error}"));
    child
        .stdin
        .take()
        .expect("语法检查器缺 stdin")
        .write_all(script.as_bytes())
        .expect("无法把安装脚本写给语法检查器");
    let output = child.wait_with_output().expect("无法读取语法检查结果");
    assert!(
        output.status.success(),
        "{shell} -n 未通过：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn generated_unix_install_scripts_are_valid_shell() {
    let params = install_params(PathBuf::from("/x"), PathBuf::from("/app/helper"));
    let linux = build_linux_install_script(&InstallPaths::linux(), &params);
    assert_shell_syntax("/bin/sh", &linux);

    let mac = build_mac_install_script(&InstallPaths::mac(), &params, "TOKEN");
    assert_shell_syntax("/bin/bash", &mac);
}

/// **跨侧契约门**：linux 安装脚本给 authfile 的权限，必须落在**读侧 helper 的 accept 面**内。
///
/// 对端判据锚点 —— `crates/helper/src/platform/linux/auth.rs`：
/// `const GROUP_OTHER_BITS: u32 = 0o077;`（:164）+ `authorize_uid()` 里
/// `if mode & GROUP_OTHER_BITS != 0 { return Err(GroupOrOtherAccessible) }`（:188）。
/// 叠加同函数的 `owner_uid != 0` 分支 ⇒ **accept 面 = root 属主 且 0600 或更严**。
///
/// 本脚本的历史值 `root:root 0644` 恰好落在**拒绝**分支上：装完 helper 对 authfile 里
/// 任何非 root uid 恒判 unauthorized，Linux SO_PEERCRED 授权腿整条不可用；且 repair 复用
/// 同一份安装脚本 ⇒ 人工 `chmod 0600` 修好的文件会被下一次修复重新改坏。两侧各自有测试
/// 且都绿（读侧钉判据、写侧钉「装了哪些步骤」），**唯独没人比过两侧指的是不是同一个面**。
///
/// 断言取「**恰一处** + 值恰为 0600」而非 `contains("chmod 0600")`：后者在脚本尾部再补一条
/// `chmod 0644 authfile` 的情况下**照样绿**（最后一条才是生效值），门等于没有。
#[test]
fn linux_install_script_chmods_authfile_0600_matching_helper_auth_contract() {
    let paths = InstallPaths::linux();
    let p = install_params(PathBuf::from("/x"), PathBuf::from("/app/helper"));
    let script = build_linux_install_script(&paths, &p);

    // 判据独立于生产常量：这里写死字面路径，不引 LINUX_AUTH_FILE
    //（引常量的话，常量一改判据跟着漂，门就跟着被判对象走了）。
    let authfile = "/var/lib/polaris/authorized-uids";

    // 取材面自检：脚本里必须真提到 authfile，否则下面「恰一条」的计数是空集恒过。
    // 期望至少 3 行：创建 / chmod / uid 合并追加（systemd unit 的 --authfile= 还有一行）。
    let mentions: Vec<&str> = script.lines().filter(|l| l.contains(authfile)).collect();
    assert!(
        mentions.len() >= 3,
        "取材面塌：脚本里提到 authfile 的行只剩 {} 条，期望 ≥3（创建/chmod/uid 追加）。实际：{mentions:#?}",
        mentions.len()
    );

    // 所有给 authfile 设权限的行（chmod 或 install -m）——必须恰一条，且恰为 0600。
    let mode_lines: Vec<&str> = mentions
        .iter()
        .copied()
        .filter(|l| l.contains("chmod") || l.contains("install "))
        .collect();
    assert_eq!(
        mode_lines.len(),
        1,
        "给 authfile 设权限的行必须恰一条（第二条会覆盖第一条，把 0600 改回宽权限也无人拦）。实际：{mode_lines:#?}"
    );
    assert_eq!(
        mode_lines[0].trim(),
        format!("chmod 0600 '{authfile}'"),
        "authfile 权限须恰为 0600 —— 读侧 auth.rs GROUP_OTHER_BITS=0o077，mode & 0o077 != 0 一律拒绝"
    );

    // 创建瞬窗：裸 `touch` 的创建模式是 0666 & ~umask，pkexec 常见 umask 022 ⇒ 先落地 0644
    // 再被 chmod 收紧，中间存在其他用户可读的瞬窗。故创建圈进 umask 077 子壳（不依赖继承 umask）。
    assert!(
        script.contains(&format!("(umask 077; touch '{authfile}')")),
        "authfile 创建须在 umask 077 子壳内，消除 chmod 0600 生效前的宽权限瞬窗"
    );
}

/// **接线门**：状态探测读的路径 / 服务名，必须与安装脚本真正写的逐字一致。
///
/// 这条测试的全部价值在于把两条曾经分叉的真相源钉在一起。此前两侧**各自都有测试且都绿**——
/// `win_install_script_has_all_steps` 钉脚本落点、`is_installed_*` 用 mock 自填路径钉判定
/// （判据与被判对象同源，是恒真的同义反复）——**唯独没人比过它们指不指同一个东西**。
/// 于是 Windows 上 `is_installed` 恒 false。典型的「测方法体不测接线」。
///
/// 顺带钉死「落点不再随源文件名漂移」：这里故意传一个**改过名**的 src_binary。
#[test]
fn win_install_script_targets_the_same_paths_status_probes() {
    let paths = InstallPaths::win();
    // 故意用与目标不同的源文件名：落点应恒为 WIN_HELPER_EXE，不随 src 漂移。
    let p = install_params(
        PathBuf::from("/x"),
        PathBuf::from(r"C:\app\renamed-helper.exe"),
    );
    let script = build_win_install_script(&paths, &p, "WTOKEN");

    let binary = paths.binary.to_string_lossy();
    assert!(
        script.contains(&format!("$helperDst = '{binary}'")),
        "状态探测查 {binary}，安装脚本却装到别处——两条真相源又分叉了"
    );
    // 源文件名出现在 `$helperSrc` 是对的（那是拷贝来源）；要钉的是**目标**不随它漂移。
    assert!(
        !script.contains(&format!(r"{WIN_SUPPORT_DIR}\renamed-helper.exe")),
        "落点不得随源文件名漂移"
    );
    assert!(
        script.contains(&format!("New-Service -Name {}", paths.service_label)),
        "安装脚本注册的服务名与状态探测用的不一致"
    );
    // 卸载脚本也得指同一个服务，否则卸不干净、下次装撞 1072。
    assert!(
        build_win_uninstall_script().contains(&format!("delete {}", paths.service_label)),
        "卸载脚本删的服务名与状态探测不一致"
    );
}

#[test]
fn win_install_script_has_all_steps() {
    let p = install_params(
        PathBuf::from("/x"),
        PathBuf::from(r"C:\app\polaris-helper.exe"),
    );
    let s = build_win_install_script(&InstallPaths::win(), &p, "WTOKEN");
    // 外置副本到 ProgramData
    assert!(
        s.contains(r"$helperDst = 'C:\ProgramData\Polaris\polaris-helper.exe'"),
        "helperDst 须外置到 ProgramData"
    );
    // 锁 ACL
    assert!(
        s.contains(r#"/grant:r "*S-1-5-18:(OI)(CI)(F)" "*S-1-5-32-544:(OI)(CI)(F)""#),
        "缺目录 ACL 锁（主体须为数值 SID，英文组名在非英文 Windows 上解析不到）"
    );
    // 写 token
    assert!(
        s.contains("Set-Content -Path $tokenFile -Value 'WTOKEN' -NoNewline -Encoding ascii"),
        "缺写 token"
    );
    // binPath 含 daemon flag（真双引号）。P4：`--singbox` 由受保护核目录派生，**不再**是
    // `params` 里那个 app 侧用户可写核（原断言是 `C:\app\sing-box.exe`）。
    assert!(
        s.contains(r#"--singbox "C:\ProgramData\Polaris\core\sing-box.exe" --confdir"#),
        "binPath 的 --singbox 必须指受保护核目录"
    );
    assert!(
        s.contains(r#"--support "C:\ProgramData\Polaris""#),
        "binPath 缺 --support"
    );
    // New-Service + start
    assert!(
        s.contains("New-Service -Name PolarisHelper -BinaryPathName $bp -StartupType Automatic"),
        "缺 New-Service"
    );
    assert!(s.contains("& $sc start PolarisHelper"), "缺 sc start");
    assert!(
        s.contains("& $icacls $support /inheritance:r"),
        "缺 icacls inheritance:r"
    );
    assert!(
        s.contains("$ErrorActionPreference = 'Stop'"),
        "缺 fail-loud"
    );
    // 🔴 病根牙（2026-08-19 提权重放首曝）：`& '$env:...'` / `& '$sc'` 这类**单引号包任何
    // $ 引用**都是字面量不展开 → CommandNotFound + EAP=Stop → 脚本必死。本脚本所有 `& `
    // 调用位都应是裸变量/裸路径，出现 `& '$` 即病（评审实证：分句带冒号的禁令是永真死针，
    // `& '$sc'` 穿透）。
    assert!(
        !s.contains("& '$"),
        "win 安装脚本出现单引号包 $ 引用的调用——CommandNotFound 必死形态"
    );
    assert!(
        s.contains(r#"$icacls = "$env:SystemRoot\System32\icacls.exe""#),
        "缺 $env: 双引号变量赋值（icacls）"
    );
    assert!(
        s.contains(r#"$sc = "$env:SystemRoot\System32\sc.exe""#),
        "缺 $env: 双引号变量赋值（sc）"
    );
}

/// W20：安装脚本必须配好两层自愈——① `sdset` 授 IU `SERVICE_START`（默认服务 DACL 只给交互
/// 用户查询权，.207 实测无 RP；不补授则未提权 app 永远拉不起停着的服务）；② `sc failure`
/// 失败恢复（任务管理器手杀/崩溃 → SCM 5s 自动重启，对齐 mac KeepAlive / linux Restart）。
/// 两者都必须在首次 `sc start` 之前，且各带 `$LASTEXITCODE` 守卫——PS 5.1 的 EAP=Stop 拦不住
/// 外部程序非零退出（评审 F3），静默失败会让安装「看着成功」而自愈全没配上。
/// 变异锁：删任一行 / 删守卫 / 挪到 start 之后 → 转红。
///
/// 第 ⑤ 段把「哪些外部命令必须带守卫」从**手写清单**改成**扫描派生**（S-3）：新增一条
/// `& $icacls …` / `& $sc …` 自动进覆盖面，不改这里一个字。
/// 变异锁：加一条无守卫的假腿 → 计数与逐条定位双红；把守卫写到别处凑数 → 计数红。
#[test]
fn win_install_script_self_heals_and_grants_iu_start_before_first_start() {
    let script = build_win_install_script(
        &InstallPaths::win(),
        &install_params(
            PathBuf::from("/x"),
            PathBuf::from(r"C:\app\polaris-helper.exe"),
        ),
        "WTOKEN",
    );
    // ① IU 启动权：逐字钉死 sdset 行——IU 的第二段 ACE 恰好只有 RP（只授 start，不授
    // stop/改配置）；改任何一段（尤其给 IU 加权）都该过 review 而不是悄悄过。
    assert!(
        script.contains(
            "& $sc sdset PolarisHelper \"D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)(A;;RP;;;IU)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)\""
        ),
        "缺 sdset 授 IU SERVICE_START（W20 恢复腿的硬前提）"
    );
    // ② 失败恢复 + ③ 三道退出码守卫（sdset/failure/start）+ ④ 次序：配置先于首启。
    let sdset_at = script.find("sdset PolarisHelper").expect("缺 sdset");
    let failure_line = "& $sc failure PolarisHelper reset= 86400 actions= restart/5000/restart/5000/restart/5000 | Out-Null";
    let failure_at = script.find(failure_line).expect("缺 sc failure 自愈配置");
    let start_at = script
        .find("& $sc start PolarisHelper")
        .expect("缺 sc start");
    assert!(
        sdset_at < failure_at && failure_at < start_at,
        "自愈配置必须在首次 start 之前（首启即被覆盖）"
    );
    // ⑤ **覆盖轴从脚本自身派生，不写手工清单**。
    //
    // 上一版这里是一张写死的 10 条 icacls 腿 + 精确计数 13。那张清单的绕过面是现成的：新增
    // 第 11 条 `& $icacls …` 且不带守卫，只要把计数改一下、再在任意一条非 icacls 行后面多写
    // 一个 `if ($LASTEXITCODE -ne 0)`（写进注释里都行——`matches()` 不剥注释），门就全绿。
    // 即「覆盖面由夹具定」而不是「由判据定」。改成扫描派生后，新增腿**自动进取材面**。
    let face = script_code_face(&script);
    let head = guarded_region(&face);
    let calls = derived_guarded_calls(head);

    // 取材面自检①：派生集合非空。空集会让下面的逐条断言与计数一起变成恒真。
    assert!(
        !calls.is_empty(),
        "取材面自检失败：一条待守卫的外部命令都没派生出来（变量名改了？）—— \
         空集上的逐条断言与计数断言都恒真"
    );
    // 取材面自检②：两族都在。只剩一族（例如 `$icacls` 改了变量名）时集合仍非空，
    // 但那一族的全部腿会**静默**退出覆盖面。
    assert!(
        calls.iter().any(|l| l.starts_with("& $icacls ")),
        "取材面自检失败：派生集合里没有 icacls 腿"
    );
    assert!(
        calls.iter().any(|l| l.starts_with("& $sc ")),
        "取材面自检失败：派生集合里没有 sc 腿"
    );
    // 取材面自检③：不许有重复行 —— 逐条定位用 `find()` 取首次出现，重复行会让第二次
    // 迭代重复验证第一处，那条真腿无人检查。
    let mut seen = HashSet::new();
    for call in &calls {
        assert!(
            seen.insert(*call),
            "派生集合里有重复行（逐条定位会重复查到第一处）：{call}"
        );
    }
    // 正面断言（防缩水）：当前脚本的待守卫外部命令是 7 条 icacls + 3 条 sc。派生集合会随脚本
    // 一起缩小，故单靠「派生数 == 守卫数」拦不住「把整段 ACL 加固删了」——那一刀下去两边同时
    // 归零仍然相等。这条地板不是门本身，是门的下限。
    assert!(
        calls.len() >= 10,
        "待守卫的外部命令只剩 {} 条（应 ≥10）：ACL 加固或自愈配置被整段删了？",
        calls.len()
    );

    // 计数：**恰好**等于派生集合大小。多一个 = 有人在非受守行后面凑了个守卫；少一个 = 有腿
    // 没守。`} catch {` 之后不计——回滚段是 EAP=SilentlyContinue 的 best-effort，刻意不守。
    assert_eq!(
        head.matches("if ($LASTEXITCODE -ne 0)").count(),
        calls.len(),
        "退出码守卫数与待守卫的外部命令数不等（EAP=Stop 拦不住外部程序非零退出）"
    );
    // 计数只保证「有 N 处」，不保证它们长在哪儿——N 条全挂在 sdset 后面也是 N。
    // 故逐条钉「这条命令之后、下一条 `& $` 调用之前就是它的守卫」。
    for call in &calls {
        let at = head
            .find(call)
            .unwrap_or_else(|| panic!("派生腿在净化面里找不回来：{call}"));
        let rest = &head[at + call.len()..];
        let guard_at = rest
            .find("if ($LASTEXITCODE -ne 0)")
            .unwrap_or_else(|| panic!("外部命令没有退出码守卫：{call}"));
        // 下一条外部命令调用（`& $`）必须在守卫之后 —— 否则守卫查的是别人的退出码。
        let next_call = rest.find("\n& $").unwrap_or(rest.len());
        assert!(
            guard_at < next_call,
            "退出码守卫被下一条外部命令抢在前面（查的不是它自己的退出码）：{call}"
        );
    }
}

/// PowerShell 脚本的**净化取材面**：`#` 整行注释整行抹空（保留行数与行序，`find()` 的顺序
/// 语义不变）。
///
/// 非剥不可：本文件的判据全是 `contains` / `find` / `matches().count()` 型，而注释里随手写一句
/// `if ($LASTEXITCODE -ne 0)` 就能替一条真腿作证 —— 判据被自己的取材面污染。
/// 偏移在净化面里自洽，故**下游一律拿净化面自己去 find**，绝不把它的偏移套回原串。
fn script_code_face(script: &str) -> String {
    script
        .lines()
        .map(|l| {
            if l.trim_start().starts_with('#') {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 净化面里「必须守退出码」的区域：升级事务的 `} catch {` **之前**。
///
/// 回滚段刻意是 best-effort（`$ErrorActionPreference = 'SilentlyContinue'`，失败也要把能恢复的
/// 恢复完），它里面的 `& $sc …` 不该被要求带守卫。锚点取不到即 panic：门失去射程必须当场红。
fn guarded_region(face: &str) -> &str {
    let at = face
        .find("} catch {")
        .expect("找不到 `} catch {` —— 事务边界没了，守卫门的射程无从谈起");
    &face[..at]
}

/// 从安装脚本**派生**「必须带退出码守卫的外部命令」集合（S-3：覆盖轴由判据定，不由夹具定）。
///
/// 取材规则逐条：
/// 1. 入参必须是 [`guarded_region`] 切出来的**净化面**（注释已抹空、catch 段已切掉）；
/// 2. 行首（`trim_start` 后）是 `& $icacls ` 或 `& $sc ` 的行 —— 即脚本里全部外部程序调用；
/// 3. **排除**以 `2>$null | Out-Null` 结尾的行：`sc stop` / `sc delete` 在幂等重装时本就期望
///    非零退出（服务不存在是常态），给它们加守卫等于把每次「首次安装」判成失败。
///    这一条是**有意不守**的唯一形态，写成后缀判据而不是列服务名，新增同形腿自动豁免。
fn derived_guarded_calls(head: &str) -> Vec<&str> {
    head.lines()
        .map(str::trim_end)
        .filter(|l| {
            let t = l.trim_start();
            (t.starts_with("& $icacls ") || t.starts_with("& $sc "))
                && !t.ends_with("2>$null | Out-Null")
        })
        .collect()
}

/// **本地化门**：icacls 的主体一律用数值 SID，不许出现英文组名。
///
/// BUILTIN / NT AUTHORITY 的账户名**是本地化的**（德语 `Benutzer`、法语 `Utilisateurs`…）。英文名
/// 在非英文 Windows 上解析不到 → icacls 报「No mapping between account names and security IDs was
/// done」并非零退出。本批给每条 icacls 都补了退出码守卫，若还留着英文名，原先的**静默弱 ACL**就会
/// 变成**装不上**；换 well-known SID 后这条故障路径整条消失，守卫也就不会误伤正常机器。
///
/// 判据钉 **SID 串本身**而不是「包含 Users」：后者改回英文名 `Users:` 照样绿，门等于没有。
#[test]
fn win_install_script_uses_numeric_sids_so_localized_windows_resolves_them() {
    let script = build_win_install_script(
        &InstallPaths::win(),
        &install_params(
            PathBuf::from("/x"),
            PathBuf::from(r"C:\app\polaris-helper.exe"),
        ),
        "WTOKEN",
    );

    // 取材面自检：先证明「所有 icacls 行」这个集合非空且完整，否则下面的全称否定是空集恒过。
    let icacls_lines: Vec<&str> = script
        .lines()
        .filter(|l| l.trim_start().starts_with("& $icacls "))
        .collect();
    // 7 = 4 条 `/setowner` + 3 条 `/inheritance:r /grant:r`（N-1a 把后两类合并成一条命令之后
    // 从 10 条降到 7 条；这是**地板**，防的是取材面缩水后全称否定变成空集恒过）。
    assert!(
        icacls_lines.len() >= 7,
        "取材面自检失败：只取到 {} 条 icacls 行（应 ≥7），全称否定会变成空集恒过",
        icacls_lines.len()
    );

    // 全称否定：一条 icacls 行都不许出现英文主体名。
    for line in &icacls_lines {
        for english in ["SYSTEM:", "Administrators:", "Users:"] {
            assert!(
                !line.contains(english),
                "icacls 用了本地化敏感的英文组名 `{english}`（非英文 Windows 上解析失败 → \
                 非零退出 → 守卫 throw → 装不上）：{line}"
            );
        }
        // 正面断言：每条带主体的腿都得真的出现 `*S-`。`/inheritance:r` 不带主体，豁免。
        assert!(
            line.contains("/inheritance:r") || line.contains("*S-"),
            "icacls 腿既没有主体 SID 也不是 /inheritance:r：{line}"
        );
    }

    // 逐字钉三个 well-known SID（跨语言恒等，与 helper 侧 `coreacl` 的白名单同源）。
    assert!(
        script.contains(
            r#"& $icacls $coreDir /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)(F)" "*S-1-5-32-544:(OI)(CI)(F)" "*S-1-5-32-545:(OI)(CI)(RX)""#
        ),
        "受保护核目录的 grant 必须是 SYSTEM/Administrators/Users 三个数值 SID"
    );
    assert!(
        script.contains(
            r#"& $icacls $support /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)(F)" "*S-1-5-32-544:(OI)(CI)(F)""#
        ),
        "support 目录的 grant 必须换成数值 SID（这条腿此前是英文名且无守卫）"
    );
    assert!(
        script.contains(
            r#"& $icacls $helperDst /inheritance:r /grant:r "*S-1-5-18:(F)" "*S-1-5-32-544:(F)""#
        ),
        "helper.exe 的 grant 必须换成数值 SID（这条腿此前是英文名且无守卫）"
    );
}

/// 🔴 **N-1a + N-1b：DACL 一次写入 + owner 归 Administrators。**
///
/// # N-1a：`/inheritance:r` 与 `/grant:r` 必须在**同一条** icacls 命令里
///
/// 拆成两条时，`/setowner` 已先把属主交出去，而**新建目录上的 ACE 全是继承来的**，
/// `/inheritance:r` 把它们一次删光 ⇒ 中间存在一个「调用者既非 owner、又没有任何 ACE」的
/// 空 DACL 瞬间。此时紧随的 `/grant:r` 写不写得进去，取决于 icacls 会不会自行启用
/// `SeRestorePrivilege` —— 无一手来源。任一依赖不成立 ⇒ 守卫 throw ⇒ **全新安装装不上**
/// （不是本地化机器才炸，是全量）。合成一条 = 一次 `SetNamedSecurityInfo`，中间态不存在。
///
/// # N-1b：`/setowner` 的目标是 `*S-1-5-32-544` 而不是 `*S-1-5-18`
///
/// 设一个**不在调用者 token 里**的 SID 当 owner（SYSTEM 对提权管理员正是如此）需要
/// `SeRestorePrivilege` 已启用；`BUILTIN\Administrators` 在提权 token 里且带 `SE_GROUP_OWNER`，
/// 设它当 owner 零特权。安全上无差：Administrators 本就持 `(F)`，owner 隐含的 `WRITE_DAC`
/// 不增加任何能力，而「预创建者以 CREATOR OWNER 留在 owner 位」照样被替掉。544 在 helper 侧
/// `coreacl::PRIVILEGED_SIDS`（owner 白名单）内 ⇒ 起核前自检照常放行。
///
/// 变异锁：把任一条合并调用拆回两条 → N-1a 段红；把任一 `/setowner` 改回 `*S-1-5-18` → N-1b 段红。
#[test]
fn win_install_script_writes_each_dacl_in_one_call_and_owns_by_administrators() {
    let script = build_win_install_script(
        &InstallPaths::win(),
        &install_params(
            PathBuf::from("/x"),
            PathBuf::from(r"C:\app\polaris-helper.exe"),
        ),
        "WTOKEN",
    );
    let face = script_code_face(&script);
    let icacls: Vec<&str> = face
        .lines()
        .map(str::trim_end)
        .filter(|l| l.trim_start().starts_with("& $icacls "))
        .collect();
    // 取材面自检：集合非空且覆盖全部四个受保护对象，否则下面按子集分组的断言会变成空集恒过。
    assert!(
        icacls.len() >= 7,
        "取材面自检失败：只取到 {} 条 icacls 行（应 ≥7）",
        icacls.len()
    );
    for obj in ["$support", "$coreDir", "$tokenFile", "$helperDst"] {
        assert!(
            icacls.iter().any(|l| l.contains(obj)),
            "取材面自检失败：没有一条 icacls 打在 {obj} 上"
        );
    }

    // ── N-1a：去继承与授权同命令 ──────────────────────────────────────────────
    let inherit: Vec<&str> = icacls
        .iter()
        .copied()
        .filter(|l| l.contains("/inheritance:r"))
        .collect();
    let grant: Vec<&str> = icacls
        .iter()
        .copied()
        .filter(|l| l.contains("/grant:r"))
        .collect();
    assert_eq!(
        inherit.len(),
        3,
        "去继承的对象应恰好是 $support / $coreDir / $helperDst 三个，实得 {inherit:?}"
    );
    assert_eq!(
        inherit, grant,
        "`/inheritance:r` 与 `/grant:r` 必须逐条同命令：拆开会在两条命令之间留出空 DACL \
         中间态（调用者既非 owner 又无 ACE），第二条能否写入取决于 icacls 是否自行启用 \
         SeRestorePrivilege —— 不成立则全新安装装不上"
    );
    for line in &inherit {
        let inherit_at = line.find("/inheritance:r").expect("上面刚筛过");
        let grant_at = line.find("/grant:r").expect("上面刚筛过");
        assert!(
            inherit_at < grant_at,
            "同一条命令里 `/inheritance:r` 必须在 `/grant:r` 之前（顺序即语义）：{line}"
        );
    }

    // ── N-1b：owner 归 Administrators ─────────────────────────────────────────
    let setowner: Vec<&str> = icacls
        .iter()
        .copied()
        .filter(|l| l.contains("/setowner"))
        .collect();
    assert_eq!(
        setowner.len(),
        4,
        "四个受保护对象（$support/$coreDir/$tokenFile/$helperDst）都要显式 /setowner，实得 {setowner:?}"
    );
    for line in &setowner {
        assert!(
            line.contains(r#"/setowner "*S-1-5-32-544""#),
            "/setowner 的主体必须是 BUILTIN\\Administrators（*S-1-5-32-544）：{line}"
        );
        assert!(
            !line.contains("*S-1-5-18"),
            "owner 设成不在提权 token 里的 SID（SYSTEM）需要 SeRestorePrivilege 已启用，\
             无一手来源保证 icacls 会自行启用它：{line}"
        );
    }
    // 正面对照：544 当 owner 不等于「放弃 SYSTEM 的权」—— 三条 grant 里 SYSTEM 的 (F) 必须还在，
    // 否则「owner 换人」就悄悄变成了「服务身份失去完全控制」。
    for line in &grant {
        assert!(
            line.contains("*S-1-5-18:("),
            "grant 里丢了 SYSTEM 的完全控制（服务本体就是以 SYSTEM 跑的）：{line}"
        );
    }
}

/// W24：覆盖升级不是“删旧 → 祈祷新服务能起”。旧 helper/token/SCM 快照必须先于第一处
/// 破坏性写入；只有新服务首启成功才删备份；catch 必须恢复两份文件和原服务运行态。
#[test]
fn win_helper_upgrade_is_transactional_and_keeps_recovery_copies_on_failure() {
    let script = build_win_install_script(
        &InstallPaths::win(),
        &install_params(
            PathBuf::from("/x"),
            PathBuf::from(r"C:\app\polaris-helper.exe"),
        ),
        "WTOKEN",
    );
    let snapshot = script
        .find("$oldService = Get-CimInstance")
        .expect("缺旧 SCM 快照");
    let backup_helper = script
        .find("Copy-Item -LiteralPath $helperDst -Destination $helperBackup")
        .expect("缺旧 helper 备份");
    let backup_token = script
        .find("Copy-Item -LiteralPath $tokenFile -Destination $tokenBackup")
        .expect("缺旧 token 备份");
    let transaction = script.find("try {\n").expect("缺升级事务边界");
    let destructive = script
        .find("Remove-Item -Force -Path $tokenFile")
        .expect("缺 token 替换腿");
    let first_start = script
        .find("& $sc start PolarisHelper")
        .expect("缺新服务首启");
    let commit = first_start
        + script[first_start..]
            .find("Remove-Item -Force -Path $helperBackup,$tokenBackup")
            .expect("缺成功 commit 清备份");
    let rollback = script.find("} catch {\n").expect("缺失败回滚腿");

    assert!(
        snapshot < backup_helper
            && backup_helper < backup_token
            && backup_token < transaction
            && transaction < destructive,
        "SCM/helper/token 快照必须完整发生在首个破坏性写入之前"
    );
    assert!(
        first_start < commit && commit < rollback,
        "首启成功后才能 commit"
    );
    for needle in [
        "Copy-Item -LiteralPath $helperBackup -Destination $helperDst -Force",
        "Copy-Item -LiteralPath $tokenBackup -Destination $tokenFile -Force",
        "New-Service -Name PolarisHelper -BinaryPathName $oldBinPath",
        "if ($oldWasRunning) { & $sc start PolarisHelper",
        "失败时保留 .rollback",
    ] {
        assert!(script[rollback..].contains(needle), "回滚腿缺：{needle}");
    }
}

/// **P4 · S 受保护内核目录的写侧门**：ImagePath 指受保护路径 + 播种 + `/setowner` 先行 + `Users:(RX)`。
///
/// 这一条是让整批「有牙」的那一条：helper 侧的 chokepoint（install-core 分派 + ACL 自检）在前两片
/// 就建好了，但只要安装脚本还把 `--singbox` 烧成 app 侧用户可写路径，装完之后起核跑的仍是老路径
/// 那一份 —— chokepoint 没人用。
///
/// 每条断言对应一个**静默**失效形态（都不编译错、真机才炸）：
/// - `--singbox` 退回 `params.singbox_path` ⇒ 受保护目录白建，helper exec 用户可写核；
/// - 播种挪到 `New-Service` 之后 ⇒ 服务先起、ImagePath 指空目录 ⇒ TUN 整条不可用（spec §4.4 首行风险）；
/// - `/setowner` 挪到 `/grant:r` 之后 ⇒ 预创建者的 owner 隐含 WRITE_DAC 没被中和，DACL 设了也能改回去；
/// - 漏 `Users:(OI)(CI)(RX)` ⇒ app 侧 `sha256_file(dest)` 读不到、自证无执行权 ⇒ 每次起核白推 80MB；
/// - 路径用 `/` 分隔（`PathBuf::join` 在 Linux 上的形态）⇒ 真机 ImagePath 指向不存在的路径。
#[test]
fn win_install_script_seeds_and_locks_the_protected_core_dir() {
    let paths = InstallPaths::win();
    let p = install_params(
        PathBuf::from("/x"),
        PathBuf::from(r"C:\app\polaris-helper.exe"),
    );
    let script = build_win_install_script(&paths, &p, "WTOKEN");

    // ── ① 受保护路径从 `InstallPaths::win().core_dir` 派生，且是纯反斜杠形态 ───────────────
    let core_dir = paths.core_dir.to_string_lossy().into_owned();
    let core_bin = format!(r"{core_dir}\{WIN_CORE_BIN_NAME}");
    let core_sidecar = format!(r"{core_dir}\{WIN_CORE_SIDECAR_NAME}");
    assert_eq!(
        core_bin, r"C:\ProgramData\Polaris\core\sing-box.exe",
        "受保护核路径的形态改了必须过 review（app 侧 reconcile 与 helper 侧自检都锚在它上）"
    );
    assert!(
        script.contains(&format!("$coreBin = '{core_bin}'")),
        "脚本里的 $coreBin 必须由 InstallPaths::win().core_dir 派生"
    );
    assert!(
        script.contains(&format!(r#"--singbox "{core_bin}" --confdir"#)),
        "服务 ImagePath 的 --singbox 必须指受保护核，而不是 app 侧用户可写核"
    );
    // 取材面自检 + 反向对照：脚本里凡提到受保护核目录的行，一条都不许出现正斜杠。
    // （`PathBuf::join` 在 Linux 上拼 `/`，这种错不编译错、只让真机 ImagePath 指向不存在的路径。）
    let core_lines: Vec<&str> = script
        .lines()
        .filter(|l| l.contains(r"ProgramData\Polaris\core") || l.contains("ProgramData/Polaris"))
        .collect();
    assert!(
        core_lines.len() >= 4,
        "取材面自检失败：提到受保护核目录的行只有 {} 条，下面的全称否定会变成空集恒过",
        core_lines.len()
    );
    for line in &core_lines {
        assert!(
            !line.contains(r"Polaris/"),
            "受保护核路径出现正斜杠（PathBuf::join 的宿主分隔符漏了出来）：{line}"
        );
    }

    // ── ② 播种：条件播、先 .seed.new 再 Move，且**整段在 New-Service 之前** ─────────────────
    // `-PathType Leaf` 是判据的一部分，不是装饰：裸 `Test-Path` 对**目录**也返 True，同账户用户
    // 抢先建一个名叫 sing-box.exe 的目录就能让播种被跳过（ACL 三步照样全过、安装「成功」），
    // 而 helper 之后 CreateProcess 一个目录必败、install-core 的 rename 覆盖目录也必败。
    let seed_guard = script
        .find("if (-not (Test-Path -LiteralPath $coreBin -PathType Leaf))")
        .expect("缺条件播种，或漏了 -PathType Leaf（预建同名目录即可跳过播种）");
    let seed_copy = script
        .find(r#"Copy-Item -LiteralPath $bundledCore -Destination "$coreBin.seed.new" -Force"#)
        .expect("缺核播种");
    let seed_move = script
        .find(r#"Move-Item -LiteralPath "$coreBin.seed.new" -Destination $coreBin -Force"#)
        .expect("缺核播种的原子落位（先 .seed.new 再 Move，杜绝 exec 到写了一半的 exe）");
    let sidecar_guard = script
        .find("if (Test-Path -LiteralPath $bundledSidecar)")
        .expect("缺 cronet 的条件播种（不随包时必须跳过，对齐 linux 的 `[ -f ] &&`）");
    let new_service = script
        .find("New-Service -Name PolarisHelper -BinaryPathName $bp")
        .expect("缺 New-Service");
    assert!(
        seed_guard < seed_copy && seed_copy < seed_move,
        "播种次序错：必须先判缺失、再落 .seed.new、最后 Move 就位"
    );
    assert!(
        seed_move < new_service && sidecar_guard < new_service,
        "播种必须早于 New-Service —— 否则服务起来时 ImagePath 指向空目录，TUN 整条不可用"
    );
    assert!(
        script.contains(&format!("$coreSidecar = '{core_sidecar}'")),
        "cronet 播种目标必须与核同目录"
    );

    // ── ③ ACL 三步的**顺序**：/setowner → /inheritance:r → /grant:r（面 I / 必改2）──────────
    // 只钉「都包含」会被「顺序反了」骗过：/setowner 在 /grant:r 之后跑，等于先设 DACL 再换 owner，
    // 中间那一刻预创建者的 WRITE_DAC 仍在，门就是摆设。
    let core_setowner = script
        .find(r#"& $icacls $coreDir /setowner "*S-1-5-32-544" /T"#)
        .expect("缺受保护核目录 /setowner（/T 连播下的文件一起收）");
    // `/inheritance:r` 与 `/grant:r` 已合成一条命令（N-1a：拆开会留空 DACL 中间态），
    // 故这里只剩两步顺序：先 setowner，再一次性重建 DACL。
    let core_dacl = script
        .find("& $icacls $coreDir /inheritance:r /grant:r")
        .expect("缺受保护核目录的 /inheritance:r + /grant:r（必须同一条命令）");
    let core_grant = core_dacl;
    assert!(
        core_setowner < core_dacl,
        "受保护核目录的 ACL 顺序必须是 /setowner → （/inheritance:r + /grant:r 同命令）"
    );
    assert!(
        seed_move < core_setowner,
        "/setowner /T 必须在播种之后 —— 提权 PowerShell 新建文件的默认属主是创建者账户，\
         不在自检白名单里，先设后播等于把 owner 判定留在放宽态"
    );
    // $support / $tokenFile / $helperDst 三处也必须 setowner 先行（面 I 同理）。
    for (obj, inherit) in [
        (
            r#"& $icacls $support /setowner "*S-1-5-32-544""#,
            "& $icacls $support /inheritance:r /grant:r",
        ),
        (
            r#"& $icacls $helperDst /setowner "*S-1-5-32-544""#,
            "& $icacls $helperDst /inheritance:r /grant:r",
        ),
    ] {
        let a = script.find(obj).unwrap_or_else(|| panic!("缺 {obj}"));
        let b = script
            .find(inherit)
            .unwrap_or_else(|| panic!("缺 {inherit}"));
        assert!(a < b, "/setowner 必须先于 /inheritance:r：{obj}");
    }
    let token_write = script
        .find("Set-Content -Path $tokenFile")
        .expect("缺写 token");
    let token_setowner = script
        .find(r#"& $icacls $tokenFile /setowner "*S-1-5-32-544""#)
        .expect("缺 token /setowner");
    assert!(
        token_write < token_setowner,
        "token 得先写出来才谈得上改属主"
    );

    // ── ④ Users:(RX) 一格都不许少（必改7）──────────────────────────────────────────────
    // 主体钉 `*S-1-5-32-545`（BUILTIN\Users 的 well-known SID）而不是英文名 `Users:` ——
    // 英文名在本地化 Windows 上解析不到，见 `win_install_script_uses_numeric_sids_...`。
    assert!(
        script.contains(
            r#"& $icacls $coreDir /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)(F)" "*S-1-5-32-544:(OI)(CI)(F)" "*S-1-5-32-545:(OI)(CI)(RX)""#
        ),
        "受保护核目录必须显式授 *S-1-5-32-545:(OI)(CI)(RX)：继承父目录只有 SYSTEM/Admin(F)，\
         Users 一条 ACE 都没有 ⇒ app 侧 sha256_file(受保护核) 读不到、内核自证无执行权"
    );
    // 反向：不得给 Users 授写类权（helper 侧 coreacl 判「放宽」→ 拒起核）。
    assert!(
        !script.contains("\"*S-1-5-32-545:(OI)(CI)(F)\"")
            && !script.contains("\"*S-1-5-32-545:(OI)(CI)(M)\""),
        "给 Users 授写类权会被 helper 的 ACL 自检判放宽并拒起核"
    );

    // ── ⑤ 事务边界：受保护核这一段刻意在 try 之外（失败时既有安装完整无损）───────────────
    let transaction = script.find("try {\n").expect("缺升级事务边界");
    assert!(
        core_grant < transaction,
        "建目录/播种/锁 ACL 应落在 try 之外：那几步对既有安装幂等且非破坏，\
         失败时在动旧 token/helper/服务之前就非零退出，比进 try 后靠 catch 收拾更可回退"
    );
}

#[test]
fn uninstall_scripts_remove_service_and_files() {
    let mac = build_mac_uninstall_script(&InstallPaths::mac());
    assert!(mac.contains("launchctl bootout system \"$PLIST\""));
    assert!(mac.contains("rm -rf '/Library/Application Support/Polaris'"));
    let lin = build_linux_uninstall_script(&InstallPaths::linux());
    assert!(lin.contains("systemctl disable --now polaris-helper.service"));
    assert!(lin.contains("rm -rf '/usr/local/lib/polaris' '/var/lib/polaris' '/run/polaris'"));
    let win = build_win_uninstall_script();
    assert!(win.contains("& $sc delete PolarisHelper"));
    assert!(win.contains("& $sc stop PolarisHelper"));
    // 同 install 的病根牙（最强形）：单引号包任何 $ 引用的调用即病；本脚本 EAP=
    // SilentlyContinue，病发时静默什么都不卸——「卸载点了没反应」的隐性形态。
    assert!(
        !win.contains("& '$"),
        "win 卸载脚本出现单引号包 $ 引用的调用——静默不卸的必死形态"
    );
    assert!(win.contains(r"Remove-Item -Recurse -Force -Path 'C:\ProgramData\Polaris'"));
}

// ── install()/uninstall() 端到端（提权接线 + 落盘 + 清理）──
#[test]
fn install_linux_end_to_end_wires_pkexec_and_writes_script() {
    let src = PathBuf::from("/app/polaris-helper");
    let (m, _tok_dir) = install_manager(Platform::Linux, &src);
    let script_dir = tempfile::tempdir().unwrap();
    let params = install_params(script_dir.path().to_path_buf(), src);
    let (exec, calls, content) = capturing((String::new(), 0));
    let outcome = m.install(&params, &exec).unwrap();
    assert_eq!(outcome, EscalationOutcome::Success);
    // 提权走 pkexec，argv[0..2] = pkexec /bin/bash <script>。
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][0], "/usr/bin/pkexec");
    assert_eq!(calls[0][1], "/bin/bash");
    assert!(
        calls[0][2].starts_with(&script_dir.path().to_string_lossy().into_owned()),
        "脚本落 script_dir"
    );
    // executor 读到的脚本内容 = build_linux_install_script（落盘真发生）。
    let written = content.lock().unwrap().clone().expect("脚本应已落盘");
    assert!(written.contains("systemctl enable \"$SERVICE\""));
    assert!(written.contains("systemctl restart \"$SERVICE\""));
    assert!(written.contains("cmp -s \"/proc/$NEW_PID/exe\" \"$DEST\""));
    // 清理：脚本文件已删（finally unlink）。
    assert!(!Path::new(&calls[0][2]).exists(), "脚本执行后须清理");
}

#[test]
fn install_missing_binary_errors() {
    // 源二进制不在 sysops.exists → HelperBinaryMissing（不落脚本、不提权）。
    let dir = tempfile::tempdir().unwrap();
    let m = HelperManager::new(
        Platform::Linux,
        dir.path().join("token"),
        Box::new(MockSysOps::default()),
    );
    let script_dir = tempfile::tempdir().unwrap();
    let params = install_params(
        script_dir.path().to_path_buf(),
        PathBuf::from("/app/absent"),
    );
    let (exec, calls, _) = capturing((String::new(), 0));
    let err = m.install(&params, &exec).unwrap_err();
    assert!(matches!(err, ManagerError::HelperBinaryMissing(_)));
    assert!(calls.lock().unwrap().is_empty(), "缺二进制不应提权");
}

#[test]
fn install_mac_selects_osascript() {
    let src = PathBuf::from("/app/helper");
    let (m, _d) = install_manager(Platform::Mac, &src);
    let script_dir = tempfile::tempdir().unwrap();
    let params = install_params(script_dir.path().to_path_buf(), src);
    let (exec, calls, _) = capturing((String::new(), 0));
    m.install(&params, &exec).unwrap();
    assert_eq!(calls.lock().unwrap()[0][0], "/usr/bin/osascript");
}

#[test]
fn install_win_selects_uac() {
    let src = PathBuf::from(r"C:\app\helper.exe");
    let (m, _d) = install_manager(Platform::Win, &src);
    let script_dir = tempfile::tempdir().unwrap();
    let params = install_params(script_dir.path().to_path_buf(), src);
    let (exec, calls, _) = capturing((String::new(), 0));
    m.install(&params, &exec).unwrap();
    let executable = calls.lock().unwrap()[0][0].clone();
    assert!(
        executable.ends_with(r"\WindowsPowerShell\v1.0\powershell.exe"),
        "Windows 安装提权必须钉住系统 PowerShell: {executable}"
    );
    assert!(
        executable.contains(r":\"),
        "Windows 安装提权不得走 PATH/当前目录解析: {executable}"
    );
}

#[test]
fn install_user_cancel_maps_to_cancelled() {
    // pkexec 126 = 取消 → Cancelled（非 Err，取消是正常流程）。
    let src = PathBuf::from("/app/helper");
    let (m, _d) = install_manager(Platform::Linux, &src);
    let script_dir = tempfile::tempdir().unwrap();
    let params = install_params(script_dir.path().to_path_buf(), src);
    let (exec, _, _) = capturing(("".into(), 126));
    assert_eq!(
        m.install(&params, &exec).unwrap(),
        EscalationOutcome::Cancelled
    );
}

#[test]
fn uninstall_clears_token_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("token");
    crate::token::write_token_content(&token_path, "TOK").unwrap();
    assert!(token_path.exists());
    let m = HelperManager::new(
        Platform::Linux,
        token_path.clone(),
        Box::new(MockSysOps::default()),
    );
    let script_dir = tempfile::tempdir().unwrap();
    let (exec, calls, _) = capturing((String::new(), 0));
    let outcome = m.uninstall(script_dir.path(), &exec).unwrap();
    assert_eq!(outcome, EscalationOutcome::Success);
    assert!(!token_path.exists(), "卸载成功须清 app 侧 token");
    assert_eq!(calls.lock().unwrap()[0][0], "/usr/bin/pkexec");
}

#[test]
fn uninstall_cancel_keeps_token() {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("token");
    crate::token::write_token_content(&token_path, "TOK").unwrap();
    let m = HelperManager::new(
        Platform::Linux,
        token_path.clone(),
        Box::new(MockSysOps::default()),
    );
    let script_dir = tempfile::tempdir().unwrap();
    let (exec, _, _) = capturing(("".into(), 126));
    assert_eq!(
        m.uninstall(script_dir.path(), &exec).unwrap(),
        EscalationOutcome::Cancelled
    );
    assert!(token_path.exists(), "取消不应清 token（helper 仍在）");
}

#[test]
fn pipe_self_uninstall_only_for_win() {
    // 非 win 恒 false（无 uninstall 命令），不触发 client。
    let m = manager(Platform::Linux, MockSysOps::default());
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Linux, "");
    assert!(!m.pipe_self_uninstall(&client));
}

#[test]
fn pipe_self_uninstall_win_ok_response() {
    let m = manager(Platform::Win, MockSysOps::default());
    // helper 回 OK（win uninstall 命令，W11）→ true。
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::with_response(
            b"OK uninstalling\n".to_vec(),
        )])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Win, "TOK");
    assert!(m.pipe_self_uninstall(&client));
}

#[test]
fn wait_until_ready_returns_ready_when_proto_ok() {
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.clone().expect("mac/linux 有描述符文件"));
    let (m, _dir) = manager_with_token(Platform::Mac, sysops);
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![MockStream::with_response(
            current_pong_wire(),
        )])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Mac, "TOK");
    let status = m.wait_until_ready(&client, 1, Duration::from_millis(1));
    assert!(status.ready);
    assert!(!status.upgradeable);
}

#[test]
fn wait_until_ready_does_not_accept_same_proto_old_build() {
    let paths = InstallPaths::mac();
    let mut sysops = MockSysOps::default();
    sysops.exists_paths.insert(paths.binary.clone());
    sysops
        .exists_paths
        .insert(paths.descriptor.clone().expect("mac/linux 有描述符文件"));
    let (m, _dir) = manager_with_token(Platform::Mac, sysops);
    let old = format!(
        "OK pong uid=0 v{} build=older-package\n",
        polaris_helper_proto::proto_version::CURRENT
    )
    .into_bytes();
    let connector = MockConnector {
        streams: Arc::new(Mutex::new(vec![
            MockStream::with_response(old),
            MockStream::with_response(current_pong_wire()),
        ])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Mac, "TOK");
    let status = m.wait_until_ready(&client, 2, Duration::from_millis(1));
    assert!(status.ready);
    assert!(
        !status.upgradeable,
        "必须等到当前随包 build，旧 build 的 ready 不算安装完成"
    );
}

/// Windows 提权脚本必须带 UTF-8 BOM —— 见 [`script_bytes`] 的文档。
///
/// 判据落在**字节**上（不是「代码里有没有那个常量」）：BOM 掉了、或被误加到 unix 腿上，都必红。
#[test]
fn windows_script_carries_a_utf8_bom_and_unix_never_does() {
    const SRC: &str = "$ErrorActionPreference = 'Stop'\n# 复制 helper.exe 失败\n";
    let win = script_bytes(SRC, true);
    assert_eq!(&win[..3], &[0xEF, 0xBB, 0xBF], "Windows 腿丢了 BOM");
    assert_eq!(
        &win[3..],
        SRC.as_bytes(),
        "BOM 之后必须逐字节等于原文（别顺手改了编码）"
    );

    let nix = script_bytes(SRC, false);
    assert_eq!(
        nix,
        SRC.as_bytes(),
        "unix 腿不得有 BOM（会顶掉 shebang 的首字节）"
    );

    // 正向对照：两条腿确实不同形，否则上面两条可能同时被一个「恒不加 BOM」的实现满足。
    assert_ne!(win, nix);
}

/// 两个 `write_secure_script` 分支各自传对了 `windows` 实参 —— 纯函数测不到接线。
#[test]
fn both_write_legs_pass_the_right_platform_flag() {
    let src = polaris_source_probe::crate_source!("manager.rs");
    // 切「锚点之后的第一个顶层 `#[cfg(test)]`」。**不能切第一个** —— 本文件第一个 `#[cfg(test)]`
    // 在 :29（远早于 `write_secure_script`），切它会把待验函数整个丢掉，门以 panic 收场。
    let at = src
        .find("fn write_secure_script(")
        .expect("write_secure_script 消失，门失去判据");
    let end = src[at..]
        .find("\n#[cfg(test)]\n")
        .map_or(src.len(), |i| at + i);
    let body = &src[at..end];
    // 切点自检：判据区域里若混进本测试自己，下面三条会被自己写的字面量喂饱。
    assert!(
        !body.contains("both_write_legs_pass_the_right_platform_flag"),
        "切点错了：判据区域包含本测试自身"
    );
    assert!(
        body.contains("script_bytes(content, false)"),
        "unix 腿没走 script_bytes(.., false)"
    );
    assert!(
        body.contains("script_bytes(content, true)"),
        "windows 腿没走 script_bytes(.., true) —— BOM 不会被写出去"
    );
    assert!(
        !body.contains("f.write_all(content.as_bytes())"),
        "还有分支在绕过 script_bytes 直接写原文"
    );
}

/// 🔴 **S-4 跨 crate 等值门：Windows 受保护核目录名只有一个值。**
///
/// `WIN_CORE_DIR_NAME` 与 helper 侧 `derived_core_dir()` 里的 `join("core")` 是两份独立字面量：
/// daemon 只从命令行拿 `--support`，核目录由它**自己派生**（刻意不从 wire 取 —— ImagePath 提权后
/// 可改，少一个被读的核路径参数就少一条注入向量）。本 crate 又看不见 `polaris-helper`
/// （不是它的依赖），故收敛只能靠门。
///
/// 漂移后果**全程静默**：ImagePath、seed、app 侧 dest 都跟本常量走，helper 的 install-core 却写进
/// 另一个目录 ⇒ app 每次起核都判「受保护核不存在」⇒ 每次白推 80MB 且日志恒「已提升」，而 helper
/// exec 的仍是安装期播下的那份 ⇒ **换核永远不生效**，且盘面上看不出任何异常。
///
/// 取材面取**整个 `crates/helper/src`** 而不是写死那一个文件：`derived_core_dir` 搬到别的模块时，
/// 写死路径的门会读到一个仍然存在、但已不含该函数的文件 —— 静默失去判据。
#[test]
fn win_core_dir_name_agrees_with_the_helper_side_derivation() {
    let files = polaris_source_probe::repo_dir_files_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/helper/src",
        "rs",
    );
    // 注释必须剥：`derived_core_dir` 的文档注释里就写着「两处各写一遍 `"core"`」，不剥的话
    // 判据会拿注释当证据（那行改成别的值也照样绿）。字符串字面量**不能**剥 —— 针本身就是它。
    let hits: Vec<(&str, String)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), polaris_source_probe::mask_comments(text)))
        .filter(|(_, code)| code.contains("fn derived_core_dir("))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "helper 侧 `derived_core_dir` 的定义点不是恰好一个（实得 {:?}）—— \
         0 个 = 函数改名/删了，门失去判据；>1 个 = 又多了一份派生，下面只验了第一份",
        hits.iter().map(|(p, _)| *p).collect::<Vec<_>>()
    );
    let (path, code) = &hits[0];
    let at = code.find("fn derived_core_dir(").expect("上面刚筛过");
    // 切到方法体的收尾 `}`（本仓 impl 内方法恒 4 空格缩进）。切点自检在下面三条。
    let end = code[at..]
        .find("\n    }")
        .map_or(code.len(), |i| at + i + "\n    }".len());
    let body = &code[at..end];
    assert!(
        body.contains("support_dir"),
        "切点自检失败：切出来的片段里没有 support_dir，切的不是 derived_core_dir（{path}）"
    );
    assert!(
        body.len() < 400,
        "切点自检失败：切出来 {} 字节，远超单表达式方法体 —— 收尾锚点没命中，\
         片段里混进了后续方法（{path}）",
        body.len()
    );
    assert_eq!(
        body.matches(".join(").count(),
        1,
        "derived_core_dir 里出现了不止一次 join —— 下面只会验到第一处，其余悄悄逃出覆盖面（{path}）"
    );

    // 取材器自检（正向对照）：同一套抠法喂一个哨兵串必须抠出哨兵值 —— 证明它在**真的读**那份
    // 源码，而不是在返回一个常量。这条是 helper 侧变异的替代品：`crates/helper` 由另一条线持有，
    // 本轮不得改动，故「改 helper 侧 → 本门转红」这个方向没法用实跑变异证。
    assert_eq!(
        join_literal(r#"fn f() { Path::new(&self.support_dir).join("polaris-sentinel") }"#),
        Some("polaris-sentinel"),
        "取材器自检失败：join 字面量抠不出来，本门的判据是假的"
    );

    // 判据：把 helper 侧真正 join 的那个字面量抠出来，与本 crate 的常量逐字相等。
    let joined = join_literal(body)
        .expect("derived_core_dir 不再是 `join(\"<字面量>\")` 形态 —— 判据失效，必须重写门");
    assert_eq!(
        joined, WIN_CORE_DIR_NAME,
        "helper 侧派生的核目录名与安装脚本/app 侧的 WIN_CORE_DIR_NAME 分叉（{path}）：\
         helper 会把 install-core 写进 `<support>\\{joined}`，而 ImagePath/seed/app 侧 dest 全指 \
         `<support>\\{WIN_CORE_DIR_NAME}` ⇒ 换核永久不生效且无任何报错"
    );
    // 正面断言：上面是「A == B」，两侧同时被改成同一个错值仍然全绿。故再钉一次绝对值 +
    // 与安装路径的实际拼接结果（脚本烧进 ImagePath 的就是它）。
    assert_eq!(WIN_CORE_DIR_NAME, "core");
    assert_eq!(
        InstallPaths::win().core_dir,
        PathBuf::from(format!(r"{WIN_SUPPORT_DIR}\{WIN_CORE_DIR_NAME}")),
        "安装脚本派生受保护核路径时没走 WIN_CORE_DIR_NAME"
    );
}

/// 抠出 `…join("<字面量>")` 里的那个字面量；形态不符返 `None`（不返空串 —— 空串上的等值断言
/// 会把「抠不出来」伪装成一次普通的不相等，错误信息指向错误的方向）。
fn join_literal(src: &str) -> Option<&str> {
    src.split(".join(\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
}

/// cronet 的播种源必须是**随包核同目录**下的绝对路径。
///
/// 回归来由（2026-09-24 207 真机）：原实现用 `Path::parent()` 取目录，而单测跑在 Linux 上，
/// `Path::new(r"C:\…\sing-box.exe").parent()` 返回 `Some("")` ⇒ 渲染成裸名 `libcronet.dll`。
/// 生产（Windows）不受影响，但此前没有断言看这个值 —— 测试看到的是错值却照样绿，
/// 将来真改坏了也照不出来。本条让 Linux 上渲染出的就是生产真值。
#[test]
fn win_install_script_seeds_cronet_from_the_bundled_core_directory() {
    let p = InstallParams {
        src_binary: PathBuf::from(
            r"C:\Users\u\AppData\Local\Polaris\_up_\resources\win\polaris-helper.exe",
        ),
        bundled_core: PathBuf::from(
            r"C:\Users\u\AppData\Roaming\com.polaris.app\polaris\core_update\sing-box.exe",
        ),
        conf_dir: PathBuf::from(r"C:\Users\u\AppData\Roaming\com.polaris.app\polaris"),
        uid: 0,
        script_dir: PathBuf::from(r"C:\Users\u\AppData\Roaming\com.polaris.app\polaris"),
    };
    let script =
        build_win_install_script(&InstallPaths::win(), &p, "0123456789abcdef0123456789abcdef");
    let want = r"$bundledSidecar = 'C:\Users\u\AppData\Roaming\com.polaris.app\polaris\core_update\libcronet.dll'";
    assert!(
        script.contains(want),
        "cronet 播种源不是随包核同目录下的绝对路径：\n{}",
        script
            .lines()
            .find(|l| l.starts_with("$bundledSidecar"))
            .unwrap_or("<缺这一行>")
    );
    // 反向：绝不能退化成相对路径的裸名（`Test-Path` 相对当前目录 ⇒ 静默不播种）。
    assert!(!script.contains("$bundledSidecar = 'libcronet.dll'"));
}
