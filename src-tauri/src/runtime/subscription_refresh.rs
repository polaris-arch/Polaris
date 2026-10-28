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
mod tests {
    use super::*;
    use std::future::Future;

    #[test]
    fn overlap_is_rejected_until_owner_finishes_and_other_ids_are_independent() {
        let admission = SubscriptionRefreshAdmission::default();
        let scheduler = admission.try_admit("s").unwrap();
        assert!(admission.try_admit("s").is_none()); // manual cannot start an inverted response
        let unrelated = admission.try_admit("other").unwrap();
        drop(scheduler);
        let manual = admission.try_admit("s").unwrap();
        assert!(admission.try_admit("s").is_none());
        drop(manual);
        drop(unrelated);
        assert!(admission.active.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelling_an_in_flight_refresh_releases_ownership() {
        let admission = SubscriptionRefreshAdmission::default();
        let mut refresh = Box::pin(async {
            let _owner = admission.try_admit("s").unwrap();
            std::future::pending::<()>().await;
        });
        // Poll once to acquire ownership without timers, tasks, network, or a device.
        std::future::poll_fn(|cx| {
            assert!(refresh.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(admission.try_admit("s").is_none());
        drop(refresh);
        assert!(admission.try_admit("s").is_some());
    }
}
