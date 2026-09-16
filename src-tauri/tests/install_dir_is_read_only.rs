//! 门：**安装目录只读** —— 凡是把 app 自身安装目录当路径根用的地方，都必须是只读消费。
//!
//! # 守的是什么（根因）
//!
//! 安装目录的**可写性随装机形态翻转**，而翻转前后源码一个字都不用改 —— 这正是它危险的地方：
//!
//! - 当前形态（`installMode: currentUser`，app 落 `%LOCALAPPDATA%\Polaris`）：安装目录对当前用户
//!   完全可写 ⇒ 往随包资源目录里写一个缓存文件、补一份下载产物、就地改一个 `.srs`，都会
//!   「正常工作」，本机、CI、打包态**都看不出问题**。
//! - 若改成 `perMachine`（app 落 `%PROGRAMFILES%\Polaris`，Users 只读）：同一次写在标准用户账户下
//!   返回 `ACCESS_DENIED`，**只在真机运行期才炸**，而且多半以「功能静默失效」而不是
//!   「启动失败」的形态出现。
//!
//! 所以本门与 `installMode` 取什么值**无关**，它守的是「安装目录只读消费」这条不变量本身：
//! 现在守住它，是把未来那次翻转的代价从「真机上逐个功能试出来」压成零；而它同时也是那次翻转的
//! **前置条件** —— 不先有这道门，翻转之后没人知道哪些功能已经悄悄不工作了。
//!
//! 这类缺陷没有任何编译期或本机运行期表征（开发机上 app 跑在 `target/debug/`，那当然可写），
//! 所以判据只能落在**源码形态**上，且必须由代码持有 —— 写在文档里的「别往安装目录写」在下一次
//! 重构时不会自己变红。
//!
//! # 判据
//!
//! 安装目录这个路径根在 `src-tauri` 里只有一个入口：`std::env::current_exe()`，以及由它派生出
//! 随包资源根的三个纯函数（`bundle_resource_roots` / `bundle_resource_candidates` /
//! `first_existing_bundle_candidate`，都住 `runtime/proxy/core_binary.rs`）。于是：
//!
//! 1. 扫全部生产 `.rs`（取材面 = [`polaris_source_probe::module_files_in`]，已排除 `tests/`），
//!    在**剥掉注释与字面量**的定位面上找这四个探针的每一次出现；
//! 2. 每一次出现都必须落在 [`REGISTRY`] 登记的那个函数作用域里 —— 出现在别处即红，
//!    逼新增的入口当场被摆上台面回答「你要拿安装目录干什么」；
//! 3. 每个登记作用域内**不得出现任何写类调用形态**（[`WRITE_FORMS`]）。想在同一个函数里既解析
//!    安装目录又写盘，就得先把写那一半拆出去 —— 这正是本门要制造的摩擦。
//!
//! # 本门**不**声称什么（诚实边界）
//!
//! 它守的是「解析安装目录的函数自己不写」。把解析结果 `return` 出去、由**另一个**函数写进去，
//! 本门看不见（那需要跨函数数据流）。真正兜住那一格的是登记表本身：12 条里每一条都写明了产物
//! 交给谁、只读在哪一句上成立，新增入口必须在这里补一条同样的说明。

use std::collections::BTreeSet;

/// 安装目录路径根的入口形态。四个探针都取「带左括号的调用形态」，避免命中 `use` 导入与类型名。
const PROBES: [&str; 4] = [
    "std::env::current_exe(",
    "bundle_resource_roots(",
    "bundle_resource_candidates(",
    "first_existing_bundle_candidate(",
];

/// 写类调用形态。取材面偏宽是刻意的：多红（吵）可以当场讨论，漏红（瞎）没人会发现。
const WRITE_FORMS: [&str; 14] = [
    "fs::write",
    "fs::copy",
    "fs::rename",
    "fs::hard_link",
    "fs::set_permissions",
    "File::create",
    "OpenOptions",
    "write_all",
    "create_dir_all",
    "create_dir(",
    "remove_dir_all",
    "remove_dir(",
    "remove_file",
    "set_permissions",
];

/// 登记表：`(生产文件（相对 `src/`）, 函数名, 这里拿安装目录干什么 + 只读在哪一句上成立)`。
///
/// 每条都必须命中至少一个探针；命中 0 次 ⇒ 它守的东西已经没了，条目本身会变成将来某个真违规的
/// 免死金牌，故当场红。
const REGISTRY: [(&str, &str, &str); 12] = [
    (
        "commands/updater/app_update.rs",
        "update_check",
        "只为判断运行形态：exe 路径喂 `is_portable_layout`（读 exe 同级 `portable.marker` 是否存在）。\
         不写盘。",
    ),
    (
        "commands/updater/app_update.rs",
        "update_install",
        "把 exe 路径喂 `decide_install_plan`（判 portable/installed + 算便携覆盖目标）。写盘发生在\
         安装脚本里、目标是下载目录与便携目录，不是安装目录。",
    ),
    (
        "commands/updater/uninstall.rs",
        "app_uninstall_all",
        "exe 路径进 `SystemUninstallOps.exe`，供 `plan_app_removal` 定位应用本体；Windows 腿只**拉起**\
         `uninstall.exe`（运行中的 exe 删不掉自己），本函数自己不删不写。",
    ),
    (
        "runtime/env_trust.rs",
        "trusted_roots",
        "把随包资源根取来做 containment 判据（逃生门给的路径是否落在可信根内）。纯比较，不碰盘。",
    ),
    (
        "runtime/geo_seed.rs",
        "bundled_data_candidates",
        "解析随包 `resources/data/` 作为 `.srs` 播种的**源**；落点是 `<userData>/rules/`（见 `seed_one`），\
         方向恒为 安装目录 → 用户目录。",
    ),
    (
        "runtime/helper.rs",
        "resolve_helper_binary",
        "解析随包 `polaris-helper`，作为提权安装链的**源**（`InstallParams::src_binary`）。安装脚本以\
         提权身份读它、拷到特权落点。当前形态（currentUser）下这个源对同账户普通进程**可写** —— 那是\
         已登记的残留（见 `manager.rs` 的 `InstallParams::src_binary`）；本条登记的是「这里只读取路径，\
         不写盘」。",
    ),
    (
        "runtime/proxy/core_binary.rs",
        "bundle_resource_candidates",
        "纯函数：由 exe 目录拼候选路径表。只拼串，不 stat 不写。",
    ),
    (
        "runtime/proxy/core_binary.rs",
        "bundle_resource_roots",
        "纯函数：随包资源根的四种平台布局。只拼串。",
    ),
    (
        "runtime/proxy/core_binary.rs",
        "first_existing_bundle_candidate",
        "候选表里取第一个真实存在的：只 stat，不创建。",
    ),
    (
        "runtime/proxy/core_binary.rs",
        "resolve_bundled_core_binary",
        "解析随包出厂核，作为 reset-factory / reseed 的**源**；落点在用户可写核目录，不回写随包目录。",
    ),
    (
        "runtime/proxy/core_binary.rs",
        "resolve_dashboard_serve_dir",
        "解析随包 dashboard 目录交给内核 serve（只读 HTTP）。**运行时下载覆盖**那一档落\
         `<config_dir>/singbox-dashboard`，不是这里。",
    ),
    (
        "runtime/startup_tasks.rs",
        "spawn_auto_download",
        "下载前先用 exe 路径判运行形态（装不上的资产不下）。下载落点是 `app_cache_dir()/updates`。",
    ),
];

/// 生产取材面：`src-tauri/src` 下全部生产 `.rs`（相对路径, 全文），已排除 `tests/`。
fn production_files() -> Vec<(String, String)> {
    polaris_source_probe::module_files_in(env!("CARGO_MANIFEST_DIR"), "")
}

/// 在净化面上唯一定位 `fn <name>(` 并返回它的花括号作用域的**字节区间** `[start, end)`。
///
/// 返回区间而不是切片：调用方既要按区间判「探针落在谁身上」，又要按切片查写类形态，
/// 一个区间两处都够用，也省掉指针换算。
///
/// 锚点必须**唯一**：同名函数出现两次时切哪一个都是猜，当场红比猜对一半强。
fn fn_scope_range(masked: &str, name: &str, origin: &str) -> (usize, usize) {
    let anchor = format!("fn {name}(");
    let hits: Vec<_> = masked.match_indices(&anchor).collect();
    assert_eq!(
        hits.len(),
        1,
        "{origin}：锚点 `{anchor}` 必须恰好命中一次，实际 {} 次 —— \
         函数被改名/重载/挪走了，登记表的这一条已经指不准地方。",
        hits.len()
    );
    let start = hits[0].0;
    let open = masked[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("{origin}：`{anchor}` 之后找不到函数体左花括号"));
    let mut depth = 0usize;
    for (offset, byte) in masked.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1).expect("花括号深度不得下溢");
                if depth == 0 {
                    return (start, open + offset + 1);
                }
            }
            _ => {}
        }
    }
    panic!("{origin}：`{anchor}` 的函数体花括号未闭合");
}

/// 净化面上的作用域切片（`fn_scope_range` 的取片糖）。
fn fn_scope<'a>(masked: &'a str, name: &str, origin: &str) -> &'a str {
    let (start, end) = fn_scope_range(masked, name, origin);
    &masked[start..end]
}

/// 本文件在登记表里的全部作用域：`(函数名, start, end)`（净化面字节区间）。
fn registered_scopes(masked: &str, file: &str) -> Vec<(&'static str, usize, usize)> {
    REGISTRY
        .iter()
        .filter(|(f, _, _)| *f == file)
        .map(|(f, name, _)| {
            let (start, end) = fn_scope_range(masked, name, &format!("src/{f}"));
            (*name, start, end)
        })
        .collect()
}

/// 🔴 每一处安装目录入口都必须登记；登记表里的每一条都必须真命中。
#[test]
fn every_install_dir_entry_point_is_registered() {
    let files = production_files();
    assert!(
        files
            .iter()
            .any(|(rel, _)| rel == "runtime/proxy/core_binary.rs"),
        "取材面里没有 runtime/proxy/core_binary.rs（随包资源根的定义处）—— 选择器失效，本门恒绿。\
         实际扫到 {} 个生产文件。",
        files.len()
    );

    let mut hit_entries: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut unregistered: Vec<String> = Vec::new();
    let mut total_hits = 0usize;

    for (rel, raw) in &files {
        let masked = polaris_source_probe::mask_comments_and_strings(raw);
        let scopes = registered_scopes(&masked, rel);
        for probe in PROBES {
            for (at, _) in masked.match_indices(probe) {
                total_hits += 1;
                match scopes
                    .iter()
                    .find(|(_, start, end)| at >= *start && at < *end)
                {
                    Some((name, _, _)) => {
                        // `REGISTRY` 里的 `&'static str` 与这里的 `rel`（String）等值不同生命周期，
                        // 取回登记表里的那一份以便 BTreeSet 的键与登记表同源。
                        let entry = REGISTRY
                            .iter()
                            .find(|(f, n, _)| f == rel && n == name)
                            .expect("作用域来自登记表，必能反查");
                        hit_entries.insert((entry.0, entry.1));
                    }
                    None => {
                        let line = masked[..at].lines().count();
                        unregistered.push(format!("src/{rel}:{line} 处的 `{probe}`"));
                    }
                }
            }
        }
    }

    assert!(
        unregistered.is_empty(),
        "发现未登记的安装目录入口：\n  {}\n\
         安装目录是否可写随装机形态翻转，而翻转前后源码一个字都不用改（改 perMachine 后\
         `%PROGRAMFILES%\\Polaris` 对普通用户只读）。\
         新增入口请在 `REGISTRY` 里补一条，写明拿它干什么、只读在哪一句上成立；\
         若这个入口确实需要写盘，那就是缺陷本身 —— 改去写用户目录。",
        unregistered.join("\n  ")
    );

    let missing: Vec<_> = REGISTRY
        .iter()
        .filter(|(f, n, _)| !hit_entries.contains(&(*f, *n)))
        .map(|(f, n, _)| format!("src/{f}::{n}"))
        .collect();
    assert!(
        missing.is_empty(),
        "登记表里这些条目一个探针都没命中：{missing:?} —— 它守的东西已经不在了。\
         删掉这一条，别让它留着给将来某个真违规当免死金牌。"
    );

    assert!(
        total_hits >= REGISTRY.len(),
        "探针总命中 {total_hits} 次 < 登记表 {} 条 —— 探针形态与仓内写法对不上了（本门会恒绿）。",
        REGISTRY.len()
    );
}

/// 🔴 登记的作用域里不得出现写类调用 —— 「解析安装目录」与「写盘」不许住在同一个函数里。
#[test]
fn install_dir_resolvers_never_write() {
    let mut offenders: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (rel, raw) in production_files() {
        let masked = polaris_source_probe::mask_comments_and_strings(&raw);
        for (file, name, _) in REGISTRY.iter().filter(|(f, _, _)| *f == rel) {
            let scope = fn_scope(&masked, name, &format!("src/{file}"));
            checked += 1;
            for form in WRITE_FORMS {
                if scope.contains(form) {
                    offenders.push(format!("src/{file}::{name} 里出现 `{form}`"));
                }
            }
        }
    }
    assert_eq!(
        checked,
        REGISTRY.len(),
        "只检到 {checked} 个登记作用域，登记表有 {} 条 —— 取材面与登记表对不上，本门覆盖不全。",
        REGISTRY.len()
    );
    assert!(
        offenders.is_empty(),
        "解析安装目录的函数里出现了写类调用：\n  {}\n\
         一旦装机形态改成 perMachine，安装目录对普通用户只读，这类写只会在真机上 ACCESS_DENIED。\
         把写那一半拆成独立函数，并让它写用户目录（`<userData>` / `app_cache_dir()`）。",
        offenders.join("\n  ")
    );
}

/// 🔴 [`fn_scope`] 的切片自检：切出来的必须是**那一个函数**，不是整份文件。
///
/// 不做这条，上面两门在「切片退化成全文」时会一起失灵：探针恒落在作用域内（永远不报未登记），
/// 写类形态也恒命中（永远全红或全绿，取决于文件内容）—— 两种失效都比漏报更难看出来。
#[test]
fn fn_scope_is_scoped_to_one_function() {
    let raw = production_files()
        .into_iter()
        .find(|(rel, _)| rel == "runtime/proxy/core_binary.rs")
        .expect("取材面里必有 core_binary.rs")
        .1;
    let masked = polaris_source_probe::mask_comments_and_strings(&raw);

    let scope = fn_scope(&masked, "resolve_bundled_core_binary", "切片自检");
    assert!(
        scope.contains("std::env::current_exe("),
        "切片自检：`resolve_bundled_core_binary` 的作用域里应含它自己的 `current_exe()` 调用"
    );
    assert!(
        !scope.contains("fn resolve_dashboard_serve_dir("),
        "切片自检：切片吃进了相邻函数 `resolve_dashboard_serve_dir` —— 作用域退化成整份文件，\
         上面两门都会失灵。"
    );
    assert!(
        scope.len() < masked.len() / 2,
        "切片自检：单个函数作用域占了全文件 {} 字节里的 {} 字节，切点明显不对。",
        masked.len(),
        scope.len()
    );
}

/// 🔴 写类探测器的正向对照：喂一份**已知含写**的合成源，必须报得出来。
///
/// 只断言「生产代码里没有写」是典型的「什么都没发生也算通过」—— 探测器整个失灵时它照样绿。
#[test]
fn write_form_detector_catches_a_known_writer() {
    const FIXTURE: &str = r#"
fn resolves_and_writes() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("resources");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join("cache.bin"), b"x").ok()?;
    Some(dir)
}

fn resolves_only() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("resources"))
}
"#;
    let masked = polaris_source_probe::mask_comments_and_strings(FIXTURE);

    let writer = fn_scope(&masked, "resolves_and_writes", "写探测器夹具");
    let hit: Vec<_> = WRITE_FORMS
        .iter()
        .filter(|form| writer.contains(**form))
        .collect();
    assert!(
        hit.len() >= 2,
        "写探测器没抓到合成夹具里的 `create_dir_all` / `fs::write`（命中 {hit:?}）—— \
         探测器失灵，`install_dir_resolvers_never_write` 会恒绿。"
    );

    let reader = fn_scope(&masked, "resolves_only", "写探测器夹具");
    assert!(
        reader.contains("std::env::current_exe("),
        "反向对照：只读函数里应仍能看到探针形态（否则说明是切片瞎了，不是探测器对）"
    );
    assert!(
        WRITE_FORMS.iter().all(|form| !reader.contains(*form)),
        "反向对照：只读函数里不该有任何写类形态 —— 探测器在乱报，上面那门会恒红"
    );
}
