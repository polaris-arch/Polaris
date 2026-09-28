//! Host contract checks complement the real Registry close/timeout/STATUS behavioral tests.
const HOST: &str =
    include_str!("../gen/android/app/src/main/java/com/polaris2/app/vpn/TransientLoginHost.kt");
const NETWORK: &str =
    include_str!("../gen/android/app/src/main/java/com/polaris2/app/vpn/TransientLoginNetwork.kt");
const MAIN: &str =
    include_str!("../gen/android/app/src/main/java/com/polaris2/app/vpn/BoxService.kt");
const REGISTRY: &str = include_str!("../src/runtime/tailscale_login_core.rs");

#[test]
fn android_login_uses_instance_factory_and_status_not_global_command_socket() {
    assert!(HOST.contains("Libbox.newTransientCommandServer(LoginHandler(entry), network)"));
    assert!(!HOST.contains("server.start()"));
    assert!(HOST.contains("Libbox.hasTunInbound(config)"));
    assert!(!HOST.contains("VpnBridge."));
    assert!(REGISTRY.contains("Arc::new(AndroidLoginCoreSpawner)"));
    assert!(REGISTRY.contains("Arc::new(AndroidLoginConfigChecker)"));
}

#[test]
fn orphaned_transient_state_claim_is_closed_before_retry_start() {
    let predecessors = HOST
        .find("entries.values.filter { it !== entry && !it.disposed")
        .unwrap();
    let close = HOST[predecessors..]
        .find("check(dispose(previous) == null)")
        .unwrap()
        + predecessors;
    let start = HOST.find("Libbox.newTransientCommandServer").unwrap();
    assert!(predecessors < close && close < start);
    assert!(HOST.contains("failure?.let { it.message ?: \"Android 登录实例关闭失败\" }"));
    assert!(HOST.contains("server.closeService()"));
    assert!(HOST.contains("server.close()"));
}

#[test]
fn main_start_reload_and_stop_share_state_ownership_boundary() {
    assert_eq!(
        MAIN.matches("TransientLoginHost.withMainConfig(this, config")
            .count(),
        2
    );
    assert!(MAIN.contains("TransientLoginHost.closeMain(this)"));
    let reserve = HOST
        .find("mainClaims[owner] = mainClaims[owner].orEmpty() + directories")
        .unwrap();
    let stop = HOST.find("check(dispose(entry) == null)").unwrap();
    assert!(reserve < stop);
    assert!(HOST.contains("File(path).canonicalPath"));
    assert!(MAIN.contains("state == ServiceState.Started && commandServer === server"));
}

#[test]
fn transient_network_never_mutates_main_monitor_and_always_releases_own_thread() {
    assert!(!NETWORK.contains("DefaultNetworkMonitor."));
    assert!(NETWORK.contains("NET_CAPABILITY_NOT_VPN"));
    assert!(NETWORK.contains("connectivity.registerNetworkCallback(request, callback"));
    let finalizer = NETWORK.find("finally {").unwrap();
    assert!(NETWORK[finalizer..].contains("thread.quitSafely()"));
    assert!(NETWORK.contains("catch (_: IllegalArgumentException)"));
    assert!(NETWORK.contains("PolarisVpnService.protectTransientSocket(fd)"));
}

#[test]
fn per_attempt_cache_is_checked_and_removed_only_after_service_close() {
    let path_check = HOST.find("Android 登录缓存路径未隔离").unwrap();
    let start = HOST.find("Libbox.newTransientCommandServer").unwrap();
    let close = HOST.find("server.closeService()").unwrap();
    let delete = HOST.find("!it.exists() || it.delete()").unwrap();
    assert!(path_check < start && close < delete);
    assert!(HOST.contains("cache.parent in entry.stateDirectories"));
}

#[test]
fn first_login_creates_private_cache_parent_before_core_initialization() {
    let validate = HOST.find("Android 登录缓存路径未隔离").unwrap();
    let create = HOST
        .find("ensurePrivateDirectory(requireNotNull(cache.parentFile))")
        .unwrap();
    let start = HOST
        .find("server.startOrReloadService(config, OverrideOptions())")
        .unwrap();
    assert!(validate < create && create < start);
    assert!(HOST.contains("if (directory.isDirectory) return"));
    assert!(HOST.contains("Os.mkdir(directory.path, 448)"));
}
