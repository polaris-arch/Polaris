//! Explicit, read-only COEX diagnostics. No lifecycle publication or classification.
//!
//! Linux and macOS have production collectors here. Missing session provenance stays
//! Unknown; this entry does not replace the legacy advisory report. Source limits
//! apply after command output returns, not to pipe allocation or total elapsed time.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

use polaris_config_engine::builder::coexistence::{
    AddressFamily, Fact, ObjectFacts, PolicySelectorScope, RouteRole, RouteScope,
};
use polaris_helper_proto::Platform;
use polaris_system_integration::coexistence::{collect_linux, macos::collect_macos};
use polaris_system_integration::exec::{
    Command, CommandCleanup, CommandOutput, CommandRunner, PendingCommand, StdCommandRunner,
};
use serde_json::{json, Value};

use crate::response::ApiResponse;
use polaris_system_integration::coexistence::windows::*;

// This is a cleanup observation allowance, not a snapshot deadline.
const CLEANUP_OBSERVATION: Duration = Duration::from_millis(25);

/// Private ownership seam. Tests inject ownership outcomes, never native proof.
trait CommandCustody: Send {
    fn poll_cleanup(&mut self, budget: Duration) -> CommandCleanup;
    fn operation(&self) -> Option<Result<CommandOutput, String>>;
}

impl CommandCustody for PendingCommand {
    fn poll_cleanup(&mut self, budget: Duration) -> CommandCleanup {
        PendingCommand::poll_cleanup(self, budget)
    }

    fn operation(&self) -> Option<Result<CommandOutput, String>> {
        PendingCommand::operation(self)
    }
}

enum StartedCommand {
    NotSpawned(String),
    OwnershipUnavailable(String),
    Owned(Box<dyn CommandCustody>),
}

trait ObservationSource: Send + 'static {
    fn start(&self, command: &Command, timeout: Duration) -> StartedCommand;
    fn windows_facts(&self) -> WindowsFactInput {
        windows_unavailable("Windows source not supplied")
    }
}

struct NativeSource;

impl ObservationSource for NativeSource {
    fn windows_facts(&self) -> WindowsFactInput {
        collect_windows(&polaris_system_integration::windows_coex::NativeWindowsSource)
    }
    fn start(&self, command: &Command, timeout: Duration) -> StartedCommand {
        let observed = StdCommandRunner.run_observed(command, timeout);
        // An original PendingCommand is transferred intact, not reconstructed from
        // a PID, error string, mock acknowledgement, or the legacy run result.
        if observed.cleanup() == CommandCleanup::NotSpawned {
            return StartedCommand::NotSpawned(match observed.operation() {
                Some(Err(error)) => error,
                _ => "command spawn result unavailable".into(),
            });
        }
        match observed.into_pending() {
            Some(pending) => StartedCommand::Owned(Box::new(pending)),
            None => {
                StartedCommand::OwnershipUnavailable("original command custody unavailable".into())
            }
        }
    }
}

#[derive(Default)]
struct SnapshotSlot {
    busy: bool,
    // This slot outlives an IPC waiter and a completed blocking task. Only an
    // observed NativeAndReadersReturned outcome allows removal of an owner.
    pending: Option<Box<dyn CommandCustody>>,
    quarantine: Option<String>,
}

#[derive(Default)]
struct SnapshotService {
    slot: Mutex<SnapshotSlot>,
}

impl SnapshotService {
    fn lock_worker(&self) -> MutexGuard<'_, SnapshotSlot> {
        match self.slot.lock() {
            Ok(slot) => slot,
            Err(error) => {
                let mut slot = error.into_inner();
                slot.quarantine = Some("snapshot ownership lock poisoned".into());
                slot
            }
        }
    }

    // Never wait on a blocking worker's mutex in the async command path.
    fn admit(self: &Arc<Self>) -> Result<Admission, String> {
        let mut slot = match self.slot.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Err("snapshot worker busy".into()),
            Err(TryLockError::Poisoned(error)) => {
                let mut slot = error.into_inner();
                slot.quarantine = Some("snapshot ownership lock poisoned".into());
                return Err("snapshot ownership lock poisoned".into());
            }
        };
        if let Some(reason) = &slot.quarantine {
            return Err(reason.clone());
        }
        if slot.busy {
            return Err("snapshot worker busy".into());
        }
        slot.busy = true;
        Ok(Admission {
            service: Arc::clone(self),
            recorded: false,
        })
    }

    fn observe_retained(&self) -> Result<(), String> {
        let mut slot = self.lock_worker();
        if let Some(reason) = &slot.quarantine {
            return Err(reason.clone());
        }
        let Some(owner) = slot.pending.as_mut() else {
            return Ok(());
        };
        if owner.poll_cleanup(CLEANUP_OBSERVATION) == CommandCleanup::NativeAndReadersReturned {
            // Both original readers and original child were observed returned.
            slot.pending = None;
            Ok(())
        } else {
            Err("original snapshot command cleanup remains Unknown".into())
        }
    }

    fn run_owned(
        &self,
        source: &impl ObservationSource,
        command: &Command,
        timeout: Duration,
    ) -> Result<CommandOutput, String> {
        {
            let slot = self.lock_worker();
            if let Some(reason) = &slot.quarantine {
                return Err(reason.clone());
            }
            if slot.pending.is_some() {
                return Err("query skipped: original snapshot command cleanup Unknown".into());
            }
        }
        let owner = match source.start(command, timeout) {
            StartedCommand::NotSpawned(error) => return Err(error),
            StartedCommand::OwnershipUnavailable(error) => {
                self.lock_worker().quarantine = Some(error.clone());
                return Err(error);
            }
            StartedCommand::Owned(owner) => owner,
        };
        let mut slot = self.lock_worker();
        // Store before polling/reading operation results. Any later unwind retains
        // the owner in this process-lived service, even if it poisons the mutex.
        slot.pending = Some(owner);
        if slot.quarantine.is_some() {
            return Err("snapshot ownership lock poisoned; original command retained".into());
        }
        let owner = slot.pending.as_mut().expect("owner was just stored");
        let cleanup = owner.poll_cleanup(CLEANUP_OBSERVATION);
        let operation = owner.operation();
        if cleanup != CommandCleanup::NativeAndReadersReturned {
            let detail = match operation {
                Some(Err(error)) => format!(": {error}"),
                _ => String::new(),
            };
            return Err(format!("original snapshot command cleanup Unknown{detail}"));
        }
        let result = operation.unwrap_or_else(|| Err("command output unavailable".into()));
        slot.pending = None;
        result
    }
}

struct Admission {
    service: Arc<SnapshotService>,
    recorded: bool,
}

impl Admission {
    fn cleanup_outcome(&self) -> Fact<&'static str> {
        let slot = self.service.lock_worker();
        if let Some(reason) = &slot.quarantine {
            Fact::Unknown(reason.clone())
        } else if slot.pending.is_some() {
            Fact::Unknown("original snapshot command retained for cleanup observation".into())
        } else {
            // Limited to this service's original commands, not host NoOwner.
            Fact::Known("noRetainedSnapshotCommand")
        }
    }

    fn finish(mut self) {
        self.service.lock_worker().busy = false;
        self.recorded = true;
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if !self.recorded {
            let mut slot = self.service.lock_worker();
            slot.quarantine =
                Some("snapshot worker exited without recording ownership outcome".into());
            // An original pending owner is deliberately NOT removed. Releasing
            // only the worker flag cannot re-enable admission in quarantine.
            slot.busy = false;
        }
    }
}

struct SnapshotRunner<'a, S> {
    service: &'a SnapshotService,
    source: &'a S,
}

impl<S: ObservationSource> CommandRunner for SnapshotRunner<'_, S> {
    fn run(&self, command: &Command, timeout: Duration) -> Result<CommandOutput, String> {
        self.service.run_owned(self.source, command, timeout)
    }
}

fn fact_wire<T>(fact: &Fact<T>, known: impl FnOnce(&T) -> Value) -> Value {
    match fact {
        Fact::Known(value) => json!({ "status": "known", "value": known(value) }),
        Fact::Unknown(reason) => json!({ "status": "unknown", "reason": reason }),
    }
}

fn unknown_wire(reason: &str) -> Value {
    json!({ "status": "unknown", "reason": reason })
}

fn object_wire(object: &ObjectFacts) -> Value {
    json!({
        "interface": object.interface,
        "tunnel": fact_wire(&object.tunnel, |v| json!(v)),
        "virtualization": fact_wire(&object.virtualization, |v| json!(v)),
        "addresses": fact_wire(&object.addresses, |rows| json!(rows.iter().map(|row| json!({
            "address": row.address.to_string(), "prefixLen": row.prefix_len,
        })).collect::<Vec<_>>())),
        "routes": fact_wire(&object.routes, |rows| json!(rows.iter().map(|row| json!({
            "prefix": row.prefix,
            "table": fact_wire(&row.table, |v| json!(v)),
            "scope": fact_wire(&row.scope, |v| json!(match v {
                RouteScope::Global => "global", RouteScope::InterfaceScoped => "interfaceScoped",
            })),
            "role": fact_wire(&row.role, |v| json!(match v {
                RouteRole::CoverageDeclaration => "coverageDeclaration", RouteRole::ResourceClaim => "resourceClaim",
            })),
        })).collect::<Vec<_>>())),
        "policyRules": fact_wire(&object.policy_rules, |rows| json!(rows.iter().map(|row| json!({
            "priority": row.priority,
            "lookupTable": fact_wire(&row.lookup_table, |v| json!(v)),
            "addressFamily": fact_wire(&row.address_family, |v| json!(match v {
                AddressFamily::V4 => "ipv4", AddressFamily::V6 => "ipv6",
            })),
            "selectorScope": fact_wire(&row.selector_scope, |v| match v {
                PolicySelectorScope::Global => json!({ "kind": "global" }),
                PolicySelectorScope::Limited(selector) => json!({ "kind": "limited", "selector": selector }),
            }),
            "appliesToObject": fact_wire(&row.applies_to_object, |v| json!(v)),
        })).collect::<Vec<_>>())),
        "stableIdentity": fact_wire(&object.stable_identity, |v| json!(v)),
    })
}

fn platform_tag(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "linux",
        Platform::Mac => "darwin",
        Platform::Win => "win32",
        Platform::Android => "android",
        Platform::Ios => "ios",
        Platform::Other => "other",
    }
}

fn snapshot_wire(
    platform: Platform,
    objects: Fact<Vec<ObjectFacts>>,
    observation: Value,
    cleanup: Fact<&str>,
) -> Value {
    json!({
        "schemaVersion": 1,
        "platform": platform_tag(platform),
        "objects": fact_wire(&objects, |rows| json!(rows.iter().map(object_wire).collect::<Vec<_>>())),
        "observation": observation,
        "commandCleanup": fact_wire(&cleanup, |v| json!(v)),
        "context": {
            "observationPhase": unknown_wire("immutable observation phase not supplied"),
            "ownInterfaces": unknown_wire("complete session TUN and own mesh attribution not supplied"),
            "criteria": unknown_wire("immutable emitted conflict criteria not supplied"),
            "repairHistory": unknown_wire("verified repair history not supplied"),
        },
        "classification": unknown_wire("session provenance not supplied; classification not performed"),
    })
}

fn windows_unavailable(reason: &str) -> WindowsFactInput {
    WindowsFactInput {
        adapters: Fact::Unknown(reason.into()),
        addresses: Fact::Unknown(reason.into()),
        routes4: Fact::Unknown(reason.into()),
        routes6: Fact::Unknown(reason.into()),
        ras: Fact::Unknown(reason.into()),
    }
}

fn windows_ref_wire(reference: &WindowsInterfaceRef) -> Value {
    json!({
        "alias": fact_wire(&reference.alias, |v| json!(v)),
        "luid": fact_wire(&reference.luid, |v| json!(v.to_string())),
        "ifIndex": fact_wire(&reference.if_index, |v| json!(v)),
    })
}

fn windows_source_wire<T>(source: &Fact<ReadRows<T>>, row: impl Fn(&T) -> Value) -> Value {
    match source {
        Fact::Known(source) => json!({
            "rows": {"status":"known", "value":source.rows.iter().map(row).collect::<Vec<_>>()},
            "complete": fact_wire(&source.complete, |v| json!(v)),
            "compartment": fact_wire(&source.compartment, |v| json!(v)),
            "error": fact_wire(&source.error, |v| json!(v)),
        }),
        Fact::Unknown(reason) => json!({
            "rows":unknown_wire(reason),"complete":unknown_wire("source completeness unavailable"),
            "compartment":unknown_wire("source compartment unavailable"),
            "error":{"status":"known","value":reason},
        }),
    }
}

fn windows_snapshot_wire(input: &WindowsFactInput, observation: Value) -> Value {
    let route = |row: &WindowsRouteObservation| {
        json!({
            "interface":windows_ref_wire(&row.interface),
            "family":if row.family == AddressFamily::V4 {"ipv4"} else {"ipv6"},
            "prefix":row.prefix,"nextHop":fact_wire(&row.next_hop, |v| json!(v.to_string())),
            "nextHopScopeId":fact_wire(&row.next_hop_scope_id, |v|json!(v)),
        "routeMetric":fact_wire(&row.route_metric, |v|json!(v)),
            "interfaceMetric":fact_wire(&row.interface_metric, |v|json!(v)),
        })
    };
    let mut value = snapshot_wire(
        Platform::Win,
        Fact::Unknown("Windows sources do not establish joined ObjectFacts".into()),
        observation,
        Fact::Unknown("native API queries provide no process-custody cleanup receipt".into()),
    );
    value["schemaVersion"] = json!(2);
    value["sources"] = json!({
        "adapters":windows_source_wire(&input.adapters, |row|json!({
            "interface":windows_ref_wire(&row.interface),"ifType":fact_wire(&row.if_type, |v|json!(v)),
            "description":fact_wire(&row.description, |v|json!(v)),
        })),
        "addresses":windows_source_wire(&input.addresses, |row|json!({
            "interface":windows_ref_wire(&row.interface),"family":if row.family == AddressFamily::V4 {"ipv4"} else {"ipv6"},
            "address":row.address.to_string(),"prefixLen":fact_wire(&row.prefix_len, |v|json!(v)),
            "scopeId":fact_wire(&row.scope_id, |v|json!(v)),
        })),
        "routes4":windows_source_wire(&input.routes4, route),
        "routes6":windows_source_wire(&input.routes6, route),
        "ras":windows_source_wire(&input.ras, |row|json!({
            "name":fact_wire(&row.name, |v|json!(v)),"allUsers":row.all_users,
            "interface":fact_wire(&row.interface, windows_ref_wire),
        })),
    });
    value
}

fn unavailable_wire(platform: Platform, reason: String) -> Value {
    if platform == Platform::Win {
        return windows_snapshot_wire(
            &windows_unavailable(&reason),
            unknown_wire("no new collection performed"),
        );
    }
    snapshot_wire(
        platform,
        Fact::Unknown(reason.clone()),
        unknown_wire("no new collection performed"),
        Fact::Unknown(reason),
    )
}

fn collect_blocking<S: ObservationSource>(
    admission: Admission,
    platform: Platform,
    source: S,
) -> Value {
    let started = Instant::now();
    if platform == Platform::Win {
        let input = match admission.service.observe_retained() {
            Ok(()) => validate_windows_input(source.windows_facts()),
            Err(reason) => windows_unavailable(&reason),
        };
        // Every native query AND the projection retain the original admission.
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let result = windows_snapshot_wire(
            &input,
            json!({"status":"known","value":{"elapsedMillis":elapsed,"atomic":false}}),
        );
        admission.finish();
        return result;
    }
    let objects = match admission.service.observe_retained() {
        Ok(()) => {
            let runner = SnapshotRunner {
                service: &admission.service,
                source: &source,
            };
            match platform {
                Platform::Linux => collect_linux(&runner).objects,
                Platform::Mac => collect_macos(&runner),
                _ => Fact::Unknown(
                    "production COEX snapshot collector unavailable on this platform".into(),
                ),
            }
        }
        Err(error) => Fact::Unknown(error),
    };
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let snapshot = snapshot_wire(
        platform,
        objects,
        json!({ "status": "known", "value": { "elapsedMillis": elapsed, "atomic": false } }),
        admission.cleanup_outcome(),
    );
    // Keep admission until projection is complete as well as native work.
    admission.finish();
    snapshot
}

async fn collect_request<S: ObservationSource>(
    service: Arc<SnapshotService>,
    platform: Platform,
    source: S,
) -> ApiResponse<Value> {
    if !matches!(platform, Platform::Linux | Platform::Mac | Platform::Win) {
        return ApiResponse::ok(unavailable_wire(
            platform,
            "production COEX snapshot collector unavailable on this platform".into(),
        ));
    }
    let admission = match service.admit() {
        Ok(admission) => admission,
        Err(reason) => return ApiResponse::ok(unavailable_wire(platform, reason)),
    };
    // The closure, not the IPC waiter/JoinHandle, owns admission and custody.
    // Dropping a waiter does not cancel spawn_blocking or enable another worker.
    match tokio::task::spawn_blocking(move || collect_blocking(admission, platform, source)).await {
        Ok(snapshot) => ApiResponse::ok(snapshot),
        Err(error) => ApiResponse::ok(unavailable_wire(
            platform,
            format!("snapshot worker join failed: {error}"),
        )),
    }
}

fn allowed_window(label: &str) -> bool {
    label == "main"
}

async fn request_for_window<S: ObservationSource>(
    service: Arc<SnapshotService>,
    platform: Platform,
    source: S,
    label: &str,
) -> ApiResponse<Value> {
    if !allowed_window(label) {
        return ApiResponse::err_with_code(
            "COEX snapshot requires the main window",
            "coex_window_denied",
        );
    }
    collect_request(service, platform, source).await
}

/// Backend diagnostic reachability only: no UI refresh, watcher, policy or exit update.
/// App commands are local-origin ACL-free today; additionally restrict this entry
/// to the main window. No new plugin capability, helper privilege or renderer argv.
#[tauri::command]
pub async fn coex_readonly_snapshot(window: tauri::WebviewWindow) -> ApiResponse<Value> {
    static SERVICE: OnceLock<Arc<SnapshotService>> = OnceLock::new();
    request_for_window(
        Arc::clone(SERVICE.get_or_init(|| Arc::new(SnapshotService::default()))),
        Platform::current(),
        NativeSource,
        window.label(),
    )
    .await
}

#[cfg(test)]
mod tests;
