use super::*;
#[test]
fn emitted_lan_cidrs_use_strict_shared_parser() {
    let input: Vec<String> = [
        "999.1.1.1/24",
        "10.0.0.0/33",
        "2001:db8::/129",
        "*.example.com",
        "10.1.2.3/24",
        "2001:db8::1/64",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(bypass_lan_cidrs(&input), ["10.1.2.0/24", "2001:db8::/64"]);
}
