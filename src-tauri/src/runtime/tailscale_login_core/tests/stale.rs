//! Leftover login-core cleanup. Nothing here signals a real process: the process scan and
//! the signal function are fakes, and the only files touched live in a per-test directory.

use super::super::stale::{
    is_stale_login_core, login_config_identity, owned_by_this_user, stale_login_config_names,
    stale_login_core_pids, sweep_stale_login_cores, sweep_windows_stale_login_cores,
    StaleLoginCoreSweeper, StaleSweepIo, WindowsStaleSweepIo, STALE_LOGIN_CORE_ALIVE,
};
use super::*;
use polaris_core_supervisor::{CoreProcess, ProcessOwner, Signal};

const BINARY: &str = "/opt/Polaris/core/sing-box";
const CONFIG_DIR: &str = "/home/u/.config/polaris";

fn argv(pid: u32, args: &[&str]) -> CoreProcess {
    CoreProcess {
        pid,
        cmdline: args.iter().map(|arg| (*arg).to_owned()).collect(),
        ..CoreProcess::default()
    }
}

fn raw(pid: u32, line: &str) -> CoreProcess {
    CoreProcess {
        pid,
        raw: line.to_owned(),
        ..CoreProcess::default()
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
    // Where the core binary lives does not matter: a leftover from an earlier session ran from
    // another mount point or install directory, yet it runs this app's own login config.
    for program in [
        "/tmp/.mount_PolarisOld/usr/lib/Polaris/_up_/resources/linux/sing-box",
        "/home/u/.config/polaris/core_update/sing-box",
        "/usr/bin/sing-box",
    ] {
        assert!(
            is_stale_login_core(&argv(20, &[program, "run", "-c", config]), binary, dir),
            "{program}"
        );
    }
    assert!(is_stale_login_core(
        &raw(
            21,
            "/opt/elsewhere/sing-box run -c /home/u/.config/polaris/tailscale-login-ts1-3.json"
        ),
        binary,
        dir
    ));
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
            "a system sing-box running a same-named config of its own",
            argv(5, &["/usr/bin/sing-box", "run", "-c", "/etc/sing-box/tailscale-login-ts1-3.json"]),
        ),
        (
            "a program that is not named like the core",
            argv(14, &["/usr/bin/not-sing-box", "run", "-c", config]),
        ),
        (
            "a core named by a relative path",
            argv(15, &["sing-box", "run", "-c", config]),
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
            "a raw line for a program that is not named like the core",
            raw(10, "/usr/bin/sing-box-wrapper run -c /home/u/.config/polaris/tailscale-login-ts1-3.json"),
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

/// Fake process numbers must never accidentally equal the test runner's own pid.
fn windows_test_pid(slot: u32) -> u32 {
    let pid = u32::MAX - slot;
    if pid == std::process::id() {
        slot + 1
    } else {
        pid
    }
}

/// The raw field is an opaque birth label only in this fake. Its argv/pid may be identical
/// to a later birth; the callbacks must still receive the original object, not rediscover it.
fn held_login_core(pid: u32, birth: &str) -> CoreProcess {
    let mut process = login_core(pid);
    process.owner = ProcessOwner::SameUser;
    process.raw = birth.to_owned();
    process
}

struct WindowsProcesses {
    snapshots: Mutex<Vec<Vec<CoreProcess>>>,
    scans: AtomicUsize,
    observations: Mutex<HashMap<String, Vec<Option<bool>>>>,
    observed: Mutex<Vec<String>>,
    ends: Mutex<Vec<String>>,
    accepts_end: bool,
    ended: tokio::sync::Notify,
}

impl WindowsProcesses {
    fn new(processes: Vec<CoreProcess>) -> Self {
        Self {
            snapshots: Mutex::new(vec![processes]),
            scans: AtomicUsize::new(0),
            observations: Mutex::new(HashMap::new()),
            observed: Mutex::new(Vec::new()),
            ends: Mutex::new(Vec::new()),
            accepts_end: true,
            ended: tokio::sync::Notify::new(),
        }
    }

    fn observed_as(mut self, birth: &str, states: Vec<Option<bool>>) -> Self {
        assert!(!states.is_empty());
        self.observations
            .get_mut()
            .unwrap()
            .insert(birth.to_owned(), states);
        self
    }

    fn scan(&self) -> Vec<CoreProcess> {
        self.scans.fetch_add(1, Ordering::SeqCst);
        let mut snapshots = self.snapshots.lock().unwrap();
        if snapshots.len() > 1 {
            snapshots.remove(0)
        } else {
            snapshots[0].clone()
        }
    }

    fn running(&self, process: &CoreProcess) -> Option<bool> {
        self.observed.lock().unwrap().push(process.raw.clone());
        let mut observations = self.observations.lock().unwrap();
        let Some(states) = observations.get_mut(&process.raw) else {
            return Some(true);
        };
        if states.len() > 1 {
            states.remove(0)
        } else {
            states[0]
        }
    }

    fn end(&self, process: &CoreProcess) -> bool {
        self.ends.lock().unwrap().push(process.raw.clone());
        self.ended.notify_one();
        self.accepts_end
    }

    async fn sweep(&self, inflight: &[u32], grace: Duration) -> Result<usize, String> {
        self.sweep_for(Path::new(BINARY), inflight, grace).await
    }

    async fn sweep_for(
        &self,
        binary: &Path,
        inflight: &[u32],
        grace: Duration,
    ) -> Result<usize, String> {
        sweep_windows_stale_login_cores(
            &WindowsStaleSweepIo {
                scan: &|| self.scan(),
                end: &|process| self.end(process),
                running: &|process| self.running(process),
            },
            grace,
            binary,
            Path::new(CONFIG_DIR),
            inflight,
        )
        .await
    }

    fn ends(&self) -> Vec<String> {
        self.ends.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn windows_sweep_admits_only_proven_same_user_and_excludes_live_registered_pids() {
    let mut unknown = held_login_core(windows_test_pid(21), "unknown");
    unknown.owner = ProcessOwner::Unknown;
    let mut other = held_login_core(windows_test_pid(22), "other");
    other.owner = ProcessOwner::Other;
    let mut elevated = held_login_core(windows_test_pid(23), "system");
    elevated.owner = ProcessOwner::Elevated;
    let processes = WindowsProcesses::new(vec![
        held_login_core(windows_test_pid(20), "ours"),
        unknown,
        other,
        elevated,
        held_login_core(windows_test_pid(24), "registered"),
        held_login_core(0, "zero"),
        held_login_core(std::process::id(), "self"),
    ])
    .observed_as("ours", vec![Some(true), Some(false)]);
    assert_eq!(
        processes.sweep(&[windows_test_pid(24)], Duration::ZERO).await,
        Ok(1)
    );
    assert_eq!(processes.ends(), vec!["ours"]);
    assert_eq!(processes.scans.load(Ordering::SeqCst), 1);
    assert!(processes
        .observed
        .lock()
        .unwrap()
        .iter()
        .all(|birth| birth == "ours"));
}

#[tokio::test]
async fn windows_unknown_other_and_system_only_are_not_a_global_owner_receipt() {
    for owner in [
        ProcessOwner::Unknown,
        ProcessOwner::Other,
        ProcessOwner::Elevated,
    ] {
        let mut process = held_login_core(windows_test_pid(20), "unselected");
        process.owner = owner;
        let processes = WindowsProcesses::new(vec![process]);
        assert_eq!(processes.sweep(&[], Duration::ZERO).await, Ok(0));
        assert!(processes.ends().is_empty());
        assert!(processes.observed.lock().unwrap().is_empty());
    }
}

#[test]
fn windows_login_shape_uses_the_existing_exact_path_and_name_matcher() {
    const WIN_DIR: &str = r"C:\Users\test user\AppData\Roaming\Polaris";
    const WIN_CORE: &str = r"C:\Program Files\Polaris\sing-box.exe";
    const WIN_CONFIG: &str =
        r"C:\Users\test user\AppData\Roaming\Polaris\tailscale-login-ts1-3.json";
    // Only the filename is consumed here. A full Windows binary path would be parsed by the
    // host std::path in is_stale_login_core, so keep these pure fixtures portable to Linux.
    let binary = Path::new("sing-box.exe");
    let dir = Path::new(WIN_DIR);
    for program in [
        WIN_CORE,
        r"D:\Old Polaris\sing-box.exe",
        r"\\old-server\old install\sing-box.exe",
        r"C:\OTHER\SING-BOX.EXE",
    ] {
        assert!(is_stale_login_core(
            &argv(20, &[program, "run", "-c", WIN_CONFIG]),
            binary,
            dir,
        ));
    }
    assert!(is_stale_login_core(
        &argv(
            20,
            &[
                WIN_CORE,
                "run",
                "-c",
                "c:/users/TEST USER/appdata/roaming/POLARIS/tailscale-login-ts1-3.json",
            ],
        ),
        binary,
        dir,
    ));
    assert!(is_stale_login_core(
        &argv(
            20,
            &[
                r"\\server\bin\sing-box.exe",
                "run",
                "-c",
                r"\\server\share\Polaris\tailscale-login-ts1-3.json",
            ],
        ),
        binary,
        Path::new(r"\\server\share\Polaris"),
    ));
    for config in [
        r"C:\Users\test user\AppData\Roaming\Polaris\config.json",
        r"C:\Users\test user\AppData\Roaming\Polaris\speedtest-1.json",
        r"C:\Users\test user\AppData\Roaming\Polaris\sub\tailscale-login-ts1-3.json",
        r"C:\Users\test user\AppData\Roaming\Polaris-other\tailscale-login-ts1-3.json",
        r"C:\Users\test user\AppData\Roaming\Polaris\..\Polaris\tailscale-login-ts1-3.json",
        r"C:\Users\test user\AppData\Roaming\Polaris\tailscale-login-ts1-3.json.bak",
        r"C:\Users\test user\AppData\Roaming\Polaris\tailscale-login-ts1-3.json:stream",
        r"C:\Users\test user\AppData\Roaming\Polaris\tailscale-login-ts1-18446744073709551616.json",
        r"C:\Users\test user\AppData\Roaming\Polaris\tailscale-login-ts1-noepoch.json",
    ] {
        assert!(
            !is_stale_login_core(&argv(20, &[WIN_CORE, "run", "-c", config]), binary, dir),
            "{config}"
        );
    }
    for args in [
        vec!["sing-box.exe", "run", "-c", WIN_CONFIG],
        vec![r"C:sing-box.exe", "run", "-c", WIN_CONFIG],
        vec![WIN_CORE, "check", "-c", WIN_CONFIG],
        vec![r"C:\Windows\cmd.exe", WIN_CORE, "run", "-c", WIN_CONFIG],
    ] {
        assert!(!is_stale_login_core(&argv(20, &args), binary, dir));
    }
}

#[cfg(windows)]
#[test]
fn windows_login_shape_accepts_the_resolved_native_binary_path() {
    assert!(is_stale_login_core(
        &argv(
            20,
            &[
                r"C:\Old Polaris\sing-box.exe",
                "run",
                "-c",
                r"C:\Users\u\Polaris\tailscale-login-ts1-3.json",
            ],
        ),
        Path::new(r"D:\New Polaris\sing-box.exe"),
        Path::new(r"C:\Users\u\Polaris"),
    ));
}

#[tokio::test]
async fn windows_sweep_does_not_end_same_user_non_login_shapes() {
    let mut main = held_login_core(windows_test_pid(20), "main");
    main.cmdline[3] = format!("{CONFIG_DIR}/config.json");
    let mut speedtest = held_login_core(windows_test_pid(21), "speedtest");
    speedtest.cmdline[3] = format!("{CONFIG_DIR}/speedtest-1.json");
    let mut subdir = held_login_core(windows_test_pid(22), "subdir");
    subdir.cmdline[3] = format!("{CONFIG_DIR}/sub/tailscale-login-ts1-3.json");
    let processes = WindowsProcesses::new(vec![main, speedtest, subdir]);
    assert_eq!(processes.sweep(&[], Duration::ZERO).await, Ok(0));
    assert!(processes.ends().is_empty());
    assert!(processes.observed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn windows_sweep_retries_only_an_original_handle_that_is_still_running() {
    let processes = WindowsProcesses::new(vec![
        held_login_core(windows_test_pid(20), "slow"),
        held_login_core(windows_test_pid(21), "quick"),
    ])
    .observed_as("slow", vec![Some(true), Some(true), Some(false)])
    .observed_as("quick", vec![Some(true), Some(false)]);
    assert_eq!(processes.sweep(&[], Duration::ZERO).await, Ok(2));
    assert_eq!(processes.ends(), vec!["slow", "quick", "slow"]);
}

#[tokio::test(start_paused = true)]
async fn windows_empty_selection_neither_waits_nor_requests_termination() {
    let processes = WindowsProcesses::new(Vec::new());
    let before = tokio::time::Instant::now();
    assert_eq!(processes.sweep(&[], Duration::from_secs(5)).await, Ok(0));
    assert_eq!(tokio::time::Instant::now(), before);
    assert!(processes.ends().is_empty());
    assert!(processes.observed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn windows_sweep_requires_exit_even_when_both_termination_requests_succeed() {
    let processes = WindowsProcesses::new(vec![held_login_core(windows_test_pid(20), "survives")]);
    assert_eq!(
        processes.sweep(&[], Duration::ZERO).await,
        Err(STALE_LOGIN_CORE_ALIVE.to_owned())
    );
    assert_eq!(processes.ends(), vec!["survives", "survives"]);
}

#[tokio::test]
async fn windows_sweep_uses_exit_observation_not_a_rejected_end_request() {
    // A failed request can mean the target already exited. Conversely a creation-time or
    // access-rights failure never permits admission while the original handle remains live.
    for (states, want) in [
        (vec![Some(true), Some(false)], Ok(1)),
        (vec![Some(true)], Err(STALE_LOGIN_CORE_ALIVE.to_owned())),
    ] {
        let mut processes =
            WindowsProcesses::new(vec![held_login_core(windows_test_pid(20), "original")])
                .observed_as("original", states);
        processes.accepts_end = false;
        assert_eq!(processes.sweep(&[], Duration::ZERO).await, want);
        assert!(!processes.ends().is_empty());
    }
}

#[tokio::test]
async fn windows_sweep_does_not_end_an_already_exited_original() {
    let processes = WindowsProcesses::new(vec![held_login_core(windows_test_pid(20), "exited")])
        .observed_as("exited", vec![Some(false)]);
    assert_eq!(processes.sweep(&[], Duration::ZERO).await, Ok(1));
    assert!(processes.ends().is_empty());
}

#[tokio::test]
async fn windows_sweep_missing_handle_never_becomes_death_or_a_pid_fallback() {
    let processes = WindowsProcesses::new(vec![held_login_core(windows_test_pid(20), "no-handle")])
        .observed_as("no-handle", vec![None]);
    assert_eq!(
        processes.sweep(&[], Duration::ZERO).await,
        Err(STALE_LOGIN_CORE_ALIVE.to_owned())
    );
    assert!(processes.ends().is_empty());
}

#[tokio::test]
async fn windows_sweep_partial_exit_is_an_overall_failure() {
    let processes = WindowsProcesses::new(vec![
        held_login_core(windows_test_pid(20), "done"),
        held_login_core(windows_test_pid(21), "unknown"),
    ])
    .observed_as("done", vec![Some(true), Some(false)])
    .observed_as("unknown", vec![None]);
    assert_eq!(
        processes.sweep(&[], Duration::ZERO).await,
        Err(STALE_LOGIN_CORE_ALIVE.to_owned())
    );
    assert_eq!(processes.ends(), vec!["done"]);
}

#[tokio::test]
async fn windows_sweep_does_not_rediscover_a_reused_pid_even_with_identical_argv() {
    let original = held_login_core(windows_test_pid(20), "birth-before");
    let replacement = held_login_core(windows_test_pid(20), "birth-after");
    assert_eq!(original.pid, replacement.pid);
    assert_eq!(original.cmdline, replacement.cmdline);
    let processes = WindowsProcesses::new(vec![original])
        .observed_as("birth-before", vec![Some(true), Some(false)]);
    processes.snapshots.lock().unwrap().push(vec![replacement]);
    assert_eq!(processes.sweep(&[], Duration::ZERO).await, Ok(1));
    assert_eq!(processes.scans.load(Ordering::SeqCst), 1);
    assert_eq!(processes.ends(), vec!["birth-before"]);
    assert!(processes
        .observed
        .lock()
        .unwrap()
        .iter()
        .all(|birth| birth == "birth-before"));
}

#[tokio::test(start_paused = true)]
async fn dropping_a_windows_sweep_cannot_send_a_late_retry() {
    let processes = Arc::new(WindowsProcesses::new(vec![held_login_core(
        windows_test_pid(20),
        "original",
    )]));
    let sweep = {
        let processes = processes.clone();
        tokio::spawn(async move { processes.sweep(&[], Duration::from_secs(5)).await })
    };
    processes.ended.notified().await;
    sweep.abort();
    assert!(sweep.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(20)).await;
    tokio::task::yield_now().await;
    assert_eq!(processes.ends(), vec!["original"]);
    assert_eq!(processes.scans.load(Ordering::SeqCst), 1);
}

struct WindowsRegistrySweeper {
    processes: Arc<WindowsProcesses>,
    grace: Duration,
}

#[async_trait]
impl StaleLoginCoreSweeper for WindowsRegistrySweeper {
    async fn sweep(&self, binary: &Path, inflight: &[u32]) -> Result<usize, String> {
        self.processes.sweep_for(binary, inflight, self.grace).await
    }
}

fn reg_with_windows_sweeper(
    processes: Arc<WindowsProcesses>,
    grace: Duration,
) -> (LoginCoreRegistry, Arc<FakeSpawner>) {
    let spawner = fake_spawner(vec![], false, false);
    let registry = reg_with(
        spawner.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    )
    .with_stale_login_sweeper(Arc::new(WindowsRegistrySweeper { processes, grace }));
    (registry, spawner)
}

#[tokio::test]
async fn windows_unretired_original_blocks_login_before_new_config_or_spawn() {
    let processes = Arc::new(WindowsProcesses::new(vec![held_login_core(
        windows_test_pid(20),
        "live",
    )]));
    let (reg, spawner) = reg_with_windows_sweeper(processes.clone(), Duration::ZERO);
    let ud = temp_ud();
    let state = ud.join("tailscaled.state");
    std::fs::write(&state, b"keep-existing-identity").unwrap();
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
    assert!(matches!(
        &outcome,
        StartLoginOutcome::Failed(reason) if reason == STALE_LOGIN_CORE_ALIVE
    ));
    assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
    assert!(!reg.shared.contains("ts1"));
    assert!(login_configs(&ud).is_empty());
    assert_eq!(std::fs::read(&state).unwrap(), b"keep-existing-identity");
    assert_eq!(processes.ends(), vec!["live", "live"]);
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn windows_login_sweep_keeps_registered_children_and_runs_before_spawn() {
    let processes = Arc::new(
        WindowsProcesses::new(vec![
            held_login_core(windows_test_pid(777), "registered"),
            held_login_core(windows_test_pid(778), "orphan"),
        ])
        .observed_as("orphan", vec![Some(true), Some(false)]),
    );
    let (reg, spawner) = reg_with_windows_sweeper(processes.clone(), Duration::ZERO);
    let ud = temp_ud();
    reg.register_inflight_for_test("other", windows_test_pid(777));
    started(&reg, &ud, &ts_server("ts1", "myts")).await;
    assert_eq!(spawner.count.load(Ordering::SeqCst), 1);
    assert_eq!(processes.ends(), vec!["orphan"]);
    assert_eq!(processes.scans.load(Ordering::SeqCst), 1);
    reg.cancel_login("ts1").await.unwrap();
    std::fs::remove_dir_all(ud).unwrap();
}

#[tokio::test]
async fn windows_logout_mutation_runs_only_after_original_exit_is_observed() {
    let processes = Arc::new(
        WindowsProcesses::new(vec![held_login_core(windows_test_pid(20), "original")])
            .observed_as("original", vec![Some(true), Some(false)]),
    );
    let (reg, _) = reg_with_windows_sweeper(processes.clone(), Duration::ZERO);
    assert!(reg
        .logout("ts1", &|| false, None, |_| {
            assert_eq!(processes.ends(), vec!["original"]);
            assert_eq!(
                processes.observations.lock().unwrap()["original"],
                vec![Some(false)]
            );
            assert!(reg.start_gate.try_lock().is_err(), "gate covers mutation");
            Ok(())
        })
        .await
        .unwrap());
}

#[tokio::test(start_paused = true)]
async fn windows_sweep_holds_the_state_gate_and_failure_preserves_logout_state() {
    let processes = Arc::new(WindowsProcesses::new(vec![held_login_core(
        windows_test_pid(20),
        "live",
    )]));
    let (reg, _) = reg_with_windows_sweeper(processes.clone(), Duration::from_secs(5));
    let reg = Arc::new(reg);
    let ud = temp_ud();
    let state = ud.join("tailscaled.state");
    std::fs::write(&state, b"keep-existing-identity").unwrap();
    let deleted = Arc::new(AtomicBool::new(false));
    let logout = {
        let reg = reg.clone();
        let state = state.clone();
        let deleted = deleted.clone();
        tokio::spawn(async move {
            reg.logout("ts1", &|| false, None, |_| {
                deleted.store(true, Ordering::SeqCst);
                std::fs::remove_file(state)
            })
            .await
        })
    };
    processes.ended.notified().await;
    assert!(reg.start_gate.try_lock().is_err());
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let prepare = {
        let reg = reg.clone();
        let entered = entered.clone();
        tokio::spawn(async move {
            entered.add_permits(1);
            reg.prepare("ts2", "late-prepare").await
        })
    };
    entered.acquire().await.unwrap().forget();
    assert!(
        !prepare.is_finished(),
        "new admission must wait for the state gate"
    );
    assert!(!deleted.load(Ordering::SeqCst));
    let error = logout.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ResourceBusy);
    assert_eq!(error.to_string(), STALE_LOGIN_CORE_ALIVE);
    prepare.await.unwrap().unwrap();
    assert!(!deleted.load(Ordering::SeqCst));
    assert_eq!(std::fs::read(&state).unwrap(), b"keep-existing-identity");
    assert_eq!(processes.ends(), vec!["live", "live"]);
    std::fs::remove_dir_all(ud).unwrap();
}

#[test]
fn windows_production_sweep_is_wired_to_original_holds_not_uid_or_pid_signals() {
    // Structural wiring guard only. Native Windows scanner/hold tests remain required.
    let source = crate::test_support::crate_code("runtime/tailscale_login_core/stale.rs");
    let adapter = source
        .split("impl StaleLoginCoreSweeper for ProcessStaleLoginSweeper")
        .nth(1)
        .unwrap();
    let windows = adapter.split("#[cfg(not(windows))]").next().unwrap();
    let compact: String = windows.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("#[cfg(windows)]"));
    assert!(compact.contains("sweep_windows_stale_login_cores("));
    assert!(compact.contains("scan:&polaris_core_supervisor::scan_running_cores"));
    assert!(compact.contains("end:&|process|process.hold.terminate()"));
    assert!(compact.contains("running:&|process|process.hold.is_running()"));
    assert!(!compact.contains("process_owner_uid"));
    assert!(!compact.contains("send_signal"));
    assert!(!compact.contains("pid_alive"));
}
