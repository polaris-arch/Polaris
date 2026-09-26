//! Request identity and cancellation are independent of the process/state-directory gate.

use std::collections::HashMap;
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
pub(super) struct Attempts(Mutex<HashMap<String, Arc<Attempt>>>);

impl Attempts {
    pub fn owns_state(&self, server_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|a| {
                a.server_id == server_id && a.claimed.load(Ordering::SeqCst) && !a.is_finished()
            })
    }

    pub fn can_preserve(&self, server_id: &str, id: &str) -> bool {
        self.get(server_id, id)
            .is_ok_and(|a| !a.claimed.load(Ordering::SeqCst) && !a.is_finished() && !a.cancelled())
    }

    pub async fn cancel_node_except(&self, server_id: &str, keep: Option<&str>) {
        let attempts: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(id, a)| a.server_id == server_id && keep != Some(id.as_str()))
            .map(|(_, a)| a.clone())
            .collect();
        for attempt in &attempts {
            attempt.cancel();
            if !attempt.claimed.load(Ordering::SeqCst) {
                attempt.finish();
            }
        }
        for attempt in attempts {
            attempt.finished().await;
        }
    }

    pub fn get(&self, server_id: &str, id: &str) -> Result<Arc<Attempt>, String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .filter(|a| a.server_id == server_id)
            .cloned()
            .ok_or_else(|| "Login request expired or was not prepared".into())
    }
    // Retain cancelled IDs long enough to fence a delayed start IPC, with a hard memory bound.
    pub fn prepare(&self, server_id: &str, id: &str) -> Result<Arc<Attempt>, String> {
        if id.is_empty() || id.len() > 128 {
            return Err("Invalid login request identity".into());
        }
        let mut entries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(attempt) = entries.get(id) {
            return if attempt.server_id == server_id {
                Ok(attempt.clone())
            } else {
                Err("Login request belongs to another node".into())
            };
        }
        if entries.len() >= 512 {
            entries.retain(|_, a| !*a.done.borrow());
        }
        if entries.len() >= 512 {
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
        entries.insert(id.into(), attempt.clone());
        Ok(attempt)
    }

    pub fn cancel(&self, server_id: &str, id: &str) -> Result<Arc<Attempt>, String> {
        let attempt = self.prepare(server_id, id)?;
        attempt.cancel();
        if !attempt.claimed.load(Ordering::SeqCst) {
            attempt.finish();
        }
        Ok(attempt)
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
