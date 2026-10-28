use super::*;
use serde_json::json;

mod compatibility;

// Exact shared G/D source-contract fixtures. All contents are explicitly synthetic.
fn sealed() -> Vec<u8> {
    polaris_source_probe::crate_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "src/tailscale_state/auth_projection/tests/synthetic-sealed-state.json",
    )
}

fn expected_bytes() -> Vec<u8> {
    polaris_source_probe::crate_bytes_in(
        env!("CARGO_MANIFEST_DIR"),
        "src/tailscale_state/auth_projection/tests/synthetic-sealed-state.expected.json",
    )
}

fn fixture() -> Value {
    parse_value(&sealed()).unwrap()
}

fn expected() -> Value {
    parse_value(&expected_bytes()).unwrap()
}

fn profile_map(document: &Value) -> Value {
    parse_value(&decode_value(&document["_profiles"]).unwrap()).unwrap()
}

fn set_profiles(document: &mut Value, profiles: &Value) {
    document["_profiles"] = json!(base64_encode(&serde_json::to_vec(profiles).unwrap()));
}

fn project(document: &Value) -> Result<AuthStateProjection, UnknownTailscaleSession> {
    project_tailscale_auth_state(
        &serde_json::to_vec(document).unwrap(),
        AuthProjectionProvenance::CurrentFileReferences,
    )
}

#[test]
fn actual_shared_sealed_fixture_binds_original_bytes_and_profile_fingerprint() {
    let expected = expected();
    assert_eq!(
        Sha256::digest(sealed())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        expected["stateFileRevision"].as_str().unwrap()
    );
    let bound = project_tailscale_auth_state(
        &sealed(),
        AuthProjectionProvenance::BoundFingerprint(
            expected["profileFingerprint"].as_str().unwrap(),
        ),
    )
    .unwrap();
    let pc =
        project_tailscale_auth_state(&sealed(), AuthProjectionProvenance::CurrentFileReferences)
            .unwrap();
    assert!(bound.changed && pc.changed);
    assert_eq!(bound.bytes, pc.bytes);
    let original = fixture();
    let projected = parse_value(&bound.bytes).unwrap();
    assert_eq!(
        projected.as_object().unwrap().len(),
        original.as_object().unwrap().len() - 1
    );
    assert!(!projected
        .as_object()
        .unwrap()
        .contains_key(expected["profileKey"].as_str().unwrap()));
    assert_eq!(projected["_current-profile"], original["_current-profile"]);
    assert_eq!(
        decode_value(&projected["_current-profile"]).unwrap(),
        expected["currentPointerPreserved"]
            .as_str()
            .unwrap()
            .as_bytes()
    );
    for key in expected["preserveKeys"].as_array().unwrap() {
        let key = key.as_str().unwrap();
        if key != "_profiles" {
            assert_eq!(
                projected[key], original[key],
                "original outer value changed"
            );
        }
    }
    let mut profiles = profile_map(&original);
    profiles
        .as_object_mut()
        .unwrap()
        .remove(expected["profileId"].as_str().unwrap());
    assert_eq!(profile_map(&projected), profiles);
    assert_eq!(
        decode_value(&projected["unknown-user-value"]).unwrap(),
        [0, 255, 7]
    );
    // Presentation remains Unknown for the deliberately retired nonempty pointer.
    assert_eq!(
        super::super::cached_session_exists(&bound.bytes),
        Err(UnknownTailscaleSession)
    );
    let repeated = project_tailscale_auth_state(
        &bound.bytes,
        AuthProjectionProvenance::CurrentFileReferences,
    )
    .unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.bytes, bound.bytes);
}

#[test]
fn wrong_bound_fingerprint_or_profile_reference_never_produces_candidate_bytes() {
    for fingerprint in [
        "",
        "not-a-fingerprint",
        "BF1FF23B84DEAE9D2DC78F5241586709DBFF294A1F8A8C76D61CCAC3BF76224D",
        &"0".repeat(64),
    ] {
        assert_eq!(
            project_tailscale_auth_state(
                &sealed(),
                AuthProjectionProvenance::BoundFingerprint(fingerprint)
            )
            .unwrap_err(),
            UnknownTailscaleSession
        );
    }
    let mut different = fixture();
    different["_current-profile"] = json!(base64_encode(b"profile-b456"));
    assert_eq!(
        project_tailscale_auth_state(
            &serde_json::to_vec(&different).unwrap(),
            AuthProjectionProvenance::BoundFingerprint(
                expected()["profileFingerprint"].as_str().unwrap()
            )
        )
        .unwrap_err(),
        UnknownTailscaleSession
    );
    assert!(project(&different).unwrap().changed);
}

#[test]
fn unknown_or_ambiguous_modern_reference_and_metadata_aliases_are_rejected() {
    let original = fixture();
    for replacement in [
        json!({"c789":{"ID":"c789","Key":"profile-a123"}}),
        json!({"a123":{"ID":"b456","Key":"profile-a123"}}),
        json!({"a123":{"ID":"a123","Key":"_machinekey"}}),
        json!({"a123":{"ID":"a123","Key":"ipn-go-bridge"}}),
        json!({"a123":{"ID":"a123","Key":false}}),
        json!({"a123":false}),
        json!({"../../node":{"ID":"../../node","Key":"profile-../../node"}}),
        json!({"A123":{"ID":"A123","Key":"profile-A123"}}),
    ] {
        let mut document = original.clone();
        let mut profiles = profile_map(&document);
        profiles
            .as_object_mut()
            .unwrap()
            .extend(replacement.as_object().unwrap().clone());
        set_profiles(&mut document, &profiles);
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
}

#[test]
fn no_active_dangling_or_legacy_selection_stays_unknown_without_projection() {
    for document in [
        json!({}),
        json!({"_machinekey":"YQ=="}),
        json!({"ipn-go-bridge":"e30="}),
        json!({"_daemon":"e30="}),
    ] {
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
    for pointer in ["", "ipn-go-bridge", "_machinekey", "user-legacy"] {
        let mut document = fixture();
        document["_current-profile"] = json!(base64_encode(pointer.as_bytes()));
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
    // A complete modern retired pair is a PC-only unchanged candidate, not a Bound profile.
    let mut retired = fixture();
    retired["_current-profile"] = json!(base64_encode(b"profile-c789"));
    let original_bytes = serde_json::to_vec(&retired).unwrap();
    let repeated = project(&retired).unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.bytes, original_bytes);
    for key in ["_current-profile", "_profiles", "profile-a123"] {
        let mut document = fixture();
        document.as_object_mut().unwrap().remove(key);
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
    // A display-only absent-account result provides no operation authority.
    assert_eq!(super::super::cached_session_exists(b"{}"), Ok(false));
    assert_eq!(
        project_tailscale_auth_state(b"{}", AuthProjectionProvenance::CurrentFileReferences)
            .unwrap_err(),
        UnknownTailscaleSession
    );
}

#[test]
fn malformed_file_base64_prefs_and_profile_shapes_fail_closed_without_secret_error_text() {
    for bytes in [
        b"".as_slice(),
        b"not-json",
        b"null",
        b"[]",
        br#"{"x":"e30="} trailing"#,
    ] {
        assert_eq!(
            project_tailscale_auth_state(bytes, AuthProjectionProvenance::CurrentFileReferences)
                .unwrap_err(),
            UnknownTailscaleSession
        );
    }
    for value in [
        json!(false),
        json!(null),
        json!("YQ"),
        json!("YR=="),
        json!("Y=Q="),
        json!("____"),
        json!("YWJ="),
    ] {
        let mut document = fixture();
        document["unknown-user-value"] = value;
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
    for key in ["_profiles", "profile-a123"] {
        for content in [b"not-json".as_slice(), b"null", b"[]", b"true"] {
            let mut document = fixture();
            document[key] = json!(base64_encode(content));
            let error = project(&document).unwrap_err();
            assert_eq!(error.to_string(), "cannot verify cached Tailscale session");
            assert_eq!(format!("{error:?}"), "UnknownTailscaleSession");
        }
    }
    let mut document = fixture();
    document["_current-profile"] = json!(base64_encode(&[0xff]));
    assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    assert_eq!(
        project_tailscale_auth_state(
            &vec![b' '; 4 * 1024 * 1024 + 1],
            AuthProjectionProvenance::CurrentFileReferences
        )
        .unwrap_err(),
        UnknownTailscaleSession
    );
}

#[test]
fn duplicate_outer_profiles_identity_and_nested_unknown_members_never_silently_overwrite() {
    let bytes = format!(
        "{{\"_machinekey\":\"YQ==\",{}",
        std::str::from_utf8(&sealed())
            .unwrap()
            .trim_start_matches('{')
    );
    assert_eq!(
        project_tailscale_auth_state(
            bytes.as_bytes(),
            AuthProjectionProvenance::CurrentFileReferences
        )
        .unwrap_err(),
        UnknownTailscaleSession
    );
    for profiles in [
        br#"{"a123":{"ID":"a123","Key":"profile-a123"},"a123":{"ID":"a123","Key":"profile-a123"}}"#
            .as_slice(),
        br#"{"a123":{"ID":"a123","ID":"b456","Key":"profile-a123"}}"#,
        br#"{"a123":{"ID":"a123","Key":"profile-a123","Unknown":{"kept":1,"kept":2}}}"#,
    ] {
        let mut document = fixture();
        document["_profiles"] = json!(base64_encode(profiles));
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
    let mut document = fixture();
    document["profile-a123"] = json!(base64_encode(
        br#"{"Config":null,"Config":{"secret":"synthetic-only"}}"#
    ));
    assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
}

#[test]
fn removing_last_modern_profile_keeps_nonempty_pointer_legacy_and_all_unrelated_values() {
    let mut document = fixture();
    let mut profiles = profile_map(&document);
    profiles.as_object_mut().unwrap().remove("b456");
    set_profiles(&mut document, &profiles);
    let projected = project(&document).unwrap();
    let after = parse_value(&projected.bytes).unwrap();
    assert_eq!(profile_map(&after), json!({}));
    for (key, value) in document.as_object().unwrap() {
        if key != "_profiles" && key != "profile-a123" {
            assert_eq!(&after[key], value);
        }
    }
    assert_eq!(after["ipn-go-bridge"], document["ipn-go-bridge"]);
    assert_eq!(after["profile-b456"], document["profile-b456"]);
    assert!(!decode_value(&after["_current-profile"]).unwrap().is_empty());
}

#[test]
fn public_projection_debug_never_formats_file_credentials() {
    let projected =
        project_tailscale_auth_state(&sealed(), AuthProjectionProvenance::CurrentFileReferences)
            .unwrap();
    let display = format!("{projected:?}");
    assert!(!display.contains("profile-a123") && !display.contains("synthetic"));
    assert!(display.contains("changed: true"));
}
