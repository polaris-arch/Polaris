//! 管理 API、dashboard 连接信息与运行核回环代理绑定。
//!
//! 建 gRPC 管理客户端、取 Bearer secret、按节点解析管理面落点、`endpointTag ↔ serverId` 逆映射、
//! dashboard 连接信息（供前端 `app:getSingboxDashboardConnection`）。

use std::collections::BTreeMap;

use polaris_config_engine::builder::build_id_to_tag_map;
use polaris_config_engine::builder::helpers::ServerLike;
use polaris_config_engine::singbox::InboundUser;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_singbox_grpc::{ClientError, Endpoint, SingBoxApiClient};
use serde_json::Value;

use crate::runtime::management_api::{current_clash_api_secret, GrpcManagementApi};

use super::ProxyRuntime;

/// 经本机 http 代理入站出网的目标（[`ProxyRuntime::local_http_proxy`] 产出）。
///
/// `Debug` 不泄凭据：`InboundUser` 的 `Debug` 已抹掉口令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHttpProxy {
    /// 本机 http 代理入站端口（`mixed-in` 或 `probe-proxy-in`）。
    pub port: u16,
    /// 该入站要求的凭据；`mixed-in` 恒零认证 ⇒ 恒 `None`。
    pub auth: Option<InboundUser>,
    /// 取到的是哪一个入站。两者在内核里受的路由策略不同，测速据此标注结果。
    pub inbound: LocalInbound,
}

/// [`LocalHttpProxy`] 背后的入站。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalInbound {
    /// `mixed-in`：不钉死，走用户的全部路由规则与 DNS 规则。
    Mixed,
    /// `probe-proxy-in`：被路由钉到 `proxy-selector`，用户规则不参与。
    ProbeProxy,
}

impl LocalInbound {
    /// 该入站在运行核里的 tag。
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Mixed => polaris_config_engine::builder::helpers::MIXED_INBOUND_TAG,
            Self::ProbeProxy => polaris_config_engine::builder::helpers::PROBE_PROXY_INBOUND_TAG,
        }
    }
}

/// 选「经本机 http 代理入站出网」用哪个口（**纯函数**，单测与对拍门的取材点）。
///
/// - `mixed_port != 0`：本平台发射了 `mixed-in`（`ProxyStatus.mixed_port` 只在
///   `emits_mixed_inbound` 为真时非 0，见起核处的赋值）→ 用它；它零认证，凭据不带。
/// - 否则用 `probe-proxy-in`（固定走 `proxy-selector`，Android 上带本次起核的凭据）。
/// - 两者都没有 → `None`：调用方按「本层无从经代理出网」处理，**不**连一个不存在的口。
///
/// 修的缺陷（2026-09-25 α 批）：此前四条腿无条件读 `mixed_port`，而 Android 不发 mixed 入站 ⇒
/// 解锁检测全超时、首页代理出口 IP 为空、测速回退超时，全程无报错。
#[must_use]
pub(super) fn select_local_http_proxy(
    mixed_port: u16,
    probe_proxy_port: Option<u16>,
    loopback_auth: Option<InboundUser>,
) -> Option<LocalHttpProxy> {
    if mixed_port != 0 {
        return Some(LocalHttpProxy {
            port: mixed_port,
            auth: None,
            inbound: LocalInbound::Mixed,
        });
    }
    probe_proxy_port
        .filter(|port| *port != 0)
        .map(|port| LocalHttpProxy {
            port,
            auth: loopback_auth,
            inbound: LocalInbound::ProbeProxy,
        })
}

pub(crate) struct ApiReadbackBinding {
    pub(crate) port: u16,
    pub(crate) secret: String,
    pub(crate) epoch: Option<std::sync::Arc<super::runtime_config::api_handoff::ApiRelayEpoch>>,
    // No borrower is cached here: an idle binding cannot hold seal_and_join
    // hostage. The actual opening/receive worker acquires and retains it.
    pub(crate) control: Option<std::sync::Arc<SingBoxApiClient>>,
}

impl ApiReadbackBinding {
    pub(crate) async fn open_events(
        self,
        interval: i64,
    ) -> Result<
        impl futures::Stream<
                Item = Result<
                    polaris_singbox_grpc::daemon::ConnectionEvents,
                    polaris_singbox_grpc::tonic::Status,
                >,
            > + Send,
        ClientError,
    > {
        let invalid = || {
            ClientError::Status(polaris_singbox_grpc::tonic::Status::failed_precondition(
                "readback carrier unavailable",
            ))
        };
        let client = match (self.epoch, self.control) {
            (Some(epoch), Some(control)) => {
                let lease = epoch.borrow();
                if lease.is_closed() || !control.captured_transport_live() {
                    return Err(invalid());
                }
                control.as_ref().clone().retain_lifetime(lease)
            }
            (None, None) => {
                SingBoxApiClient::connect(Endpoint::new("127.0.0.1", self.port), self.secret)
                    .await?
            }
            _ => return Err(invalid()),
        };
        let frames = client.connection_events_once(interval).await?;
        // Keep the exact client/borrow with the stream, including while a
        // message is pending. The epoch owns and joins the enclosing worker.
        Ok(futures::stream::unfold(
            (client, frames),
            |(client, mut frames)| async move {
                match frames.message().await {
                    Ok(Some(frame)) => Some((Ok(frame), (client, frames))),
                    Ok(None) => None,
                    Err(error) => Some((Err(error), (client, frames))),
                }
            },
        ))
    }
}

impl ProxyRuntime {
    /// 建管理 API 客户端（h2c gRPC）。核未起 / 端口未解析 / 连不上 → `not_ready()`（→ 退回重启）。
    ///
    /// 每次热切换现连：tonic channel 是 lazy 的，建连成本低；持久化客户端需处理换核/换端口后的
    /// 失效重建，属 stats-worker 批次的连接管理范畴，本批不引入该状态。
    pub(super) async fn management_api(&self) -> GrpcManagementApi {
        let status = self.status();
        if !status.running || status.clash_api_port == 0 {
            log::warn!(
                "管理 API 客户端未就绪：running={} port={}",
                status.running,
                status.clash_api_port
            );
            return GrpcManagementApi::not_ready();
        }
        match self.current_management_client(status.clash_api_port).await {
            Ok(client) => GrpcManagementApi::new(client),
            Err(error) => {
                log::warn!("管理 API 客户端未就绪：{error}");
                GrpcManagementApi::not_ready()
            }
        }
    }

    // Port and credential are one captured running-baseline value. A stale
    // caller must fail before sending a new credential to a retired tuple.
    pub(crate) async fn current_management_client(
        &self,
        expected_port: u16,
    ) -> Result<SingBoxApiClient, ClientError> {
        let invalid = |reason| {
            ClientError::Status(polaris_singbox_grpc::tonic::Status::failed_precondition(
                format!("management endpoint/epoch changed: {reason}"),
            ))
        };
        match self.managed_api_call(expected_port).map_err(invalid)? {
            Some((control, lease)) => Ok(control.as_ref().clone().retain_lifetime(lease)),
            None => {
                if expected_port == 0 {
                    return Err(invalid("endpoint unavailable"));
                }
                SingBoxApiClient::connect(
                    Endpoint::new("127.0.0.1", expected_port),
                    current_clash_api_secret(&self.config),
                )
                .await
            }
        }
    }

    /// Bind the ordinary transport to the producer's endpoint before any action I/O.
    /// This constructor never reads the current runtime's status or saved credentials.
    #[cfg(all(test, not(target_os = "ios")))]
    pub(super) async fn management_api_at(port: u16, secret: String) -> GrpcManagementApi {
        if port == 0 {
            return GrpcManagementApi::not_ready();
        }
        match SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret).await {
            Ok(c) => GrpcManagementApi::new(c),
            Err(e) => {
                log::warn!("管理 API 连接失败（热切换将退回重启）: {e}");
                GrpcManagementApi::not_ready()
            }
        }
    }

    /// 运行核的管理端点（端口与 secret）。核未运行或端口未解析 → `None`。
    ///
    /// 与 [`management_api`](Self::management_api) 取的是同一组值：测速的读回流与探针换选因此连到
    /// 同一个端点。
    pub(crate) fn management_endpoint(&self) -> Option<(u16, String)> {
        let status = self.status();
        if !status.running || status.clash_api_port == 0 {
            return None;
        }
        if let Some((_, port, secret)) = self.verified_runtime_api_endpoint() {
            return Some((port, secret));
        }
        if self.runtime_api_baseline_present() {
            return None;
        }
        Some((
            status.clash_api_port,
            current_clash_api_secret(&self.config),
        ))
    }

    /// 一轮测速的读回流要连的管理端点：持票据的运行用票据里的那一个（与它的探针换选同一个），
    /// 其余用此刻运行核的那一个。票据在手时不读当前运行态：核换代后流不会跟到新核上。
    #[cfg(all(test, not(target_os = "ios")))]
    pub(crate) fn readback_endpoint(
        &self,
        ticket: Option<&super::ReadyMainTicket>,
    ) -> Option<(u16, String)> {
        self.readback_binding(ticket)
            .map(|binding| (binding.port, binding.secret))
    }
    pub(crate) fn readback_binding(
        &self,
        ticket: Option<&super::ReadyMainTicket>,
    ) -> Option<ApiReadbackBinding> {
        match self.managed_readback_binding(ticket) {
            Ok(Some(binding)) => return Some(binding),
            Err(()) => return None,
            Ok(None) => {}
        }
        match ticket {
            Some(ticket) => Some(ApiReadbackBinding {
                port: ticket.api_port(),
                secret: ticket.api_secret().to_owned(),
                epoch: None,
                control: None,
            }),
            None => self
                .management_endpoint()
                .map(|(port, secret)| ApiReadbackBinding {
                    port,
                    secret,
                    epoch: None,
                    control: None,
                }),
        }
    }

    /// 某个节点在**当前运行核**里的管理 API 落点：`(端口, secret, 该节点的 endpoint tag)`。
    ///
    /// `None` 有两种成因，调用方对用户的表述必须一致（都是「现在做不了」而不是「出错了」）：
    /// 核没在跑；或该 serverId 不在运行快照的 `id_to_tag` 里 —— 后者意味着**核吃进去的那份配置里
    /// 没有这个节点**（刚加还没重启、或已被删）。
    ///
    /// 🔴 **tag 解不到时不得回落成 `server.name` 猜一个**。热重设 exit_node 那条腿有这个回落，
    /// 是因为它猜错只是「热切失败、退回重启」；而 Taildrop 侧猜错的后果是**静默空结果**：核对
    /// 未知 endpointTag 返回的是一帧空收件箱而非错误（`daemon/started_service_taildrop.go:90-97`，
    /// 判据见 `SingBoxApiClient::first_taildrop_inbox_snapshot` 文档）⇒ 用户看到「收件箱是空的」，
    /// 而真实的收件箱在另一个端点上。宁可明说「取不到」。
    pub(crate) fn management_target_for(&self, server_id: &str) -> Option<(u16, String, String)> {
        let status = self.status();
        if !status.running {
            return None;
        }
        let tag = self
            .switch_snapshot
            .read()
            .ok()
            .and_then(|g| g.as_ref().and_then(|s| s.id_to_tag.get(server_id).cloned()))?;
        self.management_endpoint()
            .map(|(port, secret)| (port, secret, tag))
    }

    /// 起核配置的 `endpointTag → serverId` 逆映射（`build_id_to_tag_map` 的逆）。
    ///
    /// TS STATUS 帧里端点以 `endpointTag`（= 节点显示名去重后的 outbound tag）标识；解码时据此逆映回
    /// `serverId`。**在起核时刻从核实际启动的那份配置构建**（而非读 `current_config`）：核发的 tag 恒是它
    /// 启动时的 tag，rename-without-restart 也不改运行核 tag，故 start 快照才与核 wire 一致。tag 唯一 →
    /// 逆映射 1:1；撞名（`build_id_to_tag_map` 追加 `(n)`）后仍唯一。
    pub(super) fn endpoint_tag_to_id(user_config: &UserConfig) -> BTreeMap<String, String> {
        struct SrvLike<'a>(&'a polaris_config_engine::user_config::server_config::ServerConfig);
        impl ServerLike for SrvLike<'_> {
            fn id(&self) -> &str {
                &self.0.id
            }
            fn name(&self) -> &str {
                &self.0.name
            }
        }
        let wrappers: Vec<SrvLike> = user_config.servers.iter().map(SrvLike).collect();
        build_id_to_tag_map(&wrappers)
            .into_iter()
            .map(|(id, tag)| (tag, id))
            .collect()
    }

    /// 取 dashboard 连接信息（上游 `app:getSingboxDashboardConnection`，:2377-2389）。
    ///
    /// 端口取运行期管理 API 端口（动态解析，渲染端构造不出）；secret 取 currentConfig.clashApiSecret。
    pub fn dashboard_connection(&self) -> Value {
        let s = self.status();
        if !s.running || s.clash_api_port == 0 {
            return serde_json::json!({ "ok": false, "url": "", "apiUrl": "", "secret": "" });
        }
        let Some((port, secret)) = self.management_endpoint() else {
            return serde_json::json!({"ok":false,"url":"","apiUrl":"","secret":""});
        };
        serde_json::json!({
            "ok": true,
            "url": format!("http://127.0.0.1:{}/dashboard/", port),
            "apiUrl": format!("http://127.0.0.1:{}", port),
            "secret": secret,
        })
    }
}
