//! Legacy port inspection is read-only. Observed PID/UID/name is not custody.
//! An exact managed birth must be stopped through the existing Stop protocol.

use polaris_helper_proto::response::FreePort;

/// freeport 的进程操作依赖（trait 便于测试 mock；生产用 [`ProdFreePortDeps`]）。
///
/// Read-only observations; this interface deliberately exposes no signal primitive.
pub trait FreePortDeps: Send + Sync {
    /// `/proc/<pid>` 属主 uid（Go `procUID`，:158-167）。None = 进程已退出 / 读失败。
    fn proc_uid(&self, pid: u32) -> Option<u32>;
    /// `/proc/<pid>/comm` 进程名（Go :311，trim 后）。None = 读失败（当不存在）。
    fn proc_comm(&self, pid: u32) -> Option<String>;
}

/// Read-only /proc diagnostic observations.
#[derive(Debug, Default, Clone)]
pub struct ProdFreePortDeps;

impl FreePortDeps for ProdFreePortDeps {
    fn proc_uid(&self, pid: u32) -> Option<u32> {
        // Go: os.Stat("/proc/<pid>") → Stat_t.Uid
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(format!("/proc/{pid}"))
            .ok()
            .map(|m| m.uid())
    }

    fn proc_comm(&self, pid: u32) -> Option<String> {
        // Go: os.ReadFile("/proc/<pid>/comm") → TrimSpace
        let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        Some(s.trim().to_string())
    }
}

/// ss -ltnp 输出里提取 pid（移植自 Go `ssPidRe = regexp.MustCompile("pid=(\\d+)")`，:287）。
///
/// 返回去重后的 pid 列表（Go 用 map\[string\]bool 去重，:296-299）。
#[must_use]
pub fn parse_ss_pids(ss_output: &str) -> Vec<u32> {
    let mut pids: Vec<u32> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // 手写提取 "pid=<digits>"（避免引 regex 依赖；Go 用 regexp，语义等价）。
    for hit in ss_output.match_indices("pid=") {
        let rest = &ss_output[hit.0 + 4..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u32>() {
            if seen.insert(n) {
                pids.push(n);
            }
        }
    }
    pids
}

/// Legacy read-only port inspection. No name or UID grants termination authority.
///
/// 参数：
/// - `pids`：从 ss 输出提取的 LISTEN 持有者 pid 列表（空 → `OK free`）。
/// - `caller_uid`：对端进程 uid（跨用户防误杀边界）。
/// - `deps`：进程操作依赖（mock / prod）。
pub fn free_port(pids: &[u32], caller_uid: u32, deps: &dyn FreePortDeps) -> FreePort {
    if pids.is_empty() {
        return FreePort::Free;
    }
    // A name/UID observation does not establish native process ownership.
    // This legacy operation is inspection-only; exact Stop owns termination.
    let names = pids
        .iter()
        .map(|&pid| {
            if deps.proc_uid(pid) == Some(caller_uid) {
                deps.proc_comm(pid)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("pid:{pid}"))
            } else {
                format!("pid:{pid}")
            }
        })
        .collect();
    FreePort::Foreign { names }
}

#[cfg(test)]
mod tests;
