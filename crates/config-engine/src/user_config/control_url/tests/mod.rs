use super::ControlUrlReject::*;
use super::*;

/// The selected Polaris constructor wraps the resolver independently of this host.
/// Corpus is also consumed by the frontend parity gate; these are source regressions,
/// not claims of a new native control-plane replay.
#[test]
fn ipv4_control_urls_allowed() {
    for url in [
        "http://192.168.1.10:8080",
        "http://192.168.1.10",
        "https://192.168.1.10:8080",
        "http://192.168.1.10:8080/key",
        "http://user:pw@192.168.1.10:8080",
        "http://127.0.0.1:39824",
        "https://127.0.0.1:39824",
        "http://0.0.0.0:8080",
        "https://203.0.113.9",
        "ws://192.168.1.10:8080",
    ] {
        assert_eq!(
            tailscale_control_url_reject(url),
            None,
            "规范 IPv4 被误拦: {url}"
        );
    }
}

#[test]
fn ipv6_control_urls_allowed() {
    for url in [
        "http://[fd7a:115c:a1e0::1]:8080",
        "http://[::1]",
        "http://[::1]:39824",
        "http://[::ffff:192.168.1.10]:8080",
        "http://[2001:db8:0:0:0:0:0:1]:8080",
        "http://[FD7A:115C::1]:8080",
    ] {
        assert_eq!(
            tailscale_control_url_reject(url),
            None,
            "规范 IPv6 被误拦: {url}"
        );
    }
}

/// Keep the old rejection boundary for noncanonical IP spelling and URI zones.
#[test]
fn unsupported_ip_spellings_rejected() {
    for url in [
        "http://192.168.001.010:8080",
        "http://[192.168.1.10]:8080",
        "http://[fe80::1%25eth0]:8080",
        "http://[fe80::1%eth0]:8080",
        "http://[::1::2]:8080",
        "http://[12345::1]:8080",
        "http://[1:2:3:4:5:6:7:8:9]:8080",
        "http://[::ffff:192.168.001.010]:8080",
    ] {
        assert_eq!(
            tailscale_control_url_reject(url),
            Some(IpLiteral),
            "不支持的 IP 写法漏拦: {url}"
        );
    }
}

#[test]
fn missing_scheme_rejected() {
    for url in [
        "hs.example.com",
        "not-a-url",
        "192.168.1.10:8080",
        "mailto:a@b.com",
        "//hs.example.com",
        "://hs.example.com",
        "1http://hs.example.com",
        "http_://hs.example.com",
    ] {
        assert_eq!(
            tailscale_control_url_reject(url),
            Some(MissingScheme),
            "缺少合法 scheme 漏拦: {url}"
        );
    }
}

#[test]
fn empty_host_rejected() {
    for url in ["http://", "http://:8080", "http:///path", "http://user:pw@"] {
        assert_eq!(
            tailscale_control_url_reject(url),
            Some(NoHost),
            "空 host 漏拦: {url}"
        );
    }
}

#[test]
fn malformed_host_rejected() {
    for url in [
        "http://192.168.1.10 :8080",
        "http://[::1",
        "http://fd7a::1",
        "http://[::g]:8080",
        "http://[::1]extra",
        "http://[::1]:",
        "http://[::1]:abc",
        "http://host:",
        "http://host:abc",
        "http://bad\\host",
    ] {
        assert_eq!(
            tailscale_control_url_reject(url),
            Some(Malformed),
            "畸形 host 漏拦: {url}"
        );
    }
}

/// Preserve the existing domain, scheme syntax, path and userinfo projection.
/// The identity ledger has a separate userinfo boundary, tested below.
#[test]
fn domain_forms_never_rejected() {
    for url in [
        "https://hs.example.com",
        "http://example.invalid",
        "https://headscale.local:8080",
        "http://localhost:8080",
        "http://localhost",
        "https://controlplane.tailscale.com",
        "https://hs.example.com.:8080",
        "http://1.2.3.4.5:8080",
        "http://12345",
        "HTTPS://HS.EXAMPLE.COM",
        "https://hs.example.com/key/path",
        "https://用户.example.com",
        "http://user:pw@hs.example.com:8080",
        "custom+v1://hs.example.com",
    ] {
        assert_eq!(
            tailscale_control_url_reject(url),
            None,
            "已有域名 URL 被误伤: {url}"
        );
    }
}

#[test]
fn empty_is_allowed() {
    for url in ["", "   ", "\t\n", "  https://hs.example.com  "] {
        assert_eq!(tailscale_control_url_reject(url), None);
    }
}

/// Exercise the actual backend identity-save authority validator, not a mirror.
#[test]
fn identity_authority_accepts_ip_without_relaxing_userinfo() {
    use crate::user_config::mesh_identity_reconcile::canonical_control_authority;
    for url in ["http://192.0.2.1:8080", "https://[2001:db8::1]:8443"] {
        assert_eq!(canonical_control_authority(url).as_deref(), Ok(url));
    }
    for url in [
        "http://user:pw@192.0.2.1:8080",
        "https://user@[2001:db8::1]",
    ] {
        assert!(canonical_control_authority(url).is_err());
    }
}

#[test]
fn tokens_are_stable() {
    assert_eq!(reject_token(IpLiteral), "control-url-ip");
    assert_eq!(reject_token(MissingScheme), "control-url-scheme");
    assert_eq!(reject_token(NoHost), "control-url-invalid");
    assert_eq!(reject_token(Malformed), "control-url-invalid");
}
