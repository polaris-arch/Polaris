use super::*;
use crate::test_support::{crate_file, crate_root_code};

/// 本地 renderer → 应用命令这条高权限边界的 CSP 契约（门的实体在这个子模块里）。
mod csp_contract;

/// 建 `MockRuntime` App 的集成测试每个文件只许一个测试函数（进程级 GTK/tray 初始化会撞）。
mod mock_runtime_app_isolation;

/// 建窗点登记表 + 入口线程纪律（W18 / issue #2：Windows 上 WebView2 回调帧内建窗即死锁）。
mod window_build_sites;

/// 双击拖动层 / 系统菜单最大化不经过 `window_maximize_toggle`，必须由原生 resize 事件回读并广播。
#[test]
fn main_window_native_maximize_is_bridged_to_renderer() {
    let body = crate::commands::guard_scan::top_level_fn_body(
        &crate_root_code(),
        "fn create_main_window(",
    );
    for required in [
        "tauri::WindowEvent::Resized(_)",
        "event_window.is_maximized()",
        "commit_maximized_observation(&maximized_state, maximized)",
        "emit_window_maximize_changed(&app_handle, maximized)",
    ] {
        assert!(
            body.contains(required),
            "主窗原生最大化同步链缺少 `{required}`"
        );
    }
}

/// W13：托盘图标黑/白变体的探测链（Win/Linux）。前两格是纯函数语义；后两格是源扫描守卫
/// （探测窗必须先于主窗被探、且 setup 必须真建它）——顺序翻回去 = 显式 uiTheme 下的
/// 读应用外观失真（复审 Med-1）复活，不建它 = 探测链退回旧缺陷。
/// W13：托盘图标黑/白变体的探测链（Win/Linux）。前三格是纯函数语义；后两格是守卫——
/// 注册表真值必须先于（被钉的）主窗被读；Windows 上注册表读法必须真的给出答案（CI win 腿上跑）。
#[cfg(not(target_os = "macos"))]
mod tray_dark_bg_probe {
    use super::dark_bg_from_probe;
    use crate::test_support::crate_code;

    #[test]
    fn primary_wins_when_present() {
        assert!(dark_bg_from_probe(Some(true), Some(false)));
        assert!(!dark_bg_from_probe(Some(false), Some(true)));
    }

    /// W13 的核心格：主信号（注册表→主窗）取不到时 fallback（浮层窗）接管——
    /// 旧实现这格恒白，正是浅色任务栏图标隐身的真机缺陷本体。
    #[test]
    fn fallback_takes_over_when_primary_is_gone() {
        assert!(!dark_bg_from_probe(None, Some(false)));
        assert!(dark_bg_from_probe(None, Some(true)));
    }

    #[test]
    fn all_missing_falls_back_to_dark_assumption() {
        assert!(dark_bg_from_probe(None, None));
    }

    #[test]
    fn registry_truth_is_probed_before_the_pinned_main_window() {
        let src = crate_code("app_tray.rs");
        let body = crate::commands::guard_scan::top_level_fn_body(&src, "fn set_tray_state(");
        assert!(
            !body.is_empty(),
            "set_tray_state 函数体取不到——判据失效需同步更新"
        );
        let reg = body
            .find("system_dark_bg()")
            .expect("set_tray_state 不再读注册表真值（W13 回潮）");
        let main = body
            .find("get_webview_window(\"main\")")
            .expect("set_tray_state 的主窗探测形态变了");
        assert!(
            reg < main,
            "注册表真值又排到了主窗之后：显式 uiTheme 下主窗被钉、读到应用外观而非任务栏明暗"
        );
    }

    /// Windows CI 腿上跑：Personalize 键自 Win10 1809 起恒在，读不出 Some 说明读法坏了。
    #[cfg(target_os = "windows")]
    #[test]
    fn registry_probe_answers_on_real_windows() {
        assert!(super::super::system_dark_bg().is_some());
    }
}

// ── 托盘图标汇流点（P1「中断后不回落」）──────────────────────────────────────
//
// 图标本身只能真机看；能自动断言的是**装配决策**：哪些源被接上汇流点。此前这条腿零测试，
// 于是「崩溃腿没接」这种缺失从未被任何门发现。下面三测穷举本 bug 的逃逸面：
//   ① 某条终态事件没订（本 bug 的原形：ERROR 缺失）
//   ② 事件订阅退化成只订一条 / 只订部分（补一个 ERROR 监听就收工的半修）
//   ③ 轮询自愈网没挂，或挂了但周期长到形同没有（零 emit 腿仍无人兜）

/// 首建天然发生在 setup 主线程，轻量态重建则可能由托盘 WebView IPC 线程触发。两条路径必须在
/// `show_main_window` 入口合流到主线程，否则 macOS `apply_vibrancy` 会拒绝重建窗，透明侧栏背后
/// 没有原生材质，表现为整个左侧导航直接透出桌面。
#[cfg(target_os = "linux")]
#[test]
fn main_window_rebuild_is_dispatched_to_main_thread() {
    let src = crate_root_code();
    let entry = crate::commands::guard_scan::top_level_fn_body(&src, "fn show_main_window(");
    let on_main =
        crate::commands::guard_scan::top_level_fn_body(&src, "fn show_main_window_on_main_thread(");

    assert!(
        entry.contains("run_on_main_thread"),
        "主窗唤出入口必须先投主线程；托盘 IPC 线程不得直接重建原生窗口"
    );
    // W18 第二层：主线程帧内调用（WM_COPYDATA WndProc / IPC 分发栈）时 run_on_main_thread
    // 是内联直执——必须先跳线程脱离分发帧再排回，否则重建把同步对端卡死在 SendMessageW 上。
    let spawn_at = entry
        .find("async_runtime::spawn")
        .expect("主窗唤出必须先跳 async 线程脱离消息分发帧");
    let queue_at = entry
        .find("run_on_main_thread")
        .expect("跳线程后必须排回主线程");
    assert!(
        spawn_at < queue_at,
        "次序必须是 spawn 脱帧 → run_on_main_thread 排回 → 重建/呈现"
    );
    let probe_at = entry
        .find("window_health::begin_show_probe")
        .expect("帧外调度前必须先登记唤出起点，否则真机时延漏掉排队段");
    assert!(
        probe_at < spawn_at,
        "主窗时延起点必须早于 spawn，不能把消息帧逃逸/线程排队从数据里剪掉"
    );
    assert!(
        entry.contains("show_main_window_on_main_thread("),
        "主线程闭包必须调用唯一的建窗/呈现实现"
    );
    assert!(
        !entry.contains("create_main_window("),
        "跨线程入口不得绕过主线程边界直接建窗"
    );
    assert!(
        on_main.contains("create_main_window(app, false)"),
        "轻量态重建必须留在主线程实现内"
    );
    let create = crate::commands::guard_scan::top_level_fn_body(&src, "fn create_main_window(");
    assert!(
        create.contains("rt.stats().mark_main_window_created()"),
        "builder 成功后必须提交主窗口生命周期，供三平台 stats/logs 可见性门共享"
    );
}

// ── 托盘汇流点幂等闸门（30s 轮询 × 全程 = 每 30s 一次磁盘写 + indicator 重载）────────
//
// 下面这组锁的是 `reconcile_tray_visual` 的**全部逃逸面**：短路被删（恒重画）、短路过度（变了不
// 重画）、只比部分字段、缓存不更新、失败还照存缓存 —— 任一形态都必须有一条转红。

// ── dialog 插件 ACL（真机 'plugin:dialog|confirm not allowed by ACL'）───────────
//
// 病灶不是「忘了配」，是 `dialog:default` **文案骗人**：它自称 "All dialog types are enabled"，
// 实际 permissions = [allow-message, allow-save, allow-open]，**不含 allow-confirm/allow-ask**。
// 而 tauri-plugin-dialog 的 init 脚本无条件把 `window.alert`/`window.confirm` 覆写成
// `plugin:dialog|message` / `|confirm` ⇒ 任何一句 `window.confirm(...)` 都会撞 ACL。
//
// 「读 default.json 断言含 allow-confirm」这种测只是把配置抄一遍（改配置时会顺手改测，没牙）。
// 下面改成断言**调用面 ⊆ 授权面**：扫前端源码里真实用到的 dialog 命令，逐个要求对应权限存在。

/// 判定紧接 `out` 之后的 `/` 是否**可能**是正则字面量起点（而非除法 / JSX 斜杠）。
///
/// 用白名单（只有这些前驱字符/关键字之后才允许正则）而非黑名单：宁可漏判成除法（= 今天的行为），
/// 也不能把 `</div>`、`<img … />` 这类 JSX 斜杠误当正则起点 —— 那会在 .tsx 里新造注释泄漏。
fn regex_can_start(out: &str) -> bool {
    let t = out.trim_end();
    let Some(last) = t.chars().last() else {
        return true; // 文件开头
    };
    if matches!(
        last,
        '=' | '(' | ',' | ':' | '[' | '!' | '&' | '|' | '?' | ';' | '{'
    ) {
        return true;
    }
    // `return /re/.test(x)` 一类：关键字之后同样允许正则。
    const REGEX_PREFIX_KW: [&str; 14] = [
        "return",
        "typeof",
        "instanceof",
        "in",
        "of",
        "new",
        "delete",
        "void",
        "throw",
        "case",
        "do",
        "else",
        "yield",
        "await",
    ];
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    REGEX_PREFIX_KW.iter().any(|kw| {
        t.strip_suffix(kw)
            .is_some_and(|head| head.chars().last().is_none_or(|c| !ident(c)))
    })
}

/// 剥 TS/TSX 注释（`//` 与 `/* */`），**不碰字符串/模板字面量内部**（`"https://x"` 里的 `//`
/// 不是注释起点 —— 否则同行后续真代码会被吃掉，扫描面出洞）。换行原样保留。
///
/// 守卫扫的是**代码**：本仓注释里到处在讲 `window.confirm` 这个坑（`settings-logic.ts:252/254`、
/// `SettingsHelper.tsx:101`），不剥注释的话「前端还在用 confirm」这个前提永远为真 —— 反向哨兵
/// 退化成空转，而它恰恰是写来防这个形态的。
///
/// 与前端 `settings-logic.test.ts` 的 `stripComments` 同职责（那边正则、这边多认字符串状态）。
///
/// 正则字面量（`const re = /['"]/g;`）单列一档：不认的话里面的 `'` 会被当成字符串起点，引号状态
/// **一路挂到文件后面某个落单引号**，中间所有注释都不再被剥 ⇒ 反向哨兵被注释里的
/// `window.confirm` 喂绿。识别到就**原样拷贝**（绝不删除），故判错的最坏后果 = 退化成今天的行为，
/// 不可能吃掉真代码造成扫描面出洞。起点判定用**白名单**（`= ( , : [ ! & | ? ; {` + 关键字）而非
/// 「不是标识符就算正则」：后者会把 JSX 的 `</div>`、`<img … />` 误当正则起点，在 .tsx 里
/// 反倒制造新的注释泄漏。
///
/// 另有兜底：`'` / `"` 字符串**不能跨行**（只有模板串可以），故撞见换行即判定「刚才解析错了」
/// 并复位。任何残余的引号态误判（正则、JSX 文本里的 `don't` …）都被限制在**一行内**，
/// 不再顺着文件级联。
fn strip_ts_comments(src: &str) -> String {
    strip_ts(src, false)
}

/// 在 [`strip_ts_comments`] 之上**再把字符串 / 模板 / 正则字面量的内部字符换成空格**（起止引号、
/// 换行原样留着，只清内容，行号与行数都不动）。
///
/// 只给声明面扫描（[`declares_binding`]）用：一句 `const s = "const confirm = …"` 或一段示例
/// 模板文本不该被当成「本文件真的声明了 confirm 绑定」—— 那会把一次**真实的**全局调用喂绿。
/// 调用面判据（[`calls_global`]）反过来**不能**用这一档：它要看得见 `window['confirm']` 这种
/// 写在字符串里的成员名。
fn strip_ts_comments_and_strings(src: &str) -> String {
    strip_ts(src, true)
}

/// 两档剥离的共同内核。`blank_literals` = 是否连字面量内容一起清空（见
/// [`strip_ts_comments_and_strings`]）。只有一份状态机：正则/JSX/未闭合引号那一堆边界只此一处，
/// 两档共享，不存在「一档修了另一档没修」。
///
/// 形状与 `crates/source-probe` 的 `mask_comments` / `mask_comments_and_strings` 两档一致
/// （共用一份词法扫描、只差一个布尔）。这里没有直接用那一对，是因为那份扫的是 **Rust** 词法
/// （字符 / 字节 / raw 字面量、生命周期 `'a`），认不得 TS 的模板串、正则与 JSX —— 本文件的
/// TS 状态机本来就是为此单独存在的。
fn strip_ts(src: &str, blank_literals: bool) -> String {
    let mut out = String::with_capacity(src.len());
    let mut it = src.chars().peekable();
    let mut quote: Option<char> = None; // Some(引号字符) = 字符串/模板内
    let mut escaped = false;
    while let Some(c) = it.next() {
        if let Some(q) = quote {
            // `closing` = 本字符是字面量的边界（收尾引号 / 触发复位的换行）⇒ 清空档里也要原样留。
            let mut closing = false;
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
                closing = true;
            } else if c == '\n' && q != '`' {
                quote = None; // 单/双引号串不跨行 ⇒ 走到这儿说明刚才判错了，就地收手
                closing = true;
            }
            out.push(if blank_literals && !closing && c != '\n' {
                ' '
            } else {
                c
            });
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                quote = Some(c);
                out.push(c);
            }
            '/' if it.peek() == Some(&'/') => {
                for n in it.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if it.peek() == Some(&'*') => {
                it.next(); // 吃掉 '*'
                let mut prev = '\0';
                for n in it.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    if n == '\n' {
                        out.push('\n');
                    }
                    prev = n;
                }
            }
            // 正则字面量：结构原样（起止 `/` 与换行都留着），只为屏蔽里面的引号；
            // `blank_literals` 档里内部字符一并清成空格，理由同字符串。
            '/' if regex_can_start(&out) => {
                out.push('/');
                let mut in_class = false; // `[...]` 内的 `/` 不结束字面量
                let mut esc = false;
                for n in it.by_ref() {
                    let ends = !esc && ((n == '/' && !in_class) || n == '\n');
                    out.push(if blank_literals && !ends { ' ' } else { n });
                    if esc {
                        esc = false;
                        continue;
                    }
                    match n {
                        '\\' => esc = true,
                        '[' => in_class = true,
                        ']' => in_class = false,
                        '/' if !in_class => break,
                        '\n' => break, // 正则不跨行：真跨行说明判错了，立刻收手
                        _ => {}
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// 递归收集目录下所有**生产** .ts/.tsx 源码（已剥注释、已排除 `*.test.*` / `*.spec.*`；
/// 测试期读盘，无新增依赖）。
///
/// 排除测试文件：vitest 跑在 node 环境、根本不经 Tauri ACL，测试文本里出现 `window.confirm`
/// 既不需要授权，也不该让「前端还在用 confirm」这个前提为真（`settings-logic.test.ts` 此前正是
/// 这么把反向哨兵喂绿的）。
#[cfg(target_os = "linux")]
fn collect_sources(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_sources(&p, out);
            continue;
        }
        if !matches!(
            p.extension().and_then(|s| s.to_str()),
            Some("ts") | Some("tsx")
        ) {
            continue;
        }
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        // 🔴 三种**永不进包**的模块一律排出取材面（口径的单一真值源在
        // `ui/src/contracts/test-only-modules.ts` 的 `IS_TEST_ONLY_MODULE`）。
        //
        // `.test-support.` 是 2026-09-13 补的，起因是一次真的假阳性：连接屏的登记表
        // `mobile/connections/absence-register.test-support.ts` 有一条 `note:`，那句话本身在说
        // 「移动端**不用** `window.confirm` —— 它被 dialog 插件 init 覆写成 `plugin:dialog|confirm`」，
        // 而 `DIALOG_INVOKE_TO_PERM` 按裸串匹配，把这句**解释**读成了一次调用。
        //
        // 为什么不改成「剥掉字符串再匹配」：真正的调用形态 `invoke('plugin:dialog|confirm')`
        // 本身就是字符串字面量 —— 剥了它，这张表一条都匹配不到，门当场失去全部牙齿。
        // 能分辨两者的不是「在不在字符串里」，是「这份文件会不会进包」。
        if name.contains(".test.") || name.contains(".spec.") || name.contains(".test-support.") {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(&p) {
            out.push((p, strip_ts_comments(&s)));
        }
    }
}

fn ui_src() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/src")
}

/// 直接 invoke 命令名的**裸串**形态 → 所需权限 id。
///
/// 光有这张表守不住 import 形态：`import { save } from '@tauri-apps/plugin-dialog'` 的源码里
/// 一个 `plugin:dialog|save` 字样都不会出现 —— 见 [`dialog_import`]，三条检测面并联。
const DIALOG_INVOKE_TO_PERM: [(&str, &str); 5] = [
    ("plugin:dialog|confirm", "dialog:allow-confirm"),
    ("plugin:dialog|message", "dialog:allow-message"),
    ("plugin:dialog|ask", "dialog:allow-ask"),
    ("plugin:dialog|open", "dialog:allow-open"),
    ("plugin:dialog|save", "dialog:allow-save"),
];

/// 被插件 init 脚本**覆写的全局函数** → 所需权限 id。插件无条件把 `window.confirm` / `window.alert`
/// 换成 `plugin:dialog|confirm` / `|message`，所以调用点写不写 `window.` 前缀都一样走 ACL。
const DIALOG_GLOBAL_TO_PERM: [(&str, &str); 2] = [
    ("confirm", "dialog:allow-confirm"),
    ("alert", "dialog:allow-message"),
];

/// 源码里是否调用了全局函数 `name`。
///
/// 只匹配字面量 `window.confirm(` 会漏掉一整片等价形态 —— 它们撞的是**同一条** ACL：
/// 裸调 `await confirm('x')`、括号前带空格 `window.confirm ('x')`、计算成员
/// `window['confirm'](…)`、`globalThis.confirm(…)`。漏了它们不仅是 ACL 洞（一旦收回
/// `allow-confirm` 就逃逸），更让反向哨兵「confirm 只许出现在 nativeConfirm 一处」对别处裸用不转红。
///
/// `foo.confirm(` 这类**他人成员**不算全局调用（`window` / `globalThis` 才是），否则会把无关对象
/// 的同名方法误判成 dialog 调用。
///
/// **裸调形态还要过一道作用域遮蔽**：文件自己声明了同名绑定时（依赖注入的
/// `const { confirm } = deps`、`import { confirm } from …`、形参 …），裸 `confirm(` 按 JS 作用域
/// 规则解析到那个局部绑定，**永远**到不了被插件覆写的全局 —— 判成全局调用就是假阳性，而本门守的
/// 失败模式（漏授 `dialog:allow-confirm` ⇒ 整条腿抛 rejection）在那种文件里结构上不可能发生。
/// `ui/src/mobile/nodes/node-deletion.ts` 的注入式 confirm 正是这样把本门与 ACL 门一起顶红的。
/// 遮蔽面认哪些形态、已知边界在哪，见 [`declares_binding`]。
///
/// **显式属主不受遮蔽**：`window.confirm(` / `globalThis.confirm(` / `window['confirm'](` 即使在
/// 声明了同名绑定的文件里也照抓 —— 那三种写法明确指名全局，与局部有没有同名绑定无关，而它们
/// 恰恰是「真回退到全局对话框」最常见的写法。
fn calls_global(src: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let is_global_owner = |head: &str| {
        ["window", "globalThis"].iter().any(|o| {
            head.strip_suffix(o)
                .is_some_and(|rest| rest.chars().last().is_none_or(|c| !ident(c)))
        })
    };
    for (i, _) in src.match_indices(name) {
        // token 左边界：`reconfirm(` 不算。右边界不必单独校验 —— 多出的标识符字符必然让下面
        // 「后面是 `(`」的判据落空（`confirmDelete(` 的 rest 以 `D` 开头），单写一条杀不掉任何变异。
        if src[..i].ends_with(ident) {
            continue;
        }
        // 括号前允许空白：`window.confirm ('x')`。
        let rest = &src[i + name.len()..];
        if !rest.trim_start().starts_with('(') {
            continue;
        }
        let head = src[..i].trim_end();
        if let Some(owner) = head.strip_suffix('.') {
            if !is_global_owner(owner.trim_end()) {
                continue; // `dialog.confirm(` 之类：不是被覆写的那个全局
            }
            return true; // 显式点了 window / globalThis：与同名局部绑定无关，照抓
        }
        if declares_binding(src, name) {
            continue; // 裸调，但本文件自己声明了同名绑定 ⇒ 解析到它，到不了全局
        }
        return true;
    }
    // 计算成员形态：`window['confirm'](…)` —— 上面的标识符扫描看不见（名字在字符串里）。
    ["window", "globalThis"].iter().any(|owner| {
        ['\'', '"']
            .iter()
            .any(|q| src.contains(&format!("{owner}[{q}{name}{q}]")))
    })
}

/// 从 `at` 向左找最近一个**未闭合**的开括号，返回 `(下标, 字符)`。同类括号各记各的深度：
/// 够判「这次出现落在哪种模式里」，而括号不配平的源码本来就进不了编译面。
fn enclosing_open(src: &str, at: usize) -> Option<(usize, char)> {
    let (mut paren, mut brace, mut brack) = (0i32, 0i32, 0i32);
    for (i, c) in src[..at].char_indices().rev() {
        match c {
            ')' => paren += 1,
            ']' => brack += 1,
            '}' => brace += 1,
            '(' if paren == 0 => return Some((i, '(')),
            '[' if brack == 0 => return Some((i, '[')),
            '{' if brace == 0 => return Some((i, '{')),
            '(' => paren -= 1,
            '[' => brack -= 1,
            '{' => brace -= 1,
            _ => {}
        }
    }
    None
}

/// `open` 处那个开括号的匹配闭括号下标。
fn matching_close(src: &str, open: usize) -> Option<usize> {
    let (o, c) = match src[open..].chars().next()? {
        '(' => ('(', ')'),
        '[' => ('[', ']'),
        '{' => ('{', '}'),
        _ => return None,
    };
    let mut depth = 0i32;
    for (i, ch) in src[open..].char_indices() {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some(open + i);
            }
        }
    }
    None
}

/// `open` 处那个 `(` 是**形参列表**的开括号吗（而不是调用实参、控制流条件、分组括号）？
///
/// 判据两条：闭括号之后是 `=>`（箭头函数）或 `{`（函数体），且开括号前不是 `if|while|for|switch`
/// 的条件括号（`if (confirm) {` 的 `)` 后面也是 `{`，光看后缀分不开）。
///
/// [`declares_binding`] 的 `'('` 支与 `'{'` 支**共用这一份**：`'('` 支问「`(confirm) =>` 里的
/// confirm 是不是形参」，`'{'` 支问「`foo(a, { confirm })` 外面那层 `(` 是不是形参列表」——
/// 同一个问题问两次，各写一份迟早漂。
fn is_param_list(src: &str, open: usize) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let before = src[..open].trim_end();
    matching_close(src, open).is_some_and(|close| {
        let tail = src[close + 1..].trim_start();
        tail.starts_with("=>") || tail.starts_with('{')
    }) && !["if", "while", "for", "switch"].iter().any(|kw| {
        before
            .strip_suffix(kw)
            .is_some_and(|rest| rest.chars().last().is_none_or(|c| !ident(c)))
    })
}

/// `name` 是否由某条 `import … from '…'` 引入（默认 / 具名 / `as` 改名都算）。
/// 语句射程与 [`dialog_import`] 同一套切分：本关键字 → 下一条 import/export 关键字，越不了界。
fn imports_binding(src: &str, name: &str) -> bool {
    let mut stmts: Vec<(usize, &str)> = ["import", "export"]
        .iter()
        .flat_map(|kw| keyword_positions(src, kw).map(move |i| (i, *kw)))
        .collect();
    stmts.sort_unstable();
    stmts.iter().enumerate().any(|(n, &(pos, kw))| {
        if kw != "import" {
            return false; // `export … from` 不往本文件引入绑定
        }
        let end = stmts.get(n + 1).map_or(src.len(), |(next, _)| *next);
        module_specifier(&src[pos + kw.len()..end])
            .is_some_and(|(clause, _)| keyword_positions(clause, name).next().is_some())
    })
}

/// 本文件**自己声明了**名为 `name` 的绑定吗？有则裸调 `name(` 解析到它，到不了被覆写的全局
/// （[`calls_global`] 据此豁免裸调形态）。
///
/// 取材面先过 [`strip_ts_comments_and_strings`]：注释、字符串 / 模板 / 正则字面量里的
/// `const confirm =` 一律不算声明 —— 否则一行注释或一段示例文本就能把真实的全局调用喂绿。
///
/// **认这些形态**：
/// - `import { confirm } from …` / `import confirm from …` / `import { ask as confirm } from …`
/// - `const|let|var confirm = …`、`function confirm(…)`、`class confirm`
/// - 对象解构：`const { a, confirm } = x`、`const { ask: confirm } = x`、嵌套 `const { a: { confirm } } = x`、
///   形参解构 `function f({ confirm })`
/// - 数组解构：`const [confirm] = x`（**只**认 const/let/var 前缀：否则 React 依赖数组
///   `[confirm, dismiss]` 会被当成声明，那是使用位）
/// - 形参：`function f(confirm)`、`(confirm) => …`、`(a, confirm: T) => …`、`catch (confirm)`。
///   「这对圆括号是不是形参列表」按**闭括号之后是 `=>` 或 `{`** 判，并排掉 `if|while|for|switch`
///   的条件括号（`if (confirm) {` 是使用位）
///
/// **刻意不认**（认了就是把使用位当声明、放过真实全局调用）：类型/接口字段
/// `readonly confirm: (…) => string`（`node-deletion.ts:60` 正是这形态，它不是值绑定；那个文件
/// 真正的绑定是 `:77` 的解构，本判据认的是后者）、对象字面量的键与简写 `{ confirm: x }` /
/// `{ confirm }`、实参 `register(confirm)`、依赖数组 `[confirm]`。对象字面量简写写成**实参**时
/// （`foo(a, { confirm })`）同样不认：花括号被 `(` 包着的话，得那个 `(` 经 [`is_param_list`]
/// 判定为形参列表才算解构模式 —— 只有 `function f({ confirm })` / `({ confirm }) => …` 过得了。
///
/// **已知边界（文件粒度，不是词法作用域粒度）**：本判据只问「这个文件里有没有一处 `name` 绑定」，
/// 不问那处绑定罩不罩得住调用点。于是一个文件若在某个嵌套块里声明了 `confirm`（比如某个函数的
/// 形参），同时又在模块顶层裸调 `confirm(`，本判据会**放过那次真实的全局调用**。这是刻意取的
/// 工程折中：要判准就得引 JS parser（新依赖 + 一整套 TS/TSX 语法面），而本门的实际护栏面是
/// 「有人在前端写回 window.confirm」—— 那种写法的文件里通常压根没有同名绑定，且真要绕过它的人
/// 有更省事的办法（`globalThis['con' + 'firm']`），本门从来不是对抗性判据。代价被兜在别处：
/// 三种**显式属主**形态不受本豁免影响（见 [`calls_global`]），而那才是真回退到全局对话框的
/// 常见写法。
///
/// **第二条已知边界（实测，不是推断）**：**嵌套一层**的对象字面量简写仍会被认成声明 ——
/// `foo({ a: { confirm } })`、`const x = { deps: { confirm } }` 这两个形状同文件若还有裸
/// `confirm(`，本判据会放过它。根因是它与嵌套解构 `const { a: { confirm } } = x` **前导完全同形**
/// （都是 `:` 后跟 `{`），要分开得看等号右边有没有初始化器，那已经是在写 parser 了。外层那道
/// 「花括号被 `(` 包着就得是形参列表」的收窄在这里够不着：`{ confirm }` 的外层是 `{` 不是 `(`。
/// 未嵌套的同类形状（`foo(a, { confirm })`、`foo({ confirm })`、`return { confirm }`、
/// `export default { confirm }`、`foo(a, [{ confirm }])`）都已判对。
fn declares_binding(src: &str, name: &str) -> bool {
    let stripped = strip_ts_comments_and_strings(src);
    let src = stripped.as_str();
    if imports_binding(src, name) {
        return true;
    }
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let ends_with_kw = |head: &str, kw: &str| {
        head.strip_suffix(kw)
            .is_some_and(|rest| rest.chars().last().is_none_or(|c| !ident(c)))
    };
    for i in keyword_positions(src, name) {
        let head = src[..i].trim_end();
        // ① 声明关键字直接跟着名字。
        if ["const", "let", "var", "function", "class"]
            .iter()
            .any(|kw| ends_with_kw(head, kw))
        {
            return true;
        }
        // ② 解构模式 / 形参列表里的绑定位：先问这次出现被哪种括号包着。
        let Some((open, kind)) = enclosing_open(src, i) else {
            continue;
        };
        let before = src[..open].trim_end();
        let after = src[i + name.len()..].trim_start();
        let hit = match kind {
            '{' => {
                // 这对花括号若被一个 `(` 直接包着，只有那个 `(` 真是形参列表才算解构模式。
                // 否则就是**对象字面量简写实参**（`foo(a, { confirm })`）：里面的 confirm 引用的是
                // 别处的绑定 —— 可能正是全局 —— 把它当声明，同文件那次真·全局调用就被放过了。
                let in_call_args = match enclosing_open(src, open) {
                    Some((outer, '(')) => !is_param_list(src, outer),
                    _ => false, // 顶层 / 函数体 / 数组里的解构不经这道收窄
                };
                let is_pattern = !in_call_args
                    && (["const", "let", "var"]
                        .iter()
                        .any(|kw| ends_with_kw(before, kw))
                        || before.ends_with(['(', ',', '{', ':']));
                // `{ confirm: other }`：confirm 是属性名，绑定的是 other。
                is_pattern && !after.starts_with(':') && head.ends_with(['{', ',', ':'])
            }
            '[' => {
                ["const", "let", "var"]
                    .iter()
                    .any(|kw| ends_with_kw(before, kw))
                    && head.ends_with(['[', ','])
            }
            '(' => is_param_list(src, open) && head.ends_with(['(', ',']),
            _ => false,
        };
        if hit {
            return true;
        }
    }
    false
}

/// dialog 插件的 JS 模块说明符（具名 import 形态的入口）。
const DIALOG_MODULE: &str = "@tauri-apps/plugin-dialog";

/// 插件导出的**命令面全集** → 所需权限 id（导出名与 `plugin:dialog|<name>` 同名）。
///
/// `save` 这行是本批补的洞：`allow-save` 刚被收回，而旧表里 `plugin:dialog|save` 压根不存在
/// ⇒ 有人写 `import { save } ...` 时守卫全绿、运行期真机抛
/// `Command plugin:dialog|save not allowed by ACL` —— 正是本批要根治的病灶原型复发。
const DIALOG_API_TO_PERM: [(&str, &str); 5] = [
    ("confirm", "dialog:allow-confirm"),
    ("message", "dialog:allow-message"),
    ("ask", "dialog:allow-ask"),
    ("open", "dialog:allow-open"),
    ("save", "dialog:allow-save"),
];

/// 一份源码对 [`DIALOG_MODULE`] 的使用形态。
#[derive(Debug, PartialEq, Eq)]
enum DialogImport {
    /// 没引用该模块。
    Absent,
    /// 解析出静态具名列表（可能为空：纯类型 import）→ 可精确映射到权限。
    /// 保存的是**导出名**（`open as pick` 取 `open`），因为 ACL 认的是命令名不是本地别名。
    Named(Vec<String>),
    /// 引用了模块但取不到静态具名列表（`import * as` / `await import(...)` / side-effect import）
    /// → 调用面不可判 ⇒ **失败关闭**，让人来判，而不是默默放行成第二个洞。
    Opaque,
}

/// 找出 `src` 里所有落在 token 边界上的 `kw` 关键字位置（避开 `important` / `reimport` 之类子串命中）。
fn keyword_positions<'a>(src: &'a str, kw: &'a str) -> impl Iterator<Item = usize> + 'a {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    src.match_indices(kw)
        .filter(move |(i, _)| {
            !src[..*i].ends_with(ident) && !src[*i + kw.len()..].starts_with(ident)
        })
        .map(|(i, _)| i)
}

/// 从一条 import/export 语句的**正文**（关键字之后 → 下一条语句关键字为止）里取模块说明符，
/// 返回 `(关键字与说明符之间的 clause, 模块名)`。撞见 `;` 说明本语句压根没有说明符。
fn module_specifier(body: &str) -> Option<(&str, &str)> {
    let mut it = body.char_indices();
    while let Some((off, c)) = it.next() {
        match c {
            ';' => return None,
            '\'' | '"' | '`' => {
                let start = off + c.len_utf8();
                let mut escaped = false;
                for (end, n) in it.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if n == '\\' {
                        escaped = true;
                    } else if n == c {
                        return Some((&body[..off], &body[start..end]));
                    }
                }
                return None;
            }
            _ => {}
        }
    }
    None
}

/// 解析源码对 `@tauri-apps/plugin-dialog` 的使用形态。
///
/// **不从模块名回看整份文件**：回看会一路切到*上一条* import 的关键字，于是上一条 import 的花括号
/// 就把「有没有具名列表」这个判据满足掉 —— 真实源文件几乎必有前置 import，`export … from` /
/// `export *` / 动态 `import(M)` / `require()` 因此全部被误判成 `Named(上一条 import 的名字)`，
/// 失败关闭形同虚设（甚至吞掉本条真正的具名项）。
///
/// 改为**先按语句切分再解析**：每条语句的射程 = 本关键字 → 下一条 import/export 关键字，越不了界。
/// 模块名若出现在任何 import/export 语句之外（`require('…')`、`const M = '…'`、子路径…），
/// 直接判不可判。
///
/// 两条独立的网：①「本语句 clause 必须整好是具名花括号组」②「模块名出现次数必须全被 import/export
/// 语句认领」。变异验证显示二者互为兜底 —— 单独敲掉 `export` 分支或语句射程上界，逃逸构造仍被 ②
/// 拦成 `Opaque`（失败关闭方向），不产生假绿。保留 ① 是为了**精度**（少报无谓的不可判），不是安全属性。
fn dialog_import(src: &str) -> DialogImport {
    let total = src.matches(DIALOG_MODULE).count();
    if total == 0 {
        return DialogImport::Absent;
    }
    let mut stmts: Vec<(usize, &str)> = ["import", "export"]
        .iter()
        .flat_map(|kw| keyword_positions(src, kw).map(move |i| (i, *kw)))
        .collect();
    stmts.sort_unstable();

    let mut names = Vec::new();
    let mut matched = 0usize;
    for (n, &(pos, kw)) in stmts.iter().enumerate() {
        let end = stmts.get(n + 1).map_or(src.len(), |(next, _)| *next);
        let Some((clause, module)) = module_specifier(&src[pos + kw.len()..end]) else {
            continue;
        };
        if module != DIALOG_MODULE {
            continue;
        }
        matched += 1;
        // `export … from '…'`：再导出。调用面转移到下游消费方，而消费方 import 的是**本地**模块名
        // （扫描时看不出它连着 dialog 插件）⇒ 不可判，失败关闭。
        if kw == "export" {
            return DialogImport::Opaque;
        }
        // clause 必须**整好**是一个具名花括号组 + `from`。`import * as d from` / `import d from` /
        // `import d, { x } from`（默认绑定没被覆盖）/ 动态 `import(` / side-effect import 一律不可判。
        let Some(spec) = clause.trim().strip_suffix("from") else {
            return DialogImport::Opaque;
        };
        let Some(inner) = spec
            .trim()
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
        else {
            return DialogImport::Opaque;
        };
        for raw in inner.split(',') {
            // `type Foo` → 剥 type 前缀；`open as pick` → 取导出名 `open`。
            let n = raw.trim().trim_start_matches("type ").trim();
            if let Some(exported) = n.split_whitespace().next() {
                if !exported.is_empty() {
                    names.push(exported.to_owned());
                }
            }
        }
    }
    if matched != total {
        return DialogImport::Opaque;
    }
    DialogImport::Named(names)
}

/// 一份源码需要的 dialog 权限全集（裸串形态 ∪ 具名 import 形态）。
/// `Err` = 调用面不可判，守卫按失败关闭处理。
fn required_dialog_perms(src: &str) -> Result<Vec<&'static str>, &'static str> {
    let mut need: Vec<&'static str> = DIALOG_INVOKE_TO_PERM
        .iter()
        .filter(|(call, _)| src.contains(call))
        .map(|(_, perm)| *perm)
        .chain(
            DIALOG_GLOBAL_TO_PERM
                .iter()
                .filter(|(name, _)| calls_global(src, name))
                .map(|(_, perm)| *perm),
        )
        .collect();
    match dialog_import(src) {
        DialogImport::Absent => {}
        DialogImport::Opaque => {
            return Err(
                "引用了 @tauri-apps/plugin-dialog 却取不到静态具名列表（namespace / 动态 import / \
                     side-effect import）→ 调用面不可判。请改成具名 import，否则 ACL 守卫失效",
            )
        }
        DialogImport::Named(names) => {
            for n in names {
                if let Some((_, perm)) = DIALOG_API_TO_PERM.iter().find(|(api, _)| *api == n) {
                    need.push(perm);
                }
            }
        }
    }
    Ok(need)
}

fn granted(capability_json: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(capability_json).expect("capability 非法 JSON");
    v["permissions"]
        .as_array()
        .expect("capability 缺 permissions")
        .iter()
        .filter_map(|p| p.as_str().map(str::to_owned))
        .collect()
}

/// `capabilities/*.json` → `window label → 该 window 的授权面`（一份 capability 可覆盖多个 window）。
///
/// **测试期读盘**而非手写清单：手写「次级窗有哪些」正是漏掉 `update-popup` 的成因 —— 新增一份
/// capability / 新增一个 window 必须自动进扫描面，不能指望有人记得回来改这张表。
fn capabilities_by_window() -> std::collections::BTreeMap<String, Vec<String>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for entry in std::fs::read_dir(&dir)
        .expect("capabilities/ 读不到")
        .flatten()
    {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&p).expect("capability 读不到");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("capability 非法 JSON");
        let windows: Vec<String> = v["windows"]
            .as_array()
            .expect("capability 缺 windows")
            .iter()
            .filter_map(|w| w.as_str().map(str::to_owned))
            .collect();
        let perms = granted(&raw);
        for w in windows {
            out.entry(w).or_default().extend(perms.iter().cloned());
        }
    }
    out
}

/// 次级窗的前端入口目录名（= window label）。约定：`ui/src/<label>/main.ts(x)`，与
/// `ui/vite.config.ts` 的多页 `rollupOptions.input`、Rust 侧建窗 label 一一对应。
/// 主窗入口是 `ui/src/main.tsx`（顶层文件，不是目录），故天然不在此列。
fn secondary_window_dirs() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(ui_src())
        .expect("ui/src 读不到")
        .flatten()
        .map(|e| e.path())
        .filter(|p| ["main.ts", "main.tsx"].iter().any(|f| p.join(f).is_file()))
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    out.sort();
    out
}

mod secondary_window_capabilities;

#[cfg(target_os = "linux")]
#[test]
fn dialog_acl_covers_every_frontend_dialog_call() {
    let mut files = Vec::new();
    collect_sources(&ui_src(), &mut files);
    assert!(
        !files.is_empty(),
        "没扫到前端源码，测试形同虚设（路径漂了？）"
    );

    let perms = granted(&crate_file("capabilities/default.json"));
    // `dialog:default` 缺 confirm/ask —— 它不能替代逐条授权，撞见即判缺。
    let has = |perm: &str| perms.iter().any(|p| p == perm);

    for (path, src) in &files {
        let need =
            required_dialog_perms(src).unwrap_or_else(|why| panic!("{}：{why}", path.display()));
        for perm in need {
            assert!(
                has(perm),
                "{} 用到 dialog 命令，但 capabilities/default.json 未授 {perm} \
                     → 运行期抛 'not allowed by ACL' 的未捕获 promise rejection",
                path.display()
            );
        }
    }
    // 前身是「生产代码确实还在用 window.confirm」的反向哨兵，用来防本测空转。
    // 2026-07-29 破坏性操作的二次确认改走原地二次点击（`ui/src/lib/confirm-twice.ts`）后
    // 生产代码已无 confirm 调用，该哨兵按它自己写的退役条件退役，换成下面那条**正向不变式**：
    // 生产代码不得再回退到 window.confirm。防空转的职责由本函数开头的 `!files.is_empty()` 承担。
}

/// 生产代码不得再调用 `window.confirm` —— 破坏性操作一律走原地二次点击
/// （`ui/src/lib/confirm-twice.ts` 的 `useConfirmTwice`，对齐原型 confirmTwice L3211）；
/// 需要成段解释的确认走 App 自绘 `ConfirmDialog`。
///
/// 为什么这是一条**产品不变式**而不只是风格偏好：插件 init 脚本把 `window.confirm` 覆写成
/// `plugin:dialog|confirm`，于是「二次确认」这道闸门的成立与否取决于该窗口的 capability 有没有授
/// `dialog:allow-confirm`。漏授时闸门不是降级成「无确认」，而是整条腿抛 rejection ⇒ 用户看到的是
/// 「卸载失败」。真机上就是这么表现的（2026-07-29 于 5.238 复现）。原地二次点击把二次确认从一项
/// 运行期授权变回一段普通渲染，没有这条失败模式。
///
/// 前端侧同名约束已删，本条为唯一判据（扫整个 `ui/src`）。
#[cfg(target_os = "linux")]
#[test]
fn production_code_never_calls_global_confirm() {
    let mut files = Vec::new();
    collect_sources(&ui_src(), &mut files);
    assert!(
        !files.is_empty(),
        "没扫到前端源码，测试形同虚设（路径漂了？）"
    );
    let hits: Vec<String> = files
        .iter()
        .filter(|(_, s)| calls_global(s, "confirm"))
        .map(|(p, _)| {
            p.strip_prefix(ui_src())
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert!(
        hits.is_empty(),
        "这些文件又用回了 window.confirm：{hits:?} —— 二次确认会退化成一项 ACL 授权，\
             漏授时整条腿抛 rejection（用户看到的是「操作失败」）。请改用 useConfirmTwice"
    );
}

/// 收回的授权不许悄悄回来：`dialog:allow-confirm` 已随自绘弹窗退役。
///
/// 单独一条而不是并进上面那测：两者失败的含义不同 —— 上面红 = 有人写回了 confirm 调用；
/// 这条红 = 授权面在没有调用点的情况下被重新放开（纯多余授权面，且会让上面那条的后果重新变得隐蔽）。
#[cfg(target_os = "linux")]
#[test]
fn dialog_confirm_permission_stays_revoked() {
    let perms = granted(&crate_file("capabilities/default.json"));
    assert!(
        !perms.iter().any(|p| p == "dialog:allow-confirm"),
        "capabilities/default.json 又授了 dialog:allow-confirm —— \
             生产代码已无 window.confirm 调用（见 production_code_never_calls_global_confirm），\
             该授权是多余授权面。若确有新增调用点，请先说明为什么不能走 useConfirmTwice"
    );
}

/// 这些插件仍由 Rust 宿主使用，但生产 renderer 没有任何对应 invoke/import。把宿主依赖误读成
/// “前端也要授权”会把配置、文件与进程能力平白暴露给一旦被注入的 webview 脚本。
#[cfg(target_os = "linux")]
#[test]
fn unused_high_risk_renderer_permissions_stay_revoked() {
    let perms = granted(&crate_file("capabilities/default.json"));
    for permission in [
        "process:default",
        "autostart:default",
        "autostart:allow-enable",
        "autostart:allow-disable",
        "autostart:allow-is-enabled",
        "fs:default",
        "fs:allow-read-text-file",
        "fs:allow-write-text-file",
    ] {
        assert!(
            !perms.iter().any(|granted| granted == permission),
            "capabilities/default.json 又授了未使用的 renderer 权限 {permission}；\
                 若新增了真实前端调用，请把调用点与最窄权限一起纳入审查"
        );
    }
}

// ── A2：四态派生（`resolve_tray_state` 的优先级）───────────────────────────────
//
// 断言全部走 `crate::tray::resolve_tray_state`（生产装配里 `reconcile_tray_icon` 调的就是它），
// 不在测试里另写一份判定 —— 否则删掉生产那行 `resolve_tray_state` 测试照样绿。

// ── 托盘点击所有权：mac/win 直派，Linux/未知平台交给原生菜单 ─────────────────────

#[test]
fn main_window_native_menu_is_macos_only() {
    assert_eq!(
        main_window_menu_owner(Platform::Mac),
        MainWindowMenuOwner::NativeApplicationMenu
    );
    for platform in [
        Platform::Win,
        Platform::Linux,
        Platform::Other,
        Platform::Android,
    ] {
        assert_eq!(
            main_window_menu_owner(platform),
            MainWindowMenuOwner::RendererShortcut,
            "{platform:?} 自绘主窗不得挂原生 app menu，否则会多出 Polaris 横栏"
        );
    }

    let src = crate_root_code();
    assert!(
        src.contains("if main_window_menu_owner(Platform::current())"),
        "setup 必须消费菜单所有权判据，不能只写一个无接线的纯函数"
    );
    // 拆开拼词，避免守卫自己的搜索字面量也出现在 `crate_root_code()` 读进来的取材面里。
    // （本处判据曾直接 `include_str!("main.rs")`；装配下沉后取材面换成 `main.rs` + `lib.rs`
    //   两份拼接，拼词纪律是那时留下的，换取材器后仍然要守。）
    let hidden_menu_call = [".hide_", "menu()"].concat();
    assert!(
        !src.contains(&hidden_menu_call),
        "Win/Linux 不应先创建主窗原生菜单再尝试隐藏；应根本不挂菜单"
    );
}

// ── A7：原生菜单 id ↔ 动作解析（菜单与 handler 之间唯一的契约面）─────────────────
