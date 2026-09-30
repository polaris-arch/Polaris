//! 代理模式（上游 `shared/types.ts:115-116`）。

#![forbid(unsafe_code)]

use polaris_helper_proto::Platform;
use serde::{Deserialize, Serialize};

/// 分流模式：global=真全局 / smart=智能分流（含自定义规则）/ direct=全直连。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    Global,
    #[default]
    Smart,
    Direct,
}

impl ProxyMode {
    /// 小写字符串表示（对齐 Polaris proxyMode JSON 值）。configGenerationNorm 等用。
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyMode::Global => "global",
            ProxyMode::Smart => "smart",
            ProxyMode::Direct => "direct",
        }
    }
}

/// 代理类型：systemProxy=系统代理 / tun=TUN 接管 / manual=手动（不接管）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProxyModeType {
    #[default]
    SystemProxy,
    Tun,
    Manual,
}

impl ProxyModeType {
    /// 是否 TUN 模式（buildLogConfig output 写文件谓词）。
    ///
    /// ⚠️ **配置生成侧不要直接读这个**，先过 [`ProxyModeType::effective_on`]：磁盘上存的值不一定是
    /// 当前平台能兑现的那个（成因见该函数）。直接读裸值的地方必须能自证「本平台不受影响」。
    pub fn is_tun(self) -> bool {
        matches!(self, ProxyModeType::Tun)
    }

    /// 该平台上**实际生效**的接管方式 —— 存量配置里的值不一定是这台设备能兑现的那个。
    ///
    /// # 为什么需要这个函数（Android 上「接管方式」这个选择根本不存在）
    ///
    /// 桌面三平台上「系统代理 / TUN / 手动」是三种真实存在的接管形态，各有承载物（系统代理设置项、
    /// 内核 TUN 网卡、什么都不做）。**Android 上只有一种**：应用能拿到的入网口只有
    /// `VpnService` 给的那一个 tun fd，没有第二条路 ——
    ///
    /// - 「系统代理」在 Android 上没有承载物：非 root 应用无权改全局 HTTP 代理设置；
    /// - 「手动」意味着用户自己去把流量指到本地代理口，而本仓在 Android 上**根本不发** mixed
    ///   入站（理由见 [`crate::builder::inbounds::build_inbounds`] 顶上那段：共享回环 + 零认证），
    ///   于是「手动」在这里连一个可指的端口都没有。
    ///
    /// # 不这么做会怎样（这不是洁癖，是阻断级缺陷）
    ///
    /// [`crate::user_config::app_config::UserConfig`] 的 `proxy_mode_type` 缺省值是
    /// `SystemProxy`，**全新安装的 Android 客户端拿到的就是它**（老用户、备份恢复、手改过的
    /// config 同理）。若生成侧照着这个值分流：
    ///
    /// 1. `build_inbounds` 只在 TUN 档发 tun 入站，Android 又不发 mixed ⇒ **一个用户流量入站都没有**。
    ///    隧道建得起来、核跑得起来、界面显示已连接，流量进去没有出口，且**不报错**。
    /// 2. 就算补上了入站，`route.auto_detect_interface` 仍只在 TUN 档下发，而那个键正是 Android 上
    ///    把 `VpnService.protect()` 接到核出站 socket 上的**唯一**开关（链路见
    ///    [`crate::builder::route::build_route_config_with_report`] 里那段）—— 缺席 ⇒ 核自己的
    ///    出站命中我们刚装上的默认路由 ⇒ 回灌 TUN 死循环。
    ///
    /// 两条都是「跑得起来、看着正常、就是不通」的静默形态。
    ///
    /// # 为什么判据落在这里、而不是在 UI 或者存盘时改写
    ///
    /// UI 那一侧治不了**已经存下来的**配置（老用户/备份恢复/手工改过 config），而承重面是配置生成：
    /// 只要生成侧照着 `systemProxy` 分流，配置就是坏的，UI 显示成什么样都不改变这一点。
    /// 存盘时改写则是把平台事实写进用户数据 —— 同一份配置换台机器就错了。
    ///
    /// # 为什么收在 `Platform` 枚举轴上
    ///
    /// 平台判据写成穷举 `match`，新增平台变体时编译器强制每个调用面重新答题；写成
    /// `platform == "android"` 字符串比较则对编译器完全透明（成因见
    /// `src-tauri/tests/platform_dispatch_exhaustive.rs` 的两张登记表）。字符串轴的调用方
    /// （`deps.platform` 是 `process.platform` 风格串）经 [`Platform::parse`] 这座**唯一的桥**过来。
    ///
    /// # 射程自曝：调用面是全集，唯一的例外是一个惰性实参
    ///
    /// `crates/config-engine` 里按 `proxy_mode_type` 分流的判据共 6 处，**全部**读本函数的生效值：
    ///
    /// | 站点 | 判的是什么 | Android 照裸值分流会怎样 |
    /// |---|---|---|
    /// | `builder::inbounds` | 发不发 tun 入站 | 一个用户流量入站都没有（隧道空转，不报错） |
    /// | `builder::route` | `auto_detect_interface` | 核出站拿不到 `VpnService.protect()` ⇒ 回灌 TUN 死循环 |
    /// | `builder::log` | `log.output` 落不落盘 | 一条核日志都不落盘（导出诊断是空的） |
    /// | `builder::dns` INV-1 | 节点解析器 `system` 档 | 退回 `dns-local`，防自递归那条腿关掉 |
    /// | `builder::dns` `win_loop_risk` | Win DNS 死环防护 | 今天无差异（合取项写死 win32） |
    /// | `builder::generate` | `system_interface_available` | 今天无差异（合取项是 mesh 平台允许清单） |
    ///
    /// 后两格今天接不接入结果逐字相同，**仍然接**：留例外就要靠人逐处复核「这个例外今天还成立
    /// 吗」，而 `mesh_system_supported_on_platform` 是个会变的函数，不是写死在同一行里的字面量。
    ///
    /// # iOS（2026-09-06 加变体时答的题）
    ///
    /// 答案同 Android（恒 `Tun`），**理由不同**，逐条见函数体里 `Platform::Ios` 臂上的注释。
    /// 上表六格对 iOS 逐格同样成立：核在 NE 扩展进程内、tun fd 由 `NEPacketTunnelProvider`
    /// 授予，「系统代理」与「手动」两档在 iOS 上同样没有承载物。
    ///
    /// **未验证**：上表第二行 `auto_detect_interface` 在 Android 上的机制是接 `VpnService
    /// .protect()`；iOS 上 NE 的出站 socket 由系统按扩展身份自动排除出隧道（问题被平台消解，
    /// 不需要 protect 等价物），故那一格「不判 Tun 会回灌死循环」的**具体失败形态**在 iOS 上
    /// 尚未真机取证。判 `Tun` 在 iOS 上的依据是入站那一格（第一行），它与平台机制无关。
    ///
    /// **唯一不走本函数的**是 `builder::outbounds::node_resolver_dial_tag` 传给
    /// `get_node_resolver_tag` 的那个 `proxy_mode_type` 串 —— 它不是判据而是**惰性实参**：
    /// 那个函数只在 `ctx == Rule` 时读它，Dial 侧传什么都不改变返回值（Rule 侧那份在
    /// `builder::dns`，已在上表第四行）。为它把平台参数一路穿进签名，是为零差异付改动半径。
    #[must_use]
    pub fn effective_on(self, platform: Platform) -> Self {
        match platform {
            Platform::Android => Self::Tun,
            // iOS 与 Android **答案相同、理由不同**，故写成独立臂而不是 `Android | Ios`：
            // 合并臂断言的是「同一个理由」，下一个人改 Android 那条时会连带把 iOS 改错。
            //
            // · 「系统代理」：iOS 上第三方应用没有任何 API 能改系统级 HTTP 代理（那是 MDM
            //   托管配置描述文件的领域）。NE 侧 `NEProxySettings` 只在自己这条隧道的作用域内
            //   生效，是隧道的一部分而不是「接管方式」的第三种承载物。⇒ 与 Android 同样无承载物。
            // · 「手动」：Android 的理由是**本仓不发** mixed 入站（`build_inbounds` 顶注：共享
            //   回环 + 零认证），于是连一个可指的端口都没有；iOS 的理由更靠前一步 —— **就算发了
            //   也没人能指过来**：iOS 没有让用户把任意第三方应用的出流量指到 127.0.0.1:P 的
            //   设置面（Wi-Fi 的 HTTP 代理项只作用于该 Wi-Fi 网络下的部分系统栈流量，蜂窝下
            //   整个不存在，且 App 各自的 URLSession 不必遵守）。两条理由的**依据不同**：
            //   Android 那条随「本仓发不发 mixed」这个我们自己的决定变，iOS 这条不随我们变。
            //
            // ⚠️ 本臂与 `builder::inbounds` 的 mixed 入站判据是**同一批必须一起改的**：若这里判
            // Tun 而 inbounds 仍按 `platform != "android"` 发 mixed，iOS 会同时拥有 tun 入站
            // **和**一个零认证共享回环 —— 那是桌面与 Android 都没有的第三种状态。
            Platform::Ios => Self::Tun,
            Platform::Mac | Platform::Win | Platform::Linux | Platform::Other => self,
        }
    }
}

#[cfg(test)]
mod tests;
