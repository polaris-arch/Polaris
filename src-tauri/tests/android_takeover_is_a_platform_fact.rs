//! **接管方式判据面**：`src-tauri` 里每一处按 `proxy_mode_type` 分流的判据都必须读
//! **本平台生效值**（[`ProxyModeType::effective_on`]），而不是磁盘上存的那个裸值。
//!
//! # 守的是什么（血证，不是假设）
//!
//! `UserConfig::proxy_mode_type` 的存盘缺省值是 `systemProxy`
//! （`config-engine` 的 `app_config::default_proxy_mode_type`），而 **Android 上「接管方式」
//! 这个选择根本不存在** —— 应用能拿到的入网口只有 `VpnService` 给的那一个 tun fd，没有第二条路
//! （成因全在 `ProxyModeType::effective_on` 的文档注释里）。于是全新安装 / 备份恢复 / 手改过
//! config 的 Android 客户端，在**每一处**照读裸值的判据上都走错分支。
//!
//! 2026-09-05 R1 在 runtime 侧逐处查清的四条（config-engine 侧那批已另行收口）：
//!
//! | 站点 | Android 照裸值分流会怎样 |
//! |---|---|
//! | `proxy/system_takeover.rs` 的 `should_enable_system_proxy` | 起核后去调 `enable_system_proxy`，而 `proxy_ops` 的 Android 臂显式返 `Err` ⇒ **用户当场看到一条「系统代理启用失败，流量未经代理」的假错误** |
//! | `proxy/connection_flush.rs` + `proxy/hot_switch.rs` 的 flush 守卫 | 起隧道后旧连接不被 RST（真实 IP 已泄漏且此后不自愈），且 `network_settle` 门少一段 |
//! | `proxy/system_takeover.rs` 的残留提示 | 第三方系统代理残留 advisory 在非 TUN 早退 |
//!
//! # 为什么门要长成「扫描 + 豁免登记」而不是「逐站点断言」
//!
//! config-engine 侧那道同源门（`tests/android_takeover_is_unconditional.rs`）能写成
//! 「Android 上整份产物在三种存盘档位下逐字节相同」——**它不枚举站点表**，于是日后新加一处读
//! 裸值的判据会自动红。runtime 侧**没有「整份产物」这种可比对象**：这里的产出是一串副作用
//! （去不去调系统代理、开不开那一枪 flush、挂不挂出口夺取闸），彼此不同类、多数要真核才观测得到。
//!
//! 故等价形态取在**源码面**：判据不是「这几处站点对不对」，而是
//! **「取材面里每一处按接管方式分流的判据，都在同一条语句里出现 `effective`」**。
//! 新加一处读裸值 ⇒ 不在豁免表里 ⇒ 自动红，与那道门的强制力同构。
//!
//! 正面断言（三档各自产出什么）由另一半承担：`runtime/proxy/tests/android_takeover.rs`
//! 在钉住平台的 `ProxyRuntime` 上跑生产路径。两条成对交 —— 缺了本门，那一半就是一张会腐烂的
//! 站点表；缺了那一半，本门只证明写法对了，证明不了产出的值能用。
//!
//! # 两个面，各自的针
//!
//! | 面 | 剥什么 | 针是什么 | 判据 |
//! |---|---|---|---|
//! | **① 符号面** | 注释 + 字符串字面量 | `.is_tun()` / `ProxyModeType::<变体>` / `ProxyModeType::is_tun` | 同一语句里必须有 `effective` |
//! | **② 字面量面** | 只剥注释 | `"proxyModeType"`（从原始 JSON 读接管方式的那条逃逸路） | 必须登记，写明为什么它不需要生效值 |
//!
//! 面①**只认判别形态**，不认「把 `proxy_mode_type` 当实参传下去」：那不是决定点，真正的决定
//! 发生在被调方的 `.is_tun()` 上，而被调方也在取材面里。这条边界让门不去逼人在传参处写噪音。
//!
//! # 不在本面之内的（显式声明边界，否则「扫到 0 处也叫全绿」）
//!
//! - `crates/**`：`config-engine` 侧由它自己那道整份产物门守；`config-engine::node_dns` 的 `plan_upstreams`
//!   与 `system-integration` 的 `should_reconcile_dns` 是**被喂**判据的纯函数（入参已由
//!   调用侧过完生效值），它们不知道也不该知道平台。
//! - `src-tauri/src/**/tests/`：测试里按档位分叉是**构造输入**，不是判据。
//! - `proxy/dns_takeover.rs` 的 `Some("tun")`：那是把已经过完生效值的 `is_tun: bool` 翻译成
//!   `should_reconcile_dns` 的字符串入参，是**构造**不是判别（它的 `is_tun` 由面①管着）。
//!
//! # 射程记账（本门够不着的，别高估它）
//!
//! 面①的判据是「语句里出现 `effective`」，而**函数名里的 `effective` 同样算数** ——
//! `running_effective_proxy_mode_type()` 这样的具名访问器就是靠这条过的。于是有一条明确的
//! 逃逸路：**把访问器的名字留着、把它体内的 `effective_on` 拆掉**，调用点那条语句一个字不变，
//! 本门全绿（2026-09-05 实测：M5 变异下本门 rc=0，红的是行为侧那条）。
//!
//! 不改成「只认 `effective_on(` 这个调用形态」，是因为那样会把具名访问器这条**正当**写法逼死：
//! 生效值本来就该由一个有名字的访问器收口，而不是让每个调用点各写一遍 `effective_on`。
//! 这个洞由**行为侧**那一半兜住（`runtime/proxy/tests/android_takeover.rs` 的
//! `android_running_effective_mode_is_tun_whatever_is_stored` 在 M5 下转红）——
//! 这也正是「两条成对交、只交一条算没做完」在本批的具体所指。
// 源码文本门：三平台源码逐字一致（`.gitattributes` eol=lf），只在必跑的 Linux 腿执行一次。
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use polaris_source_probe::{mask_comments, mask_comments_and_strings};

// ===================== 判据参数 =====================

/// 取材面里生产源文件数的下限（正面断言）。2026-09-05 实测 127 个。
///
/// 取 90 是给正常增删留余量，但**不给「取材面塌了」留余量**：路径写错 / 遍历坏掉表现为个位数。
const FILE_FLOOR: usize = 90;

/// 面①「按接管方式分流的判据」出现次数的下限（正面断言）。2026-09-05 实测 21 次。
///
/// 门的红绿因此不依赖「恰好没扫到违规」这种无信息量的绿：掩码把代码也剥了、正则失配、
/// 语句配平写反，一律表现为扫到的比下限少。
const DECISION_FLOOR: usize = 15;

/// 面②「从原始 JSON 读 `proxyModeType`」出现次数的下限（正面断言，同上）。2026-09-05 实测 5 次。
const RAW_JSON_FLOOR: usize = 4;

/// 面①的豁免登记表：(仓库相对路径, 折叠空白后的语句原文, 出现次数, **为什么这一处可以读裸值**)。
///
/// **当前为空**：2026-09-05 R1 把 `src-tauri` 里全部 21 处判据都接到了生效值上，一处例外都没留。
/// 留这张表不是为了将来好放行，而是为了让「确实需要裸值」的那一天有个**带理由**的落点，
/// 且过期条目会让门变红。
///
/// 想往里加一条之前先答这个问题：**这处判据在 Android 上走哪一支、那一支对不对**。
/// 答不出来就不是豁免，是缺陷。
type Exemption = (&'static str, &'static str, usize, &'static str);
const RAW_READ_EXEMPTIONS: &[Exemption] = &[];

/// 面②登记表：**从原始 JSON 读 `proxyModeType`** 的每一处。
///
/// 这些点绕过了 `UserConfig` 与 `ProxyModeType`，故面①的针够不着它们 —— 而「把接管方式当字符串
/// 从 config 里掏出来再比 `== "tun"`」正是一条完整的逃逸路。每条必须写明：**Android 上这一处
/// 得到什么、为什么那个答案可以**。
type RawJsonSite = (&'static str, &'static str, usize, &'static str);
const RAW_JSON_REGISTRY: &[RawJsonSite] = &[
    // R14 metadata transfer and error labels; no raw decision exemption.
    (
        "src-tauri/src/runtime/proxy/dns_takeover.rs",
        "for key in [ \"proxyModeType\", \"mixedPort\", \"httpPort\", \"bypassLAN\", \"bypassLANList\", ]",
        1,
        "merge_runtime_host_projection copies fields from an already accepted host projection into a private generation candidate. The key list makes no takeover decision and does not grant Android a host DNS/system-proxy adapter. Android effective TUN still comes from typed effective_on(platform); copying stored metadata cannot accept a native effect.",
    ),
    (
        "src-tauri/src/runtime/proxy/dns_takeover.rs",
        "return Err(Box::new(host_rejection( \"HOST_CARRIER_TRANSITION_REQUIRES_RESTART\", \"proxyModeType\", )))",
        1,
        "This literal labels the rejected input field after comparing both typed proxy modes through effective_on(platform). Android stored-mode changes therefore keep its effective TUN value; the string is an error location, never a raw-mode predicate or host adapter capability.",
    ),
    (
        "src-tauri/src/runtime/proxy/runtime_config/host_effects.rs",
        "HostEffectKind::SystemProxy => &[ \"proxyModeType\", \"mixedPort\", \"httpPort\", \"bypassLAN\", \"bypassLANList\", ], HostEffectKind::Dns => &[],",
        1,
        "merge_accepted_host_kind copies only the selected already-accepted host fields. This exhaustive SystemProxy/Dns metadata list neither reads a raw takeover predicate nor accepts Android native TUN. Android has no supported desktop host DNS/system-proxy adapter here; its effective mode and native retirement ledger remain separate.",
    ),
    (
        "src-tauri/src/native_verification/proxy_carrier.rs",
        "obj.insert(\"proxyModeType\".into(), serde_json::json!(\"manual\"))",
        1,
        "Closed desktop acceptance constructs private manual configuration, not stored Android takeover state",
    ),
    (
        "src-tauri/src/native_verification/proxy_carrier.rs",
        "\"servers\": [], \"selectedServerId\": \"__direct__\", \"proxyMode\": \"direct\", \"proxyModeType\": \"manual\", \"mixedPort\": mixed, \"clashApiSecret\": FIXTURE_MANAGEMENT_SECRET,",
        1,
        "Closed desktop acceptance owns its loopback-only runtime directory; the exact scope is checked separately",
    ),
    (
        "src-tauri/src/runtime/proxy/dns_takeover.rs",
        "let mode = config .get(\"proxyModeType\") .cloned() .and_then(|value| serde_json::from_value::<ProxyModeType>(value).ok()) .unwrap_or_default()",
        1,
        "Host DNS intent is read only on Mac/Linux, after the platform guard: Android/iOS/Windows/Other return false before this raw read because they do not own this host DNS effect. Mac/Linux deserialize the typed ProxyModeType, apply effective_on(platform).is_tun(), and combine it with the raw DNS switch's existing default-on semantics on both sides; this never selects Android takeover from its stored mode.",
    ),
    (
        "src-tauri/src/runtime/proxy/runtime_config.rs",
        "[ \"proxyModeType\", \"tunConfig\", \"mixedPort\", \"httpPort\", \"allowLan\", \"singboxDashboard\", ] .iter() .any(|key| old.get(*key) != new.get(*key)) || super::dns_takeover::host_dns_takeover_changed(platform, old, new)",
        1,
        "The array previews opaque D/R physical/service setting differences, not Android's effective takeover mode. The added host DNS projection returns false before raw reads on Android/iOS/Windows/Other; Mac/Linux normalize the typed mode with effective_on(platform).is_tun(). This preview is reachable on Android: Core changes still require the effective generated configuration's Plan, while host DNS changes require a separate explicit application-managed restart choice.",
    ),
    (
        "src-tauri/src/commands/config.rs",
        "next[\"proxyModeType\"] = json!(mode)",
        1,
        "config_takeover_edit 把已解析的 ProxyModeType 用户输入写入最新配置，不是从原始状态作平台判据；Android 运行态仍由 effective_on 强制 TUN。",
    ),
    (
        "src-tauri/src/app_tray.rs",
        "mode: config .get(\"proxyMode\") .and_then(serde_json::Value::as_str) .map(str::to_string), \
         mode_type: config .get(\"proxyModeType\") .and_then(serde_json::Value::as_str) \
         .map(str::to_string), selected_server_id, has_real_nodes: !nodes.is_empty(), node_groups,",
        1,
        "托盘菜单投影：`mode_type` 唯一消费点是 `build_tray_menu` 里那三个 `CheckMenuItem` 的选中\
         态（`*k == m.mode_type`），纯**呈现**，不驱动任何接管动作。整条链 `#[cfg(desktop)]`，\
         Android 不可达。",
    ),
    (
        "src-tauri/src/commands/misc/logs.rs",
        "proxy_mode: config .get(\"proxyMode\") .and_then(Value::as_str) .unwrap_or(\"\") \
         .to_string(), proxy_mode_type: config .get(\"proxyModeType\") .and_then(Value::as_str) \
         .unwrap_or(\"\") .to_string(), proxy_running: status.running, started_via_helper: \
         Some(status.started_via_helper), helper_status: None, system_proxy: None, effective_dns: \
         None, node_domain_resolver: config .get(\"dnsConfig\") .and_then(|d| \
         d.get(\"nodeDomainResolver\")) .and_then(Value::as_str) .map(str::to_owned), log_level, \
         counters: state.proxy().diagnostic_counters(),",
        1,
        "诊断报告的**上报字段**，不是判据：它要如实回答「用户盘上存的是哪一档」。改成生效值反而\
         把排障需要的原始事实抹掉（Android 上永远显示 tun，看不出那台机器存的其实是 systemProxy）。\
         Android 上这一处得到存盘裸值，那正是想要的。",
    ),
    (
        "src-tauri/src/tray/model.rs",
        "let mode_type = config .get(\"proxyModeType\") .and_then(Value::as_str) \
         .unwrap_or(\"systemProxy\") .to_ascii_lowercase()",
        1,
        "`apply_fake_ip_tun_entry`：原生兜底菜单切到 TUN 时消费「FakeIP-TUN 待纠正」快照。\
         入参是菜单**刚写进去的目标档位**（不是磁盘存量），紧接着的 `if mode_type != \\\"tun\\\"` \
         判的是「这次用户切的是不是 TUN」，与平台事实正交。整条链 `#[cfg(desktop)]`（唯一调用点在\
         `app_tray.rs` 的原生菜单动作执行器），Android 不可达；该平台也没有「切接管方式」这个入口。\
         本面的针是 `\"proxyModeType\"` 这个字面量，故本函数只计一次；紧随其后的
         `mode_type != \"tun\"` 落在同一条语句之外，由本条一并背书。",
    ),
];

// ===================== 判据本体 =====================

/// 面①：每一处按接管方式分流的判据都读生效值。
#[test]
fn every_takeover_mode_decision_reads_the_effective_value() {
    let files = production_sources();
    assert!(
        files.len() >= FILE_FLOOR,
        "只扫到 {} 个生产源文件（下限 {FILE_FLOOR}）—— 取材面塌了",
        files.len()
    );

    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (rel, path) in &files {
        let code = mask_comments_and_strings(&read(path));
        for at in decision_sites(&code) {
            let snippet = folded_statement(&code, at);
            *found.entry((rel.clone(), snippet)).or_default() += 1;
        }
    }

    // 切片自检：把取材面的规模打出来。扫到 0 处也「没有违规」，那种绿没有信息量。
    let total: usize = found.values().sum();
    println!(
        "[android-takeover] 生产源文件 {} 个，接管方式判据 {total} 次 / {} 种形态",
        files.len(),
        found.len()
    );
    assert!(
        total >= DECISION_FLOOR,
        "只扫到 {total} 处按接管方式分流的判据（下限 {DECISION_FLOOR}）—— \
         取材面塌了（路径 / 掩码 / 语句配平任一坏掉都长这样），不是全仓真的只剩这么几处"
    );

    let mut registered: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (path, snippet, count, _why) in RAW_READ_EXEMPTIONS {
        *registered
            .entry(((*path).to_owned(), (*snippet).to_owned()))
            .or_default() += *count;
    }

    let violations: Vec<String> = found
        .iter()
        .filter(|((_, snippet), _)| !snippet.contains("effective"))
        .filter(|(key, n)| registered.get(*key).copied().unwrap_or(0) < **n)
        .map(|((path, snippet), n)| format!("{path}  （实到 {n} 次）\n      {snippet}"))
        .collect();
    assert!(
        violations.is_empty(),
        "以下判据按**磁盘上存的** `proxy_mode_type` 分流，没有过 `ProxyModeType::effective_on`：\n    {}\n\n\
         那个字段的存盘缺省值是 `systemProxy`，而 Android 上只有 `VpnService` 的 tun fd 一种接管\n\
         形态 —— 于是全新安装 / 备份恢复的客户端在这一处必然走错分支，且多半是「跑得起来、看着\n\
         正常、就是不对」的静默形态（本文件头注那张表是已经踩过的四个）。\n\
         改法：把 `mode` 换成 `mode.effective_on(<平台>)`，平台从 `self.helper.platform()` /\n\
         函数的 `platform` 入参 / `Platform::parse(&deps.platform)` 取（就近同源那一个）。\n\
         确有理由读裸值的，登记进 `RAW_READ_EXEMPTIONS` 并写明「Android 上这一处走哪支、为什么对」。",
        violations.join("\n    ")
    );

    // 反腐烂：豁免条目必须真的还命中。
    for (path, snippet, _n, why) in RAW_READ_EXEMPTIONS {
        assert!(
            found.contains_key(&((*path).to_owned(), (*snippet).to_owned())),
            "`RAW_READ_EXEMPTIONS` 里的 {path} 已经找不到这条判据 —— 删掉这条豁免。（原理由：{why}）\n      {snippet}"
        );
    }
}

// Exact closed normal-library producer contract, including its actual session owner.
// Both configuration censuses keep the carrier on their ordinary source surface.
fn native_compact(source: &str) -> String {
    mask_comments(source)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}
fn native_code(source: &str) -> String {
    native_compact(&mask_comments_and_strings(source))
}
fn native_carrier_body<'a>(source: &'a str, head: &str) -> Option<&'a str> {
    let bare = mask_comments_and_strings(source);
    if bare.matches(head).count() != 1 {
        return None;
    }
    let start = bare.find(head)?;
    let open = start + bare[start..].find('{')?;
    let mut depth = 0usize;
    for (offset, byte) in bare[open..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&source[open..=open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}
// Match live code, not comments/string text, and require each causal anchor once.
fn native_ordered(body: &str, anchors: &[&str]) -> bool {
    let code = native_code(body);
    let mut previous = None;
    for anchor in anchors {
        if code.matches(anchor).count() != 1 {
            return false;
        }
        let at = code.find(anchor).unwrap();
        if previous.is_some_and(|before| before >= at) {
            return false;
        }
        previous = Some(at);
    }
    true
}
fn closed_native_session_scope(session: &str) -> bool {
    let heads = [
        "fn capability_preflight(",
        "fn caller(",
        "fn root(",
        "fn private_directory(",
        "fn lock_file(",
        "fn selected_owner(",
        "fn matches_selection(",
        "fn hold_session(",
        "fn open(",
        "fn drained(",
    ];
    let bodies: Option<Vec<_>> = heads
        .iter()
        .map(|head| native_carrier_body(session, head))
        .collect();
    let Some(bodies) = bodies else {
        return false;
    };
    let [preflight, caller, root, private, lock, owner, selection, hold, open, drained] =
        bodies.as_slice()
    else {
        return false;
    };
    let s = native_compact(session);
    let root_code = native_code(root);
    let private_code = native_code(private);
    let open_code = native_code(open);
    s.contains("usesuper::CASE_NAMES;")
        && native_code(preflight).starts_with("{super::super::core_binary::dev_core_binary_override()")
        && native_ordered(preflight, &[
            "super::super::core_binary::dev_core_binary_override()",
            "ifpolaris_core_supervisor::selected_core::selection_lane()!=polaris_core_supervisor::selected_core::SelectionLane::Formal",
            "forpurposein[ExecPurpose::CheckConfig,ExecPurpose::RunMain]",
            "check_selected_capability(purpose)",
        ])
        && native_compact(preflight).contains("super::super::core_binary::dev_core_binary_override().map(drop).map_err(|e|failure(\"selection\",\"path-override-refused\",e))?;")
        && native_code(preflight).contains("SelectionLane::Formal{returnErr(")
        && native_compact(preflight).contains("check_selected_capability(purpose).map_err(|e|failure(\"capability\",e.kind.status_token(),e))?;")
        && !["std::fs::", "ConfigManager::new(", "HelperRuntime::new(", "ProxyRuntime::new(", "TcpListener::bind("].iter().any(|effect| native_code(preflight).contains(effect))
        && native_code(caller).contains("letuid=nix::unistd::getuid().as_raw();ifuid==0{returnErr(")
        && native_code(caller).contains("Ok(polaris_helper_proto::core_protocol::CallerIdentity::Unix(uid).key())")
        && native_compact(caller).contains("ifvalue==\"sS-1-5-18\"{returnErr(")
        && native_code(caller).contains("polaris_helper::platform::windows::native_caller::current_user()")
        && root_code.contains("letkey=polaris_updater::sha256_hex(caller()?.as_bytes());")
        && native_compact(root).contains("Ok(parent.join(format!(\"polaris-native-production-verification-{key}\")))")
        && root_code.contains("std::env::temp_dir().canonicalize().map_err(|e|e.to_string())?;")
        && root_code.contains("polaris_helper::platform::windows::logic::cfg_allowed(")
        && s.contains("fnroot()->Result<PathBuf,String>{")
        && private_code.contains("std::fs::symlink_metadata(path).map_err(|e|e.to_string())?;")
        && private_code.contains("ifm.uid()!=nix::unistd::getuid().as_raw()||m.mode()&0o077!=0{returnErr(")
        && private_code.contains("PayloadDir::open(path,Some(m.uid()))")
        && private_code.contains(".custom_flags(0x02200000)")
        && private_code.contains("if!m.is_dir()||m.file_attributes()&0x400!=0{returnErr(")
        && private_code.contains("if!polaris_helper::platform::windows::coreacl::is_private_user_config(&sec,&sid[1..]){returnErr(")
        && native_ordered(lock, &["letf=matchstd::fs::symlink_metadata(root.join(name))", "f.try_lock()", "Ok(f)"])
        && native_code(lock).contains("f.try_lock().map_err(|e|format!())?;")
        && native_code(lock).contains("Err(e)ife.kind()==std::io::ErrorKind::NotFound=>create_file(&root.join(name))?,Err(e)=>returnErr(e.to_string()),")
        && native_code(owner).contains("crate::runtime::core_identity::selected_manifest(&platform)?;")
        && native_code(owner).contains("caller:caller()?,platform,core_id:id.as_str().into(),provider_fingerprint:m.provider_fingerprint,core_files:m.files,")
        && native_code(owner).contains("helper_sha256:m.helper_sha256,cleaner_sha256:m.cleaner_sha256,required_cases:CASE_NAMES.iter().map(|v|(*v).into()).collect(),")
        && native_code(selection)=="{letmuta=a.clone();a.session_id.clear();a==*b}"
        && s.contains("pub(super)fnhold_session()->Result<(),String>{")
        && native_code(hold).starts_with("{capability_preflight()?;")
        && native_ordered(hold, &["capability_preflight()?;", "letmutowner=selected_owner()", "letroot=root()", "letdir=private_directory(&root)", "let_suite=lock_file(&root,&dir,SUITE_LOCK)", "ifstd::fs::symlink_metadata(root.join(OWNER)).is_ok()", "letbefore=retained_size(&root)", "letready=provider_ready(&root);", "ifready.is_ok(){record(", "std::io::stdout().flush()", "letcase_lock=lock_file(&root,&dir,CASE_LOCK);", "letendpoint_idle=provider_idle(&root).ok();"])
        && native_code(hold).contains("ifstd::fs::symlink_metadata(root.join(OWNER)).is_ok(){returnErr(")
        && native_code(hold).contains("letmutdrained=case_lock.is_ok();")
        && native_compact(hold).contains("drained&=matches!((a,b),(Ok(a),Ok(b))ifa==b&&a[\"sessionId\"]==owner.session_id&&a[\"case\"]==CASE_NAMES[i]);")
        && native_compact(hold).contains("foriin0..CASE_NAMES.len(){create_directory(&root.join(format!(\"case-{i}\")))?;}")
        && native_code(hold).contains("record(&root.join(OWNER),&serde_json::to_value(&owner).map_err(|e|e.to_string())?,)?;")
        && native_code(hold).contains("letok=action_ok&&drained&&(started==0||endpoint_idle==Some(true))&&retained.is_ok();")
        && s.contains("pub(super)fnopen(case:&str)->Result<Self,String>{")
        && open_code.starts_with("{capability_preflight()?;")
        && native_ordered(open, &["capability_preflight()?;", "leti=CASE_NAMES.iter().position(|v|*v==case)", "letroot=root()?;", "letdirectory=private_directory(&root)?;", "letsuite=directory.open_regular(SUITE_LOCK)", "matchsuite.try_lock()", "letcase_lock=lock_file(&root,&directory,CASE_LOCK)?;", "retained_size(&root)?;", "letowner:Owner=serde_json::from_value(read_record(&directory,OWNER)?)", "ifuuid::Uuid::parse_str(&owner.session_id).is_err()||!matches_selection(&owner,&selected_owner()?)", "provider_ready(&root)", "letcase_dir=root.join(format!(", "private_directory(&case_dir)?;", "record(&root.join(format!(", "Ok(Self{"])
        && open_code.contains("ifuuid::Uuid::parse_str(&owner.session_id).is_err()||!matches_selection(&owner,&selected_owner()?){returnErr(")
        && open_code.contains("Err(std::fs::TryLockError::WouldBlock)=>{}Err(e)=>returnErr(")
        && open_code.contains("Ok(())=>{returnErr(")
        && native_compact(open).contains("provider_ready(&root).map_err(|e|failure(\"provider\",\"provider-unavailable\",e))?;")
        && native_compact(open).contains("letcase_dir=root.join(format!(\"case-{i}\"));")
        && native_compact(open).contains("record(&root.join(format!(\"case-{i}-started.json\")),&receipt)?;")
        && open_code.contains("Ok(Self{config_root:root.clone(),case_dir,root,receipt,_case_lock:case_lock,_directory:directory,})")
        && native_compact(drained).contains("record(&self.root.join(format!(\"case-{i}-drained.json\")),&self.receipt,)")
}
fn closed_native_configuration_scope(
    proxy: &str,
    library: &str,
    carrier: &str,
    session: &str,
    cleanup: &str,
    entrypoint: &str,
    manifest: &str,
) -> bool {
    let c = native_compact(carrier);
    let bare = native_code(carrier);
    let cases = [
        ("runtime::proxy::tests::lifecycle::real_core_full_lifecycle", "lifecycle_case", "real_core_full_lifecycle"),
        ("runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop", "lifecycle_race_case", "real_core_lifecycle_race_start_then_immediate_stop"),
        ("runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid", "hot_switch_case", "real_core_hot_switch_keeps_pid"),
        ("runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt", "failover_case", "real_core_auto_failover_attests_without_applying_saved_debt"),
        ("runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation", "restart_confirmation_case", "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation"),
        ("runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames", "aggregate_relay_case", "real_core_aggregate_relay_emits_real_frames"),
    ];
    let names = format!(
        "constCASE_NAMES:&[&str]=&[{}];",
        cases
            .iter()
            .map(|(name, _, _)| format!("\"{name}\","))
            .collect::<String>()
    );
    let Some(selector) = native_carrier_body(carrier, "fn selected_case_factory(") else {
        return false;
    };
    let expected_selector = format!(
        "{{matchcase{{{}_=>None,}}}}",
        cases
            .iter()
            .map(|(name, factory, _)| format!("\"{name}\"=>{{Some({factory})}}"))
            .collect::<String>()
    );
    if native_compact(selector) != expected_selector
        || !bare.contains("typeNativeCaseFactory=fn()->NativeCaseFuture;")
        || !bare.contains(
            "typeNativeCaseFuture=std::pin::Pin<Box<dynstd::future::Future<Output=()>+'static>>;",
        )
        || !cases.iter().all(|(_, factory, method)| {
            native_carrier_body(carrier, &format!("fn {factory}("))
                .is_some_and(|body| native_compact(body) == format!("{{Box::pin({method}())}}"))
        })
    {
        return false;
    }
    let Some(run_body) = native_carrier_body(carrier, "pub(crate) async fn run_case(") else {
        return false;
    };
    let Some(caught) = native_carrier_body(run_body, "let result = AssertUnwindSafe(async") else {
        return false;
    };
    if !native_ordered(caught, &["letfuture=factory();", "future.await;"])
        || !native_compact(run_body).contains(&format!(
            "letresult=AssertUnwindSafe(async{}).catch_unwind().await;",
            native_compact(caught)
        ))
    {
        return false;
    }
    let run = native_compact(run_body);
    let run_code = native_code(run_body);
    let policy = "super::core_binary::dev_core_binary_override().map(drop)?;";
    let Some(policy_at) = run_code.find(policy) else {
        return false;
    };
    if !native_ordered(
        run_body,
        &[
            "letfactory=selected_case_factory(case).ok_or(",
            "session::capability_preflight()?;",
            policy,
            "letsession=session::CaseSession::open(case)?;",
            "letdir=session.case_dir.clone();",
            "letconfig=Arc::new(ConfigManager::new(dir.clone()));",
            "lethelper=Arc::new(HelperRuntime::new(session.config_root.clone()));",
            "letmesh=Arc::new(MeshRuntime::new_with_helper(dir.clone(),helper.clone()));",
            "letclearer=Box::new(polaris_system_integration::production_proxy_controller(",
            "letruntime=Arc::new(ProxyRuntime::new(config,helper,mesh,clearer));",
            "DIRECTORY.set(dir.clone()).map_err(",
            "RUNTIME.set(runtime.clone()).map_err(",
            "letresult=AssertUnwindSafe(async{",
            "letfuture=factory();",
            "future.await;",
            ").catch_unwind().await;",
            "letcheck_fence=polaris_core_supervisor::begin_check_shutdown()",
            "letcleanup=cleanup::drain_after_fence(check_fence,runtime.shutdown_for_exit(),async{",
            "polaris_core_supervisor::shutdown_checks_for_exit()",
            "ifcleanup.is_ok()&&fenced&&empty&&no_pid",
            "session.drained(case)?;",
            "ifresult.is_err()",
        ],
    ) || run_code.matches("factory()").count() != 1
        || [
            "std::fs::",
            "eprintln!",
            "ConfigManager::new(",
            "HelperRuntime::new(",
            "ProxyRuntime::new(",
            "CaseSession::open(",
            "factory()",
        ]
        .iter()
        .any(|effect| run_code[..policy_at].contains(effect))
    {
        return false;
    }
    let Some(drain) = native_carrier_body(cleanup, "pub(super) async fn drain_after_fence(") else {
        return false;
    };
    let p = native_compact(proxy);
    let l = native_compact(library);
    let e = native_compact(entrypoint);
    p.contains("#[cfg(desktop)]#[cfg(feature=\"native-production-verification\")]#[cfg(debug_assertions)]#[path=\"../native_verification/proxy_carrier.rs\"]pub(crate)modnative_verification;")
        && l.contains("#[cfg(desktop)]#[cfg(feature=\"native-production-verification\")]#[cfg(debug_assertions)]pubasyncfnrun_native_production_verification(case:&str)->Result<(),String>{runtime::proxy::native_verification::run_case(case).await}")
        && l.contains("#[cfg(desktop)]#[cfg(feature=\"native-production-verification\")]#[cfg(debug_assertions)]pubfnhold_native_production_verification_session()->Result<(),String>{runtime::proxy::native_verification::hold_session()}")
        && e.contains("#[cfg(not(debug_assertions))]compile_error!(\"nativeadmissionverifierisunavailableinreleasebuilds\");")
        && e.contains("letresult=ifargs.len()==1&&args[0]==\"--session\"{polaris_lib::hold_native_production_verification_session()}elseifargs.len()==1{polaris_lib::run_native_production_verification(&args[0]).await}else{")
        && native_compact(manifest).contains("[[bin]]name=\"native-admission-verifier\"path=\"src/bin/native-admission-verifier.rs\"required-features=[\"native-production-verification\"]")
        && c.contains("#[path=\"session.rs\"]modsession;")
        && c.contains("pub(crate)fnhold_session()->Result<(),String>{session::hold_session()}")
        && c.contains("#[cfg(debug_assertions)]pub(crate)asyncfnrun_case(case:&str)->Result<(),String>{if!CASE_NAMES.contains(&case){returnErr(")
        && c.contains(&names)
        && run.contains("DIRECTORY.set(dir.clone()).map_err(|_|\"onecaseperprocess\")?;")
        && run.contains("RUNTIME.set(runtime.clone()).map_err(|_|\"onecaseperprocess\")?;")
        && c.contains("obj.insert(\"proxyModeType\".into(),serde_json::json!(\"manual\"));")
        && c.contains("\"proxyModeType\":\"manual\",")
        && !c.contains("\"allowLan\"") && !c.contains("allow_lan")
        && run_code.contains("letfenced=runtime.desktop_shutdown.lock().map(|v|*v).unwrap_or(false);")
        && run_code.contains(".map(|v|v.is_empty()&&!v.has_helper_start()).unwrap_or(false);")
        && run_code.contains("letno_pid=runtime.pid.lock().map(|v|v.is_none()).unwrap_or(false);")
        && native_code(drain).starts_with("{letmain=main.await;letchecks=checks.await;letmutfailures=Vec::new();")
        && native_ordered(drain,&["letmain=main.await;","letchecks=checks.await;","letmutfailures=Vec::new();"])
        && native_compact(drain).contains("[(\"Checkfence\",fence),(\"mainshutdown\",main),(\"Checkdrain\",checks),]")
        && closed_native_session_scope(session)
}
fn native_scope_faces() -> [String; 7] {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    [
        "src-tauri/src/runtime/proxy.rs",
        "src-tauri/src/lib.rs",
        "src-tauri/src/native_verification/proxy_carrier.rs",
        "src-tauri/src/native_verification/session.rs",
        "src-tauri/src/native_verification/cleanup.rs",
        "src-tauri/src/bin/native-admission-verifier.rs",
        "src-tauri/Cargo.toml",
    ]
    .map(|path| std::fs::read_to_string(root.join(path)).expect("actual native producer context"))
}
fn native_scope_is_closed(faces: &[String; 7]) -> bool {
    closed_native_configuration_scope(
        &faces[0], &faces[1], &faces[2], &faces[3], &faces[4], &faces[5], &faces[6],
    )
}
// Mutations below always target the current real producer/owner/caller source.
fn assert_native_scope_controls(faces: &[String; 7]) {
    assert!(native_scope_is_closed(faces));
    // Each conjunct is independently necessary on the actual facade and both exports.
    for (slot, tail) in [
        (0, "#[path = \"../native_verification/proxy_carrier.rs\"]"),
        (1, "pub async fn run_native_production_verification"),
        (1, "pub fn hold_native_production_verification_session"),
    ] {
        let prefix = format!("#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\n{tail}");
        assert_eq!(faces[slot].matches(&prefix).count(), 1);
        for attribute in [
            "#[cfg(desktop)]\n",
            "#[cfg(feature = \"native-production-verification\")]\n",
            "#[cfg(debug_assertions)]\n",
        ] {
            let mut changed = (*faces).clone();
            changed[slot] = faces[slot].replacen(&prefix, &prefix.replacen(attribute, "", 1), 1);
            assert!(
                !native_scope_is_closed(&changed),
                "missing native scope conjunct: {tail}: {attribute}"
            );
        }
    }
    for (slot,old,new) in [
        (0,"#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\n#[path = \"../native_verification/proxy_carrier.rs\"]","#[path = \"../native_verification/proxy_carrier.rs\"]"),
        (1,"#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\npub async fn run_native_production_verification","pub async fn run_native_production_verification"),
        (1,"#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\npub fn hold_native_production_verification_session","pub fn hold_native_production_verification_session"),
        (2,"#[cfg(debug_assertions)]\npub(crate) async fn run_case","pub(crate) async fn run_case"),
        (2,"if !CASE_NAMES.contains(&case)","if false"),
        (2,"ConfigManager::new(dir.clone())","ConfigManager::new(user_directory())"),
        (2,"let dir = session.case_dir.clone();","let dir = user_directory();"),
        (2,"HelperRuntime::new(session.config_root.clone())","HelperRuntime::new(dir.clone())"),
        (2,"let session = session::CaseSession::open(case)?;","let session = unbound_session();"),
        (2,"serde_json::json!(\"manual\")","serde_json::json!(\"tun\")"),
        (2,"\"proxyModeType\": \"manual\"","\"proxyModeType\": \"systemProxy\""),
        (2,"runtime.shutdown_for_exit()","pretend_cleanup()"),
        (2,"polaris_core_supervisor::shutdown_checks_for_exit()","pretend_check_cleanup()"),
        (3,"polaris_updater::sha256_hex(caller()?.as_bytes())","polaris_updater::sha256_hex(b\"another-caller\")"),
        (3,"polaris-native-production-verification-{key}","user-configuration-{key}"),
        (3,"m.uid() != nix::unistd::getuid().as_raw() || m.mode() & 0o077 != 0","false"),
        (3,"!polaris_helper::platform::windows::coreacl::is_private_user_config(&sec, &sid[1..])","false"),
        (3,"config_root: root.clone(),","config_root: user_directory(),"),
        (3,"_case_lock: case_lock,","_case_lock: unrelated_lock(),"),
        (3,"required_cases: CASE_NAMES.iter().map(|v| (*v).into()).collect(),","required_cases: Vec::new(),"),
        (4,"let checks = checks.await;","let checks = Ok(());"),
        (5,"polaris_lib::hold_native_production_verification_session()","ordinary_session()"),
        (6,"required-features = [\"native-production-verification\"]","required-features = []"),
    ] {
        let mut changed=(*faces).clone();
        assert!(changed[slot].contains(old),"negative must mutate actual source: {slot}: {old}");
        changed[slot]=changed[slot].replacen(old,new,1);
        assert!(!native_scope_is_closed(&changed),"closed native scope escaped: {slot}: {old}");
    }
    for (slot, head, old, new) in [
        (
            2,
            "fn selected_case_factory(",
            "Some(lifecycle_case)",
            "Some(hot_switch_case)",
        ),
        (
            2,
            "fn selected_case_factory(",
            "_ => None",
            "_ => Some(lifecycle_case)",
        ),
        (
            2,
            "fn lifecycle_case(",
            "Box::pin(real_core_full_lifecycle())",
            "Box::pin(real_core_hot_switch_keeps_pid())",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "let result = AssertUnwindSafe(async {",
            "let future = factory();\nlet result = AssertUnwindSafe(async {",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "super::core_binary::dev_core_binary_override().map(drop)?;",
            "super::core_binary::dev_core_binary_override().ok();",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "session::capability_preflight()?;",
            "session::capability_preflight().ok();",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "if cleanup.is_ok() && fenced && empty && no_pid",
            "if true",
        ),
        (
            3,
            "fn capability_preflight(",
            ".map_err(|e| failure(\"selection\", \"path-override-refused\", e))?;",
            ".ok();",
        ),
        (
            3,
            "fn capability_preflight(",
            "ExecPurpose::CheckConfig, ExecPurpose::RunMain",
            "ExecPurpose::RunMain, ExecPurpose::RunMain",
        ),
        (
            3,
            "fn capability_preflight(",
            "!= polaris_core_supervisor::selected_core::SelectionLane::Formal",
            "== polaris_core_supervisor::selected_core::SelectionLane::Formal",
        ),
        (
            3,
            "fn hold_session(",
            "let mut drained = case_lock.is_ok();",
            "let mut drained = true;",
        ),
        (
            3,
            "fn hold_session(",
            "if std::fs::symlink_metadata(root.join(OWNER)).is_ok()",
            "if false",
        ),
        (
            3,
            "fn hold_session(",
            "capability_preflight()?;",
            "capability_preflight().ok();",
        ),
        (
            3,
            "fn hold_session(",
            "lock_file(&root, &dir, SUITE_LOCK)",
            "lock_file(&root, &dir, CASE_LOCK)",
        ),
        (
            3,
            "fn open(",
            "capability_preflight()?;",
            "capability_preflight().ok();",
        ),
        (
            3,
            "fn open(",
            "Err(std::fs::TryLockError::WouldBlock) => {}",
            "Ok(()) => {}",
        ),
        (
            3,
            "fn open(",
            "!matches_selection(&owner, &selected_owner()?)",
            "false",
        ),
        (
            3,
            "fn open(",
            "private_directory(&case_dir)?;",
            "private_directory(&root)?;",
        ),
        (3, "fn lock_file(", "f.try_lock()", "f.metadata()"),
    ] {
        let body = native_carrier_body(&faces[slot], head).expect("actual native boundary");
        assert_eq!(body.matches(old).count(), 1, "{slot}: {head}: {old}");
        assert_eq!(faces[slot].matches(body).count(), 1);
        let mut changed = (*faces).clone();
        changed[slot] = faces[slot].replacen(body, &body.replacen(old, new, 1), 1);
        assert!(
            !native_scope_is_closed(&changed),
            "native owner boundary escaped: {head}: {old}"
        );
    }
    let drain = native_carrier_body(&faces[4], "pub(super) async fn drain_after_fence(").unwrap();
    assert_eq!(drain.matches("let checks = checks.await;").count(), 1);
    let early_return = drain.replacen(
        "let checks = checks.await;",
        "if main.is_err() { return main; }\nlet checks = checks.await;",
        1,
    );
    let mut changed = (*faces).clone();
    changed[4] = faces[4].replacen(drain, &early_return, 1);
    assert!(
        !native_scope_is_closed(&changed),
        "Main error cannot skip the original Check owner drain"
    );
    let run = native_carrier_body(&faces[2], "pub(crate) async fn run_case(").unwrap();
    for anchor in [
        "let future = factory();",
        "let result = AssertUnwindSafe(async {",
        "session.drained(case)?;",
        "let check_fence =",
    ] {
        assert_eq!(
            run.matches(anchor).count(),
            1,
            "actual movement target: {anchor}"
        );
    }
    let outside = run.replacen("let future = factory();", "", 1).replacen(
        "let result = AssertUnwindSafe(async {",
        "let future = factory();\nlet result = AssertUnwindSafe(async {",
        1,
    );
    let mut changed = (*faces).clone();
    changed[2] = faces[2].replacen(run, &outside, 1);
    assert!(
        !native_scope_is_closed(&changed),
        "the one selected future must be constructed inside catch"
    );
    let moved = run.replacen("session.drained(case)?;", "", 1).replacen(
        "let check_fence =",
        "session.drained(case)?;\nlet check_fence =",
        1,
    );
    let mut changed = (*faces).clone();
    changed[2] = faces[2].replacen(run, &moved, 1);
    assert!(
        !native_scope_is_closed(&changed),
        "checkpoint cannot precede original Main/Check drain"
    );
}

#[test]
fn native_manual_writes_require_the_closed_private_desktop_runtime() {
    assert_native_scope_controls(&native_scope_faces());
}

/// 面②：从原始 JSON 读 `proxyModeType` 的每一处都必须登记。
#[test]
fn every_raw_json_read_of_the_takeover_mode_is_registered() {
    let files = production_sources();
    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (rel, path) in &files {
        // 只剥注释、**保留字符串**：被判据物本身就是 `"proxyModeType"` 这个字面量。
        let code = mask_comments(&read(path));
        let mut from = 0usize;
        while let Some(offset) = code[from..].find("\"proxyModeType\"") {
            let at = from + offset;
            *found
                .entry((rel.clone(), folded_statement(&code, at)))
                .or_default() += 1;
            from = at + 1;
        }
    }

    let total: usize = found.values().sum();
    println!(
        "[android-takeover] 原始 JSON 读 proxyModeType：{total} 次 / {} 种形态",
        found.len()
    );
    assert!(
        total >= RAW_JSON_FLOOR,
        "只扫到 {total} 处原始 JSON 读（下限 {RAW_JSON_FLOOR}）—— 取材面塌了（本面保留字符串，\
         掩码若把字面量也剥了就长这样）"
    );

    let mut registered: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (path, snippet, count, _why) in RAW_JSON_REGISTRY {
        *registered
            .entry(((*path).to_owned(), (*snippet).to_owned()))
            .or_default() += *count;
    }

    let unregistered: Vec<String> = found
        .iter()
        .filter(|(key, n)| registered.get(*key).copied().unwrap_or(0) < **n)
        .map(|((path, snippet), n)| {
            let r = registered
                .get(&(path.clone(), snippet.clone()))
                .copied()
                .unwrap_or(0);
            format!("{path}  （实到 {n} 次，登记 {r} 次）\n      {snippet}")
        })
        .collect();
    let rotten: Vec<String> = registered
        .iter()
        .filter(|(key, n)| found.get(*key).copied().unwrap_or(0) < **n)
        .map(|((path, snippet), n)| {
            let f = found
                .get(&(path.clone(), snippet.clone()))
                .copied()
                .unwrap_or(0);
            format!("{path}  （登记 {n} 次，实到 {f} 次）\n      {snippet}")
        })
        .collect();

    assert!(
        unregistered.is_empty(),
        "以下地方**绕过 `UserConfig`**、直接从原始 JSON 读接管方式：\n    {}\n\n\
         面①的针（`.is_tun()` / `ProxyModeType::<变体>`）够不着这种写法，而它是一条完整的逃逸路：\n\
         把 `proxyModeType` 当字符串掏出来再比 `== \"tun\"`，Android 上就又回到了照读存盘缺省值\n\
         `systemProxy`。故每一处都必须在 `RAW_JSON_REGISTRY` 里写下「Android 上这里得到什么、\n\
         为什么那个答案可以」。能改成走 `UserConfig` + `effective_on` 的，优先改。",
        unregistered.join("\n    ")
    );
    assert!(
        rotten.is_empty(),
        "以下登记条目在源码里已经找不到（判据被改动或删除）：\n    {}\n\n\
         登记表不是垃圾桶：改了就要重新答一次「Android 在这里得到什么」，再把语句原文更新掉。",
        rotten.join("\n    ")
    );
}

/// **切点自检**：两个掩码面真的在按预期工作。
///
/// 没有这条，「掩码把整份代码抹成空白」与「全仓真的没有违规」在门里长得一模一样 ——
/// 上面两条的 FLOOR 拦得住整体塌方，拦不住「某一类语法被吃掉」这种局部失效。
#[test]
fn the_masks_keep_code_and_drop_the_right_halves() {
    let sample = r#"
// 注释里写 mode.is_tun() 不算判据
let a = "字面量里的 proxyModeType 不算符号";
let b = mode.effective_on(platform).is_tun();
let c = raw.get("proxyModeType");
"#;
    let symbols = mask_comments_and_strings(sample);
    assert!(
        !symbols.contains("注释里写"),
        "符号面必须剥掉注释，否则门会被自己的说明喂饱"
    );
    assert!(
        !symbols.contains("字面量里的"),
        "符号面必须剥掉字符串字面量"
    );
    assert!(
        symbols.contains("mode.effective_on(platform).is_tun()"),
        "符号面必须原样保留代码 —— 剥过头的话上面两条判据就恒绿了"
    );
    assert_eq!(
        symbols.matches(".is_tun()").count(),
        1,
        "注释里那处 `.is_tun()` 必须被剥掉，只剩代码里那一处"
    );

    let literals = mask_comments(sample);
    assert!(!literals.contains("注释里写"), "字面量面同样必须剥注释");
    assert!(
        literals.contains("\"proxyModeType\""),
        "字面量面必须保留字符串 —— 那正是面②的针"
    );

    // 长度与行号守恒（失败信息要说得出「第几行」）。
    assert_eq!(symbols.len(), sample.len());
    assert_eq!(literals.len(), sample.len());
}

// ===================== 取材面 =====================

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("读不到 `{}`（{error}）", path.display()))
}

/// 生产源码面：`src-tauri/src/**`，排除测试模块。
///
/// 取整个 `src/` 而不是只取 `runtime/` + `commands/`：判据要覆盖的是「这个 crate 里按接管方式
/// 分流的地方」，而不是「今天它们恰好在哪两个目录里」—— 后者会让一次搬家把门变成摆设。
fn production_sources() -> Vec<(String, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 必有上级目录")
        .to_path_buf();
    let mut paths = Vec::new();
    walk(&root.join("src-tauri/src"), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&root)
                .expect("扫描面必在仓内")
                .to_string_lossy()
                .replace('\\', "/");
            (rel, path)
        })
        .filter(|(rel, _)| !rel.contains("/tests/") && !rel.ends_with("/tests.rs"))
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// 面①的针：**判别**形态的偏移量。
///
/// 只认三种写法，都是「拿接管方式做决定」的当场：
/// - `.is_tun()`：方法调用形态；
/// - `ProxyModeType::is_tun`：函数值形态（`.is_some_and(ProxyModeType::is_tun)`）；
/// - `ProxyModeType::<变体>`：`matches!` / `==` / `!=` / `match` 臂里的判别对象。
///
/// **刻意不认**「把 `proxy_mode_type` 当实参传下去」：那不是决定点（真正的判别在被调方，而被调方
/// 也在取材面里），把它算进来只会逼人在传参处写噪音，而噪音会训练人去迎合门。
fn decision_sites(code: &str) -> Vec<usize> {
    const NEEDLES: [&str; 5] = [
        ".is_tun()",
        "ProxyModeType::is_tun",
        "ProxyModeType::SystemProxy",
        "ProxyModeType::Tun",
        "ProxyModeType::Manual",
    ];
    let mut out: Vec<usize> = Vec::new();
    for needle in NEEDLES {
        let mut from = 0usize;
        while let Some(offset) = code[from..].find(needle) {
            let at = from + offset;
            // `ProxyModeType::Tun` 是 `ProxyModeType::TunSomething` 的前缀 —— 只有后面不接标识符
            // 字符时才算一次命中，否则会把将来新增的变体名重复计数。
            let tail_is_ident = code[at + needle.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            if !tail_is_ident {
                out.push(at);
            }
            from = at + 1;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// `pos` 所在语句（`;` / `{` / `}` 为界）折叠空白后的原文。
///
/// 按语句原文而不是行号：行号在任何一次插入注释后就全错，那种红是噪音、会训练人去无脑更新表；
/// 折叠空白后的语句原文对排版免疫，只在**判据本身**改动时才变 —— 那正是要重新审的时刻。
fn folded_statement(code: &str, pos: usize) -> String {
    let bytes = code.as_bytes();
    let mut start = pos;
    while start > 0 && !matches!(bytes[start - 1], b';' | b'{' | b'}') {
        start -= 1;
    }
    let mut end = pos;
    while end < bytes.len() && !matches!(bytes[end], b';' | b'{' | b'}') {
        end += 1;
    }
    code[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
