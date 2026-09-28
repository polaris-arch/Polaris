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
}

pub(super) struct Attempt {
    pub server_id: String,
    pub claimed: AtomicBool,
    pub process_owned: AtomicBool,
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
}

impl Attempt {
    pub fn cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    pub fn cancel(&self) {
        self.cancel.send_replace(true);
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
                a.server_id == server_id && a.claimed.load(Ordering::SeqCst) && !a.is_finished()
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
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let ids: Vec<_> = state
            .entries
            .iter()
            .filter(|(id, a)| a.server_id == server_id && keep != Some(id.as_str()))
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
            state.entries.retain(|_, a| !*a.done.borrow());
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
                state.entries.retain(|_, a| !a.is_finished());
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
            self.0.cancel();
            if !self.0.process_owned.load(Ordering::SeqCst) {
                self.0.finish();
            }
        }
    }
}
