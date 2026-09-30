use crate::runtime::proxy::android_drain::{verify, AndroidDrainReceipt, REQUIRED_PRODUCERS};
use serde_json::{json, Value};

fn receipt() -> Value {
    json!({"protocolVersion":1,"coveredProducers":REQUIRED_PRODUCERS,
        "coverageComplete":true,"processNonce":"process-1","fenceId":"fence-1",
        "markerProof":"Absent","sealedRevision":3,"revision":5,"capturedCount":1,
        "captured":[{"ticket":{"id":"ticket-1","kind":"Main",
            "logicalId":"run-1"},"state":"ClosedExact"}]})
}

fn check(value: Value) -> Result<(), &'static str> {
    let parsed: AndroidDrainReceipt = serde_json::from_value(value).unwrap();
    verify(&parsed, "process-1", "fence-1")
}

#[test]
fn exact_terminal_receipt_is_only_a_shape_proof() {
    assert!(check(receipt()).is_ok());
    assert!(verify(
        &serde_json::from_value(receipt()).unwrap(),
        "old-process",
        "fence-1"
    )
    .is_err());
    assert!(verify(
        &serde_json::from_value(receipt()).unwrap(),
        "process-1",
        "old-fence"
    )
    .is_err());
}

#[test]
fn rejects_missing_revisions_duplicate_owner_and_ticket() {
    let mut value = receipt();
    value.as_object_mut().unwrap().remove("sealedRevision");
    assert!(serde_json::from_value::<AndroidDrainReceipt>(value).is_err());
    let mut value = receipt();
    value["sealedRevision"] = json!(7);
    assert!(check(value).is_err());
    let mut value = receipt();
    value["markerProof"] = json!("PresentOrUnknown");
    assert!(check(value).is_err());
    let mut value = receipt();
    let duplicate = value["captured"][0].clone();
    value["captured"].as_array_mut().unwrap().push(duplicate);
    assert!(check(value).is_err());
    let mut value = receipt();
    let mut duplicate = value["captured"][0].clone();
    duplicate["ticket"]["id"] = json!("ticket-2");
    value["captured"].as_array_mut().unwrap().push(duplicate);
    assert!(check(value).is_err());
}

#[test]
fn partial_empty_or_forged_coverage_never_passes() {
    let mut value = receipt();
    value["coveredProducers"] = json!([]);
    value["coverageComplete"] = json!(false);
    value["captured"] = json!([]);
    value["capturedCount"] = json!(0);
    assert!(check(value).is_err());
    let mut value = receipt();
    value["coveredProducers"] = json!(["main.bridge"]);
    assert!(check(value).is_err());
    let mut value = receipt();
    value["protocolVersion"] = json!(2);
    assert!(check(value).is_err());
    let mut value = receipt();
    value["capturedCount"] = json!(0);
    assert!(check(value).is_err());
    let mut value = receipt();
    value["coveredProducers"]
        .as_array_mut()
        .unwrap()
        .push(json!("main.bridge"));
    assert!(check(value).is_err());
}

#[test]
fn unknown_unsettled_and_validation_cleanup_unknown_never_pass() {
    for state in [
        "Reserved",
        "BirthEntered",
        "Unknown",
        "ValidationCleanupUnknown",
        "Completed",
    ] {
        let mut value = receipt();
        value["captured"][0]["state"] = json!(state);
        assert!(check(value).is_err(), "state={state}");
    }
    let mut validation = receipt();
    validation["captured"][0]["ticket"]["kind"] = json!("CheckConfig");
    validation["captured"][0]["state"] = json!("ValidationCleanupUnknown");
    assert!(check(validation).is_err());
    let mut validation = receipt();
    validation["captured"][0]["ticket"]["kind"] = json!("CheckConfig");
    validation["captured"][0]["state"] = json!("Completed");
    assert!(check(validation).is_err());
}

#[test]
fn strict_ascii_id_domain_matches_kotlin_and_rejects_unicode_gaps() {
    let long_non_bmp = "😀".repeat(65);
    for invalid in [
        "",
        " ",
        " leading",
        "trailing ",
        "\u{001c}",
        "\u{0085}",
        "😀",
        long_non_bmp.as_str(),
    ] {
        let mut value = receipt();
        value["processNonce"] = json!(invalid);
        assert!(verify(&serde_json::from_value(value).unwrap(), invalid, "fence-1").is_err());
        let mut value = receipt();
        value["fenceId"] = json!(invalid);
        assert!(verify(
            &serde_json::from_value(value).unwrap(),
            "process-1",
            invalid
        )
        .is_err());
        let mut value = receipt();
        value["captured"][0]["ticket"]["id"] = json!(invalid);
        assert!(check(value).is_err());
        let mut value = receipt();
        let logical_invalid = if invalid == long_non_bmp {
            "😀".repeat(129)
        } else {
            invalid.to_owned()
        };
        value["captured"][0]["ticket"]["logicalId"] = json!(logical_invalid);
        assert!(check(value).is_err());
    }
    // Rust strings cannot contain an isolated UTF-16 surrogate. A hostile JSON
    // escape is rejected at deserialization, before the verifier sees an ID.
    assert!(serde_json::from_str::<String>(r#""\ud800""#).is_err());
}

#[test]
fn kind_and_terminal_matrix_never_promotes_validation_or_owner_as_control() {
    for (kind, allowed) in [
        ("Main", vec!["CancelledBeforeBirth", "ClosedExact"]),
        ("Login", vec!["CancelledBeforeBirth", "ClosedExact"]),
        ("Speedtest", vec!["CancelledBeforeBirth", "ClosedExact"]),
        ("CheckConfig", vec!["CancelledBeforeBirth"]),
        ("TargetlessStop", vec!["CancelledBeforeBirth", "Completed"]),
        (
            "TargetlessReload",
            vec!["CancelledBeforeBirth", "Completed"],
        ),
    ] {
        for state in [
            "CancelledBeforeBirth",
            "ClosedExact",
            "Completed",
            "Unknown",
            "ValidationCleanupUnknown",
        ] {
            let mut value = receipt();
            value["captured"][0]["ticket"]["kind"] = json!(kind);
            value["captured"][0]["state"] = json!(state);
            assert_eq!(
                check(value).is_ok(),
                allowed.contains(&state),
                "{kind}→{state}"
            );
        }
    }
}
