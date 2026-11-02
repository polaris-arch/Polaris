use super::*;

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
fn mutate_document(value: &mut Value, mutate: impl FnOnce(&mut Value)) {
    let observation = &mut value["bindings"][0]["provenance"];
    let mut doc: Value = serde_json::from_str(observation["stdout"].as_str().unwrap()).unwrap();
    mutate(&mut doc);
    let raw = doc.to_string();
    observation["stdoutSha256"] = json!(polaris_updater::sha256_hex(raw.as_bytes()));
    observation["stdout"] = json!(raw);
}

#[test]
fn reviewed_directory_matches_exact_expected_core_without_observing_installed_files() {
    for platform in ["linux", "mac-arm64", "win", "mac-x64"] {
        let directory = load(&catalog(), platform).unwrap();
        let value = serde_json::to_value(directory).unwrap();
        assert_eq!(value["evidenceKind"], KIND);
        assert!(value["unavailableReason"].is_null());
        assert_eq!(value["selectedSpec"]["platform"], platform);
        let observed = matches!(platform, "linux" | "mac-arm64");
        assert_eq!(
            value["selectedSpec"]["dataOrigin"],
            if observed {
                "observed-cli-metadata"
            } else {
                "source-derived-stable-catalog"
            }
        );
        assert_eq!(
            value["selectedSpec"]["observationStdoutSha256"].is_string(),
            observed
        );
        assert_eq!(
            value["selectedSpec"]["observationStdoutSha256"].is_null(),
            !observed
        );
        assert_eq!(value["document"]["profiles"].as_array().unwrap().len(), 16);
        assert_eq!(
            value["document"]["legacyChromeAliases"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
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
fn observed_process_template_fields_never_enter_the_stable_directory() {
    let mut value = catalog();
    mutate_document(&mut value, |doc| {
        for profile in doc["profiles"].as_array_mut().unwrap() {
            profile["client"] = json!("process-specific client");
            profile["templateVersion"] = json!("process-specific version");
            profile["templateOffersX25519MLKEM768"] = json!(null);
        }
    });
    let doc = load(&value, "linux").unwrap().document.unwrap();
    assert!(doc.profiles.iter().all(|p| p.client.is_none()
        && p.template_version.is_none()
        && p.template_offers_x25519_mlkem768.is_none()));
}

#[test]
fn missing_platform_and_mismatched_compiled_selection_never_reuse_another_directory() {
    let mut value = catalog();
    value["bindings"]
        .as_array_mut()
        .unwrap()
        .retain(|o| o["platform"] != "linux");
    assert!(load(&value, "linux").is_err());
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
fn catalog_schema_pin_fields_duplicate_platforms_and_unknown_fields_fail_closed() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["schema"] = json!("future"),
        |v| v["sourceCommit"] = json!("0".repeat(40)),
        |v| v["sourceTree"] = json!("0".repeat(40)),
        |v| v["version"] = json!("other"),
        |v| v["requiredGoVersion"] = json!("go0.0"),
        |v| v["contractSha256"] = json!("0".repeat(64)),
        |v| v["sourceArchiveSha256"] = json!("0".repeat(64)),
        |v| v["sourceIndexSha256"] = json!("0".repeat(64)),
        |v| v["sourceFiles"][0]["sha256"] = json!("not-a-digest"),
        |v| v["sourceFiles"][0]["path"] = json!("unknown/source.go"),
        |v| v["sourceFiles"][0]["extra"] = json!(true),
        |v| {
            let first = v["sourceFiles"][0].clone();
            v["sourceFiles"][1] = first;
        },
        |v| v["extra"] = json!(true),
        |v| v["bindings"][0]["extra"] = json!(true),
        |v| v["bindings"][0]["platform"] = json!("unknown"),
        |v| v["bindings"][0]["core"]["sha256"] = json!("0".repeat(64)),
        |v| v["bindings"][0]["core"]["bytes"] = json!(1),
        |v| v["bindings"][0]["provenance"]["stdoutSha256"] = json!("0".repeat(64)),
        |v| {
            let duplicate = v["bindings"][0].clone();
            v["bindings"].as_array_mut().unwrap().push(duplicate);
        },
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
fn source_derived_bindings_require_distinct_closed_provenance_and_matching_build_fields() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["provenance"]["kind"] = json!("future"),
        |v| v["provenance"]["sourceTablePath"] = json!("unknown/source.go"),
        |v| v["provenance"]["sourceTableSha256"] = json!("0".repeat(64)),
        |v| v["provenance"]["stdoutSha256"] = json!("0".repeat(64)),
        |v| v["provenance"]["stdout"] = json!("invented CLI report"),
        |v| v["target"] = json!("linux/amd64"),
        |v| v["goVersion"] = json!("go0.0"),
        |v| v["buildTags"] = json!("with_utls"),
        |v| v["modules"][0]["version"] = json!("other"),
    ];
    for mutate in mutations {
        for (index, platform) in [(2, "win"), (3, "mac-x64")] {
            let mut value = catalog();
            mutate(&mut value["bindings"][index]);
            assert!(load(&value, platform).is_err());
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
fn capability_document_schema_modules_target_version_and_inventory_are_bound() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["schema"] = json!("future"),
        |v| v["sourceCommit"] = json!("0".repeat(40)),
        |v| v["sourceModified"] = json!(true),
        |v| v["coreVersion"] = json!("other"),
        |v| v["goVersion"] = json!("go0.0"),
        |v| v["requiredGoVersion"] = json!("go0.0"),
        |v| v["os"] = json!("darwin"),
        |v| v["arch"] = json!("arm64"),
        |v| v["buildTags"] = json!("with_utls"),
        |v| v["modules"][0]["version"] = json!("other"),
        |v| v["modules"][0]["replace"]["path"] = json!("./other"),
        |v| v["profiles"][0]["selection"] = json!("profile"),
        |v| v["profiles"][2]["id"] = json!("invented"),
        |v| v["profiles"][2]["extra"] = json!(true),
        |v| {
            let duplicate = v["profiles"][2].clone();
            v["profiles"].as_array_mut().unwrap().push(duplicate);
        },
        |v| {
            let duplicate = v["modules"][0].clone();
            v["modules"][1] = duplicate;
        },
        |v| {
            v["profiles"].as_array_mut().unwrap().pop();
        },
        |v| {
            v["profiles"][0]
                .as_object_mut()
                .unwrap()
                .remove("templateOffersX25519MLKEM768");
        },
    ];
    for mutate in mutations {
        let mut value = catalog();
        mutate_document(&mut value, mutate);
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
