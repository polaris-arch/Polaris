use super::*;

/// 未就绪实例：三个方法全部 NotReady（而非 panic / 静默 Ok）。
/// NotReady 是 executor 区分「核未起」与「PUT 报错」的依据，两者都退回重启但日志不同。
#[tokio::test]
async fn not_ready_client_returns_not_ready_for_all_methods() {
    let api = GrpcManagementApi::not_ready();
    assert!(matches!(
        api.select_outbound("proxy-selector", "tagB").await,
        Err(ManagementError::NotReady)
    ));
    assert!(matches!(
        api.close_connection("c1").await,
        Err(ManagementError::NotReady)
    ));
    assert!(matches!(
        api.first_connection_snapshot().await,
        Err(ManagementError::NotReady)
    ));
}

/// 读侧同样必须 NotReady 而**不是空快照**：空快照 = 「核确实没有 group」，NotReady = 「没读到」。
/// 压成前者会让起核自证把「读不到」误当成「查无此 group」，从而对真分叉保持沉默。
///
/// **变异锁**：把 `groups_snapshot` 的 `self.client()?` 换成 `unwrap_or_default()` 式回落
/// （返回 `Ok(vec![])`）→ 转红。
#[tokio::test]
async fn not_ready_client_returns_not_ready_for_groups_snapshot() {
    let api = GrpcManagementApi::not_ready();
    assert!(matches!(
        api.groups_snapshot().await,
        Err(ManagementError::NotReady)
    ));
}

/// SnapshotTimeout 必须单独成态，不得被压成 Call —— executor 对二者的处置不同
/// （超时 → 跳过断连；Call → 也跳过但日志语义不同），且上层据此判「核 wedged」。
#[test]
fn snapshot_timeout_maps_to_dedicated_variant() {
    assert!(matches!(
        map_err(ClientError::SnapshotTimeout),
        ManagementError::SnapshotTimeout
    ));
}

/// tonic Status → Call 且**保留原文**（丢了原文，真机 PUT 失败时无从定位是 Unauthenticated 还是 Unavailable）。
#[test]
fn tonic_status_maps_to_call_preserving_message() {
    let e = map_err(ClientError::Status(
        polaris_singbox_grpc::tonic::Status::unauthenticated("bad secret"),
    ));
    match e {
        ManagementError::Call(msg) => assert!(
            msg.contains("bad secret"),
            "必须保留 gRPC 原文，实得：{msg}"
        ),
        other => panic!("expected Call, got {other:?}"),
    }
}

// ── close_live_connections：逐条关闭活连接（TUN 起核 flush 与「关闭全部」共用）──────────────

/// 连接面测试替身：预设快照（或快照错误），记录每次 `close_connection` 的 id。
///
/// `ManagementApi` trait 面上**没有** close-all 方法，故「走 trait 就不可能调到 CloseAllConnections」是
/// 结构性的；本替身证明的是另一半 —— 每条活连接恰好被单条关闭一次、幽灵被跳过。
pub(crate) struct FakeConnectionApi {
    snapshot: Result<Vec<ConnectionSnapshot>, ManagementError>,
    /// 对该 id 的 `close_connection` 注入失败（验证单条失败不中断、不计数）。
    reject_id: Option<String>,
    /// `Arc`：flush 测试要把替身按值交给 `connect` 闭包，事后仍需读回记录。
    pub(crate) closed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl FakeConnectionApi {
    /// `(id, closed_at)` 列表作为快照。
    pub(crate) fn with_snapshot(conns: &[(&str, i64)]) -> Self {
        Self {
            snapshot: Ok(conns
                .iter()
                .map(|(id, closed_at)| ConnectionSnapshot {
                    id: (*id).to_string(),
                    chains: Vec::new(),
                    closed_at: *closed_at,
                })
                .collect()),
            reject_id: None,
            closed: Default::default(),
        }
    }

    /// 快照本身失败。
    pub(crate) fn snapshot_err(e: ManagementError) -> Self {
        Self {
            snapshot: Err(e),
            reject_id: None,
            closed: Default::default(),
        }
    }

    pub(crate) fn rejecting(mut self, id: &str) -> Self {
        self.reject_id = Some(id.to_string());
        self
    }

    /// 排序后的已关闭 id（并发关闭不保证调用顺序，断言集合而非次序）。
    pub(crate) fn closed_ids(&self) -> Vec<String> {
        let mut ids = self.closed.lock().unwrap().clone();
        ids.sort();
        ids
    }
}

#[async_trait::async_trait]
impl ManagementApi for FakeConnectionApi {
    async fn select_outbound(&self, _: &str, _: &str) -> Result<(), ManagementError> {
        panic!("关闭连接路径不得切 selector");
    }

    async fn close_connection(&self, id: &str) -> Result<(), ManagementError> {
        self.closed.lock().unwrap().push(id.to_string());
        if self.reject_id.as_deref() == Some(id) {
            return Err(ManagementError::Call("injected close failure".to_string()));
        }
        Ok(())
    }

    async fn first_connection_snapshot(&self) -> Result<Vec<ConnectionSnapshot>, ManagementError> {
        self.snapshot.clone()
    }
}

/// 🔴 每条活连接恰好单条关闭一次，`closed_at > 0` 的幽灵跳过，返回值 = 关闭条数（failed=0）。
///
/// **变异锁**：去掉 `closed_at <= 0` 过滤 → 幽灵 `ghost` 被关、计数 3 → 转红。
#[tokio::test]
async fn close_live_connections_closes_each_live_once_and_skips_ghosts() {
    let api = FakeConnectionApi::with_snapshot(&[("a", 0), ("ghost", 1_000_000_000), ("b", -1)]);
    let outcome = close_live_connections(&api).await.expect("快照成功");
    assert_eq!(api.closed_ids(), vec!["a".to_string(), "b".to_string()]);
    assert_eq!(
        outcome,
        CloseLiveOutcome {
            closed: 2,
            failed: 0
        }
    );
}

/// 🔴 单条失败只跳过自己：其余照关，失败那条计入 `failed` 而非 `closed`。
///
/// **变异锁**：把 `failed` 恒置 0（或把失败计进 `closed`）→ 转红。
#[tokio::test]
async fn close_live_connections_counts_single_failure_without_stopping() {
    let api = FakeConnectionApi::with_snapshot(&[("a", 0), ("b", 0), ("c", 0)]).rejecting("b");
    let outcome = close_live_connections(&api).await.expect("快照成功");
    assert_eq!(api.closed_ids(), vec!["a", "b", "c"]);
    assert_eq!(
        outcome,
        CloseLiveOutcome {
            closed: 2,
            failed: 1
        },
        "失败那条必须计入 failed、不得计入 closed"
    );
}

/// 并发上限之外的连接也全部关到（40 条 > 上限 16），集合与计数不变。
#[tokio::test]
async fn close_live_connections_closes_all_beyond_concurrency_limit() {
    let ids: Vec<String> = (0..40).map(|i| format!("c{i:02}")).collect();
    let conns: Vec<(&str, i64)> = ids.iter().map(|id| (id.as_str(), 0)).collect();
    let api = FakeConnectionApi::with_snapshot(&conns);
    let outcome = close_live_connections(&api).await.expect("快照成功");
    assert_eq!(api.closed_ids(), ids);
    assert_eq!(
        outcome,
        CloseLiveOutcome {
            closed: 40,
            failed: 0
        }
    );
}

/// 快照失败原样上抛（SnapshotTimeout 不得被压成 Ok(0)），且不关任何连接。
#[tokio::test]
async fn close_live_connections_propagates_snapshot_timeout() {
    let api = FakeConnectionApi::snapshot_err(ManagementError::SnapshotTimeout);
    assert!(matches!(
        close_live_connections(&api).await,
        Err(ManagementError::SnapshotTimeout)
    ));
    assert!(api.closed_ids().is_empty());
}
