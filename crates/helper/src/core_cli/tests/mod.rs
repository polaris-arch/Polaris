use super::*;
#[test]
fn administrative_arguments_are_exact_and_allow_a_root_sealed_manifest() {
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
    assert!(!InstallArgs::parse(&valid).unwrap().preserve_existing);
    let mut preserve = valid.clone();
    preserve.push("--preserve-existing".into());
    assert!(InstallArgs::parse(&preserve).unwrap().preserve_existing);
    preserve.push("--preserve-existing".into());
    assert!(InstallArgs::parse(&preserve).is_err());
    let mut valued = valid.clone();
    valued.push("--preserve-existing=true".into());
    assert!(InstallArgs::parse(&valued).is_err());
    let mut sealed = valid.clone();
    #[cfg(unix)]
    {
        sealed[6] = "/root/transaction/core-manifest.json".into();
    }
    #[cfg(windows)]
    {
        sealed[6] = r"C:\root-transaction\core-manifest.json".into();
    }
    assert!(InstallArgs::parse(&sealed).is_ok());
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
