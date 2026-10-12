use super::*;
use crate::commands::speedtest::{
    FailKind, FailPhase, MeasureFailure, ResultIdentity, SpeedTestOrigin,
};

fn identity(run: u64, path: MeasurePath) -> ResultIdentity {
    ResultIdentity {
        run,
        seq: 1,
        origin: SpeedTestOrigin::Manual,
        scope: None,
        path,
        url_digest: "url".to_owned(),
        instance: CoreInstance::Main {
            generation: 7,
            start_time: Some(10),
        },
        config_digest: Some("config".to_owned()),
        node_fingerprint: Some("node".to_owned()),
        network_epoch: Some(3),
        measured_at: 100,
        binding: None,
    }
}

fn context(fingerprints: &BTreeMap<String, String>) -> ReadContext<'_> {
    ReadContext {
        main_generation: Some(7),
        fingerprints,
        network_epoch: Some(3),
        url_digest: "url",
        now_ms: 200,
        foreground_epoch: None,
        freshness_cap_ms: &|_, _| 1000,
    }
}

#[test]
fn display_does_not_replace_candidates_with_a_newer_system_measurement() {
    let ledger = MeasurementLedger::new();
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    ledger.record_at("a", Ok(30), identity(1, MeasurePath::Candidate), 100);
    ledger.record_at("a", Ok(5), identity(2, MeasurePath::System), 200);
    let read = context(&fingerprints);
    let before = ledger.candidates(&["a".to_owned()], &read);
    let rows = ledger.display_rows(&read, Some("config"));
    assert_eq!(rows["a"]["candidate"]["latency"], 30);
    assert_eq!(rows["a"]["system"]["latency"], 5);
    assert_eq!(before, ledger.candidates(&["a".to_owned()], &read));
}

#[test]
fn old_node_generation_network_url_and_age_are_not_current_success() {
    let ledger = MeasurementLedger::new();
    ledger.record_at("a", Ok(30), identity(1, MeasurePath::Candidate), 100);
    let fingerprints = BTreeMap::from([("a".to_owned(), "changed".to_owned())]);
    assert_eq!(
        ledger.display_rows(&context(&fingerprints), Some("config"))["a"]["candidate"]["reason"],
        "nodeChanged"
    );
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    let mut read = context(&fingerprints);
    read.main_generation = Some(8);
    assert_eq!(
        ledger.display_rows(&read, Some("config"))["a"]["candidate"]["state"],
        "expired"
    );
    read.main_generation = Some(7);
    read.network_epoch = Some(4);
    assert_eq!(
        ledger.display_rows(&read, Some("config"))["a"]["candidate"]["reason"],
        "networkChanged"
    );
    read.network_epoch = Some(3);
    read.url_digest = "other";
    assert_eq!(
        ledger.display_rows(&read, Some("config"))["a"]["candidate"]["reason"],
        "urlChanged"
    );
    read.url_digest = "url";
    read.now_ms = 1101;
    assert_eq!(
        ledger.display_rows(&read, Some("config"))["a"]["candidate"]["state"],
        "expired"
    );
    assert_eq!(
        ledger.display_rows(&context(&BTreeMap::new()), Some("config")),
        json!({})
    );
}

#[test]
fn same_generation_changed_running_configuration_is_not_current_success() {
    let ledger = MeasurementLedger::new();
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    ledger.record_at("a", Ok(30), identity(1, MeasurePath::Candidate), 100);
    let rows = ledger.display_rows(&context(&fingerprints), Some("different emission"));
    assert_eq!(rows["a"]["candidate"]["state"], "expired");
    assert_eq!(rows["a"]["candidate"]["reason"], "configurationChanged");
    assert_eq!(
        ledger.display_rows(&context(&fingerprints), None)["a"]["candidate"]["reason"],
        "identityUnknown"
    );
}

#[test]
fn real_failure_skip_disconnected_and_missing_identity_remain_distinct() {
    let ledger = MeasurementLedger::new();
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    ledger.record_at(
        "a",
        Err(MeasureFailure::new(FailPhase::Measure, FailKind::Timeout)),
        identity(1, MeasurePath::Candidate),
        100,
    );
    assert_eq!(
        ledger.display_rows(&context(&fingerprints), Some("config"))["a"]["candidate"]["state"],
        "failed"
    );
    ledger.set_skipped(&["a".to_owned()], &[("a".to_owned(), "notInPool")]);
    assert_eq!(
        ledger.display_rows(&context(&fingerprints), Some("config"))["a"]["notIncluded"],
        "notInPool"
    );
    let mut temp = identity(2, MeasurePath::Candidate);
    temp.instance = CoreInstance::Temp;
    ledger.record_at("a", Ok(40), temp, 200);
    let rows = ledger.display_rows(&context(&fingerprints), Some("config"));
    assert_eq!(rows["a"]["candidate"]["disconnected"], true);
    assert_eq!(rows["a"]["candidate"]["state"], "success");
    assert!(rows["a"].get("notIncluded").is_none());
    let mut unknown = identity(3, MeasurePath::Candidate);
    unknown.node_fingerprint = None;
    ledger.record_at("a", Ok(1), unknown, 300);
    assert_eq!(
        ledger.display_rows(&context(&fingerprints), Some("config"))["a"]["candidate"]["reason"],
        "identityUnknown"
    );
}

#[test]
fn config_content_digest_matches_the_renderer_shared_golden_bytes() {
    let fixtures: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ui/src/contracts/measurement-config-digest.fixture.json"
    )))
    .expect("measurement config fixtures");
    for fixture in fixtures.as_array().expect("fixtures are an array") {
        let canonical =
            polaris_config_engine::builder::orchestration::stable_stringify(&fixture["input"]);
        assert_eq!(
            canonical,
            fixture["canonical"].as_str().expect("canonical bytes")
        );
        assert_eq!(
            polaris_updater::sha256_hex(canonical.as_bytes()),
            fixture["sha256"].as_str().expect("golden SHA")
        );
    }
}
