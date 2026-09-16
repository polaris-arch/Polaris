//! 外来隧道冲突：**宿主探测 → 判定 → 用户可见面**的接线层。
//!
//! 两半各自住在库里、此前**在生产里一个调用点都没有**：
//! - 探测（本机其它隧道在宣告哪些网段）= `polaris_system_integration::route_probe`；
//! - 判定（哪些算真冲突）= `polaris_config_engine::builder::tunnel_conflict`。
//!
//! 对用户而言「写完了但没人调」等于不存在：本机跑着独立 Tailscale / 公司 VPN / ZeroTier 时，
//! Polaris 既不探测也不判定 —— 而那正是 2026-09-08 那次报障的现场形态。本模块把两半接起来。
//!
//! # 三条纪律（都是这条链最容易失守的地方）
//!
//! 1. **「没探成」绝不折成「没冲突」。** 平台未实现（mac/win）、命令失败、核没起过，
//!    三种情形各有独立状态，谁都不会被渲染成一个自信的「无冲突」。空的 `conflicts`
//!    只在 [`TunnelConflictSnapshot::Probed`] 这一支里出现，那时它才真的是一句断言。
//! 2. **判据段取本次发射的那一份**（`emitted_conflict_criteria`）。核起来之后用户接着改配置、
//!    观测地址又多一条，都不该让这次判定拿到与内核吃的那份不同的段。
//! 3. **不放宽判据面。** 只认 `builder::tunnel_conflict` 登记在册的那几类，「外来隧道未被排除」刻意不算冲突
//!    （理由见 `builder::tunnel_conflict` 模块头注：逢隧道必报的告警会被无视或删掉，
//!    `plat-warn` 已经演过一遍）。本模块只接线，一类都不加。
//! 4. **收噪声只收在展示面，判定面喂的永远是全量。** 见 [`announced_routes`]：
//!    link-local 与组播在 `to_wire` 这一步才被摘掉，`detect_tunnel_conflicts` 拿到的仍是
//!    探测层原样返回的那份事实。反过来做（先滤再判）就是第 3 条说的「动判据面」——
//!    只不过方向是放窄，一样是让判定跟着一张会漂的过滤表走。
//!
//! # 形态取自哪两条既有腿
//!
//! - 后台 advisory 探测 + 世代/running 守卫：`system_takeover::spawn_system_proxy_residual_warning`；
//! - 判出问题 → warn 日志 + 一条只读 command 回给设置页：`dns_takeover` 的
//!   `displaced_dns_resolvers` / `commands::config::dns_takeover_report`。

use std::sync::Arc;

use polaris_config_engine::builder::tunnel_conflict::{
    detect_tunnel_conflicts, ConflictCriteria, ForeignTunnelRoute, TunnelConflict,
};
use polaris_config_engine::singbox::SingBoxConfig;
use polaris_config_engine::user_config::ProxyModeType;
use polaris_helper_proto::Platform;
use polaris_system_integration::route_probe::{
    is_link_local_or_multicast, ForeignTunnelProbe, RouteEntry, TunnelProbeOutcome,
};
use serde_json::{json, Value};

use super::ProxyRuntime;

/// 上一次外来隧道探测 + 判定的结果。
///
/// 🔴 **四支的存在理由就是「拿不到事实」不许长得像「事实是空的」**：只有
/// [`Self::Probed`] 里的空 `conflicts` 才是一句断言（看过了，判据面上没有冲突），
/// 其余三支各自说明「为什么没看成」。折成一个 `Vec<TunnelConflict>` 的话，
/// macOS 用户 —— 正是报障那台 —— 会看到一个自信的「无冲突」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TunnelConflictSnapshot {
    /// 本会话还没探过：核没起过 / 本次不是 TUN 模式 / 探测腿尚未跑完。
    NotProbed,
    /// 本平台没有探测实现（macOS / Windows；见 `route_probe` 模块头注「如实登记，不是忘了」）。
    Unsupported { platform: &'static str },
    /// 探测命令失败（spawn / 超时 / 非零退出）。
    ProbeFailed { error: String },
    /// 探到了。`conflicts` 空 = 判据面上真的没有冲突。
    Probed {
        foreign: Vec<ForeignTunnelRoute>,
        /// 外来隧道宣告的默认路由（全隧道）。与 `foreign` 是**两类事实**，不是它的子集。
        default_routes: Vec<ForeignTunnelRoute>,
        conflicts: Vec<TunnelConflict>,
        criteria: ConflictCriteria,
    },
}

/// 展示面：外来隧道宣告的**业务**网段 —— link-local 与组播从这里摘出去。
///
/// # 为什么这条过滤住在这一层，而不是探测层或渲染层
///
/// - **不在探测层**（`route_probe`）：那一层的职责是如实读回路由表，`netstat -rn` 打印的这些
///   条目确实是那个 utun 宣告的路由。过滤是解释，解释不该混进事实。该层只提供
///   [`is_link_local_or_multicast`] 这条判据，一处都不调用它。
/// - **不在渲染层**（`TunnelConflictBlock.tsx`）：那样一来噪声仍会跨过 IPC，命令的**每个**
///   消费方都得各自再滤一遍；且 TS 侧只有 `cidrsOverlap`、没有 `cidrContains`，收在那里等于
///   逼出第二份 CIDR 包含算术 —— 而「两份判据迟早会漂」正是这一整条链最要防的东西。
/// - **在这里**：`to_wire` 本就是事实 → 展示的那一步（同一函数已经在按「哪些键该缺」做展示决策），
///   过滤只发生一次，`conflicts` 与内存里的快照仍握着未经过滤的全量事实。
///
/// # 为什么另有一个 [`suppressed_route_count`]
///
/// 2026-09-12 实测：一台 Tailscale 断开的 mac 上，这 36 条噪声摘完**一条不剩**。只报「0 条」
/// 的话，界面上「探过了」与「压根没探」重新变得同形 —— 而那正是 `TunnelConflictSnapshot`
/// 分四支要防的事。把摘掉的条数一并下发，「这次真的读了一张路由表」才仍然看得见，
/// 顺带让过滤本身可审计（收了多少，用户看得见），而不是一次静默的消失。
///
/// # 为什么是两个函数而不是一个返回二元组的
///
/// `to_wire` 的每一支必须写成 `=> json!({…})` 的**直接字面量**：跨语言线格式门
/// （`ui/src/contracts/report-wire-parity.test.ts` 的 `toWireArms`）就是照这个形状取材的，
/// 中间插一个 `let` 会让 `probed` 这一支整个从它的取材面上消失（该门会因「四支不齐」转红，
/// 不是静默失效）。两个函数共用同一条判据、取相反的一半，按构造互补 ——
/// 由 `display_face_partitions_the_probe_fact` 钉住。
fn announced_routes(foreign: &[ForeignTunnelRoute]) -> Vec<&ForeignTunnelRoute> {
    foreign
        .iter()
        .filter(|route| !is_link_local_or_multicast(&route.prefix))
        .collect()
}

/// [`announced_routes`] 摘掉了多少条（同一条判据的另一半）。
fn suppressed_route_count(foreign: &[ForeignTunnelRoute]) -> usize {
    foreign
        .iter()
        .filter(|route| is_link_local_or_multicast(&route.prefix))
        .count()
}

impl TunnelConflictSnapshot {
    /// 下发给渲染端的线格式。
    ///
    /// **非 `Probed` 的三支不带 `conflicts` 键**（不是"带一个空数组"）：缺键会逼渲染端按
    /// `status` 分支，带空数组则会被 `conflicts.length === 0` 这种最自然的写法读成「无冲突」。
    ///
    /// `foreignTunnels` 是**展示面**（见 [`announced_routes`]）：link-local 与组播在这一步被摘掉，
    /// 摘掉的条数走 `suppressedRoutes`。`conflicts` 不受影响 —— 它是拿全量事实判出来的。
    ///
    /// **`foreignDefaultRoutes` 原样下发，不过 [`announced_routes`]**：那条过滤的判据是
    /// [`is_link_local_or_multicast`]（整条前缀落在某个 link-local / 组播块**里面**），
    /// 而 `0.0.0.0/0` / `::/0` 比任何块都宽、按构造不会被它收掉 —— 但「按构造收不掉」
    /// 与「刻意不让它收」是两件事：把默认路由喂进一张为 `foreign` 写的噪声表，
    /// 等于把这一类的可见性押在那张表将来不会变宽上。两类事实走两条通道，过滤也各归各。
    fn to_wire(&self) -> Value {
        match self {
            Self::NotProbed => json!({ "status": "notProbed" }),
            Self::Unsupported { platform } => json!({
                "status": "unsupported",
                "platform": platform,
            }),
            Self::ProbeFailed { error } => json!({
                "status": "probeFailed",
                "error": error,
            }),
            Self::Probed {
                foreign,
                default_routes,
                conflicts,
                criteria,
            } => json!({
                "status": "probed",
                "foreignTunnels": announced_routes(foreign),
                "suppressedRoutes": suppressed_route_count(foreign),
                "foreignDefaultRoutes": default_routes,
                "conflicts": conflicts,
                "criteria": criteria,
            }),
        }
    }
}

/// 平台标签（`Platform` 闭集 → 前端沿用的 Node 约定名）。
///
/// 穷举 `match`：`Platform` 日后加一支时编译器会逼这里表态，而不是静默落进某个兜底。
fn platform_tag(platform: Platform) -> &'static str {
    match platform {
        Platform::Mac => "darwin",
        Platform::Win => "win32",
        Platform::Linux => "linux",
        Platform::Other => "other",
    }
}

/// 探测结果 + 本次发射的判据段 → 快照（**纯函数**，判定的全部逻辑都在这里）。
///
/// `probe` 的 `Err` 是命令层失败的诊断串（`SystemIntegrationError` 或 `spawn_blocking` join 失败）。
pub(super) fn snapshot_from_probe(
    probe: Result<TunnelProbeOutcome, String>,
    criteria: ConflictCriteria,
) -> TunnelConflictSnapshot {
    let snapshot = match probe {
        Err(error) => return TunnelConflictSnapshot::ProbeFailed { error },
        Ok(TunnelProbeOutcome::Unsupported(platform)) => {
            return TunnelConflictSnapshot::Unsupported {
                platform: platform_tag(platform),
            }
        }
        Ok(TunnelProbeOutcome::Probed(snapshot)) => snapshot,
    };
    let foreign: Vec<ForeignTunnelRoute> = snapshot
        .foreign
        .iter()
        .map(|RouteEntry { prefix, interface }| ForeignTunnelRoute {
            interface: interface.clone(),
            prefix: prefix.clone(),
        })
        .collect();
    let default_routes: Vec<ForeignTunnelRoute> = snapshot
        .default_routes
        .iter()
        .map(|RouteEntry { prefix, interface }| ForeignTunnelRoute {
            interface: interface.clone(),
            prefix: prefix.clone(),
        })
        .collect();
    let conflicts = detect_tunnel_conflicts(&criteria.with_foreign(&foreign, &default_routes));
    TunnelConflictSnapshot::Probed {
        foreign,
        default_routes,
        conflicts,
        criteria,
    }
}

/// 本次发射的配置里、属于 **Polaris 自己** 的隧道接口名（喂 `foreign_tunnel_routes` 的 `own_interfaces`）。
///
/// 两个来源缺一不可：
/// - 配置里的 `interface_name`（Linux `polaris-tun0` / Windows 用户可改名）——**发射那一刻的意图**；
/// - `captured_alias`：post-flight 真观测到的出口接口别名。macOS 的 `utunN` 由内核分配、
///   配置里根本没有这个名字，只有这条腿认得出自己。
///
/// 漏传任一条，我方自己的路由就会被当成"别人的隧道"，与自己的 mesh/FakeIP 段一比
/// 冒出一堆自指告警 —— 而自指告警与真告警混在一起，整条告警就废了。
#[must_use]
pub(super) fn own_tunnel_interfaces(
    emitted: &SingBoxConfig,
    captured_alias: Option<&str>,
) -> Vec<String> {
    let mut names =
        polaris_config_engine::builder::tunnel_conflict::emitted_tun_interface_names(emitted);
    if let Some(alias) = captured_alias {
        let alias = alias.trim();
        if !alias.is_empty() && !names.iter().any(|n| n == alias) {
            names.push(alias.to_owned());
        }
    }
    names
}

/// 判出结果 → 日志。**四支都出声**：缺席必须自曝，否则「没执行」与「执行了没发现」在日志上同形。
fn log_snapshot(snapshot: &TunnelConflictSnapshot) {
    match snapshot {
        TunnelConflictSnapshot::NotProbed => {}
        TunnelConflictSnapshot::Unsupported { platform } => log::info!(
            "本平台（{platform}）尚无外来隧道探测实现 → 冲突判定**未进行**（这不是「无冲突」）"
        ),
        TunnelConflictSnapshot::ProbeFailed { error } => {
            log::warn!("外来隧道探测失败 → 冲突判定**未进行**（这不是「无冲突」）：{error}")
        }
        TunnelConflictSnapshot::Probed {
            foreign, conflicts, ..
        } => {
            if conflicts.is_empty() {
                // 两个数都报：只报展示面那个，日志会在一台「36 条全是噪声」的 mac 上写出
                // 「0 条外来隧道路由」—— 与「压根没探到隧道」同形。只报全量那个，日志又与
                // 用户在设置页看到的对不上。
                let suppressed = suppressed_route_count(foreign);
                log::info!(
                    "外来隧道探测完成：本机 {} 条外来隧道路由宣告业务网段（另有 {suppressed} 条 link-local / 组播已从展示面收掉），与本次配置的 FakeIP / 组网 / TUN 地址均不相交",
                    announced_routes(foreign).len()
                );
                return;
            }
            // 面向用户那路走 `tunnel_conflict_report` 命令 → 设置页；日志只是其中一路。
            let detail = conflicts
                .iter()
                .map(|c| format!("{} 的 {}（{:?}）", c.interface, c.prefix, c.kind))
                .collect::<Vec<_>>()
                .join("、");
            log::warn!(
                "本机其它隧道（Tailscale / 公司 VPN / ZeroTier …）与 Polaris 争同一网段，共 {} 条：{detail}",
                conflicts.len()
            );
        }
    }
}

impl ProxyRuntime {
    /// 外来隧道冲突的只读报告（`tunnel_conflict_report` 命令的载荷）。
    ///
    /// 只读快照拷贝；锁中毒 → 按「还没探过」下发（**不是**「无冲突」）。
    pub(crate) fn tunnel_conflict_report(&self) -> Value {
        match self.tunnel_conflicts.read() {
            Ok(snapshot) => snapshot.to_wire(),
            Err(error) => {
                log::error!("tunnel_conflicts 锁中毒: {error} → 按「还没探过」下发");
                TunnelConflictSnapshot::NotProbed.to_wire()
            }
        }
    }

    fn store_tunnel_conflict_snapshot(&self, snapshot: TunnelConflictSnapshot) {
        log_snapshot(&snapshot);
        match self.tunnel_conflicts.write() {
            Ok(mut slot) => *slot = snapshot,
            Err(error) => log::error!("tunnel_conflicts 锁中毒: {error} → 丢弃本次探测结果"),
        }
    }

    /// 起核就绪后探一次本机**其它**隧道，与本次发射的配置对撞判冲突。
    ///
    /// advisory：只读不动手，不是起核成立条件，失败只记日志。四条 `ip … show` 是只读查询，
    /// 但仍走 `spawn_blocking`（同步 `CommandRunner`，不占 async runtime）。
    ///
    /// 世代 + running 守卫在探测**完成之后**复查（与 `maybe_warn_system_proxy_residual` 同款）：
    /// 探测期间被接管/停核 ⇒ 丢弃陈旧结果，绝不让上一条核的观测覆盖新核的。
    pub(super) async fn probe_foreign_tunnel_conflicts(
        &self,
        my_generation: Option<u64>,
        criteria: ConflictCriteria,
        own_interfaces: Vec<String>,
    ) {
        let probe = tokio::task::spawn_blocking(move || {
            // 经 `platform_contracts` 装配，**不**直接调库的 `production_foreign_tunnel_probe`：
            // Windows 腿的进程内取材源（IP Helper / RAS API）在那里注入，直调会静默退回
            // 六条串行外部命令 —— 那正是起核峰值窗口里跑不完、探测超时的形态。
            super::platform_contracts::production_foreign_tunnel_probe()
                .probe_foreign_tunnels(&own_interfaces)
        })
        .await;
        if my_generation.is_some_and(|generation| {
            self.gate.generation() != generation || !self.status().running
        }) {
            log::debug!("外来隧道探测完成时起核世代已失效或核已停止 → 丢弃陈旧结果");
            return;
        }
        let probe = match probe {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(error) => Err(format!("探测任务 join 失败：{error}")),
        };
        self.store_tunnel_conflict_snapshot(snapshot_from_probe(probe, criteria));
    }

    /// advisory 探测移出起核关键路径；非 TUN 模式当场登记「没探」。
    ///
    /// # 为什么只在 TUN 模式探（射程声明，不是漏写）
    ///
    /// 三类冲突的坏处**都发生在 OS 路由表上**：`MeshOverlap` 是「两个声索人争同一段、
    /// 谁赢取决于装载顺序」，`TunAddressOverlap` 是地址空间撞车，`FakeIpOverlap` 的前提是
    /// 假 IP 的流量被内核路由送进 TUN。systemProxy / manual 模式下 Polaris **不装内核 tun、
    /// 不动 OS 路由**，应用经 CONNECT 把目的地交给本地核，外来隧道的路由与之不争 ——
    /// 在那两个模式下报冲突就是逢隧道必报的另一种形态。与 `spawn_system_proxy_residual_warning`
    /// 的 `mode.is_tun()` 同口径。
    ///
    /// **非 TUN 那支必须写回 [`TunnelConflictSnapshot::NotProbed`] 而不是直接 return**：
    /// 否则用户从 TUN 切到 systemProxy 之后，界面上还挂着上一次 TUN 运行时的冲突结论 ——
    /// 而那份结论描述的配置已经不在跑了。每次起核都重新表态，快照才恒等于「这一次」。
    pub(super) fn spawn_foreign_tunnel_conflict_probe(
        self: &Arc<Self>,
        mode: ProxyModeType,
        my_generation: u64,
        criteria: ConflictCriteria,
        own_interfaces: Vec<String>,
    ) {
        if !mode.is_tun() {
            self.store_tunnel_conflict_snapshot(TunnelConflictSnapshot::NotProbed);
            return;
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            runtime
                .probe_foreign_tunnel_conflicts(Some(my_generation), criteria, own_interfaces)
                .await;
            log::info!(
                "后台外来隧道冲突探测耗时={}ms（不阻塞起核）",
                started.elapsed().as_millis()
            );
        });
    }
}

#[cfg(test)]
mod tests;
