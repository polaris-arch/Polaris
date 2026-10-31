use super::*;
fn p(value: &str) -> PathBuf {
    PathBuf::from(value)
}

#[test]
fn attest_same_path_passes_without_versions() {
    let a = p("/x/sing-box");
    // 版本双空也判通过：同一个文件，版本无需再问。
    assert_eq!(
        attest_core_binary(&a, Some(&a), "", ""),
        CoreBinaryAttestation::SamePath
    );
}

#[test]
fn attest_same_version_across_paths_passes() {
    let got = attest_core_binary(
        &p("/u/core_update/sing-box"),
        Some(&p("/Library/Application Support/Polaris/core/sing-box")),
        "sing-box version 1.14.0-beta.3",
        "sing-box version 1.14.0-beta.3",
    );
    assert!(!got.is_alarm());
    assert!(matches!(got, CoreBinaryAttestation::SameVersion { .. }));
}

#[test]
fn attest_fixture_version_mismatch_is_alarm() {
    let got = attest_core_binary(
        &p("/Users/fixture-user/Library/Application Support/com.polaris.app/polaris/core_update/sing-box"),
        Some(&p("/Library/Application Support/Polaris/core/sing-box")),
        "sing-box version 1.14.0-beta.3",
        "sing-box version 1.14.0-alpha.45",
    );
    assert!(
        got.is_alarm(),
        "实跑 alpha.45 而期望 beta.3 必须告警，实得 {got:?}"
    );
    assert!(matches!(got, CoreBinaryAttestation::VersionMismatch { .. }));
    let msg = got.user_message();
    assert!(msg.contains("1.14.0-alpha.45") && msg.contains("1.14.0-beta.3"));
}

#[test]
fn attest_unreadable_version_is_alarm_not_pass() {
    for (rv, ev) in [("", "1.0.0"), ("1.0.0", ""), ("", "")] {
        let got = attest_core_binary(&p("/a/sing-box"), Some(&p("/b/sing-box")), ev, rv);
        assert!(
            got.is_alarm(),
            "读不出版本不得判通过（rv={rv:?} ev={ev:?}），实得 {got:?}"
        );
    }
}

#[test]
fn attest_unobservable_is_neither_pass_nor_alarm() {
    let got = attest_core_binary(&p("/a/sing-box"), None, "1.0.0", "1.0.0");
    assert_eq!(got, CoreBinaryAttestation::Unobservable);
    assert!(!got.is_alarm());
    assert!(
        !got.user_message().contains("通过"),
        "「没观测到」绝不能写成「通过」，实得 {}",
        got.user_message()
    );
}

#[test]
fn helper_start_requires_installed_id_before_ipc_and_has_no_content_write_rpc() {
    let helper = crate::test_support::crate_source("runtime/helper.rs");
    let proxy = crate::test_support::module_code("runtime/proxy");
    let body =
        crate::test_support::method_body(&proxy, "    pub(super) async fn spawn_core_via_helper(");
    assert!(body.find("require_selected_core()").unwrap() < body.find("start_core(").unwrap());
    assert!(!helper.contains("Request::InstallCore"));
    assert!(!proxy.contains("reconcile_protected_core"));
}

#[cfg(target_os = "linux")]
mod loaded_image_regressions;
