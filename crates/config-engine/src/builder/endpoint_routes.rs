//! 组网 endpoint 路由纯逻辑（上游 `shared/endpoint-routes.ts` 1:1 移植）。
//!
//! endpointForcedRouteCidrs / meshAllowsInternet / meshAlwaysRoutesSubnets /
//! shouldForceRouteSubnets / collectRuleTargetedServerIds / meshForceRoutedServers /
//! meshForcedRouteCidrs（buildInbounds 依赖的核心子集）。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use crate::builder::custom_rule_files::rule_file_base_with_prefix;
use crate::builder::helpers::host_to_exclude_cidr;
use crate::user_config::app_config::UserConfig;
use crate::user_config::collections::{dedupe, dedupe_trim};
use crate::user_config::dns_constants::is_sentinel_selection;
use crate::user_config::rule::{Rule, RuleAction};
use crate::user_config::server_config::{is_mesh_node, lands_in_endpoints, Protocol, ServerConfig};

/// 全网段（catch-all）。上游 `FULL_TUNNEL_CIDRS`。
pub const FULL_TUNNEL_CIDRS: &[&str] = &["0.0.0.0/0", "::/0"];

/// Tailscale tailnet v4 段（CGNAT）。上游 `TAILNET_CGNAT`。
pub const TAILNET_CGNAT: &str = "100.64.0.0/10";

/// Tailscale v6 tailnet 段（ULA 前缀）。上游 `TAILNET_ULA_V6`。
pub const TAILNET_ULA_V6: &str = "fd7a:115c:a1e0::/48";

/// Tailscale MagicDNS 解析器地址（v4，业界俗称 quad100），主机位 CIDR。
///
/// # ⚠️ 它与上面那两条常量**处置不同**，改之前先读完这一段
///
/// 上面的 [`TAILNET_CGNAT`] / [`TAILNET_ULA_V6`] 会被运行期观测**取代**（见
/// [`ObservedTailnetAddresses`] 的「# 语义」一节），本常量与 [`TAILNET_MAGICDNS_V6`] **不会**。
/// 分界不是「哪条更重要」，是**这两类东西的性质不同**：
///
/// - `100.64.0.0/10` / `fd7a:115c:a1e0::/48` 是**控制面配置**：headscale 的 `prefixes.v4` 可自定义，
///   2026-09-11 实测样本就发成了 `32.0.0.0/24`。观测值是同一个东西的更准确版本 ⇒ 该取代。
/// - `100.100.100.100` / `fd7a:115c:a1e0::53` 是 MagicDNS 的**服务地址**，任何 tailnet 都是这两个，
///   不随控制面变。而且它**不是 peer** ⇒ 永远不会出现在 netmap 里 ⇒ 观测面**永远抓不到它**。
///   于是「用观测取代默认段」这条规则一旦套到它头上，等价于把它删掉。
///
/// # 为什么非发不可（不是"本仓用不着"）
///
/// Polaris 自己的 tailnet 按名解析走 `builder::dns` 注入的 `type:"tailscale"` DNS server，
/// 由 endpoint 内部 netstack 解析、**不经** `route.rules` —— 这条只覆盖 Polaris 自己发起的解析。
/// 漏掉的是**应用或用户直接寻址 quad100**：TUN 模式下那个包进 TUN → 查 `route.rules` → 没有规则
/// 把它送去 TS endpoint → 落 `final`。现场取证脚本里的
/// `dig +time=3 +tries=1 @100.100.100.100 <peer>`（用途是绕过系统解析器直接问 MagicDNS，
/// 通而 `dscacheutil` 不通 ⇒ 判定是 DNS 接管问题）走的正是这条路径 —— 而它恰恰是用来诊断 Polaris 的。
///
/// # 多 tailnet 下的二义性：接受，不是回归
///
/// 两个 tailnet 并存时这个地址只能归先声明者 —— 那是**固有的**（一个地址只能有一个出口），
/// 并集时代也一样。不因此就把单节点用户（绝大多数情形）的覆盖一并砍掉。
///
/// # 无观测那一格为什么不重复发
///
/// bootstrap 腿发的 [`TAILNET_CGNAT`] / [`TAILNET_ULA_V6`] **本就包含**这两个地址，再发一遍只会让
/// 降级腿的产出不再与观测面出现之前逐字节相同。这条包含关系是**被门钉住的事实、不是此处的假设**：
/// `tests/tailnet_observed_address_force_route_gate.rs` 的
/// `magicdns_addresses_are_contained_by_the_default_segments` 用 `cidr_contains` 真算一遍 ——
/// 哪天有人改窄了默认段，红的是那道门，不是用户的 MagicDNS。
pub const TAILNET_MAGICDNS_V4: &str = "100.100.100.100/32";

/// Tailscale MagicDNS 解析器地址（v6），主机位 CIDR。处置理由见 [`TAILNET_MAGICDNS_V4`]。
pub const TAILNET_MAGICDNS_V6: &str = "fd7a:115c:a1e0::53/128";

/// serverId → **运行期观测到**的该节点 tailnet 地址（self + 全部 peer 的 v4/v6，裸地址不带掩码）。
///
/// # 为什么非有不可（2026-09-11 用真控制面实测确认的真缺陷）
///
/// [`TAILNET_CGNAT`] / [`TAILNET_ULA_V6`] 是**上游官方控制面的默认前缀**，不是协议常量：
/// headscale 的 `prefixes.v4` 可自定义，实测某自建 tailnet 把地址发成 `32.0.0.28` / `32.0.0.29`
/// （v6 仍是默认 `fd7a:115c:a1e0::/48`）。此时按硬编码常量发出的 v4 force-route 规则**一条也命中不了**：
/// TUN 的 `auto_route`（`0.0.0.0/1` + `128.0.0.0/1`）照样把 `32.0.0.x` 吞进来 → 进 sing-box 路由 →
/// 没有任何规则把它送去 tailscale endpoint → 落到 `final`。而 `32.0.0.0/8` 是 IANA 分配给 AT&T 的
/// **真实公网段**，落 final 不是"退化成直连"那么轻——它去的是别人的地址空间。
///
/// # 语义：有观测则**取代**默认段（2026-09-11 改；原为并集）
///
/// - 该节点**有**可用观测段 ⇒ 只发观测段（`/32`、`/128`）+ 用户 `routes`，**不发**那两条默认常量；
/// - 该节点**没有**观测段（从未连过 / status 流没来 / 首次运行）⇒ 发默认两段作 bootstrap。
///
/// # 为什么改掉并集：它在多节点下会造出真实错路由
///
/// 两个 TS 节点即便各自 tailnet 完全不相交（实测样本：自建 headscale `32.0.0.0/24` vs 官方
/// `100.64.0.0/10`），那两条默认常量仍然**字面相同** ⇒ `builder::route` 块 0c 的跨节点
/// 「首声明者占有」把后声明者的那两条吸收掉 ⇒ 先声明的 headscale 节点把整个 `100.64.0.0/10`
/// 拿走，官方 tailnet 那个节点自己的地址被送去别人那儿。并集在单节点上零成本，在多节点上是错路由。
///
/// # 取代为什么安全：「有观测」那一侧的清单是完备的
///
/// 观测值来自 netmap，而 netmap 含该 tailnet **全部** peer（在线离线都在 —— `online: bool`
/// 这个字段的存在即证）。落盘侧（`src-tauri` 的 `observed_addresses_of_event`）取的是
/// self ∪ `details.user_groups[].peers[]` ∪ 顶层 `peers[]` 的并集。所以「有观测」时默认段只会
/// 带来跨节点撞段，不带来覆盖。
///
/// 降级方向仍然安全：观测通道坏掉 ⇒ 观测为空 ⇒ 回落默认段 ⇒ 与本改动之前逐字节相同。
///
/// # 取代**不适用**于 MagicDNS 的两个服务地址
///
/// [`TAILNET_MAGICDNS_V4`] / [`TAILNET_MAGICDNS_V6`] 不随控制面变、也永远不会出现在 netmap 里，
/// 故有观测腿把它们**补回来**，不让取代把它们连坐删掉。分界与完整理由见 [`TAILNET_MAGICDNS_V4`]。
///
/// # 如实登记的覆盖面损失（不是遗漏，是这条语义的直接代价）
///
/// 默认段里**不是 peer 地址、也不是上面那两个服务地址**的那部分，有观测时不再被 force-route
/// 覆盖 —— 具体就是 4via6 合成地址（`fd7a:115c:a1e0:b1a::/64` 那一族）。它随对端 site 配置而生，
/// 配置期与 netmap 都不可知，本仓不为它保留整条 `/48`（保留就退回并集、把撞段问题带回来）。
/// 需要它的用户走 `tailscale.routes` 显式声明。**登记为限制，本批不修。**
///
/// # 空值是合法值，不是"没接线"
///
/// 空 map ⟺ 本轮没有任何运行期观测（A-0b 的落盘/观测腿未跑、或该节点还没连上控制面）⇒
/// 行为与观测面存在之前**逐字节相同**。所有构造点都必须显式给出这个空值（结构体字段无默认值，
/// 编译器会逼着每个构造点表态），不许靠"省略即旧行为"蒙混。
pub type ObservedTailnetAddresses = BTreeMap<String, Vec<String>>;

/// tailnet rule-set 文件名/tag 前缀。读取侧（`builder::route` 块 0c）与落盘侧（A-0b）共用，
/// 前缀改了两侧一起改，不会只改一半。
pub const TAILNET_RULE_FILE_PREFIX: &str = "tailnet-";

/// 该节点的 tailnet rule-set 文件名 base（= rule_set tag）。
///
/// 走 [`rule_file_base_with_prefix`] 而不是裸拼 `format!("tailnet-{id}")`：`ServerConfig::id`
/// 来自可导入的配置 JSON，裸拼进路径是目录穿越面；且落盘侧与读取侧必须用同一份派生规则，
/// 否则非法 id 上两边分家（写 hash 名、找裸名）⇒ 该腿静默 100% 不可达。
#[must_use]
pub fn tailnet_rule_file_base(server_id: &str) -> String {
    rule_file_base_with_prefix(TAILNET_RULE_FILE_PREFIX, server_id)
}

/// 观测到的裸地址 → 主机位 CIDR（v4 `/32`、v6 `/128`）。
///
/// 复用 [`host_to_exclude_cidr`]（本 crate 既有的「裸 IP → 主机 CIDR」单一真值），非 IP 字面量
/// 直接丢弃：把一个非法串放进 `route.rules[].ip_cidr` 会让 sing-box `netip.ParsePrefix` 启动 FATAL，
/// 而观测通道的输入是运行期抓来的、不由本 crate 校验过。丢弃而非报错的理由与 route 侧其余
/// 静默剔除同档：一条抓歪的地址不该把整个核拦在门外。
fn observed_tailnet_route_cidrs(addrs: Option<&Vec<String>>) -> Vec<String> {
    addrs
        .map(|list| {
            list.iter()
                .filter_map(|a| host_to_exclude_cidr(a.trim()))
                .collect()
        })
        .unwrap_or_default()
}

/// 该节点本轮**有没有可用的运行期观测段** —— 即它发的是观测段还是默认两段。
///
/// 判据取**解析之后**的结果，不取「`observed` 里有没有这个 key」：观测通道抓回来的是运行期
/// 字符串、不由本 crate 校验，一份全是非法字面量的地址表会被 `observed_tailnet_route_cidrs`
/// （私有，故此处不做 intra-doc 链接：公开文档链私有项会被 CI 的 `rustdoc::private_intra_doc_links`
/// 判红）过滤成空 —— 若按 key 判，这种节点会既不发观测段（没有）也不发默认段（以为有观测），
/// 整段 tailnet 静默不可路由。
///
/// 非 Tailscale 协议恒 `false`：观测面只进 Tailscale 那一支（并进 WG 的 `allowed_ips` 会让对端
/// 没有路由的段被宣称可达 ⇒ 黑洞），故它的段集与观测面无关 —— 即便 map 的 key 撞上了它的 id。
#[must_use]
pub fn has_observed_tailnet_cidrs(
    server: &ServerConfig,
    observed: &ObservedTailnetAddresses,
) -> bool {
    server.protocol == Protocol::Tailscale
        && !observed_tailnet_route_cidrs(observed.get(&server.id)).is_empty()
}

/// force-route **发射顺序**排序键：有观测的节点排前（`0`），无观测的排后（`1`）。
///
/// # 为什么必须有这个顺序
///
/// sing-box 的 `route.rules` 是**按规则顺序 first-match**，不是最长前缀匹配。有观测的节点发
/// `/32`、无观测的节点发 `100.64.0.0/10` —— 两者**字面不同** ⇒ 跨节点去重（`claimed_cidrs`）
/// 一条都拦不下，两条规则都会发出去。此时 `/10` 若排在前面，它把 `/32` 那条的流量整个吃掉，
/// 而产物里那条 `/32` 明明还在、逐条看一切正常。
///
/// # 为什么发射端与只读报告共用这一个 key
///
/// 块 0c（`builder::route`）与 [`endpoint_force_route_report`] 两处各写一次 `!has_…` 就有漏一个
/// `!` 的机会，而漏了**不会红在任何单侧断言上** —— 报告与产物各自自洽，只是顺序相反，
/// 于是报告说的「谁抢走了谁的段」与内核 first-match 的实际结果对不上。
#[must_use]
pub fn force_route_emission_order_key(
    server: &ServerConfig,
    observed: &ObservedTailnetAddresses,
) -> u8 {
    u8::from(!has_observed_tailnet_cidrs(server, observed))
}

/// System 模式内核接口固定名（TS）。原 上游 `polaris-ts`，改名 `polaris-ts`（§D.2 品牌改名）。
pub const TS_SYSTEM_INTERFACE_NAME: &str = "polaris-ts";

/// System 模式内核接口固定名（WG）。原 上游 `polaris-wg`，改名 `polaris-wg`。
pub const WG_SYSTEM_INTERFACE_NAME: &str = "polaris-wg";

fn is_catch_all(c: &str) -> bool {
    FULL_TUNNEL_CIDRS.contains(&c.trim())
}

/// 剥离全网段（catch-all），仅留具体段。上游 `stripCatchAll`。
pub fn strip_catch_all(cidrs: &[String]) -> Vec<String> {
    cidrs.iter().filter(|c| !is_catch_all(c)).cloned().collect()
}

/// CIDR 列表是否含任一全网段。上游 `hasCatchAll`。
pub fn has_catch_all(cidrs: &[String]) -> bool {
    cidrs.iter().any(|c| is_catch_all(c))
}

/// 该组网节点应被「强制路由到自身 tag」的具体 CIDR。上游 `endpointForcedRouteCidrs`。
///
/// 三个来源，都是**配置期已知**的段（这正是 `is_mesh_protocol` / `is_mesh_node` 的判据）：
///  - WireGuard：`allowedIPs` 去 catch-all；WARP 没有子网广播能力，恒为空；
///  - Tailscale：**有观测 ⇒ 观测段（`/32`、`/128`）；无观测 ⇒ tailnet 两族默认段**，再 ∪ `routes`
///    去 catch-all。取代而非并集的完整理由见 [`ObservedTailnetAddresses`] 的「# 语义」一节；
///  - openconnect / openvpn-client：用户在 `meshRoutes` 里显式声明的段（这两个协议的段本由服务端
///    运行期 push、配置期不可知，故只认用户手填的那份）。
///
/// 非组网协议 → `[]`。
///
/// `observed`：运行期观测到的 tailnet 地址（见 [`ObservedTailnetAddresses`]）。空 map、或该节点
/// 一条都解析不出来（见 [`has_observed_tailnet_cidrs`]）= 无观测 ⇒ 回落默认两段，
/// 产出与观测面存在之前逐字节相同。
pub fn endpoint_forced_route_cidrs(
    server: &ServerConfig,
    observed: &ObservedTailnetAddresses,
) -> Vec<String> {
    let raw: Vec<String> = match server.protocol {
        Protocol::Wireguard => {
            if crate::warp::is_warp_server(server) {
                return vec![];
            }
            let allowed = server
                .wireguard_settings
                .as_ref()
                .map(|w| w.allowed_ips.clone())
                .unwrap_or_default();
            strip_catch_all(&allowed)
        }
        Protocol::Tailscale => {
            let routes = server
                .tailscale_settings
                .as_ref()
                .map(|t| t.routes.clone())
                .unwrap_or_default();
            // 有观测 ⇒ 观测段**取代**默认两段；无观测 ⇒ 默认两段作 bootstrap。
            //
            // 取代的理由（并集会让两个不相交 tailnet 的默认段字面撞车、后声明者被吸收）、取代为
            // 什么安全（netmap 含全部 peer ⇒ 有观测时清单完备）、以及登记在案的覆盖面损失
            // （quad100 / 4via6），全部写在 [`ObservedTailnetAddresses`] 的「# 语义」一节 ——
            // 那是这条语义的单一真值，此处不复述，免得两份说明哪天分家。
            //
            // 判空走 `observed_cidrs.is_empty()` 而不是 `observed.get(..).is_some()`：观测通道抓
            // 回来的地址不由本 crate 校验，一份全是非法字面量的表会被过滤成空 —— 按 key 判会让
            // 这种节点既没有观测段、也不发默认段，整段 tailnet 静默不可路由。
            let observed_cidrs = observed_tailnet_route_cidrs(observed.get(&server.id));
            let mut raw = if observed_cidrs.is_empty() {
                // bootstrap 腿：默认两段。**不额外追加 MagicDNS 两条** —— 它们本就落在这两段之内
                // （包含关系由门 `magicdns_addresses_are_contained_by_the_default_segments` 钉住，
                // 不是此处的假设），重复发只会让降级腿的产出不再与观测面出现之前逐字节相同。
                vec![TAILNET_CGNAT.to_string(), TAILNET_ULA_V6.to_string()]
            } else {
                // 有观测腿：观测段取代默认两段，但 MagicDNS 两条必须**补回来**。
                // 它们不是 peer、永远不会进 netmap ⇒ 观测面抓不到 ⇒ 被取代掉就等于删掉，
                // 而删掉的后果是应用/用户直接寻址 quad100 的包落 `final`。
                // 完整理由（含为什么它与那两条会漂移的前缀不是一类）见 [`TAILNET_MAGICDNS_V4`]。
                //
                // 发射顺序上它自动落在**有观测那一批**（排序键只看节点有无观测、不看段的形态），
                // 于是它排在无观测节点的 `100.64.0.0/10` 之前 —— 正是精确地址该在的位置。
                let mut v = observed_cidrs;
                v.push(TAILNET_MAGICDNS_V4.to_string());
                v.push(TAILNET_MAGICDNS_V6.to_string());
                v
            };
            raw.extend(strip_catch_all(&routes));
            raw
        }
        // 用户手填的内网段。去 catch-all 与另两支同理：0/0 属「全隧道」意图，由各自的出网开关
        // 表达（OpenVPN 是 `redirect_gateway`），混进 force-route 会绕过那个开关。
        Protocol::Openconnect | Protocol::OpenvpnClient => strip_catch_all(&server.mesh_routes),
        _ => return vec![],
    };
    dedupe_trim(raw)
}

/// 组网节点是否允许作外网出口（缺省 true）。上游 `meshAllowsInternet`。
/// WG：allowInternet !== false；WARP 恒为云出口；TS：!!exitNode（allowInternet 由 exit_node 派生）。
pub fn mesh_allows_internet(server: &ServerConfig) -> bool {
    match server.protocol {
        Protocol::Wireguard => {
            crate::warp::is_warp_server(server)
                || server
                    .wireguard_settings
                    .as_ref()
                    .and_then(|w| w.allow_internet)
                    .unwrap_or(true)
        }
        Protocol::Tailscale => server
            .tailscale_settings
            .as_ref()
            .and_then(|t| t.exit_node.as_deref())
            .map(|e| !e.trim().is_empty())
            .unwrap_or(false),
        // OpenVPN 的全隧道开关。**缺省判 true**（同 WG 的 `allow_internet` 那支）：判 false 的后果是
        // 用户选了该节点作出口、流量却被兜底回 direct —— 静默走明文，比多一次黑洞更坏。故只在用户
        // **显式**关掉时才认为它不承载全隧道，而那恰是「只走公司内网段」的表达。
        // OpenConnect 无对应开关（本就是全隧道），落 `_ => true`。
        Protocol::OpenvpnClient => server
            .openvpn_client_settings
            .as_ref()
            .and_then(|o| o.redirect_gateway)
            .unwrap_or(true),
        _ => true,
    }
}

/// 组网节点是否「始终路由其内网段」（缺省 true）。上游 `meshAlwaysRoutesSubnets`。
pub fn mesh_always_routes_subnets(server: &ServerConfig) -> bool {
    match server.protocol {
        Protocol::Wireguard => server
            .wireguard_settings
            .as_ref()
            .and_then(|w| w.always_route_subnets)
            .unwrap_or(true),
        Protocol::Tailscale => server
            .tailscale_settings
            .as_ref()
            .and_then(|t| t.always_route_subnets)
            .unwrap_or(true),
        _ => true,
    }
}

/// 组网节点是否启用 system 内核接口（reverseMesh）。上游 `meshUsesSystemInterface`。
/// WG：reverseMesh（WARP 否决）；TS：reverseMesh。
pub fn mesh_uses_system_interface(server: &ServerConfig) -> bool {
    match server.protocol {
        Protocol::Wireguard => {
            // WARP 恒否决：它是 anycast 出口、不是子网路由器，不可被反向访问，system 对它无意义；
            // 而 `system:true` 会与主 TUN / 另一 System 接口抢内核 utun →
            // `post-start endpoint/wireguard[Cloudflare WARP]: Connect: resource busy` **FATAL**。
            // 判据与前端 `isWarpServer` 同源（见 crate::warp）——导入配置 / 手改 config.json /
            // 上游 迁移这三条腿不经渲染端，前端那道否决在这里挡不住。
            if crate::warp::is_warp_server(server) {
                return false;
            }
            server
                .wireguard_settings
                .as_ref()
                .and_then(|w| w.reverse_mesh)
                .unwrap_or(false)
        }
        Protocol::Tailscale => server
            .tailscale_settings
            .as_ref()
            .and_then(|t| t.reverse_mesh)
            .unwrap_or(false),
        _ => false,
    }
}

/// 组网节点是否承载全隧道默认出口（= 允许外网）。上游 `meshNodeCarriesFullTunnel`。
pub fn mesh_node_carries_full_tunnel(server: &ServerConfig) -> bool {
    mesh_allows_internet(server)
}

/// WireGuard peer.allowed_ips（Layer A cryptokey）。上游 `wireguardPeerAllowedIps`。
/// allowInternet=on → specific ∪ {0/0,::/0}；off → specific（空则 None=FATAL）。
///
/// **这里的空观测面是判据，不是降级**：`allowed_ips` 是 WireGuard 的 cryptokey routing table
/// （决定哪些目的地能被这条 WG 隧道加密送出），而 [`ObservedTailnetAddresses`] 装的是 **Tailscale**
/// 节点的 tailnet 地址。把后者塞进前者等于宣称"这条 WG 隧道也能送 tailnet 流量"——WG peer 的
/// 对端根本没有那些地址的路由，结果是黑洞。故本函数恒传空 —— 而"观测面不会漏进非 Tailscale
/// 协议"这条不变量由 `tests/tailnet_observed_address_force_route_gate.rs` 的
/// `observed_addresses_never_leak_into_non_tailscale_protocols` 钉住（那里让一个 WG 节点的 id
/// 与观测 map 的 key 故意撞车，段集仍须只剩它自己的 `allowed_ips`）。
pub fn wireguard_peer_allowed_ips(server: &ServerConfig) -> Option<Vec<String>> {
    let specific = endpoint_forced_route_cidrs(server, &ObservedTailnetAddresses::new());
    if mesh_node_carries_full_tunnel(server) {
        let mut all = specific;
        all.extend(FULL_TUNNEL_CIDRS.iter().map(|s| s.to_string()));
        Some(crate::user_config::collections::dedupe(all))
    } else if specific.is_empty() {
        None
    } else {
        Some(specific)
    }
}

/// 组网节点是否「关外网且无可路由网段」→ 不可发射。上游 `isMeshNodeUnroutable`。
pub fn is_mesh_node_unroutable(server: &ServerConfig) -> bool {
    if server.protocol == Protocol::Wireguard {
        wireguard_peer_allowed_ips(server).is_none()
    } else {
        false
    }
}

/// 平台是否支持组网 System 内核接口（Windows 禁）。上游 `meshSystemSupportedOnPlatform`。
pub fn mesh_system_supported_on_platform(platform: &str) -> bool {
    !platform.eq_ignore_ascii_case("win32")
}

/// 该组网节点的 force-route 段本轮是否应发射。上游 `shouldForceRouteSubnets`。
/// alwaysRouteSubnets ON → 恒发；OFF → 仅 engaged（选中/被规则指向）时发。
pub fn should_force_route_subnets(
    server: &ServerConfig,
    selected_server_id: Option<&str>,
    rule_targeted_server_ids: &BTreeSet<String>,
) -> bool {
    if mesh_always_routes_subnets(server) {
        return true;
    }
    if Some(server.id.as_str()) == selected_server_id {
        return true;
    }
    rule_targeted_server_ids.contains(&server.id)
}

/// 收集「显式指向某节点」的规则目标 id（enabled && proxy && targetServerId）。
/// 上游 `collectRuleTargetedServerIds`。接受 Rule + AppRule 混合。
pub fn collect_rule_targeted_server_ids(rules: &[Rule]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for r in rules {
        if r.enabled && r.route_action() == Some(RuleAction::Proxy) {
            if let Some(tid) = r.route_target_server_id() {
                ids.insert(tid.to_string());
            }
        }
    }
    ids
}

/// 本轮「实际会发射 force-route」的组网节点。上游 `meshForceRoutedServers`。
pub fn mesh_force_routed_servers(
    servers: &[ServerConfig],
    selected_server_id: Option<&str>,
    rule_targeted_server_ids: &BTreeSet<String>,
) -> Vec<ServerConfig> {
    servers
        .iter()
        .filter(|s| is_mesh_node(s))
        .filter(|s| should_force_route_subnets(s, selected_server_id, rule_targeted_server_ids))
        .cloned()
        .collect()
}

/// 全部节点的 mesh force-route 段并集（去重）。上游 `meshForcedRouteCidrs`。
///
/// `observed` 一路透传到 [`endpoint_forced_route_cidrs`]：本函数是 TUN 排除面
/// （`builder::inbounds` 的 `engaged_mesh`）与外来隧道冲突判定（`builder::tunnel_conflict`
/// 的 `ConflictInput::mesh_cidrs`）共同的段来源，观测地址若只进规则发射那一条腿，
/// 这两个面仍看不见自建 tailnet 的真实前缀。
pub fn mesh_forced_route_cidrs(
    servers: &[ServerConfig],
    observed: &ObservedTailnetAddresses,
) -> Vec<String> {
    let all: Vec<String> = mesh_force_routed_servers(servers, None, &BTreeSet::new())
        .iter()
        .flat_map(|s| endpoint_forced_route_cidrs(s, observed))
        .collect();
    dedupe(all)
}

/// 「显式指向某节点」的规则目标 id（自定义规则 + 应用分流混合）。
///
/// 从 `builder::route` 上移：块 0c 的 engaged 判定与只读报告
/// （[`endpoint_force_route_report`]）必须用**同一次**汇集 —— 两处各写一遍，
/// 哪天 app 规则那半支的判据改了只改一处，报告与发射端就会对同一份配置给出不同的 engaged 集。
pub fn collect_targeted_mixed(
    custom_rules: &[Rule],
    app_rules: &[crate::user_config::rule::AppRule],
) -> BTreeSet<String> {
    let mut ids = collect_rule_targeted_server_ids(custom_rules);
    for r in app_rules {
        if r.enabled && r.action == RuleAction::Proxy {
            if let Some(tid) = &r.target_server_id {
                ids.insert(tid.clone());
            }
        }
    }
    ids
}

/// 本轮 engaged 判定用的规则目标 id —— 从 `UserConfig` 一路算到底（含 proxyMode /
/// appRoutingEnabled 两道 mode-gate），供**没有 route builder 上下文**的调用方
/// （只读报告）取到与块 0c 逐字节同口径的那一份。
#[must_use]
pub fn engaged_rule_targeted_server_ids(config: &UserConfig) -> BTreeSet<String> {
    let proxy_mode = match config.proxy_mode {
        crate::user_config::ProxyMode::Smart => "smart",
        crate::user_config::ProxyMode::Global => "global",
        crate::user_config::ProxyMode::Direct => "direct",
    };
    let ordered: Vec<Rule> = config
        .ordered_traffic_rules()
        .into_iter()
        .cloned()
        .collect();
    let custom_eff = crate::builder::helpers::effective_custom_rules(proxy_mode, &ordered);
    let app_eff = crate::builder::helpers::effective_app_rules(
        config.app_routing_enabled == Some(true),
        proxy_mode,
        &config.app_rules,
    );
    collect_targeted_mixed(&custom_eff, &app_eff)
}

/// Tailscale `preferred_by` 试点开关。原为 `builder::route` 的私有常量，上移为单一真值。
///
/// sing-box 源码确证 TS 的 `routePrefixes` 运行时动态 + 就绪窗口 nil → 组网段不归位，
/// 故 TS 必走 `ip_cidr` 静态腿。发射端（块 0c）与只读报告都按它选腿；两处各持一份，
/// 就会出现「报告说走 A 腿、内核吃的是 B 腿」。
pub const TS_PREFERRED_BY_TRIAL: bool = false;

/// tailnet rule-set 文件所在的目录名（`<configDir>/tailnet-rules`）。
///
/// 落盘侧（`src-tauri` 的 `ProxyRuntime::tailnet_rules_dir`）今天仍各拼一次字面量 —— 已登记为
/// 待收口项：两边拼的若不是同一个目录，报告会一律读成「文件不存在 ⇒ inline 腿」，
/// 而内核吃的是 rule-set 腿，报告从此系统性说反。
pub const TAILNET_RULES_DIR_NAME: &str = "tailnet-rules";

/// 块 0c 为一个组网节点选的**发射腿**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForceRouteLeg {
    /// `preferred_by`：非全隧道 WG（+ TS 试点开时）。段由内核按 endpoint 自身路由表归位，
    /// 不产出 `ip_cidr` 字面量 ⇒ **不参与**跨节点去重。
    PreferredBy,
    /// 外化 tailnet rule-set（TS 且文件已落盘）：段值住在文件里、改了热重载 ⇒ 同样不产 `ip_cidr`。
    ExternalRuleSet,
    /// inline `ip_cidr` —— 唯一参与跨节点「首声明者占有」的腿。
    Inline,
}

/// 选腿。`tailnet_rule_file_exists` 由调用方做真实 `existsSync`（发射端与报告端必须问同一个目录）。
///
/// 顺序逐字节镜像块 0c：先 `preferred_by`，再 tailnet 文件，最后 inline。
#[must_use]
pub fn force_route_leg(server: &ServerConfig, tailnet_rule_file_exists: bool) -> ForceRouteLeg {
    let preferred_by = !mesh_node_carries_full_tunnel(server)
        && (server.protocol == Protocol::Wireguard
            || (server.protocol == Protocol::Tailscale && TS_PREFERRED_BY_TRIAL));
    if preferred_by {
        return ForceRouteLeg::PreferredBy;
    }
    if server.protocol == Protocol::Tailscale && tailnet_rule_file_exists {
        return ForceRouteLeg::ExternalRuleSet;
    }
    ForceRouteLeg::Inline
}

/// 一条被**更早声明者**抢走的段：`cidr` 不会经本节点路由，实际生效的是 `by_server_id` 那个节点。
///
/// 只给 `cidr` 不给抢占者，用户仍无从下手 —— 上游 `ShadowedCidr` 当初补 `byId` 就是这个理由。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbsorbedCidr {
    pub cidr: String,
    pub by_server_id: String,
}

/// 单节点的 force-route 覆盖三态。
///
/// **语义与 `tests/endpoint_force_route_silent_absorption_gate.rs` 的 `Coverage` 同源**：
/// 那道门（A-5 门③）先定义了「A=有段要发却零覆盖（危险）／B=本来就没段要发（正常）」这条分界，
/// 这里只是把它搬进生产可读的形态，不是第二套判据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForceRouteCoverage {
    /// engaged 且自身有段要发，本轮确实会发出属于它的 force-route 规则。
    Covered,
    /// 🔴 engaged 且自身有段要发，却一条都发不出 —— 全被更早的节点抢先声明走了。
    /// 节点是活的、用户以为它在工作，流量一条都不到它那儿。
    AbsorbedEmpty,
    /// 自身本来就没有段要发（如 WARP：全隧道 anycast 出口，天生没有「具体段」这回事）。不是缺陷。
    NothingToRoute,
}

/// 单节点结算结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerForceRoute {
    pub server_id: String,
    pub leg: ForceRouteLeg,
    /// 本轮该节点**有没有可用的运行期观测段**（取 [`has_observed_tailnet_cidrs`]，与发射端块 0c
    /// 的排序键同一个谓词，不是第二份判断）。
    ///
    /// 两个用途，缺一不可：
    ///  - **语义位**：真 ⇒ 本节点发的是观测段（`/32`、`/128`）；假 ⇒ 发的是 [`TAILNET_CGNAT`] /
    ///    [`TAILNET_ULA_V6`] 两条默认常量。两个无观测节点的段**必然完全重合**，那不是
    ///    「同一个 tailnet」的证据，只是两份一样的猜测；少了这一位，消费方会把「还没连上」
    ///    读成「账号撞车」。
    ///  - **顺序位**：真的那些节点在 [`EndpointForceRouteReport::servers`] 与产物 `route.rules`
    ///    里都排在假的那些之前（理由见 [`force_route_emission_order_key`]）。
    pub has_observation: bool,
    /// 本节点**实际发射**的具体段（只有 [`ForceRouteLeg::Inline`] 腿非空）。
    pub emitted: Vec<String>,
    /// 被更早声明者吸收掉的段（含抢占者 id）。
    pub absorbed: Vec<AbsorbedCidr>,
    pub coverage: ForceRouteCoverage,
}

/// 本轮 endpoint force-route 的完整结算：谁和谁撞了、撞在哪一段、谁输了、谁因此零覆盖。
///
/// 块 0c 此前只有一个 `force_route_conflicts` 计数 + 一条 warn —— 那条 warn 说得出「有 N 段重复」，
/// 说不出「哪个节点因此一条流量都收不到」。而后者才是用户能看见的那个故障。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointForceRouteReport {
    /// 逐节点结算，顺序 = **发射顺序** = 「谁先占」的顺序：有观测的节点整体在前，组内保持
    /// `config.servers` 的声明顺序（见 [`force_route_emission_order_key`]）。与产物
    /// `route.rules` 里那几条 force-route 规则的相对顺序逐项一致 —— 两者分叉，报告说的
    /// 「谁抢走了谁的段」就与内核 first-match 的实际结果对不上。
    pub servers: Vec<ServerForceRoute>,
    /// 覆盖面被吸收干净的节点 id（`coverage == AbsorbedEmpty`）。**空数组是有意义的结果**：
    /// 「本轮没有任何节点被吃干净」，不是「没算出来」。
    pub zero_coverage_server_ids: Vec<String>,
    /// 被吸收的段总数 —— 与块 0c `force_route_conflicts` 同口径（逐条被丢弃的 cidr +1）。
    pub absorbed_count: u32,
}

/// 跨节点「首声明者占有」的**一次**结算。发射端（块 0c）与只读报告共用这一份实现。
///
/// `claimants` 必须已按**发射顺序**排好（本函数不排序），且只含本轮真会发射 force-route 的节点。
/// 发射顺序由 [`force_route_emission_order_key`] 定（有观测在前、组内保持声明顺序），两个调用方
/// —— 块 0c 与 [`endpoint_force_route_report`] —— 用的是同一个 key；
/// 每项带它的 [`ForceRouteLeg`] —— 非 `Inline` 腿既不消耗也不贡献 `claimed`
/// （它本就不产出 inline cidr ⇒ 不会拿别人的段去撞，也不会有段被别人吸收）。
#[must_use]
pub fn settle_force_route_claims(
    claimants: &[(&ServerConfig, ForceRouteLeg)],
    observed: &ObservedTailnetAddresses,
) -> EndpointForceRouteReport {
    let mut claimed_by: BTreeMap<String, String> = BTreeMap::new();
    let mut servers: Vec<ServerForceRoute> = Vec::with_capacity(claimants.len());
    let mut zero_coverage_server_ids: Vec<String> = Vec::new();
    let mut absorbed_count: u32 = 0;

    for (s, leg) in claimants {
        let desired = endpoint_forced_route_cidrs(s, observed);
        let mut emitted: Vec<String> = Vec::new();
        let mut absorbed: Vec<AbsorbedCidr> = Vec::new();
        if *leg == ForceRouteLeg::Inline {
            for c in &desired {
                if let Some(owner) = claimed_by.get(c) {
                    absorbed.push(AbsorbedCidr {
                        cidr: c.clone(),
                        by_server_id: owner.clone(),
                    });
                    absorbed_count += 1;
                } else {
                    claimed_by.insert(c.clone(), s.id.clone());
                    emitted.push(c.clone());
                }
            }
        }
        let coverage = if desired.is_empty() {
            ForceRouteCoverage::NothingToRoute
        } else if *leg != ForceRouteLeg::Inline || !emitted.is_empty() {
            ForceRouteCoverage::Covered
        } else {
            ForceRouteCoverage::AbsorbedEmpty
        };
        if coverage == ForceRouteCoverage::AbsorbedEmpty {
            zero_coverage_server_ids.push(s.id.clone());
        }
        servers.push(ServerForceRoute {
            server_id: s.id.clone(),
            leg: *leg,
            has_observation: has_observed_tailnet_cidrs(s, observed),
            emitted,
            absorbed,
            coverage,
        });
    }

    EndpointForceRouteReport {
        servers,
        zero_coverage_server_ids,
        absorbed_count,
    }
}

/// 只读报告入口：按块 0c 的**同一套腿选择 + 同一次结算**回答「谁和谁撞了、谁零覆盖」。
///
/// `tailnet_rules_dir` 必须是落盘侧写 tailnet rule-set 的那个目录（见 [`TAILNET_RULES_DIR_NAME`]）——
/// 传错目录不会报错，只会让每个 TS 节点都被读成 inline 腿，报告与内核吃的那一份从此系统性分叉。
///
/// # 与块 0c 的一处**已知差异**（如实登记，方向是报多不报少）
///
/// 块 0c 的 claimant 面还要求「该节点的 tag 真在本轮 `pending_endpoints` 里」——
/// 那取决于 `build_outbounds` 的运行期能力判定（naive 缺 cronet / WG 缺密钥 / TS control_url 非法），
/// 本函数拿不到（那两个能力轴在 `src-tauri` 侧是 `pub(super)`）。故此处按 [`is_mesh_node`] +
/// [`should_force_route_subnets`] 取claimant：一个**发射失败**的节点会多出现在报告里，
/// 它的 `absorbed` 会让后面的节点少记一条。方向与 `referenced_server_ids` 一致
/// （过度纳入只多一条提示，绝不漏报真冲突）。
#[must_use]
pub fn endpoint_force_route_report(
    config: &UserConfig,
    observed: &ObservedTailnetAddresses,
    tailnet_rules_dir: &str,
) -> EndpointForceRouteReport {
    let rule_targeted = engaged_rule_targeted_server_ids(config);
    let mut claimants: Vec<(&ServerConfig, ForceRouteLeg)> = config
        .servers
        .iter()
        .filter(|s| is_mesh_node(s))
        .filter(|s| {
            should_force_route_subnets(s, config.selected_server_id.as_deref(), &rule_targeted)
        })
        .map(|s| {
            let file_exists = s.protocol == Protocol::Tailscale
                && crate::builder::custom_rule_files::ext_rule_file_exists(&format!(
                    "{tailnet_rules_dir}/{}.json",
                    tailnet_rule_file_base(&s.id)
                ));
            (s, force_route_leg(s, file_exists))
        })
        .collect();
    // 排成发射顺序（有观测在前）。`sort_by_key` 是**稳定**排序 ⇒ 组内保持 `config.servers` 的
    // 声明顺序。与块 0c 用的是同一个 key，不是第二份顺序判断 —— 两处分家不会红在任何单侧断言上，
    // 只会让报告说的「谁抢走了谁的段」与内核 first-match 的实际结果反过来。
    claimants.sort_by_key(|(s, _)| force_route_emission_order_key(s, observed));
    settle_force_route_claims(&claimants, observed)
}

/// custom-endpoint 的 raw JSON（`customSettings.outbound`）是否含「独立承载流量」语义键。
///
/// 深度扫（递归任意嵌套，含 peers[].allowed_ips），命中任一即真。
/// 上游 `customEndpointCarriesTraffic`（endpoint-routes.ts L163-172）。
fn custom_endpoint_carries_traffic(raw: &serde_json::Value) -> bool {
    match raw {
        serde_json::Value::Array(arr) => arr.iter().any(custom_endpoint_carries_traffic),
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if CARRY_TRAFFIC_KEYS.contains(&k.as_str()) {
                    return true;
                }
                if custom_endpoint_carries_traffic(v) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// custom-endpoint 承载流量的语义键集合。上游 `CARRY_TRAFFIC_KEYS`。
///
/// ⚠️ **这个集合的覆盖面由「内核支持哪些端点类型」决定，不由「写它时手边有哪些类型」决定。**
/// 2026-08-11 补 OpenVPN 三键前，全表都是 WireGuard / Tailscale 的词汇 —— 而随包核
/// （1.14.0-beta.12，tags 含 `with_openvpn` / `with_openconnect`）的 `$defs/Endpoint` 有 5 支：
/// wireguard / tailscale / openconnect / openvpn-client / openvpn-server。逐支对过：
///   · openconnect —— 路由类键只有 `system`，**原表已覆盖**；
///   · openvpn-client —— 另有 `redirect_gateway`（OpenVPN 的全隧道开关，等价 WG 的
///     `allowed_ips: 0.0.0.0/0`）、`redirect_private`、`route_no_pull`，**原表一个都不认**。
///
/// 补进来**不要求**先证明「`redirect_gateway:true` 且 `system:false` 时内核是否真独立导流」：
/// 本判据的既定安全方向是「过度纳入只多一次重启、绝不错跳」，而漏纳入的后果写在调用点
/// （`can_skip_restart_for_added_unreferenced` 那段）—— 走 defer 腿不重启、核继续用旧参数出网
/// 且无任何提示。不确定时按方向站队，不是按证据强弱站队。
const CARRY_TRAFFIC_KEYS: &[&str] = &[
    "system",
    "system_interface",
    "allowed_ips",
    "routes",
    "route_address",
    "route_exclude_address",
    "accept_routes",
    "advertise_routes",
    "exit_node",
    // ── OpenVPN（2026-08-11）──
    "redirect_gateway",
    "redirect_private",
    "route_no_pull",
];

/// 选中该节点时，成功生成的配置是否**必定**把它发射为 outbound/endpoint。
///
/// **Sound under-approximation**：返回 `true` ⇒ 一定发射；返回 `false` ⇒ **不确定**（可能发射、
/// 也可能被跳过），调用方须按保守方向处理。逐条对应 `builder/outbounds.rs` 发射循环（:127-234）：
/// - naive 缺 libcronet 时，`generate.rs` 的 selected-server 前置校验会在构建 selector **之前**终止
///   整次生成；有 libcronet 时必定发射。因此它不存在“成功生成但 selector 静默落到其它节点”的腿；
/// - WireGuard（:147-161）：无可路由段、或缺 privateKey/peerPublicKey/localAddress → 不发射。
///   这些判据全是 `ServerConfig` 的纯函数，故**直接调 `build_wireguard_endpoint` 取真判据**，
///   不在此复刻条件清单——复刻会随构建腿改动静默漂移，而漂移方向恰好是「误判必定发射」＝错跳。
///   tag / resolver / platform 三个参数不影响 Ok/Err，传占位值；
/// - custom（两条腿）：`customSettings.outbound` 不是**带 string `type` 的对象**就会被发射循环剔除
///   并记进 `invalid_nodes`（`INVALID_REASON_CUSTOM_MALFORMED`）→ 故此处同判
///   [`custom_outbound_type`](crate::user_config::protocol_settings::custom_outbound_type)。
///   注意这条从前写的是「endpoint 腿反序列化可能失败 → 恒不确定」——那个失败腿已随 raw 透传消失，
///   取而代之的是形状判据；而**非** endpoint 的 custom 从前恒记「必定发射」，那在补上形状 gate
///   之后是**不成立**的（形状坏的 custom outbound 现在会被剔），故必须一并收紧，否则这个
///   sound under-approximation 就朝「误判必定发射」＝错跳重启的方向破了；
/// - Tailscale 的非法 `control_url` 会在发射循环中被剔除；其余 Tailscale 与普通代理 outbound
///   无失败腿 → 必定发射。
///
/// 外部注入的 `gate_invalid_nodes` 不建模：两个生成入口都传空集。发射循环自身会写入的
/// Tailscale / custom 静态剔除门已在上面逐项复用；detour 剪枝若命中选中节点则返回 `Err`，不会形成
/// “成功生成但 selector 静默回退”的第三条腿。
fn selected_server_precludes_selector_fallback(s: &ServerConfig) -> bool {
    match s.protocol {
        Protocol::Naive => true,
        Protocol::Wireguard => {
            crate::builder::endpoints::build_wireguard_endpoint(s, "", None, "", None).is_ok()
        }
        Protocol::Tailscale => s
            .tailscale_settings
            .as_ref()
            .and_then(|settings| settings.control_url.as_deref())
            .and_then(crate::user_config::control_url::tailscale_control_url_reject)
            .is_none(),
        Protocol::Custom => s.custom_settings.as_ref().is_some_and(|c| {
            crate::user_config::protocol_settings::custom_outbound_type(&c.outbound).is_some()
        }),
        _ => true,
    }
}

/// `proxy-selector` 的 default 是否**可能**落到「非选中节点」的兜底节点上。
///
/// `build_outbounds`（`outbounds.rs:262-271`）在「选中节点的 tag 不在本轮已发射 tag 集合里」时，
/// 把 default 落到 `node_tags.first()`——那个节点随即承载**全部**代理流量，但它的 id 无法从
/// `UserConfig` 静态算出（取决于生成期跳过了谁，而那依赖运行期能力）。本谓词只回答
/// 「是否处于该状态」，把「是谁」交给调用方按保守方向兜。
///
/// 返回 `false` 仅两条：① 直连哨兵（default 恒 = `direct` 出站，无节点承载）；
/// ② 选中节点存在**且** `selected_server_precludes_selector_fallback`（此时成功生成的配置里
/// default 恒 = 选中节点 tag）。
/// 其余一律 `true`——含「未选节点」（`selected_tag` 是字面量 `"proxy"`，匹配不到任何节点）
/// 与「悬空选中」（id→tag 解析不到）。
///
/// **同型第二处一并覆盖**：`prune_detour_dead_references` 经 `pruned_selector_default`
/// 重算 default（`outbounds.rs:568-578`）只在「被剔 tag == 当前 default」时触发；而 default ==
/// 选中节点 tag 时该路径返回 Err（`outbounds.rs:558`）而非静默重算 ⇒ 静默重算必然发生在本谓词
/// 已为 `true` 的状态下，无需第二道判据。
pub fn selector_default_may_fall_back(config: &UserConfig) -> bool {
    let Some(sid) = config.selected_server_id.as_deref() else {
        return true; // 未选节点 → selected_tag 恒为字面量 "proxy" → 必落兜底
    };
    if is_sentinel_selection(Some(sid)) {
        return false; // direct / block 哨兵 → default 恒 = 内置出站（direct / block），无节点承载
    }
    match config.servers.iter().find(|s| s.id == sid) {
        None => true, // 悬空选中 → id→tag 解析不到 → 必落兜底
        Some(s) => !selected_server_precludes_selector_fallback(s),
    }
}

/// 「被引用节点」id 集——其定义变化会影响运行核实际行为、故必须随之重启。
///
/// = {选中节点} ∪ {所有启用规则(custom/app)目标}，按 detour（前置代理链）传递闭包展开
/// ＋ 保守纳入全部 endpoint 协议节点（WireGuard/Tailscale 可能 force-route 子网/mesh）
/// ＋ [`selector_default_may_fall_back`] 成立时纳入**全部**节点（兜底 default 承载全部流量、
/// 但它是谁静态算不出）。
/// 安全方向：过度纳入只多一次重启、绝不错跳。上游 `referencedServerIds`。
pub fn referenced_server_ids(config: &UserConfig) -> BTreeSet<String> {
    let by_id: std::collections::BTreeMap<&str, &ServerConfig> =
        config.servers.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut result: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = Vec::new();

    let seed = |id: Option<&str>, stack: &mut Vec<String>| {
        if let Some(id) = id {
            // direct / block 哨兵不是节点 id：进了引用集就会当成「悬空选中」被 detour 闭包展开，
            // 白白把全部节点纳入 → 每次配置改动都误判需重启。
            if !is_sentinel_selection(Some(id)) {
                stack.push(id.to_string());
            }
        }
    };
    seed(config.selected_server_id.as_deref(), &mut stack);
    let smart = config.proxy_mode == crate::user_config::proxy_mode::ProxyMode::Smart;
    if smart {
        for r in config.effective_traffic_rules() {
            if r.enabled && r.route_action() == Some(RuleAction::Proxy) {
                seed(r.route_target_server_id(), &mut stack);
            }
        }
    }
    if smart && config.app_routing_enabled == Some(true) {
        for a in &config.app_rules {
            if a.enabled && a.action == RuleAction::Proxy {
                seed(a.target_server_id.as_deref(), &mut stack);
            }
        }
    }
    for s in &config.servers {
        // 判据是 `lands_in_endpoints` 而非组网资格：本播种问的是「谁独立承载流量」，而 endpoint 腿的
        // 节点无论有没有声明网段都自成一条出网路径。漏纳入的后果写在 `CARRY_TRAFFIC_KEYS` 的调用点 ——
        // 走 defer 腿不重启、核继续用旧参数出网且无任何提示。
        if lands_in_endpoints(s.protocol) {
            stack.push(s.id.clone());
        } else if let Some(cs) = &s.custom_settings {
            if cs.is_endpoint.unwrap_or(false) && custom_endpoint_carries_traffic(&cs.outbound) {
                stack.push(s.id.clone());
            }
        }
    }
    // selector default 兜底节点（`outbounds.rs:262-271`）：它承载**全部**代理流量，却不在上面任何
    // 一条播种里——它是「生成期第一个成功发射的节点」，id 取决于生成期跳过了谁（naive 缺 cronet /
    // WG 构建失败 / custom-endpoint 解析失败），`UserConfig` 静态算不出。
    // 漏纳入的后果不是「少一次重启」而是**静默失效**：编辑它会被 `can_skip_restart_for_added_unreferenced`
    // 第③步判「未引用 → 放行」→ 走 defer 腿不重启 → 核继续用旧参数出网且无任何提示
    // （热切腿有 `is_server_dirty` 闸门，defer 腿没有）。
    // 故按本函数的既定安全方向（过度纳入只多一次重启、绝不错跳）：兜底**可能**触发时全员纳入。
    // 该状态本身是降级态（用户选中的出口没进核 / 还没选出口），常态（选中节点必定被发射）不受影响。
    if selector_default_may_fall_back(config) {
        for s in &config.servers {
            stack.push(s.id.clone());
        }
    }
    while let Some(id) = stack.pop() {
        if result.contains(&id) {
            continue; // 成环/重复保护
        }
        result.insert(id.clone());
        if let Some(s) = by_id.get(id.as_str()) {
            if let Some(detour) = &s.detour {
                if by_id.contains_key(detour.as_str()) {
                    stack.push(detour.clone());
                }
            }
        }
    }
    result
}

fn physical_root_ids(
    config: &UserConfig,
    seeds: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    let by_id: std::collections::BTreeMap<&str, &ServerConfig> =
        config.servers.iter().map(|s| (s.id.as_str(), s)).collect();

    let resolve_root = |id: &str| -> Option<String> {
        let mut current = *by_id.get(id)?;
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.id.as_str()) {
                return None;
            }
            let Some(detour) = current.detour.as_deref() else {
                return Some(current.id.clone());
            };
            current = *by_id.get(detour)?;
        }
    };

    let mut roots = BTreeSet::new();
    let mut uncertain = false;
    for id in seeds {
        match resolve_root(&id) {
            Some(root) => {
                roots.insert(root);
            }
            None => uncertain = true,
        }
    }
    if uncertain {
        roots.extend(
            config
                .servers
                .iter()
                .filter_map(|server| resolve_root(&server.id)),
        );
    }
    roots
}

/// 当前运行配置真正可能发起物理公网拨号的根节点 id。
///
/// 播种严格复用 [`referenced_server_ids`]（选中出口、有效流量/应用规则目标、承流 endpoint 与 selector
/// fallback），再沿 detour 收敛到唯一物理根。运行时显式网卡 fail-closed 只检查本集合：闲置节点的本机
/// 网卡策略不得阻断当前出口启动；detour child 自己的 `bindInterface` 也不代表一条物理 socket。
///
/// 活跃链出现环/死引用时无法证明物理根是谁，按保守方向纳入所有可解析物理根。正常配置不付这笔成本；
/// 真正的死引用仍由生成 gate 给出精确错误。
#[must_use]
pub fn active_physical_root_ids(config: &UserConfig) -> BTreeSet<String> {
    physical_root_ids(config, referenced_server_ids(config))
}

/// 当前配置中所有可由 selector / 规则切入的物理拨号根。
///
/// 与 [`active_physical_root_ids`] 的职责刻意分开：前者回答“现在谁承流”，本集合回答“本核存续期间可能
/// 热切到谁”。TUN 必须在接管系统路由前把这些根的逐目的出口一次规划完；否则目标虽已生成在 selector
/// 中，第一次选择仍会因缺路由事实被迫重启，名义热切换退化为冷切换。
#[must_use]
pub fn hot_switch_physical_root_ids(config: &UserConfig) -> BTreeSet<String> {
    physical_root_ids(
        config,
        config.servers.iter().map(|server| server.id.clone()),
    )
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests;
