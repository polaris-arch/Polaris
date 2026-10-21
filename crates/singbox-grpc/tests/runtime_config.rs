//! Authenticated loopback mock coverage only; not evidence of kernel implementation.
use polaris_singbox_grpc::{
    daemon::{self, runtime_config_service_server::{RuntimeConfigService, RuntimeConfigServiceServer}},
    ClientError, Endpoint, RuntimeConfigError, SingBoxApiClient, UNARY_DEADLINE,
    RUNTIME_CONFIG_MAX_MESSAGE_BYTES,
};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use tonic::{Request, Response, Status, Code};

#[derive(Clone, Default)]
struct Mock {
    code: Option<Code>,
    stall_apply: bool,
    stall_retry: bool,
    no_op: bool,
    calls: Arc<AtomicUsize>,
}
impl Mock {
    fn check<T>(&self, request: &Request<T>) -> Result<(), Status> {
        if request.metadata().get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer secret") {
            return Err(Status::unauthenticated("missing bearer"));
        }
        assert!(request.metadata().get("grpc-timeout").is_some());
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(code) = self.code { return Err(Status::new(code, "scripted")); }
        Ok(())
    }
}
fn identity() -> daemon::RuntimeIdentity {
    daemon::RuntimeIdentity { incarnation: "instance-1".into(), applied_revision: 12, effective_digest: "before".into() }
}
fn plan_request() -> daemon::PlanRuntimeConfigRequest {
    daemon::PlanRuntimeConfigRequest { contract_version: 1, expected: Some(identity()),
        config_content: "{}".into(), overrides: Some(daemon::RuntimeOverrideOptions {
            auto_redirect: true, include_package: vec!["app.one".into()], exclude_package: vec!["app.two".into()],
        }), expected_control_revision: 7,
        selector_intents: vec![daemon::RuntimeSelectorIntent {
            group: "default".into(), selected: "physical-tcp-winner".into(), pin_current_tcp: true,
            expected_selection_path: vec!["default".into(), "auto".into(), "physical-tcp-winner".into()],
        }] }
}
fn apply_request() -> daemon::ApplyRuntimeConfigRequest {
    daemon::ApplyRuntimeConfigRequest { contract_version: 1, transaction_id: "transaction".into(),
        plan_id: "plan".into(), expected: Some(identity()), candidate_digest: "candidate".into(),
        accepted_impact_digest: "impact".into(), expected_control_revision: 7 }
}
fn status_request() -> daemon::RuntimeConfigStatusRequest {
    daemon::RuntimeConfigStatusRequest { contract_version: 1, transaction_id: "transaction".into(), runtime_incarnation: "instance-1".into() }
}
fn retry_request() -> daemon::RuntimePersistenceRetryRequest {
    daemon::RuntimePersistenceRetryRequest { contract_version: 1, transaction_id: "transaction".into(),
        active: Some(identity()), control_revision: 8, persistence_mode: "app_authoritative_regeneration".into() }
}
fn acknowledgement_request() -> daemon::RuntimePersistenceAcknowledgementRequest {
    daemon::RuntimePersistenceAcknowledgementRequest { contract_version: 1, transaction_id: "transaction".into(),
        active: Some(identity()), control_revision: 8, persistence_mode: "app_authoritative_regeneration".into(),
        desired_state_digest: "desired-digest".into(), desired_state_revision: "desired-revision".into() }
}
#[tonic::async_trait]
impl RuntimeConfigService for Mock {
    async fn get_runtime_capabilities(&self, request: Request<()>) -> Result<Response<daemon::RuntimeCapabilities>, Status> {
        self.check(&request)?;
        Ok(Response::new(daemon::RuntimeCapabilities { contract_version: 1, connected_hot_enabled: true,
            active: Some(identity()), full_effective_schema_supported: false, control_revision: 7, ..Default::default() }))
    }
    async fn plan_runtime_config(&self, request: Request<daemon::PlanRuntimeConfigRequest>) -> Result<Response<daemon::RuntimeConfigPlan>, Status> {
        self.check(&request)?;
        assert_eq!(request.into_inner(), plan_request());
        Ok(Response::new(daemon::RuntimeConfigPlan { plan_id: "plan".into(), transaction_id: "transaction".into(),
            candidate_digest: "candidate".into(), impact_digest: "impact".into(), expected_control_revision: 7,
            applicable: true, no_op: self.no_op, ..Default::default() }))
    }
    async fn apply_runtime_config(&self, request: Request<daemon::ApplyRuntimeConfigRequest>) -> Result<Response<daemon::RuntimeConfigOperation>, Status> {
        self.check(&request)?;
        assert_eq!(request.into_inner(), apply_request());
        if self.stall_apply { std::future::pending::<()>().await; }
        Ok(Response::new(daemon::RuntimeConfigOperation { transaction_id: "transaction".into(),
            state: if self.no_op { daemon::RuntimeOperationState::Committed as i32 } else { daemon::RuntimeOperationState::CommittedDurabilityPending as i32 }, durable: self.no_op,
            before: Some(identity()), active: Some(identity()), control_revision: if self.no_op { 7 } else { 8 }, ..Default::default() }))
    }
    async fn retry_runtime_persistence(&self, request: Request<daemon::RuntimePersistenceRetryRequest>) -> Result<Response<daemon::RuntimeConfigOperation>, Status> {
        self.check(&request)?;
        assert_eq!(request.into_inner(), retry_request());
        if self.stall_retry { std::future::pending::<()>().await; }
        Ok(Response::new(daemon::RuntimeConfigOperation { contract_version: 1, transaction_id: "transaction".into(),
            state: daemon::RuntimeOperationState::CommittedDurabilityPending as i32, durable: false,
            active: Some(identity()), control_revision: 8, persistence_mode: "app_authoritative_regeneration".into(),
            owner_reload_acknowledged: true, ..Default::default() }))
    }
    async fn acknowledge_runtime_persistence(&self, request: Request<daemon::RuntimePersistenceAcknowledgementRequest>) -> Result<Response<daemon::RuntimeConfigOperation>, Status> {
        self.check(&request)?;
        assert_eq!(request.into_inner(), acknowledgement_request());
        Ok(Response::new(daemon::RuntimeConfigOperation { contract_version: 1, transaction_id: "transaction".into(),
            state: daemon::RuntimeOperationState::Committed as i32, durable: true, active: Some(identity()), control_revision: 8,
            persistence_mode: "app_authoritative_regeneration".into(), owner_reload_acknowledged: true,
            desired_state_digest: "desired-digest".into(), desired_state_revision: "desired-revision".into(),
            ..Default::default() }))
    }
    async fn get_runtime_config_status(&self, request: Request<daemon::RuntimeConfigStatusRequest>) -> Result<Response<daemon::RuntimeConfigStatus>, Status> {
        self.check(&request)?;
        assert_eq!(request.into_inner(), status_request());
        Ok(Response::new(daemon::RuntimeConfigStatus { active: Some(identity()),
            lookup_state: daemon::RuntimeLookupState::UnknownOrExpired as i32, control_revision: 8,
            persistence_mode: "app_authoritative_regeneration".into(), durable_desired_state_digest: "desired-digest".into(),
            durable_desired_state_revision: "desired-revision".into(), durable_owner_reload_acknowledged: true,
            ..Default::default() }))
    }
}
struct Running(tokio::task::JoinHandle<()>);
impl Drop for Running { fn drop(&mut self) { self.0.abort(); } }
async fn start(mock: Mock, secret: &str) -> (SingBoxApiClient, Running) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder().add_service(RuntimeConfigServiceServer::new(mock))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)).await.unwrap();
    });
    (SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret).await.unwrap(), Running(task))
}
#[tokio::test]
async fn authenticated_round_trip_preserves_partial_support_and_operation_states() {
    let mock = Mock::default(); let calls = mock.calls.clone();
    let (client, _server) = start(mock, "secret").await;
    let capabilities = client.get_runtime_capabilities().await.unwrap();
    assert!(capabilities.connected_hot_enabled);
    assert!(!capabilities.full_effective_schema_supported);
    assert_eq!(capabilities.active, Some(identity()));
    let plan = client.plan_runtime_config(plan_request()).await.unwrap();
    assert_eq!(plan.transaction_id, "transaction"); assert!(plan.applicable);
    let operation = client.apply_runtime_config(apply_request()).await.unwrap();
    assert_eq!(operation.state, daemon::RuntimeOperationState::CommittedDurabilityPending as i32);
    assert!(!operation.durable);
    let status = client.get_runtime_config_status(status_request()).await.unwrap();
    assert_eq!(status.lookup_state, daemon::RuntimeLookupState::UnknownOrExpired as i32);
    assert!(status.operation.is_none());
    assert_eq!(status.persistence_mode, "app_authoritative_regeneration");
    assert_eq!(status.durable_desired_state_digest, "desired-digest");
    assert_eq!(status.durable_desired_state_revision, "desired-revision");
    assert!(status.durable_owner_reload_acknowledged);
    let retried = client.retry_runtime_persistence(retry_request()).await.unwrap();
    assert_eq!(retried.active, Some(identity()));
    assert_eq!(retried.control_revision, 8);
    assert!(retried.owner_reload_acknowledged);
    assert!(!retried.durable);
    let acknowledged = client.acknowledge_runtime_persistence(acknowledgement_request()).await.unwrap();
    assert_eq!(acknowledged.state, daemon::RuntimeOperationState::Committed as i32);
    assert!(acknowledged.durable && acknowledged.owner_reload_acknowledged);
    assert_eq!(acknowledged.persistence_mode, "app_authoritative_regeneration");
    assert_eq!(acknowledged.desired_state_digest, "desired-digest");
    assert_eq!(acknowledged.desired_state_revision, "desired-revision");
    assert_eq!(calls.load(Ordering::SeqCst), 6);
}
#[tokio::test]
async fn unimplemented_is_unsupported_for_each_rpc() {
    let (client, _server) = start(Mock { code: Some(Code::Unimplemented), ..Default::default() }, "secret").await;
    assert!(matches!(client.get_runtime_capabilities().await, Err(RuntimeConfigError::Unsupported(_))));
    assert!(matches!(client.plan_runtime_config(plan_request()).await, Err(RuntimeConfigError::Unsupported(_))));
    assert!(matches!(client.apply_runtime_config(apply_request()).await, Err(RuntimeConfigError::Unsupported(_))));
    assert!(matches!(client.get_runtime_config_status(status_request()).await, Err(RuntimeConfigError::Unsupported(_))));
    assert!(matches!(client.acknowledge_runtime_persistence(acknowledgement_request()).await, Err(RuntimeConfigError::Unsupported(_))));
    assert!(matches!(client.retry_runtime_persistence(retry_request()).await, Err(RuntimeConfigError::Unsupported(_))));
}
#[tokio::test]
async fn other_statuses_are_not_unsupported() {
    for code in [Code::Unauthenticated, Code::Unavailable, Code::FailedPrecondition, Code::Aborted, Code::DeadlineExceeded] {
        let (client, _server) = start(Mock { code: Some(code), ..Default::default() }, "secret").await;
        match client.get_runtime_capabilities().await {
            Err(RuntimeConfigError::Call(ClientError::Status(status))) => assert_eq!(status.code(), code),
            other => panic!("wrong classification: {other:?}"),
        }
        match client.retry_runtime_persistence(retry_request()).await {
            Err(RuntimeConfigError::Call(ClientError::Status(status))) => assert_eq!(status.code(), code),
            other => panic!("wrong persistence retry classification: {other:?}"),
        }
        match client.acknowledge_runtime_persistence(acknowledgement_request()).await {
            Err(RuntimeConfigError::Call(ClientError::Status(status))) => assert_eq!(status.code(), code),
            other => panic!("wrong acknowledgement classification: {other:?}"),
        }
    }
    let (client, _server) = start(Mock::default(), "").await;
    assert!(matches!(client.get_runtime_capabilities().await,
        Err(RuntimeConfigError::Call(ClientError::Status(status))) if status.code() == Code::Unauthenticated));
}
#[tokio::test]
async fn apply_timeout_is_bounded_and_never_retried() {
    let mock = Mock { stall_apply: true, ..Default::default() }; let calls = mock.calls.clone();
    let (client, _server) = start(mock, "secret").await;
    let result = tokio::time::timeout(UNARY_DEADLINE * 2, client.apply_runtime_config(apply_request())).await.unwrap();
    assert!(matches!(result, Err(RuntimeConfigError::Call(ClientError::Status(status))) if status.code() == Code::DeadlineExceeded));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn persistence_retry_timeout_is_bounded_and_never_repeated() {
    let mock = Mock { stall_retry: true, ..Default::default() }; let calls = mock.calls.clone();
    let (client, _server) = start(mock, "secret").await;
    let result = tokio::time::timeout(UNARY_DEADLINE * 2, client.retry_runtime_persistence(retry_request())).await.unwrap();
    assert!(matches!(result, Err(RuntimeConfigError::Call(ClientError::Status(status))) if status.code() == Code::DeadlineExceeded));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn oversized_configuration_is_rejected_before_server_handler() {
    let mock = Mock::default(); let calls = mock.calls.clone();
    let (client, _server) = start(mock, "secret").await;
    let mut request = plan_request(); request.config_content = "x".repeat(RUNTIME_CONFIG_MAX_MESSAGE_BYTES + 1);
    assert!(matches!(client.plan_runtime_config(request).await, Err(RuntimeConfigError::Call(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn refused_transport_is_not_unsupported() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let client = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), "secret").await.unwrap();
    assert!(matches!(client.get_runtime_capabilities().await, Err(RuntimeConfigError::Call(_))));
}

include!("../runtime_config_wire_check.rs");
#[test]
fn optional_source_contract_is_exact_and_bounded() {
    use runtime_config_wire_check::*;
    assert!(compare_source(SOURCE).is_ok());
    let mut changed = SOURCE.to_vec(); changed[0] ^= 1;
    assert!(compare_source(&changed).is_err());
    assert!(compare_source(&vec![0; MAX_SOURCE_BYTES as usize + 1]).is_err());
}

#[tokio::test]
async fn no_op_is_plan_evidence_and_committed_unchanged_not_a_new_enum() {
    let (client, _server) = start(Mock { no_op: true, ..Default::default() }, "secret").await;
    let plan = client.plan_runtime_config(plan_request()).await.unwrap();
    assert!(plan.no_op);
    let operation = client.apply_runtime_config(apply_request()).await.unwrap();
    assert_eq!(operation.state, daemon::RuntimeOperationState::Committed as i32);
    assert_eq!(operation.before, operation.active);
    assert_eq!(operation.control_revision, 7);
    assert!(operation.durable);
}
