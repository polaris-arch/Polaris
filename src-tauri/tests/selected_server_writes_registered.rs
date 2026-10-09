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

use std::collections::BTreeMap;
use std::path::Path;

use polaris_source_probe::{mask_comments, mask_comments_and_strings, repo_dir_files_in};

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
    /// 系统代选：故障切换、自动选择、删除或刷新之后的兜底。不得清意图。
    System,
    /// 系统代选，且同一个函数里确有对意图键的处置（说明里写明是什么、为什么）。
    SystemTouchingIntent,
    /// 清洗、缺省值、备份恢复：按存储层的规则处置，意图由同一层的清洗跟着处置。
    Restore,
    /// 不落盘：写的是运行态投影或生成期的内存副本。
    Projection,
}

/// 一处登记：(类别, 所在函数, 说明)。
type Site = (Class, &'static str, &'static str);

/// 登记表：文件 → 按出现顺序逐处的登记。
const WRITES: &[(&str, &[Site])] = &[
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
        "src-tauri/src/runtime/config.rs",
        &[(
            Class::System,
            "update_mesh_identity_server_if_revision",
            "组网节点身份事务删除节点时，出口是它则归零",
        )],
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
            (
                Class::Projection,
                "commit_selected_projection",
                "已排程的重启快照里刷新出口",
            ),
            (
                Class::Projection,
                "restart_selected_projection",
                "已排程的重启快照里刷新出口",
            ),
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
    "scripts/qa",
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
                Class::User | Class::Restore | Class::Projection => {}
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
        Class::System,
        Class::SystemTouchingIntent,
        Class::Restore,
        Class::Projection,
    ] {
        assert!(classes.contains(&class), "{class:?} 一条都没有");
    }
    assert_eq!(
        classes
            .iter()
            .filter(|class| **class == Class::User)
            .count(),
        1,
        "用户显式选择只应有一个落盘点；多出一个就多一处要记得处置意图的地方"
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
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                // 依赖缓存、构建产物、第三方源码与以点起手的目录不是本仓的原生源码。
                if !matches!(
                    name.as_ref(),
                    "target" | "node_modules" | "vendor" | "dist" | "build"
                ) && !name.starts_with('.')
                {
                    walk(&path, out);
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
    walk(&root, &mut files);
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
