//! 测速类 command（上游 `speed-test-handlers.ts`）。
//!
//! 映射 channel：
//! - `server:speedTest` → [`server_speed_test`]
//!
//! # 两条测速路径（按运行核是否已注入主核探测池分流）
//!
//! **① 主核 K 槽探针池分波测速（池就绪 → 「批量比较多节点延迟选优」核心路径）**：起核时
//! [`ProxyRuntime`] 分配 K 个空闲口注入 `probe_pool_ports`，config-engine
//! 据此在主核 config 建 K 个 `probe-in-k`（http 入站）+ `probe-selector-k`（成员=全量 nodeTags）+
//! `probe-in-k→probe-selector-k` 路由 + `dns-probe-exit-k`。测速时把请求的 N 个节点按 K 分波（见纯逻辑
//! [`plan_waves`]），每波经 gRPC `select_outbound`（[`ProxyRuntime::probe_select_slot`]）把各槽 `probe-selector-k`
//! 热切到本波节点，再经 `probe-in-k` 端口量 warm-TTFB（同核单会话，结构性消除 WG/WARP 双会话超时）。
//! 波间串行、波内并发。对齐 上游 `SpeedTestService.testServersViaMainCore`（§15）。
//!
//! **② 回退：仅当前活跃出口（池未注入时）**：探测池端口分配失败（极少见）→ `probe_pool_ports` 空 → 主核无池。
//! 此时只能经本机混合端口（`mixed-in`，CONNECT 隧道见 [`measure_via_local_proxy`]）测【当前选中出站】
//! ——主混合代理只经当前出口出网。
//! 其余请求节点无从测（需池），如实进 `notInPool`，绝不伪造数值（裁定纯逻辑见 [`plan_speed_test`]）。
//!
//! **③ 临时核（代理**关**时；对齐 上游 `testServersViaProxy`，`SpeedTestService.ts:388-620`）**：主核未运行 →
//! 起一个**瞬态** sing-box（每个可测节点一个 HTTP 入站 → 该节点出站），经各自端口量 warm-TTFB，测完即杀。
//! 「先测速比较延迟、再选最快的连上去」是常规使用序 —— 没有这条腿，用户必须先盲选一个节点连上才能测别的。
//! 编排/隔离/让位在 [`crate::runtime::speedtest`]（独立配置文件 + 独立端口 + 不写主核任何生命周期槽；
//! **主核一起来立刻让路**）；本层只做取材、装配与信封折叠，见 [`run_temp_core_speed_test`]。
//! 临时核结构性测不了 Tailscale 节点（建不出第二个 tsnet 实例 + 会与主核抢同一份 `tailscale-state`）→
//! 如实进 `tsNotReady`。真延迟数值走真核真出站 = **真机门**，本机零验证。
//!
//! # 「测不了」必须有出口信号（反伪造 + 反卡死）
//!
//! 前端 `NodesScreen` 设 `testing=true` 后靠 `event:speedTestProgress`（`tested>=total && total>0`）复位；成功信封 +
//! 零事件 ⇒ 测速按钮**永久 disabled 到组件重挂载**。故「零可测」一律走**失败信封**（`success:false` + 结构化 code）让
//! 前端 `ipc-client` throw、`NodesScreen` catch 复位 `testing`：
//! - 池路径请求节点全未入运行核池（新增未重启）→ [`CODE_NONE_IN_POOL`]；
//! - 回退路径无活跃出口 / 活跃出口不在请求集 → [`CODE_NO_ACTIVE_EXIT`] / [`CODE_PROBE_POOL_UNWIRED`]。
//!
//! 可测节点经真实进度事件复位；code 让 UI 把「本层测不了」与「测了但失败」分开呈现。
//!
//! # 诚实缺席（波前预筛：notInPool / tsNotReady）
//!
//! 「起测即知本核测不了」的节点**不 select / 不 measure / 不 report**，如实进缺席列表 —— 而不是硬测出一个
//! `-1` 假失败（或更糟：测出一个**属于别人的**真数值）。对齐 上游 `SpeedTestService.ts:674-700` 的波前
//! 预筛（**主核池路径同样筛**，非仅临时核腿）。裁定纯逻辑见 [`partition_pool`]，两条腿各守一类伪造：
//!
//! - **`notInPool`**（上游 `:680` `!probe.hasTag`）：不在运行核 `id_to_tag` 的节点（订阅新增/改址未重启
//!   入池）→ 其 tag 非 `probe-selector-k` 成员，热切必失败 → 旧行为记假 `-1`。UI 据此显「N 未纳入」。
//! - **`dirty`**（上游 `:688` `probe.isDirty`，判据 `ProxyManager.ts:3446-3450`）：节点**已编辑但未生效**
//!   —— 用户改了地址/端口/凭据/传输，运行核仍跑**起核那一刻**的旧参数。经其槽量到的是**旧参数出口**的
//!   latency，却挂在**新参数**的节点名下 ⇒ 失真数值（比缺席更有害：用户照着一个「已经不存在的配置」的
//!   延迟去选节点）。判据见 [`partition_dirty`]：`起核快照指纹存在 && 与当前指纹不等`。
//! - **`tsNotReady`**（上游 `:692` `!probe.tsNodeReady`）：协议为 `tailscale` 但 TS 尚未登录就绪的节点。
//!   此时运行核对该出口**已让位到直连**（`login_fallback`），经其槽量到的是**直连** RTT —— 记进该节点名下
//!   即失真数值；连不通则记假 `-1`。判据见 [`ts_node_ready`]。
//!
//! **回退腿（`probe_pool_ports` 为空）无 dirty 门 —— 已知残留**：该腿唯一真测的活跃出口若已编辑未生效，
//! 经混合口量到的同样是旧参数出口。补它需要「池未注入时也能读到运行核指纹快照」的公开只读面
//! （`speed_probe_targets()` 在 `pool_ports` 空时返 `None`），属 `runtime/proxy.rs` 的只读面扩张，不在本批
//! 射程。该腿本身是端口分配失败才走的降级路径（极少见），故按已知有界残留登记，不静默。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// 测速计时用 [`tokio::time::Instant`] 而非 `std::time::Instant`。
///
/// 生产期二者**逐字等价**（`test-util` 关掉时 `tokio::time::Instant::now()` 就是 `std::time::Instant::now()`），
/// 差别只在测试期：`std` 的时钟不受 `#[tokio::test(start_paused = true)]` 的假时钟影响 ⇒ 用 `std` 时
/// 「measured 量的是第一次还是第二次 GET」这条不变式**在假时钟下测出来恒为 0ms、断言恒真**（= 没门）。
/// 换成 tokio 的 Instant 后 `measured_value_is_the_second_get_alone` 才真的有牙。
use tokio::time::Instant;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use polaris_config_engine::builder::helpers::probe_pool_inbound_tag;
use polaris_config_engine::builder::level_to_string;
use polaris_config_engine::builder::outbounds::effective_proxy_bind_interface;
use polaris_config_engine::singbox::InboundUser;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::proxy_ports::control_api_port;
use polaris_config_engine::user_config::server_config::ServerConfig;
use polaris_config_engine::user_config::LogLevel;
use polaris_core_supervisor::PortExclusions;
use polaris_helper_proto::Platform;
use polaris_net_stack::subscription::server_fingerprint;
use polaris_singbox_grpc::{Endpoint, SingBoxApiClient};

use crate::events::channel::{
    EVENT_SPEED_TEST_DONE, EVENT_SPEED_TEST_PROGRESS, EVENT_SPEED_TEST_RESULT,
};
use crate::response::ApiResponse;
use crate::runtime::auto_switch::CandidateProbe;
use crate::runtime::measurement_ledger::{self, MeasurementLedger};
use crate::runtime::proxy::{
    ActionBinding, ActionRequirement, LocalHttpProxy, LocalInbound, NormalMainAction, ProxyRuntime,
    ReadyMainTicket, SpeedBindingContext, SpeedProbeTargets,
};
use crate::runtime::speedtest::{
    emit_speed_test_done, is_temp_core_superseded, plan_temp_core_with_bindings, InterruptReason,
    TempCoreDeps, TempCoreOutcome, TempCoreSession,
};
use crate::runtime::speedtest_tunnel::{
    is_acceptable_status, open_tunnel, SpeedTestTarget, TunnelError, WarmTunnel,
};
use crate::runtime::tailscale_status::TailscaleStatusEvent;
use crate::runtime::AppRuntime;

mod witness;

pub(crate) use witness::{BindingWitness, ConnRecord, NoWitness, StreamWitness, WatchedInbounds};

/// Polaris 直连哨兵（`shared/direct-selection.ts DIRECT_SERVER_ID`；对齐 `commands/server.rs` 的本地定义）。
const DIRECT_SERVER_ID: &str = "__direct__";

/// Polaris 阻断哨兵（`domain/direct-selection.ts BLOCK_SERVER_ID`）。
const BLOCK_SERVER_ID: &str = "__block__";

/// 出口 id 是否「无真实出站」——空串（未选）/ 直连 / 阻断三者皆无节点可测。
///
/// 阻断尤其不能漏：它的 proxy-selector default 是 block 出站，伴测流量会被直接丢弃 ⇒ 测出的不是
/// 慢，而是超时，会把「用户主动阻断」记成节点故障、污染延迟表并触发误判换节点。
fn has_no_real_exit(active: &str) -> bool {
    active.is_empty() || active == DIRECT_SERVER_ID || active == BLOCK_SERVER_ID
}

/// 默认测速端点：www.gstatic.com generate_204（204 空响应，连接可立即复用）。
///
/// 不用 cp.cloudflare.com（上游 issue #154）：CF-Workers / 优选IP 节点对此 CF 自家端点测速会失败。
/// 目标域名由每个被测节点的出口远程解析（不经本机），故是否任播/有无国内镜像均与测速无关。
///
/// 原在 `crates/speedtest`（照 Electron 三路径形态 1:1 建的纯逻辑层）。该 crate 的其余抽象与 Tauri 侧
/// 实际形态不匹配（详见本文件 `resolve_speed_test_url` 上方说明），全 crate 仅本常量被消费 → crate 已删，
/// 常量就近落在唯一消费者这里。
const DEFAULT_SPEED_TEST_URL: &str = "http://www.gstatic.com/generate_204";

/// **第一阶段（冷建链）预算**：CONNECT + TLS 握手 + **第一次 GET**。
///
/// # 边界为什么划在 GET1 之**后**，而不是 CONNECT 回 200 之后
///
/// 内核对 CONNECT 是**先回 200、后拨号**：`sing/protocol/http/handshake.go:89` 先写
/// `200 Connection established`，`:104` **才** `NewConnectionEx(...)` 把这条连接交给路由拨号。
/// ⇒ **「收到 200」不蕴含「节点握手已完成」**，节点握手落在**第一次 GET** 的往返里。
/// 按字面把边界划在「CONNECT 200」会让节点握手掉进第二段那 4s 里 —— 反而**更容易误杀**慢握手的
/// 可用节点。故第一段必须一路包到 GET1 返回为止（详见 [`crate::runtime::speedtest_tunnel`] 模块文档）。
///
/// # 这不是回到「两个等长计时器」那个病（**改回单一计时器前先读完本节**）
///
/// 2026-07-31 上午修掉的是 warm 8s + measured 8s ——**两段等长**，故不可达节点的耗时整整翻倍
/// （8s → 16s），而不可达节点恰恰是整轮测速耗时的封顶项。本次分段与它有两条结构性差异：
///
///  1. **第二段远小于第一段**（4s vs 6s），不是等长复制；
///  2. **第一段超时 ⇒ 立即返回 `None`，绝不发第二次**（[`measure_warm_ttfb`] 用 `?` 早退，结构保证）。
///
/// 两条合起来 ⇒ **不可达节点的耗时恒为 6s**（与合并成一个 6s 计时器**逐字相同**），10s 只发生在
/// 「隧道已建起、GET1 已回、但复用请求卡住」这种罕见异常路径上。换言之：分段**没有**放大封顶项，
/// 只是把预算从「冷热共用一份」改成「冷的给足、热的给紧」。
///
/// 陈先生 2026-07-31 裁定：首次冷建链 6s、第二次复用请求 4s、首次超时即判超时不再浪费资源。
///
/// 代价与退路同前：真实冷建链耗时落在 6s 之外的节点判 -1（这类节点即便出值也不可用）；
/// 要放宽只改这两个常量，结构由 [`measure_warm_ttfb`] 的两段 timeout 保证，单测锁死。
///
/// ⚠️ **改这两个值必须同步前端的 `SPEEDTEST_IDLE_TIMEOUT_MS`**（`ui/src/lib/speedtest-progress-toast.ts`）
/// —— 它按 `2 ×（本值 + [`SPEED_TEST_REUSE_TIMEOUT_MS`]）` 推导。该文件的
/// `speedtest-progress-toast.test.ts` 里有一条门**直接读本文件的这两行**做算术校验，失配即转红。
const SPEED_TEST_COLD_TIMEOUT_MS: u64 = 6_000;

/// **第二阶段（复用请求）预算**：GET2 —— 也就是**上报的那个 measured 值**本身。
///
/// 隧道此刻已热（CONNECT + TLS + 节点握手都在第一段花完了），这一次纯粹是在一条已建立的 socket 上
/// 走一个往返 ⇒ 健康节点普遍几十~几百 ms，4s 已是数量级的余量。给得比第一段紧，正是为了让
/// 「隧道建起来了但复用请求卡住」这种异常尽早收口，而不是再赔一份冷建链的钱。
///
/// 边界判据与「为什么不是回到单一计时器」见 [`SPEED_TEST_COLD_TIMEOUT_MS`]。
const SPEED_TEST_REUSE_TIMEOUT_MS: u64 = 4_000;

/// 口径名：CONNECT 建隧道后在同一条连接上发两次相同的 origin-form GET，报第二次从请求写出前到
/// 响应头收齐的耗时。不是吞吐，不是 ICMP 往返，也不是冷连接耗时。
const SPEED_TEST_METRIC: &str = "warm_ttfb_v1";

/// 预热请求（GET1）的返回值是否必须通过检查。
///
/// `true`：GET1 须在冷段预算内拿到可解析的响应头且为 2xx，否则判预热失败、**不发 GET2**。
/// `false`：沿用旧行为，GET1 的返回值丢弃（只有冷段超时才阻止 GET2）。
/// 回退 = 改这个常量后重新出包；两种取值各有单测。
const WARMUP_MUST_SUCCEED: bool = true;

/// 计时开始前是否先把预热响应的响应体消费完（判据与上界见 [`WarmTunnel::drain_body`]）。
///
/// `true`：排不干净即判预热失败（连接不可复用）。`false`：沿用旧行为，不消费，残余只靠隧道层的
/// `HTTP/` 锚定兜。回退方式同 [`WARMUP_MUST_SUCCEED`]；两种取值各有单测。
const WARMUP_BODY_MUST_DRAIN: bool = true;

/// 一次测量失败时所处的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailPhase {
    /// 探针槽热切。
    Select,
    /// 建隧道（CONNECT，https 目标再加 TLS）。
    Connect,
    /// 预热（GET1 及其响应体）。
    Warmup,
    /// 计时（GET2）。
    Measure,
}

impl FailPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Connect => "connect",
            Self::Warmup => "warmup",
            Self::Measure => "measure",
        }
    }
}

/// 一次测量失败的成因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailKind {
    Timeout,
    /// 传输错 / 对端过早关闭 / 响应头畸形。
    Transport,
    /// 非 2xx，附状态码。
    HttpStatus(u16),
    /// 被拒：热切 RPC 没成功，或预热后的连接不可复用。
    Rejected,
    /// 本机侧：连不上本机的探针入站口（被拒 / 句柄或临时端口耗尽）。不是节点的负面证据。
    /// 只在分波编排里被识别；对外仍报 `transport`，事件的取值集合不变。
    Local,
}

impl FailKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport | Self::Local => "transport",
            Self::HttpStatus(_) => "http_status",
            Self::Rejected => "rejected",
        }
    }
}

/// 「真的测了，没有通过」的阶段与成因。它是节点本身的负面证据；让位、取消、跨网络代次的节点
/// 不产生它，而是缺席。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MeasureFailure {
    pub(crate) phase: FailPhase,
    pub(crate) kind: FailKind,
}

impl MeasureFailure {
    pub(crate) const fn new(phase: FailPhase, kind: FailKind) -> Self {
        Self { phase, kind }
    }

    /// 隧道层的失败种类 + 调用方当时所处的阶段。
    const fn tunnel(phase: FailPhase, error: TunnelError) -> Self {
        let kind = match error {
            TunnelError::Transport => FailKind::Transport,
            TunnelError::ConnectStatus(code) => FailKind::HttpStatus(code),
            TunnelError::NotReusable => FailKind::Rejected,
            TunnelError::Local => FailKind::Local,
        };
        Self { phase, kind }
    }

    /// [`to_json`](Self::to_json) 的逆：账本入账时从盖过章的载荷读回阶段与成因。
    fn from_json(failure: &Value) -> Self {
        let phase = match failure["phase"].as_str() {
            Some("select") => FailPhase::Select,
            Some("connect") => FailPhase::Connect,
            Some("warmup") => FailPhase::Warmup,
            _ => FailPhase::Measure,
        };
        let kind = match failure["kind"].as_str() {
            Some("timeout") => FailKind::Timeout,
            Some("rejected") => FailKind::Rejected,
            Some("http_status") => FailKind::HttpStatus(
                failure["httpStatus"]
                    .as_u64()
                    .and_then(|code| u16::try_from(code).ok())
                    .unwrap_or_default(),
            ),
            _ => FailKind::Transport,
        };
        Self { phase, kind }
    }

    fn to_json(self) -> Value {
        let mut failure = json!({ "phase": self.phase.as_str(), "kind": self.kind.as_str() });
        if let FailKind::HttpStatus(code) = self.kind {
            failure["httpStatus"] = json!(code);
        }
        failure
    }
}

/// 单节点测量结果：毫秒数，或带阶段与成因的失败。失败不带任何毫秒数。
pub(crate) type Measured = Result<u32, MeasureFailure>;

/// 逐节点结果事件的载荷（三条腿共用同一个形状）。
///
/// `serverId` / `latency`（失败为 -1）是既有字段，取值不变；`status` / `failure` 与身份块里的
/// `networkEpoch` 是新增的可选字段。身份块的其余键由 [`RunEvents::stamp`] 在发布那一刻补齐。
pub(crate) fn speed_test_result_payload(
    node_id: &str,
    measured: &Measured,
    network_epoch: Option<u64>,
) -> Value {
    let mut payload = json!({
        "serverId": node_id,
        "latency": measured.map_or(-1_i64, i64::from),
        "status": if measured.is_ok() { "ok" } else { "failed" },
        "identity": { "networkEpoch": network_epoch },
    });
    if let Err(failure) = measured {
        payload["failure"] = failure.to_json();
    }
    payload
}

/// 一次测量连同它在本机一侧的源端口。源端口是读回见证在内核连接记录里认出这条连接的键；
/// 不经真实 socket 的测量（单测、临时核腿）没有它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probed {
    pub(crate) measured: Measured,
    pub(crate) source_port: Option<u16>,
}

impl From<Measured> for Probed {
    fn from(measured: Measured) -> Self {
        Self {
            measured,
            source_port: None,
        }
    }
}

/// [`speed_test_result_payload`] 加上 `binding` 块（新增的可选字段，旧消费方不读）。
pub(crate) fn result_payload_with_binding(
    node_id: &str,
    measured: &Measured,
    network_epoch: Option<u64>,
    binding: Option<&Binding>,
) -> Value {
    let mut payload = speed_test_result_payload(node_id, measured, network_epoch);
    if let Some(binding) = binding {
        payload["binding"] = binding.to_json();
    }
    payload
}

/// 一轮测速的发起来源。决定准入优先级与事件形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpeedTestOrigin {
    /// 用户手动发起（主窗 / 托盘）。
    Manual,
    /// 后台自动故障切换。
    Failover,
    /// 周期计划。发起方是 [`crate::runtime::measurement_scheduler`]。
    Schedule,
    /// 出口 IP 探测成功后的伴测。不占单飞闸。
    Companion,
}

impl SpeedTestOrigin {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Failover => "failover",
            Self::Schedule => "schedule",
            Self::Companion => "companion",
        }
    }

    /// 准入优先级：手动 > 故障切换 > 周期计划。高者取消低者，同级或更低立即得到「忙」。
    ///
    /// 把各档设成同一个数即回到先到先得。
    const fn priority(self) -> u8 {
        match self {
            Self::Manual => 2,
            Self::Failover => 1,
            Self::Schedule | Self::Companion => 0,
        }
    }
}

/// 测量路径：这次测量想回答什么问题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MeasurePath {
    /// 经探针槽或临时核的逐节点入站，只测指定节点。
    Candidate,
    /// 经 `mixed-in`，走用户的完整路由与 DNS（有 `mixed-in` 的平台上的回退腿、出口伴测）。
    System,
    /// 经 `probe-proxy-in`，钉到用户当前选中的出口、不经用户规则（没有 `mixed-in` 的平台上的
    /// 回退腿、出口伴测）。
    Selected,
}

impl MeasurePath {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::System => "system",
            Self::Selected => "selected",
        }
    }

    /// 回退腿与出口伴测的路径由它们实际用的入站决定：有没有钉死规则，标注就是哪一个。
    pub(crate) const fn of_local_inbound(inbound: LocalInbound) -> Self {
        match inbound {
            LocalInbound::Mixed => Self::System,
            LocalInbound::ProbeProxy => Self::Selected,
        }
    }
}

/// 一条测量连接在内核里实际受的路由策略。
///
/// 它由路径与承载的核派生，不单独存放：路径是 `system` 而策略不是「走用户规则」的组合因此
/// 构造不出来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RoutePolicy {
    /// 探针槽入站，钉到探针 selector。用户规则不参与。
    SlotPin,
    /// 临时核的逐节点入站，钉到节点出站。
    NodePin,
    /// `probe-proxy-in`，钉到用户当前选中的出口。用户规则不参与。
    SelectedPin,
    /// `mixed-in`，不钉，走用户的全部路由规则与 DNS 规则。
    UserRoute,
}

impl RoutePolicy {
    pub(crate) const fn of(path: MeasurePath, instance: CoreInstance) -> Self {
        match (path, instance) {
            (MeasurePath::Candidate, CoreInstance::Main { .. }) => Self::SlotPin,
            (MeasurePath::Candidate, CoreInstance::Temp) => Self::NodePin,
            (MeasurePath::Selected, _) => Self::SelectedPin,
            (MeasurePath::System, _) => Self::UserRoute,
        }
    }

    /// 本机代理入站上的测量受的策略：只看入站，与哪个核无关。
    pub(crate) const fn of_local_inbound(inbound: LocalInbound) -> Self {
        match inbound {
            LocalInbound::Mixed => Self::UserRoute,
            LocalInbound::ProbeProxy => Self::SelectedPin,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::SlotPin => "slot_pin",
            Self::NodePin => "node_pin",
            Self::SelectedPin => "selected_pin",
            Self::UserRoute => "user_route",
        }
    }
}

/// 读回的结论：内核为这条测量连接选定并用于拨号的出站，与请求的是不是同一个。
///
/// 它只到叶子出站对象为止。叶子内部的前置链、节点域名解析到的地址、实际拨到的远端、拨号成败，
/// 以及目标域名经哪条 DNS 规则解析，都读不到，不在这个结论里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingVerdict {
    /// 读到了记录，叶子出站就是请求的节点。
    Confirmed,
    /// 读到了记录，叶子出站不是请求的节点：量到的不是它。
    Mismatch,
    /// 限时内没有读到记录（流没建起来、中途断开、超过等待上限，或内核没有连接跟踪）。
    /// 保证与读回引入之前相同：换选 RPC 返回了成功。
    Unverified,
    /// 临时核：绑定由生成配置的钉死规则保证，没有运行期对账。
    Static,
    /// 没有指定节点的测量（系统路径、选中出口）：读到了实际承载者，无所谓符不符。
    Observed,
}

impl BindingVerdict {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Mismatch => "mismatch",
            Self::Unverified => "unverified",
            Self::Static => "static",
            Self::Observed => "observed",
        }
    }

    /// [`as_str`](Self::as_str) 的逆：盖章时从载荷读回。
    fn parse(verdict: &str) -> Option<Self> {
        [
            Self::Confirmed,
            Self::Mismatch,
            Self::Unverified,
            Self::Static,
            Self::Observed,
        ]
        .into_iter()
        .find(|candidate| candidate.as_str() == verdict)
    }
}

/// 读回判定（纯函数）。`requested` 是请求的节点 tag，没有指定节点的测量传 `None`；
/// `carried_by` 是连接记录里的叶子出站 tag，没读到记录传 `None`。
pub(crate) fn judge_binding(requested: Option<&str>, carried_by: Option<&str>) -> BindingVerdict {
    match (requested, carried_by) {
        (_, None) => BindingVerdict::Unverified,
        (None, Some(_)) => BindingVerdict::Observed,
        (Some(requested), Some(leaf)) if requested == leaf => BindingVerdict::Confirmed,
        (Some(_), Some(_)) => BindingVerdict::Mismatch,
    }
}

/// 请求的节点在运行核里是普通出站还是 endpoint。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Outbound,
    Endpoint,
}

/// 逐节点结果的 `binding` 块：这条测量连接受什么策略约束、请求的是谁、实际由谁承载。
/// 与身份块并列，全部字段可选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) policy: RoutePolicy,
    /// 请求的节点 tag；没有指定节点的测量为 `None`。
    pub(crate) requested_tag: Option<String>,
    pub(crate) requested_kind: Option<NodeKind>,
    /// 读回的结论。`None` = 读回关闭，块里只有上面三项。
    pub(crate) verdict: Option<BindingVerdict>,
    /// 连接记录；没读到为 `None`。
    pub(crate) record: Option<ConnRecord>,
    /// 承载出站（没读到时取请求的节点）自带的前置出站 tag。来自生成配置，不是读回。
    pub(crate) static_detour: Option<String>,
    /// 命中的规则在用户配置里的名字（起核时的映射里有才带）。
    pub(crate) rule_name: Option<String>,
}

impl Binding {
    /// 临时核腿：没有运行期读回，绑定由配置结构保证。`detour` 是该节点在临时核配置里的前置出站。
    pub(crate) fn static_node_pin(tag: &str, is_endpoint: bool, detour: Option<&str>) -> Self {
        Self {
            static_detour: detour.map(str::to_string),
            rule_name: None,
            policy: RoutePolicy::NodePin,
            requested_tag: Some(tag.to_string()),
            requested_kind: Some(if is_endpoint {
                NodeKind::Endpoint
            } else {
                NodeKind::Outbound
            }),
            verdict: Some(BindingVerdict::Static),
            record: None,
        }
    }

    /// 叶子出站 tag（读到了记录才有）。
    fn carried_by(&self) -> Option<&str> {
        self.record.as_ref().map(|record| record.outbound.as_str())
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut block = json!({ "policy": self.policy.as_str() });
        if let Some(tag) = &self.requested_tag {
            block["requestedTag"] = json!(tag);
        }
        if let Some(kind) = self.requested_kind {
            block["requestedKind"] = json!(match kind {
                NodeKind::Outbound => "outbound",
                NodeKind::Endpoint => "endpoint",
            });
        }
        let Some(verdict) = self.verdict else {
            return block;
        };
        if let Some(detour) = &self.static_detour {
            block["staticDetour"] = json!(detour);
        }
        block["verdict"] = json!(verdict.as_str());
        block["source"] = json!(match (verdict, &self.record) {
            (BindingVerdict::Static, _) => "config",
            (_, Some(_)) => "connection",
            (_, None) => "none",
        });
        if let Some(record) = &self.record {
            block["carriedBy"] = json!({ "tag": record.outbound, "type": record.outbound_type });
            block["chain"] = json!(record.chain);
            block["rule"] = json!(record.rule);
            if let Some(name) = &self.rule_name {
                block["ruleName"] = json!(name);
            }
            // 目的地址与嗅探域名是测速 URL 的主机名，自配 URL 可能带令牌：事件里只出摘要。
            block["destinationDigest"] =
                json!(polaris_updater::sha256_hex(record.destination.as_bytes()));
            block["domainDigest"] = json!(polaris_updater::sha256_hex(record.domain.as_bytes()));
        }
        block
    }
}

/// 读回的档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadbackMode {
    /// 不建流，`binding` 块只出策略与请求的节点。
    Off,
    /// 建流、出完整的 `binding` 块；读回不符只记日志，结果照常发布。
    Observe,
    /// 读回不符的节点本次记为未测（缺席），不出数值、不记 -1。
    Enforce,
}

/// 生效的读回档位。回退 = 改这个常量后重新出包；三档各有单测。
///
/// 首发取「只观测」：读回通道在移动端的可用性与事件时延都还没有真机数据。
///
/// 「强制」档下读回不符的节点是「没测」不是「没通」：各腿都按缺席处理，故障切换腿把它报成
/// [`CandidateProbe::MismatchSkipped`]，不进失败分支。
const BINDING_READBACK: ReadbackMode = ReadbackMode::Observe;

/// 走用户规则的测量（`system` 路径）的结果记在谁名下。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemAttribution {
    /// 沿用旧行为：一律记在当前选中节点名下。
    SelectedNode,
    /// 读到了叶子出站而它不是选中节点时，结果归属「系统路径」本身，不写任何节点的延迟。
    /// 没读到（流没建起来、限时内没到、流已断）时按旧行为记在选中节点名下：读回通道出故障
    /// 不该让这条测量失效。
    LeafMatch,
}

/// 生效的系统路径归属口径。回退 = 改这个常量后重新出包；两种取值各有单测。
/// 读回关闭时读不到叶子出站，按旧行为处理。
const SYSTEM_PATH_ATTRIBUTION: SystemAttribution = SystemAttribution::LeafMatch;

/// 「系统路径」这个对象在结果里的 id。归属它的结果只入账、不向外发事件：既有界面按 `serverId`
/// 写节点延迟，而它不是节点。
pub(crate) const SYSTEM_PATH_ID: &str = "__system_path__";

/// 走本机代理入站的一次测量，结果该不该记在选中节点名下（纯函数）。
///
/// 钉到选中出口的入站量到的就是选中出口，恒记在它名下。走用户规则的入站可能被规则分流到别处，
/// 所以要看读回的叶子出站：读到了且不是选中节点才不记；没读到时照旧记（`binding` 块标
/// `unverified`）。选中节点不在起核快照里（没有 tag）而读到了叶子，算读到且不等。
pub(crate) fn system_result_belongs_to_node(
    policy: RoutePolicy,
    mode: ReadbackMode,
    attribution: SystemAttribution,
    carried_by: Option<&str>,
    selected_tag: Option<&str>,
) -> bool {
    policy != RoutePolicy::UserRoute
        || mode == ReadbackMode::Off
        || attribution == SystemAttribution::SelectedNode
        || carried_by.is_none_or(|leaf| Some(leaf) == selected_tag)
}

/// 一轮运行的读回上下文：见证、档位、节点归类与计数。克隆共享同一份。
#[derive(Clone)]
pub(crate) struct Readback {
    witness: Arc<dyn BindingWitness>,
    mode: ReadbackMode,
    /// 起核产物里的静态取材：哪些 tag 是 endpoint、各 tag 的前置出站、规则名映射。
    context: Arc<SpeedBindingContext>,
    /// 各结论的次数，按 [`BindingVerdict`] 的声明顺序（不含 `static`）。
    tally: Arc<[AtomicU64; 4]>,
}

impl Readback {
    pub(crate) fn new(
        mode: ReadbackMode,
        witness: Arc<dyn BindingWitness>,
        context: SpeedBindingContext,
    ) -> Self {
        Self {
            witness,
            mode,
            context: Arc::new(context),
            tally: Arc::default(),
        }
    }

    /// 不做读回。
    #[cfg(test)]
    pub(crate) fn off() -> Self {
        Self::new(
            ReadbackMode::Off,
            Arc::new(NoWitness),
            SpeedBindingContext::default(),
        )
    }

    /// 生产入口：按档位在给定的管理端点上开一条见证流。建流在后台进行，不阻塞准入；
    /// 建不起来不拒绝本轮，本轮结果的承载出站一律记为未验证。
    ///
    /// `watched` 是本腿的测量连接会经过的入站：别的入站上的连接不进对账。
    fn open(
        mode: ReadbackMode,
        endpoint: Option<(u16, String)>,
        context: SpeedBindingContext,
        watched: WatchedInbounds,
    ) -> Self {
        if mode == ReadbackMode::Off {
            return Self::new(mode, Arc::new(NoWitness), context);
        }
        let witness = StreamWitness::spawn(async move {
            let (port, secret) = endpoint.ok_or_else(|| "管理端点未就绪".to_string())?;
            SingBoxApiClient::connection_events_at(
                Endpoint::new("127.0.0.1", port),
                secret,
                witness::CONNECTION_EVENTS_INTERVAL_NS,
            )
            .await
            .map(|frames| witness::record_stream(frames, watched))
            .map_err(|error| error.to_string())
        });
        Self::new(mode, Arc::new(witness), context)
    }

    fn kind_of(&self, tag: &str) -> NodeKind {
        if self.context.endpoint_tags.contains(tag) {
            NodeKind::Endpoint
        } else {
            NodeKind::Outbound
        }
    }

    /// 等见证流建立（限时）。只有一次测量的腿在起测前调：流晚于测量建立时，这条测量连接只能靠
    /// 首帧里的存量补上。
    async fn ready(&self) {
        if self.mode != ReadbackMode::Off {
            self.witness.ready().await;
        }
    }

    /// 登记一条刚连上 `inbound` 入站口的测量连接（见 [`BindingWitness::expect`]）。
    fn expect(&self, inbound: &str, source_port: u16) {
        if self.mode != ReadbackMode::Off {
            self.witness.expect(inbound, source_port);
        }
    }

    /// 取一条测量连接的记录。没有源端口（连接没建起来）即没有记录可查。
    async fn lookup(&self, inbound: &str, source_port: Option<u16>) -> Option<ConnRecord> {
        match source_port {
            Some(port) if self.mode != ReadbackMode::Off => {
                self.witness.lookup(inbound, port).await
            }
            _ => None,
        }
    }

    /// 给一次测量出 `binding` 块并计数。`requested` 是请求的节点 tag；没有指定节点的测量传 `None`。
    fn bind(
        &self,
        policy: RoutePolicy,
        requested: Option<&str>,
        record: Option<ConnRecord>,
    ) -> Binding {
        let verdict = (self.mode != ReadbackMode::Off).then(|| {
            judge_binding(
                requested,
                record.as_ref().map(|record| record.outbound.as_str()),
            )
        });
        let slot = match verdict {
            Some(BindingVerdict::Confirmed) => Some(0),
            Some(BindingVerdict::Mismatch) => Some(1),
            Some(BindingVerdict::Unverified) => Some(2),
            Some(BindingVerdict::Observed) => Some(3),
            Some(BindingVerdict::Static) | None => None,
        };
        if let Some(slot) = slot {
            self.tally[slot].fetch_add(1, Ordering::AcqRel);
        }
        // 前置出站是承载者自己的配置：读到了取叶子的，没读到取请求节点的。
        let carrier = record
            .as_ref()
            .map(|record| record.outbound.as_str())
            .or(requested);
        Binding {
            policy,
            requested_tag: requested.map(str::to_string),
            requested_kind: requested.map(|tag| self.kind_of(tag)),
            verdict,
            static_detour: carrier.and_then(|tag| self.context.static_detours.get(tag).cloned()),
            rule_name: record
                .as_ref()
                .and_then(|record| self.context.rule_names.get(&record.rule).cloned()),
            record,
        }
    }

    /// 关闭见证流并等它结束。终态事件必须在这之后发。
    async fn close(&self) {
        self.witness.close().await;
    }

    /// 汇总行里的读回一段；读回关闭时为空。
    fn summary(&self) -> String {
        if self.mode == ReadbackMode::Off {
            return String::new();
        }
        let count = |slot: usize| self.tally[slot].load(Ordering::Acquire);
        format!(
            " 读回 confirmed {} mismatch {} unverified {} observed {} 流 {} 最长等待 {}ms",
            count(0),
            count(1),
            count(2),
            count(3),
            self.witness.state().as_str(),
            self.witness.max_wait_ms(),
        )
    }
}

/// 承载这次测量的核。`Main` 即「已连接」，`Temp` 即「未连接测量」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoreInstance {
    /// 运行中的主核：世代与起核就绪时刻。
    Main {
        generation: u64,
        start_time: Option<u64>,
    },
    /// 停止态临时核：只标种类，不带世代。
    Temp,
}

/// 一轮测速的请求。前端的手动测速只传节点 id 列表，其余字段由后端在准入时定下，整轮固定。
#[derive(Debug, Clone)]
pub(crate) struct SpeedTestRequest {
    pub(crate) origin: SpeedTestOrigin,
    /// 有序节点 id（手动测速缺省时已展开成确定的列表）。
    pub(crate) targets: Vec<String>,
    /// 调用方自带的不透明标签，原样回显在结果里，本层不解释。
    pub(crate) scope: Option<String>,
    pub(crate) path: MeasurePath,
    pub(crate) url: String,
}

/// 逐节点结果的身份块。与既有的 `measurementContext` 并存，不替换它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResultIdentity {
    /// 运行号（进程内单调，与事件顶层的 `runId` 同一序列）。
    pub(crate) run: u64,
    /// 本轮内的事件序号（结果 / 进度 / 终态共用一个计数）。
    pub(crate) seq: u64,
    pub(crate) origin: SpeedTestOrigin,
    pub(crate) scope: Option<String>,
    pub(crate) path: MeasurePath,
    /// 测速 URL 的 sha256。事件与日志只出摘要：自配 URL 可能带凭据或令牌。
    pub(crate) url_digest: String,
    pub(crate) instance: CoreInstance,
    /// 主核：起核时捕获的已发射配置摘要。临时核：空。
    pub(crate) config_digest: Option<String>,
    /// 该节点测量时所依据的参数指纹。
    pub(crate) node_fingerprint: Option<String>,
    /// 网络代次。`None` 是「未知」，不得解释为「未变」。
    pub(crate) network_epoch: Option<u64>,
    /// 后端出结果那一刻的 Unix 毫秒。
    pub(crate) measured_at: u64,
    /// 读回的结论（取自结果的 `binding` 块）；没有读回为 `None`。不进身份块的 JSON。
    pub(crate) binding: Option<BindingVerdict>,
}

impl ResultIdentity {
    fn to_json(&self) -> Value {
        let (context, instance) = match self.instance {
            CoreInstance::Main {
                generation,
                start_time,
            } => (
                "connected",
                json!({ "kind": "main", "generation": generation, "startTime": start_time }),
            ),
            CoreInstance::Temp => ("disconnected", json!({ "kind": "temp" })),
        };
        json!({
            // 运行号编码成十进制字符串，理由同顶层 `runId`（JS 整数精度）。
            "run": self.run.to_string(),
            "seq": self.seq,
            "origin": self.origin.as_str(),
            "scope": self.scope,
            "path": self.path.as_str(),
            "metric": SPEED_TEST_METRIC,
            "urlDigest": self.url_digest,
            "context": context,
            "instance": instance,
            "configDigest": self.config_digest,
            "nodeFingerprint": self.node_fingerprint,
            "networkEpoch": self.network_epoch,
            "measuredAt": self.measured_at,
        })
    }
}

/// 两刻的网络代次是否**确知**不同。任一方未知时不能据此下结论。
const fn network_epoch_changed(before: Option<u64>, after: Option<u64>) -> bool {
    matches!((before, after), (Some(a), Some(b)) if a != b)
}

/// 一轮运行的事件发布口：给三类事件盖序号，给逐节点结果补齐身份块，并守住流的形态。
///
/// - 序号在一轮内严格递增，终态带最后一个；终态之后的事件一律丢弃。
/// - **只有手动运行**才在事件顶层带 `runId`、才发进度与终态。其余来源只发逐节点结果，运行号放在
///   身份块里：既有消费方把「顶层有 `runId`」当作「这是一轮前台任务」，会为它建进度条、并挡住下一次
///   手动测速。
pub(crate) struct RunEvents {
    run_id: String,
    /// 既有的 `measurementContext`（只有持票据的运行才有）。
    context: Option<Value>,
    /// 运行级身份；逐节点的三个字段在盖章时填。
    identity: ResultIdentity,
    fingerprints: BTreeMap<String, String>,
    requested: usize,
    skipped: Vec<(&'static str, usize)>,
    /// 本轮的汇总行。终态到达时生成并写进日志；有值即「已收尾」，此后的事件一律丢弃。
    summary: Option<String>,
    started: Instant,
    ok: usize,
    failures: BTreeMap<String, usize>,
    /// 失败节点的前几个 id（汇总行的样本）。
    failed_samples: Vec<String>,
    /// 逐节点结果盖章后入的那本账。
    ledger: &'static MeasurementLedger,
    /// 本轮的读回上下文（汇总行取它的计数与流状态）。
    readback: Option<Readback>,
    /// 绑定由配置结构保证的结果条数（临时核腿）。
    static_bound: usize,
    /// 归属「系统路径」的结果条数。它们不是节点的结果，不进成功与失败数。
    system_path: usize,
}

impl RunEvents {
    pub(crate) fn new(
        run_id: &str,
        request: &SpeedTestRequest,
        instance: CoreInstance,
        config_digest: Option<String>,
        fingerprints: BTreeMap<String, String>,
        context: Option<Value>,
    ) -> Self {
        Self {
            run_id: run_id.to_string(),
            context,
            identity: ResultIdentity {
                // 运行号由 `next_speed_test_run_id` 发出，恒为十进制数字串。
                run: run_id.parse().unwrap_or_default(),
                seq: 0,
                origin: request.origin,
                scope: request.scope.clone(),
                path: request.path,
                url_digest: polaris_updater::sha256_hex(request.url.as_bytes()),
                instance,
                config_digest,
                node_fingerprint: None,
                network_epoch: None,
                measured_at: 0,
                binding: None,
            },
            fingerprints,
            requested: request.targets.len(),
            skipped: Vec::new(),
            summary: None,
            started: Instant::now(),
            ok: 0,
            failures: BTreeMap::new(),
            failed_samples: Vec::new(),
            ledger: measurement_ledger::global(),
            readback: None,
            static_bound: 0,
            system_path: 0,
        }
    }

    /// 接上本轮的读回上下文。
    fn with_readback(mut self, readback: &Readback) -> Self {
        self.readback = Some(readback.clone());
        self
    }

    /// 换一本账（周期一轮由调度器传入它读的那一本；测试传各自独立的）。
    pub(crate) fn with_ledger(mut self, ledger: &'static MeasurementLedger) -> Self {
        self.ledger = ledger;
        self
    }

    /// 登记起测前即知不该测的节点数（按原因），只进汇总日志。
    fn with_skipped(mut self, skipped: Vec<(&'static str, usize)>) -> Self {
        self.skipped = skipped;
        self
    }

    /// 给一条事件盖章。`None` = 这条不该发（终态之后，或非手动来源的进度 / 终态）。
    pub(crate) fn stamp(&mut self, event: &str, mut payload: Value) -> Option<Value> {
        if self.summary.is_some() {
            return None;
        }
        // 收尾对任何来源都做：汇总照出，此后不再放行事件。向外发不发才看来源。
        if event == EVENT_SPEED_TEST_DONE {
            let line = self.summary_line(&payload);
            log::info!("{line}");
            self.summary = Some(line);
        }
        let manual = self.identity.origin == SpeedTestOrigin::Manual;
        if !manual && event != EVENT_SPEED_TEST_RESULT {
            return None;
        }
        self.identity.seq += 1;
        if event == EVENT_SPEED_TEST_RESULT {
            let mut identity = self.identity.clone();
            identity.node_fingerprint = payload["serverId"]
                .as_str()
                .and_then(|id| self.fingerprints.get(id))
                .cloned();
            identity.network_epoch = payload["identity"]["networkEpoch"].as_u64();
            identity.binding = payload["binding"]["verdict"]
                .as_str()
                .and_then(BindingVerdict::parse);
            if identity.binding == Some(BindingVerdict::Static) {
                self.static_bound += 1;
            }
            identity.measured_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            let system_path = payload["serverId"] == SYSTEM_PATH_ID;
            if system_path {
                self.system_path += 1;
            } else if payload["failure"].is_object() {
                let label = format!(
                    "{}/{}",
                    payload["failure"]["phase"].as_str().unwrap_or_default(),
                    payload["failure"]["kind"].as_str().unwrap_or_default()
                );
                *self.failures.entry(label).or_insert(0) += 1;
                if self.failed_samples.len() < SUMMARY_FAILED_SAMPLES {
                    self.failed_samples
                        .extend(payload["serverId"].as_str().map(str::to_string));
                }
            } else {
                self.ok += 1;
            }
            // 入账旁路：盖章之后、出口之前。出口是空闭包的来源（故障切换）也因此不会漏。
            if let Some(node_id) = payload["serverId"].as_str() {
                let measured = match payload["latency"].as_u64() {
                    Some(ms) if !payload["failure"].is_object() => {
                        Ok(u32::try_from(ms).unwrap_or(u32::MAX))
                    }
                    _ => Err(MeasureFailure::from_json(&payload["failure"])),
                };
                self.ledger.record(node_id, measured, identity.clone());
            }
            payload["identity"] = identity.to_json();
            // 归属「系统路径」的结果到此为止：已入账，不向外发（见 [`SYSTEM_PATH_ID`]）。
            if system_path {
                return None;
            }
        } else {
            payload["seq"] = json!(self.identity.seq);
        }
        Some(if manual {
            speed_test_measurement_payload(payload, &self.run_id, self.context.as_ref())
        } else {
            payload
        })
    }

    /// 把盖章接到一个事件出口前面（生产接 `AppHandle::emit`，测试接收集器）。
    pub(crate) fn sink<'a>(
        &'a mut self,
        mut out: impl FnMut(&str, Value) + Send + 'a,
    ) -> impl FnMut(&str, Value) + Send + 'a {
        move |event, payload| {
            if let Some(payload) = self.stamp(event, payload) {
                out(event, payload);
            }
        }
    }

    /// 每轮**一行**汇总，三条腿与后台探测共用这一个出口。
    ///
    /// `-1` 与「缺席」是两件事，这一行把它们分开：成功、失败（真测了没通，按「阶段/成因」分布，
    /// 预热失败的占比可直接读出）、未测（让位 / 中断，成因在 `reason`）、起测前跳过（按原因计数）。
    /// 失败样本只带前几个 id：全量在上百节点时是一行几 KB，而排查只需要「是不是集中在某一类」。
    fn summary_line(&self, done: &Value) -> String {
        let context = match self.identity.instance {
            CoreInstance::Main { .. } => "connected",
            CoreInstance::Temp => "disconnected",
        };
        let tail = if self.failed_samples.is_empty() {
            String::new()
        } else {
            format!("；失败样本 {}", self.failed_samples.join(", "))
        };
        let mut readback = self
            .readback
            .as_ref()
            .map(Readback::summary)
            .unwrap_or_default();
        if self.static_bound > 0 {
            readback.push_str(&format!(" 读回 static {}", self.static_bound));
        }
        if self.system_path > 0 {
            readback.push_str(&format!(" 系统路径 {}", self.system_path));
        }
        format!(
            "测速一轮完成：run={} origin={} context={context} outcome={} reason={} 请求 {} 可测 {} 成功 {} 失败 {} {:?} 起测前跳过 {:?} 未测 {} 耗时 {}ms{readback}{tail}",
            self.run_id,
            self.identity.origin.as_str(),
            done["outcome"].as_str().unwrap_or("-"),
            done["reason"].as_str().unwrap_or("-"),
            self.requested,
            done["total"],
            self.ok,
            self.failures.values().sum::<usize>(),
            self.failures,
            self.skipped,
            done["pending"].as_array().map_or(0, Vec::len),
            self.started.elapsed().as_millis(),
        )
    }
}

/// 汇总行里失败样本的条数上限。
const SUMMARY_FAILED_SAMPLES: usize = 5;

/// 一轮运行的取消句柄。准入时随单飞闸一起发出，克隆共享同一个状态。
///
/// 取消与让位并入同一组检查点，处置也相同：整轮记为中断，未出值的节点缺席，绝不写 -1。
/// 本层只提供句柄，没有用户可见的取消入口。
#[derive(Clone, Default)]
pub(crate) struct SpeedTestCancel(Arc<CancelState>);

#[derive(Default)]
struct CancelState {
    /// 0 = 未取消；1 = 被取消；2 = 被更高优先级抢占。
    reason: AtomicU8,
    notify: Notify,
}

impl SpeedTestCancel {
    /// 请求取消。只有第一次生效（成因不被后来者改写）。
    pub(crate) fn cancel(&self, reason: InterruptReason) {
        let code = if reason == InterruptReason::Preempted {
            2
        } else {
            1
        };
        if self
            .0
            .reason
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.0.notify.notify_waiters();
        }
    }

    /// 已请求取消时返回成因。
    pub(crate) fn reason(&self) -> Option<InterruptReason> {
        match self.0.reason.load(Ordering::Acquire) {
            0 => None,
            2 => Some(InterruptReason::Preempted),
            _ => Some(InterruptReason::Cancelled),
        }
    }

    /// 等到被取消为止。供测量循环与在飞任务竞速，使取消不必等某个节点自己测完。
    pub(crate) async fn cancelled(&self) -> InterruptReason {
        loop {
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            // 先登记再看状态：反过来会漏掉两步之间到达的那次通知。
            notified.as_mut().enable();
            if let Some(reason) = self.reason() {
                return reason;
            }
            notified.await;
        }
    }
}

/// 结构化错误码：无活跃出口（直连 / 未选节点）→ 主混合代理没有真实出站可测。
const CODE_NO_ACTIVE_EXIT: &str = "SPEEDTEST_NO_ACTIVE_EXIT";
/// 结构化错误码（**回退路径**）：探测池未注入（分配失败）且请求集不含活跃出口 → 本层零可测。
const CODE_PROBE_POOL_UNWIRED: &str = "SPEEDTEST_PROBE_POOL_UNWIRED";
/// 结构化错误码（**池路径**）：请求节点全未纳入运行核测速池（订阅新增/改址未重启入池）→ 本波零可测。
const CODE_NONE_IN_POOL: &str = "SPEEDTEST_NONE_IN_POOL";
/// 结构化错误码（**池路径**）：请求节点全部**已编辑未生效**（运行核仍跑旧参数）→ 本波零可测。
///
/// 与 [`CODE_NONE_IN_POOL`] 分开的理由同 [`CODE_TS_NOT_READY`]：用户的下一步不同 —— 未入池要「刷新订阅 /
/// 重启核纳入」，已编辑未生效要「应用更改」（Home 待应用操作条那一下）。合成一个码会把用户指向错误的修法。
/// 渲染端 `speedtest-feedback.ts` 对未知 code 走 `default` 分支直显本层文案，故新码零 UI 改动即可用。
const CODE_ALL_DIRTY: &str = "SPEEDTEST_ALL_DIRTY";
/// 结构化错误码：本波唯一可测的（或全部请求的）节点是 **TS 未登录就绪**的 tailscale 节点 → 零可测。
///
/// 与 [`CODE_NONE_IN_POOL`] 分开：两者对用户的下一步动作不同 —— 未入池要「重启内核」，TS 未就绪要
/// 「去把该节点登录上」。合成一个码会把用户指向错误的修法。
const CODE_TS_NOT_READY: &str = "SPEEDTEST_TS_NOT_READY";
/// 结构化错误码：已有测速在飞（单飞闸拒并发）。前端 catch 后复位自身 testing 灰态，视作 no-op。
const CODE_IN_FLIGHT: &str = "SPEEDTEST_IN_FLIGHT";
/// 结构化错误码：主核**正在启动**（`ProxyStatus::starting`）→ 临时核腿视作被占用，本轮不测。
///
/// 与 [`CODE_IN_FLIGHT`] 分开：那是「别人在测速」（等几秒重试即可），这是「核在起」（等连接完成后
/// 走主核测速池，路径都不同）。渲染端对未知 code 走 `default` 直显本层文案，故新码零 UI 改动即可用。
const CODE_CORE_STARTING: &str = "SPEEDTEST_CORE_STARTING";

/// 测速进程级单飞闸（审查 MED「前后端均无 busy/single-flight」的后端半）。
///
/// 托盘浮层与主窗（首页 / 节点页）是**独立 JS 堆**，各自的「测速中」灰态只锁本窗按钮，拦不住跨窗口
/// 并发（两窗同时点 = 两条 `server_speed_test` 并发跑主混合代理测量，互相污染 warm/measured 计时）。
/// 此处以进程级的闸收口所有入口：同一时刻至多一轮运行占用探针池或临时核。
/// 对齐 上游 主进程 `TrayManager.isSpeedTesting` + 单编排 `runSpeedTest` 的去重语义。
///
/// 闸里记着持有者的来源与取消句柄：更高优先级的请求到来时取消持有者（成因「被抢占」），等它收口
/// 释放后再占用；同级或更低的请求立即被拒（不 emit 任何事件）。优先级见
/// [`SpeedTestOrigin::priority`]。
pub(crate) struct SpeedTestGate {
    holder: Mutex<Option<(SpeedTestOrigin, SpeedTestCancel)>>,
    released: Notify,
}

static SPEED_TEST_GATE: SpeedTestGate = SpeedTestGate::new();

impl SpeedTestGate {
    const fn new() -> Self {
        Self {
            holder: Mutex::new(None),
            released: Notify::const_new(),
        }
    }

    /// 抢占单飞闸：闸空 → 占用；被更低优先级占着 → 取消它并等它释放；否则 → `None`（忙）。
    async fn acquire(&self, origin: SpeedTestOrigin) -> Option<SpeedTestGuard<'_>> {
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            // 先登记再看状态：反过来会漏掉两步之间的那次释放，永远等下去。
            released.as_mut().enable();
            {
                let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
                match holder.as_ref() {
                    None => {
                        let cancel = SpeedTestCancel::default();
                        *holder = Some((origin, cancel.clone()));
                        return Some(SpeedTestGuard { gate: self, cancel });
                    }
                    Some((held, cancel)) if origin.priority() > held.priority() => {
                        log::info!("测速被抢占：{} 让位给 {}", held.as_str(), origin.as_str());
                        cancel.cancel(InterruptReason::Preempted);
                    }
                    Some((held, _)) => {
                        log::info!(
                            "测速准入被拒：来源 {}，已有 {} 测速进行中",
                            origin.as_str(),
                            held.as_str()
                        );
                        return None;
                    }
                }
            }
            released.await;
        }
    }

    /// 此刻持闸的来源。只用于在被拒之后说明「被谁占着」：与随后的释放之间没有同步。
    fn holder(&self) -> Option<SpeedTestOrigin> {
        self.holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|(origin, _)| *origin)
    }

    /// 等到闸空出来为止。不占闸，也不取消持有者。
    async fn idle(&self) {
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            // 先登记再看状态，理由同 [`acquire`](Self::acquire)。
            released.as_mut().enable();
            if self.holder().is_none() {
                return;
            }
            released.await;
        }
    }

    async fn data_settled(&self, ledger: &measurement_ledger::MeasurementLedger, seen: u64) {
        ledger.changed_since(seen).await;
        self.idle().await;
    }
}

/// 单飞闸上此刻有没有一轮在飞、是谁发起的（只读）。自动选点据此不拿一轮中途的半份数据选点。
pub(crate) fn speed_test_in_flight() -> Option<SpeedTestOrigin> {
    SPEED_TEST_GATE.holder()
}

/// Wait for new results and for the actual producer to release its single-flight gate.
pub(crate) async fn speed_test_data_settled(seen: u64) {
    SPEED_TEST_GATE
        .data_settled(measurement_ledger::global(), seen)
        .await;
}

/// RAII 单飞守卫：`acquire` 抢占，`drop` 释放——覆盖 early return / `await` 取消 / panic 展开，
/// 绝不把闸永久卡死（那会让测速功能整段熄火直到重启）。
pub(crate) struct SpeedTestGuard<'a> {
    gate: &'a SpeedTestGate,
    cancel: SpeedTestCancel,
}

impl SpeedTestGuard<'static> {
    /// 抢占进程级单飞闸（语义见 [`SpeedTestGate::acquire`]）。
    async fn acquire(origin: SpeedTestOrigin) -> Option<Self> {
        SPEED_TEST_GATE.acquire(origin).await
    }
}

impl SpeedTestGuard<'_> {
    /// 本轮运行的取消句柄。
    const fn cancel(&self) -> &SpeedTestCancel {
        &self.cancel
    }
}

impl Drop for SpeedTestGuard<'_> {
    fn drop(&mut self) {
        *self
            .gate
            .holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.gate.released.notify_waiters();
    }
}

/// 手动运行的事件出口：终态先扣在 `held` 里不发，其余事件照发。
///
/// 终态要等单飞闸释放之后才发（见 [`release_then_emit`]），而闸的守卫在命令函数手里、不在腿里，
/// 所以腿只负责把终态交出去。
fn hold_terminal<'a>(
    app: &'a AppHandle,
    held: &'a mut Option<Value>,
) -> impl FnMut(&str, Value) + Send + 'a {
    move |event, payload| {
        if event == EVENT_SPEED_TEST_DONE {
            *held = Some(payload);
        } else {
            let _ = app.emit(event, payload);
        }
    }
}

/// **先释放单飞闸，再发终态**。次序反过来，收到终态就立刻再发起测速的调用方会撞上「已有测速
/// 进行中」。走到这里时在飞任务已全部收回、临时核已关，闸后面没有别的东西要保护。
fn release_then_emit(guard: SpeedTestGuard<'_>, done: Option<Value>, emit: impl FnOnce(Value)) {
    drop(guard);
    if let Some(done) = done {
        emit(done);
    }
}

/// Process-local monotonic identity, encoded as a decimal string to avoid JS integer precision loss.
/// Issued only after the command acquires the single-flight guard; never persisted across process restart.
static SPEED_TEST_RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_speed_test_run_id(sequence: &AtomicU64) -> Option<String> {
    sequence
        .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .ok()
        .map(|previous| (previous + 1).to_string())
}

fn speed_test_run_payload(mut payload: Value, run_id: &str) -> Value {
    if let Some(object) = payload.as_object_mut() {
        object.insert("runId".to_string(), Value::String(run_id.to_string()));
    }
    payload
}

fn speed_test_measurement_payload(payload: Value, run_id: &str, context: Option<&Value>) -> Value {
    let mut payload = speed_test_run_payload(payload, run_id);
    if let (Some(object), Some(context)) = (payload.as_object_mut(), context) {
        object.insert("measurementContext".into(), context.clone());
    }
    payload
}

fn measurement_context(ticket: &ReadyMainTicket, run_id: &str) -> Value {
    json!({"runId":run_id, "requestId":ticket.request_id(),
        "mainGeneration":ticket.generation(), "startTime":ticket.start_time()})
}

/// 绑定失效时测量闭包的占位返回值。它永远不会被发布：[`bound_speed_io`] 在返回 `None` 之前已置
/// `invalid`，让位检查点随即把整轮判为中断、该节点缺席。
const BINDING_LOST: Measured = Err(MeasureFailure::new(FailPhase::Connect, FailKind::Rejected));

/// Validate each real selector/measurement before dispatch and after completion. A lost
/// binding is an interruption, never a measured timeout that could be reported as -1.
async fn bound_speed_io<Check, CheckFuture, Io, IoFuture, T>(
    check: Check,
    invalid: &AtomicBool,
    io: Io,
) -> Option<T>
where
    Check: Fn() -> CheckFuture,
    CheckFuture: Future<Output = bool>,
    Io: FnOnce() -> IoFuture,
    IoFuture: Future<Output = T>,
{
    if !check().await {
        invalid.store(true, Ordering::SeqCst);
        return None;
    }
    let result = io().await;
    if !check().await {
        invalid.store(true, Ordering::SeqCst);
        return None;
    }
    Some(result)
}

/// 本波测速裁定（纯逻辑：请求集 × 当前活跃出口 × 本层可测范围 → 测谁 / 谁缺席 / 还是零可测）。
///
/// **抽成纯函数而非内联进 command**：command 要 `AppHandle`/`State`，本机无从构造 → 内联的判定
/// 只能靠肉眼复核。裁定是本次修复的核心（错一个分支就回到「静默返回 + 前端卡死」），故必须可单测。
#[derive(Debug, PartialEq, Eq)]
enum SpeedTestPlan {
    /// 无活跃出口（直连 / 未选节点）→ 本层零可测。
    NoActiveExit,
    /// 有活跃出口，但不在请求集内 → 请求的节点个个都要探针池，本层零可测。
    ActiveNotRequested { requested: usize },
    /// 可测当前活跃出口；`skipped` = 请求集里其余节点（需探针池，本波如实缺席）。
    Measure {
        active: String,
        skipped: Vec<String>,
    },
}

/// 裁定本波测速（[`SpeedTestPlan`]）。
///
/// - `active`：当前选中节点 id（空串 / [`DIRECT_SERVER_ID`] = 无真实出站）。
/// - `requested`：本次请求集；`None` = 全部（上游 `serverIds` 缺省语义）→ 取 `all`。
/// - `all`：当前配置里的全部节点 id（`requested=None` 时的实际请求集，也是 `skipped` 的取材面）。
fn plan_speed_test(active: &str, requested: Option<&[String]>, all: &[String]) -> SpeedTestPlan {
    if has_no_real_exit(active) {
        return SpeedTestPlan::NoActiveExit;
    }
    let requested: &[String] = requested.unwrap_or(all);
    if !requested.iter().any(|id| id == active) {
        return SpeedTestPlan::ActiveNotRequested {
            requested: requested.len(),
        };
    }
    // 活跃节点自身不进 skipped（它是本波唯一真测的那个）；其余请求节点如实缺席。
    let skipped = requested
        .iter()
        .filter(|id| id.as_str() != active)
        .cloned()
        .collect();
    SpeedTestPlan::Measure {
        active: active.to_string(),
        skipped,
    }
}

/// 从用户配置抽全部节点 id（`serverIds` 缺省时的实际请求集）。
fn all_server_ids(config: &Value) -> Vec<String> {
    config
        .get("servers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ══════════════════════════════════════════════════════════════════════════════
//  §15 主核探测池分波编排（纯逻辑，可单测；真测量走真核=真机门，本层不碰宿主网络）。
// ══════════════════════════════════════════════════════════════════════════════

/// 探测池单槽指派：第 `slot` 槽（`probe-selector-{slot}` / `probe-in-{slot}` / `pool_ports[slot]`，三者 1:1）测哪个节点。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SlotAssignment {
    /// 槽序 k（0..K）：既是 `probe-selector-k` 的序，也是 `pool_ports[slot]` 的下标（1:1 绑定）。
    slot: usize,
    /// 被测节点 id（结果回填键 + `event:speedTestResult` 的 serverId + 进度计数）。
    node_id: String,
    /// 被测节点在运行核的出站 tag（`select_outbound` 的 member_tag = `probe-selector-k` 成员）。
    tag: String,
}

/// 波前预筛分区结果（[`partition_pool`] 产出）。四个列表**互斥且各自保序**（前端徽标/进度按请求序流式回填）。
#[derive(Debug, Default, PartialEq, Eq)]
struct PoolPartition {
    /// 本波真测的节点 `(id, 出站 tag)`。
    testable: Vec<(String, String)>,
    /// 不在运行核池（`hasTag` 假）→ 诚实缺席。
    not_in_pool: Vec<String>,
    /// 在池但**已编辑未生效**（指纹 ≠ 起核快照）→ 诚实缺席。
    dirty: Vec<String>,
    /// 在池但 TS 未登录就绪 → 诚实缺席。
    ts_not_ready: Vec<String>,
}

/// 波前预筛的两个**注入集**（[`run_pool_speed_test`] 的入参束，避免 `too_many_arguments`）。
///
/// 两者都在命令层「await 之前」算好（`State` 不跨 await 持有），编排层只消费不重算 —— 重算就有了第二个
/// 真相源，而预筛的失效方式是静默的（筛错了照样出数值，只是数值属于别人）。
struct PoolPrefilter<'a> {
    /// 已编辑未生效的节点 id 集（见 [`partition_dirty`]）。
    dirty: &'a BTreeSet<String>,
    /// TS 未登录就绪的节点 id 集（见 [`partition_ts_not_ready`]）。
    ts_pending: &'a BTreeSet<String>,
    /// 每个不就绪 TS 节点的**具体成因**（键集 == `ts_pending`）。只喂零可测信封的文案，
    /// 不参与分区判定 —— 分区只问「就不就绪」，文案才需要问「为什么」。
    ts_reasons: &'a BTreeMap<String, TsNotReady>,
}

/// 请求集波前预筛分区（纯逻辑，对齐 上游 `SpeedTestService.ts:674-700` 的 `poolTestable` 循环）。
///
/// **三条腿的顺序与 上游 逐字一致**（`:680` hasTag → `:688` isDirty → `:692` tsNodeReady），因为它决定同一个
/// 节点被归到哪个缺席列表，而每个列表对用户是**不同的下一步动作**：
///  - 一个「TS 未就绪 **且** 未入池」的节点算 `notInPool`（下一步是重启内核纳入，而不是先去登录一个核里根本
///    没有的出口）；
///  - 一个「已编辑未生效 **且** TS 未就绪」的节点算 `dirty`（下一步是应用更改 —— 核重起后那份 TS 配置本身
///    就换了，此刻指引「去登录旧配置」是把人引向死路）。
///
/// - `id ∉ id_to_tag` → `not_in_pool`（订阅新增/改址未重启入池：其 tag 非 `probe-selector-k` 成员，
///   热切必失败 → 旧行为记假 `-1`）；
/// - `id ∈ dirty_pending`（指纹 ≠ 起核快照，见 [`partition_dirty`]）→ `dirty`（核仍跑旧参数，测它量到的是
///   **旧参数出口**的 RTT 却挂在新参数名下 = 失真数值）；
/// - `id ∈ ts_pending`（协议 tailscale 且未登录就绪，见 [`partition_ts_not_ready`]）→ `ts_not_ready`
///   （核已让位直连，测它量到的是直连 RTT = 失真数值）；
/// - 其余 → `testable`（带出站 tag）。
fn partition_pool(
    requested: &[String],
    id_to_tag: &BTreeMap<String, String>,
    dirty_pending: &BTreeSet<String>,
    ts_pending: &BTreeSet<String>,
) -> PoolPartition {
    let mut out = PoolPartition::default();
    for id in requested {
        let Some(tag) = id_to_tag.get(id) else {
            out.not_in_pool.push(id.clone());
            continue;
        };
        if dirty_pending.contains(id) {
            out.dirty.push(id.clone());
            continue;
        }
        if ts_pending.contains(id) {
            out.ts_not_ready.push(id.clone());
            continue;
        }
        out.testable.push((id.clone(), tag.clone()));
    }
    out
}

/// 用户配置里逐节点的**当前**指纹（dirty 判据的「新」一侧）。
///
/// 键 = 节点 id，值 = [`server_fingerprint`]（`protocol|address|port|cred|network`，与运行核起核时写进
/// [`SwitchSnapshot::fingerprints`](crate::runtime::proxy::SpeedProbeTargets::fingerprints) 的**同一个公式**
/// —— 两侧必须同源，各算各的公式必然漂移，而漂移的表现是「永远 dirty」或「永远不 dirty」，两种都静默）。
///
/// 解析不出 [`ServerConfig`] 的条目（配置损坏 / 未来字段）→ 直接跳过：**没有指纹 ⇒ 不判 dirty**，
/// 保守方向正确（照旧测，与本腿接线前逐字节一致），绝不因为解析失败就把一个正常节点筛掉。
pub(crate) fn current_server_fingerprints(config: &Value) -> BTreeMap<String, String> {
    config
        .get("servers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let id = s.get("id").and_then(Value::as_str)?;
                    let parsed: ServerConfig = serde_json::from_value(s.clone()).ok()?;
                    Some((id.to_string(), server_fingerprint(&parsed)))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 请求集里「**已编辑未生效**」的节点 id（[`partition_pool`] 的 `dirty_pending` 入参）。
///
/// 判据 1:1 上游 `MainCoreProbe.isDirty`（`ProxyManager.ts:3446-3450`）：
/// `snapshot.get(id) !== undefined && snapshot.get(id) !== serverFingerprint(server)`。
///
/// 两条 `is_some_and` 缺一不可：
/// - **快照无此 id** ⇒ 不判 dirty。那是「新增未入核」，由 `hasTag`/`notInPool` 那条腿管（指引「重启纳入」）；
///   在此处误判成 dirty 会把用户指向「应用更改」——对一个核里根本没有的节点，应用更改确实也能纳入，但
///   与既有的 notInPool 语义打架、且 `partition_pool` 的腿序已保证它先被 notInPool 接走，此处再判即死码。
/// - **当前配置无此 id** ⇒ 不判 dirty（保守：拿不到「新」一侧就没有比对基准，照旧测）。真实可达形态是
///   「请求集点名了一个刚被删除的节点」，此时它多半也已不在 `id_to_tag` 里 → 走 notInPool。
///
/// **为什么当前指纹取自 `ConfigManager` 最新 config 而不是运行核的 `current_config`**：对齐 上游的
/// F-B 修正（`:3444` 注释）—— 「订阅 OFF 自动刷新」这类路径不经 `switch_mode`，运行核侧的 config 镜像会
/// 滞后 ⇒ 拿它当「新」一侧会**漏判 dirty**，于是照旧测出旧参数出口的失真值。
fn partition_dirty(
    requested: &[String],
    snapshot_fingerprints: &BTreeMap<String, String>,
    current_fingerprints: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    requested
        .iter()
        .filter(|id| {
            let Some(snap) = snapshot_fingerprints.get(id.as_str()) else {
                return false;
            };
            current_fingerprints
                .get(id.as_str())
                .is_some_and(|cur| cur != snap)
        })
        .cloned()
        .collect()
}

/// **TS 节点「已登录就绪」判据**（纯逻辑，对齐 上游 `MainCoreProbe.tsNodeReady`，`ProxyManager.ts:3435-3442`）。
///
/// 就绪 ⟺ 有末帧 **且** `backendState == "Running"` **且** key 未过期。无帧（核未起 / 首帧未到 / 已清）
/// → **不就绪**（未知一律按不就绪：宁可缺席，绝不对一个可能已让位到直连的出口写数值）。
///
/// # 为什么不需要 上游的 `tailscaleStatusGen` 世代腿
///
/// 上游的 `tailscaleStatusCache` **跨停核保留**（`ProxyManager.ts:516-518` 注释：「connected 由
/// getStatus().running 实时判，故停代理不清缓存」）⇒ 核 restart 后新核首帧到达前，缓存里还躺着旧核的
/// `Running` 帧，必须靠 `tailscaleStatusGen === lifecycleGeneration`（M-4）挡住。
///
/// Polaris 的同一危险**在数据源侧就已封死**，故此处无同名腿（是结构性不需要，不是漏移植）：
///  - `stop_inner` 停核即 `mesh.clear_ts_status()`（`runtime/proxy.rs:2772`），而 `restart` 复用 `stop_inner`
///    ⇒ 重启后缓存空、`ts_status_event` 返 `None` → 本判据即返 false，与 M-4 的结论逐字相同；
///  - 崩溃腿同样清（`:2956`）；
///  - relay 写帧前后各查一次世代（`:3773`/`:3783`）⇒ 旧核末帧不会落进新核缓存。
///
/// 即：Polaris 里「缓存有帧」已蕴含「本代帧」，世代比对是恒真的空转。
fn ts_node_ready(ev: Option<&TailscaleStatusEvent>) -> bool {
    ev.is_some_and(TailscaleStatusEvent::exit_ready)
}

/// TS 节点「不就绪」的**具体成因**。`None` = 就绪。
///
/// # 为什么必须分开
///
/// 这四种的**用户下一步动作完全不同**，而此前它们共用一句「尚未登录就绪（登录后可测）」：
/// 真机实证（陈先生 2026-07-31）—— Tailscale 管理后台显示 `Connected`、应用里的组网卡也显示
/// 「已登录」，点测速却被告知「未登录」。他照着那句话去登录，登多少次都没用，因为那个节点
/// **本来就登着**。
///
/// 撕裂的来源是两条判据共用一个词：应用里「已登录」的角标是折叠值
/// （`backendState ∈ {Running, Starting}` 且未过期，见 `contracts/tailscale-status.ts`），
/// 而本门要求严格 `Running`。节点停在 `Starting` 时两者同时为真，用户看到的就是自相矛盾。
///
/// 所以这里不再折叠：**把成因如实说出来**，让用户知道该去登录、该等一会儿、还是该重启核。
#[derive(Debug, Clone, PartialEq, Eq)]
enum TsNotReady {
    /// 没有状态帧：核未起 / 起后首帧未到 / 停核已清。**不是「没登录」**。
    NoFrame,
    /// key 已过期 —— 登录过，但必须重新交互授权。
    Expired,
    /// 后端明确要求交互登录。**只有这一种是真的「未登录」**。
    NeedsLogin,
    /// 已登录，隧道还没通（`Starting` / `NoState` / `Stopped` …）。等它起来即可，登录是白做工。
    TunnelNotUp(String),
}

impl TsNotReady {
    /// 面向用户的一句话（含下一步动作）。
    fn user_phrase(&self) -> String {
        match self {
            Self::NoFrame => "尚未收到状态帧（核未就绪，稍后重试）".to_string(),
            Self::Expired => "登录密钥已过期，需重新授权".to_string(),
            Self::NeedsLogin => "尚未登录（登录后可测）".to_string(),
            Self::TunnelNotUp(state) => {
                format!(
                    "已登录但隧道尚未就绪（当前 {state}，等待它变为 Running 即可，无需重新登录）"
                )
            }
        }
    }
}

/// 判成因。顺序即优先级：无帧 > 过期 > 需登录 > 隧道未通。
///
/// `expired` 排在 `backend_state` 之前：key 过期时 `backendState` 完全可能仍报 `Running`
/// （与 `mesh::selected_exit_backend_state` 把 expired 折成 `NeedsLogin` 同一条理由），
/// 那种情形下说「隧道未就绪」会把用户指到错误的方向。
fn ts_not_ready_reason(ev: Option<&TailscaleStatusEvent>) -> Option<TsNotReady> {
    let Some(e) = ev else {
        return Some(TsNotReady::NoFrame);
    };
    if e.expired {
        return Some(TsNotReady::Expired);
    }
    if e.backend_state == "Running" {
        return None;
    }
    if e.backend_state == "NeedsLogin" {
        return Some(TsNotReady::NeedsLogin);
    }
    Some(TsNotReady::TunnelNotUp(e.backend_state.clone()))
}

/// 一组成因 → 报给用户的尾句。**逐类报数**，不折叠成一个总数：折叠回去就退回本次缺陷。
///
/// 空集 → 空串（调用方据此不拼尾巴）。
fn ts_not_ready_phrase(reasons: &[TsNotReady]) -> String {
    if reasons.is_empty() {
        return String::new();
    }
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for r in reasons {
        *counts.entry(r.user_phrase()).or_insert(0) += 1;
    }
    counts
        .iter()
        .map(|(phrase, n)| format!("{n} 个{phrase}"))
        .collect::<Vec<_>>()
        .join("；")
}

/// 用户配置里协议为 `tailscale` 的节点 id 集（波前预筛第二腿的取材面）。
///
/// 协议大小写不敏感（对齐 上游 `s.protocol?.toLowerCase() === 'tailscale'` 与本仓
/// `commands/server.rs:669-674` 的既有口径）。
fn tailscale_server_ids(config: &Value) -> BTreeSet<String> {
    config
        .get("servers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|s| {
                    s.get("protocol")
                        .and_then(Value::as_str)
                        .is_some_and(|p| p.eq_ignore_ascii_case("tailscale"))
                })
                .filter_map(|s| s.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 请求集里「协议 tailscale **且** 未登录就绪」的节点 id（[`partition_pool`] 的 `ts_pending` 入参）。
///
/// `ready` 注入（生产传 `|id| ts_node_ready(mesh.ts_status_event(id).as_ref())`）⇒ 本函数纯、可离线单测，
/// 且**只对 tailscale 协议节点询问就绪**：非 TS 节点没有 TS 状态帧，问了必答「不就绪」，会把整批节点误筛光。
fn partition_ts_not_ready(
    requested: &[String],
    tailscale_ids: &BTreeSet<String>,
    ready: &dyn Fn(&str) -> bool,
) -> BTreeSet<String> {
    requested
        .iter()
        .filter(|id| tailscale_ids.contains(*id))
        .filter(|id| !ready(id.as_str()))
        .cloned()
        .collect()
}

/// 波前预筛后**零可测**时的失败信封裁定（纯逻辑：`(文案, code)`）。
///
/// 零可测必须走**失败信封**而非 `ok(空)`：前端 `NodesScreen` 靠进度事件复位 `testing` 灰态，零事件 +
/// 成功信封 ⇒ 测速按钮永久 disabled（见模块文档「反伪造 + 反卡死」）。
///
/// code 按**用户的下一步动作**分流，不按内部实现分：未入池 → [`CODE_NONE_IN_POOL`]（去重启内核）；
/// 已编辑未生效 → [`CODE_ALL_DIRTY`]（去点「立即应用」）；TS 未就绪 → [`CODE_TS_NOT_READY`]（去登录那些
/// 节点）。多类并存时主码按 `notInPool > dirty > tsNotReady` 取，但文案**每一类非零的数都报** —— 只报一
/// 半会让用户按错误的修法折腾。
///
/// **为什么 dirty 排在 tsNotReady 之前**：前者是一次批量动作（应用更改，一下带回全部），后者是逐节点的
/// 手工登录。且「应用更改」会重起核 —— 那批 TS 节点的配置本身也会换，此刻先指引去登录旧配置是白做工。
fn zero_testable_envelope(
    not_in_pool: usize,
    dirty: usize,
    ts_reasons: &[TsNotReady],
) -> (String, &'static str) {
    let ts_not_ready = ts_reasons.len();
    let ts_detail = ts_not_ready_phrase(ts_reasons);
    let ts_tail = if ts_not_ready > 0 {
        format!("；另有 {ts_not_ready} 个 Tailscale 节点不可测（{ts_detail}）")
    } else {
        String::new()
    };
    let dirty_tail = if dirty > 0 {
        format!("；另有 {dirty} 个节点已编辑未生效")
    } else {
        String::new()
    };
    if not_in_pool > 0 {
        return (
            format!(
                "请求的 {not_in_pool} 个节点均未纳入运行核测速池（刷新订阅或重启核后纳入）{dirty_tail}{ts_tail}"
            ),
            CODE_NONE_IN_POOL,
        );
    }
    if dirty > 0 {
        return (
            format!(
                "请求的 {dirty} 个节点已编辑但尚未生效，运行核仍跑旧参数（应用更改后可测）{ts_tail}"
            ),
            CODE_ALL_DIRTY,
        );
    }
    if ts_not_ready > 0 {
        return (
            format!("请求的 {ts_not_ready} 个 Tailscale 节点不可测：{ts_detail}"),
            CODE_TS_NOT_READY,
        );
    }
    // 退化态（请求集为空）：仍走失败信封（零进度事件 + 成功信封会把前端测速按钮永久卡灰）。
    (
        "请求的 0 个节点均未纳入运行核测速池（刷新订阅或重启核后纳入）".to_string(),
        CODE_NONE_IN_POOL,
    )
}

/// 在池节点按 K 槽分波（纯逻辑，对齐 上游 `testServersViaMainCore` 的 `for base += K`）。
///
/// N 个在池节点 → ⌈N/K⌉ 波，每波至多 K 个 `(slot, node, tag)`；槽 `slot` = **波内位次**（跨波复用同一批槽，
/// 波间串行 → 同槽先测完再重指，`probe-selector-k` 的 `interrupt_exist_connections` 断残留防跨节点串味）。
/// `K==0`（探测池关闭的回滚锚点）→ 空 vec（调用方走回退活跃出口）。
fn plan_waves(pool_testable: &[(String, String)], k: usize) -> Vec<Vec<SlotAssignment>> {
    if k == 0 {
        return Vec::new();
    }
    pool_testable
        .chunks(k)
        .map(|wave| {
            wave.iter()
                .enumerate()
                .map(|(slot, (id, tag))| SlotAssignment {
                    slot,
                    node_id: id.clone(),
                    tag: tag.clone(),
                })
                .collect()
        })
        .collect()
}

/// 结构化错误码（**临时核路径**）：请求节点没有一个能进临时核（全 tailscale / 全构造失败）→ 零可测。
const CODE_TEMP_CORE_NONE_TESTABLE: &str = "SPEEDTEST_TEMP_CORE_NONE_TESTABLE";
/// 结构化错误码（**临时核路径**）：临时核起不来 / 未就绪 / 端口分配失败 → 本轮整批不可测。
///
/// 与 [`CODE_TEMP_CORE_NONE_TESTABLE`] 分开：前者是「这些节点本层测不了」（换节点可测），后者是
/// 「本机此刻起不了测速核」（跟节点无关）。合成一个码会把用户指向错误的排查方向。
const CODE_TEMP_CORE_FAILED: &str = "SPEEDTEST_TEMP_CORE_FAILED";
/// 结构化错误码（**临时核路径**）：本批规模越过单核上限 → **起核前**拒绝
/// （[`TempCoreOutcome::Oversized`]）。
///
/// 与 [`CODE_TEMP_CORE_FAILED`] 分开是本码存在的**全部理由**：前端把后者无条件映射成
/// `nodes.speedTestInterrupted`（zh-CN「测速中断」），与「核起不来 / 未就绪超时」逐字相同 ⇒
/// 用户会朝网络/端口方向排查，而真因是本轮 naive 节点太多、少选一些当场就能测。
/// 用户可见的分流由 `ui/src/components/screens/shared/speedtest-feedback.ts` 的同名 case 承接。
const CODE_TEMP_CORE_OVERSIZED: &str = "SPEEDTEST_TEMP_CORE_OVERSIZED";

/// 临时核零可测的用户文案（纯逻辑，可单测）。
///
/// `has_tailscale` 为假时**不得**附「Tailscale 节点须先连接主核后测」：请求集里一个 TS 节点都没有
/// （零可测的原因是构造失败 / naive 缺 cronet / 节点已删）却这么说，会把用户支去查一个他根本没有的
/// 问题，而真正的原因一个字都没提。
fn temp_core_none_testable_message(
    requested: usize,
    unusable: usize,
    has_tailscale: bool,
) -> String {
    let ts_hint = if has_tailscale {
        "Tailscale 节点须先连接主核后测；"
    } else {
        ""
    };
    format!(
        "本次请求的 {requested} 个节点没有一个能经临时测速核测量（{ts_hint}另有 {unusable} 个节点不可用）"
    )
}

/// 临时核日志级别：**与主核同一求值**（用户档 + 隐私模式地板 `warn`）。
///
/// # 为什么不再把非诊断档一律压成 `warn`
///
/// 旧实现只放行 `debug|trace`，其余压 `warn`，注释理由是「免得每次测速往 app.log 灌一堆核的 info」。
/// 这条理由站不住：主核以 `info` 常驻一整天的行数远超一次测速那点量；而代价是**同一个配置字段两种
/// 解释**——用户设 `info`，主核就是 `info`，临时核却偷偷降到 `warn`。恰恰 `info` 那几类行
/// （`inbound connection from` / `outbound connection to` / `NaiveProxy started`）才是判「核停在哪
/// 一步」的材料，2026-08-02 那次「全是 -1」就因为磁盘上零核侧证据而无从复盘。
///
/// 求值复用 [`LogLevel::effective`]（`config-engine/src/user_config/log_level.rs`），主核经
/// `build_log_config` 走的也是它；字符串形态复用 [`level_to_string`]，两处同一张映射表。
///
/// # 两处与主核不同的地方（都是如实处理，不是分叉）
///
/// - **`trace` 不在 `LogLevel` 枚举里**（那是 sing-box 侧七档词汇才有的档，本仓 `LogLevel` 只有五档），
///   走 `serde` 会解析失败退成默认 `info` ⇒ 用户拨到 trace 反而降档，正是要避免的那件事。故单独放行，
///   并按它比 `debug` 更低这一事实同样受隐私地板约束（隐私模式下压回 `warn`）。
/// - 非法/缺省值经 `LogLevel` 的 `#[default]` 落到 `info`，与主核 `log_axes_from_config` 逐字同义。
///
/// **硬次序**：本函数抬级的前提是临时核的 stdout/stderr 已经有人排空（`runtime::speedtest`
/// 的 `drive_after_spawn`）。未排空时抬级 = 提前把 64 KiB 管道写满、把核堵死 —— 2026-09-02 受控实验
/// 实测 `debug`/`trace` 在未排空时是**确定性断崖**（4/4 轮复现）。两者必须同批，且排空先生效。
fn temp_core_log_level(config: &Value, privacy_mode: bool) -> String {
    let raw = config.get("logLevel").and_then(Value::as_str);
    if raw == Some("trace") {
        return if privacy_mode { "warn" } else { "trace" }.to_string();
    }
    let level: LogLevel = raw
        .and_then(|s| serde_json::from_value(Value::from(s)).ok())
        .unwrap_or_default();
    level_to_string(level.effective(privacy_mode))
}

/// 临时核运行策略的取材：把 config 解析成 [`UserConfig`]，**解析失败也必须保住端口与网卡策略**。
///
/// `from_value::<UserConfig>` 对**任何一个无关字段**的形态错误都整体失败（如 `servers` 不是数组、
/// 某个节点缺必填键）。旧写法 `.unwrap_or_default()` 在那条腿上静默把排除集退化成「默认 control +
/// http/mixed = 0」—— 恰好丢掉这段代码存在的唯一理由：临时核于是可能占住主核随后要 bind 的口，
/// 用户表现为「测完速就连不上」，而日志里一个字都没有。
///
/// 故 Err 腿：① 记 warn（这条 warn 是排查该形态的唯一线索）；② **直接从 `Value` 单独反序列化**
/// 端口、订阅策略和全局网卡策略。`controlPort` 不读：`UserConfig` 的 `PortConfig::control_port()` 恒 `None`
/// （`config-engine/src/builder/inbounds.rs`），排除的永远是默认 9090，与解析成败无关。
fn user_config_for_temp_core(config: &Value) -> UserConfig {
    match serde_json::from_value::<UserConfig>(config.clone()) {
        Ok(c) => c,
        Err(e) => {
            let port = |key: &str| {
                config
                    .get(key)
                    .and_then(Value::as_u64)
                    .and_then(|p| u16::try_from(p).ok())
            };
            let (mixed_port, http_port) = (port("mixedPort"), port("httpPort"));
            let mut fallback = UserConfig {
                mixed_port,
                http_port,
                ..Default::default()
            };
            fallback.subscriptions = config
                .get("subscriptions")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            fallback.network_interfaces = config
                .get("networkInterfaces")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok());
            log::warn!(
                "测速临时核用户配置解析失败（已从原始 JSON 单独保留端口和网卡策略，mixed={mixed_port:?} http={http_port:?}）: {e}"
            );
            fallback
        }
    }
}

/// 从用户配置里按**请求序**取出 typed 节点（临时核出站构造的取材面）。
///
/// 保序是硬要求：临时核的「节点 ↔ 入站端口 ↔ 出站 tag」是三重逐位绑定，取材乱序 ⇒ 量到的是别人的延迟。
/// 解析不出 [`ServerConfig`] 的条目直接跳过（由调用方计入缺席，不伪造数值）。
fn requested_server_configs(config: &Value, requested: &[String]) -> Vec<ServerConfig> {
    let by_id: BTreeMap<&str, &Value> = config
        .get("servers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.get("id").and_then(Value::as_str).map(|id| (id, s)))
                .collect()
        })
        .unwrap_or_default();
    requested
        .iter()
        .filter_map(|id| by_id.get(id.as_str()))
        .filter_map(|v| serde_json::from_value::<ServerConfig>((*v).clone()).ok())
        .collect()
}

/// **临时核测速腿**（主核未运行；对齐 上游 `SpeedTestService.testServersViaProxy`，`:388-620`）。
///
/// 编排：请求集 → typed 节点（保序）→ [`plan_temp_core_with_bindings`] 裁掉临时核结构性测不了的（tailscale / naive 缺
/// cronet / 构造失败）→ [`TempCoreSession::run`] 起核 + 就绪门 + 分批并发量 warm-TTFB + **无条件收尾** →
/// 折成响应信封。
///
/// # 让位基准必须在 await 之前捕获
///
/// `gen0` 与 `superseded` 闭包都在 `TempCoreSession::run` 的 `.await` **之前**建好。捕获在之后 = 跟自己比，
/// 判据恒假 ⇒ 用户中途点「连接」时临时核不让路，两个核并存跑同一批 WG/WARP peer（双会话事故）。
///
/// # 零可测 / 起核失败一律走**失败信封**
///
/// 与池路径同一条纪律：前端 `NodesScreen` 靠进度事件复位 `testing` 灰态，零事件 + 成功信封 ⇒ 测速按钮
/// 永久 disabled 到组件重挂载。
async fn run_temp_core_speed_test(
    app: &AppHandle,
    state: &State<'_, AppRuntime>,
    config: &Value,
    server_ids: Option<Vec<String>>,
    run_id: &str,
    cancel: &SpeedTestCancel,
    done: &mut Option<Value>,
) -> ApiResponse<Value> {
    let url = resolve_speed_test_url(config);
    let all = all_server_ids(config);
    let requested: Vec<String> = server_ids.unwrap_or(all);
    let servers = requested_server_configs(config, &requested);
    // 请求了但配置里查无此节点（前端状态陈旧 / 刚被删）→ 如实缺席，不伪造。
    let missing: Vec<String> = {
        let present: BTreeSet<&str> = servers.iter().map(|s| s.id.as_str()).collect();
        requested
            .iter()
            .filter(|id| !present.contains(id.as_str()))
            .cloned()
            .collect()
    };

    let proxy = state.proxy.clone();
    let user_config = user_config_for_temp_core(config);
    let bind_interfaces: BTreeMap<String, String> = servers
        .iter()
        .filter_map(|server| {
            effective_proxy_bind_interface(server, &user_config)
                .map(|interface| (server.id.clone(), interface))
        })
        .collect();
    let plan = plan_temp_core_with_bindings(&servers, &proxy.core_build_env(), &bind_interfaces);
    if plan.testable.is_empty() {
        if !plan.system_interface_blocked.is_empty() {
            #[cfg(target_os = "android")]
            return ApiResponse::err_with_code(
                "Android VPN mode cannot run a system-interface endpoint speed test",
                crate::runtime::proxy::code::SYSTEM_INTERFACE_UNSUPPORTED,
            );
            #[cfg(not(target_os = "android"))]
            {
                let blocked_names: Vec<String> = servers
                    .iter()
                    .filter(|server| plan.system_interface_blocked.contains(&server.id))
                    .map(|server| server.name.clone())
                    .collect();
                return ApiResponse::err_with_code(
                polaris_config_engine::builder::system_interfaces::system_interface_ownership_error(
                    &blocked_names,
                ),
                crate::runtime::proxy::code::SYSTEM_INTERFACE_REQUIRES_HELPER,
            );
            }
        }
        return ApiResponse::err_with_code(
            temp_core_none_testable_message(
                requested.len(),
                plan.unusable.len() + missing.len(),
                !plan.tailscale.is_empty(),
            ),
            CODE_TEMP_CORE_NONE_TESTABLE,
        );
    }

    // §15.11 让位（超代）基准：**必须在 await 之前捕获**（判据见函数文档）。
    let gen0 = proxy.core_generation();
    let superseded = || {
        let st = proxy.status();
        is_temp_core_superseded(proxy.core_generation(), gen0, st.running, st.starting)
    };

    // 端口排除集：用户配置的 control/http/mixed 口 —— 临时核占了它们，主核随后就起不来
    // （表现为「测完速就连不上」，归因极难）。
    let exclusions = PortExclusions::for_primary_api(
        Some(control_api_port(&user_config)),
        user_config.http_port,
        None,
        user_config.mixed_port,
    );
    // 隐私模式读**单一真值**（`commands::config` 的进程状态机），不另存镜像 —— 与主核经
    // `ProxyErrorEmitter::privacy_mode` 取的是同一个值（`runtime/proxy.rs:792-801`）。
    let privacy_mode = crate::commands::config::config_get_privacy_mode(State::clone(state))
        .data
        .unwrap_or(false);
    #[cfg(target_os = "android")]
    let temp_auth = match crate::commands::config::generate_local_api_secret() {
        Ok(password) => InboundUser {
            username: "polaris-temp".to_owned(),
            password,
        },
        Err(error) => return ApiResponse::err_with_code(error, CODE_TEMP_CORE_FAILED),
    };
    #[cfg(target_os = "android")]
    let deps = TempCoreDeps::production_android(
        state.config().dir().to_path_buf(),
        exclusions,
        temp_core_log_level(config, privacy_mode),
        temp_auth.clone(),
    );
    #[cfg(not(target_os = "android"))]
    let deps = TempCoreDeps::production(
        state.config().dir().to_path_buf(),
        exclusions,
        temp_core_log_level(config, privacy_mode),
    );
    let deps = deps.with_cancel(cancel.clone());

    // 临时核腿的每条结果都标「未连接测量」：实例只标种类、不带世代与配置摘要；节点指纹取当前配置
    // 按同一公式算出的那一份。网络代次只在主核运行期间计数，这里留空（未知）。
    let request = SpeedTestRequest {
        origin: SpeedTestOrigin::Manual,
        targets: requested.clone(),
        scope: None,
        path: MeasurePath::Candidate,
        url: url.clone(),
    };
    let mut events = RunEvents::new(
        run_id,
        &request,
        CoreInstance::Temp,
        None,
        current_server_fingerprints(config),
        None,
    )
    .with_skipped(vec![
        (
            "tempCoreUnsupported",
            plan.unusable.len() + plan.tailscale.len(),
        ),
        (
            "systemInterfaceBlocked",
            plan.system_interface_blocked.len(),
        ),
        ("missing", missing.len()),
    ]);

    let outcome = TempCoreSession::run(
        &deps,
        &plan.testable,
        &superseded,
        |port| {
            let url = url.clone();
            #[cfg(target_os = "android")]
            let auth = temp_auth.clone();
            async move {
                #[cfg(target_os = "android")]
                {
                    measure_via_local_proxy(port, Some(&auth), &url, &|_| {})
                        .await
                        .measured
                }
                #[cfg(not(target_os = "android"))]
                {
                    measure_via_local_proxy(port, None, &url, &|_| {})
                        .await
                        .measured
                }
            }
        },
        &mut events.sink(hold_terminal(app, done)),
    )
    .await;

    // 临时核结构性测不了的节点（tailscale）如实进 `tsNotReady` —— 与主核路径同一个键，对用户是同一件事
    // 「本轮没测」，且指引一致（先连主核 / 先登录）。对齐 上游 L-2（`:248-250`）把漂移剔除的 TS-exit
    // 计入 skipped 的处置。
    // `notInPool` 只回 id：成因（[`UnusableReason`]）是排查材料，已按原因汇总进日志，跨语言契约不动。
    let mut not_in_pool: Vec<String> = plan.unusable.into_iter().map(|(id, _)| id).collect();
    not_in_pool.extend(missing);
    match outcome {
        TempCoreOutcome::Ran { results, outcome } => ApiResponse::ok(json!({
            "runId": run_id,
            "results": results,
            "outcome": outcome,
            "notInPool": not_in_pool,
            "tsNotReady": plan.tailscale,
            "systemInterfaceBlocked": plan.system_interface_blocked,
            "dirty": Vec::<String>::new(),
        })),
        // 起核前就被主核接管 → 一个节点都没测。**失败信封**：零进度事件 + 成功信封会把前端测速按钮
        // 永久卡灰（同池路径「反伪造 + 反卡死」那一节）。
        TempCoreOutcome::AndroidCapacityClosed(error) => ApiResponse::err_with_code(
            error.to_string(),
            crate::runtime::proxy::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED,
        ),
        TempCoreOutcome::Superseded => ApiResponse::err_with_code(
            if cancel.reason().is_some() {
                "测速已取消"
            } else {
                "测速已让位给正在启动的代理内核（主核起来后可经主核测速池重测）"
            },
            CODE_TEMP_CORE_FAILED,
        ),
        TempCoreOutcome::Failed(e) => ApiResponse::err_with_code(e, CODE_TEMP_CORE_FAILED),
        TempCoreOutcome::CleanupUnknown(e) => ApiResponse::err_with_code(e, CODE_TEMP_CORE_FAILED),
        TempCoreOutcome::SystemInterfaceRequired(e) => {
            #[cfg(target_os = "android")]
            {
                let _ = e;
                ApiResponse::err_with_code(
                    "Android VPN mode cannot run a system-interface endpoint speed test",
                    crate::runtime::proxy::code::SYSTEM_INTERFACE_UNSUPPORTED,
                )
            }
            #[cfg(not(target_os = "android"))]
            {
                ApiResponse::err_with_code(
                    e,
                    crate::runtime::proxy::code::SYSTEM_INTERFACE_REQUIRES_HELPER,
                )
            }
        }
        // 规模超限**必须**是独立的码：并进 `CODE_TEMP_CORE_FAILED` 就等于告诉用户「测速中断」，
        // 与就绪超时逐字相同，而两者的修法南辕北辙（一个是少选 naive 节点，一个是查网络/端口）。
        TempCoreOutcome::Oversized(e) => ApiResponse::err_with_code(e, CODE_TEMP_CORE_OVERSIZED),
    }
}

/// 上游 `SERVER_SPEED_TEST`：测速（serverIds 缺省=全部；逐节点结果/进度经 event:speedTestResult 推送）。
///
/// 主核在跑 → 池路径 / 回退活跃出口；主核未跑 → **临时核腿**（见 [`run_temp_core_speed_test`]）。
/// 可测范围与三条波前预筛见模块文档。绝不回假延迟。
#[tauri::command]
pub async fn server_speed_test(
    app: AppHandle,
    state: State<'_, AppRuntime>,
    server_ids: Option<Vec<String>>,
) -> Result<ApiResponse<Value>, ()> {
    // The real request and single-flight slot exist before any system authorization wait.
    let Some(guard) = SpeedTestGuard::acquire(SpeedTestOrigin::Manual).await else {
        return Ok(ApiResponse::err_with_code(
            "已有测速进行中，请等待当前测速完成",
            CODE_IN_FLIGHT,
        ));
    };
    let Some(run_id) = next_speed_test_run_id(&SPEED_TEST_RUN_SEQUENCE) else {
        return Ok(ApiResponse::err("测速运行序列已耗尽，请重启应用"));
    };
    // 三条腿把终态交到这里；本函数在释放单飞闸之后才发它。
    let mut done = None;
    let emit_done = |payload| {
        let _ = app.emit(EVENT_SPEED_TEST_DONE, payload);
    };
    let proxy = state.proxy.clone();
    let saved = match state.config().current() {
        Ok(saved) => saved,
        Err(error) => return Ok(ApiResponse::err(error.to_string())),
    };
    let ticket = if NormalMainAction::ManualSpeedTest
        .requirement(polaris_helper_proto::Platform::current())
        == ActionRequirement::NormalMainRequired
    {
        let requested = server_ids.clone().unwrap_or_else(|| all_server_ids(&saved));
        if requested.is_empty() {
            return Ok(ApiResponse::err("没有可测速的节点"));
        }
        let targets = requested
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let binding = match ActionBinding::new(
            NormalMainAction::ManualSpeedTest,
            run_id.clone(),
            &saved,
            targets,
            None,
        ) {
            Ok(binding) => binding,
            Err(error) => return Ok(ApiResponse::err_with_code(error.to_string(), error.code())),
        };
        let _ = app.emit(
            EVENT_SPEED_TEST_PROGRESS,
            json!({"runId":run_id,"phase":"preparingConnection"}),
        );
        let _ = app.emit(
            EVENT_SPEED_TEST_PROGRESS,
            json!({"runId":run_id,"phase":"waitingForReady"}),
        );
        match proxy.await_normal_main(binding).await {
            Ok(ticket) => Some(ticket),
            Err(error) => return Ok(ApiResponse::err_with_code(error.to_string(), error.code())),
        }
    } else {
        None
    };
    // Read every runtime resource again after the exact committed ready receipt.
    let config = match state.config().current() {
        Ok(saved) => saved,
        Err(error) => return Ok(ApiResponse::err(error.to_string())),
    };
    if let Some(ticket) = ticket.as_ref() {
        if let Err(error) = proxy.validate_ready_main(ticket).await {
            return Ok(ApiResponse::err_with_code(error.to_string(), error.code()));
        }
    }
    let status = proxy.status();
    let context = ticket
        .as_ref()
        .map(|ticket| measurement_context(ticket, &run_id));
    if let Some(context) = context.as_ref() {
        let _ = app.emit(
            EVENT_SPEED_TEST_PROGRESS,
            json!({"runId":run_id,"phase":"measuring","measurementContext":context}),
        );
    }
    // 本机 http 代理入站（桌面 `mixed-in` / Android `probe-proxy-in`）的取址：本函数的回退腿用它测活跃出口。
    let local_proxy = match ticket.as_ref() {
        Some(ticket) => ticket.local_http_proxy(),
        None => proxy.local_http_proxy(),
    };
    // 核在跑却没有可用的本机 http 代理入站（分配失败的半态）→ 本层确实无从测：临时核腿在此形态下会被
    // 让位判据（`running == true`）当场掐掉，硬走只会空转一轮。如实 clean error，绝不回假延迟。
    // **文案不得说「核未运行」**：核正跑着，缺的是本地代理端口。说反了会把用户支去点「连接」（他已经连着），
    // 排查方向整个偏掉。
    //
    // 判据是 `local_http_proxy()` 而不是 `mixed_port == 0`：Android 不发射 mixed 入站、`mixed_port` 恒 0，
    // 旧判据会让 Android 上**每一次**测速都在这里报「端口分配失败」（α 批修）。
    if status.running && local_proxy.is_none() {
        return Ok(ApiResponse::err(
            "代理核在运行但本地代理端口缺失（端口分配失败），本层无从测速：重启内核后重试",
        ));
    }
    // 主核**正在启动**（`start` 已置在飞标记、核尚未就绪）→ 临时核腿视作「已被占用」，clean error。
    //
    // 为什么必须在入口挡：`start` 的顺序是 `start_inflight+1` →（可达数秒的）stale 清扫 →
    // `bump_generation` → spawn → 就绪门。这整段里 `running == false` 且世代可能已 bump 完
    // （⇒ 本次测速取的 `gen0` 就是新世代），让位判据的世代腿与 running 腿**同时**盖不住。用户点
    // 「连接」后紧接点测速（或托盘/另一窗口点——UI 灰态拦不住跨窗）就是确定性命中：起临时核 ⇒ 与
    // 启动中的主核同 peer 双会话踢线，且临时核可能抢走主核刚解析、尚未 bind 的 api/probe 池口 ⇒
    // 主核 FATAL address-in-use。入口这道是快路径；真正扛竞态的是让位判据的第三条腿（`st.starting`）。
    if status.starting && !status.running {
        return Ok(ApiResponse::err_with_code(
            "代理内核正在启动，请等待连接完成后再测速",
            CODE_CORE_STARTING,
        ));
    }

    // ── 临时核腿（主核**未运行**）：起一个瞬态 sing-box 逐节点量 warm-TTFB，测完即杀 ──
    // 「先测速比较延迟、再选最快的连上去」是常规使用序；没有这条腿，用户必须先盲选一个节点连上才能测别的。
    // 隔离/让位/收尾语义全在 `runtime::speedtest` 的模块文档（独立配置文件 + 独立端口 + 不写主核生命周期槽；
    // 主核一起来立刻让路）。
    if !status.running {
        if ticket.is_some() || cfg!(target_os = "ios") {
            return Ok(ApiResponse::err_with_code("superseded", "superseded"));
        }
        let response = run_temp_core_speed_test(
            &app,
            &state,
            &config,
            server_ids,
            &run_id,
            guard.cancel(),
            &mut done,
        )
        .await;
        release_then_emit(guard, done, emit_done);
        return Ok(response);
    }
    let active = config
        .get("selectedServerId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let url = resolve_speed_test_url(&config);
    let all = all_server_ids(&config);
    let tailscale_ids = tailscale_server_ids(&config);
    let current_fingerprints = current_server_fingerprints(&config);

    // §15 主核探测池分波测速（池就绪 → 「批量比较多节点延迟选优」核心路径）：把请求的 N 个节点按 K 分波，
    // 逐波经 gRPC select_outbound 热切各槽到本波节点、经 probe-in-k 端口量 warm-TTFB。详见模块文档路径①。
    if let Some(targets) = proxy.speed_probe_targets() {
        let requested: Vec<String> = server_ids.clone().unwrap_or_else(|| all.clone());
        // 波前预筛第三腿（dirty）的入参：起核快照指纹 vs **ConfigManager 最新** config 的当前指纹。
        // 当前侧取自 `config`（本函数开头刚读的最新配置），不取运行核的 config 镜像 —— 后者在「订阅 OFF
        // 自动刷新」这类不经 switch_mode 的路径上会滞后 ⇒ 漏判 dirty（对齐 上游 F-B 修正）。
        let dirty_pending =
            partition_dirty(&requested, &targets.fingerprints, &current_fingerprints);
        // 波前预筛第二腿的入参：**取值在 await 之前**（`State` 不跨 await 持有）。TS 状态活态读 mesh 末帧
        // 缓存（`ts_status_event`），判据见 [`ts_node_ready`]。
        let ts_pending = partition_ts_not_ready(&requested, &tailscale_ids, &|id| {
            ts_node_ready(state.mesh().ts_status_event(id).as_ref())
        });
        // 成因与上面的就绪判定**同一次读**同一份缓存：分开读两次会在两次之间收到新帧，
        // 出现「判了不就绪、却取不到成因」或反过来的撕裂。
        let ts_reasons: BTreeMap<String, TsNotReady> = ts_pending
            .iter()
            .filter_map(|id| {
                ts_not_ready_reason(state.mesh().ts_status_event(id).as_ref())
                    .map(|r| (id.clone(), r))
            })
            .collect();
        let prefilter = PoolPrefilter {
            dirty: &dirty_pending,
            ts_pending: &ts_pending,
            ts_reasons: &ts_reasons,
        };
        let open_readback = || {
            Readback::open(
                BINDING_READBACK,
                proxy.readback_endpoint(ticket.as_ref()),
                proxy.speed_binding_context(),
                WatchedInbounds::ProbePool,
            )
        };
        let response = run_pool_speed_test(
            &app,
            &proxy,
            &targets,
            &requested,
            &url,
            configured_concurrency(&config, Platform::current()),
            &prefilter,
            &run_id,
            ticket.as_ref(),
            context.as_ref(),
            guard.cancel(),
            &mut done,
            &open_readback,
        )
        .await;
        release_then_emit(guard, done, emit_done);
        return Ok(response);
    }

    // ── 回退：探测池未注入（端口分配失败/回滚）→ 仅当前活跃出口经 mixed 口可测 ──
    // 零可测的两条腿一律走**失败信封**（非 ok(empty)）：前端据此 throw → catch 复位 testing，
    // 且 code 让 UI 分得清「本层测不了」与「测了但失败」。详见模块文档「反伪造 + 反卡死」。
    let (active, skipped) = match plan_speed_test(&active, server_ids.as_deref(), &all) {
        SpeedTestPlan::NoActiveExit => {
            return Ok(ApiResponse::err_with_code(
                "当前出口为直连 / 未选节点，主混合代理无真实出站可测",
                CODE_NO_ACTIVE_EXIT,
            ));
        }
        SpeedTestPlan::ActiveNotRequested { requested } => {
            return Ok(ApiResponse::err_with_code(
                format!(
                    "测速探测池未就绪（端口分配失败已回退）；本层仅能测当前活跃出口，而它不在本次请求的 {requested} 个节点内"
                ),
                CODE_PROBE_POOL_UNWIRED,
            ));
        }
        SpeedTestPlan::Measure { active, skipped } => (active, skipped),
    };

    // 波前预筛（回退腿版）：本腿唯一真测的就是活跃出口，故只需筛它一个。它若是**未登录就绪的 TS 节点**，
    // 运行核已把默认路由让位到直连（`login_fallback`）⇒ 经混合口量到的是**直连** RTT，记进该节点名下就是
    // 失真数值（比记 -1 更有害：用户会照着一个假的低延迟去选这个连不通的节点）。诚实缺席 → 失败信封。
    // 其余请求节点的缺席原因是「本层无池」而非「TS 未就绪」，故仍如实归 notInPool，不在此处改判。
    if tailscale_ids.contains(&active)
        && !ts_node_ready(state.mesh().ts_status_event(&active).as_ref())
    {
        return Ok(ApiResponse::err_with_code(
            "当前出口是尚未登录就绪的 Tailscale 节点（核已让位直连），测它量到的是直连而非该出口",
            CODE_TS_NOT_READY,
        ));
    }

    // §15.11 让位（超代）基准：**回退腿同样须守**（此前本腿零 `superseded()` 覆盖，见
    // [`drive_fallback_measure`] 文档）。`gen0` 必须在 await **之前**捕获，判据与池路径共用 [`is_superseded`]。
    let gen0 = proxy.core_generation();
    let invalid = AtomicBool::new(false);
    let superseded = || {
        invalid.load(Ordering::SeqCst)
            || is_superseded(proxy.core_generation(), gen0, proxy.status().running)
            || ticket
                .as_ref()
                .is_some_and(|ticket| proxy.check_ready_main(ticket).is_err())
    };

    // 入口已判过 `running && local_proxy.is_none()` ⇒ 走到这里（running 为真）必有值；
    // 仍按 `Option` 取而不 `expect`：判据写在别处，这里不押注它。
    let Some(local_proxy) = local_proxy else {
        return Ok(ApiResponse::err(
            "代理核在运行但本地代理端口缺失（端口分配失败），本层无从测速：重启内核后重试",
        ));
    };
    // 本腿经本机代理入站出网。路径由实际用的入站定：`mixed-in` 走用户的完整路由与 DNS，记
    // `system`；`probe-proxy-in` 钉到选中出口、不经用户规则，记 `selected`。池未注入时读不到起核
    // 指纹快照（见模块文档「已知残留」）⇒ 节点指纹留空。
    let request = SpeedTestRequest {
        origin: SpeedTestOrigin::Manual,
        targets: server_ids.unwrap_or(all),
        scope: None,
        path: MeasurePath::of_local_inbound(local_proxy.inbound),
        url: url.clone(),
    };
    let readback = Readback::open(
        BINDING_READBACK,
        proxy.readback_endpoint(ticket.as_ref()),
        proxy.speed_binding_context(),
        WatchedInbounds::One(local_proxy.inbound.tag()),
    );
    let selected_tag = proxy.management_target_for(&active).map(|(_, _, tag)| tag);
    let mut events = RunEvents::new(
        &run_id,
        &request,
        CoreInstance::Main {
            generation: gen0,
            start_time: status.start_time,
        },
        proxy.ready_main_emission_digest(gen0),
        BTreeMap::new(),
        context.clone(),
    )
    .with_skipped(vec![("notInPool", skipped.len())])
    .with_readback(&readback);
    let (results, outcome) = drive_fallback_measure(
        &active,
        &superseded,
        guard.cancel(),
        &|| proxy.network_epoch(),
        || async {
            bound_speed_io(
                || async {
                    match ticket.as_ref() {
                        Some(ticket) => proxy.validate_ready_main(ticket).await.is_ok(),
                        None => true,
                    }
                },
                &invalid,
                || {
                    measure_inbound(
                        &readback,
                        local_proxy.inbound.tag(),
                        local_proxy.port,
                        local_proxy.auth.as_ref(),
                        &url,
                    )
                },
            )
            .await
            .unwrap_or(BINDING_LOST.into())
        },
        &mut events.sink(hold_terminal(&app, &mut done)),
        &SystemProbe {
            inbound: local_proxy.inbound,
            selected_tag: selected_tag.as_deref(),
            readback: &readback,
            attribution: SYSTEM_PATH_ATTRIBUTION,
        },
    )
    .await;
    release_then_emit(guard, done, emit_done);

    Ok(ApiResponse::ok(json!({
        "runId": run_id,
        "measurementContext": context,
        "results": results,
        // completed：本次入参已全部裁定（测的测了、缺席的进 notInPool）；interrupted：被核跃迁/崩溃打断，
        // 该节点**缺席**（前端据此保留旧值，见 contracts/speed-test.ts SpeedTestOutcome）。
        "outcome": outcome,
        // 请求了但本层测不了的节点（需探针池）→ 如实回报，UI 据此显「N 未纳入」而非假装测过。
        "notInPool": skipped,
        // 本腿走到这里 ⇒ 活跃出口已通过 TS 就绪预筛（未就绪已在上面早退），其余节点的缺席原因一律是
        // 「本层无池」（已进 notInPool）⇒ 本腿的 tsNotReady 恒空是**如实**，不是未接线。
        "tsNotReady": [],
    })))
}

/// **回退腿的测量 + 让位收口**（测量 / 事件发射两个 I/O 面**全部注入** ⇒ 无 `AppHandle`、不碰宿主网络、可单测）。
///
/// # 为什么这条腿也必须守让位
///
/// 池路径的让位三检查点（[`drive_pool_waves`]）此前**没有对应物在回退腿上**：`probe_pool_ports` 为空的
/// 回退腿既无 gen0 捕获、`measure_via_local_proxy` 前后也无检查，`outcome` 硬编码 `"completed"`。后果是
/// 测量中途核重启/崩溃会把一个 `-1`（或经**新**出口测得的值）记在**旧** `selectedServerId` 上 —— 正是
/// 模块文档「绝不伪造数值」承诺要消灭的那类伪造，只是发生在更少见的路径上（端口分配失败才走回退）。
///
/// # 语义与池路径逐字一致
///
/// 被取代 → 该节点**缺席**（不写 `results`、不推 `result`/`progress` 事件）+ `outcome="interrupted"`，
/// 而不是记 `-1`。「超代未测」与「真实超时」不可混淆，这是诚实性根基（同 [`drive_pool_waves`] 的让位③）。
/// 未被取代 → 照常记账：`total` 恒 1（本腿真可测数就是 1，把 `notInPool` 算进 total 等于谎报测过）。
///
/// # 终态事件的唯一出口就在本函数
///
/// 内核 [`drive_fallback_measure_inner`] 有 2 个 `return`（让位 + 正常收尾），本薄壳收成一个出口再发
/// [`EVENT_SPEED_TEST_DONE`]。本腿的 `intended` 恒为 `[active]` 一个元素 ⇒ 中断时 `pending == [active]`
/// （它就是唯一没测成的那个）。判据见 [`emit_speed_test_done`]。
async fn drive_fallback_measure<Meas, MeasFut>(
    active: &str,
    superseded: &(dyn Fn() -> bool + Sync),
    cancel: &SpeedTestCancel,
    network_epoch: &(dyn Fn() -> Option<u64> + Sync),
    measure: Meas,
    emit: &mut (dyn FnMut(&str, Value) + Send),
    probe: &SystemProbe<'_>,
) -> (serde_json::Map<String, Value>, &'static str)
where
    Meas: FnOnce() -> MeasFut,
    MeasFut: Future<Output = Probed>,
{
    let intended = [active.to_string()];
    let (results, reason) = drive_fallback_measure_inner(
        active,
        superseded,
        cancel,
        network_epoch,
        measure,
        emit,
        probe,
    )
    .await;
    // 终态发出之前，读回用的连接事件流已经关闭。
    probe.readback.close().await;
    let outcome = outcome_of(reason);
    emit_speed_test_done(emit, outcome, &results, &intended, reason);
    (results, outcome)
}

/// `outcome` 由中断成因派生，不是各腿各写一个字面量 ⇒「说自己 interrupted 却给不出成因」写不出来。
const fn outcome_of(reason: Option<InterruptReason>) -> &'static str {
    if reason.is_some() {
        "interrupted"
    } else {
        "completed"
    }
}

/// 主核两条腿（池 / 回退）共用的中断判据：取消在前，让位在后。
///
/// 未取消时恰好询问一次 `superseded` —— 让位检查点的次数与次序不因并入取消而变。
fn interrupted(
    cancel: &SpeedTestCancel,
    superseded: &(dyn Fn() -> bool + Sync),
) -> Option<InterruptReason> {
    cancel
        .reason()
        .or_else(|| superseded().then_some(InterruptReason::Superseded))
}

async fn drive_fallback_measure_inner<Meas, MeasFut>(
    active: &str,
    superseded: &(dyn Fn() -> bool + Sync),
    cancel: &SpeedTestCancel,
    network_epoch: &(dyn Fn() -> Option<u64> + Sync),
    measure: Meas,
    emit: &mut (dyn FnMut(&str, Value) + Send),
    probe: &SystemProbe<'_>,
) -> (serde_json::Map<String, Value>, Option<InterruptReason>)
where
    Meas: FnOnce() -> MeasFut,
    MeasFut: Future<Output = Probed>,
{
    let epoch0 = network_epoch();
    // 经本机混合端口真实测速：warm-TTFB（两次 GET 计第二次，对齐 mihomo unified-delay）。
    // 与取消竞速：取消命中即丢掉在飞的测量（本函数不 spawn，丢 future 即关 socket），不等它测完。
    let (measured, record) = tokio::select! {
        biased;
        reason = cancel.cancelled() => return (serde_json::Map::new(), Some(reason)),
        measured = measure_system(probe, measure) => measured,
    };

    // ── 让位（测量后）：在飞期间核跃迁/崩溃 ⇒ 在飞值量的是新核/已死核的出站 → 丢弃并略过该节点 ──
    if let Some(reason) = interrupted(cancel, superseded) {
        return (serde_json::Map::new(), Some(reason));
    }
    // 测量跨了一次网络变化 ⇒ 这个值属于哪张网说不清 → 该节点缺席（不是失败），整轮照常收尾。
    let epoch = network_epoch();
    if network_epoch_changed(epoch0, epoch) {
        log::info!("测速期间网络发生变化，节点 {active} 本轮记为未测");
        return (serde_json::Map::new(), None);
    }

    let binding = probe.bind(record);
    // 走用户规则的测量可能被规则分流到别处：叶子出站不是选中节点时，这个值不属于它。
    // 结果归属「系统路径」本身（只入账），选中节点本轮缺席。
    if !probe.belongs_to_node(&binding) {
        log::info!(
            "系统路径测量未经由选中节点 {active}（实际承载 {}），结果不记入该节点",
            binding.carried_by().unwrap_or("未读到")
        );
        emit(
            EVENT_SPEED_TEST_RESULT,
            result_payload_with_binding(SYSTEM_PATH_ID, &measured, epoch, Some(&binding)),
        );
        return (serde_json::Map::new(), None);
    }

    // 逐节点结果 + 进度（前端 onSpeedTestResult / onSpeedTestProgress 流式回填）。
    let mut results = serde_json::Map::new();
    let (mut tested, mut ok) = (0, 0);
    record_measured(
        &mut results,
        &mut tested,
        &mut ok,
        emit,
        active,
        &measured,
        epoch,
        1,
        Some(&binding),
    );
    (results, None)
}

/// 回退腿与出口伴测共用的取材：经哪个本机入站测、选中节点在运行核里的 tag、读回与归属口径。
pub(crate) struct SystemProbe<'a> {
    pub(crate) inbound: LocalInbound,
    /// 选中节点在运行核里的 tag；它不在起核快照里时为 `None`。
    pub(crate) selected_tag: Option<&'a str>,
    pub(crate) readback: &'a Readback,
    pub(crate) attribution: SystemAttribution,
}

impl SystemProbe<'_> {
    /// 这类测量不指定节点：读回只有「读到了」与「没读到」。
    fn bind(&self, record: Option<ConnRecord>) -> Binding {
        self.readback
            .bind(RoutePolicy::of_local_inbound(self.inbound), None, record)
    }

    fn belongs_to_node(&self, binding: &Binding) -> bool {
        system_result_belongs_to_node(
            binding.policy,
            self.readback.mode,
            self.attribution,
            binding.carried_by(),
            self.selected_tag,
        )
    }
}

/// **§15 主核探测池分波测速**（`server_speed_test` 池就绪腿；对齐 上游 `SpeedTestService.testServersViaMainCore`）。
///
/// 编排：请求集经 [`partition_pool`] 波前预筛分「可测 / notInPool / tsNotReady」→ 可测节点经 [`plan_waves`]
/// 按 K 分波 → 逐波：①各槽 [`ProxyRuntime::probe_select_slot`] 热切 `probe-selector-k` → 本波节点；②波内
/// **并发**经 `probe-in-k` 端口量 warm-TTFB（K 槽各测各出口不串味）；③逐节点推 `event:speedTestResult` /
/// `event:speedTestProgress` + 收集。波间串行（同槽跨波复用，selector `interrupt_exist_connections` 断残留防串味）。
///
/// **诚实性**：两条波前缺席列表如实回报（不 select / 不 measure / 不 report、绝不伪造）；热切失败/超时的槽记
/// -1（真实不可测，非缺席）；`total` = 波前预筛**后**的可测数（把缺席节点算进 total 等于谎报测过）。零可测 →
/// 失败信封（[`zero_testable_envelope`] 分流 code，前端 catch 复位、防卡死）。
///
/// **禁本机碰宿主网络**：真延迟走真核真出站 = 真机门；本函数的分波/分区/热切编排纯逻辑已由
/// [`plan_waves`]/[`partition_pool`] 单测，真数值只在真机验。
#[allow(clippy::too_many_arguments)]
async fn run_pool_speed_test(
    app: &AppHandle,
    proxy: &Arc<ProxyRuntime>,
    targets: &SpeedProbeTargets,
    requested: &[String],
    url: &str,
    concurrency: usize,
    prefilter: &PoolPrefilter<'_>,
    run_id: &str,
    ticket: Option<&ReadyMainTicket>,
    context: Option<&Value>,
    cancel: &SpeedTestCancel,
    done: &mut Option<Value>,
    open_readback: &(dyn Fn() -> Readback + Sync),
) -> ApiResponse<Value> {
    let PoolPartition {
        testable: pool_testable,
        not_in_pool,
        dirty,
        ts_not_ready,
    } = partition_pool(
        requested,
        &targets.id_to_tag,
        prefilter.dirty,
        prefilter.ts_pending,
    );

    // 波前预筛后零可测（全未入池 / 全已编辑未生效 / 全 TS 未就绪 / 混合）→ 失败信封防前端卡死 +
    // 缺席原因如实分流。
    if pool_testable.is_empty() {
        // 成因按 `ts_not_ready` 的**实际缺席集**取（不是整张 `ts_reasons`）——分区腿的优先级
        // 可能已经把某个 TS 节点归到 notInPool/dirty 去了，那种情况下再报它的 TS 成因是误导。
        let ts_reasons: Vec<TsNotReady> = ts_not_ready
            .iter()
            .filter_map(|id| prefilter.ts_reasons.get(id).cloned())
            .collect();
        let (msg, code) = zero_testable_envelope(not_in_pool.len(), dirty.len(), &ts_reasons);
        return ApiResponse::err_with_code(msg, code);
    }

    // 零可测已在上面早退：到这里才建读回流。
    let readback = &open_readback();
    let total = pool_testable.len();
    let waves = plan_waves(
        &pool_testable,
        wave_width(concurrency, targets.pool_ports.len(), total),
    );

    // §15.11 让位（超代）基准：本轮归属的核世代。三检查点均以它比对（见 [`drive_pool_waves`]）。
    let gen0 = proxy.core_generation();
    let invalid = Arc::new(AtomicBool::new(false));
    let superseded = || {
        invalid.load(Ordering::SeqCst)
            || is_superseded(proxy.core_generation(), gen0, proxy.status().running)
            || ticket.is_some_and(|ticket| proxy.check_ready_main(ticket).is_err())
    };

    let request = SpeedTestRequest {
        origin: SpeedTestOrigin::Manual,
        targets: requested.to_vec(),
        scope: None,
        path: MeasurePath::Candidate,
        url: url.to_string(),
    };
    let mut events = RunEvents::new(
        run_id,
        &request,
        CoreInstance::Main {
            generation: gen0,
            start_time: proxy.status().start_time,
        },
        proxy.ready_main_emission_digest(gen0),
        targets.fingerprints.clone(),
        context.cloned(),
    )
    .with_skipped(vec![
        ("notInPool", not_in_pool.len()),
        ("dirty", dirty.len()),
        ("tsNotReady", ts_not_ready.len()),
    ])
    .with_readback(readback);

    let (results, outcome) = drive_pool_waves(
        &waves,
        total,
        &superseded,
        cancel,
        &|| proxy.network_epoch(),
        |slot, tag: String| {
            let invalid = Arc::clone(&invalid);
            async move {
                bound_speed_io(
                    || async {
                        match ticket {
                            Some(ticket) => proxy.validate_ready_main(ticket).await.is_ok(),
                            None => true,
                        }
                    },
                    &invalid,
                    || async {
                        match ticket {
                            Some(ticket) => proxy.probe_select_slot_bound(ticket, slot, &tag).await,
                            None => proxy.probe_select_slot(slot, &tag).await,
                        }
                    },
                )
                .await
                .unwrap_or(false)
            }
        },
        |slot, port| {
            let url = url.to_string();
            // `probe-in-k` 的凭据与池端口同源同刻（`SpeedProbeTargets::auth`；桌面 `None`）。
            let auth = targets.auth.clone();
            let runtime = Arc::clone(proxy);
            let ticket = ticket.cloned();
            let invalid = Arc::clone(&invalid);
            let readback = readback.clone();
            async move {
                bound_speed_io(
                    || async {
                        match ticket.as_ref() {
                            Some(ticket) => runtime.validate_ready_main(ticket).await.is_ok(),
                            None => true,
                        }
                    },
                    &invalid,
                    || measure_slot(&readback, slot, port, auth.as_ref(), &url),
                )
                .await
                .unwrap_or(BINDING_LOST.into())
            }
        },
        &mut events.sink(hold_terminal(app, done)),
        targets.pool_ports.as_slice(),
        readback,
    )
    .await;

    ApiResponse::ok(json!({
        "runId": run_id,
        "measurementContext": context,
        "results": results,
        // completed：本次入参已全部裁定（在池的测了、notInPool 如实缺席）；interrupted：被核跃迁/崩溃打断，
        // 未测节点**缺席**（前端据此保留旧值，见 contracts/speed-test.ts SpeedTestOutcome）。
        "outcome": outcome,
        "notInPool": not_in_pool,
        // TS 未登录就绪 → 波前缺席（核已让位直连，测它量到的是直连 RTT）。判据见 [`ts_node_ready`]。
        "tsNotReady": ts_not_ready,
        // 已编辑未生效 → 波前缺席（核仍跑旧参数，测它量到的是旧参数出口的 RTT）。判据见 [`partition_dirty`]。
        //
        // **独立键、不并进 `notInPool`**（1:1 上游 `:688` 的 `continue` 不入 `runCtx.skipped`）：两者
        // 是不同的物理事实与不同的修法（未入池=重启纳入 / 已编辑未生效=应用更改），并进去等于后端谎报
        // 「这些节点不在池里」。**已知残留**：渲染端 `notInPoolMessage` 目前只累加 `notInPool + tsNotReady`
        // ⇒ 混合形态下 toast 少报 dirty 那几个（本批禁碰 `ui/`）。同一事实另有 Home「N 项待应用」操作条
        // 承载，故非静默；接线渲染端计数是后续一行改动。
        "dirty": dirty,
    }))
}

/// 后台自动故障切换复用主核探测池时的结果。它与用户测速共享同一个 [`SpeedTestGuard`]，因此两轮
/// 不会同时改写 `probe-selector-k`；后台拿不到租约就让位，不把一次人为测速误判成节点全故障。
/// 在飞期间被手动测速抢占时同样以 `Interrupted` 收尾：被抢占不是节点失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeProbeBatch {
    Busy,
    Interrupted,
    Completed(BTreeMap<String, CandidateProbe>),
}

/// 经当前运行核的 K 槽 probe pool 对一组 `(serverId, outboundTag)` 做真实协议链探测。
///
/// 候选资格（已入核、未脏、TS 已就绪）由 `ProxyRuntime` 在调用前裁定；本函数只复用现有分波、
/// `SelectOutbound`、CONNECT+warm-TTFB 与 generation guard。它不发用户测速事件，避免后台健康治理
/// 污染节点页的进度条和延迟缓存。
pub(crate) async fn probe_runtime_candidates(
    proxy: &ProxyRuntime,
    targets: &SpeedProbeTargets,
    candidates: &[(String, String)],
    url: &str,
) -> RuntimeProbeBatch {
    let Some(guard) = SpeedTestGuard::acquire(SpeedTestOrigin::Failover).await else {
        return RuntimeProbeBatch::Busy;
    };
    // 本腿的调用方不带配置，波宽取自动值；与手动、周期两腿一样不超过池端口数。
    let waves = plan_waves(
        candidates,
        wave_width(
            auto_concurrency(Platform::current()),
            targets.pool_ports.len(),
            candidates.len(),
        ),
    );
    if waves.is_empty() {
        return RuntimeProbeBatch::Completed(BTreeMap::new());
    }
    let gen0 = proxy.core_generation();
    let superseded = || is_superseded(proxy.core_generation(), gen0, proxy.status().running);
    // 本腿不向外发任何事件；仍经发布口走一遍，是为了让它与手动运行出同一行汇总。
    let request = SpeedTestRequest {
        origin: SpeedTestOrigin::Failover,
        targets: candidates.iter().map(|(id, _)| id.clone()).collect(),
        scope: None,
        path: MeasurePath::Candidate,
        url: url.to_string(),
    };
    let run_id = next_speed_test_run_id(&SPEED_TEST_RUN_SEQUENCE).unwrap_or_default();
    let readback = Readback::open(
        BINDING_READBACK,
        proxy.management_endpoint(),
        proxy.speed_binding_context(),
        WatchedInbounds::ProbePool,
    );
    let mut events = RunEvents::new(
        &run_id,
        &request,
        CoreInstance::Main {
            generation: gen0,
            start_time: proxy.status().start_time,
        },
        proxy.ready_main_emission_digest(gen0),
        targets.fingerprints.clone(),
        None,
    )
    .with_readback(&readback);
    let mut sink = events.sink(|_, _| {});
    drive_runtime_probe(
        candidates,
        &waves,
        &superseded,
        guard.cancel(),
        |slot, tag: String| async move { proxy.probe_select_slot(slot, &tag).await },
        |slot, port| {
            let url = url.to_string();
            // `probe-in-k` 的凭据与池端口同源同刻（`SpeedProbeTargets::auth`；桌面 `None`）。
            let auth = targets.auth.clone();
            let readback = readback.clone();
            async move { measure_slot(&readback, slot, port, auth.as_ref(), &url).await }
        },
        &mut sink,
        targets.pool_ports.as_slice(),
        &readback,
    )
    .await
}

/// 故障切换腿的编排核（热切、测量、事件出口全部注入）：跑一轮分波，把结果折成逐候选的结局。
#[allow(clippy::too_many_arguments)]
async fn drive_runtime_probe<Sel, SelFut, Meas, MeasFut>(
    candidates: &[(String, String)],
    waves: &[Vec<SlotAssignment>],
    superseded: &(dyn Fn() -> bool + Sync),
    cancel: &SpeedTestCancel,
    select_slot: Sel,
    measure: Meas,
    emit: &mut (dyn FnMut(&str, Value) + Send),
    pool_ports: &[u16],
    readback: &Readback,
) -> RuntimeProbeBatch
where
    Sel: Fn(usize, String) -> SelFut,
    SelFut: Future<Output = bool>,
    Meas: Fn(usize, u16) -> MeasFut,
    MeasFut: Future<Output = Probed> + Send + 'static,
{
    let mut trace = WaveTrace::default();
    let (raw, reason) = drive_pool_waves_traced(
        waves,
        candidates.len(),
        superseded,
        cancel,
        // 本腿不按网络代次作废单个节点：代次恒报未知，行为与引入代次之前相同。
        &|| None,
        select_slot,
        measure,
        emit,
        pool_ports,
        &mut trace,
        readback,
    )
    .await;
    if reason.is_some() {
        return RuntimeProbeBatch::Interrupted;
    }
    RuntimeProbeBatch::Completed(candidate_probes(candidates, &raw, &trace.mismatch_skipped))
}

/// 把一轮分波的结果折成逐候选的结局（纯函数）：出了值的是测得；记了 -1 的是真测了没通；
/// 不在结果里的，**只有因读回不符被跳过的才算「没测」**，其余缺席（本机探针口连不上、测量任务
/// 异常）与引入读回之前一样按没通计 —— 故障切换的后续判定因此在「只观测」档下与之前逐条相同。
pub(crate) fn candidate_probes(
    candidates: &[(String, String)],
    raw: &serde_json::Map<String, Value>,
    mismatch_skipped: &BTreeSet<String>,
) -> BTreeMap<String, CandidateProbe> {
    candidates
        .iter()
        .map(|(id, _)| {
            let probe = match raw.get(id).and_then(Value::as_i64) {
                Some(value) => {
                    u32::try_from(value).map_or(CandidateProbe::Failed, CandidateProbe::Measured)
                }
                None if mismatch_skipped.contains(id) => CandidateProbe::MismatchSkipped,
                None => CandidateProbe::Failed,
            };
            (id.clone(), probe)
        })
        .collect()
}

/// 「测速并发」取自动时的值：桌面 32，手机 16。全部改回 16 即回到改动前的波宽。
const fn auto_concurrency(platform: Platform) -> usize {
    match platform {
        Platform::Mac | Platform::Win | Platform::Linux => 32,
        Platform::Android | Platform::Ios | Platform::Other => 16,
    }
}

/// 配置里的「测速并发」：4 到本平台槽位上限的整数；缺省、`"auto"` 或越界值都取自动。
/// 它是原始配置的顶层键，不进内核配置，改它不重启内核。
pub(crate) fn configured_concurrency(config: &Value, platform: Platform) -> usize {
    let allowed = polaris_store::SPEED_TEST_CONCURRENCY_MIN
        ..=polaris_store::speed_test_slot_cap(platform) as u64;
    config
        .get("speedTestConcurrency")
        .and_then(Value::as_u64)
        .filter(|n| allowed.contains(n))
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(auto_concurrency(platform))
}

/// 本轮的波宽：实际并发不超过池端口数（槽位上限在起核时定），也不必超过本轮可测节点数。
/// 分波只用前 `波宽` 个槽。
pub(crate) const fn wave_width(concurrency: usize, pool_ports: usize, testable: usize) -> usize {
    let width = if concurrency < pool_ports {
        concurrency
    } else {
        pool_ports
    };
    if width < testable {
        width
    } else {
        testable
    }
}

/// 调度器取消在飞的周期一轮用的槽。取消句柄要到准入之后才有；早于准入的取消请求先记在这里，
/// 准入时兑现。
#[derive(Default)]
pub(crate) struct RoundAbort {
    requested: AtomicBool,
    cancel: Mutex<Option<SpeedTestCancel>>,
}

impl RoundAbort {
    /// 取消这一轮（成因「被取消」；为什么取消由调度器自己记）。
    pub(crate) fn abort(&self) {
        self.requested.store(true, Ordering::SeqCst);
        if let Some(cancel) = self
            .cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            cancel.cancel(InterruptReason::Cancelled);
        }
    }

    fn arm(&self, cancel: &SpeedTestCancel) {
        *self.cancel.lock().unwrap_or_else(PoisonError::into_inner) = Some(cancel.clone());
        if self.requested.load(Ordering::SeqCst) {
            cancel.cancel(InterruptReason::Cancelled);
        }
    }
}

/// 周期一轮的结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScheduledRound {
    /// 闸被同级或更高优先级的来源占着（附占用方；查的那一刻已释放则为 `None`）。
    /// 没有任何节点被测，也没有任何节点被记成失败。
    Busy(Option<SpeedTestOrigin>),
    /// 探针池不可用（起核时端口没分到）。
    PoolUnavailable,
    Ran(RoundReport),
}

/// 周期一轮跑过之后的回执。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoundReport {
    pub(crate) run: String,
    /// 准入时刻与收尾时刻（Unix 毫秒）。
    pub(crate) started_at: u64,
    pub(crate) ended_at: u64,
    /// 已入账的节点：`true` 为测出了值，`false` 为真测了没通。
    pub(crate) measured: BTreeMap<String, bool>,
    /// 可测但本轮没有结果的节点（被中断、跨网络变化、本机侧故障、冻结后丢弃），按目标顺序。
    pub(crate) unmeasured: Vec<String>,
    /// 起测前被预筛跳过的节点及原因。
    pub(crate) skipped: Vec<(String, &'static str)>,
    /// 中断成因；完成收尾为 `None`。
    pub(crate) interrupted: Option<InterruptReason>,
    /// 是否检出了设备冻结（相邻两条结果的墙钟间隔超过阈值）。
    pub(crate) frozen: bool,
    /// 本轮开始时的波宽，以及本轮内是否因本机侧报错减半过。
    pub(crate) width: usize,
    pub(crate) halved: bool,
}

/// 周期一轮在准入时定下、整轮固定的取材。
pub(crate) struct ScheduledRoundInput<'a> {
    /// 有序目标（排序由调度器定；分波按这个顺序切块）。
    pub(crate) targets: &'a [String],
    pub(crate) scope: String,
    pub(crate) url: &'a str,
    pub(crate) id_to_tag: &'a BTreeMap<String, String>,
    pub(crate) dirty: &'a BTreeSet<String>,
    pub(crate) ts_pending: &'a BTreeSet<String>,
    pub(crate) pool_ports: &'a [u16],
    pub(crate) concurrency: usize,
    pub(crate) instance: CoreInstance,
    pub(crate) config_digest: Option<String>,
    pub(crate) fingerprints: BTreeMap<String, String>,
}

/// 周期一轮的注入面：闸、运行号序列、账本、取消槽、墙钟与冻结阈值。
pub(crate) struct ScheduledRoundDeps<'a> {
    pub(crate) gate: &'a SpeedTestGate,
    pub(crate) sequence: &'a AtomicU64,
    pub(crate) ledger: &'static MeasurementLedger,
    pub(crate) abort: &'a RoundAbort,
    pub(crate) wall_clock: &'a (dyn Fn() -> u64 + Sync),
    /// 相邻两条结果的墙钟间隔超过它即判设备冻结；取 `u64::MAX` 即关掉这条判据。
    pub(crate) freeze_gap_ms: u64,
    /// 开本轮的读回上下文。确认有可测目标、领到运行号之后才调：没拿到闸或零可测的一轮不建流。
    pub(crate) open_readback: &'a (dyn Fn() -> Readback + Sync),
}

/// **周期一轮的编排核**（热切 / 测量 / 事件出口全部注入，不碰宿主网络）。
///
/// 与手动、故障切换共用同一套准入、分波、让位与发布口，不设旁路：
/// - 以周期来源抢闸，抢不到即返回「忙」，不测任何节点；
/// - 波前预筛与手动测速同一个函数，跳过的节点登记进账本；
/// - 事件只经 [`RunEvents`] 盖章后发出，所以外面只看得到逐节点结果，顶层没有 `runId`；
/// - 冻结判据接在盖章**之前**：被丢弃的结果既不发出也不入账。
pub(crate) async fn drive_scheduled_round<Sel, SelFut, Meas, MeasFut>(
    input: ScheduledRoundInput<'_>,
    deps: &ScheduledRoundDeps<'_>,
    superseded: &(dyn Fn() -> bool + Sync),
    network_epoch: &(dyn Fn() -> Option<u64> + Sync),
    select_slot: Sel,
    measure: Meas,
    out: &mut (dyn FnMut(&str, Value) + Send),
) -> ScheduledRound
where
    Sel: Fn(usize, String) -> SelFut,
    SelFut: Future<Output = bool>,
    Meas: Fn(usize, u16) -> MeasFut,
    MeasFut: Future<Output = Probed> + Send + 'static,
{
    let Some(guard) = deps.gate.acquire(SpeedTestOrigin::Schedule).await else {
        return ScheduledRound::Busy(deps.gate.holder());
    };
    deps.abort.arm(guard.cancel());
    let started_at = (deps.wall_clock)();

    let partition = partition_pool(
        input.targets,
        input.id_to_tag,
        input.dirty,
        input.ts_pending,
    );
    let skipped: Vec<(String, &'static str)> = [
        (&partition.not_in_pool, "notInPool"),
        (&partition.dirty, "dirty"),
        (&partition.ts_not_ready, "tsNotReady"),
    ]
    .into_iter()
    .flat_map(|(ids, reason)| ids.iter().map(move |id| (id.clone(), reason)))
    .collect();
    deps.ledger.set_skipped(input.targets, &skipped);

    let width = wave_width(
        input.concurrency,
        input.pool_ports.len(),
        partition.testable.len(),
    );
    let mut report = RoundReport {
        started_at,
        ended_at: started_at,
        skipped,
        width,
        ..RoundReport::default()
    };
    let waves = plan_waves(&partition.testable, width);
    // 本轮零可测：不领运行号，没有任何节点被测。
    if waves.is_empty() {
        return ScheduledRound::Ran(report);
    }
    let Some(run_id) = next_speed_test_run_id(deps.sequence) else {
        return ScheduledRound::Ran(report);
    };
    // 有可测目标、领到了运行号，才建读回流。
    let readback = (deps.open_readback)();

    let request = SpeedTestRequest {
        origin: SpeedTestOrigin::Schedule,
        targets: input.targets.to_vec(),
        scope: Some(input.scope),
        path: MeasurePath::Candidate,
        url: input.url.to_string(),
    };
    let mut events = RunEvents::new(
        &run_id,
        &request,
        input.instance,
        input.config_digest,
        input.fingerprints,
        None,
    )
    .with_skipped(vec![
        ("notInPool", partition.not_in_pool.len()),
        ("dirty", partition.dirty.len()),
        ("tsNotReady", partition.ts_not_ready.len()),
    ])
    .with_ledger(deps.ledger)
    .with_readback(&readback);

    let mut trace = WaveTrace::default();
    let mut admitted: BTreeMap<String, bool> = BTreeMap::new();
    let mut frozen = false;
    let (_, reason) = {
        let mut stamped = events.sink(out);
        let mut last_event = started_at;
        // 冻结判据：只看逐节点结果之间的墙钟间隔（首条与准入时刻比）。进程被系统冻结后，在飞的
        // 测量解冻时会表现为超时；照常记账就是把一批好节点记成失败。
        let mut gated = |event: &str, payload: Value| {
            if event == EVENT_SPEED_TEST_RESULT {
                let now = (deps.wall_clock)();
                if frozen || now.saturating_sub(last_event) > deps.freeze_gap_ms {
                    if !frozen {
                        log::warn!(
                            "周期测速：相邻两条结果相隔 {}ms，判定设备冻结，丢弃其后的结果并取消本轮",
                            now.saturating_sub(last_event)
                        );
                        frozen = true;
                        guard.cancel().cancel(InterruptReason::Cancelled);
                    }
                    return;
                }
                last_event = now;
                if let Some(id) = payload["serverId"].as_str() {
                    admitted.insert(id.to_string(), payload["latency"].as_i64() >= Some(0));
                }
            }
            stamped(event, payload);
        };
        drive_pool_waves_traced(
            &waves,
            partition.testable.len(),
            superseded,
            guard.cancel(),
            network_epoch,
            select_slot,
            measure,
            &mut gated,
            input.pool_ports,
            &mut trace,
            &readback,
        )
        .await
    };
    report.run = run_id;
    report.ended_at = (deps.wall_clock)();
    report.unmeasured = partition
        .testable
        .iter()
        .map(|(id, _)| id)
        .filter(|id| !admitted.contains_key(*id))
        .cloned()
        .collect();
    report.measured = admitted;
    report.interrupted = reason;
    report.frozen = frozen;
    report.halved = trace.halved;
    drop(guard);
    ScheduledRound::Ran(report)
}

/// 周期一轮的生产入口：取材（运行核的池、配置、波前预筛的两个注入集）后交给
/// [`drive_scheduled_round`]。逐节点结果经 `AppHandle` 发给前端；不持票据，不启动代理。
pub(crate) async fn run_scheduled_round(
    app: &AppHandle,
    ledger: &'static MeasurementLedger,
    targets: &[String],
    scope: String,
    abort: &RoundAbort,
    freeze_gap_ms: u64,
) -> ScheduledRound {
    let proxy = Arc::clone(&app.state::<AppRuntime>().proxy);
    // 取值都在 await 之前：`State` 不跨 await 持有。
    let (probe, config, dirty, ts_pending) = {
        let state = app.state::<AppRuntime>();
        let (Some(probe), Ok(config)) = (proxy.speed_probe_targets(), state.config().current())
        else {
            return ScheduledRound::PoolUnavailable;
        };
        let dirty = partition_dirty(
            targets,
            &probe.fingerprints,
            &current_server_fingerprints(&config),
        );
        let ts_pending = partition_ts_not_ready(targets, &tailscale_server_ids(&config), &|id| {
            ts_node_ready(state.mesh().ts_status_event(id).as_ref())
        });
        (probe, config, dirty, ts_pending)
    };
    let url = resolve_speed_test_url(&config);
    let gen0 = proxy.core_generation();
    let superseded = || is_superseded(proxy.core_generation(), gen0, proxy.status().running);
    // 读回流在准入之后才开；测量闭包从这里取同一份去登记自己的连接。
    let round_readback: std::sync::OnceLock<Readback> = std::sync::OnceLock::new();
    let open_readback = || {
        round_readback
            .get_or_init(|| {
                Readback::open(
                    BINDING_READBACK,
                    proxy.management_endpoint(),
                    proxy.speed_binding_context(),
                    WatchedInbounds::ProbePool,
                )
            })
            .clone()
    };
    let input = ScheduledRoundInput {
        targets,
        scope,
        url: &url,
        id_to_tag: &probe.id_to_tag,
        dirty: &dirty,
        ts_pending: &ts_pending,
        pool_ports: &probe.pool_ports,
        concurrency: configured_concurrency(&config, Platform::current()),
        instance: CoreInstance::Main {
            generation: gen0,
            start_time: proxy.status().start_time,
        },
        config_digest: proxy.ready_main_emission_digest(gen0),
        fingerprints: probe.fingerprints.clone(),
    };
    let deps = ScheduledRoundDeps {
        gate: &SPEED_TEST_GATE,
        sequence: &SPEED_TEST_RUN_SEQUENCE,
        ledger,
        abort,
        wall_clock: &crate::runtime::subscription_scheduler::now_ms,
        freeze_gap_ms,
        open_readback: &open_readback,
    };
    drive_scheduled_round(
        input,
        &deps,
        &superseded,
        &|| proxy.network_epoch(),
        |slot, tag: String| {
            let proxy = Arc::clone(&proxy);
            async move { proxy.probe_select_slot(slot, &tag).await }
        },
        |slot, port| {
            let url = url.clone();
            // `probe-in-k` 的凭据与池端口同源同刻（`SpeedProbeTargets::auth`；桌面 `None`）。
            let auth = probe.auth.clone();
            let readback = round_readback.get().cloned();
            async move {
                match readback {
                    Some(readback) => {
                        measure_slot(&readback, slot, port, auth.as_ref(), &url).await
                    }
                    // 测量只在准入之后发起，那时读回已开；取不到就照测，读回落成未验证。
                    None => measure_via_local_proxy(port, auth.as_ref(), &url, &|_| {}).await,
                }
            }
        },
        &mut |event, payload| {
            let _ = app.emit(event, payload);
        },
    )
    .await
}

/// 周期测速的计划状态（只读；字段见 [`crate::runtime::measurement_scheduler`]）。
#[tauri::command]
pub fn speed_test_schedule_status(app: AppHandle) -> ApiResponse<Value> {
    ApiResponse::ok(crate::runtime::measurement_scheduler::status(&app))
}

/// **§15.11 让位判据**（纯逻辑，对齐 上游 `SpeedTestService.ts:706` 的 `superseded()`）。
///
/// 两条腿的**析取**，缺一不可：
///  - `gen_now != gen0`：核 start/stop/restart/regen 跃迁 —— 在飞结果量的是**别的核**；
///  - `!running`：核**自发崩溃** —— 崩溃分支不 bump 世代（世代腿漏判），但 `running` 立即转 false。
///
/// 漏掉 `!running` 腿 ⇒ 崩溃窗口的在飞测量失败会被记成「真实超时 -1」，即**伪造数值**（诚实性根基）。
const fn is_superseded(gen_now: u64, gen0: u64, running: bool) -> bool {
    gen_now != gen0 || !running
}

/// **§15.11 分波编排核**（热切 / 测量 / 事件发射三个 I/O 面**全部注入** ⇒ 无 `AppHandle`、不碰宿主网络、可单测）。
///
/// 让位三检查点逐条对齐 上游 `SpeedTestService.ts:711/734/751`，各自守不同的窗口：
///  1. **波首**（`:711`）：核已跃迁 → 停发新波，已测部分照常返回，未测节点缺席；
///  2. **热切后**（`:734`）：热切期间跃迁 ⇒ 本波 `select_outbound` 的失败是**超代所致**而非节点真不可测 ——
///     不加这道，超代的热切失败会被下面记成 `-1`（伪造「真实超时」）；
///  3. **测量后**（`:751`）：测量在飞期间跃迁 ⇒ 在飞值量的是新核/已死核的出站，丢弃而非记账。
///
/// **未测节点一律缺席，绝不写假 -1** —— 这是「超代未测」与「真实超时」不可混淆的诚实性根基。
/// 返回 `(结果 map, outcome)`；任一检查点命中即 `interrupted`。
///
/// # 回填粒度：**逐节点**（对齐 上游，非按波）
///
/// 结果与进度在**每个节点自己测完那一刻**就落账 + 推事件（上游 `SpeedTestService.ts:773` 的 `report()`
/// 就写在 `wave.map` 的每个 worker 体内）。按波统一回填的话，首个延迟数字最晚要等**整波最慢的那个**
/// —— 一波里只要有一个死节点，屏幕就先空 8s，此后每波一跳。总耗时不变，主观耗时天差地别。
///
/// **代价（如实登记）**：让位③随之从「整波级」降为「逐节点级」—— 已经回填的节点不可能再撤回，故跃迁
/// 时丢弃的只是**尚未回来**的那些在飞值，而不是整波。这**正是 上游的语义**（`:751` 的超代检查也在
/// worker 体内、`report()` 之前），且诚实性根基不动：跃迁后回来的值一律丢弃、绝不写假 -1。
///
/// # 终态事件的唯一出口
///
/// 内核 [`drive_pool_waves_inner`] 有 4 个 `return`（让位三检查点 + 正常收尾），薄壳
/// [`drive_pool_waves_traced`] 把它们收成一个出口再发 [`EVENT_SPEED_TEST_DONE`] ⇒ 「中断了却没发
/// 终态」在结构上写不出来。载荷含未测集合（续测输入），判据见 [`emit_speed_test_done`]。
/// 本函数是它面向手动与故障切换两条腿的入口：中断成因折成 `outcome` 字符串。
#[allow(clippy::too_many_arguments)]
async fn drive_pool_waves<Sel, SelFut, Meas, MeasFut>(
    waves: &[Vec<SlotAssignment>],
    total: usize,
    superseded: &(dyn Fn() -> bool + Sync),
    cancel: &SpeedTestCancel,
    network_epoch: &(dyn Fn() -> Option<u64> + Sync),
    select_slot: Sel,
    measure: Meas,
    emit: &mut (dyn FnMut(&str, Value) + Send),
    pool_ports: &[u16],
    readback: &Readback,
) -> (serde_json::Map<String, Value>, &'static str)
where
    Sel: Fn(usize, String) -> SelFut,
    SelFut: Future<Output = bool>,
    Meas: Fn(usize, u16) -> MeasFut,
    MeasFut: Future<Output = Probed> + Send + 'static,
{
    let (results, reason) = drive_pool_waves_traced(
        waves,
        total,
        superseded,
        cancel,
        network_epoch,
        select_slot,
        measure,
        emit,
        pool_ports,
        &mut WaveTrace::default(),
        readback,
    )
    .await;
    (results, outcome_of(reason))
}

/// 一轮分波里发生过、但不进结果的事。
#[derive(Debug, Default)]
pub(crate) struct WaveTrace {
    /// 本轮内波宽是否因本机侧报错减半过。
    pub(crate) halved: bool,
    /// 因读回不符被「强制」档跳过的节点。它们缺席的原因是「量到的不是它」，与其余缺席
    /// （本机侧故障、测量任务异常、跨网络变化）不同。
    pub(crate) mismatch_skipped: BTreeSet<String>,
}

/// [`drive_pool_waves`] 的本体：多带一个 [`WaveTrace`]，并把中断成因原样交回（而不是折成
/// `outcome` 字符串）。终态事件仍在这里单点发出。
#[allow(clippy::too_many_arguments)]
async fn drive_pool_waves_traced<Sel, SelFut, Meas, MeasFut>(
    waves: &[Vec<SlotAssignment>],
    total: usize,
    superseded: &(dyn Fn() -> bool + Sync),
    cancel: &SpeedTestCancel,
    network_epoch: &(dyn Fn() -> Option<u64> + Sync),
    select_slot: Sel,
    measure: Meas,
    emit: &mut (dyn FnMut(&str, Value) + Send),
    pool_ports: &[u16],
    trace: &mut WaveTrace,
    readback: &Readback,
) -> (serde_json::Map<String, Value>, Option<InterruptReason>)
where
    Sel: Fn(usize, String) -> SelFut,
    SelFut: Future<Output = bool>,
    Meas: Fn(usize, u16) -> MeasFut,
    MeasFut: Future<Output = Probed> + Send + 'static,
{
    // 本腿「已裁定要测」的集合 = 分波后的全部槽位节点（`plan_waves` 就是按可测集分的波，
    // 故这里恒等于波前预筛后的 `pool_testable`，无第二真值源）。
    let intended: Vec<String> = waves.iter().flatten().map(|a| a.node_id.clone()).collect();
    let (results, reason) = drive_pool_waves_inner(
        waves,
        total,
        superseded,
        cancel,
        network_epoch,
        select_slot,
        measure,
        emit,
        pool_ports,
        LOCAL_SIDE_HALVING,
        trace,
        readback,
    )
    .await;
    // 终态发出之前，读回用的连接事件流已经关闭。
    readback.close().await;
    emit_speed_test_done(emit, outcome_of(reason), &results, &intended, reason);
    (results, reason)
}

/// 连不上本机探针口时是否把它当作本机侧故障处理：该节点本轮不记失败、排回队尾重测一次，
/// 其后各波的宽度减半（不低于 [`MIN_WAVE_WIDTH`]，本轮内只降不升）。
///
/// `false`：沿用旧行为，按传输错记账、波宽不变。回退 = 改这个常量后重新出包；两种取值各有单测。
const LOCAL_SIDE_HALVING: bool = true;

/// 波宽减半的下限。
const MIN_WAVE_WIDTH: usize = 4;

#[allow(clippy::too_many_arguments)]
async fn drive_pool_waves_inner<Sel, SelFut, Meas, MeasFut>(
    waves: &[Vec<SlotAssignment>],
    total: usize,
    superseded: &(dyn Fn() -> bool + Sync),
    cancel: &SpeedTestCancel,
    network_epoch: &(dyn Fn() -> Option<u64> + Sync),
    select_slot: Sel,
    measure: Meas,
    emit: &mut (dyn FnMut(&str, Value) + Send),
    pool_ports: &[u16],
    local_side_halving: bool,
    trace: &mut WaveTrace,
    readback: &Readback,
) -> (serde_json::Map<String, Value>, Option<InterruptReason>)
where
    Sel: Fn(usize, String) -> SelFut,
    SelFut: Future<Output = bool>,
    Meas: Fn(usize, u16) -> MeasFut,
    MeasFut: Future<Output = Probed> + Send + 'static,
{
    let mut results = serde_json::Map::new();
    let mut tested = 0usize;
    let mut ok = 0usize;

    // 传入的分波是起始计划：每波开头按**当前**波宽从剩余目标里取，槽 = 波内位次。波宽不变时
    // 取出来的就是传入的那几波；本机侧报错后波宽减半，剩余目标按新宽度重切。
    let mut width = waves.iter().map(Vec::len).max().unwrap_or(0);
    let mut queue: VecDeque<(String, String)> = waves
        .iter()
        .flatten()
        .map(|a| (a.node_id.clone(), a.tag.clone()))
        .collect();
    // 因本机侧报错排回过队尾的节点：每个只重排一次，再撞就让它缺席。
    let mut requeued: BTreeSet<String> = BTreeSet::new();

    while !queue.is_empty() {
        let wave: Vec<SlotAssignment> = queue
            .drain(..width.min(queue.len()))
            .enumerate()
            .map(|(slot, (node_id, tag))| SlotAssignment { slot, node_id, tag })
            .collect();
        let mut local_side_error = false;
        // ── 让位①（波首）：核跃迁/崩溃/已取消 → 停发新波 ──
        if let Some(reason) = interrupted(cancel, superseded) {
            return (results, Some(reason));
        }

        // 1. 波内各槽热切 probe-selector-k → 本波节点（gRPC select_outbound，live 生效）。逐槽记成败：
        //    热切失败（核未就绪 / stale tag）→ 该槽本波不测，节点记 -1（真实不可测，非伪造缺席）。
        //
        //    **并行**（对齐 上游 `SpeedTestService.ts:718-727` 的 `Promise.all(wave.map(...))`）：
        //    每次热切 = 新建一条 lazy gRPC channel + 一次 select_outbound 往返，串行时这 K 次往返
        //    全摊在每一波的关键路径上。`join_all` **保序** ⇒ `selected[i]` 仍与 `wave[i]` 逐位对应。
        //    各槽热切的是**互不相同**的 `probe-selector-k`，本层无共享可变状态。
        let selected: Vec<bool> =
            futures::future::join_all(wave.iter().map(|a| select_slot(a.slot, a.tag.clone())))
                .await;

        // ── 让位②（热切后）：热切期间跃迁 ⇒ 本波 select 结果作废，不得把超代的热切失败记成真实 -1 ──
        // 取消不与在飞的热切竞速：一次热切 RPC 自带 2s deadline，取消到终态的时延以它为上界。
        if let Some(reason) = interrupted(cancel, superseded) {
            return (results, Some(reason));
        }

        // 2. 热切失败的槽本波不测 → 立刻记 -1 回填（**真实**不可测：让位②刚放行，说明核没跃迁，
        //    这次 select 失败是 stale tag / 节点不可用，不是超代所致）。对齐 上游 `:739-744`。
        for (i, a) in wave.iter().enumerate() {
            if !selected[i] {
                record_measured(
                    &mut results,
                    &mut tested,
                    &mut ok,
                    emit,
                    &a.node_id,
                    &Err(MeasureFailure::new(FailPhase::Select, FailKind::Rejected)),
                    network_epoch(),
                    total,
                    // 换选没成功，没有测量连接可读回。
                    Some(&readback.bind(RoutePolicy::SlotPin, Some(&a.tag), None)),
                );
            }
        }

        // 3. 波内并发量 warm-TTFB（各槽经其 probe-in-k 回环端口测各自出口，互不污染）。热切失败的槽不 spawn。
        //    **每回来一个就回填一个**（不等整波）—— 首个数字几百毫秒内上屏，而不是等本波最慢的那个。
        let mut set = tokio::task::JoinSet::new();
        for (i, a) in wave.iter().enumerate() {
            if !selected[i] {
                continue;
            }
            let port = pool_ports[a.slot]; // slot < k = pool_ports.len()（plan_waves 保证）
            let assignment = a.clone();
            let fut = measure(a.slot, port);
            // 起测那一刻的网络代次：测完再取一次，两刻不同即这个值跨了一次网络变化。
            let epoch0 = network_epoch();
            let readback = readback.clone();
            set.spawn(async move {
                let Probed {
                    measured,
                    source_port,
                } = fut.await;
                // 读回在出值之后：等待不计入测量值；各节点在自己的任务里等，不拖住同波的其他节点。
                let record = readback
                    .lookup(&probe_pool_inbound_tag(assignment.slot), source_port)
                    .await;
                (assignment, epoch0, measured, record)
            });
        }
        loop {
            // 与取消竞速：窗口里的节点全挂死时，取消也不必等它们各自走完 6s / 10s。
            // 中断时**等在飞任务真正结束**（`shutdown` = 中止 + 逐个收回）再返回 ⇒ 终态发出时
            // 本轮已没有在飞的测量、它们的 socket 已全部关闭。
            let joined = tokio::select! {
                biased;
                reason = cancel.cancelled() => {
                    set.shutdown().await;
                    return (results, Some(reason));
                }
                joined = set.join_next() => joined,
            };
            let Some(res) = joined else { break };
            // JoinError（panic）→ 该节点无数值，缺席，绝不补 -1。
            let Ok((assignment, epoch0, measured, record)) = res else {
                continue;
            };
            let id = assignment.node_id;
            // ── 让位③（**每节点**测完即查）：在飞期间跃迁 ⇒ 丢弃这一个及其后的在飞值
            //    （量的是新核/已死核，非本轮出口）。已回填的节点是跃迁前量到的真值，保留。
            if let Some(reason) = interrupted(cancel, superseded) {
                set.shutdown().await;
                return (results, Some(reason));
            }
            // 测量跨了一次网络变化 ⇒ 只作废这一个节点（缺席，不是失败），整轮继续；
            // 其后的节点在新代次下起测，身份块带新代次。
            let epoch = network_epoch();
            if network_epoch_changed(epoch0, epoch) {
                log::info!("测速期间网络发生变化，节点 {id} 本轮记为未测");
                continue;
            }
            // 连不上本机探针口 ⇒ 故障在本机一侧，不是这个节点的负面证据：不落账，排回队尾。
            if local_side_halving && measured.is_err_and(|failure| failure.kind == FailKind::Local)
            {
                local_side_error = true;
                if requeued.insert(id.clone()) {
                    if let Some(a) = wave.iter().find(|a| a.node_id == id) {
                        queue.push_back((id, a.tag.clone()));
                    }
                }
                continue;
            }
            let binding = readback.bind(RoutePolicy::SlotPin, Some(&assignment.tag), record);
            if binding.verdict == Some(BindingVerdict::Mismatch) {
                log::warn!(
                    "测速读回不符：槽 {} 请求 {}，实际承载 {}，出站链 {:?}",
                    assignment.slot,
                    assignment.tag,
                    binding.carried_by().unwrap_or_default(),
                    binding.record.as_ref().map(|record| &record.chain),
                );
                // 量到的不是这个节点：数值不属于它，也不是它的负面证据 ⇒ 缺席（原因
                // binding_mismatch），整轮继续。
                if readback.mode == ReadbackMode::Enforce {
                    log::info!("节点 {id} 本轮记为未测：binding_mismatch");
                    trace.mismatch_skipped.insert(id);
                    continue;
                }
            }
            record_measured(
                &mut results,
                &mut tested,
                &mut ok,
                emit,
                &id,
                &measured,
                epoch,
                total,
                Some(&binding),
            );
        }
        if local_side_error {
            let narrowed = (width / 2).max(MIN_WAVE_WIDTH).min(width);
            log::warn!(
                "测速：连接本机探针口失败，波宽由 {width} 减到 {narrowed}（本轮内不再回升，受影响节点不记失败）"
            );
            trace.halved |= narrowed < width;
            width = narrowed;
        }
    }

    (results, None)
}

/// 单个节点的落账 + 推事件（`result` 与 `progress` 成对，计数在此处自增 ⇒ 恒单调）。
///
/// 失败 ⇒ 记 -1（**真实**不可测，阶段与成因随事件带出）。「让位未测」的节点根本
/// 不会走到这里 —— 它们缺席，见 [`drive_pool_waves`] 的三检查点。
#[allow(clippy::too_many_arguments)]
fn record_measured(
    results: &mut serde_json::Map<String, Value>,
    tested: &mut usize,
    ok: &mut usize,
    emit: &mut (dyn FnMut(&str, Value) + Send),
    node_id: &str,
    measured: &Measured,
    network_epoch: Option<u64>,
    total: usize,
    binding: Option<&Binding>,
) {
    if let Err(failure) = measured {
        log::debug!(
            "测速未取得有效延迟：nodeId={node_id} phase={} kind={}",
            failure.phase.as_str(),
            failure.kind.as_str()
        );
    }
    results.insert(
        node_id.to_string(),
        json!(measured.map_or(-1_i64, i64::from)),
    );
    emit(
        EVENT_SPEED_TEST_RESULT,
        result_payload_with_binding(node_id, measured, network_epoch, binding),
    );
    *tested += 1;
    if measured.is_ok() {
        *ok += 1;
    }
    emit(
        EVENT_SPEED_TEST_PROGRESS,
        json!({ "tested": *tested, "ok": *ok, "total": total }),
    );
}

/// 测速目标 URL 求值（单一真值）：用户配的 `speedTestUrl`（须**解析得出隧道目标**）否则
/// [`DEFAULT_SPEED_TEST_URL`]。
///
/// 池路径 / 回退路径 / 出口伴测三处共用同一口径 —— 测速值可跨路径合法比较（同端点 = 同 warm TTFB 语义）。
///
/// **判据是「能否解析成 [`SpeedTestTarget`]」而不是「是否 `http(s)://` 开头」**：CONNECT 腿要的是
/// host/port/path 三件套，`http://` 这种前缀对但解析不出 host 的值若被放行，会让每个节点都拿一个
/// -1 假失败（原因在配置、锅记在节点头上）。对齐 上游 `resolveSpeedTestTarget` 的回落语义。
pub(crate) fn resolve_speed_test_url(config: &Value) -> String {
    config
        .get("speedTestUrl")
        .and_then(Value::as_str)
        .filter(|&u| SpeedTestTarget::parse(u).is_some())
        .map_or_else(|| DEFAULT_SPEED_TEST_URL.to_string(), str::to_string)
}

/// warm-TTFB 计时的**纯时序核**（隧道的建立与 I/O 全部经 [`WarmTunnel`] 注入 ⇒ 「两段各自独立计时、
/// 首段超时不发第二次」这两个结构事实可用假时钟单测，不必碰宿主网络）。
///
/// `open` 建隧道（CONNECT + https 的 TLS 握手），`WarmTunnel::get()` 在**同一条**隧道上发一次 GET，
/// 返回状态码或隧道层的失败种类。任何一段失败都不重连、不重试、不退回冷测量。
///
/// # 两段预算，边界划在 **GET1 之后**
///
/// | 段 | 预算 | 覆盖 |
/// |---|---|---|
/// | 冷建链 | `cold` | `open`（CONNECT + TLS）+ **GET1** |
/// | 复用请求 | `reuse` | **GET2**（= 上报的 measured 值） |
///
/// 边界为什么是 GET1 之后而不是 CONNECT 200 之后（内核先回 200 后拨号，握手落在 GET1 里）、
/// 以及**为什么这不是回到「两个等长计时器」那个病**，见 [`SPEED_TEST_COLD_TIMEOUT_MS`] 的文档。
///
/// ## 🔴 首段超时 ⇒ 立即返回失败，**绝不发第二次**
///
/// 结构保证：第一段的 `timeout_at` 结果经 `?` 早退，第二段的代码在早退之后 ——「首段超时了还继续发
/// GET2」在本函数里**写不出来**，除非把这个 `?` 拆掉。这条直接决定不可达节点的耗时是 6s 而不是 10s
/// （陈先生 2026-07-31 点名：首次超时即判超时，不再浪费资源）。
///
/// 冷段内部分「建隧道」与「预热」两步，**共用同一个截止时刻**（不是各给一份预算）：分两步只为了
/// 让超时能说清卡在哪一步。
///
/// ## 预热（GET1）的返回值必须过检查
///
/// GET1 须拿到可解析的响应头且为 2xx，其响应体须在计时开始前排干净；不满足即判预热失败、不发 GET2。
/// 两条各由一个常量控制（[`WARMUP_MUST_SUCCEED`] / [`WARMUP_BODY_MUST_DRAIN`]），取 `false` 即回到
/// 「GET1 的返回值丢弃」的旧行为。
///
/// **变异锁**：
///  - 两段合用一个预算 → `cold_and_reuse_phases_have_independent_budgets` 转红；
///  - 第二段没有自己的预算（或用了第一段那份）→ `the_reuse_phase_has_its_own_smaller_budget` 转红；
///  - 首段超时后仍发 GET2 → `a_cold_phase_timeout_never_sends_the_second_get` 转红（它数 `get()` 调用次数）；
///  - 把 `open` 挪到计时器之外 → `opening_the_tunnel_spends_the_cold_budget` 转红。
///
/// # 为什么第一次 GET 必须丢弃（不是保险，是必需）
///
/// 内核对 CONNECT 是**先回 200、后拨号**（`sing/protocol/http/handshake.go:89` 写 200 → `:104` 才
/// `NewConnectionEx` 交给路由/出站）⇒ 「收到 200」不蕴含「节点握手已完成」，握手落在**第一次 GET**
/// 的往返里。只发一次 GET 会把握手原样收回 measured，退化成改前 absolute-form 的病。
/// 详见 [`crate::runtime::speedtest_tunnel`] 模块文档。
///
/// 任一段超时 / 传输错 / 非 2xx → 带阶段与成因的失败（上层记 -1，绝不伪造数值；失败不带毫秒数）。
pub(crate) async fn measure_warm_ttfb<T: WarmTunnel>(
    cold: Duration,
    reuse: Duration,
    open: impl Future<Output = Result<T, TunnelError>>,
) -> Measured {
    measure_warm_ttfb_with(
        WARMUP_MUST_SUCCEED,
        WARMUP_BODY_MUST_DRAIN,
        cold,
        reuse,
        open,
    )
    .await
}

/// [`measure_warm_ttfb`] 的本体。两个回退开关作参数传入，使两种取值都能被单测覆盖；
/// 生产只经上面那个入口，取值即两个常量。
async fn measure_warm_ttfb_with<T: WarmTunnel>(
    warmup_must_succeed: bool,
    warmup_body_must_drain: bool,
    cold: Duration,
    reuse: Duration,
    open: impl Future<Output = Result<T, TunnelError>>,
) -> Measured {
    let fail = MeasureFailure::new;
    // ── 第一阶段（冷建链）：CONNECT + TLS + GET1，共用 `cold` 一个截止时刻 ──
    // 建隧道**也在这一段预算内**（`open` 在 `timeout_at` 内部才被 poll）——挪出去就意味着一个
    // CONNECT 挂死的节点能吃掉远超 `cold` 的时间。
    let deadline = Instant::now() + cold;
    let mut tunnel = tokio::time::timeout_at(deadline, open)
        .await
        .map_err(|_| fail(FailPhase::Connect, FailKind::Timeout))?
        .map_err(|e| MeasureFailure::tunnel(FailPhase::Connect, e))?;
    // warm-up：这一次承担节点握手 + 对端冷启动，耗时不上报，但**返回值要过检查**。
    tokio::time::timeout_at(deadline, async {
        let warmed = tunnel.get().await;
        if warmup_must_succeed {
            let code = warmed.map_err(|e| MeasureFailure::tunnel(FailPhase::Warmup, e))?;
            if !is_acceptable_status(code) {
                return Err(fail(FailPhase::Warmup, FailKind::HttpStatus(code)));
            }
        }
        // 没拿到响应头就谈不上排响应体（只在上面那条检查被关掉时才会带着失败走到这里）。
        if warmup_body_must_drain && warmed.is_ok() {
            tunnel
                .drain_body()
                .await
                .map_err(|e| MeasureFailure::tunnel(FailPhase::Warmup, e))?;
        }
        Ok(())
    })
    .await
    .map_err(|_| fail(FailPhase::Warmup, FailKind::Timeout))??; // 🔴 这两个 `?` 就是「首段超时/预热没过 ⇒ 绝不发第二次」的全部实现

    // ── 第二阶段（复用请求）：GET2 = measured，独立的 `reuse` 预算 ──
    // 隧道已热（握手已在第一段付过），这里只量一个往返。
    let t0 = Instant::now();
    let code = tokio::time::timeout(reuse, tunnel.get())
        .await
        .map_err(|_| fail(FailPhase::Measure, FailKind::Timeout))?
        .map_err(|e| MeasureFailure::tunnel(FailPhase::Measure, e))?;
    if !is_acceptable_status(code) {
        return Err(fail(FailPhase::Measure, FailKind::HttpStatus(code)));
    }
    Ok(u32::try_from(t0.elapsed().as_millis()).unwrap_or(u32::MAX))
}

/// 经本机 **http 入站**口对测速 URL 做 warm-TTFB 计时（毫秒）—— **CONNECT 隧道**，不是经代理的
/// absolute-form 请求。
///
/// `proxy_port` 三条生产路径共用（都是本机 http 入站）：主核池 `probe-in-k` / 临时核为该节点建的入站口 /
/// 回退腿的 `mixed-in`。
///
/// 流程：CONNECT 建隧道（非 2xx 即失败）→ https 目标在隧道上 TLS 握手 → **同一条 socket** 上发两次
/// origin-form GET，丢第一次、量第二次到「响应头收齐」。**两段预算**
/// （[`SPEED_TEST_COLD_TIMEOUT_MS`] 包 CONNECT+TLS+GET1，[`SPEED_TEST_REUSE_TIMEOUT_MS`] 包 GET2，
/// 首段超时即返回不发第二次），见 [`measure_warm_ttfb`]；传输面见 [`crate::runtime::speedtest_tunnel`]。
///
/// URL 解析失败 → 失败：`resolve_speed_test_url` 已保证传进来的一定可解析（不可解析的用户值在那里
/// 就回落成默认端点了），故这条腿实际不可达；即便到达也**不伪造数值**（上层记 -1）。
/// 超时 / 传输错 / 非 2xx → 失败（上层记 -1，绝不伪造数值）。
///
/// `auth`：该入站要求的凭据（Android 主核的 `probe-in-k` / `probe-proxy-in`：本次起核的一次性凭据；
/// 桌面主核与临时核：`None`，那些入站零认证）。必填参数：每个调用点在编译期回答「我拿的是哪一份」。
///
/// 返回值连同这条隧道在本机一侧的源端口（读回见证的对账键）。`announce` 在隧道 socket 连上、
/// 尚未发出任何字节时拿到它（向见证登记用）；返回值里的就是那一刻交出去的同一个端口，**隧道
/// 随后没建成也带出**：登记过的连接必须有人去查或撤销，否则登记项会留到轮末。
async fn measure_via_local_proxy(
    proxy_port: u16,
    auth: Option<&InboundUser>,
    url: &str,
    announce: &(dyn Fn(u16) + Sync),
) -> Probed {
    let Some(target) = SpeedTestTarget::parse(url) else {
        return Err(MeasureFailure::new(FailPhase::Connect, FailKind::Rejected)).into();
    };
    let announced = Mutex::new(None);
    let measured = measure_warm_ttfb(
        Duration::from_millis(SPEED_TEST_COLD_TIMEOUT_MS),
        Duration::from_millis(SPEED_TEST_REUSE_TIMEOUT_MS),
        open_tunnel(proxy_port, auth, &target, &|source_port| {
            *announced.lock().unwrap_or_else(PoisonError::into_inner) = Some(source_port);
            announce(source_port);
        }),
    )
    .await;
    Probed {
        measured,
        source_port: announced
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner),
    }
}

/// 经 `inbound` 入站测一次；隧道 socket 一连上就向见证登记这条连接。
async fn measure_inbound(
    readback: &Readback,
    inbound: &str,
    proxy_port: u16,
    auth: Option<&InboundUser>,
    url: &str,
) -> Probed {
    measure_via_local_proxy(proxy_port, auth, url, &|source_port| {
        readback.expect(inbound, source_port);
    })
    .await
}

/// 池腿的一次测量：登记在第 `slot` 槽的入站名下。槽号由驱动函数给，它随后按同一个槽去查。
async fn measure_slot(
    readback: &Readback,
    slot: usize,
    port: u16,
    auth: Option<&InboundUser>,
    url: &str,
) -> Probed {
    measure_inbound(readback, &probe_pool_inbound_tag(slot), port, auth, url).await
}

/// 回退腿与出口伴测共用的一次测量：先限时等见证流建立，再测，出值之后读回。
/// 两段等待都在测量之外，不计入测量值。
async fn measure_system<Meas, MeasFut>(
    probe: &SystemProbe<'_>,
    measure: Meas,
) -> (Measured, Option<ConnRecord>)
where
    Meas: FnOnce() -> MeasFut,
    MeasFut: Future<Output = Probed>,
{
    probe.readback.ready().await;
    let Probed {
        measured,
        source_port,
    } = measure().await;
    let record = probe
        .readback
        .lookup(probe.inbound.tag(), source_port)
        .await;
    (measured, record)
}

/// 出口伴测的测量：只有这一次，读完即关流并等它结束，结果在那之后才发。
async fn companion_measure<Meas, MeasFut>(
    probe: &SystemProbe<'_>,
    measure: Meas,
) -> (Measured, Option<ConnRecord>)
where
    Meas: FnOnce() -> MeasFut,
    MeasFut: Future<Output = Probed>,
{
    let measured = measure_system(probe, measure).await;
    probe.readback.close().await;
    measured
}

// ══════════════════════════════════════════════════════════════════════════════
//  出口伴测（FX-warmttfb）：代理出口 IP 探测成功后补测活跃出口 warm RTT + 广播。
//  对齐 上游 `IpInfoService.onProxyProbeSuccess` → `SpeedTestService.measureWarmRttViaHttpProxy`：
//  切节点 / 首连后出口探测成功那刻**隧道已热** → 量 warm TTFB 广播 → UI 延迟徽标自动刷新
//  （否则切节点后徽标不自动更新）。触发时机 = 探测成功那刻（非切节点瞬刻，防冷隧道虚高）。
//  纯门控 `plan_warm_rtt_probe` 可单测；真数值走真核 = 真机门。
// ══════════════════════════════════════════════════════════════════════════════

/// 出口伴测门控裁定（纯逻辑：探测成功后是否补测活跃出口 warm RTT + 测谁）。
///
/// 四条件**全真**才 fire（对齐 oracle：只在隧道已热、有真实出站时伴测，绝不冷隧道 / 无出口虚高）：
/// - `proxy_probed`：代理出口 IP 探测**探到值**（对齐 上游 `proxyProbed`；探测失败 / 直判无效 → 不测）；
/// - `running`：核在跑（无核 = 无出站可测）；
/// - `proxy_port != 0`：本机 http 代理入站有效（伴测经此口出网；桌面 `mixed-in`，Android `probe-proxy-in`，
///   取自 [`crate::runtime::proxy::ProxyRuntime::local_http_proxy`]）；
/// - active 非空且非直连（[`DIRECT_SERVER_ID`]）：直连 / 未选节点无真实出站，无从伴测。
///
/// 返回 `Some(active_id)`（写 `EVENT_SPEED_TEST_RESULT.serverId` 的键）/ `None`（本轮不测）。
fn plan_warm_rtt_probe(
    proxy_probed: bool,
    running: bool,
    proxy_port: u16,
    active: &str,
) -> Option<String> {
    if !proxy_probed || !running || proxy_port == 0 {
        return None;
    }
    if has_no_real_exit(active) {
        return None;
    }
    Some(active.to_string())
}

/// 出口伴测入口：代理出口探测成功后 **fire-and-forget** 补测活跃出口 warm RTT + 广播（`ipinfo_get` 成功腿尾部调）。
///
/// 门控（[`plan_warm_rtt_probe`]）通过 → [`tauri::async_runtime::spawn`]（不阻塞 ipinfo 返回，保「IP 先显、延迟后到」）
/// 经主混合端口量 warm-TTFB（复用 [`measure_via_local_proxy`]：CONNECT 隧道 + 2×GET 计第二次、剔冷握手，
/// 口径 == 节点测速值）→
/// 成功广播 `EVENT_SPEED_TEST_RESULT{serverId, latency}`（前端 `onSpeedTestResult` 既有通道，零改）。
/// 伴测与手动测速从同一序列领运行号，但号只放在身份块里、事件顶层**不带** `runId`：它不是一轮
/// 前台任务（理由见 [`RunEvents`]）。
///
/// **失败（超时 / 不可达 / 非 2xx → None）不广播**：对齐 oracle `measureWarmRttViaHttpProxy` 返 null 时调用方放弃写入，
/// 保留旧徽标值、绝不伪造 -1（-1 只属用户主动测速的「测了但失败」语义；伴测是被动增益路径，静默保旧值）。
///
/// **不抢 [`SpeedTestGuard`]**：对齐 oracle fire-and-forget 语义 —— 伴测不抢主测速锁，与用户主动全量测速各测各的
/// （每次测量各建**各自的** CONNECT 隧道 = 独立连接，并发不互污 warm 计时）。与主测速偶发并发时容忍，下次探测自愈。
///
/// `epoch` / `seq` = 派生本次伴测的那条出口 IP 探测腿在**开探那一刻**取的世代号与排程线快照；
/// **emit 前复查一次**（见函数体内注释），测量期间换了出口就放弃，绝不把新出口的 RTT 记到 `active_id`
/// 那个旧节点上。两条都要：只查世代时，「更新的腿已排程但还在睡（尚未领号）」这一整个 4s 收敛窗口里
/// 复查恒真 —— 而那正是热切后最容易撞上的窗口（见 `misc::IPINFO_SCHEDULE_SEQ`）。
pub(crate) fn spawn_warm_rtt_probe(
    app: &AppHandle,
    config: &Value,
    proxy_probed: bool,
    running: bool,
    local_proxy: Option<LocalHttpProxy>,
    epoch: u64,
    seq: u64,
) {
    let active = config
        .get("selectedServerId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let proxy_port = local_proxy.as_ref().map_or(0, |p| p.port);
    let Some(active_id) = plan_warm_rtt_probe(proxy_probed, running, proxy_port, active) else {
        return;
    };
    // `plan_warm_rtt_probe` 放行 ⟹ `proxy_port != 0` ⟹ `local_proxy` 为 `Some`。
    let Some(local_proxy) = local_proxy else {
        return;
    };
    let Some(run_id) = next_speed_test_run_id(&SPEED_TEST_RUN_SEQUENCE) else {
        return;
    };
    let request = SpeedTestRequest {
        origin: SpeedTestOrigin::Companion,
        targets: vec![active_id.clone()],
        scope: None,
        path: MeasurePath::of_local_inbound(local_proxy.inbound),
        url: resolve_speed_test_url(config),
    };
    let app = app.clone();
    let proxy = Arc::clone(&app.state::<AppRuntime>().proxy);
    // 读回的取材同样在 await 之前：管理端点与选中节点的 tag 属于开测那一刻的主核。
    let endpoint = proxy.management_endpoint();
    let binding_context = proxy.speed_binding_context();
    let selected_tag = proxy
        .management_target_for(&active_id)
        .map(|(_, _, tag)| tag);
    // 身份基准同样在 await 之前取：世代、起核时刻、配置摘要属于**开测那一刻**的主核。
    let generation = proxy.core_generation();
    let instance = CoreInstance::Main {
        generation,
        start_time: proxy.status().start_time,
    };
    let config_digest = proxy.ready_main_emission_digest(generation);
    tauri::async_runtime::spawn(async move {
        let network_epoch0 = proxy.network_epoch();
        let readback = Readback::open(
            BINDING_READBACK,
            endpoint,
            binding_context,
            WatchedInbounds::One(local_proxy.inbound.tag()),
        );
        let probe = SystemProbe {
            inbound: local_proxy.inbound,
            selected_tag: selected_tag.as_deref(),
            readback: &readback,
            attribution: SYSTEM_PATH_ATTRIBUTION,
        };
        let (measured, record) = companion_measure(&probe, || {
            measure_inbound(
                &readback,
                local_proxy.inbound.tag(),
                local_proxy.port,
                local_proxy.auth.as_ref(),
                &request.url,
            )
        })
        .await;
        // 失败 → 不 emit（保留旧徽标、绝不伪造 -1）；成功 → 广播让 UI 延迟徽标自动刷新。
        if let Ok(latency) = measured {
            // 🔵 **emit 前复查出口 IP 探测上下文**：`active_id` 取自**开探时刻**的 config 快照，而本
            // 测量是异步的（秒级）。测量期间起停 / 热切会换掉出口，此刻的 `latency` 量的是**新**出口，
            // 写进 `active_id` 就是把新节点的 RTT 记到旧节点头上 —— 而延迟徽标是用户选节点的依据，
            // 记错比不记更糟（且错值持久：`latencyMap[旧节点]` 保留到下次测它为止）。
            // 判据两条缺一不可：世代管「已开探的腿谁新」，排程线管「我开探后有没有更新的事件宣告」——
            // 只查世代时，热切后那 4s（新腿已排程、还在睡）复查恒真，正是最容易撞上的窗口。
            // 任一条变了 ⇒ 静默放弃（新出口自己那条腿会带着自己的伴测跑一遍，天然自愈）。
            if !crate::commands::misc::ipinfo_probe_is_current(epoch, seq) {
                return;
            }
            // 身份未变才发布：主核换代 / 已停，或测量跨了一次网络变化 ⇒ 同样静默放弃。
            let network_epoch = proxy.network_epoch();
            if is_superseded(proxy.core_generation(), generation, proxy.status().running)
                || network_epoch_changed(network_epoch0, network_epoch)
            {
                return;
            }
            let payload = companion_result_payload(
                &run_id,
                &request,
                instance,
                config_digest,
                &active_id,
                latency,
                network_epoch,
                &probe,
                record,
            );
            if let Some(payload) = payload {
                let _ = app.emit(EVENT_SPEED_TEST_RESULT, payload);
            }
        }
    });
}

/// 伴测那一条结果事件的载荷（可单测）：既有的 `serverId` / `latency` 加身份块与 `binding` 块，
/// 顶层不带 `runId`。
///
/// 走用户规则的伴测被规则分流到别处时（叶子出站不是选中节点），结果归属「系统路径」本身：
/// 照常入账，但返回 `None`，不写任何节点的延迟。
#[allow(clippy::too_many_arguments)]
fn companion_result_payload(
    run_id: &str,
    request: &SpeedTestRequest,
    instance: CoreInstance,
    config_digest: Option<String>,
    active_id: &str,
    latency: u32,
    network_epoch: Option<u64>,
    probe: &SystemProbe<'_>,
    record: Option<ConnRecord>,
) -> Option<Value> {
    let binding = probe.bind(record);
    let owner = if probe.belongs_to_node(&binding) {
        active_id
    } else {
        log::info!(
            "出口伴测未经由选中节点 {active_id}（实际承载 {}），结果不记入该节点",
            binding.carried_by().unwrap_or("未读到")
        );
        SYSTEM_PATH_ID
    };
    // 池未必在（伴测不依赖探针池）⇒ 与回退腿同一条残留：读不到起核指纹快照，节点指纹留空。
    RunEvents::new(
        run_id,
        request,
        instance,
        config_digest,
        BTreeMap::new(),
        None,
    )
    .stamp(
        EVENT_SPEED_TEST_RESULT,
        result_payload_with_binding(owner, &Ok(latency), network_epoch, Some(&binding)),
    )
}

// ══════════════════════════════════════════════════════════════════════════════
//  结果的消费判据（纯函数）：按什么键存、新旧怎么比、算不算当前、能不能进选点。
//  Rust 侧的结果账本（`runtime::measurement_ledger`）与前端 store 共用这一套；账本本身都是
//  内存态，随进程一起清空。
// ══════════════════════════════════════════════════════════════════════════════

/// 结果的存放键：节点、路径、路由策略、URL 摘要、口径。其中任何一项不同的结果互不覆盖。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ResultKey {
    pub(crate) node_id: String,
    pub(crate) path: MeasurePath,
    pub(crate) policy: RoutePolicy,
    pub(crate) url_digest: String,
    pub(crate) metric: &'static str,
}

pub(crate) fn result_key(node_id: &str, identity: &ResultIdentity) -> ResultKey {
    ResultKey {
        node_id: node_id.to_string(),
        path: identity.path,
        policy: RoutePolicy::of(identity.path, identity.instance),
        url_digest: identity.url_digest.clone(),
        metric: SPEED_TEST_METRIC,
    }
}

/// 同一个键下，新来的结果是否该取代已有的：只接受运行号更大的；运行号相同时只接受序号更大的。
pub(crate) fn supersedes_stored(
    incoming: &ResultIdentity,
    stored: Option<&ResultIdentity>,
) -> bool {
    stored.is_none_or(|stored| (incoming.run, incoming.seq) > (stored.run, stored.seq))
}

/// 判一条结果是否仍然「当前」时的对照面（由消费方在判的那一刻取）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CurrentState<'a> {
    /// 当前主核世代；核不在运行时为 `None`。
    pub(crate) main_generation: Option<u64>,
    /// 该节点当前的参数指纹；配置里已无此节点时为 `None`。
    pub(crate) node_fingerprint: Option<&'a str>,
    /// 当前网络代次；未知时为 `None`。
    pub(crate) network_epoch: Option<u64>,
}

/// 一条结果不再当前的原因。过期结果可保留用于显示，不得用于选点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaleReason {
    /// 测出它的主核已换代，或核已不在运行。
    Instance,
    /// 节点参数在测量之后改过（或节点已不在配置里）。
    NodeFingerprint,
    /// 网络已变（两侧代次都已知且不同）。
    NetworkEpoch,
}

/// 结果是否已过期。`None` = 按这三条判不出过期。
///
/// 「未连接测量」的结果不参与实例比对（它本来就不属于任何主核世代，永远只用于显示）。
/// 网络代次任一方未知时不能据此判定，新鲜度只能靠时间上限。
pub(crate) fn stale_reason(
    identity: &ResultIdentity,
    current: &CurrentState<'_>,
) -> Option<StaleReason> {
    if let CoreInstance::Main { generation, .. } = identity.instance {
        if current.main_generation != Some(generation) {
            return Some(StaleReason::Instance);
        }
    }
    if identity.node_fingerprint.is_some()
        && identity.node_fingerprint.as_deref() != current.node_fingerprint
    {
        return Some(StaleReason::NodeFingerprint);
    }
    if network_epoch_changed(identity.network_epoch, current.network_epoch) {
        return Some(StaleReason::NetworkEpoch);
    }
    None
}

/// 一条结果能否进自动选点的候选：测出了值、经运行中主核的探针槽测得、读回没有判不符、且仍然
/// 当前。走用户规则、钉到选中出口与「未连接测量」的结果在这里被结构性排除。
///
/// 读回未验证的结果仍可选：它的保证与读回引入之前相同。条数见 [`RunEvents`] 的汇总行。
///
/// 读回不符的结果**在任何档位下都不可选**。「只观测」档不改变的是结果的状态：数值照常发布、
/// 照常显示，在账本里算「有当前结果」；但选点判据不取它。「强制」档下这类结果根本不产生。
pub(crate) fn is_selectable(
    measured: &Measured,
    identity: &ResultIdentity,
    current: &CurrentState<'_>,
) -> bool {
    measured.is_ok()
        && RoutePolicy::of(identity.path, identity.instance) == RoutePolicy::SlotPin
        && identity.binding != Some(BindingVerdict::Mismatch)
        && stale_reason(identity, current).is_none()
}

#[cfg(test)]
mod tests;
