//! Linux `systemd-resolved` 接管的 app 侧生命周期。
//!
//! root 写操作由调用方注入的 [`LinuxResolvedOps`] 完成；本层只负责 intent marker、失败收口、崩溃恢复
//! 与网络变化后的幂等重放。它与 macOS `networksetup` 控制器分离，因为 Linux 保存/恢复的是临时
//! per-link 状态，不应伪装成“备份物理网卡 DNS”。

use crate::proxy::MarkerFs;
use polaris_helper_proto::linux_dns::{CONTROLLED_DNS_IP, TUN_INTERFACE_NAME};
use serde::{Deserialize, Serialize};

/// root helper 能力的 app 侧抽象。
pub trait LinuxResolvedOps: Send + Sync {
    /// 在固定 Polaris TUN 链路上接管 resolved。
    fn takeover(&self) -> Result<(), String>;
    /// 撤销固定链路的 resolved 状态。
    fn revert(&self) -> Result<(), String>;
}

/// 崩溃恢复 marker。字段同时用于诊断与防止损坏/旧格式 marker 驱动错误接口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinuxResolvedMarkerData {
    /// 固定为 `polaris-tun0`。
    pub interface_name: String,
    /// 固定为受 TUN hijack 的 DNS 哨兵。
    pub server_ip: String,
    /// 写入时间（仅诊断，不参与决策）。
    pub at: u64,
}

/// 严格读取不会把不可读或损坏的 marker 当作不存在。
#[derive(Debug, thiserror::Error)]
pub enum LinuxResolvedMarkerError {
    #[error("read Linux resolved marker: {0}")]
    Read(#[source] std::io::Error),
    #[error("invalid Linux resolved marker: {0}")]
    Invalid(#[source] serde_json::Error),
    #[error("Linux resolved marker target does not match Polaris TUN")]
    WrongTarget,
}

struct LinuxResolvedMarker<Fs: MarkerFs> {
    fs: Fs,
    path: String,
}

impl<Fs: MarkerFs> LinuxResolvedMarker<Fs> {
    fn new(fs: Fs, path: impl Into<String>) -> Self {
        Self {
            fs,
            path: path.into(),
        }
    }

    fn write(&self) -> Result<(), String> {
        let marker = LinuxResolvedMarkerData {
            interface_name: TUN_INTERFACE_NAME.to_owned(),
            server_ip: CONTROLLED_DNS_IP.to_owned(),
            at: now_ms(),
        };
        let json = serde_json::to_string(&marker)
            .map_err(|error| format!("serialize Linux resolved marker: {error}"))?;
        self.fs
            .write_marker(&self.path, &json)
            .map_err(|error| format!("write Linux resolved marker: {error}"))
    }

    fn read_checked(&self) -> Result<Option<LinuxResolvedMarkerData>, LinuxResolvedMarkerError> {
        let Some(raw) = self
            .fs
            .read_marker_checked(&self.path)
            .map_err(LinuxResolvedMarkerError::Read)?
        else {
            return Ok(None);
        };
        let marker: LinuxResolvedMarkerData =
            serde_json::from_str(&raw).map_err(LinuxResolvedMarkerError::Invalid)?;
        if marker.interface_name != TUN_INTERFACE_NAME || marker.server_ip != CONTROLLED_DNS_IP {
            return Err(LinuxResolvedMarkerError::WrongTarget);
        }
        Ok(Some(marker))
    }

    fn clear(&self) -> Result<(), String> {
        self.fs
            .remove_marker(&self.path)
            .map_err(|error| format!("remove Linux resolved marker: {error}"))
    }
}

/// Linux resolved 生命周期控制器。
pub struct LinuxResolvedController<Ops: LinuxResolvedOps, Fs: MarkerFs> {
    ops: Ops,
    marker: LinuxResolvedMarker<Fs>,
}

impl<Ops: LinuxResolvedOps, Fs: MarkerFs> LinuxResolvedController<Ops, Fs> {
    /// 构造控制器。
    pub fn new(ops: Ops, fs: Fs, marker_path: impl Into<String>) -> Self {
        Self {
            ops,
            marker: LinuxResolvedMarker::new(fs, marker_path),
        }
    }

    /// 先写 intent，再请求 helper；helper 失败时已自行回滚，本层清除 intent 并向上返回可观测错误。
    pub fn takeover(&mut self) -> Result<(), String> {
        self.marker.write()?;
        if let Err(error) = self.ops.takeover() {
            let clear = self.marker.clear();
            return match clear {
                Ok(()) => Err(error),
                Err(clear_error) => Err(format!("{error}; {clear_error}")),
            };
        }
        Ok(())
    }

    /// 有 marker 才恢复；恢复成功后再清 marker。失败保留 marker 给下一次启动继续恢复。
    pub fn restore(&mut self) -> Result<(), String> {
        if self
            .read_marker_checked()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(());
        }
        self.ops.revert()?;
        self.marker.clear()
    }

    /// 网络变化时，仅在接管 intent 仍存在的条件下幂等重放。
    pub fn reconcile(&mut self) -> Result<(), String> {
        if self
            .read_marker_checked()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(());
        }
        self.ops.takeover()
    }

    /// 是否存在有效接管 marker。
    #[must_use]
    pub fn has_marker(&self) -> bool {
        self.read_marker_checked()
            .is_ok_and(|marker| marker.is_some())
    }

    /// 仅返回这次原 FS 读取的结果；marker 不存在本身不证明宿主资源已经恢复。
    pub fn read_marker_checked(
        &self,
    ) -> Result<Option<LinuxResolvedMarkerData>, LinuxResolvedMarkerError> {
        self.marker.read_checked()
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
