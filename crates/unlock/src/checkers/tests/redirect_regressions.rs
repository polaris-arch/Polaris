use super::*;
#[tokio::test]
async fn claude_resolves_every_relative_hop_against_its_actual_predecessor() {
    for (locations, expected) in [
        (vec!["/login"], UnlockStatus::Ok),
        (
            vec!["https://claude.ai/account/", "../login"],
            UnlockStatus::Ok,
        ),
        (
            vec!["https://www.anthropic.com/", "app-unavailable-in-region"],
            UnlockStatus::Blocked,
        ),
        (
            vec!["https://other.example/", "/login"],
            UnlockStatus::Timeout,
        ),
        (vec!["//claude.ai/login"], UnlockStatus::Ok),
    ] {
        let mut response = UnlockResponse::ok(200, "");
        response.redirect_chain = locations
            .into_iter()
            .map(|location| crate::http::RedirectHop {
                status: 302,
                location: location.into(),
            })
            .collect();
        let http = MockHttp(vec![("claude.ai", response)]);
        assert_eq!(check_claude(&http).await.status, expected);
    }
}
