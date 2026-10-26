use super::super::*;
use crate::test_support::TestDir;
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

fn fixture() -> (ResourcePlan, TestDir) {
    let plan = plan_from_builtin(&find_builtin("geosite-cn").unwrap());
    let dir = TestDir::new("polaris-builtin-test-");
    std::fs::write(dir.join(&plan.file_name), b"SRS-old").unwrap();
    (plan, dir)
}

#[tokio::test]
async fn builtin_update_commits_live_and_metadata_before_done() {
    let (plan, dir) = fixture();
    let client = TestHttp::new(200, b"SRS-new");
    let sink = RecordingSink::default();
    let saved = AtomicUsize::new(0);
    let outcome = update_builtin_resource(
        &sink,
        &client,
        &PublicLookup,
        &plan,
        &dir,
        |resource, staged, operation| {
            publish_builtin_file(operation, staged, &dir.join(&plan.file_name))?;
            assert_eq!(
                std::fs::read(dir.join(&plan.file_name)).unwrap(),
                b"SRS-new"
            );
            assert!(!resource.downloaded_at.is_empty());
            assert_eq!(sink.statuses(), ["downloading"]);
            saved.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
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
    assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
    std::fs::remove_dir_all(dir.path()).unwrap();
}

#[tokio::test]
async fn builtin_http_or_invalid_content_failure_preserves_live() {
    for (status, body) in [(500, b"SRS-new".as_slice()), (200, b"<html>".as_slice())] {
        let (plan, dir) = fixture();
        let sink = RecordingSink::default();
        let client = TestHttp::new(status, body);
        let outcome =
            update_builtin_resource(&sink, &client, &PublicLookup, &plan, &dir, |_, _, _| {
                panic!("下载失败不能保存元数据")
            })
            .await;
        assert!(matches!(outcome, DownloadOutcome::Failed { .. }));
        assert_eq!(
            std::fs::read(dir.join(&plan.file_name)).unwrap(),
            b"SRS-old"
        );
        assert_eq!(sink.statuses(), ["downloading", "error"]);
        assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
        std::fs::remove_dir_all(dir.path()).unwrap();
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
        |_, staged, operation| {
            publish_builtin_file(operation, staged, &dir.join(&plan.file_name))?;
            Err("保存更新标记失败，可重试".into())
        },
    )
    .await;
    let result = outcome.into_value(&plan);
    assert_eq!(result["ok"], false);
    assert_eq!(result["errorCode"], ERR_RESOURCE_WRITE_FAILED);
    assert_eq!(sink.statuses(), ["downloading", "error"]);
    assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
    std::fs::remove_dir_all(dir.path()).unwrap();
}

#[tokio::test]
async fn builtin_same_tag_updates_are_serialized_and_cleanup_isolated() {
    let (plan, dir) = fixture();
    let client = TestHttp::new(200, b"SRS-new");
    let first = RecordingSink::default();
    let second = RecordingSink::default();
    let saved = AtomicUsize::new(0);
    let persist = |_: &RuleResource, staged: &Path, operation: &BuiltinUpdateOperation| {
        publish_builtin_file(operation, staged, &dir.join(&plan.file_name))?;
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
    assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
    std::fs::remove_dir_all(dir.path()).unwrap();
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
    assert!(body.contains("let mut changed = false"));
    assert!(body.contains("download_registered_resource("));
    assert!(body.contains("persist_registered_resource("));
    assert!(body.contains("changed |= matches!(&outcome, DownloadOutcome::Stored { .. })"));
    assert!(body.contains("changed |= result.get(\"ok\").and_then(Value::as_bool) == Some(true)"));
    assert!(body.contains("if changed {"));
    // 空外置表没有 return，全部内置项仍需出现在逐项结果数组中。
    let external = body.find("for entry in &raw_entries").unwrap();
    let builtin = body.find("for builtin in builtins").unwrap();
    assert!(!body[external..builtin].contains("return"));
    assert!(body[builtin..].contains("results.push(result)"));
}

#[test]
fn reset_retires_active_and_queued_generations_without_resurrecting_files() {
    let (mut plan, dir) = fixture();
    plan.id = format!("reset-fixture-{}", new_uuid());
    let active = BuiltinUpdateOperation::begin(&plan.id);
    let queued = BuiltinUpdateOperation::begin(&plan.id);
    let other = BuiltinUpdateOperation::begin(&format!("other-{}", new_uuid()));
    let staged = ResourceStageDir::create(&dir).unwrap();
    let path = staged.0.join(&plan.file_name);
    let live = dir.join(&plan.file_name);
    std::fs::write(&path, b"SRS-stale").unwrap();
    // Same transition reset performs under the config writer. Cleanup already ran.
    builtin_update_lock(&plan.id).retire();
    std::fs::remove_file(&live).unwrap();
    assert!(!active.is_current());
    assert!(!queued.is_current());
    assert!(other.is_current());
    assert!(publish_builtin_file(&active, &path, &live).is_err());
    assert!(publish_builtin_file(&queued, &path, &live).is_err());
    assert!(!live.exists());
    assert!(path.exists());
    // A genuinely post-reset update owns a new generation, including before cleanup.
    let successor = BuiltinUpdateOperation::begin(&plan.id);
    assert!(successor.is_current());
    let successor_stage = ResourceStageDir::create(&dir).unwrap();
    let successor_path = successor_stage.0.join(&plan.file_name);
    std::fs::write(&successor_path, b"SRS-successor").unwrap();
    publish_builtin_file(&successor, &successor_path, &live).unwrap();
    assert!(publish_builtin_file(&active, &path, &live).is_err());
    drop(staged); // Cancellation only cleans the old operation's exclusive stage.
    assert_eq!(std::fs::read(&live).unwrap(), b"SRS-successor");
    drop(successor_stage);
    std::fs::remove_dir_all(dir.path()).unwrap();
}

#[test]
fn failed_reset_persistence_conservatively_cancels_old_work_without_file_deletion() {
    let (mut plan, dir) = fixture();
    plan.id = format!("failed-reset-fixture-{}", new_uuid());
    let active = BuiltinUpdateOperation::begin(&plan.id);
    builtin_update_lock(&plan.id).retire();
    // A persistence failure after retirement does not roll operation identity back.
    let staged = ResourceStageDir::create(&dir).unwrap();
    let path = staged.0.join(&plan.file_name);
    std::fs::write(&path, b"SRS-stale").unwrap();
    let live = dir.join(&plan.file_name);
    assert!(publish_builtin_file(&active, &path, &live).is_err());
    assert_eq!(std::fs::read(&live).unwrap(), b"SRS-old");
    let successor = BuiltinUpdateOperation::begin(&plan.id);
    assert!(successor.is_current());
    drop(active);
    // The successor keeps the weak registry entry alive; retirement cannot ABA.
    builtin_update_lock(&plan.id).retire();
    assert!(!successor.is_current());
    drop(staged);
    std::fs::remove_dir_all(dir.path()).unwrap();
}

#[test]
fn reset_and_publish_share_writer_fence_and_capture_precedes_queue() {
    use crate::commands::guard_scan::top_level_fn_body;
    let src = crate::test_support::crate_code("commands/rules/resources.rs");
    let reset = top_level_fn_body(&src, "pub fn rule_resources_reset_builtin(");
    assert!(
        reset
            .find("update_with_explicit_deletions(deletions, |cfg|")
            .unwrap()
            < reset.find(".retire()").unwrap()
    );
    assert!(!reset.contains(".await"));
    let update = top_level_fn_body(&src, "async fn update_builtin_resource<");
    assert!(
        update.find("BuiltinUpdateOperation::begin").unwrap()
            < update.find(".lock().await").unwrap()
    );
    assert!(!update.contains("std::fs::rename"));
    let persist = top_level_fn_body(&src, "fn persist_builtin_geo_updated(");
    assert!(
        persist.find("state.config().update(|cfg|").unwrap()
            < persist.find("publish_builtin_file(").unwrap()
    );
    assert!(
        persist.find("publish_builtin_file(").unwrap()
            < persist.find(".entry(\"builtinGeoMeta\")").unwrap()
    );
}

struct PausedHttp {
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl HttpClient for PausedHttp {
    async fn fetch(&self, _: &str, _: &FetchInit) -> Result<MinimalResponse, String> {
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(MinimalResponse {
            status: 200,
            body: b"SRS-new".to_vec(),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn download_paused_across_reset_cannot_publish_or_mark_metadata_done() {
    let (mut plan, dir) = fixture();
    plan.id = format!("paused-reset-{}", new_uuid());
    let client = PausedHttp {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    };
    let sink = RecordingSink::default();
    let saved = AtomicUsize::new(0);
    let update = update_builtin_resource(
        &sink,
        &client,
        &PublicLookup,
        &plan,
        &dir,
        |_, staged, operation| {
            publish_builtin_file(operation, staged, &dir.join(&plan.file_name))?;
            saved.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    );
    let reset = async {
        client.entered.notified().await;
        builtin_update_lock(&plan.id).retire();
        std::fs::remove_file(dir.join(&plan.file_name)).unwrap();
        client.resume.notify_one();
    };
    let (outcome, ()) = tokio::join!(update, reset);
    assert!(matches!(outcome, DownloadOutcome::Failed { .. }));
    assert_eq!(saved.load(Ordering::SeqCst), 0);
    assert!(!dir.join(&plan.file_name).exists());
    assert!(!sink.statuses().iter().any(|status| status == "done"));
    assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
    std::fs::remove_dir_all(dir.path()).unwrap();
}

#[tokio::test]
async fn cancelling_paused_download_cleans_only_its_stage_and_releases_gate() {
    let (mut plan, dir) = fixture();
    plan.id = format!("cancel-paused-{}", new_uuid());
    let client = PausedHttp {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    };
    let sink = RecordingSink::default();
    {
        let update =
            update_builtin_resource(&sink, &client, &PublicLookup, &plan, &dir, |_, _, _| {
                panic!("cancelled download must not publish")
            });
        tokio::pin!(update);
        tokio::select! {
            _ = &mut update => panic!("download must still be paused"),
            () = client.entered.notified() => {}
        }
    }
    assert_eq!(
        std::fs::read(dir.join(&plan.file_name)).unwrap(),
        b"SRS-old"
    );
    assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
    assert!(builtin_update_lock(&plan.id).download.try_lock().is_ok());
    std::fs::remove_dir_all(dir.path()).unwrap();
}

#[tokio::test]
async fn concurrent_builtin_fixtures_keep_failure_live_and_stage_cleanup_isolated() {
    let (mut success_plan, success_dir) = fixture();
    let (mut failure_plan, failure_dir) = fixture();
    let (mut metadata_plan, metadata_dir) = fixture();
    // One keyed gate for this test, without sharing cancellation ownership with
    // concurrently running tests of the same builtin catalog entry.
    let id = format!(
        "isolation-{}",
        success_dir.file_name().unwrap().to_string_lossy()
    );
    success_plan.id.clone_from(&id);
    failure_plan.id.clone_from(&id);
    metadata_plan.id = id;
    assert_eq!(success_plan.id, failure_plan.id);
    assert_eq!(success_plan.id, metadata_plan.id);
    assert_ne!(success_dir.path(), failure_dir.path());
    assert_ne!(success_dir.path(), metadata_dir.path());
    assert_ne!(failure_dir.path(), metadata_dir.path());
    let success_sink = RecordingSink::default();
    let failure_sink = RecordingSink::default();
    let metadata_sink = RecordingSink::default();
    let paused = PausedHttp {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    };
    let success = update_builtin_resource(
        &success_sink,
        &paused,
        &PublicLookup,
        &success_plan,
        &success_dir,
        |_, staged, operation| {
            publish_builtin_file(
                operation,
                staged,
                &success_dir.join(&success_plan.file_name),
            )
        },
    );
    let peers = async {
        paused.entered.notified().await;
        // A real owned stage is live in the first fixture. Other fixtures must
        // neither observe it nor share the success publisher's live pathname.
        assert_eq!(
            std::fs::read_dir(success_dir.join(".update"))
                .unwrap()
                .count(),
            1
        );
        assert!(!failure_dir.join(".update").exists());
        assert!(!metadata_dir.join(".update").exists());
        paused.resume.notify_one();
        let failure_client = TestHttp::new(500, b"SRS-new");
        let metadata_client = TestHttp::new(200, b"SRS-new");
        tokio::join!(
            update_builtin_resource(
                &failure_sink,
                &failure_client,
                &PublicLookup,
                &failure_plan,
                &failure_dir,
                |_, _, _| panic!("failed fetch must never publish or persist"),
            ),
            update_builtin_resource(
                &metadata_sink,
                &metadata_client,
                &PublicLookup,
                &metadata_plan,
                &metadata_dir,
                |_, staged, operation| {
                    publish_builtin_file(
                        operation,
                        staged,
                        &metadata_dir.join(&metadata_plan.file_name),
                    )?;
                    Err("fixture metadata persistence failed".into())
                },
            ),
        )
    };
    // Borrowed futures only: timeout drops every download and its owned stage;
    // there are no spawned tasks or servers to outlive this test.
    let joined = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(success, peers)
    })
    .await;
    // On timeout the borrowed download futures are already dropped. Their
    // registry entries are not RAII-owned, so remove only this test's unique id.
    cancel_inflight(&success_plan.id);
    let (success, (failure, metadata)) =
        joined.expect("builtin fixture peers must complete within the test deadline");
    assert!(matches!(success, DownloadOutcome::Stored { .. }));
    assert!(matches!(
        failure,
        DownloadOutcome::Failed {
            code: ERR_RESOURCE_DOWNLOAD_FAILED,
            ..
        }
    ));
    assert!(matches!(
        metadata,
        DownloadOutcome::Failed {
            code: ERR_RESOURCE_WRITE_FAILED,
            ..
        }
    ));
    assert_eq!(success_sink.statuses(), ["downloading", "done"]);
    assert_eq!(failure_sink.statuses(), ["downloading", "error"]);
    assert_eq!(metadata_sink.statuses(), ["downloading", "error"]);
    assert_eq!(
        std::fs::read(success_dir.join(&success_plan.file_name)).unwrap(),
        b"SRS-new"
    );
    assert_eq!(
        std::fs::read(failure_dir.join(&failure_plan.file_name)).unwrap(),
        b"SRS-old"
    );
    // Metadata failure is not a rollback claim: the injected publisher ran.
    assert_eq!(
        std::fs::read(metadata_dir.join(&metadata_plan.file_name)).unwrap(),
        b"SRS-new"
    );
    for dir in [&success_dir, &failure_dir, &metadata_dir] {
        assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
    }
}

#[test]
fn repeated_clock_candidate_reuses_old_fixture_but_owned_roots_stay_distinct() {
    let root = TestDir::new("polaris-builtin-collision-proof-");
    let shared = root.join("fixed-clock-candidate");
    let (reader, writer) = run_collision_peers(&shared, false, false);
    reader.unwrap();
    writer.unwrap();
    assert_eq!(
        std::fs::read_dir(shared.join(".update")).unwrap().count(),
        0
    );
    let first = TestDir::new_in("same-prefix-", root.path());
    let second = TestDir::new_in("same-prefix-", root.path());
    assert_ne!(first.path(), second.path());
    std::fs::write(first.join("live.srs"), b"SRS-new").unwrap();
    std::fs::write(second.join("live.srs"), b"SRS-old").unwrap();
    drop(first);
    assert_eq!(std::fs::read(second.join("live.srs")).unwrap(), b"SRS-old");
}

fn run_collision_peers(
    shared: &Path,
    fail_writer_before_publication: bool,
    fail_reader_before_ack: bool,
) -> (Result<(), &'static str>, Result<(), &'static str>) {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (published_tx, published_rx) = std::sync::mpsc::channel();
    let (observed_tx, observed_rx) = std::sync::mpsc::channel();
    let deadline = std::time::Duration::from_secs(2);
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            std::fs::create_dir_all(shared).map_err(|_| "reader-create")?;
            std::fs::write(shared.join("live.srs"), b"SRS-old").map_err(|_| "reader-write")?;
            ready_tx.send(()).map_err(|_| "reader-ready")?;
            published_rx
                .recv_timeout(deadline)
                .map_err(|_| "reader-publication-unavailable")?;
            if fail_reader_before_ack {
                return Err("injected-reader-before-ack");
            }
            // Forced repeated clock candidate, not a claim about the CI clock.
            if std::fs::read(shared.join("live.srs")).map_err(|_| "reader-read")? != b"SRS-new" {
                return Err("reader-live-mismatch");
            }
            if std::fs::read_dir(shared.join(".update"))
                .map_err(|_| "reader-stage")?
                .count()
                != 1
            {
                return Err("reader-stage-count");
            }
            observed_tx.send(()).map_err(|_| "reader-ack")?;
            Ok(())
        });
        let writer = scope.spawn(move || {
            std::fs::create_dir_all(shared).map_err(|_| "writer-create")?;
            std::fs::write(shared.join("live.srs"), b"SRS-old").map_err(|_| "writer-write")?;
            ready_rx
                .recv_timeout(deadline)
                .map_err(|_| "writer-ready-unavailable")?;
            let stage = ResourceStageDir::create(shared).map_err(|_| "writer-stage")?;
            if fail_writer_before_publication {
                return Err("injected-writer-before-publication");
            }
            let staged = stage.0.join("live.srs");
            std::fs::write(&staged, b"SRS-new").map_err(|_| "writer-staged-write")?;
            std::fs::rename(staged, shared.join("live.srs")).map_err(|_| "writer-publish")?;
            published_tx.send(()).map_err(|_| "writer-publication")?;
            observed_rx
                .recv_timeout(deadline)
                .map_err(|_| "writer-ack-unavailable")?;
            drop(stage);
            Ok(())
        });
        // Both joins are attempted even if one worker panics. Channel endpoints
        // are owned by workers, so failure disconnects the peer's bounded wait.
        let reader = reader.join().unwrap_or(Err("reader-panicked"));
        let writer = writer.join().unwrap_or(Err("writer-panicked"));
        (reader, writer)
    })
}

#[test]
fn collision_peer_failures_disconnect_waiters_and_drop_owned_stages() {
    for (writer_failure, reader_failure) in [(true, false), (false, true)] {
        let root = TestDir::new("polaris-builtin-peer-failure-");
        let (reader, writer) = run_collision_peers(root.path(), writer_failure, reader_failure);
        if writer_failure {
            assert_eq!(writer, Err("injected-writer-before-publication"));
            assert_eq!(reader, Err("reader-publication-unavailable"));
        } else {
            assert_eq!(reader, Err("injected-reader-before-ack"));
            assert_eq!(writer, Err("writer-ack-unavailable"));
        }
        assert_eq!(std::fs::read_dir(root.join(".update")).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn builtin_fixture_early_stage_failure_cannot_leave_notification_peer_waiting() {
    let (mut plan, dir) = fixture();
    plan.id = format!(
        "stage-failure-{}",
        dir.file_name().unwrap().to_string_lossy()
    );
    // Fail before PausedHttp::fetch can publish entered. A file cannot be used
    // as the .update directory; no download task is ever spawned.
    std::fs::write(dir.join(".update"), b"blocked-stage").unwrap();
    let client = PausedHttp {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    };
    let sink = RecordingSink::default();
    let update = update_builtin_resource(&sink, &client, &PublicLookup, &plan, &dir, |_, _, _| {
        panic!("stage failure must not publish")
    });
    let result = tokio::time::timeout(std::time::Duration::from_millis(25), async {
        tokio::join!(update, client.entered.notified())
    })
    .await;
    assert!(result.is_err(), "the absent notification must be bounded");
    assert_eq!(sink.statuses(), ["error"]);
    assert_eq!(
        std::fs::read(dir.join(&plan.file_name)).unwrap(),
        b"SRS-old"
    );
    assert_eq!(
        std::fs::read(dir.join(".update")).unwrap(),
        b"blocked-stage"
    );
    assert!(builtin_update_lock(&plan.id).download.try_lock().is_ok());
}

#[tokio::test]
async fn timed_out_fixture_clears_only_its_cancel_registration_after_stage_drop() {
    let (mut plan, dir) = fixture();
    plan.id = format!("timeout-{}", dir.file_name().unwrap().to_string_lossy());
    let other_id = format!("other-{}", plan.id);
    let (other_seq, mut other_rx) = register_cancellable(&other_id);
    let client = PausedHttp {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    };
    let sink = RecordingSink::default();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(25),
        update_builtin_resource(&sink, &client, &PublicLookup, &plan, &dir, |_, _, _| {
            panic!("paused download cannot publish")
        }),
    )
    .await;
    cancel_inflight(&plan.id);
    // Release the unrelated registration before assertions too. Cancelling our
    // dropped receiver must never send to the unrelated live receiver.
    unregister_cancellable(other_seq);
    assert!(result.is_err());
    assert_eq!(
        other_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
    assert!(!cancel_registry()
        .lock()
        .unwrap()
        .values()
        .any(|(id, _)| id == &plan.id));
    assert_eq!(std::fs::read_dir(dir.join(".update")).unwrap().count(), 0);
    assert_eq!(
        std::fs::read(dir.join(&plan.file_name)).unwrap(),
        b"SRS-old"
    );
    assert_eq!(sink.statuses(), ["downloading"]);
}
