//! 传输层平台分叉的判据锁。
//!
//! 本模块在**宿主**（桌面 target）上跑，故 `stream_*` 三条谓词测的是桌面腿；Android 腿的
//! 对应事实由模拟器实测 + `scripts/check-android-bridge.mjs` 的 A6/A7/A8/A9 覆盖
//! （见 `~/docs/polaris/design/polaris-android-dataplane-2026-09-04.md`）。

use super::*;
use crate::runtime::proxy::ProxyStatus;
use polaris_stats_engine::Topic;

#[tokio::test]
async fn cancelled_native_handshake_drains_callback_before_next_generation() {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let (entered, started) = tokio::sync::oneshot::channel();
    let (callback, response) = tokio::sync::oneshot::channel::<()>();
    let (finished, drained) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(detached_open(&LOCK, move |guard| async move {
        entered.send(()).unwrap();
        response.await.unwrap();
        finished.send(()).unwrap();
        Some(guard)
    }));
    started.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(
        LOCK.try_lock().is_err(),
        "old generation still owns native stream"
    );
    // Mirrors Tauri's callback: sending after cancellation must still succeed.
    callback.send(()).unwrap();
    drained.await.unwrap();
    let next = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        detached_open(&LOCK, |guard| async move { Some(guard) }),
    )
    .await
    .unwrap();
    assert!(next.is_some(), "abandoned result releases its generation");
}

fn status(running: bool, port: u16) -> ProxyStatus {
    ProxyStatus {
        running,
        clash_api_port: port,
        ..Default::default()
    }
}

/// 🔴 **变异锁：五条渲染层 topic 在 Android 上必须各有供数通道。**
///
/// 少一条 = 对应的屏在 Android 上安静地一直空着，而那件事今天没有任何编译期证据（五条 emit
/// 都在共享 relay 里）。本测把 [`TOPIC_SOURCE`] 钉在 [`Topic`] 枚举上：新增一个 topic 而不给它
/// 声明供数通道 ⇒ 下面的穷举 `match` 编译不过（新变体）或断言转红（表里没有）。
///
/// **变异探针**：从 [`TOPIC_SOURCE`] 删掉任意一行 ⇒ 转红，且红在那条 topic 上。
#[test]
fn 五条topic在android上各有供数通道() {
    // 穷举 match：`Topic` 加一个变体，这里编译不过 —— 强制回来给它声明来源。
    let all = [
        Topic::Stats,
        Topic::Connections,
        Topic::Topology,
        Topic::Detail,
        Topic::Closed,
    ];
    for topic in all {
        let key = match topic {
            Topic::Stats => "stats",
            Topic::Connections => "aggregate",
            Topic::Topology => "topology",
            Topic::Detail => "detail",
            Topic::Closed => "closed",
        };
        let source = TOPIC_SOURCE.iter().find(|(t, _)| *t == key);
        assert!(
            source.is_some(),
            "topic `{key}` 在 TOPIC_SOURCE 里没有供数通道 —— 它在 Android 上会安静地一直空着"
        );
        let channel = source.unwrap().1;
        assert!(
            channel == STATS_LIBBOX_COMMAND || channel == CONNECTIONS_LIBBOX_COMMAND,
            "topic `{key}` 声明的通道 `{channel}` 不在本仓订阅的两条 libbox 通道里"
        );
    }
    assert_eq!(
        TOPIC_SOURCE.len(),
        all.len(),
        "TOPIC_SOURCE 与 Topic 的条数必须一致 —— 多出来的那条没有任何消费者"
    );
    // topic 名与前端 `STATS_TOPIC_EVENT` 的键逐字对齐（跨语言那一侧由契约门 A6 对拍）。
    assert!(
        TOPIC_SOURCE.iter().all(|(t, _)| !t.is_empty()),
        "topic 名不得为空 —— 空串在契约门那侧会匹配到任何东西"
    );
}

/// 桌面腿的建流前提：核在跑**且**管理口已知。
///
/// **变异探针**：把 `clash_api_port != 0` 那半条去掉 ⇒ 转红（端口 0 时会去连 127.0.0.1:0）。
#[test]
fn 桌面建流前提是核在跑且管理口已知() {
    assert!(stream_ready(&status(true, 9090)));
    assert!(!stream_ready(&status(true, 0)), "管理口未知不得建流");
    assert!(!stream_ready(&status(false, 9090)), "核没跑不得建流");
    assert_eq!(stream_port(&status(true, 9090)), 9090);
}

/// 桌面腿的断流判据：核停 或 换了管理口（换核 / 重启动态口）。
///
/// `ReconnectingStream` 断了自己重连、永不 yield 错误，这两件事只能靠复核发现 —— 判据松一分
/// 就是「换核后这条流永远重连到旧端口，两个页面静默冻结」。
#[test]
fn 桌面换管理口即断流() {
    assert!(stream_still_valid(&status(true, 9090), 9090));
    assert!(!stream_still_valid(&status(true, 9091), 9090), "换口须断流");
    assert!(
        !stream_still_valid(&status(false, 9090), 9090),
        "核停须断流"
    );
}

/// 桌面日志文案与换传输层之前逐字相同（本批不改桌面的任何可观测行为）。
#[test]
fn 桌面流标识文案不变() {
    assert_eq!(stream_label(9090), "port=9090");
}
