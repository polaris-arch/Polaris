//! Request cancellation may arrive without the state gate. Registration and retirement are
//! serialized by the caller's state gate so a prepared request cannot appear during retirement.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LoginMode {
    Browser,
    Authkey,
}

/// Bound the lifetime fence. Exhaustion rejects new login registration until process restart;
/// evicting a retired ID would let a delayed IPC reuse it after the ordinary 512-entry prune.
pub(super) const MAX_RETIRED_ATTEMPTS: usize = 4096;
pub(super) const RETIRED_LIMIT_ERROR: &str =
    "Too many retired login requests; restart Polaris before authorizing again";

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub attempt_id: String,
    pub mode: LoginMode,
    #[serde(default)]
    pub replace_identity: bool,
    #[serde(default)]
    pub reuse_retained_auth_key: bool,
    #[serde(default)]
    pub expected_credential_revision: Option<String>,
}

/// Last native progress for one renderer-minted request. A read cannot infer authorization
/// from a state directory or another node's login; only this exact attempt's receipt counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginProgressReceipt {
    pub server_id: String,
    pub attempt_id: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_epoch: Option<String>,
}

pub(crate) struct Attempt {
    pub server_id: String,
    pub claimed: AtomicBool,
    pub process_owned: AtomicBool,
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
    progress: Mutex<Option<LoginProgressReceipt>>,
    main_context: Mutex<Option<(u64, Option<String>)>>,
    credential_activation: Mutex<Option<CredentialActivation>>,
    #[cfg(target_os = "android")]
    android_store:
        Mutex<Option<crate::runtime::proxy::android_bridge::tailscale_store::AndroidStoreCustody>>,
    #[cfg(any(target_os = "android", test))]
    android_action: Mutex<Option<Arc<AndroidTargetAction>>>,
}

/// The original request keeps an unresolved native reservation reachable. This
/// cell is not a writer receipt; only the private bridge's begin/finish may
/// establish or release the native action.
#[cfg(any(target_os = "android", test))]
pub(crate) struct AndroidTargetAction {
    pub(crate) state_file: String,
    pub(crate) action_id: String,
    pub(crate) active: AtomicBool,
    #[cfg(target_os = "android")]
    pub(crate) original: AndroidActionOrigin,
}

/// Original opaque native handles; neither variant manufactures native scope or
/// terminal evidence. Warm also remains manageable before a Store exists.
#[cfg(target_os = "android")]
pub(crate) enum AndroidActionOrigin {
    Runtime(crate::runtime::proxy::android_bridge::tailscale_store::AndroidStoreCustody),
    Warm(crate::runtime::proxy::android_bridge::tailscale_store::AndroidWarmTuple),
}

#[cfg(any(target_os = "android", test))]
pub(crate) struct AndroidActionActivity(pub(crate) Arc<AndroidTargetAction>);

#[cfg(any(target_os = "android", test))]
impl Drop for AndroidActionActivity {
    fn drop(&mut self) {
        // This only transfers the Rust operation back to its retained token. It
        // never claims that a pending native begin or finish has completed.
        self.0.active.store(false, Ordering::SeqCst);
    }
}

/// In-memory exact saved CAS for this request, not a producer terminal or a new state machine.
#[derive(Clone)]
pub(super) struct CredentialActivation {
    pub proxy: std::sync::Weak<crate::runtime::proxy::ProxyRuntime>,
    pub saved: serde_json::Value,
    pub generation: u64,
}

impl Attempt {
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn reserve_android_action(
        &self,
        action: Arc<AndroidTargetAction>,
    ) -> Result<(), String> {
        let mut slot = self
            .android_action
            .lock()
            .map_err(|_| "nativeRetirementUnknown")?;
        if slot.is_some() {
            return Err("nativeRetirementUnknown".into());
        }
        *slot = Some(action);
        Ok(())
    }

    #[cfg(any(target_os = "android", test))]
    pub(crate) fn android_action(&self) -> Result<Option<Arc<AndroidTargetAction>>, String> {
        self.android_action
            .lock()
            .map(|slot| slot.clone())
            .map_err(|_| "nativeRetirementUnknown".into())
    }

    #[cfg(any(target_os = "android", test))]
    pub(crate) fn release_android_action(
        &self,
        original: &Arc<AndroidTargetAction>,
    ) -> Result<(), String> {
        let mut slot = self
            .android_action
            .lock()
            .map_err(|_| "nativeRetirementUnknown")?;
        if !slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, original))
        {
            return Err("nativeRetirementUnknown".into());
        }
        *slot = None;
        Ok(())
    }

    fn has_unresolved_action(&self) -> bool {
        #[cfg(any(target_os = "android", test))]
        {
            self.android_action
                .lock()
                .map_or(true, |slot| slot.is_some())
        }
        #[cfg(not(any(target_os = "android", test)))]
        {
            false
        }
    }
    #[cfg(target_os = "android")]
    pub(crate) fn record_android_store(
        &self,
        original: crate::runtime::proxy::android_bridge::tailscale_store::AndroidStoreCustody,
    ) -> Result<(), String> {
        *self
            .android_store
            .lock()
            .map_err(|_| "nativeRetirementUnknown")? = Some(original);
        Ok(())
    }

    #[cfg(target_os = "android")]
    pub(super) fn android_store(
        &self,
    ) -> Result<crate::runtime::proxy::android_bridge::tailscale_store::AndroidStoreCustody, String>
    {
        self.android_store
            .lock()
            .map_err(|_| "nativeRetirementUnknown")?
            .clone()
            .ok_or_else(|| "nativeRetirementUnknown".to_owned())
    }

    pub(super) fn record_credential_activation(
        &self,
        proxy: &Arc<crate::runtime::proxy::ProxyRuntime>,
        saved: serde_json::Value,
        generation: u64,
    ) {
        *self
            .credential_activation
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(CredentialActivation {
            proxy: Arc::downgrade(proxy),
            saved,
            generation,
        });
    }

    pub(super) fn credential_activation(&self) -> Option<CredentialActivation> {
        self.credential_activation.lock().ok()?.clone()
    }
    /// The existing cancellation cell also serializes each synchronous action
    /// admission. Cancellation that wins this lock cannot be cleared by Start.
    pub(crate) fn while_active<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let mut result = None;
        self.cancel.send_if_modified(|cancelled| {
            if !*cancelled && !self.is_finished() {
                result = Some(action());
            }
            false
        });
        result
    }
    pub fn cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    fn cancel_dropped_request(&self) {
        self.cancel();
        let context = self.main_context();
        let mut progress = self.progress.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(receipt) = progress.as_mut() {
            if matches!(
                receipt.phase.as_str(),
                "authorized" | "failed" | "timedOut" | "cancelled"
            ) {
                return;
            }
            receipt.phase = "cancelled".into();
            receipt.reason = None;
            receipt.url = None;
            if let Some((generation, epoch)) = context {
                receipt.main_generation = Some(generation);
                receipt.identity_epoch = epoch;
            }
        }
    }

    pub async fn cancellation(&self) {
        let mut rx = self.cancel.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    pub fn is_finished(&self) -> bool {
        *self.done.borrow()
    }

    pub fn finish(&self) {
        self.process_owned.store(false, Ordering::SeqCst);
        self.done.send_replace(true);
    }

    pub async fn finished(&self) {
        let mut rx = self.done.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    pub fn bind_main(&self, generation: u64, identity_epoch: Option<String>) {
        *self
            .main_context
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((generation, identity_epoch));
    }

    pub fn main_context(&self) -> Option<(u64, Option<String>)> {
        self.main_context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn record_progress(&self, receipt: LoginProgressReceipt) -> bool {
        let mut current = self.progress.lock().unwrap_or_else(PoisonError::into_inner);
        if self.cancelled()
            && !matches!(receipt.phase.as_str(), "cancelled" | "failed" | "timedOut")
        {
            return false;
        }
        if current.as_ref().is_some_and(|last| {
            matches!(
                last.phase.as_str(),
                "authorized" | "failed" | "timedOut" | "cancelled"
            )
        }) {
            return false;
        }
        if current.as_ref() == Some(&receipt) {
            return false;
        }
        *current = Some(receipt);
        true
    }

    pub fn progress_receipt(&self) -> Option<LoginProgressReceipt> {
        self.progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[derive(Default)]
struct AttemptState {
    entries: HashMap<String, Arc<Attempt>>,
    retired_ids: HashSet<String>,
    retired_exhausted: bool,
}

#[derive(Default)]
pub(super) struct Attempts(Mutex<AttemptState>);

impl Attempts {
    /// Shutdown first closes admission independently of this lock. Poison can still notify
    /// existing requests, but cannot certify a drained table in the checked async path.
    pub fn cancel_all(&self) {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for attempt in state.entries.values() {
            attempt.cancel();
            if !attempt.claimed.load(Ordering::SeqCst) {
                attempt.finish();
            }
        }
    }

    pub fn shutdown_snapshot(&self) -> Result<Vec<Arc<Attempt>>, String> {
        let state = self
            .0
            .lock()
            .map_err(|_| "登录请求关闭状态不可用".to_owned())?;
        Ok(state.entries.values().cloned().collect())
    }

    pub fn progress(&self, server_id: &str, id: &str) -> Option<LoginProgressReceipt> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .entries
            .get(id)
            .filter(|attempt| attempt.server_id == server_id)
            .and_then(|attempt| attempt.progress_receipt())
    }

    /// Inspect all admissions for one node, including prepared requests that have not claimed
    /// the state yet. `Err` means this table cannot certify a local absence.
    #[allow(dead_code, reason = "reserved for cross-registry owner reconciliation")]
    pub fn local_owner_in_use(&self, server_id: &str) -> Result<bool, ()> {
        let state = self.0.lock().map_err(|_| ())?;
        if state.retired_exhausted {
            return Err(());
        }
        let mut busy = false;
        for (id, attempt) in state
            .entries
            .iter()
            .filter(|(_, a)| a.server_id == server_id)
        {
            if state.retired_ids.contains(id) && !attempt.cancelled() {
                return Err(());
            }
            if attempt.has_unresolved_action() {
                busy = true;
            } else if attempt.is_finished() {
                if attempt.process_owned.load(Ordering::SeqCst) {
                    return Err(());
                }
            } else {
                busy = true;
            }
        }
        Ok(busy)
    }

    pub fn registration_exhausted(&self) -> bool {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.retired_exhausted || state.retired_ids.len() >= MAX_RETIRED_ATTEMPTS
    }

    pub fn owns_state(&self, server_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .values()
            .any(|a| {
                a.server_id == server_id
                    && (a.has_unresolved_action()
                        || a.claimed.load(Ordering::SeqCst) && !a.is_finished())
            })
    }

    pub fn can_preserve(&self, server_id: &str, id: &str) -> bool {
        self.get(server_id, id)
            .is_ok_and(|a| !a.claimed.load(Ordering::SeqCst) && !a.is_finished() && !a.cancelled())
    }

    pub fn retire_node_except(
        &self,
        server_id: &str,
        keep: Option<&str>,
    ) -> Result<Vec<Arc<Attempt>>, String> {
        self.retire_node_except_inner(server_id, keep, None)
    }

    #[cfg(any(target_os = "ios", target_os = "android", test))]
    pub fn retire_node_except_claimed(
        &self,
        server_id: &str,
        keep: &Arc<Attempt>,
    ) -> Result<Vec<Arc<Attempt>>, String> {
        self.retire_node_except_inner(server_id, None, Some(keep))
    }

    fn retire_node_except_inner(
        &self,
        server_id: &str,
        keep_id: Option<&str>,
        keep_claimed: Option<&Arc<Attempt>>,
    ) -> Result<Vec<Arc<Attempt>>, String> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| "Login request ownership is unknown")?;
        if let Some(keep) = keep_claimed {
            if keep.server_id != server_id
                || !keep.claimed.load(Ordering::SeqCst)
                || keep.cancelled()
                || keep.is_finished()
                || !state
                    .entries
                    .iter()
                    .any(|(id, entry)| Arc::ptr_eq(entry, keep) && !state.retired_ids.contains(id))
            {
                return Err("Login request ownership changed".into());
            }
        }
        let ids: Vec<_> = state
            .entries
            .iter()
            .filter(|(id, a)| {
                a.server_id == server_id
                    && keep_id != Some(id.as_str())
                    && !keep_claimed.is_some_and(|keep| Arc::ptr_eq(a, keep))
            })
            .map(|(id, _)| id.clone())
            .collect();
        let new_ids = ids
            .iter()
            .filter(|id| !state.retired_ids.contains(id.as_str()))
            .count();
        if state.retired_exhausted
            || new_ids > MAX_RETIRED_ATTEMPTS.saturating_sub(state.retired_ids.len())
        {
            state.retired_exhausted = true;
            return Err(RETIRED_LIMIT_ERROR.into());
        }
        let attempts: Vec<_> = ids
            .iter()
            .filter_map(|id| state.entries.get(id).cloned())
            .collect();
        state.retired_ids.extend(ids);
        drop(state);
        for attempt in &attempts {
            attempt.cancel();
            if !attempt.claimed.load(Ordering::SeqCst) {
                attempt.finish();
            }
        }
        Ok(attempts)
    }

    pub fn owns_state_except(
        &self,
        server_id: &str,
        keep: Option<&Arc<Attempt>>,
    ) -> Result<bool, String> {
        let state = self
            .0
            .lock()
            .map_err(|_| "Login request ownership is unknown")?;
        Ok(state.entries.values().any(|attempt| {
            attempt.server_id == server_id
                && (attempt.has_unresolved_action()
                    || attempt.claimed.load(Ordering::SeqCst) && !attempt.is_finished())
                && !keep.is_some_and(|keep| Arc::ptr_eq(attempt, keep))
        }))
    }

    /// Read the original cancelled/finished row without weakening get's ordinary admission.
    /// Any newer unretired row for the node prevents compensation, even when unclaimed.
    pub(super) fn original_credential_row(&self, id: &str, original: &Arc<Attempt>) -> bool {
        self.0.lock().is_ok_and(|state| {
            state
                .entries
                .get(id)
                .is_some_and(|entry| Arc::ptr_eq(entry, original))
                && !state.entries.iter().any(|(other_id, entry)| {
                    entry.server_id == original.server_id
                        && !Arc::ptr_eq(entry, original)
                        && !state.retired_ids.contains(other_id)
                })
        })
    }

    pub fn get(&self, server_id: &str, id: &str) -> Result<Arc<Attempt>, String> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.retired_exhausted || state.retired_ids.len() >= MAX_RETIRED_ATTEMPTS {
            return Err(RETIRED_LIMIT_ERROR.into());
        }
        state
            .entries
            .get(id)
            .filter(|a| a.server_id == server_id && !a.cancelled() && !a.is_finished())
            .cloned()
            .ok_or_else(|| "Login request expired or was not prepared".into())
    }
    // Retain cancelled IDs long enough to fence a delayed start IPC, with a hard memory bound.
    pub fn prepare(&self, server_id: &str, id: &str) -> Result<Arc<Attempt>, String> {
        if id.is_empty() || id.len() > 128 {
            return Err("Invalid login request identity".into());
        }
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.retired_exhausted || state.retired_ids.len() >= MAX_RETIRED_ATTEMPTS {
            return Err(RETIRED_LIMIT_ERROR.into());
        }
        if state.retired_ids.contains(id) {
            return Err("Login request was retired".into());
        }
        if let Some(attempt) = state.entries.get(id) {
            return if attempt.server_id == server_id
                && !attempt.cancelled()
                && !attempt.is_finished()
            {
                Ok(attempt.clone())
            } else {
                Err("Login request expired or belongs to another node".into())
            };
        }
        if state.entries.len() >= 512 {
            state
                .entries
                .retain(|_, a| !*a.done.borrow() || a.has_unresolved_action());
        }
        if state.entries.len() >= 512 {
            return Err("Too many pending login requests".into());
        }
        let (cancel, _) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let attempt = Arc::new(Attempt {
            server_id: server_id.into(),
            claimed: AtomicBool::new(false),
            process_owned: AtomicBool::new(false),
            cancel,
            done,
            progress: Mutex::new(None),
            main_context: Mutex::new(None),
            credential_activation: Mutex::new(None),
            #[cfg(target_os = "android")]
            android_store: Mutex::new(None),
            #[cfg(any(target_os = "android", test))]
            android_action: Mutex::new(None),
        });
        state.entries.insert(id.into(), attempt.clone());
        Ok(attempt)
    }

    pub fn cancel(&self, server_id: &str, id: &str) -> Result<Option<Arc<Attempt>>, String> {
        if id.is_empty() || id.len() > 128 {
            return Err("Invalid login request identity".into());
        }
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let attempt = if let Some(attempt) = state.entries.get(id) {
            if attempt.server_id != server_id {
                return Err("Login request expired or belongs to another node".into());
            }
            attempt.clone()
        } else {
            if state.retired_ids.contains(id)
                || state.retired_exhausted
                || state.retired_ids.len() >= MAX_RETIRED_ATTEMPTS
            {
                return Ok(None);
            }
            if state.entries.len() >= 512 {
                state
                    .entries
                    .retain(|_, a| !a.is_finished() || a.has_unresolved_action());
            }
            if state.entries.len() >= 512 {
                return Err("Too many pending login requests".into());
            }
            let (cancel, _) = watch::channel(false);
            let (done, _) = watch::channel(false);
            let attempt = Arc::new(Attempt {
                server_id: server_id.into(),
                claimed: AtomicBool::new(false),
                process_owned: AtomicBool::new(false),
                cancel,
                done,
                progress: Mutex::new(None),
                main_context: Mutex::new(None),
                credential_activation: Mutex::new(None),
                #[cfg(target_os = "android")]
                android_store: Mutex::new(None),
                #[cfg(any(target_os = "android", test))]
                android_action: Mutex::new(None),
            });
            state.entries.insert(id.into(), attempt.clone());
            attempt
        };
        drop(state);
        attempt.cancel();
        if !attempt.claimed.load(Ordering::SeqCst) {
            attempt.finish();
        }
        Ok(Some(attempt))
    }
}

/// Dropping an IPC future cancels its request; a background process owner still performs reap.
pub(super) struct AttemptGuard(pub Arc<Attempt>, pub bool);
impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.1 {
            self.0.cancel_dropped_request();
            if !self.0.process_owned.load(Ordering::SeqCst) {
                self.0.finish();
            }
        }
    }
}
