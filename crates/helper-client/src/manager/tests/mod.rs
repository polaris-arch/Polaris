mod bootstrap_identity;

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
        (Platform::Linux, LINUX_SERVICE_NAME),
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
fn same_protocol_without_build_identity_does_not_force_a_helper_replacement() {
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
    assert!(
        !status.upgradeable,
        "build identity is diagnostic, not a version criterion"
    );
    assert!(!status.needs_repair, "升级态不是损坏态");
    assert_eq!(status.build_id, None);
}

#[test]
fn same_protocol_with_different_build_identity_does_not_force_a_helper_replacement() {
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
    assert!(!status.upgradeable);
    assert_eq!(status.build_id.as_deref(), Some("older-package"));
    assert!(!status.needs_repair);
}

#[test]
fn old_v2_helper_is_not_runtime_ready_and_offers_real_upgrade() {
    for version in [0, 1, 2] {
        let paths = InstallPaths::mac();
        let mut sysops = MockSysOps::default();
        sysops.exists_paths.insert(paths.binary.clone());
        sysops
            .exists_paths
            .insert(paths.descriptor.expect("mac descriptor"));
        let (manager, _dir) = manager_with_token(Platform::Mac, sysops);
        let connector = MockConnector {
            streams: Arc::new(Mutex::new(vec![MockStream::with_response(
                format!("OK pong uid=0 v{version}\n").into_bytes(),
            )])),
        };
        let status = manager.compute_status_with_client(&HelperClient::new(
            Box::new(connector),
            Platform::Mac,
            "TOK",
        ));
        assert!(!status.ready);
        assert!(status.upgradeable);
        assert!(status.needs_repair);
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

// Passive status must never race an administrator transaction by restarting
// a stopped service. All desktop platforms return repair-needed instead.
#[test]
fn passive_status_never_starts_installed_stopped_or_unreachable_services() {
    for platform in [Platform::Mac, Platform::Linux, Platform::Win] {
        for loaded in [false, true] {
            let paths = InstallPaths::for_platform(platform);
            let start_calls = Arc::new(Mutex::new(Vec::new()));
            let mut sysops = MockSysOps::default();
            sysops.exists_paths.insert(paths.binary.clone());
            if let Some(descriptor) = paths.descriptor {
                sysops.exists_paths.insert(descriptor);
            }
            sysops
                .registered_services
                .insert(paths.service_label.to_owned());
            sysops.loaded = loaded;
            sysops.start_calls = start_calls.clone();
            let (manager, _dir) = manager_with_token(platform, sysops);
            for _ in 0..3 {
                let connector = MockConnector {
                    streams: Arc::new(Mutex::new(vec![MockStream::broken(
                        std::io::ErrorKind::ConnectionAborted,
                    )])),
                };
                let client = HelperClient::new(Box::new(connector), platform, "TOK");
                let status = manager.compute_status_with_client(&client);
                assert!(status.installed);
                assert!(status.needs_repair);
                assert!(!status.ready);
            }
            assert!(
                start_calls.lock().unwrap().is_empty(),
                "passive status started {platform:?}"
            );
        }
    }
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
    let raw = src.to_string_lossy();
    let (parent, name) = raw.rsplit_once(['/', '\\']).expect("source parent");
    let windows = name.ends_with(".exe");
    let separator = if windows { "\\" } else { "/" };
    let manifest_path = PathBuf::from(format!("{parent}{separator}core-manifest.json"));
    let bundled_core = PathBuf::from(format!(
        "{parent}{separator}{}",
        if windows { "sing-box.exe" } else { "sing-box" }
    ));
    InstallParams {
        linux_legacy_original_proxy: None,
        manifest_path,
        manifest_sha256: "b".repeat(64),
        core_id: polaris_helper_proto::core_manifest::CoreId::parse(&"c".repeat(64)).unwrap(),
        helper_sha256: "a".repeat(64),
        cleaner_sha256: windows.then(|| "d".repeat(64)),
        helper_version: "1.0.0".into(),
        user_identity: if windows {
            polaris_helper_proto::core_protocol::CallerIdentity::windows("S-1-5-21-1-2-3-1000")
                .unwrap()
        } else {
            polaris_helper_proto::core_protocol::CallerIdentity::Unix(1000)
        },
        src_binary: src,
        bundled_core,
        conf_dir: script_dir.join("user-config"),
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

// ── Immutable installation and machine-service transaction contracts ──
fn unix_scripts() -> [String; 2] {
    let p = install_params(PathBuf::from("/x"), PathBuf::from("/app/helper"));
    [
        build_mac_install_script(&InstallPaths::mac(), &p, &"a".repeat(64)),
        build_linux_install_script(&InstallPaths::linux(), &p, &"a".repeat(64)),
    ]
}

#[test]
fn installation_seals_bytes_before_executing_the_administrator_utility() {
    for script in unix_scripts() {
        let helper_hash = script.find("[ \"$(hash_file \"$TXDIR/helper\")\"").unwrap();
        let manifest_hash = script
            .find("[ \"$(hash_file \"$TXDIR/core-manifest.json\")\"")
            .unwrap();
        // The sealed utility router keeps macOS bootstrap custody separate from
        // the staged Linux helper. Neither path may bypass byte verification.
        let admin = script
            .split("admin() {\n")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert_eq!(
            admin.trim(),
            r#"if [ -n "$BOOT" ]; then "$BOOT/helper" "$@"; else "$TXDIR/helper" "$@"; fi"#
        );
        let acquire = script.find("admin lease-acquire --lock").unwrap();
        let install = script.find("admin core-install --root").unwrap();
        assert!(helper_hash < acquire && manifest_hash < acquire && acquire < install);
        if script.contains("BOOT=$(") {
            let bootstrap_hash = script.find("/dev/fd/8 | /usr/bin/awk").unwrap();
            let bootstrap_identity = script.find("\"$BOOT_FD_ID\" = \"$BOOT_FILE_ID\"").unwrap();
            let validate = script
                .find("\"$BOOT/helper\" validate-install-path")
                .unwrap();
            assert!(bootstrap_hash < bootstrap_identity && bootstrap_identity < validate);
        }
        assert!(script.contains("--source-owner 'u1000'"));
        assert!(script.contains("chmod 0700 \"$TXDIR\""));
        assert!(script.contains("--manifest \"$TXDIR/core-manifest.json\""));
        assert!(!script.contains(".seed.new"));
        assert!(!script.contains("codesign --force"));
        // No installer payload discovery. The later Linux closure proof has a
        // separate, bounded enumeration of the exact system cgroup parent.
        assert!(!script[..install].contains("find "));
    }
}

#[test]
fn unix_upgrade_admission_precedes_every_service_replacement() {
    for script in unix_scripts() {
        let arm = script.find("trap rollback EXIT").unwrap();
        let admit = script[arm..].find("prepare-upgrade --support").unwrap() + arm;
        let mutate = script[admit..].find("MUTATED=1").unwrap() + admit;
        let stop = script[mutate..].find("then stop_service; fi").unwrap() + mutate;
        let replace = script[stop..]
            .find("mv -f \"$DEST.new.$$\" \"$DEST\"")
            .unwrap()
            + stop;
        assert!(arm < admit && admit < mutate && mutate < stop && stop < replace);
        assert!(script[admit..mutate].contains("exit 75"));
        assert!(script.contains("cancel-upgrade --support"));
        assert!(script.contains("echo polaris-helper-core-only-ok\nexit 0"));
    }
}

#[test]
fn unix_rollback_restores_helper_descriptor_receipt_and_keeps_failed_recovery() {
    for script in unix_scripts() {
        for object in ["helper.rollback", "descriptor.rollback", "record.rollback"] {
            assert!(
                script.matches(object).count() >= 2,
                "missing snapshot/restore for {object}"
            );
        }
        assert!(script.contains("restore_service || OK=0"));
        assert!(script.contains("if [ \"$OK\" -eq 1 ]"));
        assert!(script.contains("recovery transaction retained: $TXDIR"));
        assert!(script.contains("exec 9>>\"$LOCK\""));
        assert!(script.contains("lease-acquire --lock \"$LOCK\" --fd 9"));
        assert!(script.contains("helper-admission.XXXXXX"));
        assert!(script.contains("helper-upgrade.${TXDIR##*.}"));
        assert!(script.contains("HELPER_INSTALL_TRANSACTION_BUSY"));
    }
}

#[test]
fn machine_service_never_contains_an_app_profile_or_a_flat_core() {
    let paths = InstallPaths::mac();
    let plist = render_mac_plist(
        &paths.binary.to_string_lossy(),
        &paths.core_dir.to_string_lossy(),
        MAC_SUPPORT_DIR,
        false,
    );
    assert!(plist.contains("<key>KeepAlive</key><true/>"));
    assert!(plist.contains("<key>RunAtLoad</key><true/>"));
    assert!(plist.contains("<string>--support</string>"));
    assert!(plist.contains("<string>--coredir</string>"));
    assert!(!plist.contains("--confdir"));
    assert!(!plist.contains("--singbox"));
    let win = build_win_install_script(
        &InstallPaths::win(),
        &install_params(
            PathBuf::from("/x"),
            PathBuf::from(r"C:\App\polaris-helper.exe"),
        ),
        &"a".repeat(64),
    );
    assert!(!win.contains("--confdir "));
    assert!(!win.contains("--singbox"));
    assert!(win.contains(r#"$binPath = '"C:\ProgramData\Polaris\polaris-helper.exe" --support "C:\ProgramData\Polaris"'"#));
}

#[test]
fn linux_auth_merge_retains_other_users_and_matches_private_reader_contract() {
    let script = unix_scripts().into_iter().nth(1).unwrap();
    assert!(
        script.contains("grep -qxF '1000' \"$AUTHNEW\" || printf '%s\\n' '1000' >> \"$AUTHNEW\"")
    );
    assert!(script.contains("chmod 0600 \"$AUTHNEW\""));
    assert!(script.contains("invalid-authfile-owner"));
    assert!(script.contains("'helper-policy retain') KEEP_HELPER=1"));
    assert!(script.contains("retain-probe --support \"$SUPPORT\" --id"));
    let admitted = script.rfind("trap rollback EXIT\n").unwrap();
    let stopped = admitted + script[admitted..]
        .find("if [ \"$WAS_ACTIVE\" -eq 1 ] || [ \"$REQUIRE_CLOSURE\" -eq 1 ]; then stop_service; fi")
        .unwrap();
    assert!(script.contains("REQUIRE_CLOSURE=1"));
    let grant = script.rfind("\nmerge_authorization\n").unwrap();
    let start = script.rfind("systemctl restart \"$SERVICE\"").unwrap();
    assert!(
        stopped < grant && grant < start,
        "new authorization cannot reach a running legacy daemon"
    );
    let restore_auth = script.find("restore_authorization || OK=0").unwrap();
    let restore_old = script.find("restore_service || OK=0").unwrap();
    assert!(
        restore_auth < restore_old,
        "rollback restores authorization before restarting the old service"
    );
    assert!(script.contains("auth.rollback"));
}

fn windows_script() -> String {
    build_win_install_script(
        &InstallPaths::win(),
        &install_params(
            PathBuf::from("/x"),
            PathBuf::from(r"C:\App\polaris-helper.exe"),
        ),
        &"a".repeat(64),
    )
}

#[test]
fn windows_seals_helper_manifest_and_cleaner_before_any_service_mutation() {
    fn scoped_mutation_precedes_stop(script: &str) -> bool {
        let Some((_, replacement)) = script.split_once("function Begin-ServiceReplacement {\n")
        else {
            return false;
        };
        let Some((body, _)) = replacement.split_once("\n}\n") else {
            return false;
        };
        let (Some(revalidate), Some(mutate), Some(stop)) = (
            body.find("else { Assert-CurrentProductService }"),
            body.find("\n  $script:mutated = $true\n"),
            body.find("\n  Stop-ServiceChecked"),
        ) else {
            return false;
        };
        revalidate < mutate && mutate < stop && !body.contains("\n  $mutated = $true\n")
    }

    let script = windows_script();
    let helper = script.find("Check-Hash $utility '").unwrap();
    let manifest = script.find("Check-Hash $manifest '").unwrap();
    let cleaner = script
        .find("Check-Hash $cleaner $selection.core.cleanerSha256")
        .unwrap();
    let execute = script.find("Invoke-Utility @('core-install'").unwrap();
    let admit = script.find("Invoke-Utility @('prepare-upgrade'").unwrap();
    let mutate = script.find("\n  Begin-ServiceReplacement\n").unwrap();
    let register = script.find("Invoke-Utility @('register-user'").unwrap();
    assert!(helper < execute && manifest < execute && cleaner < execute);
    assert!(execute < admit && admit < mutate && mutate < register);
    assert!(scoped_mutation_precedes_stop(&script));
    assert!(!scoped_mutation_precedes_stop(&script.replacen(
        "$script:mutated = $true",
        "$mutated = $true",
        1,
    )));
    assert!(!scoped_mutation_precedes_stop(&script.replacen(
        "$script:mutated = $true\n  Stop-ServiceChecked",
        "Stop-ServiceChecked\n  $script:mutated = $true",
        1,
    )));
    assert!(script.contains("    if ($mutated) {\n      Stop-CandidateServiceForRollback"));
    assert!(script.contains("$selection.id -cne '"));
    assert!(script.contains("--source-owner', $identity"));
    assert!(!script.contains(".seed.new"));
    assert!(!script.contains("$support\\helper.token"));
    assert!(script.contains("Secure-Directory (Join-Path $users $identity) $false"));
    assert!(script.contains("[IO.FileShare]::None"));
}

#[test]
fn windows_replaces_the_entire_dacl_with_numeric_privileged_owners() {
    let script = windows_script();
    assert!(script.contains("/setowner '*S-1-5-32-544'"));
    assert!(script.contains("Security.AccessControl.DirectorySecurity"));
    assert!(script.contains("Security.AccessControl.FileSecurity"));
    assert_eq!(
        script
            .matches("$acl.SetAccessRuleProtection($true, $false)")
            .count(),
        2
    );
    assert_eq!(
        script
            .matches("Set-Acl -LiteralPath $path -AclObject $acl")
            .count(),
        2
    );
    assert!(script.contains("@('S-1-5-18', 'S-1-5-32-544')"));
    assert!(script.contains("SecurityIdentifier]::new('S-1-5-32-545'), [Security.AccessControl.FileSystemRights]::ReadAndExecute"));
    assert!(!script.contains("Administrators:"));
    assert!(!script.contains("Users:(F)"));
    assert!(script.contains("[IO.FileAttributes]::ReparsePoint"));
}

#[test]
fn windows_checks_native_exits_and_preserves_existing_service_identity() {
    let script = windows_script();
    for guard in [
        "SCM stop",
        "SCM delete",
        "SCM start permission",
        "SCM recovery",
        "SCM start",
        "SCM rollback start",
    ] {
        assert!(script.contains(&format!("Assert-NativeExit '{guard}'")));
    }
    assert!(script.contains("actions= restart/5000/restart/5000/restart/5000"));
    assert!(script.contains("(A;;RP;;;IU)"));
    assert!(script.contains("$policy.Command -cne $binPath"));
    assert!(!script.contains("-BinaryPathName $old.PathName"));
    assert!(script.contains("Restore-PreviousService"));
    assert!(script.contains("$snapshot.Backup -Destination $snapshot.Path"));
    assert!(script.contains("$keepRecovery = $true"));
    assert!(script.contains("finally { if ($null -ne $lock) { $lock.Dispose() } }"));
    assert!(script.contains("WaitForStatus([ServiceProcess.ServiceControllerStatus]::Running"));
}

#[test]
fn invalid_installation_identity_is_rejected_before_escalation() {
    let stage = tempfile::tempdir().unwrap();
    let src = stage.path().join("helper");
    let (manager, _token_dir) = install_manager(Platform::Linux, &src);
    let original = install_params(stage.path().into(), src);
    validate_install_params(Platform::Linux, &original).unwrap();
    let mut candidates = Vec::new();
    let mut missing_hash = original.clone();
    missing_hash.helper_sha256.clear();
    candidates.push(missing_hash);
    let mut wrong_uid = original.clone();
    wrong_uid.uid += 1;
    candidates.push(wrong_uid);
    let mut injection = original.clone();
    injection.conf_dir = PathBuf::from("/tmp/profile\n--bad");
    candidates.push(injection);
    for params in candidates {
        let (executor, calls, _) = capturing((String::new(), 0));
        assert!(matches!(
            manager.install(&params, &executor),
            Err(ManagerError::InvalidInstallation(_))
        ));
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[test]
fn injected_installation_platforms_use_valid_host_absolute_fixtures_and_reject_relative_inputs() {
    for platform in [Platform::Linux, Platform::Mac, Platform::Win] {
        let stage = tempfile::tempdir().unwrap();
        let name = if platform == Platform::Win {
            "helper.exe"
        } else {
            "helper"
        };
        let original = install_params(stage.path().into(), stage.path().join(name));
        validate_install_params(platform, &original).unwrap();
        let mut relative = original.clone();
        relative.conf_dir = PathBuf::from("relative-profile");
        assert!(matches!(
            validate_install_params(platform, &relative),
            Err(ManagerError::InvalidInstallation(_))
        ));
        let mut injected = original;
        injected.script_dir = stage.path().join("profile\n--bad");
        assert!(matches!(
            validate_install_params(platform, &injected),
            Err(ManagerError::InvalidInstallation(_))
        ));
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
    for script in unix_scripts() {
        assert_shell_syntax("/bin/sh", &script);
    }
}

#[test]
fn uninstall_scripts_preserve_stable_lease_and_platform_cleanup_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let utility = maintenance_fixture(dir.path());
    let mac = uninstall_unix::build_mac_uninstall_script(&InstallPaths::mac(), &utility);
    assert!(mac.contains("admin migration-stop --journal"));
    assert!(mac.contains("admin migration-recheck --journal"));
    assert!(mac.contains("stopped-retired-data-preserved"));
    assert!(!mac.contains("rm -rf \"$SUPPORT\""));
    let lin = uninstall_unix::build_linux_uninstall_script(&InstallPaths::linux(), &utility);
    assert!(lin.contains("systemctl --system disable --now \"$SERVICE\""));
    assert!(lin.contains("rm -rf \"$INSTALLDIR\" \"$RUNTIMEDIR\""));
    for script in [mac, lin] {
        assert!(script.contains("admin lease-acquire --lock \"$LOCK\" --fd 9"));
        assert!(!script.contains("rm -f \"$LOCK\""));
        assert!(!script.contains("rm -rf \"$SUPPORT\""));
    }
}

// ── install()/uninstall() 端到端（提权接线 + 落盘 + 清理）──
#[test]
fn install_linux_end_to_end_wires_pkexec_and_writes_script() {
    let source_stage = tempfile::tempdir().unwrap();
    let src = source_stage.path().join("polaris-helper");
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
    assert!(written.contains("cmp -s \"/proc/$PID/exe\" \"$DEST\""));
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
    let params = install_params(script_dir.path().to_path_buf(), dir.path().join("absent"));
    let (exec, calls, _) = capturing((String::new(), 0));
    let err = m.install(&params, &exec).unwrap_err();
    assert!(matches!(err, ManagerError::HelperBinaryMissing(_)));
    assert!(calls.lock().unwrap().is_empty(), "缺二进制不应提权");
}

#[test]
fn install_mac_selects_osascript() {
    let source_stage = tempfile::tempdir().unwrap();
    let src = source_stage.path().join("helper");
    let (m, _d) = install_manager(Platform::Mac, &src);
    let script_dir = tempfile::tempdir().unwrap();
    let params = install_params(script_dir.path().to_path_buf(), src);
    let (exec, calls, _) = capturing((String::new(), 0));
    m.install(&params, &exec).unwrap();
    assert_eq!(calls.lock().unwrap()[0][0], "/usr/bin/osascript");
}

#[test]
fn install_win_selects_native_pinned_maintenance() {
    let source = polaris_source_probe::crate_source!("manager.rs");
    let native = source
        .split_once("if self.platform == Platform::Win {")
        .unwrap()
        .1;
    assert!(native.contains("polaris_windows_cleaner::launch_install(request)"));
    assert!(native.contains("cleaner_sha256: params.cleaner_sha256"));
    assert!(native.contains("InstallOutcome::Cancelled"));
    assert!(native.contains("InstallOutcome::Completed(code)"));
}

#[test]
fn install_user_cancel_maps_to_cancelled() {
    // pkexec 126 = 取消 → Cancelled（非 Err，取消是正常流程）。
    let source_stage = tempfile::tempdir().unwrap();
    let src = source_stage.path().join("helper");
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
    let utility = maintenance_fixture(script_dir.path());
    let outcome = m
        .uninstall_with_maintenance(script_dir.path(), Some(&utility), &exec)
        .unwrap();
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
    let utility = maintenance_fixture(script_dir.path());
    assert_eq!(
        m.uninstall_with_maintenance(script_dir.path(), Some(&utility), &exec)
            .unwrap(),
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
fn wait_until_ready_accepts_compatible_proto_with_a_different_build() {
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
        streams: Arc::new(Mutex::new(vec![MockStream::with_response(old)])),
    };
    let client = HelperClient::new(Box::new(connector), Platform::Mac, "TOK");
    let status = m.wait_until_ready(&client, 1, Duration::from_millis(1));
    assert!(status.ready);
    assert!(
        !status.upgradeable,
        "兼容协议的 Helper 不因诊断 build 不同而被替换"
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

/// The immutable utility receives the exact stage directory containing both files.
#[test]
fn windows_payload_directory_is_absolute_even_when_rendered_on_unix() {
    let p = install_params(
        PathBuf::from("/x"),
        PathBuf::from(r"C:\Users\u\Polaris\stage\polaris-helper.exe"),
    );
    let script = build_win_install_script(&InstallPaths::win(), &p, &"a".repeat(64));
    assert!(script.contains(r"--source', 'C:\Users\u\Polaris\stage'"));
    assert!(
        script.contains(r"Copy-Item -LiteralPath 'C:\Users\u\Polaris\stage\polaris-cleaner.exe'")
    );
    assert!(script.contains("Check-Hash $cleaner $selection.core.cleanerSha256"));
    assert!(script.contains("foreach ($path in $replaceObjects)"));
    assert!(script.contains("$snapshot.Backup -Destination $snapshot.Path"));
}

#[test]
fn retained_binary_requires_live_content_receipt_or_repair_without_downgrade() {
    for script in unix_scripts() {
        let policy = script
            .find("'helper-policy retain') KEEP_HELPER=1")
            .unwrap();
        let probe = script.find("retain-probe --support").unwrap();
        let admit = script.find("trap rollback EXIT").unwrap();
        assert!(policy < probe && probe < admit);
        assert!(script.contains("if [ \"$KEEP_HELPER\" -eq 0 ]; then"));
    }
    let script = windows_script();
    assert!(script.contains("$keepHelper = $true"));
    assert!(script.contains("'retain-probe' '--support' $support '--id'"));
    assert!(script.contains("$replaceObjects = if ($keepHelper) { @() }"));
    assert!(script.contains("if (-not $keepHelper) {"));
    assert!(script.contains("New-Service -Name $service"));
}

#[test]
fn unix_install_retains_recovery_and_never_guesses_dead_directory_owner() {
    for script in unix_scripts() {
        let acquire = script.find("lease-acquire --lock").unwrap();
        let install = script.find("core-install --root").unwrap();
        assert!(acquire < install);
        assert!(script.contains("transaction-phase"));
        assert!(script.contains("HELPER_INSTALL_RECOVERY_REQUIRED"));
        assert!(script.contains("LEGACY_HELPER_INSTALL_RECOVERY_REQUIRED"));
        assert!(script.contains("exec 9>>"));
        assert!(!script.contains("rmdir \"$LOCK\""));
    }
}

#[test]
fn unix_install_publishes_initialized_journal_only_after_admission_and_cleans_before_unlock() {
    for script in unix_scripts() {
        let stage = script.find("helper-admission.XXXXXX").unwrap();
        let acquire = script.find("lease-acquire --lock").unwrap();
        let kind = script.find("polaris-helper-upgrade-v1 >").unwrap();
        let publish = script.find("mv \"$TXDIR\" \"$PUBLISHED\"").unwrap();
        assert!(stage < acquire && acquire < kind && kind < publish);
        assert!(!script.contains("mktemp -d \"$SUPPORT/helper-upgrade."));
        let cleanup = script
            .split("cleanup() {\n")
            .nth(1)
            .unwrap()
            .split("\n}\ntrap cleanup")
            .next()
            .unwrap();
        assert!(cleanup.find("rm -rf \"$TXDIR\"").unwrap() < cleanup.find("exec 9>&-").unwrap());
        let rollback = script
            .split("rollback() {\n")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        // Unconfirmed shutdown releases the lease while retaining the journal;
        // only the successful recovery branch may delete it before final unlock.
        let failed_stop = rollback
            .split("if ! stop_service; then\n")
            .nth(1)
            .unwrap()
            .split("\n    fi")
            .next()
            .unwrap();
        assert!(
            failed_stop
                .find("touch \"$TXDIR/recovery-required\"")
                .unwrap()
                < failed_stop.find("exec 9>&-").unwrap()
        );
        assert!(failed_stop.contains("exit 1"));
        assert!(!failed_stop.contains("rm -rf"));
        // Both Unix renderers share this guarded rollback tail. Match the whole
        // branch so failed cleanup cannot become unconditional journal removal.
        let cleanup_line = r#"  if [ "$OK" -eq 1 ]; then rm -rf "$TXDIR" || cleanup_failure; else touch "$TXDIR/recovery-required"; echo "helper rollback incomplete; recovery transaction retained: $TXDIR" >&2; fi"#;
        let cleanup = rollback
            .rfind(cleanup_line)
            .expect("exact guarded rollback journal branch");
        let unlock = rollback
            .rfind("  exec 9>&- || cleanup_failure")
            .expect("guarded final lease release");
        assert!(cleanup < unlock);
        assert!(rollback[cleanup + cleanup_line.len()..unlock]
            .trim()
            .is_empty());
        assert!(!rollback.contains("lease-release"));
        assert!(script.contains("case \"$(cat \"$OLD/transaction-phase\")\" in"));
        assert!(script.contains("staging) rm -rf \"$OLD\";;"));
        assert!(script.contains("admission)"));
        assert!(script.contains("completed|rolled-back)"));
        assert!(script.contains("if [ -f \"$OLD/migration-preserve\" ]; then"));
        assert!(script.contains("mv \"$OLD\" \"$RETIRED\""));
        assert!(script.contains("else rm -rf \"$OLD\"; fi;;"));
        assert!(script.contains("*) echo \"HELPER_INSTALL_RECOVERY_REQUIRED $OLD\""));
        assert!(script.contains("set_phase service-mutation"));
        assert!(script.contains("set_phase completed"));
    }
}

// Same closure discipline as the App's shared executable fixture writer:
// this test process never holds a write fd on the directly executed file.
// A single cp child opens/closes it before any cancellation CLI invocation.
#[cfg(unix)]
fn write_executable_stand_in(path: &Path, contents: impl AsRef<[u8]>) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let mut staged = path.as_os_str().to_owned();
    staged.push(".staged");
    let staged = PathBuf::from(staged);
    fs::write(&staged, contents).unwrap();
    let status = Command::new("cp")
        .arg(&staged)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "test executable writer failed: {status}");
    fs::remove_file(staged).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
mod unix_journal_recovery {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn recover(
        phase: &str,
        cancel_succeeds: bool,
        damage: Option<&str>,
    ) -> (tempfile::TempDir, PathBuf, bool) {
        let fixture = tempfile::tempdir().unwrap();
        let support = fixture.path();
        let old = support.join("helper-upgrade.previous");
        let admission = support.join("helper-admission.current");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&admission).unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o700)).unwrap();
        let dest = support.join("installed-helper");
        fs::write(old.join("transaction-kind"), "polaris-helper-upgrade-v1\n").unwrap();
        fs::write(old.join("transaction-destination"), dest.to_str().unwrap()).unwrap();
        fs::write(old.join("transaction-phase"), format!("{phase}\n")).unwrap();
        fs::write(old.join("helper"), b"previous bundled helper bytes").unwrap();
        match damage {
            Some("missing-kind") => fs::remove_file(old.join("transaction-kind")).unwrap(),
            Some("wrong-destination") => {
                fs::write(old.join("transaction-destination"), "other-helper").unwrap()
            }
            Some("unsafe-directory") => {
                fs::set_permissions(&old, fs::Permissions::from_mode(0o777)).unwrap()
            }
            Some("linked-marker") => {
                let marker = old.join("transaction-kind");
                fs::remove_file(&marker).unwrap();
                std::os::unix::fs::symlink(old.join("helper"), &marker).unwrap();
            }
            Some("recovery-required") => {
                fs::write(old.join("recovery-required"), b"retained").unwrap()
            }
            None | Some("native-rejection") => {}
            Some(other) => panic!("unknown recovery fixture damage: {other}"),
        }
        write_executable_stand_in(
            &admission.join("helper"),
            format!(
                r#"#!/bin/sh
set -eu
[ "$#" -eq 3 ] || exit 90
case "$1:$2" in
  validate-install-path:--path)
    printf '%s\n' "$3" >> {validated}
    [ {reject_native} -eq 0 ] || exit 1
    [ -e "$3" ] && [ ! -L "$3" ] || exit 1;;
  cancel-upgrade:--support)
    printf cancelled > "$3/cancelled"
    exit {cancel_exit};;
  *) exit 90;;
esac
"#,
                validated = shell_quote(support.join("validated").to_str().unwrap()),
                reject_native = u8::from(damage == Some("native-rejection")),
                cancel_exit = if cancel_succeeds { 0 } else { 1 },
            ),
        );
        let script = unix_scripts().into_iter().nth(1).unwrap();
        let begin = script
            .find("for OLD in \"$SUPPORT\"/helper-upgrade.*; do")
            .unwrap();
        let end = begin + script[begin..].find("\nPUBLISHED=").unwrap();
        let admin = script
            .split("admin() {\n")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        // Root ownership is a simulated identity in this unprivileged fixture.
        // Real admin routing, mode/link/marker/phase/destination operations stay
        // unchanged; the sealed utility's native validation has explicit success
        // and rejection outcomes and cannot execute an old journal's helper.
        let owner = if cfg!(target_os = "macos") {
            "stat -f '%Lp' \"$1\""
        } else {
            "stat -c '%a' \"$1\""
        };
        let prefix = format!(
            "set -eu\nSUPPORT={}\nDEST={}\nTXDIR={}\nBOOT=''\nMIGRATION=0\nadmin() {{\n{admin}\n}}\nowner_mode() {{ printf '0:'; {owner}; }}\n",
            shell_quote(support.to_str().unwrap()),
            shell_quote(dest.to_str().unwrap()),
            shell_quote(admission.to_str().unwrap())
        );
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("{prefix}{}", &script[begin..end]))
            .output()
            .unwrap();
        if damage.is_none() {
            let validated = fs::read_to_string(support.join("validated")).unwrap();
            let expected: Vec<_> = std::iter::once(old.clone())
                .chain(
                    [
                        "transaction-kind",
                        "transaction-destination",
                        "transaction-phase",
                        "helper",
                    ]
                    .into_iter()
                    .map(|name| old.join(name)),
                )
                .collect();
            assert_eq!(
                validated.lines().map(PathBuf::from).collect::<Vec<_>>(),
                expected,
                "recovery must validate the directory and each exact journal object"
            );
        }
        (fixture, old, output.status.success())
    }

    #[test]
    fn known_pre_mutation_or_completed_journal_retires_but_partial_service_mutation_does_not() {
        for phase in ["staging", "completed", "rolled-back"] {
            let (_fixture, old, success) = recover(phase, false, None);
            assert!(success, "{phase}");
            assert!(!old.exists());
        }
        for phase in ["service-mutation", "unknown"] {
            let (_fixture, old, success) = recover(phase, true, None);
            assert!(!success, "{phase}");
            assert!(old.exists());
        }
        for damage in [
            "missing-kind",
            "wrong-destination",
            "unsafe-directory",
            "linked-marker",
            "recovery-required",
            "native-rejection",
        ] {
            let (fixture, old, success) = recover("staging", true, Some(damage));
            assert!(!success, "{damage}");
            assert!(
                old.exists(),
                "invalid recovery evidence must be retained: {damage}"
            );
            assert!(!fixture.path().join("cancelled").exists(), "{damage}");
        }
    }

    #[test]
    fn possibly_accepted_upgrade_reservation_is_cancelled_before_retirement_and_failed_cancel_is_retained(
    ) {
        for accepted in [true, false] {
            let (fixture, old, success) = recover("admission", accepted, None);
            assert_eq!(success, accepted);
            assert_eq!(old.exists(), !accepted);
            assert!(fixture.path().join("cancelled").exists());
        }
        let (fixture, _old, success) = recover("staging", false, None);
        assert!(success);
        assert!(
            !fixture.path().join("cancelled").exists(),
            "no daemon exists yet in staging"
        );
    }
}

#[test]
fn linux_lifecycle_label_matches_installed_unit_and_uninstall_target() {
    let paths = InstallPaths::linux();
    let descriptor = paths.descriptor.as_ref().unwrap();
    assert_eq!(descriptor.file_name().unwrap(), paths.service_label);
    assert_eq!(paths.service_label, LINUX_SERVICE_NAME);
    let params = install_params(
        PathBuf::from("/inert"),
        PathBuf::from("/inert/polaris-helper"),
    );
    let install = build_linux_install_script(&paths, &params, &"a".repeat(64));
    assert!(install.contains(descriptor.to_str().unwrap()));
    assert!(install.contains(&format!("SERVICE='{LINUX_SERVICE_NAME}'")));
    assert!(install.contains("systemctl enable \"$SERVICE\""));
    assert!(install.contains("systemctl restart \"$SERVICE\""));
    let utility = MaintenanceUtility {
        src_binary: PathBuf::from("/inert/polaris-helper"),
        helper_sha256: "a".repeat(64),
    };
    let uninstall = uninstall_unix::build_linux_uninstall_script(&paths, &utility);
    assert!(uninstall.contains(&format!("SERVICE='{LINUX_SERVICE_NAME}'")));
}

#[test]
fn unix_rollback_stops_before_restoring_when_shutdown_is_unconfirmed() {
    for script in unix_scripts() {
        let rollback = script.find("rollback() {").unwrap();
        let stop = script[rollback..].find("if ! stop_service; then").unwrap() + rollback;
        let stop_error = script[stop..]
            .find("HELPER_ROLLBACK_STOP_UNCONFIRMED")
            .unwrap()
            + stop;
        let exit = script[stop_error..].find("exit 1").unwrap() + stop_error;
        let restore = script[exit..].find("helper.rollback").unwrap() + exit;
        assert!(stop < stop_error && stop_error < exit && exit < restore);
        assert!(!script[rollback..].contains("stop_service || OK=0"));
    }
}

#[test]
fn unix_registration_changes_follow_readiness_or_admission_and_stop() {
    for script in unix_scripts() {
        let header_call = "\nregister_user\n";
        let retained = script.find("retain-probe --support").unwrap();
        let first_register = script.find(header_call).unwrap();
        let admission = script.find("prepare-upgrade --support").unwrap();
        let stop = script[admission..].find("then stop_service; fi").unwrap() + admission;
        let next_register = script[stop..].find(header_call).unwrap() + stop;
        assert!(retained < first_register && first_register < admission);
        assert!(admission < stop && stop < next_register);
        assert!(script.contains("snapshot_registration\nif ["));
        assert!(script.contains("set_phase registration\n  REGISTRATION_CHANGED=1"));
        assert!(script.contains("restore-user --support"));
    }
}

#[test]
fn explicit_migration_rejects_other_platforms_before_validation_token_or_elevation() {
    for &platform in Platform::ALL {
        assert!(InstallMode::Ordinary.supported_on(platform));
        assert_eq!(
            InstallMode::StopAndUpgrade.supported_on(platform),
            matches!(platform, Platform::Mac | Platform::Linux)
        );
        if matches!(platform, Platform::Mac | Platform::Linux) {
            continue;
        }
        let stage = tempfile::tempdir().unwrap();
        let source = stage.path().join("helper");
        let (manager, token_dir) = install_manager(platform, &source);
        let mut params = install_params(stage.path().join("scripts"), source);
        params.helper_sha256.clear(); // Invalid parameters must not mask the unsupported mode.
        let (executor, calls, content) = capturing((String::new(), 0));
        assert!(matches!(
            manager.install_with_mode(&params, &executor, InstallMode::StopAndUpgrade),
            Err(ManagerError::UnsupportedPlatform(p)) if p == platform
        ));
        assert!(
            !manager.token_path.exists(),
            "unsupported mode wrote a token"
        );
        assert_eq!(std::fs::read_dir(token_dir.path()).unwrap().count(), 0);
        assert!(
            !params.script_dir.exists(),
            "unsupported mode created its script directory"
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "unsupported mode escalated"
        );
        assert!(content.lock().unwrap().is_none());
    }
}

#[test]
fn mac_install_mode_selects_separate_scripts_without_fallback() {
    assert_eq!(InstallMode::default(), InstallMode::Ordinary);
    let stage = tempfile::tempdir().unwrap();
    let source = stage.path().join("helper");
    let (manager, _token_dir) = install_manager(Platform::Mac, &source);
    let params = install_params(stage.path().join("scripts"), source);
    let token = "a".repeat(64);
    let ordinary = manager
        .build_install_script(&params, &token, InstallMode::Ordinary)
        .unwrap();
    let migration = manager
        .build_install_script(&params, &token, InstallMode::StopAndUpgrade)
        .unwrap();
    assert_eq!(
        ordinary,
        build_mac_install_script(&manager.paths, &params, &token)
    );
    assert_eq!(
        migration,
        build_mac_migration_script(&manager.paths, &params, &token)
    );
    assert_ne!(
        ordinary, migration,
        "explicit migration must not alias the ordinary transaction"
    );
}

#[path = "linux_uninstall.rs"]
mod linux_uninstall;

fn maintenance_fixture(dir: &Path) -> MaintenanceUtility {
    let src_binary = dir.join("bundled-helper-fixture");
    std::fs::write(&src_binary, b"inert fixture, never executed").unwrap();
    MaintenanceUtility {
        src_binary,
        helper_sha256: "a".repeat(64),
    }
}

#[test]
fn unix_uninstall_without_current_utility_never_escalates() {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("token");
    crate::token::write_token_content(&token_path, "TOK").unwrap();
    let manager = HelperManager::new(
        Platform::Linux,
        token_path.clone(),
        Box::new(MockSysOps::default()),
    );
    let (exec, calls, _) = capturing((String::new(), 0));
    assert!(matches!(
        manager.uninstall(dir.path(), &exec),
        Err(ManagerError::InvalidInstallation(_))
    ));
    assert!(calls.lock().unwrap().is_empty());
    assert!(token_path.exists());
}

#[test]
fn maintenance_identity_requires_absolute_source_and_actual_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut utility = maintenance_fixture(dir.path());
    assert!(validate_maintenance_utility(&utility).is_ok());
    utility.helper_sha256.clear();
    assert!(validate_maintenance_utility(&utility).is_err());
    utility.helper_sha256 = "a".repeat(64);
    utility.src_binary = PathBuf::from("relative");
    assert!(validate_maintenance_utility(&utility).is_err());
}

#[test]
fn windows_registration_is_transactional_and_follows_admission_and_stop() {
    let script = windows_script();
    let admission = script.find("Invoke-Utility @('prepare-upgrade'").unwrap();
    let stop = script.find("  Begin-ServiceReplacement").unwrap();
    let registration = script.find("Invoke-Utility @('register-user'").unwrap();
    let snapshot = script
        .find("$registrationSnapshot = @{ Path = $registration;")
        .unwrap();
    assert!(snapshot < admission && admission < stop && stop < registration);
    let restore = script
        .find("Restore-Registration $registrationSnapshot")
        .unwrap();
    let restart = script.rfind("      Restore-PreviousService").unwrap();
    assert!(restore < restart);
    assert!(script.contains("$keepHelper -and $wasRunning -and $sameRegistration"));
    assert!(script.contains("$registrationChanged = $true"));
    assert!(script.contains("Remove-Item -LiteralPath $snapshot.Path -Force"));
    assert!(!script.contains("polaris-helper-core-only-ok'; return"));
}

#[test]
fn windows_reports_controlled_install_stages_without_stderr_handoff() {
    let script = windows_script();
    for stage in [75, 81, 82, 83, 84, 85, 86] {
        assert!(script.contains(&format!("$failureStage = {stage}")));
    }
    assert!(script.contains("$exitCode = $failureStage"));
    assert!(script.contains("$exitCode = 87"));
    assert!(script.contains("$exitCode = 88"));
    assert!(script.contains("exit $exitCode"));
    assert!(!script.contains("Start-Transcript"));
}

#[test]
fn windows_registration_rollback_proves_atomic_bytes_and_original_security() {
    let script = windows_script();
    assert!(script.contains("AccessControlSections]'Owner,Group,Access'"));
    assert!(script.contains("registration.snapshot.json"));
    assert!(script.contains("Check-Hash $registrationBackup $registrationSnapshot.Hash"));
    assert!(script.contains("[IO.FileMode]::CreateNew"));
    assert!(script.contains("$output.Flush($true)"));
    assert!(script.contains(
        "[IO.File]::Replace($temporary, $snapshot.Path, [System.Management.Automation.Language.NullString]::Value)"
    ));
    assert!(script.contains("[IO.File]::Move($temporary, $snapshot.Path)"));
    assert!(script.contains(
        "$security.SetSecurityDescriptorSddlForm($snapshot.Sddl, $registrationSections)"
    ));
    assert!(script.contains("(Registration-Sddl $snapshot.Path) -cne $snapshot.Sddl"));
    assert!(script.contains("Check-Hash $snapshot.Path $snapshot.Hash"));
    let restore = script
        .split("function Restore-Registration")
        .nth(1)
        .unwrap()
        .split("function Stop-ServiceChecked")
        .next()
        .unwrap();
    assert!(!restore.contains("Seal-File $snapshot.Path"));
    assert!(!restore.contains("[IO.File]::Replace($temporary, $snapshot.Path, $null)"));
}

/// Runs the real restore function against synthetic files only. The existing
/// Windows workspace test job executes this; no service, UAC or global ACL calls.
#[test]
fn windows_registration_rollback_native_fixture_faults() {
    let native =
        polaris_source_probe::repo_file!("crates/windows-cleaner/src/windows/install/tests/mod.rs");
    assert!(native.contains("fn original_8_registration_cases_use_real_native_restore"));
    assert!(native.contains("restore_snapshot_with_policy"));
    for case in [
        "present",
        "missing-destination",
        "original-absent",
        "tampered-backup",
        "blocked-replacement",
        "invalid-security",
        "parent-is-file",
        "reparse-parent",
    ] {
        assert!(native.contains(case), "{case}");
    }
}

#[test]
fn windows_rejects_unsafe_registration_parents_before_reads_or_restore_eligibility() {
    let script = windows_script();
    let users = script
        .find("  Assert-PrivateRegistrationDirectory $users")
        .unwrap();
    let identity = script
        .find("  Assert-PrivateRegistrationDirectory (Join-Path $users $identity)")
        .unwrap();
    let read = script
        .find("$sameRegistration = [IO.File]::ReadAllText")
        .unwrap();
    let changed = script.find("  $registrationChanged = $true").unwrap();
    assert!(users < identity && identity < read && read < changed);
    assert!(script.contains("untrusted registration parent owner"));
    assert!(script.contains("NO_ACCESS_CONTROL"));
    assert!(script.contains("registration parent grants nonprivileged access"));
}

#[test]
fn unsupported_uninstall_never_prepares_or_escalates_maintenance() {
    let dir = tempfile::tempdir().unwrap();
    for platform in [Platform::Android, Platform::Ios, Platform::Other] {
        let manager = HelperManager::new(
            platform,
            dir.path().join("token"),
            Box::new(MockSysOps::default()),
        );
        let (exec, calls, _) = capturing((String::new(), 0));
        assert!(
            matches!(manager.uninstall_with_maintenance(dir.path(), None, &exec), Err(ManagerError::UnsupportedPlatform(p)) if p == platform)
        );
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[test]
fn missing_maintenance_binary_never_escalates_or_clears_token() {
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    crate::token::write_token_content(&token, "TOK").unwrap();
    let manager = HelperManager::new(
        Platform::Linux,
        token.clone(),
        Box::new(MockSysOps::default()),
    );
    let utility = MaintenanceUtility {
        src_binary: dir.path().join("absent-helper"),
        helper_sha256: "a".repeat(64),
    };
    let (exec, calls, _) = capturing((String::new(), 0));
    assert!(matches!(
        manager.uninstall_with_maintenance(dir.path(), Some(&utility), &exec),
        Err(ManagerError::HelperBinaryMissing(_))
    ));
    assert!(calls.lock().unwrap().is_empty());
    assert!(token.exists());
}

#[test]
fn migration_uses_real_closed_plist_renderer_before_candidate_start() {
    let paths = InstallPaths::mac();
    let params = install_params(PathBuf::from("/inert"), PathBuf::from("/inert/helper"));
    let ordinary = build_mac_install_script(&paths, &params, &"a".repeat(64));
    let migration = build_mac_migration_script(&paths, &params, &"a".repeat(64));
    let descriptor = |script: &str, marker: &str| -> String {
        script
            .split(&format!("<<'{marker}'\n"))
            .nth(1)
            .unwrap()
            .split(&format!("\n{marker}\n"))
            .next()
            .unwrap()
            .to_owned()
    };
    let plain = render_mac_plist(
        &paths.binary.to_string_lossy(),
        &paths.core_dir.to_string_lossy(),
        MAC_SUPPORT_DIR,
        false,
    );
    let closed = render_mac_plist(
        &paths.binary.to_string_lossy(),
        &paths.core_dir.to_string_lossy(),
        MAC_SUPPORT_DIR,
        true,
    );
    assert_eq!(descriptor(&ordinary, "POLARIS_DESCRIPTOR_EOF"), plain);
    assert_eq!(descriptor(&migration, "POLARIS_DESCRIPTOR_EOF"), plain);
    assert_eq!(
        descriptor(&migration, "POLARIS_CLOSED_DESCRIPTOR_EOF"),
        closed
    );
    let arguments = |xml: &str| -> Vec<String> {
        xml.split("<array>")
            .nth(1)
            .unwrap()
            .split("</array>")
            .next()
            .unwrap()
            .split("<string>")
            .skip(1)
            .map(|entry| entry.split("</string>").next().unwrap().to_owned())
            .collect()
    };
    let expected = vec![
        paths.binary.to_string_lossy().into_owned(),
        "--support".into(),
        MAC_SUPPORT_DIR.into(),
        "--coredir".into(),
        paths.core_dir.to_string_lossy().into_owned(),
    ];
    assert_eq!(arguments(&plain), expected);
    let mut expected_closed = expected;
    expected_closed.push("--upgrade-closed".into());
    assert_eq!(arguments(&closed), expected_closed);
    assert!(!ordinary.contains("POLARIS_CLOSED_DESCRIPTOR_EOF"));
    let install_closed = migration
        .find("install -o root -g \"$GROUP\" -m 0644 \"$TXDIR/descriptor.closed\"")
        .unwrap();
    let start = migration.find("set_phase candidate-starting").unwrap();
    assert!(install_closed < start);
}

#[test]
fn retained_windows_helper_requires_framing_before_service_or_registration_changes() {
    let source = polaris_source_probe::crate_source!("manager/install_windows.rs");
    let probe = source.find("Invoke-Utility @('admin-probe'").unwrap();
    let registration = source.find("Invoke-Utility @('register-user'").unwrap();
    let admission = source.find("Invoke-Utility @('prepare-upgrade'").unwrap();
    assert!(probe < registration && probe < admission);
    assert!(source.contains("retained newer helper capability unavailable while stopped"));
    assert!(source.contains(".Hash.ToLowerInvariant() -cne '{helper_hash}'"));
}

/// Three fixtures are complete outputs from the real split Unix producers, with
/// synthetic values only. Never execute these administrator script bytes.
#[test]
fn linux_script_producer_matches_package_acceptance_fixtures() {
    // Existing Package exact-test entry also executes the shared service proof
    // with inert command fixtures, never the generated administrator payload.
    #[cfg(target_os = "linux")]
    {
        linux_uninstall::linux_uninstall_requires_effective_unit_and_hierarchical_closure();
        linux_uninstall::linux_install_inactive_snapshot_still_requires_production_closure();
        linux_uninstall::linux_cleanup_failure_markers_do_not_change_success_or_recovery();
        linux_uninstall::linux_maintenance_traps_keep_success_silent_and_cleanup_unknown();
    }
    let paths = InstallPaths::linux();
    // Target is Linux even when this pure producer test runs on Windows.
    let staged_helper = PathBuf::from("/tmp/producer fixture/用户's stage/polaris-helper");
    let mut params = install_params(
        PathBuf::from("/tmp/producer fixture/用户's config"),
        staged_helper.clone(),
    );
    params.conf_dir = params.script_dir.clone();
    params.uid = 1001;
    params.user_identity = polaris_helper_proto::core_protocol::CallerIdentity::Unix(1001);
    let install = build_linux_install_script(&paths, &params, &"e".repeat(64));
    let utility = MaintenanceUtility {
        src_binary: staged_helper,
        helper_sha256: "a".repeat(64),
    };
    let uninstall = uninstall_unix::build_linux_uninstall_script(&paths, &utility);
    let mut legacy_params = params.clone();
    legacy_params.linux_legacy_original_proxy = Some([
        "'127.0.0.1'".to_owned(),
        "3128".to_owned(),
        "false".to_owned(),
        "''".to_owned(),
        "0".to_owned(),
        "''".to_owned(),
        "0".to_owned(),
        "['localhost', '127.0.0.0/8']".to_owned(),
        "'none'".to_owned(),
        "/home/producer-user".to_owned(),
    ]);
    let legacy =
        install_unix::build_linux_legacy_install_script(&paths, &legacy_params, &"e".repeat(64));
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linux-installer");
    for (fixture, kind, bytes) in [
        ("install", "install", install.as_bytes()),
        ("uninstall", "uninstall", uninstall.as_bytes()),
        ("legacy-upgrade", "install", legacy.as_bytes()),
    ] {
        let expected = std::fs::read(fixtures.join(format!("linux-helper-{fixture}.sh"))).unwrap();
        assert_eq!(
            bytes,
            expected.as_slice(),
            "complete {kind} producer bytes differ"
        );
        assert!(bytes.starts_with(b"#!/bin/sh\nset -eu\n"));
        assert!(!bytes.starts_with(b"#!/bin/bash"));
        let text = std::str::from_utf8(bytes).unwrap();
        assert!(text.contains(&shell_quote(&utility.src_binary.to_string_lossy())));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let name = format!("polaris-helper-{kind}.sh");
            let path = write_secure_script(dir.path(), &name, text).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(
                path.file_name().unwrap().to_str().unwrap().len(),
                25 + name.len()
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            // Model run_privileged_script's finally unlink without executing it.
            let mut held = std::fs::File::open(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            let mut still_held = Vec::new();
            std::io::Read::read_to_end(&mut held, &mut still_held).unwrap();
            assert_eq!(still_held, bytes);
            assert!(!path.exists());
        }
    }
}

/// Compile the native SCM reader and run the real policy/transaction functions
/// against inert SCM ops and temporary files; never query/mutate real services.
#[test]
fn windows_existing_service_identity_and_rollback_native_inert_fixture() {
    let native =
        polaris_source_probe::repo_file!("crates/windows-cleaner/src/install/tests/mod.rs");
    assert!(native.contains("original_17_inert_scm_cases_use_production_transaction"));
    assert!(native.contains("original_20_recovery_cases_use_production_catch_and_finish"));
    assert!(native.contains("run(&mut f)"));
    let manager = polaris_source_probe::crate_source!("manager.rs");
    assert!(manager.contains("#[cfg(test)]\nmod install_windows;"));
}

#[test]
fn typed_maintenance_scope_preserves_mac_retirement_linux_removal_and_failures() {
    for platform in [Platform::Mac, Platform::Linux] {
        let dir = tempfile::tempdir().unwrap();
        let utility = maintenance_fixture(dir.path());
        let manager = HelperManager::new(
            platform,
            dir.path().join("token"),
            Box::new(MockSysOps::default()),
        );
        let (executor, calls, _) = capturing((String::new(), 0));
        let result = manager
            .uninstall_with_maintenance_outcome(dir.path(), Some(&utility), &executor)
            .unwrap();
        assert_eq!(
            result,
            if platform == Platform::Mac {
                MaintenanceOutcome::RetiredDataPreserved
            } else {
                MaintenanceOutcome::Removed
            }
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
        let cancellation = if platform == Platform::Mac {
            (1, "User canceled (-128)")
        } else {
            (126, "cancelled")
        };
        let (executor, _, _) = capturing((cancellation.1.into(), cancellation.0));
        assert_eq!(
            manager
                .uninstall_with_maintenance_outcome(dir.path(), Some(&utility), &executor)
                .unwrap(),
            MaintenanceOutcome::Cancelled
        );
        for (code, expected) in [
            (126, MaintenanceOutcome::Cancelled),
            (
                88,
                MaintenanceOutcome::Failed {
                    stderr: "unknown cleanup".into(),
                    code: 88,
                },
            ),
        ] {
            let (executor, _, _) = capturing(("unknown cleanup".into(), code));
            let result = manager
                .uninstall_with_maintenance_outcome(dir.path(), Some(&utility), &executor)
                .unwrap();
            // macOS cancellation is AppleScript -128, not Linux pkexec 126.
            if platform == Platform::Linux {
                assert_eq!(result, expected)
            } else {
                assert!(matches!(result, MaintenanceOutcome::Failed { .. }));
            }
        }
    }
    assert_eq!(
        maintenance_outcome(Platform::Win, EscalationOutcome::Success),
        MaintenanceOutcome::Removed
    );
    assert_eq!(
        maintenance_outcome(Platform::Win, EscalationOutcome::Cancelled),
        MaintenanceOutcome::Cancelled
    );
}

#[test]
fn linux_explicit_forward_only_producer_never_creates_an_ordinary_fallback_after_native_failure() {
    let params = install_params(
        PathBuf::from("/inert/config"),
        PathBuf::from("/inert/stage/polaris-helper"),
    );
    let script = install_unix::build_linux_legacy_install_script(
        &InstallPaths::linux(),
        &params,
        &"e".repeat(64),
    );
    assert!(script.starts_with("#!/bin/sh\nset -eu\n"));
    let legacy = script
        .split("  legacy)\n")
        .nth(1)
        .unwrap()
        .split("  ordinary)")
        .next()
        .unwrap();
    assert!(legacy.contains("admin linux-legacy-install"));
    assert!(!legacy.contains("||"));
    assert!(!legacy.contains("restart"));
    assert!(legacy.contains("exit 0;;"));
    let preflight = script.split("CLASS=$(admin").next().unwrap();
    assert!(!preflight.contains("secure_dir '/usr/local/lib/polaris'"));
    assert!(!preflight.contains("secure_dir \"$COREDIR\""));
    assert!(preflight.contains("secure_dir \"$SUPPORT\""));
    assert!(preflight.contains("exec 9>>\"$LOCK\""));
    assert!(preflight.contains(": > \"$BOOT/original-proxy\""));
}

#[test]
fn linux_ownmask_routing_hint_never_performs_implicit_legacy_recovery() {
    let params = install_params(
        PathBuf::from("/inert/config"),
        PathBuf::from("/inert/stage/polaris-helper"),
    );
    let script = install_unix::build_linux_install_script(&InstallPaths::linux(), &params, "token");
    assert!(script.contains("[ -L \"$DESCRIPTOR\" ]"));
    assert!(script.contains("readlink -- \"$DESCRIPTOR\""));
    assert!(script.contains("HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY"));
    assert!(!script.contains("linux-legacy-install"));
}
