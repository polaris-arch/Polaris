use super::super::*;
use polaris_net_stack::safe_redirect::{FetchInit, MinimalResponse};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct TestHttp {
    status: u16,
    body: Vec<u8>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

impl TestHttp {
    fn new(status: u16, body: &[u8]) -> Self {
        Self {
            status,
            body: body.to_vec(),
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl HttpClient for TestHttp {
    async fn fetch(&self, _: &str, _: &FetchInit) -> Result<MinimalResponse, String> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::task::yield_now().await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(MinimalResponse {
            status: self.status,
            body: self.body.clone(),
            ..Default::default()
        })
    }
}

struct PublicLookup;
impl DnsLookup for PublicLookup {
    async fn lookup_all(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(vec!["93.184.216.34".into()])
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<Value>>);
impl ProgressSink for RecordingSink {
    fn emit(&self, frame: Value) {
        self.0.lock().unwrap().push(frame);
    }
}
impl RecordingSink {
    fn statuses(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|frame| frame["status"].as_str().map(str::to_owned))
            .collect()
    }
}

fn fixture() -> (ResourcePlan, std::path::PathBuf) {
    let plan = plan_from_builtin(&find_builtin("geosite-cn").unwrap());
    let dir = std::env::temp_dir().join(format!("polaris-builtin-test-{}", new_uuid()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(&plan.file_name), b"SRS-old").unwrap();
    (plan, dir)
}

#[tokio::test]
async fn builtin_update_commits_live_and_metadata_before_done() {
    let (plan, dir) = fixture();
    let client = TestHttp::new(200, b"SRS-new");
    let sink = RecordingSink::default();
    let saved = AtomicUsize::new(0);
    let outcome = update_builtin_resource(&sink, &client, &PublicLookup, &plan, &dir, |resource| {
        assert_eq!(
            std::fs::read(dir.join(&plan.file_name)).unwrap(),
            b"SRS-new"
        );
        assert!(!resource.downloaded_at.is_empty());
        assert_eq!(sink.statuses(), ["downloading"]);
        saved.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .await;
    assert!(matches!(
        outcome,
        DownloadOutcome::Stored {
            existed_before: true,
            ..
        }
    ));
    assert_eq!(saved.load(Ordering::SeqCst), 1);
    assert_eq!(sink.statuses(), ["downloading", "done"]);
    assert!(!dir.join(".update").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn builtin_http_or_invalid_content_failure_preserves_live() {
    for (status, body) in [(500, b"SRS-new".as_slice()), (200, b"<html>".as_slice())] {
        let (plan, dir) = fixture();
        let sink = RecordingSink::default();
        let client = TestHttp::new(status, body);
        let outcome = update_builtin_resource(&sink, &client, &PublicLookup, &plan, &dir, |_| {
            panic!("下载失败不能保存元数据")
        })
        .await;
        assert!(matches!(outcome, DownloadOutcome::Failed { .. }));
        assert_eq!(
            std::fs::read(dir.join(&plan.file_name)).unwrap(),
            b"SRS-old"
        );
        assert_eq!(sink.statuses(), ["downloading", "error"]);
        assert!(!dir.join(".update").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn builtin_metadata_failure_is_retryable_error_and_never_done() {
    let (plan, dir) = fixture();
    let sink = RecordingSink::default();
    let outcome = update_builtin_resource(
        &sink,
        &TestHttp::new(200, b"SRS-new"),
        &PublicLookup,
        &plan,
        &dir,
        |_| Err("保存更新标记失败，可重试".into()),
    )
    .await;
    let result = outcome.into_value(&plan);
    assert_eq!(result["ok"], false);
    assert_eq!(result["errorCode"], ERR_RESOURCE_WRITE_FAILED);
    assert_eq!(sink.statuses(), ["downloading", "error"]);
    assert!(!dir.join(".update").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn builtin_same_tag_updates_are_serialized_and_cleanup_isolated() {
    let (plan, dir) = fixture();
    let client = TestHttp::new(200, b"SRS-new");
    let first = RecordingSink::default();
    let second = RecordingSink::default();
    let saved = AtomicUsize::new(0);
    let persist = |_: &RuleResource| {
        saved.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    let (a, b) = tokio::join!(
        update_builtin_resource(&first, &client, &PublicLookup, &plan, &dir, persist),
        update_builtin_resource(&second, &client, &PublicLookup, &plan, &dir, persist),
    );
    assert!(matches!(a, DownloadOutcome::Stored { .. }));
    assert!(matches!(b, DownloadOutcome::Stored { .. }));
    assert_eq!(client.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(saved.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read(dir.join(&plan.file_name)).unwrap(),
        b"SRS-new"
    );
    assert!(!dir.join(".update").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn manual_all_wires_real_builtin_registry_and_one_deferred_batch_broadcast() {
    let source = crate::test_support::crate_code("commands/rules/resources.rs");
    let start = source
        .find("pub async fn rule_resources_update_all(")
        .unwrap();
    let end = source[start..]
        .find("pub async fn rule_resources_download(")
        .unwrap()
        + start;
    let body = &source[start..end];
    assert!(body.contains("let builtins = builtin_geo_rulesets()"));
    assert!(body.contains("for builtin in builtins"));
    assert!(body.contains("update_builtin_with_mode("));
    assert!(body.contains("ProgressMode::Live"));
    assert_eq!(body.matches("BroadcastMode::Deferred").count(), 2);
    assert!(!body.contains("BroadcastMode::Immediate"));
    assert_eq!(body.matches("broadcast_config_changed(").count(), 1);
    assert!(body.contains("if !stored.is_empty() || builtin_ok"));
    // 空外置表没有 return，全部内置项仍需出现在逐项结果数组中。
    let external = body.find("for entry in &raw_entries").unwrap();
    let builtin = body.find("for builtin in builtins").unwrap();
    assert!(!body[external..builtin].contains("return"));
    assert!(body[builtin..].contains("results.push(result)"));
}
