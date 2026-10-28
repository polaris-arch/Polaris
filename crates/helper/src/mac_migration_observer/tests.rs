use super::*;
use std::collections::VecDeque;

enum Step {
    List(Result<Vec<u32>, UnknownReason>),
    Process(u32, Result<Process, ReadError>),
}

struct Script(VecDeque<Step>);

impl Source for Script {
    fn pids(&mut self) -> Result<Vec<u32>, UnknownReason> {
        match self.0.pop_front().expect("unexpected enumeration") {
            Step::List(value) => value,
            _ => panic!("expected a PID list read"),
        }
    }
    fn process(&mut self, pid: u32) -> Result<Process, ReadError> {
        match self.0.pop_front().expect("unexpected process read") {
            Step::Process(expected, value) => {
                assert_eq!(expected, pid);
                value
            }
            _ => panic!("expected a process read"),
        }
    }
}

fn identity(pid: u32, ppid: u32, pgid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        ppid,
        pgid,
        effective_uid: 0,
        start_seconds: 1000 + u64::from(pid),
        start_microseconds: 17,
    }
}

fn object(inode: u64) -> FileIdentity {
    FileIdentity { device: 7, inode }
}

fn process(pid: u32, ppid: u32, pgid: u32, path: &str, inode: u64) -> Process {
    Process {
        identity: identity(pid, ppid, pgid),
        path: path.into(),
        path_file: object(inode),
    }
}

fn launchd() -> Process {
    process(1, 0, 1, "/sbin/launchd", 1)
}
fn helper() -> Process {
    process(10, 1, 10, "/protected/helper", 10)
}
fn core() -> Process {
    process(20, 10, 10, "/protected/core", 20)
}

fn held(path: &str) -> HeldExecutable {
    // A read-only descriptor suffices for pure policy fixtures; no native
    // pinning, external service, process signal, or filesystem mutation.
    HeldExecutable {
        path: path.into(),
        file: File::open(std::env::current_exe().unwrap()).unwrap(),
    }
}

fn stable_steps(processes: &[Process]) -> Vec<Step> {
    let pids: Vec<u32> = processes.iter().map(|p| p.identity.pid).collect();
    let mut steps = vec![Step::List(Ok(pids.clone()))];
    for p in processes {
        steps.push(Step::Process(p.identity.pid, Ok(p.clone())));
    }
    steps.push(Step::List(Ok(pids.clone())));
    for p in processes {
        steps.push(Step::Process(p.identity.pid, Ok(p.clone())));
    }
    steps.push(Step::List(Ok(pids)));
    steps
}

fn read(steps: Vec<Step>) -> Result<Vec<Process>, UnknownReason> {
    let mut source = Script(steps.into());
    let result = census(&mut source, Instant::now() + OBSERVATION_BUDGET);
    assert!(
        source.0.is_empty(),
        "the observer skipped scripted evidence"
    );
    result
}

fn target<'a>(helper: &'a HeldExecutable, cores: &'a [HeldExecutable]) -> Target<'a> {
    Target {
        helper_pid: 10,
        helper,
        managed_executables: cores,
    }
}

fn ready<'a>(target: Target<'a>, processes: &[Process]) -> Captured<'a> {
    match preflight(target, vec![object(10), object(20)], processes) {
        Preflight::Ready(captured) => captured,
        other => panic!("expected ready: {other:?}"),
    }
}

#[test]
fn stable_complete_census_reads_every_pid_twice() {
    let processes = vec![launchd(), helper(), core()];
    assert_eq!(read(stable_steps(&processes)).unwrap(), processes);
}

#[test]
fn kernel_is_the_only_excluded_pid() {
    assert_eq!(normalize_pids(vec![0, 1, 2, 12]).unwrap(), vec![1, 2, 12]);
}

#[test]
fn empty_truncated_duplicate_and_invalid_lists_are_unknown() {
    for pids in [
        vec![],
        vec![0],
        vec![2],
        vec![1, 1],
        vec![0, 0, 1],
        vec![1, u32::MAX],
        vec![1; MAX_PROCESSES],
    ] {
        assert!(normalize_pids(pids).is_err());
    }
}

#[test]
fn enumeration_error_is_not_an_empty_census() {
    assert_eq!(
        read(vec![Step::List(Err(UnknownReason::new(
            "census", None, "EPERM"
        )))])
        .unwrap_err()
        .detail,
        "EPERM"
    );
}

#[test]
fn denied_unresolved_deleted_and_partial_processes_are_unknown() {
    for message in [
        "EPERM",
        "EACCES",
        "deleted executable",
        "partial BSD record",
        "image path unresolved",
        "API unavailable",
    ] {
        let result = read(vec![
            Step::List(Ok(vec![1])),
            Step::Process(
                1,
                Err(ReadError::Unknown(UnknownReason::new(
                    "native",
                    Some(1),
                    message,
                ))),
            ),
        ]);
        assert_eq!(result.unwrap_err().detail, message);
    }
}

#[test]
fn vanished_pid_requires_a_new_complete_census() {
    let mut steps = vec![
        Step::List(Ok(vec![1, 10])),
        Step::Process(1, Ok(launchd())),
        Step::Process(10, Err(ReadError::Race)),
    ];
    steps.extend(stable_steps(&[launchd()]));
    assert_eq!(read(steps).unwrap(), vec![launchd()]);
}

#[test]
fn pid_reuse_requires_reobservation_of_the_new_birth() {
    let old = helper();
    let mut replacement = old.clone();
    replacement.identity.start_microseconds += 1;
    let mut steps = vec![
        Step::List(Ok(vec![1, 10])),
        Step::Process(1, Ok(launchd())),
        Step::Process(10, Ok(old)),
        Step::List(Ok(vec![1, 10])),
        Step::Process(1, Ok(launchd())),
        Step::Process(10, Ok(replacement.clone())),
    ];
    steps.extend(stable_steps(&[launchd(), replacement.clone()]));
    assert_eq!(read(steps).unwrap(), vec![launchd(), replacement]);
}

#[test]
fn exec_or_reparent_races_are_reconciled_by_a_whole_new_census() {
    for mode in 0..3 {
        let mut after = helper();
        match mode {
            0 => after.path = "/other/image".into(),
            1 => after.identity.ppid = 2,
            _ => after.path_file.inode += 1,
        }
        let mut steps = vec![
            Step::List(Ok(vec![1, 10])),
            Step::Process(1, Ok(launchd())),
            Step::Process(10, Ok(helper())),
            Step::List(Ok(vec![1, 10])),
            Step::Process(1, Ok(launchd())),
            Step::Process(10, Ok(after.clone())),
        ];
        steps.extend(stable_steps(&[launchd(), after.clone()]));
        assert_eq!(read(steps).unwrap(), vec![launchd(), after]);
    }
}

#[test]
fn process_churn_cannot_exhaust_retries_into_clear() {
    let mut steps = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        steps.push(Step::List(Ok(vec![1])));
        steps.push(Step::Process(1, Err(ReadError::Race)));
    }
    assert_eq!(read(steps).unwrap_err().stage, "census");
}

#[test]
fn a_process_added_at_the_final_boundary_requires_rescan() {
    let mut steps = stable_steps(&[launchd()]);
    *steps.last_mut().unwrap() = Step::List(Ok(vec![1, 10]));
    steps.extend(stable_steps(&[launchd(), helper()]));
    assert_eq!(read(steps).unwrap(), vec![launchd(), helper()]);
}

#[test]
fn timeout_is_unknown() {
    let mut source = Script(vec![Step::List(Ok(vec![1]))].into());
    assert_eq!(
        census(&mut source, Instant::now()).unwrap_err().stage,
        "deadline"
    );
}

#[test]
fn active_managed_connection_is_captured_for_confirmed_stop() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper(), core()]);
    assert_eq!(captured.managed_runtimes(), &[core().identity]);
    assert_eq!(captured.descendants(), &[core().identity]);
}

#[test]
fn helper_image_is_expected_before_stop_and_busy_afterward() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper()]);
    assert!(matches!(
        closure(&captured, &[launchd(), helper()]),
        Closure::Busy(BusyReason {
            kind: BusyKind::OriginalHelper,
            ..
        })
    ));
}

#[test]
fn an_additional_helper_rejects_preflight() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let second = process(30, 1, 30, "/protected/helper", 10);
    assert!(matches!(
        preflight(
            target(&file, &cores),
            vec![object(10), object(20)],
            &[launchd(), helper(), second]
        ),
        Preflight::Busy(BusyReason {
            kind: BusyKind::AdditionalHelper,
            ..
        })
    ));
}

#[test]
fn wrong_helper_path_inode_uid_parent_or_group_is_unknown() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    for mode in 0..5 {
        let mut wrong = helper();
        match mode {
            0 => wrong.path = "/other/helper".into(),
            1 => wrong.path_file.inode += 1,
            2 => wrong.identity.effective_uid = 501,
            3 => wrong.identity.ppid = 9,
            _ => wrong.identity.pgid = 1,
        }
        assert!(matches!(
            preflight(
                target(&file, &cores),
                vec![object(10), object(20)],
                &[launchd(), wrong]
            ),
            Preflight::Unknown(_)
        ));
    }
}

#[test]
fn detached_managed_runtime_is_detected_without_ancestry() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper()]);
    let escaped = process(99, 1, 99, "/protected/core", 20);
    assert!(matches!(
        closure(&captured, &[launchd(), escaped]),
        Closure::Busy(BusyReason {
            kind: BusyKind::ManagedExecutable,
            ..
        })
    ));
}

#[test]
fn a_configured_path_stays_busy_even_if_its_reported_inode_differs() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper()]);
    assert!(matches!(
        closure(
            &captured,
            &[launchd(), process(99, 1, 99, "/protected/core", 999)]
        ),
        Closure::Busy(_)
    ));
}

#[test]
fn a_resolved_path_alias_of_a_managed_inode_is_busy() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper()]);
    assert!(matches!(
        closure(
            &captured,
            &[launchd(), process(99, 1, 99, "/alias/core", 20)]
        ),
        Closure::Busy(_)
    ));
}

#[test]
fn captured_managed_birth_surviving_exec_is_busy() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let detached = process(20, 1, 20, "/protected/core", 20);
    let captured = ready(
        target(&file, &cores),
        &[launchd(), helper(), detached.clone()],
    );
    let mut escaped = detached;
    escaped.path = "/unrelated/runtime".into();
    escaped.path_file = object(80);
    assert!(matches!(
        closure(&captured, &[launchd(), escaped]),
        Closure::Busy(BusyReason {
            kind: BusyKind::CapturedManagedRuntime,
            ..
        })
    ));
}

#[test]
fn recursive_descendant_surviving_exec_reparent_and_group_escape_is_busy() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let grandchild = process(30, 20, 30, "/unrelated/worker", 30);
    let captured = ready(
        target(&file, &cores),
        &[launchd(), helper(), core(), grandchild.clone()],
    );
    let mut escaped = grandchild;
    escaped.identity.ppid = 1;
    escaped.identity.pgid = 77;
    escaped.path = "/unrelated/next".into();
    escaped.path_file = object(77);
    assert!(matches!(
        closure(&captured, &[launchd(), escaped]),
        Closure::Busy(BusyReason {
            kind: BusyKind::CapturedDescendant,
            ..
        })
    ));
}

#[test]
fn original_process_group_member_is_busy_without_matching_image() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper()]);
    assert!(matches!(
        closure(&captured, &[launchd(), process(99, 1, 10, "/other", 99)]),
        Closure::Busy(BusyReason {
            kind: BusyKind::OriginalProcessGroup,
            ..
        })
    ));
}

#[test]
fn captured_group_member_stays_busy_after_leaving_the_group() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let member = process(99, 1, 10, "/other", 99);
    let captured = ready(
        target(&file, &cores),
        &[launchd(), helper(), member.clone()],
    );
    let mut escaped = member;
    escaped.identity.pgid = 99;
    assert!(matches!(
        closure(&captured, &[launchd(), escaped]),
        Closure::Busy(BusyReason {
            kind: BusyKind::CapturedProcessGroupMember,
            ..
        })
    ));
}

#[test]
fn unrelated_pid_reuse_does_not_claim_the_original_birth_is_alive() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper(), core()]);
    let mut unrelated = process(20, 1, 80, "/other", 80);
    unrelated.identity.start_microseconds += 1;
    assert_eq!(closure(&captured, &[launchd(), unrelated]), Closure::Clear);
}

#[test]
fn complete_quiet_configured_runtime_census_can_clear() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper(), core()]);
    let after = read(stable_steps(&[launchd()])).unwrap();
    assert_eq!(closure(&captured, &after), Closure::Clear);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn unsupported_platform_never_returns_ready() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    assert!(matches!(
        capture(target(&file, &cores)),
        Preflight::Unknown(_)
    ));
}

#[test]
fn stable_exact_zombie_identity_is_reconciled_without_an_image() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper(), core()]);
    let zombie = core().identity;
    let steps = vec![
        Step::List(Ok(vec![1, 20])),
        Step::Process(1, Ok(launchd())),
        Step::Process(20, Err(ReadError::Terminated(zombie))),
        Step::List(Ok(vec![1, 20])),
        Step::Process(1, Ok(launchd())),
        Step::Process(20, Err(ReadError::Terminated(zombie))),
        Step::List(Ok(vec![1, 20])),
    ];
    let observed = read(steps).unwrap();
    assert_eq!(observed, vec![launchd()]);
    assert_eq!(closure(&captured, &observed), Closure::Clear);
}

#[test]
fn zombie_to_live_or_reused_pid_is_rescanned_and_never_false_clear() {
    let file = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = ready(target(&file, &cores), &[launchd(), helper(), core()]);
    for reused in [false, true] {
        let zombie = core().identity;
        let mut live = core();
        if reused {
            live.identity.start_microseconds += 1;
        }
        let mut steps = vec![
            Step::List(Ok(vec![1, 20])),
            Step::Process(1, Ok(launchd())),
            Step::Process(20, Err(ReadError::Terminated(zombie))),
            Step::List(Ok(vec![1, 20])),
            Step::Process(1, Ok(launchd())),
            Step::Process(20, Ok(live.clone())),
        ];
        steps.extend(stable_steps(&[launchd(), live.clone()]));
        let observed = read(steps).unwrap();
        assert_eq!(observed, vec![launchd(), live]);
        assert!(matches!(closure(&captured, &observed), Closure::Busy(_)));
    }
}

#[test]
fn zombie_birth_change_without_reconciliation_is_unknown() {
    let before = core().identity;
    let mut after = before;
    after.start_microseconds += 1;
    let mut steps = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        steps.extend([
            Step::List(Ok(vec![1, 20])),
            Step::Process(1, Ok(launchd())),
            Step::Process(20, Err(ReadError::Terminated(before))),
            Step::List(Ok(vec![1, 20])),
            Step::Process(1, Ok(launchd())),
            Step::Process(20, Err(ReadError::Terminated(after))),
        ]);
    }
    assert_eq!(read(steps).unwrap_err().stage, "census");
}

#[test]
fn an_unreconciled_zombie_read_error_is_not_terminal_proof() {
    let mut steps = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        steps.extend([
            Step::List(Ok(vec![1, 20])),
            Step::Process(1, Ok(launchd())),
            Step::Process(20, Err(ReadError::Terminated(core().identity))),
            Step::List(Ok(vec![1, 20])),
            Step::Process(1, Ok(launchd())),
            Step::Process(20, Err(ReadError::Race)),
        ]);
    }
    assert_eq!(read(steps).unwrap_err().stage, "census");
}

#[test]
fn live_enoent_is_unknown_even_for_an_ordinary_user_process() {
    let observed = read(vec![
        Step::List(Ok(vec![1, 20])),
        Step::Process(1, Ok(launchd())),
        Step::Process(
            20,
            Err(ReadError::Unknown(UnknownReason::new(
                "executable-path",
                Some(20),
                "ENOENT; live SRUN uid 501",
            ))),
        ),
    ]);
    assert_eq!(observed.unwrap_err().stage, "executable-path");
}

#[test]
fn live_to_zombie_requires_a_whole_new_census() {
    let mut steps = vec![
        Step::List(Ok(vec![1, 20])),
        Step::Process(1, Ok(launchd())),
        Step::Process(20, Ok(core())),
        Step::List(Ok(vec![1, 20])),
        Step::Process(1, Ok(launchd())),
        Step::Process(20, Err(ReadError::Terminated(core().identity))),
    ];
    steps.extend(stable_steps(&[launchd()]));
    assert_eq!(read(steps).unwrap(), vec![launchd()]);
}

fn receipt() -> ReceiptIdentities {
    ReceiptIdentities {
        helper: helper().identity,
        descendants: vec![core().identity],
        managed_runtimes: vec![core().identity],
        group_members: vec![core().identity],
    }
}

#[test]
fn receipt_reuses_closure_without_recapturing_absent_helper() {
    let helper = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = receipt_capture(
        target(&helper, &cores),
        vec![object(10), object(20)],
        &receipt(),
    )
    .unwrap();
    let processes = read(stable_steps(&[launchd()])).unwrap();
    assert_eq!(closure(&captured, &processes), Closure::Clear);
    let mut detached = core();
    detached.identity.ppid = 1;
    detached.identity.pgid = 20;
    detached.path = "/unrelated/new-image".into();
    detached.path_file = object(99);
    assert!(matches!(
        closure(&captured, &[launchd(), detached]),
        Closure::Busy(_)
    ));
}

#[test]
fn receipt_cannot_hide_configured_runtime_with_a_new_birth() {
    let helper = held("/protected/helper");
    let cores = [held("/protected/core")];
    let captured = receipt_capture(
        target(&helper, &cores),
        vec![object(10), object(20)],
        &receipt(),
    )
    .unwrap();
    let mut fresh = core();
    fresh.identity = identity(40, 1, 40);
    assert!(matches!(
        closure(&captured, &[launchd(), fresh]),
        Closure::Busy(BusyReason {
            kind: BusyKind::ManagedExecutable,
            ..
        })
    ));
}

#[test]
fn receipt_identity_bounds_and_helper_constraints_reject() {
    for helper in [
        ProcessIdentity {
            pid: 0,
            ..helper().identity
        },
        ProcessIdentity {
            pid: u32::MAX,
            ..helper().identity
        },
        ProcessIdentity {
            ppid: 2,
            ..helper().identity
        },
        ProcessIdentity {
            effective_uid: 501,
            ..helper().identity
        },
        ProcessIdentity {
            pgid: 1,
            ..helper().identity
        },
        ProcessIdentity {
            start_seconds: 0,
            ..helper().identity
        },
        ProcessIdentity {
            start_seconds: u64::MAX,
            ..helper().identity
        },
        ProcessIdentity {
            start_microseconds: 1_000_000,
            ..helper().identity
        },
    ] {
        let value = ReceiptIdentities {
            helper,
            ..receipt()
        };
        assert!(validate_receipt_identities(10, &value).is_err());
    }
    assert!(validate_receipt_identities(11, &receipt()).is_err());
    let mut value = receipt();
    value.managed_runtimes = vec![core().identity; MAX_RECEIPT_IDENTITIES + 1];
    assert!(validate_receipt_identities(10, &value).is_err());
}

#[test]
fn receipt_duplicate_and_cross_category_inconsistent_births_reject() {
    let mut value = receipt();
    value.descendants.push(core().identity);
    assert!(validate_receipt_identities(10, &value).is_err());
    let mut value = receipt();
    value.managed_runtimes[0].start_seconds += 1;
    assert!(validate_receipt_identities(10, &value).is_err());
    let mut value = receipt();
    value.group_members.push(value.helper);
    assert!(validate_receipt_identities(10, &value).is_err());
    assert!(validate_receipt_identities(10, &receipt()).is_ok());
}

#[test]
fn receipt_descendant_parent_chain_and_original_group_must_match() {
    let mut value = receipt();
    value.descendants = vec![identity(20, 30, 10), identity(30, 20, 10)];
    value.managed_runtimes.clear();
    value.group_members.clear();
    assert!(validate_receipt_identities(10, &value).is_err());
    value.descendants = vec![identity(20, 99, 10)];
    assert!(validate_receipt_identities(10, &value).is_err());
    value.descendants = vec![identity(20, 10, 10), identity(30, 20, 30)];
    assert!(validate_receipt_identities(10, &value).is_ok());
    value.group_members = vec![identity(40, 1, 40)];
    assert!(validate_receipt_identities(10, &value).is_err());
}

#[test]
fn receipt_missing_protected_cohort_does_not_construct_capture() {
    let helper = held("/protected/helper");
    let cores = [held("/protected/core")];
    assert!(receipt_capture(target(&helper, &cores), vec![], &receipt()).is_err());
}

#[test]
fn exact_live_identity_pair_accepts_without_image_resolution() {
    let identity = identity(30, 1, 30);
    assert_eq!(
        reconcile_live_identity(30, (identity, false), (identity, false)).unwrap(),
        identity
    );
}

#[test]
fn live_identity_pair_rejects_zombie_reuse_or_changed_relationships() {
    let before = identity(30, 1, 30);
    for (first, second) in [
        ((before, true), (before, true)),
        ((before, false), (before, true)),
        ((before, true), (before, false)),
        (
            (before, false),
            (
                ProcessIdentity {
                    start_seconds: before.start_seconds + 1,
                    ..before
                },
                false,
            ),
        ),
        (
            (before, false),
            (ProcessIdentity { ppid: 2, ..before }, false),
        ),
        (
            (before, false),
            (ProcessIdentity { pgid: 31, ..before }, false),
        ),
    ] {
        assert!(reconcile_live_identity(30, first, second).is_err());
    }
    assert!(reconcile_live_identity(31, (before, false), (before, false)).is_err());
}
