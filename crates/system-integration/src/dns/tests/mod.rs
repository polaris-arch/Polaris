use super::*;

#[test]
fn controlled_ip_excluded_from_bootstrap() {
    assert!(is_controlled_dns_ip_valid(CONTROLLED_TUN_DNS_IP));
    assert!(!is_controlled_dns_ip_valid("223.5.5.5"));
}

#[test]
fn parse_mac_dns_explicit_ips() {
    let out = parse_mac_get_dns_servers("192.168.1.1\n8.8.8.8\n");
    assert_eq!(out, vec!["192.168.1.1".to_string(), "8.8.8.8".to_string()]);
}

#[test]
fn parse_mac_dns_unset_returns_empty() {
    let out = parse_mac_get_dns_servers("There aren't any DNS Servers set on Wi-Fi.");
    assert!(out.is_empty());
}

#[test]
fn mac_set_dns_args_empty_is_dhcp() {
    assert_eq!(
        mac_set_dns_args("Wi-Fi", &[]),
        vec!["-setdnsservers".to_string(), "Wi-Fi".into(), "Empty".into()]
    );
    assert_eq!(
        mac_set_dns_args("Wi-Fi", &["8.8.8.8".into()]),
        vec![
            "-setdnsservers".to_string(),
            "Wi-Fi".into(),
            "8.8.8.8".into()
        ]
    );
}

#[test]
fn win_set_dns_dhcp_when_empty() {
    let cmds = win_set_dns_commands("netsh.exe", "Wi-Fi", &[]);
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].contains("source=dhcp"));
}

#[test]
fn win_set_dns_static_primary_then_add() {
    let cmds = win_set_dns_commands("netsh.exe", "Wi-Fi", &["8.8.8.8".into(), "8.8.4.4".into()]);
    assert_eq!(cmds.len(), 2);
    assert!(cmds[0].contains("static 8.8.8.8 primary"));
    assert!(cmds[1].contains("add dnsservers"));
    assert!(cmds[1].contains("address=8.8.4.4"));
    assert!(cmds[1].contains("index=2"));
}

#[test]
fn parse_win_show_dns_static_only() {
    // DHCP（自动）→ []
    assert!(parse_win_show_dns_servers(
        "Configuration for interface \"Wi-Fi\"\n    DNS configured through DHCP"
    )
    .is_empty());
    // 静态 → 提取
    let out = parse_win_show_dns_servers("Statically Configured DNS Servers: 8.8.8.8\n    8.8.4.4");
    assert_eq!(out, vec!["8.8.8.8".to_string(), "8.8.4.4".to_string()]);
}

#[test]
fn parse_win_interfaces_connected_non_loopback() {
    let stdout = "\nIdx     Met     MTU          State                Name\n---  ----------  ----------  ------------  ---------------------------\n  12          10        1500  connected     Wi-Fi\n  1           50        1500  connected     Loopback Pseudo-Interface 1\n  20          10        1500  disconnected  Ethernet\n";
    let ifaces = parse_win_interfaces(stdout);
    assert_eq!(ifaces, vec!["Wi-Fi".to_string()]);
}

#[test]
fn is_private_ipv4_checks() {
    assert!(is_private_ipv4("10.0.0.1"));
    assert!(is_private_ipv4("172.16.0.1"));
    assert!(is_private_ipv4("172.31.255.255"));
    assert!(is_private_ipv4("192.168.1.1"));
    assert!(!is_private_ipv4("172.32.0.1"));
    assert!(!is_private_ipv4("8.8.8.8"));
    assert!(!is_private_ipv4("not.an.ip.addr"));
}

#[test]
fn parse_scutil_nameservers_dedup_ordered() {
    let stdout =
        "nameserver[0] : 192.168.1.1\nnameserver[1] : 8.8.8.8\nnameserver[2] : 192.168.1.1\n";
    assert_eq!(
        parse_scutil_nameservers(stdout),
        vec!["192.168.1.1".to_string(), "8.8.8.8".to_string()]
    );
}

#[test]
fn extract_ipv4s_dedup() {
    let out = extract_ipv4s("8.8.8.8 and 1.2.3.4 and 8.8.8.8");
    assert_eq!(out, vec!["8.8.8.8".to_string(), "1.2.3.4".to_string()]);
}

#[test]
fn pick_lan_resolver_skips_controlled_and_public() {
    let cands = vec![
        "8.8.8.8".to_string(),
        "1.1.1.1".to_string(),
        "192.168.1.1".to_string(),
    ];
    assert_eq!(
        pick_lan_resolver_ip(&cands, "8.8.8.8"),
        Some("192.168.1.1".to_string())
    );
    // 全公网 → None
    assert!(
        pick_lan_resolver_ip(&["8.8.8.8".to_string(), "1.1.1.1".to_string()], "8.8.8.8").is_none()
    );
}

#[test]
fn compute_original_first_takeover_keeps_current() {
    let mut current = BTreeMap::new();
    current.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
    let out = compute_original_to_save(&current, "8.8.8.8", None);
    assert_eq!(out.get("Wi-Fi").unwrap(), &vec!["192.168.1.1".to_string()]);
}

#[test]
fn compute_original_retake_falls_back_to_marker_truth() {
    // 再次接管：当前已是受控 IP（我们设的）→ 回退 marker 里的真实原始。
    let mut current = BTreeMap::new();
    current.insert("Wi-Fi".into(), vec!["8.8.8.8".to_string()]);
    let mut existing = BTreeMap::new();
    existing.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
    let out = compute_original_to_save(&current, "8.8.8.8", Some(&existing));
    assert_eq!(out.get("Wi-Fi").unwrap(), &vec!["192.168.1.1".to_string()]);
}

#[test]
fn compute_original_mixed_controlled_and_real() {
    // Wi-Fi 已受控（回退 marker 真值），Ethernet 是新出现的真实 LAN（直接捕获）。
    let mut current = BTreeMap::new();
    current.insert("Wi-Fi".into(), vec!["8.8.8.8".to_string()]);
    current.insert("Ethernet".into(), vec!["10.0.0.1".to_string()]);
    let mut existing = BTreeMap::new();
    existing.insert("Wi-Fi".into(), vec!["192.168.1.1".to_string()]);
    let out = compute_original_to_save(&current, "8.8.8.8", Some(&existing));
    assert_eq!(out.get("Wi-Fi").unwrap(), &vec!["192.168.1.1".to_string()]);
    assert_eq!(out.get("Ethernet").unwrap(), &vec!["10.0.0.1".to_string()]);
}

#[test]
fn is_controlled_predicate() {
    assert!(is_controlled(&["8.8.8.8".to_string()], "8.8.8.8"));
    assert!(!is_controlled(
        &["8.8.8.8".to_string(), "1.1.1.1".to_string()],
        "8.8.8.8"
    ));
    assert!(!is_controlled(&["192.168.1.1".to_string()], "8.8.8.8"));
    assert!(!is_controlled(&[], "8.8.8.8"));
}

// ── 接管挤掉的非公网解析器（C1：让「MagicDNS 被覆盖」这件事可见）─────────────────

#[test]
fn displaced_resolver_predicate_covers_cgnat_and_ula() {
    let controlled = "198.18.0.2";

    // 本函数存在的理由：这两个是 `is_private_ipv4` **命不中**的。
    assert!(
        super::is_displaced_private_resolver("100.100.100.100", controlled),
        "Tailscale 的 quad100 必须命中 —— 它正是被静默覆盖的那个"
    );
    assert!(
        !super::is_private_ipv4("100.100.100.100"),
        "前提自检：quad100 确实不在 is_private_ipv4 的面里（若它变了，本函数的理由要重写）"
    );
    assert!(super::is_displaced_private_resolver(
        "fd7a:115c:a1e0::53",
        controlled
    ));
    assert!(!super::is_private_ipv4("fd7a:115c:a1e0::53"));

    // RFC1918 三族照收。
    for ip in ["10.0.0.53", "172.16.0.53", "192.168.1.1"] {
        assert!(super::is_displaced_private_resolver(ip, controlled), "{ip}");
    }

    // 排除面：公网 / 受控 IP / 回环 / 链路本地。
    for ip in [
        "8.8.8.8",
        "1.1.1.1",
        "223.5.5.5",
        controlled,
        "127.0.0.1",
        "::1",
        "169.254.1.1",
        "fe80::1",
        "",
    ] {
        assert!(
            !super::is_displaced_private_resolver(ip, controlled),
            "{ip} 不该命中 —— 覆盖公网解析器正是接管的本意，报出来只会成为被忽略的噪声"
        );
    }

    // CGNAT 边界：100.64 ~ 100.127 在内，100.63 / 100.128 在外。
    assert!(super::is_displaced_private_resolver(
        "100.64.0.1",
        controlled
    ));
    assert!(super::is_displaced_private_resolver(
        "100.127.255.254",
        controlled
    ));
    assert!(!super::is_displaced_private_resolver(
        "100.63.0.1",
        controlled
    ));
    assert!(!super::is_displaced_private_resolver(
        "100.128.0.1",
        controlled
    ));
}

#[test]
fn displaced_resolvers_dedupes_and_keeps_order() {
    let effective = vec![
        "192.168.1.1".to_string(),
        "8.8.8.8".to_string(),
        "100.100.100.100".to_string(),
        "192.168.1.1".to_string(),
        "198.18.0.2".to_string(),
    ];
    assert_eq!(
        super::displaced_private_resolvers(&effective, "198.18.0.2"),
        vec!["192.168.1.1".to_string(), "100.100.100.100".to_string()]
    );
}

/// 现场形态：macOS 上跑官方 Tailscale ⇒ 生效解析器里有 quad100，接管会把它挤掉。
#[test]
fn tailscale_quad100_is_reported_as_displaced() {
    let scutil = "resolver #1\n  nameserver[0] : 192.168.1.1\n  nameserver[1] : 8.8.8.8\n\
                  resolver #2\n  domain : example.ts.net\n  nameserver[0] : 100.100.100.100\n";
    let effective = super::parse_scutil_nameservers(scutil);
    let displaced = super::displaced_private_resolvers(&effective, "198.18.0.2");
    assert!(
        displaced.iter().any(|ip| ip == "100.100.100.100"),
        "quad100 没被报出来：{displaced:?}"
    );
}
