use super::super::{with_fresh_hop_guard, with_hop_guard};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[tokio::test]
async fn revoked_manual_guard_never_polls_transport_io() {
    let calls = AtomicUsize::new(0);
    let result = with_hop_guard(&|| Err("superseded".into()), || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(200)
    })
    .await;
    assert_eq!(result.unwrap_err(), "superseded");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stop_during_send_discards_the_response_before_redirect_can_continue() {
    let current = AtomicBool::new(true);
    let guard = || {
        if current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("superseded".into())
        }
    };
    let calls = AtomicUsize::new(0);
    let result = with_hop_guard(&guard, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        current.store(false, Ordering::SeqCst);
        Ok("302 Location: /next")
    })
    .await;
    assert_eq!(result.unwrap_err(), "superseded");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stop_between_redirect_hops_prevents_the_next_send_even_at_the_same_port() {
    let current = AtomicBool::new(true);
    let guard = || {
        if current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("configurationPending".into())
        }
    };
    let calls = AtomicUsize::new(0);
    let first = with_hop_guard(&guard, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(302)
    })
    .await
    .unwrap();
    assert_eq!(first, 302);
    current.store(false, Ordering::SeqCst);
    let second = with_hop_guard(&guard, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(200)
    })
    .await;
    assert_eq!(second.unwrap_err(), "configurationPending");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn binding_lost_during_terminal_body_read_discards_body() {
    let current = AtomicBool::new(true);
    let guard = || {
        if current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("readyUnknown".into())
        }
    };
    let result = with_hop_guard(&guard, || async {
        current.store(false, Ordering::SeqCst);
        Ok("successful private body")
    })
    .await;
    assert_eq!(result.unwrap_err(), "readyUnknown");
}

#[test]
fn production_redirect_send_and_terminal_body_use_the_same_guarded_boundary() {
    let source = polaris_source_probe::crate_source!("lib.rs");
    let start = source.find("    pub async fn request_guarded(").unwrap();
    let body = source[start..].split("\n#[async_trait]").next().unwrap();
    assert_eq!(body.matches("with_fresh_hop_guard(").count(), 1);
    assert_eq!(body.matches("with_hop_guard(").count(), 1);
    assert_eq!(body.matches("builder.send()").count(), 1);
    let first_guard = body.find("with_fresh_hop_guard(").unwrap();
    let send = body.find("builder.send()").unwrap();
    let second_guard = body.rfind("with_hop_guard(").unwrap();
    let read = body.find("read_body_truncating(").unwrap();
    assert!(first_guard < send && send < second_guard && second_guard < read);
    assert!(
        read < body
            .find("validate().await.and_then(|()| guard())")
            .unwrap()
    );
    let ordinary = source
        .split("impl UnlockHttp for UnlockClient {")
        .nth(1)
        .unwrap();
    assert!(ordinary.contains("self.request_guarded(req, &|| Ok(())).await"));
}

#[tokio::test]
async fn second_redirect_requires_fresh_native_validation_before_send() {
    let validations = AtomicUsize::new(0);
    let sends = AtomicUsize::new(0);
    let fresh = || async {
        if validations.fetch_add(1, Ordering::SeqCst) >= 2 {
            Err("readyUnknown".into())
        } else {
            Ok(())
        }
    };
    let first = with_fresh_hop_guard(&|| Ok(()), &fresh, || async {
        sends.fetch_add(1, Ordering::SeqCst);
        Ok(302)
    })
    .await
    .unwrap();
    assert_eq!(first, 302);
    let second = with_fresh_hop_guard(&|| Ok(()), &fresh, || async {
        sends.fetch_add(1, Ordering::SeqCst);
        Ok(200)
    })
    .await;
    assert_eq!(second.unwrap_err(), "readyUnknown");
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(validations.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn fresh_validation_after_send_rejects_a_response_with_valid_sync_fence() {
    let validations = AtomicUsize::new(0);
    let result = with_fresh_hop_guard(
        &|| Ok(()),
        &|| async {
            if validations.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                Err("readyUnknown".into())
            }
        },
        || async { Ok(200) },
    )
    .await;
    assert_eq!(result.unwrap_err(), "readyUnknown");
}
