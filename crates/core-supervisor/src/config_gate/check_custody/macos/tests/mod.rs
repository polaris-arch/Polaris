//! Registry-only negative controls. No Root provider/Core fixture is executed.
use super::*;

#[test]
fn remote_cancel_is_intent_and_unknown_cannot_retire_its_original_row() {
    let custody = CheckCustody::default();
    let request = CheckRequestRef {
        id: 1,
        issuer: Arc::clone(&custody.issuer),
        identity: Arc::new(CheckRequestIdentity),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(true));
    custody.state.lock().unwrap().runs.insert(
        1,
        CheckRun {
            mac: None,
            mac_child: None,
            payload: None,
            lease_owner: None,
            completion: Arc::new(CheckCompletion::default()),
            request: request.clone(),
            factory: NativeFactoryObservation::Entered,
            native_exited: None,
            native_terminal: None,
            child: None,
            tail: CheckTail::MacRemote {
                cancel: Arc::clone(&cancel),
                unknown: failed,
            },
            exit: None,
            debt: true,
            lost_wait_ownership: false,
            io: Arc::new(NativeIo),
        },
    );
    custody.request_close(&request).unwrap();
    assert!(cancel.load(Ordering::SeqCst));
    assert!(custody.retire(&request).is_err());
    assert!(custody.assert_admission().is_err());
    let s = custody.state.lock().unwrap();
    let run = &s.runs[&1];
    assert!(run.request.same(&request));
    assert!(run.native_terminal.is_none());
    assert!(!run.completion.terminal.load(Ordering::SeqCst));
}

#[test]
fn foreign_issuer_cannot_cancel_even_an_unborn_remote_request() {
    let custody = CheckCustody::default();
    let request = CheckRequestRef {
        id: 1,
        issuer: Arc::new(ValidationIssuerIdentity),
        identity: Arc::new(CheckRequestIdentity),
    };
    assert!(custody.request_close(&request).is_err());
    assert!(custody.state.lock().unwrap().runs.is_empty());
}
