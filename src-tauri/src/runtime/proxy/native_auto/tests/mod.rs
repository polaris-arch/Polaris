use super::*;
use polaris_config_engine::user_config::{
    app_config::SubscriptionInterfacePolicy, server_config::ServerConfig,
};
use polaris_singbox_grpc::daemon::GroupItem;

fn fixture() -> (
    UserConfig,
    Vec<Group>,
    BTreeMap<String, String>,
    BTreeSet<String>,
) {
    let mut config = UserConfig::default();
    config.auto_select_enabled = true;
    config.selection_intent = Some(model::SelectionIntent {
        mode: "auto".into(),
        target: AutoSelectTarget::Global,
    });
    config.servers = [("a", "one"), ("b", "two")]
        .into_iter()
        .map(|(id, sub)| ServerConfig {
            id: id.into(),
            name: id.into(),
            subscription_id: Some(sub.into()),
            ..ServerConfig::default()
        })
        .collect();
    config.subscriptions = ["one", "two"]
        .into_iter()
        .map(|id| SubscriptionInterfacePolicy {
            id: id.into(),
            ..Default::default()
        })
        .collect();
    config.app_rules=serde_json::from_value(json!([{"appId":"app","enabled":true,"action":"proxy","targetAutoSelect":{"scope":"subscription","subscriptionId":"two"}}])).unwrap();
    let sub = AutoSelectTarget::Subscription {
        subscription_id: "two".into(),
    };
    let group = |tag: String, selected: String, members: Vec<(&str, i64)>| Group {
        tag,
        selected,
        items: members
            .into_iter()
            .map(|(tag, time)| GroupItem {
                tag: tag.into(),
                url_test_time: time,
                url_test_delay: 25,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let global = model::wrapper_tag(&AutoSelectTarget::Global);
    let gt = model::urltest_tag(&AutoSelectTarget::Global);
    let sw = model::wrapper_tag(&sub);
    let st = model::urltest_tag(&sub);
    let groups = vec![
        group(
            "proxy-selector".into(),
            global.clone(),
            vec![(&global, 0), ("A", 0), ("B", 0)],
        ),
        group(global, gt.clone(), vec![(&gt, 0), ("A", 0), ("B", 0)]),
        group(gt, "A".into(), vec![("A", 1000), ("B", 1000)]),
        group(sw, st.clone(), vec![(&st, 0), ("B", 0)]),
        group(st, "B".into(), vec![("B", 1000)]),
    ];
    (
        config,
        groups,
        BTreeMap::from([("a".into(), "A".into()), ("b".into(), "B".into())]),
        BTreeSet::from(["a".into(), "b".into()]),
    )
}

#[test]
fn tcp_truth_is_recursive_and_never_guesses_missing_or_cyclic_choices() {
    let (_, mut groups, nodes, _) = fixture();
    assert_eq!(
        tcp_leaf("proxy-selector", &groups, &nodes).as_deref(),
        Some("a")
    );
    groups[0].selected = "invented".into();
    assert_eq!(tcp_leaf("proxy-selector", &groups, &nodes), None);
    groups[0].selected = "proxy-selector".into();
    groups[0].items.push(GroupItem {
        tag: "proxy-selector".into(),
        ..Default::default()
    });
    assert_eq!(tcp_leaf("proxy-selector", &groups, &nodes), None);
    assert_eq!(tcp_leaf("missing", &groups, &nodes), None);
}

#[test]
fn independent_scopes_have_distinct_winners_and_original_measurement_time() {
    let (config, groups, nodes, clean) = fixture();
    let status = project(&config, 7, Some(&groups), &nodes, &clean, "url", 1_010_000);
    assert_eq!(status.actual_server_id.as_deref(), Some("a"));
    assert_eq!(status.native_auto_groups.len(), 2);
    assert!(status
        .native_auto_groups
        .iter()
        .all(|group| group.confirmed));
    assert_eq!(
        status.native_auto_groups[0].actual_server_id.as_deref(),
        Some("a")
    );
    assert_eq!(
        status.native_auto_groups[1].actual_server_id.as_deref(),
        Some("b")
    );
    assert!(status
        .native_measurements
        .iter()
        .all(|sample| sample.measured_at == 1_000_000));
    let later = project(&config, 7, Some(&groups), &nodes, &clean, "url", 1_020_000);
    assert_eq!(
        status.native_measurements[0].measured_at,
        later.native_measurements[0].measured_at
    );
}

#[test]
fn initial_fallback_and_expired_or_dirty_samples_are_not_verified_winners() {
    let (config, mut groups, nodes, clean) = fixture();
    let expired = project(&config, 7, Some(&groups), &nodes, &clean, "url", 3_000_001);
    assert!(expired
        .native_auto_groups
        .iter()
        .all(|group| !group.confirmed));
    for item in groups.iter_mut().flat_map(|group| &mut group.items) {
        item.url_test_time = 0;
    }
    let pending = project(&config, 7, Some(&groups), &nodes, &clean, "url", 1_010_000);
    assert_eq!(pending.actual_server_id.as_deref(), Some("a"));
    assert_eq!(
        pending.native_auto_groups[0].actual_server_id.as_deref(),
        Some("a")
    );
    assert_eq!(pending.native_auto_groups[0].state, "pending");
    assert!(!pending.native_auto_groups[0].confirmed);
    assert!(pending.native_measurements.is_empty());
    let dirty = project(
        &config,
        7,
        Some(&groups),
        &nodes,
        &BTreeSet::new(),
        "url",
        1_010_000,
    );
    assert_eq!(dirty.actual_server_id.as_deref(), Some("a"));
    assert!(dirty
        .native_auto_groups
        .iter()
        .all(|group| !group.confirmed));
}

#[test]
fn freeze_requires_all_groups_and_attests_all_writes() {
    let (config, groups, nodes, clean) = fixture();
    let mut status = project(&config, 7, Some(&groups), &nodes, &clean, "url", 1_010_000);
    let plan = freeze_plan(&config, &status, &groups, &nodes).unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].0.server_id, "a");
    assert_eq!(plan[1].0.server_id, "b");
    status.native_auto_groups[1].confirmed = false;
    assert_eq!(
        freeze_plan(&config, &status, &groups, &nodes).unwrap_err(),
        "AUTO_SELECT_NO_ACTUAL_EXIT"
    );
    let writes: Vec<_> = plan
        .iter()
        .map(|(_, wrapper, node)| (wrapper.clone(), node.clone()))
        .collect();
    assert!(!updates_attested(&groups, &writes));
    let mut applied = groups.clone();
    for group in &mut applied {
        if let Some((_, node)) = writes.iter().find(|(tag, _)| tag == &group.tag) {
            group.selected = node.clone();
        }
    }
    assert!(updates_attested(&applied, &writes));
    applied
        .iter_mut()
        .find(|group| group.tag == writes[1].0)
        .unwrap()
        .selected = "unknown".into();
    assert!(!updates_attested(&applied, &writes));
}

#[test]
fn enabling_feature_without_references_does_not_steal_app_http_work() {
    let (mut config, _, _, _) = fixture();
    config.selection_intent = None;
    config.app_rules.clear();
    assert!(native_owned_members(&serde_json::to_value(&config).unwrap()).is_empty());
    config.selection_intent = Some(model::SelectionIntent {
        mode: "auto".into(),
        target: AutoSelectTarget::Subscription {
            subscription_id: "two".into(),
        },
    });
    assert_eq!(
        native_owned_members(&serde_json::to_value(&config).unwrap()),
        BTreeSet::from(["b".into()])
    );
    config.auto_select_enabled = false;
    assert!(native_owned_members(&serde_json::to_value(&config).unwrap()).is_empty());
}
#[test]
fn edited_or_reassigned_nodes_retire_old_verified_pins() {
    let (mut config, _, _, _) = fixture();
    config.auto_select_pins = vec![AutoSelectPin {
        target: AutoSelectTarget::Subscription {
            subscription_id: "one".into(),
        },
        server_id: "a".into(),
    }];
    let previous = serde_json::to_value(config).unwrap();
    let mut unchanged = previous.clone();
    invalidate_changed_pins(&previous, &mut unchanged);
    assert_eq!(unchanged["autoSelectPins"].as_array().unwrap().len(), 1);
    let mut edited = previous.clone();
    edited["servers"][0]["address"] = json!("different.example");
    invalidate_changed_pins(&previous, &mut edited);
    assert_eq!(edited["autoSelectPins"], json!([]));
    let mut reassigned = previous.clone();
    reassigned["servers"][0]["subscriptionId"] = json!("two");
    invalidate_changed_pins(&previous, &mut reassigned);
    assert_eq!(reassigned["autoSelectPins"], json!([]));
}

#[test]
fn direct_and_block_identity_require_runtime_evidence_never_desired_fallback() {
    let (_, mut groups, nodes, _) = fixture();
    let proxy = groups
        .iter_mut()
        .find(|group| group.tag == "proxy-selector")
        .unwrap();
    proxy.items.push(GroupItem {
        tag: "direct".into(),
        ..Default::default()
    });
    proxy.selected = "direct".into();
    assert_eq!(
        runtime_exit_id(None, None, &groups, &nodes).as_deref(),
        Some("__direct__")
    );
    assert_eq!(runtime_exit_id(None, None, &[], &nodes), None);
    let blocked = json!({"selectedServerId":"__block__","proxyMode":"smart"});
    let manual = json!({"selectedServerId":"a","proxyMode":"smart"});
    assert_eq!(
        runtime_exit_id(Some(&blocked), Some(&blocked), &groups, &nodes).as_deref(),
        Some("__block__")
    );
    assert_eq!(
        runtime_exit_id(Some(&manual), Some(&blocked), &[], &nodes),
        None
    );
    assert_eq!(
        runtime_exit_id(Some(&blocked), Some(&manual), &[], &nodes),
        None
    );
    let direct_mode = json!({"selectedServerId":"__block__","proxyMode":"direct"});
    assert_eq!(
        runtime_exit_id(Some(&direct_mode), Some(&blocked), &groups, &nodes).as_deref(),
        Some("__direct__")
    );
}
