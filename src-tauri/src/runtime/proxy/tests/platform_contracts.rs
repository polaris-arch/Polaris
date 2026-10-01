use super::*;
use crate::runtime::proxy::core_binary::LINUX_BUNDLE_PRODUCT_DIR;
use crate::runtime::proxy::platform_contracts::core_platform_dirs;

/// 内核平台目录必须与 fetch-core.mjs / tauri.conf.json 逐字一致。
/// 打断 macOS 分支（若回退成 "mac"）→ 本测转红，即「mac 找不到内核、代理起不来」那个 bug。
#[test]
fn core_platform_dirs_match_fetch_layout() {
    // macOS：按 arch 选 mac-arm64/mac-x64，**绝不是 "mac"**（那是 bug 值）。
    assert_eq!(
        core_platform_dirs("macos", "aarch64"),
        vec!["mac-arm64", "mac-x64"]
    );
    assert_eq!(
        core_platform_dirs("macos", "x86_64"),
        vec!["mac-x64", "mac-arm64"]
    );
    assert!(
        !core_platform_dirs("macos", "aarch64").contains(&"mac"),
        "macOS 目录必须带 arch 后缀，裸 'mac' 是 fetch-core 里不存在的目录"
    );
    // 其余平台与 fetch-core 落地目录一致。
    assert_eq!(core_platform_dirs("linux", "x86_64"), vec!["linux"]);
    assert_eq!(core_platform_dirs("windows", "x86_64"), vec!["win"]);
}

/// **钉 macOS `_up_` 布局回归**：tauri 把 `../resources/` 打进 `Contents/Resources/_up_/resources/`。
/// 候选必须含 `_up_` 段，否则打包态 mac 上 sing-box 核 / polaris-helper 恒找不到、proxy 起不来
/// （真机踩坑：recipe 里「core 路径已修」只加了无 `_up_` 的 `Resources/resources/`，仍找不到）。
#[test]
fn bundle_candidates_include_macos_up_layout() {
    use std::path::Path;
    let exe_dir = Path::new("/Applications/Polaris.app/Contents/MacOS");
    let manifest = Path::new("/dev/polaris/src-tauri");
    let c = bundle_resource_candidates(
        Some(exe_dir),
        Some(manifest),
        &["mac-arm64", "mac-x64"],
        "sing-box",
    );
    // 关键断言：mac `_up_` 布局候选必须在，且必须**从 mac 前缀**来——单 contains 会被
    // `<exe>/_up_/resources/`（Windows NSIS 形态，2026-08-19 加）喂成恒绿假钉（评审实证：
    // 删 mac `_up_` push 后单条件版本 2/2 仍绿）。双条件合取恢复杀伤力。
    // Windows 上 Path::join 产生 `\` 分隔 → 归一成 `/` 再比子串（断言的是布局结构非分隔符）。
    assert!(
        c.iter().any(|p| {
            let n = p.to_string_lossy().replace('\\', "/");
            n.contains("_up_/resources/mac-arm64/sing-box")
                && n.starts_with("/Applications/Polaris.app/Contents/MacOS/../Resources/_up_")
        }),
        "缺 macOS `_up_` 布局候选 → 打包态核找不到；候选={c:?}"
    );
    // 开发态 CARGO_MANIFEST_DIR/../resources 兜底也在（`..` 不规范化，串里保留 `src-tauri/../`）。
    assert!(c.iter().any(|p| p
        .to_string_lossy()
        .replace('\\', "/")
        .contains("src-tauri/../resources/mac-arm64/sing-box")));
    // exe_dir=None（取不到 exe）时只剩开发态候选，不 panic。
    let none = bundle_resource_candidates(None, Some(manifest), &["linux"], "sing-box");
    assert_eq!(none.len(), 1);
}

/// **钉 Windows NSIS 装机布局回归（W10 根因）**：资源在 `<exe目录>\_up_\resources\`。
/// 候选缺 `<exe>/_up_/resources/<平台>/` 形态 → 装机态 helper 安装报「未找到二进制」不触发提权、
/// 核解析同函数同病（2026-08-19 真机 toast 首曝；候选表当时只有 mac 的两种 `_up_` 形态）。
#[test]
fn bundle_candidates_include_windows_nsis_up_layout() {
    use std::path::Path;
    let exe_dir = Path::new(r"C:\Users\doveh\AppData\Local\Polaris");
    let manifest = Path::new("/dev/polaris/src-tauri");
    let c = bundle_resource_candidates(
        Some(exe_dir),
        Some(manifest),
        &["win"],
        "polaris-helper.exe",
    );
    // 关键断言：`<exe>/_up_/resources/win/` 必须在（删那行 push → 本测转红）。
    assert!(
        c.iter().any(|p| p
            .to_string_lossy()
            .replace('\\', "/")
            .contains("/_up_/resources/win/polaris-helper.exe")
            && p.to_string_lossy()
                .replace('\\', "/")
                .starts_with("C:/Users/doveh/AppData/Local/Polaris/_up_")),
        "缺 Windows NSIS `_up_` 装机布局候选 → 装机态核/helper恒找不到；候选={c:?}"
    );
}

/// **`productName` 塌成一份事实之后，仅存的那道保险**。
///
/// 此前 Rust 侧的 `LINUX_BUNDLE_PRODUCT_DIR` 与 `tauri.conf.json` 的 `productName` 是两份
/// 字面量，由 `verify-packaging.mjs confs` 正则抓源码对拍（那道门的代价见常量文档）。现在
/// Rust 侧的值由 `build.rs::export_product_name` 注入，本测试是**唯一**还在核对「注入链有没有
/// 真把 conf 里那个值送到这里」的东西：独立再读一遍 conf，逐字比 `env!` 的结果。
///
/// 能被它抓到的失败形态：build.rs 读错键、注入了硬编码值、或漏了 `rerun-if-changed`
/// 导致改完 conf 不重编、注入值陈旧。
#[test]
fn injected_product_name_matches_tauri_conf() {
    let conf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw =
        std::fs::read_to_string(&conf).unwrap_or_else(|e| panic!("读不到 {}：{e}", conf.display()));
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).expect("tauri.conf.json 不是合法 JSON");
    let from_conf = parsed
        .get("productName")
        .and_then(|v| v.as_str())
        .expect("tauri.conf.json 缺少字符串 productName");
    assert_eq!(
        LINUX_BUNDLE_PRODUCT_DIR, from_conf,
        "编译期注入的 productName 与 tauri.conf.json 不一致 —— 注入链断了。\n\
             后果：Linux deb/AppImage 的 /usr/lib/<productName>/ 候选路径用错名字，包内 payload \n\
             校验仍会绿，运行期 core/helper/geo/dashboard 四类消费者一起判成缺失。"
    );
}

/// **钉 Linux deb/AppImage FHS 布局回归**：Tauri 把可执行文件放 `usr/bin`，资源放
/// `usr/lib/Polaris/_up_/resources`。候选缺这一腿时，AppImage/DEB 包内 payload 校验仍会绿，
/// 但运行期 core/helper/geo/dashboard 四类消费者会一起报「未找到」。
#[test]
fn bundle_candidates_include_linux_fhs_tauri_layout() {
    let root = TestDir::new("polaris-linux-fhs-");
    let exe_dir = root.join("usr").join("bin");
    let authoritative = exe_dir
        .join("..")
        .join("lib")
        // 名字取常量：本测试断言的是 FHS **布局形状**，不是产品叫什么；
        // 写死字面量等于把 productName 又存了第三份（改名即误红，且报错文案指错地方）。
        .join(LINUX_BUNDLE_PRODUCT_DIR)
        .join("_up_")
        .join("resources")
        .join("linux")
        .join("sing-box");
    let legacy = exe_dir.join("resources").join("linux").join("sing-box");
    std::fs::create_dir_all(authoritative.parent().unwrap()).unwrap();
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&authoritative, b"FHS").unwrap();
    std::fs::write(&legacy, b"LEGACY").unwrap();

    let candidates = bundle_resource_candidates(
        Some(&exe_dir),
        Some(&root.join("src-tauri")),
        &["linux"],
        "sing-box",
    );
    assert!(
        candidates.iter().any(|p| p == &authoritative),
        "缺 Linux FHS/Tauri 资源布局候选；候选={candidates:?}"
    );
    assert_eq!(
        first_existing_bundle_candidate(&candidates).unwrap(),
        authoritative,
        "FHS 权威 payload 必须先于可能残留的 usr/bin/resources legacy 副本"
    );
}

/// 安装升级不会替应用删除旧版 `resources/`：Windows/macOS 两套目录并存时必须取新包
/// `_up_`，但 portable 只有裸目录时仍须正常回落。
#[test]
fn bundle_selection_prefers_up_resources_over_legacy_layouts() {
    let dir = TestDir::new("polaris-bundle-order-");
    let exe_dir = dir.join("Polaris");
    let authoritative = exe_dir
        .join("_up_")
        .join("resources")
        .join("win")
        .join("sing-box.exe");
    let legacy = exe_dir.join("resources").join("win").join("sing-box.exe");
    std::fs::create_dir_all(authoritative.parent().unwrap()).unwrap();
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&authoritative, b"NEW").unwrap();
    std::fs::write(&legacy, b"OLD").unwrap();

    let candidates = bundle_resource_candidates(
        Some(&exe_dir),
        Some(&dir.join("src-tauri")),
        &["win"],
        "sing-box.exe",
    );
    let selected = first_existing_bundle_candidate(&candidates).unwrap();
    assert_eq!(selected, authoritative, "NSIS 两套并存时不得命中旧 payload");
    assert_eq!(std::fs::read(&selected).unwrap(), b"NEW");

    std::fs::remove_file(&authoritative).unwrap();
    assert_eq!(
        first_existing_bundle_candidate(&candidates).unwrap(),
        legacy,
        "portable/legacy 只有裸 resources 时仍须回落"
    );

    let mac_exe_dir = dir.join("Polaris.app").join("Contents").join("MacOS");
    let mac_authoritative = mac_exe_dir
        .join("..")
        .join("Resources")
        .join("_up_")
        .join("resources")
        .join("mac-arm64")
        .join("sing-box");
    let mac_legacy = mac_exe_dir
        .join("..")
        .join("Resources")
        .join("resources")
        .join("mac-arm64")
        .join("sing-box");
    std::fs::create_dir_all(mac_authoritative.parent().unwrap()).unwrap();
    std::fs::create_dir_all(mac_legacy.parent().unwrap()).unwrap();
    std::fs::write(&mac_authoritative, b"NEW-MAC").unwrap();
    std::fs::write(&mac_legacy, b"OLD-MAC").unwrap();
    let mac_candidates = bundle_resource_candidates(
        Some(&mac_exe_dir),
        Some(&dir.join("src-tauri")),
        &["mac-arm64"],
        "sing-box",
    );
    assert_eq!(
        first_existing_bundle_candidate(&mac_candidates).unwrap(),
        mac_authoritative,
        "macOS 两套并存时不得命中旧 payload"
    );
}

/// 平台标签必须是 Node 约定（config-engine 的平台分支按此比较），不是 Rust 的 consts::OS。
#[test]
fn platform_tag_uses_node_convention() {
    let t = platform_tag();
    assert!(
        matches!(t, "linux" | "darwin" | "win32"),
        "platform_tag 必须映射为 Node 约定，得到 {t}"
    );
    // 绝不能把 Rust 名直接漏出去（漏了 config-engine 的 win32/darwin 分支会全落空）。
    assert_ne!(t, "macos");
    assert_ne!(t, "windows");
}

/// `cronet_available` 逐核形态：naive 可用性判定。**三种核形态各占一行**（不是「四象限」——
/// 那个名字在 Android 进来之后就是句谎话，覆盖面从 6 条长到 8 条、平台从 3 个长到 4 个）：
/// 独立可执行 + 静态编入（darwin）、进程内 `.so` + 静态编入（android）、独立可执行 + 动态库（linux/win32）。
///
/// 累积式断言（不短路）便于变异验证——把 `core_has_builtin_cronet` 改回 `platform == "darwin"`，
/// android 两条**同时**列入失败而 darwin/linux/win32 六条恒绿，失败清单直接指出丢的是哪一支。
#[test]
fn cronet_available_across_core_forms() {
    // (lib_exists, platform, arch, expected, label)
    let cases = [
        // Android：核是进程内 libbox.so（**没有核二进制** ⇒ lib_exists 恒 false），cronet 静态在里面。
        // 这两条是本批修的真机缺陷本体：漏了它们 → has_cronet=false → 全部 naive 节点被静默剔除。
        (
            false,
            "android",
            "aarch64",
            true,
            "android/arm64 静态编入 libbox.so → 无落盘动态库也须 true",
        ),
        (
            false,
            "android",
            "x86_64",
            true,
            "android/x86_64 同一份 aar → 无落盘动态库也须 true",
        ),
        // macOS 两架构 cronet 静态编入内核 → 无 dylib 也可用（真机 bug 修复的核心断言）。
        (
            false,
            "darwin",
            "aarch64",
            true,
            "mac-arm64 静态编入 → 无 dylib 也须 true",
        ),
        (
            false,
            "darwin",
            "x86_64",
            true,
            "mac-x64 也静态编入 → 无 dylib 也须 true",
        ),
        // linux/win 按 libcronet 动态库落盘。
        (true, "linux", "x86_64", true, "linux 有 libcronet → true"),
        (
            false,
            "linux",
            "x86_64",
            false,
            "linux 无 libcronet → false",
        ),
        (true, "win32", "x86_64", true, "Windows 有 libcronet → true"),
        (
            false,
            "win32",
            "x86_64",
            false,
            "Windows 无 libcronet → false",
        ),
        // iOS：本仓今天构不出 iOS 产物 ⇒ 核里有没有 cronet 没有事实可查 ⇒ 谓词答 false。
        // 这两条钉的是**「这个 false 是答过的」**：把 `core_has_builtin_cronet` 的 `"ios"` 臂
        // 翻成 true（或并进 darwin/android 那条），两条同时红。
        //
        // 第二条（lib_exists=true）不是凑数：它证明 `cronet_available` 的**并集语义**在 iOS 上
        // 仍然成立 —— 将来若 iOS 改走动态库形态，探测到库就该 true，那时该改的是探测腿而不是
        // 本谓词。少了它，「把 cronet_available 写成 `core_has_builtin_cronet(platform)` 单条」
        // 这种退化在 iOS 这一格上不会被抓到。
        (
            false,
            "ios",
            "aarch64",
            false,
            "iOS 无核产物 + 无落盘动态库 → false（具名的 false，见 core_has_builtin_cronet §ios）",
        ),
        (
            true,
            "ios",
            "aarch64",
            true,
            "iOS 若探测到落盘 cronet 动态库 → true（并集语义不因平台而失效）",
        ),
    ];
    let mut fails = Vec::new();
    for (lib, plat, arch, want, label) in cases {
        let got = cronet_available(lib, plat, arch);
        if got != want {
            fails.push(format!("{label}（期望 {want}，得到 {got}）"));
        }
    }
    assert!(
        fails.is_empty(),
        "cronet_available 逐核形态失败:\n  {}",
        fails.join("\n  ")
    );
}

#[test]
fn cronet_probe_follows_the_actual_core_directory() {
    let dir = TestDir::new("polaris-cronet-probe-");
    let core = dir.join("sing-box.exe");
    std::fs::write(&core, b"CORE").unwrap();

    assert!(!cronet_lib_exists_beside_core(&core, "windows"));
    std::fs::write(dir.join("libcronet.dll"), b"CRONET").unwrap();
    assert!(
        cronet_lib_exists_beside_core(&core, "windows"),
        "Windows 必须按打包名 libcronet.dll 且只在实际核心同目录探测"
    );
    assert!(
        !cronet_lib_exists_beside_core(&dir.join("other/sing-box.exe"), "windows"),
        "配置根目录里有 DLL 不能替另一个核心目录冒充依赖可用"
    );
}

#[test]
fn is_valid_srs_file_checks_magic() {
    let dir = TestDir::new("polaris-srs-test-");
    let good = dir.join("good.srs");
    std::fs::write(&good, b"SRS\x01\x02").unwrap();
    assert!(is_valid_srs_file(good.to_str().unwrap()));

    let bad = dir.join("bad.srs");
    std::fs::write(&bad, b"XXX\x01").unwrap();
    assert!(!is_valid_srs_file(bad.to_str().unwrap()));

    // 短于 3 字节 → false（read_exact 失败）。
    let short = dir.join("short.srs");
    std::fs::write(&short, b"SR").unwrap();
    assert!(!is_valid_srs_file(short.to_str().unwrap()));

    // 不存在 → false，不 panic。
    assert!(!is_valid_srs_file(dir.join("nope.srs").to_str().unwrap()));
}

/// **C12 只读 smoke**：`enumerate_own_lan_cidrs` 真枚举本机接口（unix `getifaddrs` / Windows
/// `GetAdaptersAddresses`，二者皆只读、非破坏性）—— 断言**格式**不变式（每项合法 CIDR、含 `/`、
/// 非回环、去重），**不**断言具体网段（随宿主网络变，会 flaky）。允许空集（容器/无接口环境）。
/// 打断枚举（如漏滤回环 / 不 dedupe / Windows 侧漏了 `prefix_is_valid` 让哨兵 255 混进来）→ 本测转红。
///
/// **cfg 从 `unix` 放宽到 `any(unix, windows)`**：Windows 腿此前是恒空 stub、无可测；现在它走真枚举，
/// 同一组格式不变式必须同样守住（本机跑不到，但 Windows CI/真机跑到的就是这条）。
#[cfg(any(unix, windows))]
#[test]
fn enumerate_own_lan_cidrs_yields_valid_non_loopback_cidrs() {
    use std::collections::HashSet;
    let cidrs = enumerate_own_lan_cidrs();
    let mut seen = HashSet::new();
    for c in &cidrs {
        // 形态：`addr/prefix`（含主机位）。
        let (addr, prefix) = c
            .rsplit_once('/')
            .unwrap_or_else(|| panic!("每项须为 CIDR 形态（含 /），实得: {c}"));
        assert!(!addr.is_empty(), "地址段非空: {c}");
        // prefix 是合法数字（v4 ≤32 / v6 ≤128）。
        let p: u32 = prefix
            .parse()
            .unwrap_or_else(|_| panic!("前缀须为数字: {c}"));
        let max = if addr.contains(':') { 128 } else { 32 };
        assert!(p <= max, "前缀越界: {c}");
        // 非回环（滤回环生效）。
        assert_ne!(addr, "127.0.0.1", "回环须被剔除: {c}");
        assert_ne!(addr, "::1", "回环须被剔除: {c}");
        assert!(!addr.starts_with("127."), "127/8 回环段须被剔除: {c}");
        // 去重生效（dedupe_own_lan）。
        assert!(seen.insert(c.clone()), "枚举结果须去重，实得重复项: {c}");
    }
}

/// **产物级**断言：Android 上 naive 节点确实被构进生成出的 config，而不只是 `has_cronet` 为真。
///
/// 为什么不判中间变量：`has_cronet=true` 只是**必要条件**。它与「配置里真有一个 naive 出站」之间
/// 还隔着两道各自独立的门 —— `generate::is_node_usable`（选中节点校验，不过就整次生成 Err）与
/// `outbounds.rs:331` 的 `!deps.has_cronet_lib` 跳过腿。任一处另有平台判据，谓词改对了照样剔。
/// 故判据取**生成出来的 JSON**：`outbounds[].type == "naive"` 在场。
///
/// 反向对照同批跑（`has_cronet=false`）：同一份输入必须**生成失败**（选中的就是那个 naive 节点）。
/// 没有这一半，本门会被「naive 恒在场」之类的恒真实现骗过。
#[test]
fn android_config_actually_contains_the_naive_outbound() {
    use polaris_config_engine::builder::{generate_sing_box_config, GenerateConfigDeps};
    use polaris_config_engine::user_config::app_config::UserConfig;

    fn deps_for(platform: &str, has_cronet: bool) -> GenerateConfigDeps {
        GenerateConfigDeps {
            platform: platform.to_string(),
            arch: "aarch64".into(),
            race_server_port: 0,
            probe_direct_port: None,
            probe_proxy_port: None,
            debug_probe_mixed_udp: false,
            update_in_port: None,
            subscription_update_in_port: None,
            loopback_auth: None,
            network_canary_port: None,
            probe_pool_ports: vec![],
            lan_resolver_for_dns: None,
            race_upstream_ips: vec![],
            race_upstream_ports: vec![],
            has_cronet,
            cronet_copy_failed: false,
            has_management_api: true,
            privacy_mode: false,
            log_level: polaris_config_engine::user_config::LogLevel::Info,
            disable_log_file: false,
            dashboard_serve_dir: None,
            tailscale_api_port: 0,
            cache_path: "/fake/userData/cache.db".into(),
            log_file_path: None,
            runtime_rules_dir: "/fake/userData/rules".into(),
            rule_resources_path: "/fake/userData/rule-resource".into(),
            custom_rules_dir: "/fake/userData/custom-rules".into(),
            tailscale_state_dir_prefix: "/fake/userData/tailscale".into(),
            is_valid_srs_fn: |p| p.ends_with(".srs"),
            own_lan_cidrs: vec![],
            system_dns_takeover_active: false,
            netenv_dhcp_suppressed: false,
            // 本门问的是「Android 的配置里真有那个 naive 出站没有」，与组网段结算无关：
            // 两条新入参都显式给空值，目录指一个不存在的路径（免得结果取决于跑它那台机器上有什么文件）。
            tailnet_rules_dir: "/nonexistent/polaris-tailnet-rules".into(),
            observed_tailnet_addresses: Default::default(),
            log: |_, _| {},
            on_degraded: || {},
        }
    }

    let raw = serde_json::json!({
        "servers": [
            { "id": "nv", "name": "NAIVE", "protocol": "naive",
              "address": "nv.example.com", "port": 443,
              "naiveSettings": { "username": "u", "password": "p" } }
        ],
        "selectedServerId": "nv",
        "proxyMode": "smart",
        "proxyModeType": "tun"
    });
    let config: UserConfig = serde_json::from_value(raw).expect("UserConfig 反序列化");
    let resolved = std::collections::BTreeMap::new();

    // 正面：Android 的 has_cronet 必须由**真实谓词**算出（不是硬编码 true），且产物里有 naive 出站。
    // `cronet_lib_exists_for_start()` 在 Android 上恒 false（没有核二进制可解析），故第一参传 false。
    let android_has_cronet = cronet_available(false, "android", "aarch64");
    assert!(
        android_has_cronet,
        "Android 的核（libbox.so）静态编入 cronet，谓词必须判 true"
    );
    let cfg =
        generate_sing_box_config(&config, &resolved, &deps_for("android", android_has_cronet))
            .expect("Android + naive 选中节点必须生成成功");
    let v = serde_json::to_value(&cfg).expect("序列化");
    let naive_count = v["outbounds"]
        .as_array()
        .expect("outbounds 必须是数组")
        .iter()
        .filter(|o| o.get("type").and_then(serde_json::Value::as_str) == Some("naive"))
        .count();
    assert_eq!(
        naive_count, 1,
        "Android 生成出的 config 必须含恰好一个 naive 出站，实得 {naive_count}；outbounds={}",
        v["outbounds"]
    );

    // 反向对照：谓词若判 false，同一份输入必须**生成失败**——证明上面那条绿不是恒真。
    let neg = generate_sing_box_config(&config, &resolved, &deps_for("android", false));
    assert!(
        neg.is_err(),
        "has_cronet=false 时选中的 naive 节点必须不可用（否则本门无杀伤力）"
    );
}
