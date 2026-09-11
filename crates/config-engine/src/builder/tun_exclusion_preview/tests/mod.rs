use super::*;
use crate::user_config::app_config::UserConfig;
use crate::user_config::proxy_mode::ProxyModeType;
use crate::user_config::server_config::{Protocol, ServerConfig, TailscaleSettings};
use crate::user_config::tun_config::TunModeConfig;

fn tun_config_with(inbound_exclude: Vec<String>) -> UserConfig {
    UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        tun_config: Some(TunModeConfig {
            inbound_exclude_cidrs: Some(inbound_exclude),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// 同上，但带一个 engaged 的 Tailscale 节点。
///
/// **观测面只有在有 TS 节点时才是活输入**：`engaged_mesh` 按 `serverId` 取观测地址，没有节点
/// 就没有 key 能对上 ⇒ 传空与传真值同值，任何拿它当判据的断言都是恒真的（无信息量）。
/// 本文件里凡要证明「观测面确实被消费」的用例，都必须用这份配置。
fn tun_config_with_ts_node(inbound_exclude: Vec<String>) -> UserConfig {
    UserConfig {
        servers: vec![ServerConfig {
            id: "ts1".into(),
            name: "ts1".into(),
            protocol: Protocol::Tailscale,
            // `alwaysRouteSubnets` 缺省 true ⇒ 恒 engaged，不依赖选中/规则点名。
            tailscale_settings: Some(Box::new(TailscaleSettings::default())),
            ..Default::default()
        }],
        selected_server_id: Some("__direct__".into()),
        ..tun_config_with(inbound_exclude)
    }
}

/// 「本轮没有任何运行期观测」——合法值，不是占位。
fn no_observation() -> ObservedTailnetAddresses {
    ObservedTailnetAddresses::new()
}

/// 2026-09-11 真控制面实测：自建 headscale 的 `prefixes.v4` 非默认，self 拿到 `32.0.0.28`
/// （`32.0.0.0/8` 是 IANA 分给 AT&T 的公网段，刻意不编一个假地址）。
fn observed_ts1() -> ObservedTailnetAddresses {
    ObservedTailnetAddresses::from([("ts1".to_string(), vec!["32.0.0.28".to_string()])])
}

thread_local! {
    /// **真实生成**那一侧的诊断缓冲。与预览用的 `CAPTURED` 分开：同一个缓冲会让两次调用
    /// 互相污染，而本门恰恰要逐条比对两侧的 notes。
    static REAL_NOTES: RefCell<Vec<PreviewNote>> = const { RefCell::new(Vec::new()) };
}

fn capture_real(level: LogLevel, message: &str) {
    REAL_NOTES.with(|c| {
        c.borrow_mut().push(PreviewNote {
            level: format!("{level:?}").to_lowercase(),
            message: message.to_owned(),
        });
    });
}

/// 跑一次**真实** `build_inbounds`，读回内核那份 `route_exclude_address` + 同时捕获它的诊断。
fn real_build(
    config: &UserConfig,
    platform: &str,
    own_lan: Vec<String>,
    observed: ObservedTailnetAddresses,
) -> (Vec<String>, Vec<PreviewNote>) {
    REAL_NOTES.with(|c| c.borrow_mut().clear());
    let inbounds = build_inbounds(
        config,
        None,
        &InboundsDeps {
            probe_direct_port: None,
            probe_proxy_port: None,
            update_in_port: None,
            subscription_update_in_port: None,
            probe_pool_ports: vec![],
            platform: platform.to_string(),
            own_lan_cidrs: own_lan,
            observed_tailnet_addresses: observed,
            log: capture_real,
        },
    );
    let exclude = inbounds
        .iter()
        .find(|i| i.type_field == "tun")
        .and_then(|t| t.route_exclude_address.clone())
        .unwrap_or_default();
    (
        exclude,
        REAL_NOTES.with(|c| std::mem::take(&mut *c.borrow_mut())),
    )
}

/// **本模块唯一的核心不变量**：预览结果必须与真实生成**逐条相等**。
///
/// 它锁的是"预览是读回来的、不是另算的"。今天 `preview_tun_exclusion` 给端口类入参传中性值，
/// 依据是"排除面不看端口"—— 那是**当下**的事实，不是永恒的。哪天有人让排除依赖某个端口，
/// 本门当场红，而不是让预览悄悄开始骗人（那比没有预览更坏：用户会拿它当真值去排查）。
#[test]
fn preview_matches_real_build() {
    let cases: Vec<(&str, UserConfig)> = vec![
        ("空表", tun_config_with(vec![])),
        (
            "自建 tailnet 两族",
            tun_config_with(vec!["32.0.0.0/24".into(), "fd7a:115c:a1e0::/48".into()]),
        ),
        (
            "含非法段",
            tun_config_with(vec!["not-a-cidr".into(), "10.0.0.0/8".into()]),
        ),
        ("过宽段", tun_config_with(vec!["0.0.0.0/0".into()])),
        // 带 TS 节点的两份：观测面在这里才是活输入（见 `tun_config_with_ts_node`）。
        (
            "TS 节点 + 自建 tailnet 段",
            tun_config_with_ts_node(vec!["32.0.0.0/24".into(), "fd7a:115c:a1e0::/48".into()]),
        ),
        (
            "TS 节点 + 与观测无关的段",
            tun_config_with_ts_node(vec!["10.0.0.0/8".into()]),
        ),
    ];
    let own_lan = vec!["192.168.1.23/24".to_string()];
    // **观测面必须成为一个维度**：此前两侧都硬传空，这道门证明的只是「都传空的时候相等」——
    // 而生产里 observed 已经非空（A-0b 接线），门还在、牙没了。
    let observations: Vec<(&str, ObservedTailnetAddresses)> =
        vec![("无观测", no_observation()), ("有观测", observed_ts1())];

    for platform in ["win32", "darwin", "linux"] {
        for (label, config) in &cases {
            for (obs_label, observed) in &observations {
                let (real_exclude, real_notes) =
                    real_build(config, platform, own_lan.clone(), observed.clone());
                let preview =
                    preview_tun_exclusion(config, platform, own_lan.clone(), observed.clone());
                assert_eq!(
                    preview.effective, real_exclude,
                    "{platform}/{label}/{obs_label}：预览与真实生成分叉 —— \
                     预览必须是读回来的，不是另算一份"
                );
                // notes 也必须同源：观测地址进来后会触发新的静默剔除（组网段重叠那支），
                // 预览若捕不到同样的诊断，用户看到的「为什么没生效」就与实际原因不符。
                assert_eq!(
                    preview.notes, real_notes,
                    "{platform}/{label}/{obs_label}：诊断行分叉 —— \
                     用户读到的剔除原因与内核那侧实际发生的不是同一批"
                );
            }
        }
    }
}

/// 🔴 **上面那道门的反向对照**：只喂一侧观测，两边必须分叉。
///
/// 没有这条，`preview_matches_real_build` 无法自证它真的把观测面当成了输入 —— 一个两侧都
/// 忽略该参数的实现同样能让它全绿（那正是本次修复之前的状态：两边硬传空，门绿、牙无）。
///
/// 取材点选 darwin + engaged TS 节点 + 用户声明 `32.0.0.0/24`：观测到的 `32.0.0.28` 进
/// `engaged_mesh` 后与该段相交 ⇒ 真实生成把它整条剔除；预览若看不见观测面就会**留着**它，
/// 于是界面显示一条其实没生效的排除 —— 正是本模块要终结的那类谎。
#[test]
fn preview_diverges_when_only_one_side_sees_the_observation() {
    let config = tun_config_with_ts_node(vec!["32.0.0.0/24".into()]);
    let own_lan: Vec<String> = vec![];

    let (real_with_obs, _) = real_build(&config, "darwin", own_lan.clone(), observed_ts1());
    let preview_blind = preview_tun_exclusion(&config, "darwin", own_lan.clone(), no_observation());

    // 先钉住差异的**具体形态**（否则「不相等」可能来自任何无关原因，对照就没有指向）。
    assert!(
        !real_with_obs.iter().any(|c| c == "32.0.0.0/24"),
        "前提没建立：观测到 32.0.0.28 之后，真实生成本应把相交的 32.0.0.0/24 整条剔除。实际：{real_with_obs:?}"
    );
    assert!(
        preview_blind.effective.iter().any(|c| c == "32.0.0.0/24"),
        "前提没建立：看不见观测面的预览本应留着 32.0.0.0/24。实际：{:?}",
        preview_blind.effective
    );
    assert_ne!(
        preview_blind.effective, real_with_obs,
        "只喂真实生成一侧观测，两边却仍然相等 —— 说明观测面根本没被消费，\
         `preview_matches_real_build` 的绿没有信息量"
    );

    // 反过来也一样（只喂预览一侧）。
    let (real_blind, _) = real_build(&config, "darwin", own_lan.clone(), no_observation());
    let preview_with_obs = preview_tun_exclusion(&config, "darwin", own_lan, observed_ts1());
    assert_ne!(
        preview_with_obs.effective, real_blind,
        "只喂预览一侧观测，两边却仍然相等 —— 同上，参数是死输入"
    );

    // 而喂同一份观测时必须相等（这条与主门同义，放在这里是为了让本用例自带正向锚点：
    // 「不等」不是因为这对输入天生对不上，只是因为两侧看到的观测面不同）。
    let (real_same, _) = real_build(&config, "darwin", vec![], observed_ts1());
    assert_eq!(
        preview_with_obs.effective, real_same,
        "同一份观测下预览与真实生成仍不等 —— 那是另一个缺陷，本对照的前提被打破"
    );
}

/// 事故复现：macOS + 自建 tailnet 前缀。**填对表** ⇒ 两条都进生效值。
#[test]
fn macos_inbound_exclude_reaches_effective() {
    let config = tun_config_with(vec!["32.0.0.0/24".into(), "fd7a:115c:a1e0::/48".into()]);
    let preview = preview_tun_exclusion(
        &config,
        "darwin",
        vec!["192.168.1.23/24".into()],
        no_observation(),
    );
    assert!(preview.tun_active);
    for cidr in ["32.0.0.0/24", "fd7a:115c:a1e0::/48"] {
        assert!(
            preview.effective.iter().any(|c| c == cidr),
            "{cidr} 没进生效值：{:?}",
            preview.effective
        );
    }
}

/// 同一次事故的**另一半**：把网段填进 `bypassLANList`（外部排查建议给的那张表）
/// 在 macOS 上**一条都不会生效**。
///
/// 这条是本模块的存在理由，必须有正向断言钉住，不能只靠"mac 分支只放回环"的代码阅读。
#[test]
fn macos_bypass_lan_list_never_reaches_tun() {
    let mut config = UserConfig {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    config.bypass_lan_list = Some(vec!["32.0.0.0/24".into(), "fd7a:115c:a1e0::/48".into()]);
    let preview = preview_tun_exclusion(&config, "darwin", vec![], no_observation());
    for cidr in ["32.0.0.0/24", "fd7a:115c:a1e0::/48"] {
        assert!(
            !preview.effective.iter().any(|c| c == cidr),
            "macOS 上 bypassLANList 居然进了 TUN 排除 —— 若这是有意改动，\
             `§1.4` 的结论与给用户的操作指引都要跟着改"
        );
    }

    // 正向对照：同一份清单在 Windows 上**确实**进 TUN，证明上面的"没进"不是因为整条链路失效。
    let win = preview_tun_exclusion(&config, "win32", vec![], no_observation());
    assert!(
        win.effective.iter().any(|c| c == "32.0.0.0/24"),
        "Windows 上 bypassLANList 也没进 TUN —— 那说明本用例的取材面塌了，上面的阴性断言无意义"
    );
}

/// 静默剔除必须在 `notes` 里留下线索（用户填了没生效时，原因只能从这里看到）。
#[test]
fn silent_drops_surface_as_notes() {
    // 非法/过宽：两条都该被剔并 warn。
    let config = tun_config_with(vec!["not-a-cidr".into(), "0.0.0.0/0".into()]);
    let preview = preview_tun_exclusion(&config, "darwin", vec![], no_observation());
    assert!(
        preview.notes.iter().any(|n| n.level == "warn"),
        "非法段被静默剔除且无诊断 —— 用户看到的是「填了没反应」"
    );

    // Linux 恒忽略本表，同样必须出声。
    let linux = preview_tun_exclusion(
        &tun_config_with(vec!["10.0.0.0/8".into()]),
        "linux",
        vec![],
        no_observation(),
    );
    assert!(linux.effective.is_empty(), "Linux 的 route_exclude 恒空");
    assert!(
        linux.notes.iter().any(|n| n.message.contains("Linux")),
        "Linux 忽略本表却没有任何诊断：{:?}",
        linux.notes
    );
}

/// 非 TUN 模式：`tun_active` 为假，`effective` 空。UI 据此显示"当前不是 TUN 模式"，
/// 而不是显示一个会被误读成"什么都没排除"的空列表。
#[test]
fn reports_when_tun_is_not_active() {
    let config = UserConfig {
        proxy_mode_type: ProxyModeType::SystemProxy,
        ..Default::default()
    };
    let preview = preview_tun_exclusion(&config, "darwin", vec![], no_observation());
    assert!(!preview.tun_active);
    assert!(preview.effective.is_empty());
}

/// 诊断缓冲不得跨调用泄漏（线程局部 + 进入时清空 + 返回时取走）。
#[test]
fn notes_do_not_leak_between_calls() {
    let dirty = preview_tun_exclusion(
        &tun_config_with(vec!["bad".into()]),
        "darwin",
        vec![],
        no_observation(),
    );
    assert!(!dirty.notes.is_empty(), "前置：这次调用应当产生诊断");
    let clean = preview_tun_exclusion(&tun_config_with(vec![]), "darwin", vec![], no_observation());
    assert!(
        clean.notes.is_empty(),
        "上一次调用的诊断泄漏到了这一次：{:?}",
        clean.notes
    );
}

/// 线格式必须是 camelCase —— 与 `ui/src/contracts/tun-exclusion-preview.ts` 对位。
///
/// 这条不是形式主义：漏了 `rename_all` 只会让前端读到 `undefined`，而 `undefined` 是假值，
/// UI 会把"TUN 正在跑"渲染成"当前不是 TUN 模式"。tsc 查不出（类型说有、运行期没有），
/// 两侧单测也各自绿。只有把键名当断言写下来才拦得住。
#[test]
fn wire_shape_is_camel_case() {
    let preview =
        preview_tun_exclusion(&tun_config_with(vec![]), "darwin", vec![], no_observation());
    let json = serde_json::to_value(&preview).expect("序列化");
    let obj = json.as_object().expect("应是对象");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["effective", "notes", "tunActive"],
        "线格式键名漂了 —— 前端按这三个键读"
    );
}
