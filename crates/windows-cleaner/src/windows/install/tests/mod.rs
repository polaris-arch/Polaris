use super::*;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

#[test]
fn policy_fixture_host_is_the_native_absolute_system_powershell() {
    let mut path = vec![0u16; 32768];
    // SAFETY: bounded writable system-directory buffer; no PATH/env lookup.
    let n = unsafe { GetSystemDirectoryW(path.as_mut_ptr(), path.len() as u32) };
    assert!(n > 0 && (n as usize) < path.len());
    let host = PathBuf::from(OsString::from_wide(&path[..n as usize]))
        .join(r"WindowsPowerShell\v1.0\powershell.exe");
    assert!(host.is_absolute());
    println!(
        "\nPOLARIS_C05_SYSTEM_PS={}\n",
        serde_json::to_string(host.to_str().unwrap()).unwrap()
    );
}

fn fixture(root: &Path, sid: &str) -> Snapshot {
    let path = root.join("registration");
    let backup = root.join("registration.rollback");
    std::fs::write(&path, b"original fixture bytes").unwrap();
    let sd = format!("D:P(A;;FA;;;{sid})(A;;FA;;;SY)(A;;FR;;;BA)");
    apply_path_security(&path, &sd, false).unwrap();
    let parent = Chain::open(root, Some(sid)).unwrap();
    let file = child(
        parent.last(),
        OsStr::new("registration"),
        false,
        false,
        null_mut(),
    )
    .unwrap();
    let snapshot = Snapshot {
        path: path.clone(),
        backup: backup.clone(),
        present: true,
        hash: Some(hash(&file).unwrap()),
        sddl: Some(
            file_descriptor(&file)
                .unwrap()
                .text(
                    OWNER_SECURITY_INFORMATION
                        | GROUP_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION,
                )
                .unwrap(),
        ),
    };
    drop(file);
    drop(parent);
    std::fs::copy(&path, &backup).unwrap();
    std::fs::write(&path, b"replacement fixture bytes").unwrap();
    apply_path_security(&path, &format!("D:(A;;FA;;;{sid})(A;;FA;;;SY)"), false).unwrap();
    snapshot
}

#[test]
fn original_8_registration_cases_use_real_native_restore() {
    let (sid, _) = token_info().unwrap();
    let mut count = 0;
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
        let dir = tempfile::tempdir().unwrap();
        let mut snapshot = fixture(dir.path(), &sid);
        let sd = format!("D:P(A;;FA;;;{sid})(A;;FA;;;SY)");
        let mut held = None;
        let mut junction = None;
        let before = std::fs::read(&snapshot.path).unwrap();
        match case {
            "missing-destination" => std::fs::remove_file(&snapshot.path).unwrap(),
            "original-absent" => snapshot.present = false,
            "tampered-backup" => std::fs::write(&snapshot.backup, b"tampered").unwrap(),
            "blocked-replacement" => {
                held = Some(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(FILE_SHARE_READ)
                        .open(&snapshot.path)
                        .unwrap(),
                )
            }
            "invalid-security" => snapshot.sddl = Some("not valid SDDL".into()),
            "parent-is-file" => {
                let parent = dir.path().join("file-parent");
                std::fs::write(&parent, b"parent").unwrap();
                snapshot.path = parent.join("registration");
            }
            "reparse-parent" => {
                // Native test-only mount-point creator already lives in the
                // sibling custody fixture; no cmd/mklink or privilege mutation.
                let target = dir.path().join("real-parent");
                std::fs::create_dir(&target).unwrap();
                std::fs::write(target.join("sentinel"), b"untouched").unwrap();
                let link = dir.path().join("junction");
                super::super::tests::create_junction(&link, &target).unwrap();
                snapshot.path = link.join("registration");
                junction = Some((link, target));
            }
            _ => {}
        }
        let result = restore_snapshot_with_policy(&snapshot, true, Some(&sid), &sd);
        drop(held);
        match case {
            "present" | "missing-destination" => {
                result.unwrap();
                let parent = Chain::open(dir.path(), Some(&sid)).unwrap();
                let file = child(
                    parent.last(),
                    OsStr::new("registration"),
                    false,
                    false,
                    null_mut(),
                )
                .unwrap();
                assert_eq!(
                    hash(&file).unwrap(),
                    snapshot.hash.as_ref().unwrap().as_str()
                );
                assert_eq!(
                    file_descriptor(&file)
                        .unwrap()
                        .text(
                            OWNER_SECURITY_INFORMATION
                                | GROUP_SECURITY_INFORMATION
                                | DACL_SECURITY_INFORMATION
                        )
                        .unwrap(),
                    snapshot.sddl.as_ref().unwrap().as_str()
                );
            }
            "original-absent" => {
                result.unwrap();
                assert!(!snapshot.path.exists());
            }
            _ => {
                assert!(result.is_err(), "{case}");
                assert!(snapshot.backup.exists());
                if case == "tampered-backup" {
                    assert_eq!(std::fs::read(&snapshot.path).unwrap(), before);
                }
            }
        }
        if let Some((link, target)) = junction {
            assert_eq!(
                std::fs::read(target.join("sentinel")).unwrap(),
                b"untouched"
            );
            std::fs::remove_dir(link).unwrap();
        }
        count += 1;
    }
    assert_eq!(count, 8);
    println!("Passed 8 native registration fixture cases.");
}

#[test]
fn source_custody_rejects_old_writer_hardlink_and_ancestor_reparse() {
    let (sid, _) = token_info().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    std::fs::write(&path, b"selected bytes").unwrap();
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(&path)
        .unwrap();
    assert!(open_security(&path, false).is_err());
    drop(writer);
    let chain = Chain::open(dir.path(), Some(&sid)).unwrap();
    let file = child(chain.last(), OsStr::new("source"), false, false, null_mut()).unwrap();
    let id = object(&file).unwrap();
    assert!(std::fs::write(&path, b"changed bytes").is_err());
    assert!(std::fs::rename(&path, dir.path().join("moved")).is_err());
    assert!(std::fs::rename(dir.path(), dir.path().with_extension("moved")).is_err());
    assert_eq!(object(&file).unwrap(), id);
    drop(file);
    drop(chain);
    std::fs::hard_link(&path, dir.path().join("hardlink")).unwrap();
    let chain = Chain::open(dir.path(), Some(&sid)).unwrap();
    assert!(child(chain.last(), OsStr::new("source"), false, false, null_mut()).is_err());
    drop(chain);
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    super::super::tests::create_junction(&link, &target).unwrap();
    assert!(Chain::open(&link, Some(&sid)).is_err());
    std::fs::remove_dir(link).unwrap();
}

#[test]
fn publication_chain_rejects_old_directory_writer_and_retains_ancestor_custody() {
    let (sid, _) = token_info().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let name = wide(dir.path().as_os_str());
    // SAFETY: fresh owned fixture directory and an ordinary old writer handle.
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_ADD_FILE | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    let writer = unsafe { File::from_raw_handle(raw.cast()) };
    assert!(publication_chain(dir.path(), Some(&sid)).is_err());
    drop(writer);

    let guard = publication_chain(dir.path(), Some(&sid)).unwrap();
    assert!(std::fs::rename(dir.path(), dir.path().with_extension("moved")).is_err());
    let old = dir.path().join("old");
    let new = dir.path().join("new");
    std::fs::write(&old, b"old fixture").unwrap();
    std::fs::write(&new, b"restored fixture").unwrap();
    // The retained parent permits normal atomic publication, without releasing
    // its rename fence or opting into POSIX/ignore-merge behavior.
    bool_ok(unsafe {
        ReplaceFileW(
            wide(old.as_os_str()).as_ptr(),
            wide(new.as_os_str()).as_ptr(),
            null(),
            0,
            null(),
            null(),
        )
    })
    .unwrap();
    assert_eq!(std::fs::read(&old).unwrap(), b"restored fixture");
    assert!(!new.exists());
    assert!(std::fs::rename(dir.path(), dir.path().with_extension("moved")).is_err());
    drop(guard);
}

fn source_fixture() -> (tempfile::TempDir, Request) {
    use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest};
    let dir = tempfile::tempdir().unwrap();
    let (sid, _) = token_info().unwrap();
    let digest = |b: &[u8]| hex::encode(sha2::Sha256::digest(b));
    let bytes = [
        ("sing-box.exe", b"inert core".as_slice()),
        ("libcronet.dll", b"inert sidecar".as_slice()),
        ("polaris-helper.exe", b"inert helper".as_slice()),
        (IMAGE_NAME, b"inert cleaner".as_slice()),
    ];
    for (name, data) in bytes {
        std::fs::write(dir.path().join(name), data).unwrap();
    }
    let core = CoreManifest {
        platform: "win".into(),
        version: "1.0.0".into(),
        provider_fingerprint: "a".repeat(64),
        files: bytes[..2]
            .iter()
            .map(|(name, data)| CoreFile {
                name: (*name).into(),
                bytes: data.len() as u64,
                sha256: digest(data),
            })
            .collect(),
        helper_sha256: Some(digest(bytes[2].1)),
        cleaner_sha256: Some(digest(bytes[3].1)),
    };
    let id = core.id(digest).unwrap().as_str().to_owned();
    let manifest = InstalledCoreManifest {
        schema: "polaris-core-manifest-v1".into(),
        id: id.clone(),
        app_version: "1.0.0".into(),
        core,
    };
    let raw = manifest.to_json().unwrap();
    std::fs::write(dir.path().join("core-manifest.json"), &raw).unwrap();
    let request = Request {
        stage: dir.path().to_str().unwrap().into(),
        original_sid: sid,
        config: dir.path().to_str().unwrap().into(),
        core_id: id,
        manifest_sha256: digest(&raw),
        helper_sha256: digest(bytes[2].1),
        cleaner_sha256: digest(bytes[3].1),
        token: "a".repeat(64),
        stage_identity: [0; 3],
        source_identities: vec![],
    };
    (dir, request)
}

#[test]
fn production_source_admission_binds_ids_bytes_and_alternate_admin_read_acl() {
    let (dir, mut request) = source_fixture();
    let source = Source::open(&request, true).unwrap();
    request.source_identities = source.ids().unwrap();
    request.stage_identity = object(source.chain.last()).unwrap();
    let admitted = Source::open(&request, false).unwrap();
    assert_eq!(admitted.ids().unwrap(), request.source_identities);
    drop(admitted);
    let mut wrong = request.clone();
    wrong.source_identities[0].object[2] ^= 1;
    assert!(Source::open(&wrong, false).is_err());
    wrong = request.clone();
    wrong.stage_identity[2] ^= 1;
    assert!(Source::open(&wrong, false).is_err());
    wrong = request.clone();
    wrong.original_sid = "S-1-5-21-1-2-3-9999".into();
    assert!(Source::open(&wrong, true).is_err());
    assert!(std::fs::write(dir.path().join("polaris-helper.exe"), b"foreign helper").is_err());
    assert!(
        std::fs::rename(dir.path(), dir.path().with_extension("moved")).is_err(),
        "LIST_DIRECTORY full-chain guard must block ancestor rename; attributes-only is not evidence"
    );
    let text = file_descriptor(source.chain.last())
        .unwrap()
        .text(DACL_SECURITY_INFORMATION)
        .unwrap();
    assert!(text.contains("BA"));
    // Actual alternate-admin token execution is a separate UAC window. This
    // source test proves the explicit BA read ACL, not that device execution.
    drop(source);
    std::fs::write(dir.path().join("libcronet.dll"), b"foreign sidecar").unwrap();
    assert!(Source::open(&request, true).is_err());
}

#[test]
fn worker_without_a_bin_anchor_rejects_before_admin_or_request_io() {
    assert_eq!(
        run_install_worker(Path::new(r"C:\nonexistent-c05\install-request.json"), None),
        81
    );
    assert_eq!(
        run_install_worker(
            Path::new(r"C:\nonexistent-c05\install-request.json"),
            Some("{}")
        ),
        81
    );
}

// Only the input namespace and inert utility effects are fixture-owned. Every
// snapshot/restore/retire call below is the actual production FileTransaction.
pub(crate) struct FixtureFiles {
    files: FileTransaction,
    original_sddl: String,
    _root: tempfile::TempDir,
}
impl FixtureFiles {
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let (sid, _) = token_info().unwrap();
        let support = root.path().join("support");
        let registration = support
            .join("users")
            .join(format!("s{sid}"))
            .join("registration");
        let files = FileTransaction {
            support_path: support.clone(),
            registration_path: registration.clone(),
            caller: Some(sid.clone()),
            private_sd: format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FR;;;BA)"),
            support: None,
            tx: None,
            lock: None,
            snapshots: vec![],
            registration: None,
        };
        for dir in [
            &support,
            &support.join("users"),
            registration.parent().unwrap(),
        ] {
            files.secure_dir(dir, false).unwrap();
        }
        for (name, bytes) in [
            ("polaris-helper.exe", "original payload"),
            (IMAGE_NAME, "original cleaner"),
            ("helper-installed.json", "original metadata"),
        ] {
            let path = support.join(name);
            std::fs::write(&path, bytes).unwrap();
            apply_path_security(&path, &files.private_sd, false).unwrap();
        }
        std::fs::write(&registration, "original registration").unwrap();
        apply_path_security(
            &registration,
            &format!("D:P(A;;FA;;;{sid})(A;;FA;;;SY)(A;;FR;;;BA)"),
            false,
        )
        .unwrap();
        let reg = open_security(&registration, false).unwrap();
        let original_sddl = file_descriptor(&reg)
            .unwrap()
            .text(
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            )
            .unwrap();
        drop(reg);
        // Input bytes only; never an executable probe or policy bypass.
        for (name, bytes) in [
            ("new-helper", "replacement payload"),
            ("new-cleaner", "replacement cleaner"),
        ] {
            std::fs::write(root.path().join(name), bytes).unwrap();
        }
        Self {
            files,
            original_sddl,
            _root: root,
        }
    }
    pub(crate) fn prepare(&mut self) -> io::Result<()> {
        self.files.prepare()?;
        for (input, name) in [
            ("new-helper", "polaris-helper.exe"),
            ("new-cleaner", IMAGE_NAME),
        ] {
            let c = Chain::open(self._root.path(), self.files.caller())?;
            let f = child(c.last(), OsStr::new(input), false, false, null_mut())?;
            self.files
                .copy_to(&f, &self.files.tx()?.join(name), Some(&hash(&f)?))?;
        }
        Ok(())
    }
    pub(crate) fn snapshot(&mut self, replace: bool) -> io::Result<()> {
        self.files.snapshot(replace)
    }
    pub(crate) fn register_utility_effect(&self) -> io::Result<()> {
        // Inert replacement for external register-user, not a rollback routine.
        std::fs::write(&self.files.registration_path, "replacement registration")?;
        apply_path_security(
            &self.files.registration_path,
            &format!("D:(A;;FA;;;{})(A;;FA;;;SY)", self.files.caller().unwrap()),
            false,
        )
    }
    pub(crate) fn replace_payload(&mut self) -> io::Result<()> {
        self.files.replace_payload(
            &hex::encode(sha2::Sha256::digest(b"replacement payload")),
            &hex::encode(sha2::Sha256::digest(b"replacement cleaner")),
        )?;
        // Inert helper-record utility response/effect after the real copier.
        std::fs::write(
            self.files.support_path.join("helper-installed.json"),
            "replacement metadata",
        )
    }
    pub(crate) fn restore_payload(&mut self) -> io::Result<()> {
        self.files.restore_payload()
    }
    pub(crate) fn restore_registration(&mut self) -> io::Result<()> {
        self.files.restore_registration()?;
        self.assert_restored();
        Ok(())
    }
    pub(crate) fn finish(&mut self, keep: bool) -> io::Result<()> {
        let result = self.files.finish(keep);
        assert!(
            self.files.lock.is_none(),
            "real filesystem finally released its lease"
        );
        result
    }
    pub(crate) fn tx_present(&self) -> bool {
        self.files
            .tx
            .as_ref()
            .is_some_and(|p| entry_present(p).expect("cannot prove transaction presence"))
    }
    pub(crate) fn path(&self, name: &str) -> PathBuf {
        match name {
            "payload" => self.files.support_path.join("polaris-helper.exe"),
            "registration" => self.files.registration_path.clone(),
            "TX/payload.rollback" => self.files.tx().unwrap().join("polaris-helper.exe.rollback"),
            "TX/registration.rollback" => self.files.tx().unwrap().join("registration.rollback"),
            _ => panic!("unknown fixture alias"),
        }
    }
    fn assert_restored(&self) {
        for (name, bytes) in [
            ("polaris-helper.exe", "original payload"),
            (IMAGE_NAME, "original cleaner"),
            ("helper-installed.json", "original metadata"),
        ] {
            assert_eq!(
                std::fs::read_to_string(self.files.support_path.join(name)).unwrap(),
                bytes
            );
        }
        assert_eq!(
            std::fs::read_to_string(&self.files.registration_path).unwrap(),
            "original registration"
        );
        let c = Chain::open(
            self.files.registration_path.parent().unwrap(),
            self.files.caller(),
        )
        .unwrap();
        let f = child(
            c.last(),
            OsStr::new("registration"),
            false,
            false,
            null_mut(),
        )
        .unwrap();
        assert_eq!(
            file_descriptor(&f)
                .unwrap()
                .text(
                    OWNER_SECURITY_INFORMATION
                        | GROUP_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION
                )
                .unwrap(),
            self.original_sddl
        );
    }
    pub(crate) fn assert_backups(&self) {
        for snapshot in self
            .files
            .snapshots
            .iter()
            .chain(self.files.registration.iter())
        {
            let c = Chain::open(snapshot.backup.parent().unwrap(), self.files.caller()).unwrap();
            let f = child(
                c.last(),
                snapshot.backup.file_name().unwrap(),
                false,
                false,
                null_mut(),
            )
            .unwrap();
            security(&f, self.files.caller(), false).unwrap();
            assert_eq!(hash(&f).unwrap(), snapshot.hash.as_ref().unwrap().as_str());
        }
        assert_eq!(
            std::fs::read_to_string(self.path("TX/payload.rollback")).unwrap(),
            "original payload"
        );
        assert_eq!(
            std::fs::read_to_string(self.path("TX/registration.rollback")).unwrap(),
            "original registration"
        );
        assert_eq!(
            self.files.registration.as_ref().unwrap().sddl.as_deref(),
            Some(self.original_sddl.as_str())
        );
        let journal: Snapshot = serde_json::from_slice(
            &std::fs::read(self.files.tx().unwrap().join("registration.snapshot.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(journal.sddl.as_deref(), Some(self.original_sddl.as_str()));
    }
}

#[test]
fn service_handle_access_contract_is_fresh_only_and_includes_config_change() {
    assert_eq!(FRESH_SERVICE_ACCESS, 0x01070037);
    assert_ne!(
        FRESH_SERVICE_ACCESS & SERVICE_CHANGE_CONFIG,
        0,
        "ChangeServiceConfig2 requires this handle bit"
    );
    assert_eq!(
        EXISTING_SERVICE_ACCESS
            & (SERVICE_CHANGE_CONFIG | DELETE | WRITE_DAC | ACCESS_SYSTEM_SECURITY),
        0
    );
    let source = polaris_source_probe::crate_source!("windows/install.rs");
    let existing = source
        .split("fn inspect(")
        .nth(1)
        .unwrap()
        .split("fn prepare(")
        .next()
        .unwrap();
    let fresh = source
        .split("fn create(")
        .nth(1)
        .unwrap()
        .split("fn configure_fresh(")
        .next()
        .unwrap();
    assert!(existing.contains("EXISTING_SERVICE_ACCESS"));
    assert!(!existing.contains("FRESH_SERVICE_ACCESS"));
    assert!(fresh.contains("FRESH_SERVICE_ACCESS"));
}

#[test]
fn production_file_transaction_restores_all_payloads_sd_and_retires_actual_tx() {
    let mut f = FixtureFiles::new();
    f.prepare().unwrap();
    f.snapshot(true).unwrap();
    f.assert_backups();
    let tx = f.files.tx().unwrap().to_path_buf();
    f.register_utility_effect().unwrap();
    f.replace_payload().unwrap();
    f.restore_payload().unwrap();
    f.restore_registration().unwrap();
    f.finish(false).unwrap();
    assert!(!entry_present(&tx).unwrap());
    assert!(!f.tx_present());
}
#[test]
fn production_file_transaction_rejects_bad_backup_locked_restore_and_reparse_retirement() {
    for case in ["backup-hash", "locked-registration", "reparse-retirement"] {
        let mut f = FixtureFiles::new();
        f.prepare().unwrap();
        f.snapshot(true).unwrap();
        let tx = f.files.tx().unwrap().to_path_buf();
        if case == "backup-hash" {
            std::fs::write(f.path("TX/payload.rollback"), "tampered backup").unwrap();
            assert!(f.restore_payload().is_err());
            f.finish(true).unwrap();
        } else if case == "locked-registration" {
            f.register_utility_effect().unwrap();
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(f.path("registration"))
                .unwrap();
            assert!(f.files.restore_registration().is_err());
            drop(lock);
            f.finish(true).unwrap();
            f.assert_backups();
        } else {
            let external = f._root.path().join("external");
            std::fs::create_dir(&external).unwrap();
            std::fs::write(external.join("sentinel"), "keep").unwrap();
            let link = tx.join("junction");
            super::super::tests::create_junction(&link, &external).unwrap();
            assert!(f.finish(false).is_err());
            assert_eq!(
                std::fs::read_to_string(external.join("sentinel")).unwrap(),
                "keep"
            );
            // Explicit fixture junction removal only after proving production
            // retirement refused it; never traverses the external directory.
            std::fs::remove_dir(link).unwrap();
            f.assert_backups();
        }
        assert!(entry_present(&tx).unwrap(), "{case}");
        assert!(f.files.lock.is_none());
    }
}

#[test]
fn utility_pipe_child() {
    let Some(mode) = std::env::var_os("POLARIS_C05_PIPE_CHILD") else {
        return;
    };
    if let Some(path) = std::env::var_os("POLARIS_C05_PIPE_READY") {
        std::fs::write(path, b"C05_READY").unwrap();
    }
    if mode == "flood-and-wait" {
        // More than a pipe buffer on both streams. Failure of the second reader
        // must not allow stderr backpressure to strand wait or the first reader.
        let _ = std::io::stdout().write_all(&[b'o'; 131072]);
        let _ = std::io::stderr().write_all(&[b'e'; 131072]);
        std::thread::sleep(Duration::from_secs(60));
    } else {
        println!("INERT_READER_OUT");
        eprintln!("INERT_READER_ERR");
        std::process::exit(7);
    }
}
struct FailingReaderThreads {
    attempts: usize,
    fail_at: usize,
}
impl ReaderThreads for FailingReaderThreads {
    fn spawn(&mut self, name: &'static str, job: ReaderJob) -> io::Result<ReaderHandle> {
        self.attempts += 1;
        if self.attempts == self.fail_at {
            return Err(io::Error::from_raw_os_error(ERROR_NOT_ENOUGH_MEMORY as i32));
        }
        NativeReaderThreads.spawn(name, job)
    }
}
fn pipe_child(mode: &str, ready: Option<&Path>) -> std::process::Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "windows::install::tests::utility_pipe_child",
            "--nocapture",
        ])
        .env("POLARIS_C05_PIPE_CHILD", mode)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = ready {
        command.env("POLARIS_C05_PIPE_READY", path);
    }
    command.spawn().unwrap()
}
fn await_pipe_ready(owned: &mut UtilityChild<'_>, path: &Path) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            owned.child.try_wait().unwrap().is_none(),
            "fixture exited before entering its body"
        );
        if std::fs::read(path).is_ok_and(|bytes| bytes == b"C05_READY") {
            break;
        }
        assert!(Instant::now() < until, "owned inert child never entered");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn either_reader_creation_failure_reaps_real_owned_child_and_joins_every_created_reader() {
    for failed in [1, 2] {
        let uncertain = std::cell::Cell::new(false);
        let ready = tempfile::tempdir().unwrap();
        let marker = ready.path().join("ready");
        let mut owned = UtilityChild::new(pipe_child("flood-and-wait", Some(&marker)), &uncertain);
        await_pipe_ready(&mut owned, &marker);
        let mut factory = FailingReaderThreads {
            attempts: 0,
            fail_at: failed,
        };
        let started = Instant::now();
        let result = owned.capture(&mut factory);
        assert_eq!(
            result.unwrap_err().raw_os_error(),
            Some(ERROR_NOT_ENOUGH_MEMORY as i32)
        );
        assert!(uncertain.get());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(factory.attempts, failed);
        assert_eq!(owned.joined, failed - 1);
        assert!(owned.terminal && owned.readers.iter().all(Option::is_none));
        // Actual original HANDLE state, not an injected terminal result or PID.
        assert_eq!(
            unsafe { WaitForSingleObject(owned.child.as_raw_handle().cast(), 0) },
            WAIT_OBJECT_0
        );
        assert!(owned.child.try_wait().unwrap().is_some());
    }
}
#[test]
fn owned_utility_success_and_early_drop_both_collect_real_child_and_readers() {
    let uncertain = std::cell::Cell::new(false);
    let mut owned = UtilityChild::new(pipe_child("emit-and-exit", None), &uncertain);
    let (status, stdout) = owned.capture(&mut NativeReaderThreads).unwrap();
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8(stdout)
        .unwrap()
        .contains("INERT_READER_OUT"));
    assert!(!uncertain.get());
    assert_eq!(owned.joined, 2);
    assert!(owned.terminal);
    assert_eq!(
        unsafe { WaitForSingleObject(owned.child.as_raw_handle().cast(), 0) },
        WAIT_OBJECT_0
    );
    // Drop before capture is a real error/unwind boundary. Retain an independent
    // SYNCHRONIZE handle only for observing the original object after cleanup.
    let ready = tempfile::tempdir().unwrap();
    let marker = ready.path().join("ready");
    let flag = std::cell::Cell::new(false);
    let mut owned = UtilityChild::new(pipe_child("flood-and-wait", Some(&marker)), &flag);
    await_pipe_ready(&mut owned, &marker);
    let mut duplicate = null_mut();
    // SAFETY: duplicate the original owned process object, never reopen a PID.
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                owned.child.as_raw_handle().cast(),
                GetCurrentProcess(),
                &mut duplicate,
                SYNCHRONIZE,
                0,
                0,
            )
        },
        0
    );
    let observed = OwnedHandle(duplicate);
    drop(owned);
    assert!(flag.get());
    assert_eq!(unsafe { WaitForSingleObject(observed.0, 0) }, WAIT_OBJECT_0);
}
