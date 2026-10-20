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
