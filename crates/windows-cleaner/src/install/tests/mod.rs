//! Shared SCM/utility scenarios. On Windows the real FileTransaction owns all
//! filesystem/SD/recovery/finalization work. Other hosts check pure state only
//! and never emit the native17/native20 execution receipt markers.
use super::*;

fn policy() -> Policy {
    Policy {
        account: "LocalSystem".into(),
        command: "fixed".into(),
        display: crate::SERVICE.into(),
        description: "original description".into(),
        owner: crate::SYSTEM.into(),
        group: crate::ADMINS.into(),
        dacl: "expected".into(),
        service_type: 16,
        start_type: 3,
        error_control: 1,
        tag: 0,
        load_group: String::new(),
        empty_dependencies: true,
        recovery_reset: 86400,
        recovery_command: String::new(),
        reboot_message: String::new(),
        action_types: vec![1, 1, 1],
        action_delays: vec![5000, 5000, 5000],
        non_crash: false,
        sid_type: 0,
        launch_protected: 0,
        empty_privileges: true,
    }
}
struct Fake {
    #[cfg(windows)]
    files: crate::windows::install::tests::FixtureFiles,
    old: Option<State>,
    state: State,
    policy: Policy,
    unknown: bool,
    second_conflict: bool,
    case: &'static str,
    trace: Vec<&'static str>,
    service_object: u64,
    creates: usize,
    deletes: usize,
    configures: usize,
    starts: usize,
    controller: usize,
    waits: usize,
    disposes: usize,
    keep: bool,
    #[cfg(not(windows))]
    model_tx_present: bool,
    fail_activation: bool,
    fail_create: bool,
    fail_candidate_query: bool,
    unknown_utility: bool,
    retained: bool,
    same_user_registration: bool,
    ready: bool,
    busy: bool,
}
impl Fake {
    fn new(active: bool) -> Self {
        Self {
            #[cfg(windows)]
            files: crate::windows::install::tests::FixtureFiles::new(),
            old: Some(if active {
                State::Running
            } else {
                State::Stopped
            }),
            state: if active {
                State::Running
            } else {
                State::Stopped
            },
            policy: policy(),
            unknown: false,
            second_conflict: false,
            case: "pending-running",
            trace: vec![],
            service_object: 42,
            creates: 0,
            deletes: 0,
            configures: 0,
            starts: 0,
            controller: 0,
            waits: 0,
            disposes: 0,
            keep: false,
            #[cfg(not(windows))]
            model_tx_present: false,
            fail_activation: true,
            fail_create: false,
            fail_candidate_query: false,
            unknown_utility: false,
            retained: false,
            same_user_registration: false,
            ready: false,
            busy: false,
        }
    }
    fn step(&mut self, name: &'static str) -> io::Result<()> {
        self.trace.push(name);
        Ok(())
    }
    #[cfg(windows)]
    fn p(&self, name: &str) -> std::path::PathBuf {
        self.files.path(name)
    }
    fn tx_present(&self) -> bool {
        #[cfg(windows)]
        {
            self.files.tx_present()
        }
        #[cfg(not(windows))]
        {
            self.model_tx_present
        }
    }
    fn check_policy(&self) -> io::Result<()> {
        if self.unknown || self.second_conflict || !self.policy.supported("fixed", "expected") {
            Err(io::Error::other("unknown or unsupported policy"))
        } else {
            Ok(())
        }
    }
    fn assert_backups(&self) {
        #[cfg(windows)]
        self.files.assert_backups();
    }
}
impl Ops for Fake {
    fn uncertain(&self) -> bool {
        self.unknown_utility
    }
    fn inspect(&mut self) -> io::Result<Option<State>> {
        self.step("inspect")?;
        self.check_policy()?;
        Ok(self.old)
    }
    fn prepare(&mut self) -> io::Result<()> {
        self.step("prepare")?;
        #[cfg(windows)]
        self.files.prepare()?;
        #[cfg(not(windows))]
        {
            self.model_tx_present = true;
        }
        Ok(())
    }
    fn install_core(&mut self) -> io::Result<()> {
        self.step("core")?;
        if self.unknown_utility {
            return Err(io::Error::other("unknown owned child"));
        }
        Ok(())
    }
    fn selection(&mut self) -> io::Result<Selection> {
        self.step("policy")?;
        Ok(if self.retained {
            Selection::Retain
        } else {
            Selection::Replace
        })
    }
    fn retained_capability(&mut self, _: Option<State>) -> io::Result<()> {
        self.step("capability")
    }
    fn same_registration(&mut self) -> io::Result<bool> {
        self.step("same-registration")?;
        Ok(self.same_user_registration)
    }
    fn retain_probe(&mut self) -> io::Result<bool> {
        self.step("retain-probe")?;
        Ok(self.ready)
    }
    fn snapshot(&mut self, replace: bool) -> io::Result<()> {
        self.step("snapshot")?;
        #[cfg(windows)]
        self.files.snapshot(replace)?;
        #[cfg(not(windows))]
        let _ = replace;
        Ok(())
    }
    fn admit(&mut self) -> io::Result<bool> {
        self.step("admit")?;
        Ok(!self.busy)
    }
    fn revalidate(&mut self) -> io::Result<()> {
        self.step("revalidate")?;
        self.check_policy()
    }
    fn stop(&mut self) -> io::Result<()> {
        self.step("stop")?;
        self.state = State::Stopped;
        Ok(())
    }
    fn register_user(&mut self) -> io::Result<()> {
        self.step("register")?;
        #[cfg(windows)]
        self.files.register_utility_effect()?;
        Ok(())
    }
    fn replace_payload(&mut self) -> io::Result<()> {
        self.step("replace")?;
        #[cfg(windows)]
        self.files.replace_payload()?;
        Ok(())
    }
    fn create(&mut self) -> io::Result<()> {
        self.step("create")?;
        if self.fail_create {
            return Err(io::Error::other("creation completion unknown"));
        }
        self.creates += 1;
        self.service_object = 84;
        Ok(())
    }
    fn configure_fresh(&mut self) -> io::Result<()> {
        self.step("configure")?;
        self.configures += 1;
        Ok(())
    }
    fn start(&mut self) -> io::Result<()> {
        self.step("activation")?;
        if self.fail_activation {
            return Err(io::Error::other("injected activation failure"));
        }
        self.state = State::Running;
        Ok(())
    }
    fn stop_candidate(&mut self, old: Option<State>, tx: &Transaction) -> io::Result<()> {
        self.step("stop-candidate")?;
        assert!(tx.mutated);
        assert!(tx.registration_changed);
        if self.fail_candidate_query {
            return Err(io::Error::other("candidate query unknown"));
        }
        self.check_policy()?;
        if old.is_none() {
            if tx.created {
                self.deletes += 1;
            } else if tx.create_attempted {
                return Err(io::Error::other("creation not proven"));
            }
        }
        self.state = State::Stopped;
        Ok(())
    }
    fn restore_payload(&mut self) -> io::Result<()> {
        self.step("restore-payload")?;
        #[cfg(windows)]
        self.files.restore_payload()?;
        Ok(())
    }
    fn restore_registration(&mut self) -> io::Result<()> {
        self.step("restore-registration")?;
        #[cfg(windows)]
        self.files.restore_registration()?;
        Ok(())
    }
    fn restore_previous(&mut self, old: Option<State>) -> io::Result<()> {
        self.step("restore-previous")?;
        restore_service(self, old)
    }
    fn cancel_admission(&mut self) -> io::Result<()> {
        self.step("cancel")
    }
    fn finish(&mut self, keep: bool) -> io::Result<()> {
        self.step("finish")?;
        self.keep = keep;
        #[cfg(windows)]
        self.files.finish(keep)?;
        #[cfg(not(windows))]
        {
            if !keep {
                self.model_tx_present = false;
            }
        }
        Ok(())
    }
}

impl RecoveryOps for Fake {
    fn recovery_policy(&mut self) -> io::Result<()> {
        let final_check = self.trace.last() == Some(&"final-state");
        if final_check
            && matches!(
                self.case,
                "final-policy-unknown" | "final-policy-conflict" | "stopped-policy-unknown"
            )
        {
            return Err(io::Error::other("final policy unavailable/conflict"));
        }
        self.check_policy()
    }
    fn recovery_start(&mut self) -> io::Result<()> {
        self.starts += 1;
        if self.case == "start-exit-failure" {
            return Err(io::Error::other("native start exit failure"));
        }
        self.state = if self.case == "immediate-running" {
            State::Running
        } else {
            State::Stopped
        };
        Ok(())
    }
    fn status_begin(&mut self) -> io::Result<()> {
        self.controller += 1;
        if self.case == "controller-query-unknown" {
            Err(io::Error::other("controller query unknown"))
        } else {
            Ok(())
        }
    }
    fn status_wait_running(&mut self) -> io::Result<()> {
        self.waits += 1;
        match self.case {
            "wait-query-unknown" | "pending-timeout" | "pending-stopped" => {
                Err(io::Error::other("bounded wait failed"))
            }
            _ => {
                if self.case == "pending-running" {
                    self.trace.extend(["Start Pending", "Running"]);
                }
                self.state = State::Running;
                Ok(())
            }
        }
    }
    fn status_refresh(&mut self) -> io::Result<Option<State>> {
        match self.case {
            "refresh-unknown" => Err(io::Error::other("refresh unknown")),
            "refresh-pending" => Ok(None),
            "refresh-stopped" | "refresh-cim-disagree" => Ok(Some(State::Stopped)),
            _ => Ok(Some(self.state)),
        }
    }
    fn status_end(&mut self) {
        self.disposes += 1;
    }
    fn final_state(&mut self) -> io::Result<Option<State>> {
        self.trace.push("final-state");
        match self.case {
            "final-description-unknown" | "stopped-description-unknown" => {
                Err(io::Error::other("final query failed"))
            }
            "final-missing" => Ok(None),
            "final-stopped" => Ok(Some(State::Stopped)),
            "stopped-became-running" => Ok(Some(State::Running)),
            _ => Ok(Some(self.state)),
        }
    }
}

#[test]
fn original_17_inert_scm_cases_use_production_transaction() {
    let mut count = 0;
    for bad in [
        "account",
        "command",
        "cim-command",
        "owner",
        "dacl",
        "recovery",
        "noncrash",
        "privileges",
        "dependencies",
        "display",
        "query",
        "description-query",
    ] {
        let mut f = Fake::new(true);
        match bad {
            "account" => f.policy.account = "LocalService".into(),
            "command" => f.policy.command = "foreign".into(),
            "cim-command" => f.second_conflict = true,
            "owner" => f.policy.owner = "foreign".into(),
            "dacl" => f.policy.dacl = "foreign".into(),
            "recovery" => f.policy.action_delays[0] = 1,
            "noncrash" => f.policy.non_crash = true,
            "privileges" => f.policy.empty_privileges = false,
            "dependencies" => f.policy.empty_dependencies = false,
            "display" => f.policy.display = "foreign".into(),
            "query" | "description-query" => f.unknown = true,
            _ => unreachable!(),
        }
        let before = f.policy.clone();
        assert_eq!(run(&mut f), 81, "{bad}");
        assert_eq!(f.trace, ["inspect", "finish"]);
        assert_eq!(f.policy, before);
        assert_eq!(f.service_object, 42);
        assert_eq!(f.state, State::Running);
        assert_eq!((f.creates, f.deletes, f.configures, f.starts), (0, 0, 0, 0));
        count += 1;
    }
    for active in [true, false] {
        let mut f = Fake::new(active);
        f.case = if active { "pending-running" } else { "stopped" };
        let before = f.policy.clone();
        assert_eq!(run(&mut f), 86);
        assert_eq!(f.policy, before);
        assert_eq!(f.service_object, 42);
        assert_eq!((f.creates, f.deletes, f.configures), (0, 0, 0));
        assert_eq!(f.starts, usize::from(active));
        assert_eq!(
            f.state,
            if active {
                State::Running
            } else {
                State::Stopped
            }
        );
        #[cfg(windows)]
        assert_eq!(
            std::fs::read_to_string(f.p("payload")).unwrap(),
            "original payload"
        );
        #[cfg(windows)]
        assert_eq!(
            std::fs::read_to_string(f.p("registration")).unwrap(),
            "original registration"
        );
        count += 1;
    }
    let mut f = Fake::new(false);
    f.old = None;
    assert_eq!(run(&mut f), 86);
    assert_eq!((f.creates, f.configures, f.deletes), (1, 1, 1));
    count += 1;
    let mut f = Fake::new(false);
    f.old = None;
    f.fail_candidate_query = true;
    assert_eq!(run(&mut f), 87);
    assert_eq!(f.deletes, 0);
    assert!(f.keep);
    f.assert_backups();
    count += 1;
    let mut f = Fake::new(false);
    f.old = None;
    f.fail_create = true;
    assert_eq!(run(&mut f), 87);
    assert_eq!(f.deletes, 0);
    assert!(f.keep);
    f.assert_backups();
    count += 1;
    assert_eq!(count, 17);
    #[cfg(windows)]
    println!("Passed 17 native inert SCM cases.");
}

#[test]
fn original_20_recovery_cases_use_production_catch_and_finish() {
    let cases = [
        "pending-running",
        "immediate-running",
        "pending-stopped",
        "pending-timeout",
        "controller-query-unknown",
        "wait-query-unknown",
        "refresh-unknown",
        "refresh-pending",
        "refresh-stopped",
        "refresh-cim-disagree",
        "final-description-unknown",
        "final-stopped",
        "final-missing",
        "final-policy-unknown",
        "final-policy-conflict",
        "start-exit-failure",
        "stopped",
        "stopped-became-running",
        "stopped-policy-unknown",
        "stopped-description-unknown",
    ];
    for case in cases {
        let active = !case.starts_with("stopped");
        let mut f = Fake::new(active);
        f.case = case;
        let policy = f.policy.clone();
        let confirmed = matches!(case, "pending-running" | "immediate-running" | "stopped");
        assert_eq!(run(&mut f), if confirmed { 86 } else { 87 }, "{case}");
        assert_eq!(f.keep, !confirmed);
        assert_eq!(f.tx_present(), !confirmed);
        assert_eq!(f.policy, policy);
        assert_eq!(f.service_object, 42);
        assert_eq!((f.creates, f.deletes, f.configures), (0, 0, 0));
        #[cfg(windows)]
        assert_eq!(
            std::fs::read_to_string(f.p("payload")).unwrap(),
            "original payload"
        );
        #[cfg(windows)]
        assert_eq!(
            std::fs::read_to_string(f.p("registration")).unwrap(),
            "original registration"
        );
        if !confirmed {
            f.assert_backups();
        }
        if !active {
            assert_eq!((f.starts, f.controller, f.waits, f.disposes), (0, 0, 0, 0));
        } else if case == "start-exit-failure" {
            assert_eq!((f.starts, f.controller, f.waits, f.disposes), (1, 0, 0, 0));
        } else if case == "controller-query-unknown" {
            assert_eq!((f.starts, f.controller, f.waits, f.disposes), (1, 1, 0, 0));
        } else {
            assert_eq!((f.starts, f.controller, f.waits, f.disposes), (1, 1, 1, 1));
        }
        if case == "pending-running" {
            assert!(f
                .trace
                .windows(2)
                .any(|s| s == ["Start Pending", "Running"]));
        }
        assert!(
            f.trace
                .iter()
                .position(|s| *s == "restore-registration")
                .unwrap()
                < f.trace
                    .iter()
                    .position(|s| *s == "restore-previous")
                    .unwrap()
        );
    }
    #[cfg(windows)]
    println!("Passed 20 native rollback state/transaction cases.");
}

#[test]
fn production_order_marks_mutation_and_partial_registration_before_failure() {
    let mut f = Fake::new(true);
    assert_eq!(run(&mut f), 86);
    let positions: Vec<_> = [
        "prepare",
        "core",
        "snapshot",
        "admit",
        "stop",
        "register",
        "replace",
        "stop-candidate",
        "restore-payload",
        "restore-registration",
        "restore-previous",
        "finish",
    ]
    .iter()
    .map(|step| f.trace.iter().position(|s| s == step).unwrap())
    .collect();
    assert!(positions.windows(2).all(|p| p[0] < p[1]));
}

fn request() -> Request {
    Request {
        stage: r"C:\private\stage".into(),
        original_sid: "S-1-5-21-1-2-3-1000".into(),
        config: r"C:\private\config".into(),
        core_id: "a".repeat(64),
        manifest_sha256: "b".repeat(64),
        helper_sha256: "c".repeat(64),
        cleaner_sha256: "d".repeat(64),
        token: "e".repeat(64),
        stage_identity: [0; 3],
        source_identities: vec![],
    }
}

#[test]
fn caller_hashes_cannot_redefine_the_bin_only_authority() {
    let r = request();
    let anchor = Anchor {
        core_id: r.core_id.clone(),
        helper_sha256: r.helper_sha256.clone(),
    };
    anchor.admits(&r).unwrap();
    let mut changed = r.clone();
    changed.helper_sha256 = "f".repeat(64);
    assert!(anchor.admits(&changed).is_err());
    changed = r.clone();
    changed.core_id = "f".repeat(64);
    assert!(anchor.admits(&changed).is_err());
    assert!(Anchor::parse(b"").is_err());
    assert!(Anchor::parse(b"{}").is_err());
    let raw = serde_json::to_vec(&r).unwrap();
    Request::parse(&raw).unwrap();
    let mut object: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    for field in ["service", "root", "executable", "command", "argv", "pid"] {
        object[field] = "caller-controlled".into();
        assert!(Request::parse(&serde_json::to_vec(&object).unwrap()).is_err());
        object.as_object_mut().unwrap().remove(field);
    }
    let text = String::from_utf8(raw).unwrap();
    let duplicate = text.replacen('{', "{\"token\":\"duplicate\",", 1);
    assert!(Request::parse(duplicate.as_bytes()).is_err());
    for path in [
        r"relative",
        r"C:\stage\..\other",
        r"C:\stage:file",
        r"\\server\share\stage",
    ] {
        let mut bad = r.clone();
        bad.stage = path.into();
        assert!(bad.validate().is_err());
    }
}

#[test]
fn canonical_core_id_excludes_both_install_hashes_but_binds_payload_bytes() {
    use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest};
    let first = CoreManifest {
        platform: "win".into(),
        version: "1.0.0".into(),
        provider_fingerprint: "a".repeat(64),
        files: vec![
            CoreFile {
                name: "sing-box.exe".into(),
                bytes: 10,
                sha256: "b".repeat(64),
            },
            CoreFile {
                name: "libcronet.dll".into(),
                bytes: 11,
                sha256: "c".repeat(64),
            },
        ],
        helper_sha256: Some("d".repeat(64)),
        cleaner_sha256: Some("e".repeat(64)),
    };
    let mut second = first.clone();
    second.helper_sha256 = Some("f".repeat(64));
    second.cleaner_sha256 = Some("0".repeat(64));
    assert_eq!(
        first.canonical_bytes().unwrap(),
        second.canonical_bytes().unwrap()
    );
    second.files[1].sha256 = "1".repeat(64);
    assert_ne!(
        first.canonical_bytes().unwrap(),
        second.canonical_bytes().unwrap()
    );
}

#[test]
fn unknown_utility_never_runs_competing_rollback_or_discards_transaction() {
    let mut f = Fake::new(true);
    f.unknown_utility = true;
    assert_eq!(run(&mut f), 87);
    assert!(f.keep && f.tx_present());
    for forbidden in [
        "admit",
        "stop",
        "restore-payload",
        "restore-registration",
        "restore-previous",
    ] {
        assert!(!f.trace.contains(&forbidden));
    }
}

#[test]
fn core_only_retention_requires_same_registration_and_live_selected_id_ready() {
    let mut f = Fake::new(true);
    f.retained = true;
    f.same_user_registration = true;
    f.ready = true;
    assert_eq!(run(&mut f), 0);
    assert!(!f.keep && !f.tx_present());
    assert!(f.trace.contains(&"retain-probe"));
    assert!(f.trace.contains(&"finish"));
    for op in ["admit", "stop", "register", "replace", "create", "start"] {
        assert!(!f.trace.contains(&op));
    }
    #[cfg(windows)]
    assert_eq!(
        std::fs::read_to_string(f.p("registration")).unwrap(),
        "original registration"
    );
    for same in [false, true] {
        let mut f = Fake::new(true);
        f.retained = true;
        f.same_user_registration = same;
        f.busy = true;
        assert_eq!(run(&mut f), 75);
        assert!(f.trace.contains(&"admit"));
        assert_eq!(f.trace.contains(&"retain-probe"), same);
        assert!(!f.trace.contains(&"stop"));
        assert!(!f.trace.contains(&"register"));
        #[cfg(windows)]
        assert_eq!(
            std::fs::read_to_string(f.p("registration")).unwrap(),
            "original registration"
        );
    }
}
