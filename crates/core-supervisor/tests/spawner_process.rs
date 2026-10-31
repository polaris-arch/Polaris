//! The public API cannot turn a candidate path into execution authority.
use polaris_core_supervisor::{SingBoxSpawner, SpawnRequest, StdioPolicy, TokioSpawner};
#[tokio::test]
async fn unadmitted_intent_never_enters_factory_or_calls_drain() {
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = called.clone();
    let request = SpawnRequest::unadmitted_intent(
        env!("CARGO_BIN_EXE_argv_probe"),
        "--sleep",
        StdioPolicy::drain(move |_, _| {
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
        }),
    );
    assert!(matches!(
        TokioSpawner::new().spawn(request),
        Err(polaris_core_supervisor::SpawnError::Admission(_))
    ));
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}
