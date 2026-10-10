//! C3 自动换节点域：专用出口心跳 → 决策机 → 已加载 clean 候选的零重启热切事务。
//!
//! 决策全在 [`crate::runtime::auto_switch`]（`AutoSwitchMachine` + 纯选择函数，真值表 + 变异锁死）；
//! 本模块只做 I/O 编排与**单一提交事务**：D 侧只改 `selectedServerId`、R 侧只做可证明的 selector
//! 热切，两侧任一不自证即整笔回退（见 [`AutoHotSwitchOutcome`]）。与崩溃恢复解耦——进程崩溃由
//! `spawn_crash_monitor` 原地重启同节点兜底，本腿只对「核活着但代理链不通」换节点。

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::ops::ControlFlow;
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::Value;

use polaris_config_engine::builder::endpoint_routes::mesh_node_carries_full_tunnel;
use polaris_config_engine::builder::hotswitch::{resolve_global_exit_tag, HotSwitchPlan};
use polaris_config_engine::builder::mesh_mode::{selected_mode, MESH_DIRECT};
use polaris_config_engine::singbox::InboundUser;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::is_mesh_node;
use polaris_helper_proto::Platform;
use polaris_switch_engine::{HotSwitchOutcome, SwitchDecision, SwitchExecutor};

use crate::commands::speedtest::{
    current_server_fingerprints, probe_runtime_candidates, resolve_speed_test_url,
    speed_test_data_settled, speed_test_in_flight, RuntimeProbeBatch,
};
use crate::runtime::auto_select::{
    self, Cause, CommitFailure, Decision as SelectDecision, Epoch, Evaluation, Facts, Intent, Leg,
    Starved, StatusInputs, SwitchRecord, Switches,
};
use crate::runtime::auto_switch::{
    decide_tick, judge_probes, plan_runtime_candidates, switch_blocked_by_restart, switch_payload,
    AutoSwitchMachine, HeartbeatOutcome, ProbeVerdict, RuntimeCandidate, RuntimeCandidatePlan,
    SwitchGate, TickAction, TickInput, CONNECTIVITY_TIMEOUT_MS, CONNECTIVITY_URLS,
    HEARTBEAT_INTERVAL_MS,
};
use crate::runtime::config::Decision;
use crate::runtime::measurement_ledger::{
    self, Candidate, Candidates, LedgerEntry, MeasurementLedger,
};
use crate::runtime::measurement_scheduler::{
    self, entry_freshness_cap_ms, plan_subscriptions, Idle, LedgerView, Pause, Signals, SubPlan,
    Verdict,
};
use crate::runtime::subscription_scheduler::now_ms;
use crate::runtime::tailscale_status::TailscaleStatusEvent;

use super::hot_switch::{selected_server_present, ClassifiedSwitch, RuntimeSelectionApi};
use super::lifecycle::monotonic_now_ms;
use super::{code, ProxyRuntime};

/// 「自动切换落空、只能请用户手动换节点」的诊断文案 —— [`ProxyRuntime::do_switch_io`] 的两个上报点
/// 共用一份，防两处措辞漂移。**用户可见文案不由它决定**：渲染端按稳定码
/// [`code::AUTO_SWITCH_NEEDS_RESTART`] 选 locale（`ui/src/domain/proxy-error-text.ts`），本串只进
/// 日志与 wire 诊断字段。
///
/// 措辞刻意不说「可用的节点」：被剔掉的那些候选**一个都没被探测过**（剔除发生在探测之前），
/// 「可用」是未核实的断言。也刻意不建议「重启代理」：走到上报点时 D 与 R 的选中节点已确证相同，
/// 同配置重启只会世代 +1、锁存复位、同情形再报一次 —— 用户照做就进循环。
const RESTART_BLOCKED_MESSAGE: &str =
    "自动切换已触发，但本轮未能换成节点：有候选节点需要重启内核才能切换过去";

pub(super) fn heartbeat_mode_blocked(
    startup_blocked: bool,
    dynamic_mesh_id: Option<&str>,
    running_id: Option<&str>,
    reconciliation_required: bool,
) -> bool {
    match dynamic_mesh_id {
        Some(mesh_id) => running_id.is_none_or(|id| id == mesh_id) || reconciliation_required,
        None => startup_blocked,
    }
}

/// 自动故障切换的热切事务结果。它刻意没有 `Restarting`：后台故障治理只允许操作当前运行核已加载的
/// clean selector 成员；管理 API 不可用就失败，不得借一次整核重启把 D 中其他待 Apply 修改带进去。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AutoHotSwitchOutcome {
    Applied,
    Busy,
    Superseded,
    NotEligible,
    /// D 已提交目标，但目标与旧 selector 均无法自证；后台受限对账 actor 已获恢复所有权。
    ReconcilePending {
        intent_generation: u64,
    },
    Failed,
}

/// 一个候选「切过去」的三态裁定（[`ProxyRuntime::candidate_switch_plan`] 的产物）。
///
/// **三态而不是 `Option`**：`Option::None` 会把「这个候选切过去要整核重启」和「本轮根本无从判定」
/// 压成同一个值。后者（核已停 / 基准过期）是**运行时**的属性，对每个候选都成立 ⇒ 压扁之后停核
/// 窗口里「全部候选都需要重启」，于是往刚被 `ProxyStatus::default()` 清零的状态里写一条非致命
/// 错误，被渲染端判成终态故障 —— 而用户刚点的是停止。
pub(super) enum CandidateSwitchPlan {
    /// 可零重启热切过去；三个载荷交给提交腿直接消费。
    HotSwitchable {
        switched: Value,
        plan: HotSwitchPlan,
        new_cfg: Box<UserConfig>,
    },
    /// 切过去必须整核重启：route 投影 guard（force-route engaged / 全隧道兜底翻转）、目标未入
    /// selector、目标 dirty、TUN 未规划的自动物理出口。**这一桶是上报判据的唯一区分项**。
    NeedsRestart,
    /// 本轮无从判定（核已停、运行核基准已过期）。整轮作废，一个候选都不许记账。
    Void,
}

impl ProxyRuntime {
    /// 候选**能否零重启切过去**的单一判据：拿运行核基准配置 clone 一份、把 `selectedServerId` 换成
    /// 候选 id，重跑 [`Self::classify_switch`]；要求落在热切腿，且 `puts` 里确有一条
    /// `proxy-selector → 候选 tag`。
    ///
    /// [`HotSwitchable`](CandidateSwitchPlan::HotSwitchable) 的三个载荷都是判定的**副产物**，
    /// 交给提交腿直接消费（它要把 `switched` 提交为 R 的新 `current_config`、按 `plan` 执行 PUT、
    /// 从 `new_cfg` 读 `interrupt_connections_on_switch` 与出口路由对账入参）。不这么交、让提交腿
    /// 自己再算一遍，就等于又开了一个可与判定分歧的读点。
    ///
    /// # 为什么两个消费点共用它，而不是各写一份
    ///
    /// 消费点是[提交事务](Self::auto_hot_switch_transaction_with_api)（非热切 ⇒ `NotEligible`）与
    /// [探测前的候选剔除](Self::retain_hot_switchable_candidates)（非热切 ⇒ 根本不探）。两处判据**必须
    /// 逐字相同**：探测前放行、提交时才拒 = 整轮全量探测白跑，且 `do_switch_io` 返 false 不计熔断 ⇒
    /// 约 90 秒后原样重来一轮，永不成功 —— 而那正是用户真需要切换的时刻。
    ///
    /// 另立一份翻转谓词还会漏：切不过去不止来自 `plan_hot_switch` 的 route 投影 guard，
    /// `classify_switch` 自己的 TUN 绑定根 `Fallback`（`hot_switch.rs` 的
    /// 「目标包含本核未规划的自动物理出口」）同样会让候选切不过去，而那条腿在 config-engine 里没有
    /// 对应谓词可抄。共用整条判定链是唯一覆盖得全的做法。
    ///
    /// 仓内既定做法就是**复用而非复写**：`classify_staged`（`hot_switch.rs`，理由写在它头注的
    /// 「预告与实际在构造上不可能分歧」）直接复用 `classify_switch` 而不另写一份预告判据，
    /// 本方法是同一条推理在候选剔除上的应用。
    ///
    /// 代价如实记：剔除腿会对每个候选各跑一次完整判定（两次 `UserConfig` 解析 + 两次 norm +
    /// 一次 `plan_hot_switch`）。它比它替掉的东西便宜若干个数量级 —— 替掉的是同一批候选的**真实
    /// 协议链探测**（K 槽分波、每槽 CONNECT + warm-TTFB，含超时），且整条腿每 30 秒才可能触发一次。
    pub(super) fn candidate_switch_plan(
        &self,
        runtime_config: &Value,
        candidate: &RuntimeCandidate,
    ) -> CandidateSwitchPlan {
        let mut switched = runtime_config.clone();
        let Some(object) = switched.as_object_mut() else {
            return CandidateSwitchPlan::Void;
        };
        object.insert(
            "selectedServerId".to_string(),
            Value::String(candidate.id.clone()),
        );
        match self.classify_switch(&switched, false) {
            // 核已停 —— 这是**运行时**的属性，对每个候选都一样，不是「这个候选要重启」。
            ClassifiedSwitch::NotRunning => CandidateSwitchPlan::Void,
            // 候选恒 ≠ 当前出口（`plan_runtime_candidates` 保证），故「与运行核配置逐字节全等」
            // 只可能是运行核在本轮读到基准之后被换掉了 ⇒ 本轮的基准已过期。
            ClassifiedSwitch::Unchanged => CandidateSwitchPlan::Void,
            // 能走到这里的 `Fallback` 只剩「目标包含本核未规划的自动物理出口」，那确实要整核重启。
            // 另外三条都不是候选的属性，各有整轮判掉的地方：无 `current_config` 基准由调用方
            // [`Self::do_switch_io`] 取基准时挡下；解析失败与无热切基准快照由
            // [`Self::retain_hot_switchable_candidates`] 在循环外挡下（理由见那里）。
            ClassifiedSwitch::Fallback(_) => CandidateSwitchPlan::NeedsRestart,
            ClassifiedSwitch::Decided {
                decision: SwitchDecision::HotSwitch(plan),
                new_cfg,
            } => {
                let cross_mode = self
                    .switch_snapshot
                    .read()
                    .ok()
                    .and_then(|guard| guard.as_ref().map(|snapshot| snapshot.mesh_mode_ready))
                    .unwrap_or(false)
                    && serde_json::from_value::<UserConfig>(runtime_config.clone())
                        .ok()
                        .is_some_and(|old| selected_mode(&old) != selected_mode(&new_cfg));
                if cross_mode {
                    return CandidateSwitchPlan::NeedsRestart;
                }
                let puts_the_candidate = plan.puts.iter().any(|put| {
                    put.selector_tag == "proxy-selector" && put.member_tag == candidate.tag
                });
                if puts_the_candidate {
                    CandidateSwitchPlan::HotSwitchable {
                        switched,
                        plan,
                        new_cfg,
                    }
                } else {
                    CandidateSwitchPlan::NeedsRestart
                }
            }
            ClassifiedSwitch::Decided { .. } => CandidateSwitchPlan::NeedsRestart,
        }
    }

    /// 探测**之前**把切不过去的候选剔掉并**分两个桶**记账；返回 `false` = 本轮无从判定，
    /// 调用方必须直接早退且**不做任何上报**。
    ///
    /// 两个桶的分界就是「这件事该不该告诉用户」：
    ///  - [`not_exit`](RuntimeCandidatePlan::not_exit)：候选**自身**是只走内网的组网节点
    ///    （`is_mesh_node && !mesh_node_carries_full_tunnel` —— TS 无 exit node / WG 关
    ///    `allowInternet` / OpenVPN 关 `redirect_gateway` 且 `meshRoutes` 非空）。自动切换永远不该
    ///    切到它：切过去公网流量就整体兜底直连，那是把用户静默推去明文直连，不是「换了个能用的
    ///    节点」。静默排除，与 TS 未就绪的 `not_ready` 同类，**不入上报判据**。
    ///  - [`needs_restart`](RuntimeCandidatePlan::needs_restart)：判据即
    ///    [`Self::candidate_switch_plan`]，与提交事务同一份。装的是真的「切过去要整核重启」者。
    ///
    /// 返回 `false` 的三个来源全是**运行时**属性、不是候选属性：运行核基准解析不了、任一候选判到
    /// [`CandidateSwitchPlan::Void`]、剔除期间世代跃迁或核停（末尾复查 —— 停在最后一个候选判完
    /// 之后，前面那些裁定虽各自成立，此刻也已无人需要）。把它们记进 `needs_restart` 就是把
    /// 「用户刚点了停止」报成「自动切换帮不上忙」。
    ///
    /// **射程自曝**：末尾那道复查**没有行为门** —— 要构造「循环最后一个候选判完之后才停核」得在
    /// 本方法内部插手，本机造不出来。停核窗口整体与 per-candidate 那条腿各有门
    /// （`stop_window_voids_the_round_…` / `stale_runtime_baseline_voids_the_round_…`）。
    ///
    /// # 当前出口本身是只走内网的组网节点时
    ///
    /// 那时**每个**普通候选都不可热切（方向相反、同样要重下发段规则），于是全员进 `needs_restart`。
    /// 这条腿在**生产上不可达**：心跳层的停摆守卫
    /// （[`auto_switch_blocked_for_generation`](crate::runtime::auto_switch::auto_switch_blocked_for_generation)）
    /// 用的正是同一个谓词，那样的世代压根不会跑到这里；只有单测直接调本方法时可达
    /// （`mesh_only_current_exit_drops_every_plain_candidate`）。保留是因为本方法的正确性不该
    /// 依赖调用方的守卫。
    ///
    /// 之所以不落在 `plan_runtime_candidates` 里：那是只看 `&Value` 的纯函数，而本判据要
    /// `UserConfig` 与运行核快照。
    pub(super) fn retain_hot_switchable_candidates(
        &self,
        generation: u64,
        runtime_config: &Value,
        plan: &mut RuntimeCandidatePlan,
    ) -> bool {
        let Ok(runtime_cfg) = serde_json::from_value::<UserConfig>(runtime_config.clone()) else {
            return false;
        };
        // 热切基准快照缺失是**运行时**属性，对每个候选都一样。不在这里整轮判掉的话，
        // `classify_switch` 会对每个候选各返一次 `Fallback("无热切换基准快照")` ⇒ 全部计入
        // `needs_restart` ⇒ 候选清零 + `needs_restart > 0` ⇒ 向用户误报一次「需手动切换」，
        // 而真相是本轮压根无从判断。与上面那条解析失败同理，作废整轮。
        if self
            .switch_snapshot
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .is_none()
        {
            return false;
        }
        let mut kept: Vec<RuntimeCandidate> = Vec::with_capacity(plan.candidates.len());
        let mut not_exit = 0usize;
        let mut needs_restart = 0usize;
        for candidate in &plan.candidates {
            let mesh_only = runtime_cfg
                .servers
                .iter()
                .find(|server| server.id == candidate.id)
                .is_some_and(|server| {
                    is_mesh_node(server) && !mesh_node_carries_full_tunnel(server)
                });
            if mesh_only {
                not_exit += 1;
                continue;
            }
            match self.candidate_switch_plan(runtime_config, candidate) {
                CandidateSwitchPlan::HotSwitchable { .. } => kept.push(candidate.clone()),
                CandidateSwitchPlan::NeedsRestart => needs_restart += 1,
                CandidateSwitchPlan::Void => return false,
            }
        }
        if self.core_generation() != generation || !self.core_running() {
            return false;
        }
        plan.candidates = kept;
        plan.not_exit = not_exit;
        plan.needs_restart = needs_restart;
        true
    }

    /// 自动故障切换的单一提交事务：D 只改 `selectedServerId`，R 只做可证明的 selector 热切。
    ///
    /// 与普通 `switch_mode` 的关键差异是**禁止失败回退整核重启**。普通用户 Apply 的目标就是把完整 D
    /// 入核，失败回退重启正确；后台 failover 只获授权切出口，若沿用同一回退会把 DNS/TUN/规则等
    /// 已保存未 Apply 的修改一起带入核，破坏 Save/Apply 边界。
    pub(super) async fn auto_hot_switch_transaction(
        self: &Arc<Self>,
        generation: u64,
        expected_current_id: &str,
        candidate: &RuntimeCandidate,
        expected_candidate_fingerprint: &str,
        required_auto: Option<&str>,
    ) -> AutoHotSwitchOutcome {
        let api = self.management_api().await;
        self.auto_hot_switch_transaction_with_api(
            generation,
            expected_current_id,
            candidate,
            expected_candidate_fingerprint,
            required_auto,
            &api,
        )
        .await
    }

    /// 可注入管理面的事务本体。所有 generation/lifecycle/config CAS 均在拿到 `switch_serial` 后重验，
    /// 因此生产侧在锁外建立 lazy gRPC channel 不会把陈旧客户端变成一次陈旧提交。
    ///
    /// `required_auto`：这次换点是凭「自动选择意图指向该订阅」发起的（择优腿，或自动意图下收窄了
    /// 候选的故障腿）。落盘那一刻意图已不是它即让位：用户在评估与提交之间点了节点（哪怕点的就是
    /// 当前出口，实际出口没变、只有意图变了），这次换点已无授权。手动意图下的故障腿传 `None`。
    pub(super) async fn auto_hot_switch_transaction_with_api(
        self: &Arc<Self>,
        generation: u64,
        expected_current_id: &str,
        candidate: &RuntimeCandidate,
        expected_candidate_fingerprint: &str,
        required_auto: Option<&str>,
        api: &dyn RuntimeSelectionApi,
    ) -> AutoHotSwitchOutcome {
        let switch_guard = self.switch_serial.lock().await;
        if self.gate.generation() != generation || !self.core_running() {
            return AutoHotSwitchOutcome::Superseded;
        }
        let starting_intent_generation = self.selector_reconcile.intent_generation();
        if self.gate.is_busy() {
            return AutoHotSwitchOutcome::Busy;
        }
        if self.selector_reconcile.is_required() {
            return AutoHotSwitchOutcome::NotEligible;
        }
        let staged = self.config.staged_node_mask();
        if staged.pending
            && (!staged.scope_known || staged.node_ids.contains(candidate.id.as_str()))
        {
            return AutoHotSwitchOutcome::Superseded;
        }

        let Some(old_runtime) = self
            .current_config
            .read()
            .ok()
            .and_then(|guard| guard.clone())
        else {
            return AutoHotSwitchOutcome::NotEligible;
        };
        if old_runtime.get("selectedServerId").and_then(Value::as_str) != Some(expected_current_id)
        {
            return AutoHotSwitchOutcome::Superseded;
        }
        // 旧出口的 selector 成员 tag：节点查起核时的 id→tag 表，直连哨兵是 `direct`（恒为成员）；
        // 阻断没有成员 tag，解析不到即不具备后台事务的条件。
        let old_tag = self.switch_snapshot.read().ok().and_then(|guard| {
            guard.as_ref().and_then(|snapshot| {
                resolve_global_exit_tag(Some(expected_current_id), Some(&snapshot.id_to_tag))
            })
        });
        let Some(old_tag) = old_tag else {
            return AutoHotSwitchOutcome::NotEligible;
        };
        // The background transaction only drives selector PUTs. It must never claim a
        // cross-mode TS→ordinary switch, or commit atop mode/dashboard drift left by a
        // superseded manual intent. The manual transaction owns mode changes and restart.
        let dual_mode = self
            .switch_snapshot
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|snapshot| snapshot.mesh_mode_ready))
            .unwrap_or(false);
        if dual_mode {
            let old_cfg = serde_json::from_value::<UserConfig>(old_runtime.clone()).ok();
            if old_cfg
                .as_ref()
                .is_none_or(|config| selected_mode(config) == MESH_DIRECT)
                || !self.mesh_live_state_matches_current(generation).await
            {
                return AutoHotSwitchOutcome::NotEligible;
            }
        }

        // 资格判定与 `do_switch_io` 的探测前剔除**同一份**（见 [`Self::candidate_switch_plan`]）：
        // 两处分歧的形态是「探了一整轮、提交时才拒、不计熔断、90 秒后重来」。三态里只有
        // `HotSwitchable` 放行，另两态同归 `NotEligible`（本腿不区分「要重启」与「判不了」：
        // 两者都是不许在这里提交，而重试由下一轮心跳负责）。
        let CandidateSwitchPlan::HotSwitchable {
            switched: new_runtime,
            plan: hot_plan,
            new_cfg,
        } = self.candidate_switch_plan(&old_runtime, candidate)
        else {
            return AutoHotSwitchOutcome::NotEligible;
        };
        let new_cfg = *new_cfg;

        // 在持有 switch_serial 时原子复核并只写 selectedServerId。其它 writer 仍可先写盘，但其广播会
        // 排在本事务之后；闭包再次核对当前选择、候选仍存在且连接指纹未变，任何一项漂移即让位。
        let target_id = candidate.id.clone();
        let expected_fingerprint = expected_candidate_fingerprint.to_string();
        let persisted = self.config.update(|latest| {
            // 显式用户选择也在同一个 ConfigManager 写事务内 bump；因此本检查到 claim 之间没有
            // “同目标但更新意图”可穿过。只比较 selectedServerId 无法分辨这种所有权交接。
            if self.selector_reconcile.intent_generation() != starting_intent_generation {
                return Decision::Skip(None);
            }
            if required_auto.is_some_and(|subscription| {
                auto_select::effective_subscription(
                    latest,
                    Switches::PRODUCTION,
                    self.helper.platform(),
                ) != Some(subscription)
            }) {
                return Decision::Skip(None);
            }
            if latest.get("selectedServerId").and_then(Value::as_str) != Some(expected_current_id)
                || current_server_fingerprints(latest).get(&target_id)
                    != Some(&expected_fingerprint)
            {
                return Decision::Skip(None);
            }
            let Some(object) = latest.as_object_mut() else {
                return Decision::Skip(None);
            };
            let intent_generation = self.register_selector_intent();
            object.insert(
                "selectedServerId".to_string(),
                Value::String(target_id.clone()),
            );
            Decision::Write(Some(intent_generation))
        });
        let intent_generation = match persisted {
            Ok((Some(intent_generation), Some(_))) => intent_generation,
            Ok((None, None)) => return AutoHotSwitchOutcome::Superseded,
            Ok(_) => unreachable!("auto failover persistence decision must agree"),
            Err(error) => {
                log::warn!("自动故障切换：保存目标出口失败：{error}");
                return AutoHotSwitchOutcome::Failed;
            }
        };

        let interrupt = new_cfg.interrupt_connections_on_switch == Some(true);
        let applied_disconnects = match SwitchExecutor.execute(api, &hot_plan, interrupt).await {
            HotSwitchOutcome::Applied { disconnect } => {
                Some(disconnect.map_or(0, |result| result.closed_ids.len()))
            }
            other => {
                log::warn!("自动故障切换：selector 热切失败（{other:?}），禁止回退整核重启");
                None
            }
        };
        if !self.selector_operation_is_current(generation, intent_generation) {
            self.selector_reconcile.mark_required();
            log::info!("自动故障切换：target PUT 后 selector 所有权已交接 → 不恢复、不回滚 D");
            return AutoHotSwitchOutcome::Superseded;
        }

        // PUT 回执不是最终真值：读回 sing-box 当前 group，只有实际指向目标成员才提交成功事件。
        let groups_after_target = if applied_disconnects.is_some() {
            api.groups_snapshot().await.ok()
        } else {
            None
        };
        if !self.selector_operation_is_current(generation, intent_generation) {
            self.selector_reconcile.mark_required();
            log::info!("自动故障切换：target 自证后 selector 所有权已交接 → 不恢复、不回滚 D");
            return AutoHotSwitchOutcome::Superseded;
        }
        let attested = groups_after_target.is_some_and(|groups| {
            groups
                .iter()
                .any(|group| group.tag == "proxy-selector" && group.selected == candidate.tag)
        });
        // PUT 在 await 期间配置 writer 仍可前进；因此自证不只读 selector，还要再次核对目标指纹与
        // staged 遮罩。候选若恰被订阅替换/用户编辑，哪怕 PUT 已成功也必须恢复旧出口，不能把运行核
        // 里的旧参数成员冒充成磁盘里的新节点。
        let staged_after = self.config.staged_node_mask();
        let target_still_clean = !(staged_after.pending
            && (!staged_after.scope_known
                || staged_after.node_ids.contains(candidate.id.as_str())))
            && self
                .config
                .with_current(|latest| {
                    latest.get("selectedServerId").and_then(Value::as_str)
                        == Some(candidate.id.as_str())
                        && current_server_fingerprints(latest).get(&candidate.id)
                            == Some(&expected_fingerprint)
                })
                .unwrap_or(false);
        if attested && target_still_clean {
            // 只有管理面回读与配置 CAS 双重自证后，才把候选提交为 R 的真实选择并刷新依赖出口的
            // 派生状态。PUT 成功但随后回滚的瞬态不应污染 current_config / 解锁 / 出口 IP 缓存。
            self.commit_applied(&new_runtime);
            self.mesh
                .exit_route_reconcile(&new_cfg, new_cfg.enable_ipv6.unwrap_or(false))
                .await;
            if !self.selector_operation_is_current(generation, intent_generation) {
                log::info!("自动故障切换：出口路由对账期间 selector 所有权已交接 → 抑制旧成功通知");
                return AutoHotSwitchOutcome::Superseded;
            }
            self.invalidate_unlock_cache(true, false);
            self.schedule_exit_ip_refresh(true);
            self.reconcile_login_fallback_locked(&switch_guard).await;
            if !self.selector_operation_is_current(generation, intent_generation) {
                return AutoHotSwitchOutcome::Superseded;
            }
            self.push_pending_changes();
            log::info!(
                "自动故障切换：selector 已热切并通过运行态回读，精准断连 {} 条",
                applied_disconnects.unwrap_or_default()
            );
            return AutoHotSwitchOutcome::Applied;
        }

        // 未自证成功则 best-effort 恢复旧 selector。恢复也只走管理 API，不以重启“兜底”。只有运行态
        // 确认回到旧成员后才把 D 的 selectedServerId 回滚，避免盘面声称旧节点而核仍实际指向新节点。
        log::warn!("自动故障切换：目标 selector 未通过运行态回读，尝试恢复原出口");
        let restore_put_ok = api
            .select_outbound("proxy-selector", &old_tag)
            .await
            .is_ok();
        if !self.selector_operation_is_current(generation, intent_generation) {
            self.selector_reconcile.mark_required();
            log::info!("自动故障切换：restore PUT 后 selector 所有权已交接 → 禁止回滚 D");
            return AutoHotSwitchOutcome::Superseded;
        }
        let groups_after_restore = if restore_put_ok {
            api.groups_snapshot().await.ok()
        } else {
            None
        };
        if !self.selector_operation_is_current(generation, intent_generation) {
            self.selector_reconcile.mark_required();
            log::info!("自动故障切换：restore 自证后 selector 所有权已交接 → 禁止回滚 D");
            return AutoHotSwitchOutcome::Superseded;
        }
        let restored = groups_after_restore.is_some_and(|groups| {
            groups
                .iter()
                .any(|group| group.tag == "proxy-selector" && group.selected == old_tag)
        });
        if restored {
            let rollback_target = candidate.id.clone();
            let old_id = expected_current_id.to_string();
            let _ = self.config.update(|latest| {
                if self.selector_reconcile.intent_generation() != intent_generation {
                    return Decision::Skip(false);
                }
                if latest.get("selectedServerId").and_then(Value::as_str)
                    != Some(rollback_target.as_str())
                {
                    return Decision::Skip(false);
                }
                let Some(object) = latest.as_object_mut() else {
                    return Decision::Skip(false);
                };
                object.insert(
                    "selectedServerId".to_string(),
                    Value::String(old_id.clone()),
                );
                Decision::Write(true)
            });
            self.push_pending_changes();
        } else {
            let message = "自动故障切换未能确认目标出口，恢复原出口也失败；正在后台对账运行出口";
            self.set_nonfatal_error(message, code::EXIT_MISMATCH);
            self.push_pending_changes();
            log::error!("{message}");
            return AutoHotSwitchOutcome::ReconcilePending { intent_generation };
        }
        if !self.selector_operation_is_current(generation, intent_generation) {
            AutoHotSwitchOutcome::Superseded
        } else {
            AutoHotSwitchOutcome::Failed
        }
    }

    // ════════════════ C3：自动换节点（节点不可达 → 已加载 clean 候选零重启热切）════════════════
    //
    // 决策全在 [`AutoSwitchMachine`] + 纯选择函数（`runtime/auto_switch.rs`，真值表 + 变异锁死）；
    // 本层只做「专用出口心跳 → 喂决策机 → 复用主核探测池 → selector 热切并回读 → emit」的 I/O。
    // **与崩溃恢复解耦**：
    // 进程崩溃由 `spawn_crash_monitor` 原地重启同节点兜底，本腿只对「核活着但代理链不通」换节点
    // （1:1 移植 上游 AutoSwitchService 的职责边界）。
    //
    // 当前出口经 `probe-proxy-in → proxy-selector` 钉死，不受用户流量规则影响；候选经
    // `probe-in-k → probe-selector-k` 的 CONNECT+warm-TTFB 验证完整协议链。两者均真碰宿主网络，
    // 编排/资格/结果状态用离线测试覆盖，真实数值留 bundled-core 门。

    /// **C3**：核就绪后挂自动换节点心跳（`spawn_tailscale_status_relay` 的世代范式）。
    ///
    /// **无条件挂**（与崩溃监测同接线点），开关在循环内每 tick 读原始配置 `autoSwitchNode` 动态判
    /// （对齐 上游 config-change-handler 的运行期 enable/disable，轮询版——避免动命令层加事件驱动，
    /// 本批禁区 commands/config.rs）。**世代守卫**：核被停/接管（stop/restart 先 bump 世代）→ 退场，
    /// 绝不让旧核的心跳污染新核（探测/切换均先复查世代）。
    ///
    /// `generation_blocked` 是单态核的启动判据。Android 双态核另传唯一无出口 TS id；每 tick
    /// 从已提交的 R 选中 id 覆写该判据，避免同世代热切后探错出口或永久停摆。不会读磁盘 D。
    ///
    /// 每 tick 的分支裁决在纯函数 [`decide_tick`]（真值表 + 变异锁死）；本方法只做「睡 → 查世代 →
    /// 同步开关 → 喂裁决 → 执行 I/O」。
    pub(super) fn spawn_auto_switch_heartbeat(
        self: &Arc<Self>,
        my_gen: u64,
        probe_proxy_port: Option<u16>,
        loopback_auth: Option<InboundUser>,
        generation_blocked: bool,
        dynamic_mesh_id: Option<String>,
    ) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let machine = AutoSwitchMachine::new();
            let tick = Duration::from_millis(HEARTBEAT_INTERVAL_MS);
            log::debug!("自动换节点心跳起（世代 {my_gen}，专用出口探针端口={probe_proxy_port:?}）");
            // 世代常量、**只打一行**（不是每 tick）：停摆是本世代的固定事实，重复播报无新信息。
            if generation_blocked {
                log::info!(
                    "自动换节点本世代不工作：选中的组网节点已关闭外网访问，用户流量整体兜底直连\
                     （而心跳探针恒钉死走 proxy-selector，探的不是用户在走的那条路）"
                );
            }
            me.auto_select_log_generation(my_gen);
            let mut legs = LiveLegs {
                seen: measurement_ledger::global().version(),
                rt: Arc::clone(&me),
                machine,
                my_gen,
                probe_proxy_port,
                loopback_auth,
                generation_blocked,
                dynamic_mesh_id,
                cache: SelectCache::default(),
            };
            run_heartbeat(&mut legs, tick, &me.auto_select_wake).await;
        });
    }

    /// 故障腿的一拍：同步开关 → 裁决 → 连通性探测 → 喂决策机。`Break` = 探测期间核已被接管，
    /// 心跳任务退场。
    async fn failover_beat(
        self: &Arc<Self>,
        machine: &mut AutoSwitchMachine,
        my_gen: u64,
        probe_proxy_port: Option<u16>,
        loopback_auth: Option<&InboundUser>,
        generation_blocked: bool,
        dynamic_mesh_id: Option<&str>,
    ) -> ControlFlow<()> {
        // 动态开关（上游 config-change-handler，轮询版）：autoSwitchNode 真才启用。自动选择意图
        // 生效时视同启用：自动选择的承诺包含「坏了会换走」，不看那个开关。
        let want_enabled = self.auto_switch_enabled() || self.auto_select_active();
        if want_enabled && !machine.is_enabled() {
            machine.enable();
            log::info!("自动换节点已启用（应用层连通性检测）");
        } else if !want_enabled && machine.is_enabled() {
            machine.disable();
            log::info!("自动换节点已禁用");
        }
        // 分支裁决全在纯函数（各腿的理由与「复位/不复位」的分界见 [`decide_tick`] 文档）。
        // 两处运行态在此**无条件求值**：`decide_tick` 的优先级保证前几道拦下时它们的值不被读到，
        // 求值本身则各是一次持锁投影（无深拷贝，同 `auto_switch_enabled` 已有的每 tick 读），
        // 未改任何决策语义。
        // A dual-mode Android core can change its selected exit without a new core
        // generation. Use committed R, not disk D, for the split-only TS guard.
        let running_id = self.current_config.read().ok().and_then(|guard| {
            guard.as_ref().and_then(|config| {
                config
                    .get("selectedServerId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        });
        let currently_blocked = heartbeat_mode_blocked(
            generation_blocked,
            dynamic_mesh_id,
            running_id.as_deref(),
            self.selector_reconcile.is_required(),
        );
        let probe_proxy_port = match decide_tick(TickInput {
            enabled: machine.is_enabled(),
            switching: machine.is_switching(),
            core_running: self.core_running(),
            selected_server_is_real: self.selected_server_is_real(),
            generation_blocked: currently_blocked,
            probe_proxy_port,
        }) {
            TickAction::Skip(_) => return ControlFlow::Continue(()),
            TickAction::SkipAfterResettingFailures(_) => {
                machine.reset_failures_only();
                return ControlFlow::Continue(());
            }
            TickAction::Probe { probe_proxy_port } => probe_proxy_port,
        };
        // 应用层连通性探测（真机门：真起核 + 碰网络）。
        // `loopback_auth` 与端口同为世代常量（本次起核生成、随本心跳任务一起退场）。
        let alive = probe_proxy_connectivity(probe_proxy_port, loopback_auth).await;
        // 探测耗时窗口内可能已被接管 → 复查世代。
        if self.gate.generation() != my_gen {
            return ControlFlow::Break(());
        }
        match machine.on_heartbeat(alive) {
            HeartbeatOutcome::Trigger => {
                log::warn!(
                    "连通性连续 {} 次失败 → 触发自动换节点",
                    crate::runtime::auto_switch::MAX_CONSECUTIVE_FAILURES
                );
                self.run_auto_switch(machine, "connectivity").await;
            }
            HeartbeatOutcome::Recovered { prior } => {
                log::info!("连通性恢复正常（此前连续失败 {prior} 次）");
            }
            HeartbeatOutcome::Failing { failures } => {
                log::warn!(
                    "连通性检测失败 [{failures}/{}]",
                    crate::runtime::auto_switch::MAX_CONSECUTIVE_FAILURES
                );
            }
            HeartbeatOutcome::Stable => {}
        }
        ControlFlow::Continue(())
    }

    /// 原始配置 `autoSwitchNode === true`（上游 index.ts:1846 门控）。**从原始 JSON 读**——该字段不在
    /// `UserConfig` 结构体（同 `restartOnNodeChange` / `meshLoginFallbackDirect`，见 `switch_mode` 注）。
    ///
    /// 走 [`ConfigManager::with_current`](crate::runtime::config::ConfigManager::with_current) 而非 `current()`：本方法由自动换节点心跳**每 tick 无条件**
    /// 调用（`HEARTBEAT_INTERVAL_MS`，核在跑就一直跑），而它只要一个 bool —— 为此深拷贝整份配置
    /// （含 200 节点级 `servers`）纯属常驻浪费。闭包内只取字段，不回调任何子系统。
    ///
    fn auto_switch_enabled(&self) -> bool {
        self.config
            .with_current(|c| c.get("autoSwitchNode").and_then(Value::as_bool))
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    /// **自动换节点心跳守卫**（上游 `AutoSwitchService.runHeartbeat`:113-116）：当前选中节点是否真实
    /// 存在于 `servers`。委托纯谓词 [`selected_server_present`]（无选中 / direct 哨兵 `__direct__` 不在
    /// servers / 选中被删 → false）。读配置失败 → false（保守跳过心跳，绝不误切）。
    ///
    /// 与 [`auto_switch_enabled`](Self::auto_switch_enabled) 同属心跳**每 tick 的无条件调用**，故同样走
    /// [`ConfigManager::with_current`](crate::runtime::config::ConfigManager::with_current)：谓词本体只需 `&Value`，不需要 owned 快照。
    fn selected_server_is_real(&self) -> bool {
        self.config
            .with_current(selected_server_present)
            .unwrap_or(false)
    }

    /// **C3 换节点执行体**。闸门（熔断/冷却/在飞）决策全在
    /// [`AutoSwitchMachine::evaluate_switch`]；放行后只测运行核 clean 候选 → 选最优 →
    /// [`Self::auto_hot_switch_transaction`] 做零重启提交并回读自证 → emit。**真机门**（真起核 + 碰网络）。
    async fn run_auto_switch(self: &Arc<Self>, machine: &mut AutoSwitchMachine, reason: &str) {
        match machine.evaluate_switch(monotonic_now_ms()) {
            SwitchGate::Proceed => {}
            SwitchGate::InFlight => return,
            SwitchGate::Breaker { remaining_ms } => {
                log::warn!(
                    "自动切换已熔断（连续切换未恢复连通），{}s 内暂停切换，请检查网络/订阅",
                    remaining_ms.div_ceil(1000)
                );
                return;
            }
            SwitchGate::Cooldown { remaining_ms } => {
                log::info!(
                    "自动换节点冷却中，{}s 后可再次触发",
                    remaining_ms.div_ceil(1000)
                );
                return;
            }
        }
        // 放行 → 进入在飞态（提前置 lastSwitchTime → 失败/无候选也进冷却，防空转，上游 :180-181）。
        machine.begin_switch(monotonic_now_ms());
        let switched = self.do_switch_io(machine, reason).await;
        // 真发生了切换 → 记账熔断窗口（上游 :233-236）；候选空/全不可达的早退不记（对齐 上游 两个 return）。
        if switched {
            machine.record_switch_success(monotonic_now_ms());
        }
        // finally：退出在飞态（上游 :257-259）。
        machine.end_switch();
    }

    /// 换节点的纯 I/O 段：取 D/R/S 快照 → 规划运行核 clean 候选 → 经主核 probe pool 做真实协议链探测
    /// → 只热切 selector → 运行态回读 → emit。返回 `true` 仅表示整条事务已自证成功。
    ///
    /// # 为什么要把 `machine` 借进来（而不是让调用方按返回值决定）
    ///
    /// 唯一的用途是「候选全被『需要重启』剔光」这条告警的**每世代一次**锁存（见
    /// [`AutoSwitchMachine::claim_restart_blocked_report`]）。另一条路是把返回值从 `bool` 换成结局
    /// 枚举、由已持有 machine 的 [`run_auto_switch`](Self::run_auto_switch) 决定报不报 —— 但本方法有
    /// 十余处 `return false` 早退，它们彼此并无语义差别（调用方只关心「切没切成」），为一条腿把它们
    /// 全部变成具名变体，是为一个锁存位付一次全函数改写。锁存位本身又恰恰是**世代作用域的决策态**，
    /// 与 `is_switching` / `last_switch_time` 同类，`AutoSwitchMachine` 就是它的归宿。故取「多借一个
    /// `&mut`」这条更小的改动面，判据与锁存两者仍都是决策层的纯逻辑（各自有真值表锁死）。
    async fn do_switch_io(self: &Arc<Self>, machine: &mut AutoSwitchMachine, reason: &str) -> bool {
        let generation = self.core_generation();
        let config = match self.config.current() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("自动换节点：读配置失败 → 跳过：{e}");
                return false;
            }
        };
        let Some(runtime_config) = self
            .current_config
            .read()
            .ok()
            .and_then(|guard| guard.clone())
        else {
            log::warn!("自动故障切换：运行核缺少 current_config 基准 → 跳过");
            return false;
        };
        if self
            .switch_snapshot
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|snapshot| snapshot.mesh_mode_ready))
            .unwrap_or(false)
            && serde_json::from_value::<UserConfig>(runtime_config.clone())
                .ok()
                .is_none_or(|config| selected_mode(&config) == MESH_DIRECT)
        {
            log::info!("自动故障切换：双态核当前使用无出口 TS，后台 selector-only 事务禁用");
            return false;
        }
        let current_id = runtime_config
            .get("selectedServerId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(current_id) = current_id else {
            return false;
        };
        // D 与 R 的当前选择不同意味着已有保存/广播/切换在排队。自动治理不得在这个缝里另立第三个意图。
        if config.get("selectedServerId").and_then(Value::as_str) != Some(current_id.as_str()) {
            log::info!("自动故障切换：磁盘期望出口与运行核出口不同 → 让位给既有待应用事务");
            return false;
        }
        let staged = self.config.staged_node_mask();
        if staged.pending && !staged.scope_known {
            log::info!("自动故障切换：存在范围未知的未保存草稿 → 保守跳过本轮");
            return false;
        }
        let Some(targets) = self.speed_probe_targets() else {
            log::warn!("自动故障切换：主核探测池未就绪，拒绝回退裸 TCP 候选探测");
            return false;
        };
        let current_fingerprints = current_server_fingerprints(&config);
        let not_ready_ids: BTreeSet<String> = config
            .get("servers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|server| {
                let id = server.get("id").and_then(Value::as_str)?;
                let is_tailscale =
                    server.get("protocol").and_then(Value::as_str) == Some("tailscale");
                (is_tailscale
                    && !self
                        .mesh
                        .ts_status_event(id)
                        .as_ref()
                        .is_some_and(TailscaleStatusEvent::exit_ready))
                .then(|| id.to_string())
            })
            .collect();
        // 自动选择意图生效时，候选收窄到意图指向的订阅：不借别的订阅，也不转直连。
        let auto_subscription = auto_select::effective_subscription(
            &config,
            Switches::PRODUCTION,
            self.helper.platform(),
        );
        let scope: Option<BTreeSet<String>> = auto_subscription.map(|subscription| {
            auto_select::subscription_members(&config, subscription)
                .into_iter()
                .collect()
        });
        let mut candidate_plan = plan_runtime_candidates(
            &config,
            Some(&current_id),
            &targets.id_to_tag,
            &targets.fingerprints,
            &current_fingerprints,
            &staged.node_ids,
            &not_ready_ids,
            scope.as_ref(),
        );
        // 探测**之前**剔除切不过去的候选（判据同提交事务）。放在这里而不是让它们探完再被最后一道门
        // 拒：被拒的那轮 `do_switch_io` 返 false 不计熔断，整轮全量探测白跑、90 秒后原样重来。
        //
        // 返 false = 本轮无从判定（核已停/被接管、运行核基准过期）⇒ **直接早退且不上报**：那时
        // `status` 刚被 `ProxyStatus::default()` 清零，往里写一条非致命错误会被渲染端判成终态故障，
        // 而用户刚点的是停止。
        if !self.retain_hot_switchable_candidates(generation, &runtime_config, &mut candidate_plan)
        {
            log::info!("自动故障切换：候选资格判定期间内核已停或被接管 → 本轮作废");
            return false;
        }
        if candidate_plan.candidates.is_empty() {
            log::warn!(
                "自动故障切换：无可验证的运行态候选（草稿={} 未入核={} 参数脏={} 未就绪={} 不可作出口={} 需重启={}）",
                candidate_plan.staged,
                candidate_plan.not_loaded,
                candidate_plan.dirty,
                candidate_plan.not_ready,
                candidate_plan.not_exit,
                candidate_plan.needs_restart
            );
            // 自动切换**真的触发了**（连通性连续 3 次失败）、候选也规划出来了，却被上一行那道剔除
            // 剃光 ⇒ 故障切换实际什么也没做，而用户完全无感。这是可行动的（手动换节点 / 重启代理让
            // 新配置入核），故上屏；候选为空的另外四因不上屏（判据与理由见 `switch_blocked_by_restart`）。
            //
            // **两个合取项的顺序是判据的一部分**：先问「这轮该不该报」（纯函数、无副作用），
            // 再认领上报权（有副作用）。反过来写会让任何一轮候选为空的早退都吃掉本世代唯一那次
            // 上报权，真该报的那轮反而静默 —— 那是把锁存装反。
            if switch_blocked_by_restart(&candidate_plan) && machine.claim_restart_blocked_report()
            {
                self.set_nonfatal_error(RESTART_BLOCKED_MESSAGE, code::AUTO_SWITCH_NEEDS_RESTART);
            }
            return false;
        }
        log::info!(
            "[{reason}] 经主核探测池验证 {} 个 clean 候选（排除：草稿={} 未入核={} 参数脏={} 未就绪={} 不可作出口={} 需重启={}）",
            candidate_plan.candidates.len(),
            candidate_plan.staged,
            candidate_plan.not_loaded,
            candidate_plan.dirty,
            candidate_plan.not_ready,
            candidate_plan.not_exit,
            candidate_plan.needs_restart
        );
        let barred =
            self.auto_select_failover_barred_from(&config, generation, &SelectReadings::live());
        let url = resolve_speed_test_url(&config);
        let Some(verdict) = self
            .probe_failover_candidates_from(
                generation,
                &candidate_plan.candidates,
                &barred,
                |probe_input| {
                    let targets = &targets;
                    let url = &url;
                    async move { probe_runtime_candidates(self, targets, &probe_input, url).await }
                },
            )
            .await
        else {
            return false;
        };
        let best = match verdict {
            ProbeVerdict::Best(best) => best,
            // 有候选因读回不符被跳过（只在读回的「强制」档下出现）：量到的不是它，不能据此说它
            // 不可用，故不下「全部不可用」的结论，也不走那条结论的后续（重启受阻上报）。
            ProbeVerdict::Inconclusive { mismatch_skipped } => {
                log::info!(
                    "自动故障切换：没有测出可用候选，另有 {mismatch_skipped} 个候选因读回不符被跳过 → 本轮不下结论"
                );
                return false;
            }
            ProbeVerdict::AllFailed => {
                log::warn!("所有运行态 clean 候选均未通过真实代理链探测，无法自动切换");
                // 与「候选被剃光」那条**同码同锁存**：对用户而言两种现场的可行动性完全相同 —— 有
                // `needs_restart` 个节点本来能救场，只是被切换门挡在探测之外，而他无从得知。
                // 区分项仍只有 `needs_restart > 0` 这一条，故不会因为「探测全败」本身多报一条
                // （那件事用户下不了手，只该进日志）。
                if candidate_plan.needs_restart > 0 && machine.claim_restart_blocked_report() {
                    self.set_nonfatal_error(
                        RESTART_BLOCKED_MESSAGE,
                        code::AUTO_SWITCH_NEEDS_RESTART,
                    );
                }
                return false;
            }
        };
        let best_latency = best.latency_ms.unwrap_or(0);
        log::info!("选中最优节点: {} ({best_latency}ms)", best.name);

        let Some(mut payload) = switch_payload(&best, reason) else {
            log::warn!("自动换节点：候选缺少有效延迟 → 跳过");
            return false;
        };
        payload.old_server_name = server_name(&config, &current_id);
        let Some(candidate) = candidate_plan
            .candidates
            .iter()
            .find(|candidate| candidate.id == best.id)
        else {
            return false;
        };
        let Some(expected_fingerprint) = current_fingerprints.get(&candidate.id) else {
            return false;
        };
        match self
            .auto_hot_switch_transaction(
                generation,
                &current_id,
                candidate,
                expected_fingerprint,
                auto_subscription,
            )
            .await
        {
            AutoHotSwitchOutcome::Applied => {}
            AutoHotSwitchOutcome::Busy => {
                log::info!("自动故障切换：生命周期事务在飞 → 本轮让位");
                return false;
            }
            AutoHotSwitchOutcome::Superseded => {
                log::info!("自动故障切换：配置、草稿或内核世代已变化 → 本轮作废");
                return false;
            }
            AutoHotSwitchOutcome::NotEligible => {
                log::warn!("自动故障切换：目标不再满足零重启热切条件 → 跳过");
                return false;
            }
            AutoHotSwitchOutcome::ReconcilePending { intent_generation } => {
                self.spawn_selector_reconciliation(generation, intent_generation);
                return false;
            }
            AutoHotSwitchOutcome::Failed => return false,
        }
        log::info!("自动换节点已自证成功: {}", payload.new_server_name);
        self.auto_select_note_failover(
            auto_subscription.map(|subscription| {
                plan_subscriptions(&config, &BTreeSet::from([subscription.to_string()]))
                    .get(subscription)
                    .map_or(
                        polaris_store::SPEED_TEST_INTERVAL_MINUTES_DEFAULT * 60_000,
                        |sub| sub.period_ms,
                    )
            }),
            SwitchRecord {
                at: now_ms(),
                leg: Leg::Failover,
                cause: Cause::Failover,
                from_id: Some(current_id.clone()),
                from_name: payload.old_server_name.clone(),
                to_id: best.id.clone(),
                to_name: best.name.clone(),
                from_latency_ms: None,
                to_latency_ms: best.latency_ms,
                unverified: None,
            },
            monotonic_now_ms(),
        );

        // emit（未接线 emitter：单测 / setup 前 → 静默跳过，对齐既有 emit 腿）。
        if let Some(emitter) = self.error_emitter.get() {
            emitter.emit_auto_node_switched(&payload);
        }
        true
    }
}

/// 配置里某个节点的显示名。直连、阻断哨兵与已不在配置里的 id 没有。
fn server_name(config: &Value, id: &str) -> Option<String> {
    config
        .get("servers")
        .and_then(Value::as_array)?
        .iter()
        .find(|server| server.get("id").and_then(Value::as_str) == Some(id))
        .map(|server| {
            server
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string()
        })
}

// ════════════════ 自动选点：择优腿的 I/O 编排与对外状态 ════════════════
//
// 裁决全在 [`crate::runtime::auto_select`]（纯函数加显式记忆量）；本段只做「取读数 → 裁决 → 经
// 上面那一个提交事务换点 → 记进状态槽 → 发事件」。择优腿与故障腿跑在同一个心跳任务里、顺序执行，
// 共用决策机上的在飞标志与冷却时刻，不会同时换点；与手动选择、订阅刷新、核世代更替的互斥全部
// 落在提交事务已有的守卫上（选择意图代次、配置写事务里的比对、世代与生命周期闸门）。

/// 心跳任务的两条腿与两个判据。心跳循环（[`run_heartbeat`]）只认这四件事：生产实现是
/// [`LiveLegs`]，单测换成记录调用时刻的替身、在假时钟下驱动同一个循环。
pub(super) trait HeartbeatLegs {
    /// 世代守卫：核被停或被接管后为假，循环退场。
    fn current(&self) -> bool;
    /// 等到「账本有新结果，且没有测速在飞」：一轮中途的半份数据不参与选点。
    async fn data_settled(&self);
    /// 故障腿的一拍。`Break` = 任务退场。
    async fn failover(&mut self) -> ControlFlow<()>;
    /// 择优腿的一步。
    async fn select(&mut self);
}

/// **心跳循环**：何时醒、醒了做什么。
///
/// 三路唤醒：心跳拍到期 → 故障腿一拍，再择优腿一步；账本有新结果（且测速已收口）、或自动意图
/// 刚被写入 → 只走择优腿。心跳拍的到期时刻单独记，后两路唤醒不提前也不推迟连通性探测：相邻两次
/// 探测的间隔恒为「上一拍做完的时刻加一个拍长」，与只有故障腿时相同。
pub(super) async fn run_heartbeat<L: HeartbeatLegs>(
    legs: &mut L,
    tick: Duration,
    intent_written: &tokio::sync::Notify,
) {
    let mut next_beat = tokio::time::Instant::now() + tick;
    loop {
        let beat = tokio::select! {
            () = tokio::time::sleep_until(next_beat) => true,
            () = legs.data_settled() => false,
            () = intent_written.notified() => false,
        };
        // 世代守卫：核被停/接管 → 退场。
        if !legs.current() {
            return;
        }
        if beat {
            if legs.failover().await.is_break() {
                return;
            }
            next_beat = tokio::time::Instant::now() + tick;
        }
        legs.select().await;
    }
}

/// 心跳任务两条腿的生产实现。
struct LiveLegs {
    rt: Arc<ProxyRuntime>,
    machine: AutoSwitchMachine,
    my_gen: u64,
    probe_proxy_port: Option<u16>,
    loopback_auth: Option<InboundUser>,
    generation_blocked: bool,
    dynamic_mesh_id: Option<String>,
    /// 择优腿上一次看到的账本版本。
    seen: u64,
    cache: SelectCache,
}

impl HeartbeatLegs for LiveLegs {
    fn current(&self) -> bool {
        self.rt.gate.generation() == self.my_gen
    }

    async fn data_settled(&self) {
        speed_test_data_settled(self.seen).await;
    }

    async fn failover(&mut self) -> ControlFlow<()> {
        self.rt
            .failover_beat(
                &mut self.machine,
                self.my_gen,
                self.probe_proxy_port,
                self.loopback_auth.as_ref(),
                self.generation_blocked,
                self.dynamic_mesh_id.as_deref(),
            )
            .await
    }

    async fn select(&mut self) {
        self.seen = measurement_ledger::global().version();
        self.rt
            .auto_select_tick(self.my_gen, &mut self.machine, &mut self.cache)
            .await;
    }
}

/// 一个候选「切过去」的资格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eligibility {
    Hot,
    NeedsRestart,
    /// 只走内网的组网节点：不是出口候选。
    NotExit,
}

/// 资格判定的缓存。判定要拿运行核基准重跑一次热切判定，逐候选每拍重算太贵；它只取决于运行核
/// 这一份配置与当前出口，两者任一变了即整体作废。
#[derive(Debug, Default)]
pub(super) struct SelectCache {
    key: Option<(u64, String)>,
    verdicts: BTreeMap<String, Eligibility>,
}

impl SelectCache {
    fn align(&mut self, generation: u64, current_id: &str) {
        if self
            .key
            .as_ref()
            .is_none_or(|(cached, id)| *cached != generation || id != current_id)
        {
            self.key = Some((generation, current_id.to_string()));
            self.verdicts.clear();
        }
    }
}

/// 择优腿准备提交的一次换点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SelectPlan {
    /// 发起这次换点所凭的自动意图指向的订阅。
    pub(super) subscription: String,
    pub(super) current_id: String,
    pub(super) candidate: RuntimeCandidate,
    pub(super) fingerprint: String,
    pub(super) cause: Cause,
    pub(super) latency_ms: u32,
    from_name: Option<String>,
    from_latency_ms: Option<u32>,
    unverified: bool,
}

/// 自动选点从进程级设施取的读数与两个时钟。生产取活的那一份（[`SelectReadings::live`]）；单测
/// 各自注入，不与同进程里别的用例共用账本与单飞闸，时刻也由用例给。
pub(super) struct SelectReadings<'a> {
    pub(super) ledger: &'a MeasurementLedger,
    pub(super) signals: Signals,
    /// 单飞闸上有测速在飞。
    pub(super) measuring: bool,
    /// 单调毫秒：一切「相隔多久」据它算。
    pub(super) now: u64,
    /// Unix 毫秒：账本的新鲜期与给界面看的时间戳。
    pub(super) now_wall: u64,
}

impl SelectReadings<'static> {
    fn live() -> Self {
        Self {
            ledger: measurement_ledger::global(),
            signals: measurement_scheduler::signals(),
            measuring: speed_test_in_flight().is_some(),
            now: monotonic_now_ms(),
            now_wall: now_ms(),
        }
    }
}

/// 某个订阅从配置里投影出来的那一部分。
struct AutoView {
    subscription: String,
    /// 订阅成员，按配置顺序。
    members: Vec<String>,
    /// 成员里的 Tailscale 节点（运行态是否就绪要另查）。
    tailscale: BTreeSet<String>,
    view: LedgerView,
    plan: BTreeMap<String, SubPlan>,
    period_ms: u64,
}

impl AutoView {
    fn of(
        config: &Value,
        subscription: &str,
        generation: Option<u64>,
        network_epoch: Option<u64>,
        platform: Platform,
    ) -> Self {
        let plan = plan_subscriptions(config, &BTreeSet::from([subscription.to_string()]));
        let members = auto_select::subscription_members(config, subscription);
        let tailscale = config
            .get("servers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|server| server.get("protocol").and_then(Value::as_str) == Some("tailscale"))
            .filter_map(|server| server.get("id").and_then(Value::as_str))
            .filter(|id| members.iter().any(|member| member == id))
            .map(str::to_string)
            .collect();
        Self {
            period_ms: plan.get(subscription).map_or(
                polaris_store::SPEED_TEST_INTERVAL_MINUTES_DEFAULT * 60_000,
                |sub| sub.period_ms,
            ),
            subscription: subscription.to_string(),
            members,
            tailscale,
            view: LedgerView::new(config, generation, network_epoch, platform),
            plan,
        }
    }

    /// 给定节点在账本里的分布（可选点与各类排除）。新鲜度判据与调度器共用 [`LedgerView`]；
    /// 新鲜期上限取自每条记录入账时的降频倍数。
    fn read(&self, node_ids: &[String], readings: &SelectReadings<'_>) -> Candidates {
        let cap = |id: &str, entry: &LedgerEntry| entry_freshness_cap_ms(&self.plan, id, entry);
        readings.ledger.candidates(
            node_ids,
            &self.view.read(&cap, readings.ledger, readings.now_wall),
        )
    }
}

/// 择优腿与状态读取共用的配置投影。
struct SelectView {
    /// 磁盘期望态里的实际出口。
    selected: Option<String>,
    selected_name: Option<String>,
    intent: Intent,
    /// 配置里存着自动意图时才有。
    auto: Option<AutoView>,
}

fn selected_of(config: &Value) -> Option<String> {
    config
        .get("selectedServerId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

impl ProxyRuntime {
    fn auto_select_slot(&self) -> MutexGuard<'_, auto_select::Slot> {
        self.auto_select
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// 自动选择意图此刻是否生效（存着、总开关开着、本平台开放）。心跳每拍调用，只投影一个布尔。
    pub(super) fn auto_select_active(&self) -> bool {
        let platform = self.helper.platform();
        self.config
            .with_current(|config| {
                auto_select::effective_subscription(config, Switches::PRODUCTION, platform)
                    .is_some()
            })
            .unwrap_or(false)
    }

    /// 配置投影。非自动意图下只取实际出口，不读节点表。
    fn auto_select_view(&self, generation: Option<u64>) -> SelectView {
        let platform = self.helper.platform();
        let network_epoch = self.network_epoch();
        self.config
            .with_current(|config| {
                let selected = selected_of(config);
                let intent = Intent::stored(config);
                let auto = match &intent {
                    Intent::Auto { subscription_id } => Some(AutoView::of(
                        config,
                        subscription_id,
                        generation,
                        network_epoch,
                        platform,
                    )),
                    Intent::Manual | Intent::Unrecognized => None,
                };
                SelectView {
                    selected_name: auto
                        .as_ref()
                        .and(selected.as_deref())
                        .and_then(|id| server_name(config, id)),
                    selected,
                    intent,
                    auto,
                }
            })
            .unwrap_or(SelectView {
                selected: None,
                selected_name: None,
                intent: Intent::Manual,
                auto: None,
            })
    }

    /// 取一次裁决要用的读数并对齐记忆量的有效范围，给出不含候选的 [`Facts`]。后台与显式路径
    /// 共用：两者看到的当前出口状况、首轮锁存与供数状况来自同一处。
    #[allow(
        clippy::too_many_arguments,
        reason = "逐项都是一次裁决的独立输入，收成结构体只是换个地方列"
    )]
    fn auto_select_facts<'a>(
        &self,
        intent: &'a Intent,
        auto: &'a AutoView,
        selected: Option<&'a str>,
        read: &Candidates,
        readings: &SelectReadings<'_>,
        generation: u64,
        core_running: bool,
        explicit: bool,
        last_attempt_at: Option<u64>,
    ) -> Facts<'a> {
        let platform = self.helper.platform();
        let signals = &readings.signals;
        let first_round_done = {
            let mut slot = self.auto_select_slot();
            slot.memory.observe_context(
                &Epoch {
                    subscription: auto.subscription.clone(),
                    generation,
                    network_epoch: self.network_epoch(),
                    foreground_epoch: measurement_scheduler::is_mobile(platform)
                        .then(|| readings.ledger.foreground_epoch()),
                },
                signals.round_serial,
                signals.metered_change_epoch,
            );
            slot.memory.latch_first_round(
                read.covers_all_testable(),
                signals
                    .subscriptions
                    .get(&auto.subscription)
                    .and_then(|(serial, _)| *serial),
            )
        };
        // 磁盘期望出口与运行核出口不同，或有范围未知的未保存草稿：已有保存、广播或切换在排队。
        let runtime_selected = self
            .current_config
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().and_then(selected_of));
        let staged = self.config.staged_node_mask();
        Facts {
            intent,
            switches: Switches::PRODUCTION,
            platform_open: auto_select::platform_open(platform),
            core_running,
            explicit,
            foreground: signals.foreground,
            reconciling: self.selector_reconcile.is_required(),
            config_pending: runtime_selected.as_deref() != selected
                || (staged.pending && !staged.scope_known),
            measuring: readings.measuring,
            members: auto.members.len(),
            first_round_done,
            starved: match signals.verdict {
                Verdict::Idle(Idle::UserDisabled) => Some(Starved::PeriodicDisabled),
                Verdict::Paused(Pause::Metered) => Some(Starved::MeteredPaused),
                Verdict::Paused(Pause::PowerSave) => Some(Starved::PowerSave),
                _ => None,
            },
            current: auto_select::classify_current(selected, &auto.members, read),
            current_id: selected,
            candidates: &[],
            restart_blocked: 0,
            restart_candidates: &[],
            now: readings.now,
            last_attempt_at,
        }
    }

    /// **显式路径的裁决**：用户点某订阅的「自动选择」时的首次落点、「立即切换」。目标由与后台
    /// 择优同一个 [`auto_select::decide`] 给出，差别只在 [`Facts::explicit`]：不受冷却与后台的
    /// 几道闸门约束，候选也不要求能零重启切过去（显式路径允许整核重启）。
    ///
    /// `config` 由调用方给（配置写事务里的那一份）；核不在运行时账本里没有当前结果，裁决为不评估。
    pub(crate) fn auto_select_explicit(
        &self,
        config: &Value,
        subscription: &str,
    ) -> SelectDecision {
        self.auto_select_explicit_from(config, subscription, &SelectReadings::live())
    }

    pub(super) fn auto_select_explicit_from(
        &self,
        config: &Value,
        subscription: &str,
        readings: &SelectReadings<'_>,
    ) -> SelectDecision {
        let core_running = self.core_running();
        let generation = self.core_generation();
        let auto = AutoView::of(
            config,
            subscription,
            core_running.then_some(generation),
            self.network_epoch(),
            self.helper.platform(),
        );
        let selected = selected_of(config);
        let read = auto.read(&auto.members, readings);
        let intent = Intent::Auto {
            subscription_id: subscription.to_string(),
        };
        let mut facts = self.auto_select_facts(
            &intent,
            &auto,
            selected.as_deref(),
            &read,
            readings,
            generation,
            core_running,
            true,
            None,
        );
        let candidates: Vec<Candidate> = read
            .selectable
            .iter()
            .filter(|candidate| Some(candidate.node_id.as_str()) != selected.as_deref())
            .cloned()
            .collect();
        facts.candidates = &candidates;
        auto_select::decide(&facts, &mut self.auto_select_slot().memory)
    }

    /// 订阅刷新删掉当前出口时的改选：在意图指向的订阅里，优先未屏蔽的最低延迟可选点
    /// （仍存在且未改动的才有当前结果）。仅无可选未屏蔽成员时救援屏蔽成员；不经择优的迟滞。
    pub(crate) fn auto_select_fallback(&self, config: &Value) -> Option<Candidate> {
        self.auto_select_fallback_from(config, &SelectReadings::live())
    }

    pub(super) fn auto_select_fallback_from(
        &self,
        config: &Value,
        readings: &SelectReadings<'_>,
    ) -> Option<Candidate> {
        let platform = self.helper.platform();
        let subscription =
            auto_select::effective_subscription(config, Switches::PRODUCTION, platform)?;
        let generation = self.core_running().then(|| self.core_generation())?;
        let network_epoch = self.network_epoch();
        let auto = AutoView::of(
            config,
            subscription,
            Some(generation),
            network_epoch,
            platform,
        );
        let read = auto.read(&auto.members, readings);
        let ranked = auto_select::rank(&read.selectable);
        let mut slot = self.auto_select_slot();
        slot.memory.observe_context(
            &Epoch {
                subscription: auto.subscription.clone(),
                generation,
                network_epoch,
                foreground_epoch: measurement_scheduler::is_mobile(platform)
                    .then(|| readings.ledger.foreground_epoch()),
            },
            readings.signals.round_serial,
            readings.signals.metered_change_epoch,
        );
        ranked
            .iter()
            .copied()
            .find(|candidate| !slot.memory.is_barred(&candidate.node_id, readings.now))
            .or_else(|| ranked.first().copied())
            .cloned()
    }

    /// 故障腿只消费有效 Auto 订阅的既有屏蔽；手动故障切换不读写择优记忆。
    pub(super) fn auto_select_failover_barred_from(
        &self,
        config: &Value,
        generation: u64,
        readings: &SelectReadings<'_>,
    ) -> BTreeSet<String> {
        let platform = self.helper.platform();
        let Some(subscription) =
            auto_select::effective_subscription(config, Switches::PRODUCTION, platform)
        else {
            return BTreeSet::new();
        };
        let network_epoch = self.network_epoch();
        let mut slot = self.auto_select_slot();
        slot.memory.observe_context(
            &Epoch {
                subscription: subscription.to_string(),
                generation,
                network_epoch,
                foreground_epoch: measurement_scheduler::is_mobile(platform)
                    .then(|| readings.ledger.foreground_epoch()),
            },
            readings.signals.round_serial,
            readings.signals.metered_change_epoch,
        );
        slot.memory
            .barred(readings.now)
            .into_iter()
            .map(|(id, _)| id.to_string())
            .collect()
    }

    /// 合格候选按屏蔽分两层，逐层探测和裁决：未屏蔽层为空或明确全败才救援屏蔽层。
    /// 无结论、抢占、停核与世代变化整轮作废；资格与最后提交仍由调用方裁定。
    pub(super) async fn probe_failover_candidates_from<P, F>(
        &self,
        generation: u64,
        candidates: &[RuntimeCandidate],
        barred: &BTreeSet<String>,
        mut probe: P,
    ) -> Option<ProbeVerdict>
    where
        P: FnMut(Vec<(String, String)>) -> F,
        F: Future<Output = RuntimeProbeBatch>,
    {
        if candidates.is_empty() {
            return None;
        }
        let (unbarred, rescue): (Vec<_>, Vec<_>) = candidates
            .iter()
            .partition(|candidate| !barred.contains(&candidate.id));
        for tier in [&unbarred, &rescue] {
            if tier.is_empty() {
                continue;
            }
            if self.core_generation() != generation || !self.core_running() {
                return None;
            }
            let input = tier
                .iter()
                .map(|candidate| (candidate.id.clone(), candidate.tag.clone()))
                .collect();
            let probes = match probe(input).await {
                RuntimeProbeBatch::Completed(probes) => probes,
                RuntimeProbeBatch::Busy => {
                    log::info!("自动故障切换：用户测速正在占用 probe pool → 本轮让位");
                    return None;
                }
                RuntimeProbeBatch::Interrupted => {
                    log::info!("自动故障切换：候选探测被抢占或内核世代变化 → 本轮作废");
                    return None;
                }
            };
            if self.core_generation() != generation || !self.core_running() {
                return None;
            }
            let named = tier
                .iter()
                .map(|candidate| (candidate.id.clone(), candidate.name.clone()))
                .collect::<Vec<_>>();
            match judge_probes(&named, &probes) {
                // Only an explicit failure for every eligible member proves this tier failed.
                // The existing probe adapter supplies all members; do not turn a missing
                // receipt into a rescue decision if that contract is ever violated.
                ProbeVerdict::AllFailed
                    if tier
                        .iter()
                        .any(|candidate| !probes.contains_key(&candidate.id)) =>
                {
                    log::info!("自动故障切换：候选探测回执不完整 → 本轮不下全败结论");
                    return None;
                }
                ProbeVerdict::AllFailed => {}
                verdict => return Some(verdict),
            }
        }
        Some(ProbeVerdict::AllFailed)
    }

    /// 每个核世代开始时一行：意图、订阅、成员数与两个回退开关的取值。
    fn auto_select_log_generation(&self, generation: u64) {
        let view = self.auto_select_view(Some(generation));
        match view.auto {
            Some(auto) => log::info!(
                "自动选择：世代 {generation}，意图为自动（订阅 {}，{} 个成员），总开关={}，择优腿={}，本平台开放={}",
                auto.subscription,
                auto.members.len(),
                Switches::PRODUCTION.master,
                Switches::PRODUCTION.better_leg,
                auto_select::platform_open(self.helper.platform())
            ),
            None => log::debug!("自动选择：世代 {generation}，意图为 {:?}", view.intent),
        }
    }

    /// 自动选择在这里不可用的原因（总开关关着、本平台未开放）；可用时为 `None`。
    pub(crate) fn auto_select_closed(&self) -> Option<auto_select::Gate> {
        auto_select::closed(Switches::PRODUCTION, self.helper.platform())
    }

    /// 自动意图刚被写入：清掉上一个意图留下的评估，记下写入时刻，唤醒心跳任务立即评估一次。
    pub(crate) fn auto_select_armed(&self) {
        {
            let mut slot = self.auto_select_slot();
            slot.leave();
            slot.intent_set_mono = Some(monotonic_now_ms());
        }
        self.auto_select_wake.notify_one();
        self.auto_select_announce();
    }

    /// 意图回到手动（用户点了节点，或意图指向的订阅被删）。
    pub(crate) fn auto_select_disarmed(&self) {
        self.auto_select_slot().leave();
        self.auto_select_announce();
    }

    /// 不经心跳任务的一次换点（显式路径、订阅刷新的改选）之后记账。
    pub(crate) fn auto_select_record(&self, record: SwitchRecord) {
        self.auto_select_slot()
            .record_switch(record, monotonic_now_ms());
        self.auto_select_announce();
    }

    /// 故障腿换点成功之后的记账。只在自动意图生效时（`auto_period_ms` 是意图指向的订阅的周期）
    /// 才动择优的状态：记一次换点（驻留从这里起算），并把被换走的节点暂时排除出「更优」的
    /// 挑战者 —— 它测速好而连通性差，不排除就会被择优腿换回去再被故障腿换走。
    /// 手动意图下什么都不记：没开自动选择的用户不留任何择优状态。
    pub(super) fn auto_select_note_failover(
        &self,
        auto_period_ms: Option<u64>,
        record: SwitchRecord,
        now: u64,
    ) {
        let Some(period_ms) = auto_period_ms else {
            return;
        };
        {
            let mut slot = self.auto_select_slot();
            if let Some(from) = record.from_id.as_deref() {
                slot.memory.bar(from, now, period_ms);
            }
            slot.record_switch(record, now);
        }
        self.auto_select_announce();
    }

    /// 选择状态（状态命令与状态事件共用）。读时投影：评估任务不在运行时也答得出来。
    pub(crate) fn auto_select_status(&self) -> auto_select::Status {
        self.auto_select_status_from(&SelectReadings::live())
    }

    pub(super) fn auto_select_status_from(
        &self,
        readings: &SelectReadings<'_>,
    ) -> auto_select::Status {
        let core_running = self.core_running();
        let view = self.auto_select_view(core_running.then(|| self.core_generation()));
        let signals = &readings.signals;
        let (basis, in_subscription, period_ms, last_skip) = match view.auto.as_ref() {
            Some(auto) => {
                let mut node_ids = auto.members.clone();
                let in_subscription = view
                    .selected
                    .as_ref()
                    .is_some_and(|id| auto.members.contains(id));
                // 实际出口不在订阅里时也给出它的依据（它是真实节点的话）。
                if let Some(id) = view.selected.as_ref().filter(|_| !in_subscription) {
                    node_ids.push(id.clone());
                }
                let read = auto.read(&node_ids, readings);
                (
                    view.selected
                        .as_deref()
                        .filter(|_| view.selected_name.is_some())
                        .map(|id| auto_select::Basis::of(id, &read)),
                    in_subscription,
                    auto.period_ms,
                    signals
                        .subscriptions
                        .get(&auto.subscription)
                        .and_then(|(_, skip)| *skip),
                )
            }
            None => (None, false, 0, None),
        };
        auto_select::project_status(
            &StatusInputs {
                intent: &view.intent,
                switches: Switches::PRODUCTION,
                platform_open: auto_select::platform_open(self.helper.platform()),
                core_running,
                // 核就绪的时刻只有墙钟的那一份（状态里给界面看的启动时间），折成「距今多久」。
                core_ready_ago_ms: self
                    .status()
                    .start_time
                    .map(|at| readings.now_wall.saturating_sub(at)),
                foreground: signals.foreground,
                exit_id: view.selected.as_deref(),
                exit_in_subscription: in_subscription,
                basis,
                period_ms,
                data_source: auto_select::DataSource {
                    blocked_by: signals.blocked_by(),
                    last_skip,
                },
                now: readings.now,
            },
            &self.auto_select_slot(),
        )
    }

    /// 状态变了才发事件；自曝标记置位时记一行警告（界面没开着也进日志）。
    pub(crate) fn auto_select_announce(&self) {
        self.auto_select_announce_from(&SelectReadings::live());
    }

    /// 取状态、比对、发射在同一把锁里完成：并发的两次播报因此先后有序，每次发的都是它拿到锁
    /// 那一刻的状态，旧状态不会排到新状态后面。锁里没有 await。
    pub(super) fn auto_select_announce_from(&self, readings: &SelectReadings<'_>) {
        let mut announced = self
            .auto_select_announced
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let status = self.auto_select_status_from(readings);
        let Ok(payload) = serde_json::to_value(&status) else {
            return;
        };
        // 去重的键不含随时间流逝自己会变的量：最近一次评估的时刻、各个「还剩多久」「已持续多久」。
        let mut key = payload.clone();
        if let Some(object) = key.as_object_mut() {
            object.remove("noDataForMs");
            if let Some(evaluation) = object
                .get_mut("lastEvaluation")
                .and_then(Value::as_object_mut)
            {
                evaluation.remove("at");
            }
            for barred in object
                .get_mut("barred")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(barred) = barred.as_object_mut() {
                    barred.remove("remainingMs");
                }
            }
        }
        let key = key.to_string();
        if announced.as_deref() == Some(key.as_str()) {
            return;
        }
        *announced = Some(key);
        let flags = status.flags;
        if flags.stalled || flags.starved || flags.exit_outside_overdue {
            log::warn!(
                "自动选择：状态异常（评估停滞={} 长期无数据={} 出口长期不在订阅内={}），模式 {}，原因 {:?}，周期测速 {:?}",
                flags.stalled,
                flags.starved,
                flags.exit_outside_overdue,
                status.mode,
                status.reason,
                status.data_source
            );
        }
        if flags.all_unverified {
            log::info!("自动选择：订阅里的可选点全部没有被读回证实，读回通道在本机很可能不可用");
        }
        if let Some(emitter) = self.error_emitter.get() {
            emitter.emit_auto_select_status(&payload);
        }
    }

    /// 从账本的可选点里取出**能零重启切过去**的候选里延迟最小的那一个；它若因近期被故障腿换走
    /// 而被排除，再多取一个未被排除的里延迟最小的（裁决要区分这两者）。另给排在它们前面、因
    /// 「切过去要整核重启」落选的候选读数（没有一个能切时即全部落选者），用于保留比较胜次。
    /// 资格筛选是故障腿
    /// 现成的那两道：[`plan_runtime_candidates`] 与 [`Self::retain_hot_switchable_candidates`]。
    ///
    /// `None` = 本轮无从判定（核已停、运行核基准已过期），调用方整轮作废。
    async fn auto_select_candidates(
        self: &Arc<Self>,
        generation: u64,
        current_id: &str,
        auto: &AutoView,
        read: &Candidates,
        barred: &BTreeSet<String>,
        cache: &mut SelectCache,
    ) -> Option<(Vec<(Candidate, RuntimeCandidate)>, Vec<Candidate>)> {
        let (id_to_tag, running_fingerprints) =
            self.switch_snapshot.read().ok().and_then(|guard| {
                guard.as_ref().map(|snapshot| {
                    (
                        snapshot.id_to_tag.clone(),
                        snapshot.dirty_fingerprints.clone(),
                    )
                })
            })?;
        let staged = self.config.staged_node_mask();
        let not_ready: BTreeSet<String> = auto
            .tailscale
            .iter()
            .filter(|id| {
                !self
                    .mesh
                    .ts_status_event(id)
                    .as_ref()
                    .is_some_and(TailscaleStatusEvent::exit_ready)
            })
            .cloned()
            .collect();
        let scope: BTreeSet<String> = read
            .selectable
            .iter()
            .map(|candidate| candidate.node_id.clone())
            .collect();
        let plan = self
            .config
            .with_current(|config| {
                plan_runtime_candidates(
                    config,
                    Some(current_id),
                    &id_to_tag,
                    &running_fingerprints,
                    auto.view.fingerprints(),
                    &staged.node_ids,
                    &not_ready,
                    Some(&scope),
                )
            })
            .ok()?;
        let by_id: BTreeMap<&str, &RuntimeCandidate> = plan
            .candidates
            .iter()
            .map(|candidate| (candidate.id.as_str(), candidate))
            .collect();
        let eligible: Vec<Candidate> = read
            .selectable
            .iter()
            .filter(|candidate| by_id.contains_key(candidate.node_id.as_str()))
            .cloned()
            .collect();
        cache.align(generation, current_id);
        let mut runtime_config: Option<Value> = None;
        let mut restart_candidates = Vec::new();
        let mut found: Vec<(Candidate, RuntimeCandidate)> = Vec::new();
        for candidate in auto_select::rank(&eligible) {
            let runtime_candidate = by_id[candidate.node_id.as_str()];
            let verdict = match cache.verdicts.get(&candidate.node_id) {
                Some(verdict) => *verdict,
                None => {
                    if runtime_config.is_none() {
                        runtime_config =
                            Some(self.current_config.read().ok().and_then(|g| g.clone())?);
                    }
                    let mut single = RuntimeCandidatePlan {
                        candidates: vec![runtime_candidate.clone()],
                        ..RuntimeCandidatePlan::default()
                    };
                    if !self.retain_hot_switchable_candidates(
                        generation,
                        runtime_config.as_ref()?,
                        &mut single,
                    ) {
                        return None;
                    }
                    let verdict = if !single.candidates.is_empty() {
                        Eligibility::Hot
                    } else if single.needs_restart > 0 {
                        Eligibility::NeedsRestart
                    } else {
                        Eligibility::NotExit
                    };
                    cache.verdicts.insert(candidate.node_id.clone(), verdict);
                    // 每判一个让出一次：判定是同步的 CPU 活，不占着执行器线程连判一长串。
                    tokio::task::yield_now().await;
                    verdict
                }
            };
            match verdict {
                Eligibility::Hot => {
                    found.push((candidate.clone(), runtime_candidate.clone()));
                    if !barred.contains(&candidate.node_id) {
                        break;
                    }
                }
                Eligibility::NeedsRestart => restart_candidates.push(candidate.clone()),
                Eligibility::NotExit => {}
            }
        }
        // 排除中的只留延迟最小的一个：再多的对裁决没有用处。
        if found.len() > 2 {
            let trusted = found.pop();
            found.truncate(1);
            found.extend(trusted);
        }
        Some((found, restart_candidates))
    }

    /// 择优腿的一次评估：取读数 → 裁决 → 记进状态槽。裁决为换点时返回要提交的那一笔，状态槽里
    /// 先记成「正在提交」，由 [`Self::auto_select_commit`] 落成换点或没换成的原因。
    pub(super) async fn auto_select_assess(
        self: &Arc<Self>,
        generation: u64,
        last_attempt_at: Option<u64>,
        cache: &mut SelectCache,
        readings: &SelectReadings<'_>,
    ) -> Option<SelectPlan> {
        let core_running = self.core_running() && self.gate.generation() == generation;
        let view = self.auto_select_view(core_running.then_some(generation));
        let Some(auto) = view.auto.as_ref() else {
            // 意图不是自动。上一个自动意图留下的评估与连胜清掉（确有残留时才动），此后本腿零开销。
            let had = self.auto_select_slot().leave();
            if had {
                self.auto_select_announce_from(readings);
            }
            return None;
        };
        let read = auto.read(&auto.members, readings);
        let mut facts = self.auto_select_facts(
            &view.intent,
            auto,
            view.selected.as_deref(),
            &read,
            readings,
            generation,
            core_running,
            false,
            last_attempt_at,
        );
        // 资格筛选只在闸门都过了之后做：它要拿运行核基准逐候选重跑热切判定。
        let mut found: Vec<(Candidate, RuntimeCandidate)> = Vec::new();
        let mut restart_candidates = Vec::new();
        if auto_select::gate(&facts).is_none() {
            match view.selected.as_deref() {
                Some(current_id) => {
                    let barred: BTreeSet<String> = self
                        .auto_select_slot()
                        .memory
                        .barred(readings.now)
                        .into_iter()
                        .map(|(id, _)| id.to_string())
                        .collect();
                    let (candidates, restart) = self
                        .auto_select_candidates(generation, current_id, auto, &read, &barred, cache)
                        .await?;
                    found = candidates;
                    restart_candidates = restart;
                    facts.restart_blocked = restart_candidates.len();
                }
                // 没有选中任何出口：后台事务无从比对旧出口，只能走显式路径。
                None => facts.restart_blocked = read.selectable.len(),
            }
        }
        let candidates: Vec<Candidate> = found
            .iter()
            .map(|(candidate, _)| candidate.clone())
            .collect();
        facts.candidates = &candidates;
        facts.restart_candidates = &restart_candidates;
        let counts = auto_select::Counts::of(&read, facts.restart_blocked);
        let outside = facts.current == auto_select::Current::Outside;
        let from_latency_ms = match facts.current {
            auto_select::Current::Fresh { latency_ms, .. } => Some(latency_ms),
            _ => None,
        };
        let decision = {
            let mut slot = self.auto_select_slot();
            let decision = auto_select::decide(&facts, &mut slot.memory);
            // 结论变了才记一行，同一结论不逐拍重复。
            let line = format!("{decision:?}");
            if slot.logged.as_deref() != Some(line.as_str()) {
                log::info!(
                    "自动选择：订阅 {} 的评估结论 {line}（可选 {} 失败 {} 过期 {} 未测 {} 未纳入 {} 未证实 {} 需重启 {}）",
                    auto.subscription,
                    counts.selectable,
                    counts.failed,
                    counts.stale,
                    counts.unmeasured,
                    counts.skipped,
                    counts.unverified,
                    counts.needs_restart
                );
                slot.logged = Some(line);
            }
            // 裁决为换点时，槽里记的是「正在提交」：换没换成由提交的结果说了算。
            let recorded = match &decision {
                SelectDecision::Switch { .. } => {
                    SelectDecision::Hold(auto_select::Hold::Committing)
                }
                other => other.clone(),
            };
            slot.record_evaluation(
                Evaluation {
                    at: readings.now_wall,
                    at_mono: readings.now,
                    subscription: auto.subscription.clone(),
                    decision: recorded,
                    counts,
                },
                outside,
            );
            decision
        };
        let SelectDecision::Switch {
            target,
            latency_ms,
            cause,
        } = decision
        else {
            self.auto_select_announce_from(readings);
            return None;
        };
        let plan = found
            .into_iter()
            .find(|(candidate, _)| candidate.node_id == target)
            .and_then(|(candidate, runtime_candidate)| {
                Some(SelectPlan {
                    subscription: auto.subscription.clone(),
                    current_id: view.selected.clone()?,
                    fingerprint: auto.view.fingerprints().get(&target)?.clone(),
                    candidate: runtime_candidate,
                    cause,
                    latency_ms,
                    from_name: view.selected_name.clone(),
                    from_latency_ms,
                    unverified: read.unverified.contains(&candidate.node_id),
                })
            });
        if plan.is_none() {
            // 裁决的目标在这一步之间失去了提交所需的材料（指纹表里已没有它）：按没换成记。
            self.auto_select_slot().record_commit_failure(
                CommitFailure::NotEligible,
                &target,
                readings.now_wall,
            );
            self.auto_select_announce_from(readings);
        }
        plan
    }

    /// 提交择优腿的一次换点：走故障腿那一个提交事务（磁盘侧只改实际出口，运行侧只换
    /// `proxy-selector`，读回确认，失败回滚，不重启内核）。意图不变。返回是否换成。
    ///
    /// 两条腿共用决策机上的在飞标志与冷却时刻：这里成败都进冷却；熔断只计故障腿，不在这里记。
    /// 结果回写状态槽：换成了记一次换点；没换成记下原因，连续失败的次数拉长下一次重试的间隔
    /// （见 [`auto_select::retry_after_ms`]），让位不计。
    pub(super) async fn auto_select_commit(
        self: &Arc<Self>,
        generation: u64,
        machine: &mut AutoSwitchMachine,
        plan: &SelectPlan,
        api: &dyn RuntimeSelectionApi,
        cache: &mut SelectCache,
        readings: &SelectReadings<'_>,
    ) -> bool {
        machine.begin_switch(readings.now);
        let outcome = self
            .auto_hot_switch_transaction_with_api(
                generation,
                &plan.current_id,
                &plan.candidate,
                &plan.fingerprint,
                Some(&plan.subscription),
                api,
            )
            .await;
        machine.end_switch();
        let target = &plan.candidate.name;
        let failure = match outcome {
            AutoHotSwitchOutcome::Applied => None,
            AutoHotSwitchOutcome::Busy => {
                log::info!("自动选择：生命周期事务在飞 → 本次换到 {target} 让位");
                Some(CommitFailure::Yielded)
            }
            AutoHotSwitchOutcome::Superseded => {
                log::info!(
                    "自动选择：配置、草稿、手动选择或内核世代已变化 → 本次换到 {target} 作废"
                );
                Some(CommitFailure::Yielded)
            }
            AutoHotSwitchOutcome::NotEligible => {
                // 评估时判它可热切、提交时被拒：缓存的资格已不可信。
                cache.verdicts.clear();
                log::warn!("自动选择：{target} 已不满足零重启热切条件 → 本次不换");
                Some(CommitFailure::NotEligible)
            }
            AutoHotSwitchOutcome::ReconcilePending { intent_generation } => {
                self.spawn_selector_reconciliation(generation, intent_generation);
                Some(CommitFailure::Reconciling)
            }
            AutoHotSwitchOutcome::Failed => {
                log::warn!("自动选择：换到 {target} 未能自证，已恢复原出口");
                Some(CommitFailure::Failed)
            }
        };
        if let Some(failure) = failure {
            let failures = {
                let mut slot = self.auto_select_slot();
                slot.record_commit_failure(failure, &plan.candidate.id, readings.now_wall);
                slot.memory.commit_failures()
            };
            if failure.counts() {
                log::warn!(
                    "自动选择：提交连续 {failures} 次没有换成，{} 秒后才会再试",
                    auto_select::retry_after_ms(failures) / 1_000
                );
            }
            self.auto_select_announce_from(readings);
            return false;
        }
        log::info!(
            "自动选择：已换点（择优腿，原因 {}）{} → {target}，延迟 {:?} → {}ms，胜出依据未证实={}",
            plan.cause.as_str(),
            plan.from_name.as_deref().unwrap_or(&plan.current_id),
            plan.from_latency_ms,
            plan.latency_ms,
            plan.unverified
        );
        self.auto_select_slot().record_switch(
            SwitchRecord {
                at: readings.now_wall,
                leg: Leg::Select,
                cause: plan.cause,
                from_id: Some(plan.current_id.clone()),
                from_name: plan.from_name.clone(),
                to_id: plan.candidate.id.clone(),
                to_name: plan.candidate.name.clone(),
                from_latency_ms: plan.from_latency_ms,
                to_latency_ms: Some(plan.latency_ms),
                unverified: Some(plan.unverified),
            },
            readings.now,
        );
        if let Some(emitter) = self.error_emitter.get() {
            emitter.emit_auto_node_switched(
                &crate::runtime::auto_switch::AutoNodeSwitchedPayload {
                    reason: plan.cause.as_str().to_string(),
                    new_server_name: plan.candidate.name.clone(),
                    latency: plan.latency_ms,
                    leg: Leg::Select,
                    cause: plan.cause,
                    old_server_name: plan.from_name.clone(),
                },
            );
        }
        self.auto_select_announce_from(readings);
        true
    }

    /// 心跳任务里择优腿的一步：评估，裁决为换点即提交。
    async fn auto_select_tick(
        self: &Arc<Self>,
        generation: u64,
        machine: &mut AutoSwitchMachine,
        cache: &mut SelectCache,
    ) {
        let readings = SelectReadings::live();
        self.auto_select_tick_from(generation, machine, cache, &readings, self.management_api())
            .await;
    }

    pub(super) async fn auto_select_tick_from<A: RuntimeSelectionApi>(
        self: &Arc<Self>,
        generation: u64,
        machine: &mut AutoSwitchMachine,
        cache: &mut SelectCache,
        readings: &SelectReadings<'_>,
        api: impl std::future::Future<Output = A>,
    ) {
        let Some(plan) = self
            .auto_select_assess(generation, machine.last_switch_time(), cache, readings)
            .await
        else {
            return;
        };
        let api = api.await;
        self.auto_select_commit(generation, machine, &plan, &api, cache, readings)
            .await;
    }
}

/// **C3**：应用层连通性检测：只经钉死到 `proxy-selector` 的专用 HTTP 入站，以绝对 URI GET
/// generate_204，任一端点返回 2xx/3xx → 判通。该入口不经过用户路由规则，因此结果只描述当前代理出口，
/// 不会被一条 direct 分流伪装成“节点健康”。**真机门**：需真起核 + 碰网络。
async fn probe_proxy_connectivity(probe_proxy_port: u16, auth: Option<&InboundUser>) -> bool {
    for url in CONNECTIVITY_URLS {
        if probe_through_proxy(probe_proxy_port, auth, url).await {
            return true;
        }
    }
    false
}

/// 连通性探针发给本机 http 入站的请求报文（absolute-form GET；`auth` 非空时带 `Proxy-Authorization`）。
///
/// 纯函数，抽出来只为让报文形状可单测（[`probe_through_proxy`] 本身要真起核 + 碰网络）。
/// 目标不是 `http://<host>/…` 形态 → `None`（取不到 Host 头）。
pub(super) fn connectivity_probe_request(
    target_url: &str,
    auth: Option<&InboundUser>,
) -> Option<String> {
    // 取 Host 头（`http://<host>/path` → `<host>`）。
    let host = target_url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .filter(|h| !h.is_empty())?;
    let proxy_auth = crate::runtime::http::proxy_authorization_line(auth);
    Some(format!(
        "GET {target_url} HTTP/1.1\r\nHost: {host}\r\n{proxy_auth}Proxy-Connection: close\r\nConnection: close\r\n\r\n"
    ))
}

/// 经指定的本地 HTTP 探针入口以绝对 URI GET 目标，判是否拿到 2xx/3xx。调用方负责保证该入口
/// 固定路由到待测出口；这里仅实现通用 HTTP 代理握手。**真机门**：需真起核 + 碰网络，禁本机单测。
///
/// `auth`：Android 上 `probe-proxy-in` 要求本次起核的一次性凭据；缺了内核回 407 ⇒ 每拍都判「不通」
/// ⇒ 连续失败后**自动换走一个好好的节点**。这是比「探针失效」更坏的失效形态，故凭据是必填参数。
async fn probe_through_proxy(
    proxy_port: u16,
    auth: Option<&InboundUser>,
    target_url: &str,
) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Some(request) = connectivity_probe_request(target_url, auth) else {
        return false;
    };
    let addr = format!("127.0.0.1:{proxy_port}");
    let probe = async {
        let mut stream = tokio::net::TcpStream::connect(&addr).await.ok()?;
        stream.write_all(request.as_bytes()).await.ok()?;
        // 只需状态行（`HTTP/1.1 204 No Content`）；读一小段即可解析首行。
        let mut buf = [0u8; 64];
        let n = stream.read(&mut buf).await.ok()?;
        let text = std::str::from_utf8(&buf[..n]).ok()?;
        let code: u32 = text.split_whitespace().nth(1)?.parse().ok()?;
        Some((200..400).contains(&code))
    };
    matches!(
        tokio::time::timeout(Duration::from_millis(CONNECTIVITY_TIMEOUT_MS), probe).await,
        Ok(Some(true))
    )
}
