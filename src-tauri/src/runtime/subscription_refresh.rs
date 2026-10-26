//! Per-subscription refresh ownership shared by IPC and the scheduler.
//! An edit publishes a replacement only after persistence, under the config writer lock.
//! Registry operations never acquire the configuration lock or perform network work.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

#[derive(Default)]
struct RefreshState {
    next_generation: u64,
    active: HashMap<String, (u64, watch::Sender<bool>)>,
}

#[derive(Default)]
pub struct SubscriptionRefreshAdmission {
    active: Arc<Mutex<RefreshState>>,
}

impl SubscriptionRefreshAdmission {
    pub fn try_admit(&self, id: &str) -> Option<SubscriptionRefreshOwner> {
        self.admit(id, false)
    }

    /// Called only after the edit is durably saved, before releasing the config writer.
    pub fn supersede(&self, id: &str) -> SubscriptionRefreshOwner {
        self.admit(id, true)
            .expect("replacement admission cannot overlap-reject")
    }

    fn admit(&self, id: &str, replace: bool) -> Option<SubscriptionRefreshOwner> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !replace && active.active.contains_key(id) {
            return None;
        }
        active.next_generation = active
            .next_generation
            .checked_add(1)
            .expect("subscription refresh generation exhausted");
        let generation = active.next_generation;
        let (cancel, receiver) = watch::channel(false);
        if let Some((_, previous)) = active.active.insert(id.to_owned(), (generation, cancel)) {
            let _ = previous.send(true);
        }
        Some(SubscriptionRefreshOwner {
            token: SubscriptionRefreshToken {
                active: Arc::clone(&self.active),
                id: id.to_owned(),
                generation,
            },
            cancel: receiver,
        })
    }
}

#[derive(Clone)]
pub struct SubscriptionRefreshToken {
    active: Arc<Mutex<RefreshState>>,
    id: String,
    generation: u64,
}

impl SubscriptionRefreshToken {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn subscription_id(&self) -> &str {
        &self.id
    }

    pub fn is_current(&self) -> bool {
        self.with_current(|| ()).is_some()
    }

    /// Small synchronous publication only. Never call configuration/network code in this closure.
    pub fn with_current<T>(&self, publish: impl FnOnce() -> T) -> Option<T> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.active.get(&self.id).map(|entry| entry.0) != Some(self.generation) {
            return None;
        }
        Some(publish())
    }
}

pub struct SubscriptionRefreshOwner {
    token: SubscriptionRefreshToken,
    cancel: watch::Receiver<bool>,
}

impl SubscriptionRefreshOwner {
    pub fn token(&self) -> SubscriptionRefreshToken {
        self.token.clone()
    }

    pub async fn cancelled(&self) {
        let mut cancel = self.cancel.clone();
        loop {
            if *cancel.borrow_and_update() {
                return;
            }
            if cancel.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Drop for SubscriptionRefreshOwner {
    fn drop(&mut self) {
        let mut active = self
            .token
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.active.get(&self.token.id).map(|entry| entry.0) == Some(self.token.generation) {
            active.active.remove(&self.token.id);
        }
    }
}

#[cfg(test)]
mod tests;
