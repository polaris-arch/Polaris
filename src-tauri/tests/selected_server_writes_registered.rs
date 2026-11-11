//! 写 `selectedServerId` 的位置必须逐处登记，并注明它是哪一类写入、落在哪个函数里。
//!
//! # 守的是什么
//!
//! 选择意图（顶层键 `selectionIntent`）与实际出口（`selectedServerId`）分列。约定只有一条：
//! **用户显式选了出口，意图清回手动；系统代选出口，意图不动。** 漏掉前一半，界面显示「手动」而
//! 自动选择还在换点，或者反过来显示「自动选择」而它早已被一次手动选择绕开；做错后一半，一次
//! 故障切换、一次订阅刷新就把用户选的「自动选择」悄悄关掉。
//!
//! 这条约定没有运行期表征：新增一处写 `selectedServerId` 的代码，无论清不清意图，全部既有测试
//! 照样绿。所以判据取在源码面 —— 每一处写入都必须出现在 [`WRITES`] 里，标明类别与所在函数，
//! 新增一处而不登记即红。登记时就要回答「这是用户的选择还是系统的代选」。
//!
//! # 取材面
//!
//! `src-tauri/src/**` 与 `crates/*/src/**` 的生产 `.rs`（排除 `tests/` 目录与 `tests.rs`）。
//! 先剥注释再取材（字符串字面量保留：键名本身就是字面量）。逐处找键名与强类型字段
//! `selected_server_id`，按前后的代码形态分三堆：
//!
//! - **读**：`.get("selectedServerId")`、`["selectedServerId"]` 后面不是赋值、与它做相等比较、
//!   `x.selected_server_id` 的读取、字段与形参的类型声明。
//! - **写**：`insert(`、`remove(`、`set(&mut x,`、`["selectedServerId"] =`、JSON 字面量里的键、
//!   `.selected_server_id =`、结构体字面量里的 `selected_server_id: <值>`。
//! - **其余**：认不出的形态，以及一切**不是恰好那个带引号字面量**的出现（`"/selectedServerId"`
//!   这样的 JSON 指针、拼进别的字符串里的、抽成常量的）。不许默认当成读 —— 必须登记在
//!   [`OTHER`] 里说明它是什么，否则红。
//!
//! 每一处写入除了数个数，还核对它**所在的函数**与登记的一致：同一个文件里删掉一处、在别的函数
//! 里加一处，个数不变，函数对不上，照样红。登记为系统代选的写入，它所在的函数体里不得出现
//! 意图键（确实要动意图的，登记时显式标出并说明）：把一处系统代选悄悄改成「顺手清意图」会红。
//!
//! # 管不到的（如实写出）
//!
//! - 不提键名的通用写入口：整份保存、顶层键补丁、备份导入的整类合并。它们写什么键由入参决定，
//!   源码里没有这个字面量。它们登记在 [`GENERIC`]，本门只守「这几个入口还在、名字没变」；
//!   它们冲不掉也造不出意图，由 `commands/config.rs` 的后端权威字段表与对应的行为测试守。
//! - 结构体字面量的字段简写（`UserConfig { selected_server_id, .. }`，没有冒号）与整体更新
//!   语法（`..other`）：本词法门不追踪其数据流。测速的临时核配置已有整体更新写法；
//!   `UserConfig` 不被序列化回配置文件，持久配置以 JSON 值落盘。
//! - 同一个函数里删一处又加一处：个数与函数都不变。这种改动没有换入口，分类不变。
//! - 键名被抽成常量之后，常量的每一次使用：抽常量那一刻会因 [`OTHER`] 多出一处而红一次，
//!   登记时必须同时把常量名加进 [`KEY_ALIASES`]，此后常量的出现按键名同样分类。
//! - 类别本身是人填的。本门能核对的只有两条可机判的后果：系统代选的函数不碰意图键；用户显式
//!   选择只有一个落盘点，它的每个调用方都处置了意图。
//!
//! 前端的对应门在 `ui/src/contracts/auto-select.test.ts`（界面写出口的位置逐处登记、键名的
//! 字符串形态逐处登记、界面不写意图）。原生侧见本文件末尾一条。
// 源码文本门：三平台源码逐字一致（`.gitattributes` eol=lf），只在必跑的 Linux 腿执行一次。
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::Path;

use polaris_source_probe::{
    is_downloaded_core_legal_root, mask_comments, mask_comments_and_strings, repo_dir_files_in,
};

const KEY: &str = "selectedServerId";
const QUOTED: &str = "\"selectedServerId\"";
const FIELD: &str = "selected_server_id";
/// 键名的常量别名（今天没有）。键名被抽成常量时把常量名加进来：它的出现按键名同样分类。
const KEY_ALIASES: &[&str] = &[];

/// 一处写入的类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// 用户显式选了出口。写入方必须在同一次写里处置选择意图（清掉，或显式路径明确保留）。
    User,
    /// 用户明确操作原生 Auto：选择 scope 或切换模式，同一 CAS writer 处置意图与真实出口 seed。
    UserAuto,
    /// 系统代选：故障切换、自动选择、删除或刷新之后的兜底。不得清意图。
    System,
    /// 系统代选，且同一个函数里确有对意图键的处置（说明里写明是什么、为什么）。
    SystemTouchingIntent,
    /// 清洗、缺省值、备份恢复：按存储层的规则处置，意图由同一层的清洗跟着处置。
    Restore,
    /// 不落盘：写的是运行态投影或生成期的内存副本。
    Projection,
    /// Host-only fixture writes its own TestDir configuration, not a user choice.
    TestFixture,
    /// Debug native acceptance writes only its newly allocated private runtime.
    AcceptanceFixture,
}

/// 一处登记：(类别, 所在函数, 说明)。
type Site = (Class, &'static str, &'static str);

/// 登记表：文件 → 按出现顺序逐处的登记。
const WRITES: &[(&str, &[Site])] = &[
    (
        "src-tauri/src/native_verification/proxy_carrier.rs",
        &[
            (Class::AcceptanceFixture, "two_node_config_ports", "Construct a fresh private manual two-node configuration, never a user selection"),
            (Class::AcceptanceFixture, "local_only_config", "Construct a private direct/manual configuration for the closed desktop carrier"),
            (Class::AcceptanceFixture, "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation", "Write node-b only to the retained private acceptance runtime through its real ConfigManager transaction"),
        ],
    ),
    (
        "src-tauri/src/runtime/proxy/prerequisite.rs",
        &[(
            Class::TestFixture,
            "ready_api_commit_fixture_for_test",
            "cfg(all(test, not(iOS))) only: seed the owned tests::runtime/TestDir with a real taildrop node for the generated config and Ready commit fixture; no production writer or selection-intent authority.",
        )],
    ),
    (
        "src-tauri/src/runtime/proxy/native_auto.rs",
        &[
            (Class::UserAuto, "native_auto_set_enabled",
             "用户关闭 Auto：冻结已观测实际出口并清 Auto 意图，next 经同一 generation/intent/config CAS writer 落盘，不是系统代选"),
            (Class::UserAuto, "native_auto_set_enabled",
             "核未运行时用户关闭 Auto：沿已验证 eligible 的原 pin 冻结 default 并清选择意图，next 经同一 CAS writer 落盘"),
            (Class::Projection, "native_auto_set_enabled",
             "CAS 成功后已接受 pin 和模式投影到运行内存，不另写磁盘或重选出口"),
            (Class::UserAuto, "native_auto_select_default",
             "用户明确选择Auto scope，CAS写入意图与当前eligible真实出口seed，不运行App竞速"),
            (Class::Projection, "native_auto_select_default",
             "已接受的选择及真实seed投影到当前运行配置副本"),
            (Class::Projection, "native_auto_select_default",
             "运行配置缺选择时显式Null，只作投影，不编造真实出口"),
        ],
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        &[(
            Class::Projection,
            "status_with_owned_stop_context",
            "只把 saved-writer/intent fence 内读取的 selectedServerId 放入 owned Stop 状态凭据，不写配置、不清选择意图",
        )],
    ),
    (
        "crates/config-engine/src/builder/generate.rs",
        &[
            (
                Class::Projection,
                "generate_sing_box_config_with_report_and_runtime_bindings",
                "双态核生成：给内存里的普通态副本设出口，不落盘",
            ),
            (
                Class::Projection,
                "generate_sing_box_config_with_report_and_runtime_bindings",
                "双态核生成：给内存里的组网态副本设出口，不落盘",
            ),
        ],
    ),
    (
        "crates/config-engine/src/user_config/app_config.rs",
        &[(
            Class::Projection,
            "default",
            "强类型 UserConfig 的缺省值（未选）。UserConfig 只用于生成，不被序列化回配置文件",
        )],
    ),
    (
        "crates/store/src/backup.rs",
        &[(
            Class::Restore,
            "merge_categories",
            "备份导入后选中节点已不在节点表：归零。选择意图在同一函数末尾按同样的规则清洗",
        )],
    ),
    (
        "crates/store/src/sanitize.rs",
        &[(
            Class::Restore,
            "sanitize_value_in_place",
            "清洗：类型不对的值删除。选择意图由紧随其后的 sanitize_selection_intent 清洗",
        )],
    ),
    (
        "crates/store/src/store.rs",
        &[(
            Class::Restore,
            "default_config",
            "新建配置的缺省值（未选）；缺省没有意图键",
        )],
    ),
    (
        "src-tauri/src/app_tray.rs",
        &[(
            Class::Projection,
            "reconcile_tray_menu",
            "托盘菜单模型的字段（从配置投影出来给原生托盘渲染），不是配置",
        )],
    ),
    (
        "src-tauri/src/commands/server.rs",
        &[
            (
                Class::System,
                "apply_selection_fallback",
                "删掉当前出口节点后的兜底出口。用户删的是节点，不是在选出口",
            ),
            (
                Class::User,
                "select_in_place",
                "显式选择的唯一落盘点。server_switch_core 随后清意图；自动选择的首次落点与\
                 「立即切换」随后置上或保留自动意图",
            ),
        ],
    ),
    (
        "src-tauri/src/commands/subscription.rs",
        &[
            (
                Class::SystemTouchingIntent,
                "apply_subscription_delete",
                "出口随订阅被删，置直连哨兵。同函数里另有一处清意图：只在意图指向的正是被删的\
                 这个订阅时（意图失去了对象），与出口的兜底无关",
            ),
            (
                Class::System,
                "prefer_auto_exit_after_removal",
                "刷新删掉胜出节点，改选同订阅的候选",
            ),
            (
                Class::System,
                "reconcile_subscription_servers",
                "刷新删掉当前出口后的兜底出口",
            ),
        ],
    ),
    (
        "src-tauri/src/runtime/proxy/auto_switch.rs",
        &[
            (
                Class::Projection,
                "candidate_switch_plan",
                "在运行核基准的副本上换出口，用来重跑热切判定",
            ),
            (
                Class::System,
                "auto_hot_switch_transaction_with_api",
                "后台提交事务的落盘：故障切换与自动选择换点。只改实际出口",
            ),
            (
                Class::System,
                "auto_hot_switch_transaction_with_api",
                "后台提交事务未自证时回滚到旧出口",
            ),
        ],
    ),
    (
        "src-tauri/src/runtime/proxy/hot_switch.rs",
        &[
            (
                Class::Projection,
                "copy_selection_projection",
                "将已准备选择的 selectedServerId/selectionIntent 一同复制或移除到运行快照，不独立选择出口",
            ),
            (
                Class::Projection,
                "switch_selected_server_if_current",
                "把已落盘的显式选择投到运行态副本",
            ),
            (
                Class::Projection,
                "validate_selected_server_candidate_blocking",
                "校验用的运行态副本",
            ),
            (
                Class::Projection,
                "reconcile_persisted_selector_with_api",
                "对账用的运行态副本",
            ),
            // commit_selected_projection publishes a prepared whole config;
            // defer_selected_projection only marks pending. Neither writes this key.
            // The constructing write sites above remain registered independently.
        ],
    ),
];

/// 既不是读也不是写的出现：登记它是什么。
const OTHER: &[(&str, usize, &str)] = &[
    (
        "crates/config-engine/src/user_config/app_config.rs",
        2,
        "强类型字段的 serde 重命名属性，与 UserConfig::FIELD_NAMES 里的键名",
    ),
    (
        "crates/store/src/backup.rs",
        1,
        "DATA_FIELDS 里的键名（备份归类表）",
    ),
];

/// 不提键名的通用写入口：(文件, 函数签名起笔, 说明)。
const GENERIC: &[(&str, &str, &str)] = &[
    (
        "src-tauri/src/commands/config.rs",
        "fn config_save_core(",
        "整份保存（暂存层的保存腿）。出口变化只来自界面删节点后的兜底（ui/src/lib/staged-config.ts），\
         属系统代选；意图键是后端权威字段，以盘上为准",
    ),
    (
        "src-tauri/src/commands/config.rs",
        "fn config_patch_core(",
        "顶层键补丁（含旧的单键写命令）。界面不经它选出口；意图键以盘上为准",
    ),
    (
        "src-tauri/src/commands/config.rs",
        "pub(crate) fn backup_import_save_core(",
        "备份导入：经 merge_categories 合并，属恢复数据；意图键以盘上为准，悬空时由保存前的清洗移除",
    ),
];

/// 原生源码所在的目录（仓库相对）。全仓枚举出来的原生源码必须恰好落在这些目录下：新增一处
/// 原生源码目录而这里不动即红。
const NATIVE_ROOTS: &[&str] = &[
    "crates/tauri-plugin-polaris-ios/ios",
    "src-tauri/gen/android",
    "src-tauri/gen/apple",
];

/// 一处出现的形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Read,
    Write,
    Other,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn ends_with_call(before: &str, callee: &str) -> bool {
    before.strip_suffix('(').is_some_and(|head| {
        head.ends_with(callee) && !head[..head.len() - callee.len()].ends_with(is_ident)
    })
}

/// `set(&mut <标识符>,`：按键名写值的小工具函数的调用形态。
fn ends_with_set_call(before: &str) -> bool {
    let Some(head) = before.strip_suffix(',') else {
        return false;
    };
    let ident_start = head
        .rfind(|c: char| !is_ident(c))
        .map_or(0, |index| index + 1);
    ident_start < head.len() && head[..ident_start].trim_end().ends_with("set(&mut")
}

fn mutable_place(before: &str) -> bool {
    before.rsplit_once("&mut").is_some_and(|(_, place)| {
        !place.trim().is_empty()
            && place
                .chars()
                .all(|c| is_ident(c) || c.is_whitespace() || matches!(c, '.' | '(' | ')'))
    })
}

/// Parentheses can wrap an indexed value or typed field before a mutating method.
fn unwrapped_suffix(after: &str) -> &str {
    after.trim_start_matches(|c: char| c == ')' || c.is_whitespace())
}

fn starts_assignment(rest: &str) -> bool {
    (rest.starts_with('=') && !rest.starts_with("=="))
        || ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="]
            .iter()
            .any(|operator| rest.starts_with(operator))
}

/// 键（带引号的字面量，或它的常量别名）的一处出现属于哪种形态。`before` / `after` 是它前后
/// 的代码（已去掉相邻空白）。
fn shape_of_key(before: &str, after: &str) -> Shape {
    // Exact finite key loop used by copy_selection_projection. Classify it as a
    // write owner (never OTHER/read); renamed or new loop shapes remain unknown.
    if before.ends_with("for key in [") && after.starts_with(',') {
        return Shape::Write;
    }
    if ends_with_call(before, "insert")
        || ends_with_call(before, "remove")
        || ends_with_set_call(before)
        || after.starts_with(':')
    {
        return Shape::Write;
    }
    if before.ends_with('[') {
        if mutable_place(before.trim_end_matches('[')) {
            return Shape::Write;
        }
        return match after.strip_prefix(']').map(unwrapped_suffix) {
            Some(rest) if starts_assignment(rest) => Shape::Write,
            Some(rest) if rest.starts_with('.') => {
                let method = rest.strip_prefix('.').unwrap();
                let name = method
                    .split(|c: char| !is_ident(c))
                    .next()
                    .unwrap_or_default();
                if matches!(
                    name,
                    "as_str"
                        | "as_array"
                        | "as_object"
                        | "as_bool"
                        | "as_u64"
                        | "as_i64"
                        | "as_f64"
                        | "is_null"
                        | "is_string"
                        | "is_array"
                        | "is_object"
                        | "is_boolean"
                        | "is_number"
                        | "clone"
                        | "to_string"
                        | "to_owned"
                        | "get"
                        | "pointer"
                ) {
                    Shape::Read
                } else {
                    Shape::Write
                }
            }
            Some(rest)
                if rest.is_empty()
                    || rest.starts_with([',', ';', '}', ']', '?'])
                    || rest.starts_with("==")
                    || rest.starts_with("!=") =>
            {
                Shape::Read
            }
            _ => Shape::Other,
        };
    }
    if ends_with_call(before, "get") || before.ends_with("==") || after.starts_with("==") {
        return Shape::Read;
    }
    Shape::Other
}

/// 强类型字段名的一处出现是不是写。
fn field_is_written(before: &str, after: &str) -> bool {
    if before.ends_with(is_ident) || after.starts_with(is_ident) {
        // 更长的标识符的一部分（`_selected_server_id` 这样的形参名也落在这里，它后面是类型）。
        return after.starts_with(':')
            && !type_position(&after[1..])
            && !before.ends_with(is_ident);
    }
    if before.ends_with('.') {
        if mutable_place(before) {
            return true;
        }
        // Unknown methods may take &mut self. Only known read methods are exempt.
        let after = unwrapped_suffix(after);
        if let Some(method) = after.strip_prefix('.') {
            let name = method
                .split(|c: char| !is_ident(c))
                .next()
                .unwrap_or_default();
            return !matches!(
                name,
                "clone" | "as_ref" | "as_deref" | "is_some" | "is_none" | "filter"
            );
        }
        return starts_assignment(after);
    }
    // `selected_server_id: <值>`：结构体字面量里的字段初始化是写；后面跟的是类型则是声明。
    after.starts_with(':') && !after.starts_with("::") && !type_position(&after[1..])
}

/// 冒号后面是不是类型（字段或形参的声明）。
fn type_position(after_colon: &str) -> bool {
    let rest = after_colon.trim_start();
    rest.starts_with("Option<") || rest.starts_with('&') || rest.starts_with("String")
}

/// `offset` 所在的函数：它之前最近的一个 `fn <名字>`。`code` 须是注释与字符串都已抹掉的那一份。
fn enclosing_fn(code: &str, offset: usize) -> (usize, &str) {
    let mut search = &code[..offset];
    while let Some(at) = search.rfind("fn ") {
        let preceded_by_ident = search[..at].ends_with(is_ident);
        let name: &str = code[at + 3..]
            .split(|c: char| !is_ident(c))
            .next()
            .unwrap_or_default();
        if !preceded_by_ident && !name.is_empty() {
            return (at, name);
        }
        search = &search[..at];
    }
    (0, "")
}

/// 从 `start` 处的 `fn` 起、到与它的函数体配对的右花括号为止。
fn fn_span(code: &str, start: usize) -> std::ops::Range<usize> {
    let open = start + code[start..].find('{').unwrap_or(0);
    let mut depth = 0usize;
    for (index, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return start..open + index + 1;
                }
            }
            _ => {}
        }
    }
    start..code.len()
}

/// 一处写入：行号、所在函数、函数体里是否出现了意图键。
#[derive(Debug, PartialEq, Eq)]
struct Write {
    line: usize,
    function: String,
    touches_intent: bool,
}

/// 一个文件里的全部写入处，与认不出形态的出现个数。
fn scan(source: &str) -> (Vec<Write>, usize) {
    let code = mask_comments(source);
    let bare = mask_comments_and_strings(source);
    assert_eq!(code.len(), bare.len(), "两份净化面须逐字节对齐");
    let mut offsets: Vec<usize> = Vec::new();
    let mut other = 0;
    // 键名的每一次出现：恰好是那个带引号的字面量才按形态分类，其余一律是「认不出」。
    for (offset, _) in code.match_indices(KEY) {
        let quoted = offset > 0
            && code[offset - 1..].starts_with(QUOTED)
            && !code[..offset - 1].ends_with('\\');
        if !quoted {
            other += 1;
            continue;
        }
        let before = code[..offset - 1].trim_end();
        let after = code[offset - 1 + QUOTED.len()..].trim_start();
        match shape_of_key(before, after) {
            Shape::Write => offsets.push(offset),
            Shape::Read => {}
            Shape::Other => other += 1,
        }
    }
    for alias in KEY_ALIASES {
        for (offset, _) in bare.match_indices(alias) {
            let before = bare[..offset].trim_end();
            let after = bare[offset + alias.len()..].trim_start();
            if before.ends_with(is_ident) || after.starts_with(is_ident) {
                continue;
            }
            match shape_of_key(before, after) {
                Shape::Write => offsets.push(offset),
                Shape::Read => {}
                Shape::Other => other += 1,
            }
        }
    }
    // 强类型字段：取在连字符串也抹掉的那一份上（日志文本里提到字段名不算）。
    for (offset, _) in bare.match_indices(FIELD) {
        let before = bare[..offset].trim_end();
        let after = bare[offset + FIELD.len()..].trim_start();
        if field_is_written(before, after) {
            offsets.push(offset);
        }
    }
    offsets.sort_unstable();
    let writes = offsets
        .into_iter()
        .map(|offset| {
            let (start, function) = enclosing_fn(&bare, offset);
            let body = &code[fn_span(&bare, start)];
            Write {
                line: code[..offset].matches('\n').count() + 1,
                function: function.to_string(),
                touches_intent: body.contains("SELECTION_INTENT_KEY")
                    || body.contains("selectionIntent")
                    || body.split(|c: char| !is_ident(c)).any(|name| {
                        name.contains("selection_intent")
                            && !matches!(
                                name,
                                "selection_intent_subscription" | "effective_selection_intent"
                            )
                    }),
            }
        })
        .collect();
    (writes, other)
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
        && c.contains("ProxyModeType::Manual.effective_on(runtime.helper.platform())")
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
        (2,".effective_on(runtime.helper.platform())",".without_platform_projection()"),
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

const NATIVE_ACCEPTANCE_OWNER: &str = "src-tauri/src/native_verification/proxy_carrier.rs";
fn native_acceptance_writer_is_private(
    path: &str,
    function: &str,
    source: &str,
    faces: &[String; 7],
) -> bool {
    if path != NATIVE_ACCEPTANCE_OWNER {
        return false;
    }
    let compact = |s: &str| {
        mask_comments(s)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let code = mask_comments(source);
    let bare = mask_comments_and_strings(source);
    let head = format!("fn {function}(");
    if bare.matches(&head).count() != 1 {
        return false;
    }
    let body = compact(&code[fn_span(&bare, bare.find(&head).unwrap())]);
    if !closed_native_configuration_scope(
        &faces[0], &faces[1], source, &faces[3], &faces[4], &faces[5], &faces[6],
    ) {
        return false;
    }
    match function {
        "two_node_config_ports" => body.contains("letmutcfg=polaris_store::default_config();")
            && body.contains("obj.insert(\"selectedServerId\".into(),serde_json::json!(selected));")
            && body.contains("obj.insert(\"proxyModeType\".into(),serde_json::json!(\"manual\"));"),
        "local_only_config" => body.contains("serde_json::json!({\"servers\":[],\"selectedServerId\":\"__direct__\",\"proxyMode\":\"direct\",\"proxyModeType\":\"manual\",") ,
        "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation" => body.contains("let(rt,dir,_core,_cleanup)=real_core_runtime();")
            && body.contains("cfg_b[\"selectedServerId\"]=serde_json::json!(\"node-b\");")
            && body.contains("rt.config.update(|current|{*current=cfg_b.clone();Decision::Write(())}).map(drop)") ,
        _ => false,
    }
}

#[test]
fn native_acceptance_class_cannot_be_borrowed_by_user_or_unknown_writers() {
    let faces = native_scope_faces();
    assert_native_scope_controls(&faces);
    let source = faces[2].clone();
    let proxy = faces[0].clone();
    let functions = [
        "two_node_config_ports",
        "local_only_config",
        "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation",
    ];
    let predicate_code = |text: &str| {
        mask_comments(text)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    for function in functions {
        assert!(
            native_acceptance_writer_is_private(NATIVE_ACCEPTANCE_OWNER, function, &source, &faces),
            "private writer: {function}"
        );
        assert!(!native_acceptance_writer_is_private(
            "src-tauri/src/commands/server.rs",
            function,
            &source,
            &faces
        ));
    }
    for (function, old, new) in [
        (
            functions[0],
            "let mut cfg = polaris_store::default_config();",
            "let mut cfg = load_user_config();",
        ),
        (
            functions[1],
            "\"proxyModeType\": \"manual\"",
            "\"proxyModeType\": \"tun\"",
        ),
        (
            functions[2],
            "rt.config\n        .update(|current| {\n            *current = cfg_b.clone();",
            "user_config\n        .update(|current| {\n            *current = cfg_b.clone();",
        ),
    ] {
        let bare = mask_comments_and_strings(&source);
        let head = format!("fn {function}(");
        assert_eq!(
            bare.matches(&head).count(),
            1,
            "unique target function: {function}"
        );
        let span = fn_span(&bare, bare.find(&head).unwrap());
        let body = &source[span.clone()];
        assert_eq!(
            body.matches(old).count(),
            1,
            "negative must bind once inside target body: {function}: {old}"
        );
        let changed_body = body.replacen(old, new, 1);
        assert_ne!(
            predicate_code(body),
            predicate_code(&changed_body),
            "predicate body must change: {function}"
        );
        let changed = format!(
            "{}{}{}",
            &source[..span.start],
            changed_body,
            &source[span.end..]
        );
        let changed_bare = mask_comments_and_strings(&changed);
        assert_eq!(changed_bare.matches(&head).count(), 1);
        let changed_span = fn_span(&changed_bare, changed_bare.find(&head).unwrap());
        assert_eq!(
            &changed[changed_span], changed_body,
            "reassembled target body must carry the mutation"
        );
        assert!(changed.starts_with(&source[..span.start]));
        assert!(changed.ends_with(&source[span.end..]));
        assert!(!native_acceptance_writer_is_private(
            NATIVE_ACCEPTANCE_OWNER,
            function,
            &changed,
            &faces
        ));
    }
    let guard = "#[cfg(debug_assertions)]\n#[path = \"../native_verification/proxy_carrier.rs\"]";
    assert_eq!(proxy.matches(guard).count(), 1);
    let changed_proxy = proxy.replacen(
        guard,
        "#[path = \"../native_verification/proxy_carrier.rs\"]",
        1,
    );
    assert_ne!(predicate_code(&proxy), predicate_code(&changed_proxy));
    let mut changed_faces = faces.clone();
    changed_faces[0] = changed_proxy;
    assert!(!native_acceptance_writer_is_private(
        NATIVE_ACCEPTANCE_OWNER,
        functions[0],
        &source,
        &changed_faces
    ));
    let added = format!("{source}\nfn unknown_writer() {{ let mut cfg = polaris_store::default_config(); cfg[\"selectedServerId\"] = serde_json::json!(\"node-a\"); }}");
    assert_eq!(
        mask_comments_and_strings(&source)
            .matches("fn unknown_writer(")
            .count(),
        0
    );
    assert_eq!(
        mask_comments_and_strings(&added)
            .matches("fn unknown_writer(")
            .count(),
        1
    );
    assert_eq!(scan(&added).0.len(), scan(&source).0.len() + 1);
    for function in functions {
        assert!(native_acceptance_writer_is_private(
            NATIVE_ACCEPTANCE_OWNER,
            function,
            &added,
            &faces
        ));
    }
    assert!(!native_acceptance_writer_is_private(
        NATIVE_ACCEPTANCE_OWNER,
        "unknown_writer",
        &added,
        &faces
    ));
}

/// 取材面：两棵源码树里的生产 `.rs`。
fn production_sources() -> Vec<(String, String)> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut files = repo_dir_files_in(manifest, "src-tauri/src", "rs");
    files.extend(repo_dir_files_in(manifest, "crates", "rs"));
    files.retain(|(path, _)| {
        !path.contains("/tests/") && !path.ends_with("/tests.rs") && path.contains("/src/")
    });
    files
}

#[test]
fn every_write_of_the_selected_server_is_registered_with_its_class_and_function() {
    let sources = production_sources();
    let native_faces = native_scope_faces();
    // 取材面下限：扫描器坏了、目录改名了，表现为扫到的文件骤减而变红，不会空跑判绿。
    assert!(
        sources.len() > 300,
        "只取到 {} 个生产源文件，取材面塌了",
        sources.len()
    );
    let registered: BTreeMap<&str, &[Site]> =
        WRITES.iter().map(|(path, sites)| (*path, *sites)).collect();
    assert_eq!(registered.len(), WRITES.len(), "登记表里有重复的文件");
    let other: BTreeMap<&str, usize> = OTHER
        .iter()
        .map(|(path, count, _)| (*path, *count))
        .collect();

    let mut found_writes = 0;
    let mut mentions = 0;
    for (path, source) in &sources {
        let (writes, unknown) = scan(source);
        mentions += usize::from(source.contains(QUOTED));
        found_writes += writes.len();
        let expected = registered.get(path.as_str()).copied().unwrap_or(&[]);
        let found: Vec<(usize, &str)> = writes
            .iter()
            .map(|write| (write.line, write.function.as_str()))
            .collect();
        assert_eq!(
            found
                .iter()
                .map(|(_, function)| *function)
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|(_, function, _)| *function)
                .collect::<Vec<_>>(),
            "{path}：扫到的写 selectedServerId 的位置（行, 函数）是 {found:?}，与登记表不符。\
             新增或挪动的写入先回答：这是用户显式选了出口（须在同一次写里处置选择意图），还是\
             系统代选（不得清意图），再登记到本文件的 WRITES。"
        );
        for (write, (class, function, _)) in writes.iter().zip(expected) {
            match class {
                Class::System => assert!(
                    !write.touches_intent,
                    "{path}:{}：`{function}` 登记为系统代选，函数体里却出现了选择意图的键。\
                     系统代选不得清意图；确实另有理由要在这里处置意图，就把类别改成 \
                     SystemTouchingIntent 并在说明里写明是什么、为什么。",
                    write.line
                ),
                Class::SystemTouchingIntent => assert!(
                    write.touches_intent,
                    "{path}:{}：`{function}` 登记为「系统代选且处置意图」，函数体里却没有意图键：\
                     登记已过期，改回 System。",
                    write.line
                ),
                Class::UserAuto => assert!(
                    write.touches_intent,
                    "{path}:{}: explicit native Auto writer must settle selection intent in the same function",
                    write.line
                ),
                Class::User | Class::Restore | Class::Projection => {}
                Class::AcceptanceFixture => {
                    assert!(!write.touches_intent);
                    assert!(native_acceptance_writer_is_private(path, function, source, &native_faces),
                        "acceptance classification escaped its exact private writer: {path}::{function}");
                }
                Class::TestFixture => {
                    let bare = mask_comments(source);
                    assert!(bare.contains(
                        "#[cfg(all(test, not(target_os = \"ios\")))]\n    pub(crate) fn ready_api_commit_fixture_for_test("
                    ));
                    assert!(source.contains("let (runtime, directory) = tests::runtime();"));
                    assert_eq!(*function, "ready_api_commit_fixture_for_test");
                }
            }
        }
        assert_eq!(
            unknown,
            other.get(path.as_str()).copied().unwrap_or(0),
            "{path}：有 {unknown} 处 selectedServerId 既不是已知的读形态也不是已知的写形态\
             （含不是恰好那个带引号字面量的出现，如 JSON 指针）。是新的写法就登记到 WRITES 并把\
             形态加进 shape_of_key；确实不是读写就登记到 OTHER。"
        );
    }
    // 登记表里的每个文件都真的在取材面里（文件改名后条目成了摆设即红）。
    let listed = WRITES
        .iter()
        .map(|(path, _)| *path)
        .chain(OTHER.iter().map(|(path, _, _)| *path));
    for path in listed {
        assert!(
            sources.iter().any(|(file, _)| file == path),
            "登记表里的 {path} 不在取材面里"
        );
    }
    let registered_total: usize = registered.values().map(|sites| sites.len()).sum();
    assert_eq!(found_writes, registered_total);
    assert!(mentions >= 20, "只有 {mentions} 个文件提到键名，取材面塌了");

    // 类别不是装饰：每一类都真的有条目，且「用户显式选择」只有一处落盘点。
    let classes: Vec<Class> = WRITES
        .iter()
        .flat_map(|(_, sites)| sites.iter().map(|(class, _, _)| *class))
        .collect();
    for class in [
        Class::User,
        Class::UserAuto,
        Class::System,
        Class::SystemTouchingIntent,
        Class::Restore,
        Class::Projection,
        Class::TestFixture,
        Class::AcceptanceFixture,
    ] {
        assert!(classes.contains(&class), "{class:?} 一条都没有");
    }
    assert_eq!(
        classes
            .iter()
            .filter(|class| **class == Class::User)
            .count(),
        1,
        "既有手动选择只应有一个落盘点；原生Auto scope另有精确登记"
    );
    let user_auto: Vec<(&str, &str)> = WRITES
        .iter()
        .flat_map(|(path, sites)| {
            sites.iter().filter_map(move |(class, function, _)| {
                (*class == Class::UserAuto).then_some((*path, *function))
            })
        })
        .collect();
    assert_eq!(
        user_auto,
        vec![
            (
                "src-tauri/src/runtime/proxy/native_auto.rs",
                "native_auto_set_enabled"
            ),
            (
                "src-tauri/src/runtime/proxy/native_auto.rs",
                "native_auto_set_enabled"
            ),
            (
                "src-tauri/src/runtime/proxy/native_auto.rs",
                "native_auto_select_default"
            ),
        ]
    );
}

/// 「用户显式选择」那一个落盘点的三个调用方各自处置了意图：点节点的清掉，自动选择的置上，
/// 立即切换的保留（不清、不改）。
#[test]
fn the_single_explicit_write_has_three_callers_that_each_settle_the_intent() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let source = polaris_source_probe::crate_source_in(manifest, "commands/server.rs");
    let code = mask_comments(&source);
    let bare = mask_comments_and_strings(&source);
    let body_of = |head: &str| {
        let start = bare
            .find(head)
            .unwrap_or_else(|| panic!("找不到 `{head}`：函数改名后本门的取材锚点失效"));
        &code[fn_span(&bare, start)]
    };
    assert_eq!(
        bare.matches("select_in_place(").count(),
        4,
        "一处定义加三处调用；新增调用方须先说明它如何处置选择意图"
    );
    let manual = body_of("fn server_switch_core<F>(");
    assert!(manual.contains("select_in_place("));
    assert!(
        manual.contains("obj.remove(polaris_store::SELECTION_INTENT_KEY)"),
        "手动选择必须在同一次写里清意图"
    );
    let enable = body_of("fn auto_select_enable_core(");
    assert!(enable.contains("select_in_place("));
    assert!(enable.contains("polaris_store::selection_intent_auto(subscription_id)"));
    let now = body_of("fn auto_select_switch_now_core(");
    assert!(now.contains("select_in_place("));
    assert!(
        !now.contains("SELECTION_INTENT_KEY") && !now.contains("selection_intent_auto"),
        "立即切换不动意图"
    );
    // 两条显式命令的目标都取自裁决，不另取候选。
    for body in [enable, now] {
        assert!(body.contains("decide(cfg, "), "落点必须取自裁决");
    }
}

/// 通用写入口还在、名字没变。
#[test]
fn the_generic_writers_are_still_where_the_register_says() {
    for (path, head, _) in GENERIC {
        let relative = path.trim_start_matches("src-tauri/src/");
        let source = polaris_source_probe::crate_source_in(env!("CARGO_MANIFEST_DIR"), relative);
        assert!(
            source.contains(head),
            "{path} 里找不到 `{head}`：通用写入口改名或拆分后，重新核对它对选择意图的处置并更新 GENERIC"
        );
    }
    let config =
        polaris_source_probe::crate_source_in(env!("CARGO_MANIFEST_DIR"), "commands/config.rs");
    assert!(
        config.contains("polaris_store::SELECTION_INTENT_KEY,"),
        "选择意图必须在后端权威字段表里：通用写入口靠它冲不掉也造不出意图"
    );
}

/// 形态分类的自检：每种写法与读法各一例；注释不进取材面；JSON 指针、拼在别的字符串里的键名、
/// 认不出的调用都落进「其余」；所在函数与「函数体里有没有意图键」取得对。分类器哑了这里先红。
#[test]
fn the_shape_classifier_tells_reads_writes_and_unknowns_apart() {
    let fixture = r#"
        fn writes(obj: &mut Map, raw: &mut Value, config: &mut UserConfig) {
            // obj.insert("selectedServerId".to_string(), json!(x));   注释里的不算
            obj.insert("selectedServerId".to_string(), json!(x));
            object.insert(
                "selectedServerId".into(),
                selected,
            );
            obj.remove("selectedServerId");
            set(&mut result, "selectedServerId", Value::Null);
            raw["selectedServerId"] = Value::Null;
            let d = json!({ "selectedServerId": null });
            config.selected_server_id = Some(id);
        }
        fn literal() -> UserConfig {
            obj.remove(polaris_store::SELECTION_INTENT_KEY);
            UserConfig { selected_server_id: Some(id), ..other }
        }
        struct Declared { pub selected_server_id: Option<String>, name: String }
        fn reads(selected_server_id: Option<&str>, _selected_server_id: Option<&str>) {
            let a = cfg.get("selectedServerId").and_then(Value::as_str);
            let b = raw["selectedServerId"].as_str();
            if raw["selectedServerId"] == other {}
            if k.as_str() == "selectedServerId" {}
            if config.selected_server_id == other.selected_server_id {}
            let selected_server_id = config.selected_server_id.clone();
            log::info!("selected_server_id: {x}");
        }
        fn unknown() {
            const FIELDS: [&str; 1] = ["servers", "selectedServerId"];
            obj.entry("selectedServerId");
            raw.pointer_mut("/selectedServerId");
            let text = "the selectedServerId key";
            reinsert("selectedServerId");
        }
    "#;
    let (writes, other) = scan(fixture);
    let functions: Vec<&str> = writes.iter().map(|w| w.function.as_str()).collect();
    assert_eq!(
        functions,
        ["writes", "writes", "writes", "writes", "writes", "writes", "writes", "literal"],
        "七种写法各一处，外加结构体字面量一处"
    );
    assert!(writes[..7].iter().all(|write| !write.touches_intent));
    assert!(writes[7].touches_intent, "函数体里有意图键");
    assert_eq!(
        other, 5,
        "数组元素、entry(、JSON 指针、拼在别的字符串里的、reinsert( 都须登记而不是放过"
    );
    assert_eq!(scan("let x = cfg.get(\"selectedServerId\");"), (vec![], 0));
    for operation in [
        "take()",
        "clone_from(&next)",
        "replace(next)",
        "as_str_mut()",
        "unknown_mutator()",
    ] {
        let source = format!("fn change() {{ cfg[\"selectedServerId\"].{operation}; }}");
        assert_eq!(scan(&source).0.len(), 1, "{operation}");
    }
    for method in ["as_str()", "clone()", "as_array()"] {
        assert_eq!(
            scan(&format!(
                "fn read() {{ cfg[\"selectedServerId\"].{method}; }}"
            )),
            (vec![], 0)
        );
    }
    for place in [
        "cfg[\"selectedServerId\"]",
        "cfg.selected_server_id",
        "cfg. selected_server_id",
    ] {
        for wrapped in [format!("({place})"), format!("(( {place} ))")] {
            for operation in ["take()", "clone_from(&next)", "unknown_mutator()"] {
                assert_eq!(
                    scan(&format!("fn change() {{ {wrapped}.{operation}; }}"))
                        .0
                        .len(),
                    1,
                    "wrapped mutation: {wrapped}.{operation}"
                );
            }
            assert_eq!(
                scan(&format!("fn read() {{ let v = {wrapped}.clone(); }}")),
                (vec![], 0)
            );
            assert_eq!(
                scan(&format!("fn read() {{ inspect({wrapped}); }}")),
                (vec![], 0)
            );
            assert_eq!(
                scan(&format!("fn change() {{ replace(&mut {wrapped}, next); }}"))
                    .0
                    .len(),
                1
            );
        }
    }
    assert_eq!(
        scan("fn unknown() { cfg[\"selectedServerId\"] as Opaque; }").1,
        1
    );
    assert_eq!(
        scan("fn change() { replace(&mut cfg[\"selectedServerId\"], next); }")
            .0
            .len(),
        1
    );
    assert_eq!(
        scan("fn change() { replace(&mut cfg.selected_server_id, next); }")
            .0
            .len(),
        1
    );
    assert_eq!(
        scan("fn change() { cfg[\"selectedServerId\"] += next; }")
            .0
            .len(),
        1
    );
    assert!(scan("// raw[\"selectedServerId\"] = 1;").0.is_empty());
    // 所在函数：取最近的那个 `fn`，不被 `impl Fn(…)` 或标识符里的 `fn` 骗过。
    let nested = "fn outer(f: impl Fn(u8)) { let defn = 1; obj.remove(\"selectedServerId\"); }";
    assert_eq!(scan(nested).0[0].function, "outer");
    for method in [
        "take()",
        "clone_from(&other)",
        "replace(id)",
        "insert(id)",
        "get_or_insert(id)",
        "unknown_mutation()",
    ] {
        let source = format!("fn mutate() {{ cfg.selected_server_id.{method}; }}");
        assert_eq!(
            scan(&source).0.len(),
            1,
            "method {method} must be registered"
        );
    }
    let via_helper =
        "fn system() { cfg.selected_server_id.take(); sanitize_selection_intent(cfg); }";
    assert!(scan(via_helper).0[0].touches_intent);
    let read_helper =
        "fn system() { cfg.selected_server_id.take(); selection_intent_subscription(cfg); }";
    assert!(!scan(read_helper).0[0].touches_intent);
}

/// 原生侧（Kotlin / Swift）不读不写这个键：出口的选择只经 Rust 的命令。取材面是全仓枚举出来的
/// 原生源码 —— 不在 [`NATIVE_ROOTS`] 登记的目录下出现原生源码即红，登记了而已不存在的也红。
#[test]
fn native_code_never_touches_the_selected_server() {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                let relative = path
                    .strip_prefix(root)
                    .expect("walk remains inside workspace")
                    .to_string_lossy()
                    .replace('\\', "/");
                if is_downloaded_core_legal_root(&relative) {
                    continue;
                }
                // 依赖缓存、构建产物、第三方源码与以点起手的目录不是本仓的原生源码。
                if !matches!(
                    name.as_ref(),
                    "target" | "node_modules" | "vendor" | "dist" | "build"
                ) && !name.starts_with('.')
                {
                    walk(root, &path, out);
                }
            } else if name.ends_with(".kt") || name.ends_with(".swift") || name.ends_with(".java") {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 必有上级目录")
        .to_path_buf();
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    assert!(
        files.len() >= 20,
        "只扫到 {} 个原生源文件，取材面塌了",
        files.len()
    );
    let mut seen_roots = std::collections::BTreeSet::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let native_root = NATIVE_ROOTS
            .iter()
            .find(|native_root| relative.starts_with(&format!("{native_root}/")))
            .unwrap_or_else(|| {
                panic!(
                    "{relative} 不在登记的原生源码目录下：新增的原生源码目录先加进 NATIVE_ROOTS，\
                     它随即进入本门的取材面"
                )
            });
        seen_roots.insert(*native_root);
        let source = std::fs::read_to_string(file).unwrap_or_default();
        assert!(
            !source.contains("selectedServerId") && !source.contains("selectionIntent"),
            "{relative} 提到了出口或选择意图的键：原生侧不得直接读写它们"
        );
    }
    assert_eq!(
        seen_roots.into_iter().collect::<Vec<_>>(),
        NATIVE_ROOTS.to_vec(),
        "登记的原生源码目录里有的已经没有源码了"
    );
}

#[test]
fn finite_selection_projection_loop_is_a_registered_write_owner() {
    let source = r#"fn copy_selection_projection(destination: &mut Value, source: &Value) {
        for key in ["selectedServerId", "selectionIntent"] {
            if let Some(value) = source.get(key) {
                destination.as_object_mut().unwrap().insert(key.into(), value.clone());
            }
        }
    }"#;
    let (writes, unknown) = scan(source);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].function, "copy_selection_projection");
    assert!(writes[0].touches_intent);
    assert_eq!(unknown, 0);
    let changed = source.replace("for key in", "for field in");
    assert_eq!(
        scan(&changed).1,
        1,
        "unrecognized dynamic key loops must not be treated as reads"
    );
}

/// Recognize the already-existing native Auto transaction, not a permissive writer count.
fn native_auto_transaction_is_guarded(body: &str) -> bool {
    let code = mask_comments(body);
    let bare = mask_comments_and_strings(body);
    let anchor = ".update(|latest| {";
    if bare.matches(anchor).count() != 1 {
        return false;
    }
    let start = bare.find(anchor).unwrap();
    let range = fn_span(&bare, start);
    let closure_code = &code[range.clone()];
    let closure_bare = &bare[range];
    let compact: String = closure_bare
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let seed_head = "if let Some(seed) = &seed {";
    if closure_bare.matches(seed_head).count() != 1 {
        return false;
    }
    let seed_range = fn_span(closure_bare, closure_bare.find(seed_head).unwrap());
    let seed_code: String = closure_code[seed_range]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if seed_code != "ifletSome(seed)=&seed{latest[\"selectedServerId\"]=json!(seed);}" {
        return false;
    }
    let guard = "if*latest!=saved||self.core_generation()!=started_generation||self.core_running()!=started_running||self.selector_reconcile.intent_generation()!=started_intent{returnDecision::Skip(None);}";
    let intent = "latest[polaris_store::SELECTION_INTENT_KEY]=intent.clone();";
    let seed = "ifletSome(seed)=&seed{latest[]=json!(seed);}";
    let commit = "Decision::Write(Some(self.register_selector_intent()))";
    let positions: Option<Vec<usize>> = [guard, intent, seed, commit]
        .iter()
        .map(|part| (compact.matches(part).count() == 1).then(|| compact.find(part).unwrap()))
        .collect();
    positions.is_some_and(|positions| positions.windows(2).all(|pair| pair[0] < pair[1]))
}

#[test]
fn explicit_native_auto_seed_and_intent_share_the_guarded_writer() {
    let source = polaris_source_probe::crate_source_in(
        env!("CARGO_MANIFEST_DIR"),
        "runtime/proxy/native_auto.rs",
    );
    let code = mask_comments(&source);
    let bare = mask_comments_and_strings(&source);
    let signature = "pub(crate) async fn native_auto_select_default(";
    assert_eq!(bare.matches(signature).count(), 1);
    let body = &code[fn_span(&bare, bare.find(signature).unwrap())];
    assert!(native_auto_transaction_is_guarded(body));
    let body_bare = mask_comments_and_strings(body);
    let guard_range = fn_span(&body_bare, body_bare.find("if *latest != saved").unwrap());
    let guard_text = &body[guard_range.clone()];
    let fake_guard = format!("let _ = {:?};", guard_text);
    let mut forged = body.to_owned();
    forged.replace_range(guard_range, &fake_guard);
    assert!(
        !native_auto_transaction_is_guarded(&forged),
        "string text cannot replace the live CAS guard"
    );
    let compact: String = body_bare.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("letseed=ifself.core_running(){self.native_auto_status().await.actual_server_id.filter(|id|model::eligible_members(&typed,&target).contains(id))}else{None};"));
    for (old, replacement) in [
        (
            "latest[polaris_store::SELECTION_INTENT_KEY] = intent.clone();",
            "",
        ),
        ("latest[\"selectedServerId\"] = json!(seed);", ""),
        ("self.core_generation() != started_generation", "false"),
        ("self.core_running() != started_running", "false"),
        (
            "self.selector_reconcile.intent_generation() != started_intent",
            "false",
        ),
        ("return Decision::Skip(None);", "record(None);"),
    ] {
        assert!(body.contains(old), "mutation anchor {old}");
        assert!(
            !native_auto_transaction_is_guarded(&body.replacen(old, replacement, 1)),
            "{old}"
        );
    }
    let outside = body.replacen(
        "latest[polaris_store::SELECTION_INTENT_KEY] = intent.clone();",
        "",
        1,
    );
    let outside =
        format!("latest[polaris_store::SELECTION_INTENT_KEY] = intent.clone();\n{outside}");
    assert!(
        !native_auto_transaction_is_guarded(&outside),
        "intent before the writer does not qualify"
    );
}
