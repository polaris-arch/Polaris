//! Per-subscription refresh ownership shared by IPC and the scheduler.
//! The mutex is held only for admission/release, never across network/parse/commit work.
//! Overlaps are rejected explicitly; different subscriptions remain independent.

use std::{collections::HashSet, sync::Mutex};

#[derive(Default)]
pub struct SubscriptionRefreshAdmission {
    active: Mutex<HashSet<String>>,
}

impl SubscriptionRefreshAdmission {
    pub fn try_admit(&self, id: &str) -> Option<SubscriptionRefreshOwner<'_>> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !active.insert(id.to_owned()) {
            return None;
        }
        Some(SubscriptionRefreshOwner {
            admission: self,
            id: id.to_owned(),
        })
    }
}

/// Dropping a cancelled future or returning through any failure releases ownership.
/// No completed IDs are retained, so the registry is bounded by active refreshes.
pub struct SubscriptionRefreshOwner<'a> {
    admission: &'a SubscriptionRefreshAdmission,
    id: String,
}

impl Drop for SubscriptionRefreshOwner<'_> {
    fn drop(&mut self) {
        self.admission
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.id);
    }
}

#[cfg(test)]
mod tests;
