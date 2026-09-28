use super::*;

#[test]
fn strict_normalization_preserves_family_and_masks_host_bits() {
    assert_eq!(
        normalize_cidr("10.20.1.9/16").as_deref(),
        Some("10.20.0.0/16")
    );
    assert_eq!(normalize_cidr("10.20.1.9").as_deref(), Some("10.20.1.9/32"));
    assert_eq!(
        normalize_cidr("FD7A:115C:A1E0:0::9/48").as_deref(),
        Some("fd7a:115c:a1e0::/48")
    );
    assert_eq!(
        normalize_cidr("::ffff:192.0.2.1/120").as_deref(),
        Some("::ffff:c000:200/120")
    );
    assert!(cidr_contains("::ffff:c000:200/120", "::ffff:c000:201/128"));
    assert!(!cidrs_overlap("::ffff:c000:200/120", "192.0.2.0/24"));
}

#[test]
fn strict_normalization_rejects_ambiguous_or_bad_input() {
    for input in [
        "010.0.0.1/8",
        "192.168.1.1%eth0/24",
        "fe80::1%eth0/64",
        "1.2.3.4/33",
        "::1/129",
        "1.2.3.4/24/1",
    ] {
        assert_eq!(normalize_cidr(input), None, "{input}");
    }
    assert_eq!(normalize_cidr("0.0.0.1/0").as_deref(), Some("0.0.0.0/0"));
    assert_eq!(normalize_cidr("fd7a::1/0").as_deref(), Some("::/0"));
}

#[test]
fn v4_overlap() {
    assert!(ipv4_cidrs_overlap("192.168.0.0/16", "192.168.1.0/24"));
    assert!(ipv4_cidrs_overlap("10.0.0.0/8", "10.1.2.0/24"));
    assert!(!ipv4_cidrs_overlap("10.0.0.0/8", "192.168.0.0/16"));
    assert!(ipv4_cidrs_overlap("0.0.0.0/0", "1.2.3.0/24")); // 全覆盖
}

#[test]
fn v4_contains() {
    assert!(cidr_contains("10.0.0.0/8", "10.1.2.0/24"));
    assert!(!cidr_contains("10.1.2.0/24", "10.0.0.0/8"));
    assert!(!cidr_contains("10.0.0.0/8", "192.168.0.0/16"));
}

#[test]
fn v4_subtract_basic() {
    // 10.0.0.0/8 ∖ 10.1.0.0/16 → 10.0.0.0/16 + 10.2.0.0/15...10.128.0.0/9
    let result = subtract_cidrs(&["10.0.0.0/8".to_string()], &["10.1.0.0/16".to_string()]);
    assert!(result.len() > 1);
    // 验证不含 10.1.0.0/16 且并集覆盖原 /8 减该段。
    assert!(!result.iter().any(|c| c == "10.1.0.0/16"));
}

#[test]
fn v4_subtract_equal() {
    let result = subtract_cidrs(&["10.0.0.0/8".to_string()], &["10.0.0.0/8".to_string()]);
    assert!(result.is_empty());
}

#[test]
fn v4_subtract_no_overlap() {
    let result = subtract_cidrs(&["10.0.0.0/8".to_string()], &["192.168.0.0/16".to_string()]);
    assert_eq!(result, vec!["10.0.0.0/8".to_string()]);
}

#[test]
fn v6_overlap() {
    assert!(ipv6_cidrs_overlap("fc00::/7", "fd00::/8"));
    assert!(ipv6_cidrs_overlap("2001:db8::/32", "2001:db8:1::/48"));
    assert!(!ipv6_cidrs_overlap("fc00::/7", "2001::/16"));
}

#[test]
fn v6_subtract() {
    let result = subtract_cidrs(&["fc00::/7".to_string()], &["fd00::/8".to_string()]);
    assert!(!result.is_empty());
}

#[test]
fn cross_family_disjoint() {
    assert!(!cidrs_overlap("10.0.0.0/8", "fc00::/7"));
}

#[test]
fn partition_by_overlap() {
    let cidrs = vec![
        "10.0.0.0/8".to_string(),
        "192.168.0.0/16".to_string(),
        "1.2.3.0/24".to_string(),
    ];
    let ranges = vec!["192.168.0.0/16".to_string()];
    let (overlapping, disjoint) = partition_cidrs_by_overlap(&cidrs, &ranges);
    assert_eq!(overlapping, vec!["192.168.0.0/16".to_string()]);
    assert_eq!(disjoint.len(), 2);
}
