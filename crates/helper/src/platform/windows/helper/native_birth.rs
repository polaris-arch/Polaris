//! Exact native object custody. Lock order: helper ChildState then ProcOps managed HANDLE.

use super::*;
use crate::platform::windows::ops::NativeChildPoll;
use polaris_helper_proto::{
    HelperBirthTarget, NativeBirthStart, NativeBirthStatus, NativeBirthStop,
};

#[derive(Debug, Clone)]
pub(super) struct NativeCustody {
    pub(super) target: HelperBirthTarget,
    stopping: bool,
    unknown: bool,
    pub(super) owner: Option<polaris_helper_proto::core_protocol::CallerIdentity>,
    core: Option<Arc<crate::core_store::VerifiedCore>>,
}

pub(super) fn cache_exit<P: ProcOps>(
    state: &mut ChildState,
    proc: &P,
    target: HelperBirthTarget,
) -> bool {
    if state.native.as_ref().is_none_or(|c| c.target != target) || !proc.retire_native_child(target)
    {
        return false;
    }
    let owner = state
        .native
        .as_ref()
        .and_then(|native| native.owner.clone());
    state.terminal.push_back((target, owner));
    if state.terminal.len() > 16 {
        state.terminal.pop_front();
    }
    state.native = None;
    state.pid = None;
    true
}

pub(super) fn stop_locked<P: ProcOps>(
    state: &mut ChildState,
    proc: &P,
    requested: HelperBirthTarget,
) -> NativeBirthStop {
    // A cached old receipt must never signal or clear a replacement birth.
    if state
        .terminal
        .iter()
        .any(|(target, _)| *target == requested)
    {
        return NativeBirthStop::Stopped { target: requested };
    }
    let Some(current) = &mut state.native else {
        return NativeBirthStop::Unknown { target: requested };
    };
    if current.target != requested {
        return NativeBirthStop::Mismatch {
            requested,
            current: current.target,
        };
    }
    current.stopping = true;
    match proc.stop_native_child(requested) {
        NativeChildPoll::Exited if cache_exit(state, proc, requested) => {
            NativeBirthStop::Stopped { target: requested }
        }
        NativeChildPoll::Exited | NativeChildPoll::Unknown => {
            state
                .native
                .as_mut()
                .expect("failed retirement retains custody")
                .unknown = true;
            NativeBirthStop::Unknown { target: requested }
        }
        NativeChildPoll::Running => NativeBirthStop::Pending { target: requested },
    }
}

impl<T: TokenStore, P: ProcOps + 'static, N: NetTableOps> WinHelper<T, P, N> {
    pub(super) fn handle_native_pinned(
        &self,
        params: &polaris_helper_proto::core_protocol::PinnedStartParams,
        peer: Option<&crate::user_control::NativePeer>,
    ) -> HandleOutcome {
        let Some(peer) = peer else {
            return HandleOutcome::AuthFailed;
        };
        let conf = match self.registered_config(peer) {
            Ok(conf) => conf,
            Err(_) => {
                return start_error(std::io::Error::other("registered user config unavailable"));
            }
        };
        let Some(conf) = conf.to_str() else {
            return start_error(std::io::Error::other("invalid registered config"));
        };
        let core = match self.verified_core(&params.id) {
            Ok(core) => core,
            Err(_) => return start_error(std::io::Error::other("core-invalid")),
        };
        let outcome = self.handle_native_start(&params.common, core, peer, conf);
        match outcome {
            HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(start))) => {
                HandleOutcome::Respond(Response::Ok(ResponseKind::PinnedStart {
                    id: params.id.clone(),
                    start: polaris_helper_proto::core_protocol::PinnedBirthStart::Native(start),
                }))
            }
            other => other,
        }
    }
    pub(super) fn handle_native_stop(
        &self,
        target: HelperBirthTarget,
        peer: &crate::user_control::NativePeer,
    ) -> HandleOutcome {
        let Ok(mut state) = self.child_mu.lock() else {
            return HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStop(
                NativeBirthStop::Unknown { target },
            )));
        };
        let owner = state
            .terminal
            .iter()
            .find(|(birth, _)| *birth == target)
            .map(|(_, owner)| owner.as_ref())
            .or_else(|| state.native.as_ref().map(|native| native.owner.as_ref()));
        if !peer.administrator && owner != Some(Some(&peer.identity)) {
            return HandleOutcome::AuthFailed;
        }
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStop(stop_locked(
            &mut state,
            self.proc.as_ref(),
            target,
        ))))
    }

    pub(super) fn handle_native_status(
        &self,
        peer: &crate::user_control::NativePeer,
    ) -> HandleOutcome {
        let mut state = match self.child_mu.lock() {
            Ok(state) => state,
            Err(poison) => {
                return HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStatus(
                    NativeBirthStatus::Unknown {
                        target: poison.get_ref().native.as_ref().map(|c| c.target),
                    },
                )));
            }
        };
        if !peer.administrator
            && state
                .native
                .as_ref()
                .is_some_and(|native| native.owner.as_ref() != Some(&peer.identity))
        {
            return HandleOutcome::AuthFailed;
        }
        let status = if let Some(native) = &state.native {
            let target = native.target;
            match self.proc.poll_native_child(target) {
                NativeChildPoll::Exited if cache_exit(&mut state, self.proc.as_ref(), target) => {
                    NativeBirthStatus::Empty
                }
                NativeChildPoll::Exited | NativeChildPoll::Unknown => {
                    state.native.as_mut().unwrap().unknown = true;
                    NativeBirthStatus::Unknown {
                        target: Some(target),
                    }
                }
                NativeChildPoll::Running => {
                    let native = state.native.as_ref().unwrap();
                    if native.unknown {
                        NativeBirthStatus::Unknown {
                            target: Some(target),
                        }
                    } else if native.stopping {
                        NativeBirthStatus::Stopping { target }
                    } else {
                        let identity = self.proc.managed_identity(target.pid.get());
                        NativeBirthStatus::Running {
                            target,
                            created: identity.created,
                            image: identity.image,
                        }
                    }
                }
            }
        } else if state.pid.is_some() {
            NativeBirthStatus::Unknown { target: None }
        } else if self.proc.native_custody_empty() {
            NativeBirthStatus::Empty
        } else {
            NativeBirthStatus::Unknown { target: None }
        };
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStatus(status)))
    }

    pub(super) fn handle_native_start(
        &self,
        p: &polaris_helper_proto::StartParams,
        core: Arc<crate::core_store::VerifiedCore>,
        peer: &crate::user_control::NativePeer,
        conf_dir: &str,
    ) -> HandleOutcome {
        let started = {
            let mut state = match self.child_mu.lock() {
                Ok(state) => state,
                Err(poison) => {
                    return start_response(NativeBirthStart::NotAdmittedUnknown {
                        target: poison.get_ref().native.as_ref().map(|c| c.target),
                    });
                }
            };
            if state.closing {
                return start_response(NativeBirthStart::NotAdmittedUnknown {
                    target: state.native.as_ref().map(|c| c.target),
                });
            }
            if !peer.administrator
                && state
                    .native
                    .as_ref()
                    .is_some_and(|native| native.owner.as_ref() != Some(&peer.identity))
            {
                return HandleOutcome::AuthFailed;
            }
            if let Some(native) = &state.native {
                if native
                    .core
                    .as_ref()
                    .is_none_or(|old| old.manifest.id != core.manifest.id)
                {
                    return start_error(std::io::Error::other("core-occupied"));
                }
                let target = native.target;
                match self.proc.poll_native_child(target) {
                    NativeChildPoll::Exited
                        if cache_exit(&mut state, self.proc.as_ref(), target) => {}
                    NativeChildPoll::Exited | NativeChildPoll::Unknown => {
                        return start_response(NativeBirthStart::NotAdmittedUnknown {
                            target: Some(target),
                        });
                    }
                    NativeChildPoll::Running => {
                        let current = state.native.as_ref().unwrap();
                        return start_response(if current.unknown {
                            NativeBirthStart::NotAdmittedUnknown {
                                target: Some(target),
                            }
                        } else if current.stopping {
                            NativeBirthStart::NotAdmittedPending { target }
                        } else {
                            NativeBirthStart::Already { target }
                        });
                    }
                }
            }
            if state.pid.is_some() {
                return start_response(NativeBirthStart::NotAdmittedUnknown { target: None });
            }
            if !self.proc.native_custody_empty() {
                return start_response(NativeBirthStart::NotAdmittedUnknown { target: None });
            }
            let invalid = if p.cfg.is_empty() {
                Some(polaris_helper_proto::ErrorCode::NoConfig)
            } else if !logic::cfg_allowed(&p.cfg, conf_dir) {
                Some(polaris_helper_proto::ErrorCode::ConfigPathDenied)
            } else if !p.log.is_empty() && !logic::cfg_allowed(&p.log, conf_dir) {
                Some(polaris_helper_proto::ErrorCode::LogPathDenied)
            } else {
                None
            };
            if let Some(code) = invalid {
                return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                    code,
                )));
            }
            let birth = match self.proc.mint_native_birth() {
                Ok(birth) => birth,
                Err(error) => return start_error(error),
            };
            let started = match self.proc.start_native_singbox(
                core.directory.join("sing-box.exe").to_str().unwrap_or(""),
                &p.cfg,
                &p.log,
                p.fwd,
                birth,
            ) {
                Ok(started) => started,
                Err(error) => return start_error(error),
            };
            let Some(pid) = std::num::NonZeroU32::new(started.pid) else {
                return start_response(NativeBirthStart::NotAdmittedUnknown { target: None });
            };
            let target = HelperBirthTarget { pid, birth };
            state.pid = Some(started.pid);
            state.native = Some(NativeCustody {
                target,
                stopping: false,
                unknown: false,
                owner: Some(peer.identity.clone()),
                core: Some(core),
            });
            (target, started)
        };
        let (target, info) = started;
        if let Some(ppid) = p.parent_pid.filter(|p| *p > 0) {
            let current = Arc::clone(&self.child_mu);
            let dead = Arc::clone(&self.child_mu);
            let proc = Arc::clone(&self.proc);
            self.proc.spawn_watch_parent(
                ppid,
                target.pid.get(),
                Box::new(move |_| {
                    current.lock().is_ok_and(|state| {
                        state.native.as_ref().is_some_and(|c| c.target == target)
                    })
                }),
                Box::new(move |_| {
                    if let Ok(mut state) = dead.lock() {
                        let _ = stop_locked(&mut state, proc.as_ref(), target);
                    }
                }),
            );
        }
        start_response(NativeBirthStart::Started {
            target,
            timing: Some(info.timing),
            created: info.created,
        })
    }
}

fn start_response(start: NativeBirthStart) -> HandleOutcome {
    HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStart(start)))
}
fn start_error(error: std::io::Error) -> HandleOutcome {
    HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
        polaris_helper_proto::ErrorCode::Start,
        error.to_string(),
    )))
}
