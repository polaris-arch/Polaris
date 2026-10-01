//! sing-box inbound 类型（`singbox-config-types.ts:86-114 SingBoxInbound`）。

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// `inbounds[]`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inbound {
    #[serde(rename = "type")]
    pub type_field: String,
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    /// 监听的传输层（`"udp"` / `"tcp"`；缺席 = 两者都听）。只有网络场景 canary 探针入站发它：
    /// 那是纯 UDP 的 `direct` 入站，不该白占一个 TCP 口（见 `builder::network_env::apply_network_canaries`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    // TUN 模式
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_route: Option<bool>,
    /// auto_redirect（1.10+）：Linux nftables 改善 TUN 路由/性能。P6 LAN 网关按 MAC 过滤时必发。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_redirect: Option<bool>,
    /// 严格路由（防绕行泄漏）。
    ///
    /// **Android 上恒 `None`**：该字段在 libbox + `VpnService` tun fd 这条路上一行代码都不会跑
    /// （唯一落点在 sing-tun 的 `NativeTun::rules()`，只由 `NativeTun::Start()` 调用，而 tun fd
    /// 不走 `Start()`；模拟器实测起隧道前后 `iptables -S` 逐字节相同）。判据与「为什么是不发、
    /// 而不是发 `false`」见 [`crate::builder::inbounds::build_inbounds`] 里该值的计算处。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_route: Option<bool>,
    // 🔴 刻意**没有** `stack` 字段：sing-box 1.15.0-alpha.3 起写了 `stack`（含 `"go"`）就报弃用，
    // 1.17 删除；缺席即走 sing-tun 新栈。删字段而不是留 `Option` 恒 `None`：本结构体只由
    // `builder::inbounds` 构造，没有任何读入外部 inbound 的路径依赖它，删掉后「重新发出 stack」
    // 连编译都过不了。生成期断言见 `builder::inbounds::tests::tun_inbound_never_emits_stack_on_any_platform`。
    /// udp_mapping / udp_filtering（1.14 新增；**只有 tun / tproxy 两个 inbound 变体带这组键** ——
    /// 随包 beta.7 `sing-box schema` 实测：`$defs/Inbound/oneOf[16].properties.type.const == "tun"`、
    /// `oneOf[13] == "tproxy"`，其余 20 个变体没有）。
    ///
    /// 两项**合起来**才是用户感知的「NAT 类型」：mapping 决定同一本地端口对不同目的地复用不复用同一个
    /// 映射，filtering 决定允许哪些远端的回包进来。档位映射表与判据在 `builder::inbounds::udp_nat_behaviors`。
    ///
    /// **缺席即最宽松**：上游两项默认都是 `endpoint_independent`（<https://sing-box.sagernet.org/configuration/shared/udp-nat/>），
    /// 即全锥。故 Polaris 只在用户显式选档时下发（入口见 `user_config::tun_config::UdpNatType`），
    /// 默认一个键都不发 —— 这既保住金样零 delta，也避免把「当前默认值」硬编码进配置、日后上游改默认时
    /// 我们还钉在旧值上（这里没有任何判据说全锥不该是默认）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udp_mapping: Option<UdpNatBehavior>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udp_filtering: Option<UdpNatBehavior>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_exclude_address: Option<Vec<String>>,
    /// include/exclude_mac_address（1.14；P6 LAN 网关，互斥，仅 Linux+auto_route+auto_redirect）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_mac_address: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_mac_address: Option<Vec<String>>,
    /// `include_package`（Android 专属；`option/tun.go:44`）——**本仓恒不发射**，字段只为把这条裁定
    /// 表达成可编译、可变异、可被门看见的东西，而不是留在注释里。
    ///
    /// 语义是**白名单**：一旦非空，只有列出的包进隧道，**其余全部应用当场脱离 VPN 直连出网**。
    /// 它与 [`Self::exclude_package`] 不是「同一个轴的两个方向」，把 `AppRule::action` 的 proxy 一档
    /// 映到这里是本批最危险的一种写法：`default_app_rules()` 把全部 16 条内置预设默认置为 proxy
    /// （见 `user_config::app_rules_preset::default_app_rules`），那样一份默认配置在 Android 上会
    /// 生成一个「只有这 16 个应用走 VPN」的白名单 —— 用户什么都没设置，全设备其余流量已经明文直出。
    /// 失效是静默的：隧道正常、图标正常、日志无异常。
    ///
    /// 要发射它，前提是 UI 上先存在一个显式的「仅代理选中应用」白名单档；那时它的输入是那个档，
    /// **不是** `AppRule::action`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_package: Option<Vec<String>>,
    /// `exclude_package`（Android 专属；`option/tun.go:45`）：列出的包不进隧道。
    ///
    /// 承载 `AppRule::action == Direct` 一档 —— 两者同向（都是「这个应用不经代理出网」），且
    /// 由 `VpnService.Builder.addDisallowedApplication` 在**系统边界**兑现，不依赖任何进程内匹配。
    /// 映射表与 proxy/block 两档为何不落在这个轴上，见 `builder::inbounds::android_exclude_packages`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_package: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<InboundPlatform>,
    /// `users`（http / socks / mixed 入站的认证表；`option/simple.go` 的 `Users []auth.User`）。
    ///
    /// **只在 Android 的回环探针/更新入站上发射**，值是每次起核随机生成的一次性凭据
    /// （判据与平台轴见 [`crate::builder::inbounds::loopback_inbounds_require_auth`]）。
    /// 桌面与 iOS 恒 `None` —— 缺席即内核不做认证，与改动前逐字节相同。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub users: Option<Vec<InboundUser>>,
}

/// sing-box 入站认证表的一项（`auth.User{Username, Password}`，http 走 Basic proxy auth，
/// socks 走 RFC 1929 用户名/密码子协商）。
///
/// **`Debug` 刻意手写并抹掉 `password`**：`Inbound` / `SingBoxConfig` / 起核依赖束都派生 `Debug`，
/// 任何一处 `{:?}` 进日志就会把这枚凭据原样打出去 —— 它的全部价值在于「只有本进程知道」。
/// 序列化（写给内核的配置）仍输出明文，那是内核认证所必需的唯一出口。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundUser {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for InboundUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundUser")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// sing-box UDP NAT 行为闭集（`udp_mapping` / `udp_filtering` 共用同一个类型 `option.UDPNATBehavior`，
/// 见随包 beta.7 二进制里的结构体 tag）。
///
/// 刻意用**枚举**而非裸 `String`，与 [`super::dns::DomainStrategy`] 同一条理由：值是内核 schema 的闭集，
/// 而 `sing-box check` 对写错的值只在 unmarshal 到该 inbound 时才报 `unknown UDP NAT behavior`，
/// 靠人眼与 check 都不是稳定拦截点；编译期才是。
///
/// **不含 schema 里的空串 `""` 变体**：那一档在内核语义上等价于「键缺席 → 用默认」，而本仓表达「缺席」
/// 一律用 `Option::None` + `skip_serializing_if`（`Inbound` 其余每一个可选字段都是这套）。同一语义留两条
/// 表达路径必然漂：`Some(Empty)` 与 `None` 序列化出**不同 JSON、相同行为**，对拍与 diff 就再也说不清
/// 一处差异是不是回归。故只保留三个有值的档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UdpNatBehavior {
    /// 同一本地端口对所有目的地复用同一映射 / 接受任意远端来的包。
    EndpointIndependent,
    /// 按目的**地址**分映射 / 只接受曾发过包的地址来的包。
    AddressDependent,
    /// 按目的**地址+端口**分映射 / 只接受曾发过包的地址+端口来的包（最严）。
    AddressAndPortDependent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundPlatform {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<HttpProxyPlatform>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpProxyPlatform {
    pub enabled: bool,
    pub server: String,
    pub server_port: u16,
}
