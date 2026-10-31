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
pub const COMPILED:Option<DevelopmentSelection>=Some(DevelopmentSelection{source_commit:"402f855858cead86d2ded3134a3bc77dfb0e4e26",source_tree:"9acb2bdc6fd4ff7f74aee010a590c806b00da04b",producer_commit:"37ede2e4e2f8c599855a002f6b4579e73eba5d9e",producer_tree:"835276075e2fd2eabe0864fd3b590323bf1a8cb3",target:"linux/amd64",go_version:"go1.27.2",tags:"with_gvisor,with_quic,with_dhcp,with_wireguard,with_utls,with_acme,with_clash_api,with_tailscale,with_ccm,with_ocm,with_cloudflared,with_naive_outbound,with_usbip,with_openvpn,with_openconnect,badlinkname,tfogo_checklinkname0,with_purego",core_bytes:87585031,core_sha256:"ab13ed4f270574a6ab6e7a460a865522be6d4e54e677f448e3a99bc0808bf5da",sidecar_bytes:12370520,sidecar_sha256:"99eb98eb47102c7418c7fed1e666d27e2bfb67c6b5517f7ae6b033748d657ed4",selection_sha256:"c60e3b25efeec18d98d15c0405e62e08be09fdc86eb658f09a2198a2b659b0c8",contract_sha256:"9773e2d7626275e91c5ee1d5dcb9e8b1413600f36e01e87465babaf084c7d3a3",purposes:&["check-config-v1", "tls-inventory-v1", "run-main-v1", "run-login-v1", "run-speedtest-v1"]});
