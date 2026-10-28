//! Optional additive API: RPC support is not evidence of full-schema hot support.
//! Callers must inspect capabilities, plan errors and returned operation state.
use crate::{daemon, within_unary_deadline, ClientError, SingBoxApiClient, UNARY_DEADLINE};
use daemon::runtime_config_service_client::RuntimeConfigServiceClient;
use tonic::{transport::Channel, Request};

/// Bounded request and response envelopes, including effective configuration text.
pub const RUNTIME_CONFIG_MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeConfigError {
    /// An older kernel or missing method explicitly returned Unimplemented.
    #[error("runtime configuration RPC is unsupported: {0}")]
    Unsupported(tonic::Status),
    /// Authentication, transport, timeout and other RPC errors remain distinguishable.
    #[error(transparent)]
    Call(#[from] ClientError),
}

fn classify(error: ClientError) -> RuntimeConfigError {
    match error {
        ClientError::Status(status) if status.code() == tonic::Code::Unimplemented => {
            RuntimeConfigError::Unsupported(status)
        }
        other => RuntimeConfigError::Call(other),
    }
}

impl SingBoxApiClient {
    fn runtime_config_client(&self) -> RuntimeConfigServiceClient<Channel> {
        RuntimeConfigServiceClient::new(self.channel.clone())
            .max_encoding_message_size(RUNTIME_CONFIG_MAX_MESSAGE_BYTES)
            .max_decoding_message_size(RUNTIME_CONFIG_MAX_MESSAGE_BYTES)
    }

    fn runtime_config_request<T>(&self, value: T) -> Request<T> {
        let mut request = self.with_auth(Request::new(value));
        request.set_timeout(UNARY_DEADLINE);
        request
    }

    /// Query this runtime; an absent additive service is unsupported, never success.
    pub async fn get_runtime_capabilities(
        &self,
    ) -> Result<daemon::RuntimeCapabilities, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(
            within_unary_deadline(client.get_runtime_capabilities(self.runtime_config_request(())))
                .await
                .map_err(classify)?
                .into_inner(),
        )
    }

    /// Forward the exact effective configuration, overrides and optimistic identity.
    pub async fn plan_runtime_config(
        &self,
        request: daemon::PlanRuntimeConfigRequest,
    ) -> Result<daemon::RuntimeConfigPlan, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(
            within_unary_deadline(client.plan_runtime_config(self.runtime_config_request(request)))
                .await
                .map_err(classify)?
                .into_inner(),
        )
    }

    /// Submit once. A timeout/transport failure is an unknown outcome, not proof of
    /// rollback. Reconcile using the same transaction ID via status; never auto-retry.
    pub async fn apply_runtime_config(
        &self,
        request: daemon::ApplyRuntimeConfigRequest,
    ) -> Result<daemon::RuntimeConfigOperation, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(
            within_unary_deadline(
                client.apply_runtime_config(self.runtime_config_request(request)),
            )
            .await
            .map_err(classify)?
            .into_inner(),
        )
    }

    /// Retry only the original transaction's persistence tail once. Never resend
    /// graph Apply or replace its identity after an ambiguous response.
    pub async fn retry_runtime_persistence(
        &self,
        request: daemon::RuntimePersistenceRetryRequest,
    ) -> Result<daemon::RuntimeConfigOperation, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(within_unary_deadline(
            client.retry_runtime_persistence(self.runtime_config_request(request)),
        )
        .await
        .map_err(classify)?
        .into_inner())
    }

    /// Forward the original owner's durable desired-state acknowledgement once.
    /// A missing method remains unsupported; errors never imply durable completion.
    pub async fn acknowledge_runtime_persistence(
        &self,
        request: daemon::RuntimePersistenceAcknowledgementRequest,
    ) -> Result<daemon::RuntimeConfigOperation, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(within_unary_deadline(
            client.acknowledge_runtime_persistence(self.runtime_config_request(request)),
        )
        .await
        .map_err(classify)?
        .into_inner())
    }

    /// Preserve lookup state, incarnation, operation and durability evidence verbatim.
    pub async fn get_runtime_config_status(
        &self,
        request: daemon::RuntimeConfigStatusRequest,
    ) -> Result<daemon::RuntimeConfigStatus, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(within_unary_deadline(
            client.get_runtime_config_status(self.runtime_config_request(request)),
        )
        .await
        .map_err(classify)?
        .into_inner())
    }
}
