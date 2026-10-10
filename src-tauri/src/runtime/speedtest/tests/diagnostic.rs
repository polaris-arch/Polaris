use super::super::{build_temp_core_config, TempNode, TEMP_CORE_CONFIG_NAME};
use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("polaris-q6-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn raw(&self) -> PathBuf {
        self.0.join(TEMP_CORE_CONFIG_NAME)
    }
    fn kept(&self) -> PathBuf {
        self.0.join(TEMP_CORE_LAST_CONFIG_NAME)
    }
    fn put(&self, value: &Value) {
        fs::write(self.raw(), serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn archive(&self) -> Value {
        serde_json::from_slice(&fs::read(self.kept()).unwrap()).unwrap()
    }
    fn assert_empty(&self) {
        assert_eq!(fs::read_dir(&self.0).unwrap().count(), 0);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for file in fs::read_dir(&self.0).unwrap() {
            let file = file.unwrap();
            if file.file_type().unwrap().is_dir() {
                fs::remove_dir(file.path()).unwrap();
            } else {
                fs::remove_file(file.path()).unwrap();
            }
        }
        fs::remove_dir(&self.0).unwrap();
    }
}

#[test]
fn multi_protocol_credentials_and_free_text_never_reach_the_archive() {
    let f = Fixture::new();
    let sentinel = "Q6_SYNTHETIC_SECRET";
    let mut nodes = Vec::new();
    for protocol in [
        "shadowsocks",
        "trojan",
        "vmess",
        "vless",
        "hysteria",
        "hysteria2",
        "tuic",
        "naive",
        "anytls",
        "ssh",
        "snell",
        "shadowtls",
        "tailcat",
        "tor",
        "wireguard",
        "masque",
        "openconnect",
        "openvpn-client",
    ] {
        let tag = format!("{sentinel}-{protocol}");
        let node = json!({"type":protocol,"tag":tag,"server":format!(
            "https://user:{sentinel}@vpn.example/vpn/{sentinel}?token={sentinel}#fragment"),
            "server_port":443, "password":sentinel,"username":sentinel,"uuid":sentinel,
            "auth_str":sentinel,"auth_key":sentinel,"private_key":sentinel,
            "private_key_path":sentinel,"private_key_passphrase":sentinel,
            "psk":sentinel,"userkey":sentinel,"pre_shared_key":sentinel,
            "server_public_key":sentinel,"server_disco_key":sentinel,
            "tls":{"enabled":true,"server_name":sentinel,"certificate": [sentinel],
                "key":sentinel,"reality":{"enabled":true,"public_key":sentinel,"short_id":sentinel},
                "ech":{"enabled":true,"config":[sentinel]}},
            "transport":{"type":"ws","path":format!("/x?session={sentinel}"),
                "headers":{"Authorization":sentinel,"Cookie":[sentinel],sentinel:sentinel}},
            "headers":{"X-Api-Key":sentinel},"plugin_opts":format!("token={sentinel}"),
            "extra_args":[format!("--password={sentinel}")],"torrc":{sentinel:sentinel},
            "authentication":{"username":sentinel,"password":sentinel,"token":sentinel},
            "users":[{"username":sentinel,"password":sentinel}],
            "peers":[{"address":sentinel,"port":51820,"public_key":sentinel,
                "pre_shared_key":sentinel,"allowed_ips":["0.0.0.0/0"]}],
            "tls_auth":sentinel,"tls_crypt":sentinel,"client_key":sentinel,
            "client_certificate":sentinel, "unknown_extension": {sentinel:sentinel}});
        nodes.push(TempNode {
            id: protocol.into(),
            tag,
            node,
            companion_outbounds: vec![],
            is_endpoint: matches!(
                protocol,
                "wireguard" | "masque" | "openconnect" | "openvpn-client"
            ),
            has_local_v6: false,
        });
    }
    // Use the actual speedtest builder, including endpoint DNS rules and tags.
    let ports: Vec<u16> = (20000..20000 + nodes.len() as u16).collect();
    let cfg = build_temp_core_config(&nodes, &ports, "trace");
    assert!(cfg.to_string().contains(sentinel));
    f.put(&cfg);
    retire(&f.raw(), true).unwrap();
    assert!(!f.raw().exists());
    let archived = f.archive();
    assert!(!archived.to_string().contains(sentinel));
    assert_eq!(archived["_diagnostic"]["runnable"], false);
    assert_eq!(archived["log"]["level"], "trace");
    assert_eq!(archived["inbounds"].as_array().unwrap().len(), nodes.len());
    assert_eq!(archived["outbounds"].as_array().unwrap().len(), 15); // 14 + direct
    assert_eq!(archived["endpoints"].as_array().unwrap().len(), 4);
    for entry in archived["outbounds"]
        .as_array()
        .unwrap()
        .iter()
        .chain(archived["endpoints"].as_array().unwrap())
    {
        assert_ne!(entry["type"], REDACTED);
        if entry["type"] != "direct" {
            assert_eq!(entry["server_port"], 443);
            assert_eq!(entry["tls"]["enabled"], true);
            assert_eq!(entry["transport"]["type"], "ws");
            assert_eq!(entry["transport"]["headers"], REDACTED);
        }
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}

#[test]
fn projection_preserves_numeric_tuning_enums_and_anonymous_tag_links() {
    let f = Fixture::new();
    f.put(&json!({"log":{"level":"debug","timestamp":true},
        "inbounds":[{"type":"http","tag":"sensitive-in","listen_port":20001}],
        "outbounds":[{"type":"naive","tag":"sensitive-out","detour":"outer",
            "tls":{"engine":"cronet","enabled":true,"alpn":["h2","private-alpn"]},
            "up_mbps":20,"multiplex":{"enabled":false,"min_streams":4},
            "version":2,"password":123456789},
            {"type":"shadowtls","tag":"outer","password":false}],
        "route":{"rules":[{"inbound":["sensitive-in"],"outbound":"sensitive-out","action":"route"}]}}));
    retire(&f.raw(), true).unwrap();
    let a = f.archive();
    assert_eq!(
        a["route"]["rules"][0]["inbound"][0],
        a["inbounds"][0]["tag"]
    );
    assert_eq!(a["route"]["rules"][0]["outbound"], a["outbounds"][0]["tag"]);
    assert_eq!(a["outbounds"][0]["detour"], a["outbounds"][1]["tag"]);
    assert_eq!(a["outbounds"][0]["tls"]["engine"], "cronet");
    assert_eq!(a["outbounds"][0]["tls"]["alpn"], json!(["h2", REDACTED]));
    assert_eq!(a["outbounds"][0]["up_mbps"], 20);
    assert_eq!(a["outbounds"][0]["multiplex"]["min_streams"], 4);
    assert_eq!(a["outbounds"][0]["multiplex"]["enabled"], false);
    assert_eq!(a["outbounds"][0]["password"], REDACTED);
    assert_eq!(a["outbounds"][1]["password"], REDACTED);
    assert!(!a.to_string().contains("sensitive"));
}

#[test]
fn unknown_fields_keys_and_malformed_safe_options_are_withheld() {
    let f = Fixture::new();
    f.put(
        &json!({"outbounds":[{"type":"private-protocol-token","tag":"private-tag",
        "server_port":"private-numeric-token","tls":{"enabled":"private-enabled-token"},
        "private-field-name":{"arbitrary_secret":1234},"headers":{"private-header":1234}}]}),
    );
    retire(&f.raw(), true).unwrap();
    let a = f.archive();
    assert!(!a.to_string().contains("private"));
    assert!(!a.to_string().contains("1234"));
    assert_eq!(a["outbounds"][0]["_omitted_fields"], 1);
    assert_eq!(a["outbounds"][0]["server_port"], REDACTED);
}

#[test]
fn invalid_json_and_invalid_root_remove_raw_and_legacy_archive() {
    for bad in [
        b"{\"password\":\"SYNTHETIC_SECRET\"".as_slice(),
        b"[1,2]",
        b"null",
    ] {
        let f = Fixture::new();
        fs::write(f.raw(), bad).unwrap();
        fs::write(f.kept(), "LEGACY_SECRET").unwrap();
        assert_eq!(
            retire(&f.raw(), true).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        f.assert_empty();
    }
}

#[test]
fn legacy_raw_archive_is_scrubbed_with_or_without_a_new_source() {
    for present in [true, false] {
        let f = Fixture::new();
        fs::write(
            f.kept(),
            br#"{"outbounds":[{"type":"trojan","password":"LEGACY_SECRET"}]}"#,
        )
        .unwrap();
        if present {
            f.put(&json!({"outbounds":[{"type":"naive","password":"NEW_SECRET"}]}));
        }
        retire(&f.raw(), true).unwrap();
        let a = f.archive();
        assert_eq!(
            a["outbounds"][0]["type"],
            if present { "naive" } else { "trojan" }
        );
        assert_eq!(a["outbounds"][0]["password"], REDACTED);
        assert!(!a.to_string().contains("SECRET"));
    }
}

#[test]
fn double_retirement_preserves_safe_archive_and_its_omitted_count() {
    let f = Fixture::new();
    f.put(
        &json!({"inbounds":[{"type":"http","tag":"in"}],"outbounds":[
        {"type":"trojan","tag":"out","password":"secret","unknown":true}],
        "route":{"rules":[{"inbound":["in"],"outbound":"out"}]}}),
    );
    retire(&f.raw(), true).unwrap();
    let first = f.archive();
    retire(&f.raw(), true).unwrap();
    let second = f.archive();
    assert_eq!(second["outbounds"][0]["_omitted_fields"], 1);
    assert_eq!(first, second);
}

#[test]
fn normal_level_removes_current_and_legacy_archive_even_when_source_is_missing() {
    for present in [true, false] {
        let f = Fixture::new();
        fs::write(f.kept(), "LEGACY_SECRET").unwrap();
        if present {
            f.put(&json!({"password":"NEW_SECRET"}));
        }
        retire(&f.raw(), false).unwrap();
        retire(&f.raw(), false).unwrap();
        f.assert_empty();
    }
}

#[test]
fn partial_write_failure_cleans_staging_raw_and_old_archive() {
    let f = Fixture::new();
    f.put(&json!({"password":"NEW_SECRET","outbounds":[{"type":"naive"}]}));
    fs::write(f.kept(), "OLD_SECRET").unwrap();
    let result = retire_with_io(
        &f.raw(),
        true,
        |file, bytes| {
            assert!(!f.raw().exists());
            assert!(!f.kept().exists());
            assert!(!String::from_utf8_lossy(bytes).contains("SECRET"));
            file.write_all(&bytes[..bytes.len() / 2])?;
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "synthetic disk failure",
            ))
        },
        |_, _| panic!("partial write must not publish"),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WriteZero);
    f.assert_empty();
}

#[test]
fn rename_failure_leaves_no_raw_archive_or_staging_file() {
    let f = Fixture::new();
    f.put(&json!({"password":"NEW_SECRET"}));
    fs::write(f.kept(), "OLD_SECRET").unwrap();
    let result = retire_with_io(
        &f.raw(),
        true,
        |file, bytes| file.write_all(bytes),
        |temp, kept| {
            assert!(!f.raw().exists());
            assert!(!kept.exists());
            assert!(!String::from_utf8(fs::read(temp)?)
                .unwrap()
                .contains("SECRET"));
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "synthetic rename failure",
            ))
        },
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    f.assert_empty();
}

#[test]
fn old_archive_removal_failure_still_retires_raw_and_does_not_publish() {
    let f = Fixture::new();
    f.put(&json!({"password":"NEW_SECRET"}));
    fs::create_dir(f.kept()).unwrap();
    assert!(retire(&f.raw(), true).is_err());
    assert!(!f.raw().exists());
    assert!(f.kept().is_dir()); // cannot claim erasure when the filesystem rejects it
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn stage_and_archive_start_private_and_publish_only_complete_sanitized_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.put(&json!({"password":"SECRET"}));
    retire_with_io(
        &f.raw(),
        true,
        |file, bytes| file.write_all(bytes),
        |temp, kept| {
            assert_eq!(fs::metadata(temp)?.permissions().mode() & 0o777, 0o600);
            assert!(!f.raw().exists());
            assert!(!kept.exists());
            let parsed: Value = serde_json::from_slice(&fs::read(temp)?).unwrap();
            assert_eq!(parsed["password"], REDACTED);
            fs::rename(temp, kept)
        },
    )
    .unwrap();
    assert_eq!(
        fs::metadata(f.kept()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn source_symlink_is_unlinked_without_reading_target() {
    let f = Fixture::new();
    let outside = f.0.join("untouched.json");
    fs::write(&outside, "SYNTHETIC_SECRET").unwrap();
    std::os::unix::fs::symlink(&outside, f.raw()).unwrap();
    assert_eq!(
        retire(&f.raw(), true).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(!f.raw().exists());
    assert!(!f.kept().exists());
    assert_eq!(fs::read_to_string(outside).unwrap(), "SYNTHETIC_SECRET");
}

#[cfg(unix)]
#[test]
fn source_read_failure_cleans_both_files_without_raw_fallback() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.put(&json!({"password":"SECRET"}));
    fs::set_permissions(f.raw(), fs::Permissions::from_mode(0)).unwrap();
    fs::write(f.kept(), "OLD_SECRET").unwrap();
    assert_eq!(
        retire(&f.raw(), true).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    f.assert_empty();
}

#[test]
fn oversized_input_is_rejected_and_removed_without_a_diagnostic_fallback() {
    let f = Fixture::new();
    File::create(f.raw())
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    fs::write(f.kept(), "LEGACY_SECRET").unwrap();
    assert_eq!(
        retire(&f.raw(), true).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    f.assert_empty();
}

#[test]
fn unremovable_source_prevents_archive_publication() {
    let f = Fixture::new();
    fs::create_dir(f.raw()).unwrap();
    fs::write(f.kept(), "LEGACY_SECRET").unwrap();
    assert!(retire(&f.raw(), true).is_err());
    assert!(f.raw().is_dir());
    assert!(!f.kept().exists());
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}
