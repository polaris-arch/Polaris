//! Canonical TS route files and local writer admission. Disk-only host routes are
//! weak LastKnown planning hints, never control-plane identity or membership.
//! A qualified current frame replaces weak legacy history; compatible known
//! history can accumulate within its admitted binding. File writes do not attest
//! core reload or reconfigure the already-applied physical TUN.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::PoisonError;

use polaris_config_engine::builder::endpoint_routes::{
    endpoint_forced_route_cidrs, tailnet_rule_file_base, ObservedTailnetAddresses,
    TAILNET_RULES_DIR_NAME,
};
use polaris_config_engine::builder::helpers::host_to_exclude_cidr;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::collections::dedupe_trim;
use polaris_config_engine::user_config::server_config::{Protocol, ServerConfig};

use super::startup::atomic_write_custom_rule;
use super::ProxyRuntime;
use crate::runtime::tailscale_status::TailscaleStatusEvent;
use polaris_config_engine::singbox::SingBoxConfig;
use polaris_config_engine::user_config::effective_view::tailscale_control_authority;
use polaris_config_engine::user_config::mesh_route_state::{MeshBindingState, MeshRouteState};
use serde_json::Value;

// Functional source/input byte boundary, independent of report CIDR display caps.
// Startup, hot preflight and readback share the existing 1 MiB source-file limit.
pub(super) const MAX_TAILNET_RULE_BYTES: usize = 1024 * 1024;

/// 文件内容形态：sing-box headless rule-set 的 **source JSON**（非二进制 `.srs`）。
///
/// 与 `custom_rule_files::rule_file_json` 同款 `{version:1, rules:[…]}` + 2 空格缩进
/// （`to_string_pretty`）——那个函数是 crate 私有的，此处不是第二份形态定义，是同一形态的
/// 同构写法；**若两边形态哪天分家，读取侧（块 0c 的 `RuleSet{format:"source"}`）会先炸**。
#[must_use]
pub(super) fn tailnet_rule_file_json(cidrs: &[String]) -> String {
    // A zero-condition rule is invalid in the Core. Empty knowledge must be a
    // genuine match-none rule set; keep the nonempty canonical bytes unchanged.
    if cidrs.is_empty() {
        return serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "rules": [],
        }))
        .expect("tailnet rule-set JSON 序列化不应失败");
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "version": 1,
        "rules": [ { "ip_cidr": cidrs } ],
    }))
    .expect("tailnet rule-set JSON 序列化不应失败")
}

/// 读回文件声明的全部 `ip_cidr`（遍历所有 rule，不假设只有一条）。
///
/// 解析不出 → `None`，由调用方当作「没有上次已知集合」。**不保留一个解析不出的文件**：
/// `NewLocalRuleSet` 起核时首次 `reloadFile` 失败即整个核起不来，而这个文件是上一次会话
/// 写的、可能被断电/外部工具弄坏 —— 起核前腿按配置期段重写它，是这条链上唯一的修复点。
#[must_use]
pub(super) fn parse_tailnet_rule_file_cidrs(content: &str) -> Option<Vec<String>> {
    if content.len() > MAX_TAILNET_RULE_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let rules = value.get("rules")?.as_array()?;
    let mut out = Vec::new();
    for rule in rules {
        let Some(value) = rule.get("ip_cidr") else {
            continue;
        };
        let list = value.as_array()?;
        for cidr in list {
            let text = cidr.as_str()?;
            if polaris_config_engine::user_config::cidr::normalize_cidr(text).is_none() {
                return None;
            }
            out.push(text.to_owned());
        }
    }
    Some(out)
}

/// 主机位 CIDR（`/32` / `/128`）→ 裸地址；其余（`100.64.0.0/10` 这类段）→ `None`。
///
/// 这是 [`host_to_exclude_cidr`] 的逆向，只用于把盘上文件读回成观测面。**只认主机位**：
/// 配置期已知段（官方默认两段、用户 `routes`）由 [`endpoint_forced_route_cidrs`] 每次重算，
/// 把它们也读回观测面会让用户删掉的 `routes` 永远删不掉。
#[must_use]
pub(super) fn bare_addr_of_host_cidr(cidr: &str) -> Option<&str> {
    let (addr, mask) = cidr.trim().rsplit_once('/')?;
    match (addr.parse::<IpAddr>().ok()?, mask) {
        (IpAddr::V4(_), "32") | (IpAddr::V6(_), "128") => Some(addr),
        _ => None,
    }
}

/// 一帧里某 endpoint 的全部 tailnet 地址 = self ∪ 全部 peer。
///
/// **离线 peer 也算**：`online: bool` 这个字段存在本身就证明离线 peer 仍在 netmap 里，漏掉
/// 它们会让那台机器上线的那一刻路由不到（而那正是用户会去按的时候）。
///
/// 取 `details.user_groups[].peers[]` **与**顶层 `peers[]` 的并集：顶层那份是摊平后**按
/// hostName 去重**的 UI lean 投影，同名两台机器会被吃掉一台；`user_groups` 是未去重的全量。
/// 并集对「漏掉一台机器」这个风险单调更安全，成本是几次 clone。
#[must_use]
pub(super) fn observed_addresses_of_event(event: &TailscaleStatusEvent) -> Vec<String> {
    let mut out: Vec<String> = event.tailscale_ips.clone();
    for group in &event.details.user_groups {
        for peer in &group.peers {
            out.extend(peer.details.tailscale_ips.iter().cloned());
        }
    }
    for peer in &event.peers {
        out.extend(peer.details.tailscale_ips.iter().cloned());
    }
    dedupe_trim(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TailnetIdentity {
    authority: String,
    epoch: Option<String>,
    stable_id: Option<String>,
    retired: bool,
}

impl TailnetIdentity {
    fn compatible(&self, other: &Self) -> bool {
        !self.retired
            && !other.retired
            && self.authority == other.authority
            && !matches!((&self.epoch, &other.epoch), (Some(a), Some(b)) if a != b)
            && !matches!((&self.stable_id, &other.stable_id), (Some(a), Some(b)) if a != b)
    }
}

fn configured_identity(raw: &Value, server: &ServerConfig) -> Option<TailnetIdentity> {
    let authority = tailscale_control_authority(&serde_json::to_value(server).ok()?).ok()?;
    let mut result = TailnetIdentity {
        authority,
        epoch: None,
        stable_id: None,
        retired: false,
    };
    if let Some(value) = raw.get("meshRouteState").filter(|value| !value.is_null()) {
        let ledger: MeshRouteState = serde_json::from_value(value.clone()).ok()?;
        // Match mesh_identity_reconcile::current_identity: retired epochs remain
        // in the ledger after retire+append and are not duplicate current owners.
        let mut identities = ledger.identities.iter().filter(|item| {
            item.server_id == server.id
                && matches!(
                    item.binding_state,
                    MeshBindingState::Bound | MeshBindingState::Unbound
                )
        });
        if let Some(identity) = identities.next() {
            if identities.next().is_some() {
                return None;
            }
            let canonical = polaris_config_engine::user_config::mesh_identity_reconcile::canonical_control_authority(&identity.control_authority).ok()?;
            if canonical != result.authority {
                return None;
            }
            result.epoch = Some(identity.identity_epoch.clone());
            result.stable_id = identity.self_stable_id.clone().filter(|id| !id.is_empty());
        } else {
            result.retired = ledger.identities.iter().any(|item| {
                item.server_id == server.id
                    && matches!(
                        item.binding_state,
                        MeshBindingState::Retiring | MeshBindingState::Retired
                    )
            });
        }
    }
    Some(result)
}

/// The raw generation axis omitted by UserConfig's typed norm. Reuse the same
/// identity resolution as planning; ledger revision, observations and transaction
/// metadata are not generation inputs. This carries no hosts or current evidence.
pub(super) fn tailnet_generation_identity(
    raw: &Value,
) -> Option<BTreeMap<String, Option<TailnetIdentity>>> {
    let user: UserConfig = serde_json::from_value(raw.clone()).ok()?;
    let mut result = BTreeMap::new();
    for server in &user.servers {
        if server.protocol != Protocol::Tailscale {
            continue;
        }
        if result.contains_key(&server.id) {
            return None;
        }
        result.insert(server.id.clone(), configured_identity(raw, server));
    }
    Some(result)
}

#[derive(Clone)]
pub(super) struct TailnetRouteBinding {
    server: ServerConfig,
    tag: String,
    locator: Option<String>,
    descriptor: polaris_config_engine::singbox::Endpoint,
    identity: TailnetIdentity,
}
impl TailnetRouteBinding {
    fn same_writer(&self, other: &Self) -> bool {
        self.tag == other.tag
            && self.locator == other.locator
            && self.descriptor == other.descriptor
            && self.identity.compatible(&other.identity)
            && self.server.tailscale_settings.as_ref().map(|s| &s.routes)
                == other.server.tailscale_settings.as_ref().map(|s| &s.routes)
    }
}

#[derive(Clone)]
pub(super) struct TailnetRouteContext {
    pub(super) generation: u64,
    pub(super) token: u64,
    pub(super) bindings: BTreeMap<String, TailnetRouteBinding>,
    pub(super) suspended: bool,
}
impl TailnetRouteContext {
    pub(super) fn tag_to_id(&self) -> BTreeMap<String, String> {
        self.bindings
            .iter()
            .map(|(id, binding)| (binding.tag.clone(), id.clone()))
            .collect()
    }
}
#[derive(Clone, Default)]
struct TailnetHistory {
    // None means legacy disk data: do not stamp the current authority onto it.
    identity: Option<TailnetIdentity>,
    // Negative knowledge from an actual admitted context, not provenance of H.
    // In particular this does NOT turn a disk-only identity=None into observed
    // membership. Keep it across Stop so an in-process target change cannot
    // erase known incompatibility by first withdrawing writer authority.
    last_binding_identity: Option<TailnetIdentity>,
    hosts: Vec<String>,
    fresh_token: Option<u64>,
    fresh_hosts: Vec<String>,
}
impl TailnetHistory {
    fn accepts_target(&self, target: &TailnetIdentity) -> bool {
        self.identity
            .as_ref()
            .is_none_or(|known| known.compatible(target))
            && self
                .last_binding_identity
                .as_ref()
                .is_none_or(|known| known.compatible(target))
    }
}
#[derive(Clone, Default)]
pub(super) struct TailnetRouteState {
    next_token: u64,
    pub(super) limited: bool,
    pub(super) active: Option<TailnetRouteContext>,
    history: BTreeMap<String, TailnetHistory>,
}

impl TailnetRouteState {
    fn active_binding_accepts_target(&self, id: &str, target: &TailnetIdentity) -> bool {
        self.active
            .as_ref()
            .and_then(|context| context.bindings.get(id))
            .is_none_or(|binding| binding.identity.compatible(target))
    }
    fn compatible_history(&self, id: &str, target: &TailnetIdentity) -> Option<&TailnetHistory> {
        self.history.get(id).filter(|history| {
            history.accepts_target(target) && self.active_binding_accepts_target(id, target)
        })
    }
}

fn valid_hosts(addrs: Vec<String>) -> Vec<String> {
    dedupe_trim(
        addrs
            .into_iter()
            .filter_map(|addr| addr.trim().parse::<IpAddr>().ok().map(|ip| ip.to_string()))
            .collect::<Vec<_>>(),
    )
}

fn canonical_content(server: &ServerConfig, hosts: &[String]) -> Result<String, String> {
    let observed = BTreeMap::from([(server.id.clone(), hosts.to_vec())]);
    let content = tailnet_rule_file_json(&endpoint_forced_route_cidrs(server, &observed));
    if content.len() > MAX_TAILNET_RULE_BYTES {
        return Err("tailnet source exceeds functional byte limit".into());
    }
    Ok(content)
}

// Count the complete decoded address input, including invalid/duplicate strings,
// before normalization. No prefix is silently accepted at the expense of its tail.
fn event_input_within_limit(event: &TailscaleStatusEvent) -> bool {
    event
        .tailscale_ips
        .iter()
        .chain(
            event
                .details
                .user_groups
                .iter()
                .flat_map(|group| &group.peers)
                .flat_map(|peer| &peer.details.tailscale_ips),
        )
        .chain(
            event
                .peers
                .iter()
                .flat_map(|peer| &peer.details.tailscale_ips),
        )
        .try_fold(0usize, |bytes, addr| {
            bytes
                .checked_add(addr.len())
                .filter(|bytes| *bytes <= MAX_TAILNET_RULE_BYTES)
        })
        .is_some()
}

impl ProxyRuntime {
    /// Capture only final emitted TS descriptors. Duplicate IDs/tags or an
    /// authority disagreement fail closed; configured-but-peeled nodes have no writer.
    pub(super) fn tailnet_bindings(
        &self,
        raw: &Value,
        generated: &SingBoxConfig,
    ) -> BTreeMap<String, TailnetRouteBinding> {
        let Ok(user) = serde_json::from_value::<UserConfig>(raw.clone()) else {
            return BTreeMap::new();
        };
        let mut bindings = BTreeMap::new();
        let mut tags = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        for server in &user.servers {
            if server.id.is_empty() || !ids.insert(server.id.clone()) {
                return BTreeMap::new();
            }
            if server.protocol != Protocol::Tailscale {
                continue;
            }
            let tag = Self::endpoint_tag_to_id(&user)
                .into_iter()
                .find_map(|(tag, id)| (id == server.id).then_some(tag));
            let Some(tag) = tag else {
                return BTreeMap::new();
            };
            let emitted: Vec<_> = generated
                .endpoints
                .iter()
                .flatten()
                .filter(|endpoint| endpoint.type_field == "tailscale" && endpoint.tag == tag)
                .collect();
            let [endpoint] = emitted.as_slice() else {
                if emitted.is_empty() {
                    continue;
                }
                return BTreeMap::new();
            };
            if !tags.insert(tag.clone()) {
                return BTreeMap::new();
            }
            let Some(identity) =
                configured_identity(raw, server).filter(|identity| !identity.retired)
            else {
                continue;
            };
            let emitted_authority = polaris_config_engine::user_config::mesh_identity_reconcile::canonical_control_authority(endpoint.control_url.as_deref().filter(|url| !url.is_empty()).unwrap_or("https://controlplane.tailscale.com")).ok();
            if emitted_authority.as_deref() != Some(identity.authority.as_str()) {
                continue;
            }
            let mut descriptor = (**endpoint).clone();
            descriptor.control_url = Some(identity.authority.clone());
            bindings.insert(
                server.id.clone(),
                TailnetRouteBinding {
                    server: server.clone(),
                    tag,
                    locator: endpoint.state_directory.clone(),
                    descriptor,
                    identity,
                },
            );
        }
        bindings
    }

    /// Caller holds publication; no gate reentry or async work inside the state lock.
    pub(super) fn activate_tailnet_context(
        &self,
        generation: u64,
        mut bindings: BTreeMap<String, TailnetRouteBinding>,
    ) -> bool {
        let mut state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.active.as_ref().is_some_and(|active| {
            active.generation == generation
                && !active.suspended
                && active.bindings.len() == bindings.len()
                && active.bindings.iter().all(|(id, binding)| {
                    bindings
                        .get(id)
                        .is_some_and(|next| binding.same_writer(next))
                })
        }) {
            return false;
        }
        let mut next = state.clone();
        for (id, binding) in &mut bindings {
            if let Some(prior) = next
                .history
                .get(id)
                .and_then(|history| history.identity.as_ref())
                .filter(|prior| prior.compatible(&binding.identity))
            {
                if binding.identity.stable_id.is_none() {
                    binding.identity.stable_id = prior.stable_id.clone();
                }
                if binding.identity.epoch.is_none() {
                    binding.identity.epoch = prior.epoch.clone();
                }
            }
            if next.history.contains_key(id)
                && next.compatible_history(id, &binding.identity).is_none()
            {
                next.history.remove(id);
            }
        }
        for history in next.history.values_mut() {
            history.fresh_token = None;
            history.fresh_hosts.clear();
        }
        next.next_token = next.next_token.wrapping_add(1).max(1);
        let token = next.next_token;
        let context = TailnetRouteContext {
            generation,
            token,
            bindings,
            suspended: false,
        };
        next.limited = false;
        let mut writes = Vec::new();
        for (id, binding) in &context.bindings {
            if let Some(history) = next.history.get_mut(id) {
                history.last_binding_identity = Some(binding.identity.clone());
            }
            let hosts = next
                .history
                .get(id)
                .map(|history| history.hosts.clone())
                .unwrap_or_default();
            match self.prepare_tailnet_binding(binding, &hosts) {
                Ok(Some(write)) => writes.push(write),
                Ok(None) => {}
                Err(error) => {
                    // A committed graph must never keep its previous writer if
                    // this final defensive preflight fails. Preserve H/files;
                    // revoke writing and report Unknown instead of partial writes.
                    state.limited = true;
                    let token = state
                        .active
                        .as_ref()
                        .filter(|active| active.generation == generation)
                        .map(|active| active.token);
                    self.invalidate_tailnet_route_ack(generation, token);
                    state.active = None;
                    log::warn!("tailnet activation refused: {error}");
                    return true;
                }
            }
        }
        for (path, content) in writes {
            self.commit_tailnet_write(&path, &content);
        }
        next.active = Some(context);
        *state = next;
        true
    }
    pub(super) fn tailnet_context(&self) -> Option<TailnetRouteContext> {
        self.tailnet_routes.lock().ok()?.active.clone()
    }
    pub(super) fn revoke_tailnet_before_generation(&self, generation: u64) {
        let mut state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.generation < generation)
        {
            state.active = None;
            for history in state.history.values_mut() {
                history.fresh_token = None;
                history.fresh_hosts.clear();
            }
        }
    }
    pub(super) fn revoke_tailnet_context(&self) {
        let mut state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.active = None;
        for history in state.history.values_mut() {
            history.fresh_token = None;
            history.fresh_hosts.clear();
        }
    }
    pub(super) fn suspend_tailnet_context(
        &self,
        next: &BTreeMap<String, TailnetRouteBinding>,
    ) -> Option<u64> {
        let mut state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let active = state.active.as_mut()?;
        if active.bindings.len() == next.len()
            && active
                .bindings
                .iter()
                .all(|(id, binding)| next.get(id).is_some_and(|item| binding.same_writer(item)))
        {
            return None;
        }
        active.suspended = true;
        Some(active.token)
    }
    pub(super) fn resume_tailnet_context(&self, generation: u64, token: u64) {
        if let Ok(mut state) = self.tailnet_routes.lock() {
            if let Some(active) = state
                .active
                .as_mut()
                .filter(|active| active.generation == generation && active.token == token)
            {
                active.suspended = false;
            }
        }
    }
    pub(crate) fn observed_tailnet_snapshot(&self) -> ObservedTailnetAddresses {
        self.tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .history
            .iter()
            .map(|(id, history)| (id.clone(), history.hosts.clone()))
            .collect()
    }
    /// Target D/next-start projection. Known incompatible identity is excluded;
    /// unknown legacy remains a weak planning hint and is never made fresh here.
    pub(crate) fn observed_tailnet_for_config(&self, raw: &Value) -> ObservedTailnetAddresses {
        let Ok(user) = serde_json::from_value::<UserConfig>(raw.clone()) else {
            return BTreeMap::new();
        };
        let state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut result = BTreeMap::new();
        let mut ids = std::collections::BTreeSet::new();
        for server in &user.servers {
            if !ids.insert(&server.id) {
                return BTreeMap::new();
            }
            if server.protocol != Protocol::Tailscale {
                continue;
            }
            let Some(target) =
                configured_identity(raw, server).filter(|identity| !identity.retired)
            else {
                continue;
            };
            if let Some(history) = state.compatible_history(&server.id, &target) {
                result.insert(server.id.clone(), history.hosts.clone());
            }
        }
        result
    }
    /// Runtime candidate may only subtract incompatible accepted planning hosts.
    /// It must not replace accepted physical TUN facts with a newer STATUS snapshot.
    pub(super) fn accepted_tailnet_for_config(
        &self,
        accepted: &Value,
        target: &Value,
        hosts: &ObservedTailnetAddresses,
    ) -> ObservedTailnetAddresses {
        let (Ok(previous), Ok(next)) = (
            serde_json::from_value::<UserConfig>(accepted.clone()),
            serde_json::from_value::<UserConfig>(target.clone()),
        ) else {
            return BTreeMap::new();
        };
        let state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut ids = std::collections::BTreeSet::new();
        let mut result = BTreeMap::new();
        for server in &next.servers {
            if !ids.insert(&server.id) {
                return BTreeMap::new();
            }
            if server.protocol != Protocol::Tailscale {
                continue;
            }
            let Some(identity) =
                configured_identity(target, server).filter(|identity| !identity.retired)
            else {
                continue;
            };
            let mut owners = previous
                .servers
                .iter()
                .filter(|old| old.id == server.id && old.protocol == Protocol::Tailscale);
            let old = owners.next();
            if owners.next().is_some() {
                return BTreeMap::new();
            }
            if !old
                .and_then(|old| configured_identity(accepted, old))
                .is_some_and(|old| old.compatible(&identity))
            {
                continue;
            }
            if !state.active_binding_accepts_target(&server.id, &identity)
                || (state.history.contains_key(&server.id)
                    && state.compatible_history(&server.id, &identity).is_none())
            {
                continue;
            }
            if let Some(hosts) = hosts.get(&server.id) {
                result.insert(server.id.clone(), hosts.clone());
            }
        }
        result
    }
    pub(super) fn tailnet_bindings_within_limit(
        &self,
        bindings: &BTreeMap<String, TailnetRouteBinding>,
    ) -> bool {
        let state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        bindings.iter().all(|(id, binding)| {
            let hosts = state
                .compatible_history(id, &binding.identity)
                .map(|history| history.hosts.as_slice())
                .unwrap_or_default();
            canonical_content(&binding.server, hosts).is_ok()
        })
    }
    pub(super) fn tailnet_routes_limited(&self) -> bool {
        self.tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .limited
    }
    pub(super) fn fresh_tailnet_snapshot(&self, generation: u64) -> ObservedTailnetAddresses {
        let state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(active) = state
            .active
            .as_ref()
            .filter(|active| active.generation == generation && !active.suspended)
        else {
            return BTreeMap::new();
        };
        state
            .history
            .iter()
            .filter(|(id, history)| {
                active.bindings.contains_key(*id) && history.fresh_token == Some(active.token)
            })
            .map(|(id, history)| (id.clone(), history.fresh_hosts.clone()))
            .collect()
    }

    /// 诊断写代在实际文件写入前进入奇数、结束后回偶数；锁使并发写者不能互相抵消奇数窗口。
    /// 这只影响报告证据，不改变文件内容或运行路由。
    pub(super) fn write_tailnet_file_with_epoch(
        &self,
        path: &std::path::Path,
        content: &str,
        remove_on_error: bool,
    ) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        let _guard = self
            .tailnet_file_write_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.tailnet_file_write_epoch.fetch_add(1, Ordering::SeqCst);
        let result = atomic_write_custom_rule(path, content);
        if result.is_err() && remove_on_error {
            let _ = std::fs::remove_file(path);
        }
        self.tailnet_file_write_epoch.fetch_add(1, Ordering::SeqCst);
        result
    }

    /// tailnet rule-set 目录（`<configDir>/tailnet-rules`）。
    ///
    /// 落盘侧与 `generate_deps` 注入给 config-engine 的 `tailnet_rules_dir` **同源**（单一真值，
    /// 同 [`custom_rules_dir`](Self::custom_rules_dir) 的理由）：目录名取 config-engine 导出的
    /// [`TAILNET_RULES_DIR_NAME`]，读侧（`builder::route` 判 rule-set 文件是否存在）用的是同一个常量。
    ///
    /// **为什么不各拼一次字符串**：写的和找的一旦分家，那条腿会静默 100% 不可达 —— 报告端会把
    /// 每个 Tailscale 节点都读成 inline 腿、系统性说反，而且两侧单独看都自洽、不会红。
    pub(super) fn tailnet_rules_dir(&self) -> PathBuf {
        self.config.dir().join(TAILNET_RULES_DIR_NAME)
    }

    /// Preparation before generate; the captured generation is checked at every
    /// synchronous disk/history commit. Legacy hosts remain unqualified history.
    pub(super) async fn write_tailnet_rule_files_for_generation(
        &self,
        config: &UserConfig,
        raw: &Value,
        generation: u64,
    ) -> Result<(), String> {
        if !config
            .servers
            .iter()
            .any(|server| server.protocol == Protocol::Tailscale)
        {
            return Ok(());
        }
        let _publication = self.gate.lock_generation_publication();
        let mut state = self
            .tailnet_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut capacity_refused = false;
        let result = self
            .gate
            .with_current_generation(generation, |_live| {
                let dir = self.tailnet_rules_dir();
                let mut next = state.clone();
                let mut writes = Vec::new();
                for server in config
                    .servers
                    .iter()
                    .filter(|server| server.protocol == Protocol::Tailscale)
                {
                    let target = configured_identity(raw, server)
                        .ok_or("ambiguous or invalid current tailnet identity")?;
                    let incompatible = target.retired
                        || !next.active_binding_accepts_target(&server.id, &target)
                        || (next.history.contains_key(&server.id)
                            && next.compatible_history(&server.id, &target).is_none());
                    if incompatible {
                        next.history.remove(&server.id);
                    }
                    let path = dir.join(format!("{}.json", tailnet_rule_file_base(&server.id)));
                    if !incompatible && !next.history.contains_key(&server.id) {
                        if let Some(cidrs) = read_tailnet_file(&path)
                            .and_then(|text| parse_tailnet_rule_file_cidrs(&text))
                        {
                            let hosts = valid_hosts(
                                cidrs
                                    .iter()
                                    .filter_map(|cidr| bare_addr_of_host_cidr(cidr))
                                    .map(str::to_owned)
                                    .collect(),
                            );
                            if !hosts.is_empty() {
                                let last_binding_identity = next
                                    .active
                                    .as_ref()
                                    .and_then(|context| context.bindings.get(&server.id))
                                    .map(|binding| binding.identity.clone());
                                next.history.insert(
                                    server.id.clone(),
                                    TailnetHistory {
                                        hosts,
                                        last_binding_identity,
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                    }
                    let hosts = next
                        .history
                        .get(&server.id)
                        .map(|history| history.hosts.as_slice())
                        .unwrap_or_default();
                    // Retired/incompatible history is never re-imported. Rebuild the
                    // existing cold/default + explicit declarations instead of leaving
                    // an old JSON for builder's actual exists-based registration.
                    let content = match canonical_content(server, hosts) {
                        Ok(content) => content,
                        Err(error) => {
                            state.limited = true;
                            capacity_refused = true;
                            return Err(error);
                        }
                    };
                    if read_tailnet_file(&path).as_deref() != Some(content.as_str()) {
                        writes.push((path, content, incompatible));
                    }
                }
                // All sources fit before any file or retained-history commit.
                if std::fs::create_dir_all(&dir).is_err() {
                    if writes
                        .iter()
                        .any(|(path, _, incompatible)| *incompatible && path.exists())
                    {
                        return Err("cannot replace retired tailnet source".into());
                    }
                    next.limited = false;
                    *state = next;
                    return Ok(()); // original missing-file inline fallback.
                }
                for (path, content, incompatible) in writes {
                    if let Err(error) = self.write_tailnet_file_with_epoch(&path, &content, true) {
                        log::warn!("tailnet preparation failed: {} ({error})", path.display());
                        if incompatible && path.exists() {
                            return Err(
                                "retired tailnet source could not be replaced or removed".into()
                            );
                        }
                    }
                }
                next.limited = false;
                *state = next;
                Ok(())
            })
            .unwrap_or(Ok(()));
        if capacity_refused {
            let token = state
                .active
                .as_ref()
                .filter(|active| active.generation == generation)
                .map(|active| active.token);
            self.invalidate_tailnet_route_ack(generation, token);
        }
        result
    }

    fn prepare_tailnet_binding(
        &self,
        binding: &TailnetRouteBinding,
        hosts: &[String],
    ) -> Result<Option<(PathBuf, String)>, String> {
        let content = canonical_content(&binding.server, hosts)?;
        let path = self.tailnet_rules_dir().join(format!(
            "{}.json",
            tailnet_rule_file_base(&binding.server.id)
        ));
        let Some(current) = read_tailnet_file(&path) else {
            return Ok(None);
        };
        let Some(current_cidrs) = parse_tailnet_rule_file_cidrs(&current) else {
            return Ok(None);
        };
        if Some(current_cidrs) == parse_tailnet_rule_file_cidrs(&content) {
            return Ok(None);
        }
        Ok(Some((path, content)))
    }
    fn commit_tailnet_write(&self, path: &std::path::Path, content: &str) {
        if let Err(error) = self.write_tailnet_file_with_epoch(path, content, false) {
            log::warn!("tailnet hot write failed: {} ({error})", path.display());
        }
    }

    /// Actual changed selfStableID rotates only the TS token. Missing identity
    /// is Unknown and cannot log out, clear history or change the writer.
    pub(super) fn learn_tailnet_identity(
        &self,
        state: &mut TailnetRouteState,
        events: &[TailscaleStatusEvent],
    ) -> bool {
        let Some(mut context) = state.active.clone() else {
            return false;
        };
        let mut changed = false;
        for event in events {
            if valid_hosts(observed_addresses_of_event(event)).is_empty() {
                continue;
            }
            let Some(binding) = context.bindings.get_mut(&event.server_id) else {
                continue;
            };
            let Some(stable) = event
                .details
                .self_peer
                .as_ref()
                .and_then(|peer| peer.stable_id.as_ref())
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            if binding.identity.stable_id.as_ref() == Some(stable) {
                continue;
            }
            if binding.identity.stable_id.is_some() {
                state.history.remove(&event.server_id);
            }
            binding.identity.stable_id = Some(stable.clone());
            changed = true;
        }
        if changed {
            state.next_token = state.next_token.wrapping_add(1).max(1);
            context.token = state.next_token;
            for history in state.history.values_mut() {
                history.fresh_token = None;
                history.fresh_hosts.clear();
            }
            state.active = Some(context);
        }
        changed
    }

    /// Only the admitted relay calls this while holding publication/state/gate.
    /// Recompute from captured declarations, never union current file CIDRs.
    pub(super) fn commit_tailnet_events(
        &self,
        state: &mut TailnetRouteState,
        context: &TailnetRouteContext,
        events: &[TailscaleStatusEvent],
    ) -> Result<(), String> {
        let mut writes = Vec::new();
        let mut accepted_hosts = false;
        for event in events {
            let Some(binding) = context.bindings.get(&event.server_id) else {
                continue;
            };
            if !event_input_within_limit(event) {
                return Err("tailnet input exceeds functional byte limit".into());
            }
            let hosts = valid_hosts(observed_addresses_of_event(event));
            if hosts.is_empty() {
                continue;
            }
            accepted_hosts = true;
            let mut identity = binding.identity.clone();
            let stable = event
                .details
                .self_peer
                .as_ref()
                .and_then(|peer| peer.stable_id.clone())
                .filter(|id| !id.is_empty());
            if let Some(stable) = stable {
                identity.stable_id = Some(stable);
            }
            let history = state.history.entry(event.server_id.clone()).or_default();
            if history
                .identity
                .as_ref()
                .is_none_or(|prior| !prior.compatible(&identity))
            {
                history.hosts.clear(); // first qualified frame cannot promote weak legacy hosts.
            }
            if history.fresh_token != Some(context.token) {
                history.fresh_hosts.clear();
            }
            history.fresh_hosts.extend(hosts.iter().cloned());
            history.fresh_hosts = valid_hosts(std::mem::take(&mut history.fresh_hosts));
            history.hosts.extend(hosts);
            history.hosts = valid_hosts(std::mem::take(&mut history.hosts));
            history.last_binding_identity = Some(binding.identity.clone());
            history.identity = Some(identity);
            history.fresh_token = Some(context.token);
            if let Some(write) = self.prepare_tailnet_binding(binding, &history.hosts)? {
                writes.push(write);
            }
        }
        // Preflight the entire frame, including declaration/MagicDNS overhead,
        // before changing any file. Caller publishes this staged state only on Ok.
        for (path, content) in writes {
            self.commit_tailnet_write(&path, &content);
        }
        if accepted_hosts {
            state.limited = false;
        }
        Ok(())
    }
}

// Read exactly the same functional byte boundary used before startup/hot writes.
fn read_tailnet_file(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_TAILNET_RULE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_TAILNET_RULE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}
