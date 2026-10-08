//! Leftover login-core cleanup. Nothing here signals a real process: the process scan and
//! the signal function are fakes, and the only files touched live in a per-test directory.

use super::super::stale::{
    is_stale_login_core, login_config_identity, owned_by_this_user, stale_login_config_names,
    stale_login_core_pids, sweep_stale_login_cores, StaleLoginCoreSweeper, StaleSweepIo,
    STALE_LOGIN_CORE_ALIVE,
};
use super::*;
use polaris_core_supervisor::{CoreProcess, Signal};

const BINARY: &str = "/opt/Polaris/core/sing-box";
const CONFIG_DIR: &str = "/home/u/.config/polaris";

fn argv(pid: u32, args: &[&str]) -> CoreProcess {
    CoreProcess {
        pid,
        cmdline: args.iter().map(|arg| (*arg).to_owned()).collect(),
        raw: String::new(),
    }
}

fn raw(pid: u32, line: &str) -> CoreProcess {
    CoreProcess {
        pid,
        cmdline: Vec::new(),
        raw: line.to_owned(),
    }
}

fn login_core(pid: u32) -> CoreProcess {
    argv(
        pid,
        &[
            BINARY,
            "run",
            "-c",
            "/home/u/.config/polaris/tailscale-login-ts1-3.json",
            "--disable-color",
        ],
    )
}

#[test]
fn login_config_identity_accepts_only_the_exact_file_name_shape() {
    assert_eq!(
        login_config_identity("tailscale-login-ts1-3.json"),
        Some(("ts1", 3))
    );
    assert_eq!(
        login_config_identity("tailscale-login-a_b-c-12.json"),
        Some(("a_b-c", 12))
    );
    for foreign in [
        "tailscale-login-preexisting.json",
        "tailscale-login-ts1-.json",
        "tailscale-login--3.json",
        "tailscale-login-ts1-3.json.tmp",
        "tailscale-login-ts1-3x.json",
        "tailscale-login-ts 1-3.json",
        "tailscale-login-ts1-3",
        "config.json",
        "xtailscale-login-ts1-3.json",
    ] {
        assert_eq!(login_config_identity(foreign), None, "{foreign}");
    }
}

#[test]
fn stale_login_config_names_keep_registered_and_foreign_files() {
    let dir = Path::new(CONFIG_DIR);
    let live = vec![dir.join("tailscale-login-ts1-3.json")];
    let names = [
        "tailscale-login-ts1-3.json",
        "tailscale-login-ts1-1.json",
        "tailscale-login-ts2-9.json",
        "config.json",
        "tailscale-login-preexisting.json",
    ];
    assert_eq!(
        stale_login_config_names(names, dir, &live),
        vec!["tailscale-login-ts1-1.json", "tailscale-login-ts2-9.json"]
    );
}

#[test]
fn only_this_apps_login_core_argv_is_a_stale_login_core() {
    let binary = Path::new(BINARY);
    let dir = Path::new(CONFIG_DIR);
    assert!(is_stale_login_core(&login_core(1), binary, dir));
    assert!(is_stale_login_core(
        &raw(
            2,
            "/Library/Application Support/Polaris/core/sing-box run -c /Users/u/Library/Application Support/polaris/tailscale-login-ts1-3.json --disable-color"
        ),
        Path::new("/Library/Application Support/Polaris/core/sing-box"),
        Path::new("/Users/u/Library/Application Support/polaris"),
    ));
    let config = "/home/u/.config/polaris/tailscale-login-ts1-3.json";
    for (why, process) in [
        (
            "the main core of this app",
            argv(3, &[BINARY, "run", "-c", "/home/u/.config/polaris/config.json", "--disable-color"]),
        ),
        (
            "a temporary speed-test core of this app",
            argv(4, &[BINARY, "run", "-c", "/home/u/.config/polaris/speedtest-1.json"]),
        ),
        (
            "a system sing-box running a same-named config",
            argv(5, &["/usr/bin/sing-box", "run", "-c", config]),
        ),
        (
            "our binary with a login-shaped config in another directory",
            argv(6, &[BINARY, "run", "-c", "/tmp/tailscale-login-ts1-3.json"]),
        ),
        (
            "our binary checking, not running, the config",
            argv(7, &[BINARY, "check", "-c", config]),
        ),
        (
            "a pager that merely opened the config",
            argv(8, &["/usr/bin/less", config]),
        ),
        (
            "a wrapper that mentions the same argv later",
            argv(9, &["/bin/sh", BINARY, "run", "-c", config]),
        ),
        (
            "a raw line for another binary",
            raw(10, "/usr/bin/sing-box run -c /home/u/.config/polaris/tailscale-login-ts1-3.json"),
        ),
        (
            "a raw line for a sibling directory with the same prefix",
            raw(11, "/opt/Polaris/core/sing-box run -c /home/u/.config/polaris-other/tailscale-login-ts1-3.json"),
        ),
        (
            "a raw line whose config name only starts like ours",
            raw(12, "/opt/Polaris/core/sing-box run -c /home/u/.config/polaris/tailscale-login-ts1-3.json.bak"),
        ),
        ("an empty process row", raw(13, "")),
    ] {
        assert!(!is_stale_login_core(&process, binary, dir), "{why}");
    }
}

#[test]
fn registered_children_are_never_stale_login_core_pids() {
    let candidates = [
        login_core(10),
        login_core(11),
        argv(12, &[BINARY, "run", "-c", "/x/config.json"]),
    ];
    assert_eq!(
        stale_login_core_pids(&candidates, Path::new(BINARY), Path::new(CONFIG_DIR), &[11]),
        vec![10]
    );
}

/// Scripted process table: each scan pops the next snapshot (the last one repeats).
struct FakeProcesses {
    scans: Mutex<Vec<Vec<CoreProcess>>>,
    signals: Mutex<Vec<(u32, Signal)>>,
    /// pid → owner; a pid that is absent runs as this user ([`ME`]).
    owners: HashMap<u32, Option<u32>>,
}

const ME: u32 = 1000;

impl FakeProcesses {
    fn new(scans: Vec<Vec<CoreProcess>>) -> Self {
        Self {
            scans: Mutex::new(scans),
            signals: Mutex::new(Vec::new()),
            owners: HashMap::new(),
        }
    }
    fn owned(mut self, pid: u32, owner: Option<u32>) -> Self {
        self.owners.insert(pid, owner);
        self
    }
    fn scan(&self) -> Vec<CoreProcess> {
        let mut scans = self.scans.lock().unwrap();
        if scans.len() > 1 {
            scans.remove(0)
        } else {
            scans[0].clone()
        }
    }
    async fn sweep(&self, inflight: &[u32]) -> Result<usize, String> {
        sweep_stale_login_cores(
            &StaleSweepIo {
                scan: &|| self.scan(),
                owner: &|pid| self.owners.get(&pid).copied().unwrap_or(Some(ME)),
                signal: &|pid, signal| self.signals.lock().unwrap().push((pid, signal)),
            },
            Some(ME),
            Duration::ZERO,
            Path::new(BINARY),
            Path::new(CONFIG_DIR),
            inflight,
        )
        .await
    }
    fn signals(&self) -> Vec<(u32, Signal)> {
        self.signals.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn sweep_signals_only_identified_leftovers_and_reports_them_ended() {
    let main = argv(
        3,
        &[BINARY, "run", "-c", "/home/u/.config/polaris/config.json"],
    );
    let processes = FakeProcesses::new(vec![
        vec![login_core(10), login_core(11), main.clone()],
        vec![main],
    ]);
    assert_eq!(processes.sweep(&[11]).await, Ok(1));
    assert_eq!(processes.signals(), vec![(10, Signal::Sigterm)]);
}

#[tokio::test]
async fn sweep_without_leftovers_signals_nothing() {
    let processes = FakeProcesses::new(vec![vec![argv(
        3,
        &[BINARY, "run", "-c", "/home/u/.config/polaris/config.json"],
    )]]);
    assert_eq!(processes.sweep(&[]).await, Ok(0));
    assert!(processes.signals().is_empty());
}

#[tokio::test]
async fn sweep_forces_only_a_pid_that_still_carries_the_same_argv() {
    // pid 10 ignores SIGTERM; pid 11 exited and its number now belongs to an unrelated process.
    let reused = argv(11, &["/usr/bin/vim", "notes.txt"]);
    let processes = FakeProcesses::new(vec![
        vec![login_core(10), login_core(11)],
        vec![login_core(10), reused.clone()],
        vec![reused],
    ]);
    assert_eq!(processes.sweep(&[]).await, Ok(2));
    assert_eq!(
        processes.signals(),
        vec![
            (10, Signal::Sigterm),
            (11, Signal::Sigterm),
            (10, Signal::Sigkill)
        ]
    );
}

#[tokio::test]
async fn sweep_reports_a_leftover_that_survives_both_signals() {
    let processes = FakeProcesses::new(vec![vec![login_core(10)]]);
    assert_eq!(
        processes.sweep(&[]).await,
        Err(STALE_LOGIN_CORE_ALIVE.to_owned())
    );
    assert_eq!(
        processes.signals(),
        vec![(10, Signal::Sigterm), (10, Signal::Sigkill)]
    );
}

#[test]
fn only_a_process_of_this_user_with_a_readable_owner_is_ours() {
    assert!(owned_by_this_user(Some(1000), Some(1000)));
    assert!(!owned_by_this_user(Some(0), Some(1000)));
    assert!(!owned_by_this_user(Some(1001), Some(1000)));
    assert!(!owned_by_this_user(None, Some(1000)));
    assert!(!owned_by_this_user(Some(1000), None));
    assert!(!owned_by_this_user(None, None));
}

#[tokio::test]
async fn another_users_same_shaped_core_is_neither_signalled_nor_a_reason_to_refuse() {
    // pid 10 is root's, pid 11's owner cannot be read; neither ever exits. pid 12 is ours.
    let processes = FakeProcesses::new(vec![
        vec![login_core(10), login_core(11), login_core(12)],
        vec![login_core(10), login_core(11)],
    ])
    .owned(10, Some(0))
    .owned(11, None);
    assert_eq!(processes.sweep(&[]).await, Ok(1));
    assert_eq!(processes.signals(), vec![(12, Signal::Sigterm)]);

    let foreign_only = FakeProcesses::new(vec![vec![login_core(10)]]).owned(10, Some(0));
    assert_eq!(foreign_only.sweep(&[]).await, Ok(0));
    assert!(foreign_only.signals().is_empty());
}

/// Records what the registry knew when it asked for the sweep.
struct RecordingSweeper {
    fail: bool,
    calls: Mutex<Vec<(PathBuf, Vec<u32>, usize)>>,
    spawned: Arc<AtomicUsize>,
}

#[async_trait]
impl StaleLoginCoreSweeper for RecordingSweeper {
    async fn sweep(&self, binary: &Path, inflight: &[u32]) -> Result<usize, String> {
        self.calls.lock().unwrap().push((
            binary.to_path_buf(),
            inflight.to_vec(),
            self.spawned.load(Ordering::SeqCst),
        ));
        if self.fail {
            Err(STALE_LOGIN_CORE_ALIVE.into())
        } else {
            Ok(1)
        }
    }
}

fn reg_with_sweeper(fail: bool) -> (LoginCoreRegistry, Arc<FakeSpawner>, Arc<RecordingSweeper>) {
    let spawner = fake_spawner(vec![], false, false);
    let sweeper = Arc::new(RecordingSweeper {
        fail,
        calls: Mutex::new(Vec::new()),
        spawned: spawner.count.clone(),
    });
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    )
    .with_stale_login_sweeper(sweeper.clone());
    (reg, spawner, sweeper)
}

#[tokio::test]
async fn login_admission_sweeps_leftover_cores_before_it_spawns() {
    let (reg, spawner, sweeper) = reg_with_sweeper(false);
    let ud = temp_ud();
    reg.register_inflight_for_test("other", 777);
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    assert_eq!(
        *sweeper.calls.lock().unwrap(),
        vec![(PathBuf::from("/fake/sing-box"), vec![777], 0)],
        "the sweep runs once, before the spawn, and is told the registered children"
    );
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    reg.cancel_login("ts1").await.unwrap();
    let _ = std::fs::remove_dir_all(&ud);
}

#[tokio::test]
async fn login_admission_is_refused_while_a_leftover_core_survives() {
    let (reg, spawner, sweeper) = reg_with_sweeper(true);
    let ud = temp_ud();
    let outcome = reg
        .start_login(
            &ts_server("ts1", "myts"),
            &ud,
            false,
            None,
            0,
            Arc::new(FakeEmitter::default()),
        )
        .await;
    assert!(
        matches!(&outcome, StartLoginOutcome::Failed(reason) if reason == STALE_LOGIN_CORE_ALIVE)
    );
    assert_eq!(sweeper.calls.lock().unwrap().len(), 1);
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    assert!(!reg.shared.contains("ts1"));
    assert!(login_configs(&ud).is_empty());
    let _ = std::fs::remove_dir_all(&ud);
}

#[tokio::test]
async fn logout_sweeps_leftover_cores_before_it_rewrites_state() {
    let (reg, _spawner, sweeper) = reg_with_sweeper(false);
    let swept_before_delete = AtomicUsize::new(usize::MAX);
    assert!(reg
        .logout("ts1", &|| false, None, |_| {
            swept_before_delete.store(sweeper.calls.lock().unwrap().len(), Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap());
    assert_eq!(swept_before_delete.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn logout_is_refused_while_a_leftover_core_survives() {
    let (reg, _spawner, sweeper) = reg_with_sweeper(true);
    let error = reg
        .logout("ts1", &|| false, None, |_| {
            panic!("state must not be rewritten while a leftover writer lives")
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), STALE_LOGIN_CORE_ALIVE);
    assert_eq!(error.kind(), std::io::ErrorKind::ResourceBusy);
    assert_eq!(sweeper.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn login_removes_leftover_configs_and_no_longer_collides_with_one() {
    let spawner = fake_spawner(vec![], false, false);
    let reg = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    let ud = temp_ud();
    // A fresh registry restarts its numbering, so the killed process's files carry the very
    // names this one is about to create exclusively. Cover every number the fixture can draw.
    let colliding: Vec<PathBuf> = (1..=4)
        .map(|epoch| ud.join(format!("tailscale-login-ts1-{epoch}.json")))
        .collect();
    let other = ud.join("tailscale-login-gone-7.json");
    let foreign = ud.join("tailscale-login-preexisting.json");
    let unrelated = ud.join("config.json");
    for path in colliding.iter().chain([&other, &foreign, &unrelated]) {
        std::fs::write(path, b"leftover").unwrap();
    }
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    let live: Vec<&PathBuf> = colliding.iter().filter(|path| path.exists()).collect();
    assert_eq!(
        live.len(),
        1,
        "exactly the new login's config remains: {live:?}"
    );
    assert_ne!(std::fs::read(live[0]).unwrap(), b"leftover");
    assert!(
        !other.exists(),
        "a leftover of a node that no longer logs in is removed"
    );
    assert_eq!(std::fs::read(&foreign).unwrap(), b"leftover");
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"leftover");

    // The registered login's own config survives the next admission's sweep.
    std::fs::remove_file(&foreign).unwrap();
    started(&reg, &ud, &ts_server("ts2", "second")).await;
    assert!(
        live[0].exists(),
        "a registered login's config must not be swept"
    );
    assert_eq!(login_configs(&ud).len(), 2);
    reg.cancel_login("ts1").await.unwrap();
    reg.cancel_login("ts2").await.unwrap();
    let _ = std::fs::remove_dir_all(&ud);
}
