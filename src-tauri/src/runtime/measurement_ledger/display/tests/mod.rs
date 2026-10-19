use super::*;
use crate::commands::speedtest::{
    FailKind, FailPhase, MeasureFailure, ResultIdentity, SpeedTestOrigin,
};
use crate::runtime::node_fingerprints::MeasurementConnections;

fn connection_view(values: &BTreeMap<String, String>) -> MeasurementConnections {
    MeasurementConnections {
        main: values.clone(),
        temp: values.clone(),
    }
}
fn connections() -> BTreeMap<String, String> {
    BTreeMap::from([("a".to_owned(), "e".repeat(64))])
}
fn record(
    ledger: &MeasurementLedger,
    id: &str,
    measured: super::super::Measured,
    identity: ResultIdentity,
    mono: u64,
) {
    ledger.record_connection_at(id, measured, identity, Some("e".repeat(64)), mono);
}

fn identity(run: u64, path: MeasurePath) -> ResultIdentity {
    ResultIdentity {
        core_binary_sha256: None,
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
        core_binary_sha256: None,
        main_generation: Some(7),
        fingerprints,
        main_connections: Box::leak(Box::new(connections())),
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
    record(
        &ledger,
        "a",
        Ok(30),
        identity(1, MeasurePath::Candidate),
        100,
    );
    record(&ledger, "a", Ok(5), identity(2, MeasurePath::System), 200);
    let read = context(&fingerprints);
    let before = ledger.candidates(&["a".to_owned()], &read);
    assert_eq!(before.selectable.len(), 1);
    let rows = ledger.display_rows(&read, Some("config"), &connection_view(&connections()));
    assert_eq!(rows["a"]["candidate"]["latency"], 30);
    assert_eq!(rows["a"]["system"]["latency"], 5);
    assert_eq!(before, ledger.candidates(&["a".to_owned()], &read));
}

#[test]
fn old_node_generation_network_url_and_age_are_not_current_success() {
    let ledger = MeasurementLedger::new();
    record(
        &ledger,
        "a",
        Ok(30),
        identity(1, MeasurePath::Candidate),
        100,
    );
    let fingerprints = BTreeMap::from([("a".to_owned(), "changed".to_owned())]);
    assert_eq!(
        ledger.display_rows(
            &context(&fingerprints),
            Some("config"),
            &connection_view(&connections())
        )["a"]["candidate"]["reason"],
        "nodeChanged"
    );
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    let mut read = context(&fingerprints);
    read.main_generation = Some(8);
    assert_eq!(
        ledger.display_rows(&read, Some("config"), &connection_view(&connections()))["a"]
            ["candidate"]["state"],
        "expired"
    );
    read.main_generation = Some(7);
    read.network_epoch = Some(4);
    assert_eq!(
        ledger.display_rows(&read, Some("config"), &connection_view(&connections()))["a"]
            ["candidate"]["reason"],
        "networkChanged"
    );
    read.network_epoch = Some(3);
    read.url_digest = "other";
    assert_eq!(
        ledger.display_rows(&read, Some("config"), &connection_view(&connections()))["a"]
            ["candidate"]["reason"],
        "urlChanged"
    );
    read.url_digest = "url";
    read.now_ms = 1101;
    assert_eq!(
        ledger.display_rows(&read, Some("config"), &connection_view(&connections()))["a"]
            ["candidate"]["state"],
        "expired"
    );
    assert_eq!(
        ledger.display_rows(
            &context(&BTreeMap::new()),
            Some("config"),
            &connection_view(&connections())
        ),
        json!({})
    );
}

#[test]
fn same_generation_changed_running_configuration_is_not_current_success() {
    let ledger = MeasurementLedger::new();
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    record(
        &ledger,
        "a",
        Ok(30),
        identity(1, MeasurePath::Candidate),
        100,
    );
    let rows = ledger.display_rows(
        &context(&fingerprints),
        Some("different emission"),
        &connection_view(&connections()),
    );
    assert_eq!(rows["a"]["candidate"]["state"], "expired");
    assert_eq!(rows["a"]["candidate"]["reason"], "configurationChanged");
    assert_eq!(
        ledger.display_rows(
            &context(&fingerprints),
            None,
            &connection_view(&connections())
        )["a"]["candidate"]["reason"],
        "identityUnknown"
    );
}

#[test]
fn real_failure_skip_disconnected_and_missing_identity_remain_distinct() {
    let ledger = MeasurementLedger::new();
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    record(
        &ledger,
        "a",
        Err(MeasureFailure::new(FailPhase::Measure, FailKind::Timeout)),
        identity(1, MeasurePath::Candidate),
        100,
    );
    assert_eq!(
        ledger.display_rows(
            &context(&fingerprints),
            Some("config"),
            &connection_view(&connections())
        )["a"]["candidate"]["state"],
        "failed"
    );
    ledger.set_display_skipped(
        &["a".to_owned()],
        &[("a".to_owned(), "notInPool")],
        identity(1, MeasurePath::Candidate),
        &connections(),
    );
    assert_eq!(
        ledger.display_rows(
            &context(&fingerprints),
            Some("config"),
            &connection_view(&connections())
        )["a"]["notIncluded"],
        "notInPool"
    );
    let mut temp = identity(2, MeasurePath::Candidate);
    temp.instance = CoreInstance::Temp;
    record(&ledger, "a", Ok(40), temp, 200);
    let rows = ledger.display_rows(
        &context(&fingerprints),
        Some("config"),
        &connection_view(&connections()),
    );
    assert_eq!(rows["a"]["candidate"]["disconnected"], true);
    assert_eq!(rows["a"]["candidate"]["state"], "success");
    assert!(rows["a"].get("notIncluded").is_none());
    let mut unknown = identity(3, MeasurePath::Candidate);
    unknown.node_fingerprint = None;
    record(&ledger, "a", Ok(1), unknown, 300);
    assert_eq!(
        ledger.display_rows(
            &context(&fingerprints),
            Some("config"),
            &connection_view(&connections())
        )["a"]["candidate"]["reason"],
        "identityUnknown"
    );
}

#[test]
fn config_content_digest_matches_the_renderer_shared_golden_bytes() {
    let fixtures: Value = serde_json::from_str(&crate::test_support::repo_file(
        "ui/src/contracts/measurement-config-digest.fixture.json",
    ))
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

#[test]
fn actual_producer_identity_round_trips_to_the_renderer_golden_without_raw_credentials() {
    use crate::commands::speedtest::{RunEvents, SpeedTestRequest};
    use crate::runtime::node_fingerprints;
    let fixture: Value = serde_json::from_str(&crate::test_support::repo_file(
        "ui/src/contracts/measurement-producer.fixture.json",
    ))
    .unwrap();
    let config = json!({"servers": [fixture["server"].clone()]});
    let server = serde_json::from_value(fixture["server"].clone()).unwrap();
    let legacy = node_fingerprints::dirty_fingerprint(&server);
    assert_eq!(legacy, fixture["legacyFingerprint"]);
    let fingerprints = BTreeMap::from([("interop".to_owned(), legacy)]);
    let connections = node_fingerprints::measurement_table_json(&config);
    let ledger = Box::leak(Box::new(MeasurementLedger::new()));
    let request = SpeedTestRequest {
        origin: SpeedTestOrigin::Manual,
        targets: vec!["interop".to_owned()],
        scope: None,
        path: MeasurePath::Candidate,
        url: fixture["url"].as_str().unwrap().to_owned(),
    };
    let mut events = RunEvents::new(
        "10",
        &request,
        CoreInstance::Main {
            generation: 7,
            start_time: Some(10),
        },
        Some("b".repeat(64)),
        fingerprints.clone(),
        None,
    )
    .with_ledger(ledger)
    .with_connection_fingerprints(connections.clone());
    let payload = events
        .stamp(
            crate::events::channel::EVENT_SPEED_TEST_RESULT,
            crate::commands::speedtest::MeasurementEvent::Result(
                crate::commands::speedtest::NodeResult::new("interop", &Ok(40), Some(3), None),
            ),
        )
        .unwrap();
    let entry = ledger.lock().entries.values().next().unwrap().clone();
    // The existing event has the original raw legacy shape; only the read IPC is digest-only.
    assert_eq!(
        payload["identity"]["nodeFingerprint"],
        fixture["legacyFingerprint"]
    );
    let mut read = context(&fingerprints);
    read.url_digest = fixture["url"]
        .as_str()
        .map(|_| entry.identity.url_digest.as_str())
        .unwrap();
    read.now_ms = entry.identity.measured_at;
    let mut rows =
        ledger.display_rows(&read, Some(&"b".repeat(64)), &connection_view(&connections));
    // Wall time is intentionally the only runtime-generated fixture field.
    rows["interop"]["candidate"]["identity"]["measuredAt"] = json!(1000);
    rows["interop"]["candidate"]["expiresAt"] = json!(2000);
    assert_eq!(rows, fixture["snapshot"]["rows"]);
    assert!(!serde_json::to_string(&rows)
        .unwrap()
        .contains("fixture-secret"));
}

#[test]
fn tls_sni_ws_edits_expire_temp_results_without_changing_the_subscription_fingerprint() {
    use crate::runtime::node_fingerprints;
    let base = json!({"id":"a", "name":"A", "protocol":"trojan", "address":"example.test", "port":443,
        "password":"fixture", "network":"ws", "security":"tls", "tlsSettings":{"serverName":"old.test"},
        "wsSettings":{"path":"/old"}});
    let typed = serde_json::from_value(base.clone()).unwrap();
    let legacy = node_fingerprints::dirty_fingerprint(&typed);
    let config = json!({"servers":[base.clone()]});
    let connections = node_fingerprints::measurement_table_json(&config);
    let fingerprints = BTreeMap::from([("a".to_owned(), legacy.clone())]);
    let ledger = MeasurementLedger::new();
    let mut temp = identity(1, MeasurePath::Candidate);
    temp.instance = CoreInstance::Temp;
    temp.node_fingerprint = Some(legacy.clone());
    ledger.record_connection_at("a", Ok(40), temp, connections.get("a").cloned(), 100);
    assert_eq!(
        ledger.display_rows(
            &context(&fingerprints),
            None,
            &connection_view(&connections)
        )["a"]["candidate"]["state"],
        "success"
    );
    for (key, value) in [
        ("security", json!("none")),
        ("tlsSettings", json!({"serverName":"new.test"})),
        ("wsSettings", json!({"path":"/new"})),
    ] {
        let mut changed = base.clone();
        changed[key] = value;
        let typed = serde_json::from_value(changed.clone()).unwrap();
        assert_eq!(node_fingerprints::dirty_fingerprint(&typed), legacy);
        let current = node_fingerprints::measurement_table_json(&json!({"servers":[changed]}));
        let rows = ledger.display_rows(&context(&fingerprints), None, &connection_view(&current));
        assert_eq!(rows["a"]["candidate"]["state"], "expired");
        assert_eq!(rows["a"]["candidate"]["reason"], "connectionChanged");
    }
}

#[test]
fn omission_identity_is_bounded_by_node_url_generation_network_and_cleared_by_new_result() {
    let ledger = MeasurementLedger::new();
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    ledger.set_display_skipped(
        &["a".to_owned()],
        &[("a".to_owned(), "tsNotReady")],
        identity(2, MeasurePath::Candidate),
        &connections(),
    );
    assert_eq!(
        ledger.display_rows(
            &context(&fingerprints),
            Some("config"),
            &connection_view(&connections())
        )["a"]["notIncluded"],
        "tsNotReady"
    );
    let changed = BTreeMap::from([("a".to_owned(), "f".repeat(64))]);
    assert!(ledger
        .display_rows(
            &context(&fingerprints),
            Some("config"),
            &connection_view(&changed)
        )
        .get("a")
        .is_none());
    let mut read = context(&fingerprints);
    read.main_generation = Some(8);
    assert!(ledger
        .display_rows(&read, Some("config"), &connection_view(&connections()))
        .get("a")
        .is_none());
    read.main_generation = Some(7);
    read.network_epoch = Some(4);
    assert!(ledger
        .display_rows(&read, Some("config"), &connection_view(&connections()))
        .get("a")
        .is_none());
    read.network_epoch = Some(3);
    read.url_digest = "other";
    assert!(ledger
        .display_rows(&read, Some("config"), &connection_view(&connections()))
        .get("a")
        .is_none());
    record(
        &ledger,
        "a",
        Ok(30),
        identity(3, MeasurePath::Candidate),
        200,
    );
    assert!(ledger.display_rows(
        &context(&fingerprints),
        Some("config"),
        &connection_view(&connections())
    )["a"]
        .get("notIncluded")
        .is_none());
    ledger.set_display_skipped(
        &["a".to_owned()],
        &[("a".to_owned(), "notInPool")],
        identity(2, MeasurePath::Candidate),
        &connections(),
    );
    assert!(ledger.display_rows(
        &context(&fingerprints),
        Some("config"),
        &connection_view(&connections())
    )["a"]
        .get("notIncluded")
        .is_none());
}

#[test]
fn measurement_digest_binds_detour_parameters_and_rejects_missing_or_cyclic_chains() {
    use crate::runtime::node_fingerprints::measurement_table_json;
    let mut config = json!({"servers":[
        {"id":"a","name":"A","protocol":"socks","address":"a.test","port":1080,"detour":"b"},
        {"id":"b","name":"B","protocol":"trojan","address":"b.test","port":443,"password":"old"}]});
    let before = measurement_table_json(&config);
    config["servers"][1]["name"] = json!("Renamed");
    assert_eq!(measurement_table_json(&config)["a"], before["a"]);
    config["servers"][1]["password"] = json!("new");
    assert_ne!(measurement_table_json(&config)["a"], before["a"]);
    config["servers"][1]["detour"] = json!("a");
    assert!(measurement_table_json(&config).is_empty());
    config["servers"][1]["detour"] = json!("missing");
    assert!(measurement_table_json(&config).is_empty());
}

#[test]
fn admitted_manual_and_temp_prefilters_publish_all_reasons_before_the_first_real_result() {
    use crate::commands::speedtest::{RunEvents, SpeedTestRequest};
    for instance in [
        CoreInstance::Main {
            generation: 7,
            start_time: Some(10),
        },
        CoreInstance::Temp,
    ] {
        let ledger = Box::leak(Box::new(MeasurementLedger::new()));
        let members = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let fingerprints: BTreeMap<String, String> = members
            .iter()
            .map(|id| (id.clone(), "node".to_owned()))
            .collect();
        let connections: BTreeMap<String, String> = members
            .iter()
            .map(|id| (id.clone(), "e".repeat(64)))
            .collect();
        let disconnected = instance == CoreInstance::Temp;
        let network = if disconnected { Some(0) } else { Some(3) };
        let emission = if disconnected {
            None
        } else {
            Some("config".to_owned())
        };
        let request = SpeedTestRequest {
            origin: SpeedTestOrigin::Manual,
            targets: members.clone(),
            scope: None,
            path: MeasurePath::Candidate,
            url: "https://example.test/measure".to_owned(),
        };
        let mut events = RunEvents::new(
            "10",
            &request,
            instance,
            emission.clone(),
            fingerprints.clone(),
            None,
        )
        .with_ledger(ledger)
        .with_connection_fingerprints(connections.clone())
        .with_display_skipped(
            &members,
            &[
                ("a".to_owned(), "notInPool"),
                ("b".to_owned(), "dirty"),
                ("c".to_owned(), "tsNotReady"),
            ],
            &connections,
            network,
        );
        let url_digest = polaris_updater::sha256_hex(request.url.as_bytes());
        let mut read = context(&fingerprints);
        read.main_generation = if disconnected { None } else { Some(7) };
        read.network_epoch = network;
        read.url_digest = &url_digest;
        read.now_ms = crate::runtime::subscription_scheduler::now_ms();
        let rows = ledger.display_rows(&read, emission.as_deref(), &connection_view(&connections));
        for (id, reason) in [("a", "notInPool"), ("b", "dirty"), ("c", "tsNotReady")] {
            assert_eq!(rows[id]["notIncluded"], reason);
        }
        if disconnected {
            assert_eq!(read.main_generation, None);
            assert_eq!(read.network_epoch, Some(0));
            read.network_epoch = Some(1);
            let changed = ledger.display_rows(&read, None, &connection_view(&connections));
            for id in &members {
                assert!(changed.get(id).is_none());
            }
            read.network_epoch = Some(0);
        }
        // Existing temp results leave network epoch unknown; omission registration
        // independently uses the actual desktop runtime counter above.
        let result_network = if disconnected { None } else { network };
        let result = events.stamp(
            crate::events::channel::EVENT_SPEED_TEST_RESULT,
            crate::commands::speedtest::MeasurementEvent::Result(
                crate::commands::speedtest::NodeResult::new("a", &Ok(30), result_network, None),
            ),
        );
        assert!(result.is_some());
        read.now_ms = crate::runtime::subscription_scheduler::now_ms();
        let rows = ledger.display_rows(&read, emission.as_deref(), &connection_view(&connections));
        assert!(rows["a"].get("notIncluded").is_none());
        assert_eq!(rows["a"]["candidate"]["state"], "success");
        assert_eq!(rows["b"]["notIncluded"], "dirty");
    }
}

#[test]
fn a_system_measurement_does_not_clear_candidate_prefilter_omission() {
    let ledger = MeasurementLedger::new();
    let fingerprints = BTreeMap::from([("a".to_owned(), "node".to_owned())]);
    ledger.set_display_skipped(
        &["a".to_owned()],
        &[("a".to_owned(), "notInPool")],
        identity(1, MeasurePath::Candidate),
        &connections(),
    );
    record(&ledger, "a", Ok(5), identity(2, MeasurePath::System), 200);
    let rows = ledger.display_rows(
        &context(&fingerprints),
        Some("config"),
        &connection_view(&connections()),
    );
    assert_eq!(rows["a"]["notIncluded"], "notInPool");
    assert_eq!(rows["a"]["system"]["state"], "success");
}

#[test]
fn temp_subscription_and_global_interface_edits_expire_results_without_changing_node_or_stable_identity(
) {
    use crate::runtime::node_fingerprints;
    use polaris_config_engine::user_config::app_config::{
        NetworkInterfaceDefaults, SubscriptionInterfacePolicy,
    };
    use polaris_config_engine::user_config::UserConfig;
    let server: polaris_config_engine::user_config::server_config::ServerConfig = serde_json::from_value(json!({"id":"a","name":"A","protocol":"trojan", "address":"a.test","port":443,"password":"fixture","subscriptionId":"sub"})).unwrap();
    for subscription_override in [true, false] {
        let mut config = UserConfig {
            servers: vec![server.clone()],
            network_interfaces: Some(NetworkInterfaceDefaults {
                direct: None,
                proxy: Some("Global-A".to_owned()),
            }),
            subscriptions: vec![SubscriptionInterfacePolicy {
                id: "sub".to_owned(),
                proxy_bind_interface: subscription_override.then(|| "Sub-A".to_owned()),
                speed_test_interval_minutes: None,
            }],
            ..Default::default()
        };
        let expected = if subscription_override {
            "Sub-A"
        } else {
            "Global-A"
        };
        assert_eq!(
            polaris_config_engine::builder::outbounds::effective_proxy_bind_interface(
                &config.servers[0],
                &config
            )
            .as_deref(),
            Some(expected)
        );
        // This exact typed input also produces the real temp plan's interface map.
        let measured = node_fingerprints::measurement_temp_table(&config.servers, &config);
        let legacy = node_fingerprints::dirty_fingerprint(&config.servers[0]);
        let fingerprints = BTreeMap::from([("a".to_owned(), legacy.clone())]);
        let ledger = MeasurementLedger::new();
        let mut temp = identity(1, MeasurePath::Candidate);
        temp.instance = CoreInstance::Temp;
        temp.config_digest = None;
        temp.network_epoch = None;
        temp.node_fingerprint = Some(legacy.clone());
        ledger.record_connection_at("a", Ok(40), temp, measured.get("a").cloned(), 100);
        let mut read = context(&fingerprints);
        read.main_generation = None;
        read.network_epoch = Some(0);
        let current = node_fingerprints::measurement_connections_json(
            &serde_json::to_value(&config).unwrap(),
        );
        assert_eq!(current.temp, measured);
        assert_eq!(
            ledger.display_rows(&read, None, &current)["a"]["candidate"]["state"],
            "success"
        );
        if subscription_override {
            config.subscriptions[0].proxy_bind_interface = Some("Sub-B".to_owned());
        } else {
            config.network_interfaces.as_mut().unwrap().proxy = Some("Global-B".to_owned());
        }
        assert_eq!(config.servers[0], server);
        assert_eq!(
            node_fingerprints::dirty_fingerprint(&config.servers[0]),
            legacy
        );
        let changed = node_fingerprints::measurement_connections_json(
            &serde_json::to_value(&config).unwrap(),
        );
        let rows = ledger.display_rows(&read, None, &changed);
        assert_eq!(rows["a"]["candidate"]["state"], "expired");
        assert_eq!(rows["a"]["candidate"]["reason"], "connectionChanged");
    }
}

#[test]
fn main_detour_identity_follows_physical_root_but_temp_identity_follows_its_actual_single_node_binding(
) {
    use crate::runtime::node_fingerprints;
    use polaris_config_engine::user_config::app_config::{
        NetworkInterfaceDefaults, SubscriptionInterfacePolicy,
    };
    use polaris_config_engine::user_config::UserConfig;
    let mut config=UserConfig {servers:serde_json::from_value(json!([
        {"id":"a","name":"A","protocol":"socks","address":"a.test","port":1080,"detour":"b","subscriptionId":"sub"},
        {"id":"b","name":"B","protocol":"socks","address":"b.test","port":1080,"bindInterface":"Root-A"}])).unwrap(),
        subscriptions:vec![SubscriptionInterfacePolicy{id:"sub".to_owned(),proxy_bind_interface:Some("Child-A".to_owned()),speed_test_interval_minutes:None}],
        network_interfaces:Some(NetworkInterfaceDefaults{direct:None,proxy:Some("Global-A".to_owned())}),..Default::default()};
    let read = |config: &UserConfig| {
        node_fingerprints::measurement_connections_json(&serde_json::to_value(config).unwrap())
    };
    let before = read(&config);
    config.subscriptions[0].proxy_bind_interface = Some("Child-B".to_owned());
    let child_changed = read(&config);
    assert_eq!(
        child_changed.main["a"], before.main["a"],
        "main child opens no physical socket"
    );
    assert_ne!(
        child_changed.temp["a"], before.temp["a"],
        "temp strips detour and uses this node's resolved interface"
    );
    config.servers[1].bind_interface = Some("Root-B".to_owned());
    let root_changed = read(&config);
    assert_ne!(
        root_changed.main["a"], child_changed.main["a"],
        "physical root interface propagates through detour identity"
    );
    assert_eq!(
        root_changed.temp["a"], child_changed.temp["a"],
        "temp did not measure the detached root"
    );
}
