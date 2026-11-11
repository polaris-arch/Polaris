//! Android **release 构建路径**的接线门 —— R8 keep 规则 / 签名凭据 / 密钥不入库。
//!
//! # 守的是什么（根因）
//!
//! 这个应用曾长期没有在 R8 开着的情况下跑过一次。于是 release 路径上的**每一条**判据
//! 都没有任何观测面 ——
//! 剪错一个反射符号、少一份 keep 规则文件、口令被写死成默认值、未签名包被静默产出，
//! 四种事故的共同形状都是「debug 包全绿、release 包坏掉，且两侧都不报错」。
//!
//! 这一整类缺陷**没有运行期表征**可供单测捕捉：keep 规则是喂给 R8 的文本，签名凭据是喂给
//! AGP 的字符串，`.gitignore` 是喂给 git 的模式。它们的唯一观察面就是源码本身，所以本门
//! 是源码级的，与 `android_platform_verifier_wiring.rs` 同一口径。
//!
//! # 分工（与既有两道门）
//!
//! | | 它证明的事 | 它证明不了的事 |
//! |---|---|---|
//! | 本门（源码级） | 那些**只有源码这一个观察面**的事：签名接线那一行在 release 块里**出现且恰好一次**、口令只经环境变量、逃生门**那一条绑定**的右值逐字是哪一条、密钥模式在 `.gitignore` 里 | 构建**跑起来是什么行为**（R8 到底跑不跑、AGP 手里到底有几份规则、逃生门实际开没开）；以及「出现了」之后**有没有被后一句盖掉**（在那行接线后补一句 `signingConfig = null`，本门不红） |
//! | `scripts/gate-android-release-behavior.sh`（**行为裁判**，android.yml 的 apk 腿） | 跑一次 gradle **配置期**：零凭据必红且点名、逃生门**在它枚举得到的四个来源上**只认命令行实参、`minifyArm64ReleaseWithR8` 在 release 图里恰好 1 次而 debug 图里 0 次、AGP 装配好的 `proguardFiles` 在 release **构建类型**那一处与登记的三份**集合恰等**（多一份少一份都红）且 `defaultConfig`/`productFlavor` 两处为空、凭据在场时 `signingConfigs` **容器**里那份配置的 `storeFile` 存在可读且就是本工程认的那一份 | **执行期**发生的一切：`onlyIf`/`enabled=false` 让任务在图里却不跑、`doFirst` 改文件、R8 剪完之后包里还剩什么；以及配置期里它**没枚举到**的两条缝：逃生门换一个新变量名、release 构建类型有没有真把 signingConfig 接上 |
//! | `android_platform_verifier_wiring.rs` | rustls 校验器那条腿的三侧符号对得上、keep 规则**覆盖得到**那个类全名 | 运行期真的执行了 |
//! | `scripts/verify-wry-keep-rules.mjs`（CI 上跑） | RustWebView 那条 keep 的成员签名与生成物**同一个类里**对得上 | 裸 checkout 上跑不了（取材点 gitignored） |
//! | `scripts/assert-r8-evidence.mjs`（release-profile 验证腿） | R8 的 configuration.txt（**剥注释后**）里各来源的规则文本都到过、seeds.txt 里那些 keep 真的**命中**了类与成员 | 只能由 CI 的真实 release 构建证明 |
//!
//! # 两条教训（本门第二、第三版的由来）
//!
//! 第一版守着一圈周边，唯独没守那两条承重的行 —— 实测把 `signingConfig = …` 整行删掉、
//! 或把守卫谓词的 `endsWith("Release")` 改成永不匹配的串，**五条断言全绿、rc=0**，
//! 而两种变异下 release 路径都静默产出未签名包。
//! 一圈完好的周边不是证据：判据要落在承重的那一行本身，不是落在它的变量名或附近的注释上。
//!
//! 第二版把判据钉到了承重行上，但**判据形态仍然选错**：要守的是构建的**行为**，
//! 判据却在断言构建脚本的**文本**。2026-09-05 的逐行变异在 30 多条探针里打出 24 条门全绿，
//! 每一条的形状都一样 —— 文本里那串还在，行为已经变了：`proguardFiles` 只喂进三份里的一份、
//! 守卫谓词的 `||` 改成 `&&`、守卫体顶部插一句 `return@Action`、`isMinifyEnabled` 后面追一行
//! `false`、逃生门读取点后面接一个 `|| System.getenv(...)`。
//! 第三版把这些面整体交给上表那条**行为裁判**，本门只留下它真守得住的部分，
//! 并在每条断言的头注里写清楚「它守什么、什么形态会溜过去」——
//! 一条不受判据管辖的声明，比没有这句话更坏。
//!
//! # 🔴 天花板：本门与行为裁判合起来仍然够不到的那一层
//! （2026-09-05 收官轮立；2026-09-06 终止轮逐句复核并收窄）
//!
//! 行为裁判跑的是 gradle 的**配置期**。release 构建的另一半在**执行期**，那一层结构上
//! 看不见：`tasks.matching { … }.configureEach { onlyIf { false } }` 让 minify 留在任务图里
//! 却不跑；`doFirst { …writeText("") }` 让配置期读到的规则清单全对而 R8 到手的是空文件。
//! 那三件归 `.github/workflows/android.yml` 的 `release_check` job（真跑一次未签名、不会分发的
//! release-profile 构建，判据是 `scripts/assert-r8-evidence.mjs` 读 configuration.txt / seeds.txt，
//! 并由 `verify-apk.mjs` 开箱验证）。release-risk 的普通调用会自动运行它。
//!
//! 另外两件**连 release_check 都不归**（终止轮 R4 / R5 / R3）：
//!
//!  · **签名的接线与有效性**。`release_check` 那条腿跑的是
//!    `assembleArm64Release … -PpolarisAllowUnsigned=true`，走的正是逃生门、出的就是
//!    未签名包，所以它本身对签名零信息量。`release-apk` job 现在由 package 发布 DAG 自动调用：
//!    四项仓库 secret 注入正常签名构建，
//!    `apksigner verify` 验 APK Signature Scheme，上传后再把 GitHub digest 与本地 sha256 对拍。
//!    本门仍只守源码接线；真实密钥库/口令是否有效要由 tag 发布候选的 CI 执行结果证明。
//!  · **逃生门换一个新变量名**。本门的形状断言钉死的是 `val allowUnsignedRelease` **那一条
//!    绑定**；行为裁判的 ④/④b 是一张只试四个名字的枚举表。在守卫体里、逃生门分支之前插一句
//!    `if (System.getenv("POLARIS_XXX") == "true") return@Action`，绑定一字未动、名字不在
//!    枚举表里 ⇒ 两侧同时全绿，而那是一个从环境继承、永久生效的逃生门。这一形今天没有门。
//!
//! 完整的边界表在 `scripts/gate-android-release-behavior.sh` 的头注里，只此一份，本处不复写。
//!
//! # 取材与自污染
//!
//! 只读**别的**文件，不读自己。Gradle 侧走 [`polaris_source_probe::mask_comments`]（剥注释、
//! **留字符串**），ProGuard 与 `.gitignore` 侧先剥 `#` 注释再取材 —— 不剥的话，注释里写着的
//! 同一句话会给正面断言喂一份与生产配置无关的证据（本仓那三份文件的注释里恰好写满了本门要
//! 找的串）；而字符串必须留着，因为本门的针有一半**就是**字符串字面量
//! （`System.getenv("…")` 的实参、`file("…")` 的路径）。
//!
//! 🔴 手写一个「见 `/` 加 `*` 就当块注释起笔」的剥离器在这个文件上会**静默吃掉大半个文件**：
//! `jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")` 这类字符串里就带着 `/*` 与 `*/`。
//! 本门第一版正是这么栽的（实测：`getByName("release")` 在净化面上找不到）。`mask_comments`
//! 逐个跳过字符串字面量，不会把字面量内部的 `/*` 当注释。
//!
//! `.gitignore` 那条断言的期望值从 gradle 里那行默认密钥库路径**反推**扩展名，不在本文件
//! 另写一份字面量：改路径与改忽略模式必须一起改，只改一边就红。
//!
//! # 判据形态：正面断言，不写「不许出现 X」
//!
//! 口令那三条尤其如此。写成「不许出现 `?:`」会被两种东西骗过：什么都没发生（表达式整个被删），
//! 以及一个写在别处的默认值。本门改为把**取值表达式**与**消费表达式**逐个取出来做形状相等
//! 判定 —— 右边必须**恰好**是 `System.getenv("…")`，而 AGP 的 `storePassword` / `keyPassword` /
//! `keyAlias` 必须**恰好**赋成那三个绑定本身。这样一来，任何中途插进来的回退都改变了形状。
// 源码文本门：三平台源码逐字一致（`.gitattributes` eol=lf），只在必跑的 Linux 腿执行一次。
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};

/// Android 应用工程的 Gradle 脚本（相对 workspace 根）。
const GRADLE_APP: &str = "src-tauri/gen/android/app/build.gradle.kts";
/// 行为裁判的取材任务住的地方 —— 刻意**不是**上面那份被审判的文件（见 ⑧ 的头注）。
const GRADLE_FACTS: &str = "src-tauri/gen/android/app/polaris-release-facts.gradle.kts";
/// 唯一**入库**的 R8 规则文件（相对 workspace 根）。
const PROGUARD: &str = "src-tauri/gen/android/app/proguard-rules.pro";
/// 仓库根的 `.gitignore`（射程覆盖全仓，故密钥模式写在这里而不是 `gen/android/` 那份）。
const GITIGNORE: &str = ".gitignore";
/// Android 工程的 `gradle.properties` —— 逃生门的第二条取材面。
///
/// 它此前**不在任何门的取材面里**，而往它末尾写一行 `polarisAllowUnsigned=true` 就能把逃生门
/// 永久打开（本机实测：gradle 配置期 rc=0、警告行照出）。今天 gradle 侧改成只读命令行实参，
/// 这条腿是第二重：即使有人把读取点改回 `findProperty`，这里也拦得住持久化那一形。
const GRADLE_PROPERTIES: &str = "src-tauri/gen/android/gradle.properties";

/// AGP 的 release 构建类型名。
///
/// Gradle 任务名的后缀是它的**首字母大写**形态（`release` → `assembleArm64Release`），
/// 守卫里那条 `task.name.endsWith(...)` 的期望值由此推导，不在本门另写一份 `"Release"`
/// 字面量 —— 改构建类型名就必须一起改守卫，只改一边就红。
const RELEASE_BUILD_TYPE: &str = "release";

/// AGP 打包任务的三个前缀。守卫必须逐个认得，少一个就漏掉一条出包路径。
const RELEASE_TASK_PREFIXES: &[(&str, &str)] = &[
    ("assemble", "出 APK 的那条路（`assembleArm64Release`）"),
    (
        "bundle",
        "出 AAB 的那条路（`bundleArm64Release`，Play 上架用的形态）",
    ),
    (
        "package",
        "AGP 内部真正把产物封起来的任务；漏掉它，直接点名 `packageArm64Release` 时守卫不开口",
    ),
];

/// 本批为 R8 的 "Missing classes" 硬失败补的 `-dontwarn`：`(类名, 为什么需要它)`。
///
/// 这些不是 keep，是**抑制**：类根本不在任何输入里，`-keep` 对不存在的类是空操作。
/// 取证与「为什么不用通配」记在 `proguard-rules.pro` 那两行的头注里，本处不复述（复述两份必然漂）。
const R8_MISSING_CLASS_SUPPRESSIONS: &[(&str, &str)] = &[
    (
        "java.beans.ConstructorProperties",
        "jackson-databind 2.15.3 的 Java7SupportImpl 常量池里引用它，而 android.jar 的 \
         java/beans/ 下只有 5 个 PropertyChange* 类。缺这行 ⇒ 首次 release 构建在 \
         minifyArm64ReleaseWithR8 上直接失败。",
    ),
    ("java.beans.Transient", "同上，同一个类里的另一个引用。"),
    (
        "javax.lang.model.element.Modifier",
        "error_prone_annotations 2.15.0 的 IncompatibleModifiers / RequiredModifiers 各有一个 \
         `Modifier[] value()`，于是这个类**只**以 `()[Ljavax/…/Modifier;` 这个方法描述符的形态\
         出现（不是一条 CONSTANT_Class 项）—— 上一轮的姊妹腿扫描正是这么把它漏掉的。\
         它在 arm64ReleaseRuntimeClasspath 上，android-36 的 javax/lang/ 下一个条目都没有，\
         那个 jar 也不带 consumer 规则。取证与「R8 会不会为它硬失败尚未实测」记在 \
         proguard-rules.pro 那一行的头注里。",
    ),
];

/// release 必须逐条要求到场的三份 R8 规则文件（相对 app 工程目录）。
///
/// 后两份由 Tauri CLI 现场生成且被 `app/.gitignore` 忽略：缺席时 R8 照样跑完、构建照样绿，
/// 产物是个白屏包。所以 gradle 里必须**按名**列出它们，而不是靠一个通配去碰运气。
const REQUIRED_PROGUARD_FILES: &[&str] = &[
    "proguard-rules.pro",
    "proguard-tauri.pro",
    "src/main/java/com/polaris2/app/generated/proguard-wry.pro",
];

/// 三个**秘密**环境变量。
///
/// 密钥库路径变量 `POLARIS_KEYSTORE_FILE` 刻意不在此列：它不是秘密，且按裁定允许有默认值
/// （默认值就是约定位置）。把它混进来会让下面那条「右边必须恰好是 `System.getenv`」的
/// 形状断言无法成立，从而把整条判据逼软。
const SECRET_ENV_VARS: &[&str] = &[
    "POLARIS_KEYSTORE_PASSWORD",
    "POLARIS_KEY_PASSWORD",
    "POLARIS_KEY_ALIAS",
];

/// AGP `SigningConfig` 上接收那三个值的属性名。集合相等即可，不规定谁配谁。
const SIGNING_PROPERTIES: &[&str] = &["storePassword", "keyPassword", "keyAlias"];

/// 本批新增的 keep 规则登记表：`(片段, 它守什么 / 缺了会怎样)`。每条必须**恰好命中一次**。
///
/// 命中 0 次 = 规则被删而门还绿；命中多次 = 同一条规则有两处来源，将来必然漂。
const NEW_KEEP_RULES: &[(&str, &str)] = &[
    (
        "-keepattributes SourceFile,LineNumberTable",
        "release 独有的运行期异常的取证入口。缺了 logcat 里只剩混淆类名与行号 0，\
         而 debug 包复现不出来 —— 等于失去唯一的排查面。",
    ),
    (
        "-renamesourcefileattribute SourceFile",
        "拿到行号但不把原始源文件名铺进包里；反混淆靠 mapping.txt，不靠这个名字。",
    ),
    (
        "-keep class com.polaris2.app.RustWebView {",
        "承载下面两个方法的 keep 块。类本身另有 generated/proguard-wry.pro 保着，\
         但那份的成员清单漏了这两个。",
    ),
    (
        "void clearAllBrowsingData();",
        "wry 在 Rust 侧按方法名 JNI 调它（main_pipe.rs 的 call_method），Kotlin 侧零调用点，\
         也不是 android.webkit.WebView 的覆写。缺了就是 release 独有的 NoSuchMethodError。",
    ),
    (
        "java.lang.String getCookies(java.lang.String);",
        "同上。AGP 默认档那条 View getter 规则够不到它 —— 那条限定无参 getter，\
         而这个方法带一个 String 形参。",
    ),
];

/// 必须被仓库根 `.gitignore` 覆盖的密钥类文件模式。
const KEY_MATERIAL_IGNORES: &[&str] = &[
    "*.jks",
    "*.keystore",
    "keystore.properties",
    "key.properties",
];

/// 取非空行并逐行去首尾空白 —— 「整行相等」判据的取材面。
///
/// 为什么需要它：`contains` 对「在末尾接几个字符」是瞎的，而那一形恰恰把规则指向另一个
/// （多半不存在的）符号。整行相等把「这条规则原样在」变成正面断言。
fn trimmed_lines(src: &str) -> Vec<&str> {
    src.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

/// 剥掉 `#` 行注释（ProGuard 与 `.gitignore` 的唯一注释形态），保行结构。
fn strip_hash_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find('#') {
            Some(i) => line[..i].to_string(),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 取 `marker` 之后第一个 `{` 起的花括号配平块（不含首尾大括号）。
///
/// 切片失败一律 panic 并点名：读不到目标就静默返回空串的话，下游断言会在空取材面上恒真。
fn balanced_block<'a>(src: &'a str, marker: &str, origin: &str) -> &'a str {
    let at = src.find(marker).unwrap_or_else(|| {
        panic!("{origin}：找不到 `{marker}` —— 取材切片失败，本门下游断言全部作废。")
    });
    let rest = &src[at..];
    let open = rest
        .find('{')
        .unwrap_or_else(|| panic!("{origin}：`{marker}` 之后没有 `{{`，块结构变了。"));
    let mut depth = 0usize;
    for (idx, ch) in rest[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[open + 1..open + idx];
                }
            }
            _ => {}
        }
    }
    panic!("{origin}：`{marker}` 起的花括号没有配平 —— 取材切片失败。")
}

/// 一条声明**可以**由下一行接着往下写时，那一行会以这些 token 起笔。
///
/// Kotlin 没有语句终止符：换行只有在「这一行已经是一条完整表达式，且下一行不能接上去」时
/// 才结束一条声明。空行同样不结束表达式 —— 这正是 2026-09-05 那条绕法的立足点。
const CONTINUATION_STARTS: &[&str] = &[
    "||", "&&", "?:", "?.", "==", "!=", ">=", "<=", "->", "..", "!!", ".", "+", "-", "*", "/", "%",
    ",", ")", "]", "}", "else", "in ", "is ", "as ",
];

/// 一条声明**尚未**写完时，已写出的部分会以这些字符收尾（悬空运算符）。
const DANGLING_ENDS: &[&str] = &[
    "=", "||", "&&", "?:", "+", "-", "*", "/", "%", "(", "[", "{", ",", ".", "?", ":", "->", "==",
    "!=", ">", "<",
];

/// 从 `at` 起把**整条声明**切出来：一直吃到 Kotlin 语法上不可能再续接的位置为止。
///
/// # 为什么不是「切到下一个空行」
///
/// 那是 2026-09-05 收官轮验收给出的绕法（A4）：在绑定末尾空一行再接 `|| System.getenv(...)`，
/// Kotlin 把它当同一条表达式，而切到空行为止的判据只看见前半截 —— 两侧全绿，永久逃生门回来。
///
/// # 判定规则（两条，都是 Kotlin 的换行语义）
///
/// 1. 下一行以续接 token 起笔（`||` / `&&` / `.` / `?:` …）⇒ 属于本声明；
/// 2. 已吃下的部分以悬空运算符收尾（`=` / `||` / `(` …）⇒ 下一行必然属于本声明。
///
/// 中间的空行不结束声明，只是「先记账、等下一行表态」：下一行接得上就一起吃，接不上就吐回去。
///
/// # 边界（如实登记）
///
/// 括号内的换行（`foo(\n  a,\n  b\n)`）靠规则 2 的 `(` / `,` 与规则 1 的 `)` 覆盖，不做括号配平：
/// 本门今天切的是一条布尔绑定。真要切带块体的声明时改用 [`balanced_block`]。
fn declaration_slice(src: &str, at: usize) -> &str {
    let tail = &src[at..];
    let mut committed = 0usize;
    let mut pending = 0usize;
    for (index, line) in tail.split_inclusive('\n').enumerate() {
        if index > 0 {
            if line.trim().is_empty() {
                pending += line.len();
                continue;
            }
            let joins = CONTINUATION_STARTS
                .iter()
                .any(|token| line.trim_start().starts_with(token));
            let dangling = {
                let sofar = tail[..committed].trim_end();
                DANGLING_ENDS.iter().any(|token| sofar.ends_with(token))
            };
            if !joins && !dangling {
                break;
            }
        }
        committed += pending + line.len();
        pending = 0;
    }
    &tail[..committed]
}

/// 读 `build.gradle.kts` 并剥注释；哨兵串挑的是这个文件独有的那行本地 aar 装配。
fn gradle_source() -> String {
    polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(GRADLE_APP),
        GRADLE_APP,
        "implementation(files(\"libs/libbox.aar\"))",
    ))
}

/// ① release 构建类型的**文本形状**：minify 的开关写着 true、`proguardFiles` 消费的是那份
/// 收敛后的清单、三份规则文件被逐条按名要求。
///
/// # 🔴 收窄声明（2026-09-05）：这条守得住什么、什么形态会溜过去
///
/// **守得住**：这几个标识符与路径字面量被整段删掉、改名、或换成别的对象。
///
/// **守不住**（三条都实测过：门全绿而构建行为已经变了）——
/// 1. `isMinifyEnabled = true` 之后再追一行 `isMinifyEnabled = false`。`contains` 只问「有没有
///    这一串」，不问「最后一次赋值是什么」。变异后 `minifyArm64ReleaseWithR8` 直接从任务图消失，
///    本批全部 keep / dontwarn 一次都不生效。
/// 2. `proguardFiles(...)` 的**实参内容**。换成
///    `proguardRuleFiles.filter { it.name == "proguard-rules.pro" }` 之后：`proguardFiles(` 在、
///    `proguardRuleFiles` 在、三条 `file("…")` 的计数一条不变（那三行属于
///    `requiredProguardRuleFiles`，与消费侧无关）—— 而另外两份规则整体失效，产物白屏。
/// 3. debug 侧那条 `isMinifyEnabled = false` 同理。
///
/// 这三件的真值由 `scripts/gate-android-release-behavior.sh` 持有（⑤ release 图里 R8 恰好 1 次 /
/// ⑥ debug 图里 0 次 / ⑦ AGP 装配好的 `proguardFiles` 清单三份都在且逐份非空）。
/// 那条腿要 Android SDK 与两份 `tauri android build` 生成物，只在 `android.yml` 的 apk job 上跑得动。
/// 本条留着的理由是**便宜且早**：`cargo test` 在裸 checkout 上就能跑。两条腿是「早说」与
/// 「说得准」的分工，不是同一条判据的两份拷贝。
#[test]
fn the_release_build_type_is_minified_and_requires_every_keep_rule_file() {
    let gradle = gradle_source();

    let release = balanced_block(&gradle, "getByName(\"release\")", GRADLE_APP);
    assert!(
        release.contains("isMinifyEnabled = true"),
        "{GRADLE_APP}：release 构建类型没有 `isMinifyEnabled = true` —— \
         R8 不跑，本文件其余全部 keep 规则一条都不生效，而构建照样绿。\n\
         （本条只问这一串在不在。后面再追一行 `isMinifyEnabled = false` 它照样绿 —— \
         那一形由 scripts/gate-android-release-behavior.sh 的 ⑤/⑥ 守。）"
    );
    assert!(
        release.contains("proguardFiles("),
        "{GRADLE_APP}：release 没有 `proguardFiles(...)` —— minify 开着却没喂规则，\
         等于把全部反射面交给 R8 自由裁量。"
    );
    assert!(
        release.contains("getDefaultProguardFile(\"proguard-android-optimize.txt\")"),
        "{GRADLE_APP}：release 没串上 AGP 默认档。那份档里带着 native 方法、\
         @JavascriptInterface、enum values/valueOf 等一批全局兜底规则。"
    );
    assert!(
        release.contains("proguardRuleFiles"),
        "{GRADLE_APP}：release 的 `proguardFiles(...)` 不再消费那份收敛后的规则文件清单 —— \
         取材面换了对象，下面那条「三份必到」的断言就管不到真正生效的集合了。\n\
         （本条只问这个标识符出没出现在实参里。给它接一个 `.filter {{ … }}` 本条照样绿 —— \
         「实际喂进 R8 的到底是哪几份」由 scripts/gate-android-release-behavior.sh 的 ⑦ 守。）"
    );

    for rel in REQUIRED_PROGUARD_FILES {
        let needle = format!("file(\"{rel}\")");
        let hits = gradle.matches(needle.as_str()).count();
        assert_eq!(
            hits, 1,
            "{GRADLE_APP}：`{needle}` 命中 {hits} 次（应恰好 1 次）。\
             这三份规则文件里有两份由 Tauri CLI 现场生成、被 app/.gitignore 忽略，\
             缺席时 R8 会在零 wry/Tauri keep 的情况下跑完 —— 构建全绿、产物白屏。\n\
             （本条守的是「谁被点名要求到场」，不是「谁真的被喂给了 R8」：\
             这三行属于 requiredProguardRuleFiles，与 proguardFiles 的实参无关。\
             后者由 scripts/gate-android-release-behavior.sh 的 ⑦ 守。）"
        );
    }

    let debug = balanced_block(&gradle, "getByName(\"debug\")", GRADLE_APP);
    assert!(
        debug.contains("isMinifyEnabled = false"),
        "{GRADLE_APP}：debug 构建类型的 `isMinifyEnabled = false` 没了。\
         debug 包是当前唯一能跑的验收产物，给它开 minify 等于把唯一的对照组也变成实验组。"
    );
}

/// ①b release 构建类型**真的把 signingConfig 接上了** —— 这条门整个存在的理由。
///
/// # 为什么单独一条，而不是并进 ①
///
/// 上一版的门守着一圈周边（minify 开着、三份规则文件到场、口令只走环境变量、缺凭据会 throw），
/// 唯独**没有守那条承重的赋值**。实测：把 `signingConfig = signingConfigs.getByName("release")`
/// 整行删掉（全文命中 1 次），五条断言 **5 passed、rc=0**；再在凭据齐全的环境下跑 gradle，
/// 静默出一个未签名包 —— 而「拒绝静默产出未签名包」正是本门写下来的目的。
///
/// 一圈完好的周边不构成证据：判据必须落在那一行赋值本身，不是落在变量名、也不是落在附近的注释。
///
/// # 配置名从 `signingConfigs` 块里**读**出来，不在这里另写一份
///
/// `create("release")` 与 `getByName("release")` 是同一个名字的两次出现。把期望值写死在门里，
/// 改名时门照绿而两边已经对不上（AGP 会在执行期报 `SigningConfig 'release' not found`）。
/// 改为：从创建侧读出名字，再要求消费侧恰好接上**同一个**名字。
#[test]
fn the_release_build_type_actually_attaches_the_signing_config() {
    let gradle = gradle_source();

    // ── 创建侧：signingConfigs 里恰好创建一个配置，名字由它持有 ──
    let configs = balanced_block(&gradle, "signingConfigs {", GRADLE_APP);
    let created = configs.matches("create(\"").count();
    assert_eq!(
        created, 1,
        "{GRADLE_APP}：`signingConfigs` 块里 `create(\"…\")` 出现 {created} 次（应恰好 1 次）。\
         0 次 = 根本没有可接的签名配置；多次 = 有第二个配置，下面「消费侧接的是哪一个」不再唯一。"
    );
    let at = configs.find("create(\"").expect("上一条断言已保证它在");
    let rest = &configs[at + "create(\"".len()..];
    let end = rest
        .find('"')
        .unwrap_or_else(|| panic!("{GRADLE_APP}：`create(\"` 之后没有收尾引号 —— 取材切片失败。"));
    let config_name = &rest[..end];
    assert!(
        !config_name.is_empty(),
        "{GRADLE_APP}：signingConfig 的名字是空串 —— 反推不出消费侧该接什么。"
    );

    // ── 消费侧：release 构建类型必须恰好把它接上 ──
    let release = balanced_block(&gradle, "getByName(\"release\")", GRADLE_APP);
    let wiring = format!("signingConfig = signingConfigs.getByName(\"{config_name}\")");
    let hits = release.matches(wiring.as_str()).count();
    assert_eq!(
        hits, 1,
        "{GRADLE_APP}：release 构建类型里 `{wiring}` 命中 {hits} 次（应恰好 1 次）。\n\
         0 次 = **签名配置根本没接到 release 上**。AGP 此时不会报错，它会一声不吭地产出 \
         `app-<flavor>-release-unsigned.apk` —— 凭据齐全、门全绿、包装不上任何真机。\n\
         这是本门存在的唯一理由，别把它降级成「差不多接上了」。\n\
         多次 = 有第二处赋值，后一次会覆盖前一次，而本门只对得上其中一条。"
    );

    // ── 且它必须在「凭据齐了」那个分支里：缺凭据时接一个空壳配置，AGP 只会报一句
    //    语焉不详的 `Keystore file not set`，而不是守卫那段点名到变量的失败信息。 ──
    let ready = balanced_block(release, "if (releaseSigningReady)", GRADLE_APP);
    let guarded = ready.matches(wiring.as_str()).count();
    assert_eq!(
        guarded, 1,
        "{GRADLE_APP}：`{wiring}` 不在 `if (releaseSigningReady)` 分支里（命中 {guarded} 次）。\
         无条件接线会让缺凭据时的报错从「缺的是哪个环境变量」退化成 AGP 的 \
         `Keystore file not set` —— 离真因两跳远。"
    );

    // ── 守卫的读回点接的也必须是**同一个**配置 ──
    //
    // 守卫在凭据齐全时要断言 storeFile 存在、可读、且就是 keystoreFile（收官轮 A3）。
    // 那条断言读的是 `releaseSigningConfig`，而它必须绑到刚才那个名字上：绑错一个名字，
    // 断言就在**另一份**配置上作证，而那份配置多半是空壳 ⇒ 判据恒真或恒假。
    let readback =
        format!("val releaseSigningConfig = android.signingConfigs.getByName(\"{config_name}\")");
    let readback_hits = gradle.matches(readback.as_str()).count();
    assert_eq!(
        readback_hits, 1,
        "{GRADLE_APP}：`{readback}` 命中 {readback_hits} 次（应恰好 1 次）。\
         0 次 = 守卫里那条 storeFile 读回没有取材点（或取的是别的配置）；\
         多次 = 有第二个读回点，而两处必然漂。"
    );
}

/// ①c 行为裁判的取材任务必须住在**被审判的那个文件之外**。
///
/// # 根因（2026-09-05 收官轮 A2）
///
/// `printPolarisReleaseFacts` 是 `scripts/gate-android-release-behavior.sh` 的唯一事实来源：
/// AGP 手里有几份规则文件、storeFile 指向哪儿、逃生门开没开，四条判据全靠它打出来的行。
/// 而它此前就写在 `app/build.gradle.kts` 的末尾 —— 也就是它自己审判的那个文件里。
/// 把取材表达式一换（写死一份三元素清单即可），裁判的 ⑦ 原样全绿而 AGP 手里的清单已经变了。
///
/// **判据的取材源与被判对象是同一份可改的文本时，那条判据没有独立性可言。**
///
/// # 本条守什么、守不住什么
///
/// **守得住**：任务定义搬回 `build.gradle.kts`、`apply(from = …)` 那一行被删、事实文件被清空。
///
/// **守不住**：事实文件**自己**被改。那是文本判据够不到的一层 —— 「它读的到底是不是 AGP
/// 装配好的那份清单」由行为裁判的 ⑨ 用差分实验回答（往 `app/` 放一份新的 `.pro`，
/// AGP 的扫描面变了，事实表必须跟着变；打不出那一行就是它读的不是 AGP 的清单）。
#[test]
fn the_release_facts_task_lives_outside_the_file_it_judges() {
    let gradle = gradle_source();
    let facts_apply = format!("apply(from = \"{}\")", "polaris-release-facts.gradle.kts");
    let apply_hits = gradle.matches(facts_apply.as_str()).count();
    assert_eq!(
        apply_hits, 1,
        "{GRADLE_APP}：`{facts_apply}` 命中 {apply_hits} 次（应恰好 1 次）。\
         0 次 = 事实任务根本不会被注册，行为裁判每一轮都会 rc≠0（不会静默，但也就没了判据）。"
    );
    assert!(
        !gradle.contains("tasks.register(\"printPolarisReleaseFacts\")"),
        "{GRADLE_APP}：事实任务又被定义回了它自己审判的这个文件里。\
         取材源与被判对象同文件 = 改一行取材表达式就能让裁判的 ⑦ 对着假事实作证。"
    );

    let facts = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(GRADLE_FACTS),
        GRADLE_FACTS,
        "printPolarisReleaseFacts",
    ));
    assert!(
        facts.contains("tasks.register(\"printPolarisReleaseFacts\")"),
        "{GRADLE_FACTS}：事实任务不在这里了 —— 裁判会因为「任务不存在」直接 rc≠0。"
    );
    assert!(
        facts.contains("androidExtension.buildTypes.getByName(\"release\").proguardFiles"),
        "{GRADLE_FACTS}：事实任务不再从 AGP 的 release 构建类型上取 `proguardFiles`。\
         取材对象一换（比如写死一份清单），裁判的 ⑦ 就在假事实上作证。\
         （这一条只守文本；「读的真的是 AGP 那份清单吗」由行为裁判的 ⑨ 差分实验守。）"
    );
    // AGP 把 defaultConfig + productFlavor + buildType 三处的 proguardFiles 合并后喂给 R8
    // （AGP 口径；本仓没在 R8 输入那一层实测过）（终止轮 R2）。
    // 上一条只钉住了第三处；另外两处不摊开的话，往 `defaultConfig` 上挂一份 `../x.pro`
    // （app/ 的 fileTree 扫描面之外）在行为裁判眼里完全不存在 —— 本机实测 ⑦ 零失败、rc=0。
    for source in [
        "androidExtension.defaultConfig.proguardFiles",
        "androidExtension.productFlavors",
    ] {
        assert!(
            facts.contains(source),
            "{GRADLE_FACTS}：事实任务不再摊开 `{source}` 上的 proguardFiles。\
             AGP 是把三处合并后喂给 R8 的，少摊一处 ⇒ 行为裁判的 ⑦b 没了取材面，\
             而 ⑦ 的「集合恰等」只覆盖三分之一。"
        );
    }
    assert!(
        facts.contains("androidExtension.signingConfigs.findByName(\"release\")?.storeFile"),
        "{GRADLE_FACTS}：事实任务不再打印签名配置的 `storeFile` —— \
         「凭据在场那条分支」重新变回零观测面（收官轮 A3 就是这一条）。"
    );
    // 逃生门的开关值必须**取自生产绑定**，不许在这里重算一次。
    assert!(
        facts.contains("project.extra[\"releaseFactHatchOpen\"]"),
        "{GRADLE_FACTS}：逃生门的观察值不再取自 build.gradle.kts 导出的那个生产绑定。"
    );
    assert!(
        !facts.contains("polarisAllowUnsigned"),
        "{GRADLE_FACTS}：这里出现了属性名 `polarisAllowUnsigned` —— \
         那等于给逃生门开第二个读取点：把生产读取点改回 `findProperty` 的变异，\
         在裁判眼里仍然是绿的（裁判读到的是这份没被改的语义）。"
    );
}

/// ② 口令与别名**只**经环境变量：取值表达式与消费表达式两侧都做形状相等判定。
#[test]
fn the_signing_secrets_come_only_from_environment_variables() {
    let gradle = gradle_source();

    // ── 取值侧：右边必须恰好是 `System.getenv("…")` ──
    let mut bindings: BTreeSet<String> = BTreeSet::new();
    for name in SECRET_ENV_VARS {
        let call = format!("System.getenv(\"{name}\")");
        let hits = gradle.matches(call.as_str()).count();
        assert_eq!(
            hits, 1,
            "{GRADLE_APP}：`{call}` 命中 {hits} 次（应恰好 1 次）。\
             0 次 = 这个秘密不再从环境变量取（多半是被写死或落盘了）；\
             多次 = 有第二条取值路径，而两条路径必然漂。"
        );
        let line = gradle
            .lines()
            .find(|line| line.contains(call.as_str()))
            .unwrap_or_else(|| panic!("{GRADLE_APP}：切不出含 `{call}` 的那一行。"));
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("val ").unwrap_or_else(|| {
            panic!(
                "{GRADLE_APP}：`{call}` 不是一条 `val <名> = …` 绑定，而是 `{trimmed}`。\
                 本门要求它是独立绑定，才能把「取到的值」与「喂给 AGP 的值」逐字对上。"
            )
        });
        let (ident, rhs) = rest.split_once(" = ").unwrap_or_else(|| {
            panic!("{GRADLE_APP}：`{trimmed}` 里找不到 ` = ` —— 绑定形态变了。")
        });
        assert_eq!(
            rhs, call,
            "{GRADLE_APP}：`{ident}` 的右边是 `{rhs}`，而不是裸的 `{call}`。\
             口令/别名后面不许接 `?:` / `orElse` / `getOrDefault` 之类的回退：\
             有默认值就等于有一把人人都知道的钥匙，而且它会让「凭据缺失」这件事再也不会被发现。"
        );
        assert!(
            !ident.is_empty() && ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{GRADLE_APP}：`{ident}` 不像一个普通标识符 —— 绑定被拆成了别的形态，\
             下面的消费侧对拍会失去意义。"
        );
        assert!(
            bindings.insert(ident.to_string()),
            "{GRADLE_APP}：`{ident}` 被两个环境变量共用 —— 消费侧一对一的对拍不再成立。"
        );
    }

    // ── 消费侧：AGP 的三个属性必须恰好赋成上面那三个绑定本身 ──
    let mut consumed: BTreeMap<&str, String> = BTreeMap::new();
    for line in gradle.lines() {
        let trimmed = line.trim();
        for prop in SIGNING_PROPERTIES.iter().copied() {
            let prefix = format!("{prop} = ");
            if let Some(rhs) = trimmed.strip_prefix(prefix.as_str()) {
                let previous = consumed.insert(prop, rhs.trim().to_string());
                assert!(
                    previous.is_none(),
                    "{GRADLE_APP}：`{prop}` 被赋值两次 —— 后一次会覆盖前一次，\
                     而本门只对得上其中一条。"
                );
            }
        }
    }
    let expected_props: BTreeSet<&str> = SIGNING_PROPERTIES.iter().copied().collect();
    let actual_props: BTreeSet<&str> = consumed.keys().copied().collect();
    assert_eq!(
        actual_props, expected_props,
        "{GRADLE_APP}：signingConfig 里被赋值的属性集合不对。少一个 = 那一件根本没接线\
         （AGP 会拿到 null 并在执行期报一句语焉不详的错，或者干脆产出未签名包）。"
    );
    let actual_sources: BTreeSet<String> = consumed.values().cloned().collect();
    assert_eq!(
        actual_sources, bindings,
        "{GRADLE_APP}：喂给 AGP 的值不是那三个 `System.getenv` 绑定本身。\
         中间只要插进一个表达式（`?:`、字符串拼接、另一个变量），\
         「口令只经环境变量」这句话就不再由代码持有。"
    );
}

/// ③ 守卫的**文本形状**：三个任务前缀 / 后缀 / 工程限定各出现一次，逃生门的读取点右值逐字对上。
///
/// 剥过注释之后再找，所以「把 throw 注释掉」这一形当场红 —— 那正是本条要防的失效方式。
///
/// # 🔴 收窄声明（2026-09-05）：这条守得住什么、什么形态会溜过去
///
/// **守得住**：谓词里那三个前缀、那个后缀、工程限定、`missingRules` / `signingBlockers` /
/// `releaseSigningReady` 的消费点，以及逃生门读取点的**整条右值**（下面新增的那条形状相等）。
///
/// **守不住**（两条都实测过：门全绿而守卫已经彻底闭嘴）——
/// 1. **布尔算子**。三个字面量各钉一次、组合方式没钉：把谓词里的 `||` 改成 `&&`，
///    上面每一条计数都不变，而 `wantsRelease` 恒 false ⇒ 零凭据跑 release 的 rc 由 1 变 0。
/// 2. **控制流**。在 `Action` 体顶部插一句
///    `if (System.getenv("CI") == null || true) return@Action`：被断言的串一个不少，
///    守卫既不判也不报。
///
/// 两者的真值都由 `scripts/gate-android-release-behavior.sh` 的 ①（零凭据跑 release 必须 rc≠0
/// 且失败信息逐条点名三个环境变量）持有 —— 那是「守卫到底开不开口」唯一说得清的地方。
/// 本条留着的是「谁被点名了」这一层：`assemble` / `bundle` / `package` 少一个的时候，
/// 行为裁判只跑 `assemble` 那条路，看不见另外两条出包路径。两条腿互补，谁也替不了谁。
#[test]
fn missing_credentials_fail_loudly_instead_of_yielding_an_unsigned_apk() {
    let gradle = gradle_source();
    let guard = balanced_block(&gradle, "gradle.taskGraph.whenReady(", GRADLE_APP);

    assert!(
        guard.contains("if (!wantsRelease) return@Action"),
        "{GRADLE_APP}：守卫没有先判「这次到底构不构 release」。\
         少了这一跳，判据会落到 debug 构建头上 —— 而 debug 包是当前唯一能跑的验收产物。"
    );

    // ── 守卫谓词的**定义**：判据必须钉住它算的是什么，不是钉住它叫什么 ──
    //
    // 上一版只断言了 `wantsRelease` 这个**名字**被消费。实测：把谓词里的
    // `task.name.endsWith("Release")` 改成 `endsWith("ReleaseNeverMatches")`（全文命中 1 次），
    // 门 5 passed、rc=0 —— 而同一变异下，零凭据跑 release 时守卫彻底闭嘴：`wantsRelease` 恒 false，
    // 于是「缺凭据要硬失败」「三份规则文件要到场」两条判据一起被短路，静默出未签名包。
    //
    // 名字对不上是编译错误（Kotlin 会报 unresolved reference），本来就不需要门来守；
    // 真正会静默变形的是**谓词的内容**。
    let predicate = balanced_block(guard, "val wantsRelease", GRADLE_APP);
    assert!(
        predicate.contains("task.project == project"),
        "{GRADLE_APP}：守卫谓词不再把任务限定在**本工程**（`task.project == project`）。\
         少了它，别的子工程（tauri-android / 五个插件项目）里任何一个名字以 Release 收尾的任务\
         都会把本工程的守卫点着 —— 判据从此对着别人的任务图作证。"
    );

    // 期望的任务名后缀由构建类型名推导（`release` → `Release`），不在本门另写字面量。
    let mut chars = RELEASE_BUILD_TYPE.chars();
    let suffix: String = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => panic!("RELEASE_BUILD_TYPE 是空串 —— 推导不出任务名后缀。"),
    };
    let ends_with = format!("task.name.endsWith(\"{suffix}\")");
    let hits = predicate.matches(ends_with.as_str()).count();
    assert_eq!(
        hits, 1,
        "{GRADLE_APP}：守卫谓词里 `{ends_with}` 命中 {hits} 次（应恰好 1 次）。\n\
         这个后缀是从构建类型名 `{RELEASE_BUILD_TYPE}` 推出来的（AGP 的任务名把构建类型名首字母\
         大写接在后面）。串一改（哪怕只是多几个字符），谓词就永远不匹配任何任务 ⇒ 守卫恒闭嘴 ⇒ \
         缺凭据照样出未签名包、缺规则文件照样静默跑完 R8，而构建全绿。\n\
         这条断言钉的是谓词**定义**里的那个串，不是它被引用的地方。"
    );

    for (prefix, why) in RELEASE_TASK_PREFIXES {
        let needle = format!("task.name.startsWith(\"{prefix}\")");
        let hits = predicate.matches(needle.as_str()).count();
        assert_eq!(
            hits, 1,
            "{GRADLE_APP}：守卫谓词里 `{needle}` 命中 {hits} 次（应恰好 1 次）。\
             它认的是：{why}。少一条 = 那条出包路径上守卫不开口。"
        );
    }
    assert!(
        guard.contains("missingRules"),
        "{GRADLE_APP}：守卫不再检查那三份 R8 规则文件是否到场。\
         缺席时 R8 会在零 keep 的情况下跑完，构建全绿而产物白屏。"
    );
    assert!(
        guard.contains("if (releaseSigningReady) {"),
        "{GRADLE_APP}：守卫里没有「凭据齐了」那个分支 —— 判据的正向分支没了。"
    );
    assert!(
        guard.contains("unregisteredRules"),
        "{GRADLE_APP}：守卫不再检查「有没有多出来一份没登记的规则文件」。\
         规则集是集合：往 app/ 放一份 `-keep class ** {{ *; }}` 就把整包混淆关掉了，\
         而「三份都在」这条 membership 判定对它是瞎的。\
         （行为面由 scripts/gate-android-release-behavior.sh 的 ⑨ 守。）"
    );
    // ── 凭据齐全那条分支的**读回**：storeFile 必须被看一眼 ──
    //
    // 2026-09-05 收官轮 A3：`storeFile` 指哪儿此前没有任何东西看。最坏的一形不是指向空气
    // （AGP 执行期还会报一句 `Keystore file not set`），而是指向**另一份存在的**密钥库 ——
    // 构建全绿、包也签得出来，只是签它的不是本工程的密钥。
    assert!(
        guard.contains("releaseSigningConfig.storeFile"),
        "{GRADLE_APP}：守卫在凭据齐全时不读回 AGP 手里那份 signingConfig 的 `storeFile`。\
         `signingBlockers` 查的是取值侧（keystoreFile），这里查的是消费侧 —— \
         两侧对不上正是「取值对了、接线接错了」那一形，而它今天没有别的观察面。"
    );
    assert!(
        guard.contains("assembled.canonicalFile != keystoreFile.canonicalFile"),
        "{GRADLE_APP}：守卫没有把 `storeFile` 与 `keystoreFile` 逐字比对。\
         只判「存在且可读」时，指向另一份**存在的**密钥库照样全绿。\
         （行为面由 scripts/gate-android-release-behavior.sh 的 ⑩ 守。）"
    );
    assert!(
        guard.contains("signingBlockers"),
        "{GRADLE_APP}：失败信息不再消费 `signingBlockers` —— \
         那就说不出「缺的是哪一个变量」，只剩一句「签名没配好」。"
    );

    let hatch_at = guard.find("if (allowUnsignedRelease)").unwrap_or_else(|| {
        panic!(
            "{GRADLE_APP}：守卫里没有显式逃生门 `-PpolarisAllowUnsigned=true`。\
             没有它，「只想量一次体积」这件事做不到，人就会去把整段守卫注释掉。"
        )
    });
    // 只在逃生门**之后**找那条硬失败：这样一次断言同时钉住「有没有」与「谁先谁后」。
    // 用 rfind 从全段找会被前面那条 missingRules 的 throw 顶上，于是把 throw 注释掉之后
    // 红出来的是「顺序不对」而不是「没有硬失败」—— 红是红了，点名却指向了错的那条。
    let throw_at = guard[hatch_at..]
        .find("throw GradleException(")
        .map(|offset| hatch_at + offset)
        .unwrap_or_else(|| {
            panic!(
                "{GRADLE_APP}：逃生门之后没有 `throw GradleException(` —— 要么缺凭据时不再硬失败，\
                 要么硬失败被挪到了逃生门**之前**（那样逃生门永远走不到，等于没有逃生门）。\
                 AGP 的默认行为恰恰是静默产出 `app-*-release-unsigned.apk`，\
                 那个包装不上任何真机，而构建全绿。"
            )
        });
    assert!(
        guard[hatch_at..throw_at].contains("logger.lifecycle("),
        "{GRADLE_APP}：走逃生门时不打印警告行。未签名包与已签名包只差文件名里一个后缀，\
         没有那行警告，产物很容易被当成可分发包。"
    );

    let hatch_read = "gradle.startParameter.projectProperties[\"polarisAllowUnsigned\"]";

    // ── 整条右值形状相等：命中一次只证明「这条路在」，证明不了「只有这一条路」 ──
    //
    // 实测 M01：在读取点后面接一句 `|| System.getenv("POLARIS_ALLOW_UNSIGNED") == "true"`，
    // 下面那条计数仍是 1、全门绿，而逃生门从此可以被一个环境变量**永久且不可见**地打开 ——
    // 逃生门的语义是「这一次」，一个从环境继承下来的开关把它变成了「从此以后」。
    // 判据改为：把这条绑定整段取出来，归一空白后与期望值逐字相等。
    // 于是任何续接、任何回退、任何中间变量都改变了形状。
    //
    // 🔴 切片终点不是「下一个空行」（2026-09-05 收官轮 A4 实测的绕法）：
    //    在绑定末尾**空一行**再写 `|| System.getenv("POLARIS_BUILD_UNSIGNED") == "true"`，
    //    Kotlin 照样把它算作同一条表达式（换行与空行都不结束表达式），而切到空行为止的判据
    //    只看见前半截 ⇒ 两侧全绿、永久逃生门回来。
    //    今天走 [`declaration_slice`]：吃到 Kotlin 语法上**不可能再续接**的地方为止。
    let binding_at = gradle.find("val allowUnsignedRelease").unwrap_or_else(|| {
        panic!("{GRADLE_APP}：找不到 `val allowUnsignedRelease` 绑定 —— 逃生门的读取点没了。")
    });
    let binding_raw = declaration_slice(&gradle, binding_at);
    let binding: String = binding_raw.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        binding.len() > "val allowUnsignedRelease".len(),
        "{GRADLE_APP}：切出来的逃生门绑定是 `{binding}` —— 切片自检失败，下面的相等断言作废。"
    );
    // 切片自检（另一头）：吃过头同样让判据失去意义 —— 相等断言会红在一个说不清的地方。
    assert!(
        !binding.contains("android {"),
        "{GRADLE_APP}：逃生门绑定的切片一路吃进了 `android {{` —— 续接判定过宽，\
         下面的相等断言量的不再是那一条绑定。实得：{binding}"
    );
    let expected_binding = format!("val allowUnsignedRelease: Boolean = {hatch_read} == \"true\"");
    assert_eq!(
        binding, expected_binding,
        "{GRADLE_APP}：逃生门的读取点不再**恰好**是那一个表达式。\n\
         实得：{binding}\n\
         期望：{expected_binding}\n\
         后面接一个 `|| System.getenv(...)` / `?:` / 另一个变量，都会让「这一次」变成\
         「从此以后」：那个来源会被下一次构建继承，而没有任何东西会说话。\n\
         🔴 射程（终止轮 R3 收窄）：本条只守**这一条绑定**。新起一个变量名写在这条绑定里会红，\
         写在守卫**体内别处**（例如在逃生门分支之前多插一句 \
         `if (System.getenv(\"POLARIS_XXX\") == \"true\") return@Action`）本条一字不红，\
         而行为裁判的 ④/④b 只试它枚举得到的四个名字，同样不红 —— 那条缝两侧都没人守，\
         登记在 gate-android-release-behavior.sh 头注天花板表的「看不见 ⑤」。"
    );

    let hits = gradle.matches(hatch_read).count();
    assert_eq!(
        hits, 1,
        "{GRADLE_APP}：`{hatch_read}` 命中 {hits} 次（应恰好 1 次）。\n\
         逃生门**只能**从 `startParameter.projectProperties` 读 —— 那张表里装的恰好是本次调用的 \
         `-P` 实参。换成 `project.findProperty(...)` 会把 `gradle.properties` 文件与 \
         `ORG_GRADLE_PROJECT_*` 环境变量一视同仁地认进来，于是逃生门可以被**永久且不可见**地\n\
         打开（本机实测两条路都 rc=0），此后每次 release 都静默出未签名包。\n\
         逃生门的语义是「这一次我知道自己在做什么」，不是「从此以后都不用管」。"
    );

    // 属性名在净化面上只许出现这一次：`mask_comments` 留字符串、剥注释，所以失败信息里那句
    // `-PpolarisAllowUnsigned=true`（没有引号包着属性名）不会顶到这条计数上。多一次 =
    // 有第二个读取点，也就有第二套开关语义。
    let key_literal = "\"polarisAllowUnsigned\"";
    let key_hits = gradle.matches(key_literal).count();
    assert_eq!(
        key_hits, 1,
        "{GRADLE_APP}：属性名字面量 `{key_literal}` 命中 {key_hits} 次（应恰好 1 次）。\
         多出来的那次多半是一个绕过上面那条形状断言的第二读取点。"
    );

    // ── 第二重：持久化那一形必须不可能，而不是只靠读取点的形状 ──
    //
    // `gradle.properties` 此前不在任何门的取材面里。今天读取点改了之后它写进去也不生效，
    // 但这条断言仍要留着：它守的是「有人把读取点改回 findProperty」与「有人顺手写一行进去」
    // 这两件事的**合流**，而那正是逃生门被永久打开的唯一路径。
    let properties = strip_hash_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(GRADLE_PROPERTIES),
        GRADLE_PROPERTIES,
        "android.useAndroidX=true",
    ));
    assert!(
        !properties.contains("polarisAllowUnsigned"),
        "{GRADLE_PROPERTIES}：出现了 `polarisAllowUnsigned`。\
         逃生门是「命令行传一次」，不是「写进文件从此永远开着」—— \
         写进这里的开关会跟着仓库传给每一台机器、每一次构建，而没有任何东西会说话。\
         要量一次体积就在命令行上传：`-PpolarisAllowUnsigned=true`。"
    );
}

/// ④ 本批新增的每一条 keep 规则都在**入库的**那份 `proguard-rules.pro` 里，且各恰好一次。
///
/// 为什么不去 `generated/proguard-wry.pro` 里对差：那份是 Tauri CLI 生成物、被
/// `app/.gitignore` 忽略，Rust 门这条腿（`cargo test`，不跑 `tauri android build`）在裸
/// checkout 上根本读不到它。把它拉进取材面等于让本门在 CI 上必然 panic。
#[test]
fn every_keep_rule_this_batch_added_is_present() {
    let keeps = strip_hash_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PROGUARD),
        PROGUARD,
        "-keep class org.rustls.platformverifier.",
    ));
    // 逐**整行**对拍，不用 `contains`：`-dontwarn a.b.C` 是 `-dontwarn a.b.CSomething` 的前缀，
    // 子串计数对「在类名后面接几个字符」这一形是瞎的 —— 而那一形下规则一个类都罩不住。
    // （同一个形状 2026-09-05 在 rustls 那条 keep 上实测过：改成 `.NoSuchThing` 两支门全绿。）
    let lines = trimmed_lines(&keeps);
    for (rule, why) in NEW_KEEP_RULES {
        let hits = lines.iter().filter(|line| *line == rule).count();
        assert_eq!(
            hits, 1,
            "{PROGUARD}：整行 `{rule}` 命中 {hits} 次（应恰好 1 次）。它守的是：{why}"
        );
    }
}

/// ④b R8 的 "Missing classes" 抑制规则在位 —— 少了它，**首次** release 构建当场失败。
///
/// 与 ④ 分开是因为判据的语义不同：④ 守的是「反射面别被剪」（运行期故障），本条守的是
/// 「构建能不能跑完」（构建期故障）。两者的失败时刻、症状、修法都不一样，混在一条断言里
/// 会让失败信息说不清是哪一类。
#[test]
fn the_r8_missing_class_suppressions_are_present() {
    let keeps = strip_hash_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PROGUARD),
        PROGUARD,
        "-keep class org.rustls.platformverifier.",
    ));
    // 同 ④：逐整行。`-dontwarn java.beans.Transient` 是 `-dontwarn java.beans.TransientX` 的前缀，
    // 子串计数下后者照样绿，而它抑制的是一个不存在的类 —— 首次 release 构建仍会当场失败。
    let lines = trimmed_lines(&keeps);
    for (class, why) in R8_MISSING_CLASS_SUPPRESSIONS {
        let rule = format!("-dontwarn {class}");
        let hits = lines.iter().filter(|line| **line == rule).count();
        assert_eq!(
            hits, 1,
            "{PROGUARD}：整行 `{rule}` 命中 {hits} 次（应恰好 1 次）。它需要在这里的理由：{why}"
        );
    }
}

/// ⑤ 密钥类文件被仓库根 `.gitignore` 覆盖 —— 扩展名从 gradle 那条默认路径反推。
#[test]
fn key_material_can_never_enter_the_repository() {
    let gradle = gradle_source();
    let at = gradle.find("/.polaris/android/").unwrap_or_else(|| {
        panic!(
            "{GRADLE_APP}：找不到默认密钥库路径 `/.polaris/android/…` —— \
             约定位置改了。改路径就要一并改 `.gitignore` 的模式，本门正是为此存在。"
        )
    });
    let rest = &gradle[at..];
    let end = rest
        .find('"')
        .unwrap_or_else(|| panic!("{GRADLE_APP}：默认密钥库路径的字符串没有收尾引号。"));
    let default_path = &rest[..end];
    let (_, extension) = default_path.rsplit_once('.').unwrap_or_else(|| {
        panic!("{GRADLE_APP}：默认密钥库路径 `{default_path}` 没有扩展名，反推不出忽略模式。")
    });

    let ignore = strip_hash_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(GITIGNORE),
        GITIGNORE,
        "!/resources/data/",
    ));
    let entries: BTreeSet<&str> = ignore
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let derived = format!("*.{extension}");
    assert!(
        entries.contains(derived.as_str()),
        "{GITIGNORE}：没有忽略 `{derived}`，而 gradle 的默认密钥库正是 `{default_path}`。\
         签名密钥一旦入库，撤销它的成本是重新签一版并让所有已装用户手动重装。"
    );
    for pattern in KEY_MATERIAL_IGNORES {
        assert!(
            entries.contains(pattern),
            "{GITIGNORE}：没有忽略 `{pattern}`。口令按裁定只走环境变量、绝不落盘，\
             但「不落盘」这件事需要一条守卫兜住手滑 —— 这就是那条守卫。"
        );
    }
}
