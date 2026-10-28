use super::*;

fn started() -> StopReceipt {
    let facts = PlistFacts {
        label: "com.polaris.helper".into(),
        program: "/Library/PrivilegedHelperTools/com.polaris.helper".into(),
        arguments: vec![
            "/Library/PrivilegedHelperTools/com.polaris.helper".into(),
            "--support".into(),
            "/Library/Application Support/Polaris".into(),
        ],
        support_path: "/Library/Application Support/Polaris".into(),
        core_dir: "/Library/Application Support/Polaris/core".into(),
        singbox: None,
        confdir: None,
        upgrade_closed: false,
        exit_timeout_seconds: Some(20),
        keep_alive: true,
        run_at_load: true,
        inferred_program: true,
    };
    StopReceipt {
        schema: "polaris-mac-migration-stop-v1".into(),
        journal_path: "/Library/Application Support/Polaris/helper-upgrade.ABC123".into(),
        transaction_id: "helper-upgrade.ABC123".into(),
        journal_device: 1,
        journal_inode: 2,
        boot_session: "00000000-0000-0000-0000-000000000001".into(),
        shell: Birth {
            pid: 11,
            ppid: 10,
            pgid: 10,
            effective_uid: 0,
            start_seconds: 1,
            start_microseconds: 2,
        },
        lease_device: 1,
        lease_inode: 3,
        original_enabled: true,
        configuration: Configuration::new(&facts, 20),
        objects: vec![Object {
            path: facts.program,
            device: 1,
            inode: 4,
            size: 32,
            mode: 0o100755,
            directory: false,
            sha256: Some("a".repeat(64)),
        }],
        helper: Birth {
            pid: 100,
            ppid: 1,
            pgid: 100,
            effective_uid: 0,
            start_seconds: 2,
            start_microseconds: 3,
        },
        descendants: vec![],
        managed_runtimes: vec![],
        group_members: vec![],
        completed: false,
        disabled: false,
        job_absent: false,
        census_closed: false,
        endpoint_closed: false,
        endpoint: None,
    }
}
fn complete(original: &StopReceipt) -> StopReceipt {
    let mut receipt = original.clone();
    receipt.completed = true;
    receipt.disabled = true;
    receipt.job_absent = true;
    receipt.census_closed = true;
    receipt.endpoint_closed = true;
    receipt
}
#[test]
fn completed_receipt_requires_every_positive_fact_and_same_capture() {
    let initial = started();
    let receipt = complete(&initial);
    validate_stop_pair(&initial, &receipt).unwrap();
    for field in 0..5 {
        let mut broken = receipt.clone();
        match field {
            0 => broken.completed = false,
            1 => broken.disabled = false,
            2 => broken.job_absent = false,
            3 => broken.census_closed = false,
            _ => broken.endpoint_closed = false,
        }
        assert!(validate_stop_pair(&initial, &broken).is_err());
    }
    for field in 0..8 {
        let mut copied = receipt.clone();
        match field {
            0 => copied.transaction_id.push('X'),
            1 => copied.journal_path.push("other"),
            2 => copied.boot_session.push('X'),
            3 => copied.shell.start_microseconds += 1,
            4 => copied.lease_inode += 1,
            5 => copied.objects[0].inode += 1,
            6 => copied.configuration.arguments.push("--other".into()),
            _ => copied.helper.start_seconds += 1,
        }
        assert!(validate_stop_pair(&initial, &copied).is_err());
    }
}
#[test]
fn stale_socket_identity_is_a_fact_not_an_unlink_permission() {
    let initial = started();
    let mut receipt = complete(&initial);
    receipt.endpoint = Some(Endpoint {
        device: 1,
        inode: 9,
        owner: 0,
        mode: 0o140777,
    });
    validate_stop_pair(&initial, &receipt).unwrap();
    // The endpoint is optional because ENOENT is also a terminal connect result.
    // The receipt still needs endpoint_closed=true in either case.
    receipt.endpoint_closed = false;
    assert!(validate_stop_pair(&initial, &receipt).is_err());
}
#[test]
fn receipt_schema_rejects_extra_duplicate_or_missing_fields() {
    let raw = serde_json::to_string(&started()).unwrap();
    serde_json::from_str::<StopReceipt>(&raw).unwrap();
    let extra = format!("{{\"unrelated_environment\":\"secret\",{}", &raw[1..]);
    assert!(serde_json::from_str::<StopReceipt>(&extra).is_err());
    let duplicate = format!("{{\"schema\":\"forged\",{}", &raw[1..]);
    assert!(serde_json::from_str::<StopReceipt>(&duplicate).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value.as_object_mut().unwrap().remove("shell");
    assert!(serde_json::from_value::<StopReceipt>(value).is_err());
}
#[test]
fn private_receipt_does_not_persist_unparsed_loaded_output() {
    let raw = serde_json::to_string(&started()).unwrap();
    assert!(raw.contains("actual_exit_timeout_seconds"));
    assert!(!raw.contains("loaded_print"));
    assert!(!raw.contains("environment"));
}
#[test]
fn migration_verbs_never_fall_through_to_daemon_arguments() {
    for verb in [
        "migration-stop",
        "migration-recheck",
        "migration-candidate",
        "migration-rollback",
    ] {
        let argv = vec![verb.to_owned()];
        assert!(crate::core_cli::run(&argv).is_some());
    }
    assert!(run(&["--support".into(), "/unused".into()]).is_none());
}
#[test]
fn source_keeps_mutation_after_durable_intent_and_does_not_signal() {
    let source = include_str!("../../mac_migration_guard.rs");
    let stop = source
        .split("fn stop(journal:")
        .nth(1)
        .unwrap()
        .split("fn verify_binding")
        .next()
        .unwrap();
    assert!(
        stop.find("journal.write_new(STARTED").unwrap()
            < stop.find("launchctl(&[\"disable\"").unwrap()
    );
    assert!(
        stop.find("observe_closed(&captured)").unwrap() < stop.find("endpoint_closed(").unwrap()
    );
    assert!(!source.contains("libc::kill"));
    assert!(!source.contains("std::process::Command"));
    assert!(!source.contains("remove_file("));
    assert!(source.contains("CommandCleanup::NativeAndReadersReturned"));
    assert!(source.contains("process_lease::apply(&self.lease.path, 9, false)"));
}

// Pure path-binding/parser fixture. ProtectedPath root/ACL validation remains
// a separate native acceptance requirement; no privileged fixture runs here.
#[test]
fn journal_descriptor_binding_preserves_fixed_loaded_job_identity() {
    let fixed = Expected {
        label: "com.example.helper",
        plist_path: Path::new("/Library/LaunchDaemons/com.example.helper.plist"),
        helper_path: Path::new("/Library/PrivilegedHelperTools/com.example.helper"),
        support_path: Path::new("/Library/Application Support/Example"),
        core_dir: Path::new("/Library/Application Support/Example/core"),
    };
    let mut dictionary = plist::Dictionary::new();
    dictionary.insert("Label".into(), plist::Value::String(fixed.label.into()));
    dictionary.insert("KeepAlive".into(), plist::Value::Boolean(true));
    dictionary.insert("RunAtLoad".into(), plist::Value::Boolean(true));
    let args = vec![
        fixed.helper_path.display().to_string(),
        "--support".into(),
        fixed.support_path.display().to_string(),
        "--coredir".into(),
        fixed.core_dir.display().to_string(),
        "--upgrade-closed".into(),
    ];
    dictionary.insert(
        "ProgramArguments".into(),
        plist::Value::Array(args.iter().cloned().map(plist::Value::String).collect()),
    );
    let path = PathBuf::from("/private/inert-journal/descriptor.closed");
    let stored = journal_expected(&path, &fixed);
    assert_eq!(stored.plist_path, path);
    assert_ne!(stored.plist_path, fixed.plist_path);
    assert_eq!(stored.helper_path, fixed.helper_path);
    assert_eq!(stored.support_path, fixed.support_path);
    assert_eq!(stored.core_dir, fixed.core_dir);
    let mut bytes = Vec::new();
    plist::Value::Dictionary(dictionary)
        .to_writer_xml(&mut bytes)
        .unwrap();
    let facts = launchd::parse_plist(&bytes, &stored).unwrap();
    assert!(facts.upgrade_closed);
    let fixture = include_str!("../../mac_migration_launchd/full-job-print.txt");
    let start = fixture.find("\targuments = {\n").unwrap();
    let end = start + fixture[start..].find("\t}\n").unwrap() + "\t}\n".len();
    let arguments: String = args.iter().map(|arg| format!("\t\t{arg}\n")).collect();
    let print = format!(
        "{}\targuments = {{\n{}\t}}\n{}",
        &fixture[..start],
        arguments,
        &fixture[end..]
    );
    let loaded = launchd::validate_loaded_print(&print, &facts, &fixed).unwrap();
    assert_eq!(loaded.pid, 321);
    let wrong_path = print.replace(
        &fixed.plist_path.display().to_string(),
        &path.display().to_string(),
    );
    assert!(launchd::validate_loaded_print(&wrong_path, &facts, &fixed).is_err());
}

#[test]
fn original_renames_require_same_device_before_any_disable() {
    validate_rename_devices(9, &[9, 9]).unwrap();
    validate_rename_devices(9, &[9, 9, 9]).unwrap();
    for devices in [&[8, 9, 9][..], &[9, 8, 9][..], &[9, 9, 8][..]] {
        assert!(validate_rename_devices(9, devices).is_err());
    }
    let source = include_str!("../../mac_migration_guard.rs");
    let stop = source
        .split("fn stop(journal:")
        .nth(1)
        .unwrap()
        .split("fn verify_binding")
        .next()
        .unwrap();
    assert!(
        stop.find("validate_rename_devices(").unwrap()
            < stop.find("journal.write_new(STARTED").unwrap()
    );
}

#[test]
fn candidate_slot_empty_cannot_hide_observed_runtime_or_children() {
    let process = started().helper.identity();
    require_empty_candidate_cohort(&[], &[], &[]).unwrap();
    assert!(require_empty_candidate_cohort(&[process], &[], &[]).is_err());
    assert!(require_empty_candidate_cohort(&[], &[process], &[]).is_err());
    assert!(require_empty_candidate_cohort(&[], &[], &[process]).is_err());
}

#[test]
fn endpoint_state_distinguishes_absence_wrong_object_and_accepting() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        endpoint_identity(&directory.path().join("absent")).unwrap(),
        None
    );
    let path = directory.path().join("ordinary");
    std::fs::write(&path, b"not a socket").unwrap();
    assert!(endpoint_identity(&path).is_err());
    assert!(matches!(EndpointClosure::Busy, EndpointClosure::Busy));
    assert!(matches!(
        EndpointClosure::Closed(None),
        EndpointClosure::Closed(None)
    ));
}

#[test]
fn preflight_cleanup_requires_exact_absence_not_unknown_or_existing_marker() {
    assert!(require_metadata_absent(Err(io::Error::from(io::ErrorKind::NotFound))).is_ok());
    for kind in [
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::Interrupted,
        io::ErrorKind::Other,
    ] {
        assert!(require_metadata_absent(Err(io::Error::from(kind))).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("started");
    fs::write(&marker, b"intent").unwrap();
    assert!(require_metadata_absent(fs::symlink_metadata(&marker)).is_err());
    let link = directory.path().join("dangling");
    std::os::unix::fs::symlink(directory.path().join("absent"), &link).unwrap();
    assert!(require_metadata_absent(fs::symlink_metadata(link)).is_err());
}
