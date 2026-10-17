use super::super::*;
#[test]
fn deleted_and_edited_records_reject_stale_staged_bytes() {
    let dir =
        std::env::temp_dir().join(format!("polaris-resource-identity-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let staged = dir.join("stage");
    let downloaded = RuleResource {
        id: "A".into(),
        name: "A".into(),
        category: "geoip".into(),
        source_url: "https://example.invalid/new".into(),
        file_name: "A.srs".into(),
        format: RuleResourceFormat::Binary,
        size: 3,
        downloaded_at: "new".into(),
    };
    let mut original = downloaded.clone();
    original.source_url = "https://example.invalid/old".into();
    let expected = serde_json::to_value(&original).unwrap();
    let mut config = json!({ "ruleResources": [] });
    std::fs::write(&staged, b"new").unwrap();
    assert!(
        commit_download_resource(&mut config, Some(&expected), &downloaded, &staged, &dir).is_err()
    );
    assert!(config["ruleResources"].as_array().unwrap().is_empty());
    assert!(staged.exists());
    config["ruleResources"] = json!([downloaded]);
    assert!(
        commit_download_resource(&mut config, Some(&expected), &downloaded, &staged, &dir).is_err()
    );
    assert!(staged.exists());
    assert!(commit_download_resource(&mut config, None, &downloaded, &staged, &dir).is_err());
    config["ruleResources"] = json!([]);
    commit_download_resource(&mut config, None, &downloaded, &staged, &dir).unwrap();
    assert_eq!(std::fs::read(dir.join("A.srs")).unwrap(), b"new");
    assert_eq!(
        config["ruleResources"][0]["sourceUrl"],
        downloaded.source_url
    );
    std::fs::remove_dir_all(dir).unwrap();
}
