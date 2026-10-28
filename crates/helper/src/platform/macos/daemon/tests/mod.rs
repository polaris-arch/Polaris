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
