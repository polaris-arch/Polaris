#![allow(clippy::too_many_lines)]

use super::*;
use crate::platform::linux::auth::{NoPeerCred, StaticPeerCred};
use crate::platform::linux::ops::{SystemdAction, SystemdOps, SystemdResult};
use crate::platform::linux::state::{
    BirthAdmission, CoreHandle, CoreSpawner, ManagedChild, ReapBlockerState, SpawnedCore,
    StartAdmission, StopReap,
};
use std::sync::{Arc, Mutex as StdMutex};
use tempfile::tempdir;

// ===== Mock Conn（预置读行队列 + 记录写行）=====

struct MockConn {
    reads: StdMutex<std::collections::VecDeque<String>>,
    writes: StdMutex<Vec<String>>,
}

impl MockConn {
    fn new(reads: Vec<&str>) -> Self {
        Self {
            reads: StdMutex::new(reads.into_iter().map(String::from).collect()),
            writes: StdMutex::new(Vec::new()),
        }
    }
    fn writes(&self) -> Vec<String> {
        self.writes.lock().unwrap().clone()
    }
}

impl Conn for MockConn {
    fn read_line(&mut self) -> String {
        self.reads.lock().unwrap().pop_front().unwrap_or_default()
    }
    fn read_line_strict(&mut self) -> Result<Option<String>, StrictReadError> {
        Ok(self.reads.lock().unwrap().pop_front())
    }
    fn write_line(&mut self, line: &str) -> bool {
        self.writes.lock().unwrap().push(line.to_string());
        true
    }
}

// ===== Mock Spawner =====

struct MockSpawner {
    next_pid: u32,
    fail: bool,
    spawn_calls: StdMutex<Vec<SpawnCoreRequest>>,
    terminate_calls: StdMutex<Vec<u32>>,
    kill_calls: StdMutex<Vec<u32>>,
    start_admission: StdMutex<Option<StartAdmission>>,
    birth_admission: StdMutex<Option<BirthAdmission>>,
    terminate_results: StdMutex<std::collections::VecDeque<StopReap>>,
    birth_stop_results: StdMutex<std::collections::VecDeque<StopReap>>,
    birth_stop_calls: StdMutex<Vec<polaris_helper_proto::HelperBirthTarget>>,
    reaped_births: StdMutex<Vec<polaris_helper_proto::HelperBirthTarget>>,
}

impl MockSpawner {
    fn succeeding(start_pid: u32) -> Self {
        Self {
            next_pid: start_pid,
            fail: false,
            spawn_calls: StdMutex::new(Vec::new()),
            terminate_calls: StdMutex::new(Vec::new()),
            kill_calls: StdMutex::new(Vec::new()),
            start_admission: StdMutex::new(None),
            birth_admission: StdMutex::new(None),
            terminate_results: StdMutex::new(std::collections::VecDeque::new()),
            birth_stop_results: StdMutex::new(std::collections::VecDeque::new()),
            birth_stop_calls: StdMutex::new(Vec::new()),
            reaped_births: StdMutex::new(Vec::new()),
        }
    }
}

impl CoreSpawner for MockSpawner {
    fn start_admission(&self, current_running: Option<&CoreHandle>) -> StartAdmission {
        self.start_admission.lock().unwrap().unwrap_or_else(|| {
            current_running.map_or(StartAdmission::Admitted, |handle| StartAdmission::Already {
                pid: handle.pid,
            })
        })
    }

    fn birth_admission(&self, current_running: Option<&CoreHandle>) -> BirthAdmission {
        self.birth_admission.lock().unwrap().unwrap_or_else(|| {
            current_running.map_or(BirthAdmission::Admitted, |handle| {
                handle.target().map_or(
                    BirthAdmission::Blocked {
                        target: None,
                        state: ReapBlockerState::Unknown,
                    },
                    BirthAdmission::Already,
                )
            })
        })
    }
    fn stop_birth(&self, target: &polaris_helper_proto::HelperBirthTarget) -> StopReap {
        self.birth_stop_calls.lock().unwrap().push(*target);
        self.birth_stop_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(StopReap::Unknown)
    }
    fn reaped_birth(&self, target: &polaris_helper_proto::HelperBirthTarget) -> bool {
        self.reaped_births.lock().unwrap().contains(target)
    }

    fn spawn(&self, req: &SpawnCoreRequest) -> Result<SpawnedCore, SpawnError> {
        self.spawn_calls.lock().unwrap().push(req.clone());
        if self.fail {
            return Err(SpawnError::Spawn {
                detail: "mock spawn failure".into(),
            });
        }
        let pid = self.next_pid;
        Ok(SpawnedCore {
            handle: req.birth.map_or_else(
                || CoreHandle::new(pid),
                |birth| CoreHandle::exact(std::num::NonZeroU32::new(pid).unwrap(), birth),
            ),
            process_ms: 0,
            log_handoff_ms: 0,
        })
    }
    fn terminate(&self, h: &CoreHandle) -> StopReap {
        self.terminate_calls.lock().unwrap().push(h.pid);
        self.terminate_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(StopReap::Reaped)
    }
    fn kill(&self, h: &CoreHandle) {
        self.kill_calls.lock().unwrap().push(h.pid);
    }
}

// ===== Mock FreePortDeps =====

struct MockFreePort {
    uid_map: StdMutex<std::collections::HashMap<u32, u32>>,
    comm_map: StdMutex<std::collections::HashMap<u32, String>>,
    killed: StdMutex<Vec<u32>>,
}

impl MockFreePort {
    fn empty() -> Self {
        Self {
            uid_map: StdMutex::new(std::collections::HashMap::new()),
            comm_map: StdMutex::new(std::collections::HashMap::new()),
            killed: StdMutex::new(Vec::new()),
        }
    }
}

impl FreePortDeps for MockFreePort {
    fn proc_uid(&self, pid: u32) -> Option<u32> {
        self.uid_map.lock().unwrap().get(&pid).copied()
    }
    fn proc_comm(&self, pid: u32) -> Option<String> {
        self.comm_map.lock().unwrap().get(&pid).cloned()
    }
    fn kill(&self, pid: u32) -> bool {
        self.killed.lock().unwrap().push(pid);
        true
    }
}

// ===== Mock Systemd（handler 测试用，记录调用）=====

#[derive(Default)]
struct MockSystemd {
    calls: StdMutex<Vec<(String, SystemdAction)>>,
}

impl SystemdOps for MockSystemd {
    fn run(&self, unit: &str, action: SystemdAction) -> SystemdResult {
        self.calls.lock().unwrap().push((unit.to_string(), action));
        SystemdResult::ok()
    }
}

struct NoopResolvedDns;

impl ResolvedDnsOps for NoopResolvedDns {
    fn takeover(&self, _interface_name: &str, _server_ip: &str) -> Result<(), String> {
        Ok(())
    }

    fn revert(&self, _interface_name: &str) -> Result<(), String> {
        Ok(())
    }
}

static NOOP_RESOLVED_DNS: NoopResolvedDns = NoopResolvedDns;

#[derive(Default)]
struct MockResolvedDns {
    calls: StdMutex<Vec<Vec<String>>>,
    error: Option<String>,
}

impl ResolvedDnsOps for MockResolvedDns {
    fn takeover(&self, interface_name: &str, server_ip: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(vec![
            "takeover".into(),
            interface_name.into(),
            server_ip.into(),
        ]);
        self.error.clone().map_or(Ok(()), Err)
    }

    fn revert(&self, interface_name: &str) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(vec!["revert".into(), interface_name.into()]);
        self.error.clone().map_or(Ok(()), Err)
    }
}

// ===== 装配 helper =====

#[allow(clippy::too_many_arguments)]
fn make_deps<'a, PE: PeerCredProvider>(
    core_dir: Option<&'a Path>,
    auth_file: &'a Path,
    peer: &'a PE,
    spawner: &'a MockSpawner,
    fp: &'a MockFreePort,
    systemd: &'a MockSystemd,
    ss: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
    fwd: &'a (dyn Fn(bool) + Send + Sync),
) -> HandlerDeps<'a, PE, MockSpawner, MockFreePort, MockSystemd> {
    make_deps_with_resolved(
        core_dir,
        auth_file,
        peer,
        spawner,
        fp,
        systemd,
        ss,
        fwd,
        &NOOP_RESOLVED_DNS,
    )
}

#[allow(clippy::too_many_arguments)]
fn make_deps_with_resolved<'a, PE: PeerCredProvider>(
    core_dir: Option<&'a Path>,
    auth_file: &'a Path,
    peer: &'a PE,
    spawner: &'a MockSpawner,
    fp: &'a MockFreePort,
    systemd: &'a MockSystemd,
    ss: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
    fwd: &'a (dyn Fn(bool) + Send + Sync),
    resolved_dns: &'a dyn ResolvedDnsOps,
) -> HandlerDeps<'a, PE, MockSpawner, MockFreePort, MockSystemd> {
    HandlerDeps {
        core_dir,
        auth_file,
        peer_cred: peer,
        spawner,
        freeport_deps: fp,
        systemd,
        resolved_dns,
        ss_provider: ss,
        set_forward: fwd,
    }
}

/// 造一个授权文件 + coreDir（含 sing-box 二进制）。
///
/// **本文件里跨过鉴权门的用例一律取 root 对端（`StaticPeerCred::new(0, 0)`）**：可信 authfile 的判据是
/// owner==root(0) 且权限不含 group/other 位（见 `auth::authorize_uid`），非特权测试进程造不出这样一份
/// 文件 —— 这里造的 authfile 属主是当前用户，对非 root 对端恒判不可信。root 对端不读 authfile（恒授权），
/// 被测的分发/命令语义与对端 uid 的具体值无关，故取 root 是等价改写。鉴权门本身由
/// [`unauthorized_uid_rejected_for_status`] / [`root_always_authorized`] 与 auth 模块的判据测试覆盖。
fn setup_env() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempdir().unwrap();
    let auth = dir.path().join("auth");
    std::fs::write(&auth, "1000\n").unwrap();
    let core_dir = dir.path().join("core");
    std::fs::create_dir_all(&core_dir).unwrap();
    std::fs::write(core_dir.join("sing-box"), b"#!bin\nfake sing-box").unwrap();
    (dir, auth, core_dir)
}

fn no_op_fwd() -> impl Fn(bool) {
    |_| {}
}

fn no_op_ss() -> impl Fn(&str) -> Option<String> {
    move |_: &str| None
}

fn exact_handle(pid: u32, hex: &str) -> CoreHandle {
    CoreHandle::exact(
        std::num::NonZeroU32::new(pid).unwrap(),
        HelperBirthToken::parse_wire(hex).unwrap(),
    )
}

const BIRTH_ONE: &str = "00112233445566778899aabbccddeeff";
const BIRTH_TWO: &str = "ffeeddccbbaa99887766554433221100";

#[test]
fn exact_start_status_and_stop_keep_a_single_birth_identity() {
    let (dir, auth, core_dir) = setup_env();
    let cfg = dir.path().join("cfg.json");
    std::fs::write(&cfg, b"{}").unwrap();
    let uid = nix::unistd::getuid().as_raw();
    let peer = StaticPeerCred::new(uid, uid);
    let cred = PeerCred { uid, gid: uid };
    let spawner = MockSpawner::succeeding(7777);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let forwards = StdMutex::new(Vec::new());
    let fwd = |value| forwards.lock().unwrap().push(value);
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let mut state = HandlerState::new();
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let cfg = cfg.to_string_lossy().into_owned();
    let mut start = MockConn::new(vec![&sb, &cfg, "", "1", ""]);
    dispatch_locked(&mut state, &deps, &cred, lcmd::START_BIRTH_SAFE, &mut start);
    let Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::Started { target, .. })) =
        Response::parse(&start.writes()[0])
    else {
        panic!("exact start did not return target")
    };
    assert_eq!(target.pid.get(), 7777);
    assert_eq!(
        spawner.spawn_calls.lock().unwrap()[0].birth,
        Some(target.birth)
    );
    assert_eq!(
        state.child.as_ref().unwrap().handle().target(),
        Some(target)
    );

    let mut already = MockConn::new(vec![&sb, &cfg, "", "1", ""]);
    dispatch_locked(
        &mut state,
        &deps,
        &cred,
        lcmd::START_BIRTH_SAFE,
        &mut already,
    );
    assert_eq!(
        Response::parse(&already.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStart(LinuxBirthStart::Already {
            target
        }))
    );
    assert_eq!(spawner.spawn_calls.lock().unwrap().len(), 1);

    let mut status = MockConn::new(vec![]);
    dispatch_locked(
        &mut state,
        &deps,
        &cred,
        lcmd::STATUS_BIRTH_SAFE,
        &mut status,
    );
    assert_eq!(
        Response::parse(&status.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Running {
            target
        }))
    );

    spawner
        .birth_stop_results
        .lock()
        .unwrap()
        .extend([StopReap::Pending, StopReap::Reaped]);
    let pid = target.pid.to_string();
    let token = target.birth.to_wire();
    let mut pending = MockConn::new(vec![&pid, &token]);
    dispatch_locked(
        &mut state,
        &deps,
        &cred,
        lcmd::STOP_BIRTH_SAFE,
        &mut pending,
    );
    assert_eq!(
        Response::parse(&pending.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStop(LinuxBirthStop::Pending {
            target
        }))
    );
    assert!(matches!(state.child, Some(ManagedChild::Stopping(_))));
    assert_eq!(*forwards.lock().unwrap(), [true]);
    let mut stopping = MockConn::new(vec![]);
    dispatch_locked(
        &mut state,
        &deps,
        &cred,
        lcmd::STATUS_BIRTH_SAFE,
        &mut stopping,
    );
    assert_eq!(
        Response::parse(&stopping.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Stopping {
            target
        }))
    );
    let mut reaped = MockConn::new(vec![&pid, &token]);
    dispatch_locked(&mut state, &deps, &cred, lcmd::STOP_BIRTH_SAFE, &mut reaped);
    assert_eq!(
        Response::parse(&reaped.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStop(LinuxBirthStop::Stopped {
            target
        }))
    );
    assert!(state.child.is_none());
    assert_eq!(*forwards.lock().unwrap(), [true, false]);
}

#[test]
fn exact_stop_rejects_mismatch_and_bad_frames_without_mutation() {
    let (_dir, auth, _core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let cred = PeerCred { uid: 0, gid: 0 };
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let current = exact_handle(42, BIRTH_ONE);
    let mut state = HandlerState {
        child: Some(ManagedChild::Running(current.clone())),
    };
    for args in [
        vec!["0", BIRTH_ONE],
        vec!["42", BIRTH_TWO, BIRTH_TWO],
        vec!["42", "00112233445566778899AABBCCDDEEFF"],
        vec!["42"],
    ] {
        let mut conn = MockConn::new(args);
        dispatch_locked(&mut state, &deps, &cred, lcmd::STOP_BIRTH_SAFE, &mut conn);
        assert_eq!(conn.writes(), ["ERR bad-args"]);
    }
    let mut wrong = MockConn::new(vec!["42", BIRTH_TWO]);
    dispatch_locked(&mut state, &deps, &cred, lcmd::STOP_BIRTH_SAFE, &mut wrong);
    assert!(matches!(
        Response::parse(&wrong.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStop(
            LinuxBirthStop::Mismatch { .. }
        ))
    ));
    assert!(state.child.as_ref().unwrap().handle().same_birth(&current));
    assert!(spawner.birth_stop_calls.lock().unwrap().is_empty());

    let old = exact_handle(42, BIRTH_TWO).target().unwrap();
    spawner.reaped_births.lock().unwrap().push(old);
    let mut late = MockConn::new(vec!["42", BIRTH_TWO]);
    dispatch_locked(&mut state, &deps, &cred, lcmd::STOP_BIRTH_SAFE, &mut late);
    assert_eq!(
        Response::parse(&late.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStop(LinuxBirthStop::Stopped {
            target: old
        }))
    );
    assert!(state.child.as_ref().unwrap().handle().same_birth(&current));
    assert!(spawner.birth_stop_calls.lock().unwrap().is_empty());
}

#[test]
fn stopping_exact_birth_cannot_be_reused_as_already_or_admitted() {
    let (_dir, auth, _core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let cred = PeerCred { uid: 0, gid: 0 };
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let forwards = StdMutex::new(Vec::new());
    let fwd = |value| forwards.lock().unwrap().push(value);
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let old = exact_handle(42, BIRTH_ONE);
    let target = old.target().unwrap();
    let mut state = HandlerState {
        child: Some(ManagedChild::Stopping(old)),
    };
    let mut conn = MockConn::new(vec!["/core/sing-box", "/cfg.json", "", "0", ""]);
    dispatch_locked(&mut state, &deps, &cred, lcmd::START_BIRTH_SAFE, &mut conn);
    assert_eq!(
        Response::parse(&conn.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStart(
            LinuxBirthStart::NotAdmittedPending { target }
        ))
    );
    assert!(matches!(state.child, Some(ManagedChild::Stopping(_))));
    assert!(spawner.spawn_calls.lock().unwrap().is_empty());
    assert!(forwards.lock().unwrap().is_empty());
}

#[test]
fn hidden_legacy_custody_never_reports_exact_empty_or_spawns() {
    let (_dir, auth, _core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let cred = PeerCred { uid: 0, gid: 0 };
    let spawner = MockSpawner::succeeding(100);
    *spawner.birth_admission.lock().unwrap() = Some(BirthAdmission::Blocked {
        target: None,
        state: ReapBlockerState::Pending,
    });
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let forwards = StdMutex::new(Vec::new());
    let fwd = |value| forwards.lock().unwrap().push(value);
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    let mut status = MockConn::new(vec![]);
    dispatch_locked(
        &mut state,
        &deps,
        &cred,
        lcmd::STATUS_BIRTH_SAFE,
        &mut status,
    );
    assert_eq!(status.writes(), ["ERR unknown"]);
    let mut start = MockConn::new(vec!["/core/sing-box", "/cfg.json", "", "0", ""]);
    dispatch_locked(&mut state, &deps, &cred, lcmd::START_BIRTH_SAFE, &mut start);
    assert_eq!(
        Response::parse(&start.writes()[0]),
        Response::Ok(ResponseKind::LinuxBirthStart(
            LinuxBirthStart::NotAdmittedUnknown { target: None }
        ))
    );
    assert!(spawner.spawn_calls.lock().unwrap().is_empty());
    assert!(forwards.lock().unwrap().is_empty());
}

#[test]
fn exact_commands_reject_extra_lines_before_custody_changes() {
    let (_dir, auth, _core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let cred = PeerCred { uid: 0, gid: 0 };
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let forwards = StdMutex::new(Vec::new());
    let fwd = |value| forwards.lock().unwrap().push(value);
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    let mut status = MockConn::new(vec!["unexpected"]);
    dispatch_locked(
        &mut state,
        &deps,
        &cred,
        lcmd::STATUS_BIRTH_SAFE,
        &mut status,
    );
    assert_eq!(status.writes(), ["ERR bad-args"]);
    let mut start = MockConn::new(vec!["/core/sing-box", "/cfg.json", "", "0", "", "extra"]);
    dispatch_locked(&mut state, &deps, &cred, lcmd::START_BIRTH_SAFE, &mut start);
    assert_eq!(start.writes(), ["ERR bad-args"]);
    assert!(state.child.is_none());
    assert!(spawner.spawn_calls.lock().unwrap().is_empty());
    assert!(forwards.lock().unwrap().is_empty());
}

#[test]
fn legacy_mutating_commands_cannot_touch_exact_birth() {
    let (_dir, auth, _core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let cred = PeerCred { uid: 0, gid: 0 };
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss_calls = StdMutex::new(0);
    let ss = |_: &str| {
        *ss_calls.lock().unwrap() += 1;
        None
    };
    let forwards = StdMutex::new(Vec::new());
    let fwd = |value| forwards.lock().unwrap().push(value);
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let current = exact_handle(42, BIRTH_ONE);
    let mut state = HandlerState {
        child: Some(ManagedChild::Running(current.clone())),
    };
    for command in [
        cmd::START,
        lcmd::START_REAP_SAFE,
        cmd::STOP,
        lcmd::STOP_REAP_SAFE,
        cmd::STATUS,
        cmd::CLEANUP,
        cmd::FREEPORT,
        lcmd::INSTALL_CORE,
    ] {
        assert!(legacy_command_rejected(command, &state, &spawner));
    }
    // These dispatched paths use only mocks if the guard regresses; cleanup's
    // external pkill is covered by the same pure predicate above.
    for command in [cmd::STOP, cmd::FREEPORT, cmd::START] {
        let mut conn = MockConn::new(vec!["9090"]);
        dispatch_locked(&mut state, &deps, &cred, command, &mut conn);
        assert_eq!(conn.writes(), ["ERR unknown"]);
    }
    assert!(state.child.as_ref().unwrap().handle().same_birth(&current));
    assert!(spawner.terminate_calls.lock().unwrap().is_empty());
    assert!(spawner.spawn_calls.lock().unwrap().is_empty());
    assert_eq!(*ss_calls.lock().unwrap(), 0);
    assert!(forwards.lock().unwrap().is_empty());
}

// ===== ping / version（鉴权前）=====

#[test]
fn ping_responds_before_auth() {
    // 即使 uid 不在授权列表，ping 也应响应（Go :345-347）。
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(9999, 9999); // 未授权 uid
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["ping"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(
        conn.writes(),
        vec![format!(
            "OK pong uid=9999 v{PROTO_VERSION} build={}",
            polaris_helper_proto::build_identity::current()
        )]
    );
}

#[test]
fn version_responds_before_auth() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["version"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec![format!("OK {PROTO_VERSION}")]);
}

// ===== SO_PEERCRED 失败 =====

#[test]
fn peercred_failure_returns_err_peercred() {
    let (_dir, auth, _core) = setup_env();
    let peer = NoPeerCred; // SO_PEERCRED 失败
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["ping"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR peercred"]);
}

// ===== unauthorized =====

#[test]
fn unauthorized_uid_rejected_for_status() {
    let (_dir, auth, _core) = setup_env();
    // auth 只授权 1000；对端 uid 9999 → unauthorized。
    let peer = StaticPeerCred::new(9999, 9999);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["status"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR unauthorized"]);
}

#[test]
fn root_always_authorized() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0); // root
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["status"]);
    handle(&state, &deps, &mut conn);
    // root 应通过鉴权 → OK stopped（无 child）。
    assert_eq!(conn.writes(), vec!["OK stopped"]);
}

// ===== status / stop =====

#[test]
fn status_stopped_when_no_child() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["status"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK stopped"]);
}

#[test]
fn status_running_when_child_present() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(4242);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    state.child = Some(ManagedChild::Running(CoreHandle::new(4242)));
    let state = Mutex::new(state);
    let mut conn = MockConn::new(vec!["status"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK running 4242"]);
}

#[test]
fn stop_notrunning_when_no_child() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["stop"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK notrunning"]);
}

#[test]
fn stop_terminates_child_and_reports_pid() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(555);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd_called = Arc::new(StdMutex::new(Vec::new()));
    let fwd = {
        let fc = Arc::clone(&fwd_called);
        move |on: bool| fc.lock().unwrap().push(on)
    };
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    state.child = Some(ManagedChild::Running(CoreHandle::new(555)));
    let state = Mutex::new(state);
    let mut conn = MockConn::new(vec!["stop"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK stopped 555"]);
    assert_eq!(*spawner.terminate_calls.lock().unwrap(), vec![555]);
    assert_eq!(
        *fwd_called.lock().unwrap(),
        vec![false],
        "stop 应复位转发态"
    );
}

#[test]
fn reap_safe_stop_retains_stopping_birth_until_exact_reap() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(555);
    spawner
        .terminate_results
        .lock()
        .unwrap()
        .extend([StopReap::Pending, StopReap::Reaped]);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd_called = Arc::new(StdMutex::new(Vec::new()));
    let fwd = {
        let calls = Arc::clone(&fwd_called);
        move |on| calls.lock().unwrap().push(on)
    };
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState {
        child: Some(ManagedChild::Running(CoreHandle::new(555))),
    });

    let mut pending = MockConn::new(vec![lcmd::STOP_REAP_SAFE, "555"]);
    handle(&state, &deps, &mut pending);
    assert_eq!(pending.writes(), ["OK stop-pending 555"]);
    assert!(matches!(
        state.lock().unwrap().child.as_ref(),
        Some(ManagedChild::Stopping(_))
    ));
    assert!(fwd_called.lock().unwrap().is_empty());

    let mut reaped = MockConn::new(vec![lcmd::STOP_REAP_SAFE, "555"]);
    handle(&state, &deps, &mut reaped);
    assert_eq!(reaped.writes(), ["OK stopped 555"]);
    assert!(state.lock().unwrap().child.is_none());
    assert_eq!(*fwd_called.lock().unwrap(), [false]);
}

#[test]
fn stop_unknown_retains_custody_and_never_claims_success() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(556);
    spawner
        .terminate_results
        .lock()
        .unwrap()
        .push_back(StopReap::Unknown);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd_called = Arc::new(StdMutex::new(Vec::new()));
    let fwd = {
        let calls = Arc::clone(&fwd_called);
        move |on| calls.lock().unwrap().push(on)
    };
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState {
        child: Some(ManagedChild::Running(CoreHandle::new(556))),
    });
    let mut conn = MockConn::new(vec!["stop", "556"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), ["OK stop-unknown 556"]);
    assert!(matches!(
        state.lock().unwrap().child.as_ref(),
        Some(ManagedChild::Stopping(_))
    ));
    assert!(fwd_called.lock().unwrap().is_empty());
}

// ===== stop 的受管 pid 身份判据（杀错进程的防线）=====

/// **变异门（核心）**：身份不匹配时 **一个进程都不许动**。
///
/// 场景（真机时序）：客户端的老 stop 腿挂在 IPC 上，期间用户重装 helper 并起了新核 9001；
/// 这条腿醒来后拿着旧 pid 555 落到 daemon —— daemon 手里已是新核。
///
/// 变异（逃逸面穷举）：
/// - 删掉 `handle_stop` 里的 `stop_pid_matches` 判据（退回「反正要停就杀当前的」）→
///   `terminate_calls == [9001]` + 响应变 `OK stopped 9001` → 转红。
/// - 判据改成只比大小/恒真 → 同上转红。
/// - 只改响应不改行为（回 mismatch 但仍 `take()` + `terminate`）→ 后两条断言转红。
/// - 顺手把 `set_forward(false)` 留着（让位腿却复位了新会话的转发态）→ fwd 断言转红。
#[test]
fn stop_refuses_to_kill_when_managed_pid_is_another_session() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(9001);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd_called = Arc::new(StdMutex::new(Vec::new()));
    let fwd = {
        let fc = Arc::clone(&fwd_called);
        move |on: bool| fc.lock().unwrap().push(on)
    };
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    // daemon 手里的是**新会话**的核。
    state.child = Some(ManagedChild::Running(CoreHandle::new(9001)));
    let state = Mutex::new(state);
    // 老 stop 腿声明它要停的是 555。
    let mut conn = MockConn::new(vec!["stop", "555"]);
    handle(&state, &deps, &mut conn);

    assert_eq!(
        conn.writes(),
        vec!["OK stop-mismatch 555 9001"],
        "身份不匹配 → 诚实 no-op 并回报两个 pid（客户端据此记账/记日志）"
    );
    assert!(
        spawner.terminate_calls.lock().unwrap().is_empty(),
        "绝不能杀：9001 是用户刚连上的新核，杀它 = 静默断线且现象酷似核自己崩了"
    );
    assert!(
        spawner.kill_calls.lock().unwrap().is_empty(),
        "也不许走 kill 腿"
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .child
            .as_ref()
            .map(|child| child.handle().pid),
        Some(9001),
        "child 记账必须原样留给新会话（摘掉 = 新核失联，daemon 再也停不掉它）"
    );
    assert!(
        fwd_called.lock().unwrap().is_empty(),
        "让位腿不得复位新会话的 IP 转发态"
    );
}

/// 身份**匹配**时照常停（反向失效门）：判据不能收得太紧，否则停核彻底失效。
#[test]
fn stop_proceeds_when_managed_pid_matches_request() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(555);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    state.child = Some(ManagedChild::Running(CoreHandle::new(555)));
    let state = Mutex::new(state);
    let mut conn = MockConn::new(vec!["stop", "555"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK stopped 555"]);
    assert_eq!(*spawner.terminate_calls.lock().unwrap(), vec![555]);
    assert!(state.lock().unwrap().child.is_none());
}

/// 无 child 时带身份 → 诚实 `notrunning`（不是 mismatch —— 本来就没东西可杀）。
#[test]
fn stop_with_identity_reports_notrunning_when_no_child() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["stop", "555"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK notrunning"]);
}

/// **wire 向后兼容门**：旧客户端只发 `stop`（无身份行）→ 沿用「停当前受管核」旧语义。
///
/// 变异：把 `parse_stop_pid` 的空串处置改成 `Some(0)` → 判据恒不匹配 → 本测转红（那会让
/// 装了新 helper 的机器上、任何不带身份的停核请求全部失效 = 永远停不掉核）。
#[test]
fn stop_without_identity_line_keeps_legacy_semantics() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(777);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    state.child = Some(ManagedChild::Running(CoreHandle::new(777)));
    let state = Mutex::new(state);
    let mut conn = MockConn::new(vec!["stop"]); // 无身份行（read_line 在耗尽后返 ""）
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK stopped 777"]);
    assert_eq!(*spawner.terminate_calls.lock().unwrap(), vec![777]);
}

// ===== unknown command =====

#[test]
fn unknown_command_returns_err_unknown() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["frobnicate"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR unknown"]);
}

// ===== freeport =====

#[test]
fn freeport_bad_port_rejected() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["freeport", "abc"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR bad-port"]);
}

#[test]
fn freeport_empty_port_rejected() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["freeport", ""]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR bad-port"]);
}

#[test]
fn freeport_free_when_ss_returns_none() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss(); // ss 缺失
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["freeport", "9090"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK free"]);
}

#[test]
fn freeport_kills_own_singbox() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    // 归属判定是 `free_port` 里的 `proc_uid(pid) == caller_uid`（无 root 特殊分支），
    // 故 caller 与 /proc 属主一同取 0 时，被测的「只动对端自己的进程」语义逐字不变。
    fp.uid_map.lock().unwrap().insert(1234, 0);
    fp.comm_map.lock().unwrap().insert(1234, "sing-box".into());
    let systemd = MockSystemd::default();
    let ss = |_p: &str| Some("pid=1234".to_string());
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["freeport", "9090"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK killed 1234"]);
    assert_eq!(*fp.killed.lock().unwrap(), vec![1234]);
}

// ===== install-core =====

#[test]
fn install_core_coredir_unset() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState::new());
    let hash = "a".repeat(64);
    let mut conn = MockConn::new(vec!["install-core", "/tmp/src", &hash]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR coredir-unset"]);
}

#[test]
fn install_core_bad_args_for_short_hash() {
    let (_dir, auth, core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec!["install-core", "/tmp/src", "abc"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR bad-args"]);
}

// ===== start =====

#[test]
fn start_bad_args_when_cfg_empty() {
    let (_dir, auth, core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let state = Mutex::new(HandlerState::new());
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    // singbox / cfg="" / log / fwd / ppid
    let mut conn = MockConn::new(vec!["start", &sb, "", "", "0", ""]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR bad-args"]);
}

#[test]
fn start_core_path_denied_when_singbox_mismatch() {
    let (_dir, auth, core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let state = Mutex::new(HandlerState::new());
    // singbox 传一个错误路径（!= coreDir/sing-box）。
    let mut conn = MockConn::new(vec![
        "start",
        "/tmp/evil/sing-box",
        "/tmp/cfg.json",
        "",
        "0",
        "",
    ]);
    handle(&state, &deps, &mut conn);
    let w = &conn.writes()[0];
    assert!(w.starts_with("ERR core-path-denied"), "got {w}");
}

#[test]
fn start_core_missing_when_binary_absent() {
    // coreDir 存在但 sing-box 不存在 → core-missing。
    let dir = tempdir().unwrap();
    let auth = dir.path().join("auth");
    std::fs::write(&auth, "1000\n").unwrap();
    let core_dir = dir.path().join("core");
    std::fs::create_dir_all(&core_dir).unwrap();
    // 不建 sing-box 二进制。

    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let state = Mutex::new(HandlerState::new());
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let mut conn = MockConn::new(vec!["start", &sb, "/tmp/c.json", "", "0", ""]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR core-missing"]);
}

#[test]
fn start_config_not_owned_rejected() {
    let (_dir, auth, core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    // cfg 路径不存在 → owned_by 返回 err → config-not-owned。
    let state = Mutex::new(HandlerState::new());
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let mut conn = MockConn::new(vec!["start", &sb, "/nonexistent/cfg.json", "", "0", ""]);
    handle(&state, &deps, &mut conn);
    let w = &conn.writes()[0];
    assert!(w.starts_with("ERR config-not-owned"), "got {w}");
}

#[test]
fn start_spawns_and_reports_pid() {
    let (dir, auth, core_dir) = setup_env();
    // 造一个属主 = 本进程 uid 的 cfg。
    let cfg = dir.path().join("cfg.json");
    let self_uid = nix::unistd::getuid().as_raw();
    std::fs::write(&cfg, b"{}").unwrap();
    let peer = StaticPeerCred::new(self_uid, self_uid);

    let spawner = MockSpawner::succeeding(7777);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let mut state = HandlerState::new();
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let cfg_s = cfg.to_string_lossy().into_owned();
    let mut conn = MockConn::new(vec![&sb, &cfg_s, "", "0", ""]);
    // 直调 `dispatch_locked` 跳过 `handle` 的 authfile 鉴权门：本例的被测判据要求对端 uid == cfg 属主
    // （非 root），而可信 authfile 必须 root 属主 + 无 group/other 位，非特权测试进程造不出；鉴权门本身
    // 由 unauthorized_uid_rejected_for_status / root_always_authorized 与 auth 模块判据测试独立覆盖。
    let cred = PeerCred {
        uid: self_uid,
        gid: self_uid,
    };
    dispatch_locked(&mut state, &deps, &cred, cmd::START, &mut conn);
    let writes = conn.writes();
    assert_eq!(writes.len(), 1);
    assert!(matches!(
        Response::parse(&writes[0]),
        Response::Ok(ResponseKind::Start(Start::StartedTimed { pid: 7777, .. }))
    ));
    let calls = spawner.spawn_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].binary, core_dir.join("sing-box"));
}

/// log 必须与 cfg 同父目录 —— 不校验就是「root 在任意位置建文件、再 fchown 给调用者」。
#[test]
fn start_log_path_denied_when_outside_cfg_dir() {
    let (dir, auth, core_dir) = setup_env();
    let cfg = dir.path().join("cfg.json");
    let self_uid = nix::unistd::getuid().as_raw();
    std::fs::write(&cfg, b"{}").unwrap();
    let peer = StaticPeerCred::new(self_uid, self_uid);
    let spawner = MockSpawner::succeeding(7777);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let mut state = HandlerState::new();
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let cfg_s = cfg.to_string_lossy().into_owned();
    // cfg 合法（属主对、目录对），只有 log 越界。
    let mut conn = MockConn::new(vec![&sb, &cfg_s, "/etc/cron.d/pwn", "0", ""]);
    // 直调 `dispatch_locked` 跳过 `handle` 的 authfile 鉴权门：本例的被测判据要求对端 uid == cfg 属主
    // （非 root），而可信 authfile 必须 root 属主 + 无 group/other 位，非特权测试进程造不出；鉴权门本身
    // 由 unauthorized_uid_rejected_for_status / root_always_authorized 与 auth 模块判据测试独立覆盖。
    let cred = PeerCred {
        uid: self_uid,
        gid: self_uid,
    };
    dispatch_locked(&mut state, &deps, &cred, cmd::START, &mut conn);
    assert_eq!(conn.writes(), vec!["ERR log-path-denied"]);
    // 越界就不该 spawn —— 只看错误行不够，恒拒的实现也会让上一条通过。
    assert!(
        spawner.spawn_calls.lock().unwrap().is_empty(),
        "被拒了却仍然起了核"
    );
}

/// 生产形态（log 与 cfg 同目录）必须放行，且 log 被原样下发给 spawner。
#[test]
fn start_ok_when_log_beside_cfg() {
    let (dir, auth, core_dir) = setup_env();
    let cfg = dir.path().join("singbox-runtime.json");
    let log = dir.path().join("singbox-startup.log");
    let self_uid = nix::unistd::getuid().as_raw();
    std::fs::write(&cfg, b"{}").unwrap();
    let peer = StaticPeerCred::new(self_uid, self_uid);
    let spawner = MockSpawner::succeeding(7777);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let mut state = HandlerState::new();
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let cfg_s = cfg.to_string_lossy().into_owned();
    let log_s = log.to_string_lossy().into_owned();
    let mut conn = MockConn::new(vec![&sb, &cfg_s, &log_s, "0", ""]);
    // 直调 `dispatch_locked` 跳过 `handle` 的 authfile 鉴权门：本例的被测判据要求对端 uid == cfg 属主
    // （非 root），而可信 authfile 必须 root 属主 + 无 group/other 位，非特权测试进程造不出；鉴权门本身
    // 由 unauthorized_uid_rejected_for_status / root_always_authorized 与 auth 模块判据测试独立覆盖。
    let cred = PeerCred {
        uid: self_uid,
        gid: self_uid,
    };
    dispatch_locked(&mut state, &deps, &cred, cmd::START, &mut conn);
    let writes = conn.writes();
    assert_eq!(writes.len(), 1);
    assert!(
        matches!(
            Response::parse(&writes[0]),
            Response::Ok(ResponseKind::Start(Start::StartedTimed { pid: 7777, .. }))
        ),
        "生产形态被误拒: {writes:?}"
    );
    let calls = spawner.spawn_calls.lock().unwrap();
    assert_eq!(calls[0].log.as_deref(), Some(log.as_path()));
}

#[test]
fn start_already_when_child_present() {
    let (_dir, auth, core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let mut state = HandlerState::new();
    state.child = Some(ManagedChild::Running(CoreHandle::new(8888)));
    let state = Mutex::new(state);
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let mut conn = MockConn::new(vec!["start", &sb, "/tmp/c.json", "", "0", ""]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK already 8888"]);
    // 已有 child → 不再 spawn。
    assert!(spawner.spawn_calls.lock().unwrap().is_empty());
}

#[test]
fn start_blocker_precedes_already_validation_forwarding_and_spawn() {
    let (_dir, auth, core_dir) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    *spawner.start_admission.lock().unwrap() = Some(StartAdmission::Blocked {
        pid: 7007,
        state: ReapBlockerState::Unknown,
    });
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd_called = Arc::new(StdMutex::new(Vec::new()));
    let fwd = {
        let calls = Arc::clone(&fwd_called);
        move |on| calls.lock().unwrap().push(on)
    };
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let state = Mutex::new(HandlerState {
        child: Some(ManagedChild::Running(CoreHandle::new(8888))),
    });
    // Deliberately invalid arguments: admission must run before validation too.
    let mut conn = MockConn::new(vec![lcmd::START_REAP_SAFE, "/wrong", "", "", "1", ""]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), ["OK start-not-admitted unknown 7007"]);
    assert!(spawner.spawn_calls.lock().unwrap().is_empty());
    assert!(fwd_called.lock().unwrap().is_empty());
    assert_eq!(
        state
            .lock()
            .unwrap()
            .child
            .as_ref()
            .map(|child| child.handle().pid),
        Some(8888)
    );
}

#[test]
fn unknown_command_is_rejected_before_arguments_or_child_mutation() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(9001);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd_called = Arc::new(StdMutex::new(Vec::new()));
    let fwd = {
        let calls = Arc::clone(&fwd_called);
        move |on| calls.lock().unwrap().push(on)
    };
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let state = Mutex::new(HandlerState {
        child: Some(ManagedChild::Running(CoreHandle::new(9001))),
    });
    let mut conn = MockConn::new(vec!["future-mutating-command", "9001"]);

    handle(&state, &deps, &mut conn);

    assert_eq!(conn.writes(), ["ERR unknown"]);
    assert!(spawner.spawn_calls.lock().unwrap().is_empty());
    assert!(spawner.terminate_calls.lock().unwrap().is_empty());
    assert!(spawner.kill_calls.lock().unwrap().is_empty());
    assert!(fwd_called.lock().unwrap().is_empty());
    assert_eq!(
        state
            .lock()
            .unwrap()
            .child
            .as_ref()
            .map(|child| child.handle().pid),
        Some(9001)
    );
}

#[test]
fn start_failure_reports_err_start_and_resets_forward() {
    let (dir, auth, core_dir) = setup_env();
    let self_uid = nix::unistd::getuid().as_raw();
    let peer = StaticPeerCred::new(self_uid, self_uid);
    let spawner = MockSpawner {
        next_pid: 0,
        fail: true,
        spawn_calls: StdMutex::new(Vec::new()),
        terminate_calls: StdMutex::new(Vec::new()),
        kill_calls: StdMutex::new(Vec::new()),
        start_admission: StdMutex::new(None),
        birth_admission: StdMutex::new(None),
        terminate_results: StdMutex::new(std::collections::VecDeque::new()),
        birth_stop_results: StdMutex::new(std::collections::VecDeque::new()),
        birth_stop_calls: StdMutex::new(Vec::new()),
        reaped_births: StdMutex::new(Vec::new()),
    };
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd_called = Arc::new(StdMutex::new(Vec::new()));
    let fwd = {
        let fc = Arc::clone(&fwd_called);
        move |on: bool| fc.lock().unwrap().push(on)
    };
    let deps = make_deps(
        Some(&core_dir),
        &auth,
        &peer,
        &spawner,
        &fp,
        &systemd,
        &ss,
        &fwd,
    );
    let cfg = dir.path().join("cfg.json");
    std::fs::write(&cfg, b"{}").unwrap();
    let mut state = HandlerState::new();
    let sb = core_dir.join("sing-box").to_string_lossy().into_owned();
    let cfg_s = cfg.to_string_lossy().into_owned();
    let mut conn = MockConn::new(vec![&sb, &cfg_s, "", "1", ""]);
    // 直调 `dispatch_locked` 跳过 `handle` 的 authfile 鉴权门：本例的被测判据要求对端 uid == cfg 属主
    // （非 root），而可信 authfile 必须 root 属主 + 无 group/other 位，非特权测试进程造不出；鉴权门本身
    // 由 unauthorized_uid_rejected_for_status / root_always_authorized 与 auth 模块判据测试独立覆盖。
    let cred = PeerCred {
        uid: self_uid,
        gid: self_uid,
    };
    dispatch_locked(&mut state, &deps, &cred, cmd::START, &mut conn);
    let w = &conn.writes()[0];
    assert!(w.starts_with("ERR start"), "got {w}");
    // fwd=1 先设，spawn 失败后复位为 false。
    assert_eq!(*fwd_called.lock().unwrap(), vec![true, false]);
}

// ===== cleanup =====

#[test]
fn cleanup_kills_child_and_reports_cleaned() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps(None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd);
    let mut state = HandlerState::new();
    state.child = Some(ManagedChild::Running(CoreHandle::new(333)));
    let state = Mutex::new(state);
    let mut conn = MockConn::new(vec!["cleanup"]);
    handle(&state, &deps, &mut conn);
    assert_eq!(conn.writes(), vec!["OK cleaned"]);
    assert_eq!(*spawner.kill_calls.lock().unwrap(), vec![333]);
    assert!(matches!(
        state.lock().unwrap().child.as_ref(),
        Some(ManagedChild::Stopping(_))
    ));
}

#[test]
fn resolved_dns_commands_are_authorized_and_typed() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let resolved = MockResolvedDns::default();
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps_with_resolved(
        None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd, &resolved,
    );
    let state = Mutex::new(HandlerState::new());

    let mut set = MockConn::new(vec![
        lcmd::RESOLVED_DNS_SET,
        polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME,
        polaris_helper_proto::linux_dns::CONTROLLED_DNS_IP,
    ]);
    handle(&state, &deps, &mut set);
    assert_eq!(set.writes(), ["OK resolved-dns-set"]);

    let mut revert = MockConn::new(vec![
        lcmd::RESOLVED_DNS_REVERT,
        polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME,
    ]);
    handle(&state, &deps, &mut revert);
    assert_eq!(revert.writes(), ["OK resolved-dns-reverted"]);
    assert_eq!(resolved.calls.lock().unwrap().len(), 2);
}

#[test]
fn resolved_dns_failure_keeps_structured_error_detail() {
    let (_dir, auth, _core) = setup_env();
    let peer = StaticPeerCred::new(0, 0);
    let spawner = MockSpawner::succeeding(100);
    let fp = MockFreePort::empty();
    let systemd = MockSystemd::default();
    let resolved = MockResolvedDns {
        error: Some("read-back failed;\npartial state reverted".into()),
        ..Default::default()
    };
    let ss = no_op_ss();
    let fwd = no_op_fwd();
    let deps = make_deps_with_resolved(
        None, &auth, &peer, &spawner, &fp, &systemd, &ss, &fwd, &resolved,
    );
    let state = Mutex::new(HandlerState::new());
    let mut conn = MockConn::new(vec![
        lcmd::RESOLVED_DNS_SET,
        polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME,
        polaris_helper_proto::linux_dns::CONTROLLED_DNS_IP,
    ]);
    handle(&state, &deps, &mut conn);
    assert_eq!(
        conn.writes(),
        ["ERR resolved-dns read-back failed; partial state reverted"]
    );
}

// ===== wire 响应形态锁住（对照 Go 源每个 Fprintln/Fprintf）=====

#[test]
fn wire_forms_match_go_source() {
    // v1 是 wire 断代真值；build 字段是尾部向后兼容扩展（旧 app 忽略）。
    assert_eq!(PROTO_VERSION, 1);
    assert_eq!(
        Response::Ok(ResponseKind::Pong(polaris_helper_proto::Pong::current(0))).to_wire_line(),
        format!(
            "OK pong uid=0 v1 build={}",
            polaris_helper_proto::build_identity::current()
        )
    );
    assert_eq!(format!("OK {PROTO_VERSION}"), "OK 1");
    assert_eq!("OK stopped", "OK stopped");
    assert_eq!("OK running 12345", "OK running 12345");
    assert_eq!("OK notrunning", "OK notrunning");
    assert_eq!("OK stopped 12345", "OK stopped 12345");
    assert_eq!("OK already 12345", "OK already 12345");
    assert_eq!("OK started 12345", "OK started 12345");
    assert_eq!("OK cleaned", "OK cleaned");
    assert_eq!("OK free", "OK free");
    assert_eq!("OK killed 123,456", "OK killed 123,456");
    assert_eq!("OK foreign a | b", "OK foreign a | b");
    assert_eq!("OK installed", "OK installed");
    assert_eq!("ERR peercred", "ERR peercred");
    assert_eq!("ERR unauthorized", "ERR unauthorized");
    assert_eq!("ERR unknown", "ERR unknown");
    assert_eq!("ERR bad-port", "ERR bad-port");
    assert_eq!("ERR bad-args", "ERR bad-args");
    assert_eq!("ERR core-missing", "ERR core-missing");
    assert_eq!(
        "ERR core-path-denied (want /x)",
        "ERR core-path-denied (want /x)"
    );
    assert_eq!("ERR config-not-owned", "ERR config-not-owned");
    assert_eq!("ERR coredir-unset", "ERR coredir-unset");
    assert_eq!("ERR start boom", "ERR start boom");
}
