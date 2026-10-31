//! h2c（HTTP/2 cleartext，非 TLS）通道。
//!
//! sing-box 管理 API 本地端点（127.0.0.1:&lt;port&gt;）走 h2c——不上 TLS。tonic 的
//! [`tonic::transport::Channel`] 默认连接器假定 TLS；h2c 需用 `connect_with_connector(_lazy)`
//! 传入一个**只做 TCP 连接**的 [`tower_service::Service`]`<Uri>`，tonic 内部经
//! `hyper::client::conn::http2::Builder::handshake` 跑明文 HTTP/2 握手（prior knowledge，无 ALPN）。
//!
//! 故本连接器只负责 `host:port` → `TcpStream`；h2 body 适配 / stream 多路复用全归 tonic+hyper。
//! 对齐 Polaris：`grpc.credentials.createInsecure()` 在 h2c 上跑 HTTP/2 明文。

use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tonic::transport::Channel;
use tower_service::Service;
// 与 `reconnect.rs` 同一理由换 `log` 门面：本仓只给 `log` 接了 sink（`src-tauri/src/logging.rs`），
// `tracing` 无 subscriber ⇒ 恒为 no-op。同 crate 内不留两套门面，否则「这条日志到底会不会落盘」
// 又要逐文件确认一次。
use log::debug;

type DialFuture = Pin<Box<dyn Future<Output = std::io::Result<TcpStream>> + Send>>;

/// 把 `host:port` 拨成一条 TCP 连接。生产只有 [`TcpStream::connect`] 一种；做成可替换的值，
/// 是为了让「对端对 SYN 毫无响应」这种不出回环就造不出来的情形也有用例。
pub(crate) type Dialer = Arc<dyn Fn(String, u16) -> DialFuture + Send + Sync>;

/// 连接到 sing-box 管理 API（h2c，明文 HTTP/2）。
///
/// `target` = `host:port`（裸 IPv6 字面量已方括号包裹）。返回一条 **lazy** tonic Channel——
/// 不立即建连，首个 RPC 时才握 TCP + h2。复用同 channel 的所有 RPC 走同一条 h2 连接。
pub(crate) async fn connect_h2c(target: &str) -> Result<Channel, tonic::transport::Error> {
    connect_h2c_with(
        target,
        Arc::new(|host, port| Box::pin(TcpStream::connect((host, port)))),
    )
}

/// [`connect_h2c`] 的本体：拨号器可注入。
pub(crate) fn connect_h2c_with(
    target: &str,
    dial: Dialer,
) -> Result<Channel, tonic::transport::Error> {
    // tonic 0.14：from_shared 接收 Into<Bytes>（字符串即可）。h2c = `http://` scheme（明文）。
    let endpoint = tonic::transport::Endpoint::from_shared(format!("http://{target}"))?
        // 关键：用自定义 TCP 连接器覆盖默认 TLS 连接器；tonic 内部跑 h2 明文握手（prior knowledge）。
        .connect_with_connector_lazy(TcpConnector {
            dial,
            capture: None,
        });
    Ok(endpoint)
}

/// 明文 TCP 连接器：把 `http://host:port` Uri 解析出 host:port，TCP connect，返回字节流。
/// tonic 自行在其上跑 hyper h2 明文握手。
///
/// 拨号以 [`crate::CONNECT_TIMEOUT`] 为限。lazy channel 的拨号发生在 tonic 的 `poll_ready` 里，
/// 请求上的 deadline 只从 `call` 起算、管不到这一段；不在这里设限，拨号要多久就全看系统
/// （对端丢弃 SYN 时是分钟级的重传），其间这条 channel 上的每个调用都排在它后面。
#[derive(Clone)]
struct TcpConnector {
    dial: Dialer,
    capture: Option<Arc<CapturedTransport>>,
}

impl Service<http::Uri> for TcpConnector {
    type Response = hyper_util::rt::TokioIo<TrackedTcp>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // 总是就绪——每连接独立 TCP 握手，无背压。
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: http::Uri) -> Self::Future {
        let dial = Arc::clone(&self.dial);
        let capture = self.capture.clone();
        Box::pin(async move {
            let host = uri
                .host()
                .ok_or_else(|| io("h2c connector: uri missing host"))?;
            let port = uri.port_u16().unwrap_or(80);
            // 去掉 IPv6 字面量的方括号（`[::1]` → `::1`），传给 tokio 解析。
            let host = host.trim_start_matches('[').trim_end_matches(']');
            let addr = format!("{host}:{port}");

            if let Some(c) = &capture {
                if c.connected.load(Ordering::Acquire) {
                    c.close();
                    return Err(io("captured API transport cannot reconnect"));
                }
            }
            debug!("h2c：TCP 拨号 {addr}");
            let limit = crate::CONNECT_TIMEOUT;
            let tcp = match tokio::time::timeout(limit, dial(host.to_owned(), port)).await {
                Ok(dialed) => dialed.map_err(|e| io(&format!("h2c TCP connect {addr}: {e}")))?,
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("h2c TCP connect {addr}: no answer within {limit:?}"),
                    ))
                }
            };
            // TokioIo 适配 tokio AsyncRead/Write → hyper rt::Read/Write（tonic 要求）。
            if let Some(c) = &capture {
                if c.connected.swap(true, Ordering::AcqRel) {
                    c.close();
                    return Err(io("captured API transport already owns a connection"));
                }
            }
            Ok(hyper_util::rt::TokioIo::new(TrackedTcp { tcp, capture }))
        })
    }
}

// Connection custody is not peer authentication by itself. It preserves the
// caller's already trusted bootstrap transport and never reinterprets a later
// same-address listener as that original owner.
pub(crate) struct CapturedTransport {
    connected: AtomicBool,
    closed: AtomicBool,
    on_close: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl CapturedTransport {
    pub(crate) fn live(&self) -> bool {
        self.connected.load(Ordering::Acquire) && !self.closed.load(Ordering::Acquire)
    }
    fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let callback = self
                .on_close
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(callback) = callback {
                callback()
            }
        }
    }
    pub(crate) fn on_closed(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_close.lock().unwrap_or_else(|p| p.into_inner()) = Some(callback.clone());
        if self.closed.load(Ordering::Acquire) {
            callback()
        }
    }
}
pub(crate) fn connect_captured(
    target: &str,
) -> Result<(Channel, Arc<CapturedTransport>), tonic::transport::Error> {
    let capture = Arc::new(CapturedTransport {
        connected: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        on_close: Mutex::new(None),
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{target}"))?
        .connect_with_connector_lazy(TcpConnector {
            dial: Arc::new(|host, port| Box::pin(TcpStream::connect((host, port)))),
            capture: Some(capture.clone()),
        });
    Ok((channel, capture))
}
struct TrackedTcp {
    tcp: TcpStream,
    capture: Option<Arc<CapturedTransport>>,
}
impl Drop for TrackedTcp {
    fn drop(&mut self) {
        if let Some(c) = &self.capture {
            c.close()
        }
    }
}
impl AsyncRead for TrackedTcp {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.tcp).poll_read(cx, buf)
    }
}
impl AsyncWrite for TrackedTcp {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.tcp).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.tcp).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.tcp).poll_shutdown(cx)
    }
}

fn io(msg: &str) -> std::io::Error {
    std::io::Error::other(msg)
}

#[cfg(test)]
mod tests;
