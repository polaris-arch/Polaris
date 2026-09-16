//! 协议分派核心（移植自 `helper-win/helper.go` 的 `handle()` switch 骨架）。
//!
//! ## 设计
//!
//! Go 源 `handle(conn)` 读 token+command+args 行 → `mu.Lock()` → switch command → 回复。本 Rust 实现把
//! 「帧已解析」与「分派」解耦：调用方（`service.rs` 的 serve 循环）负责读帧 + 鉴权 + 解析为
//! [`polaris_helper_proto::Request`]，本模块的 [`WinHelper::handle`] 负责 switch 分派 + 子进程状态管理
//! + 回复构造。这样：
//! - 分派逻辑（=Go switch 主体）跨平台纯逻辑，Linux 可单测。
//! - IO（命名管道读写）由 `service.rs` 承接（`#[cfg(windows)]`）。
//!
//! ## 并发纪律（对齐 Go `mu` / `child` / `childDone`）
//!
//! Go 用 `sync.Mutex` 保护 `child *exec.Cmd` + `childDone chan`，规则：
//! - **持锁摘除 child**（`child, childDone = nil, nil`），**不持锁收割**（`terminateChild` 最长阻塞 2s，
//!   持锁会饿死并发 ping/status）。
//! - 摘除后由摘除方独占收割权（后台 goroutine / `terminateChild`），watchParent 见 `child != c` 即退。
//!
//! 本实现用 [`std::sync::Mutex`]`<`[`ChildState`]`>` 镜像：持锁摘 pid，不持锁调 [`ops::ProcOps::reap_child`]。
//! 收割权的独占性由「摘除时取出 pid → 释放锁 → 对该 pid 调 reap」保证（同一 pid 只被取一次）。
//!
//! ## 子进程状态
//!
//! Go 的 `child *exec.Cmd` 承载 pid + Wait 语义。本 trait 抽象的 [`ops::ProcOps`] 只需 pid + reap_child，
//! 故 [`ChildState`] 只存 `Option<u32>`（pid）。真正的 `Wait()` 回收（Go `go func() { c.Wait(); close(done) }`）
//! 由 [`ops::ProcOps`] 实现内部承载（生产侧 `winproc` 持有 `std::process::Child` 或 Job Object 句柄）。

use crate::core_install::{install_core_files, InstallResult, SINGBOX_BIN_NAME_WIN};
use crate::platform::windows::coreacl;
use crate::platform::windows::logic;
use crate::platform::windows::ops::{NetTableOps, ProcOps};
use crate::token::{is_authed_constant_time, TokenStore};
use polaris_helper_proto::{Request, Response, ResponseKind};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// Windows helper 错误（helper 内部分派/状态错误，非协议错误码）。
#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    /// sing-box 启动失败（对应 Go `ERR start %v`，详情保留 io 错误）。
    #[error("sing-box start failed: {0}")]
    Start(String),
    /// LISTEN 持有者枚举失败（对应 Go `ERR enum %v`）。
    #[error("enum listen table failed: {0}")]
    Enum(String),
}

/// 子进程状态（Go `child` + `childDone` 的 Rust 等价）。
#[derive(Debug, Default, Clone)]
struct ChildState {
    /// 当前 child sing-box 的 pid（None = 无 child）。
    pid: Option<u32>,
}

/// Windows helper 核心分派器（移植自 `helper.go` 的 `handle()` + 模块级 `mu/child/childDone`）。
///
/// 持有：
/// - `child_mu`：保护 `child` 状态（Go `mu sync.Mutex`）。
/// - `child`：当前 child sing-box pid（Go `child *exec.Cmd`）。
/// - `token`：token 存储（鉴权）。
/// - `proc`：进程操作（spawn/kill/reap/job）。
/// - `net`：网络表操作（freeport）。
/// - `singbox_bin`：安装时锁定的 sing-box 路径（Go `singboxBin`）。
/// - `conf_dir`：允许的配置目录（Go `confDir`）。
/// - `service_name` / `support_dir`：自卸载旁路参数（Go `serviceName` / `supportDir`）。
pub struct WinHelper<T, P, N> {
    /// `Arc<Mutex<..>>`：child 状态须在**父死看护后台线程**（W15）与管道命令线程间共享（Go 的
    /// 包级 `mu`/`child` 全局，goroutine 直接引用；Rust 用 Arc 共享所有权）。
    child_mu: Arc<Mutex<ChildState>>,
    token: T,
    /// `Arc<P>`：父死看护的 `on_parent_dead` 闭包（后台线程）须持 proc 收割 child，故需共享所有权。
    proc: Arc<P>,
    net: N,
    singbox_bin: String,
    conf_dir: String,
    service_name: String,
    support_dir: String,
}

/// 单次 handle 的结果（成功响应，或鉴权失败/其它需提前终止的情形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandleOutcome {
    /// 正常响应（已成功处理，回此 Response 给客户端）。
    Respond(Response),
    /// 鉴权失败（Go `ERR auth` + return，不进 switch）。
    AuthFailed,
    /// uninstall 后自退（Go `uninstall` 分支：回 OK uninstalling → 800ms 后 os.Exit(0)）。
    /// 调用方（serve 循环）据此触发进程退出。
    UninstallAndExit(Response),
}

impl<T, P, N> WinHelper<T, P, N>
where
    T: TokenStore,
    P: ProcOps + 'static, // 'static：父死看护闭包捕获 Arc<P> 进 'static 后台线程（W15）。
    N: NetTableOps,
{
    /// 构造。
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        token: T,
        proc: P,
        net: N,
        singbox_bin: impl Into<String>,
        conf_dir: impl Into<String>,
        service_name: impl Into<String>,
        support_dir: impl Into<String>,
    ) -> Self {
        Self {
            child_mu: Arc::new(Mutex::new(ChildState::default())),
            token,
            proc: Arc::new(proc),
            net,
            singbox_bin: singbox_bin.into(),
            conf_dir: conf_dir.into(),
            service_name: service_name.into(),
            support_dir: support_dir.into(),
        }
    }

    /// 处理一个已解析的 [`Request`]（移植自 `helper.go:161-393` 的 `handle()`）。
    ///
    /// # 参数
    ///
    /// - `client_token`：客户端发来的 token 行（已 trim `\r\n`）。
    /// - `req`：解析后的请求。
    ///
    /// # 返回
    ///
    /// [`HandleOutcome`]，调用方据此回复 + 决定是否退出。
    #[must_use]
    pub fn handle(&self, client_token: &str, req: Request) -> HandleOutcome {
        // Go helper.go:169: if tok == "" || tok != tokenValue() { ERR auth; return }
        if !is_authed_constant_time(client_token, &self.token.token_value()) {
            return HandleOutcome::AuthFailed;
        }
        // 鉴权通过 → 进 switch（持锁分派）
        match req {
            Request::Ping => HandleOutcome::Respond(Response::Ok(ResponseKind::Pong(
                // Go helper.go:179: Windows Getuid()=-1 破坏正则，固定发 0。
                polaris_helper_proto::Pong::current(0),
            ))),
            Request::Version => HandleOutcome::Respond(Response::Ok(ResponseKind::Version {
                proto_version: crate::platform::windows::PROTO_VERSION,
            })),
            Request::Status => {
                // `pid` 是受管核记账，不是存活证据。Windows 生产侧已主动 drop `Child` 句柄，核自然退出
                // 后没有 reaper 回写这格；若只看 `Some(pid)`，app 会永久收到 running，崩溃恢复也永远
                // 不会触发。helper 自身以 SYSTEM 运行，能可靠执行这次原生进程探活；确定已死即在同一把
                // 锁里清账，再诚实回 stopped。探活的「未知」仍由 ProcOps 按宁漏勿误折为 true。
                let mut state = self.child_mu.lock().unwrap_or_else(PoisonError::into_inner);
                match self.live_managed_pid(&mut state) {
                    Some(pid) => {
                        // D2/D3：把 helper 手里那个句柄读到的两个事实一并回传。app 是 Medium IL，
                        // 对 SYSTEM child 的 OpenProcess 会被拒 ⇒ 它自己既读不到创建时间（pid 复用
                        // 不可发现），也读不到实跑映像（内核自证恒「未能进行」）。
                        let identity = self.proc.managed_identity(pid);
                        HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
                            polaris_helper_proto::Status::Running {
                                pid,
                                created: identity.created,
                                image: identity.image,
                            },
                        )))
                    }
                    None => HandleOutcome::Respond(Response::Ok(ResponseKind::Status(
                        polaris_helper_proto::Status::Stopped,
                    ))),
                }
            }
            Request::Stop { pid: want } => {
                // Go helper.go:189-200: 持锁摘 child → 后台收割（不持 mu）→ OK stopped <pid> / notrunning
                //
                // **受管 pid 身份判据**（本协议新增）：判定与摘除同在 child_mu 临界区内。手里的核不是
                // 请求所指的那个 = 它属另一个会话（老 stop 腿在管道上挂住期间用户重装 helper / 重起了
                // 核）⇒ 诚实 no-op，绝不「反正要停就杀当前的」。见 `stop_pid_matches`。
                let pid = {
                    let mut state = self.child_mu.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(cur) = state.pid {
                        if !polaris_helper_proto::stop_pid_matches(want, cur) {
                            return HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
                                polaris_helper_proto::Stop::Mismatch {
                                    want: want.unwrap_or(0),
                                    current: cur,
                                },
                            )));
                        }
                    }
                    state.pid.take() // 摘除（独占收割权）
                };
                match pid {
                    Some(pid) => {
                        // 摘除后不持锁、后台异步收割（Go: go terminateChild(c, done)）。
                        // W6 修：reap_child 内部起线程（不再同步阻塞 2s）→ stop 立即回复。
                        self.proc.reap_child(pid);
                        HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
                            polaris_helper_proto::Stop::Stopped { pid },
                        )))
                    }
                    None => HandleOutcome::Respond(Response::Ok(ResponseKind::Stop(
                        polaris_helper_proto::Stop::NotRunning,
                    ))),
                }
            }
            Request::Cleanup => {
                // Go helper.go:202-211: 先摘自家 child 精准收割，再 killAllSingbox 兜底外部孤儿 → OK cleaned
                if let Some(pid) = {
                    self.child_mu
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pid
                        .take()
                } {
                    self.proc.reap_child(pid);
                }
                let _ = self.proc.kill_all_singbox(&self.singbox_bin);
                HandleOutcome::Respond(Response::Ok(ResponseKind::Cleaned))
            }
            Request::Uninstall => {
                // Go helper.go:276-294: 收割 child → killAllSingbox 兜底 → spawnSelfUninstall →
                // OK uninstalling → 800ms 后 os.Exit(0)
                if let Some(pid) = {
                    self.child_mu
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pid
                        .take()
                } {
                    self.proc.reap_child(pid);
                }
                let _ = self.proc.kill_all_singbox(&self.singbox_bin);
                self.proc
                    .spawn_self_uninstall(&self.service_name, &self.support_dir);
                HandleOutcome::UninstallAndExit(Response::Ok(ResponseKind::Uninstalling))
            }
            Request::Start(params) => self.handle_start(&params),
            Request::FreePort { port } => self.handle_freeport(port),
            // W9 修：route-add / route-del 须区分 add/delete（此前合并丢了 op 区分）。
            Request::RouteAdd(rp) => self.handle_route(&rp.iface, &rp.cidrs, false),
            Request::RouteDel(rp) => self.handle_route(&rp.iface, &rp.cidrs, true),
            Request::IfaceMetric { iface, metric } => self.handle_iface_metric(&iface, metric),
            // D4：Windows 也刷 DNS 缓存。此前并在下方 `ERR unknown` 分支里 —— app 侧 Medium IL 跑
            // ipconfig 该机 rc=1（需提权），停核后 FakeIP 记录留在系统缓存里继续命中。
            Request::FlushDns => self.handle_flush_dns(),
            // P4：Windows 也落受保护内核目录（此前只有 mac/linux 有这个 chokepoint，win 落 ERR unknown，
            // helper 以 LocalSystem 直接 exec 用户可写路径下的 sing-box.exe）。
            Request::InstallCore(p) => self.handle_install_core(&p),
            // 以下命令 Windows helper 不支持（mac/linux 专属）—— Go default 分支回 ERR unknown。
            Request::LinuxStart(_)
            | Request::LinuxDnsSet(_)
            | Request::LinuxDnsRevert { .. }
            | Request::MacProxyTransaction { .. }
            | Request::MacProxyCompareTransaction { .. }
            | Request::MacProxyCompareCapability
            | Request::DefaultRestore { .. } => HandleOutcome::Respond(Response::Err(
                polaris_helper_proto::Error::new(polaris_helper_proto::ErrorCode::Unknown),
            )),
        }
    }

    /// `start` 分支（Go helper.go:338-390）。
    ///
    /// **并发纪律（修 W3）**：「已在跑？」判定 + `start_singbox` + 记录 pid 须在**同一 child_mu 临界区**内
    /// （对齐 Go `handle()` 顶部 `mu.Lock` 覆盖整个 start）。此前判定与记录之间放锁 → 两个并发 start 都见
    /// `None` → 双起核。记录 pid 后**先释放锁**再接父死看护（W15）——`spawn_watch_parent` 的闭包会重入
    /// child_mu（`is_current`/`on_parent_dead`），持锁调用即自死锁。
    fn handle_start(&self, p: &polaris_helper_proto::StartParams) -> HandleOutcome {
        // ===== 全程持锁的临界区：check → start → record pid =====
        let (started_pid, start_timing, started_created) = {
            let mut state = self.child_mu.lock().unwrap_or_else(PoisonError::into_inner);
            // 先复核旧记账，避免核自然退出后下一次 start 仍把死 pid 当作 already。探活与清账在同一
            // 临界区内，故并发 start 不会越过此判据双起核。
            if let Some(pid) = self.live_managed_pid(&mut state) {
                return HandleOutcome::Respond(Response::Ok(ResponseKind::Start(
                    polaris_helper_proto::Start::Already { pid },
                )));
            }
            // Go helper.go:349-351: cfg 空 → ERR no-config
            if p.cfg.is_empty() {
                return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                    polaris_helper_proto::ErrorCode::NoConfig,
                )));
            }
            // Go helper.go:352-356: cfg 不在 confDir 白名单 → ERR config-path-denied
            if !logic::cfg_allowed(&p.cfg, &self.conf_dir) {
                return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                    polaris_helper_proto::ErrorCode::ConfigPathDenied,
                )));
            }
            // **Polaris 新增（上游无）**：log 走与 cfg 同一条 lexical 白名单；`start_singbox`
            // 再逐级持有 no-reparse/no-share-delete prefix，固定 current/.1 两个 file object。
            // 否则 SYSTEM helper 既能越界创建，也会在用户可写父目录的 junction/rename 竞态中
            // 写错对象。生产下发的 log 与 cfg 同在 conf_dir，收紧无行为变化；空串放行。
            //
            // cfg 内容仍属于登录账户信任域：同一账户可读 token 且可写 conf_dir。抵抗同账户恶意
            // 进程需要签名 app identity + 完整资源闭包封存，不是 canonicalize/staging 主 JSON。
            // 协议契约见 StartParams 文档。
            if !p.log.is_empty() && !logic::cfg_allowed(&p.log, &self.conf_dir) {
                return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                    polaris_helper_proto::ErrorCode::LogPathDenied,
                )));
            }
            // P4（spec §3.4 · Q9）：exec 之前复核「核住的地方还是 SYSTEM 写、普通用户只读」吗。
            // 判定为放宽/异常 → 拒起核（下方 `core_acl_gate`）。锁内调用：这一步是 3 次
            // `GetNamedSecurityInfoW`，比本临界区里已有的 `start_singbox`（真起进程）便宜得多。
            if let Some(err) = self.core_acl_gate() {
                return HandleOutcome::Respond(Response::Err(err));
            }
            // Go helper.go:358-369: fwd=="1" → enableIPForwarding + startSingbox（均在生产侧
            // start_singbox 内；失败 → ERR start）。持锁调用 —— start_singbox 起进程后即返回（不 Wait），
            // 不长阻塞（与 Go 持 mu 调 c.Start() 同）。
            match self
                .proc
                .start_singbox(&self.singbox_bin, &p.cfg, &p.log, p.fwd)
            {
                Ok(started) => {
                    // Go helper.go:374-385: child = c（收割/Wait goroutine 由生产侧 ProcOps 承载）。
                    state.pid = Some(started.pid);
                    (started.pid, started.timing, started.created)
                }
                Err(e) => {
                    return HandleOutcome::Respond(Response::Err(
                        polaris_helper_proto::Error::with_detail(
                            polaris_helper_proto::ErrorCode::Start,
                            e.to_string(),
                        ),
                    ));
                }
            }
            // MutexGuard 在此块结束时释放 → 下方接线看护时锁已放。
        };

        // ===== W15 修：父死看护接线（Go helper.go:386-389: ppid>0 → go watchParent）=====
        // 此前 parent_pid 被丢弃 → app 崩溃/taskkill（管道 stop 够不到）后 sing-box 成孤儿。
        // 必须在释放 child_mu 后接线（闭包重入 child_mu，持锁调用即自死锁）。
        if let Some(ppid) = p.parent_pid.filter(|&ppid| ppid > 0) {
            // is_current(child_pid)：child 是否仍是当前的（被 stop/cleanup/新 start 摘除后返 false）。
            let st_cur = Arc::clone(&self.child_mu);
            let is_current = Box::new(move |cpid: u32| {
                st_cur.lock().unwrap_or_else(PoisonError::into_inner).pid == Some(cpid)
            });
            // on_parent_dead(child_pid)：与 stop 竞态下持锁摘除（独占收割权）→ 后台异步收割。
            let st_dead = Arc::clone(&self.child_mu);
            let proc = Arc::clone(&self.proc);
            let on_parent_dead = Box::new(move |cpid: u32| {
                let taken = {
                    let mut s = st_dead.lock().unwrap_or_else(PoisonError::into_inner);
                    if s.pid == Some(cpid) {
                        s.pid.take()
                    } else {
                        None // 他人已摘除（Go: child != c）→ 由他收割
                    }
                };
                if let Some(pid) = taken {
                    proc.reap_child(pid);
                }
            });
            self.proc
                .spawn_watch_parent(ppid, started_pid, is_current, on_parent_dead);
        }

        // Go helper.go:390: OK started <pid>
        HandleOutcome::Respond(Response::Ok(ResponseKind::Start(
            polaris_helper_proto::Start::StartedTimed {
                pid: started_pid,
                timing: start_timing,
                // D3：起核当时就把身份基线交给 app（**只发 created，绝不发 image**——
                // image 的 hex 路径不是 u64，会让旧 app 的 timing 解析整段返回 None）。
                created: started_created,
            },
        )))
    }

    /// `flush-dns` 分支（D4，Windows 新增；mac 的同名命令在 `platform/macos/flush_dns.rs`）。
    ///
    /// DNS 缓存是**机器级单缓存**，SYSTEM 下刷一次即全局生效（helper 与 app 不必各刷一次）。
    /// 失败时把 helper 侧自捕的 stdout+stderr 带进 `ERR ipconfig <detail>` —— 报错路径日志为空
    /// 等于这条腿没法诊断，而 ipconfig 的错误文字恰恰只在 stdout。
    fn handle_flush_dns(&self) -> HandleOutcome {
        match self.proc.flush_dns() {
            Ok(()) => HandleOutcome::Respond(Response::Ok(ResponseKind::FlushDns(
                polaris_helper_proto::FlushDns::Flushed,
            ))),
            Err(detail) => {
                HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::with_detail(
                    polaris_helper_proto::ErrorCode::Ipconfig,
                    detail,
                )))
            }
        }
    }

    /// `install-core` 分支（P4：S 受保护内核目录，Windows 拉平到 mac/linux 的形态）。
    ///
    /// **定位**：这是与 mac/linux 拉平的 chokepoint + 纵深，**不是消除提权面** —— helper.exe 本体
    /// 仍源自用户可写域（面 K 仍开着），故对同账户攻击者的边际收益≈0。它买到的是：核此后住在
    /// SYSTEM 写、普通用户只读的目录里，且每次换核都过一道 sha256 闸（读全字节进内存再落盘）。
    ///
    /// **与 mac/linux 有意不同构**：core_dir 由 `--support` 派生（`<support>\core`），**不读**任何
    /// 命令行/wire 传来的核路径。mac/linux 的 coreDir 来自安装期烧进 plist/unit 的 `--coredir`；
    /// Windows 这侧连那个入口一起收掉 —— SCM ImagePath 是提权后可改的，少一个会被读的路径参数
    /// 就少一条注入向量。
    ///
    /// **child 在跑 → `ERR busy`**：Windows 既 rename 不动运行中的 exe，也 rename 不动已被加载的
    /// DLL，不挡就是「`.new` 写得进、rename 失败」的半吊子安装。判活用 [`Self::live_managed_pid`]
    /// —— 与 [`Self::handle_start`] 同一把锁、同一个判据，不另造判活原语。
    ///
    /// **落盘目标被放宽 → `ERR coredir-acl-weakened` 且不写盘**（[`Self::install_core_acl_gate`]）：
    /// 与起核那道自检守的是**不同的面** —— 那条守 `dir(--singbox)`，本条守 `<support>\core`，而
    /// 未迁移窗口里前者按设计跳过。往一个普通用户能改的目录里落核，等于亲手把 sha256 闸校验过的
    /// 字节交给别人替换。
    fn handle_install_core(&self, p: &polaris_helper_proto::InstallCoreParams) -> HandleOutcome {
        // 判活在 child_mu 临界区内（与 handle_start 同款，顺带清掉核自然退出后的陈旧记账）。
        // 锁只覆盖判活，不覆盖落盘：装核要写几十 MB，持锁会把并发 ping/status/stop 一起按住
        // （本模块顶部的并发纪律）。放锁后若真有 start 抢进来，rename 会硬失败 → `ERR rename …`，
        // 是一条如实的错误；`.new + rename` 保证半成品永远不会变成生效的那个文件。
        let busy = {
            let mut state = self.child_mu.lock().unwrap_or_else(PoisonError::into_inner);
            self.live_managed_pid(&mut state).is_some()
        };
        if busy {
            return HandleOutcome::Respond(InstallResult::Busy.to_response());
        }
        // S5：落盘前复核落盘目标本身（**不是** exec 面）。放宽即回 `ERR coredir-acl-weakened` 且
        // 一个字节都不写 —— 见 [`Self::install_core_acl_gate`]。判活闸在前：`ERR busy` 是更具体的
        // 前置条件，且核在跑时 rename 本就必失败。
        if let Some(err) = self.install_core_acl_gate() {
            return HandleOutcome::Respond(Response::Err(err));
        }
        let core_dir = self.derived_core_dir();
        match install_core_files(
            &core_dir,
            Path::new(&p.src_dir),
            &p.want_hash,
            SINGBOX_BIN_NAME_WIN,
        ) {
            Ok(_) => HandleOutcome::Respond(Response::Ok(ResponseKind::Installed)),
            Err(e) => HandleOutcome::Respond(e.to_response()),
        }
    }

    /// 起核前的受保护核目录自检（P4 · spec §3.4 · Q9）。
    ///
    /// 返回 `Some(err)` = **拒起核**（判定为放宽/异常）；`None` = 放行。
    ///
    /// **两条腿刻意不合并**（Q9 拍板）：
    /// - 判定为「放宽/异常」→ 拒起核 + `ERR coredir-acl-weakened <哪个路径哪条 ACE>`。
    ///   通过态是不可被改的（安装脚本设成 SYSTEM/Administrators 私有 + `Users:(RX)`），所以
    ///   误判只可能来自判定函数本身 —— 宁严。
    /// - **读不到**（FFI 报错、对象不存在、`--singbox` 推不出目录）→ **warn 继续**。
    ///   读不到 ≠ 被放宽：把它折进拒绝腿，一次 FFI 失败就让用户连不上网，而那台机器的 ACL
    ///   可能完全正常。`libcronet.dll` 在不带 cronet 的核里本就缺席，正落这条腿。
    ///
    /// 自检对象取 `singbox_bin` 的**父目录**（真正要被 exec 的那个），不是 `<support>\core`
    /// —— 理由见 [`crate::platform::windows::coreacl::core_acl_targets`]。
    ///
    /// ## 两者不一致 ⇒ **warn + 放行，且跳过 ACL 判定**（不是「照样喂判据」）
    ///
    /// 不一致唯一会发生的状态里，`dir(--singbox)` 就是 `%APPDATA%\…\core_update`，owner 是登录
    /// 用户 ⇒ 必判 `OwnerNotPrivileged` ⇒ 拒起核。而这台机器**本就还没被加固**：它要跑的核与本批
    /// 之前跑的是同一个，拒它换不来任何安全收益，只是把「加固未生效」变成「彻底不可用」——
    /// 违反 spec §3.5「四象限无一 brick」与 §3.6「未迁移用户不打断」。
    ///
    /// 这条路径不是理论：安装脚本每次 `sc delete` + `New-Service`，正常升级不留不一致；**可达的是
    /// catch 回滚腿** —— 新增的 icacls 守卫任一 throw → 回滚 → `Copy-Item $helperBackup → $helperDst`
    /// 若被文件锁挡住，机器就停在「新 helper.exe + 旧 binPath」，那时拒起核 = 永久 brick。
    ///
    /// **放行不是安全退让**：要构造这个不一致必须改服务 ImagePath（HKLM，需管理员），已在威胁模型外；
    /// 而这台机器的 `<support>\core` 那一面仍由 install-core 那条腿守着（见 [`Self::install_core_acl_gate`]）。
    fn core_acl_gate(&self) -> Option<polaris_helper_proto::Error> {
        let Some(targets) = coreacl::core_acl_targets(&self.singbox_bin) else {
            // 推不出目录 = 配置异常，不是「被放宽」→ 同读不到腿（warn 继续）。但必须说出来：
            // 静默放行等于这道门在这台机器上根本不存在。
            log::warn!(
                "core acl self-check skipped: cannot derive a directory from --singbox {}",
                self.singbox_bin
            );
            return None;
        };
        // 实际执行面 != install-core 的落盘目标 ⇒ 这台机器的 helper ImagePath 还没迁移到受保护
        // 路径 ⇒ 加固根本还没生效。按「读不到」腿处理：warn + 放行，不喂判定（理由见函数文档）。
        let derived = self.derived_core_dir();
        let derived = derived.to_string_lossy();
        if !coreacl::same_win_path(&targets[0], &derived) {
            log::warn!(
                "core acl self-check skipped: the core still runs from {} instead of the protected \
                 {derived} — this machine's core-directory hardening is NOT in effect yet; \
                 reinstalling or upgrading the helper migrates it",
                targets[0]
            );
            return None;
        }
        self.judge_acl_targets(targets)
    }

    /// install-core 落盘前的受保护核目录自检（S5）。
    ///
    /// **与 [`Self::core_acl_gate`] 不是同形第二腿，守的是不同的面**：那条守 `dir(--singbox)`
    /// （实际执行面），本条守 [`Self::derived_core_dir`]（落盘目标面）。两者相等时它们查同一批对象，
    /// 但在未迁移窗口里前者**主动跳过** —— 少了本条，那段窗口里 `<support>\core` 没有任何腿在守，
    /// 而 install-core 恰恰是往那里写几十 MB 的那条命令。
    ///
    /// 判定为放宽 ⇒ 回 `ERR coredir-acl-weakened` 且**不写盘**（往一个普通用户能改的目录里落核，
    /// 等于亲手把 sha256 闸校验过的字节交给别人替换）。
    fn install_core_acl_gate(&self) -> Option<polaris_helper_proto::Error> {
        let core_dir = self.derived_core_dir();
        // 复用同一个兜底取材面构造器（目录 + 核 + 配套 DLL）：两处各拼一遍 = 两份会分叉的白名单。
        let bin = coreacl::join_win(&core_dir.to_string_lossy(), SINGBOX_BIN_NAME_WIN);
        let Some(targets) = coreacl::core_acl_targets(&bin) else {
            log::warn!(
                "install-core acl self-check skipped: cannot derive a directory from {}",
                core_dir.display()
            );
            return None;
        };
        self.judge_acl_targets(targets)
    }

    /// 两条腿共用的「枚举取材面 → 逐对象读 owner/DACL → 判定 → 分篮子处置」。
    ///
    /// **取材面 = 兜底白名单 ∪ 目录实际条目**：只问硬编码的两个文件名，攻击者在首装前预创建的
    /// 第三个文件（带 `SE_DACL_PROTECTED` 的 `Users:(F)`，安装脚本 `/inheritance:r` 的传播**按定义
    /// 跳过**它）永远不会被问到 —— 全程 Medium IL、零特权即可布置。判据侧（`judge_object`）零改动。
    ///
    /// 目录列不出来 ⇒ 进 `unreadable`（warn 继续），不是拒起核：Q9 的「读不到 ≠ 被放宽」在这里同样成立。
    fn judge_acl_targets(&self, targets: Vec<String>) -> Option<polaris_helper_proto::Error> {
        let dir = targets[0].clone();
        let (targets, enumerate_error) = match self.proc.list_dir_names(&dir) {
            Ok(names) => (
                coreacl::extend_targets_with_dir_entries(&targets, &dir, &names),
                None,
            ),
            Err(e) => (targets, Some(e)),
        };
        let read: Vec<(String, Result<coreacl::ObjectSecurity, String>)> = targets
            .into_iter()
            .map(|path| {
                let sec = self.proc.read_object_security(&path);
                (path, sec)
            })
            .collect();
        // `Users:(RX)` 缺席只 warn、绝不拒起核（helper 改不了 ACL，但这条链没有别的自曝腿 ——
        // app 读不到 dest ⇒ 每次起核白推 80MB + 内核自证恒告警，真机上完全静默）。
        let missing_read: Vec<&str> = read
            .iter()
            .filter(|(_, sec)| {
                sec.as_ref()
                    .is_ok_and(|s| !coreacl::grants_non_privileged_read_execute(s))
            })
            .map(|(path, _)| path.as_str())
            .collect();
        if !missing_read.is_empty() {
            log::warn!(
                "core acl self-check: no non-privileged read/execute grant on {} — the app runs at \
                 medium integrity and will not see the protected core, re-pushing it on every start",
                missing_read.join("; ")
            );
        }
        let mut outcome = coreacl::judge_core_objects(&read);
        if let Some(error) = enumerate_error {
            outcome.unreadable.push(coreacl::UnreadableObject {
                path: dir,
                error: format!("enumerate entries: {error}"),
            });
        }
        if !outcome.unreadable.is_empty() {
            log::warn!(
                "core acl self-check could not read: {}",
                outcome.unreadable_detail()
            );
        }
        if outcome.is_weakened() {
            log::error!("refusing to start the core: {}", outcome.findings_detail());
            return Some(polaris_helper_proto::Error::with_detail(
                polaris_helper_proto::ErrorCode::CoredirAclWeakened,
                outcome.findings_detail(),
            ));
        }
        None
    }

    /// install-core 的落盘目标 = `<support>\core`（**派生**，不从命令行/wire 取 —— SCM ImagePath
    /// 是提权后可改的，少一个会被读的核路径参数就少一条注入向量）。
    ///
    /// 与起核自检共用同一个表达式：两处各写一遍 `"core"`，改一处漏一处就是「装到 A、守着 B」。
    fn derived_core_dir(&self) -> std::path::PathBuf {
        Path::new(&self.support_dir).join("core")
    }

    /// 返回仍存活的受管核 pid；确定已死时原子清除陈旧记账。
    fn live_managed_pid(&self, state: &mut ChildState) -> Option<u32> {
        let pid = state.pid?;
        if self.proc.process_alive(pid) {
            Some(pid)
        } else {
            state.pid = None;
            None
        }
    }

    /// `freeport` 分支（Go helper.go:295-337）。
    fn handle_freeport(&self, port: u16) -> HandleOutcome {
        // Go helper.go:308-316: listenPidsForPort 失败 → ERR enum
        let pids = match self.net.listen_pids_for_port(port) {
            Ok(pids) => pids,
            Err(e) => {
                return HandleOutcome::Respond(Response::Err(
                    polaris_helper_proto::Error::with_detail(
                        polaris_helper_proto::ErrorCode::Enum,
                        e.to_string(),
                    ),
                ));
            }
        };
        // Go helper.go:313-315: 无 LISTEN → OK free
        if pids.is_empty() {
            return HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
                polaris_helper_proto::FreePort::Free,
            )));
        }
        // Go helper.go:317-330: 遍历 pid：是锁定 sing-box → TerminateProcess；否则回报占用者名（不杀）
        let mut killed: Vec<u32> = Vec::new();
        let mut foreign: Vec<String> = Vec::new();
        for pid in &pids {
            let img = self.proc.process_image_name(*pid);
            if logic::is_locked_singbox(&img, &self.singbox_bin) {
                if self.proc.terminate_pid(*pid).is_ok() {
                    killed.push(*pid);
                }
            } else {
                // Go helper.go:325-328: name := filepath.Base(img)；空 → "pid:<n>"
                let name = {
                    let base = logic::filepath_base(&img);
                    if base.is_empty() || base == "." {
                        format!("pid:{pid}")
                    } else {
                        base.to_owned()
                    }
                };
                foreign.push(name);
            }
        }
        // Go helper.go:333-337: foreign 非空 → OK foreign <names | names>（混合占用也归此，诚实告知未释放）
        if !foreign.is_empty() {
            HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
                polaris_helper_proto::FreePort::Foreign { names: foreign },
            )))
        } else {
            HandleOutcome::Respond(Response::Ok(ResponseKind::FreePort(
                polaris_helper_proto::FreePort::Killed { pids: killed },
            )))
        }
    }

    /// `route-add` / `route-del` 分支（Go helper.go:212-241）。
    ///
    /// **修 W9**：此前是空壳（只校验 iface 即回 `OK route`，netsh 从未执行，注释谎称在 winproc）。现真跑
    /// netsh：iface 白名单 + 每个 CIDR 校验后，经 [`ProcOps::apply_route`] 对每个合法 CIDR 跑
    /// `netsh interface <fam> add|delete route ... store=active`（`del` 区分 add/delete）。生产 FFI 在
    /// [`crate::platform::windows::winproc`]（真机门验生效），本分派层做校验 + 每 CIDR 派发（可 mock 测）。
    fn handle_route(&self, iface: &str, cidrs: &[String], del: bool) -> HandleOutcome {
        // Go helper.go:218-221: !ifaceAllowed(iface) → ERR iface-denied
        if !logic::iface_allowed(iface) {
            return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                polaris_helper_proto::ErrorCode::IfaceDenied,
            )));
        }
        // Go helper.go:226-240: for c := range Split(cidrsLine, ","): 空跳过 / ParseCIDR 失败跳过 /
        // 否则跑 netsh。CIDR 校验用 helper-proto::codec::is_valid_cidr（stdlib 解析，严于 Go 手写）。
        for cidr in cidrs {
            let cidr = cidr.trim();
            if cidr.is_empty() {
                continue; // Go: c == "" → continue
            }
            if !polaris_helper_proto::codec::is_valid_cidr(cidr) {
                continue; // Go: net.ParseCIDR err → continue（静默跳过非法 CIDR）
            }
            self.proc.apply_route(iface, cidr, del);
        }
        // Go helper.go:241: OK route（幂等 best-effort，最终由 app 侧校验）
        HandleOutcome::Respond(Response::Ok(ResponseKind::Route))
    }

    /// `iface-metric` 分支（Go helper.go:242-275，退役保留兼容）。
    ///
    /// DESIGN-REVIEW(iface-metric-retired-stub)：Go 侧此命令**已退役保留兼容**（proto v3/v4，自 Windows
    /// 禁 System 起客户端不再调用；新客户端 EXPECTED_PROTO 不调）。本 Rust 侧**忠实保留退役桩**——校验
    /// iface 白名单 + metric 范围后即回 `OK iface-metric`，**不执行** PowerShell `Set-NetIPInterface`。
    /// 这是与 Go 的**有意差异**（Go 仍跑 PowerShell 以兼容已部署的旧 proto helper；新客户端不触发本路径，
    /// 故不实现 PowerShell 无功能损失）。若未来需兼容旧 proto helper 的真实降权，须补 PowerShell 执行。
    fn handle_iface_metric(&self, iface: &str, metric: u16) -> HandleOutcome {
        // Go helper.go:253-256: !ifaceAllowed → ERR iface-denied
        if !logic::iface_allowed(iface) {
            return HandleOutcome::Respond(Response::Err(polaris_helper_proto::Error::new(
                polaris_helper_proto::ErrorCode::IfaceDenied,
            )));
        }
        // Go helper.go:257-261: metric 非法（Go atoi err 或 <0 或 >65535）→ ERR bad-metric
        // u16 天然 ∈ [0,65535]，无需额外检查（Go 的 <0/>65535 在 u16 下不可能）。
        let _ = metric;
        // Go helper.go:262-274: PowerShell Set-NetIPInterface（生产侧 exec，失败 → ERR set-metric）
        // 退役命令：生产侧直接 Ok（新客户端不再调用；保留仅为兼容已部署 proto≥3 helper）。
        HandleOutcome::Respond(Response::Ok(ResponseKind::IfaceMetric))
    }

    /// 服务停止/关机时的兜底收割（Go helper.go:400-410 `reapChildOnExit`）。
    ///
    /// 持锁摘 child → 不持锁 reap；child==nil 时兜底 killAllSingbox（覆盖「停止恰落在某次 stop 的
    /// 后台收割窗口内」—— Go 注释 helper.go:398-399）。
    pub fn reap_child_on_exit(&self) {
        let pid = {
            self.child_mu
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pid
                .take()
        };
        if let Some(pid) = pid {
            // 退出路径用**同步**收割（Go reapChildOnExit 的同步 terminateChild）：进程即将退出，
            // 须在返回前杀完 child，否则后台异步收割线程随进程消失 → 孤儿。命令路径才用异步 reap_child。
            self.proc.reap_child_blocking(pid);
        } else {
            let _ = self.proc.kill_all_singbox(&self.singbox_bin);
        }
    }
}

#[cfg(test)]
mod tests;
