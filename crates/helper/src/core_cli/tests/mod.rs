use super::*;
#[test]
fn administrative_arguments_are_exact_and_manifest_is_part_of_stage() {
    let args = |values: &[&str]| {
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    };
    #[cfg(unix)]
    let valid = args(&[
        "core-install",
        "--root",
        "/root/core",
        "--source",
        "/user/stage",
        "--manifest",
        "/user/stage/core-manifest.json",
    ]);
    #[cfg(windows)]
    let valid = args(&[
        "core-install",
        "--root",
        r"C:\core",
        "--source",
        r"C:\stage",
        "--manifest",
        r"C:\stage\core-manifest.json",
    ]);
    assert!(InstallArgs::parse(&valid).is_ok());
    let mut duplicate = valid.clone();
    duplicate[3] = "--root".into();
    assert!(InstallArgs::parse(&duplicate).is_err());
    let mut unrelated = valid.clone();
    unrelated[6] = valid[2].clone();
    assert!(InstallArgs::parse(&unrelated).is_err());
    let mut extra = valid;
    extra.push("--daemon".into());
    assert!(InstallArgs::parse(&extra).is_err());
}
