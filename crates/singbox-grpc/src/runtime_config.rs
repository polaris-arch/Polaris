//! Optional additive API: RPC support is not evidence of full-schema hot support.
//! Callers must inspect capabilities, plan errors and returned operation state.
use crate::{daemon, ClientError, SingBoxApiClient, UNARY_DEADLINE};
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
        Ok(self
            .unary(client.get_runtime_capabilities(self.runtime_config_request(())))
            .await
            .map_err(classify)?
            .into_inner())
    }

    /// Forward the exact effective configuration, overrides and optimistic identity.
    pub async fn plan_runtime_config(
        &self,
        request: daemon::PlanRuntimeConfigRequest,
    ) -> Result<daemon::RuntimeConfigPlan, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(self
            .unary(client.plan_runtime_config(self.runtime_config_request(request)))
            .await
            .map_err(classify)?
            .into_inner())
    }

    /// Submit once. A timeout/transport failure is an unknown outcome, not proof of
    /// rollback. Reconcile using the same transaction ID via status; never auto-retry.
    pub async fn apply_runtime_config(
        &self,
        request: daemon::ApplyRuntimeConfigRequest,
    ) -> Result<daemon::RuntimeConfigOperation, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(self
            .unary(client.apply_runtime_config(self.runtime_config_request(request)))
            .await
            .map_err(classify)?
            .into_inner())
    }

    /// Retry only the original transaction's persistence tail once. Never resend
    /// graph Apply or replace its identity after an ambiguous response.
    pub async fn retry_runtime_persistence(
        &self,
        request: daemon::RuntimePersistenceRetryRequest,
    ) -> Result<daemon::RuntimeConfigOperation, RuntimeConfigError> {
        let mut client = self.runtime_config_client();
        Ok(self
            .unary(client.retry_runtime_persistence(self.runtime_config_request(request)))
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
        Ok(self
            .unary(client.acknowledge_runtime_persistence(self.runtime_config_request(request)))
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
        Ok(self
            .unary(client.get_runtime_config_status(self.runtime_config_request(request)))
            .await
            .map_err(classify)?
            .into_inner())
    }
}

/// API-only listener handoff observation. The key is sensitive and is only
/// meaningful when received through the caller's already trusted old channel.
/// This type deliberately has no Debug/Serialize implementation.
pub struct ApiHandoffMetadata {
    pub protocol: String,
    pub generation: String,
    pub endpoint: String,
    pub key: Option<String>,
    pub proof: Option<String>,
    pub acknowledged: bool,
}

pub struct RuntimeStatusEnvelope {
    pub status: daemon::RuntimeConfigStatus,
    pub status_wire: Vec<u8>,
    pub handoff: Option<ApiHandoffMetadata>,
}

impl SingBoxApiClient {
    /// Use the existing Status RPC; never resend Apply. Challenge and ack are
    /// bounded API-handoff metadata, not actor declarations or credentials.
    pub async fn get_runtime_config_status_handoff(
        &self,
        status: daemon::RuntimeConfigStatusRequest,
        challenge: Option<&str>,
        ack: Option<&str>,
    ) -> Result<RuntimeStatusEnvelope, RuntimeConfigError> {
        let invalid = || {
            classify(ClientError::Status(tonic::Status::data_loss(
                "invalid API handoff metadata",
            )))
        };
        let mut request = self.runtime_config_request(status);
        for (name, value) in [
            ("x-polaris-api-challenge", challenge),
            ("x-polaris-api-ack", ack),
        ] {
            if let Some(value) = value {
                if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(invalid());
                }
                request
                    .metadata_mut()
                    .insert(name, value.parse().map_err(|_| invalid())?);
            }
        }
        let mut client = tonic::client::Grpc::new(self.channel.clone())
            .max_encoding_message_size(RUNTIME_CONFIG_MAX_MESSAGE_BYTES)
            .max_decoding_message_size(RUNTIME_CONFIG_MAX_MESSAGE_BYTES);
        client.ready().await.map_err(|_| {
            classify(ClientError::Status(tonic::Status::unavailable(
                "API transport unavailable",
            )))
        })?;
        let response = self
            .unary(client.unary(
                request,
                http::uri::PathAndQuery::from_static(
                    "/daemon.RuntimeConfigService/GetRuntimeConfigStatus",
                ),
                RawStatusCodec,
            ))
            .await
            .map_err(classify)?;
        let metadata = response.metadata();
        let field = |name: &str, max: usize| -> Result<Option<String>, RuntimeConfigError> {
            let mut values = metadata.get_all(name).iter();
            let value = values.next();
            if values.next().is_some() {
                return Err(invalid());
            }
            match value {
                None => Ok(None),
                Some(v) => {
                    let v = v.to_str().map_err(|_| invalid())?;
                    if v.is_empty() || v.len() > max {
                        return Err(invalid());
                    }
                    Ok(Some(v.to_owned()))
                }
            }
        };
        let handoff = match field("x-polaris-api-protocol", 40)? {
            None => None,
            Some(protocol) => Some(ApiHandoffMetadata {
                protocol,
                generation: field("x-polaris-api-generation", 32)?.ok_or_else(invalid)?,
                endpoint: field("x-polaris-api-endpoint", 64)?.ok_or_else(invalid)?,
                key: field("x-polaris-api-key", 64)?,
                proof: field("x-polaris-api-proof", 64)?,
                acknowledged: field("x-polaris-api-acknowledged", 5)?.as_deref() == Some("true"),
            }),
        };
        let raw = response.into_inner();
        Ok(RuntimeStatusEnvelope {
            status: raw.status,
            status_wire: raw.wire,
            handoff,
        })
    }
}

// Decode only this Status response with exact-wire retention. Unknown fields,
// legal field ordering and explicit defaults must not disappear before MAC
// verification. Tonic applies the same 4MiB receive bound before this decoder.
struct RawRuntimeStatus {
    status: daemon::RuntimeConfigStatus,
    wire: Vec<u8>,
}
struct RawStatusCodec;
struct RawStatusDecoder;
impl tonic::codec::Codec for RawStatusCodec {
    type Encode = daemon::RuntimeConfigStatusRequest;
    type Decode = RawRuntimeStatus;
    type Encoder=<tonic_prost::ProstCodec<Self::Encode,daemon::RuntimeConfigStatus> as tonic::codec::Codec>::Encoder;
    type Decoder = RawStatusDecoder;
    fn encoder(&mut self) -> Self::Encoder {
        tonic::codec::Codec::encoder(&mut tonic_prost::ProstCodec::<
            Self::Encode,
            daemon::RuntimeConfigStatus,
        >::default())
    }
    fn decoder(&mut self) -> Self::Decoder {
        RawStatusDecoder
    }
}
fn decode_raw_status(wire: Vec<u8>) -> Result<RawRuntimeStatus, tonic::Status> {
    let status = <daemon::RuntimeConfigStatus as prost::Message>::decode(wire.as_slice())
        .map_err(|_| tonic::Status::data_loss("invalid API status protobuf"))?;
    Ok(RawRuntimeStatus { status, wire })
}
impl tonic::codec::Decoder for RawStatusDecoder {
    type Item = RawRuntimeStatus;
    type Error = tonic::Status;
    fn decode(
        &mut self,
        buf: &mut tonic::codec::DecodeBuf<'_>,
    ) -> Result<Option<Self::Item>, Self::Error> {
        use prost::bytes::Buf;
        let wire = buf.copy_to_bytes(buf.remaining()).to_vec();
        decode_raw_status(wire).map(Some)
    }
}

#[cfg(test)]
mod tests;
