use super::*;
use std::collections::HashMap;

/// 内存 FS mock：dir → 条目名列表。未注册的 dir → None。
struct MockFs {
    dirs: HashMap<PathBuf, Vec<String>>,
}

impl TailscaleStateFs for MockFs {
    fn read_dir_names(&self, dir: &Path) -> Option<Vec<String>> {
        self.dirs.get(dir).cloned()
    }
}

#[test]
fn state_dir_joins_user_data_tailscale_serverid() {
    let p = tailscale_state_dir(Path::new("/app/userdata"), "srv-1").unwrap();
    assert_eq!(p, PathBuf::from("/app/userdata/tailscale/srv-1"));
}

#[test]
fn state_exists_true_when_nonempty() {
    let dir = tailscale_state_dir(Path::new("/ud"), "s1").unwrap();
    let fs = MockFs {
        dirs: [(dir, vec!["tailscaled.state".to_string()])]
            .into_iter()
            .collect(),
    };
    assert!(state_exists(&fs, Path::new("/ud"), "s1"));
}

#[test]
fn state_exists_false_when_empty() {
    let dir = tailscale_state_dir(Path::new("/ud"), "s1").unwrap();
    let fs = MockFs {
        dirs: [(dir, vec![])].into_iter().collect(),
    };
    assert!(!state_exists(&fs, Path::new("/ud"), "s1"));
}

#[test]
fn state_exists_false_when_missing_or_read_fails() {
    let fs = MockFs {
        dirs: HashMap::new(),
    };
    // 目录缺失 → read_dir_names 返 None → false（失败安全）。
    assert!(!state_exists(&fs, Path::new("/ud"), "absent"));
}

#[test]
fn state_dir_rejects_non_portable_or_escaping_ids() {
    for id in [
        "",
        ".",
        "..",
        "../victim",
        "/tmp/victim",
        r"..\victim",
        r"C:\victim",
        r"\\server\share",
        "bad\0id",
    ] {
        assert!(
            tailscale_state_dir(Path::new("/ud"), id).is_err(),
            "must reject {id:?}"
        );
    }
    assert!(tailscale_state_dir(Path::new("/ud"), &"x".repeat(256)).is_err());
}

#[test]
fn invalid_id_never_reaches_the_filesystem_boundary() {
    let fs = MockFs {
        dirs: HashMap::new(),
    };
    assert!(!state_exists(&fs, Path::new("/ud"), "../victim"));
}

fn observed_retirement_fixture() -> (
    super::retirement::TailscaleStoreExport,
    Vec<super::retirement::ObservedTailscaleStoreRun>,
) {
    use super::retirement::{ObservedTailscaleStoreRun, ObservedTailscaleStoreScope};
    let observed: Vec<_> = [("a", "b", "first"), ("c", "d", "second")]
        .into_iter()
        .map(|(nonce, digest, tag)| ObservedTailscaleStoreRun {
            run_nonce: nonce.repeat(64),
            config_digest: digest.repeat(64),
            scopes: vec![ObservedTailscaleStoreScope {
                tag: tag.into(),
                state_directory: format!("/private/{tag}"),
                state_file: format!("/private/{tag}/tailscaled.state"),
            }],
        })
        .collect();
    let instances: Vec<_> = observed
        .iter()
        .map(|run| {
            let scope = &run.scopes[0];
            serde_json::json!({
                "runNonce": run.run_nonce, "configDigest": run.config_digest,
                "terminal": "NoStoreConstruction", "censusComplete": true,
                "nodes": [{"tag": scope.tag, "stateDirectory": scope.state_directory,
                    "stateFile": scope.state_file, "writerState": "NoStoreConstruction",
                    "stateFileState": "Missing", "stateFileRevision": "",
                    "profileState": "None", "profileFingerprint": ""}]
            })
        })
        .collect();
    let export = serde_json::from_value(serde_json::json!({
        "contractVersion": "polaris-ts-auth-writer-retirement-v1",
        "globalCleanupEvidence": "CleanupUnknown", "instances": instances
    }))
    .unwrap();
    (export, observed)
}

#[test]
fn observed_runs_bind_mixed_reload_digests_without_weakening_single_digest() {
    let (export, mut observed) = observed_retirement_fixture();
    assert!(export.validate(&"b".repeat(64), true).is_err());
    assert!(export.validate(&"d".repeat(64), true).is_err());
    export.validate_observed_runs(&observed, true).unwrap();
    observed.reverse();
    export.validate_observed_runs(&observed, true).unwrap();
}

#[test]
fn observed_runs_reject_missing_rebound_duplicate_and_extra_original_scopes() {
    let (export, observed) = observed_retirement_fixture();
    assert!(export.validate_observed_runs(&[], true).is_err());
    assert!(export.validate_observed_runs(&observed[..1], true).is_err());
    for mutation in [
        "nonce",
        "digest",
        "duplicate-run",
        "missing-scope",
        "extra-scope",
        "duplicate-scope",
        "path",
    ] {
        let mut changed = observed.clone();
        match mutation {
            "nonce" => changed[0].run_nonce = "e".repeat(64),
            "digest" => changed[0].config_digest = "e".repeat(64),
            "duplicate-run" => changed[1] = changed[0].clone(),
            "missing-scope" => changed[0].scopes.clear(),
            "extra-scope" => {
                let scope = changed[1].scopes[0].clone();
                changed[0].scopes.push(scope);
            }
            "duplicate-scope" => {
                let scope = changed[0].scopes[0].clone();
                changed[0].scopes.push(scope);
            }
            "path" => changed[0].scopes[0].state_directory.push_str("/other"),
            _ => unreachable!(),
        }
        assert!(
            export.validate_observed_runs(&changed, true).is_err(),
            "{mutation}"
        );
    }
}

#[test]
fn observed_runs_do_not_turn_active_or_empty_export_into_terminal_fact() {
    let (export, observed) = observed_retirement_fixture();
    let mut value = serde_json::json!({
        "contractVersion": "polaris-ts-auth-writer-retirement-v1",
        "globalCleanupEvidence": "CleanupUnknown", "instances": []
    });
    let empty: super::retirement::TailscaleStoreExport =
        serde_json::from_value(value.clone()).unwrap();
    assert!(empty.validate_observed_runs(&observed, true).is_err());
    value["instances"] = serde_json::json!([{
        "runNonce": observed[0].run_nonce, "configDigest": observed[0].config_digest,
        "terminal": "Unknown", "censusComplete": false, "nodes": []
    }]);
    let active: super::retirement::TailscaleStoreExport = serde_json::from_value(value).unwrap();
    let mut actual = observed[..1].to_vec();
    actual[0].scopes.clear();
    active.validate_observed_runs(&actual, false).unwrap();
    assert!(active.validate_observed_runs(&actual, true).is_err());
    export.validate_observed_runs(&observed, true).unwrap();
}
