//! Read-only, privileged install-time probe. Never executes an installed daemon
//! with new CLI flags: legacy daemons may interpret unknown flags as serve mode.
use polaris_helper_client::{HelperClient, InstallPaths};
use polaris_helper_proto::{
    LinuxBirthStatus, NativeBirthStatus, Platform, Request, Response, ResponseKind,
};
use std::io::{self, Read};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Empty,
    Occupied,
    Unknown,
}
impl Slot {
    pub fn token(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Occupied => "occupied",
            Self::Unknown => "unknown",
        }
    }
}

pub fn classify(platform: Platform, response: &Response) -> Slot {
    match (platform, response) {
        (
            Platform::Linux,
            Response::Ok(ResponseKind::LinuxBirthStatus(LinuxBirthStatus::Empty)),
        )
        | (
            Platform::Mac | Platform::Win,
            Response::Ok(ResponseKind::NativeBirthStatus(NativeBirthStatus::Empty)),
        ) => Slot::Empty,
        (
            Platform::Linux,
            Response::Ok(ResponseKind::LinuxBirthStatus(
                LinuxBirthStatus::Running { .. } | LinuxBirthStatus::Stopping { .. },
            )),
        )
        | (
            Platform::Mac | Platform::Win,
            Response::Ok(ResponseKind::NativeBirthStatus(
                NativeBirthStatus::Running { .. } | NativeBirthStatus::Stopping { .. },
            )),
        ) => Slot::Occupied,
        _ => Slot::Unknown,
    }
}

fn legacy_token(support: &Path) -> io::Result<String> {
    let policy = crate::core_store::OwnerPolicy::privileged();
    policy.check(support)?;
    policy.check(&support.join("helper.token"))?;
    let objects = crate::core_objects::PayloadDir::open(support, policy.uid)?;
    let (file, bytes) = objects.open_regular("helper.token")?;
    if bytes > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy token too large",
        ));
    }
    let mut raw = String::new();
    file.take(129).read_to_string(&mut raw)?;
    Ok(raw)
}

fn client(platform: Platform, token: &str) -> io::Result<HelperClient> {
    let paths = InstallPaths::for_platform(platform);
    #[cfg(unix)]
    let connector: Box<dyn polaris_helper_client::Connector> =
        Box::new(polaris_helper_client::UnixConnector::new(paths.socket));
    #[cfg(windows)]
    let connector: Box<dyn polaris_helper_client::Connector> = Box::new(
        polaris_helper_client::PipeConnector::new(paths.socket.to_string_lossy().as_ref()),
    );
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (paths, token);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native helper unavailable",
        ));
    }
    #[cfg(any(unix, windows))]
    {
        Ok(HelperClient::new(connector, platform, token))
    }
}

fn authenticated(support: &Path) -> io::Result<(HelperClient, polaris_helper_proto::Pong)> {
    let platform = Platform::current();
    let mut transport = client(platform, "")?;
    let mut ping = transport.send_with_timeout(&Request::Ping, Duration::from_secs(5));
    if !matches!(ping, Ok(Response::Ok(ResponseKind::Pong(_))))
        && matches!(platform, Platform::Mac | Platform::Win)
    {
        transport = client(platform, &legacy_token(support)?)?;
        ping = transport.send_with_timeout(&Request::Ping, Duration::from_secs(5));
    }
    match ping {
        Ok(Response::Ok(ResponseKind::Pong(pong))) => Ok((transport, pong)),
        _ => Err(io::Error::other("native helper handshake unavailable")),
    }
}

/// Atomic close/empty admission. Legacy helpers reject; an empty snapshot is
/// deliberately insufficient because a start could race a service replacement.
pub fn upgrade_admission(support: &Path, close: bool) -> io::Result<()> {
    let (client, _) = authenticated(support)?;
    let request = if close {
        Request::PrepareHelperUpgrade
    } else {
        Request::CancelHelperUpgrade
    };
    let response = client
        .send_with_timeout(&request, Duration::from_secs(5))
        .map_err(|_| io::Error::other("upgrade admission unavailable"))?;
    let accepted = matches!(
        (close, response),
        (true, Response::Ok(ResponseKind::HelperUpgradeAdmitted))
            | (false, Response::Ok(ResponseKind::HelperUpgradeCancelled))
    );
    if accepted {
        Ok(())
    } else {
        Err(io::Error::other(
            "helper busy, unknown or legacy upgrade admission unsupported",
        ))
    }
}

pub fn probe(support: &Path) -> io::Result<String> {
    let platform = Platform::current();
    let (client, pong) = authenticated(support)?;
    let request = match platform {
        Platform::Linux => Request::LinuxStatusBirth,
        Platform::Mac | Platform::Win => Request::NativeStatusBirth,
        Platform::Other | Platform::Android | Platform::Ios => {
            return Err(io::Error::other("native helper unavailable"))
        }
    };
    let response = client
        .send_with_timeout(&request, Duration::from_secs(5))
        .map_err(|_| io::Error::other("native helper custody unavailable"))?;
    Ok(format!(
        "helper-slot {} proto={}",
        classify(platform, &response).token(),
        pong.proto_version
    ))
}

#[cfg(test)]
mod tests;
