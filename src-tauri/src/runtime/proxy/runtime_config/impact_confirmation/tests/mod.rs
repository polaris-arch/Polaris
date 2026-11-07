use super::*;

fn candidate(runtime: &Arc<ProxyRuntime>) -> PendingOperation {
    runtime.bump_generation();
    runtime
        .config
        .save_full(&super::super::tests::runnable_config_fixture())
        .unwrap();
    let raw = runtime.config.current().unwrap();
    *runtime.current_config.write().unwrap() = Some(raw.clone());
    let mut pending = super::super::tests::ts_pending_raw_fixture(runtime, raw);
    let mut capabilities = super::super::tests::paired_capabilities();
    capabilities.supported_client_impact_policy_versions = vec![1];
    pending.pairing = super::super::observe_runtime_pairing(&capabilities).unwrap();
    pending.plan.plan_id = "private-original-plan".into();
    pending.plan.client_impact_policy_version = 1;
    pending.plan.impact_confirmation_required = true;
    pending.plan.expires_at_unix_milli = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60_000) as i64;
    pending
}

#[tokio::test]
async fn cancel_and_wrong_token_never_admit_or_replace_the_retained_plan() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let pending = candidate(&runtime);
    let result = runtime.prepare_runtime_impact_choice(pending);
    assert_eq!(result.status, "impactConfirmationRequired");
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("private-original-plan"));
    let token = result.impact_confirmation.unwrap();
    assert_eq!(
        runtime
            .confirm_runtime_impact_choice("wrong".into())
            .await
            .status,
        "conflict"
    );
    {
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert_eq!(
            state.impact.as_ref().unwrap().pending.plan.plan_id,
            "private-original-plan"
        );
        assert!(state.pending.is_none());
    }
    assert_eq!(
        runtime.cancel_runtime_impact_choice(&token).status,
        "cancelled"
    );
    assert_eq!(
        runtime.confirm_runtime_impact_choice(token).await.status,
        "conflict"
    );
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .pending
        .is_none());
}

#[tokio::test]
async fn expired_choice_and_changed_desired_running_generation_or_intent_fail_before_rpc() {
    for drift in 0..5 {
        let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
        let pending = candidate(&runtime);
        let result = runtime.prepare_runtime_impact_choice(pending);
        let token = result.impact_confirmation.expect("real retained choice");
        match drift {
            0 => {
                runtime
                    .runtime_config_apply
                    .lock()
                    .unwrap()
                    .impact
                    .as_mut()
                    .unwrap()
                    .expires = Instant::now() - Duration::from_secs(1);
            }
            1 => {
                let mut newer = runtime.config.current().unwrap();
                newer["mixedPort"] = serde_json::json!(3090);
                runtime.config.save_full(&newer).unwrap();
            }
            2 => {
                *runtime.current_config.write().unwrap() =
                    Some(serde_json::json!({"newRuntime": true}));
            }
            3 => {
                runtime.bump_generation();
            }
            _ => {
                runtime.register_selector_intent();
            }
        }
        assert_eq!(
            runtime.confirm_runtime_impact_choice(token).await.status,
            "conflict"
        );
        let state = runtime.runtime_config_apply.lock().unwrap();
        assert!(state.impact.is_none());
        assert!(state.pending.is_none());
    }
}

#[test]
fn already_expired_kernel_plan_does_not_create_a_confirmation_choice() {
    let (runtime, _dir, _calls) = crate::runtime::proxy::tests::test_runtime_recording();
    let mut pending = candidate(&runtime);
    pending.plan.expires_at_unix_milli = 1;
    assert_eq!(
        runtime.prepare_runtime_impact_choice(pending).status,
        "conflict"
    );
    assert!(runtime
        .runtime_config_apply
        .lock()
        .unwrap()
        .impact
        .is_none());
}
