use super::*;
use serde_json::json;

const CONTRACT: &str = polaris_core_supervisor::selected_core::RELEASE_CONTRACT;
fn selected(platform: &str) -> CoreManifest {
    polaris_core_supervisor::selected_core::formal_manifest(platform).unwrap()
}
fn catalog() -> Value {
    serde_json::from_str(CATALOG).unwrap()
}
fn load(value: &Value, platform: &str) -> Result<SupportDirectory, String> {
    directory_for(&value.to_string(), CONTRACT, platform, &selected(platform))
}
#[test]
fn reviewed_directory_matches_exact_expected_core_without_observing_installed_files() {
    for platform in ["linux", "mac-arm64", "win", "mac-x64"] {
        let directory = load(&catalog(), platform).unwrap();
        let value = serde_json::to_value(directory).unwrap();
        assert_eq!(value["evidenceKind"], KIND);
        assert!(value["unavailableReason"].is_null());
        assert_eq!(value["selectedSpec"]["platform"], platform);
        assert_eq!(
            value["selectedSpec"]["dataOrigin"],
            "source-derived-stable-catalog"
        );
        assert!(value["selectedSpec"]["observationStdoutSha256"].is_null());
        for field in [
            "identity",
            "binarySha256",
            "observationScope",
            "byteIdentityScope",
        ] {
            assert!(
                value.get(field).is_none(),
                "data must not impersonate {field}"
            );
        }
    }
}

#[test]
fn every_directory_profile_keeps_dynamic_template_fields_unknown_and_stable_semantics() {
    for platform in ["linux", "mac-arm64", "win", "mac-x64"] {
        let doc = load(&catalog(), platform).unwrap().document.unwrap();
        for profile in &doc.profiles {
            assert!(profile.client.is_none());
            assert!(profile.template_version.is_none());
            assert!(profile.template_offers_x25519_mlkem768.is_none());
        }
        let chrome = doc.profiles.iter().find(|p| p.id == "chrome_155").unwrap();
        assert_eq!(chrome.display_name, "Chrome 155");
        assert!(doc.random_semantics.contains("per process"));
        assert!(doc.randomized_semantics.contains("process-scoped seed"));
        assert!(doc
            .negotiation_evidence
            .contains("no runtime or handshake observation"));
    }
}

#[test]
fn missing_platform_and_mismatched_compiled_selection_never_reuse_another_directory() {
    assert!(directory_for(CATALOG, CONTRACT, "unknown", &selected("linux")).is_err());
    for platform in ["linux", "mac-arm64", "win", "mac-x64"] {
        let mut spec = selected(platform);
        let core_name = if platform == "win" {
            "sing-box.exe"
        } else {
            "sing-box"
        };
        spec.files
            .iter_mut()
            .find(|file| file.name == core_name)
            .unwrap()
            .sha256 = "0".repeat(64);
        assert!(directory_for(CATALOG, CONTRACT, platform, &spec).is_err());
        spec = selected(platform);
        spec.files
            .iter_mut()
            .find(|file| file.name == core_name)
            .unwrap()
            .bytes += 1;
        assert!(directory_for(CATALOG, CONTRACT, platform, &spec).is_err());
        spec = selected(platform);
        spec.version.push_str("-other");
        assert!(directory_for(CATALOG, CONTRACT, platform, &spec).is_err());
    }
    assert!(directory_for(CATALOG, CONTRACT, "mac-arm64", &selected("linux")).is_err());
    let unavailable = SupportDirectory::unavailable(reject());
    assert!(unavailable.selected_spec.is_none() && unavailable.document.is_none());
}

#[test]
fn catalog_schema_and_unknown_fields_fail_closed() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["schema"] = json!("future"),
        |v| v["extra"] = json!(true),
        |v| v["bindings"] = json!([]),
    ];
    for mutate in mutations {
        let mut value = catalog();
        mutate(&mut value);
        assert!(load(&value, "linux").is_err());
    }
    assert!(directory_for("{}", CONTRACT, "linux", &selected("linux")).is_err());
    assert!(directory_for(
        &" ".repeat(MAX_CATALOG + 1),
        CONTRACT,
        "linux",
        &selected("linux")
    )
    .is_err());
    let duplicate_key = CATALOG.replacen("{", "{\"schema\":\"duplicate\",", 1);
    assert!(directory_for(&duplicate_key, CONTRACT, "linux", &selected("linux")).is_err());
}

#[test]
fn contract_row_build_fields_are_bound_to_the_directory() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["target"] = json!("unknown/unknown"),
        |v| v["goVersion"] = json!("go0.0"),
        |v| v["tags"] = json!("with_gvisor"),
        |v| v["sourceCommit"] = json!("0".repeat(40)),
        |v| v["version"] = json!("other"),
        |v| v["core"]["sha256"] = json!("0".repeat(64)),
        |v| v["core"]["extra"] = json!(true),
    ];
    for mutate in mutations {
        for platform in ["linux", "mac-arm64", "win", "mac-x64"] {
            let mut contract: Value = serde_json::from_str(CONTRACT).unwrap();
            mutate(&mut contract["platforms"][platform]);
            let raw = contract.to_string();
            assert!(directory_for(CATALOG, &raw, platform, &selected(platform)).is_err());
        }
    }
}

#[test]
fn stable_support_rejects_invented_duplicate_or_misclassified_choices() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["defaultFingerprint"] = json!("other"),
        |v| v["extra"] = json!(true),
        |v| v["profiles"][0]["id"] = json!("invented"),
        |v| v["profiles"][0]["selection"] = json!("profile"),
        |v| v["profiles"][0]["client"] = json!("invented runtime detail"),
        |v| v["legacyChromeAliases"][0] = json!("chrome"),
        |v| {
            let duplicate = v["profiles"][0].clone();
            v["profiles"].as_array_mut().unwrap().push(duplicate);
        },
    ];
    for mutate in mutations {
        let mut value = catalog();
        mutate(&mut value["stableSupport"]);
        assert!(load(&value, "linux").is_err());
    }
}

#[test]
fn compiled_directory_cache_does_not_acquire_execution_evidence() {
    assert!(std::ptr::eq(expected_tls_support(), expected_tls_support()));
    let value = serde_json::to_value(expected_tls_support()).unwrap();
    assert_eq!(value["evidenceKind"], KIND);
    assert!(value.get("identity").is_none());
    // The desktop IPC branch must not accidentally be rewired to the old
    // process-backed capability method while mobile still uses its native path.
    let source = crate::test_support::crate_source("commands/updater/core_info.rs");
    let branch = source
        .split("if capabilities_only.unwrap_or(false) {")
        .nth(1)
        .unwrap()
        .split("let read =")
        .next()
        .unwrap();
    assert!(branch.contains("crate::runtime::core_identity::expected_tls_support()"));
    assert!(branch.contains("#[cfg(any(target_os = \"android\", target_os = \"ios\"))]"));
    assert!(branch.contains(".tls_fingerprint_capabilities()"));
}
