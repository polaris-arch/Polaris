//! Windows dispatcher supplies the actual authenticated pipe peer, never an RPC SID.
use super::*;
impl<T: TokenStore, P: ProcOps + 'static, N: NetTableOps> WinHelper<T, P, N> {
    pub(super) fn handle_check_config(
        &self,
        request: &polaris_helper_proto::check_config::CheckRequest,
        peer: Option<&crate::user_control::NativePeer>,
    ) -> HandleOutcome {
        let Some(peer) = peer else {
            return HandleOutcome::AuthFailed;
        };
        #[cfg(not(windows))]
        {
            let _ = (request, peer);
            HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
                polaris_helper_proto::ErrorCode::Unknown,
                "helper-check-unsupported",
            )))
        }
        #[cfg(windows)]
        {
            use polaris_helper_proto::check_config::CheckRequest;
            let result = (|| -> Result<_, String> {
                if matches!(
                    request,
                    CheckRequest::Begin { .. }
                        | CheckRequest::Append { .. }
                        | CheckRequest::Commit { .. }
                ) && self
                    .child_mu
                    .lock()
                    .map_err(|_| "child state poisoned")?
                    .closing
                {
                    return Err("helper closing".into());
                }
                let _registered = self.registered_config(peer).map_err(|e| e.to_string())?;
                match request {
                    CheckRequest::Begin {
                        core,
                        bytes,
                        sha256,
                    } => {
                        let verified = self.verified_core(core).map_err(|e| e.to_string())?;
                        let execution = verified
                            .prepare(
                                polaris_core_supervisor::admitted_payload::ExecPurpose::CheckConfig,
                            )
                            .map_err(|e| e.to_string())?;
                        let lease = verified.execution_lease().map_err(|e| e.to_string())?;
                        polaris_core_supervisor::config_gate::begin_windows_helper_check(
                            execution,
                            lease,
                            peer.identity.clone(),
                            core.clone(),
                            *bytes,
                            *sha256,
                        )
                        .map_err(|e| e.to_string())
                    }
                    _ => polaris_core_supervisor::config_gate::windows_helper_check_request(
                        peer.identity.clone(),
                        request,
                    )
                    .map_err(|e| e.to_string()),
                }
            })();
            match result {
                Ok(reply) => {
                    HandleOutcome::Respond(Response::Ok(ResponseKind::WindowsCheck(reply)))
                }
                Err(e) => {
                    HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
                        polaris_helper_proto::ErrorCode::BadArgs,
                        e.replace(['\r', '\n'], " "),
                    )))
                }
            }
        }
    }
}
