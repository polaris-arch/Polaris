//! Production wiring omitted from host runtime tests by cfg(windows).

use polaris_source_probe::{crate_source, mask_comments_and_strings, repo_file};

fn compact(source: &str) -> String {
    mask_comments_and_strings(source)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

#[test]
fn native_session_window_is_installed_outside_the_webview_lifetime_and_uses_proxy_receipts() {
    let root = compact(&crate_source!("lib.rs"));
    assert!(root.contains("windows_session_end::install(app.handle())?"));
    assert!(root.contains("windows_session_end::destroy(app_handle)"));
    let source = compact(&crate_source!("windows_session_end.rs"));
    assert!(source.contains("CreateWindowExW("));
    assert!(!source.contains("HWND_MESSAGE"));
    let query = source
        .split("WM_QUERYENDSESSION=>")
        .nth(1)
        .unwrap()
        .split("WM_ENDSESSION=>")
        .next()
        .unwrap();
    for call in [
        "begin_windows_session_query()",
        "ShutdownBlockReasonCreate(",
        "crate::session_restore::wait(QUERY_BUDGET",
        "restore_system_proxy_for_exit().await",
        "crate::session_restore::complete(",
        "ShutdownBlockReasonDestroy(",
    ] {
        assert!(query.contains(call), "query wiring lost {call}");
    }
    for forbidden in [
        "prepare_desktop_exit(",
        "begin_shutdown(",
        "flush_dns(",
        "flush_os_dns_cache(",
        "app_lang(",
        "cancel_windows_session_query(",
        ".lock(",
    ] {
        assert!(
            !query.contains(forbidden),
            "query must remain reversible and cache-independent: {forbidden}"
        );
    }
    assert!(query.contains("ShutdownBlockReasonCreate(hwnd,context.reason.as_ptr())"));
    assert!(query.contains("failed(context.app.clone(),context.proxy.clone(),epoch)"));
    let install = source
        .split("fninstall(")
        .nth(1)
        .unwrap()
        .split("fndestroy(")
        .next()
        .unwrap();
    assert!(install.find("app_lang(app)").unwrap() < install.find("CreateWindowExW(").unwrap());
    assert!(source.contains("proxy.defer_windows_session_query_cancel(epoch,"));
    assert!(source.contains("ifwparam!=0"));
    assert!(source.contains("context.query.take()"));
    assert!(source.contains("proxy.stop_for_session_cancel().await"));
    assert!(source.contains("proxy.cancel_windows_session_query(epoch)"));
}

#[test]
fn both_windows_cache_producers_use_native_work_and_the_app_requests_the_new_capability() {
    let dns = compact(&repo_file!("crates/system-integration/src/dns_flush.rs"));
    assert!(dns.contains("crate::windows_dns::begin_flush()?.wait(timeout)"));
    let win = dns
        .split("Platform::Win=>")
        .nth(1)
        .unwrap()
        .split("Platform::Linux=>")
        .next()
        .unwrap();
    assert!(win.contains("exec.windows_flush(EXEC_TIMEOUT)"));
    assert!(!win.contains("exec.exec("));
    let helper = compact(&crate_source!("runtime/helper.rs"));
    assert!(helper.contains("letrequest=flush_dns_request(self.platform())"));
    assert!(helper.contains("send_with_timeout(&request,HELPER_FLUSH_TIMEOUT)"));
    assert!(helper.contains("Platform::Win=>Request::WindowsFlushDnsNative"));
    let native = compact(&repo_file!(
        "crates/helper/src/platform/windows/winproc/win.rs"
    ));
    let flush = native
        .split("fnflush_dns(")
        .nth(1)
        .unwrap()
        .split("fnenable_ip_forwarding(")
        .next()
        .unwrap();
    assert!(flush.contains("windows_dns::begin_flush"));
    assert!(!flush.contains("Command::") && !flush.contains(".spawn("));
}
