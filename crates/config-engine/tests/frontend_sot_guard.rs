//! **SoT 守门**：证明前端不再持有 Rust 已拥有的数据表。
//!
//! # 这扇门的形态与「对拍」不同
//!
//! 对拍的前提是两侧都有表。本批把表收敛到 Rust 一份后，门的形态变成**证明另一侧不存在** ——
//! 前端若再长出一张同构表（复制粘贴 / 回退 commit / 新人不知情地"补个常量"），本门转红。
//!
//! 迁移当时的对拍证据（一次性、跑完即删的 `zz_migration_parity_probe.rs`）：
//! 预设 16/16、catalog 33/33 逐字段全等，且对 Rust 表做变异验证 6/6 转红 → 确认 Rust 那份是
//! 忠实转录而非有损手抄，方才删 TS 表。本门是那份证据的**常驻延续**。
//!
//! # 为何扫描前必须剥注释
//!
//! 首版本门直接 `src.contains("APP_PRESETS")` → **被自己的文档注释误伤**：新文件头写着
//! 「这里曾有 `APP_PRESETS` 全表……」正是在解释迁移，却被判成「又长出第二份表」。
//! 会对散文误报的门，最后一定是被人删掉、而不是被人遵守。故扫描前剥掉块注释与整行行注释：
//! **只看代码，不看散文**。

/// 剥 TS 注释：`/* … */`（含 JSDoc `/** … */`）+ 整行 `//` 行注释。
///
/// 刻意**不**处理行尾 `//`：TS 里 `https://` 与正则 `/\//` 都含 `//`，按行尾切会误伤代码；
/// 而本门只需判「标识符是否出现在代码里」，散文全在块注释与整行注释中，剥这两类已足够。
fn strip_ts_comments(src: &str) -> String {
    let mut out = String::new();
    let mut chars = src.chars().peekable();
    let mut in_block = false;
    while let Some(c) = chars.next() {
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block = true;
            continue;
        }
        out.push(c);
    }
    out.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn frontend_code(rel: &str) -> String {
    let path = format!("{}/../../ui/src/{rel}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到前端文件 {path}: {e}（文件移动请同步本测试）"));
    strip_ts_comments(&src)
}

#[test]
fn strip_ts_comments_removes_prose_but_keeps_code() {
    // 门的自检：剥注释器本身得对，否则整个门要么恒绿（假安全）要么恒红。
    let src = r#"
/**
 * 历史：这里曾有 APP_PRESETS 全表。
 */
// 行注释里也提 APP_PRESETS
const x = 1;
const re = /\/(geo|geo-lite)\//i; // 行尾注释含 APP_PRESETS
export const KEEP_ME = 2;
"#;
    let out = strip_ts_comments(src);
    assert!(!out.contains("历史"), "块注释未剥净");
    assert!(!out.contains("行注释里也提"), "整行行注释未剥净");
    assert!(out.contains("const x = 1;"), "代码被误删");
    assert!(out.contains("KEEP_ME"), "代码被误删");
    assert!(out.contains("geo-lite"), "含 // 的正则代码被误删");
}

#[test]
fn frontend_holds_no_second_preset_table() {
    // 前端曾持有 APP_PRESETS 全表 + QURE_BASE 图标基址（16 条 ↔ Rust 16 条，两处维护必然漂移，
    // 且 Rust 那份文件头还自认「来源 = 本 TS 文件」→ 两边都以为对方是抄本）。
    let code = frontend_code("domain/app-rules-preset.ts");
    for marker in ["APP_PRESETS", "QURE_BASE", "jsdelivr", "geositeTags: ["] {
        assert!(
            !code.contains(marker),
            "前端 app-rules-preset.ts 的**代码**里又出现 {marker:?} —— 内置预设表的 SoT 在 Rust \
             (config-engine/user_config/app_rules_preset_data.rs)，前端只应经 app_presets_list 拉取"
        );
    }
    // 反向自检：文件被清空/改名时上面的 !contains 会平凡通过 → 断言它仍是那个模块。
    assert!(
        code.contains("getAppPreset") && code.contains("mergeAppPresets"),
        "前端 app-rules-preset.ts 已非预期模块，本门失效"
    );
}

#[test]
fn frontend_holds_no_second_catalog_table() {
    let code = frontend_code("domain/rule-resource-catalog.ts");
    for marker in [
        "RULE_RESOURCE_CATALOG",
        "MRD_RAW_BASE",
        "raw.githubusercontent.com",
    ] {
        assert!(
            !code.contains(marker),
            "前端 rule-resource-catalog.ts 的**代码**里又出现 {marker:?} —— catalog 的 SoT 在 Rust \
             (config-engine/user_config/rule_resource_catalog.rs)，前端只应经 rule_resources_get_catalog 拉取"
        );
    }
    assert!(
        code.contains("deriveResourceMeta"),
        "前端 rule-resource-catalog.ts 已非预期模块，本门失效"
    );
}

#[test]
fn frontend_screen_renders_from_store_not_static_table() {
    // AppPolicyScreen 曾 `import { APP_PRESETS }` 直连静态表，且**内联重实现**了一份 merge
    // （与 getAppPreset 那份行为还不一致 —— category 一个用占位 'tools' 一个用真值）。
    let code = frontend_code("components/screens/app-policy/AppPolicyScreen.tsx");
    assert!(
        !code.contains("APP_PRESETS"),
        "AppPolicyScreen 又直连静态预设表 —— 应订阅 useAppPresetsStore（Rust SoT）"
    );
    assert!(
        code.contains("mergeAppPresets") && code.contains("useAppPresetsStore"),
        "AppPolicyScreen 未走 store + 单一 merge 收口 —— 内联重实现 merge 是本批修掉的前置坑"
    );
}

/// 判断 TS 代码里是否**声明**了字段 `name`（形如 `name: T` 或 `name?: T`）。
///
/// 刻意不用 `code.contains(name)`：**首版就是这么写的，被变异验证抓了个假阴性** —— 把 Rust DTO 的
/// `labelKey` 改名成 `label`，`contains("label")` 仍命中前端的 `labelKey` → 门恒绿。
/// 故此处两侧都卡死：前面不能接标识符字符（`label` 不得命中 `labelKey`），后面必须是 `:` 或 `?:`。
fn declares_field(code: &str, name: &str) -> bool {
    code.match_indices(name).any(|(i, _)| {
        let before_ok = code[..i]
            .chars()
            .last()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_' && c != '$');
        let mut rest = code[i + name.len()..].chars();
        let after_ok = match rest.next() {
            Some(':') => true,
            Some('?') => rest.next() == Some(':'),
            _ => false,
        };
        before_ok && after_ok
    })
}

#[test]
fn declares_field_rejects_substring_of_longer_identifier() {
    // 门的自检（这条正是变异验证 M4 逼出来的）。
    let code = "export interface AppPreset { labelKey: string; iconUrl?: string; id: string; }";
    assert!(declares_field(code, "labelKey"));
    assert!(declares_field(code, "iconUrl"), "`?:` 形态应认");
    assert!(declares_field(code, "id"));
    assert!(!declares_field(code, "label"), "label 不得命中 labelKey");
    assert!(!declares_field(code, "icon"), "icon 不得命中 iconUrl");
    assert!(!declares_field(code, "AppPreset"), "类型名不是字段声明");
}

#[test]
fn app_presets_list_channel_is_synced_across_three_places() {
    // §N（灾难级）的复发面：前端 138 个 channel 常量里 136 个含冒号（Electron 遗产），
    // 能对上 Rust command 名的 **0 个** → 真机 `Command config:get not found`，而
    // **tsc 绿、vite build 绿**（类型全对，只是运行时名字对不上），错误又被 `.catch(()=>{})` 吞掉。
    //
    // 「加 command 要同步三处」靠人记 = 迟早再断。本门把三处钉死：
    //   ① 前端 channel 常量值  ② Rust `#[tauri::command]` fn 名  ③ generate_handler! 实际注册
    // 声明了没注册 = 一样调不到，故 ③ 必须单独验。
    const FN: &str = "app_presets_list";

    let root = env!("CARGO_MANIFEST_DIR");
    let channels = std::fs::read_to_string(format!("{root}/../../ui/src/domain/ipc-channels.ts"))
        .expect("读 ipc-channels.ts");
    let cmd_src =
        std::fs::read_to_string(format!("{root}/../../src-tauri/src/commands/rules/crud.rs"))
            .expect("读 commands/rules/crud.rs");
    let main_rs =
        std::fs::read_to_string(format!("{root}/../../src-tauri/src/main.rs")).expect("读 main.rs");

    // ① 前端常量值必须**恰好**是 snake_case fn 名。
    assert!(
        channels.contains(&format!("APP_PRESETS_LIST: '{FN}'")),
        "前端 APP_PRESETS_LIST 的值必须是 '{FN}'（= Rust fn 名）"
    );
    // Tauri command 名 = Rust 标识符 → 冒号不合法。（event 名才是自由字符串，冒号合法，勿混。）
    let line = channels
        .lines()
        .find(|l| l.contains("APP_PRESETS_LIST:"))
        .expect("找不到 APP_PRESETS_LIST 常量");
    let value = line.split('\'').nth(1).expect("常量值解析失败");
    assert!(
        !value.contains(':'),
        "channel 值 {value:?} 含冒号 —— Tauri command 名 = Rust 函数名，冒号在标识符里不合法"
    );

    // ② Rust 侧确实有这个 #[tauri::command]。
    assert!(
        cmd_src.contains(&format!("pub fn {FN}(")),
        "commands/rules/crud.rs 里找不到 `pub fn {FN}`"
    );
    // ③ generate_handler! 里确实注册了（声明≠注册）。
    assert!(
        main_rs.contains(&format!("            {FN},")),
        "main.rs 的 generate_handler![] 未注册 {FN} —— 声明了没注册，前端一样调不到"
    );
}

#[test]
fn frontend_preset_interface_matches_rust_dto_fields() {
    // DTO 键名 = 前端 AppPreset interface 字段名。任一侧改名而另一侧未跟 → 前端拿不到数据，
    // 且 **tsc 不报**（invoke 返回值是 as-cast）→ 只能靠本门守。
    let code = frontend_code("domain/app-rules-preset.ts");
    let dto = &polaris_config_engine::user_config::all_presets_dto()[0];
    let json = serde_json::to_value(dto).expect("DTO 序列化");
    for key in json.as_object().expect("DTO 应为对象").keys() {
        assert!(
            declares_field(&code, key),
            "Rust DTO 有字段 {key:?}，但前端 AppPreset interface **未声明**它 → 跨语言漂移\
             （前端渲染会拿到 undefined，且 tsc 不报）"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 配置默认值：前端一条兜底都不许写
// ─────────────────────────────────────────────────────────────────────────────

/// 递归收集 `ui/src` 下的 `.ts` / `.tsx`（跳过测试与 `node_modules`）。
///
/// **取材面必须是整个 `ui/src`，不能只盯 `SettingsTun.tsx`**：这个 bug 的两次发生分别在
/// 「网络」与「TUN」两个屏，盯单文件的门对第三个屏结构性失明。
fn frontend_sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if name == "node_modules" || name == "__tests__" {
                    continue;
                }
                walk(&path, out);
                continue;
            }
            let is_source = name.ends_with(".ts") || name.ends_with(".tsx");
            // 测试自己会构造缺省态做断言，不该被本门管。
            let is_test = name.contains(".test.") || name.contains(".spec.");
            if is_source && !is_test {
                if let Ok(src) = std::fs::read_to_string(&path) {
                    out.push((path.display().to_string(), strip_ts_comments(&src)));
                }
            }
        }
    }
    let root = format!("{}/../../ui/src", env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    walk(std::path::Path::new(&root), &mut out);
    assert!(
        out.len() > 100,
        "只扫到 {} 个前端源文件 —— 取材面塌了（路径变了？），本门此刻是假绿",
        out.len()
    );
    out
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// 在 `code` 里找 `leaf` 的**读取点自带兜底**，命中返回那一小段原文。
///
/// 三种兜底形态都要认（只认 `??` 会漏掉另外两种）：
///   - `x.leaf ?? […]`  空值合并
///   - `x.leaf || […]`  逻辑或
///   - `{ leaf = […] }` 解构默认值
///
/// 跨行也要认（`x.leaf\n  ?? […]`），故向后跳过空白**含换行**再判，不做逐行切分。
fn find_default_fallback(code: &str, leaf: &str) -> Option<String> {
    let bytes = code.as_bytes();
    for (idx, _) in code.match_indices(leaf) {
        // 标识符边界：`leaf` 不得是更长标识符的一截（`stack` 不许命中 `stackMode`）。
        let before_ok = idx == 0 || !is_ident_char(code[..idx].chars().next_back().unwrap_or(' '));
        let after = idx + leaf.len();
        let after_ok =
            after >= bytes.len() || !is_ident_char(code[after..].chars().next().unwrap_or(' '));
        if !before_ok || !after_ok {
            continue;
        }
        let rest = code[after..].trim_start();
        // `=` 形态只在**解构模式**里才是默认值：`const { tunConfig = {} } = cfg`。
        // 普通声明 `const inboundExcludeCidrs = injectedList(...)` 的 `=` 是赋值，不是兜底——
        // 首版没分这两者，立门当场就把注入读取点自己误报了（合法写法被误报的门最后一定被删掉）。
        let prev = code[..idx].trim_end().chars().next_back();
        let in_destructuring = matches!(prev, Some('{') | Some(','));
        let hit = rest.starts_with("??")
            || rest.starts_with("||")
            || (in_destructuring
                && rest.starts_with('=')
                && !rest.starts_with("==")
                && !rest.starts_with("=>"));
        if hit {
            let end = (after + 60).min(code.len());
            let start = idx.saturating_sub(30);
            // 切片必须落在 char 边界上（源码含中文注释已剥，但标识符前后仍可能有多字节）。
            let (mut s, mut e) = (start, end);
            while !code.is_char_boundary(s) {
                s -= 1;
            }
            while !code.is_char_boundary(e) {
                e -= 1;
            }
            return Some(code[s..e].replace('\n', " "));
        }
    }
    None
}

/// **门的自检 + 历史缺陷回放**：探测器必须对真实发生过的两种写法转红。
///
/// 不先回放就立门 = 只证明了"当前代码干净"，没证明"门抓得住"。
/// 三条正例都是仓里真实存在过的原文，`?? []` 那条是**修复途中最可能写出的错解**
/// （空数组同样是前端在挑默认，不因为它"看起来无害"就放行）。
#[test]
fn fallback_detector_catches_the_two_historical_defects() {
    let historical_bypass =
        "const bypassList = config.bypassLANList ?? ['localhost', '127.0.0.1', '192.168.0.0/16'];";
    assert!(
        find_default_fallback(historical_bypass, "bypassLANList").is_some(),
        "漏掉 bypassLANList 的历史兜底"
    );

    let historical_inbound =
        "const inboundExcludeCidrs = tun.inboundExcludeCidrs ?? ['100.64.0.0/10'];";
    assert!(
        find_default_fallback(historical_inbound, "inboundExcludeCidrs").is_some(),
        "漏掉 inboundExcludeCidrs 的历史兜底"
    );

    let historical_tun = "const tun: TunModeConfig = config.tunConfig ?? {\n stack: 'auto',\n};";
    assert!(
        find_default_fallback(historical_tun, "tunConfig").is_some(),
        "漏掉 tunConfig 的历史兜底"
    );

    // 另外三种形态。
    assert!(
        find_default_fallback("const l = config.bypassLANList ?? [];", "bypassLANList").is_some(),
        "`?? []` 同样是前端挑默认，必须红"
    );
    assert!(
        find_default_fallback("const l = config.bypassLANList || [];", "bypassLANList").is_some(),
        "`||` 形态漏网"
    );
    assert!(
        find_default_fallback("const { tunConfig = {} } = config;", "tunConfig").is_some(),
        "解构默认值形态漏网"
    );
    assert!(
        find_default_fallback(
            "const l = tun.inboundExcludeCidrs\n  ?? ['100.64.0.0/10'];",
            "inboundExcludeCidrs"
        )
        .is_some(),
        "跨行兜底漏网"
    );

    // 负向对照：合法写法不得误报，否则门会被人删掉而不是被遵守。
    for clean in [
        "const l = injectedList(config.bypassLANList, 'bypassLANList');",
        "patchTun({ inboundExcludeCidrs: next })",
        "if (tun.inboundExcludeCidrs === undefined) report();",
        "const same = a.tunConfig == b.tunConfig;",
        "const f = (tunConfig) => tunConfig;",
    ] {
        for leaf in ["bypassLANList", "inboundExcludeCidrs", "tunConfig"] {
            assert!(
                find_default_fallback(clean, leaf).is_none(),
                "合法写法被误报为兜底：{clean:?}（leaf={leaf}）"
            );
        }
    }

    // 普通声明的 `=` 不是兜底（首版在此误报了注入读取点自己）。
    assert!(
        find_default_fallback(
            "const inboundExcludeCidrs = injectedList(tun.inboundExcludeCidrs, 'x');",
            "inboundExcludeCidrs"
        )
        .is_none(),
        "普通 `const x = …` 被误判成解构默认值"
    );

    // 标识符边界自检：更长标识符的一截不算命中。
    assert!(
        find_default_fallback("const tunConfigDraft = x ?? {};", "tunConfig").is_none(),
        "`tunConfig` 误命中 `tunConfigDraft`"
    );
}

/// **本门主体**：`INJECTED_FIELD_PATHS` 里的每个字段，在整个 `ui/src` 都不许有读取点兜底。
///
/// 取材面直接读 Rust 那张表 —— 表里加一行，本门自动开始管那个字段，不必改这里。
#[test]
fn frontend_holds_no_config_default_fallback() {
    use polaris_config_engine::user_config::effective_view::INJECTED_FIELD_PATHS;

    let leaves: std::collections::BTreeSet<&str> = INJECTED_FIELD_PATHS
        .iter()
        .map(|p| p.rsplit('.').next().unwrap_or(p))
        .collect();
    assert!(
        !leaves.is_empty(),
        "INJECTED_FIELD_PATHS 空 —— 本门无事可做，必是表被清了"
    );

    let sources = frontend_sources();
    let mut offenders: Vec<String> = Vec::new();
    for (path, code) in &sources {
        for leaf in &leaves {
            if let Some(snippet) = find_default_fallback(code, leaf) {
                offenders.push(format!("{path}\n    …{snippet}…"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "前端又给「由 config:get 边界注入」的字段写了兜底默认值 —— 这正是 bypassLANList / \
         inboundExcludeCidrs 两次犯过的同一个 bug（UI 与内核分叉 + 首个按键把兜底持久化）。\n\
         值的真值源在 Rust `user_config::effective_view::ensure_effective_config`；\n\
         读取点请走 `ui/src/domain/effective-config.ts` 的 injectedList / injectedRecord。\n\
         命中：\n  {}",
        offenders.join("\n  ")
    );
}

/// 正面断言：注入读取入口确实存在且被消费。
///
/// 只有上面那条 `!contains` 的话，把读取点整个删掉、或把 helper 删掉，门都会**平凡通过**。
#[test]
fn frontend_reads_injected_fields_through_the_single_accessor() {
    let helper = frontend_code("domain/effective-config.ts");
    for export in [
        "export function injectedList",
        "export function injectedRecord",
    ] {
        assert!(
            helper.contains(export),
            "domain/effective-config.ts 缺 {export:?} —— 前端唯一允许的缺席处理点没了"
        );
    }

    let screen = frontend_code("components/screens/settings/SettingsTun.tsx");
    for call in [
        "injectedList(config.bypassLANList",
        "injectedList(\n    tun.inboundExcludeCidrs",
        "injectedRecord<TunModeConfig>(config.tunConfig",
    ] {
        assert!(
            screen.contains(call),
            "SettingsTun 未经注入读取入口取值：找不到 {call:?}"
        );
    }
}
