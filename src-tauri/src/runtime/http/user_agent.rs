//! Shared default UA for subscriptions and application HTTP requests.
//! The kernel identity is the selected bundled build, not a live-process probe.
use std::sync::OnceLock;

/// Preserve caller-supplied subscription UAs and mesh's protocol-specific UA.
pub fn app_user_agent() -> String {
    static USER_AGENT: OnceLock<String> = OnceLock::new();
    USER_AGENT
        .get_or_init(|| {
            polaris_net_stack::subscription::default_subscription_user_agent(
                env!("CARGO_PKG_VERSION"),
                crate::runtime::core_version::selected_bundled_version(),
            )
        })
        .clone()
}

#[cfg(test)]
mod tests;
