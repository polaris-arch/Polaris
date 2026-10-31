//! Windows 特权 helper 本体（§D 特权矩阵 win 行，移植自 上游 `helper-win/` Go module）。
//!
//! ## 职责（system-design §D.3 + 维度7）
//!
//! Polaris Windows helper 是一个以 LocalSystem 身份运行的 SCM 服务，监听 token 鉴权的命名管道，按行协议驱动
//! sing-box 启停。装一次（安装期一次 UAC）后，普通用户 app 经命名管道零提权启停 sing-box —— 切节点/停止/
//! 退出/崩溃回收均免再次 UAC。本模块是该 helper 的 Rust 重写（day-1 全 Rust 收益：与 core-supervisor /
//! helper-proto 共享类型，消灭跨语言 wire drift）。
//!
//! 源真值（Go 源仅 oracle，Rust 重写不编译 Go）：
//! - `~/Code/polaris/helper-win/helper.go` —— 协议骨架 + 并发纪律（mu/child/watchParent/terminateChild）
//! - `~/Code/polaris/helper-win/main.go` —— 入口（--console dev / SCM 服务）
//! - `~/Code/polaris/helper-win/service.go` —— SCM 托管 + 命名管道（SDDL）+ serve 循环
//! - 卸载由共享 `polaris-windows-cleaner` 原生 SCM/对象句柄实现负责。
//! - `~/Code/polaris/helper-win/winproc.go` —— 进程枚举/kill/Job Object/freeport/IP 转发/spawn 旁路
//!
//! ## 共用层（不在本模块 —— 见 crate 顶层）
//!
//! - token 文件读取 + **常量时间比对**：[`crate::token`]（与 mac 逐字同 → 单一真值。原
//!   `helper-win/src/token.rs` 是 thin re-export 壳，单 crate 后已删；调用点直接用
//!   [`crate::token::is_authed_constant_time`]）。
//! - **不用** [`crate::line_io`]：win 走裸 Win32 `HANDLE` 的**整帧读**（命名管道，一次 `ReadFile` 取整个
//!   请求帧再切行），不经 `std::io::BufRead` —— 是**真平台差异**，不强行归一。
//! - **管理员内容安装**：[`crate::core_cli`] 与 [`crate::core_store`]；运行时只验选定 CoreID。
//! - `polaris-windows-cleaner`：固定用途原生卸载，独立 worker，拒绝任意命令/路径。
//! - [`logic`]：协议层纯逻辑（iface/cfg/port 白名单、filepath basename、TCP 端口字节序解析、sing-box 镜像匹配）。
//! - [`coreacl`]：受保护核目录的 owner/DACL **判据**（纯逻辑，Linux 单测；搬运腿在 [`winproc`]）。
//! - [`ops`]：系统操作 trait（[`ops::ProcessOps`] / [`ops::NetTableOps`] / [`ops::IpForwardingOps`]）+ mock。
//! - [`wintun`]：wintun 适配器释放探测（维度7 #30，trait 抽象的有界轮询）。
//! - [`helper`]：协议分派核心（Go `handle()` 的 switch 分支，经 trait 抽象，跨平台可测）。
//! - [`winproc`]：Windows FFI 生产实现（`#[cfg(windows)]`，`helper-win/winproc.go` 对应）。
//! - [`service`]：SCM 服务 + 命名管道监听（`#[cfg(windows)]`，`helper-win/service.go` + `main.go` 对应）。
//!
//! ## 地雷（system-design §C / 维度7，B3 真机必验）
//!
//! - **session-0 无 console**：服务模式 `GenerateConsoleCtrlEvent(CTRL_BREAK)` 是 no-op（只投给共享 console 的
//!   进程），停核走 `TerminateProcess` 硬杀 → wintun 适配器/路由残留竞态（§C #8）。Job Object
//!   （`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`）兜底防孤儿进程（不防 wintun 残留）。
//! - SYSTEM 自卸只确认固定 worker 已启动；worker 私有映像残留被计为 partial。
//!   不以 cmd/rmdir 或任意删除命令处理正在运行的映像；App/NSIS 可使用随包 UAC cleaner。

#![deny(unsafe_code)]

pub mod coreacl;
pub mod daemon;
pub mod helper;
pub mod logic;
#[cfg(windows)]
pub mod native_caller;
/// 本机网络接口只读枚举（**免提权**，app 进程直调；住在本 crate 纯为复用已有的 `windows-sys` FFI —— 理由见模块文档）。
pub mod netinfo;
pub mod ops;
pub mod wintun;

#[cfg(windows)]
pub mod service;
#[cfg(windows)]
pub(crate) mod user_data;
#[cfg(windows)]
pub mod winproc;

// 便利重导出：让 `platform::windows::WinHelper` 等无需钻模块路径。
pub use helper::{HandleOutcome, WinHelper};

// daemon 入口（binary main 经 cfg 调）：serve 入口即 [`daemon::daemon_main`]（内部 MkdirAll support +
// console/SCM 分支，接现有 service::run_console / run_service）。
pub use daemon::{daemon_main, parse_args as parse_daemon_args, WinArgs};

/// Windows helper protoVersion（三平台统一演进，见 `polaris_helper_proto` crate 文档）。
///
/// **不移植** 上游的 win=5：那个 5 是 Go helper 的功能加法计数（v2 加 route-add/del、v3/v4 换
/// iface-metric 实现、v5 PowerShell 全路径），用途是让新 client 认出机器上装着的旧 helper。
/// Polaris 的 Rust helper 是首发，**上来就带齐全部命令**；后续代次仍随统一协议演进。
/// 含 install-core（P4 起与 mac/linux 拉平，见模块文档「共用层」一节）。
pub const PROTO_VERSION: u32 = polaris_helper_proto::proto_version::CURRENT;

/// 本 helper 的协议谱系平台标识（命名管道 + token 行）。
///
/// 用于 [`polaris_helper_proto::codec`] 帧编解码时决定是否在头部加 token 行
/// （[`polaris_helper_proto::Platform::has_token_line`]）。
pub const PLATFORM: polaris_helper_proto::Platform = polaris_helper_proto::Platform::Win;

// Windows SCM 服务名 / 命名管道名 / 默认 supportDir：真值住 helper-proto 的无 cfg 模块。
// 本模块的门带 `cfg(any(target_os = "windows", test))`，放在这里的常量别的 crate 在 Linux 上看不见；
// helper-client（装卸脚本、连管道、`sc` 起停）必须与本侧逐字一致，故两侧共引同一份（构造上单源）。
// sc.exe stop/delete 与自毁旁路均引用 `SERVICE_NAME`；`DEFAULT_SUPPORT_DIR` 是 `--support` 默认值，
// `helper.token` 落此目录（SYSTEM 私有，ACL 由安装期设定）。
pub use polaris_helper_proto::windows_helper::{DEFAULT_SUPPORT_DIR, PIPE_NAME, SERVICE_NAME};

/// 命名管道 SDDL（纵深防御；token 仍是主鉴权边界，`helper-win/service.go:34`）。
///
/// `D:(A;;FA;;;SY)(A;;GRGW;;;IU)`：
/// - `D:` DACL 起始。
/// - `(A;;FA;;;SY)` Allow / FILE_ALL_ACCESS / SYSTEM —— 服务本体（LocalSystem）完全控制管道。
/// - `(A;;GRGW;;;IU)` Allow / GENERIC_READ+GENERIC_WRITE / Interactive Users —— 桌面登录用户可连接读写。
///
/// 选 IU（交互登录用户）而非 BU/AU：Polaris GUI 由当前桌面登录用户运行，IU 精确覆盖「本机交互会话」，
/// 不授予服务账户/远程会话/网络登录。GENERIC_READ|WRITE 足以连接管道 + ReadFile/WriteFile，
/// 不授予 FILE_ALL_ACCESS（无需改 ACL/删管道）。
pub const PIPE_SDDL: &str = "D:(A;;FA;;;SY)(A;;GRGW;;;IU)";

#[cfg(test)]
mod tests;
