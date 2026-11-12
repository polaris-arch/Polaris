//! 门：来源只限当前安装包的 compiled identity 及其受信恢复数据副本。
//! 不得重新长出任意上传、外部路径或在线换核的入口；恢复数据不是执行许可。
//!
//! # 守的是什么
//!
//! 应用应当运行与本应用版本配套的内核。本门守的是**路径来源**这一层（内容层面的配套校验
//! 不在这里）。能把另一个内核文件交给应用执行的入口，
//! 历史上有三类：手动上传、应用内在线更新（连同回滚 / 恢复出厂 / 自动更新）、以及「设置里
//! 填一个内核路径」。前两类已整体删除，第三类从未存在。本门以结构判据钉住它们不被悄悄加回来：
//!
//! 1. [`release_core_resolution_has_exactly_one_leg`]：release 构型下 `resolve_core_binary`
//!    的函数体就是「调随包解析」这一句，没有第二条腿；
//! 2. [`core_path_has_one_source`]：应用 crate 里能拿到内核路径的地方是一张登记表 ——
//!    `resolve_core_binary` 的每个引用点、以及内核文件名（定位一个内核文件绕不开它）的每个
//!    出现点，都逐文件登记；新出现一处即红；
//! 3. [`no_setting_points_at_a_core_binary`]：用户配置的键集里不得出现指向内核文件 / 目录的键。
//!
//! 环境变量那条入口由 `release_escape_hatches.rs` 的 `core_path_env_override_is_dev_only` 守。
//!
//! # 失效形态
//!
//! 这类入口「加回来」没有任何运行期表征：不点它、不填它时行为完全正常，单测全绿。
//! 它只在被使用的那一刻让一个与应用不配套的内核跑起来，而那一刻不在 CI 里。
//!
//! # 不在射程内
//!
//! `crates/helper`、`crates/helper-proto`、`crates/helper-client` 与应用侧向提权助手递送内核的
//! 那条腿（`install-core`）：它递送的是随包内核，收口另有安排，不归本门。
// 源码文本门：三平台源码逐字一致（`.gitattributes` eol=lf），只在必跑的 Linux 腿执行一次。
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    polaris_source_probe::workspace_root_from(env!("CARGO_MANIFEST_DIR"))
}

/// `dir` 下全部生产源文件（跳过 `tests/` 目录、`node_modules`、构建产物与测试文件）。
fn production_files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|err| panic!("读不到 `{}`（{err}）", dir.display()));
    for entry in entries {
        let path = entry.expect("读目录项失败").path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_dir() {
            if !matches!(
                name.as_str(),
                "tests" | "node_modules" | "target" | "dist" | "gen"
            ) {
                production_files(&path, exts, out);
            }
        } else if exts.iter().any(|ext| name.ends_with(ext))
            && !name.contains(".test.")
            && !name.contains(".test-support.")
        {
            out.push(path);
        }
    }
}

/// `needle` 作为**完整标识**出现的位置（两侧都不是标识符字符）。
fn identifier_hits(text: &str, needle: &str) -> usize {
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = text.as_bytes();
    let mut count = 0;
    let mut from = 0;
    while let Some(offset) = text[from..].find(needle) {
        let at = from + offset;
        let end = at + needle.len();
        let left_ok = at == 0 || !is_ident(bytes[at - 1]);
        let right_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if left_ok && right_ok {
            count += 1;
        }
        from = end;
    }
    count
}

/// 把一个键拆成小写的词：`camelCase` / `snake_case` / `kebab-case` 的词界，数字并入前一个词。
fn key_words(key: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in key.chars() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        current.push(ch.to_ascii_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// 一个设置键是否在说「内核的文件 / 目录在哪」。
///
/// 按**整词**判，不按子串：键里要同时有一个指内核的词与一个指文件系统位置的词。
/// 于是 `disableLogFile`（没有内核词）、`scoreFilter`（`core` 只是 `score` 的一部分）、
/// `singboxDashboard`（没有位置词）都不算。`sing` + `box` 相邻视同 `singbox`。
fn names_a_core_location(key: &str) -> bool {
    let words = key_words(key);
    let subject = words
        .iter()
        .any(|word| matches!(word.as_str(), "core" | "singbox" | "kernel"))
        || words
            .windows(2)
            .any(|pair| pair[0] == "sing" && pair[1] == "box");
    let location = words.iter().any(|word| {
        matches!(
            word.as_str(),
            "path"
                | "binary"
                | "bin"
                | "executable"
                | "exe"
                | "file"
                | "dir"
                | "directory"
                | "location"
        )
    });
    subject && location
}

/// `interface UserConfig { … }` 的字段名（前端配置合同的键集）。
fn user_config_keys(types_ts: &str) -> Vec<String> {
    let start = types_ts
        .find("export interface UserConfig {")
        .expect("ui/src/contracts/types.ts 里没有 `export interface UserConfig {`");
    let body = &types_ts[start..];
    let end = body.find("\n}\n").expect("UserConfig 接口没有闭合");
    body[..end]
        .lines()
        .skip(1)
        .filter_map(|line| {
            // 顶层字段恰好缩进两格；嵌套对象里的字段缩进更深，不是配置的顶层键。
            let field = line.strip_prefix("  ")?;
            if field.starts_with([' ', '/', '*']) {
                return None;
            }
            let name: String = field
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let rest = &field[name.len()..];
            (!name.is_empty() && (rest.starts_with(':') || rest.starts_with("?:"))).then_some(name)
        })
        .collect()
}

/// 源码里全部形如 `"key"` 的标识符样字符串字面量（配置默认值与清洗表的键都长这样）。
fn quoted_identifiers(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let literal = &after[..close];
        if !literal.is_empty()
            && literal
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            out.push(literal.to_owned());
        }
        rest = &after[close + 1..];
    }
    out
}

/// 🔴 用户配置里不得有指向内核文件 / 目录的键。
///
/// 键集取自三处：前端配置合同（`UserConfig` 的顶层字段）、配置默认值与配置清洗表
/// （`crates/store/src/store.rs`、`sanitize.rs` 里的键字面量）。
///
/// **变异探针**：给 `UserConfig` 加一个字段 `singboxPath?: string;` ⇒ 本条转红并点名它；
/// 给 `sanitize.rs` 加一行 `string_or_remove(obj, "coreBinaryPath", true, true);` ⇒ 同样转红。
#[test]
fn no_setting_points_at_a_core_binary() {
    // 判据自检（正反两面）：认得出该拦的，也不误伤只提到内核的键。
    for bad in [
        "singboxPath",
        "coreBinaryPath",
        "core_dir",
        "kernelExecutable",
        "customCoreFile",
        "singBoxExe",
    ] {
        assert!(names_a_core_location(bad), "判据漏掉了 `{bad}`");
    }
    assert_eq!(key_words("singBoxExe"), ["sing", "box", "exe"]);
    assert_eq!(key_words("core_dir"), ["core", "dir"]);
    for fine in [
        "singboxDashboard",
        "singboxDashboardUrl",
        "logLevel",
        "ghProxyPrefix",
        "disableLogFile",
        "appUpdateChannel",
        // 子串里碰巧有 core / file / exe / dir，但不是整词。
        "scoreFilter",
        "profileDirection",
        "encoreExecutor",
        "redirectPort",
    ] {
        assert!(!names_a_core_location(fine), "判据误伤了 `{fine}`");
    }

    let root = env!("CARGO_MANIFEST_DIR");
    let types_ts = polaris_source_probe::repo_file_in(root, "ui/src/contracts/types.ts");
    let contract_keys = user_config_keys(&types_ts);
    // 取材面自检：接口改形状 / 切片跑偏时键集会塌成很小的一撮，否定断言随之平凡成立。
    assert!(
        contract_keys.len() >= 60,
        "UserConfig 只读出 {} 个顶层键 —— 切片跑偏了",
        contract_keys.len()
    );
    for expected in [
        "servers",
        "selectedServerId",
        "language",
        "singboxDashboard",
    ] {
        assert!(
            contract_keys.iter().any(|key| key == expected),
            "UserConfig 的键集里没有 `{expected}` —— 切片跑偏了"
        );
    }

    let mut surfaces = vec![("ui/src/contracts/types.ts", contract_keys)];
    for rel in ["crates/store/src/store.rs", "crates/store/src/sanitize.rs"] {
        let keys = quoted_identifiers(&polaris_source_probe::repo_file_in(root, rel));
        assert!(
            keys.iter()
                .any(|key| key == "ghProxyPrefix" || key == "language"),
            "`{rel}` 里没读出任何已知的配置键 —— 取材跑偏了"
        );
        surfaces.push((rel, keys));
    }

    let mut offenders = Vec::new();
    for (rel, keys) in &surfaces {
        for key in keys {
            if names_a_core_location(key) {
                offenders.push(format!("    {rel} · `{key}`"));
            }
        }
    }
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "用户配置里出现了指向内核文件 / 目录的键：\n{}\n\n\
         应用只选择当前安装包身份或其受信恢复数据。设置里一旦能填内核的位置，任何一份配置（导入的备份、\
         被改写的 config.json）都能让应用去执行一个与它不配套的内核。",
        offenders.join("\n")
    );
}

// ============================================================================
// 结构判据
// ============================================================================

/// 应用 crate 的生产源码：`(crate 内相对路径, 剥掉注释的文本)`。
fn app_crate_code() -> Vec<(String, String)> {
    let root = repo_root().join("src-tauri/src");
    let mut files = Vec::new();
    production_files(&root, &[".rs"], &mut files);
    files
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("读不到 `{}`（{err}）", path.display()));
            (rel, polaris_source_probe::mask_comments(&text))
        })
        .collect()
}

/// `signature` 起、到与其后第一个 `{` 配对的 `}` 止的函数体（含花括号）。
fn fn_body<'a>(code: &'a str, signature: &str) -> &'a str {
    let start = code
        .find(signature)
        .unwrap_or_else(|| panic!("找不到 `{signature}` —— 守卫已失去判据"));
    let open = start + code[start..].find('{').expect("签名之后没有函数体");
    let mut depth = 0usize;
    for (offset, byte) in code[open..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &code[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{signature}` 的花括号不配对");
}

/// 去掉 `body` 里被 `#[cfg(any(debug_assertions, test))]` 罩住的块（release 构型不编译）。
fn without_dev_only_blocks(body: &str) -> String {
    const DEV_ONLY: &str = "#[cfg(any(debug_assertions, test))]";
    let mut out = String::new();
    let mut rest = body;
    while let Some(at) = rest.find(DEV_ONLY) {
        out.push_str(&rest[..at]);
        let after = &rest[at + DEV_ONLY.len()..];
        let open = after.find('{').expect("dev-only 属性之后没有块");
        assert!(
            after[..open].trim().is_empty(),
            "dev-only 属性罩的不是一个裸块 —— 本切片不认识这个形态"
        );
        let mut depth = 0usize;
        let mut end = None;
        for (offset, byte) in after[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &after[end.expect("dev-only 块不闭合")..];
    }
    out.push_str(rest);
    out
}

/// 🔴 release 构型下，内核解析只有随包这一条腿。
///
/// 判据是函数体本身：去掉 release 不编译的块之后，`resolve_core_binary` 的函数体**恰好**是
/// 一句 `resolve_bundled_core_binary()`。任何第二条腿（用户目录、配置字段、`PATH`、另起名字的
/// 解析函数）都得往这个函数体里加东西，或者另开一个函数 —— 后者由 [`core_path_has_one_source`] 管。
/// 随包解析自身的候选只能来自随包资源布局，不读进程环境、不读配置根。
///
/// **变异探针**：在 `resolve_bundled_core_binary()` 之前加一句
/// `if let Some(core) = some_other_place() { return Ok(core); }` ⇒ 第 1 条转红；把 dev-only 块的
/// `cfg` 改成 `cfg(unix)` ⇒ 第 1 条转红（那一块不再被切掉）；在随包解析里读 `std::env::var`
/// 或 `core_paths::base_dir()` ⇒ 第 2 条转红。
#[test]
fn release_core_resolution_has_exactly_one_leg() {
    let raw = polaris_source_probe::repo_file_in(
        env!("CARGO_MANIFEST_DIR"),
        "src-tauri/src/runtime/proxy/core_binary.rs",
    );
    let code = polaris_source_probe::mask_comments(&raw);

    let body = fn_body(
        &code,
        "pub(crate) fn resolve_core_binary() -> Result<PathBuf, String>",
    );
    // 切片自检：dev-only 块确实在，且确实被切掉了。
    assert!(body.contains("dev_core_binary_override()"));
    let release: String = without_dev_only_blocks(body)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert_eq!(
        release, "{resolve_bundled_core_binary()}",
        "release 构型下 resolve_core_binary 的函数体不再只是「调随包解析」"
    );

    let bundled = fn_body(
        &code,
        "fn resolve_bundled_core_binary() -> Result<PathBuf, String>",
    );
    assert!(
        bundled.contains("bundle_resource_candidates("),
        "切片自检：随包解析应从随包资源布局取候选"
    );
    for forbidden in [
        "env::var",
        "core_paths::",
        "base_dir(",
        "config_dir",
        "home_dir",
    ] {
        assert!(
            !bundled.contains(forbidden),
            "随包解析里出现了 `{forbidden}` —— 候选不再只来自安装包"
        );
    }
}

/// 应用 crate 里**能拿到内核路径**的位置：`(文件, 标识, 次数, 用途)`。
///
/// 两类标识：
/// - `resolve_core_binary`：执行入口的共同解析门。起核、起核前 `check`、测速临时核、Tailscale
///   登录核、管理员安装暂存，全部经它拿路径；
/// - 内核文件名：`core_filename()` / `core_filename_for()` 的调用，以及字面量 `"sing-box"` /
///   `"sing-box.exe"`。不经解析函数而自己定位一个内核文件，绕不开文件名。
///
/// 定义处（`runtime/proxy/core_binary.rs`、`runtime/core_paths.rs`）整文件不计。
const CORE_PATH_SITES: &[(&str, &str, usize, &str)] = &[
    (
        "native_verification/proxy_carrier.rs",
        "\"sing-box\"",
        1,
        "Private acceptance process census; this basename never supplies a core path",
    ),
    (
        "native_verification/proxy_carrier.rs",
        "resolve_core_binary",
        1,
        "Direct acceptance uses the normal compiled bundle admission",
    ),
    (
        "runtime/proxy/tls_capabilities.rs",
        "\"sing-box\"",
        1,
        "Compiled manifest basename binds TLS capabilities to an authenticated live birth",
    ),
    (
        "runtime/proxy/tls_capabilities.rs",
        "\"sing-box.exe\"",
        1,
        "Windows compiled manifest basename binds TLS capabilities to an authenticated live birth",
    ),
    (
        "runtime/helper.rs",
        "core_filename(",
        1,
        "Selected manifest whitelist names staged installer bytes resolved from the bundle",
    ),
    (
        "runtime/core_identity.rs",
        "\"sing-box\"",
        2,
        "Compiled selection binding, selected data and verified installer permission",
    ),
    (
        "runtime/core_identity.rs",
        "\"sing-box.exe\"",
        1,
        "Compiled selection data binding on Windows, not an execution provider",
    ),
    (
        "runtime/core_identity/recovery.rs",
        "\"sing-box\"",
        1,
        "Strict compiled-member archive stage, not renderer-supplied payload",
    ),
    (
        "runtime/core_identity/recovery.rs",
        "\"sing-box.exe\"",
        1,
        "Same strict compiled-member archive stage on Windows",
    ),
    (
        "runtime/core_identity/user_store.rs",
        "\"sing-box\"",
        1,
        "Exact compiled-ID persisted data; purpose/platform admission remains separate",
    ),
    (
        "runtime/core_identity/user_store.rs",
        "\"sing-box.exe\"",
        1,
        "Same exact compiled-ID data candidate on Windows",
    ),
    (
        "runtime/core_identity/user_store/windows_native_tests.rs",
        "\"sing-box.exe\"",
        2,
        "Only cfg(all(test, windows)) actual data fixture; module guard checked below",
    ),
    (
        "commands/proxy.rs",
        "resolve_core_binary",
        2,
        "节点探测与规则正则准入的 check 均使用 resolve_core_binary 给出的随包核",
    ),
    (
        "runtime/proxy.rs",
        "resolve_core_binary",
        1,
        "façade 再导出",
    ),
    (
        "runtime/proxy/startup.rs",
        "resolve_core_binary",
        2,
        "起核：导入 + `core_binary_for_start`",
    ),
    (
        "runtime/speedtest.rs",
        "resolve_core_binary",
        1,
        "测速临时核",
    ),
    (
        "runtime/tailscale_login_core.rs",
        "resolve_core_binary",
        2,
        "Tailscale 登录核：导入 + 注入",
    ),
    ("logging.rs", "\"sing-box\"", 2, "日志来源名，不是路径"),
    (
        "runtime/proxy/process_supervision.rs",
        "core_filename(",
        1,
        "孤儿核清扫按文件名认进程",
    ),
];

/// 🔴 内核路径只有一个来源：解析函数与内核文件名的每个出现点都在登记表里。
///
/// 这是对「换个名字重做」的结构判据：新写一个从别处找内核的函数，要么调 `resolve_core_binary`
/// （新调用点 ⇒ 红，过目它拿路径去做什么），要么自己拼出内核文件名（新出现点 ⇒ 红）。
/// 表里的条目对不上源码（少了、多了）同样红，登记不会静默失效。
///
/// **变异探针**：在 `runtime/geo_seed.rs` 里加一个
/// `fn pick_core(dir: &Path) -> PathBuf { dir.join("sing-box") }` ⇒ 红并点名该文件；在任意未登记
/// 文件里调 `core_paths::core_filename()` 或 `resolve_core_binary()` ⇒ 同样红。
#[test]
fn core_path_has_one_source() {
    const DEFINITIONS: [&str; 2] = ["runtime/proxy/core_binary.rs", "runtime/core_paths.rs"];
    const NEEDLES: [&str; 5] = [
        "resolve_core_binary",
        "core_filename_for(",
        "core_filename(",
        "\"sing-box\"",
        "\"sing-box.exe\"",
    ];
    let files = app_crate_code();
    // Scan this exact fixture rather than broadly skipping *_tests.rs. Removing
    // its test-only module cfg must make the source gate fail.
    let (_, user_store) = files
        .iter()
        .find(|(rel, _)| rel == "runtime/core_identity/user_store.rs")
        .expect("selected data source missing");
    let compact: String = user_store.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("#[cfg(all(test,windows))]modwindows_native_tests;"),
        "Windows fixture must remain guarded by cfg(all(test, windows))"
    );
    assert!(
        files.len() >= 120,
        "只扫到 {} 个文件 —— 取材面塌了",
        files.len()
    );
    for definition in DEFINITIONS {
        assert!(
            files.iter().any(|(rel, _)| rel == definition),
            "取材面里没有定义处 `{definition}`"
        );
    }

    // 字面量按原样数；函数按「调用形态」数（`name(`，左侧不是标识符字符），同名的局部变量与
    // 形参不算；解析函数连同不带括号的引用（当作函数值传递）一起数。
    let count = |text: &str, needle: &str| -> usize {
        if needle.starts_with('"') {
            text.matches(needle).count()
        } else if let Some(name) = needle.strip_suffix('(') {
            text.match_indices(needle)
                .filter(|(at, _)| {
                    *at == 0 || {
                        let before = text.as_bytes()[at - 1];
                        !(before.is_ascii_alphanumeric() || before == b'_')
                    }
                })
                .count()
                .min(identifier_hits(text, name))
        } else {
            identifier_hits(text, needle)
        }
    };
    let mut found: Vec<(String, &str, usize)> = Vec::new();
    for (rel, text) in &files {
        if DEFINITIONS.contains(&rel.as_str()) {
            continue;
        }
        for needle in NEEDLES {
            let hits = count(text, needle);
            if hits > 0 {
                found.push((rel.clone(), needle, hits));
            }
        }
    }
    found.sort();
    let mut registered: Vec<(String, &str, usize)> = CORE_PATH_SITES
        .iter()
        .map(|(rel, needle, hits, _)| ((*rel).to_owned(), *needle, *hits))
        .collect();
    registered.sort();
    assert_eq!(
        found, registered,
        "\n内核路径来源的出现点与登记表不一致（左：源码实测；右：登记）。\n\
         新增出现点：先确认它拿到的是 `resolve_core_binary` 给的随包核路径，再登记用途；\n\
         任意外部路径、上传或在线换核来源不许登记。"
    );
}
