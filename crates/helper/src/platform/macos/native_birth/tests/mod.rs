use super::*;
#[test]
fn cached_stop_receipt_keeps_the_original_user_owner() {
    let custody = NativeCustody::default();
    let old = target(42, 1);
    custody.slot().stopped.push_back((old, Some(1000)));
    assert!(custody.caller_may_stop(Some(1000), old));
    assert!(custody.caller_may_stop(Some(0), old));
    assert!(!custody.caller_may_stop(Some(1001), old));
    assert!(!custody.caller_may_stop(None, old));
    assert_eq!(custody.stop(old), NativeBirthStop::Stopped { target: old });
}
use polaris_helper_proto::HelperBirthToken;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;

#[derive(Default)]
struct Probe {
    exited: AtomicBool,
    wait_error: Mutex<Option<i32>>,
    waits: AtomicUsize,
    terms: AtomicUsize,
    kills: AtomicUsize,
    drops: AtomicUsize,
}

struct Child(Arc<Probe>);
impl Drop for Child {
    fn drop(&mut self) {
        self.0.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl NativeChild for Child {
    fn try_wait(&mut self) -> io::Result<bool> {
        self.0.waits.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = *self.0.wait_error.lock().unwrap() {
            return Err(io::Error::from_raw_os_error(error));
        }
        Ok(self.0.exited.load(Ordering::SeqCst))
    }
    fn terminate(&mut self) -> io::Result<()> {
        self.0.terms.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn kill(&mut self) -> io::Result<()> {
        self.0.kills.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn target(pid: u32, birth: u8) -> HelperBirthTarget {
    HelperBirthTarget {
        pid: std::num::NonZeroU32::new(pid).unwrap(),
        birth: HelperBirthToken::from_bytes([birth; 16]),
    }
}
fn install(custody: &NativeCustody, target: HelperBirthTarget) -> (Arc<NativeBirth>, Arc<Probe>) {
    custody.admit().unwrap();
    let probe = Arc::new(Probe::default());
    let birth = custody.publish(target, Box::new(Child(Arc::clone(&probe))));
    (birth, probe)
}

#[test]
fn mismatched_birth_same_pid_never_signals_or_retires() {
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    let wrong = target(42, 2);
    assert_eq!(
        custody.stop(wrong),
        NativeBirthStop::Mismatch {
            requested: wrong,
            current: birth.target
        }
    );
    custody.drive(&birth, Instant::now(), &|| panic!("live child has no tail"));
    assert_eq!(probe.terms.load(Ordering::SeqCst), 0);
    assert_eq!(probe.kills.load(Ordering::SeqCst), 0);
    assert!(custody.is_current(&birth));
}

#[test]
fn wait_error_and_worker_drop_keep_exact_custody_and_block_admission() {
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    *probe.wait_error.lock().unwrap() = Some(5);
    custody.drive(&birth, Instant::now(), &|| {
        panic!("wait error cannot run tail")
    });
    let old_target = birth.target;
    drop(birth);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
    assert_eq!(
        custody.admit(),
        Err(NativeBirthStart::NotAdmittedUnknown {
            target: Some(old_target)
        })
    );
    assert_eq!(
        custody.stop(old_target),
        NativeBirthStop::Unknown { target: old_target }
    );
    *probe.wait_error.lock().unwrap() = None;
    probe.exited.store(true, Ordering::SeqCst);
    let birth = custody.current().unwrap();
    custody.drive(&birth, Instant::now(), &|| Ok(()));
    assert_eq!(
        custody.stop(old_target),
        NativeBirthStop::Stopped { target: old_target }
    );
}

#[test]
fn kill_ack_remains_pending_until_native_wait_and_tail_pass() {
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    let now = Instant::now();
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Pending {
            target: birth.target
        }
    );
    custody.drive(&birth, now, &|| panic!("live child has no tail"));
    custody.drive(&birth, now + STOP_GRACE, &|| panic!("kill ack is not exit"));
    assert_eq!(probe.terms.load(Ordering::SeqCst), 1);
    assert_eq!(probe.kills.load(Ordering::SeqCst), 1);
    assert_eq!(
        custody.admit(),
        Err(NativeBirthStart::NotAdmittedPending {
            target: birth.target
        })
    );
    probe.exited.store(true, Ordering::SeqCst);
    custody.drive(&birth, now + STOP_GRACE, &|| Ok(()));
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Stopped {
            target: birth.target
        }
    );
}

#[cfg(unix)]
#[test]
fn echild_permanently_loses_wait_and_signal_authority() {
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    *probe.wait_error.lock().unwrap() = Some(nix::errno::Errno::ECHILD as i32);
    custody.drive(&birth, Instant::now(), &|| panic!("ECHILD is not exit"));
    *probe.wait_error.lock().unwrap() = None;
    probe.exited.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(
            custody.stop(birth.target),
            NativeBirthStop::Unknown {
                target: birth.target
            }
        );
        custody.drive(&birth, Instant::now() + STOP_GRACE, &|| {
            panic!("lost wait cannot be repaired")
        });
    }
    assert_eq!(probe.waits.load(Ordering::SeqCst), 1);
    assert_eq!(probe.terms.load(Ordering::SeqCst), 0);
    assert_eq!(probe.kills.load(Ordering::SeqCst), 0);
    assert!(custody.is_current(&birth));
}

#[test]
fn unfinished_tail_blocks_stopped_and_successor_then_old_cache_is_noop() {
    let custody = Arc::new(NativeCustody::default());
    let (birth, probe) = install(&custody, target(42, 1));
    probe.exited.store(true, Ordering::SeqCst);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let custody_ref = &custody;
        let birth_ref = &birth;
        scope.spawn(move || {
            custody_ref.drive(birth_ref, Instant::now(), &|| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        });
        entered_rx.recv().unwrap();
        assert_eq!(
            custody.stop(birth.target),
            NativeBirthStop::Pending {
                target: birth.target
            }
        );
        assert_eq!(
            custody.admit(),
            Err(NativeBirthStart::NotAdmittedPending {
                target: birth.target
            })
        );
        release_tx.send(()).unwrap();
    });
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Stopped {
            target: birth.target
        }
    );
    let (successor, next_probe) = install(&custody, target(42, 2));
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Stopped {
            target: birth.target
        }
    );
    custody.drive(&birth, Instant::now(), &|| {
        panic!("old callback cannot chown new generation")
    });
    assert!(custody.is_current(&successor));
    assert_eq!(next_probe.terms.load(Ordering::SeqCst), 0);
}

#[test]
fn tail_failure_retains_reaped_birth_and_retry_only_repeats_tail() {
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    birth.attach_log_custody(polaris_log_budget::spawn_pipe_drainers_custodied(
        None::<std::io::Empty>,
        None::<std::io::Empty>,
    ));
    probe.exited.store(true, Ordering::SeqCst);
    custody.drive(&birth, Instant::now(), &|| Err("chown denied".into()));
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Unknown {
            target: birth.target
        }
    );
    assert!(custody.is_current(&birth));
    assert_eq!(probe.waits.load(Ordering::SeqCst), 1);
    custody.drive(&birth, Instant::now(), &|| Ok(()));
    assert_eq!(probe.waits.load(Ordering::SeqCst), 1);
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Stopped {
            target: birth.target
        }
    );
}

#[test]
fn shutdown_during_admitted_spawn_notifies_published_birth() {
    let custody = NativeCustody::default();
    custody.admit().unwrap();
    custody.close_admission();
    assert!(
        !custody.shutdown_complete().unwrap(),
        "admitted unpublished spawn still owns admission"
    );
    let probe = Arc::new(Probe::default());
    let birth = custody.publish(target(42, 1), Box::new(Child(Arc::clone(&probe))));
    custody.drive(&birth, Instant::now(), &|| panic!("live child has no tail"));
    assert_eq!(probe.terms.load(Ordering::SeqCst), 1);
    assert!(custody.admit().is_err());
    assert!(
        !custody.shutdown_complete().unwrap(),
        "kill request is not exit plus tail"
    );
    probe.exited.store(true, Ordering::SeqCst);
    custody.drive(&birth, Instant::now(), &|| Ok(()));
    assert!(custody.shutdown_complete().unwrap());
    assert!(custody.admit().is_err(), "shutdown never reopens admission");
    assert_eq!(
        custody.stop(target(42, 3)),
        NativeBirthStop::Unknown {
            target: target(42, 3)
        }
    );
}

#[test]
fn failed_pre_spawn_admission_can_abort_but_never_reopens_shutdown() {
    let custody = NativeCustody::default();
    custody.admit().unwrap();
    assert!(custody.active());
    custody.abort_start();
    assert!(!custody.active());
    custody.admit().unwrap();
    custody.close_admission();
    custody.abort_start();
    assert!(
        custody.active(),
        "closing remains sticky even after a failed spawn"
    );
    assert!(custody.shutdown_complete().unwrap());
    assert!(custody.admit().is_err());
}

fn poison_slot(custody: &NativeCustody) {
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _slot = custody.slot.lock().unwrap();
        panic!("controlled slot poison");
    }))
    .is_err());
}

#[test]
fn poisoned_slot_cannot_admit_report_empty_or_recover_cached_stop() {
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    probe.exited.store(true, Ordering::SeqCst);
    custody.drive(&birth, Instant::now(), &|| Ok(()));
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Stopped {
            target: birth.target
        }
    );
    poison_slot(&custody);
    assert_eq!(
        custody.status(),
        NativeBirthStatus::Unknown { target: None }
    );
    assert_eq!(
        custody.admit(),
        Err(NativeBirthStart::NotAdmittedUnknown { target: None })
    );
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Unknown {
            target: birth.target
        }
    );
    custody.close_admission();
    assert!(custody.active());
    assert!(custody.shutdown_complete().is_err());
}

#[test]
fn slot_poison_during_tail_keeps_reaped_birth_without_stopped_cache() {
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    probe.exited.store(true, Ordering::SeqCst);
    custody.drive(&birth, Instant::now(), &|| {
        poison_slot(&custody);
        Ok(())
    });
    assert!(custody.current().is_some());
    assert_eq!(
        custody.stop(birth.target),
        NativeBirthStop::Unknown {
            target: birth.target
        }
    );
    assert_eq!(
        custody.admit(),
        Err(NativeBirthStart::NotAdmittedUnknown {
            target: Some(birth.target)
        })
    );
    assert_eq!(probe.waits.load(Ordering::SeqCst), 1);
    custody.drive(&birth, Instant::now(), &|| {
        panic!("poison cannot authorize another tail")
    });
    assert_eq!(probe.waits.load(Ordering::SeqCst), 1);
}

#[test]
fn pinned_admission_binds_owner_and_content_under_the_same_slot_lock() {
    use polaris_helper_proto::core_manifest::{
        CoreFile, CoreId, CoreManifest, InstalledCoreManifest,
    };
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    // Immutable inert bytes are read and hashed on every host, never executed.
    let raw = b"inert-mac-core";
    std::fs::write(directory.path().join("sing-box"), raw).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            directory.path().join("sing-box"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let core = CoreManifest {
        platform: "mac-x64".into(),
        version: "1.15.0-alpha.12".into(),
        provider_fingerprint: "a".repeat(64),
        helper_sha256: None,
        cleaner_sha256: None,
        files: vec![CoreFile {
            name: "sing-box".into(),
            bytes: raw.len() as u64,
            sha256: crate::core_hash::sha256_hex(raw),
        }],
    };
    let id = core.id(crate::core_hash::sha256_hex).unwrap();
    let manifest = InstalledCoreManifest {
        schema: "polaris-core-manifest-v1".into(),
        id: id.as_str().into(),
        app_version: "1.0.0".into(),
        core,
    };
    let verified = Arc::new(
        crate::core_store::VerifiedCore::fixture(
            directory.path(),
            &directory.path().join("lease"),
            manifest,
        )
        .unwrap(),
    );
    let custody = NativeCustody::default();
    let (birth, probe) = install(&custody, target(42, 1));
    birth.attach_core(verified, 1000);
    for (uid, requested) in [
        (1001, id.clone()),
        (1000, CoreId::parse(&"b".repeat(64)).unwrap()),
    ] {
        assert_eq!(
            custody.admit_pinned(uid, &requested),
            Err(NativeBirthStart::NotAdmittedUnknown { target: None })
        );
    }
    assert_eq!(
        custody.admit_pinned(1000, &id),
        Err(NativeBirthStart::Already {
            target: birth.target
        })
    );
    assert_eq!(probe.terms.load(Ordering::SeqCst), 0);
    assert_eq!(probe.kills.load(Ordering::SeqCst), 0);
    assert!(custody.is_current(&birth));
}

#[test]
fn authorization_does_not_wait_for_post_exit_tail_or_invert_its_locks() {
    let custody = Arc::new(NativeCustody::default());
    let (birth, probe) = install(&custody, target(510, 4));
    let (answer, received) = mpsc::channel();
    let barrier = std::sync::Barrier::new(2);
    let timely = std::thread::scope(|scope| {
        // Retain the exact native-child/tail critical section. Release it after
        // the observation even for the old inversion, so this regression cannot
        // deadlock its test process while reporting a failed deadline.
        let owned = birth.owned.lock().unwrap();
        let c = &custody;
        let b = &birth;
        let ready = &barrier;
        scope.spawn(move || {
            ready.wait();
            answer
                .send((
                    c.caller_may_stop(Some(0), b.target),
                    c.caller_allowed(Some(0), None),
                ))
                .unwrap();
        });
        barrier.wait();
        let result = received.recv_timeout(Duration::from_secs(1));
        drop(owned);
        result
    });
    assert_eq!(timely.unwrap(), (true, true));
    probe.exited.store(true, Ordering::SeqCst);
    custody.drive(&birth, Instant::now(), &|| Ok(()));
    assert!(!custody.active());
}

#[test]
fn upgrade_admission_closes_only_empty_custody_and_can_be_cancelled() {
    let custody = NativeCustody::default();
    assert!(custody.upgrade_admission(true));
    assert_eq!(
        custody.admit(),
        Err(NativeBirthStart::NotAdmittedUnknown { target: None })
    );
    assert!(custody.upgrade_admission(false));
    custody.admit().unwrap();
    assert!(
        !custody.upgrade_admission(true),
        "in-flight spawn owns the reservation"
    );
    custody.abort_start();
    let (birth, probe) = install(&custody, target(42, 1));
    assert!(
        !custody.upgrade_admission(true),
        "retained native child blocks upgrade"
    );
    assert!(custody.is_current(&birth));
    assert_eq!(probe.terms.load(Ordering::SeqCst), 0);
    assert_eq!(probe.kills.load(Ordering::SeqCst), 0);
    probe.exited.store(true, Ordering::SeqCst);
    custody.drive(&birth, Instant::now(), &|| Ok(()));
    assert!(!custody.is_current(&birth));
    assert!(custody.upgrade_admission(true));
}

#[test]
fn upgrade_closed_startup_blocks_all_admission_until_explicit_release() {
    let custody = NativeCustody::with_upgrade_closed(true);
    let id = polaris_helper_proto::core_manifest::CoreId::parse(&"a".repeat(64)).unwrap();
    assert!(custody.active());
    for uid in [0, 1000, 1001] {
        assert!(!custody.caller_allowed(Some(uid), Some(&id)));
        assert_eq!(
            custody.admit_pinned(uid, &id),
            Err(NativeBirthStart::NotAdmittedUnknown { target: None })
        );
    }
    assert_eq!(
        custody.admit(),
        Err(NativeBirthStart::NotAdmittedUnknown { target: None })
    );
    custody.abort_start();
    assert!(
        custody.admit().is_err(),
        "aborting a start must not release the upgrade fence"
    );
    assert!(custody.upgrade_admission(false));
    assert!(!custody.active());
    assert!(custody.caller_allowed(Some(1000), Some(&id)));
    assert_eq!(custody.admit_pinned(1000, &id), Ok(()));
}

#[test]
fn ordinary_startup_keeps_admission_open() {
    let custody = NativeCustody::with_upgrade_closed(false);
    assert!(!custody.active());
    assert_eq!(custody.admit(), Ok(()));
}

// Exercise the real wire decoder/dispatch against custody and inert native IO.
struct WireServices {
    custody: NativeCustody,
    legacy: Mutex<Option<crate::platform::macos::handler::ChildHandle>>,
    spawns: AtomicUsize,
}
impl crate::token::TokenStore for WireServices {
    fn token_value(&self) -> String {
        "wire-token".into()
    }
}
impl crate::platform::macos::exec::CommandRunner for WireServices {
    fn run(
        &self,
        _: Duration,
        _: &str,
        _: &[&str],
    ) -> Result<(), crate::platform::macos::exec::RunError> {
        panic!("no native command")
    }
    fn output(
        &self,
        _: Duration,
        _: &str,
        _: &[&str],
    ) -> Result<Vec<u8>, crate::platform::macos::exec::RunError> {
        panic!("no native command")
    }
    fn combined(
        &self,
        _: Duration,
        _: &str,
        _: &[&str],
    ) -> Result<Vec<u8>, crate::platform::macos::exec::RunError> {
        panic!("no native command")
    }
}
impl crate::platform::macos::handler::MacServices for WireServices {
    fn token_store(&self) -> &dyn crate::token::TokenStore {
        self
    }
    fn runner(&self) -> &dyn crate::platform::macos::exec::CommandRunner {
        self
    }
    fn child(&self) -> &Mutex<Option<crate::platform::macos::handler::ChildHandle>> {
        &self.legacy
    }
    fn upgrade_admission(&self, close: bool) -> bool {
        self.custody.upgrade_admission(close)
    }
    fn native_caller_allowed(
        &self,
        uid: Option<u32>,
        id: Option<&polaris_helper_proto::core_manifest::CoreId>,
    ) -> bool {
        self.custody.caller_allowed(uid, id)
    }
    fn native_custody_active(&self) -> bool {
        self.custody.active()
    }
    fn native_start_pinned(
        &self,
        p: &polaris_helper_proto::core_protocol::PinnedStartParams,
        uid: u32,
    ) -> Result<NativeBirthStart, crate::platform::macos::handler::SpawnError> {
        if let Err(outcome) = self.custody.admit_pinned(uid, &p.id) {
            return Ok(outcome);
        }
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let target = target(701, 7);
        self.custody
            .publish(target, Box::new(Child(Arc::new(Probe::default()))));
        Ok(NativeBirthStart::Started {
            target,
            timing: None,
            created: None,
        })
    }
}
fn wire(
    services: &WireServices,
    request: polaris_helper_proto::Request,
    uid: u32,
) -> polaris_helper_proto::Response {
    let frame = polaris_helper_proto::codec::encode_frame(
        polaris_helper_proto::Platform::Mac,
        "wire-token",
        &request,
    );
    let decoded =
        crate::platform::macos::server::decode_request(&frame[1], &mut frame[2..].iter().cloned())
            .unwrap();
    crate::platform::macos::handler::dispatch_from_peer(
        services,
        &crate::platform::macos::handler::MacConfig::new("/inert/support", "/inert/core"),
        &frame[0],
        &decoded,
        Some(uid),
    )
}
fn wire_start() -> polaris_helper_proto::Request {
    polaris_helper_proto::Request::NativeStartPinned(
        polaris_helper_proto::core_protocol::PinnedStartParams {
            id: polaris_helper_proto::core_manifest::CoreId::parse(&"a".repeat(64)).unwrap(),
            common: polaris_helper_proto::StartParams {
                cfg: "/inert/config.json".into(),
                log: String::new(),
                fwd: true,
                parent_pid: None,
            },
        },
    )
}
#[test]
fn queued_wire_cancel_cannot_reopen_shutdown_empty_drained_or_unpublished_birth() {
    use polaris_helper_proto::{Request, Response};
    for pending in [false, true] {
        let services = WireServices {
            custody: NativeCustody::default(),
            legacy: Mutex::new(None),
            spawns: AtomicUsize::new(0),
        };
        if pending {
            services.custody.admit().unwrap();
        }
        services.custody.close_admission();
        assert!(matches!(
            wire(&services, Request::CancelHelperUpgrade, 0),
            Response::Err(_)
        ));
        assert!(matches!(
            wire(&services, wire_start(), 1000),
            Response::Err(_)
        ));
        assert_eq!(services.spawns.load(Ordering::SeqCst), 0);
        if pending {
            let probe = Arc::new(Probe::default());
            let old = services
                .custody
                .publish(target(702, 8), Box::new(Child(probe.clone())));
            services.custody.drive(&old, Instant::now(), &|| Ok(()));
            assert_eq!(probe.terms.load(Ordering::SeqCst), 1);
            probe.exited.store(true, Ordering::SeqCst);
            services.custody.drive(&old, Instant::now(), &|| Ok(()));
            assert_eq!(
                services.custody.stop(old.target),
                NativeBirthStop::Stopped { target: old.target }
            );
        }
        assert!(services.custody.shutdown_complete().unwrap());
        assert!(matches!(
            wire(&services, Request::CancelHelperUpgrade, 0),
            Response::Err(_)
        ));
        assert!(matches!(
            wire(&services, wire_start(), 1000),
            Response::Err(_)
        ));
        assert_eq!(services.spawns.load(Ordering::SeqCst), 0);
    }
}
#[test]
fn ordinary_upgrade_and_migration_wire_cancel_still_release_upgrade_only_fence() {
    use polaris_helper_proto::{Request, Response};
    for migration in [false, true] {
        let services = WireServices {
            custody: NativeCustody::with_upgrade_closed(migration),
            legacy: Mutex::new(None),
            spawns: AtomicUsize::new(0),
        };
        if !migration {
            assert!(matches!(
                wire(&services, Request::PrepareHelperUpgrade, 0),
                Response::Ok(_)
            ));
        }
        assert!(matches!(
            wire(&services, wire_start(), 1000),
            Response::Err(_)
        ));
        assert!(matches!(
            wire(&services, Request::CancelHelperUpgrade, 0),
            Response::Ok(_)
        ));
        assert!(matches!(
            wire(&services, wire_start(), 1000),
            Response::Ok(_)
        ));
        assert_eq!(services.spawns.load(Ordering::SeqCst), 1);
    }
}
