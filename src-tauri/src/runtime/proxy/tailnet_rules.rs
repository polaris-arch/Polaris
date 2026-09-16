//! A-0b：把**运行期观测到的 tailnet 地址**变成盘上真值（tailnet rule-set 文件的落盘 + 热更）。
//!
//! # 这条腿为什么存在
//!
//! `builder::endpoint_routes` 的 `TAILNET_CGNAT` = `100.64.0.0/10` 是**上游官方控制面的默认
//! 前缀**，不是协议常量：headscale 的 `prefixes.v4` 可自定义。2026-09-11 真控制面实测，某自建
//! tailnet 把地址发成 `32.0.0.28` / `32.0.0.29` —— 而 `32.0.0.0/8` 是 IANA 分配给 AT&T 的真实
//! 公网段。A-0a 已把「观测地址并进 force-route 覆盖面」的**发射面**做完（四个消费者全改到），
//! 但 `GenerateConfigDeps::observed_tailnet_addresses` 当时恒空、`tailnet_rules_dir` 只给路径不
//! 落盘 —— 本模块补的就是那个真值源。
//!
//! # 两条腿（形态照搬 `startup::write_custom_rule_files` / `sync_custom_rule_files`）
//!
//! - **起核前**（`start_inner` 在 generate 之前调 [`ProxyRuntime::write_tailnet_rule_files`]）：
//!   按**上次已知**的集合落盘。generate 的块 0c 按文件真存在性（`ext_rule_file_exists`）决定
//!   走 `rule_set` 引用还是回落 inline，故必须在 generate 之前。
//! - **运行中**（`apply_ts_status_frame` 每帧调 [`ProxyRuntime::sync_tailnet_rule_files`]）：
//!   收到 STATUS 帧、集合变了 ⇒ 原子替换 ⇒ sing-box fswatch 自动 reload，**绝不删文件**
//!   （运行中删被挂载的文件会致 reload 报错；`NewLocalRuleSet` 首次 reloadFile 失败更是直接
//!   让核起不来）。
//!
//! # 持久层就是那个 JSON 文件本身
//!
//! 观测面必须活过 app 重启，否则每次冷启动那段「还没收到首帧」的窗口里整个 tailnet 不可路由。
//! 不另造一份存储：起核前腿把盘上文件的**主机位条目**（`/32`、`/128` —— 观测地址就是这么写
//! 进去的）读回内存观测面，于是文件同时是「给内核吃的那一份」和「上次已知集合」。
//!
//! 这条设计有一个如实登记的代价：一台被移出 tailnet 的机器，其地址会以 `/32` 形式**永久**留
//! 在文件里。选它而不是「每次重算、丢掉盘上值」是因为两侧代价不对称 —— 残留一条指向同一个
//! tailscale endpoint 的主机路由（那个地址空间本就归这个 tailnet），与冷启动期整段 tailnet
//! 不可路由，不是一个量级。
//!
//! # 空集是「什么都别做」，不是「写个空文件」
//!
//! 空 rule-set 内核收得下，但文件一旦存在，块 0c 就走 `rule_set` 腿并 `continue` —— inline 的
//! 两条官方默认段一条都不会发。于是「空文件」= 该节点 tailnet 全断，比没有文件坏得多。故：
//! 算出来的 cidr 集为空 ⇒ 不写；收到不含任何地址的帧（核刚起、netmap 还没同步）⇒ 保持原样。
//!
//! # 这个目录不做孤儿清扫（与 `custom-rules` 的**刻意**差异）
//!
//! `custom-rules` 的期望集是纯函数 `build_custom_rule_files(config)` 的全量产物，凡不在其中的
//! 都是孤儿。tailnet 文件不是那种形态：它的内容有一半来自运行期观测，而「本轮该有哪些文件」
//! 只在有 TS 节点时才谈得上。删一个仍被在跑的核挂载的文件是硬故障，收益却只是省下几 KB。

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

/// 文件内容形态：sing-box headless rule-set 的 **source JSON**（非二进制 `.srs`）。
///
/// 与 `custom_rule_files::rule_file_json` 同款 `{version:1, rules:[…]}` + 2 空格缩进
/// （`to_string_pretty`）——那个函数是 crate 私有的，此处不是第二份形态定义，是同一形态的
/// 同构写法；**若两边形态哪天分家，读取侧（块 0c 的 `RuleSet{format:"source"}`）会先炸**。
#[must_use]
pub(super) fn tailnet_rule_file_json(cidrs: &[String]) -> String {
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
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let rules = value.get("rules")?.as_array()?;
    let mut out = Vec::new();
    for rule in rules {
        let Some(list) = rule.get("ip_cidr").and_then(serde_json::Value::as_array) else {
            continue;
        };
        out.extend(list.iter().filter_map(|c| c.as_str().map(str::to_owned)));
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
    matches!(mask, "32" | "128").then_some(addr)
}

/// 裸地址 → 主机位 CIDR（复用 config-engine 的单一真值；非 IP 字面量直接丢弃）。
///
/// 丢弃而非报错：把一个抓歪的串放进 `ip_cidr` 会让 sing-box `netip.ParsePrefix` 启动 FATAL，
/// 而观测通道的输入是运行期从核里抓来的、不由本侧校验过（同 `endpoint_routes` 的口径）。
#[must_use]
fn host_cidrs(addrs: &[String]) -> Vec<String> {
    dedupe_trim(
        addrs
            .iter()
            .filter_map(|a| host_to_exclude_cidr(a.trim()))
            .collect::<Vec<_>>(),
    )
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

impl ProxyRuntime {
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

    /// 观测面快照（注入 `GenerateConfigDeps::observed_tailnet_addresses`）。
    ///
    /// 空 map 是合法值 = 「本轮没有任何运行期观测」⇒ 产出与观测面存在之前逐字节相同。
    ///
    /// **`pub(crate)` 而非 `pub(super)`**：`commands::config` 的 TUN 排除面预览必须拿到
    /// **同一份**快照喂 `preview_tun_exclusion` —— 那个模块的存在理由就是「预览是读回来的、
    /// 不是另算一份」，它少拿一个输入就等于又分叉出一份计算（见该模块头注那次事故）。
    pub(crate) fn observed_tailnet_snapshot(&self) -> ObservedTailnetAddresses {
        self.observed_tailnet
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 把一批观测地址并进内存观测面（**并集，不替换**）。
    ///
    /// 并集而非替换：观测面本身可能残缺（peer 还没上线 / 控制面刚换前缀 / 观测通道降级），
    /// 拿一份可能残缺的快照去顶掉手里已有的，方向是错的 —— 丢一条 = 那台机器的流量落
    /// `final` 去公网。
    ///
    /// **注意这里的「并集」与 `endpoint_routes` 那边的语义不是一回事**，别照着改：
    /// 本函数并的是「同一节点跨帧的观测地址」，越并越全；而 `endpoint_routes` 那边讲的是
    /// 「观测段 vs 默认段常量」，已改为**有观测则取代默认段**（并集会让两个 tailnet 节点
    /// 撞在同样的默认两段上、后声明者被去重吸收）。两处都叫并集，管的是不同的轴。
    fn merge_observed(&self, server_id: &str, addrs: Vec<String>) {
        if addrs.is_empty() {
            return;
        }
        let mut guard = self
            .observed_tailnet
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let entry = guard.entry(server_id.to_owned()).or_default();
        let mut merged = std::mem::take(entry);
        merged.extend(addrs);
        *entry = dedupe_trim(merged);
    }

    /// **起核前**落盘 tailnet rule-set 文件（`start_inner` 在 generate 之前调）。
    ///
    /// 顺序（两趟，刻意分开）：
    /// 1. 先把盘上每个 TS 节点文件的主机位条目读回内存观测面 —— 盘是唯一的持久层，本进程
    ///    刚起来时内存里什么都没有；
    /// 2. 再按 `endpoint_forced_route_cidrs(server, 观测面)` **重算**每个文件的完整内容。
    ///
    /// 重算而非「原样保留盘上内容」有两个不可省的作用：① 用户改了 `tailscale.routes` 能生效
    /// （原样保留会让删掉的段永远留着）；② 上一次会话留下的坏文件在这里被修好（见
    /// [`parse_tailnet_rule_file_cidrs`]）。
    ///
    /// 失败一律降级、不抛：mkdir 失败 → 整条腿放弃；逐文件写失败 → **删旧副本** + warn。
    /// 删旧副本是对齐 `write_custom_rule_files` 的先例，且此刻核还没起、文件没被挂载，删是
    /// 安全的；删掉之后块 0c 按 `ext_rule_file_exists` 回落 inline，而 inline 腿吃的是
    /// `deps.observed_tailnet_addresses`（本方法第 1 趟已经把观测面填好）⇒ **功能不损**。
    /// 反过来「写失败就留着旧文件」才是坏的：块 0c 会走 rule_set 腿用那份陈旧内容，新观测
    /// 一个字节都进不去。
    pub(super) async fn write_tailnet_rule_files(&self, config: &UserConfig) {
        let tailscale_nodes: Vec<&ServerConfig> = config
            .servers
            .iter()
            .filter(|s| s.protocol == Protocol::Tailscale)
            .collect();
        if tailscale_nodes.is_empty() {
            return;
        }
        let dir = self.tailnet_rules_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!(
                "落盘 tailnet 规则文件失败（回退 inline）：创建目录 {} 失败：{e}",
                dir.display()
            );
            return;
        }
        // 第 1 趟：盘上的「上次已知集合」→ 内存观测面。
        for server in &tailscale_nodes {
            let path = dir.join(format!("{}.json", tailnet_rule_file_base(&server.id)));
            let Some(cidrs) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| parse_tailnet_rule_file_cidrs(&c))
            else {
                continue;
            };
            let bare: Vec<String> = cidrs
                .iter()
                .filter_map(|c| bare_addr_of_host_cidr(c))
                .map(str::to_owned)
                .collect();
            self.merge_observed(&server.id, bare);
        }
        // 第 2 趟：按观测面重算内容并落盘（内容未变跳过）。
        let observed = self.observed_tailnet_snapshot();
        for server in &tailscale_nodes {
            let cidrs = endpoint_forced_route_cidrs(server, &observed);
            if cidrs.is_empty() {
                // 空 rule-set = 该节点 tailnet 全断（块 0c 走 rule_set 腿就不发 inline 了）。
                // TS 节点恒有两条官方默认段 ⇒ 此分支实际不可达，留着是因为「不写空文件」这条
                // 不变式的成本是一个 if，而它被打破的代价是整段 tailnet 不可路由。
                continue;
            }
            let content = tailnet_rule_file_json(&cidrs);
            let path = dir.join(format!("{}.json", tailnet_rule_file_base(&server.id)));
            if std::fs::read_to_string(&path).ok().as_deref() == Some(content.as_str()) {
                continue;
            }
            if let Err(e) = atomic_write_custom_rule(&path, &content) {
                let _ = std::fs::remove_file(&path);
                log::warn!(
                    "tailnet 规则文件写失败，已删旧副本回退 inline：{}（{e}）",
                    path.display()
                );
            }
        }
    }

    /// **运行中**热更（`apply_ts_status_frame` 每帧调）：把本帧观测到的地址并进已有文件。
    ///
    /// 三条硬约束，每条都对应一种已知的坏死法：
    /// - **空帧不清空**：不含任何地址的帧（核刚起、netmap 还没同步）⇒ 整帧跳过。清空 = 该
    ///   节点 tailnet 全断，比「更新慢一拍」危险得多。
    /// - **绝不删文件**：运行中删一个被在跑的核挂载的文件会致 sing-box reload 报错。本方法
    ///   里没有任何 unlink，连失败腿也没有（见下一条）。
    /// - **文件不存在就不创建**：核跑的是起核那一刻生成的 config —— 那份 config 里要么有这个
    ///   `rule_set` 引用，要么没有。文件缺席说明起核前腿失败了、块 0c 已回落 inline，此刻凭空
    ///   造一个文件，核根本不会去读它；而造出来的内容只有观测段、缺配置期段，万一下次起核前
    ///   腿又没跑到，它会变成一份**缺默认段**的 rule-set。
    ///
    /// 写失败只 warn，**不走去抖重启**（与 `sync_custom_rule_files` 的先例有意分家）：STATUS 帧
    /// 是秒级持续推送，写失败通常是磁盘态问题、下一帧还会失败 ⇒ 挂重启兜底等于重启风暴，
    /// 而它要换回的收益只是「观测地址晚一点进路由」——那恰好等于 A-0a 之前的行为，不是故障。
    ///
    /// 成本：每帧对每个在册 TS endpoint 读一个几百字节的文件做比较。帧是秒级、endpoint 是
    /// 个位数，相对 relay 本身的 gRPC 往返可忽略；换来的是「盘上内容」这个唯一真值不必在
    /// 内存里再镜像一份（镜像就会有第二个可能分叉的状态）。
    pub(super) fn sync_tailnet_rule_files(&self, events: &[TailscaleStatusEvent]) {
        let dir = self.tailnet_rules_dir();
        for event in events {
            let addrs = observed_addresses_of_event(event);
            if addrs.is_empty() {
                continue; // 空帧不清空。
            }
            self.merge_observed(&event.server_id, addrs.clone());
            let hosts = host_cidrs(&addrs);
            if hosts.is_empty() {
                continue; // 一帧里全是非法字面量：同样什么都别做。
            }
            let path = dir.join(format!("{}.json", tailnet_rule_file_base(&event.server_id)));
            // 文件不存在 → 不创建（见方法文档第三条）。
            let Ok(current) = std::fs::read_to_string(&path) else {
                continue;
            };
            // 解析不出 → 运行中不碰它：删是禁令，重写又会丢掉配置期段（这里没有 config）。
            // 修复点在起核前那条腿。
            let Some(current_cidrs) = parse_tailnet_rule_file_cidrs(&current) else {
                continue;
            };
            let mut merged = current_cidrs.clone();
            merged.extend(hosts);
            let merged = dedupe_trim(merged);
            if merged == current_cidrs {
                continue; // 值没变 → 不 rename，别白白惊动 fswatch。
            }
            let content = tailnet_rule_file_json(&merged);
            if let Err(e) = atomic_write_custom_rule(&path, &content) {
                log::warn!(
                    "热更 tailnet 规则文件失败（本帧观测未生效，下一帧重试）：{}（{e}）",
                    path.display()
                );
            }
        }
    }
}
