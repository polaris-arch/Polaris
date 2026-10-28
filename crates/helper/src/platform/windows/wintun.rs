//! wintun 适配器**存在性**探测（#327）。
//!
//! ## 地雷背景
//!
//! 就绪门（`core-supervisor::wait_for_core_ready`）的判据只有「管理 API 端口可连 + 进程活」，两条都与
//! TUN 网卡无关 —— 于是「sing-box 活着、mixed 代理正常、wintun 网卡从未创建」这一态会被判成起核成功，
//! 用户看到「已连接」而 TUN 根本不生效。
//!
//! 本模块给出那条缺失的**正向**判据 [`probe_adapter_present`]：有界轮询枚举，看见本次配置的那张适配器
//! 即成立；超时则回报「没建出来」（上层据此把本条起核腿计入重试预算）。
//!
//! ## 为什么**没有**反向的「起核前等上一张释放」
//!
//! 曾经有（#159：Windows session-0 服务模式下停核走 `TerminateProcess` 硬杀，可能残留 `polaris-tun0`，
//! 故起核前有界轮询等它消失）。**已删**，根因是那条腿要解决的问题内核自己就解决了：sing-tun
//! `tun_windows.go` 的 `New()` 里 `wintun.CreateAdapter` 撞 `os.ErrExist` 会立刻 `OpenAdapter` **复用**
//! 同名网卡，残留适配器压根不构成起核失败。那条腿的四个分支也无一阻断起核（全 fail-open 只记日志），
//! 唯一的实际效果是在残留时把起核热路径白拖最多 3s。
//!
//! ## 与 app 侧 `verify_tun_route_captured`（出口归属差分）**不重叠**
//!
//! 那条问「默认路由被谁占」，答不了「适配器压根没建出来」（网卡不存在时默认路由当然也没切走，但两者的
//! 用户动作完全不同 —— 一个是「断开别的 VPN」，一个是「wintun 驱动/权限有问题」）。
//!
//! ## 生产接线点在 app 侧，不在 helper 侧
//!
//! **本模块 Polaris 自有增强，Go helper-win 无对应**（Go 靠 sing-box 核自管 wintun TUN，无适配器探测）。
//!
//! `probe_adapter_present` 的生产调用点是 **app 侧** `src-tauri/src/runtime/proxy.rs` 的
//! `ProxyRuntime::probe_tun_adapter_present`，挂在**起核重试循环内**的就绪判定之后（每条腿验一次；健康
//! 路径首次枚举即命中，零 sleep）。逐腿而非循环外单次的理由、以及「不可断言就放行」的纪律见该函数文档。
//!
//! **helper 侧 [`crate::platform::windows::winproc::WinProcOps::start_singbox`] 仍不调用本模块**，且这是
//! 有意的：`GetAdaptersAddresses` 是免提权普通用户 API，app 进程直调即可；为它加一条 helper wire 命令
//! 只会凭空多出「协议版本 × 已部署旧 helper 不识别」的兼容面，换不来任何能力。
//!
//! ## 不触碰宿主
//!
//! 适配器枚举经 [`AdapterProbe`] trait 抽象：生产侧由 Windows IP Helper API 实现（`#[cfg(windows)]`，
//! `GetAdaptersAddresses`），测试用 [`MockAdapterProbe`]（纯内存，Linux 可测轮询骨架）。

use std::time::{Duration, Instant};

/// 把 [`std::net::Ipv4Addr`] 写成 WinSock `IN_ADDR.S_addr` 的内存序。
///
/// `S_addr` 虽然在 Rust 侧是 `u32`，WinSock 读的是这 4 个字节的**网络序**。用 `from_ne_bytes`
/// 保证字段在大小端机器上的内存字节都逐字等于 IPv4 octets；直接 `u32::from_be_bytes` 在 Windows
/// 小端机上会把 `1.2.3.4` 写成 `4.3.2.1`。
#[must_use]
fn ipv4_s_addr(ip: std::net::Ipv4Addr) -> u32 {
    u32::from_ne_bytes(ip.octets())
}

/// 探测的目标适配器名前缀（Polaris 命名谱系）。
///
/// - `polaris-`：Polaris 自身命名谱系（`polaris-tun0` 等）。
pub const PROBE_PREFIXES: &[&str] = &["polaris-"];

/// 默认探测超时（有界轮询，宁可超时回报也不无限阻塞 start）。
///
/// 选 3s：就绪门过后内核挂上 wintun 网卡通常 <1s，3s 给驱动的异步挂载留余量；超时则回报
/// [`PresenceOutcome::Absent`]（上层计入起核重试预算，不把 start 主路径拖太久）。
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_millis(3000);

/// 默认轮询间隔（生产 200ms，测试可设更短）。
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// 适配器探测抽象（trait 便于测试 mock；生产用 Windows IP Helper API）。
///
/// 返回当前存在的、匹配 [`PROBE_PREFIXES`] 之一的适配器名列表。
pub trait AdapterProbe: Send + Sync {
    /// 枚举当前匹配前缀的适配器名。空 = 本机眼下没有任何 Polaris 谱系适配器。
    fn list_matching_adapters(&self) -> std::io::Result<Vec<String>>;
}

/// wintun 适配器**存在性**探测结果（#327）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceOutcome {
    /// 目标适配器已出现（在超时内枚举到同名项）。
    Present,
    /// 超时仍未见到目标适配器；`seen` 为最后一次枚举到的同前缀适配器名（可能为空）。
    Absent { seen: Vec<String> },
    /// 枚举出错（上层按「不可断言」处理：**不闸**，宁可漏检也不误拦正常起核）。
    Error(String),
}

/// 适配器名是否落在 [`PROBE_PREFIXES`] 的可枚举面内（**枚举实现与调用方共用的唯一判据**）。
///
/// # 为什么必须导出给调用方，而不是只在枚举实现里内联
///
/// [`AdapterProbe::list_matching_adapters`] 只返回前缀命中的适配器 —— 这是设计（不枚举用户的物理网卡）。
/// 于是「用户把 TUN 接口名改成 `my-tun`」时，本模块**永远**看不到那张网卡，存在性探测会恒返
/// [`PresenceOutcome::Absent`]。若调用方据此判「网卡没建出来」，后果是把一次**完全正常**的起核判成失败
/// 并杀核 —— 比它要修的缺陷严重得多。故调用方必须先用本谓词判「这个名字我方到底看不看得见」，
/// 看不见就整条跳过（不可断言）。把判据留在本模块 = 前缀表改动时两侧同步，不产生第二真值源。
#[must_use]
pub fn adapter_name_is_probeable(name: &str) -> bool {
    PROBE_PREFIXES.iter().any(|pfx| name.starts_with(pfx))
}

/// 有界轮询骨架（#327）。
///
/// 返回 `Ok((satisfied, last_list))`：`satisfied` = `done` 在超时内成立过；`last_list` = 最后一次枚举
/// 结果（调用方据此给出语境相关的诊断信息）。枚举 `Err` → `Err(msg)`，**立即**返回不重试
///（best-effort：探测手段本身坏了就交回上层，不把 start 拖在这里）。
///
/// # 为什么与 [`probe_adapter_present`] 分成两层
///
/// 这一层只管**时序**（deadline、零间隔兜底、先判后睡的顺序），谓词由调用方给。时序里最易悄悄写错的
/// 是顺序：先睡后判在健康路径上会白等一个 `poll_interval`（起核热路径上就是 200ms），而这种差异不会有
/// 任何测试自己冒出来 —— 收在一处、由 `present_immediately_when_adapter_already_up` 的零 sleep 断言钉死，
/// 比让它散在谓词旁边可靠。
fn poll_adapters<P, S>(
    probe: &P,
    timeout: Duration,
    poll_interval: Duration,
    sleep: &S,
    done: impl Fn(&[String]) -> bool,
) -> Result<(bool, Vec<String>), String>
where
    P: AdapterProbe + ?Sized,
    S: ThreadSleep,
{
    let deadline = Instant::now() + timeout;
    let interval = if poll_interval.is_zero() {
        Duration::from_millis(1)
    } else {
        poll_interval
    };
    loop {
        match probe.list_matching_adapters() {
            Ok(list) if done(&list) => return Ok((true, list)),
            Ok(list) => {
                if Instant::now() >= deadline {
                    return Ok((false, list));
                }
                sleep.sleep(interval);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// 有界轮询探测**目标适配器出现**（#327：起核后逐腿正向验证 TUN 网卡真被建出来）。
///
/// 语义：
/// - 每 `poll_interval` 调一次 [`AdapterProbe::list_matching_adapters`]。
/// - 枚举结果里出现 `expected`（ASCII 大小写不敏感）→ [`PresenceOutcome::Present`]，立即返回。
/// - `deadline` 到仍没有 → [`PresenceOutcome::Absent`]，带最后一次枚举到的同前缀适配器名。
/// - 枚举 `Err` → [`PresenceOutcome::Error`]，立即返回（上层按不可断言处理，不闸）。
///
/// # 参数
///
/// - `probe`：适配器枚举实现。
/// - `expected`：本次配置的 TUN 接口名。
/// - `timeout`：总轮询预算。
/// - `poll_interval`：两次枚举之间的 sleep。
/// - `sleep`：sleep 抽象（测试用 fake clock，生产用 [`std::thread::sleep`]）。
///
/// # 调用前必须先过 [`adapter_name_is_probeable`]
///
/// 见该谓词文档：`expected` 不在可枚举前缀面内时本函数恒返 `Absent`，那是「我方看不见」而非
/// 「网卡不存在」，两者的正确处置完全相反。
///
/// # 大小写不敏感比对的理由
///
/// Windows 适配器 FriendlyName 由我们经 sing-box config 指定，理论上逐字回读；但注册表/网络配置层
/// 历来对网卡名做大小写不敏感处理，且这里判错的代价是**杀掉一个正常的核**。取宽松侧。
pub fn probe_adapter_present<P, S>(
    probe: &P,
    expected: &str,
    timeout: Duration,
    poll_interval: Duration,
    sleep: &S,
) -> PresenceOutcome
where
    P: AdapterProbe + ?Sized,
    S: ThreadSleep,
{
    let hit = |list: &[String]| list.iter().any(|n| n.eq_ignore_ascii_case(expected));
    match poll_adapters(probe, timeout, poll_interval, sleep, hit) {
        Ok((true, _)) => PresenceOutcome::Present,
        Ok((false, seen)) => PresenceOutcome::Absent { seen },
        Err(e) => PresenceOutcome::Error(e),
    }
}

/// sleep 抽象（测试 fake clock；生产用 [`StdSleep`]）。
pub trait ThreadSleep {
    fn sleep(&self, dur: Duration);
}

/// 生产 sleep（[`std::thread::sleep`]）。
#[derive(Debug, Default, Clone, Copy)]
pub struct StdSleep;

impl ThreadSleep for StdSleep {
    fn sleep(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}

// ===== Windows 生产实现 =====

#[cfg(windows)]
#[allow(unsafe_code)]
// windows-sys FFI（GetAdaptersAddresses/解引用 widestring）必须 unsafe；每处附 SAFETY。
mod win_impl;

#[cfg(windows)]
pub use win_impl::{best_route_interface_alias, best_route_interface_index, WinAdapterProbe};

// ===== 测试 mock =====

#[cfg(test)]
#[derive(Debug, Default)]
struct MockAdapterProbe {
    /// 适配器列表序列：每次 list_matching_adapters 返回下一个（模拟「逐渐释放」）。
    /// 用 `Mutex<Vec<Vec<String>>>` 模拟「每次轮询后适配器减少」。
    pub(crate) snapshots: std::sync::Mutex<std::collections::VecDeque<Vec<String>>>,
}

#[cfg(test)]
impl MockAdapterProbe {
    fn new(snapshots: Vec<Vec<String>>) -> Self {
        Self {
            snapshots: std::sync::Mutex::new(snapshots.into()),
        }
    }
}

#[cfg(test)]
impl AdapterProbe for MockAdapterProbe {
    fn list_matching_adapters(&self) -> std::io::Result<Vec<String>> {
        let mut q = self.snapshots.lock().unwrap();
        if q.len() == 1 {
            // 最后一帧一直返回（模拟稳态）
            Ok(q.front().cloned().unwrap_or_default())
        } else {
            Ok(q.pop_front().unwrap_or_default())
        }
    }
}

/// 测试 fake sleep（不真睡，记录总睡眠时长）。
#[cfg(test)]
#[derive(Debug, Default)]
struct FakeSleep {
    slept: std::sync::Mutex<Duration>,
}

#[cfg(test)]
impl ThreadSleep for FakeSleep {
    fn sleep(&self, dur: Duration) {
        *self.slept.lock().unwrap() += dur;
    }
}

#[cfg(test)]
mod tests;
