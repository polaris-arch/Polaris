use super::*;
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
