use super::*;

fn argv(args: &[&str]) -> std::vec::IntoIter<String> {
    let mut v = vec!["polaris-helper".to_owned()];
    v.extend(args.iter().map(|s| (*s).to_owned()));
    v.into_iter()
}

#[test]
fn parse_args_all_flags() {
    let a = parse_args(argv(&[
        "--singbox",
        r"C:\sb.exe",
        "--confdir",
        r"C:\conf",
        "--support",
        r"C:\ProgramData\Polaris",
        "--console",
    ]));
    assert_eq!(a.singbox_bin, r"C:\sb.exe");
    assert_eq!(a.conf_dir, r"C:\conf");
    assert_eq!(a.support_dir, r"C:\ProgramData\Polaris");
    assert!(a.console);
}

/// `--coredir` 已删（P4）：它曾是「接受并忽略」的空壳，而受保护内核目录现在由 `--support` 派生。
/// 传了也只是被当成未知 flag 丢掉，**不得**再有字段承接它 —— 否则 SCM ImagePath 就又成了一条
/// 能改核路径的入口。这里正面钉住「其余 flag 照常解析」，反面钉住「coredir 不改变任何解析结果」。
#[test]
fn parse_args_ignores_removed_coredir_flag() {
    let with_flag = parse_args(argv(&[
        "--singbox",
        r"C:\sb.exe",
        "--support",
        r"C:\ProgramData\Polaris",
        "--coredir",
        r"C:\evil\core",
    ]));
    let without_flag = parse_args(argv(&[
        "--singbox",
        r"C:\sb.exe",
        "--support",
        r"C:\ProgramData\Polaris",
    ]));
    assert_eq!(with_flag.singbox_bin, r"C:\sb.exe");
    assert_eq!(with_flag.support_dir, r"C:\ProgramData\Polaris");
    assert_eq!(with_flag, without_flag, "--coredir 不得影响任何解析结果");
}

#[test]
fn parse_args_support_defaults_and_service_mode() {
    // 缺 --support → Go 默认（品牌改名 上游→Polaris）；缺 --console → SCM 服务模式。
    let a = parse_args(argv(&["--singbox", r"C:\sb.exe"]));
    assert_eq!(a.support_dir, super::super::DEFAULT_SUPPORT_DIR);
    assert!(!a.console);
    assert_eq!(a.conf_dir, "");
}

#[test]
fn parse_args_console_bool_does_not_consume() {
    let a = parse_args(argv(&["--console", "--singbox", r"C:\sb.exe"]));
    assert!(a.console);
    assert_eq!(a.singbox_bin, r"C:\sb.exe");
}
