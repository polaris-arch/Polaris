use super::*;
fn argv(args: &[&str]) -> std::vec::IntoIter<String> {
    std::iter::once("polaris-helper".to_owned())
        .chain(args.iter().map(|value| (*value).to_owned()))
        .collect::<Vec<_>>()
        .into_iter()
}
#[test]
fn machine_support_and_console_parse_both_flag_forms() {
    let args = parse_args(argv(&["--console", r"--support=C:\ProgramData\Polaris"]));
    assert!(args.console);
    assert_eq!(args.support_dir, r"C:\ProgramData\Polaris");
    let args = parse_args(argv(&[]));
    assert_eq!(args.support_dir, super::super::DEFAULT_SUPPORT_DIR);
    assert!(!args.console);
}
#[test]
fn daemon_rejects_retired_flags_before_service_creation() {
    for flag in [
        r"--singbox=C:\sb.exe",
        r"--confdir=C:\Users\A",
        r"--coredir=C:\core",
    ] {
        assert_eq!(daemon_main(argv(&[flag])), std::process::ExitCode::FAILURE);
    }
}
