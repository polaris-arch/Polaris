//! Generated only by the owned local producer after actual protected execution.
//! This compiler input is data, not a signature or a public authority constructor.
#[derive(Debug)]
pub struct DevelopmentSelection {
    pub source_commit: &'static str,
    pub source_tree: &'static str,
    pub producer_commit: &'static str,
    pub producer_tree: &'static str,
    pub target: &'static str,
    pub go_version: &'static str,
    pub tags: &'static str,
    pub core_bytes: u64,
    pub core_sha256: &'static str,
    pub sidecar_bytes: u64,
    pub sidecar_sha256: &'static str,
    pub selection_sha256: &'static str,
    pub contract_sha256: &'static str,
    pub purposes: &'static [&'static str],
}
#[rustfmt::skip]
pub const COMPILED:Option<DevelopmentSelection>=Some(DevelopmentSelection{source_commit:"c87159cc6a7ec82138132b2f24fd4e0be4d9d1fa",source_tree:"d3529920b636dd548d71be380a981f46e8fa739c",producer_commit:"c87159cc6a7ec82138132b2f24fd4e0be4d9d1fa",producer_tree:"d3529920b636dd548d71be380a981f46e8fa739c",target:"linux/amd64",go_version:"go1.27.2",tags:"with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0,with_purego",core_bytes:87724295,core_sha256:"addca74cf27dd51ff46ab54086b646eb38438b61baea7bd49c9efa80ffba1c7c",sidecar_bytes:12370520,sidecar_sha256:"99eb98eb47102c7418c7fed1e666d27e2bfb67c6b5517f7ae6b033748d657ed4",selection_sha256:"8a112ae99ff79fd078b6ba1e12a26c0e95d980a8e13298f01e54f0c8807df40b",contract_sha256:"4ace06dc13898f301b230b1eb1d908b5b8adbf2dc6fe3b2916e9d3fa4e0e6a6f",purposes:&["check-config-v1", "tls-inventory-v1", "run-main-v1", "run-login-v1", "run-speedtest-v1"]});
