use super::*;
use crate::http::UnlockResponse;
struct Http(u16);
#[async_trait::async_trait]
impl UnlockHttp for Http {
    async fn request(&self, _: &UnlockRequest) -> UnlockResponse {
        UnlockResponse {
            status: self.0,
            body: "ordinary upstream response".into(),
            ..Default::default()
        }
    }
}
#[tokio::test]
async fn generic_error_pages_are_not_positive_unlock_evidence() {
    for code in [404, 429, 500, 503] {
        let http = Http(code);
        for result in [
            check_chatgpt(&http).await,
            check_netflix(&http).await,
            check_gemini(&http).await,
            check_tiktok(&http).await,
        ] {
            assert_eq!(result.status, UnlockStatus::Timeout, "HTTP {code}");
        }
    }
    assert_eq!(check_chatgpt(&Http(200)).await.status, UnlockStatus::Ok);
    assert_eq!(check_tiktok(&Http(200)).await.status, UnlockStatus::Ok);
}

struct Mixed;
#[async_trait::async_trait]
impl UnlockHttp for Mixed {
    async fn request(&self, req: &UnlockRequest) -> UnlockResponse {
        UnlockResponse {
            status: if req.url == ChatgptEndpoints::IOS_URL {
                503
            } else {
                200
            },
            body: String::new(),
            ..Default::default()
        }
    }
}
#[tokio::test]
async fn mixed_success_and_error_legs_are_unknown() {
    assert_eq!(check_chatgpt(&Mixed).await.status, UnlockStatus::Timeout);
}
struct BlockLanding;
#[async_trait::async_trait]
impl UnlockHttp for BlockLanding {
    async fn request(&self, _: &UnlockRequest) -> UnlockResponse {
        UnlockResponse {
            status: 403,
            redirect_chain: vec![crate::http::RedirectHop {
                status: 302,
                location: "https://www.tiktok.com/about".into(),
            }],
            ..Default::default()
        }
    }
}
#[tokio::test]
async fn explicit_tiktok_block_landing_keeps_existing_semantics() {
    assert_eq!(
        check_tiktok(&BlockLanding).await.status,
        UnlockStatus::Blocked
    );
}
