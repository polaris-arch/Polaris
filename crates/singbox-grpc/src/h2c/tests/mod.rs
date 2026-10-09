//! 拨号期限与整次调用期限。除「接受连接但不应答」那一条用本机回环上的真 socket 之外，其余全靠
//! 注入的拨号器：对 SYN 毫无响应的对端不出回环造不出来，这里用一个永不完成的拨号替它。

use super::*;
use crate::{ClientError, SingBoxApiClient, CONNECT_TIMEOUT, UNARY_DEADLINE};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tonic::Code;

fn client_dialing_through(dial: Dialer) -> SingBoxApiClient {
    SingBoxApiClient {
        channel: connect_h2c_with("127.0.0.1:1", dial).unwrap(),
        target: "127.0.0.1:1".into(),
        secret: None,
    }
}

/// 永不完成的拨号，并数它被调了几次。
fn silent_dialer() -> (Dialer, Arc<AtomicUsize>) {
    let dials = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&dials);
    let dial: Dialer = Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    });
    (dial, dials)
}

/// 接受连接、读走请求、但一个字节都不回的对端。返回它的地址；任务随运行时结束。
async fn accepting_but_mute_listener() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    addr
}

fn status_code(error: &ClientError) -> Option<Code> {
    match error {
        ClientError::Status(status) => Some(status.code()),
        _ => None,
    }
}

#[test]
fn a_dial_gets_less_than_the_whole_unary_deadline() {
    assert!(CONNECT_TIMEOUT < UNARY_DEADLINE);
}

/// 没有 unary 期限的调用（这里是取件流的建流）也不会陪着一次无响应的拨号挂下去：拨号在
/// [`CONNECT_TIMEOUT`] 被切断，报「连不上」。
#[tokio::test(start_paused = true)]
async fn a_dial_nobody_answers_is_cut_at_the_connect_limit() {
    let (dial, dials) = silent_dialer();
    let client = client_dialing_through(dial);
    let started = tokio::time::Instant::now();
    let error = client
        .download_taildrop_file("ts", "file")
        .await
        .expect_err("拨号无响应");
    let waited = started.elapsed();
    assert_eq!(dials.load(Ordering::SeqCst), 1, "走的是注入的拨号器");
    assert_eq!(status_code(&error), Some(Code::Unavailable), "{error}");
    assert!(
        waited >= CONNECT_TIMEOUT && waited < CONNECT_TIMEOUT + Duration::from_millis(100),
        "等了 {waited:?}"
    );
}

/// 同一种对端上的 unary 调用：在自己的期限之内收场，而且是以「连不上」收场。
#[tokio::test(start_paused = true)]
async fn a_unary_call_over_a_silent_dial_ends_inside_its_deadline() {
    let (dial, dials) = silent_dialer();
    let client = client_dialing_through(dial);
    let started = tokio::time::Instant::now();
    let error = client
        .select_outbound("selector", "member")
        .await
        .expect_err("拨号无响应");
    let waited = started.elapsed();
    assert_eq!(dials.load(Ordering::SeqCst), 1, "走的是注入的拨号器");
    assert_eq!(status_code(&error), Some(Code::Unavailable), "{error}");
    assert!(waited < UNARY_DEADLINE, "等了 {waited:?}");
}

/// 接受连接但不应答的对端：调用在期限到时报到期，不挂。走生产的拨号与生产的 `connect`。
#[tokio::test]
async fn a_unary_call_to_an_accepting_but_mute_peer_expires_at_the_deadline() {
    let addr = accepting_but_mute_listener().await;
    let (host, port) = addr.rsplit_once(':').unwrap();
    let client = SingBoxApiClient::connect(crate::Endpoint::new(host, port.parse().unwrap()), "")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let error = client
        .select_outbound("selector", "member")
        .await
        .expect_err("对端不应答");
    let waited = started.elapsed();
    assert_eq!(status_code(&error), Some(Code::DeadlineExceeded), "{error}");
    assert!(
        waited >= UNARY_DEADLINE && waited < UNARY_DEADLINE + Duration::from_millis(500),
        "等了 {waited:?}"
    );
}

/// 期限从发起起算，不从连上起算：拨号用掉的时间不会另外加在应答的等待之上。拨号先耗掉
/// 大半个 [`CONNECT_TIMEOUT`] 才连上一个不应答的对端，整次调用仍在 [`UNARY_DEADLINE`] 收场。
#[tokio::test]
async fn time_spent_dialing_counts_against_the_unary_deadline() {
    let addr = accepting_but_mute_listener().await;
    let spent_dialing = CONNECT_TIMEOUT - Duration::from_millis(200);
    let dial: Dialer = Arc::new(move |_| {
        let addr = addr.clone();
        Box::pin(async move {
            tokio::time::sleep(spent_dialing).await;
            TcpStream::connect(addr).await
        })
    });
    let client = client_dialing_through(dial);
    let started = std::time::Instant::now();
    let error = client
        .select_outbound("selector", "member")
        .await
        .expect_err("对端不应答");
    let waited = started.elapsed();
    assert_eq!(status_code(&error), Some(Code::DeadlineExceeded), "{error}");
    // 不把拨号算进去的话，这里会是 `spent_dialing + UNARY_DEADLINE`。
    assert!(
        waited >= UNARY_DEADLINE && waited < UNARY_DEADLINE + spent_dialing / 2,
        "等了 {waited:?}"
    );
}
