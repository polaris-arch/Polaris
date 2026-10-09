use crate::commands::guard_scan::{strip_block_comments, strip_line_comments, top_level_fn_body};
use crate::test_support::{crate_code, crate_root_code, repo_dir_files};

/// 渲染端取材面：`ui/src/**` 的全部生产 `.ts` / `.tsx`（仓内相对路径 → 源码）。
///
/// # 为什么是**现扫**而不是一张手写清单（这条门自己就踩过）
///
/// 上一版把消费方写死成三行（`App.tsx` / `TrayMenu.tsx` / `use-config.ts`），于是
/// **表外的文件它一眼都不看**：移动端补上配置订阅、成为第四个 `.onChanged(` 消费方时，那道门
/// 既不会转红、也不管辖它 —— 新消费方等于**自动免检**，而「这张表已经不完整」这件事**没有任何
/// 东西会报**。判据面靠枚举，枚举会腐烂，且腐烂时静默。
///
/// 现在覆盖面由**判据**定：凡是 `ui/src/**` 里出现 `.onChanged(` 的地方都受这条不变式约束，
/// 第五个消费方落地时自动进面。取材器单点在 [`repo_dir_files`]（`module_files_in` 在 `.rs` 面上
/// 治的是同一个缺陷类）。
///
/// # 为什么排除 `*.test.ts` / `*.test.tsx`
///
/// 测试里出现 `.onChanged(` 是**构造输入**（喂 mock 订阅、断言回调被调用），不是渲染端消费点。
/// 把它们算进来，判据就会被夹具喂饱：`assert` 要找的形态出现在测试替身里，于是「生产代码里其实
/// 没有」也判成有。这与 `polaris_source_probe::module_files_in` 排除 `tests/` 的理由逐字相同。
///
/// # 为什么只取 `.ts` + `.tsx` 两种扩展名就是全集
///
/// 2026-09-05 实测 `ui/src` 下的非 `.ts`/`.tsx` 文件只有 `.css` / `.json` / `.txt` / `.woff2`
/// —— 样式、数据与字体，一个能承载 `.onChanged(` 订阅的都没有。前端若哪天引入 `.js`/`.jsx`/
/// `.vue`，**本函数要跟着加扩展名**；这条边界写在这里，不写就是一个静默的取材面缺口。
///
/// # 跨语言耦合是刻意的
///
/// 前端源码被直接嵌进 Rust 测试判据：`ui/src/**` 里任一个 `.onChanged(` 订阅读了 payload，
/// 都会让 `cargo test -p polaris` 转红。只改前端的人未必会想到去跑 Rust 测试 —— 灯下记账：
///
/// CI 覆盖面（`.github/workflows/ci.yml` 实测）：`pull_request` 触发**无路径过滤**，纯改前端
/// 的 PR 仍会跑 `cargo test --workspace`，本测试正常拦截。只有**绕过 PR 直接 push 到 main**、
/// 且改动只命中 `on.push.paths-ignore` 里的 `ui/**`/`**.md`/`docs/**` 时，整条 Rust 链
/// （含本测试）才会被跳过——那是 push 主干的调试期额度优化，不针对本测试。结论：这道门在
/// 「PR 流程」下始终执行；只在「绕过 PR 的直接 push」这一条路径上失效。
fn renderer_sources() -> Vec<(String, String)> {
    let mut files = repo_dir_files("ui/src", "ts");
    files.extend(repo_dir_files("ui/src", "tsx"));
    files.retain(|(path, _)| !path.ends_with(".test.ts") && !path.ends_with(".test.tsx"));
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// `ui/src/**` 生产 `.ts`/`.tsx` 文件数的下限（**取材面自检**）。
///
/// 2026-09-05 实测 330 个。取 200 是给正常增删留余量，但**不给「取材面塌回枚举」留余量**：
/// 上一版那张手写清单只有 3 行，任何形式的回退（改回 `repo_file` 逐个列、路径写错、
/// 遍历坏掉）都远低于本值。这条是「派生而非枚举」这件事**唯一**的运行期证据 ——
/// 没有它，把 [`renderer_sources`] 改回三行清单不会有任何判据说话。
const RENDERER_FILE_FLOOR: usize = 200;

/// `.onChanged(` 消费方数的下限（**正面断言**）。
///
/// 2026-09-05 在本工作树实测 **3** 个：`ui/src/App.tsx`、`ui/src/tray/TrayMenu.tsx`、
/// `ui/src/components/screens/settings/use-config.ts`（移动端那一份订阅尚未落到本树）。
///
/// 取 3 而不是更大：下限的职责是让「一个消费方都没扫到」自曝 —— 只写「不许违反形态」在扫到
/// 0 个时是**免费的绿**。它**不**承担「消费方数目不许变」那件事：那正是上一版枚举表的做法，
/// 而新增消费方本来就该被自动纳入判据、逐个断言形态，不该因为「数目变了」而红。
/// 删到只剩 2 个仍会红，这是本条要拦的方向。
const CONSUMER_FLOOR: usize = 3;

/// 发射点：`app.emit(EVENT_CONFIG_CHANGED, …)` 的实参必须是空对象字面量 `json!({})`。
///
/// 判据是**对实参的正向等值断言**，不是负向枚举——旧版判据是「实参里不出现 `cfg`/`newValue`
/// 这两个今天恰好在用的标识符」，换个变量名（`broadcast_config_changed_with` 的形参本身就叫
/// `new_value`）或直接把载荷内容写成字面量，两条禁词一条都不命中，守卫全绿而配置树已在路上。
/// 判据按配对括号取实参，不要求 emit 与其实参写在同一行（rustfmt 拆行不影响本判据）。
///
/// 扫**全部** `app.emit(` 调用点，只对事件名匹配 `EVENT_CONFIG_CHANGED` 的逐一断言载荷、且
/// 数量必须恰为 1——而不是只看函数体里第一个 `app.emit(`：只看第一个会两头出错：本函数如果
/// 先发别的事件（如隐私模式跃迁）再发 configChanged，事件名断言会误红；反过来，如果
/// configChanged 之后又插入第二个带载荷的 `app.emit(EVENT_CONFIG_CHANGED, …)`，第一个合规、
/// 第二个违规，只看第一个会让第二个静默漏检。数量断言与消费方那侧（`sites == 1`）同规：多插
/// 一个**合规**的重复 emit 同样要停下来裁定——重复广播 = 三个前端消费方各多跑一次全量
/// `config_get`，正是本批要防的白付出。
///
/// 事件名不匹配时不再直接跳过不留痕迹：扫到的全部事件名收进 `seen_events`，0 命中时打进失败
/// 消息——有人把 `EVENT_CONFIG_CHANGED` 改写成全路径或换了个本地别名，emit 明明还在原地，
/// 消息也不会说成「发射点没了」这种指错方向的话。
///
/// 牙：把载荷改回 `json!({ "config": new_value })`（或任何非空内容，哪怕换个变量名）→ 转红；
/// 在合规 emit 之后再插一个**同样合规**的 `app.emit(EVENT_CONFIG_CHANGED, json!({}))` → 数量
/// 断言转红；把 `EVENT_CONFIG_CHANGED` 换成一个不存在的名字 → 转红且消息里能看到扫到的事件名
/// 不含它。
#[test]
fn emit_site_carries_no_config_content() {
    let broadcast_body = top_level_fn_body(
        &crate_code("commands/config.rs"),
        "pub(crate) fn broadcast_config_changed_with_completion<F>(",
    );
    // 切点自检①：扫到的确实是那个生产函数体。
    assert!(
        broadcast_body.contains("strip_privacy_secrets(&mut cfg)"),
        "扫到的不是 broadcast_config_changed_with_completion 的函数体 —— 守卫已失去判据"
    );
    assert!(
        broadcast_body.contains("emit_config_changed_signal(app)"),
        "普通配置汇流点必须复用 signal-only 发射函数，禁止另立第二个 configChanged emit"
    );
    // 切点自检②：判据词在本文件的测试代码里也各有一份，切片若漏封顶就会被自己喂饱 ——
    // 那正是「源码级判据被自己污染」的形态。
    assert!(
        !broadcast_body.contains("config_changed_payload_tests"),
        "切片切进了本测试模块，判据会被自己写的字面量喂饱"
    );
    let body = top_level_fn_body(
        &crate_code("commands/config.rs"),
        "pub(crate) fn emit_config_changed_signal(",
    );
    assert!(
        !body.contains("config_changed_payload_tests"),
        "signal-only 发射函数切片切进了测试模块"
    );

    let mut config_changed_emits = 0usize;
    // 扫到的每个 emit 的事件名，仅用于失败诊断——事件名对不上时把它打进消息，不能只说
    // 「发射点没了」（那会把排查方向指反：emit 明明在原地，只是名字变了）。
    let mut seen_events: Vec<&str> = Vec::new();
    for (call_at, _) in body.match_indices("app.emit(") {
        let args_at = call_at + "app.emit(".len();
        // 按配对括号取到本次调用的实参列表（而非要求「事件名 + 逗号」紧跟在 `app.emit(`
        // 后面同一行）。
        let mut depth = 1i32;
        let mut close = None;
        for (k, ch) in body[args_at..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close.expect("app.emit(...) 括号未配对 —— 发射调用格式已变，需要更新守卫");
        let args = body[args_at..args_at + close].trim();
        let event = args.split_once(',').map_or(args, |(event, _)| event.trim());
        seen_events.push(event);
        if event != "EVENT_CONFIG_CHANGED" {
            continue; // 别的事件，不归本守卫管。
        }
        config_changed_emits += 1;
        if let Some((_, payload)) = args.split_once(',') {
            let payload = payload.trim().trim_end_matches(',').trim();
            assert_eq!(
                payload, "json!({})",
                "configChanged 的发射载荷不是空对象字面量（实参：`{payload}`）——\
                     要发载荷必须用剥过隐私的那一份（`strip_privacy_secrets` 之后），且必须\
                     同步改本断言"
            );
        } // 单参数 emit（无逗号）：天然无载荷可言，直接过。
    }
    // 与消费方那侧（`sites == 1`）同规：增减都要停下来人工裁定，不止拦删除。多插一个**合规**
    // 的 `app.emit(EVENT_CONFIG_CHANGED, json!({}))` 一样是重复广播——三个前端消费方各多跑一次
    // 全量 `config_get`、托盘多一次 reconcile，正是本批要防的那类白付出。
    assert_eq!(
        config_changed_emits, 1,
        "configChanged 的发射点数不是 1（实为 {config_changed_emits}）。本函数体内扫到的全部 \
             emit 事件名：{seen_events:?}"
    );
}

/// 一份源码里全部 `.onChanged(` 订阅的**回调形参表**（按出现顺序）。
///
/// 抽成对 `(path, src)` 的纯函数而不是内联进测试：判定本身与「文件从哪来」无关，抽出来才能
/// 用**合成源码**喂进去 —— 否则「第五个消费方读了 payload 会不会红」这件事只能靠往
/// `ui/src/**` 里真写一个文件来验，而那不在本 crate 的改动面上。
/// 生产取材面（[`renderer_sources`]）与本函数的配对由 [`every_consumer_discards_the_payload`]
/// 交：那条测试既跑真实文件、又断言取材面下限，两半都在。
///
/// 判据是「形参表为空」，不是字面 `() =>` 前缀匹配。TS 可赋值性规则是「source **必需**形参数
/// ≤ target 形参数」，rest 形参在这条规则下视作「零个必需形参」——`(...a: unknown[]) => void`、
/// `async (...a: unknown[]) => void`，以及先具名再传入的
/// `const h = (...a: unknown[]) => {…}; onChanged(h)`，**全部**能合法赋给
/// `onChanged(listener: () => void)`（签名见 `ui/src/ipc/api/config.ts`）——类型层完全挡不住
/// rest 参数，这正是本结构守卫存在的理由；「非箭头字面量就退回类型层」这个论证只在「箭头函数
/// 只有裸 `(...) =>` 一种写法」时成立，`async` 前缀与具名传参都会绕开它。
///
/// 故判定前先剥可选的 `async ` 前缀，落到真正的形参括号上再取；剥完仍不是 `(` 开头
/// （裸标识符、`function` 表达式、或其它未识别形态，如无括号的单参箭头 `x => …`）
/// **不静默放过**——源码扫描判不出那类实参的形参表，直接 panic 要求人工裁定。
///
/// `function` 表达式**故意**没有像 `async` 那样被剥前缀特殊处理，即便它形参表可以是空
/// `()`——因为 `function () { … }` 会绑定 `arguments`，`arguments[0]` 照样能读到完整 payload；
/// 箭头函数不绑定 `arguments`，才是「形参表空 ⇒ 读不到 payload」这条判据成立的前提。把
/// `function` 也纳入「形参表为空即放行」会在这条新腿上开一个箭头函数没有的洞，故与裸标识符
/// 归同一类：源码扫描判不全，一律 panic 要求人工裁定，不假定它已被类型层挡住。
///
/// # 射程记账（别高估它）
///
/// 只抗块注释伪造（见 `strip_block_comments`），**不抗**行尾注释
/// （`foo(); // 见 api.onChanged(cb)` 照数）、也不抗字符串 / 模板字面量 / JSX 文本里出现
/// `.onChanged(` 这串字面量 —— 这两类都不做词法分析，真被这么写会被静默算作一次订阅。
fn on_changed_param_lists(path: &str, src: &str) -> Vec<String> {
    const CALL: &str = ".onChanged(";
    // 先剥块注释（含 JSDoc）再剥整行注释：注释里出现调用形态（如 `use-config.ts` 头部 JSDoc
    // 提到的 `` `configApi.onChanged` ``）会喂饱/顶红判据（与 Rust 侧剥行注释同一理由）。
    let src = strip_line_comments(&strip_block_comments(src));
    // **自曝**：`strip_block_comments` 找不到闭合就不清空、原样保留——那份「不作为」必须
    // 自己被看见，不能只在剩余文本恰好含 `.onChanged(` 时才被间接带出来（那是零信号的巧合绿）。
    // 扫一遍剥完的文本，任何一行 trim 后仍以 `/*`/`{/*` 开头，说明这正是一次未闭合起笔被原样吐回。
    for (n, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("/*") && !trimmed.starts_with("{/*"),
            "{path}:{} 有一个块注释起笔从未找到闭合 `*/`，strip_block_comments 按 doc 原样\
             保留了它——这段残留文本没有被清空扫描过，可能藏着一次伪造/丢失的 `.onChanged(` \
             订阅，需要人工核实",
            n + 1
        );
    }

    let mut out = Vec::new();
    for (at, _) in src.match_indices(CALL) {
        let rest = src[at + CALL.len()..].trim_start();
        // 剥 `async `：`async (...) => …` 与 `(...) => …` 的形参表位置相同。`function`
        // 前缀不剥——理由见上面 doc 的 `arguments` 那段。
        let param_scan_at = rest.strip_prefix("async").map_or(rest, str::trim_start);
        let Some(after_open) = param_scan_at.strip_prefix('(') else {
            panic!(
                "{path} 的 `.onChanged(` 实参不是箭头函数字面量（实处：`{}`）——具名回调 / \
                 `function` 表达式源码扫描判不出（`function` 还会绑定 `arguments`，形参表\
                 为空也可能读到 payload），需要人工核实该回调是否读了 payload，再决定是否\
                 扩展本判据",
                rest.chars().take(60).collect::<String>()
            )
        };
        // 形参表 = 首个 `(` 到与之配对的 `)`（含首尾括号）。
        let mut depth = 1i32;
        let mut close = None;
        for (k, ch) in after_open.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close.unwrap_or_else(|| {
            panic!(
                "{path} 的 `.onChanged(` 实参括号未配对（实处：`{}`）",
                rest.chars().take(60).collect::<String>()
            )
        });
        out.push(param_scan_at[..close + 2].to_owned());
    }
    out
}

/// 渲染端**全部** `.onChanged(` 消费方必须丢弃 payload，Rust 侧托盘汇流同规。
///
/// 取材面是**现扫**出来的（[`renderer_sources`]），不是手写清单 —— 上一版那张三行表的问题、
/// 以及「派生 vs 枚举」的全部理由，写在 [`renderer_sources`] 的头注里。
/// 形态判据本体（含为什么是「形参表为空」而不是字面 `() =>`、`async` / `function` / 具名回调
/// 各自怎么处置、射程边界）在 [`on_changed_param_lists`] 的头注里，此处不复述。
///
/// 三条断言各守一件事：
/// 1. **取材面自检**（文件数下限）：扫描面塌回枚举 / 遍历坏掉 ⇒ 红；
/// 2. **正面断言**（消费方数下限 + 逐个报出路径与形参表）：扫到 0 个也「没有违规」，
///    那种绿没有信息量；
/// 3. **形态断言**：每个消费方的形参表必须是 `()`。
///
/// 牙：`onChanged(() => …)` 改成 `onChanged((...args: unknown[]) => …)`（或加 `async`）→
/// 转红并点名文件；改成 `onChanged(onCfg)`（具名回调）或 `onChanged(function () { … })` →
/// panic 要求人工裁定；把 [`renderer_sources`] 改回三行手写清单 → 第 1 条转红。
#[test]
fn every_consumer_discards_the_payload() {
    let files = renderer_sources();
    // **取材面自检**：这条是「派生而非枚举」唯一的运行期证据（见 `RENDERER_FILE_FLOOR`）。
    assert!(
        files.len() >= RENDERER_FILE_FLOOR,
        "只扫到 {} 个渲染端生产源文件（下限 {RENDERER_FILE_FLOOR}）—— 取材面塌了：\
         要么 `renderer_sources` 被改回手写清单，要么遍历/路径坏了。\
         覆盖面必须由判据定，不由夹具定。",
        files.len()
    );

    let mut roster: Vec<(String, String)> = Vec::new();
    for (path, src) in &files {
        for params in on_changed_param_lists(path, src) {
            roster.push((path.clone(), params));
        }
    }

    // 正面断言①：报出扫到了几个消费方、分别在哪。只写「不许违反形态」在扫到 0 个时是免费的绿。
    println!(
        "[configChanged] 渲染端生产源文件 {} 个，`.onChanged(` 消费方 {} 个：",
        files.len(),
        roster.len()
    );
    for (path, params) in &roster {
        println!("    {path}  回调形参表 {params}");
    }
    assert!(
        roster.len() >= CONSUMER_FLOOR,
        "只扫到 {} 个 `.onChanged(` 消费方（下限 {CONSUMER_FLOOR}）—— 订阅被删光了，\
         或者扫描器坏掉了。实到清单：{roster:?}",
        roster.len()
    );

    // 正面断言②：逐个断言形态。新增消费方自动进面，不需要有人回来改任何清单。
    for (path, params) in &roster {
        assert_eq!(
            params, "()",
            "{path} 的 configChanged 订阅读了 payload —— 事件已是无载荷信号（发射点由本文件的 \
             `emit_site_carries_no_config_content` 钉成 `json!({{}})`），读到的只会是 `{{}}`。\
             形参表：`{params}`"
        );
    }

    // Rust 侧那一腿：`TRAY_SYNC_EVENTS` 含 `EVENT_CONFIG_CHANGED`（订阅面由 crate 根自己的
    // `tray_icon_events_are_the_proxy_lifecycle_channels` 钉住），本条只钉**回调丢弃 payload**。
    let main_body = top_level_fn_body(&crate_root_code(), "pub fn run() {");
    assert!(
        main_body.contains("wire_tray_icon_sync("),
        "扫到的不是应用装配入口 run() 的函数体 —— 守卫已失去判据"
    );
    // 回调现有两项工作（同步 warm 偏好 + reconcile tray），不能再把整条闭包钉成单表达式；
    // 真正的契约只有形参必须是 `_`，这样闭包体结构扩展也不会误红，同时 payload 仍结构性不可读。
    assert!(
        main_body.contains("handle.listen_any(ev, move |_| {"),
        "托盘汇流的事件回调不再以 `_` 丢弃 payload —— configChanged 已无载荷，读它只会拿到空对象"
    );
}

/// **形态判据自检**：喂一个「新落地的消费方」进 [`on_changed_param_lists`]，合规的放行、
/// 读了 payload 的要被抓出来。
///
/// # 为什么用合成源码而不是真往 `ui/src/**` 里加文件
///
/// 上一版那道门的缺陷是「表外文件一眼都不看」，改成现扫之后，**证明它真会看新文件**这件事
/// 落在两处：取材面下限（`every_consumer_discards_the_payload` 的第 1 条断言，证明扫的是整个
/// `ui/src/**` 而不是三行清单）+ 本条（证明扫到之后判据真的有牙）。两半合起来才等价于
/// 「第五个消费方落地会被管辖」。往 `ui/src/**` 真写一个文件既不在本 crate 的改动面上，
/// 也会把一次性的验证物留进生产源码。
///
/// 逐格覆盖 [`on_changed_param_lists`] 的四种形态：合规箭头 / rest 形参 / `async` + rest /
/// 具名回调（panic 腿另测）。
#[test]
fn a_newly_landed_consumer_is_judged_by_its_callback_shape() {
    // 合规：零形参箭头 —— 结构上读不到 payload。
    assert_eq!(
        on_changed_param_lists(
            "ui/src/mobile/MobileApp.tsx",
            "const off = api.config.onChanged(() => void hydrate());"
        ),
        vec!["()".to_owned()],
        "零形参箭头是合规形态，不该被误判"
    );

    // 违规①：rest 形参。TS 可赋值性把它算作「零个必需形参」⇒ 类型层完全放行，
    // 回调却拿得到完整 payload —— 这正是本结构守卫存在的理由。
    assert_eq!(
        on_changed_param_lists(
            "ui/src/mobile/MobileApp.tsx",
            "const off = api.config.onChanged((...args: unknown[]) => void hydrate(args));"
        ),
        vec!["(...args: unknown[])".to_owned()],
        "rest 形参必须被原样报出来，才谈得上被上面那条形态断言抓住"
    );

    // 违规②：`async` + rest（`async` 前缀会绕开「裸 `(...) =>` 前缀匹配」那种判法）。
    assert_eq!(
        on_changed_param_lists(
            "ui/src/mobile/MobileApp.tsx",
            "const off = api.config.onChanged(async (cfg: UserConfig) => { await save(cfg); });"
        ),
        vec!["(cfg: UserConfig)".to_owned()],
        "`async` 前缀必须先剥掉再取形参表，否则这条腿判不出来"
    );

    // 一份源码里多个订阅点都要各报一次（同文件挂两个消费方不是「一次」）。
    assert_eq!(
        on_changed_param_lists(
            "ui/src/mobile/MobileApp.tsx",
            "a.onChanged(() => x());\nb.onChanged((e) => y(e));"
        ),
        vec!["()".to_owned(), "(e)".to_owned()],
        "同文件多个订阅点必须逐个报出，只报第一个会让第二个静默漏检"
    );
}

/// **形态判据自检（panic 腿）**：具名回调判不出形参表，必须要求人工裁定而不是静默放行。
#[test]
#[should_panic(expected = "不是箭头函数字面量")]
fn a_named_callback_consumer_demands_manual_review() {
    on_changed_param_lists(
        "ui/src/mobile/MobileApp.tsx",
        "const h = (...a: unknown[]) => {};\nconst off = api.config.onChanged(h);",
    );
}

/// **预防性自检**：块注释（含 JSDoc）里若提到调用形态 `.onChanged(cb)` 不得被计入。
///
/// 今天的收益是 0：`use-config.ts` 头部 JSDoc 提到的是 `` `configApi.onChanged` ``（**没有**左
/// 括号），不含判据串 `.onChanged(`，就算没有 `strip_block_comments` 也数不进来——本用例钉的是
/// 「JSDoc 一旦被后人改写成带括号的调用形态」这类将来态，不是复现今天已经存在的漏洞。少了这条
/// 剥离、且真出现这种改写时：注释能伪造一次订阅、真订阅被删也仍全绿（`sites == 1` 是三腿
/// 「订阅还在」唯一的钉子）。
///
/// 变异锁：把 `strip_block_comments(src)` 换成裸 `src` → 本用例转红（`sites` 变 2）。
#[test]
fn block_comment_mentioning_on_changed_is_not_counted() {
    let src = "/**\n * see `configApi.onChanged(cb)` for details\n */\n\
                   const off = api.onChanged(() => void load());\n";
    let src = strip_line_comments(&strip_block_comments(src));
    let sites = src.match_indices(".onChanged(").count();
    assert_eq!(
        sites, 1,
        "块注释里的 `.onChanged(` 被计入了 —— TS 取材器漏剥块注释，注释能伪造一次订阅"
    );
}
