use super::*;
use crate::runtime::proxy::android_capacity::{CapacityClosed, CheckFailure};

struct CapacityChecker;
#[async_trait]
impl ConfigChecker for CapacityChecker {
    async fn check(&self, _: &Path, _: &Path) -> Result<(), String> {
        panic!("typed entry required")
    }
    async fn check_admitted(&self, _: &Path, _: &Path) -> Result<(), CheckFailure> {
        Err(CheckFailure::CapacityClosed(CapacityClosed))
    }
}

struct CapacitySpawner;
#[async_trait]
impl LoginCoreSpawner for CapacitySpawner {
    async fn spawn(&self, _: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        Err(CapacityClosed.spawn_error())
    }
}

#[test]
fn capacity_progress_receipt_keeps_only_the_stable_reason() {
    let attempts = Attempts::default();
    let attempt = attempts.prepare("ts1", "request").unwrap();
    let emitter = AttemptReceiptEmitter {
        inner: Arc::new(FakeEmitter::default()),
        attempt,
        attempt_id: "request".into(),
    };
    emitter.progress(
        "ts1",
        "request",
        "failed",
        Some(crate::runtime::proxy::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED),
        Some("https://private/token"),
    );
    let receipt = attempts.progress("ts1", "request").unwrap();
    assert_eq!(
        receipt.reason.as_deref(),
        Some(crate::runtime::proxy::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED)
    );
    assert!(!serde_json::to_string(&receipt).unwrap().contains("private"));
}

#[tokio::test]
async fn default_desktop_checker_preserves_accept_and_reject_without_classifying_capacity() {
    let path = Path::new("unused");
    assert!(FakeChecker { ok: true }
        .check_admitted(path, path)
        .await
        .is_ok());
    match (FakeChecker { ok: false }).check_admitted(path, path).await {
        Err(CheckFailure::Rejected(message)) => assert_eq!(message, "fake check 判定配置无效"),
        other => panic!("desktop fallback changed: {other:?}"),
    }
    assert!(FakeChecker { ok: true }
        .check_for_spawn(path, path)
        .await
        .is_ok());
    match (FakeChecker { ok: false })
        .check_for_spawn(path, path)
        .await
    {
        Err(ConfigCheckFailure::Rejected(message)) => {
            assert_eq!(message, "fake check 判定配置无效")
        }
        other => panic!("spawn checker diagnostics changed: {other:?}"),
    }
    let admitted: &dyn ConfigChecker = &CapacityChecker;
    assert!(matches!(
        admitted.check_for_spawn(path, path).await,
        Err(ConfigCheckFailure::AndroidCapacityClosed(CapacityClosed))
    ));
}

#[tokio::test]
async fn dynamic_spawn_checker_preserves_plain_rejection_without_classifying_capacity_text() {
    struct TextChecker;
    #[async_trait]
    impl ConfigChecker for TextChecker {
        async fn check(&self, _: &Path, _: &Path) -> Result<(), String> {
            Err(CapacityClosed.to_string())
        }
    }
    let path = Path::new("unused");
    let accepting: Arc<dyn ConfigChecker> = Arc::new(FakeChecker { ok: true });
    assert!(accepting.check_for_spawn(path, path).await.is_ok());
    let rejecting: Arc<dyn ConfigChecker> = Arc::new(TextChecker);
    match rejecting.check_for_spawn(path, path).await {
        Err(ConfigCheckFailure::Rejected(detail)) => {
            assert_eq!(detail, CapacityClosed.to_string())
        }
        other => panic!("plain diagnostic changed cause: {other:?}"),
    }
}

#[tokio::test]
async fn typed_check_and_spawn_capacity_reach_login_outcome_without_registering_a_child() {
    for during_check in [true, false] {
        let spawner = fake_spawner(vec![], false, false);
        let checker: Arc<dyn ConfigChecker> = if during_check {
            Arc::new(CapacityChecker)
        } else {
            Arc::new(FakeChecker { ok: true })
        };
        let selected: Arc<dyn LoginCoreSpawner> = if during_check {
            spawner.clone()
        } else {
            Arc::new(CapacitySpawner)
        };
        let reg = LoginCoreRegistry::with_deps(
            selected,
            checker,
            fake_subscriber(false),
            Arc::new(|| Ok("/fake/sing-box".into())),
            Duration::from_secs(60),
        );
        let dir = temp_ud();
        let outcome = reg
            .start_login(
                &ts_server("ts1", "node"),
                &dir,
                false,
                None,
                0,
                Arc::new(FakeEmitter::default()),
            )
            .await;
        assert!(matches!(
            outcome,
            StartLoginOutcome::AndroidCapacityClosed(_)
        ));
        assert!(!reg.shared.contains("ts1"));
        assert!(login_configs(&dir).is_empty());
        assert_eq!(spawner.count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
