//! Read-only prerequisites for emitted Linux MAC-filtered routed-LAN TUNs.
//! LAN proxy listeners and neighbor-name resolution never imply router ownership.
use polaris_config_engine::singbox::SingBoxConfig;
use serde_json::Value;
use std::io::Read;
use std::net::IpAddr;

pub(super) const REQUIRED: &str = "LAN_GATEWAY_FORWARDING_REQUIRED";
pub(super) const UNCONFIRMED: &str = "LAN_GATEWAY_FORWARDING_UNCONFIRMED";

/// Early admission uses the same production inbound builder as final generation.
/// These omitted runtime dependencies affect exclusions/probe listeners only, not
/// Linux TUN MAC filters, auto-route/redirect, or the configured address families.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum UserPreflightError {
    InvalidConfig,
    Gateway(&'static str),
}

pub(super) fn check_user_config(config: &Value, platform: &str) -> Result<(), UserPreflightError> {
    // Schema validation is shared across hosts; a malformed config is not a
    // failed forwarding observation. Only emitted Linux MAC filters read host facts.
    let user =
        serde_json::from_value(config.clone()).map_err(|_| UserPreflightError::InvalidConfig)?;
    if platform != "linux" {
        return Ok(());
    }
    let emitted = user_inbounds(&user, platform)?;
    check_with(&emitted, platform, read_forwarding).map_err(UserPreflightError::Gateway)
}

fn user_inbounds(
    config: &polaris_config_engine::user_config::UserConfig,
    platform: &str,
) -> Result<Value, UserPreflightError> {
    use polaris_config_engine::builder::inbounds::{build_inbounds, InboundsDeps};
    let deps = InboundsDeps {
        probe_direct_port: None,
        probe_proxy_port: None,
        debug_probe_mixed_udp: false,
        update_in_port: None,
        subscription_update_in_port: None,
        probe_pool_ports: vec![],
        platform: platform.to_owned(),
        own_lan_cidrs: vec![],
        log: |_, _| {},
        observed_tailnet_addresses: Default::default(),
        loopback_auth: None,
    };
    let inbounds = build_inbounds(config, None, &deps);
    serde_json::to_value(inbounds)
        .map(|inbounds| serde_json::json!({"inbounds": inbounds}))
        .map_err(|_| UserPreflightError::Gateway(UNCONFIRMED))
}

fn read_forwarding(ipv6: bool) -> Result<String, ()> {
    let path = if ipv6 {
        "/proc/sys/net/ipv6/conf/all/forwarding"
    } else {
        "/proc/sys/net/ipv4/ip_forward"
    };
    let mut value = String::new();
    std::fs::File::open(path)
        .map_err(|_| ())?
        .take(8)
        .read_to_string(&mut value)
        .map_err(|_| ())?;
    Ok(value)
}

pub(super) fn check(config: &SingBoxConfig, platform: &str) -> Result<(), &'static str> {
    if platform != "linux" {
        return Ok(());
    }
    let value = serde_json::to_value(config).map_err(|_| UNCONFIRMED)?;
    check_with(&value, platform, read_forwarding)
}

fn check_with(
    config: &Value,
    platform: &str,
    mut read: impl FnMut(bool) -> Result<String, ()>,
) -> Result<(), &'static str> {
    if platform != "linux" {
        return Ok(());
    }
    let inbounds = config
        .get("inbounds")
        .and_then(Value::as_array)
        .ok_or(UNCONFIRMED)?;
    let mut families = [false, false];
    for inbound in inbounds {
        let mac_filter = ["include_mac_address", "exclude_mac_address"]
            .iter()
            .any(|field| {
                inbound
                    .get(field)
                    .and_then(Value::as_array)
                    .is_some_and(|rows| !rows.is_empty())
            });
        if inbound.get("type").and_then(Value::as_str) != Some("tun") || !mac_filter {
            continue;
        }
        if inbound.get("auto_route") != Some(&Value::Bool(true))
            || inbound.get("auto_redirect") != Some(&Value::Bool(true))
        {
            return Err(UNCONFIRMED);
        }
        let addresses = inbound
            .get("address")
            .and_then(Value::as_array)
            .filter(|rows| !rows.is_empty())
            .ok_or(UNCONFIRMED)?;
        for address in addresses {
            let (ip, prefix) = address
                .as_str()
                .and_then(|v| v.split_once('/'))
                .ok_or(UNCONFIRMED)?;
            let ip: IpAddr = ip.parse().map_err(|_| UNCONFIRMED)?;
            let prefix: u8 = prefix.parse().map_err(|_| UNCONFIRMED)?;
            let index = usize::from(ip.is_ipv6());
            if prefix > if index == 0 { 32 } else { 128 } {
                return Err(UNCONFIRMED);
            }
            families[index] = true;
        }
    }
    for (index, needed) in families.into_iter().enumerate() {
        if !needed {
            continue;
        }
        match read(index == 1).map_err(|_| UNCONFIRMED)?.trim() {
            "1" => (),
            "0" => return Err(REQUIRED),
            _ => return Err(UNCONFIRMED),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
