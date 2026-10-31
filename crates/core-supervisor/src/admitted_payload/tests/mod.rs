use super::*;

#[tokio::test]
async fn initialization_exits_retire_unregistered_plans_but_keep_actual_child_and_lease() {
    use std::fs::{self, OpenOptions};
    let directories = ["versions", "manifests", ".staging", ".trash", "leases"];
    for failure in std::iter::once("root").chain(directories) {
        let payload = fixture(
            b"#!/bin/sh\nread line\nexit 0\n",
            vec![ExecPurpose::CheckConfig, ExecPurpose::RunMain],
        );
        let check = payload.prepare(ExecPurpose::CheckConfig).unwrap();
        let run = payload.prepare(ExecPurpose::RunMain).unwrap();
        let root = payload
            .candidate
            .binary
            .parent()
            .unwrap()
            .with_extension("init-fault");
        fs::create_dir(&root).unwrap();
        let _cleanup = FixtureDirectory(root.clone());
        let lease_path = root.join("live-child.lease");
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lease_path)
            .unwrap();
        lease.try_lock_shared().unwrap();
        let live = payload
            .prepare(ExecPurpose::RunMain)
            .unwrap()
            .with_store_lease(Arc::new(lease))
            .unwrap();
        let mut command = live.command().unwrap();
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = command.spawn().unwrap();
        drop(command);
        drop(live);
        let slot = Mutex::new(Some(Arc::clone(&payload)));
        let namespace = root.join("namespace");
        if failure == "root" {
            fs::write(&namespace, b"retain unknown root").unwrap();
        }
        let result: std::io::Result<()> = with_retirement_on_exit(&slot, || {
            fs::create_dir(&namespace)?;
            for name in directories {
                if name == failure {
                    fs::write(namespace.join(name), b"retain unknown member")?;
                }
                fs::create_dir(namespace.join(name))?;
            }
            Ok(())
        });
        assert!(result.is_err(), "{failure}");
        // Neither plan was registered as an operation owner. Both must be
        // refused before a spawn command exists: no native birth is attempted.
        assert_eq!(
            check.command().err().unwrap().kind,
            AdmissionErrorKind::Changed
        );
        assert_eq!(
            run.command().err().unwrap().kind,
            AdmissionErrorKind::Changed
        );
        assert!(slot.lock().unwrap().is_none());
        let blocker = if failure == "root" {
            namespace.clone()
        } else {
            namespace.join(failure)
        };
        assert!(fs::read(blocker).unwrap().starts_with(b"retain unknown"));
        assert!(child.try_wait().unwrap().is_none());
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lease_path)
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(child.stdin.take());
        assert!(child.wait().await.unwrap().success());
        probe.try_lock().unwrap();
        probe.unlock().unwrap();
    }
}

#[test]
fn retirement_on_exit_also_runs_on_unwind_and_success() {
    for unwind in [false, true] {
        let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
        let queued = payload.prepare(ExecPurpose::RunMain).unwrap();
        let slot = Mutex::new(Some(payload));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_retirement_on_exit(&slot, || {
                if unwind {
                    panic!("initialization fault");
                }
                17
            })
        }));
        assert_eq!(result.is_err(), unwind);
        assert_eq!(
            queued.command().err().unwrap().kind,
            AdmissionErrorKind::Changed
        );
    }
}

#[test]
fn explicit_retirement_revokes_queued_plans_without_turning_data_into_admission() {
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let execution = payload.prepare(ExecPurpose::RunMain).unwrap();
    payload
        .retired
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(!payload.is_current());
    assert!(execution.command().is_err());
    assert!(payload.prepare(ExecPurpose::RunMain).is_err());
}

#[tokio::test]
async fn operation_lease_survives_native_exec_and_idle_admission_does_not_retain_it() {
    let payload = fixture(
        b"#!/bin/sh\nread line\nexit 0\n",
        vec![ExecPurpose::RunMain],
    );
    let path = std::env::temp_dir().join(format!(
        "polaris-operation-lease-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    lease.try_lock_shared().unwrap();
    let execution = payload
        .prepare(ExecPurpose::RunMain)
        .unwrap()
        .with_store_lease(Arc::new(lease))
        .unwrap();
    let mut command = execution.command().unwrap();
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().unwrap();
    drop(command);
    drop(execution);
    let probe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    assert!(matches!(
        probe.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(child.stdin.take());
    assert!(child.wait().await.unwrap().success());
    probe.try_lock().unwrap();
    probe.unlock().unwrap();
    assert!(
        payload.is_current(),
        "cached admission remains an execution cohort, not a store occupancy owner"
    );
    std::fs::remove_file(path).unwrap();
}
use crate::{SingBoxSpawner, SpawnRequest, StdioPolicy, TokioSpawner};

#[test]
fn warm_issue_reuses_same_protected_cohort_and_changed_source_evicts_before_failure() {
    let original = fixture(b"#!/bin/sh\necho selected\n", vec![ExecPurpose::RunMain]);
    let mut slot = None;
    let input = original.candidate.clone();
    let first = admit_into(&mut slot, input.clone(), ExecPurpose::RunMain, || {
        Ok(original.policy.clone())
    })
    .unwrap();
    let cold = first.protected.counters();
    assert_eq!(cold.0, 2);
    assert_eq!(cold.2, 2);
    assert_eq!(
        cold.1,
        first
            .policy
            .manifest
            .files
            .iter()
            .map(|f| f.bytes)
            .sum::<u64>()
    );
    for _ in 0..30 {
        let next = admit_into(&mut slot, input.clone(), ExecPurpose::RunMain, || {
            panic!("warm cohort reconstructed")
        })
        .unwrap();
        assert!(Arc::ptr_eq(&first, &next));
        next.prepare(ExecPurpose::RunMain).unwrap();
        assert_eq!(next.protected.counters(), cold);
    }
    std::fs::write(&input.binary, b"#!/bin/sh\necho replaced\n").unwrap();
    let error = admit_into(&mut slot, input, ExecPurpose::RunMain, || {
        Ok(original.policy.clone())
    })
    .unwrap_err();
    assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
    assert!(
        slot.is_none(),
        "failed replacement must not lend old cohort"
    );
    assert!(first.prepare(ExecPurpose::RunMain).is_err());
}

#[tokio::test]
async fn prepared_factory_executes_sealed_original_after_namespace_race() {
    let payload = fixture(
        b"#!/bin/sh\nprintf 'original:%s:%s:%s' \"$1\" \"$2\" \"$3\"\n",
        vec![ExecPurpose::RunMain],
    );
    let execution = payload.prepare(ExecPurpose::RunMain).unwrap();
    let mut command = execution.command().unwrap();
    command.args(["run", "-c", "marker"]);
    std::fs::write(
        payload.diagnostic_path(),
        b"#!/bin/sh\nprintf replacement\n",
    )
    .unwrap();
    let output = command.output().await.unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"original:run:-c:marker");
    assert!(!payload.is_current());
    assert!(payload.prepare(ExecPurpose::RunMain).is_err());
}

#[test]
fn purpose_cannot_upgrade_check_only_or_inventory_to_run() {
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::CheckConfig]);
    assert_eq!(
        payload.prepare(ExecPurpose::RunMain).unwrap_err().kind,
        AdmissionErrorKind::UnsupportedCapability
    );
    let execution = payload.prepare(ExecPurpose::CheckConfig).unwrap();
    let error = TokioSpawner::new()
        .spawn(SpawnRequest::new(execution, "unused", StdioPolicy::Discard))
        .unwrap_err();
    assert!(matches!(
        error,
        crate::SpawnError::Admission(AdmissionError {
            kind: AdmissionErrorKind::UnsupportedCapability,
            ..
        })
    ));
}

#[tokio::test]
async fn native_command_clears_ambient_environment_and_only_injects_internal_selector() {
    let payload = fixture(b"#!/bin/sh\nenv\n", vec![ExecPurpose::RunMain]);
    let execution = payload.prepare(ExecPurpose::RunMain).unwrap();
    let mut command = execution.command().unwrap();
    let output = command.output().await.unwrap();
    let env = String::from_utf8(output.stdout).unwrap();
    assert!(env.lines().any(|v| v == "PATH=/usr/bin:/bin"));
    assert!(env
        .lines()
        .any(|v| v.starts_with("POLARIS_CRONET_LIBRARY_FD=")));
    assert!(!env.lines().any(|v| v.starts_with("LD_")
        || v.starts_with("DYLD_")
        || v.starts_with("HOME=")
        || v.starts_with("POLARIS_CRONET_LIBRARY_PATH=")));
}

#[tokio::test]
async fn validation_rejects_wrong_purpose_before_birth() {
    let payload = fixture(
        b"#!/bin/sh\nexit 0\n",
        vec![ExecPurpose::CheckConfig, ExecPurpose::TlsInventory],
    );
    let config =
        std::env::temp_dir().join(format!("polaris-admit-check-{}.json", std::process::id()));
    std::fs::write(&config, b"{}").unwrap();
    let error = crate::run_check_raw(
        payload.prepare(ExecPurpose::TlsInventory).unwrap(),
        &config,
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        crate::ValidationLifecycleError::Admission(_)
    ));
    std::fs::remove_file(config).unwrap();
}

#[test]
fn compiled_selection_never_accepts_foreign_same_named_files_or_disk_manifest() {
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let error = admit_selected(payload.candidate.clone(), ExecPurpose::RunMain).unwrap_err();
    assert!(matches!(
        error.kind,
        AdmissionErrorKind::Mismatch | AdmissionErrorKind::UnsupportedCapability
    ));
    let error = admit_selected(
        UntrustedPayload::bundled("/nonexistent-selected/sing-box"),
        ExecPurpose::RunMain,
    )
    .unwrap_err();
    assert!(matches!(
        error.kind,
        AdmissionErrorKind::Mismatch
            | AdmissionErrorKind::UnsupportedCapability
            | AdmissionErrorKind::Missing
    ));
}

#[test]
fn invalid_source_kinds_and_wrong_members_never_create_an_execution() {
    use std::os::unix::fs::symlink;
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let binary = payload.diagnostic_path();
    let original = binary.with_extension("original");
    std::fs::rename(binary, &original).unwrap();
    symlink(&original, binary).unwrap();
    let error = platform::protect_members(&payload.candidate, &payload.policy).unwrap_err();
    assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
    assert_eq!(error.recovery, RecoveryAction::ReinstallCurrentApp);
    std::fs::remove_file(binary).unwrap();
    std::fs::hard_link(&original, binary).unwrap();
    let error = platform::protect_members(&payload.candidate, &payload.policy).unwrap_err();
    assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
    assert_eq!(error.recovery, RecoveryAction::ReinstallCurrentApp);
}

#[test]
fn same_size_replacement_invalidates_without_issuing_old_owner() {
    let payload = fixture(b"#!/bin/sh\necho original\n", vec![ExecPurpose::RunMain]);
    let path = payload.diagnostic_path();
    let modified = std::fs::metadata(path).unwrap().modified().unwrap();
    let replacement = path.with_extension("replacement");
    std::fs::write(&replacement, b"#!/bin/sh\necho replaced\n").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(&replacement, path).unwrap();
    assert!(!payload.is_current());
    assert!(payload.prepare(ExecPurpose::RunMain).is_err());
}

#[tokio::test]
async fn tls_inventory_reuses_same_admitted_cohort_and_preserves_query_arguments() {
    let payload = fixture(
        b"#!/bin/sh\n[ \"$1 $2\" = 'tools tls-fingerprints' ] || exit 9\nprintf inventory\n",
        vec![ExecPurpose::TlsInventory],
    );
    let before = payload.protected.counters();
    let raw = crate::run_tls_fingerprint_query(
        payload.prepare(ExecPurpose::TlsInventory).unwrap(),
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(
        matches!(raw,crate::RawCheck::Done {success:true,ref stdout,..} if stdout=="inventory")
    );
    assert_eq!(payload.protected.counters(), before);
}

#[test]
fn prepared_handles_remain_parent_cloexec_and_only_child_duplicates_are_inherited() {
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let plan = payload.prepare(ExecPurpose::RunMain).unwrap();
    let command = plan.command().unwrap();
    let selector = command
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("POLARIS_CRONET_LIBRARY_FD"))
        .and_then(|(_, v)| v)
        .unwrap()
        .to_str()
        .unwrap()
        .parse::<i32>()
        .unwrap();
    // fcntl is a pure observation of a descriptor held by this command.
    #[allow(unsafe_code)]
    let flags = unsafe { nix::libc::fcntl(selector, nix::libc::F_GETFD) };
    assert_ne!(flags & nix::libc::FD_CLOEXEC, 0);
}

#[tokio::test]
async fn child_and_independent_stdio_tails_retain_the_same_admitted_owner_until_real_reap() {
    let payload = fixture(b"#!/bin/sh\nprintf owned\n", vec![ExecPurpose::RunMain]);
    let weak = Arc::downgrade(&payload);
    let (send, receive) = tokio::sync::oneshot::channel();
    let request = SpawnRequest::new(
        payload.prepare(ExecPurpose::RunMain).unwrap(),
        "unused",
        StdioPolicy::drain(move |stdout, stderr| {
            let _ = send.send((stdout, stderr));
        }),
    );
    drop(payload);
    let mut child = TokioSpawner::new().spawn(request).unwrap();
    let streams = receive.await.unwrap();
    child.child.wait().await.unwrap();
    drop(child);
    assert!(
        weak.upgrade().is_some(),
        "independent readers must retain their cohort after native reap"
    );
    drop(streams);
    assert!(
        weak.upgrade().is_none(),
        "all owners retire after native reap and tail release"
    );
}

#[test]
fn replaced_parent_directory_cannot_issue_the_prior_cohort() {
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let dir = payload.diagnostic_path().parent().unwrap().to_path_buf();
    let moved = dir.with_extension("moved");
    std::fs::rename(&dir, &moved).unwrap();
    std::fs::create_dir(&dir).unwrap();
    for name in ["sing-box", "libcronet.so"] {
        std::fs::copy(moved.join(name), dir.join(name)).unwrap();
    }
    assert!(!payload.is_current());
    assert!(payload.prepare(ExecPurpose::RunMain).is_err());
    drop(payload);
    std::fs::remove_dir_all(moved).unwrap();
}

#[test]
fn sealed_members_keep_required_access_after_helper_uid_drop_and_mode_change_invalidates() {
    use std::os::unix::fs::PermissionsExt;
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let command = payload
        .prepare(ExecPurpose::RunMain)
        .unwrap()
        .command()
        .unwrap();
    let binary = std::path::Path::new(command.as_std().get_program());
    assert_eq!(
        std::fs::metadata(binary).unwrap().permissions().mode() & 0o005,
        0o005
    );
    let sidecar = command
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == "POLARIS_CRONET_LIBRARY_FD")
        .and_then(|(_, v)| v)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        std::fs::metadata(format!("/proc/self/fd/{sidecar}"))
            .unwrap()
            .permissions()
            .mode()
            & 0o004,
        0o004
    );
    std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(!payload.is_current());
    assert_eq!(
        payload.prepare(ExecPurpose::RunMain).unwrap_err().kind,
        AdmissionErrorKind::Changed
    );
}

#[test]
fn copy_stage_foreign_write_through_old_fd_is_detected_by_only_post_seal_hash() {
    use linux::AdmissionStage;
    use std::os::unix::fs::FileExt;
    let original = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let mut slot = None;
    let wrote = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = wrote.clone();
    let error = linux::with_hook(
        Box::new(move |stage, name, file| {
            if name == "sing-box" && stage == AdmissionStage::CopiedChunk {
                // A writable descriptor opened before sealing, including via proc,
                // must not be mistaken for source bytes observed by a rolling hash.
                let old = file.try_clone()?;
                assert_eq!(old.write_at(b"X", 0)?, 1);
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        }),
        || {
            admit_into(
                &mut slot,
                original.candidate.clone(),
                ExecPurpose::RunMain,
                || Ok(original.policy.clone()),
            )
        },
    )
    .unwrap_err();
    assert!(wrote.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
    assert_eq!(error.member.as_deref(), Some("sing-box"));
    assert!(
        slot.is_none(),
        "a changed protected object cannot produce authority"
    );
    assert!(
        original.is_current(),
        "failed replacement does not revoke a live original owner"
    );
}

#[test]
fn seals_precede_the_only_hash_and_block_even_preexisting_writable_descriptors() {
    use linux::AdmissionStage;
    use std::os::unix::fs::FileExt;
    let original = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let old = std::rc::Rc::new(std::cell::RefCell::new(None::<std::fs::File>));
    let held = old.clone();
    let phases = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let record = phases.clone();
    let protected = linux::with_hook(
        Box::new(move |stage, name, file| {
            if name != "sing-box" {
                return Ok(());
            }
            record.borrow_mut().push(stage);
            if stage == AdmissionStage::BeforeSeal {
                *held.borrow_mut() = Some(file.try_clone()?);
            }
            if stage == AdmissionStage::Sealed {
                let actual = nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_GET_SEALS)
                    .map_err(std::io::Error::from)?;
                let required = nix::fcntl::SealFlag::F_SEAL_WRITE
                    | nix::fcntl::SealFlag::F_SEAL_GROW
                    | nix::fcntl::SealFlag::F_SEAL_SHRINK
                    | nix::fcntl::SealFlag::F_SEAL_SEAL;
                assert_eq!(actual & required.bits(), required.bits());
                assert_eq!(
                    held.borrow()
                        .as_ref()
                        .unwrap()
                        .write_at(b"X", 0)
                        .unwrap_err()
                        .raw_os_error(),
                    Some(nix::libc::EPERM)
                );
            }
            Ok(())
        }),
        || linux::protect_members(&original.candidate, &original.policy),
    )
    .unwrap();
    assert_eq!(
        protected.counters().0,
        2,
        "one full hash for each completed member"
    );
    let phases = phases.borrow();
    assert!(
        phases
            .iter()
            .position(|s| *s == AdmissionStage::Sealed)
            .unwrap()
            < phases
                .iter()
                .position(|s| *s == AdmissionStage::Hashed)
                .unwrap()
    );
    assert_eq!(
        phases
            .iter()
            .filter(|s| **s == AdmissionStage::Hashed)
            .count(),
        1
    );
    assert!(old.borrow().as_ref().unwrap().set_len(1).is_err());
}

struct SharedWriteMap {
    address: *mut nix::libc::c_void,
    length: usize,
}
#[allow(
    unsafe_code,
    reason = "private negative fixture holds a writable shared mapping to prevent sealing"
)]
impl SharedWriteMap {
    fn new(file: &std::fs::File) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let length = file.metadata()?.len() as usize;
        // SAFETY: the supplied private fixture descriptor is live, nonempty and
        // read/write. The mapping remains held until its matching Drop below.
        let address = unsafe {
            nix::libc::mmap(
                std::ptr::null_mut(),
                length,
                nix::libc::PROT_READ | nix::libc::PROT_WRITE,
                nix::libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if address == nix::libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { address, length })
    }
}
#[allow(
    unsafe_code,
    reason = "private negative fixture releases precisely its own shared mapping"
)]
impl Drop for SharedWriteMap {
    fn drop(&mut self) {
        // SAFETY: this address/length came from successful mmap and has not
        // been unmapped or exposed to any other owner.
        let result = unsafe { nix::libc::munmap(self.address, self.length) };
        assert_eq!(result, 0);
    }
}
#[test]
fn existing_writable_mapping_prevents_sealing_and_never_issues_an_owner() {
    use linux::AdmissionStage;
    let original = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let mapping = std::rc::Rc::new(std::cell::RefCell::new(None::<SharedWriteMap>));
    let held = mapping.clone();
    let mut slot = None;
    let error = linux::with_hook(
        Box::new(move |stage, name, file| {
            if name == "sing-box" {
                if stage == AdmissionStage::BeforeSeal {
                    *held.borrow_mut() = Some(SharedWriteMap::new(file)?);
                }
                assert_ne!(
                    stage,
                    AdmissionStage::Hashed,
                    "a writable mapping must prevent immutable admission before hashing"
                );
            }
            Ok(())
        }),
        || {
            admit_into(
                &mut slot,
                original.candidate.clone(),
                ExecPurpose::RunMain,
                || Ok(original.policy.clone()),
            )
        },
    )
    .unwrap_err();
    assert!(mapping.borrow().is_some());
    assert_eq!(error.kind, AdmissionErrorKind::UnsupportedExactObject);
    assert!(slot.is_none());
    drop(mapping);
}

#[test]
fn pre_seal_size_change_is_refused_on_the_same_object() {
    use linux::AdmissionStage;
    let original = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let error = linux::with_hook(
        Box::new(|stage, name, file| {
            if name == "sing-box" && stage == AdmissionStage::BeforeSeal {
                file.set_len(1)?;
            }
            Ok(())
        }),
        || linux::protect_members(&original.candidate, &original.policy),
    )
    .unwrap_err();
    assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
}

#[test]
fn interrupted_copy_cannot_publish_partial_authority_and_clean_retry_rehashes_once() {
    use linux::AdmissionStage;
    let original = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let mut slot = None;
    let error = linux::with_hook(
        Box::new(|stage, _, _| {
            if stage == AdmissionStage::CopiedChunk {
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into());
            }
            Ok(())
        }),
        || {
            admit_into(
                &mut slot,
                original.candidate.clone(),
                ExecPurpose::RunMain,
                || Ok(original.policy.clone()),
            )
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, AdmissionErrorKind::Io);
    assert!(slot.is_none());
    let retried = admit_into(
        &mut slot,
        original.candidate.clone(),
        ExecPurpose::RunMain,
        || Ok(original.policy.clone()),
    )
    .unwrap();
    assert_eq!(retried.protected.counters().0, 2);
    assert_eq!(retried.protected.counters().2, 2);
}

#[test]
fn concurrent_cold_requests_use_the_same_single_flight_and_one_complete_hash_per_member() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let original = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let slot = Arc::new(Mutex::new(None));
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let cold = Arc::new(AtomicUsize::new(0));
    let threads = (0..4)
        .map(|_| {
            let (slot, barrier, cold, input, policy) = (
                slot.clone(),
                barrier.clone(),
                cold.clone(),
                original.candidate.clone(),
                original.policy.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                admit_locked(&slot, input, ExecPurpose::RunMain, || {
                    cold.fetch_add(1, Ordering::SeqCst);
                    Ok(policy)
                })
                .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let owners = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cold.load(Ordering::SeqCst), 1);
    assert!(owners.iter().all(|owner| Arc::ptr_eq(owner, &owners[0])));
    assert_eq!(owners[0].protected.counters().0, 2);
    assert_eq!(owners[0].protected.counters().2, 2);
}

#[test]
fn link_ancestor_and_fifo_damage_have_a_real_reinstall_classification() {
    use std::os::unix::fs::symlink;
    for name in ["sing-box", "libcronet.so"] {
        for fifo in [false, true] {
            let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
            let victim = payload.diagnostic_path().parent().unwrap().join(name);
            let original = victim.with_extension("held-original");
            std::fs::rename(&victim, &original).unwrap();
            if fifo {
                nix::unistd::mkfifo(&victim, nix::sys::stat::Mode::S_IRUSR).unwrap();
            } else {
                symlink(&original, &victim).unwrap();
            }
            let mut slot = None;
            let error = admit_into(
                &mut slot,
                payload.candidate.clone(),
                ExecPurpose::RunMain,
                || Ok(payload.policy.clone()),
            )
            .unwrap_err();
            assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
            assert_eq!(error.recovery, RecoveryAction::ReinstallCurrentApp);
            assert!(slot.is_none());
            assert_eq!(
                payload.prepare(ExecPurpose::RunMain).unwrap_err().kind,
                AdmissionErrorKind::Changed
            );
        }
    }
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
    let directory = payload.diagnostic_path().parent().unwrap();
    let linked = directory.with_extension("linked-ancestor");
    symlink(directory, &linked).unwrap();
    let error = platform::protect_members(
        &UntrustedPayload::bundled(linked.join("sing-box")),
        &payload.policy,
    )
    .unwrap_err();
    std::fs::remove_file(linked).unwrap();
    assert_eq!(error.kind, AdmissionErrorKind::Mismatch);
    assert_eq!(error.recovery, RecoveryAction::ReinstallCurrentApp);
}

#[test]
fn permission_failure_remains_environment_io_without_a_reinstall_promise() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let io = AdmissionError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert_eq!(io.kind, AdmissionErrorKind::Io);
    assert_eq!(io.recovery, RecoveryAction::None);
    if std::fs::metadata("/proc/self").unwrap().uid() == 0 {
        return; // root bypasses DAC; no privileged mutation is needed by this test
    }
    for name in ["sing-box", "libcronet.so"] {
        let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::RunMain]);
        let member = payload.diagnostic_path().parent().unwrap().join(name);
        let original = std::fs::metadata(&member).unwrap().permissions();
        std::fs::set_permissions(&member, std::fs::Permissions::from_mode(0)).unwrap();
        let result = platform::protect_members(&payload.candidate, &payload.policy);
        std::fs::set_permissions(&member, original).unwrap();
        let error = result.unwrap_err();
        assert_eq!(error.kind, AdmissionErrorKind::Io);
        assert_eq!(error.recovery, RecoveryAction::None);
    }
}

#[test]
#[allow(
    unsafe_code,
    reason = "isolated test process owns and reaps its controlled exec descendants"
)]
fn lease_exec_survives_parent_daemon_exit() {
    use std::fs::{OpenOptions, TryLockError};
    use std::os::unix::fs::MetadataExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    const TEST: &str = "admitted_payload::tests::lease_exec_survives_parent_daemon_exit";
    const MODE: &str = "POLARIS_TEST_LEASE_MODE";
    const CONTROL: &str = "POLARIS_TEST_LEASE_CONTROL";
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    struct Adopted(Option<i32>);
    impl Drop for Adopted {
        fn drop(&mut self) {
            if let Some(pid) = self.0.take() {
                // SAFETY: the isolated controller adopted this exact child and
                // no other waiter can reap it or release its PID before us.
                unsafe {
                    nix::libc::kill(pid, nix::libc::SIGKILL);
                }
                let _ = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None);
            }
        }
    }
    let mode = std::env::var(MODE).unwrap_or_default();
    if mode.is_empty() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(MODE, "controller")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    if mode == "daemon" {
        let control = PathBuf::from(std::env::var_os(CONTROL).unwrap());
        // Exercise a source lease in fd0 and closed-stdio descriptor collisions.
        // SAFETY: this exact test is the only fixture in this isolated process.
        unsafe {
            for fd in [0, 1, 2] {
                nix::libc::close(fd);
            }
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .open(control.join("lease"))
            .unwrap();
        lease.try_lock_shared().unwrap();
        let payload = fixture(
            b"#!/bin/sh\nexec /bin/sleep 20\n",
            vec![ExecPurpose::RunMain],
        );
        let plan = payload.prepare(ExecPurpose::RunMain).unwrap();
        let mut command = plan.command().unwrap().into_std();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        linux::attach_store_lease(&mut command, &lease).unwrap();
        let child_fd = command
            .get_envs()
            .find(|(name, _)| *name == std::ffi::OsStr::new("POLARIS_CORE_STORE_LEASE_FD"))
            .and_then(|(_, value)| value)
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<i32>()
            .unwrap();
        assert!(child_fd >= 3);
        let child = command.spawn().unwrap();
        assert_ne!(
            nix::fcntl::fcntl(&lease, nix::fcntl::FcntlArg::F_GETFD).unwrap()
                & nix::libc::FD_CLOEXEC,
            0
        );
        let ready = format!(
            "{}\n{}\n{}\n",
            child.id(),
            child_fd,
            payload.diagnostic_path().parent().unwrap().display()
        );
        std::fs::write(control.join("ready.new"), ready).unwrap();
        std::fs::rename(control.join("ready.new"), control.join("ready")).unwrap();
        let _owners = (child, command, plan, payload, lease);
        loop {
            std::thread::park_timeout(Duration::from_secs(1));
        }
    }
    assert_eq!(mode, "controller");
    // SAFETY: this isolated controller has no unrelated child processes.
    assert_eq!(
        unsafe { nix::libc::prctl(nix::libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let control = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("polaris-exec-lease-{}", std::process::id()));
    std::fs::create_dir(&control).unwrap();
    let _directory = FixtureDirectory(control.clone());
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(control.join("lease"))
        .unwrap();
    // A denied credential transition keeps the ordinary Command exec-error
    // pipe and cannot publish a successful Child or change the parent flags.
    {
        let payload = fixture(b"#!/bin/sh\nexit 99\n", vec![ExecPurpose::RunMain]);
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .open(control.join("lease"))
            .unwrap();
        held.try_lock_shared().unwrap();
        let plan = payload.prepare(ExecPurpose::RunMain).unwrap();
        let mut command = plan.command().unwrap().into_std();
        linux::attach_store_lease(&mut command, &held).unwrap();
        linux::attach_helper_identity(
            &mut command,
            LinuxHelperIdentity {
                uid: u32::MAX,
                gid: u32::MAX,
                groups: Vec::new(),
            },
        );
        assert!(command.spawn().is_err());
        assert_ne!(
            nix::fcntl::fcntl(&held, nix::fcntl::FcntlArg::F_GETFD).unwrap()
                & nix::libc::FD_CLOEXEC,
            0
        );
    }
    {
        let payload = fixture(b"#!/bin/sh\nexit 99\n", vec![ExecPurpose::RunMain]);
        let plan = payload.prepare(ExecPurpose::RunMain).unwrap();
        let mut command = plan.command().unwrap().into_std();
        let mut bad = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(control.join("bad-lease"))
            .unwrap();
        std::io::Write::write_all(&mut bad, b"nonempty").unwrap();
        assert_eq!(
            linux::attach_store_lease(&mut command, &bad)
                .unwrap_err()
                .kind,
            AdmissionErrorKind::Mismatch
        );
        nix::fcntl::fcntl(
            &bad,
            nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
        )
        .unwrap();
        bad.set_len(0).unwrap();
        assert_eq!(
            linux::attach_store_lease(&mut command, &bad)
                .unwrap_err()
                .kind,
            AdmissionErrorKind::LoaderPolicy
        );
        assert_eq!(
            nix::fcntl::fcntl(&bad, nix::fcntl::FcntlArg::F_GETFD).unwrap() & nix::libc::FD_CLOEXEC,
            0
        );
    }
    let mut daemon = Daemon(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(MODE, "daemon")
            .env(CONTROL, &control)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !control.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "daemon did not publish its controlled child"
        );
        assert!(daemon.0.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(10));
    }
    let raw = std::fs::read_to_string(control.join("ready")).unwrap();
    let fields: Vec<_> = raw.lines().collect();
    let pid: i32 = fields[0].parse().unwrap();
    let mut core = Adopted(Some(pid));
    let fd: i32 = fields[1].parse().unwrap();
    while std::fs::read_link(format!("/proc/{pid}/exe"))
        .is_ok_and(|path| path.file_name() != Some(std::ffi::OsStr::new("sleep")))
    {
        assert!(Instant::now() < deadline, "controlled child did not exec");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .unwrap()
            .file_name(),
        Some(std::ffi::OsStr::new("sleep"))
    );
    let inherited = std::fs::metadata(format!("/proc/{pid}/fd/{fd}")).unwrap();
    let original = lease.metadata().unwrap();
    assert_eq!(
        (inherited.dev(), inherited.ino()),
        (original.dev(), original.ino())
    );
    daemon.0.kill().unwrap();
    daemon.0.wait().unwrap();
    // The Helper's complete address space and every parent Arc are now gone.
    assert!(matches!(lease.try_lock(), Err(TryLockError::WouldBlock)));
    // SAFETY: this adopted, unreaped child is still our controlled native birth.
    assert_eq!(unsafe { nix::libc::kill(pid, nix::libc::SIGTERM) }, 0);
    nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None).unwrap();
    core.0 = None;
    lease.try_lock().unwrap();
    let source = PathBuf::from(fields[2]);
    assert_eq!(source.parent(), Some(control.parent().unwrap()));
    assert!(source
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with(&format!("polaris-admitted-fixture-{}-", daemon.0.id())));
    std::fs::remove_dir_all(source).unwrap();
    assert_ne!(
        nix::fcntl::fcntl(&lease, nix::fcntl::FcntlArg::F_GETFD).unwrap() & nix::libc::FD_CLOEXEC,
        0
    );
}

#[test]
fn check_only_capability_cannot_issue_tls_inventory() {
    let payload = fixture(b"#!/bin/sh\nexit 0\n", vec![ExecPurpose::CheckConfig]);
    let error = payload.prepare(ExecPurpose::TlsInventory).unwrap_err();
    assert_eq!(error.kind, AdmissionErrorKind::UnsupportedCapability);
    assert_eq!(error.purpose, Some(ExecPurpose::TlsInventory));
}

#[test]
fn trusted_execution_v1_rejects_unknown_or_missing_fields_and_keeps_row_binding() {
    let source = "a".repeat(40);
    let tree = "b".repeat(40);
    let policy = serde_json::json!({"schema":"polaris-trusted-execution-v1","sourceCommit":source.clone(),"sourceTree":tree.clone(),"loader":"linux-sealed-fd-v1","purposes":["tls-inventory-v1"]});
    let (purposes, _) = SelectedPolicy::declaration(&policy, &source, &tree, "linux").unwrap();
    assert_eq!(purposes, vec![ExecPurpose::TlsInventory]);
    for extra in ["version", "coreSHA", "capabilityHash", "futureField"] {
        let mut changed = policy.clone();
        changed[extra] = serde_json::json!("ignored-before");
        assert_eq!(
            SelectedPolicy::declaration(&changed, &source, &tree, "linux")
                .unwrap_err()
                .kind,
            AdmissionErrorKind::UnsupportedCapability
        );
    }
    for key in ["schema", "sourceCommit", "sourceTree", "loader", "purposes"] {
        let mut changed = policy.clone();
        changed.as_object_mut().unwrap().remove(key);
        assert!(SelectedPolicy::declaration(&changed, &source, &tree, "linux").is_err());
    }
    for (field, value) in [
        ("sourceCommit", "c".repeat(40)),
        ("sourceTree", "d".repeat(40)),
    ] {
        let mut changed = policy.clone();
        changed[field] = serde_json::json!(value);
        assert!(SelectedPolicy::declaration(&changed, &source, &tree, "linux").is_err());
    }
    for purposes in [
        serde_json::json!(["tls-inventory-v1", "tls-inventory-v1"]),
        serde_json::json!(["unknown-purpose"]),
    ] {
        let mut changed = policy.clone();
        changed["purposes"] = purposes;
        assert!(SelectedPolicy::declaration(&changed, &source, &tree, "linux").is_err());
    }
}

#[test]
fn streaming_digest_uses_exact_lowercase_byte_encoding() {
    use sha2::{Digest, Sha256};
    let mut streaming = Sha256::new();
    streaming.update(b"a");
    streaming.update(b"bc");
    let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert_eq!(super::lowercase_hex(&streaming.finalize()), expected);
    assert_eq!(super::sha256_hex(b"abc"), expected);
    assert_eq!(super::lowercase_hex(&[0, 1, 15, 16, 255]), "00010f10ff");
}
