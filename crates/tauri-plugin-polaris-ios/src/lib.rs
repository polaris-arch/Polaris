//! Native iOS VPN lifecycle. The core lives in the Packet Tunnel extension.

#[cfg(test)]
#[path = "../swift_package.rs"]
mod swift_package;

#[cfg(any(target_os = "ios", test))]
mod lifecycle_state;
#[cfg(target_os = "ios")]
mod native;
#[cfg(target_os = "ios")]
pub use native::*;
