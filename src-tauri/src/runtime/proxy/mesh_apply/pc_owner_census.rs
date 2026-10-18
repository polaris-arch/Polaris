//! F1 seals original producer membership. It issues no resource-terminal or birth permission.

use std::sync::{Arc, Mutex};

use super::owner_proof::{MainProducerView, ProducerCell};
use crate::runtime::config::StopRuntimeDomain;
use crate::runtime::proxy::ProxyRuntime;
use crate::runtime::tailscale_login_core::LoginProducerView;
use polaris_core_supervisor::config_gate::{AdmissionPause, CheckProducerView};

pub(in crate::runtime::proxy) struct PcPauseCustody {
    domain: Arc<StopRuntimeDomain>,
    creation_generation: u64,
    pause: AdmissionPause,
    main: Vec<Arc<ProducerCell>>,
    login: LoginProducerView,
    members: Mutex<Option<Arc<OriginalMembers>>>,
}

struct OriginalMembers {
    main: Vec<MainProducerView>,
    login: LoginProducerView,
    #[cfg(not(target_os = "android"))]
    temp: crate::runtime::speedtest::TempProducerView,
    check: CheckProducerView,
}

/// Cloning an observer preserves the original custody; losing it never removes the cutoff.
#[derive(Clone)]
pub(crate) struct PcPauseBorrow {
    custody: Arc<PcPauseCustody>,
}

pub(crate) struct SealedProducerMembership {
    custody: Arc<PcPauseCustody>,
    members: Arc<OriginalMembers>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TailObligation {
    MainDetachedLogTasks,
    TempDetachedLogTasks,
    TempConfigBestEffort,
    OriginalBirthInputMissing,
    RuntimeAuxiliaryOwnerUnknown,
}

impl ProxyRuntime {
    /// A new cutoff must match live generation. Retry/surrender use the original opaque borrow.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) async fn pause_pc_producers(
        &self,
        expected_generation: u64,
    ) -> Result<PcPauseBorrow, String> {
        // Stabilize the original Login registration window before the central cutoff.
        // The existing Login gate is acquired before any synchronous Main/Check lock.
        let login = self.mesh.pc_login_producer_view().await?;
        let mut starts = self
            .normal_start
            .lock()
            .map_err(|_| "Main producer custody poisoned")?;
        if starts.pause.is_some() {
            return Err("PC producers already paused; use the original borrow".into());
        }
        if self.core_generation() != expected_generation {
            return Err("pause generation superseded".into());
        }
        if *self
            .desktop_shutdown
            .lock()
            .map_err(|_| "Main shutdown admission poisoned")?
        {
            return Err("proxy is shutting down".into());
        }
        let pause = polaris_core_supervisor::config_gate::pause_check_producers()
            .map_err(|e| e.to_string())?;
        let custody = Arc::new(PcPauseCustody {
            domain: Arc::clone(&self.stop_domain),
            creation_generation: expected_generation,
            pause,
            main: starts.producer_refs(),
            login: login.into_view(),
            members: Mutex::new(None),
        });
        starts.pause = Some(Arc::clone(&custody));
        Ok(PcPauseBorrow { custody })
    }
}

impl PcPauseBorrow {
    fn original_owner(&self, runtime: &ProxyRuntime) -> Result<(), String> {
        let starts = runtime
            .normal_start
            .lock()
            .map_err(|_| "Main producer custody poisoned")?;
        if !Arc::ptr_eq(&runtime.stop_domain, &self.custody.domain)
            || !starts
                .pause
                .as_ref()
                .is_some_and(|custody| Arc::ptr_eq(custody, &self.custody))
        {
            return Err("foreign or surrendered PC pause".into());
        }
        self.custody.pause.is_current().map_err(|e| e.to_string())
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn original_members(&self, runtime: &ProxyRuntime) -> Result<Arc<OriginalMembers>, String> {
        self.original_owner(runtime)?;
        if let Some(members) = self
            .custody
            .members
            .lock()
            .map_err(|_| "pause membership poisoned")?
            .clone()
        {
            return Ok(members);
        }
        // The original Login cohort was captured while its gate covered the cutoff.
        // No synchronous lock crosses an await, and retry never recaptures a replacement.
        let main = self
            .custody
            .main
            .iter()
            .map(|producer| producer.census())
            .collect::<Result<Vec<_>, _>>()?;
        let login = self.custody.login.clone();
        let temp = crate::runtime::speedtest::pc_temp_producer_view()?;
        let check = self
            .custody
            .pause
            .check_members()
            .map_err(|e| e.to_string())?;
        self.original_owner(runtime)?;
        let mut original = self
            .custody
            .members
            .lock()
            .map_err(|_| "pause membership poisoned")?;
        Ok(Arc::clone(original.get_or_insert_with(|| {
            Arc::new(OriginalMembers {
                main,
                login,
                temp,
                check,
            })
        })))
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) fn seal(&self, runtime: &ProxyRuntime) -> Result<SealedProducerMembership, String> {
        let members = self.original_members(runtime)?;
        if runtime.core_generation() != self.custody.creation_generation {
            return Err("census generation superseded".into());
        }
        Ok(SealedProducerMembership {
            custody: Arc::clone(&self.custody),
            members,
        })
    }

    /// Abort returns admission to the same original owners, even after ordinary Stop advanced generation.
    /// It grants no freshness, NoOwner, StopLease, deletion or managed birth authority.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) fn surrender(&self, runtime: &ProxyRuntime) -> Result<(), String> {
        let members = self.original_members(runtime)?;
        {
            let slot = runtime
                .child
                .lock()
                .map_err(|_| "original Main Child custody poisoned")?;
            for main in &members.main {
                main.verify_surrender(&slot)?;
            }
        }
        members.login.verify_surrender()?;
        members.temp.verify_surrender()?;
        members
            .check
            .verify_original_custody()
            .map_err(|e| e.to_string())?;
        let mut starts = runtime
            .normal_start
            .lock()
            .map_err(|_| "Main producer custody poisoned")?;
        if !Arc::ptr_eq(&runtime.stop_domain, &self.custody.domain)
            || !starts
                .pause
                .as_ref()
                .is_some_and(|custody| Arc::ptr_eq(custody, &self.custody))
        {
            return Err("foreign or already surrendered PC pause".into());
        }
        if *runtime
            .desktop_shutdown
            .lock()
            .map_err(|_| "Main shutdown admission poisoned")?
        {
            return Err("proxy is shutting down".into());
        }
        self.custody.pause.surrender().map_err(|e| e.to_string())?;
        starts.pause = None;
        Ok(())
    }
}

impl SealedProducerMembership {
    pub(crate) fn member_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.members.main.len(),
            self.members.login.member_count(),
            {
                #[cfg(not(target_os = "android"))]
                {
                    self.members.temp.member_count()
                }
                #[cfg(target_os = "android")]
                {
                    0
                }
            },
            self.members.check.member_count(),
        )
    }

    pub(crate) fn main_native_phases(&self) -> Vec<(u64, Vec<u8>)> {
        self.members
            .main
            .iter()
            .map(|member| (member.queue_base(), member.phases()))
            .collect()
    }

    pub(crate) fn tail_obligations(&self) -> [TailObligation; 5] {
        [
            TailObligation::MainDetachedLogTasks,
            TailObligation::TempDetachedLogTasks,
            TailObligation::TempConfigBestEffort,
            TailObligation::OriginalBirthInputMissing,
            TailObligation::RuntimeAuxiliaryOwnerUnknown,
        ]
    }

    pub(crate) fn assert_fresh(&self, runtime: &ProxyRuntime) -> Result<(), String> {
        PcPauseBorrow {
            custody: Arc::clone(&self.custody),
        }
        .original_owner(runtime)?;
        if runtime.core_generation() != self.custody.creation_generation {
            return Err("census generation superseded".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
