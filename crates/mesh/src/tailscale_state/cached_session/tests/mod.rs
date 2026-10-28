use super::*;
use serde_json::json;

// Synthetic FileStore values only; no private keys or real account data.
const ACCOUNT: &str = "eyJXYW50UnVubmluZyI6ZmFsc2UsIkxvZ2dlZE91dCI6ZmFsc2UsIkNvbmZpZyI6eyJOb2RlSUQiOiJuLWZpeHR1cmUiLCJVc2VyUHJvZmlsZSI6eyJMb2dpbk5hbWUiOiJmaXh0dXJlQGV4YW1wbGUuaW52YWxpZCJ9fX0=";
const PROFILES: &str = "eyJhYjEyIjp7IklEIjoiYWIxMiIsIktleSI6InByb2ZpbGUtYWIxMiJ9fQ==";

fn current_store(prefs: &str) -> Value {
    json!({"_current-profile":"cHJvZmlsZS1hYjEy", "_profiles":PROFILES, "profile-ab12":prefs})
}

fn parse(document: Value) -> Result<bool, UnknownTailscaleSession> {
    cached_session_exists(&serde_json::to_vec(&document).unwrap())
}

#[test]
fn empty_store_machine_key_and_taildrop_metadata_are_not_cached_accounts() {
    for document in [
        json!({}),
        json!({"_machinekey":"bWFjaGluZS1maXh0dXJl"}),
        json!({"_machinekey":"bWFjaGluZS1maXh0dXJl", "_taildrop-received":"MQ==", "_profiles":"e30="}),
    ] {
        assert_eq!(
            cached_session_exists(&serde_json::to_vec(&document).unwrap()),
            Ok(false)
        );
    }
}

#[test]
fn only_the_current_saved_profile_is_a_cached_session_even_when_runtime_is_stopped() {
    assert_eq!(parse(current_store(ACCOUNT)), Ok(true));
    let mut cleared = current_store(ACCOUNT);
    cleared["_current-profile"] = json!("");
    assert_eq!(parse(cleared), Ok(false));
    let mut unselected = current_store(ACCOUNT);
    unselected
        .as_object_mut()
        .unwrap()
        .remove("_current-profile");
    assert_eq!(parse(unselected), Ok(false));
}

#[test]
fn current_logout_or_null_persist_is_not_shadowed_by_an_old_account() {
    for prefs in ["eyJDb25maWciOm51bGx9", "eyJXYW50UnVubmluZyI6ZmFsc2UsIkxvZ2dlZE91dCI6dHJ1ZSwiQ29uZmlnIjp7Ik5vZGVJRCI6Im4tZml4dHVyZSIsIlVzZXJQcm9maWxlIjp7IkxvZ2luTmFtZSI6ImZpeHR1cmVAZXhhbXBsZS5pbnZhbGlkIn19fQ=="] {
        let mut document = current_store(prefs);
        document["profile-dead"] = json!(ACCOUNT);
        assert_eq!(parse(document), Ok(false));
    }
}

#[test]
fn windows_server_mode_per_user_or_legacy_records_do_not_replace_tsnet_current_selection() {
    for key in ["_daemon", "ipn-android", "ipn-go-bridge", "user-fixture"] {
        assert_eq!(parse(json!({key:ACCOUNT})), Err(UnknownTailscaleSession));
    }
    let mut document = current_store("eyJDb25maWciOm51bGx9");
    document["server-mode-start-key"] = json!("cHJvZmlsZS1kZWFk");
    document["_current/fixture-user"] = json!("cHJvZmlsZS1kZWFk");
    document["profile-dead"] = json!(ACCOUNT);
    assert_eq!(parse(document), Ok(false));
}

#[test]
fn malformed_unknown_or_dangling_profile_state_is_an_error_never_absence() {
    for bytes in [
        b"".as_slice(),
        b"not json",
        b"[]",
        b"null",
        b"{",
        br#"{"profile-ab12":false}"#,
        br#"{"profile-ab12":"e30="}"#,
        br#"{"profile-ab12":"YmFk"}"#,
        br#"{"profile-ab12":"eyJDb25maWciOnsiTm9kZUlEIjoyfX0="}"#,
        br#"{"_current-profile":"cHJvZmlsZS1hYjEy"}"#,
        br#"{"future-auth-format":"e30="}"#,
    ] {
        assert_eq!(cached_session_exists(bytes), Err(UnknownTailscaleSession));
    }
    for prefs in ["e30=", "YmFk", "eyJDb25maWciOnsiTm9kZUlEIjoyfX0="] {
        assert_eq!(parse(current_store(prefs)), Err(UnknownTailscaleSession));
    }
    for metadata in [
        "eyJhYjEyIjp7IktleSI6InByb2ZpbGUtYWIxMiJ9fQ==",
        "eyJhYjEyIjp7IklEIjoiIiwiS2V5IjoicHJvZmlsZS1hYjEyIn19",
        "eyJhYjEyIjp7IklEIjoiZGVhZCIsIktleSI6InByb2ZpbGUtYWIxMiJ9fQ==",
        "eyJhYjEyIjpmYWxzZX0=",
    ] {
        let mut document = current_store(ACCOUNT);
        document["_profiles"] = json!(metadata);
        assert_eq!(parse(document), Err(UnknownTailscaleSession));
    }
    let mut missing = current_store(ACCOUNT);
    missing.as_object_mut().unwrap().remove("profile-ab12");
    assert_eq!(parse(missing), Err(UnknownTailscaleSession));
    let mut mismatched = current_store(ACCOUNT);
    mismatched["_current-profile"] = json!("cHJvZmlsZS1kZWFk");
    assert_eq!(parse(mismatched), Err(UnknownTailscaleSession));
}

#[test]
fn filestore_base64_is_strict_despite_the_reused_tolerant_warp_decoder() {
    for encoded in [
        "***", "YQ", "Y Q==", "YQ===", "Y=Q=", "YR==", "YWJ=", "____",
    ] {
        let document = json!({"_machinekey":encoded});
        assert_eq!(
            cached_session_exists(&serde_json::to_vec(&document).unwrap()),
            Err(UnknownTailscaleSession)
        );
    }
    for encoded in ["", "YQ==", "YWI=", "YWJj"] {
        assert!(decode_value(&Value::String(encoded.into())).is_ok());
    }
}
