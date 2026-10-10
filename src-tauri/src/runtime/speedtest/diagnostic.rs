//! Q6 diagnostic projection, not a runnable config or an arbitrary secret detector.
//! Source: ACC-01 preparation plan §3.4/Q6 (signed 2026-10-08). Preserve counts,
//! protocol types, selected modeled structure, numeric/boolean tuning and anonymous
//! tag links. Free text (addresses, URLs including query/userinfo/path, headers,
//! certificate/key material, commands) is withheld. Unknown keys AND values are
//! omitted with a count: custom pass-through is not a closed credential schema.
//! Only explicitly enumerated strings survive. Do not extend this to arbitrary
//! string passthrough without reviewing the corresponding config-engine shape.
//!
//! Failure/crash boundary: reproject a previous archive if raw source is missing (idempotent
//! retirement also scrubs legacy files); delete old archive on invalid input; project in memory, unlink raw input, then write only sanitized bytes
//! to a same-directory create_new file, sync and rename. Unix files start at 0600
//! and directory sync orders unlink/publication; Windows inherits the config
//! directory ACL (no new ACL guarantee). A crash BEFORE raw unlink may leave the
//! runtime config, as before, and V3-30 must observe it. A crash AFTER unlink can
//! lose diagnosis or leave a sanitized staging file. Unlink/permission failures
//! are reported and cannot guarantee erasure; no raw fallback is ever published.
//! Existing copies/hardlinks, malicious concurrent directory writers and secrets
//! deliberately encoded in allowed numeric/enumerated options are outside scope.

use super::TEMP_CORE_LAST_CONFIG_NAME;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const REDACTED: &str = "[REDACTED]";
const MAX_BYTES: u64 = 64 * 1024 * 1024;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "unsupported diagnostic input")
}

/// Deliberately conservative projection of config-engine's singbox structs and
/// endpoint flattened settings. Unknown custom extension names are not retained.
fn project(value: &Value, key: &str, tags: &mut BTreeMap<String, String>) -> Value {
    if matches!(
        key,
        "password"
            | "username"
            | "uuid"
            | "auth_str"
            | "auth_key"
            | "auth"
            | "cookie"
            | "token"
            | "private_key"
            | "private_key_path"
            | "private_key_passphrase"
            | "pre_shared_key"
            | "psk"
            | "userkey"
            | "user"
            | "public_key"
            | "short_id"
            | "server_public_key"
            | "server_disco_key"
            | "headers"
            | "http_headers"
            | "certificate"
            | "certificate_path"
            | "key"
            | "key_path"
            | "ca"
            | "ca_str"
            | "client_certificate"
            | "client_key"
            | "tls_auth"
            | "tls_crypt"
            | "extra_args"
            | "torrc"
            | "plugin_opts"
            | "config"
            | "certificate_sha256"
            | "certificate_public_key_sha256"
            | "host_key"
            | "reserved"
    ) {
        return REDACTED.into();
    }
    match value {
        Value::Object(object) => {
            let mut result = Map::new();
            let mut omitted = 0;
            for (name, child) in object {
                // Reserved output metadata never becomes a credential bypass.
                if name == "_diagnostic" {
                    continue;
                }
                if known_field(name) {
                    result.insert(name.clone(), project(child, name, tags));
                } else {
                    omitted += 1;
                }
            }
            if omitted > 0 {
                result.insert("_omitted_fields".into(), omitted.into());
            }
            Value::Object(result)
        }
        Value::Array(values) => {
            Value::Array(values.iter().map(|v| project(v, key, tags)).collect())
        }
        Value::String(text) => {
            if matches!(
                key,
                "tag"
                    | "inbound"
                    | "outbound"
                    | "detour"
                    | "default"
                    | "default_domain_resolver"
                    | "domain_resolver"
            ) {
                let next = format!("ref-{}", tags.len() + 1);
                tags.entry(text.clone()).or_insert(next).clone().into()
            } else if allowed_string(key, text) {
                text.clone().into()
            } else {
                REDACTED.into()
            }
        }
        // Only typed non-secret scalar options survive, including malformed
        // strings being redacted. Numeric credentials must not bypass redaction.
        Value::Number(_) | Value::Bool(_) if scalar_option(key) => value.clone(),
        Value::Null => Value::Null,
        _ => REDACTED.into(),
    }
}

fn allowed_string(key: &str, value: &str) -> bool {
    match key {
        "type" => matches!(
            value,
            "direct"
                | "block"
                | "dns"
                | "udp"
                | "tcp"
                | "tls"
                | "https"
                | "quic"
                | "http"
                | "mixed"
                | "socks"
                | "shadowsocks"
                | "vmess"
                | "vless"
                | "trojan"
                | "hysteria"
                | "hysteria2"
                | "tuic"
                | "naive"
                | "anytls"
                | "shadowtls"
                | "ssh"
                | "snell"
                | "tor"
                | "wireguard"
                | "masque"
                | "masque-client"
                | "openconnect"
                | "openvpn-client"
                | "tailcat"
                | "tailscale"
                | "ws"
                | "grpc"
                | "httpupgrade"
                | "salamander"
                | "gecko"
                | "selector"
                | "urltest"
                | "custom"
        ),
        "level" => matches!(
            value,
            "trace" | "debug" | "info" | "warn" | "error" | "fatal" | "panic"
        ),
        "action" => matches!(
            value,
            "route" | "reject" | "hijack-dns" | "resolve" | "sniff"
        ),
        "network" => matches!(value, "tcp" | "udp"),
        "strategy" => matches!(
            value,
            "prefer_ipv4" | "prefer_ipv6" | "ipv4_only" | "ipv6_only"
        ),
        "method" => matches!(
            value,
            "none"
                | "aes-128-gcm"
                | "aes-256-gcm"
                | "chacha20-ietf-poly1305"
                | "2022-blake3-aes-128-gcm"
                | "2022-blake3-aes-256-gcm"
                | "2022-blake3-chacha20-poly1305"
                | "GET"
                | "POST"
                | "CONNECT"
        ),
        "security" => matches!(
            value,
            "auto" | "none" | "zero" | "aes-128-gcm" | "chacha20-poly1305"
        ),
        "flow" => value == "xtls-rprx-vision",
        "packet_encoding" => matches!(value, "xudp" | "packetaddr"),
        "engine" => matches!(value, "go" | "cronet"),
        "congestion_control" => matches!(value, "cubic" | "new_reno" | "bbr"),
        "alpn" => matches!(value, "h2" | "http/1.1" | "h3"),
        _ => false,
    }
}

fn scalar_option(key: &str) -> bool {
    matches!(
        key,
        "timestamp"
            | "enabled"
            | "insecure"
            | "listen_port"
            | "server_port"
            | "port"
            | "alter_id"
            | "up_mbps"
            | "down_mbps"
            | "version"
            | "mtu"
            | "workers"
            | "system"
            | "system_interface"
            | "udp_fragment"
            | "quic"
            | "reuse"
            | "zero_rtt_handshake"
            | "disable_chrome_parrot"
            | "max_early_data"
            | "max_connections"
            | "min_streams"
            | "padding"
            | "fragment"
            | "min_idle_session"
            | "disable_cache"
            | "disable_expire"
            | "query_type"
            | "persistent_keepalive_interval"
            | "redirect_gateway"
            | "redirect_private"
            | "route_no_pull"
            | "on_demand"
            | "idle_timeout"
            | "keep_alive_period"
            | "stream_receive_window"
            | "connection_receive_window"
            | "max_concurrent_streams"
            | "initial_packet_size"
            | "disable_path_mtu_discovery"
            | "min_packet_size"
            | "max_packet_size"
            | "interrupt_exist_connections"
            | "_omitted_fields"
    )
}

fn known_field(key: &str) -> bool {
    scalar_option(key)
        || matches!(
            key,
            "log"
                | "level"
                | "dns"
                | "servers"
                | "rules"
                | "route"
                | "inbounds"
                | "outbounds"
                | "endpoints"
                | "type"
                | "tag"
                | "inbound"
                | "outbound"
                | "server"
                | "address"
                | "listen"
                | "detour"
                | "default"
                | "default_domain_resolver"
                | "domain_resolver"
                | "action"
                | "strategy"
                | "method"
                | "security"
                | "flow"
                | "packet_encoding"
                | "engine"
                | "network"
                | "tls"
                | "transport"
                | "multiplex"
                | "utls"
                | "reality"
                | "ech"
                | "obfs"
                | "peers"
                | "users"
                | "authentication"
                | "http_client"
                | "udp_over_tcp"
                | "username"
                | "password"
                | "uuid"
                | "auth_str"
                | "auth_key"
                | "auth"
                | "cookie"
                | "token"
                | "private_key"
                | "private_key_path"
                | "private_key_passphrase"
                | "pre_shared_key"
                | "psk"
                | "userkey"
                | "user"
                | "public_key"
                | "short_id"
                | "server_public_key"
                | "server_disco_key"
                | "headers"
                | "http_headers"
                | "certificate"
                | "certificate_path"
                | "key"
                | "key_path"
                | "ca"
                | "ca_str"
                | "client_certificate"
                | "client_key"
                | "tls_auth"
                | "tls_crypt"
                | "extra_args"
                | "torrc"
                | "plugin_opts"
                | "config"
                | "certificate_sha256"
                | "certificate_public_key_sha256"
                | "host_key"
                | "reserved"
                | "server_name"
                | "host"
                | "hostname"
                | "path"
                | "url"
                | "control_url"
                | "derp_map_url"
                | "service_name"
                | "early_data_header_name"
                | "executable_path"
                | "data_directory"
                | "state_directory"
                | "allowed_ips"
                | "server_ports"
                | "alpn"
                | "fingerprint"
                | "congestion_control"
                | "plugin"
                | "udp_timeout"
                | "heartbeat"
                | "hop_interval"
        )
}

fn sanitized_bytes(path: &Path) -> io::Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let input: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if !input.is_object() {
        return Err(invalid());
    }
    let mut result = project(&input, "", &mut BTreeMap::new());
    result["_diagnostic"] = json!({"format": 1, "sanitized": true, "runnable": false,
        "policy": "modeled-structure-free-text-withheld"});
    serde_json::to_vec_pretty(&result).map_err(|_| invalid())
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    // std has no Windows directory flush API; atomic visibility is guaranteed
    // by same-directory rename, power-loss durability is not claimed there.
    Ok(())
}

pub(super) fn retire(path: &Path, keep: bool) -> io::Result<()> {
    retire_with_io(
        path,
        keep,
        |file, bytes| file.write_all(bytes),
        |temporary, kept| fs::rename(temporary, kept),
    )
}

// Injection tests exercise the real unlink/write/sync path without a real core.
fn retire_with_io(
    path: &Path,
    keep: bool,
    write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let kept = path.with_file_name(TEMP_CORE_LAST_CONFIG_NAME);
    // Retirement can be called twice by the existing checker/Drop paths. If
    // raw input is absent, reproject the previous archive: a legacy raw archive
    // is scrubbed, and a safe one survives (no trusted marker shortcut).
    let source = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => &kept,
        _ => path,
    };
    let projected = keep.then(|| sanitized_bytes(source));
    let old = remove(&kept);
    let raw = remove(path); // attempt both removals on every failure path
    old.and(raw)?;
    let directory = path.parent().ok_or_else(invalid)?;
    sync_directory(directory)?;
    let bytes = match projected {
        None => return Ok(()),
        Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Some(result) => result?,
    };
    let temporary_path = directory.join(format!(
        ".speedtest-core.diagnostic.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let opened = options.open(&temporary_path)?;
    // Own cleanup only after create_new succeeds; a collision is not ours.
    let temporary = Staging(temporary_path);
    let mut file = opened; // drops before the pathname guard on error (Windows)
    write(&mut file, &bytes)?;
    file.sync_all()?;
    drop(file); // required before Windows rename
    publish(&temporary.0, &kept)?;
    sync_directory(directory)
}

#[cfg(test)]
#[path = "tests/diagnostic.rs"]
mod tests;
