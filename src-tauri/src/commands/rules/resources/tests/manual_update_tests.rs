use super::super::*;

#[test]
fn pure_builtin_manual_update_has_every_builtin_target() {
    let (external, builtins) = manual_update_targets(Vec::new());
    assert!(external.is_empty());
    let expected = builtin_geo_rulesets();
    assert!(!expected.is_empty());
    assert_eq!(
        builtins.iter().map(|b| &b.tag).collect::<Vec<_>>(),
        expected.iter().map(|b| &b.tag).collect::<Vec<_>>()
    );
}

#[test]
fn manual_update_deduplicates_registered_ids_and_builtin_overlap() {
    let first_builtin = builtin_geo_rulesets().remove(0);
    let (external, builtins) = manual_update_targets(vec![
        json!({ "id": builtin_id_for(&first_builtin.tag) }),
        json!({ "id": "external-1" }),
        json!({ "id": "external-1" }),
        json!({ "name": "malformed without id" }),
    ]);
    assert_eq!(external.len(), 2);
    assert_eq!(external[0]["id"], "external-1");
    assert!(external[1].get("id").is_none());
    assert_eq!(builtins.len(), builtin_geo_rulesets().len());
}


use crate::runtime::config::{ConfigManager, Decision};
use crate::test_support::TestDir;
use polaris_net_stack::safe_redirect::{FetchInit, MinimalResponse};

struct PausedResourceHttp {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl PausedResourceHttp {
    fn new() -> Self {
        Self { entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() }
    }
}
impl HttpClient for PausedResourceHttp {
    async fn fetch(&self, _: &str, _: &FetchInit) -> Result<MinimalResponse, String> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(MinimalResponse { status: 200, body: b"SRS-new".to_vec(), ..Default::default() })
    }
}
struct ResourcePublicLookup;
impl DnsLookup for ResourcePublicLookup {
    async fn lookup_all(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(vec!["93.184.216.34".to_owned()])
    }
}
#[derive(Default)]
struct ResourceSink(Mutex<Vec<Value>>);
impl ProgressSink for ResourceSink {
    fn emit(&self, frame: Value) { self.0.lock().unwrap().push(frame); }
}
impl ResourceSink {
    fn statuses(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|f| f["status"].as_str().unwrap().to_owned()).collect()
    }
}
fn registered_fixture() -> (TestDir, ConfigManager, RuleResource) {
    let dir = TestDir::new("polaris-resource-commit-");
    let manager = ConfigManager::new(dir.clone());
    let resource: RuleResource = serde_json::from_value(json!({
        "id":format!("res_{}", new_uuid()), "name":"Original", "category":"custom",
        "sourceUrl":"https://example.com/original.srs", "fileName":"original.srs",
        "format":"binary", "size":7, "downloadedAt":"before"
    })).unwrap();
    manager.update(|cfg| {
        cfg["ruleResources"] = json!([resource]);
        Decision::Write(())
    }).unwrap();
    let res_dir = dir.path().join("rule-resource");
    std::fs::create_dir_all(&res_dir).unwrap();
    std::fs::write(res_dir.join(&resource.file_name), b"SRS-old").unwrap();
    (dir, manager, resource)
}
fn guarded_test_commit(
    manager: &ConfigManager, expected: &RuleResource, downloaded: &RuleResource,
    staged: &Path, res_dir: &Path,
) -> Result<(), String> {
    let (result, _) = manager.update(|cfg| {
        match commit_registered_resource(cfg, expected, downloaded, staged, res_dir) {
            Ok(()) => Decision::Write(Ok(())),
            Err(error) => Decision::Skip(Err(error)),
        }
    }).map_err(|error| error.to_string())?;
    result
}

#[tokio::test]
async fn delayed_registered_refresh_cannot_resurrect_deleted_or_changed_resource() {
    for change in ["delete", "source", "file", "metadata"] {
        let (dir, manager, resource) = registered_fixture();
        let res_dir = dir.path().join("rule-resource");
        let plan = plan_from_resource(&resource);
        let http = PausedResourceHttp::new();
        let sink = ResourceSink::default();
        let download = download_registered_resource(
            &sink, &http, &ResourcePublicLookup, &plan, &res_dir,
            |downloaded, staged| guarded_test_commit(&manager, &resource, downloaded, staged, &res_dir),
        );
        let edit = async {
            http.entered.notified().await;
            manager.update(|cfg| {
                match change {
                    "delete" => cfg["ruleResources"] = json!([]),
                    "source" => cfg["ruleResources"][0]["sourceUrl"] = json!("https://example.com/new.srs"),
                    "file" => cfg["ruleResources"][0]["fileName"] = json!("replacement.srs"),
                    _ => cfg["ruleResources"][0]["downloadedAt"] = json!("newer-refresh"),
                }
                Decision::Write(())
            }).unwrap();
            if change == "delete" {
                std::fs::remove_file(res_dir.join(&resource.file_name)).unwrap();
            } else {
                std::fs::write(res_dir.join(&resource.file_name), b"SRS-newer-owner").unwrap();
            }
            let edited = manager.load_full().unwrap();
            http.release.notify_one();
            edited
        };
        let (outcome, edited) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            async { tokio::join!(download, edit) },
        ).await.expect("delayed refresh must settle after response release");
        assert!(matches!(outcome, DownloadOutcome::Failed { code: ERR_RESOURCE_WRITE_FAILED, .. }), "{change}");
        assert_eq!(manager.load_full().unwrap(), edited);
        assert_eq!(sink.statuses(), ["downloading", "error"]);
        assert_eq!(std::fs::read_dir(res_dir.join(".update")).unwrap().count(), 0);
        if change == "delete" {
            assert!(!res_dir.join(&resource.file_name).exists());
        } else {
            assert_eq!(std::fs::read(res_dir.join(&resource.file_name)).unwrap(), b"SRS-newer-owner");
        }
    }
}

#[tokio::test]
async fn unchanged_registered_refresh_commits_before_done_and_rejects_older_metadata() {
    let (dir, manager, resource) = registered_fixture();
    let res_dir = dir.path().join("rule-resource");
    let plan = plan_from_resource(&resource);
    let http = PausedResourceHttp::new();
    http.release.notify_one();
    let sink = ResourceSink::default();
    let outcome = download_registered_resource(
        &sink, &http, &ResourcePublicLookup, &plan, &res_dir, |downloaded, staged| {
            assert_eq!(std::fs::read(res_dir.join(&resource.file_name)).unwrap(), b"SRS-old");
            assert_eq!(sink.statuses(), ["downloading"]);
            guarded_test_commit(&manager, &resource, downloaded, staged, &res_dir)?;
            assert_eq!(std::fs::read(res_dir.join(&resource.file_name)).unwrap(), b"SRS-new");
            assert_eq!(manager.load_full().unwrap()["ruleResources"][0]["downloadedAt"], downloaded.downloaded_at);
            Ok(())
        },
    ).await;
    assert!(matches!(outcome, DownloadOutcome::Stored { existed_before: true, .. }));
    assert_eq!(sink.statuses(), ["downloading", "done"]);
    assert_eq!(std::fs::read_dir(res_dir.join(".update")).unwrap().count(), 0);
    // A second response that captured the original record cannot replace the newer commit.
    let stale = res_dir.join("late.srs");
    std::fs::write(&stale, b"SRS-late").unwrap();
    assert!(guarded_test_commit(&manager, &resource, &resource, &stale, &res_dir).is_err());
    assert_eq!(std::fs::read(res_dir.join(&resource.file_name)).unwrap(), b"SRS-new");
}

#[tokio::test]
async fn registered_refresh_rename_failure_never_reports_done_or_changes_metadata() {
    let (dir, manager, resource) = registered_fixture();
    let res_dir = dir.path().join("rule-resource");
    let live = res_dir.join(&resource.file_name);
    std::fs::remove_file(&live).unwrap();
    std::fs::create_dir(&live).unwrap();
    std::fs::write(live.join("keep"), b"keep").unwrap();
    let before = manager.load_full().unwrap();
    let http = PausedResourceHttp::new();
    http.release.notify_one();
    let sink = ResourceSink::default();
    let plan = plan_from_resource(&resource);
    let outcome = download_registered_resource(
        &sink, &http, &ResourcePublicLookup, &plan, &res_dir,
        |downloaded, staged| guarded_test_commit(&manager, &resource, downloaded, staged, &res_dir),
    ).await;
    assert!(matches!(outcome, DownloadOutcome::Failed { .. }));
    assert_eq!(manager.load_full().unwrap(), before);
    assert_eq!(std::fs::read(live.join("keep")).unwrap(), b"keep");
    assert_eq!(sink.statuses(), ["downloading", "error"]);
    assert_eq!(std::fs::read_dir(res_dir.join(".update")).unwrap().count(), 0);
}

#[tokio::test]
async fn registered_refresh_cancellation_drops_network_and_staging_without_commit() {
    let (dir, _manager, resource) = registered_fixture();
    let res_dir = dir.path().join("rule-resource");
    let plan = plan_from_resource(&resource);
    let http = PausedResourceHttp::new();
    let sink = ResourceSink::default();
    let download = download_registered_resource(
        &sink, &http, &ResourcePublicLookup, &plan, &res_dir,
        |_, _| panic!("cancelled refresh cannot commit"),
    );
    let cancel = async {
        http.entered.notified().await;
        assert_eq!(cancel_inflight(&resource.id), 1);
    };
    let (outcome, ()) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async { tokio::join!(download, cancel) },
    ).await.expect("cancellation must settle without releasing the HTTP response");
    assert!(matches!(outcome, DownloadOutcome::Cancelled));
    assert_eq!(sink.statuses(), ["downloading", "cancelled"]);
    assert_eq!(std::fs::read(res_dir.join(&resource.file_name)).unwrap(), b"SRS-old");
    assert_eq!(std::fs::read_dir(res_dir.join(".update")).unwrap().count(), 0);
}

#[test]
fn registered_refresh_paths_share_staged_transaction_commit_instead_of_upsert() {
    use crate::commands::guard_scan::top_level_fn_body;
    let code = crate::test_support::crate_code("commands/rules/resources.rs");
    for signature in ["async fn redownload_with_mode(", "pub async fn rule_resources_update_all("] {
        let body = top_level_fn_body(&code, signature);
        assert!(body.contains("download_registered_resource("));
        assert!(body.contains("persist_registered_resource("));
        assert!(!body.contains("persist_resources("));
    }
    let persist = top_level_fn_body(&code, "fn persist_registered_resource(");
    assert!(persist.find("state.config().update(").unwrap() < persist.find("commit_registered_resource(").unwrap());
    let delete = top_level_fn_body(&code, "pub fn rule_resources_delete(");
    assert!(delete.find("return Decision::Skip(plan)").unwrap() < delete.find("cancel_inflight(&id)").unwrap());
    assert!(delete.find("cancel_inflight(&id)").unwrap() < delete.find("arr.retain(").unwrap());
}


#[test]
fn stage_name_collision_never_acquires_or_removes_a_foreign_directory() {
    let dir = TestDir::new("polaris-resource-stage-foreign-");
    let foreign = dir.path().join(".update").join("same-name");
    std::fs::create_dir_all(&foreign).unwrap();
    let keep = foreign.join("foreign.srs");
    std::fs::write(&keep, b"foreign-owner").unwrap();
    let mut names = ["same-name", "new-owned-name"].into_iter();
    let stage = ResourceStageDir::create_with(dir.path(), || {
        names.next().expect("must retry the collision exactly once").to_owned()
    }).unwrap();
    let owned = stage.0.clone();
    assert_ne!(owned, foreign);
    std::fs::write(owned.join("download.srs"), b"owned").unwrap();
    drop(stage);
    assert!(!owned.exists());
    assert_eq!(std::fs::read(&keep).unwrap(), b"foreign-owner");
    assert!(foreign.parent().unwrap().is_dir(), "the shared parent is not owned");
}

#[test]
fn concurrent_identical_stage_names_retry_exclusively_and_cleanup_only_their_owner() {
    let dir = TestDir::new("polaris-resource-stage-concurrent-");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for worker_id in 0..2 {
        let root = dir.path().to_path_buf();
        let barrier = std::sync::Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            let mut attempts = 0;
            ResourceStageDir::create_with(&root, || {
                attempts += 1;
                if attempts == 1 {
                    // Both workers really try the identical path while both owners
                    // remain alive; UUID probability is irrelevant to this regression.
                    barrier.wait();
                    "same-name".to_owned()
                } else {
                    assert_eq!(attempts, 2, "only the losing worker retries");
                    format!("retry-{worker_id}")
                }
            }).unwrap()
        }));
    }
    let second = workers.pop().unwrap().join().unwrap();
    let first = workers.pop().unwrap().join().unwrap();
    assert_ne!(first.0, second.0);
    std::fs::write(first.0.join("download.srs"), b"first").unwrap();
    std::fs::write(second.0.join("download.srs"), b"second").unwrap();
    let first_path = first.0.clone();
    let second_path = second.0.clone();
    drop(first);
    assert!(!first_path.exists());
    assert_eq!(std::fs::read(second_path.join("download.srs")).unwrap(), b"second");
    drop(second);
    assert!(!second_path.exists());
    assert_eq!(std::fs::read_dir(dir.path().join(".update")).unwrap().count(), 0);
}

#[test]
fn failed_stage_creation_after_a_collision_never_cleans_the_rejected_path() {
    let dir = TestDir::new("polaris-resource-stage-error-");
    let foreign = dir.path().join(".update").join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("keep"), b"foreign").unwrap();
    let mut names = ["foreign", "invalid\0name"].into_iter();
    let result = ResourceStageDir::create_with(dir.path(), || {
        names.next().expect("creation error must return, not retry forever").to_owned()
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(foreign.join("keep")).unwrap(), b"foreign");
}
