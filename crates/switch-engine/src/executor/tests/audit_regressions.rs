use super::*;
struct Api(bool);
#[async_trait]
impl ManagementApi for Api {
    async fn select_outbound(&self, _: &str, _: &str) -> Result<(), ManagementError> {
        Ok(())
    }
    async fn close_connection(&self, _: &str) -> Result<(), ManagementError> {
        panic!("empty/failed snapshot must not close anything")
    }
    async fn first_connection_snapshot(&self) -> Result<Vec<ConnectionSnapshot>, ManagementError> {
        if self.0 {
            Err(ManagementError::SnapshotTimeout)
        } else {
            Ok(vec![])
        }
    }
}
#[tokio::test]
async fn failed_snapshot_is_distinct_from_successful_empty_snapshot() {
    let puts = vec![polaris_config_engine::builder::hotswitch::HotSwitchPut {
        selector_tag: "proxy-selector".into(),
        member_tag: "new".into(),
        old_member_tag: Some("old".into()),
    }];
    let failed = SwitchExecutor.precision_disconnect(&Api(true), &puts).await;
    let empty = SwitchExecutor
        .precision_disconnect(&Api(false), &puts)
        .await;
    assert!(failed.snapshot_error.is_some());
    assert!(empty.snapshot_error.is_none());
    assert!(failed.closed_ids.is_empty() && empty.closed_ids.is_empty());
    assert_ne!(failed, empty);
}
