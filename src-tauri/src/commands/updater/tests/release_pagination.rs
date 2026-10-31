use super::super::shared::{
    fetch_release_pages, ReleaseFetchLimits, GITHUB_RELEASES_PER_PAGE, RELEASE_FETCH_LIMITS,
};
use super::*;
use polaris_net_stack::safe_redirect::{FetchInit, HttpClient, MinimalResponse};
use polaris_net_stack::ssrf::DnsLookup;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct PublicDns;

impl DnsLookup for PublicDns {
    async fn lookup_all(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(vec!["8.8.8.8".to_string()])
    }
}

struct Step {
    response: Result<MinimalResponse, String>,
    delay: Duration,
}

impl Step {
    fn bytes(body: Vec<u8>) -> Self {
        Self {
            response: Ok(MinimalResponse {
                status: 200,
                body,
                ..Default::default()
            }),
            delay: Duration::ZERO,
        }
    }

    fn page(releases: &[Value]) -> Self {
        Self::bytes(serde_json::to_vec(releases).unwrap())
    }
}

struct ScriptedHttp {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<(String, FetchInit)>>,
    active: AtomicUsize,
    completed: AtomicUsize,
    canceled: AtomicUsize,
    started: tokio::sync::Notify,
}

struct InFlight<'a> {
    http: &'a ScriptedHttp,
    completed: bool,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.http.active.fetch_sub(1, Ordering::SeqCst);
        if !self.completed {
            self.http.canceled.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl ScriptedHttp {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: Mutex::new(steps.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            canceled: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
        }
    }

    fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl HttpClient for ScriptedHttp {
    async fn fetch(&self, url: &str, init: &FetchInit) -> Result<MinimalResponse, String> {
        self.requests
            .lock()
            .unwrap()
            .push((url.to_string(), init.clone()));
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra page");
        self.active.fetch_add(1, Ordering::SeqCst);
        let mut flight = InFlight {
            http: self,
            completed: false,
        };
        self.started.notify_one();
        if !step.delay.is_zero() {
            tokio::time::sleep(step.delay).await;
        }
        flight.completed = true;
        self.completed.fetch_add(1, Ordering::SeqCst);
        // Deliberately do not enforce max_body_bytes here: the production helper must
        // recheck even a faulty injected transport. Requests still expose the streaming cap.
        step.response
    }
}

fn release(version: &str, apk: bool) -> Value {
    let version_for_asset = polaris_updater::github::strip_v(version);
    let mut assets = vec![json!({
        "name": format!("Polaris_{version_for_asset}_aarch64-mac.dmg"),
        "browser_download_url": format!("https://example.com/{version}/mac.dmg"),
        "size": 123,
        "digest": format!("sha256:{}", "ab".repeat(32)),
    })];
    if apk {
        assets.push(json!({
            "name": format!("Polaris_{version_for_asset}_arm64-v8a-android.apk"),
            "browser_download_url": format!("https://example.com/{version}/app.apk"),
            "size": 124,
        }));
    }
    json!({ "tag_name": version, "assets": assets, "unknown_metadata": { "keep": "原文" } })
}

fn full_page(offset: usize) -> Vec<Value> {
    (offset..offset + GITHUB_RELEASES_PER_PAGE)
        .map(|i| json!({ "tag_name": format!("v1.0.{i}") }))
        .collect()
}

async fn fetch(http: &ScriptedHttp, limits: ReleaseFetchLimits) -> Result<String, String> {
    fetch_release_pages(
        http,
        &PublicDns,
        "polaris-arch",
        "Polaris",
        Instant::now() + Duration::from_millis(GITHUB_FETCH_TIMEOUT_MS),
        limits,
    )
    .await
}

#[tokio::test]
async fn real_pagination_finds_highest_and_current_only_on_page_two_before_host_selection() {
    let first = full_page(0);
    let second = [release("v3.0.0", false), release("v2.0.0", true)];
    let mut first_step = Step::page(&first);
    // Link is not a source of executable request URLs; controlled page numbers are.
    first_step.response.as_mut().unwrap().headers = vec![(
        "Link".to_string(),
        "<http://127.0.0.1/evil>; rel=\"next\"".to_string(),
    )];
    let http = ScriptedHttp::new([first_step, Step::page(&second)]);
    let body = fetch(&http, RELEASE_FETCH_LIMITS).await.unwrap();
    let values: Vec<Value> = serde_json::from_str(&body).unwrap();
    assert_eq!(values.len(), 102);
    assert_eq!(values[100]["unknown_metadata"]["keep"], "原文");
    assert!(body.len() <= MAX_GITHUB_JSON_BYTES);
    let requests = http.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for (index, (url, init)) in requests.iter().enumerate() {
        assert_eq!(
            url,
            &format!(
                "https://api.github.com/repos/polaris-arch/Polaris/releases?per_page=100&page={}",
                index + 1
            )
        );
        assert_eq!(
            init.max_body_bytes,
            Some(
                MAX_GITHUB_JSON_BYTES
                    - if index == 0 {
                        0
                    } else {
                        serde_json::to_vec(&first).unwrap().len()
                    }
            )
        );
        assert!(init
            .headers
            .iter()
            .any(|(name, value)| name == "Accept" && value == "application/vnd.github.v3+json"));
    }
    drop(requests);
    let target = (AssetPlatform::Macos, AssetArch::Arm64, false);
    let newer = check_requested_app_release(&body, "2.0.0", false, false, None, target).unwrap();
    assert!(matches!(newer, AppUpdateCheck::Available(info) if info.version == "v3.0.0"));
    let current = check_requested_app_release(&body, "2.0.0", false, true, None, target).unwrap();
    assert!(matches!(current, AppUpdateCheck::Available(info) if info.version == "v2.0.0"));
    assert_eq!(
        check_requested_app_release(&body, "1.0.0", false, false, Some("3.0.0"), target).unwrap(),
        AppUpdateCheck::NoUpdate
    );
    assert_eq!(
        check_requested_app_release(
            &body,
            "1.0.0",
            false,
            false,
            None,
            (AssetPlatform::Android, AssetArch::Arm64, false)
        )
        .unwrap(),
        AppUpdateCheck::NoUpdate
    );
    let android = release_only_response(&body, "1.0.0", false, None, false)
        .data
        .unwrap();
    assert_eq!(android["updateInfo"]["version"], "v3.0.0");
    assert_eq!(android["updateInfo"]["downloadUrl"], "");

    // Keep the actual App and debug fixture entrypoints attached to the tested production helper.
    let production = crate::commands::guard_scan::top_level_fn_body(
        src(),
        "pub(crate) async fn fetch_releases_json(",
    );
    assert!(production.contains("fetch_release_pages("));
    assert!(production.contains("local_acceptance::releases(MAX_GITHUB_JSON_BYTES).await"));
    assert!(production
        .contains("parse_release_list(body.as_bytes(), RELEASE_FETCH_LIMITS.max_entries)?"));
    assert!(production
        .contains("tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), fetch)"));
}

#[tokio::test]
async fn empty_short_and_exact_multiple_lists_have_proven_terminal_pages() {
    for (pages, count, calls) in [
        (vec![Step::page(&[])], 0, 1),
        (vec![Step::page(&[release("v2.0.0", false)])], 1, 1),
        (
            vec![
                Step::page(&full_page(0)),
                Step::page(&full_page(100)),
                Step::page(&[]),
            ],
            200,
            3,
        ),
    ] {
        let http = ScriptedHttp::new(pages);
        let body = fetch(&http, RELEASE_FETCH_LIMITS).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<Value>>(&body).unwrap().len(),
            count
        );
        assert_eq!(http.calls(), calls);
    }
}

#[tokio::test]
async fn later_http_network_and_ssrf_failures_never_return_partial_releases() {
    let mut first = full_page(0);
    first[0] = release("v9.0.0", true);
    for step in [
        Step {
            response: Err("later connection failed".to_string()),
            delay: Duration::ZERO,
        },
        Step {
            response: Ok(MinimalResponse {
                status: 404,
                ..Default::default()
            }),
            delay: Duration::ZERO,
        },
        Step {
            response: Ok(MinimalResponse {
                status: 503,
                ..Default::default()
            }),
            delay: Duration::ZERO,
        },
        Step {
            response: Ok(MinimalResponse {
                status: 302,
                location: Some("http://127.0.0.1/private".to_string()),
                ..Default::default()
            }),
            delay: Duration::ZERO,
        },
    ] {
        let http = ScriptedHttp::new([Step::page(&first), step]);
        assert!(fetch(&http, RELEASE_FETCH_LIMITS).await.is_err());
        assert_eq!(http.calls(), 2);
    }
}

#[tokio::test]
async fn malformed_later_pages_and_overfull_arrays_fail_schema_validation() {
    for body in [
        b"{}".to_vec(),
        b"null".to_vec(),
        b"[".to_vec(),
        b"[null]".to_vec(),
        br#"[{"tag_name":null}]"#.to_vec(),
        br#"[{"tag_name":"v9.0.0","assets":{}}]"#.to_vec(),
        b"[] []".to_vec(),
        vec![0xff],
    ] {
        let http = ScriptedHttp::new([Step::page(&full_page(0)), Step::bytes(body)]);
        assert!(fetch(&http, RELEASE_FETCH_LIMITS).await.is_err());
        assert_eq!(http.calls(), 2);
    }
    let mut overfull = full_page(0);
    overfull.push(release("v9.0.0", false));
    let http = ScriptedHttp::new([Step::page(&overfull)]);
    assert!(fetch(&http, RELEASE_FETCH_LIMITS)
        .await
        .unwrap_err()
        .contains("entry limit"));
    assert_eq!(http.calls(), 1);
}

#[tokio::test]
async fn total_bytes_pages_entries_and_repeated_page_bounds_are_fail_closed() {
    assert_eq!(GITHUB_RELEASES_PER_PAGE, 100);
    assert_eq!(
        (
            RELEASE_FETCH_LIMITS.max_pages,
            RELEASE_FETCH_LIMITS.max_entries,
            RELEASE_FETCH_LIMITS.max_bytes
        ),
        (20, 2_000, 16 * 1024 * 1024)
    );
    let first = full_page(0);
    let second = [release("v3.0.0", false)];
    let byte_limit =
        serde_json::to_vec(&first).unwrap().len() + serde_json::to_vec(&second).unwrap().len() - 1;
    let http = ScriptedHttp::new([Step::page(&first), Step::page(&second)]);
    assert!(fetch(
        &http,
        ReleaseFetchLimits {
            max_bytes: byte_limit,
            ..RELEASE_FETCH_LIMITS
        }
    )
    .await
    .unwrap_err()
    .contains("byte limit"));
    assert_eq!(http.calls(), 2);

    let http = ScriptedHttp::new([Step::page(&first)]);
    assert!(fetch(
        &http,
        ReleaseFetchLimits {
            max_pages: 1,
            ..RELEASE_FETCH_LIMITS
        }
    )
    .await
    .unwrap_err()
    .contains("page limit"));
    assert_eq!(http.calls(), 1);
    let http = ScriptedHttp::new([Step::page(&first), Step::page(&second)]);
    assert!(fetch(
        &http,
        ReleaseFetchLimits {
            max_entries: 100,
            ..RELEASE_FETCH_LIMITS
        }
    )
    .await
    .unwrap_err()
    .contains("entry limit"));
    let http = ScriptedHttp::new([Step::page(&first), Step::page(&[])]);
    assert!(fetch(
        &http,
        ReleaseFetchLimits {
            max_entries: 100,
            ..RELEASE_FETCH_LIMITS
        }
    )
    .await
    .is_ok());

    let http = ScriptedHttp::new([Step::page(&first), Step::page(&first)]);
    assert!(fetch(&http, RELEASE_FETCH_LIMITS)
        .await
        .unwrap_err()
        .contains("repeated"));
    assert_eq!(http.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn all_pages_share_one_deadline_and_timeout_drops_the_later_request() {
    let mut first = Step::page(&full_page(0));
    first.delay = Duration::from_secs(10);
    let mut second = Step::page(&[release("v3.0.0", false)]);
    second.delay = Duration::from_secs(10);
    let http = ScriptedHttp::new([first, second]);
    let started = tokio::time::Instant::now();
    let result = fetch_release_pages(
        &http,
        &PublicDns,
        "polaris-arch",
        "Polaris",
        (started + Duration::from_secs(15)).into_std(),
        RELEASE_FETCH_LIMITS,
    )
    .await;
    assert!(result.unwrap_err().contains("timeout"));
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "per-page timeout must not reset"
    );
    assert_eq!(http.calls(), 2);
    assert_eq!(http.completed.load(Ordering::SeqCst), 1);
    assert_eq!(http.canceled.load(Ordering::SeqCst), 1);
    assert_eq!(http.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn canceling_discovery_drops_in_flight_transport_without_partial_success_or_tasks() {
    let mut step = Step::page(&full_page(0));
    step.delay = Duration::from_secs(3_600);
    let http = ScriptedHttp::new([step]);
    let mut discovery = Box::pin(fetch(&http, RELEASE_FETCH_LIMITS));
    tokio::select! {
        result = &mut discovery => panic!("unexpected early discovery result: {result:?}"),
        () = http.started.notified() => {},
    }
    drop(discovery);
    assert_eq!(http.calls(), 1);
    assert_eq!(http.active.load(Ordering::SeqCst), 0);
    assert_eq!(http.canceled.load(Ordering::SeqCst), 1);
    assert_eq!(http.completed.load(Ordering::SeqCst), 0);
}
