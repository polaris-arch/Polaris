//! **建窗点登记表 + 入口线程纪律**（issue #2：Windows 打开面板后白屏、整个应用关不掉）。
//!
//! # 守的不变量
//!
//! Windows 上在 WebView2 回调帧内（同步 `#[tauri::command]` 的执行帧、托盘 / WndProc 等 OS 消息
//! 分发帧）建 WebView 窗 ⇒ wry 开嵌套消息循环等一个不可重入的完成回调 ⇒ 永久死锁（W18）。tauri
//! 自己在 `WebviewWindowBuilder::new` 的 Known issues 里写明要用 async command。本仓 W18 真机修过
//! 主窗与托盘浮层两处，守卫只钉在那两个具体函数上 —— 于是第三处（`open_singbox_dashboard`，同步
//! command 里直接 `build()`）没有任何门看得见，直到用户报 issue。
//!
//! # 为什么是「登记表恰好相等」，而不是「同步 command 体内不许有 builder」
//!
//! 后者两个方向都错：**漏报**间接调用（command 调 helper、helper 里建窗）、事件回调入口、
//! `from_config` 形态；**误报**已经 `async_runtime::spawn` 包裹好的写法。本门换成两层：
//!
//! 1. [`window_build_sites_match_the_registry_exactly`]：`src-tauri/src` 全部生产 `.rs` 上的建窗
//!    构造点集合必须**恰好等于** [`REGISTRY`]。多一处 = 新建窗点没人裁定过入口线程（红）；少一处 =
//!    登记项已失效、它挂的纪律断言在守一个不存在的东西（红）。
//! 2. [`every_registered_build_site_keeps_its_entry_discipline`]：每个登记项钉死「构造点在哪个函数里」
//!    +「这个函数被谁直接调用」，再按项断言入口线程纪律。
//!
//! # 取材面（如实登记）
//!
//! - **文件**：`module_files("")` = `src-tauri/src/**.rs`，递归、剔除 `tests/`。
//! - **净化**：[`mask_comments_and_strings`]（注释与字面量抹成等长空格，偏移与行号守恒）——针是
//!   **符号**，注释 / 文档 / 错误串里提到 `WebviewWindowBuilder::new(` 都不是建窗点。
//! - **针**：[`BUILDER_NEEDLES`]，按标识符边界计数（`WebviewWindowBuilder::new(` 不会被
//!   `WindowBuilder::new(` 再算一次）。
//!
//! # 不在射程内（显式声明，不是遗漏）
//!
//! - `use tauri::WebviewWindowBuilder as B;` 这类**改名导入**后的 `B::new(`：词法门看不见别名。
//! - `tauri.conf.json` 里 `create: true` 的声明式窗：由 tauri 在 setup 前建，不经过任何 Rust 调用点。
//! - 调用方用 `block_on` 在同步回调里驱动 async 入口：`async fn` 只保证「正常 await 的路径不在回调帧里」。
//! - [`enclosing_fn`] 取「调用点之前最近的 `fn` 签名」：函数体内若嵌套了一个**已结束**的 `fn` 项、
//!   调用点又写在它之后，会被归到那个嵌套项上（今天全仓无此形态；误归只会让登记表对不上 ⇒ 红）。

use crate::commands::guard_scan::top_level_fn_body;
use crate::test_support::{crate_code, module_files};
use polaris_source_probe::mask_comments_and_strings;

/// 建窗构造器的全部调用形态（tauri 2.11.5 公开 API）。
///
/// `WebviewWindow::builder(` / `Window::builder(` 是前两者的关联函数别名（`webview/webview_window.rs`、
/// `window/mod.rs` 的 `pub fn builder`），不列就是一条绕过登记表的正门。`Webview::builder(` 不列：
/// 它只产出 `WebviewBuilder`，要成窗必经 `.add_child(`。
const BUILDER_NEEDLES: [&str; 7] = [
    "WebviewWindowBuilder::new(",
    "WebviewWindowBuilder::from_config(",
    "WebviewWindow::builder(",
    "WebviewBuilder::new(",
    "WindowBuilder::new(",
    "Window::builder(",
    ".add_child(",
];

/// 一个登记过的建窗点。
struct BuildSite {
    /// `src-tauri/src/` 下的路径。
    file: &'static str,
    /// 命中的构造器形态（[`BUILDER_NEEDLES`] 之一）。
    needle: &'static str,
    /// 构造点所在函数的签名锚点（[`top_level_fn_body`] 用）。
    enclosing: &'static str,
    /// 该函数在生产代码里的**全部**直接调用方（[`enclosing_fn`] 的返回形态，升序）。
    /// 建窗函数多一个调用方 = 多一条入口，必须重新裁定它的线程。
    callers: &'static [&'static str],
    /// 入口线程纪律由谁守（失败信息里给新增者看）。
    discipline: &'static str,
}

/// 登记表。**新增建窗点必须在这里加一行，并在 [`every_registered_build_site_keeps_its_entry_discipline`]
/// 里给它配一条入口线程纪律断言**。
const REGISTRY: [BuildSite; 4] = [
    BuildSite {
        file: "main.rs",
        needle: "WebviewWindowBuilder::from_config(",
        enclosing: "fn create_main_window(",
        callers: &["fn main(", "fn show_main_window_on_main_thread("],
        discipline: "首建在 setup；重建由 `show_main_window` 先 spawn 脱帧再 run_on_main_thread（W18，\
                     守卫 `tests::main_window_rebuild_is_dispatched_to_main_thread`）",
    },
    BuildSite {
        file: "tray/window.rs",
        needle: "WebviewWindowBuilder::new(",
        enclosing: "fn build_overlay(",
        callers: &["fn queue_overlay_build("],
        discipline: "`queue_overlay_build` 先 spawn 脱帧再 run_on_main_thread（W18，守卫 \
                     `overlay_lifecycle_gate::warm_overlay_is_prebuilt_after_tray_setup_and_cold_build_stays_off_click_frame`）",
    },
    BuildSite {
        file: "runtime/update_popup.rs",
        needle: "WebviewWindowBuilder::new(",
        enclosing: "fn build_popup_window(",
        callers: &["pub fn show_update_popup("],
        discipline: "`show_update_popup` 的每个调用点必须在 `async fn` 内或 `async_runtime::spawn` 内",
    },
    BuildSite {
        file: "commands/misc/dashboard.rs",
        needle: "WebviewWindowBuilder::new(",
        enclosing: "pub async fn open_singbox_dashboard(",
        callers: &[],
        discipline: "`#[tauri::command]` 必须是 `pub async fn`（issue #2）",
    },
];

/// 已有 W18 断言的登记项：引用而不重写。这里只钉「被引用的那条测试还在」—— 改名 / 删掉它，
/// 登记表里的 `discipline` 就成了指向虚空的注释。
const REFERENCED_W18_GUARDS: [(&str, &str); 2] = [
    (
        "tests/mod.rs",
        "fn main_window_rebuild_is_dispatched_to_main_thread()",
    ),
    (
        "tray/tests/overlay_lifecycle_gate.rs",
        "fn warm_overlay_is_prebuilt_after_tray_setup_and_cold_build_stays_off_click_frame()",
    ),
];

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `needle` 在净化面 `code` 上的命中字节偏移（标识符边界：前一字节不是标识符字符；`.` 起手的针不判）。
fn hits(code: &str, needle: &str) -> Vec<usize> {
    code.match_indices(needle)
        .map(|(at, _)| at)
        .filter(|&at| needle.starts_with('.') || at == 0 || !is_ident_byte(code.as_bytes()[at - 1]))
        .collect()
}

/// 调用点 `at` 所在函数的签名（`[可见性] [async] fn 名(`，已 trim）：取 `at` 之前最近的 `fn` 关键字。
fn enclosing_fn(code: &str, at: usize) -> String {
    let fn_at = code[..at]
        .match_indices("fn ")
        .map(|(i, _)| i)
        .filter(|&i| i == 0 || !is_ident_byte(code.as_bytes()[i - 1]))
        .last()
        .unwrap_or_else(|| panic!("偏移 {at} 之前没有任何 `fn` —— 调用点不在函数体内？"));
    let line_start = code[..fn_at].rfind('\n').map_or(0, |i| i + 1);
    let open = fn_at + code[fn_at..].find('(').expect("`fn` 签名后必有左括号");
    code[line_start..=open].trim().to_string()
}

/// 函数 `name` 在生产面上的全部**直接调用点**：`(文件, 调用点偏移, 所在函数签名)`。定义行不算。
fn call_sites<'a>(files: &'a [(String, String)], name: &str) -> Vec<(&'a str, usize, String)> {
    let needle = format!("{name}(");
    let mut out = Vec::new();
    for (path, code) in files {
        for at in hits(code, &needle) {
            if code[..at].trim_end().ends_with("fn") {
                continue;
            }
            out.push((path.as_str(), at, enclosing_fn(code, at)));
        }
    }
    out
}

/// 生产面逐文件净化：`(src 下路径, 净化面)`。
fn production_code() -> Vec<(String, String)> {
    module_files("")
        .into_iter()
        .map(|(path, source)| (path, mask_comments_and_strings(&source)))
        .collect()
}

fn line_of(code: &str, at: usize) -> usize {
    code[..at].matches('\n').count() + 1
}

/// 🔴 **allowlist 门**：建窗构造点集合恰好等于 [`REGISTRY`]。
#[test]
fn window_build_sites_match_the_registry_exactly() {
    let files = production_code();
    let mut found: Vec<(String, &str)> = Vec::new();
    let mut located: Vec<String> = Vec::new();
    for (path, code) in &files {
        for needle in BUILDER_NEEDLES {
            for at in hits(code, needle) {
                found.push((path.clone(), needle));
                located.push(format!("{path}:{} `{needle}`", line_of(code, at)));
            }
        }
    }
    found.sort_unstable();
    let mut expected: Vec<(String, &str)> = REGISTRY
        .iter()
        .map(|s| (s.file.to_string(), s.needle))
        .collect();
    expected.sort_unstable();

    assert_eq!(
        found,
        expected,
        "\n生产代码里的建窗构造点与登记表 `REGISTRY` 不一致。实际命中：\n  {}\n\n\
         多出来的 = 新增了建窗点：先到 `src/tests/window_build_sites.rs` 的 `REGISTRY` 登记，并在\
         `every_registered_build_site_keeps_its_entry_discipline` 里给它配入口线程纪律 —— Windows 上\
         **不得**在 WebView2 回调帧内建窗（同步 `#[tauri::command]` 体内、托盘 / 窗口事件回调内）：\
         要么入口是 `async` command，要么先 `tauri::async_runtime::spawn` 脱帧再 `run_on_main_thread`\
         （W18 / issue #2：帧内建窗 = 嵌套消息循环等不可重入回调，面板白屏 + 整个应用关不掉）。\n\
         少了的 = 登记项已失效：删掉那一行和它的纪律断言。",
        located.join("\n  ")
    );
}

/// 🔴 **入口纪律**：每个登记项的构造点在登记的函数里、该函数的直接调用方恰为登记名单，
/// 并逐项断言入口线程纪律。
#[test]
fn every_registered_build_site_keeps_its_entry_discipline() {
    let files = production_code();
    let code_of = |rel: &str| -> &str {
        files
            .iter()
            .find(|(path, _)| path == rel)
            .map(|(_, code)| code.as_str())
            .unwrap_or_else(|| panic!("取材面里没有 `{rel}`"))
    };

    // 次序：逐项的专属断言在前、通用的登记项核对在后 —— 通用段的 `top_level_fn_body` 锚点一旦对不上
    // 会直接 panic「命中 0 次」，把真正该给新增者看的那句（为什么必须 async）盖掉。

    // ── 面板窗（issue #2）：IPC 入口必须是 async command ──
    let dashboard = code_of("commands/misc/dashboard.rs");
    let sig = "pub async fn open_singbox_dashboard(";
    let sig_at = dashboard.find(sig).unwrap_or_else(|| {
        panic!(
            "`open_singbox_dashboard` 不再是 `pub async fn`。同步 command 在 Windows 上跑在 WebView2 \
             `WebResourceRequested` 回调帧内，帧内 build() = 嵌套消息循环等不可重入回调 ⇒ 面板白屏、\
             全部 IPC 卡死、应用关不掉（issue #2）。"
        )
    });
    assert!(
        dashboard[..sig_at]
            .trim_end()
            .ends_with("#[tauri::command]"),
        "`open_singbox_dashboard` 签名正上方必须是 `#[tauri::command]`（它是面板窗的 IPC 入口；\
         不再是 command 就换了入口形态，需重新裁定线程纪律）"
    );
    let body = top_level_fn_body(dashboard, sig);
    for variant in ["WindowLabelAlreadyExists", "WebviewLabelAlreadyExists"] {
        assert!(
            body.contains(variant),
            "面板 build() 失败时必须把 `tauri::Error::{variant}` 当「已存在则聚焦」处理：async 之后\
             快速双击的两个请求都能越过查重，后到者会撞 label 冲突，回错误信封就是一条假失败 toast"
        );
    }

    // ── 更新弹窗：`show_update_popup` 的每个调用点在 async fn 内或 spawn 内 ──
    let popup_calls = call_sites(&files, "show_update_popup");
    assert!(
        !popup_calls.is_empty(),
        "生产面上找不到 `show_update_popup` 的调用点 —— 选择器失效，下面的循环会零次执行"
    );
    for (path, at, sig) in &popup_calls {
        let code = code_of(path);
        let fn_at = code[..*at].rfind(sig.as_str()).expect("签名就在调用点之前");
        let is_async = sig.split_whitespace().any(|token| token == "async");
        let in_spawn = code[fn_at..*at].contains("async_runtime::spawn(");
        assert!(
            is_async || in_spawn,
            "`{path}:{}` 在同步函数 `{sig}` 里直接调 `show_update_popup`（它会建 WebView 窗）。\
             同步入口在 Windows 上可能处于 WebView2 回调帧内 ⇒ 帧内建窗死锁（W18 / issue #2）。\
             改成 `async fn`，或包进 `tauri::async_runtime::spawn`。",
            line_of(code, *at)
        );
    }

    // ── 全部登记项：构造点在登记的函数体内，且该函数的直接调用方恰为登记名单 ──
    for site in &REGISTRY {
        let body = top_level_fn_body(code_of(site.file), site.enclosing);
        assert!(
            body.contains(site.needle),
            "`{}` 的建窗构造 `{}` 已不在 `{}` 体内 —— 登记项与代码脱钩，入口纪律（{}）守的是空函数",
            site.file,
            site.needle,
            site.enclosing,
            site.discipline
        );

        let name = site
            .enclosing
            .trim_end_matches('(')
            .rsplit(' ')
            .next()
            .expect("签名锚点含函数名");
        let mut callers: Vec<String> = call_sites(&files, name)
            .into_iter()
            .map(|(_, _, sig)| sig)
            .collect();
        callers.sort_unstable();
        assert_eq!(
            callers, site.callers,
            "建窗函数 `{name}`（{}）的直接调用方变了。每多一个调用方就多一条建窗入口，\
             必须重新裁定它的线程：{}",
            site.file, site.discipline
        );
    }

    // ── 主窗 / 托盘浮层：已有 W18 断言，引用而不重写 ──
    for (rel, test_fn) in REFERENCED_W18_GUARDS {
        let code = crate_code(rel);
        let at = code
            .find(test_fn)
            .unwrap_or_else(|| panic!("登记表引用的 W18 守卫 `{rel}::{test_fn}` 不见了"));
        assert!(
            code[..at].trim_end().ends_with("#[test]"),
            "`{rel}::{test_fn}` 不再是 `#[test]`"
        );
    }
}

/// 计数器与调用方归属的自检：只喂合成夹具，逐形态对差。没有这一格，上面两条的绿有两种读法：
/// 判据真的成立，或者计数器压根数不出东西。
#[test]
fn build_site_scanner_self_check() {
    let count = |src: &str, needle: &str| hits(&mask_comments_and_strings(src), needle).len();

    // 标识符边界：长名不被短名重复计数。
    let src = "let w = tauri::WebviewWindowBuilder::new(app, \"x\", url);";
    assert_eq!(count(src, "WebviewWindowBuilder::new("), 1);
    assert_eq!(count(src, "WindowBuilder::new("), 0, "长名被短名重复计数");
    let src = "let w = tauri::WebviewWindow::builder(app, \"x\", url);";
    assert_eq!(count(src, "WebviewWindow::builder("), 1);
    assert_eq!(count(src, "Window::builder("), 0, "长名被短名重复计数");
    assert_eq!(count("win.add_child(b, p, s)", ".add_child("), 1);
    assert_eq!(
        count(
            "tauri::window::WindowBuilder::new(app, \"x\")",
            "WindowBuilder::new("
        ),
        1
    );

    // 注释 / 字符串里的针不算建窗点。
    for src in [
        "// WebviewWindowBuilder::new(app, ..)",
        "/// 用 `WebviewWindowBuilder::from_config(` 复用 conf",
        "/* WebviewWindowBuilder::new( */",
        "const S: &str = \"WebviewWindowBuilder::new(\";",
    ] {
        for needle in BUILDER_NEEDLES {
            assert_eq!(count(src, needle), 0, "净化面上不该命中：{src}");
        }
    }

    // 调用方归属：async / 同步 / 定义行不算调用。
    let files = vec![(
        "x.rs".to_string(),
        mask_comments_and_strings(
            "pub fn show_update_popup(a: u8) {}\n\
             pub async fn entry(app: X) {\n    show_update_popup(1);\n}\n\
             fn sync_entry() {\n    tauri::async_runtime::spawn(async move { show_update_popup(2); });\n}\n\
             // show_update_popup(3);\n",
        ),
    )];
    let sites: Vec<String> = call_sites(&files, "show_update_popup")
        .into_iter()
        .map(|(_, _, sig)| sig)
        .collect();
    assert_eq!(sites, ["pub async fn entry(", "fn sync_entry("]);
}
