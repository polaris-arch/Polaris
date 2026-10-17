//! 生产 [`Connector`] 实现 —— Unix socket（mac/linux）+ 命名管道（win）。
//!
//! ## 为什么在这里
//!
//! [`client::HelperClient`](crate::client::HelperClient) 经 [`Connector`] 工厂建连接（每请求一连接，对齐
//! 上游 `net.connect(SOCKET_PATH)` 模型）。测试注入返回 `MockStream`（测试替身）
//! 的 mock；**生产注入本模块的 [`UnixConnector`]（mac/linux）/ `PipeConnector`（win）** —— 这两处正是
//! `client.rs` / `transport.rs` 文档反复引用、但此前只有 mock、真实现缺失的落点（§2.4 装配级 missing）。
//!
//! ## 平台差异（真差异，cfg 隔离）
//!
//! - **mac/linux**：`std::os::unix::net::UnixStream::connect` + 有界非阻塞读取（移植 Go
//!   `conn.SetReadDeadline`，默认预算 5s）。写完请求帧后 `shutdown(Write)` 半关闭 = 上游 `sock.end(frame)`，通知
//!   helper「请求发完」（helper 据此立即处理并回响应，不必等 5s 读超时）。
//! - **win**：命名管道 `\\.\pipe\polaris-helper`。复用 helper 服务端同款同步 Win32 原语：
//!   `CreateFileW + WriteFile + ReadFile`，逐字节读到单行响应结束。真机同帧原生 A/B 往返约
//!   0.2s；`std::fs::File` 虽同为同步句柄，却会在 helper 已回包后额外挂数秒，不能替代。
//!   `ERROR_PIPE_BUSY` 在 `READ_TIMEOUT` 内有界重试。
//!
//! ## 移植纪律
//!
//! - Unix 路径全为安全 std；Windows FFI 收口在 `windows_pipe` 单模块，逐处审计安全不变量。
//! - trait 边界只暴露字节流原语（[`ConnectionStream`]），三平台 [`HelperClient`](crate::client::HelperClient) 共用。

use crate::client::{ClientError, Connector};
use crate::transport::ConnectionStream;
#[cfg(any(unix, windows))]
use crate::transport::READ_TIMEOUT;
#[cfg(unix)]
use std::io;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::{Duration, Instant};

// ===== Unix socket 连接器（mac/linux）=====

/// Unix domain socket 连接器（mac/linux 生产 [`Connector`]）。
///
/// 每次 [`Connector::connect`] 打开一条到 `socket_path` 的新连接并保存 5s 读预算（对齐 Go
/// `SetReadDeadline`）。连接拒绝（socket 不存在/helper 未跑）→ [`ClientError::Connect`]。
#[cfg(unix)]
pub struct UnixConnector {
    socket_path: PathBuf,
    read_timeout: Duration,
}

#[cfg(unix)]
impl UnixConnector {
    /// 用默认读超时（[`READ_TIMEOUT`] = 5s）构造。
    #[must_use]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            read_timeout: READ_TIMEOUT,
        }
    }

    /// 自定义读超时构造。
    #[must_use]
    pub fn with_timeout(socket_path: impl Into<PathBuf>, read_timeout: Duration) -> Self {
        Self {
            socket_path: socket_path.into(),
            read_timeout,
        }
    }

    /// socket 路径。
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

#[cfg(unix)]
impl Connector for UnixConnector {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        use std::os::unix::net::UnixStream;
        let stream = UnixStream::connect(&self.socket_path).map_err(|e| {
            ClientError::Connect(format!("connect {}: {e}", self.socket_path.display()))
        })?;
        if self.read_timeout.is_zero() {
            return Err(ClientError::Io(io::ErrorKind::InvalidInput.into()));
        }
        Ok(Box::new(UnixConnStream {
            stream,
            read_timeout: self.read_timeout,
        }))
    }
}

/// [`UnixStream`](std::os::unix::net::UnixStream) 的 [`ConnectionStream`] 包装。
///
/// [`ConnectionStream::shutdown`] 走 `Shutdown::Write` 半关闭（= 上游 `sock.end()`）—— 这正是
/// [`IoAdapter`](crate::transport::IoAdapter) 的 no-op shutdown 无法提供、必须由本包装实现的语义。
#[cfg(unix)]
struct UnixConnStream {
    stream: std::os::unix::net::UnixStream,
    /// 当前读预算：建连时 = 连接器默认（[`READ_TIMEOUT`]），随后由
    /// [`ConnectionStream::set_read_timeout`] 改写为调用方单请求预算的剩余量。
    read_timeout: Duration,
}

#[cfg(unix)]
impl ConnectionStream for UnixConnStream {
    fn read_until_timeout(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        use std::io::Read;
        // XNU rejects setsockopt after both halves close, even when a complete
        // response is still buffered. Never reset SO_RCVTIMEO after shutdown:
        // nonblocking reads + one absolute deadline also bound trickled bytes.
        let deadline = Instant::now() + self.read_timeout;
        self.stream.set_nonblocking(true)?;
        let result = (|| {
            let mut byte = [0u8; 1];
            let mut total = 0;
            loop {
                if Instant::now() >= deadline {
                    return Err(io::Error::from(io::ErrorKind::TimedOut));
                }
                let result = self.stream.read(&mut byte);
                if Instant::now() >= deadline {
                    return Err(io::Error::from(io::ErrorKind::TimedOut));
                }
                match result {
                    Ok(0) => return Ok(total),
                    Ok(_) => {
                        buf.push(byte[0]);
                        total += 1;
                        if byte[0] == b'\n' {
                            return Ok(total);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        // Wait only when no bytes are available, never beyond
                        // this call's remaining budget; do not busy-spin.
                        std::thread::sleep(
                            deadline
                                .saturating_duration_since(Instant::now())
                                .min(Duration::from_millis(1)),
                        );
                    }
                    Err(error) => return Err(error),
                }
            }
        })();
        // This adapter exposes blocking writes. Restore that mode even on read
        // failure so a caller reusing the stream does not inherit EWOULDBLOCK.
        let restored = self.stream.set_nonblocking(false);
        match result {
            Ok(total) => restored.map(|()| total),
            Err(error) => Err(error),
        }
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        if timeout.is_zero() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.read_timeout = timeout;
        Ok(())
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.stream.write_all(data)
    }

    fn shutdown(&mut self) -> io::Result<()> {
        // Polaris sock.end(frame)：半关闭写端，通知 helper「请求已发完」。
        self.stream.shutdown(std::net::Shutdown::Write)
    }
}

// ===== 命名管道连接器（win）=====

/// Windows 命名管道连接器（win 生产 [`Connector`]）。
///
/// 以原生同步 Win32 流打开 `\\.\pipe\polaris-helper`；写端无需半关闭（helper win 侧
/// 「裸 HANDLE 整帧读」按 `\n` 判帧完整，不依赖 EOF）。
#[cfg(windows)]
pub struct PipeConnector {
    pipe_path: PathBuf,
}

#[cfg(windows)]
impl PipeConnector {
    /// 用管道路径构造（默认 `\\.\pipe\polaris-helper`，由调用方从 [`InstallPaths`](crate::manager::InstallPaths) 取）。
    #[must_use]
    pub fn new(pipe_path: impl Into<PathBuf>) -> Self {
        Self {
            pipe_path: pipe_path.into(),
        }
    }

    /// 管道路径。
    #[must_use]
    pub fn pipe_path(&self) -> &Path {
        &self.pipe_path
    }
}

#[cfg(windows)]
impl Connector for PipeConnector {
    fn connect(&self) -> Result<Box<dyn ConnectionStream>, ClientError> {
        use crate::windows_pipe::WinPipeStream;
        use std::time::Instant;

        const ERROR_PIPE_BUSY: i32 = 231;
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            match WinPipeStream::connect(&self.pipe_path) {
                Ok(pipe) => return Ok(Box::new(pipe)),
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(ClientError::Connect(format!(
                        "open pipe {}: {e}",
                        self.pipe_path.display()
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
