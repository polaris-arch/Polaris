//! 订阅安全拉取的 DNS 解析、guard 地址 pin 与单跳传输适配。
//!
//! 父模块继续持有共享 reqwest client 与通用 body 读取 primitive；本模块只负责 net-stack 的
//! DnsLookup / HttpClient 端口，避免订阅专属解析策略挤占通用更新传输实现。

use std::{
    future::Future,
    net::IpAddr,
    time::{Duration, Instant},
};

use polaris_net_stack::safe_redirect::{FetchInit, GuardedTarget, HttpClient, MinimalResponse};
use polaris_net_stack::ssrf::{is_polaris_fake_ip, is_private_ip, DnsLookup};
use polaris_singbox_grpc::RuntimeConfigError;

use crate::runtime::{proxy::UpdateProxyTarget, AppRuntime};

use super::{
    app_user_agent, collect_headers, install_ring_provider, loopback_proxy, read_body_capped,
    BodyReadError, HttpRuntime, RESPONSE_TIMEOUT, STALL_TIMEOUT,
};

// ── DnsLookup 生产实现 ─────────────────────────────────────────────────────────

/// 系统解析器（`tokio::net::lookup_host`）—— net-stack [`DnsLookup`] 的**生产实现**。
///
/// 此前全仓只有测试 mock：意味着 `assert_host_allowed` 的 SSRF guard 在生产路径上
/// **没有解析器可注入**，H1（DNS rebinding）防线是「逻辑在、接线不在」。本类型接上它。
///
/// 端口传 0：只要 A/AAAA 记录，不实际连接。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemDnsLookup;

impl DnsLookup for SystemDnsLookup {
    fn lookup_all(&self, host: &str) -> impl Future<Output = Result<Vec<String>, String>> + Send {
        let host = host.to_string();
        async move {
            let lookup_started = Instant::now();
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| format!("DNS 解析失败 {host}: {e}"))?;
            log::debug!(
                "guarded DNS timing: lookup_ms={}",
                lookup_started.elapsed().as_millis()
            );
            // 逐 IP 交给 guard 判定（**全部**返回，不只首个：rebinding 常把恶意 IP 藏在第二条）。
            let ips: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
            if ips.is_empty() {
                return Err(format!("DNS 解析 {host} 无结果"));
            }
            Ok(ips)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HttpResolutionMode {
    /// 系统直连：必须使用 SSRF guard 同一次解析结果 pin 真实 dial。
    DirectPinned,
    /// HTTP proxy / SOCKS5h：目标由代理端解析；保留 remote-DNS/FakeIP 语义。
    ProxyRemoteDns,
    /// 测试专用的预置 reqwest override，不再覆盖其固定解析。
    #[cfg(test)]
    TestOverride,
}

pub(super) struct PinnedClient {
    origin: String,
    target: GuardedTarget,
    client: reqwest::Client,
    created: Instant,
}

const DIRECT_PINNED_REUSE_WINDOW: Duration = Duration::from_secs(30);

// ── 适配 ①：net-stack HttpClient（订阅拉取）──────────────────────────────────

/// reqwest 的错误 Display 即使去掉 URL，也可能只剩 `error sending request for url`；真正的
/// DNS/拒绝/超时通常藏在 source chain。这里只**读取**链做判定，绝不把链原文回传（其中可能带订阅
/// hostname）；输出稳定、无凭据的诊断 token，供 net-stack 的既有分类器识别。
pub(super) fn classify_transport_failure(
    is_timeout: bool,
    is_connect: bool,
    source_chain: &str,
) -> &'static str {
    if is_timeout {
        return "request timeout";
    }
    let chain = source_chain.to_ascii_lowercase();
    if [
        "dns",
        "getaddrinfo",
        "name resolution",
        "lookup address",
        "no such host",
        "nodename nor servname",
        "11001",
    ]
    .iter()
    .any(|needle| chain.contains(needle))
    {
        return "dns resolution failed";
    }
    if [
        "connection refused",
        "actively refused",
        "connection reset",
        "unreachable",
        "10061",
    ]
    .iter()
    .any(|needle| chain.contains(needle))
    {
        return "connection refused or unreachable";
    }
    if ["tls", "certificate", "handshake"]
        .iter()
        .any(|needle| chain.contains(needle))
    {
        return "tls handshake failed";
    }
    if is_connect {
        return "connection failed";
    }
    "request failed"
}

/// reqwest 错误 → 无 URL/hostname/token 的稳定诊断串。
pub(super) fn sanitized_reqwest_error(error: &reqwest::Error) -> String {
    let mut source_chain = String::new();
    let mut source = std::error::Error::source(error);
    // source 链通常 3~5 层；双上限防第三方错误构造循环/超长 Display 把日志撑大。
    for _ in 0..8 {
        let Some(current) = source else { break };
        if source_chain.len() < 2_048 {
            let rendered = current.to_string();
            let mut end = (2_048 - source_chain.len()).min(rendered.len());
            while !rendered.is_char_boundary(end) {
                end -= 1;
            }
            source_chain.push_str(&rendered[..end]);
            if source_chain.len() < 2_048 {
                source_chain.push('\n');
            }
        }
        source = current.source();
    }
    classify_transport_failure(error.is_timeout(), error.is_connect(), &source_chain).to_string()
}

pub(super) fn sanitized_body_read_error(error: BodyReadError) -> String {
    match error {
        BodyReadError::Stalled => "request timeout while reading response body".to_string(),
        BodyReadError::TooLarge(limit) => {
            format!("response body too large (limit {limit} bytes)")
        }
        BodyReadError::Io { message, .. } => {
            classify_transport_failure(false, false, &message).to_string()
        }
        // `read_body_capped` 没有 sink，生产不可达；完整匹配保留类型未来扩展时的编译器提醒。
        BodyReadError::Sink { .. } => "response body sink failed".to_string(),
    }
}

pub(super) fn build_direct_pinned_client(
    target: &GuardedTarget,
) -> Result<reqwest::Client, String> {
    pinned_client_builder(target, &target.addresses)?
        .build()
        .map_err(|e| format!("建 DNS-pinned HTTP 客户端失败: {e}"))
}

fn pinned_client_builder(
    target: &GuardedTarget,
    addresses: &[IpAddr],
) -> Result<reqwest::ClientBuilder, String> {
    install_ring_provider();
    let addrs: Vec<std::net::SocketAddr> = addresses
        .iter()
        .copied()
        .map(|ip| std::net::SocketAddr::new(ip, 0))
        .collect();
    if addrs.is_empty() {
        return Err(format!(
            "SSRF guard 未提供可拨号地址，已拒绝: {}",
            target.host
        ));
    }
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent(app_user_agent())
        .pool_max_idle_per_host(1)
        .pool_idle_timeout(DIRECT_PINNED_REUSE_WINDOW)
        // URL hostname 保持不变（Host/SNI 仍是域名），只把底层 socket 解析钉到 guard 的同一组 IP。
        .resolve_to_addrs(&target.host, &addrs))
}

pub(super) fn build_socks_pinned_client(
    guarded: &GuardedTarget,
    address: &IpAddr,
    port: u16,
    auth: Option<&polaris_config_engine::singbox::InboundUser>,
) -> Result<reqwest::Client, String> {
    let proxy = loopback_proxy(&format!("socks5://127.0.0.1:{port}"), port, auth)?;
    pinned_client_builder(guarded, std::slice::from_ref(address))?
        .proxy(proxy)
        .build()
        .map_err(|_| "cannot build guarded update transport".to_owned())
}

fn hop_request(client: &reqwest::Client, url: &str, init: &FetchInit) -> reqwest::RequestBuilder {
    let mut req = client.get(url);
    if !init.user_agent.is_empty() {
        req = req.header(reqwest::header::USER_AGENT, init.user_agent.as_str());
    }
    for (k, v) in &init.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    req
}

impl HttpRuntime {
    fn direct_client_for_target(
        &self,
        url: &str,
        target: &GuardedTarget,
    ) -> Result<reqwest::Client, String> {
        let origin = reqwest::Url::parse(url)
            .map_err(|_| "订阅请求地址无效".to_string())?
            .origin()
            .ascii_serialization();
        // 每跳仍由 guard 重新解析和放行。仅复用同源、同一组已校验地址的连接。
        if let Some(entry) = self
            .direct_pinned
            .lock()
            .map_err(|_| "直连客户端缓存不可用".to_string())?
            .as_ref()
            .filter(|entry| {
                entry.origin == origin
                    && entry.target == *target
                    && entry.created.elapsed() < DIRECT_PINNED_REUSE_WINDOW
            })
        {
            return Ok(entry.client.clone());
        }
        let client = build_direct_pinned_client(target)?;
        *self
            .direct_pinned
            .lock()
            .map_err(|_| "直连客户端缓存不可用".to_string())? = Some(PinnedClient {
            origin,
            target: target.clone(),
            client: client.clone(),
            created: Instant::now(),
        });
        Ok(client)
    }
}

async fn fetch_one_hop(
    client: reqwest::Client,
    url: String,
    init: FetchInit,
) -> Result<MinimalResponse, String> {
    let response_timeout = init
        .timeout_ms
        .map_or(RESPONSE_TIMEOUT, Duration::from_millis);
    let send_started = Instant::now();
    let resp = tokio::time::timeout(response_timeout, hop_request(&client, &url, &init).send())
        .await
        .map_err(|_| "request timeout".to_string())?
        .map_err(|e| sanitized_reqwest_error(&e))?;
    let headers_ms = send_started.elapsed().as_millis();

    let body_started = Instant::now();
    let response = read_hop_response(resp, init.max_body_bytes).await?;
    log::debug!(
        "guarded HTTP timing: status={} response_headers_ms={headers_ms} body_ms={} body_bytes={}",
        response.status,
        body_started.elapsed().as_millis(),
        response.body.len()
    );
    Ok(response)
}

async fn read_hop_response(
    mut resp: reqwest::Response,
    max_body_bytes: Option<usize>,
) -> Result<MinimalResponse, String> {
    let status = resp.status().as_u16();
    let headers = collect_headers(&resp);
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let body = if (300..400).contains(&status) {
        Vec::new()
    } else {
        // absolute deadline 由 safe_redirect_fetch_until 外包整条 future；这里保留更早的 body idle 失败。
        read_body_capped(&mut resp, max_body_bytes, STALL_TIMEOUT)
            .await
            .map_err(sanitized_body_read_error)?
    };

    Ok(MinimalResponse {
        status,
        location,
        headers,
        body,
    })
}

const CORE_LOOKUP_MAX_ADDRESSES: usize = 64;
const UPDATE_PROXY_OWNER_CHANGED: &str = "update proxy owner changed";

pub(super) fn canonical_public_update_address(address: IpAddr) -> Result<IpAddr, String> {
    let address = address.to_canonical();
    // 经代理的更新只接受普通公网单播，以及内嵌 IPv4 同样合规的知名 NAT64 前缀。
    let supported_scope = match address {
        IpAddr::V4(ip) => {
            !ip.is_documentation()
                && !matches!(
                    ip.octets(),
                    [224..=255, ..] | [192, 0, 0, 0..=8 | 11..=255] | [192, 88, 99, ..]
                )
        }
        IpAddr::V6(ip) => match ip.segments() {
            [0x0064, 0xff9b, 0, 0, 0, 0, high, low] => {
                let embedded = (u32::from(high) << 16) | u32::from(low);
                canonical_public_update_address(std::net::Ipv4Addr::from_bits(embedded).into())
                    .is_ok()
            }
            // 协议分配段、文档段与 6to4。
            [0x2001, 0..=0x01ff | 0x0db8, ..] | [0x2002, ..] | [0x3fff, 0..=0x0fff, ..] => false,
            [0x2000..=0x3fff, ..] => true,
            _ => false,
        },
    };
    let text = address.to_string();
    if !supported_scope || is_private_ip(&text) || is_polaris_fake_ip(&text) {
        return Err("update target is outside the supported public address scope".into());
    }
    Ok(address)
}

pub(crate) struct UpdateHttp<'a> {
    state: &'a AppRuntime,
    target: Result<Option<UpdateProxyTarget>, String>,
}

impl<'a> UpdateHttp<'a> {
    /// 为整次操作（含每一跳重定向）选定同一条路由。
    pub(crate) fn new(state: &'a AppRuntime) -> Self {
        let preference = state
            .config()
            .with_current(|cfg| cfg["mainSessionViaProxy"].as_bool());
        let target = state
            .proxy()
            .update_proxy_target(preference.as_ref().ok().copied().flatten())
            .and_then(|target| {
                if target.is_some() && preference.is_err() {
                    return Err("update proxy preference unavailable".into());
                }
                Ok(target)
            });
        Self { state, target }
    }

    fn target(&self) -> Result<Option<&UpdateProxyTarget>, String> {
        self.target
            .as_ref()
            .map(Option::as_ref)
            .map_err(Clone::clone)
    }

    async fn core_lookup(
        &self,
        target: &UpdateProxyTarget,
        host: &str,
    ) -> Result<Vec<String>, String> {
        let proxy = self.state.proxy();
        if !proxy.update_proxy_is_current(target) {
            return Err(UPDATE_PROXY_OWNER_CHANGED.into());
        }
        let client = target.dns_client();
        let reply = tokio::select! {
            biased;
            () = proxy.update_proxy_generation_changed(target) => {
                return Err(UPDATE_PROXY_OWNER_CHANGED.into());
            }
            result = client.lookup_public_dns(host, RESPONSE_TIMEOUT) => result,
        };
        if !proxy.update_proxy_is_current(target) {
            return Err(UPDATE_PROXY_OWNER_CHANGED.into());
        }
        let reply = reply.map_err(|error| match error {
            RuntimeConfigError::Unsupported(_) => {
                "running Core does not support guarded update DNS"
            }
            RuntimeConfigError::Call(_) => "update DNS lookup failed",
        })?;
        if reply.version != 1
            || reply.addresses.is_empty()
            || reply.addresses.len() > CORE_LOOKUP_MAX_ADDRESSES
        {
            return Err("invalid update DNS response".into());
        }
        reply
            .addresses
            .into_iter()
            .map(|address| {
                let address = address.parse().map_err(|_| "invalid update DNS response")?;
                canonical_public_update_address(address).map(|address| address.to_string())
            })
            .collect()
    }
}

impl DnsLookup for UpdateHttp<'_> {
    async fn lookup_all(&self, host: &str) -> Result<Vec<String>, String> {
        match self.target()? {
            Some(target) => self.core_lookup(target, host).await,
            None => SystemDnsLookup.lookup_all(host).await,
        }
    }
}

impl HttpClient for UpdateHttp<'_> {
    async fn fetch(&self, _url: &str, _init: &FetchInit) -> Result<MinimalResponse, String> {
        Err("update requests require a guarded target".into())
    }

    async fn fetch_guarded(
        &self,
        url: &str,
        init: &FetchInit,
        guarded: &GuardedTarget,
    ) -> Result<MinimalResponse, String> {
        let Some(target) = self.target()? else {
            return self.state.http().fetch_guarded(url, init, guarded).await;
        };
        // 字面量 IP 的重定向不经 DNS，拨号前重新校验整组数字地址。
        let addresses: Vec<_> = guarded
            .addresses
            .iter()
            .copied()
            .map(canonical_public_update_address)
            .collect::<Result<_, _>>()?;
        let proxy = self.state.proxy();
        let lease = target.epoch.borrow();
        if lease.is_closed() || !proxy.update_proxy_is_current(target) {
            return Err(UPDATE_PROXY_OWNER_CHANGED.into());
        }
        let deadline = tokio::time::Instant::now()
            + init
                .timeout_ms
                .map_or(RESPONSE_TIMEOUT, Duration::from_millis);
        let fetch = async {
            let mut failure = "update DNS returned no addresses".to_owned();
            // reqwest SOCKS 只用第一个覆盖地址；仅在连接失败时于同一跳预算内改试其余已放行地址。
            for address in &addresses {
                if !proxy.update_proxy_is_current(target) {
                    return Err(UPDATE_PROXY_OWNER_CHANGED.into());
                }
                let client =
                    build_socks_pinned_client(guarded, address, target.port, target.auth.as_ref())?;
                match tokio::time::timeout_at(deadline, hop_request(&client, url, init).send())
                    .await
                {
                    Ok(Ok(response)) => {
                        return read_hop_response(response, init.max_body_bytes).await
                    }
                    Ok(Err(error)) if error.is_connect() => {
                        failure = sanitized_reqwest_error(&error)
                    }
                    Ok(Err(error)) => return Err(sanitized_reqwest_error(&error)),
                    Err(_) => return Err("request timeout".into()),
                }
            }
            Err(failure)
        };
        let result = tokio::select! {
            biased;
            () = proxy.update_proxy_generation_changed(target) => {
                return Err(UPDATE_PROXY_OWNER_CHANGED.into());
            }
            () = lease.cancelled() => return Err(UPDATE_PROXY_OWNER_CHANGED.into()),
            result = fetch => result,
        };
        if !proxy.update_proxy_is_current(target) {
            return Err(UPDATE_PROXY_OWNER_CHANGED.into());
        }
        result
    }
}

impl HttpClient for HttpRuntime {
    /// 单跳 GET（manual redirect：30x 原样返回，**不跟随**）。
    ///
    /// 逐跳 SSRF 复检与链路编排归 `safe_redirect_fetch`（net-stack），本适配器**只做一跳传输**。
    fn fetch(
        &self,
        url: &str,
        init: &FetchInit,
    ) -> impl Future<Output = Result<MinimalResponse, String>> + Send {
        fetch_one_hop(self.client.clone(), url.to_string(), init.clone())
    }

    fn fetch_guarded(
        &self,
        url: &str,
        init: &FetchInit,
        target: &GuardedTarget,
    ) -> impl Future<Output = Result<MinimalResponse, String>> + Send {
        let client = match self.resolution_mode {
            HttpResolutionMode::DirectPinned => self.direct_client_for_target(url, target),
            // SOCKS5h/HTTP proxy 必须保留 hostname 交代理端解析；使用本地 guard 地址会破坏抗污染与
            // 外部 FakeIP。代理端最终 IP 无法由 SOCKS5 协议回传，故该模式的 TOCTOU 是显式残留。
            HttpResolutionMode::ProxyRemoteDns => Ok(self.client.clone()),
            #[cfg(test)]
            HttpResolutionMode::TestOverride => Ok(self.client.clone()),
        };
        let url = url.to_string();
        let init = init.clone();
        async move { fetch_one_hop(client?, url, init).await }
    }
}
