//! Read-only visibility. Receipt epochs describe observed events, never global atomicity.
use crate::commands::coexistence_snapshot::{self as snapshot, Admission, SnapshotService};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MIN_INTERVAL: Duration = Duration::from_secs(2);
const MAX_PENDING: Duration = Duration::from_secs(10);
type Sink = Arc<dyn Fn(Value) + Send + Sync>;
type Collector = Arc<dyn Fn(&Admission) -> Value + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    session: Option<u64>,
    lifecycle: Option<u64>,
    config: Option<u64>,
    network: Option<u64>,
    phase: &'static str,
}
fn fact(value: Option<u64>) -> Value {
    value.map_or_else(
        || unknown("binding unavailable"),
        |v| json!({"status":"known","value":v.to_string()}),
    )
}
fn unknown(reason: &str) -> Value {
    json!({"status":"unknown","reason":reason})
}
impl Binding {
    fn wire(&self) -> Value {
        json!({"sessionId":fact(self.session),"lifecycleGeneration":fact(self.lifecycle),"configGeneration":fact(self.config),"networkEpoch":fact(self.network),"lifecyclePhase":self.phase})
    }
    fn qualified(&self) -> bool {
        self.session.is_some()
            && self.lifecycle.is_some()
            && self.config.is_some()
            && self.network.is_some()
            && self.phase == "ready"
    }
}
#[derive(Clone)]
struct Pending {
    first: Duration,
    trigger: &'static str,
    manual: bool,
}
struct State {
    binding: Binding,
    revision: u64,
    invalid: u64,
    network: u64,
    watcher: u64,
    config: u64,
    report: Option<Value>,
    freshness: &'static str,
    reason: &'static str,
    pending: Option<Pending>,
    running: bool,
    driver: bool,
    last_started: Option<Duration>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            binding: Binding {
                session: None,
                lifecycle: None,
                config: None,
                network: None,
                phase: "idle",
            },
            revision: 0,
            invalid: 0,
            network: 0,
            watcher: 0,
            config: 0,
            report: None,
            freshness: "unavailable",
            reason: "notCollected",
            pending: None,
            running: false,
            driver: false,
            last_started: None,
        }
    }
}
impl State {
    fn advance_revision(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("COEX revision overflow");
    }
    fn commit(&mut self) -> Value {
        self.advance_revision();
        self.wire()
    }
    fn invalidate(&mut self, reason: &'static str) {
        self.invalid = self
            .invalid
            .checked_add(1)
            .expect("COEX invalidation overflow");
        self.freshness = if self.report.is_some() {
            "stale"
        } else {
            "unavailable"
        };
        self.reason = reason;
    }
    fn wire(&self) -> Value {
        let activity = match (self.running, self.pending.is_some()) {
            (false, false) => "idle",
            (true, false) => "running",
            (false, true) => "pending",
            (true, true) => "runningWithPending",
        };
        let pending=self.pending.as_ref().map_or_else(||unknown("no pending request"),|p|json!({"status":"known","value":{
            "requestedAtMonotonicMillis":p.first.as_millis().to_string(),"expiresAtMonotonicMillis":(p.first+MAX_PENDING).as_millis().to_string(),"trigger":p.trigger,"manualRequested":p.manual}}));
        json!({"schemaVersion":1,"reportRevision":self.revision.to_string(),"binding":self.binding.wire(),"freshness":self.freshness,"activity":activity,
            "reason":if self.freshness=="latest"{unknown("no unavailable reason")}else{json!({"status":"known","value":{"code":self.reason,"detail":self.reason}})},"pending":pending,
            "report":self.report.as_ref().map_or_else(||unknown("no current session report"),|r|json!({"status":"known","value":r}))})
    }
}

pub(crate) struct Observer {
    state: Mutex<State>,
    sink: Mutex<Option<Sink>>,
    service: Arc<SnapshotService>,
    collector: Collector,
    clock: Arc<dyn Fn() -> Duration + Send + Sync>,
    interval: Duration,
}
impl Default for Observer {
    fn default() -> Self {
        let origin = Instant::now();
        Self {
            state: Mutex::default(),
            sink: Mutex::default(),
            service: snapshot::shared_service(),
            collector: Arc::new(|admission| {
                snapshot::collect_admitted(
                    admission,
                    polaris_helper_proto::Platform::current(),
                    snapshot::NativeSource,
                )
            }),
            clock: Arc::new(move || origin.elapsed()),
            interval: MIN_INTERVAL,
        }
    }
}
impl Observer {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(s) => s,
            Err(e) => {
                let mut s = e.into_inner();
                s.binding.config = None;
                s.binding.network = None;
                s.pending = None;
                s.invalidate("bindingUnavailable");
                s.advance_revision();
                s
            }
        }
    }
    pub(crate) fn set_sink(&self, sink: Sink) {
        if let Ok(mut s) = self.sink.lock() {
            *s = Some(sink);
        }
    }
    fn emit(&self, wire: Value) {
        let sink = self.sink.lock().ok().and_then(|s| s.clone());
        if let Some(sink) = sink {
            sink(wire);
        }
    }
    fn dispatch(self: &Arc<Self>, wire: Value, spawn: bool) {
        self.emit(wire);
        if spawn {
            let this = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                this.drive().await;
            });
        }
    }
    pub(crate) fn get(&self) -> Value {
        self.lock().wire()
    }
    pub(crate) fn claim(self: &Arc<Self>, generation: u64, start: bool) {
        let wire = {
            let mut s = self.lock();
            s.invalidate("lifecycleChanged");
            s.report = None;
            s.freshness = "unavailable";
            s.pending = None;
            s.binding = Binding {
                session: start.then_some(generation),
                lifecycle: Some(generation),
                config: None,
                network: None,
                phase: if start { "starting" } else { "stopped" },
            };
            s.network = 0;
            s.commit()
        };
        self.emit(wire);
    }
    pub(crate) fn invalidate_generation(&self, generation: u64) {
        let wire = {
            let mut s = self.lock();
            s.invalidate("lifecycleChanged");
            s.report = None;
            s.freshness = "unavailable";
            s.pending = None;
            s.binding.session = None;
            s.binding.lifecycle = Some(generation);
            s.binding.config = None;
            s.binding.network = None;
            s.binding.phase = "stopped";
            s.commit()
        };
        self.emit(wire);
    }
    pub(crate) fn ready(self: &Arc<Self>, generation: u64) {
        let (wire, spawn) = {
            let mut s = self.lock();
            if s.binding.session != Some(generation) {
                return;
            }
            s.invalidate("lifecycleChanged");
            s.binding.phase = "ready";
            let spawn = self.queue(&mut s, "startup", false);
            (s.commit(), spawn)
        };
        self.dispatch(wire, spawn);
    }
    pub(crate) fn fail_start(&self, generation: u64) {
        let wire = {
            let mut s = self.lock();
            if s.binding.session != Some(generation) || s.binding.phase != "starting" {
                return;
            }
            s.invalidate("lifecycleChanged");
            s.report = None;
            s.freshness = "unavailable";
            s.pending = None;
            s.binding.session = None;
            s.binding.config = None;
            s.binding.network = None;
            s.binding.phase = "stopped";
            s.commit()
        };
        self.emit(wire);
    }
    pub(crate) fn terminal(&self, generation: u64, crashed: bool) {
        let wire = {
            let mut s = self.lock();
            if s.binding.session != Some(generation) {
                return;
            }
            s.invalidate("lifecycleChanged");
            s.report = None;
            s.freshness = "unavailable";
            s.pending = None;
            s.binding.session = None;
            s.binding.config = None;
            s.binding.network = None;
            s.binding.phase = if crashed { "crashed" } else { "stopped" };
            s.commit()
        };
        self.emit(wire);
    }
    /// Caller holds current_config.write; actual value and binding commit under the same lock.
    /// An unchanged value still receives a new stamp when rebinding a new session's Unknown config.
    pub(crate) fn commit_config(self: &Arc<Self>, current: &mut Option<Value>, value: Value) {
        let (wire, spawn) = {
            let mut s = self.lock();
            let changed = current.as_ref() != Some(&value);
            *current = Some(value);
            if !changed && s.binding.config.is_some() {
                return;
            }
            s.config = s.config.checked_add(1).expect("COEX config overflow");
            s.binding.config = Some(s.config);
            s.invalidate("configChanged");
            let spawn = self.queue(&mut s, "config", false);
            (s.commit(), spawn)
        };
        self.dispatch(wire, spawn);
    }
    pub(crate) fn config_unavailable(&self) {
        let wire = {
            let mut s = self.lock();
            s.binding.config = None;
            s.invalidate("bindingUnavailable");
            s.pending = None;
            s.commit()
        };
        self.emit(wire);
    }
    pub(crate) fn begin_watcher(&self, generation: u64) -> Option<u64> {
        let (wire, token) = {
            let mut s = self.lock();
            if s.binding.session != Some(generation) {
                return None;
            }
            s.watcher = s.watcher.checked_add(1).expect("COEX watcher overflow");
            s.binding.network = None;
            s.invalidate("watcherUnavailable");
            (s.commit(), s.watcher)
        };
        self.emit(wire);
        Some(token)
    }
    pub(crate) fn watcher_unavailable(&self) {
        let wire = {
            let mut s = self.lock();
            s.watcher = s.watcher.checked_add(1).expect("COEX watcher overflow");
            s.binding.network = None;
            s.invalidate("watcherUnavailable");
            s.commit()
        };
        self.emit(wire);
    }
    /// Unknown -> Unknown invalidates too; only this original watcher token can recover.
    pub(crate) fn watcher(self: &Arc<Self>, generation: u64, token: u64, available: bool) {
        let (wire, spawn) = {
            let mut s = self.lock();
            if s.binding.session != Some(generation) || s.watcher != token || token == 0 {
                return;
            }
            s.invalidate("watcherUnavailable");
            s.binding.network = available.then_some(s.network);
            let spawn = available && self.queue(&mut s, "network", false);
            (s.commit(), spawn)
        };
        self.dispatch(wire, spawn);
    }
    pub(crate) fn receipt(self: &Arc<Self>, generation: u64, token: u64) {
        let committed = {
            let mut s = self.lock();
            if s.binding.session != Some(generation) || s.watcher != token || token == 0 {
                return;
            }
            let coalesced = s.pending.is_some() && s.freshness != "latest";
            s.network = s.network.checked_add(1).expect("COEX network overflow");
            s.binding.network = Some(s.network);
            s.invalidate("networkChanged");
            if coalesced {
                if let Some(pending) = s.pending.as_mut() {
                    pending.trigger = "network";
                }
                // No event is published for a coalesced receipt; state and its new revision
                // still commit together, so any get-state sees a consistent snapshot.
                s.advance_revision();
                None
            } else {
                let spawn = self.queue(&mut s, "network", false);
                Some((s.commit(), spawn))
            }
        };
        if let Some((wire, spawn)) = committed {
            self.dispatch(wire, spawn);
        }
    }
    pub(crate) fn manual(self: &Arc<Self>, expected: u64) -> Value {
        let (wire, spawn) = {
            let mut s = self.lock();
            if expected != s.revision {
                return s.wire();
            }
            let spawn = self.queue(&mut s, "manual", true);
            (s.commit(), spawn)
        };
        self.dispatch(wire.clone(), spawn);
        wire
    }
    /// Queue mutation is part of the caller's state/revision transaction.
    fn queue(&self, s: &mut State, trigger: &'static str, manual: bool) -> bool {
        if !cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            s.reason = "unsupportedPlatform";
            return false;
        }
        if manual && s.binding.session.is_none() {
            s.reason = "noActiveSession";
            return false;
        }
        if !manual && s.binding.phase != "ready" {
            return false;
        }
        if let Some(p) = s.pending.as_mut() {
            p.manual |= manual;
            p.trigger = trigger;
        } else {
            s.pending = Some(Pending {
                first: (self.clock)(),
                trigger,
                manual,
            });
        }
        if s.driver {
            false
        } else {
            s.driver = true;
            true
        }
    }
    async fn drive(self: Arc<Self>) {
        loop {
            let (delay, expired_wire) = {
                let mut s = self.lock();
                let now = (self.clock)();
                let Some(p) = s.pending.as_ref() else {
                    s.driver = false;
                    return;
                };
                if now >= p.first + MAX_PENDING {
                    s.pending = None;
                    s.reason = if s.running {
                        "workerStillRunning"
                    } else {
                        "pendingExpired"
                    };
                    (None, Some(s.commit()))
                } else {
                    let next = s.last_started.map_or(now, |last| last + self.interval);
                    (
                        Some(
                            next.saturating_sub(now)
                                .min((p.first + MAX_PENDING).saturating_sub(now)),
                        ),
                        None,
                    )
                }
            };
            if let Some(wire) = expired_wire {
                self.emit(wire);
            }
            let Some(delay) = delay else {
                continue;
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
                continue;
            }
            let wire = {
                let mut s = self.lock();
                if s.pending.is_none() {
                    continue;
                }
                s.pending = None;
                s.running = true;
                s.commit()
            };
            self.emit(wire);
            let this = Arc::clone(&self);
            let mut worker = tokio::task::spawn_blocking(move || this.run());
            // A pending deadline never cancels or replaces the physical worker owner.
            loop {
                tokio::select! {
                    result = &mut worker => {
                        if result.is_err() {
                            let wire = { let mut s=self.lock(); s.running=false; s.reason="serviceQuarantined"; s.freshness="unavailable"; s.commit() };
                            self.emit(wire);
                        }
                        break;
                    },
                    () = tokio::time::sleep(Duration::from_millis(100)) => {
                        let wire = { let mut s=self.lock(); if s.pending.as_ref().is_some_and(|p|(self.clock)()>=p.first+MAX_PENDING) { s.pending=None; s.reason="workerStillRunning"; Some(s.commit()) } else { None } };
                        if let Some(wire)=wire { self.emit(wire); }
                    }
                }
            }
        }
    }
    fn run(&self) {
        let admission = match self.service.admit() {
            Ok(a) => a,
            Err(reason) => {
                let wire = {
                    let mut s = self.lock();
                    s.running = false;
                    s.reason = if reason.contains("busy") {
                        "admissionBusy"
                    } else {
                        "serviceQuarantined"
                    };
                    s.freshness = if s.report.is_some() {
                        "stale"
                    } else {
                        "unavailable"
                    };
                    s.commit()
                };
                self.emit(wire);
                return;
            }
        };
        let start = (self.clock)();
        let unix_start = unix_millis();
        let (binding, invalid) = {
            let mut s = self.lock();
            s.last_started = Some(start);
            (s.binding.clone(), s.invalid)
        };
        let snapshot = (self.collector)(&admission);
        let end = (self.clock)();
        let unix_end = unix_millis();
        let wire = {
            let mut s = self.lock();
            let end_binding = s.binding.clone();
            // Unknown is never an equality wildcard; independent invalidation catches availability ABA.
            let same_session = binding.session.is_some() && binding.session == end_binding.session;
            if same_session {
                let qualifies = binding.qualified()
                    && end_binding.qualified()
                    && binding == end_binding
                    && invalid == s.invalid;
                let content = has_source_content(&snapshot);
                s.freshness = if qualifies && content {
                    "latest"
                } else if content {
                    "stale"
                } else {
                    "unavailable"
                };
                s.reason = if qualifies && content {
                    "notCollected"
                } else if !content {
                    "sourceUnavailable"
                } else if binding.config != end_binding.config {
                    "configChanged"
                } else if binding.network != end_binding.network {
                    "networkChanged"
                } else if binding.phase != end_binding.phase {
                    "lifecycleChanged"
                } else if invalid != s.invalid {
                    "watcherUnavailable"
                } else {
                    "bindingUnavailable"
                };
                s.report = Some(
                    json!({"reportRevision":(s.revision+1).to_string(),"acquisition":{"startedAtUnixMillis":unix_start.to_string(),"endedAtUnixMillis":unix_end.to_string(),"startedAtMonotonicMillis":start.as_millis().to_string(),"endedAtMonotonicMillis":end.as_millis().to_string(),"startBinding":binding.wire(),"endBinding":end_binding.wire(),"atomic":false},"snapshot":snapshot}),
                );
            }
            // Projection, outer revision and activity are one observable transaction.
            // END projection still precedes release of the original physical admission.
            admission.finish();
            s.running = false;
            s.commit()
        };
        self.emit(wire);
    }
}
fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn has_source_content(snapshot: &Value) -> bool {
    snapshot["objects"]["status"] == "known"
        || snapshot["sources"]
            .as_object()
            .is_some_and(|sources| sources.values().any(|s| s["rows"]["status"] == "known"))
}

#[cfg(test)]
mod tests;

/// Covers Start early returns after claim, without trusting the unguarded legacy failed event.
pub(super) struct StartAttempt {
    observer: Arc<Observer>,
    pub(super) generation: std::cell::Cell<u64>,
}
impl StartAttempt {
    pub(super) fn new(observer: Arc<Observer>, generation: u64) -> Self {
        Self {
            observer,
            generation: std::cell::Cell::new(generation),
        }
    }
}
impl Drop for StartAttempt {
    fn drop(&mut self) {
        self.observer.fail_start(self.generation.get());
    }
}
