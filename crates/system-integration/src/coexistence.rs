//! COEX fact collection, separate from the legacy advisory route probe.
//!
//! This first slice supports Linux iproute2 JSON only. It does not schedule runtime
//! reprobes, notify, persist repair history, or project exits. Collection is sequential
//! and read-only; callers must offload it when running in an async context. Enumeration
//! is not atomic: no phase, own-interface attribution or stable identity is inferred.
//! Fixture tests validate parsing and classifier wiring, not real devices.

#![forbid(unsafe_code)]

use crate::exec::{Command, CommandRunner};
use polaris_config_engine::builder::coexistence::{
    classify, ClassificationInput, ClassificationReport, Fact, ObjectFacts, ObservationPhase,
};
use polaris_config_engine::builder::tunnel_conflict::ConflictCriteria;
use polaris_helper_proto::Platform;
use std::time::Duration;

mod linux;

/// Unknown enumeration must not be represented as an empty object list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxFacts {
    pub objects: Fact<Vec<ObjectFacts>>,
}

impl LinuxFacts {
    /// Caller supplies provenance that an OS query cannot establish. No implicit empty
    /// own-interface or history list, and no inferred before-TUN phase.
    pub fn classify(
        &self,
        phase: &Fact<ObservationPhase>,
        own_interfaces: &Fact<Vec<String>>,
        repair_history: &Fact<Vec<String>>,
        criteria: &ConflictCriteria,
    ) -> Fact<Vec<ClassificationReport>> {
        match &self.objects {
            Fact::Unknown(reason) => Fact::Unknown(reason.clone()),
            Fact::Known(objects) => Fact::Known(classify(&ClassificationInput {
                platform: Platform::Linux,
                observation_phase: phase,
                own_interfaces,
                objects,
                forced_repair_identities: repair_history,
                criteria,
            })),
        }
    }
}

/// Six bounded read-only queries, through the existing command seam. `-d` forces
/// main-table attribution into route JSON; `-N` requests numeric table identifiers.
/// Older/unsupported iproute2 or permissions failures remain Unknown; no fallback
/// to main-only text output. Sources: iproute2 ip/iproute.c, ip/iprule.c, ip/ipaddress.c.
pub fn collect_linux(runner: &impl CommandRunner) -> LinuxFacts {
    let read = |args: &[&str]| {
        runner
            .run(
                &Command::new("ip", args.iter().copied()),
                Duration::from_secs(2),
            )
            .map(|output| output.stdout)
    };
    let links = read(&["-j", "-d", "link", "show"]);
    let addresses = read(&["-j", "address", "show"]);
    let routes4 = read(&["-j", "-d", "-N", "-4", "route", "show", "table", "all"]);
    let routes6 = read(&["-j", "-d", "-N", "-6", "route", "show", "table", "all"]);
    let rules4 = read(&["-j", "-N", "-4", "rule", "show"]);
    let rules6 = read(&["-j", "-N", "-6", "rule", "show"]);
    LinuxFacts {
        objects: linux::assemble(links, addresses, routes4, routes6, rules4, rules6),
    }
}

#[cfg(test)]
mod tests;
