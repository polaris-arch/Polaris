use super::super::tailscale_store::*;
use serde_json::{json, Value};
fn node(tag: &str, terminal: bool) -> Value {
    json!({"tag":tag,"stateDirectory":format!("/data/{tag}"),"stateFile":format!("/data/{tag}/tailscaled.state"),"writerState":if terminal {"NoStoreConstruction"} else {"Unknown"},"stateFileState":"Missing","stateFileRevision":"","profileState":"None","profileFingerprint":""})
}
fn run(nonce: char, digest: char, tags: &[&str], terminal: bool) -> Value {
    json!({"runNonce":nonce.to_string().repeat(64),"configDigest":digest.to_string().repeat(64),"terminal":if terminal {"NoStoreConstruction"} else {"Unknown"},"censusComplete":terminal,"nodes":tags.iter().map(|tag|node(tag,terminal)).collect::<Vec<_>>()})
}
fn export(runs: Vec<Value>) -> String {
    json!({"contractVersion":"polaris-ts-auth-writer-retirement-v1","globalCleanupEvidence":"CleanupUnknown","instances":runs}).to_string()
}
fn store(terminal: bool) -> Value {
    let runs = vec![
        run('a', 'a', &["one"], terminal),
        run('b', 'b', &["two"], terminal),
    ];
    let observed=runs.iter().map(|r|json!({"runNonce":r["runNonce"],"configDigest":r["configDigest"],"scopes":r["nodes"].as_array().unwrap().iter().map(|n|json!({"tag":n["tag"],"stateDirectory":n["stateDirectory"],"stateFile":n["stateFile"]})).collect::<Vec<_>>()})).collect::<Vec<_>>();
    json!({"contractVersion":"polaris-android-ts-store-custody-v1","producerKind":"Main","processNonce":"process","nativeTicketId":"native-main","logicalInstanceId":"main-logical","mainBirthNonce":"birth","actualConfigDigest":"b".repeat(64),"originalObservedRuns":observed,"historyUnknown":false,"terminal":terminal,"storeRetirement":export(runs)})
}
fn target(store: Value) -> Value {
    json!({"processNonce":"process","revision":1,"actionRequestId":"action","stateFile":"/data/one/tailscaled.state","entries":[{"nativeTicketId":"native-main","producerKind":"Main","logicalInstanceId":"main-logical","globalState":"Unknown","related":true,"store":store}]})
}
fn validation() -> Value {
    let request = "native-check";
    let digest = "d".repeat(64);
    json!({"nativeTicketId":request,"producerKind":"CheckConfig","logicalInstanceId":"check-logical","globalState":"ValidationCleanupUnknown","related":false,"validation":{"nativeContractVersion":"polaris-validation-v1","validation":"Accepted","cleanup":"CleanupUnknown","requestID":request,"configDigest":digest,"membership":json!({"contractVersion":"polaris-ts-store-target-membership-v1","requestID":request,"configDigest":digest,"runNonce":"c".repeat(64),"membershipState":"Complete","targets":[]}).to_string(),"storeRetirement":export(vec![run('c','d',&[],false)])}})
}
#[test]
fn original_scope_roundtrip_and_terminal_do_not_rebind_history() {
    let original = decode_store(&store(false).to_string(), None).unwrap();
    assert_eq!(original.original().original_observed_runs.len(), 2);
    assert!(original.retired_export().is_err());
    let closed = decode_store(&store(true).to_string(), Some(&original)).unwrap();
    assert!(closed.retired_export().is_ok());
    assert!(decode_store(&closed.binding_json().unwrap(), Some(&original)).is_ok());
    for key in [
        "processNonce",
        "nativeTicketId",
        "logicalInstanceId",
        "mainBirthNonce",
        "actualConfigDigest",
    ] {
        let mut foreign = store(true);
        foreign[key] = json!("e".repeat(64));
        assert!(
            decode_store(&foreign.to_string(), Some(&original)).is_err(),
            "{key}"
        );
    }
}
#[test]
fn missing_extra_rebound_late_native_runs_and_schema_fail_closed() {
    for change in 0..7 {
        let mut native = store(true);
        match change {
            0 => native["historyUnknown"] = json!(true),
            1 => {
                native["originalObservedRuns"].as_array_mut().unwrap().pop();
            }
            2 => {
                let duplicate = native["originalObservedRuns"][0].clone();
                native["originalObservedRuns"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            3 => native["originalObservedRuns"][0]["configDigest"] = json!("e".repeat(64)),
            4 => {
                native["originalObservedRuns"][0]["scopes"][0]["stateFile"] =
                    json!("/other/tailscaled.state")
            }
            5 => native["storeRetirement"] = json!(export(vec![])),
            6 => native["extraProof"] = json!(true),
            _ => unreachable!(),
        }
        assert!(
            decode_store(&native.to_string(), None).is_err(),
            "change {change}"
        );
    }
}
#[test]
fn validation_complete_empty_excludes_only_same_original_check_ticket() {
    let original = decode_store(&store(false).to_string(), None).unwrap();
    let mut family = target(store(true));
    family["entries"].as_array_mut().unwrap().push(validation());
    assert!(decode_target(
        &family.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "action"
    )
    .is_ok());
    for mutation in 0..6 {
        let mut mutated = family.clone();
        let row = &mut mutated["entries"][1];
        match mutation {
            0 => row["nativeTicketId"] = json!("new-ticket-same-sha"),
            1 => row["producerKind"] = json!("Login"),
            2 => row["globalState"] = json!("BirthEntered"),
            3 => row["validation"]["storeRetirement"] = json!(export(vec![])),
            4 => {
                let mut member: Value =
                    serde_json::from_str(row["validation"]["membership"].as_str().unwrap())
                        .unwrap();
                member["membershipState"] = json!("Unknown");
                row["validation"]["membership"] = json!(member.to_string());
            }
            5 => row["related"] = json!(true),
            _ => unreachable!(),
        }
        assert!(
            decode_target(
                &mutated.to_string(),
                &original,
                "/data/one/tailscaled.state",
                "action"
            )
            .is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn complete_unrelated_runtime_census_does_not_require_its_writer_terminal() {
    let original = decode_store(&store(false).to_string(), None).unwrap();
    let mut unrelated = store(false);
    unrelated["nativeTicketId"] = json!("other-main");
    unrelated["logicalInstanceId"] = json!("other-logical");
    unrelated["mainBirthNonce"] = json!("other-birth");
    unrelated["terminal"] = json!(true);
    let mut native: Value =
        serde_json::from_str(unrelated["storeRetirement"].as_str().unwrap()).unwrap();
    for instance in native["instances"].as_array_mut().unwrap() {
        instance["censusComplete"] = json!(true);
        for node in instance["nodes"].as_array_mut().unwrap() {
            let old = node["tag"].as_str().unwrap().to_owned();
            *node = self::node(&format!("other-{old}"), false);
        }
    }
    for observed in unrelated["originalObservedRuns"].as_array_mut().unwrap() {
        for scope in observed["scopes"].as_array_mut().unwrap() {
            let tag = format!("other-{}", scope["tag"].as_str().unwrap());
            *scope = json!({"tag":tag,"stateDirectory":format!("/data/{tag}"),"stateFile":format!("/data/{tag}/tailscaled.state")});
        }
    }
    unrelated["storeRetirement"] = json!(native.to_string());
    let mut family = target(store(true));
    family["entries"].as_array_mut().unwrap().push(json!({"nativeTicketId":"other-main","producerKind":"Main","logicalInstanceId":"other-logical","globalState":"Unknown","related":false,"store":unrelated}));
    assert!(decode_target(
        &family.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "action"
    )
    .is_ok());
    family["entries"][1]["store"]["terminal"] = json!(false);
    assert!(decode_target(
        &family.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "action"
    )
    .is_err());
}
#[test]
fn target_family_rejects_missing_original_foreign_ticket_and_wrong_action() {
    let original = decode_store(&store(false).to_string(), None).unwrap();
    let family = target(store(true));
    assert!(decode_target(
        &family.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "wrong-action"
    )
    .is_err());
    for mutation in 0..7 {
        let mut f = family.clone();
        match mutation {
            0 => f["processNonce"] = json!("foreign"),
            1 => f["stateFile"] = json!("/data/other/tailscaled.state"),
            2 => f["revision"] = json!(0),
            3 => f["entries"] = json!([]),
            4 => f["entries"][0]["globalState"] = json!("invented-closed"),
            5 => f["entries"][0]["store"]["nativeTicketId"] = json!("foreign"),
            6 => {
                let row = f["entries"][0].clone();
                f["entries"].as_array_mut().unwrap().push(row);
            }
            _ => unreachable!(),
        }
        assert!(
            decode_target(
                &f.to_string(),
                &original,
                "/data/one/tailscaled.state",
                "action"
            )
            .is_err(),
            "{mutation}"
        );
    }
}
#[test]
fn completed_control_only_uses_same_parent_in_full_family() {
    let original = decode_store(&store(false).to_string(), None).unwrap();
    let mut f = target(store(true));
    f["entries"].as_array_mut().unwrap().push(json!({"nativeTicketId":"native-control","producerKind":"TargetlessReload","logicalInstanceId":"control-logical","globalState":"Completed","related":true,"parentNativeTicketId":"native-main"}));
    assert!(decode_target(
        &f.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "action"
    )
    .is_ok());
    f["entries"][1]["parentNativeTicketId"] = json!("other-main");
    assert!(decode_target(
        &f.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "action"
    )
    .is_err());
}
#[test]
fn courier_contract_contains_only_exact_typed_fields() {
    let payload = TailscaleStoreArgs {
        operation: "query".into(),
        binding: Some("original".into()),
        instance_id: None,
        expected_actual_config_digest: None,
        run_id: None,
        birth_nonce: None,
        state_file: Some("/data/one/tailscaled.state".into()),
        action_request_id: Some("action".into()),
    };
    let value = serde_json::to_value(payload).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 8);
    assert_eq!(value["actionRequestId"], "action");
    let response: ScopedStoreResponse =
        serde_json::from_value(json!({"envelope":"immutable"})).unwrap();
    assert_eq!(response.envelope, "immutable");
    assert!(serde_json::from_value::<ScopedStoreResponse>(
        json!({"envelope":"immutable","proof":true})
    )
    .is_err());
}
fn warm_validation() -> Value {
    let mut row = validation();
    let native = &mut row["validation"];
    let mut membership: Value =
        serde_json::from_str(native["membership"].as_str().unwrap()).unwrap();
    membership["targets"] = json!([{"tag":"one","stateDirectory":"/data/one","stateFile":"/data/one/tailscaled.state"}]);
    native["membership"] = json!(membership.to_string());
    native["storeRetirement"] = json!(export(vec![run('c', 'd', &["one"], true)]));
    json!({"contractVersion":"polaris-android-validation-custody-v1","processNonce":"process","nativeTicketId":"native-check","validation":native})
}
fn warm_tuple() -> AndroidWarmTuple {
    let custody = decode_validation(&warm_validation().to_string(), &"d".repeat(64)).unwrap();
    make_warm_tuple(
        &custody,
        "/data/one/tailscaled.state",
        "action",
        "original-config-epoch-7",
    )
    .unwrap()
}
fn warm_response() -> Value {
    json!({"tuple":serde_json::from_str::<Value>(&warm_tuple().binding_json().unwrap()).unwrap(),"nativeTicketId":"native-warm","globalState":"Reserved","held":true,"claimed":false,"childRegistrationOpen":false,"childNativeTicketId":"","childGlobalState":""})
}
fn warm_store() -> Value {
    let mut data = store(true);
    data["producerKind"] = json!("Login");
    data["nativeTicketId"] = json!("native-warm");
    data["logicalInstanceId"] = json!("original-config-epoch-7");
    data["mainBirthNonce"] = json!("");
    data["actualConfigDigest"] = json!("d".repeat(64));
    data["originalObservedRuns"] = json!([{"runNonce":"b".repeat(64),"configDigest":"d".repeat(64),"scopes":[{"tag":"one","stateDirectory":"/data/one","stateFile":"/data/one/tailscaled.state"}]}]);
    data["storeRetirement"] = json!(export(vec![run('b', 'd', &["one"], true)]));
    data
}
#[test]
fn single_check_response_carries_original_validation_without_second_invocation_or_authority() {
    let raw = warm_validation().to_string();
    let check = decode_validation_check(
        serde_json::from_value(json!({"tailscaleValidationEnvelope":raw})).unwrap(),
        &"d".repeat(64),
    )
    .unwrap();
    assert!(matches!(
        check.verdict(),
        super::ConfigCheckVerdict::Accepted
    ));
    let custody = check.custody().unwrap();
    let tuple = make_warm_tuple(
        custody,
        "/data/one/tailscaled.state",
        "action",
        "original-config-epoch-7",
    )
    .unwrap();
    assert_eq!(tuple.logical_instance_id(), "original-config-epoch-7");
    assert_eq!(tuple.state_file(), "/data/one/tailscaled.state");
    assert_eq!(tuple.action_request_id(), "action");
    assert_eq!(tuple.config_digest(), "d".repeat(64));
    let intent: Value = serde_json::from_str(&tuple.binding_json().unwrap()).unwrap();
    assert_eq!(intent.as_object().unwrap().len(), 5);
    assert!(!intent.to_string().contains("auth_key"));
    assert!(make_warm_tuple(custody, "/data/other/tailscaled.state", "action", "other").is_err());
    assert!(make_warm_tuple(custody, "/data/one/tailscaled.state", " ", "other").is_err());
    let legacy =
        decode_validation_check(serde_json::from_str("{}").unwrap(), &"d".repeat(64)).unwrap();
    assert!(matches!(
        legacy.verdict(),
        super::ConfigCheckVerdict::Accepted
    ));
    assert!(legacy.custody().is_none());
    let unknown = decode_validation_check(
        serde_json::from_value(
            json!({"tailscaleValidationEnvelope":warm_validation().to_string()}),
        )
        .unwrap(),
        &"e".repeat(64),
    )
    .unwrap();
    assert!(unknown.custody().is_none());
    assert!(decode_validation_check(serde_json::from_value(json!({"error":"capacity","errorCode":super::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED})).unwrap(), &"d".repeat(64)).is_err());
}
#[test]
fn actual_rejected_no_construction_can_only_exclude_its_original_check_ticket() {
    let mut rejected = warm_validation();
    rejected["validation"]["validation"] = json!("Rejected");
    rejected["validation"]["cleanup"] = json!("NoConstruction");
    let mut member: Value =
        serde_json::from_str(rejected["validation"]["membership"].as_str().unwrap()).unwrap();
    member["membershipState"] = json!("Unknown");
    member["targets"] = json!([]);
    rejected["validation"]["membership"] = json!(member.to_string());
    rejected["validation"]["storeRetirement"] = json!(export(vec![]));
    let rejected_custody = decode_validation(&rejected.to_string(), &"d".repeat(64)).unwrap();
    assert!(make_warm_tuple(
        &rejected_custody,
        "/data/one/tailscaled.state",
        "action",
        "new"
    )
    .is_err());
    let original = decode_store(&store(false).to_string(), None).unwrap();
    let mut family = target(store(true));
    let mut row = validation();
    row["validation"] = rejected["validation"].clone();
    family["entries"].as_array_mut().unwrap().push(row);
    assert!(decode_target(
        &family.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "action"
    )
    .is_ok());
    for mutation in 0..10 {
        let mut bad = rejected.clone();
        match mutation {
            0 => bad["validation"]["validation"] = json!("Accepted"),
            1 => bad["validation"]["validation"] = json!("InternalFailure"),
            2 => bad["validation"]["cleanup"] = json!("CleanupUnknown"),
            3 => bad["validation"]["requestID"] = json!("other"),
            4 => bad["validation"]["configDigest"] = json!("e".repeat(64)),
            5 => bad["validation"]["nativeContractVersion"] = json!("future"),
            6 => {
                let mut m = member.clone();
                m["membershipState"] = json!("Complete");
                bad["validation"]["membership"] = json!(m.to_string());
            }
            7 => {
                let mut m = member.clone();
                m["runNonce"] = json!("invalid");
                bad["validation"]["membership"] = json!(m.to_string());
            }
            8 => {
                let mut m = member.clone();
                m["targets"] = json!([{"tag":"one","stateDirectory":"/data/one","stateFile":"/data/one/tailscaled.state"}]);
                bad["validation"]["membership"] = json!(m.to_string());
            }
            9 => {
                bad["validation"].as_object_mut().unwrap().remove("cleanup");
            }
            _ => unreachable!(),
        }
        assert!(
            decode_validation(&bad.to_string(), &"d".repeat(64)).is_err(),
            "{mutation}"
        );
    }
    family["entries"][1]["nativeTicketId"] = json!("same-sha-other-ticket");
    assert!(decode_target(
        &family.to_string(),
        &original,
        "/data/one/tailscaled.state",
        "action"
    )
    .is_err());
}
#[test]
fn pending_or_maybe_born_warm_keeps_exact_management_without_minting_runtime_custody() {
    let tuple = warm_tuple();
    let reserved = decode_warm(&warm_response().to_string(), &tuple).unwrap();
    assert!(reserved.runtime_custody().is_none());
    assert!(reserved.held_retirement().is_err());
    let mut response = warm_response();
    response["claimed"] = json!(true);
    response["childRegistrationOpen"] = json!(true);
    assert!(decode_warm(&response.to_string(), &tuple)
        .unwrap()
        .held_retirement()
        .is_err());
    response["globalState"] = json!("Unknown");
    response["childRegistrationOpen"] = json!(false);
    response["childNativeTicketId"] = json!("original-child");
    response["childGlobalState"] = json!("CancelledBeforeBirth");
    let maybe = decode_warm(&response.to_string(), &tuple).unwrap();
    assert!(maybe.runtime_custody().is_none());
    assert!(maybe.held_retirement().is_err());
    for mutation in 0..6 {
        let mut bad = response.clone();
        match mutation {
            0 => bad["tuple"]["logicalInstanceId"] = json!("successor"),
            1 => bad["tuple"]["validation"]["nativeTicketId"] = json!("other-check"),
            2 => bad["tuple"]["actionRequestId"] = json!("other-action"),
            3 => bad["nativeTicketId"] = json!(""),
            4 => bad["claimed"] = json!(false),
            5 => bad["childGlobalState"] = json!(""),
            _ => unreachable!(),
        }
        assert!(decode_warm(&bad.to_string(), &tuple).is_err(), "{mutation}");
    }
}
#[test]
fn held_warm_needs_original_runtime_and_full_family_and_cannot_use_generic_finish() {
    let tuple = warm_tuple();
    let mut response = warm_response();
    let data = warm_store();
    response["claimed"] = json!(true);
    response["globalState"] = json!("Unknown");
    response["childNativeTicketId"] = json!("child");
    response["childGlobalState"] = json!("CancelledBeforeBirth");
    response["runtime"] = data.clone();
    let mut family = target(data.clone());
    family["entries"][0]["nativeTicketId"] = json!("native-warm");
    family["entries"][0]["producerKind"] = json!("Login");
    family["entries"][0]["logicalInstanceId"] = json!("original-config-epoch-7");
    response["family"] = family.clone();
    let held = decode_warm(&response.to_string(), &tuple).unwrap();
    assert!(held.runtime_custody().is_some());
    assert!(held.held_retirement().is_ok());
    for key in [
        "processNonce",
        "nativeTicketId",
        "logicalInstanceId",
        "actualConfigDigest",
    ] {
        let mut bad = response.clone();
        bad["runtime"][key] = json!("e".repeat(64));
        assert!(decode_warm(&bad.to_string(), &tuple).is_err(), "{key}");
    }
    for mutation in ["unclaimed", "registration-open", "no-child", "reserved"] {
        let mut bad = response.clone();
        match mutation {
            "unclaimed" => bad["claimed"] = json!(false),
            "registration-open" => bad["childRegistrationOpen"] = json!(true),
            "no-child" => {
                bad["childNativeTicketId"] = json!("");
                bad["childGlobalState"] = json!("");
            }
            "reserved" => bad["globalState"] = json!("Reserved"),
            _ => unreachable!(),
        }
        assert!(decode_warm(&bad.to_string(), &tuple).is_err(), "{mutation}");
    }
    let mut released = response.clone();
    released["held"] = json!(false);
    assert!(decode_warm(&released.to_string(), &tuple).is_err());
    let mut missing = response.clone();
    missing.as_object_mut().unwrap().remove("runtime");
    assert!(decode_warm(&missing.to_string(), &tuple).is_err());
    response.as_object_mut().unwrap().remove("family");
    assert!(decode_warm(&response.to_string(), &tuple)
        .unwrap()
        .held_retirement()
        .is_err());
    family["entries"].as_array_mut().unwrap().push(json!({"nativeTicketId":"unknown","producerKind":"Main","logicalInstanceId":"unobserved","globalState":"Unknown","related":false}));
    response["family"] = family;
    assert!(decode_warm(&response.to_string(), &tuple).is_err());
}
#[test]
fn warm_release_requires_exact_native_intent_cancellation_or_original_terminal_ticket() {
    let tuple = warm_tuple();
    let release = json!({"released":true,"beforeBirth":true,"processNonce":"process","nativeTicketId":"","actionRequestId":"action","stateFile":"/data/one/tailscaled.state"});
    assert!(decode_warm_release(&release.to_string(), &tuple).is_ok());
    let mut terminal = release.clone();
    terminal["beforeBirth"] = json!(false);
    terminal["nativeTicketId"] = json!("native-warm");
    assert!(decode_warm_release(&terminal.to_string(), &tuple).is_ok());
    for mutation in 0..8 {
        let mut bad = release.clone();
        match mutation {
            0 => bad["released"] = json!(false),
            1 => bad["processNonce"] = json!("foreign"),
            2 => bad["nativeTicketId"] = json!("unbound-ticket"),
            3 => bad["actionRequestId"] = json!("later-action"),
            4 => bad["stateFile"] = json!("/data/other/tailscaled.state"),
            5 => bad["beforeBirth"] = json!(false),
            6 => {
                bad.as_object_mut().unwrap().remove("beforeBirth");
            }
            7 => bad["globalNoOwner"] = json!(true),
            _ => unreachable!(),
        }
        assert!(
            decode_warm_release(&bad.to_string(), &tuple).is_err(),
            "{mutation}"
        );
    }
    assert!(decode_warm_release("{}", &tuple).is_err());
}
