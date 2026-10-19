use super::*;

#[test]
fn default_ua_uses_selected_build_without_starting_core() {
    assert_eq!(
        app_user_agent(),
        polaris_net_stack::subscription::default_subscription_user_agent(
            env!("CARGO_PKG_VERSION"),
            crate::runtime::core_version::selected_bundled_version(),
        )
    );
}
