use super::*;

// wire 命令名是已部署 helper 的硬约束 —— 改名 = 协议破坏。本测锁住移植正确性（逐字对照 Go 源）。
#[test]
fn common_command_names_match_polaris_go_source() {
    // mac helper/helper.go:421-507；win helper-win/helper.go:177-338；linux helper-linux/helper.go:345-499
    assert_eq!(common::PING, "ping");
    assert_eq!(common::VERSION, "version");
    assert_eq!(common::START, "start");
    assert_eq!(common::STOP, "stop");
    assert_eq!(common::STATUS, "status");
    assert_eq!(common::CLEANUP, "cleanup");
    assert_eq!(common::FREEPORT, "freeport");
    assert_eq!(common::ROUTE_ADD, "route-add");
    assert_eq!(common::ROUTE_DEL, "route-del");
}

#[test]
fn mac_specific_commands_match_polaris_go_source() {
    // helper/helper.go:481-585（default-restore v8、flush-dns v9、install-core v5）
    assert_eq!(mac::INSTALL_CORE, "install-core");
    assert_eq!(mac::DEFAULT_RESTORE, "default-restore");
    assert_eq!(mac::FLUSH_DNS, "flush-dns");
    assert_eq!(mac::SYSTEM_PROXY_TRANSACTION, "system-proxy-transaction");
    assert_eq!(mac::MAX_WIRE_LINE_BYTES, 256 * 1024);
    assert_eq!(
        mac::SYSTEM_PROXY_COMPARE_TRANSACTION,
        "system-proxy-compare-transaction"
    );
    assert_eq!(
        mac::SYSTEM_PROXY_COMPARE_CAPABILITY,
        "system-proxy-compare-capability"
    );
}

#[test]
fn win_specific_commands_match_polaris_go_source() {
    // helper-win/helper.go:242-294（iface-metric v3-v5 退役保留、uninstall）
    assert_eq!(win::UNINSTALL, "uninstall");
    assert_eq!(win::IFACE_METRIC, "iface-metric");
}

#[test]
fn linux_specific_commands_are_pinned() {
    // 旧命令对照 helper-linux/helper.go；reap-safe 是 Rust helper 的原子能力门。
    assert_eq!(linux::INSTALL_CORE, "install-core");
    assert_eq!(linux::RESOLVED_DNS_SET, "resolved-dns-set");
    assert_eq!(linux::RESOLVED_DNS_REVERT, "resolved-dns-revert");
    assert_eq!(linux::START_REAP_SAFE, "start-reap-safe");
    assert_eq!(linux::STOP_REAP_SAFE, "stop-reap-safe");
    assert_ne!(linux::START_REAP_SAFE, common::START);
    assert_ne!(linux::STOP_REAP_SAFE, common::STOP);
    assert_eq!(linux::START_BIRTH_SAFE, "start-birth-safe");
    assert_eq!(linux::STATUS_BIRTH_SAFE, "status-birth-safe");
    assert_eq!(linux::STOP_BIRTH_SAFE, "stop-birth-safe");
    assert_ne!(linux::START_BIRTH_SAFE, linux::START_REAP_SAFE);
    assert_ne!(linux::STOP_BIRTH_SAFE, linux::STOP_REAP_SAFE);
}
