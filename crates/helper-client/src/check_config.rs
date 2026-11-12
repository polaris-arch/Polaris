//! Fixed, OS-authenticated system Helper clients for native birth status.
use crate::HelperClient;
use polaris_helper_proto::{Platform, Request, Response, ResponseKind};
use std::time::Duration;

/// Each connection goes to the fixed installed Mac endpoint and authenticates its
/// actual kernel root peer before sending bytes. Generic wire clients/streams
/// cannot be converted into it.
///
/// ```compile_fail
/// use polaris_helper_client::HelperClient;
/// use polaris_helper_client::check_config::SystemMacCheckClient;
/// fn spoofed_same_key_wire_client_is_not_an_issuer(client: HelperClient) {
///     // Even a connector forwarding real Begin before fabricating Released is
///     // only a generic wire client; there is no production conversion.
///     let _: SystemMacCheckClient = client;
/// }
/// ```
#[cfg(target_os = "macos")]
pub struct SystemMacCheckClient {
    client: HelperClient,
}

#[cfg(target_os = "macos")]
impl SystemMacCheckClient {
    /// Lazy construction only. Token is authentication data, not a completion
    /// issuer; path/transport/peer identity are never supplied by the caller.
    pub fn system(token: String) -> Result<Self, String> {
        if token.is_empty()
            || token.len() > polaris_helper_proto::command::mac::MAX_WIRE_LINE_BYTES
            || token.contains(['\r', '\n'])
        {
            return Err("Root Helper token is not one bounded nonempty line".into());
        }
        Ok(Self {
            client: HelperClient::new(
                Box::new(crate::connector::MacRootConnector),
                Platform::Mac,
                token,
            ),
        })
    }

    pub fn native_birth_status(
        &self,
    ) -> Result<polaris_helper_proto::response::NativeBirthStatus, String> {
        match self
            .client
            .send_with_timeout(&Request::NativeStatusBirth, Duration::from_millis(1500))
            .map_err(|e| e.to_string())?
        {
            Response::Ok(ResponseKind::NativeBirthStatus(status)) => Ok(status),
            _ => Err("Closed Helper native status unavailable".into()),
        }
    }
}

/// Only this fixed, OS-authenticated system connector reaches the Windows Helper.
#[cfg(windows)]
pub struct SystemWindowsCheckClient {
    client: HelperClient,
}
#[cfg(windows)]
impl SystemWindowsCheckClient {
    pub fn system(token: String) -> Result<Self, String> {
        if token.is_empty() || token.len() > 256 * 1024 || token.contains(['\r', '\n']) {
            return Err("SYSTEM Helper token must be one bounded nonempty line".into());
        }
        Ok(Self {
            client: HelperClient::new(
                Box::new(crate::connector::WindowsSystemConnector),
                Platform::Win,
                token,
            ),
        })
    }
    pub fn native_birth_status(
        &self,
    ) -> Result<polaris_helper_proto::response::NativeBirthStatus, String> {
        match self
            .client
            .send_with_timeout(&Request::NativeStatusBirth, Duration::from_millis(1500))
            .map_err(|e| e.to_string())?
        {
            Response::Ok(ResponseKind::NativeBirthStatus(status)) => Ok(status),
            _ => Err("Closed Helper native status unavailable".into()),
        }
    }
}

#[cfg(test)]
mod tests;
