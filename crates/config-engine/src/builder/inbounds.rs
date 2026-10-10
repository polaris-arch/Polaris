//! sing-box Inbound 配置生成（上游 `singbox-inbounds-builder.ts` 1:1 移植）。
//!
//! 装配 mixed inbound（HTTP+SOCKS 同口）+ 探针 inbound（probe-direct/proxy-in/update-in）+
//! TUN inbound（平台相关排除段/MTU/IPv6/macOS http_proxy platform；不发 `stack`，一律走 sing-tun 新栈）。

#![forbid(unsafe_code)]

use crate::builder::endpoint_routes::{
    collect_rule_targeted_server_ids, mesh_force_routed_servers, mesh_forced_route_cidrs,
    ObservedTailnetAddresses,
};
use crate::builder::helpers::{
    effective_app_rules, effective_custom_rules, get_custom_domestic_dns_endpoint,
    host_to_exclude_cidr, is_ipv4_host, is_ipv6_host, probe_pool_inbound_tag,
};
use crate::builder::tun_route_exclude::{compute_user_tun_exclude, UserTunExcludeInput};
use crate::singbox::inbound::TunDnsMode;
use crate::singbox::{HttpProxyPlatform, Inbound, InboundPlatform, InboundUser, UdpNatBehavior};
use crate::user_config::cidr::cidr_overlaps_any;
use crate::user_config::collections::dedupe;
use crate::user_config::dns_constants::{BOOTSTRAP_DIRECT_DNS_IPS, CONTROLLED_TUN_DNS_IP};
use crate::user_config::log_level::LogLevel;
use crate::user_config::neighbor::{is_tun_mac_filter_supported, is_valid_mac_address};
use crate::user_config::proxy_ports::{local_proxy_port, PortConfig};
use crate::user_config::rule::RuleAction;
use crate::user_config::rule_validate::is_valid_ip_cidr;
use crate::user_config::rules::rule_ip_cidrs;
use crate::user_config::system_proxy_bypass::{
    bypass_lan_cidrs, effective_bypass_lan, BypassConfig,
};
use crate::user_config::tun_config::{
    resolve_win_tun_interface_name, UdpNatType, FAKEIP_INET4_RANGE, FAKEIP_INET6_RANGE,
};
use crate::user_config::UserConfig;
use polaris_helper_proto::Platform;

/// buildInbounds 依赖注入（实例态 + FS）。上游 `InboundsDeps`。
pub struct InboundsDeps {
    pub probe_direct_port: Option<u16>,
    pub probe_proxy_port: Option<u16>,
    pub debug_probe_mixed_udp: bool,
    pub update_in_port: Option<u16>,
    pub subscription_update_in_port: Option<u16>,
    pub probe_pool_ports: Vec<u16>,
    pub platform: String,
    /// 本机所有非回环接口 CIDR（os.networkInterfaces 注入；对拍固定假值）。上游 `getOwnLanCidrs`。
    pub own_lan_cidrs: Vec<String>,
    /// 日志回调（静默剔除告警：非法段/组网重叠/macOS 物理 LAN 重叠/非直连自定义规则重叠）。
    /// 上游 `deps.log`（`singbox-inbounds-builder.ts` L308-355）。
    pub log: fn(LogLevel, &str),
    /// 运行期观测到的 tailnet 地址（serverId → 裸地址）。见 [`ObservedTailnetAddresses`]。
    ///
    /// **TUN 排除面必须看得见它**：`engaged_mesh` 是「本轮真的会发 force-route 的组网段」，
    /// 它在本文件有两个下游 —— Windows bypassLAN carve（把组网段从内核排除表里挖掉）与
    /// 「连入来源排除」减法（用户声明的排除段与组网段相交则整条丢弃，mesh 优先，否则声明段
    /// 把组网架空）。自建 tailnet 用 `32.0.0.x` 时，若这里仍只有硬编码的 `100.64.0.0/10`，
    /// 两个下游都会把真实 tailnet 前缀当成「与组网无关的段」处理 —— 规则发射那一条腿修好了，
    /// 排除面这条腿照样漏。
    pub observed_tailnet_addresses: ObservedTailnetAddresses,
    /// 回环探针/更新入站的一次性凭据（运行期每次起核由 CSPRNG 生成，只存进程内存）。
    ///
    /// **只在 [`loopback_inbounds_require_auth`] 为真的平台上被读**：桌面上传了也不发射，
    /// 桌面配置因此与改动前逐字节相同。要求凭据的平台上传 `None` ⇒ 这批入站**整批不发射**
    /// （fail-closed），绝不退回零认证。
    pub loopback_auth: Option<InboundUser>,
}

/// 要求凭据的平台上没注入凭据时的告警文案（fail-closed 腿）。
const LOOPBACK_AUTH_MISSING_WARNING: &str =
    "本平台回环入站要求凭据但未注入 → 探针/更新入站整批不发射（fail-closed）";

/// 本平台是否发射 `mixed-in`（本地 HTTP+SOCKS 同口入站）。
///
/// **单一真值**：生成侧（[`build_inbounds`]）与运行期（`ProxyStatus.mixed_port` 的赋值）都问这里。
/// 两处各写一份判据，就会出现「状态里报着一个端口、生成出来的配置里根本没人监听它」—— 解锁检测 /
/// 出口 IP / 测速回退三条消费腿连向一个不存在的口，全程无报错（Android 上真实发生过）。
///
/// Android / iOS 不发射的理由写在 [`build_inbounds`] 的 mixed 段注释里，此处不复述。
#[must_use]
pub const fn emits_mixed_inbound(platform: Platform) -> bool {
    match platform {
        Platform::Mac | Platform::Win | Platform::Linux | Platform::Other => true,
        Platform::Android | Platform::Ios => false,
    }
}

/// 本平台的回环探针/更新入站（`probe-direct-in` / `probe-proxy-in` / `probe-in-k` / `update-in` /
/// `subscription-update-in`）是否**必须**带凭据。
///
/// - **Android → 是**（陈先生 2026-09-25 裁定）：回环由全设备应用共享、无 per-app 命名空间，
///   被 `exclude_package` 排出隧道的应用照样能连这批口拿到代理出口 ⇒ 按应用排除的承诺落空。
///   凭据只在本进程内存里（每次起核重新生成），外部应用拿不到。
/// - **桌面三端 / 未知平台 → 否**：`127.0.0.1` 在桌面是机器级信任边界（同机进程本就同一用户域），
///   行为与桌面金样不变。
/// - **iOS → 否（本批不改）**：裁定只覆盖 Android。iOS 上这批入站同样是共享回环上的零认证口，
///   是否一并收口需另行裁定 —— 写成具名臂就是为了让它在这里被看见，而不是被 `_` 吸收。
#[must_use]
pub const fn loopback_inbounds_require_auth(platform: Platform) -> bool {
    match platform {
        Platform::Android => true,
        Platform::Mac | Platform::Win | Platform::Linux | Platform::Ios | Platform::Other => false,
    }
}

/// 生成 sing-box inbounds。上游 `buildInbounds`。
pub fn build_inbounds(
    config: &UserConfig,
    resolved_ips: Option<&std::collections::BTreeMap<String, String>>,
    deps: &InboundsDeps,
) -> Vec<Inbound> {
    let mut inbounds: Vec<Inbound> = Vec::new();
    // 🔴 **接管方式取「本平台生效值」，不是磁盘上存的那个**（判据与因果链全在
    // [`ProxyModeType::effective_on`] 的文档注释里，此处不复述）。
    //
    // 一句话：Android 上这个选择不存在 —— 应用能拿到的入网口只有 `VpnService` 的那一个 tun fd。
    // 而 `proxy_mode_type` 的**缺省值是 `SystemProxy`**，于是「照存的值分流」在 Android 上的
    // 直接后果是：mixed 入站本就不发（见下方那段），tun 入站又因为不是 TUN 档而不发 ⇒
    // **一个用户流量入站都没有**。核起得来、隧道建得起来、界面显示已连接，流量进去没出口，
    // 且全程零报错 —— 与「回环 `excludeRoute` 让 `establish()` 整个失败」同一档的阻断级缺陷，
    // 只是那条会炸、这条静默。
    let is_tun = config
        .proxy_mode_type
        .effective_on(Platform::parse(&deps.platform))
        .is_tun();
    let listen_addr = if config.allow_lan == Some(true) {
        "::"
    } else {
        "127.0.0.1"
    };

    // mixed inbound（HTTP+SOCKS 同口）。
    //
    // **Android 不生成**：`127.0.0.1` 在桌面是机器级信任边界，本地代理本该这么绑；Android 的回环
    // 却由全设备应用共享（无 per-app loopback 命名空间），而本结构体没有任何认证字段
    // （全文件 `users|username|password|auth` 零命中）。于是被 `exclude_package` 排出隧道的应用
    // 照样能连这个端口走代理 —— 按应用分流当场变成一句守不住的承诺，且失效静默：隧道正常、
    // 图标正常、日志无异常，用户以为排除了、实际没有。同类项目已被报过（AmneziaVPN #2452）。
    //
    // [选 A：不生成] 洞的载体不存在，是唯一不依赖「用户/攻击者不去连那个口」的处置。
    // [不选 B：加认证] 凭据要发给本机每一个需要用它的进程，Android 上没有比 loopback 更强的信道
    //   把凭据只给「该用的那个进程」——绕了一圈仍是同一条边界。
    // [不选 C：换绑地址] Android 上没有只有 VPN 内应用可达的本地地址；绑 TUN 地址反而对全隧道可达。
    //
    // 代价见 `build_inbounds` 的连带面：Android 上系统代理入口、macOS 的 `platform.http_proxy`
    // 与测速回退腿都随之消失（前两者本就是 darwin/win32 分支，第三条要走别的机制）。
    //
    // 2026-09-25 α 批补记：「第三条的别的机制」= 进程内消费方（解锁检测 / 出口 IP / 测速回退 / warm RTT）
    // 在 Android 上改走 `probe-proxy-in`，而下面那批回环探针/更新入站在 Android 上**带一次性凭据**
    // （[`loopback_inbounds_require_auth`]）。那与上面 [不选 B] 不矛盾：B 说的是「把凭据发给本机**别的**
    // 进程」没有安全信道；这批入站的消费方全在**本进程内**，凭据只在内存里、从不出进程。
    // `ProxyStatus.mixed_port` 与本判据同源（[`emits_mixed_inbound`]），本平台不发时报 0。
    //
    // 🔴 **iOS 同样不生成**（2026-09-06，主会话裁定 D1）。理由与 Android **同向但不同源**，
    // 两条都成立，任一条单独也够：
    //
    // ① 洞的载体同构：iOS 的 `127.0.0.1` 同样是**全设备应用共享**的回环，没有 per-app loopback
    //    命名空间；本结构体仍然零认证字段。差别只在「谁能连上」的名单怎么来 —— Android 是
    //    `exclude_package` 排出隧道的应用，iOS 是任何一个在前台跑着的第三方 app。iOS 上甚至
    //    没有 `exclude_package` 这条承诺可违背，但「一个零认证的本地全代理口」本身就是洞。
    // ② **发了也没人能指过来**（这条是 iOS 独有的，Android 上不成立）：iOS 没有让用户把任意
    //    第三方应用的出流量指到 `127.0.0.1:P` 的设置面，故这个入站连一个正当消费者都没有。
    //
    // ⚠️ **必须与 `ProxyModeType::effective_on` 的 `Ios` 臂同批**：那条把 iOS 的接管方式判成
    // `Tun`。若那边判 Tun 而这里仍按 `!= "android"` 发 mixed，iOS 会**同时**拥有 tun 入站
    // **和**一个零认证共享回环 —— 那是桌面三端与 Android 都没有的第三种状态。
    // 反过来若这里排除了 iOS 而那边没判 Tun，则是「一个用户流量入站都没有」（见函数顶注）。
    // 两条判据一起才成立，故写在同一批里。
    let platform = Platform::parse(&deps.platform);
    if emits_mixed_inbound(platform) {
        inbounds.push(Inbound {
            type_field: "mixed".into(),
            tag: crate::builder::helpers::MIXED_INBOUND_TAG.into(),
            listen: Some(listen_addr.into()),
            listen_port: Some(local_proxy_port(config)),
            network: None,
            interface_name: None,
            address: None,
            mtu: None,
            auto_route: None,
            dns_mode: None,
            auto_redirect: None,
            strict_route: None,
            udp_mapping: None,
            udp_filtering: None,
            route_exclude_address: None,
            include_mac_address: None,
            exclude_mac_address: None,
            include_package: None,
            exclude_package: None,
            platform: None,
            users: None,
        });
    }

    // 回环探针/更新入站的认证表。三种结局，**没有第四种**：
    //   不要求凭据的平台 → `Some(None)`：照旧零认证（桌面金样逐字节不变，传进来的凭据也不用）；
    //   要求且有凭据     → `Some(Some(users))`：这批入站全部带同一份一次性凭据；
    //   要求却没有凭据   → `None`：**整批不发射**（fail-closed）。宁可让测速/更新/订阅经代理这几条
    //                     能力当场失效（各自已有「端口为 0 → 直连或报错」的腿），也不在共享回环上
    //                     开一个零认证的代理口 —— 后者失效是静默的，前者看得见。
    let loopback_users: Option<Option<Vec<InboundUser>>> = match (
        loopback_inbounds_require_auth(platform),
        &deps.loopback_auth,
    ) {
        (false, _) => Some(None),
        (true, Some(user)) => Some(Some(vec![user.clone()])),
        (true, None) => None,
    };
    // 只在「确实有入站被压掉」时告警：预览等只关心 TUN 段的调用方本就一个端口都不传，
    // 无条件告警会把一句与它无关的话塞进它们收集的告警面。
    let any_loopback_port = deps.probe_direct_port.is_some()
        || deps.probe_proxy_port.is_some()
        || !deps.probe_pool_ports.is_empty()
        || deps.update_in_port.is_some()
        || deps.subscription_update_in_port.is_some();
    if loopback_users.is_none() && any_loopback_port {
        (deps.log)(LogLevel::Warn, LOOPBACK_AUTH_MISSING_WARNING);
    }

    if let Some(users) = loopback_users {
        // 探针 inbound（两条能力独立接线；只启用代理出口探针时不必白占一个 direct 端口）。
        if let Some(dp) = deps.probe_direct_port {
            inbounds.push(http_loopback("probe-direct-in", dp, users.clone()));
        }
        if let Some(pp) = deps.probe_proxy_port {
            let mut probe = http_loopback(
                crate::builder::helpers::PROBE_PROXY_INBOUND_TAG,
                pp,
                users.clone(),
            );
            // Same authenticated tag/selector and HTTP behavior; SOCKS5 adds UDP only in Debug.
            if cfg!(debug_assertions) && deps.platform == "android" && deps.debug_probe_mixed_udp {
                probe.type_field = "mixed".into();
            }
            inbounds.push(probe);
        }

        // §15 探测池 probe-in-k。
        for (k, port) in deps.probe_pool_ports.iter().enumerate() {
            inbounds.push(http_loopback(
                &probe_pool_inbound_tag(k),
                *port,
                users.clone(),
            ));
        }

        // update-in（socks，图标/资源等共享更新链路）。
        if let Some(up) = deps.update_in_port {
            inbounds.push(socks_loopback("update-in", up, users.clone()));
        }
        // 订阅专用更新入口：仅此入口附带逐目标 resolve + 私网拒绝规则。
        if let Some(port) = deps.subscription_update_in_port {
            inbounds.push(socks_loopback(
                crate::builder::subscription_guard::SUBSCRIPTION_UPDATE_INBOUND_TAG,
                port,
                users,
            ));
        }
    }

    // TUN inbound。Android 上 `is_tun` 恒真（见函数开头那段），故这里是**无条件**发射。
    if is_tun {
        if let Some(tun) = build_tun_inbound(config, resolved_ips, deps) {
            inbounds.push(tun);
        }
    }

    inbounds
}

fn http_loopback(tag: &str, port: u16, users: Option<Vec<InboundUser>>) -> Inbound {
    Inbound {
        type_field: "http".into(),
        tag: tag.into(),
        listen: Some("127.0.0.1".into()),
        listen_port: Some(port),
        network: None,
        interface_name: None,
        address: None,
        mtu: None,
        auto_route: None,
        dns_mode: None,
        auto_redirect: None,
        strict_route: None,
        udp_mapping: None,
        udp_filtering: None,
        route_exclude_address: None,
        include_mac_address: None,
        exclude_mac_address: None,
        include_package: None,
        exclude_package: None,
        platform: None,
        users,
    }
}

/// 只听 UDP 的回环 `direct` 入站（网络场景 canary 探针用，spec §6.3 方案 2）。
///
/// **恒 `127.0.0.1`，不看 `allowLan`**：它和其它探针入站一样只服务本应用自己，局域网放开只针对 mixed-in。
pub(crate) fn udp_direct_loopback(tag: &str, port: u16) -> Inbound {
    Inbound {
        type_field: "direct".into(),
        network: Some("udp".into()),
        ..socks_loopback(tag, port, None)
    }
}

fn socks_loopback(tag: &str, port: u16, users: Option<Vec<InboundUser>>) -> Inbound {
    Inbound {
        type_field: "socks".into(),
        tag: tag.into(),
        listen: Some("127.0.0.1".into()),
        listen_port: Some(port),
        network: None,
        interface_name: None,
        address: None,
        mtu: None,
        auto_route: None,
        dns_mode: None,
        auto_redirect: None,
        strict_route: None,
        udp_mapping: None,
        udp_filtering: None,
        route_exclude_address: None,
        include_mac_address: None,
        exclude_mac_address: None,
        include_package: None,
        exclude_package: None,
        platform: None,
        users,
    }
}

/// 用户「NAT 类型」档 → sing-box `(udp_mapping, udp_filtering)` 取值对。
///
/// # 表本身（RFC 3489 §5 的锥形分类，逐档钉在 `udp_nat_type_maps_each_tier`）
///
/// | 档 | `udp_mapping` | `udp_filtering` |
/// |---|---|---|
/// | 全锥 | `endpoint_independent` | `endpoint_independent` |
/// | 受限锥 | `endpoint_independent` | `address_dependent` |
/// | 端口受限锥 | `endpoint_independent` | `address_and_port_dependent` |
///
/// **三档的 mapping 全是 `endpoint_independent`，这不是复制粘贴漏改**：锥形（cone）的定义就是
/// 「同一本地端口对所有目的地共用同一个外部映射」，三种锥的差别**只在 filtering**。mapping 一旦
/// 收紧就不再是锥形而是对称 NAT —— 那个档本仓刻意不提供（理由见 [`UdpNatType`] 的文档注释）。
///
/// # 为什么两个键一起发，而不是「只发变化的那个 filtering」
///
/// 只发 filtering 的话，「全锥」这一档会退化成「filtering 跟默认一样、mapping 听天由命」：档位名对
/// 用户的承诺（这是全锥）就依赖于上游默认恰好也是 `endpoint_independent`。上游改默认，档位名当场
/// 变成谎话且没有任何门会红。选了档 = 用户要一个**确定**的 NAT 形态，两个键一起钉死才兑现得了。
/// 反过来「不选档」仍是一个键都不发（见 [`build_tun_inbound`] 的 `None` 腿），那才是「跟随内核」。
fn udp_nat_behaviors(nat: UdpNatType) -> (UdpNatBehavior, UdpNatBehavior) {
    match nat {
        UdpNatType::FullCone => (
            UdpNatBehavior::EndpointIndependent,
            UdpNatBehavior::EndpointIndependent,
        ),
        UdpNatType::RestrictedCone => (
            UdpNatBehavior::EndpointIndependent,
            UdpNatBehavior::AddressDependent,
        ),
        UdpNatType::PortRestrictedCone => (
            UdpNatBehavior::EndpointIndependent,
            UdpNatBehavior::AddressAndPortDependent,
        ),
    }
}

fn build_tun_inbound(
    config: &UserConfig,
    resolved_ips: Option<&std::collections::BTreeMap<String, String>>,
    deps: &InboundsDeps,
) -> Option<Inbound> {
    let should_bypass_lan = config.bypass_lan != Some(false);
    let fakeip_ranges: Vec<String> = if uses_fake_ip(config) && deps.platform != "linux" {
        let mut r = vec![FAKEIP_INET4_RANGE.to_string()];
        if config.enable_ipv6 == Some(true) {
            r.push(FAKEIP_INET6_RANGE.to_string());
        }
        r
    } else {
        vec![]
    };

    // engaged mesh force-route 段：rule_targeted 含 custom + app 规则指向的节点。
    let mut rule_targeted = collect_rule_targeted_server_ids(&effective_custom_rules_proxy(config));
    for app in &effective_app_rules_proxy(config) {
        if app.enabled && app.action == RuleAction::Proxy {
            if let Some(tid) = &app.target_server_id {
                rule_targeted.insert(tid.clone());
            }
        }
    }
    let engaged_mesh = mesh_forced_route_cidrs(
        &mesh_force_routed_servers(
            &config.servers,
            config.selected_server_id.as_deref(),
            &rule_targeted,
        ),
        &deps.observed_tailnet_addresses,
    );

    // TUN 地址。提前到排除表之前算：移动端「绕过局域网」要从排除段里挖掉 TUN 自身网段
    // （见 [`mobile_bypass_lan_exclude`]），其余平台对它无依赖。
    let tun_cfg = config.tun_config.as_ref();
    let mut tun_address = vec![tun_cfg
        .and_then(|t| t.inet4_address.clone())
        .unwrap_or_else(|| {
            if deps.platform == "darwin" {
                "172.19.0.1/30".into()
            } else {
                "172.19.0.1/16".into()
            }
        })];
    if config.enable_ipv6 == Some(true) {
        tun_address.push(
            tun_cfg
                .and_then(|t| t.inet6_address.clone())
                .unwrap_or_else(|| "fdfe:dcba:9876::1/126".into()),
        );
    }

    let mut exclude_addr: Vec<String> = if deps.platform == "win32" && should_bypass_lan {
        let bypass = bypass_lan_cidrs(&effective_bypass_lan(&UConfigBypass(config)));
        let win = crate::builder::tun_route_exclude::compute_win_bypass_exclude(
            &crate::builder::tun_route_exclude::WinBypassExcludeInput {
                bypass_cidrs: &bypass,
                engaged_mesh_cidrs: &engaged_mesh,
                own_lan_cidrs: &deps.own_lan_cidrs,
                fakeip_ranges: &fakeip_ranges,
            },
        );
        win.exclude
    } else if deps.platform == "linux" {
        // Linux 加法态：route_exclude 恒空（VM185 实证）。
        vec![]
    } else if deps.platform == "android" {
        // 🔴 Android 的**平台基线恒空**，且这条不是「不需要」而是「发了就起不来」。
        //
        // ⚠️ **射程：这里说的只有基线这一半**（2026-09-06 更正，上一版写的是「Android 恒空」，
        // 那是一句已经为假的全称话）。下面 `:418` 那段的用户声明段（`tunConfig.inboundExcludeCidrs`）
        // **只对 linux 短路**，android 走 `else` 支，算完之后 `exclude_addr.extend(user_exclude.extra)`
        // 照样往这个数组里加 ⇒ Android 的 `tun.route_exclude_address` **可以非空**。
        // 谁按旧注释以为「生成侧在 android 上一条都不发」，就会去掉承载侧
        // （`PolarisVpnService.kt` 的 `excludeRouteTolerantly`）那层容错。
        //
        // Android 的 tun 由 `VpnService.Builder` 建，`route_exclude_address` 最终落到
        // `Builder.excludeRoute()`。该方法**拒收回环前缀**：喂 `127.0.0.0/8` 抛 `Bad address`，
        // 而 Builder 的语义是任何一条被拒就整个 `establish()` 失败 ⇒ 症状是
        // `configure tun interface: Bad address`，**隧道一次都建不起来**（2026-09-04 模拟器实测，
        // logcat 原文 `W/PolarisVpnService: 排除路由 127.0.0.0/8 被系统拒收`）。
        // 用户声明段那条腿上的同一枚毒输入，由下面 `drop_mobile_loopback_excludes` 在生成侧剔掉。
        //
        // 而且基线那两条本来也不需要：回环流量在 Android 上根本不进 VPN 的路由表，内核直接在 lo 上
        // 闭环，排不排除都一样。所以这里发空集是「按平台给对」，不是为了绕开一个报错。
        //
        // 承载侧（`PolarisVpnService`）另有逐条容错 + 点名警告，那是**第二道**：
        // 系统还会拒收别的形态（同一个 Builder 对多播/保留段也挑），生成侧不可能穷举。
        // 两道都要，删任何一道都会回到「一条拒收 = 整条隧道建不起来」。
        //
        // 「绕过局域网」（2026-09-25 接线）：基线仍是空集，开关开着时加上 `bypassLANList` 的
        // 网段子集。此前这一支恒空，依据只有上面那条**回环**拒收实证，它不覆盖 RFC1918 等段；
        // 回环段照样在生成侧剔掉，其余处理见 [`mobile_bypass_lan_exclude`]。
        if should_bypass_lan {
            mobile_bypass_lan_exclude(config, deps, &engaged_mesh, &fakeip_ranges, &tun_address)
        } else {
            vec![]
        }
    } else if deps.platform == "ios" {
        // 🔴 iOS 恒空。**独立分支而不是并进 Android**：Android 那条的依据是一条实测的拒收
        // （`VpnService.Builder.excludeRoute("127.0.0.0/8")` 抛 `Bad address` ⇒ `establish()`
        // 整个失败），iOS 上对应的 `NEPacketTunnelNetworkSettings` 收不收回环前缀
        // **本仓没有验证过**（构不出 iOS 产物）。把两者并成一条就是把一个未验证的猜测
        // 冒充成那条实测结论。
        //
        // 那为什么仍然发空集？因为**另一半理由与 Android 逐字相同、且不依赖那次实测**：
        // 回环流量根本不进隧道的路由表 —— 内核在 lo 上直接闭环，排不排除都一样。⇒ 发这两条
        // 前缀在 iOS 上**买不到任何东西**，而代价上限是「隧道一次都建不起来」（若 NE 也拒收）。
        // 收益为零、代价上限为阻断级 ⇒ 不发。
        //
        // **未验证，待真机实测**：NE 是否拒收回环排除；若实测为「接受」，这一条仍应保持空集
        // （理由是上一段的「买不到任何东西」），只是可以把风险那半划掉。
        //
        // 「绕过局域网」与 Android 同做（2026-09-25）：移动端 UI 是两端共用的一份
        // （`ui/src/mobile/settings/TunPage.tsx`），开关在 iOS 上若不发射就是一颗按下去什么都不
        // 发生的开关；而本仓没有任何「NE 拒收 RFC1918 排除段」的平台事实。回环段同样剔掉
        // （理由见上段），NE 上的实际接受情况**未验证**，待有 iOS 产物时实测。
        if should_bypass_lan {
            mobile_bypass_lan_exclude(config, deps, &engaged_mesh, &fakeip_ranges, &tun_address)
        } else {
            vec![]
        }
    } else {
        // mac/其它：回环排除。
        vec!["127.0.0.0/8".into(), "::1/128".into()]
    };

    // Windows 额外排除 DNS IP（防回流死循环）。
    if deps.platform == "win32" {
        let mut dns_ips: Vec<String> = BOOTSTRAP_DIRECT_DNS_IPS
            .iter()
            .map(|s| s.to_string())
            .collect();
        dns_ips.push(CONTROLLED_TUN_DNS_IP.to_string());
        dns_ips = dedupe(dns_ips);
        for ip in &dns_ips {
            exclude_addr.push(format!("{ip}/32"));
        }
        if let Some((ip, _port)) = get_custom_domestic_dns_endpoint(
            config
                .dns_config
                .as_ref()
                .and_then(|d| d.domestic_dns.as_deref()),
        ) {
            if let Some(cidr) = host_to_exclude_cidr(&ip) {
                exclude_addr.push(cidr);
            }
        }
    }

    // 节点 IP 排除（非 Linux）。
    if deps.platform != "linux" {
        let mut all_server_ids: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        if let Some(sid) = &config.selected_server_id {
            all_server_ids.insert(sid.clone());
        }
        for r in &effective_app_rules_proxy(config) {
            if let Some(tid) = &r.target_server_id {
                all_server_ids.insert(tid.clone());
            }
        }
        for sid in &all_server_ids {
            if let Some(server) = config.servers.iter().find(|s| &s.id == sid) {
                if is_ipv4_host(&server.address) || is_ipv6_host(&server.address) {
                    if let Some(cidr) = host_to_exclude_cidr(&server.address) {
                        exclude_addr.push(cidr);
                    }
                } else if let Some(ips) = resolved_ips {
                    if let Some(ip) = ips.get(sid) {
                        if let Some(cidr) = host_to_exclude_cidr(ip) {
                            exclude_addr.push(cidr);
                        }
                    }
                }
                // Tailcat 的 `address` 恒空（对端经 DERP 定位），上面两腿什么都不加；其「服务器」是 DERP。
                // servers 模式：各项的 host（裸字符串项即 host）/ `ipv4` / `ipv6` 是 IP 字面值 → 排除；
                // 主机名 / `"none"` / 空由 `host_to_exclude_cidr` 返回 None 跳过，同一 IP 只加一次（host 常与 ipv4 同值）。
                // region / 自定义 map 模式 DERP IP 在起核后才从地图得知，此处无从排除 ——
                // 靠 `route.auto_detect_interface` 让内核自身的拨号绑物理网卡兜底。
                if let Some(t) = server.tailcat_settings.as_deref().filter(|_| {
                    server.protocol == crate::user_config::server_config::Protocol::Tailcat
                }) {
                    for item in &t.derp_servers {
                        let field = |k: &str| item.get(k).and_then(serde_json::Value::as_str);
                        let host = item.as_str().or_else(|| field("host"));
                        for addr in [host, field("ipv4"), field("ipv6")].into_iter().flatten() {
                            if let Some(cidr) = host_to_exclude_cidr(addr) {
                                if !exclude_addr.contains(&cidr) {
                                    exclude_addr.push(cidr);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 「连入来源排除」（上游 singbox-inbounds-builder.ts L297-358）：本机作服务端被 **off-subnet** 私网连入
    // （如经 ZeroTier→路由器 DNAT）时，回包目的地落在本机直连子网外 → 被 TUN 捕获 → 用户态栈误当新连接重拨 →
    // 连接断。把用户显式声明的来源网段追加进 `route_exclude_address`，内核层就不把该段交给 TUN、回包走物理网卡。
    // ⚠️ 双向语义：排除一个段会让该段出/入两个方向都绕过 TUN，故该段不再能经代理/自定义规则出网。
    //
    // 减法（顺序即 compute_user_tun_exclude 内部顺序）：先规范化（裸 IP 补 /32|/128、拒非法/过宽，否则
    // sing-box `netip.ParsePrefix` FATAL 或半个地址空间被排出 TUN），再减 engaged 组网 force-route 段
    // （mesh 优先，否则声明段把组网架空），再减 fakeip 段（假 IP 被排出 TUN → 绕过 fakeip 反查、服务端收不到域名），
    // macOS 额外减本机物理 LAN 段（排除物理 LAN 触发 NetworkExtension 反向路由拦截、drop 从 TUN 发回该段的回包）。
    // own_lan_cidrs 无条件传入，平台判定在 compute_user_tun_exclude 内（非 darwin 忽略该参数）。
    //
    // **Linux 恒忽略、不发射**（上游 L304-311 同款短路）：Linux 加法态下 route_exclude 非空即触发策略路由表
    // 2022 两族分解 → 表 9001 全抓 → same/off-subnet 服务端连入与 allowLan 回包全断；而 Linux 服务端回包本就由
    // 内核策略路由天然保护（表 9002 main 具体路由优先），此项对它既不需要、又是毒丸。故这里**不生成无效字段**，
    // 与 UI「Linux 无效」说明一致 —— 上面 exclude_addr 初始化的 linux 分支恒 [] 也是同一不变量的另一半。
    // 注：mesh/fakeip 减法**不能**在 Linux 上"算了但不发射"——engaged_mesh/fakeip_ranges 在 Linux 下本就是
    // 空/未算（fakeip_ranges 有 linux 守卫），进来只会白算。
    let user_inbound_cidrs: &[String] = config
        .tun_config
        .as_ref()
        .and_then(|t| t.inbound_exclude_cidrs.as_deref())
        .unwrap_or(&[]);
    if !user_inbound_cidrs.is_empty() && deps.platform == "linux" {
        // Linux 忽略腿也必须**出声**（上游 L304-311 有、移植时丢了）：用户在 UI 里填了段、
        // 生成侧整块跳过，日志里一个字都没有 = 与「填了但没生效」不可区分。与本批「静默剔除必告警」
        // 的其余三类（非法/过宽、组网重叠、macOS 物理 LAN）同档 warn，口径一致。
        (deps.log)(
            LogLevel::Warn,
            &format!(
                "Linux：服务端回包已由内核策略路由天然保护，「连入来源排除」不生效且会重新触发路由表分解引入回归，已忽略 {} 条声明段。",
                user_inbound_cidrs.len()
            ),
        );
    } else if !user_inbound_cidrs.is_empty() {
        let mut user_exclude = compute_user_tun_exclude(&UserTunExcludeInput {
            platform: &deps.platform,
            user_cidrs: user_inbound_cidrs,
            mesh_cidrs: &engaged_mesh,
            fakeip_ranges: &fakeip_ranges,
            own_lan_cidrs: &deps.own_lan_cidrs,
        });

        // 🔴 移动端：基地址为回环的前缀**不发**（生成侧收口，2026-09-06 新增，射程与依据见
        // [`drop_mobile_loopback_excludes`]）。
        //
        // 这条腿此前没有 UI，本批（移动端 `settings/TunPage.tsx` 的「连入来源排除」）第一次让用户
        // 填得进来 ⇒ 也是第一次能把 `127.0.0.0/8` 这枚**仓里已经点过名**的毒输入送到 Android 上：
        // `VpnService.Builder.excludeRoute` 对回环前缀抛 `Bad address`，Builder 的语义是一条被拒
        // 整个 `establish()` 失败（依据同上面 android 基线那段的模拟器实测）。
        // 在此之前挡住它的唯一一件事是承载侧 `PolarisVpnService.kt:150 excludeRouteTolerantly`
        // 的逐条 `runCatching`，而那段头注自己写着「真正该修的是生成侧」——这里就是那个收口。
        //
        // **只剔回环，不剔别的**：AOSP `VpnService.Builder.check()` 判的就是
        // `prefix.getAddress().isLoopbackAddress()` 这一条（v4 首字节 127 / v6 `::1`），
        // 与 [`is_loopback_prefix`] 用的 `IpAddr::is_loopback()` 同判据。多播/链路本地本仓没有
        // 拒收实证，跟着一起剔就是**替系统发明规则**、静默吞掉用户声明的段。它们仍由承载侧那道兜住。
        let dropped_loopback =
            drop_mobile_loopback_excludes(&deps.platform, &mut user_exclude.extra);
        if !dropped_loopback.is_empty() {
            // 与本块其余四类静默剔除同档 warn：剔了不出声 = 与「填了但没生效」不可区分。
            (deps.log)(
                LogLevel::Warn,
                &format!(
                    "移动端：「连入来源排除」{} 段是回环前缀，系统 VPN 接口拒收（一条被拒整条隧道建不起来），已跳过；回环流量本就不进 VPN 路由表，排除它买不到任何东西：{}",
                    dropped_loopback.len(),
                    dropped_loopback.join(", ")
                ),
            );
        }

        // 静默剔除告警（上游 L324-355）：4 类，用户填的段被剔时零线索，须逐类 warn。
        if user_exclude.dropped_invalid > 0 {
            (deps.log)(
                LogLevel::Warn,
                &format!(
                    "「连入来源排除」剔除 {} 条非法/过宽网段（须合法 CIDR、不含 0.0.0.0/0 等过宽段）。",
                    user_exclude.dropped_invalid
                ),
            );
        }
        if !user_exclude.dropped_mesh_overlap.is_empty() {
            (deps.log)(
                LogLevel::Warn,
                &format!(
                    "「连入来源排除」{} 段与生效组网(WG/Tailscale)路由段重叠，已跳过排除（该段经组网节点）：{}",
                    user_exclude.dropped_mesh_overlap.len(),
                    user_exclude.dropped_mesh_overlap.join(", ")
                ),
            );
        }
        if !user_exclude.dropped_own_lan_mac.is_empty() {
            (deps.log)(
                LogLevel::Warn,
                &format!(
                    "macOS：「连入来源排除」{} 段与本机物理 LAN 相交，已跳过（排除物理 LAN 会触发 NetworkExtension 反向路由丢包）：{}",
                    user_exclude.dropped_own_lan_mac.len(),
                    user_exclude.dropped_own_lan_mac.join(", ")
                ),
            );
        }

        // 与非直连（走代理/拦截）自定义规则段重叠告警（上游 L342-356，双向语义副作用）：被排除的段出/入
        // 均绕过 TUN，若某 enabled 的非直连（proxy/block）custom rule 想把该段走代理/拦截，会被静默架空。
        // 刻意**不减**（排除=用户显式声明意图更明确），仅告警。
        let overridable_rule_cidrs: Vec<String> = effective_custom_rules_proxy(config)
            .iter()
            .filter(|r| r.enabled && r.route_action() != Some(RuleAction::Direct))
            .flat_map(rule_ip_cidrs)
            .collect();
        if !overridable_rule_cidrs.is_empty() {
            let conflict: Vec<String> = user_exclude
                .extra
                .iter()
                .filter(|c| cidr_overlaps_any(c, &overridable_rule_cidrs))
                .cloned()
                .collect();
            if !conflict.is_empty() {
                (deps.log)(
                    LogLevel::Warn,
                    &format!(
                        "「连入来源排除」{} 段与非直连（走代理/拦截）自定义规则段重叠：排除使其出/入均绕过 TUN 走直连，该自定义规则对这些段将不生效：{}",
                        conflict.len(),
                        conflict.join(", ")
                    ),
                );
            }
        }

        exclude_addr.extend(user_exclude.extra);
    }

    // 不发 `stack`：sing-box 1.15.0-alpha.3 起写了就报弃用（含 `"go"`），缺席即走 sing-tun 新栈。
    // `Inbound` 已无该字段，故这里无从发出；用户配置里遗留的 `stack` 在反序列化时即被忽略。
    //
    // MTU：用户显式值逐字下发；缺席则**不发键**，由内核按运行环境取默认（上游
    // `protocol/tun/inbound.go` 的 `options.MTU == 0` 分支，理由见 `TunModeConfig::mtu`）。
    // 此处**没有**任何哨兵值 —— 旧实现把 `Some(9000)` 当「未设置」，导致真想要 9000 的用户被静默改写。
    let user_mtu = tun_cfg.and_then(|t| t.mtu);

    let auto_route = tun_cfg.map(|t| t.auto_route).unwrap_or(true);
    // Audited sing-tun 5c2edb183cc9: unset + auto_route=true is hijack,
    // unset + false is disabled. Preserve the user's flag and compensate only
    // known desktop manual-routing TUNs. Mobile stays on its alpha.8 contract.
    // Windows delivery additionally requires the source-owner fix restoring
    // both family AutoRoute guards and SetDNS(family, nil, nil) else branches;
    // unmodified D052 + explicit hijack changes interface DNS. The dependency
    // fingerprint gate remains closed until that source closure is signed.
    let dns_mode = if !auto_route
        && matches!(
            Platform::parse(&deps.platform),
            Platform::Linux | Platform::Mac | Platform::Win
        ) {
        Some(TunDnsMode::Hijack)
    } else {
        None
    };

    // 🔴 `strict_route`：**Android 上一个键都不发**（其余平台逐字下发用户档位）。
    //
    // 它在 Android 上是**彻底的空转**，两层证据：
    //
    // ① 源码链（sing-box v1.14.0 + sing-tun v0.9.0-beta.4，读源码取证）：
    //    - `experimental/libbox/service.go:54` 的 `UsePlatformInterface()` 恒 true；
    //    - 于是 `protocol/tun/inbound.go:428` 走 `platformInterface.OpenInterface`，
    //      `tun.New()`（`else` 腿，:431）永不执行 ⇒ 没有 `NativeTun`；
    //    - 而该字段在 Linux/Android 侧的**唯一**落点是 `sing-tun/tun_linux.go:929`，位于
    //      `NativeTun::rules()`（:686），只由 `setRules()`（:354，`NativeTun::Start()` 内）调用；
    //    - libbox 虽把它透出为 `TunOptions::GetStrictRoute()`（`experimental/libbox/tun.go:118`），
    //      官方 Android 客户端全仓大小写不敏感 `strictroute` **零命中** ⇒ 平台侧也没人读。
    // ② 真机（模拟器）正反双向对照，2026-09-04：起隧道前后 `iptables -S` / `ip6tables -S`
    //    **逐字节相同**（md5 与行数都不变）⇒ 内核侧一条防绕行规则都没装。正对照是同一时刻
    //    `ip rule` **+14 条**（全是 Android netd 为 `VpnService` 装的 uidrange 规则）⇒
    //    观测手段确实看得见「规则被装上」这件事，前一条的相等不是量错了。
    //
    // # 为什么是「不发」，而不是照发、也不是发 `false`
    //
    // 非 root 机器走 `VpnService` 这条路**不支持**这个能力，处置口径是**能力缺席**：
    //
    // - [不选：照发 `true`] 那是在配置里写一句假话。任何读配置排查问题的人（包括未来的我们）
    //   都会以为防绕行是开着的 —— 而 UI 侧已经按同一条判据把开关**撤掉**了
    //   （`ui/src/mobile/settings/TunPage.tsx`：一个承诺了「严格锁入 TUN 防止绕行泄漏」却
    //   一行都不跑的开关比没有更坏），发射面再留着它，就成了「UI 说没有、配置说有」。
    // - [不选：发 `false`] 同样是假话，只是方向相反：`false` 的语义是「这台设备支持、但我们
    //   刻意关掉了」，与桌面用户主动关掉它时产出的配置**字面相同** ⇒ 两件完全不同的事在配置里
    //   长得一模一样。而且它把上游当前的默认值冻进每一份配置（与 `udp_mapping/udp_filtering`
    //   缺席即不发是同一条判据，见 `singbox::Inbound::udp_mapping`）。
    // - [选：一个键都不发] `None` 是本文件里「这个平台没有这个东西」的既有形态
    //   （同 Android 的 `route_exclude_address`、非 darwin 的 `platform.http_proxy`）。
    //
    // # 为什么这一处留在字符串轴上，而函数开头的 `is_tun` 收进了 `Platform` 枚举
    //
    // 两者不是同一类判据：`is_tun` 那条**跨四个 builder**（入站/路由/日志/DNS）共享同一个答案，
    // 必须只有一个判断点，故收进 `ProxyModeType::effective_on` 的穷举 `match`，让编译器替新平台
    // 守着。本条只管**本文件这一个字段的发射**，与紧邻的 `route_exclude_address` / `exclude_package`
    // 两条 Android 臂同形同轴 —— 把三条里的一条换成枚举反而割裂。字符串轴的守法是登记表：
    // 本条已录入 `src-tauri/tests/platform_dispatch_exhaustive.rs` 的 `STRING_DISPATCH_REGISTRY`，
    // 连同「一个本仓从未构建过的平台在这里得到什么」一起写下（新增即红 / 腐烂即红）。
    //
    // 🔴 **iOS 同样一个键都不发**（2026-09-06）。两条依据分开记，强弱不同：
    //
    // ① **仓内可查、不依赖任何未验证的上游细节（承重的那条）**：移动端 UI 是 Android/iOS
    //    共用的一份（`ui/mobile.html` → `ui/src/mobile/*`，见 `src-tauri/src/lib.rs` 的
    //    `#[cfg(mobile)]` 换文档那一行），而 `ui/src/mobile/settings/TunPage.tsx` 里
    //    `strict_route` 开关**整个不显示**。若 iOS 照发 `true`，就成了「UI 上没有这个东西、
    //    配置里却写着防绕行已开启」—— 与 Android 那条要避免的形态逐字相同。
    // ② **推论，未验证**：Android 那条的源码链（libbox `UsePlatformInterface()` 恒 true ⇒
    //    走 `platformInterface.OpenInterface` ⇒ 没有 `NativeTun` ⇒ 该字段唯一落点
    //    `sing-tun/tun_linux.go` 的 `NativeTun::rules()` 永不执行）在 iOS 上**只对了一半**：
    //    NE 扩展同样经 libbox 的 platform interface 拿 fd，但 `tun_linux.go` 在 Apple 上根本
    //    不参与编译，iOS 侧对应的是 `tun_darwin.go` —— 那份实现本仓没有读过。故不拿这条当依据。
    //
    // 处置口径与 Android 同：**能力缺席 ⇒ 一个键都不发**，而不是发 `true`（写一句假话）或
    // 发 `false`（把「不支持」与「用户主动关掉」在配置里写成同一个样子）。
    let strict_route = if deps.platform == "android" || deps.platform == "ios" {
        None
    } else {
        Some(tun_cfg.map(|t| t.strict_route).unwrap_or(true))
    };

    // NAT 类型：**缺席就一个键都不发**（`unwrap_or((None, None))`，不是「缺席回落成全锥」）。
    // 上游两项默认已是 endpoint_independent（全锥），回落写死等价值只会把「当前默认」冻进每一份
    // 生成的配置 —— 金样 config-snapshot.json 当场 delta，且上游日后改默认时我们钉在旧值上却没有
    // 任何判据支撑（对比 MTU：那里显式下发是为了让设置页占位符与内核实际取值是同一个数）。
    let (udp_mapping, udp_filtering) = tun_cfg
        .and_then(|t| t.udp_nat_type)
        .map(|nat| {
            let (m, f) = udp_nat_behaviors(nat);
            (Some(m), Some(f))
        })
        .unwrap_or((None, None));

    let mut tun = Inbound {
        type_field: "tun".into(),
        tag: "tun-in".into(),
        listen: None,
        listen_port: None,
        network: None,
        interface_name: None,
        address: Some(tun_address),
        mtu: user_mtu,
        auto_route: Some(auto_route),
        dns_mode,
        auto_redirect: None,
        strict_route,
        udp_mapping,
        udp_filtering,
        route_exclude_address: None,
        include_mac_address: None,
        exclude_mac_address: None,
        include_package: None,
        exclude_package: None,
        platform: None,
        users: None,
    };
    if !exclude_addr.is_empty() {
        tun.route_exclude_address = Some(exclude_addr);
    }

    // Linux 的 systemd-resolved per-link 接管、app marker 与 root helper 共同引用该稳定接口名；
    // 不允许内核随机命名，否则网络热切换后无法重放，也无法做到 helper 最窄白名单。
    if deps.platform == "linux" {
        tun.interface_name = Some(polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME.to_owned());
    }

    // Windows 接口名。
    if deps.platform == "win32" {
        let ifname =
            resolve_win_tun_interface_name(tun_cfg.and_then(|t| t.interface_name.as_deref()));
        tun.interface_name = Some(ifname);
    }

    // P6 LAN MAC 过滤（仅 Linux + auto_route + 合法 MAC）。
    if let Some(mac_mode) = tun_cfg.and_then(|t| t.mac_filter_mode) {
        if is_tun_mac_filter_supported(&deps.platform) && auto_route {
            let macs: Vec<String> = tun_cfg
                .map(|t| {
                    t.mac_filter_list
                        .iter()
                        .map(|m| m.trim().to_string())
                        .filter(|m| is_valid_mac_address(Some(m)))
                        .collect()
                })
                .unwrap_or_default();
            if !macs.is_empty() {
                tun.auto_redirect = Some(true);
                match mac_mode {
                    crate::user_config::neighbor::TunMacFilterMode::Exclude => {
                        tun.exclude_mac_address = Some(macs)
                    }
                    crate::user_config::neighbor::TunMacFilterMode::Include => {
                        tun.include_mac_address = Some(macs)
                    }
                }
            }
        }
    }

    // macOS http_proxy platform。
    //
    // **不扩到 Android**：这个键在 Android 侧会被消费成 `VpnService.Builder.setHttpProxy`
    // （官方客户端 `VPNService.kt:167`），把**全设备**的 HTTP 代理指到 `server_port` ——
    // 而 Android 上那个端口的监听已经被上面那条裁定拿掉了，指过去只会让全设备 HTTP 连一个空口。
    if deps.platform == "darwin" {
        tun.platform = Some(InboundPlatform {
            http_proxy: Some(HttpProxyPlatform {
                enabled: true,
                server: "127.0.0.1".into(),
                server_port: local_proxy_port(config),
            }),
        });
    }

    // Android 按应用分流（映射表与方向依据见 `android_exclude_packages`）。
    if deps.platform == "android" {
        let exclude_packages = android_exclude_packages(config);
        if !exclude_packages.is_empty() {
            tun.exclude_package = Some(exclude_packages);
        }
    }

    Some(tun)
}

/// Android 按应用分流 → tun 层 `exclude_package`。
///
/// # 映射表（本函数就是这张表的实现，改表先改这里的依据）
///
/// | `AppRule::action` | tun 字段 | 依据 |
/// |---|---|---|
/// | `Direct` | 追加进 `exclude_package` | **同向**：桌面 direct 与 Android「不进隧道」都兑现「这个应用不经代理出网」 |
/// | `Proxy` | **一个 tun 字段都不写** | 默认全设备在隧道内，proxy 已经是缺省态；它要的是「走哪个节点」，那是 route 规则的事 |
/// | `Block` | **一个 tun 字段都不写** | 排除 = 该应用拿到完整互联网，与「阻断」方向**完全相反** |
/// | 任何一档 | `include_package` **恒不发射** | 它是白名单，语义见 [`crate::singbox::Inbound::include_package`] |
///
/// # 为什么 `Proxy` 不能映到 `include_package`（本批最危险的那条）
///
/// 两个字段看着像一个轴的两个方向，其实不是：`exclude_package` 是**减法**（默认全进、点名的出去），
/// `include_package` 是**换模式**（一旦非空就变白名单、没点名的全部出去）。而 `default_app_rules()`
/// 把 16 条内置预设**全部默认置为 proxy** —— 「proxy → include_package」会让一份用户零设置的默认
/// 配置在 Android 上把除这 16 个包以外的全设备流量踢出 VPN，明文直出，且隧道看起来一切正常。
/// 这就是「本该走代理的应用直连出网」的最大化形态。
///
/// # 为什么 `Direct` 走 tun 层而不是 route 规则
///
/// [选 A：`exclude_package`] 由 `VpnService.Builder.addDisallowedApplication` 在**系统边界**兑现，
/// 不需要逐连接的 uid→package 反查，不会因为查不到属主而静默漏判；且与本文件顶上「Android 不生成
/// mixed inbound」是同一条防线的两半 —— 排出去的应用再也没有本地代理口可以绕回来。
///
/// [不选 B：`package_name` route 规则 → direct 出站] 语义与桌面 1:1，但流量仍在隧道内、仍依赖
/// 进程属主查询。proxy/block 两档走的正是这条腿（`builder::route` 的 `app_owner_leg` Android 臂，
/// 2026-09-25 A5 接通）；direct 档在那边**刻意不发**，只由本函数兑现，免得同一档有两份真值。
///
/// # 射程自曝
///
/// - 预设 `package_names` 为空 ⇒ 该条不贡献任何包名，退回 geosite/geoip 腿（仍是直连，方向不反）。
/// - 自定义预设恒不贡献（理由见 `app_rules_preset::get_app_preset`）。
/// - 门（`app_routing_enabled` + smart 模式）复用 [`effective_app_rules_proxy`]，与桌面同一处判定。
fn android_exclude_packages(config: &UserConfig) -> Vec<String> {
    let pkgs: Vec<String> = effective_app_rules_proxy(config)
        .iter()
        .filter(|r| r.enabled && r.action == RuleAction::Direct)
        .filter_map(|r| {
            crate::user_config::app_rules_preset::get_app_preset(
                &r.app_id,
                &config.custom_app_presets,
            )
        })
        .flat_map(|p| p.package_names)
        .collect();
    dedupe(pkgs)
}

/// 前缀的**基地址**是不是回环地址。
///
/// 判据逐字对位 AOSP `VpnService.Builder.check(InetAddress)` 的第一条：它取的是
/// `prefix.getAddress().isLoopbackAddress()`（v4 首字节 127 / v6 `::1`），**不看掩码长度** ——
/// `127.9.0.0/16` 与 `127.0.0.0/8` 一样被拒。`IpAddr::is_loopback()` 是同一个判据的 Rust 侧。
///
/// 解析不出来的串返回 `false`：走到这里的条目已经过 `normalize_tun_exclude_cidr` 的严格校验
/// （`is_valid_ip_cidr`），解析失败在这里意味着上游校验漏了，处置是**交给承载侧那道**而不是
/// 在这里静默吞掉一条用户声明的段。
fn is_loopback_prefix(cidr: &str) -> bool {
    let addr = cidr.split('/').next().unwrap_or(cidr);
    addr.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// 移动端上把回环前缀从用户声明的排除段里剔掉，返回被剔的那些（供调用方 warn）。
///
/// 桌面三端**一条都不动**（原地返回空表）：那边的回环排除是正确且必要的，
/// win32/darwin 的平台基线本来就含 `127.0.0.0/8` 与 `::1/128`。
///
/// # 为什么 iOS 与 Android 同答（依据的**两半强度不同**，逐条说清）
///
/// - **Android**：有实测 —— `VpnService.Builder.excludeRoute` 对回环前缀抛 `Bad address`，
///   一条被拒整个 `establish()` 失败（2026-09-04 模拟器，logcat 原文见本文件 android 基线那段）。
/// - **iOS**：`NEPacketTunnelNetworkSettings` 收不收回环前缀**本仓没验证过**（构不出 iOS 产物），
///   故不拿 Android 那条当依据。承重的是**另一半、且它不依赖那次实测**：回环流量根本不进隧道的
///   路由表（内核在 lo 上直接闭环），排除它**买不到任何东西**，而代价上限是「隧道一次都建不起来」。
///   收益为零、代价上限为阻断级 ⇒ 不发。这与本文件 ios 平台基线那一臂是**同一条**已采纳的推理，
///   不是在这里新发明一条规则。
fn drop_mobile_loopback_excludes(platform: &str, extra: &mut Vec<String>) -> Vec<String> {
    if platform != "android" && platform != "ios" {
        return vec![];
    }
    let dropped: Vec<String> = extra
        .iter()
        .filter(|c| is_loopback_prefix(c))
        .cloned()
        .collect();
    extra.retain(|c| !is_loopback_prefix(c));
    dropped
}

/// 移动端（android / ios）「绕过局域网」进 `tun.route_exclude_address` 的那一份。
///
/// 调用方只在开关开着时调用；清单取 [`effective_bypass_lan`]（缺省即 `DEFAULT_BYPASS_LAN`）的网段子集。
/// 依次过四步，每一步都有一条不做就出事的理由：
///
/// 1. **严格校验**（[`is_valid_ip_cidr`]）+ **拒 `/0`**：`bypass_lan_cidrs` 只做形状粗判，
///    `256.1.1.1/24` 会放行进内核 ⇒ sing-box `netip.ParsePrefix` 启动 FATAL；`0.0.0.0/0` / `::/0`
///    放行 ⇒ 整个地址空间排出 VPN、流量全部绕过隧道而界面仍显示已连接。剔掉的条目 warn。
///    **不用** [`normalize_tun_exclude_cidr`](crate::builder::tun_route_exclude::normalize_tun_exclude_cidr)：它的过宽下限（v4 < /8）会把默认清单里的
///    `224.0.0.0/4`（组播，mDNS/SSDP 局域网发现靠它）与 `240.0.0.0/4` 每次起核都剔掉并告警。
/// 2. **FakeIP 段整条剔 + 生效组网段挖掉**：复用
///    [`compute_win_bypass_exclude`](crate::builder::tun_route_exclude::compute_win_bypass_exclude) 的算术（算法本身与
///    平台无关，名字里的 win 是它最初的调用方）。不挖的话默认清单里的 `100.64.0.0/10` / `fc00::/7`
///    会把 tailnet 段排出隧道，组网节点静默不可达。
/// 3. **挖掉 TUN 自身网段**：默认 `172.19.0.1/16` 落在 `172.16.0.0/12` 内、`fdfe:dcba:9876::1/126`
///    落在 `fc00::/7` 内，而隧道 DNS 地址（sing-tun `DNSServerAddress` = 地址 +1）就在这里。
///    API < 33 走 `BuildAutoRouteRanges` 预减，全量路由减掉 `172.16.0.0/12` 之后没有任何一条
///    路由覆盖隧道 DNS 地址〔推断：是否另有接口直连路由兜住未实测〕⇒ 挖掉最稳。
/// 4. **剔回环前缀**（[`drop_mobile_loopback_excludes`]）：默认清单含 `127.0.0.0/8`，
///    `VpnService.Builder.excludeRoute` 拒收回环（依据见 android 基线那段）。回环本就不进 VPN
///    路由表，剔掉语义无损 ⇒ 记 debug 不记 warn（默认清单每次起核都会命中，warn 只是噪音）。
fn mobile_bypass_lan_exclude(
    config: &UserConfig,
    deps: &InboundsDeps,
    engaged_mesh: &[String],
    fakeip_ranges: &[String],
    tun_address: &[String],
) -> Vec<String> {
    let raw = bypass_lan_cidrs(&effective_bypass_lan(&UConfigBypass(config)));
    let mut dropped_invalid = 0usize;
    let normalized: Vec<String> = raw
        .iter()
        .filter_map(|c| {
            let catch_all = c.split('/').nth(1).map(str::trim) == Some("0");
            if is_valid_ip_cidr(c) && !catch_all {
                Some(c.clone())
            } else {
                dropped_invalid += 1;
                None
            }
        })
        .collect();
    if dropped_invalid > 0 {
        (deps.log)(
            LogLevel::Warn,
            &format!("「绕过局域网」剔除 {dropped_invalid} 条非法网段（须合法 CIDR，且不能是 0.0.0.0/0 · ::/0 这种全量段）。"),
        );
    }
    let carved = crate::builder::tun_route_exclude::compute_win_bypass_exclude(
        &crate::builder::tun_route_exclude::WinBypassExcludeInput {
            bypass_cidrs: &dedupe(normalized),
            engaged_mesh_cidrs: engaged_mesh,
            own_lan_cidrs: &deps.own_lan_cidrs,
            fakeip_ranges,
        },
    )
    .exclude;
    let own_tun = crate::builder::tun_route_exclude::carve_candidates_within(&carved, tun_address);
    let mut exclude = crate::builder::tun_route_exclude::carve_out(carved, &own_tun);
    let dropped_loopback = drop_mobile_loopback_excludes(&deps.platform, &mut exclude);
    if !dropped_loopback.is_empty() {
        (deps.log)(
            LogLevel::Debug,
            &format!(
                "移动端：「绕过局域网」清单里的回环前缀不进排除表（系统 VPN 接口拒收，回环本就不进 VPN 路由表）：{}",
                dropped_loopback.join(", ")
            ),
        );
    }
    exclude
}

/// usesFakeIp：enableFakeIp 缺省 true。上游 `custom-rule-files.usesFakeIp`。
fn uses_fake_ip(config: &UserConfig) -> bool {
    config
        .dns_config
        .as_ref()
        .and_then(|d| d.enable_fake_ip)
        .unwrap_or(true)
}

/// UserConfig → effectiveCustomRules（smart gate）。复用 helpers。
fn effective_custom_rules_proxy(config: &UserConfig) -> Vec<crate::user_config::rule::Rule> {
    let mode = match config.proxy_mode {
        crate::user_config::ProxyMode::Smart => "smart",
        crate::user_config::ProxyMode::Global => "global",
        crate::user_config::ProxyMode::Direct => "direct",
    };
    effective_custom_rules(mode, &config.custom_rules)
}

/// UserConfig → effectiveAppRules（smart + appRoutingEnabled gate）。
fn effective_app_rules_proxy(config: &UserConfig) -> Vec<crate::user_config::rule::AppRule> {
    let mode = match config.proxy_mode {
        crate::user_config::ProxyMode::Smart => "smart",
        crate::user_config::ProxyMode::Global => "global",
        crate::user_config::ProxyMode::Direct => "direct",
    };
    effective_app_rules(
        config.app_routing_enabled != Some(false),
        mode,
        &config.app_rules,
    )
}

/// BypassConfig 适配器（UserConfig → effective_bypass_lan）。
struct UConfigBypass<'a>(&'a UserConfig);
impl<'a> BypassConfig for UConfigBypass<'a> {
    fn bypass_lan(&self) -> Option<bool> {
        self.0.bypass_lan
    }
    fn bypass_lan_list(&self) -> Option<&[String]> {
        self.0.bypass_lan_list.as_deref()
    }
}

/// PortConfig 适配器。
impl PortConfig for UserConfig {
    fn mixed_port(&self) -> Option<u16> {
        self.mixed_port
    }
    fn http_port(&self) -> Option<u16> {
        self.http_port
    }
    fn control_port(&self) -> Option<u16> {
        None
    }
}

#[cfg(test)]
mod tests;
