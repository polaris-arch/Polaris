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
    assert!(admission.active.lock().unwrap().active.is_empty());
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

#[tokio::test]
async fn saved_edit_supersedes_old_work_without_old_drop_releasing_new_owner() {
    let admission = SubscriptionRefreshAdmission::default();
    let old = admission.try_admit("s").unwrap();
    let old_token = old.token();
    let new = admission.supersede("s");
    old.cancelled().await;
    assert!(!old_token.is_current());
    assert!(new.token().is_current());
    assert!(new.token().generation() > old_token.generation());
    drop(old);
    assert!(admission.try_admit("s").is_none());
    assert!(new.token().is_current());
    drop(new);
    assert!(admission.try_admit("s").is_some());
}

#[tokio::test]
async fn repeated_edits_and_aba_cannot_publish_old_progress_or_commit() {
    let admission = SubscriptionRefreshAdmission::default();
    let a = admission.try_admit("s").unwrap();
    let b = admission.supersede("s");
    let a_again = admission.supersede("s");
    a.cancelled().await;
    b.cancelled().await;
    assert_eq!(a.token().with_current(|| "old-a"), None);
    assert_eq!(b.token().with_current(|| "old-b"), None);
    assert_eq!(
        a_again.token().with_current(|| "latest-a"),
        Some("latest-a")
    );
    let other = admission.try_admit("other").unwrap();
    assert!(other.token().is_current());
}
