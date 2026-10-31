use super::*;

fn source() -> String {
    polaris_source_probe::mask_comments(&polaris_source_probe::crate_source!(
        "linux_legacy_maintenance.rs"
    ))
}
fn body<'a>(source: &'a str, name: &str, next: &str) -> &'a str {
    source
        .split_once(name)
        .unwrap()
        .1
        .split_once(next)
        .unwrap()
        .0
}
fn before(source: &str, first: &str, second: &str) {
    assert!(
        source.find(first).unwrap() < source.find(second).unwrap(),
        "{first} must precede {second}"
    );
}
fn start(argv: &str) -> String {
    format!("{{ path={HELPER} ; argv[]={argv} ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 ; }}")
}

#[test]
fn real_current_and_historical_fixed_producers_are_the_only_start_scope() {
    let canonical = polaris_helper_client::manager::linux_canonical_unit();
    assert!(unit_bytes_allowed(canonical.as_bytes()));
    let argv = canonical
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .unwrap();
    assert!(allowed_start(&start(argv)));
    assert!(allowed_start(&start(
        &argv.replace(" --authfile=/var/lib/polaris/authorized-uids", "")
    )));
    for mutated in [
        format!("{argv} --unknown"),
        argv.replace(HELPER, "/tmp/helper"),
        format!("/bin/sh -c '{argv}'"),
        format!("{argv} ; /tmp/foreign"),
    ] {
        assert!(!allowed_start(&start(&mutated)), "{mutated}");
    }
    assert!(!allowed_start(&format!("{} {}", start(argv), start(argv))));
    assert!(!allowed_start(
        &start(argv).replace("ignore_errors=no", "ignore_errors=yes")
    ));
}

#[test]
fn actual_fragment_hooks_aliases_and_unsupported_service_properties_are_rejected() {
    let canonical = polaris_helper_client::manager::linux_canonical_unit();
    for addition in [
        "ExecStop=/tmp/foreign",
        "ExecStopPost=/tmp/foreign",
        "ExecStartPre=/tmp/foreign",
        "ExecStartPost=/tmp/foreign",
        "ExecReload=/tmp/foreign",
        "EnvironmentFile=/tmp/env",
        "RootDirectory=/tmp/root",
        "User=1000",
        "KillMode=process",
        "Delegate=yes",
    ] {
        let mutated = canonical.replace(
            "RuntimeDirectory=",
            &format!("{addition}\nRuntimeDirectory="),
        );
        assert!(!unit_bytes_allowed(mutated.as_bytes()), "{addition}");
    }
    assert!(!unit_bytes_allowed(
        canonical
            .replace("ExecStart=", "ExecStart=/bin/sh ")
            .as_bytes()
    ));
    assert!(!unit_bytes_allowed(
        canonical.replace("[Install]", "[Unknown]").as_bytes()
    ));
    assert!(!unit_bytes_allowed(
        b"[Service]\nExecStart=\\\n/tmp/helper\n"
    ));
}

#[test]
fn hashes_do_not_promote_a_different_executable_object_to_stop_authority() {
    let home = tempfile::tempdir().unwrap();
    let first = home.path().join("one");
    let second = home.path().join("two");
    fs::write(&first, b"inert image data").unwrap();
    fs::write(&second, b"inert image data").unwrap();
    let actual = File::open(&first).unwrap();
    let same = File::open(&first).unwrap();
    let copy = File::open(&second).unwrap();
    assert_eq!(hash(&actual).unwrap(), hash(&copy).unwrap());
    assert!(image_objects_match(&actual, &same).unwrap());
    assert!(!image_objects_match(&actual, &copy).unwrap());
    assert_ne!(
        Object::of(&actual.metadata().unwrap()),
        Object::of(&copy.metadata().unwrap())
    );
}

#[test]
fn inherited_lease_file_is_not_a_directory_seal_descriptor() {
    let home = tempfile::tempdir().unwrap();
    let lease = File::create(home.path().join("lease")).unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let gid = nix::unistd::Gid::effective().as_raw();
    assert!(seal_held_directory(&lease, uid, gid, || Ok(())).is_err());
    assert!(lease.metadata().unwrap().is_file());
}

#[test]
fn held_directory_seal_is_idempotent_before_a_receipt_and_catches_name_replacement() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("owned-directory");
    fs::create_dir(&path).unwrap();
    let held = File::open(&path).unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let gid = nix::unistd::Gid::effective().as_raw();
    let id = Object::of(&held.metadata().unwrap());
    nix::sys::stat::fchmod(&held, nix::sys::stat::Mode::from_bits_truncate(0o775)).unwrap();
    let unchanged = || require(Object::of(&fs::symlink_metadata(&path)?) == id, "changed");
    seal_held_directory(&held, uid, gid, unchanged).unwrap();
    assert_eq!(held.metadata().unwrap().mode() & 0o7777, 0o755);
    seal_held_directory(&held, uid, gid, unchanged).unwrap();
    let mut calls = 0;
    let retired = home.path().join("retired-directory-data");
    let result = seal_held_directory(&held, uid, gid, || {
        calls += 1;
        if calls == 2 {
            fs::rename(&path, &retired)?;
            fs::create_dir(&path)?;
            let replacement = File::open(&path)?;
            nix::sys::stat::fchmod(
                &replacement,
                nix::sys::stat::Mode::from_bits_truncate(0o775),
            )?;
        }
        require(
            Object::of(&fs::symlink_metadata(&path)?) == id,
            "name changed",
        )
    });
    assert!(result.is_err());
    assert_eq!(held.metadata().unwrap().mode() & 0o7777, 0o755);
    assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o7777, 0o775);
}

#[test]
fn noncanonical_terminal_modes_owners_and_special_objects_are_not_normalized() {
    let home = tempfile::tempdir().unwrap();
    let directory = File::open(home.path()).unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let gid = nix::unistd::Gid::effective().as_raw();
    for mode in [0o777, 0o770, 0o700, 0o1775, 0o2775, 0o4775] {
        nix::sys::stat::fchmod(&directory, nix::sys::stat::Mode::from_bits_truncate(mode)).unwrap();
        assert!(
            check_directory_owner(&directory, true, uid, gid).is_err(),
            "{mode:o}"
        );
    }
    nix::sys::stat::fchmod(&directory, nix::sys::stat::Mode::from_bits_truncate(0o755)).unwrap();
    assert!(check_directory_owner(&directory, true, uid.wrapping_add(1), gid).is_err());
    assert!(check_directory_owner(&directory, true, uid, gid.wrapping_add(1)).is_err());
    let data = home.path().join("data");
    fs::write(&data, b"data").unwrap();
    let alias = home.path().join("alias");
    std::os::unix::fs::symlink(&data, &alias).unwrap();
    assert!(open_regular(&alias, uid).is_err());
}

#[test]
fn native_acl_absence_is_distinct_from_unknown_and_extended_or_default_grants() {
    assert!(
        acl_failure(io::Error::from_raw_os_error(nix::libc::ENODATA))
            .unwrap()
            .is_none()
    );
    for errno in [
        nix::libc::EACCES,
        nix::libc::ENOTSUP,
        nix::libc::ERANGE,
        nix::libc::EIO,
    ] {
        assert!(acl_failure(io::Error::from_raw_os_error(errno)).is_err());
    }
    let mut ordinary = 2u32.to_le_bytes().to_vec();
    for tag in [1u16, 4, 32] {
        ordinary.extend_from_slice(&tag.to_le_bytes());
        ordinary.extend_from_slice(&7u16.to_le_bytes());
        ordinary.extend_from_slice(&u32::MAX.to_le_bytes());
    }
    assert!(ordinary_access_acl(&ordinary));
    let mut extended = ordinary.clone();
    extended.extend_from_slice(&[2, 0, 7, 0, 0, 0, 0, 0]);
    assert!(!ordinary_access_acl(&extended));
    ordinary[4] = 2;
    assert!(!ordinary_access_acl(&ordinary));
    assert!(!ordinary_access_acl(&[]));
}

#[test]
fn finite_journal_unknown_duplicate_cross_action_and_recovery_predecessors_fail_closed() {
    for unknown in ["", "staging", "rolled-back", "activation-pending", "future"] {
        assert!(Phase::parse(unknown).is_err());
    }
    assert!(!phase_allowed(Action::Install, Phase::CompletedUninstall));
    assert!(!phase_allowed(
        Action::Uninstall,
        Phase::CommittedActivationPending
    ));
    assert!(!phase_allowed(Action::Uninstall, Phase::CandidateStaged));
    assert!(!phase_allowed(Action::Install, Phase::RecoveryRequired));
    for phase in [
        Phase::Prepared,
        Phase::FenceIntent,
        Phase::StoppedFenced,
        Phase::DirectorySealed,
        Phase::CandidateStaged,
        Phase::CommittedActivationPending,
        Phase::Completed,
    ] {
        assert_eq!(Phase::parse(phase.name()).unwrap(), phase);
        assert!(phase_allowed(Action::Install, phase));
    }
    assert!(parse_fields(b"kind=x\nkind=x\n").is_err());
    assert!(parse_fields(b"unknown=field\n").is_err());
    assert!(parse_fields(b"kind=x").is_err());
    assert!(parse_fields(b"kind=x\r\n").is_err());
    let complete = FIELDS
        .iter()
        .map(|key| format!("{key}=\n"))
        .collect::<String>();
    assert_eq!(
        parse_fields(complete.as_bytes()).unwrap().len(),
        FIELDS.len()
    );
}

#[test]
fn missing_partial_and_malformed_user_baselines_never_become_release_receipts() {
    for input in [
        Vec::new(),
        b"none\n".to_vec(),
        b"a\nb\nc\n".to_vec(),
        vec![0xff],
    ] {
        assert!(baseline(&input).is_err());
    }
    let valid = format!("{}{}\n", "value\n".repeat(9), "/home/owned-user");
    let parsed = baseline(valid.as_bytes()).unwrap();
    assert_eq!(parsed[9], "/home/owned-user");
    assert!(baseline(valid.replace("value\n", "\n").as_bytes()).is_err());
    assert!(baseline(valid.replace("value", "bad\rvalue").as_bytes()).is_err());
}

#[test]
fn no_old_carrier_cli_or_rollback_path_exists_and_all_postcommit_destinations_are_selected() {
    let source = source();
    assert!(!source.contains("Command::new(HELPER)"));
    assert!(!source.contains("rollback("));
    assert!(!source.contains("\"restart\""));
    assert!(!source.contains("upgrade-closed"));
    assert!(source.contains("File::open(\"/proc/self/exe\")"));
    assert!(source.contains("image_objects_match(&actual, installed)?"));
    let activate = body(&source, "fn activate(", "fn remove_business_tree(");
    before(
        activate,
        "journal.set_phase(Phase::CommittedActivationPending)?",
        "publish(\n            journal,\n            \"candidate-unit\",",
    );
    before(
        activate,
        "candidate_valid(journal)?",
        "journal.set_phase(Phase::CommittedActivationPending)?",
    );
    before(
        activate,
        "image_identity(pid, &installed.file)?",
        "crate::admin_probe::retained_ready(",
    );
    assert!(activate.contains("image_identity(pid, &installed.file)? == image"));
}

#[test]
fn persistent_ownmask_and_same_journal_crash_boundaries_keep_actual_object_custody() {
    let source = source();
    let fence = body(&source, "fn fence(", "fn preserve_flat(");
    before(
        fence,
        "journal.set_phase(Phase::FenceIntent)?",
        "fs::rename(&temporary, UNIT)?",
    );
    before(
        fence,
        "fs::rename(&temporary, UNIT)?",
        "journal.close_fenced()?",
    );
    before(
        fence,
        "journal.close_fenced()?",
        "journal.set_phase(Phase::StoppedFenced)",
    );
    assert!(fence.contains("Object::of(&metadata).text() == journal.get(\"mask\")?"));
    let stage = body(&source, "fn stage(", "fn candidate_valid(");
    before(stage, "journal.close_fenced()?", "directory.seal()?");
    before(stage, "directory.seal()?", "crate::core_cli::install(");
    before(
        stage,
        "preserve_flat(journal)?",
        "crate::core_cli::install(",
    );
    assert!(stage.contains("preserve_existing: true"));
    assert!(
        stage.contains("compatible retained helper requires ordinary repair; downgrade refused")
    );
    assert!(source.contains("crate::process_lease::apply(&lease.path, 9, false)"));
    assert!(
        source.contains("incomplete admission journal retained; no new transaction may bypass it")
    );
    assert!(source.contains("unknown recovery predecessor"));
}

#[test]
fn complete_cohort_and_responsible_user_network_reads_are_not_empty_session_inferences() {
    let source = source();
    assert!(source.contains("polaris_helper_client::manager::LINUX_SERVICE_CLOSURE_FUNCTIONS"));
    let closed = body(&source, "fn close_fenced(", "fn parse_fields(");
    before(
        closed,
        "current_namespace == self.get(\"namespace\")?",
        "systemctl(&[\"--system\", \"stop\", SERVICE])?",
    );
    before(closed, "self.manager_masked()?", "\"stop\", SERVICE");
    before(closed, "\"stop\", SERVICE", "shared_cohort_closed()?");
    assert!(source.contains("\"pid\", \"mnt\", \"cgroup\", \"net\", \"user\""));
    assert!(source.contains("owners.len() == 1"));
    assert!(source.contains("\"/usr/sbin/runuser\""));
    assert!(source.contains("captured HOME differs from actual responsible user"));
    assert!(source.contains("for query in [\"dns\", \"domain\", \"default-route\"]"));
    assert!(source.contains("!links.trim().is_empty()"));
    assert!(source.contains("fixed Polaris TUN responsibility remains"));
    assert!(source.contains("fixed resolved link responsibility remains"));
}

#[test]
fn uninstall_tombstone_and_reinstall_preserve_the_fence_and_stable_lease() {
    let source = source();
    let removal = body(&source, "fn uninstall(", "fn retire(");
    assert!(!removal.contains("fs::remove_file(UNIT)"));
    assert!(!removal.contains("helper-install.lease"));
    assert!(!removal.contains("helper-retired."));
    assert!(removal.contains("fresh_release(journal)?"));
    let retired = body(&source, "fn retire(", "fn maintain(");
    before(
        retired,
        "atomic_data(\n            &support,\n            \"helper-legacy-tombstone\",",
        "fs::rename(&journal.directory.path",
    );
    assert!(source.contains("ambiguous tombstone journals"));
    assert!(
        source.contains("Phase::CompletedUninstall && journal.get(\"action\")? == \"uninstall\"")
    );
    assert!(source.contains("foreign or replaced canonical mask"));
    let publication = body(&source, "fn atomic_publication(", "fn baseline(");
    before(publication, "fchmod(&file", "fs::rename(&temporary");
    assert!(publication.contains("atomic publication mode differs"));
}

#[test]
fn malformed_administrative_verbs_never_fall_through_to_daemon_startup() {
    for args in [
        vec!["linux-legacy-unknown".to_owned()],
        vec!["linux-legacy-install".to_owned()],
        vec!["linux-legacy-classify".to_owned(), "--force".into()],
    ] {
        assert_eq!(run(&args), Some(std::process::ExitCode::FAILURE));
    }
    assert_eq!(run(&["--socket=/inert".into()]), None);
    for value in ["", "a:1", "1:2:3", "-1:3"] {
        assert!(Object::parse(value).is_err());
    }
}

#[test]
fn newer_receipt_is_only_a_pre_fence_refusal_and_known_recovery_stays_forward_only() {
    let source = source();
    let preparing = body(&source, "fn prepare(", "fn fence(");
    before(
        preparing,
        "if let Some(path) = find_journal()?",
        "preflight_selected_policy(request)?",
    );
    before(
        preparing,
        "preflight_selected_policy(request)?",
        "fs::DirBuilder::new()",
    );
    let preflight = body(&source, "fn preflight_selected_policy(", "fn prepare(");
    assert!(preflight.contains("crate::helper_policy::may_replace"));
    assert!(preflight.contains("actual == record.sha256"));
    assert!(!preflight.contains("Command::"));
    assert!(crate::helper_policy::may_replace("0.0.1", 0, "1.0.0", 1).unwrap());
    assert!(!crate::helper_policy::may_replace("99.0.0", 1, "1.0.0", 1).unwrap());
}

#[test]
fn admission_lease_precedes_every_transaction_snapshot_and_is_not_reacquired_by_journals() {
    let source = source();
    let maintain = body(&source, "fn maintain(", "pub(crate) fn run(");
    before(
        maintain,
        "acquire_admission_lease()?",
        "prepare(&request, &lease)?",
    );
    let acquire = body(&source, "fn acquire_admission_lease(", "fn maintain(");
    before(
        acquire,
        "crate::process_lease::apply(&lease.path, 9, false)?",
        "Ok(Arc::new(lease))",
    );
    let preparing = body(&source, "fn prepare(", "fn fence(");
    assert!(preparing.contains("lease: Arc::clone(lease)"));
    assert!(!preparing.contains("process_lease::apply"));
    let check = body(&source, "fn check(&self)", "fn save(&self)");
    assert!(check.contains("self.lease.unchanged()"));
    assert!(!check.contains("process_lease::apply"));
    assert_eq!(source.matches("crate::process_lease::apply(").count(), 1);
}

#[test]
fn concurrent_admission_reads_current_objects_only_after_the_existing_lease_is_acquired() {
    use std::sync::{mpsc, Barrier};
    let root = tempfile::tempdir().unwrap();
    let lease_path = root.path().join("owned-lease");
    let owner = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lease_path)
        .unwrap();
    let unit = root.path().join("current-data");
    let active = root.path().join("active-data");
    let prepared = root.path().join("prepared-data");
    fs::write(&unit, b"old").unwrap();
    let old_object = Object::of(&fs::metadata(&unit).unwrap());
    crate::process_lease::apply(&lease_path, owner.as_raw_fd(), false).unwrap();
    let ready = Arc::new(Barrier::new(2));
    let (busy, observed_busy) = mpsc::channel();
    let contender = {
        let (lease_path, unit, active, prepared, ready) = (
            lease_path.clone(),
            unit.clone(),
            active.clone(),
            prepared.clone(),
            ready.clone(),
        );
        std::thread::spawn(move || {
            // Routing may have seen old bytes and no journal. It is not a snapshot.
            assert!(!active.exists());
            assert_eq!(fs::read(&unit).unwrap(), b"old");
            let lease = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lease_path)
                .unwrap();
            let blocked =
                crate::process_lease::apply(&lease_path, lease.as_raw_fd(), false).unwrap_err();
            assert_eq!(blocked.kind(), io::ErrorKind::WouldBlock);
            assert!(!prepared.exists());
            busy.send(()).unwrap();
            ready.wait();
            crate::process_lease::apply(&lease_path, lease.as_raw_fd(), false).unwrap();
            // Every actionable observation is fresh under B's acquired lease.
            assert!(!active.exists());
            let current = Object::of(&fs::metadata(&unit).unwrap());
            assert_eq!(fs::read(&unit).unwrap(), b"new");
            fs::write(&prepared, current.text()).unwrap();
            current
        })
    };
    observed_busy.recv_timeout(Duration::from_secs(5)).unwrap();
    fs::write(&active, b"A").unwrap();
    let candidate = root.path().join("new-data");
    fs::write(&candidate, b"new").unwrap();
    fs::rename(&candidate, &unit).unwrap();
    fs::rename(&active, root.path().join("retired-data")).unwrap();
    drop(owner); // A's last description releases the original stable lease.
    ready.wait();
    let current = contender.join().unwrap();
    assert_ne!(current, old_object);
    assert_eq!(fs::read_to_string(&prepared).unwrap(), current.text());
    assert_eq!(current, Object::of(&fs::metadata(&unit).unwrap()));
}
