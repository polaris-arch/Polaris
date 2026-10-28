use super::*;
use serde_json::json;
fn gateway(address: Value) -> Value {
    json!({"inbounds":[{"type":"tun","auto_route":true,"auto_redirect":true,"include_mac_address":["00:11:22:33:44:55"],"address":address}]})
}
#[test]
fn ordinary_listener_local_tun_and_neighbor_names_never_read_forwarding() {
    for config in [
        json!({"inbounds":[{"type":"mixed","listen":"::"}]}),
        json!({"inbounds":[{"type":"tun","address":["172.19.0.1/16"],"auto_route":true}],"dns":{"neighbor_domain":[".lan"]}}),
    ] {
        assert_eq!(
            check_with(&config, "linux", |_| panic!("unrelated global read")),
            Ok(())
        );
    }
}
#[test]
fn address_families_are_from_emitted_tun_and_deduplicated() {
    let mut reads = vec![];
    assert_eq!(
        check_with(
            &gateway(json!(["172.19.0.1/16", "fd00::1/64"])),
            "linux",
            |six| {
                reads.push(six);
                Ok("1\n".into())
            }
        ),
        Ok(())
    );
    assert_eq!(reads, vec![false, true]);
    assert_eq!(
        check_with(&gateway(json!(["172.19.0.1/16"])), "linux", |six| {
            assert!(!six);
            Ok("1".into())
        }),
        Ok(())
    );
}
#[test]
fn missing_disabled_malformed_are_errors_not_automatic_writes() {
    for (read, wanted) in [
        (Ok("0".into()), REQUIRED),
        (Ok("2".into()), UNCONFIRMED),
        (Ok("".into()), UNCONFIRMED),
        (Err(()), UNCONFIRMED),
    ] {
        assert_eq!(
            check_with(&gateway(json!(["fd00::1/64"])), "linux", |_| read.clone()),
            Err(wanted)
        );
    }
    for address in [
        json!([]),
        json!(["bad"]),
        json!(["1.2.3.4/33"]),
        json!(["::1/129"]),
    ] {
        assert_eq!(
            check_with(&gateway(address), "linux", |_| panic!(
                "malformed before read"
            )),
            Err(UNCONFIRMED)
        );
    }
}
#[test]
fn exclude_mac_gate_and_platform_are_not_allow_lan() {
    let mut config = gateway(json!(["1.2.3.4/32"]));
    config["inbounds"][0]
        .as_object_mut()
        .unwrap()
        .remove("include_mac_address");
    config["inbounds"][0]["exclude_mac_address"] = json!(["00:11:22:33:44:55"]);
    assert_eq!(
        check_with(&config, "linux", |_| Ok("0".into())),
        Err(REQUIRED)
    );
    assert_eq!(
        check_with(&config, "darwin", |_| panic!("Linux-only capability")),
        Ok(())
    );
    config["inbounds"][0]["auto_route"] = json!(false);
    assert_eq!(
        check_with(&config, "linux", |_| panic!("invalid routed intent")),
        Err(UNCONFIRMED)
    );
}
#[test]
fn empty_filters_need_no_router_and_missing_redirect_cannot_pass() {
    let mut config = gateway(json!(["1.2.3.4/32"]));
    config["inbounds"][0]["include_mac_address"] = json!([]);
    assert_eq!(
        check_with(&config, "linux", |_| panic!("empty MAC is local TUN")),
        Ok(())
    );
    config["inbounds"][0]["include_mac_address"] = json!(["00:11:22:33:44:55"]);
    config["inbounds"][0]
        .as_object_mut()
        .unwrap()
        .remove("auto_redirect");
    assert_eq!(
        check_with(&config, "linux", |_| panic!("invalid before read")),
        Err(UNCONFIRMED)
    );
}
#[test]
fn ipv6_only_duplicate_families_and_all_baselines() {
    let mut calls = vec![];
    assert_eq!(
        check_with(
            &gateway(json!(["fd00::1/64", "fd01::1/64"])),
            "linux",
            |six| {
                calls.push(six);
                Ok("1".into())
            }
        ),
        Ok(())
    );
    assert_eq!(calls, vec![true]);
    for values in [[0, 0], [1, 0], [0, 1], [1, 1]] {
        let original = values;
        let result = check_with(
            &gateway(json!(["1.2.3.4/32", "fd00::1/64"])),
            "linux",
            |six| Ok(values[usize::from(six)].to_string()),
        );
        assert_eq!(
            result,
            if values == [1, 1] {
                Ok(())
            } else {
                Err(REQUIRED)
            }
        );
        assert_eq!(values, original);
    }
}
#[test]
fn early_builder_uses_actual_mac_and_family_emission_and_rejects_parse_failure() {
    assert_eq!(
        check_user_config(&Value::Null, "linux"),
        Err(UserPreflightError::InvalidConfig)
    );
    let mut user =
        serde_json::to_value(polaris_config_engine::user_config::UserConfig::default()).unwrap();
    user["proxyModeType"] = json!("tun");
    user["allowLan"] = json!(true);
    user["enableIPv6"] = json!(true);
    user["tunConfig"] = serde_json::to_value(
        polaris_config_engine::user_config::tun_config::TunModeConfig::default(),
    )
    .unwrap();
    user["tunConfig"]["macFilterMode"] = json!("include");
    user["tunConfig"]["macFilterList"] = json!([" 00:11:22:33:44:55 ", "invalid"]);
    user["tunConfig"]["inet4Address"] = json!("172.19.0.1/16");
    user["tunConfig"]["inet6Address"] = json!("fd00::1/64");
    let emitted = user_inbounds(&serde_json::from_value(user.clone()).unwrap(), "linux").unwrap();
    let tun = emitted["inbounds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|inbound| inbound["type"] == "tun")
        .unwrap();
    assert_eq!(tun["include_mac_address"], json!(["00:11:22:33:44:55"]));
    assert_eq!(tun["auto_route"], json!(true));
    assert_eq!(tun["auto_redirect"], json!(true));
    assert_eq!(tun["address"], json!(["172.19.0.1/16", "fd00::1/64"]));
    let mut reads = vec![];
    assert_eq!(
        check_with(&emitted, "linux", |six| {
            reads.push(six);
            Ok("1".into())
        }),
        Ok(())
    );
    assert_eq!(reads, vec![false, true]);
    for field in ["macFilterList", "neighborDomains"] {
        user["tunConfig"]["macFilterList"] = json!([]);
        user["tunConfig"][field] = json!([]);
        let emitted =
            user_inbounds(&serde_json::from_value(user.clone()).unwrap(), "linux").unwrap();
        assert_eq!(
            check_with(&emitted, "linux", |_| panic!("no emitted MAC")),
            Ok(())
        );
    }
    user["tunConfig"]["macFilterList"] = json!(["00:11:22:33:44:55"]);
    user["tunConfig"]["autoRoute"] = json!(false);
    let emitted = user_inbounds(&serde_json::from_value(user.clone()).unwrap(), "linux").unwrap();
    assert_eq!(
        check_with(&emitted, "linux", |_| panic!("builder does not emit MAC")),
        Ok(())
    );
}

#[test]
fn startup_and_hot_apply_share_readonly_gate_before_commit() {
    let start = crate::test_support::crate_source("runtime/proxy/startup.rs");
    let hot = crate::test_support::crate_source("runtime/proxy/runtime_config.rs");
    let lifecycle = crate::test_support::crate_source("runtime/proxy/lifecycle.rs");
    let start_entry = lifecycle.split("pub async fn start(").nth(1).unwrap();
    assert!(
        start_entry.find("gateway_start_preflight").unwrap()
            < start_entry.find("explicit_start_completion").unwrap()
    );
    let restart = lifecycle.split("async fn restart_inner(").nth(1).unwrap();
    assert!(
        restart.find("gateway_start_preflight").unwrap()
            < restart.find("self.admit_legacy_start").unwrap()
    );
    let debounce = lifecycle
        .split("pub(super) fn claim_debounced_restart(")
        .nth(1)
        .unwrap();
    assert!(
        debounce.find("gateway_start_preflight").unwrap()
            < debounce.find(".try_begin_restart(").unwrap()
    );
    let rejected = debounce
        .split("gateway_start_preflight")
        .nth(1)
        .unwrap()
        .split(".try_begin_restart(")
        .next()
        .unwrap();
    assert!(rejected.contains("self.publish_owned_restart_failure"));
    assert!(!rejected.contains("self.push_lifecycle"));

    assert!(start.contains("super::lan_gateway::check(&gen_out.config, &deps.platform)"));
    let generated = start
        .split("pub(super) async fn generate_and_gate_with_runtime_bindings")
        .nth(1)
        .unwrap();
    assert!(
        generated.find("super::lan_gateway::check").unwrap()
            < generated.find("write_private_runtime_config").unwrap()
    );
    assert!(hot.contains("super::lan_gateway::check(&outcome.config, &deps.platform)"));
    assert!(
        hot.find("super::lan_gateway::check").unwrap()
            < hot
                .find("let network_canary = outcome.network_canary")
                .unwrap()
    );
    assert!(start.contains("let fwd = false;"));
    assert!(!start.contains("let fwd = user_config.allow_lan"));
}

#[test]
fn malformed_user_config_is_not_a_forwarding_observation_on_any_platform() {
    for platform in ["linux", "darwin", "win32"] {
        for config in [
            Value::Null,
            json!({}),
            json!({"servers":[],"proxyModeType":12345}),
        ] {
            assert_eq!(
                check_user_config(&config, platform),
                Err(UserPreflightError::InvalidConfig)
            );
        }
        assert_eq!(
            check_user_config(&json!({"servers":[],"allowLan":true}), platform),
            Ok(())
        );
    }
}
