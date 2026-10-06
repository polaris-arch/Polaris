use super::*;
use crate::tailscale_state::{cached_session_exists, cached_session_exists_for_presentation};

fn retired_fixture() -> AuthStateProjection {
    project_tailscale_auth_state(
        &sealed(),
        AuthProjectionProvenance::BoundFingerprint(
            expected()["profileFingerprint"].as_str().unwrap(),
        ),
    )
    .unwrap()
}

#[test]
fn actual_projection_reopens_absent_with_strict_unknown_and_pc_repeat_preserves_exact_bytes() {
    let retired = retired_fixture();
    assert!(retired.changed);
    assert_eq!(
        cached_session_exists(&retired.bytes),
        Err(UnknownTailscaleSession)
    );
    assert_eq!(
        cached_session_exists_for_presentation(&retired.bytes),
        Ok(false)
    );
    // Preserve even non-normalized original bytes on an admitted no-write repeat.
    let mut bytes = b" \n".to_vec();
    bytes.extend_from_slice(&retired.bytes);
    bytes.extend_from_slice(b"\n ");
    let repeated =
        project_tailscale_auth_state(&bytes, AuthProjectionProvenance::CurrentFileReferences)
            .unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.bytes, bytes);
    assert_eq!(
        project_tailscale_auth_state(
            &bytes,
            AuthProjectionProvenance::BoundFingerprint(
                expected()["profileFingerprint"].as_str().unwrap(),
            ),
        )
        .unwrap_err(),
        UnknownTailscaleSession
    );
    let mut last = fixture();
    let mut profiles = profile_map(&last);
    profiles.as_object_mut().unwrap().remove("b456");
    set_profiles(&mut last, &profiles);
    let retired_last = project(&last).unwrap();
    assert_eq!(
        cached_session_exists_for_presentation(&retired_last.bytes),
        Ok(false)
    );
    let repeated_last = project_tailscale_auth_state(
        &retired_last.bytes,
        AuthProjectionProvenance::CurrentFileReferences,
    )
    .unwrap();
    assert!(!repeated_last.changed);
    assert_eq!(repeated_last.bytes, retired_last.bytes);
}

#[test]
fn incomplete_ambiguous_legacy_or_corrupt_retired_pairs_remain_unknown_for_both_consumers() {
    let original = parse_value(&retired_fixture().bytes).unwrap();
    let mut invalid = Vec::new();
    for key in ["_profiles", "_current-profile"] {
        let mut document = original.clone();
        document.as_object_mut().unwrap().remove(key);
        // Missing current delegates to the old display getter; it still cannot authorize retirement.
        if key == "_profiles" {
            invalid.push(document);
        } else {
            assert_eq!(
                cached_session_exists_for_presentation(&serde_json::to_vec(&document).unwrap()),
                cached_session_exists(&serde_json::to_vec(&document).unwrap())
            );
            assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
        }
    }
    for pointer in [
        "",
        "profile-A123",
        "profile-ab12-extra",
        "ipn-go-bridge",
        "user-legacy",
    ] {
        let mut document = original.clone();
        document["_current-profile"] = json!(base64_encode(pointer.as_bytes()));
        if pointer.is_empty() {
            assert_eq!(
                cached_session_exists_for_presentation(&serde_json::to_vec(&document).unwrap()),
                Ok(false)
            );
            assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
        } else {
            invalid.push(document);
        }
    }
    for profiles in [
        json!(null),
        json!([]),
        json!({"b456":{"ID":"a123","Key":"profile-b456"}}),
        json!({"b456":{"ID":"b456","Key":"profile-a123"}}),
        json!({"B456":{"ID":"B456","Key":"profile-B456"}}),
        json!({"b456":{"ID":"b456","Key":false}}),
    ] {
        let mut document = original.clone();
        set_profiles(&mut document, &profiles);
        invalid.push(document);
    }
    let mut leftover = original.clone();
    leftover["profile-a123"] = json!(base64_encode(b"{}"));
    invalid.push(leftover);
    for value in [json!(false), json!("YR=="), json!("YQ"), json!("____")] {
        let mut document = original.clone();
        document["unknown-user-value"] = value;
        invalid.push(document);
    }
    for document in invalid {
        let bytes = serde_json::to_vec(&document).unwrap();
        assert_eq!(
            cached_session_exists_for_presentation(&bytes),
            Err(UnknownTailscaleSession)
        );
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
    for profiles in [
        br#"{"b456":{"ID":"b456","Key":"profile-b456"},"b456":{"ID":"b456","Key":"profile-b456"}}"#
            .as_slice(),
        br#"{"b456":{"ID":"b456","ID":"a123","Key":"profile-b456"}}"#,
    ] {
        let mut document = original.clone();
        document["_profiles"] = json!(base64_encode(profiles));
        let bytes = serde_json::to_vec(&document).unwrap();
        assert_eq!(
            cached_session_exists_for_presentation(&bytes),
            Err(UnknownTailscaleSession)
        );
        assert_eq!(project(&document).unwrap_err(), UnknownTailscaleSession);
    }
    let duplicate = format!(
        "{{\"_machinekey\":\"YQ==\",{}",
        serde_json::to_string(&original)
            .unwrap()
            .trim_start_matches('{')
    );
    assert_eq!(
        cached_session_exists_for_presentation(duplicate.as_bytes()),
        Err(UnknownTailscaleSession)
    );
    assert_eq!(
        project_tailscale_auth_state(
            duplicate.as_bytes(),
            AuthProjectionProvenance::CurrentFileReferences
        )
        .unwrap_err(),
        UnknownTailscaleSession
    );
}

#[test]
fn presentation_delegates_other_states_without_turning_absence_into_retirement_permission() {
    let mut active = fixture();
    active["profile-a123"] = json!(base64_encode(
        br#"{"Config":{"NodeID":"synthetic-node","UserProfile":{"LoginName":"synthetic-user"}}}"#
    ));
    let bytes = serde_json::to_vec(&active).unwrap();
    assert_eq!(cached_session_exists(&bytes), Ok(true));
    assert_eq!(cached_session_exists_for_presentation(&bytes), Ok(true));
    for bytes in [b"{}".as_slice(), &sealed(), br#"{"ipn-go-bridge":"e30="}"#] {
        assert_eq!(
            cached_session_exists_for_presentation(bytes),
            cached_session_exists(bytes)
        );
    }
    assert_eq!(cached_session_exists_for_presentation(b"{}"), Ok(false));
    assert_eq!(
        project_tailscale_auth_state(b"{}", AuthProjectionProvenance::CurrentFileReferences)
            .unwrap_err(),
        UnknownTailscaleSession
    );
}
