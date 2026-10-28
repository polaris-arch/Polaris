use super::*;
fn argv(args: &[&str]) -> std::vec::IntoIter<String> {
    std::iter::once("polaris-helper".to_owned())
        .chain(args.iter().map(|value| (*value).to_owned()))
        .collect::<Vec<_>>()
        .into_iter()
}
#[test]
fn machine_roots_parse_both_flag_forms() {
    let cfg = parse_args(argv(&[
        "--support",
        "/Library/Application Support/Polaris",
        "--coredir=/opt/polaris/core",
    ]));
    assert_eq!(cfg.support_dir, "/Library/Application Support/Polaris");
    assert_eq!(cfg.core_dir, "/opt/polaris/core");
    let cfg = parse_args(argv(&[]));
    assert_eq!(cfg.core_dir, format!("{DEFAULT_SUPPORT_DIR}/core"));
}
#[test]
fn daemon_rejects_retired_machine_user_and_flat_core_flags_without_serving() {
    for flag in [
        "--singbox=/opt/sb",
        "--confdir=/Users/A/Polaris",
        "--unknown=x",
    ] {
        assert_eq!(daemon_main(argv(&[flag])), std::process::ExitCode::FAILURE);
    }
}

#[test]
fn upgrade_closed_is_opt_in_and_a_presence_only_flag() {
    assert!(!MacConfig::new("/support", "/core").upgrade_closed);
    assert!(!parse_args(argv(&[])).upgrade_closed);
    let args: Vec<_> = argv(&["--upgrade-closed", "--support", "/support"]).collect();
    assert!(
        crate::cli::validate_flags(&args, &["support", "coredir"], &["upgrade-closed"]).is_ok()
    );
    let cfg = parse_args(args.into_iter());
    assert!(cfg.upgrade_closed);
    assert_eq!(cfg.support_dir, "/support");
}

#[test]
fn upgrade_closed_rejects_values_duplicates_and_near_matches_before_serving() {
    for args in [
        vec!["--upgrade-closed=true"],
        vec!["--upgrade-closed=false"],
        vec!["--upgrade-closed=1"],
        vec!["--upgrade-closed="],
        vec!["--upgrade-closed", "true"],
        vec!["--upgrade-closed", "false"],
        vec!["--upgrade-closed", "--upgrade-closed"],
        vec!["--upgrade-close"],
    ] {
        assert_eq!(daemon_main(argv(&args)), std::process::ExitCode::FAILURE);
    }
}
