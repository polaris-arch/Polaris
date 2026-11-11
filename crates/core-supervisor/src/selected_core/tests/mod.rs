use super::*;

#[test]
fn installer_bootstrap_pin_does_not_admit_an_execution_selection() {
    let Ok(platform) = current_platform() else {
        return;
    };
    let formal_bootstrap = selection_lane() == SelectionLane::Formal
        && COMPILED_INSTALLATION == br#"{"schema":"polaris-formal-desktop-unavailable-v1"}"#
        && COMPILED_VALIDATION_SOURCE == b"null";
    if formal_bootstrap {
        let expected = formal_manifest(platform).unwrap().id(sha256_hex).unwrap();
        assert_eq!(selected_installer_id(platform).unwrap(), expected);
        assert!(selected_manifest(platform).is_err());
        assert!(selected_id(platform).is_err());
        assert!(selected_check_source(platform).is_err());
        println!("POLARIS_TEST_FORMAL_PENDING_INSTALLER_PIN_ACCEPTED_EXECUTION_REJECTED");
    } else {
        // Candidate/temporary, installed formal and local lanes cannot acquire
        // an installer-only fallback or borrow a different compiled core id.
        assert_eq!(selected_installer_id(platform), selected_id(platform));
    }
    assert!(selected_installer_id("unsupported-platform").is_err());
}

fn inventory(platform: &str) -> serde_json::Value {
    use serde_json::json;
    let contract: serde_json::Value = serde_json::from_str(RELEASE_CONTRACT).unwrap();
    let archive = &contract["platforms"][platform]["archive"];
    let mut sizes = serde_json::Map::new();
    // Non-payload sizes here are header fixtures, not archive verification.
    // The non-executing decoder must still verify every actual byte/hash.
    for name in archive["members"].as_object().unwrap().keys() {
        sizes.insert(name.clone(), json!(1));
    }
    for file in formal_manifest(platform).unwrap().files {
        sizes.insert(
            format!("{}/{}", archive["root"].as_str().unwrap(), file.name),
            json!(file.bytes),
        );
    }
    json!({"schema":"polaris-core-recovery-inventory-v1","platform":platform,"contractSha256":sha256_hex(RELEASE_CONTRACT.as_bytes()),"archive":{"name":archive["name"],"format":if platform=="win" {"zip"}else{"tar.gz"},"bytes":archive["bytes"],"sha256":archive["sha256"],"root":archive["root"]},"memberBytes":sizes})
}
#[test]
fn compiled_lane_never_uses_validation_receipts_as_local_authority() {
    #[cfg(feature = "formal-selection")]
    {
        assert_eq!(selection_lane(), SelectionLane::Formal);
        assert_eq!(COMPILED_DEVELOPMENT_SELECTION, b"null");
        assert_eq!(COMPILED_DEVELOPMENT_CONTRACT, b"null");
    }
    #[cfg(feature = "local-development-selection")]
    {
        assert_eq!(selection_lane(), SelectionLane::LocalDevelopment);
        assert_eq!(COMPILED_VALIDATION_SOURCE, b"null");
        let pure = selected_pure_manifest("linux").unwrap();
        let source = selected_source_commit("linux").unwrap();
        assert_ne!(source, selected_source_from(b"null", "linux").unwrap());
        let mut installed = pure.clone();
        installed.helper_sha256 = Some("f".repeat(64));
        assert_eq!(
            pure.id(sha256_hex).unwrap(),
            installed.id(sha256_hex).unwrap()
        );
        assert!(selected_pure_manifest("win").is_err());
        assert!(selected_pure_manifest("mac-arm64").is_err());
        assert_eq!(selected_check_source("linux").unwrap().0, source);
    }
}

#[test]
fn recovery_data_uses_the_one_compiled_hash_set_and_derives_core_id() {
    for platform in ["linux", "win", "mac-x64", "mac-arm64"] {
        let inventory = inventory(platform);
        let spec = recovery_spec_from_inventory(platform, &serde_json::to_vec(&inventory).unwrap())
            .unwrap();
        assert_eq!(
            spec["coreId"],
            formal_manifest(platform)
                .unwrap()
                .id(sha256_hex)
                .unwrap()
                .as_str()
        );
        assert_eq!(
            spec["payloadMembers"].as_array().unwrap().len(),
            formal_manifest(platform).unwrap().files.len()
        );
        assert_eq!(
            spec["members"].as_array().unwrap().len(),
            inventory["memberBytes"].as_object().unwrap().len()
        );
    }
}
#[test]
fn wrong_or_extra_inventory_fields_cannot_select_another_archive_or_member() {
    for path in [
        vec!["platform"],
        vec!["contractSha256"],
        vec!["archive", "sha256"],
        vec!["archive", "root"],
        vec!["archive", "name"],
        vec!["archive", "format"],
        vec!["archive", "bytes"],
    ] {
        let mut bad = inventory("linux");
        let mut value = &mut bad;
        for part in path {
            value = &mut value[part];
        }
        *value = serde_json::json!("foreign");
        assert!(recovery_spec_from_inventory("linux", &serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut bad = inventory("linux");
    bad["memberBytes"]["foreign/member"] = serde_json::json!(1);
    assert!(recovery_spec_from_inventory("linux", &serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = inventory("linux");
    let key = bad["memberBytes"]
        .as_object()
        .unwrap()
        .keys()
        .find(|key| key.ends_with("/sing-box"))
        .unwrap()
        .clone();
    bad["memberBytes"][key] = serde_json::json!(1);
    assert!(recovery_spec_from_inventory("linux", &serde_json::to_vec(&bad).unwrap()).is_err());
}
