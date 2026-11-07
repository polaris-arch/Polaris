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
// Actual 3155 CLI observations are historical fixtures, never S19 observations.
const HISTORICAL_OBSERVED_BINDINGS: &str = r#"[
  {
    "platform": "linux",
    "target": "linux/amd64",
    "core": {
      "member": "sing-box",
      "bytes": 87285987,
      "sha256": "1188baf18f7e4d9d4daabcf01633d72a2174ccb95ed71847d5032d857ad6679b"
    },
    "goVersion": "go1.27.2",
    "buildTags": "with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0,with_purego",
    "modules": [
      {
        "path": "github.com/refraction-networking/utls",
        "version": "v1.8.3-0.20261006222701-ff1b50fbbe9a",
        "sum": "h1:YwnpbX4nj1GfLEsvGDDqzpbxTVVbHcEqCfUcfMYe1gc="
      },
      {
        "path": "github.com/metacubex/utls",
        "version": "v1.8.8",
        "sum": "",
        "replace": {
          "path": "./third_party/metacubex-utls",
          "version": "(devel)",
          "sum": ""
        }
      }
    ],
    "provenance": {
      "kind": "observed-cli-metadata",
      "stdoutSha256": "801967de5b95cba408d56d160ce6888082222f194301bcb80728a7bfa635ca1e",
      "stdout": "{\"schema\":\"polaris-tls-fingerprint-capabilities-v1\",\"enabled\":true,\"coreVersion\":\"1.15.0-polaris-alpha.12.7\",\"sourceCommit\":\"3155c9151f553967aabcee93f09e51084fc89899\",\"sourceModified\":false,\"goVersion\":\"go1.27.2\",\"requiredGoVersion\":\"go1.27.2\",\"buildTags\":\"with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0,with_purego\",\"os\":\"linux\",\"arch\":\"amd64\",\"modules\":[{\"path\":\"github.com/metacubex/utls\",\"version\":\"v1.8.8\",\"sum\":\"\",\"replace\":{\"path\":\"./third_party/metacubex-utls\",\"version\":\"(devel)\",\"sum\":\"\"}},{\"path\":\"github.com/refraction-networking/utls\",\"version\":\"v1.8.3-0.20261006222701-ff1b50fbbe9a\",\"sum\":\"h1:YwnpbX4nj1GfLEsvGDDqzpbxTVVbHcEqCfUcfMYe1gc=\"}],\"defaultFingerprint\":\"chrome\",\"profiles\":[{\"id\":\"random\",\"displayName\":\"Random browser\",\"client\":\"Safari\",\"templateVersion\":\"26.3\",\"selection\":\"process-browser\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"randomized\",\"displayName\":\"Randomized ClientHello\",\"client\":\"Randomized\",\"templateVersion\":\"0\",\"selection\":\"process-seeded-generator\",\"templateOffersX25519MLKEM768\":null},{\"id\":\"chrome\",\"displayName\":\"Chrome (Auto)\",\"client\":\"Chrome\",\"templateVersion\":\"155\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"chrome_155\",\"displayName\":\"Chrome 155\",\"client\":\"Chrome\",\"templateVersion\":\"155\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"chrome_133\",\"displayName\":\"Chrome 133\",\"client\":\"Chrome\",\"templateVersion\":\"133\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"firefox\",\"displayName\":\"Firefox (Auto)\",\"client\":\"Firefox\",\"templateVersion\":\"148\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"firefox_148\",\"displayName\":\"Firefox 148\",\"client\":\"Firefox\",\"templateVersion\":\"148\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"firefox_120\",\"displayName\":\"Firefox 120\",\"client\":\"Firefox\",\"templateVersion\":\"120\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"edge\",\"displayName\":\"Edge (Auto)\",\"client\":\"Edge\",\"templateVersion\":\"85\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"safari\",\"displayName\":\"Safari (Auto)\",\"client\":\"Safari\",\"templateVersion\":\"26.3\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"safari_26_3\",\"displayName\":\"Safari 26.3\",\"client\":\"Safari\",\"templateVersion\":\"26.3\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"safari_16_0\",\"displayName\":\"Safari 16.0\",\"client\":\"Safari\",\"templateVersion\":\"16.0\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"360\",\"displayName\":\"360 (Auto)\",\"client\":\"360Browser\",\"templateVersion\":\"7.5\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"qq\",\"displayName\":\"QQ (Auto)\",\"client\":\"QQBrowser\",\"templateVersion\":\"11.1\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"ios\",\"displayName\":\"iOS (Auto)\",\"client\":\"iOS\",\"templateVersion\":\"14\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"android\",\"displayName\":\"Android 11 OkHttp\",\"client\":\"Android\",\"templateVersion\":\"11\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false}],\"legacyChromeAliases\":[\"chrome_psk\",\"chrome_psk_shuffle\",\"chrome_padding_psk_shuffle\",\"chrome_pq\",\"chrome_pq_psk\"],\"randomSemantics\":\"one browser Auto template selected per process; libbox instance restart does not reselect\",\"randomizedSemantics\":\"weighted randomized generator with process-scoped seed; not a browser draw and not guaranteed different per connection\",\"shadowTLSSessionIDWithECHSupported\":false,\"curvePreferencesSupported\":false,\"realityX25519MLKEM768\":\"removed from supported groups and key shares before REALITY authentication\",\"negotiationEvidence\":\"template offer only; negotiated group requires an actual completed TLS handshake\"}\n"
    }
  },
  {
    "platform": "mac-arm64",
    "target": "darwin/arm64",
    "core": {
      "member": "sing-box",
      "bytes": 89491186,
      "sha256": "056809246f89d715c026cbc11e072f74a04c4795004907227d274dae9cd26a9f"
    },
    "goVersion": "go1.27.2",
    "buildTags": "with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0",
    "modules": [
      {
        "path": "github.com/refraction-networking/utls",
        "version": "v1.8.3-0.20261006222701-ff1b50fbbe9a",
        "sum": "h1:YwnpbX4nj1GfLEsvGDDqzpbxTVVbHcEqCfUcfMYe1gc="
      },
      {
        "path": "github.com/metacubex/utls",
        "version": "v1.8.8",
        "sum": "",
        "replace": {
          "path": "./third_party/metacubex-utls",
          "version": "(devel)",
          "sum": ""
        }
      }
    ],
    "provenance": {
      "kind": "observed-cli-metadata",
      "stdoutSha256": "eda9a1e8fa5d90c15c2f813664483d5b967337aa1b7d0578849875ece800f7a0",
      "stdout": "{\"schema\":\"polaris-tls-fingerprint-capabilities-v1\",\"enabled\":true,\"coreVersion\":\"1.15.0-polaris-alpha.12.7\",\"sourceCommit\":\"3155c9151f553967aabcee93f09e51084fc89899\",\"sourceModified\":false,\"goVersion\":\"go1.27.2\",\"requiredGoVersion\":\"go1.27.2\",\"buildTags\":\"with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0\",\"os\":\"darwin\",\"arch\":\"arm64\",\"modules\":[{\"path\":\"github.com/metacubex/utls\",\"version\":\"v1.8.8\",\"sum\":\"\",\"replace\":{\"path\":\"./third_party/metacubex-utls\",\"version\":\"(devel)\",\"sum\":\"\"}},{\"path\":\"github.com/refraction-networking/utls\",\"version\":\"v1.8.3-0.20261006222701-ff1b50fbbe9a\",\"sum\":\"h1:YwnpbX4nj1GfLEsvGDDqzpbxTVVbHcEqCfUcfMYe1gc=\"}],\"defaultFingerprint\":\"chrome\",\"profiles\":[{\"id\":\"random\",\"displayName\":\"Random browser\",\"client\":\"Safari\",\"templateVersion\":\"26.3\",\"selection\":\"process-browser\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"randomized\",\"displayName\":\"Randomized ClientHello\",\"client\":\"Randomized\",\"templateVersion\":\"0\",\"selection\":\"process-seeded-generator\",\"templateOffersX25519MLKEM768\":null},{\"id\":\"chrome\",\"displayName\":\"Chrome (Auto)\",\"client\":\"Chrome\",\"templateVersion\":\"155\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"chrome_155\",\"displayName\":\"Chrome 155\",\"client\":\"Chrome\",\"templateVersion\":\"155\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"chrome_133\",\"displayName\":\"Chrome 133\",\"client\":\"Chrome\",\"templateVersion\":\"133\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"firefox\",\"displayName\":\"Firefox (Auto)\",\"client\":\"Firefox\",\"templateVersion\":\"148\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"firefox_148\",\"displayName\":\"Firefox 148\",\"client\":\"Firefox\",\"templateVersion\":\"148\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"firefox_120\",\"displayName\":\"Firefox 120\",\"client\":\"Firefox\",\"templateVersion\":\"120\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"edge\",\"displayName\":\"Edge (Auto)\",\"client\":\"Edge\",\"templateVersion\":\"85\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"safari\",\"displayName\":\"Safari (Auto)\",\"client\":\"Safari\",\"templateVersion\":\"26.3\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"safari_26_3\",\"displayName\":\"Safari 26.3\",\"client\":\"Safari\",\"templateVersion\":\"26.3\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":true},{\"id\":\"safari_16_0\",\"displayName\":\"Safari 16.0\",\"client\":\"Safari\",\"templateVersion\":\"16.0\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"360\",\"displayName\":\"360 (Auto)\",\"client\":\"360Browser\",\"templateVersion\":\"7.5\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"qq\",\"displayName\":\"QQ (Auto)\",\"client\":\"QQBrowser\",\"templateVersion\":\"11.1\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"ios\",\"displayName\":\"iOS (Auto)\",\"client\":\"iOS\",\"templateVersion\":\"14\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false},{\"id\":\"android\",\"displayName\":\"Android 11 OkHttp\",\"client\":\"Android\",\"templateVersion\":\"11\",\"selection\":\"profile\",\"templateOffersX25519MLKEM768\":false}],\"legacyChromeAliases\":[\"chrome_psk\",\"chrome_psk_shuffle\",\"chrome_padding_psk_shuffle\",\"chrome_pq\",\"chrome_pq_psk\"],\"randomSemantics\":\"one browser Auto template selected per process; libbox instance restart does not reselect\",\"randomizedSemantics\":\"weighted randomized generator with process-scoped seed; not a browser draw and not guaranteed different per connection\",\"shadowTLSSessionIDWithECHSupported\":false,\"curvePreferencesSupported\":false,\"realityX25519MLKEM768\":\"removed from supported groups and key shares before REALITY authentication\",\"negotiationEvidence\":\"template offer only; negotiated group requires an actual completed TLS handshake\"}\n"
    }
  }
]"#;

fn observed_catalog() -> Value {
    let mut value = catalog();
    value["sourceCommit"] = json!("3155c9151f553967aabcee93f09e51084fc89899");
    value["sourceTree"] = json!("457a8906cd3088578aa7e9d93185f07142178b5b");
    value["version"] = json!("1.15.0-polaris-alpha.12.7");
    value["contractSha256"] =
        json!("9537af1d9de8d16885022f21e70647b8c5bec920179b8c1e2727a1e8ea420fdf");
    value["sourceArchiveSha256"] =
        json!("a765460d44fcc94d7d1ce0bb7ff03369cbea467fc4e1f98e1647c32b51a52d9e");
    value["sourceIndexSha256"] =
        json!("6a1ccf4dc27e7a9e2216688a65aecb72bcdc0874e308aa2bb589cf5011eb1c52");
    value["bindings"] = serde_json::from_str(HISTORICAL_OBSERVED_BINDINGS).unwrap();
    value
}

// Minimal historical row exercises the unchanged observed provenance guard;
// it is not a full release contract or a current public-consumption receipt.
fn load_observed(value: &Value, platform: &str) -> Result<(), String> {
    let parsed: Catalog = serde_json::from_value(value.clone()).map_err(|_| reject())?;
    let index = parsed
        .bindings
        .iter()
        .position(|b| b.platform == platform)
        .ok_or_else(reject)?;
    let binding = &value["bindings"][index];
    let row = json!({
        "sourceCommit": value["sourceCommit"], "sourceTree": value["sourceTree"],
        "version": value["version"], "target": binding["target"], "core": binding["core"],
        "goVersion": binding["goVersion"], "tags": binding["buildTags"],
        "buildGraph": {
            "normalModules": [{"module":"github.com/refraction-networking/utls",
                "version":"v1.8.3-0.20261006222701-ff1b50fbbe9a",
                "h1":"h1:YwnpbX4nj1GfLEsvGDDqzpbxTVVbHcEqCfUcfMYe1gc="}],
            "localReplacements": [{"module":"github.com/metacubex/utls", "version":"v1.8.8", "h1":"",
                "replacement":{"module":"./third_party/metacubex-utls", "version":"(devel)", "h1":""}}]
        }
    });
    validate_binding(&parsed.bindings[index], &parsed, &row)
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
        assert_eq!(
            value["selectedSpec"]["dataOrigin"],
            "source-derived-stable-catalog"
        );
        assert!(value["selectedSpec"]["observationStdoutSha256"].is_null());
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
    let mut value = observed_catalog();
    load_observed(&value, "linux").unwrap();
    mutate_document(&mut value, |doc| {
        for profile in doc["profiles"].as_array_mut().unwrap() {
            profile["client"] = json!("process-specific client");
            profile["templateVersion"] = json!("process-specific version");
            profile["templateOffersX25519MLKEM768"] = json!(null);
        }
    });
    load_observed(&value, "linux").unwrap();
    let doc = load(&catalog(), "linux").unwrap().document.unwrap();
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
        |v| v["target"] = json!("unknown/unknown"),
        |v| v["goVersion"] = json!("go0.0"),
        |v| v["buildTags"] = json!("with_utls"),
        |v| v["modules"][0]["version"] = json!("other"),
    ];
    for mutate in mutations {
        for (index, platform) in [(0, "linux"), (1, "mac-arm64"), (2, "win"), (3, "mac-x64")] {
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
    load_observed(&observed_catalog(), "linux").unwrap();
    for mutate in mutations {
        let mut value = observed_catalog();
        mutate_document(&mut value, mutate);
        assert!(load_observed(&value, "linux").is_err());
    }
}

#[test]
fn historical_observed_stdout_cannot_claim_current_s19_cli_provenance() {
    let history = observed_catalog();
    for (index, platform, expected_sha) in [
        (
            0,
            "linux",
            "801967de5b95cba408d56d160ce6888082222f194301bcb80728a7bfa635ca1e",
        ),
        (
            1,
            "mac-arm64",
            "eda9a1e8fa5d90c15c2f813664483d5b967337aa1b7d0578849875ece800f7a0",
        ),
    ] {
        load_observed(&history, platform).unwrap();
        let observation = &history["bindings"][index]["provenance"];
        assert_eq!(observation["stdoutSha256"], expected_sha);
        assert_eq!(
            polaris_updater::sha256_hex(observation["stdout"].as_str().unwrap().as_bytes()),
            expected_sha
        );
        let mut invalid_hash = history.clone();
        invalid_hash["bindings"][index]["provenance"]["stdoutSha256"] = json!("0".repeat(64));
        assert!(load_observed(&invalid_hash, platform).is_err());
        let mut current = catalog();
        current["bindings"][index]["provenance"] = observation.clone();
        assert!(load(&current, platform).is_err());
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
