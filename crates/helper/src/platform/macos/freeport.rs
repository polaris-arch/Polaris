//! Observe port holders without killing by image/name. A process can be stopped
//! only through the helper's retained exact native birth and caller ownership.
use crate::platform::macos::exec::{CommandRunner, RunError, EXEC_TIMEOUT};
use polaris_helper_proto::{Error as ProtoError, ErrorCode, FreePort, Response, ResponseKind};
pub const LSOF_BIN: &str = "/usr/sbin/lsof";
pub const PS_BIN: &str = "/bin/ps";
#[must_use]
pub fn is_valid_port_str(port: &str) -> bool {
    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
}
#[must_use]
pub fn parse_lsof_pids(raw: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(raw)
        .split_whitespace()
        .filter_map(|value| value.parse::<u32>().ok())
        .collect()
}
#[must_use]
pub fn bad_port_response() -> Response {
    Response::Err(ProtoError::new(ErrorCode::BadPort))
}
pub fn run_freeport(runner: &dyn CommandRunner, port: &str) -> Response {
    if !is_valid_port_str(port) {
        return bad_port_response();
    }
    let tcp = format!("tcp:{port}");
    let raw = match runner.output(EXEC_TIMEOUT, LSOF_BIN, &["-ti", &tcp, "-sTCP:LISTEN"]) {
        Ok(raw) => raw,
        Err(RunError::NonZero { code: 1, output })
            if output.stdout.is_empty() && output.stderr.is_empty() =>
        {
            Vec::new()
        }
        Err(_) => {
            return Response::Err(ProtoError::with_detail(
                ErrorCode::Enum,
                "port-holder-observation-failed",
            ))
        }
    };
    let Ok(text) = std::str::from_utf8(&raw) else {
        return Response::Err(ProtoError::new(ErrorCode::Enum));
    };
    if text
        .split_whitespace()
        .any(|pid| pid.parse::<std::num::NonZeroU32>().is_err())
    {
        return Response::Err(ProtoError::new(ErrorCode::Enum));
    }
    let pids = parse_lsof_pids(&raw);
    if pids.is_empty() {
        return Response::Ok(ResponseKind::FreePort(FreePort::Free));
    }
    let names = pids
        .into_iter()
        .map(|pid| {
            let raw = runner
                .output(
                    EXEC_TIMEOUT,
                    PS_BIN,
                    &["-o", "comm=", "-p", &pid.to_string()],
                )
                .unwrap_or_default();
            let text = String::from_utf8_lossy(&raw);
            let base = std::path::Path::new(text.trim())
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if base.is_empty() {
                format!("pid:{pid}")
            } else {
                base.replace(['\r', '\n', '\0', '|'], "_")
            }
        })
        .collect();
    Response::Ok(ResponseKind::FreePort(FreePort::Foreign { names }))
}
#[cfg(test)]
mod tests;
