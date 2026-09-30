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
//! | `proxy/dns_race.rs` 的 `plan_upstreams` | INV-1「TUN 期把 `system` 上游从竞速池摘除」不生效 ⇒ sidecar 的 `system` 上游经 OS resolver 明文 `:53` 出去、被 `hijack-dns` 抓回来 ⇒ 自递归（第二道防线仍在，故是降级不是熔毁） |
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
//! - `crates/**`：`config-engine` 侧由它自己那道整份产物门守；`dns-race` 的 `plan_upstreams`
//!   与 `system-integration` 的 `should_reconcile_dns` 是**被喂**判据的纯函数（入参已由
//!   `src-tauri` 侧过完生效值），它们不知道也不该知道平台。
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
    (
        "src-tauri/src/app_tray.rs",
        "cfg[\"proxyModeType\"] = serde_json::Value::String(kind.to_string())",
        1,
        "原生兜底托盘菜单「切接管方式」的落盘腿 —— 这是**写**不是读，写的是用户在菜单里点的那一档。\
         Android 不可达：`MenuAction::Takeover` 与整棵原生菜单都在 `#[cfg(desktop)]` 之下\
         （移动端 `tauri::menu` 模块整个不存在），且该平台上「系统代理 vs TUN」这个维度不存在\
         （见 app_tray.rs 头注那张能力裁定表）。",
    ),
    (
        "src-tauri/src/app_tray.rs",
        "for key in [\"proxyModeType\", \"dnsConfig\", \"dnsDefaults\"]",
        1,
        "同上那条落盘腿的原子补丁键清单（proxyModeType + FakeIP-TUN 纠正一起落盘，不许拆）。\
         `#[cfg(desktop)]`，Android 不可达。",
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
