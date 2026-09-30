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
    state.terminal.push_back(target);
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
    if state.terminal.contains(&requested) {
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
    pub(super) fn handle_native_stop(&self, target: HelperBirthTarget) -> HandleOutcome {
        let Ok(mut state) = self.child_mu.lock() else {
            return HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStop(
                NativeBirthStop::Unknown { target },
            )));
        };
        HandleOutcome::Respond(Response::Ok(ResponseKind::NativeBirthStop(stop_locked(
            &mut state,
            self.proc.as_ref(),
            target,
        ))))
    }

    pub(super) fn handle_native_status(&self) -> HandleOutcome {
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
            if let Some(native) = &state.native {
                let target = native.target;
                match self.proc.poll_native_child(target) {
                    NativeChildPoll::Exited
                        if cache_exit(&mut state, self.proc.as_ref(), target) => {}
                    NativeChildPoll::Exited | NativeChildPoll::Unknown => {
                        return start_response(NativeBirthStart::NotAdmittedUnknown {
                            target: Some(target),
                        })
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
            } else if !logic::cfg_allowed(&p.cfg, &self.conf_dir) {
                Some(polaris_helper_proto::ErrorCode::ConfigPathDenied)
            } else if !p.log.is_empty() && !logic::cfg_allowed(&p.log, &self.conf_dir) {
                Some(polaris_helper_proto::ErrorCode::LogPathDenied)
            } else {
                None
            };
            if let Some(code) = invalid {
                return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                    code,
                )));
            }
            if let Some(error) = self.core_acl_gate() {
                return HandleOutcome::Respond(Response::Err(error));
            }
            let birth = match self.proc.mint_native_birth() {
                Ok(birth) => birth,
                Err(error) => return start_error(error),
            };
            let started = match self.proc.start_native_singbox(
                &self.singbox_bin,
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
