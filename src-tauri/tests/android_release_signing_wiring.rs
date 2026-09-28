//! Android **release 构建路径**的接线门 —— R8 keep 规则 / 签名凭据 / 密钥不入库。
//!
//! # 守的是什么（根因）
//!
//! 这个应用从来没有在 R8 开着的情况下跑过一次：CI 只跑 `assembleArm64Debug`，而 debug 侧
//! `isMinifyEnabled = false`。于是 release 路径上的**每一条**判据今天都没有任何观测面 ——
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
//! | 本门（源码级） | 那些**只有源码这一个观察面**的事：签名接线那一行在 release 块里**出现且恰好一次**、口令只经环境变量、逃生门**那一条绑定**的右值逐字是哪一条、密钥模式在 `.gitignore` 里、CI 上那几条承重命令行还逐字接着 | 构建**跑起来是什么行为**（R8 到底跑不跑、AGP 手里到底有几份规则、逃生门实际开没开）；以及「出现了」之后**有没有被后一句盖掉**（在那行接线后补一句 `signingConfig = null`，本门不红） |
//! | `scripts/gate-android-release-behavior.sh`（**行为裁判**，android.yml 的 apk 腿） | 跑一次 gradle **配置期**：零凭据必红且点名、逃生门**在它枚举得到的四个来源上**只认命令行实参、`minifyArm64ReleaseWithR8` 在 release 图里恰好 1 次而 debug 图里 0 次、AGP 装配好的 `proguardFiles` 在 release **构建类型**那一处与登记的三份**集合恰等**（多一份少一份都红）且 `defaultConfig`/`productFlavor` 两处为空、凭据在场时 `signingConfigs` **容器**里那份配置的 `storeFile` 存在可读且就是本工程认的那一份 | **执行期**发生的一切：`onlyIf`/`enabled=false` 让任务在图里却不跑、`doFirst` 改文件、R8 剪完之后包里还剩什么；以及配置期里它**没枚举到**的两条缝：逃生门换一个新变量名、release 构建类型有没有真把 signingConfig 接上 |
//! | `android_platform_verifier_wiring.rs` | rustls 校验器那条腿的三侧符号对得上、keep 规则**覆盖得到**那个类全名 | 运行期真的执行了 |
//! | `scripts/verify-wry-keep-rules.mjs`（CI 上跑） | RustWebView 那条 keep 的成员签名与生成物**同一个类里**对得上 | 裸 checkout 上跑不了（取材点 gitignored） |
//! | `scripts/assert-r8-evidence.mjs`（release 冒烟腿） | R8 的 configuration.txt（**剥注释后**）里各来源的规则文本都到过、seeds.txt 里那些 keep 真的**命中**了类与成员 | 提交前拦不住任何东西；且默认不跑 |
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
//! 那三件归 `.github/workflows/android.yml` 的 `release-smoke` job（真跑一次 release 构建，
//! 判据是 `scripts/assert-r8-evidence.mjs` 读 configuration.txt / seeds.txt）—— 而**它至今
//! 一次都没跑过**：`if: inputs.release_smoke` 默认 false，且今天**没有任何自动调用方**会传
//! true（release-risk.yml 的 android job 是一句裸 `uses:`，不带 `with:`），只能手动
//! `workflow_dispatch` 勾上。
//!
//! 另外两件**连 release-smoke 都不归**（终止轮 R4 / R5 / R3）：
//!
//!  · **签名的接线与有效性**。`release-smoke` 那条腿跑的是
//!    `assembleArm64Release … -PpolarisAllowUnsigned=true`，走的正是逃生门、出的就是
//!    未签名包；全仓 `apksigner` / `jarsigner` 一次都没出现过。所以「storeFile 是不是一个
//!    合法密钥库、口令对不对」没有任何门，而**接线**（release 构建类型有没有真把那份
//!    signingConfig 接上）同样没有：本门数的是那行赋值出现几次，行为裁判的 ⑩ 读的是
//!    `signingConfigs` **容器**——在那行之后补一句 `signingConfig = null`，两侧同时全绿。
//!
//!    🔴 2026-09-13 新增的 `release-apk` job（真签名 + 上传成 release 资产）**没有改变这一条**：
//!    它要的四个 secret 今天在本仓一个都没有，且没有任何调用方传 `publish_release: true`
//!    ⇒ **它一次都还没跑过**。本门对它只守两件源码级的事（下方 ⑧）：那条 assemble 行在、
//!    且这条腿上不许出现逃生门。「密钥库合不合法、口令对不对、出的包装不装得上」仍然零观测面，
//!    唯一的观察面还是本机手签一次。
//!  · **逃生门换一个新变量名**。本门的形状断言钉死的是 `val allowUnsignedRelease` **那一条
//!    绑定**；行为裁判的 ④/④b 是一张只试四个名字的枚举表。在守卫体里、逃生门分支之前插一句
//!    `if (System.getenv("POLARIS_XXX") == "true") return@Action`，绑定一字未动、名字不在
//!    枚举表里 ⇒ 两侧同时全绿，而那是一个从环境继承、永久生效的逃生门。这一形今天没有门。
//!
//! 本门的 [`execution_phase_hooks_must_be_registered`] 只是那之前的一道**文本**闸门：
//! 它要求这类钩子在三份**入库的** gradle 脚本里逐处登记，能被什么绕过（`buildSrc/` 就在其中）
//! 写在它自己的头注里。别把它当行为判据用。
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

use std::collections::{BTreeMap, BTreeSet};

/// Android 应用工程的 Gradle 脚本（相对 workspace 根）。
const GRADLE_APP: &str = "src-tauri/gen/android/app/build.gradle.kts";
/// 行为裁判的取材任务住的地方 —— 刻意**不是**上面那份被审判的文件（见 ⑧ 的头注）。
const GRADLE_FACTS: &str = "src-tauri/gen/android/app/polaris-release-facts.gradle.kts";
/// Android 工程**根**的 Gradle 脚本（相对 workspace 根）。
///
/// 它**入库**（`git ls-files` 认得它），裸 checkout 上就在盘上 —— 与 `tauri.build.gradle.kts`
/// 那种 Tauri CLI 生成物、被 `app/.gitignore` 忽略的文件不是一类东西。而它的 `allprojects { }`
/// 碰得到 `:app`：一句 `allprojects { tasks.matching { … }.configureEach { onlyIf { false } } }`
/// 写在这里，与写在 `app/build.gradle.kts` 里对 release 路径是等效的。
/// B2 上一版的取材面只有 app 下那两份，这一份整个不在射程内（2026-09-06 终止轮 R7）。
const GRADLE_ROOT: &str = "src-tauri/gen/android/build.gradle.kts";
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
/// Android 的 CI workflow —— release 冒烟腿、mapping 留存、wry keep 对拍的落点。
const ANDROID_WORKFLOW: &str = ".github/workflows/android.yml";
/// 从生成物里反推 wry keep 规则的那份脚本（判据的唯一实现）。
const WRY_KEEP_SCRIPT: &str = "scripts/verify-wry-keep-rules.mjs";
/// release 路径的**行为**裁判（判据的唯一实现）。本门守它还接在 `android.yml` 上。
const BEHAVIOR_GATE_SCRIPT: &str = "scripts/gate-android-release-behavior.sh";
/// release 冒烟腿的产物级判据本体（configuration.txt 剥注释 + seeds.txt 命中）。
const R8_EVIDENCE_SCRIPT: &str = "scripts/assert-r8-evidence.mjs";

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

#[test]
fn the_declaration_slice_eats_the_whole_binding() {
    let one_line = "val a: Boolean = f(\"x\") == \"true\"\n\nandroid {\n";
    assert_eq!(
        declaration_slice(one_line, 0).trim(),
        "val a: Boolean = f(\"x\") == \"true\""
    );

    // 生产里的形态：`=` 收尾，表达式写在下一行。
    let wrapped = "val a: Boolean =\n    f(\"x\") == \"true\"\n\nandroid {\n";
    assert_eq!(
        declaration_slice(wrapped, 0)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        "val a: Boolean = f(\"x\") == \"true\""
    );

    // 🔴 A4 那条绕法：空一行之后接 `||`。切片必须把它吃进来，否则相等断言看不见它。
    let bypass =
        "val a: Boolean =\n    f(\"x\") == \"true\"\n\n    || g(\"y\") == \"true\"\n\nandroid {\n";
    let sliced = declaration_slice(bypass, 0)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        sliced,
        "val a: Boolean = f(\"x\") == \"true\" || g(\"y\") == \"true\""
    );

    // 反向对照：没有它，一个「吃到文件尾」的切片器也能满足上面三条。
    assert!(
        !declaration_slice(bypass, 0).contains("android {"),
        "切片吃过头了：`android {{` 不属于那条绑定"
    );
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

/// ⑥⑦⑧ CI 上那几条只能靠产物说话的判据，必须还接在 `android.yml` 上。
///
/// # 为什么这两件事的门开在这里
///
/// 它们各自的**真正判据**都跑在 CI 上，本条只守「那条腿还在不在」：
///
/// - **wry keep 对拍**（⑥）：判据要从 `generated/RustWebView.kt` 里提方法签名再对拍规则，
///   而那份文件是 gitignored 的生成物 —— 裸 checkout 上不存在，`cargo test` 读不到。
///   让 Rust 门去读它只有两种结局：CI 上必然 panic，或者写成「文件不在就跳过」而变成一条
///   静默缺席的假门。出路是把判据放在生成物保证在场的那一刻（`tauri android build` 之后），
///   本条守那一步没被删掉。判据本体只有 `scripts/verify-wry-keep-rules.mjs` 一份实现。
/// - **release 冒烟腿**（⑦）：R8 到底吃进了哪些规则、mapping 有没有留下来、包多大，
///   三件都只有跑一次 release 构建才知道。本条守那条腿的定义还在、判据还在。
///
/// # 取材：剥 `#` 注释
///
/// 不剥的话，「把断言步骤整段注释掉」这一形会被注释里剩下的同一串喂成绿。
#[test]
fn the_ci_legs_that_can_only_speak_through_artifacts_are_still_wired() {
    let workflow_raw = polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(ANDROID_WORKFLOW),
        ANDROID_WORKFLOW,
        "name: Android",
    );
    let workflow = strip_hash_comments(&workflow_raw);

    // ── ⑥ 三份判据本体各自还在，且没被换成空壳 ──
    //
    // 🔴 取材面是**剥掉整行注释之后**的代码面（2026-09-05 收官轮 A7）：
    //    实测把 `gate-android-release-behavior.sh` 掏空只留注释 + `exit 0`（53 行），
    //    三根针照样命中 —— 命中的是被判文件**自己的头注**。本仓这已经是第三次同形
    //    （前两次在 nodes.css 与 configuration.txt）。
    for (path, needle, comment_probe, min_code_lines, why) in JUDGE_SCRIPT_SENTINELS {
        let body = polaris_source_probe::repo_file!(path);
        let face = code_face(path, &body);
        // 剥离自检：这句话只写在该文件的注释里，剥干净了下面两条才轮得到说话。
        assert!(
            !face.contains(comment_probe),
            "{path}：注释剥离失效 —— 只出现在注释里的 `{comment_probe}` 仍留在代码面上。\
             判据在这种面上会被文件自己的头注喂绿。"
        );
        let lines = face.lines().count();
        assert!(
            lines >= *min_code_lines,
            "{path}：剥掉注释之后只剩 {lines} 行代码（下限 {min_code_lines}）—— \
             判据本体被掏空成一个只有注释的壳了。它守的是：{why}"
        );
        assert!(
            face.contains(needle),
            "{path}：**代码面**里没有 `{needle}` —— 判据的唯一实现被换掉或清空了。它守的是：{why}"
        );
    }

    // ── ⑥b 退出码承重的命令行：逐字整行对拍 ──
    //
    // 实测 M02：给这几行接一个 `|| true`，「含某串」形态的判据一条不红，而那一步从此永远通过。
    // 退出码是这些步骤**唯一**的输出，判据只能落在整行上。
    let workflow_lines: Vec<&str> = workflow_raw.lines().map(str::trim_end).collect();
    assert!(
        workflow_lines.len() > 200,
        "{ANDROID_WORKFLOW}：只读到 {} 行 —— 取材面塌了，下面的逐行对拍会在空面上恒真。",
        workflow_lines.len()
    );
    for (line, want, why) in CI_EXIT_CODE_BEARING_LINES {
        let hits = workflow_lines.iter().filter(|l| *l == line).count();
        assert_eq!(
            hits, *want,
            "{ANDROID_WORKFLOW}：`{line}` 作为**整行**出现 {hits} 次（应恰好 {want} 次）。\n\
             它守的是：{why}\n\
             这条判据是整行相等，不是「含某串」—— 后面接一个 `|| true` / `; true` / `|| :` \
             会让这一步的退出码永远是 0，而「含某串」的判据一条都不红。\n\
             等价改写（换缩进、拆行、加 env 前缀）同样会红：那是刻意的，改变退出码语义的改写\
             与没改变的，在这里必须由人看一眼。"
        );
    }

    // ── ⑥c 承重行从 workflow 里**派生**，不手抄；并连「步级中和」一起看 ──
    //
    // 两条实测（2026-09-05 收官轮 A6 / A10）：
    //  · A6：给那个步骤插一行 `continue-on-error: true`（`run:` 一字未动）⇒ 上面那张整行表
    //    全绿，而该步骤失败不再让腿红。整行钉死只守 `run:` 那一行，守不住它周围的步级开关。
    //  · A10：上面那张表是**手挑**的。表外还有同形的行（`verify-apk.mjs` 那条、
    //    `gradlew assembleArm64Debug` 那条），给它们接 `|| true` 照绿。
    //
    // 下面把承重行改成从 workflow 派生：凡是 `run:` 里调本仓脚本或 `./gradlew` 的行都算承重。
    let derived = load_bearing_lines(&workflow_lines);
    // 哨兵：派生器变哑（空表 / 少抓）在此红，否则下面两个 for 恒真。
    assert!(
        derived.len() >= 8,
        "{ANDROID_WORKFLOW}：只派生出 {} 条承重命令行 —— 派生器与 workflow 的形状对不上了，\
         下面的中和判定会在一张空表上恒真。",
        derived.len()
    );
    for (line, _, _) in CI_EXIT_CODE_BEARING_LINES {
        assert!(
            derived.iter().any(|(_, text)| text == line),
            "{ANDROID_WORKFLOW}：派生器没抓到登记表里的 `{line}` —— \
             登记表与派生面已经对不上，两者必须覆盖同一族行。"
        );
    }
    for (index, text) in &derived {
        for neutralizer in EXIT_CODE_NEUTRALIZERS {
            assert!(
                !text.contains(neutralizer),
                "{ANDROID_WORKFLOW}:{}：承重命令行上出现了 `{neutralizer}`：\n  {text}\n\
                 这一步的全部输出就是退出码，接上它之后这一步永远通过，而判据一条都不红。",
                index + 1
            );
        }
        let (start, end) = step_block(&workflow_lines, *index, ANDROID_WORKFLOW);
        for line in &workflow_lines[start..end] {
            assert!(
                !line.trim().starts_with("continue-on-error"),
                "{ANDROID_WORKFLOW}:{}：承重命令行所在的**步骤**上挂了 `continue-on-error` —— \
                 `run:` 一字未动，而这一步失败不再让整条腿红。\n  承重行：{text}\n  中和行：{}",
                index + 1,
                line.trim()
            );
        }
    }

    // ── ⑥d 登记表里那几条是「唯一观测面」，它们的步骤连 `if:` 都不许有 ──
    //
    // 派生面上的 `if:` 有正当用法（`if: steps.libbox.outputs.cache-hit != 'true'`），
    // 故上面那个循环不禁它。但登记表这几条是各自那件事**唯一**会说话的地方：
    // 给它们挂一个求值为假的 `if:`，整步静默跳过 —— 与删掉它没有区别，而删掉会被上面的整行
    // 计数抓住，跳过不会。
    //
    // 🔴 射程只到 **step 级**（2026-09-06 终止轮 R8 收窄）：[`step_block`] 切的是
    //    `      - …` 那一段，job 级的 `if:`（缩进 4，写在 `  apk:` 底下）根本不在切片里。
    //    实测：给 `apk` job 挂一句 `if: false`，本条与 ⑥b/⑥c/⑥e 全绿，而那条腿上的每一步
    //    —— 行为裁判、wry 对拍、开箱验 —— 一次都不跑。这一形今天没有门。
    //    （⑥e 那条 `continue-on-error` 是全份文件计数，job 级的能抓住；`if:` 没有对应的一条，
    //    因为 workflow 里本来就有正当的 job 级 `if:`，全份禁掉会把判据改成噪声。）
    for (line, _, _) in CI_EXIT_CODE_BEARING_LINES {
        for (index, text) in derived.iter().filter(|(_, text)| text == line) {
            let (start, end) = step_block(&workflow_lines, *index, ANDROID_WORKFLOW);
            for candidate in &workflow_lines[start..end] {
                assert!(
                    !candidate.trim().starts_with("if:"),
                    "{ANDROID_WORKFLOW}:{}：唯一观测面那几步里出现了 `if:`：\n  承重行：{text}\n  \
                     条件行：{}\n\
                     求值为假时整步静默跳过，而「跳过」与「跑过且通过」在腿的结论上不可分辨。\
                     真要加条件就来改这条判据，并在这里写清楚为什么那个条件不会把判据关掉。",
                    index + 1,
                    candidate.trim()
                );
            }
        }
    }

    // ── ⑥e 整份 workflow 不许出现 `continue-on-error`（含 job 级）──
    //
    // 上面那条只看承重行所在的步骤。`continue-on-error` 也可以挂在 **job** 上，
    // 那样整个 job 失败都不再让调用方红 —— 而 release-risk 的 gate job 断言的正是
    // 「被选中就必须 success」。取材面剥过注释：注释里写着这个词不算。
    let neutralized = workflow.matches("continue-on-error").count();
    assert_eq!(
        neutralized, 0,
        "{ANDROID_WORKFLOW}：出现了 {neutralized} 处 `continue-on-error`。\
         本腿的每一步都是判据，没有一条允许「失败也算过」。\
         真有非判据的步骤要它，就来改这条判据并写明哪一步、为什么。"
    );

    // ── ⑦ release 冒烟腿：定义 + 四条判据 ──
    for (needle, why) in CI_RELEASE_SMOKE_REQUIREMENTS {
        let hits = workflow.matches(needle).count();
        assert!(
            hits > 0,
            "{ANDROID_WORKFLOW}：找不到 `{needle}`（命中 {hits} 次）。它守的是：{why}"
        );
    }

    // ── ⑧ 发布腿不许带逃生门（2026-09-13 新增） ──
    //
    // `release-apk` 是**唯一**会把包发给用户的那条腿。`-PpolarisAllowUnsigned=true` 换来的是
    // `app-arm64-release-unsigned.apk`，而 Android 只接受与已装应用**同一把签名**的升级包 ——
    // 未签名 / 签名不一致的包发出去，所有老用户点安装得到的是一句「应用未安装」，
    // 且那件事**应用内观测不到**（ACTION_VIEW 交出去之后没有回调）。
    //
    // 判据取材面是整个 `release-apk` job 的**原文**（含注释）：注释里写着这个词也红。
    // 那是刻意的 —— 说明与实参在文本上不可分辨，把判据改成「只扫非注释行」等于给
    // 「注释里写一句、实参里也写一句」开一条缝。代价是那条腿的注释得绕着这个字面量走，
    // 它自己在头注里写清了这一点（2026-09-13 落地时实测撞过一次，故留此记录）。
    let publish_job = slice_between(&workflow_raw, "\n  release-apk:\n", "", ANDROID_WORKFLOW);
    assert!(
        publish_job.contains("bash scripts/build-android-apk.sh --apk --split-per-abi --target aarch64 --ci \\\n"),
        "{ANDROID_WORKFLOW}：`release-apk` 里没有那条**不带逃生门**的 Tauri 构建行 —— \
         发布腿要么没在构建 release，要么已经改成了别的形态，先来这里说清楚。"
    );
    assert!(
        !publish_job.contains("polarisAllowUnsigned"),
        "{ANDROID_WORKFLOW}：`release-apk`（唯一会把包发给用户的那条腿）里出现了 \
         `polarisAllowUnsigned`。那个逃生门产出的是未签名包，而 Android 只接受与已装应用同一把 \
         签名的升级包 —— 发出去就是一个所有老用户都装不上的资产，且失败发生在系统安装器里，\
         应用内看不见。"
    );
    let smoke_job = slice_between(
        &workflow_raw,
        "\n  release-smoke:\n",
        "\n  release-apk:\n",
        ANDROID_WORKFLOW,
    );
    assert!(
        smoke_job.contains(":app:assembleArm64Release -PpolarisAllowUnsigned=true")
            && smoke_job.contains("-x :app:rustBuildArm64Release")
            && smoke_job.contains("cargo clean -p polaris --target aarch64-linux-android --release")
            && smoke_job.contains("Android release 签名凭据缺失 —— 拒绝静默产出未签名包。")
            && smoke_job.contains("[ -s \"$so\" ] && [ \"$so\" -nt \"$marker\" ]")
            && smoke_job.contains("readlink -f \"$jni/libpolaris_lib.so\"")
            && smoke_job.contains("source=$(git rev-parse HEAD)")
            && !smoke_job.contains("--config src-tauri/tauri.android.conf.json -- -PpolarisAllowUnsigned=true"),
        "{ANDROID_WORKFLOW}：release 冒烟腿必须先新建 Rust SO、仅接受签名守卫拒绝，再由 Gradle 本次命令行显式开启 unsigned；Tauri 尾参会误传给 Cargo"
    );
    // 正面对照：这个词在别处（`release-smoke` 那条腿）**必须**还在，否则上面那条否定断言
    // 可能只是因为整份 workflow 里它已经消失了（那时逃生门的判据本身塌了，不是发布腿干净）。
    assert!(
        workflow_raw.contains("-PpolarisAllowUnsigned=true"),
        "{ANDROID_WORKFLOW}：整份 workflow 里已经没有 `-PpolarisAllowUnsigned=true` —— \
         上面那条「发布腿不许有它」于是变成一条恒真的断言。先确认冒烟腿还在。"
    );

    // ── 前置步骤逐字对拍：三个 job 的 prelude 是复制出来的，会漂 ──
    //
    // GitHub Actions 没有 job 内的 step 复用。复制的代价是三份必然漂，而漂的方向恰好是
    // 最糟的那种：某条腿用着一套旧的 NDK/JDK 解析逻辑，出的产物与另一条腿不可比，
    // 且没有任何东西会说话。这里按**原文**（不剥注释）对拍，注释漂了同样红。
    //
    // 🔴 切片顺序承重：`release-apk` 排在 `release-smoke` **之后**，故 smoke 的切片终点必须是
    //    下一个 job 的头，不能再取到文件尾 —— 取到尾会把 `release-apk` 的 prelude 也圈进来，
    //    而 `slice_between` 取的是**第一处** `PRELUDE_END`，于是两条腿比的是同一段文本，
    //    那条相等断言从此恒真。
    let apk_job = slice_between(
        &workflow_raw,
        "\n  apk:\n",
        "\n  release-smoke:\n",
        ANDROID_WORKFLOW,
    );
    let smoke_job = slice_between(
        &workflow_raw,
        "\n  release-smoke:\n",
        "\n  release-apk:\n",
        ANDROID_WORKFLOW,
    );
    let preludes = [
        (
            "apk",
            slice_between(apk_job, PRELUDE_START, PRELUDE_END, ANDROID_WORKFLOW),
        ),
        (
            "release-smoke",
            slice_between(smoke_job, PRELUDE_START, PRELUDE_END, ANDROID_WORKFLOW),
        ),
        (
            "release-apk",
            slice_between(publish_job, PRELUDE_START, PRELUDE_END, ANDROID_WORKFLOW),
        ),
    ];
    for (job, prelude) in &preludes {
        assert!(
            prelude.len() > 2000,
            "{ANDROID_WORKFLOW}：`{job}` 切出来的 prelude 只有 {} 字节 —— 切片锚点失效了，\
             下面那条相等断言会在空串上恒真。",
            prelude.len()
        );
    }
    for (job, prelude) in &preludes[1..] {
        assert_eq!(
            preludes[0].1, *prelude,
            "{ANDROID_WORKFLOW}：`apk` 与 `{job}` 两个 job 的前置步骤已经不一致。\n\
             射程：从 `{PRELUDE_START}` 起，到 `{PRELUDE_END}` 止。\n\
             这段是复制出来的（GHA 没有 job 内 step 复用），改一处就要改另外两处 —— \
             否则某条腿会用着一套旧的工具链解析逻辑，出的产物与别的腿不可比。"
        );
    }
}

/// prelude 对拍的起止锚点（两个 job 里都逐字存在、且各只出现一次的两个串）。
///
/// 终点刻意落在「libbox.aar 必须在位」那步的最后一行，而不是下一个 step 的 `- name:` ——
/// 从那里往后两条腿**本来就该不同**（debug 侧 `tauri android build --debug`，release 侧不带
/// 那个 flag），把差异面圈进对拍只会逼后人把判据改宽 = 门被磨钝。
const PRELUDE_START: &str = "      - uses: actions/checkout@v7";
const PRELUDE_END: &str = "sha256=$(sha256sum";

/// release 冒烟腿必须持有的判据：`(在 workflow 里要找到的串, 它守什么)`。
///
/// 🔴 这张表只守「这条腿的**结构**还在」：job 名、触发条件、留存清单、体积基准。
/// 「R8 到底吃进了什么、保住了什么」的针**不在这里** —— 它们住在
/// `scripts/assert-r8-evidence.mjs`，由 `scripts/assert-r8-evidence.test.mjs` 的正反用例钉着。
/// 上一版把那些针写在 workflow 的内联 shell 里，于是判据既没法单测，取材也没剥注释：
/// `io.nekohasekai` 命中的是本仓 `proguard-rules.pro` 注释里的同一句话（R8 的
/// `-printconfiguration` 会把每份规则文件的原文连注释一起抄进 `configuration.txt`），
/// libbox 的 consumer 规则一条没进 R8 也照绿。
const CI_RELEASE_SMOKE_REQUIREMENTS: &[(&str, &str)] = &[
    (
        "  release-smoke:",
        "冒烟腿本体。没有它，「R8 到底跑不跑得完」「包多大」两件事至今零观测面。",
    ),
    (
        "if: inputs.release_smoke",
        "它只在显式请求时跑 —— 这条腿是 debug 腿的全部成本再加上 R8，不该挂在每次调用上。",
    ),
    (
        "mapping/arm64Release/configuration.txt",
        "R8 自己吐出来的**实际生效规则全文**。「libbox.aar 的 consumer 规则到底进不进 R8」\
         这一跳发生在 AGP 内部、源码上看不见，这是今天唯一能拿到的产物级证据。",
    ),
    (
        "mapping/arm64Release/seeds.txt",
        "R8 列出的「每条 keep 规则实际命中的类与成员」。configuration.txt 只证明规则**文本**\
         到过 R8：一条指向不存在类名的 keep 同样会原样出现在那里，而那个类已被整包剪掉。\
         判据本体（scripts/assert-r8-evidence.mjs）读的就是这份，所以它必须被留存。",
    ),
    (
        "name: android-release-mapping",
        "mapping.txt 的保全落点。它只存在于构建机的 app/build/ 下，clean 一次或换台机器，\
         线上那条崩溃就永远还原不出来（重构一版也没用：R8 输入一变，映射就对不上）。",
    ),
    (
        "if-no-files-found: error",
        "留存必须是正面断言。一个文件都没命中 = R8 没跑或产物路径变了，不许静默绿。",
    ),
    (
        "219011984",
        "体积对比的 debug 基准（2026-09-05 实测）。没有基准，量出来的数字读不出信息。",
    ),
];

/// 三份判据本体各自的存在性哨兵：
/// `(路径, 代码面里必须有的串, 只出现在注释里的自检串, 代码行数下限, 它守什么)`。
///
/// 只证明「文件还在、不是个空壳」。判据本身有没有牙由各自的测试与变异回放守，不由这里守。
///
/// # 三个字段各自防的是什么（2026-09-05 收官轮 A7）
///
/// 上一版只有「文件里含某串」一条，取材面是**原文**。实测把
/// `gate-android-release-behavior.sh` 掏空只留注释 + `exit 0`（53 行）⇒ 三根针照样命中，
/// 因为命中的是被判文件自己的头注 —— 每份脚本的头注里都写满了本门要找的那些串。
///
///  · **代码面**（剥掉整行注释后）里必须有那个串 —— 掏空成注释壳时当场红；
///  · **自检串**只出现在该文件的注释里，剥干净之后必须**消失** —— 没有它，剥离器哪天变哑
///    （比如扩展名判定漏了一支）会让上一条又退回到原文匹配，而没有任何东西会说话；
///  · **代码行数下限**兜住「留一行代码骗过针」那一形。三个数字取的是 2026-09-05 实测值
///    （188 / 330 / 158 行代码面）的一半上下：正常增删碰不到，真缩到那个量级时该有人看一眼。
const JUDGE_SCRIPT_SENTINELS: &[(&str, &str, &str, usize, &str)] = &[
    (
        WRY_KEEP_SCRIPT,
        "RustWebView",
        "ProGuard 对「keep 了一个不存在的成员」不报错",
        90,
        "wry 那条 keep 的成员签名与 gitignored 生成物的对拍。",
    ),
    (
        BEHAVIOR_GATE_SCRIPT,
        "printPolarisReleaseFacts",
        "文本里那串还在，行为已经变了",
        120,
        "行为裁判问 AGP 要事实的那个只读任务名。任务名两侧一改就对不上，\
         而裁判会因为「任务不存在」直接 rc≠0 —— 这条守的是它还在问同一个人。",
    ),
    (
        R8_EVIDENCE_SCRIPT,
        "seeds.txt",
        "内联 shell 的判据没法单测",
        80,
        "产物级判据里「keep 到底命中没命中」那一半。只剩 configuration.txt 那一半时，\
         指向不存在类名的 keep 依然全绿。",
    ),
];

/// 剥掉**整行**注释，得到一份「只剩代码」的取材面。
///
/// 按扩展名分流：`.sh` 认 `#`，其余（`.mjs`）认 `//` / `/*` / `*`（JSDoc 的续行）。
///
/// # 为什么是整行而不是逐字符词法
///
/// 方向是「宁可多留」：行尾注释仍会进面，于是失败形态是**多要求**一处代码，而不是漏掉一处。
/// 逐字符剥离要处理 JS 的正则字面量（`/^#+\s*/` 里就带着 `*/`）与模板串，一个边界没考虑到
/// 就会静默吃掉大半个文件 —— 本仓 2026-09-05 在 `build.gradle.kts` 上刚栽过一次同形的。
fn code_face(path: &str, body: &str) -> String {
    let is_comment = |line: &str| {
        if path.ends_with(".sh") {
            line.starts_with('#')
        } else {
            line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
        }
    };
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_comment(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `android.yml` 里**退出码承重**的命令行：`(整行原文, 应出现次数, 它守什么)`。
///
/// 这几步的全部输出就是退出码：判据必须落在整行上。实测 M02 —— 给它们各接一个 `|| true`，
/// 「含某串」形态的判据一条不红，而那三步从此永远通过。
const CI_EXIT_CODE_BEARING_LINES: &[(&str, usize, &str)] = &[
    (
        "        run: node scripts/verify-wry-keep-rules.mjs",
        3,
        "wry keep 对拍，debug 腿 / release 冒烟腿 / release 资产腿各一次。那条 keep 盯的是 \
         gitignored 生成物，只有这三处的上一步把它铺出来；删掉任何一处，wry 改名/改签名就再也 \
         没人说话。2026-09-13 从 2 改成 3：`release-apk` 那条是**真发给用户**的那个包，\
         在两个不发布的包上守而放过发布包，守的方向是反的。",
    ),
    (
        "        run: bash scripts/gate-android-release-behavior.sh",
        1,
        "release 路径的行为裁判。它是 minify 开没开、逃生门认哪几个来源、AGP 手里到底有几份\
         规则文件这三件事**唯一**的观测面 —— 源码级判据对它们全部是瞎的。",
    ),
    (
        "        run: node scripts/assert-r8-evidence.mjs \
         src-tauri/gen/android/app/build/outputs/mapping/arm64Release",
        1,
        "release 冒烟腿的产物级判据：R8 吃进了什么（剥注释后的 configuration.txt）、\
         又保住了什么（seeds.txt）。",
    ),
    (
        "          bash scripts/build-android-apk.sh --apk --split-per-abi --target aarch64 --ci \\",
        1,
        "发布腿必须走未带 unsigned 逃生门的正常 Tauri release 构建。",
    ),
    (
        "          if bash scripts/build-android-apk.sh --apk --split-per-abi --target aarch64 --ci \\",
        1,
        "冒烟腿先强制产出本次源码的 Release SO，只允许明确的签名守卫拒绝。",
    ),
    (
        "          bash src-tauri/gen/android/gradlew --project-dir src-tauri/gen/android \\",
        1,
        "冒烟腿随后由 Gradle 本次命令行显式开启 unsigned，并只跳过已完成的 Rust task。",
    ),
];

/// 「这一行调了本仓的判据/构建」的识别串 —— 承重行由此**派生**，不手抄。
///
/// 2026-09-05 收官轮 A10：上一版的承重行是一张手挑的表，表外还有同形的行
/// （`verify-apk.mjs` 那条、`gradlew assembleArm64Debug` 那条），接 `|| true` 照绿。
/// 手挑的表守不住「同形的下一条」，因为下一条不在表里。
const LOAD_BEARING_INVOCATIONS: &[&str] = &[
    "node scripts/",
    "bash scripts/",
    "sh scripts/",
    "./gradlew",
    "src-tauri/gen/android/gradlew",
];

/// 把一条命令的退出码中和掉的写法。命中任一即红。
///
/// # 🔴 这是一张**黑名单**，不是白名单（2026-09-06 终止轮 R8 收窄）
///
/// 上一版把它当成「退出码中和这一形已经封住了」在用。封不住：shell 里中和退出码的写法是
/// 无穷的，这张表只列得出今天认得的这几个。表外照样绿的一形（终止轮实测）：
///
/// ```text
/// run: node scripts/verify-apk.mjs <path> || test 1
/// ```
///
/// 它不含表里任何一个串，那一行又不在 [`CI_EXIT_CODE_BEARING_LINES`] 登记表里（整行相等
/// 判定管不到它），于是本测试 12 项全绿而那一步的退出码恒为 0。同形的还有
/// `; exit 0`、`|| :;`、把命令挪进多行体里先 `set +e` 等等 —— 一条都没枚举。
///
/// 它守得住的只有一件事：**这几个最顺手的写法**一出现就红，逼人换一个显眼的写法 ——
/// 而显眼的写法要过 review。别把它读成「承重行的退出码有人守着」。
const EXIT_CODE_NEUTRALIZERS: &[&str] = &[
    "|| true",
    "||true",
    "|| :",
    "; true",
    "|| exit 0",
    "|| echo",
];

/// 从 workflow 的行里派生出全部**承重命令行**（行号从 0 起，返回原行）。
///
/// 整行注释不算：注释里写着 `node scripts/x.mjs` 的地方不是一条会跑的命令。
fn load_bearing_lines(lines: &[&str]) -> Vec<(usize, String)> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim();
            !trimmed.starts_with('#')
                && LOAD_BEARING_INVOCATIONS
                    .iter()
                    .any(|needle| trimmed.contains(needle))
        })
        .map(|(index, line)| (index, (*line).to_string()))
        .collect()
}

/// 取 `idx` 行所在的那个 **step**（`      - …` 起，到下一个缩进 ≤ 6 的非空行止）。
///
/// 切不出来一律 panic 并点名：读不到 step 边界就返回整份文件的话，
/// 「这一步上有没有 `continue-on-error`」会退化成「整份 workflow 里有没有」。
fn step_block(lines: &[&str], idx: usize, origin: &str) -> (usize, usize) {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let mut start = idx;
    loop {
        let line = lines[start];
        if line.trim_start().starts_with("- ") && indent(line) <= 6 {
            break;
        }
        if start == 0 {
            panic!(
                "{origin}:{}：往上找不到这一行所属的 step（`      - …`）—— \
                 workflow 的缩进形状变了，步级判据全部作废。",
                idx + 1
            );
        }
        start -= 1;
    }
    let mut end = idx + 1;
    while end < lines.len() {
        let line = lines[end];
        if !line.trim().is_empty() && indent(line) <= 6 {
            break;
        }
        end += 1;
    }
    (start, end)
}

/// 取 `from` 之后、`until` 之前的切片；`until` 为空串表示取到结尾。
///
/// 切不出来一律 panic 并点名：读不到目标就静默返回空串的话，下游的相等断言会在两个空串上恒真。
fn slice_between<'a>(source: &'a str, from: &str, until: &str, origin: &str) -> &'a str {
    let start = source.find(from).unwrap_or_else(|| {
        panic!("{origin}：找不到切片起点 `{from}` —— 取材失败，下游断言全部作废。")
    }) + from.len();
    let rest = &source[start..];
    if until.is_empty() {
        return rest;
    }
    let end = rest
        .find(until)
        .unwrap_or_else(|| panic!("{origin}：`{from}` 之后找不到切片终点 `{until}` —— 取材失败。"));
    &rest[..end]
}

/// 执行期钩子的 token：出现即必须登记。
///
/// 大小写敏感是刻意的：`isMinifyEnabled` 里的 `Enabled` 首字母大写，不会顶到 `enabled` 上。
const EXECUTION_PHASE_HOOKS: &[&str] = &[
    "onlyIf",
    "enabled",
    "doFirst",
    "doLast",
    "configureEach",
    "matching",
];

/// 执行期钩子的**登记表**：`(文件, 钩子, 允许出现次数, 为什么允许)`。
///
/// 表外一律 0 次。要加一个就来这里写一行，并写清楚「它在执行期做什么、为什么不改判据面」。
const REGISTERED_EXECUTION_HOOKS: &[(&str, &str, usize, &str)] = &[(
    GRADLE_FACTS,
    "doLast",
    1,
    "事实任务的打印体。配置期把三样值取好，执行期只 println —— 不改任何文件、不改任何任务状态。\
     它自己就是裁判的取材点：真被换成会改文件的东西，裁判读到的事实与 R8 到手的东西就对不上了。",
)];

/// B2 `release` / minify 相关的**执行期钩子**必须逐处登记。
///
/// # 🔴 这是一条**文本判据**，不是行为判据 —— 它能被什么绕过，写在这里
///
/// 本仓的行为裁判（`scripts/gate-android-release-behavior.sh`）跑的是 gradle 的**配置期**，
/// 而下面这几种写法把事情推到**执行期**，配置期结构上看不见：
///
/// ```text
/// tasks.matching { it.name.contains("minify") }.configureEach { onlyIf { false } }
///     ⇒ 任务仍在任务图里（--dry-run 照样打印它），真跑时被跳过；裁判的 ⑤ 数的是图，全绿。
/// tasks.named("…") { doFirst { file("proguard-rules.pro").writeText("") } }
///     ⇒ 配置期读到的清单与字节数全对，R8 到手的是一份空规则；裁判的 ⑦ 量的是配置期那一刻。
/// ```
///
/// 真正能判它们的是**真跑一次 release 构建**，即 `android.yml` 的 `release-smoke` job
/// （`if: inputs.release_smoke`，**至今一次都没跑过**）。本条只是在那之前多要一次人的确认：
/// 这几个 token 一出现就红，逼作者来登记表里写一行「它在执行期做什么」。
///
/// **它挡不住**（逐条如实登记，不假装）：
///  1. 换个拼写的等价 API：`setEnabled(false)`、`task.enabled` 经反射、`dependsOn` 造环；
///  2. 挂在别处的监听器：`gradle.taskGraph.beforeTask { … }`、`TaskExecutionListener`、
///     `gradle.projectsEvaluated { … }` 里改任务状态；
///  3. **取材面之外的文件**（取材面就是 [`GRADLE_APP`] / [`GRADLE_FACTS`] / [`GRADLE_ROOT`]
///     这三份，别的一份都不读）。`tauri.build.gradle.kts` 是 Tauri CLI 生成物、被
///     `app/.gitignore` 忽略，裸 checkout 上不存在 ⇒ 本条读不到它（`build.gradle.kts` 末尾
///     `apply(from = …)` 它）；init script、`~/.gradle/init.d/`、`GRADLE_USER_HOME` 下的
///     东西根本不在仓里；而 `buildSrc/`（`RustPlugin.kt` / `BuildTask.kt`）**入库**、裸
///     checkout 上就在，本工程那五个 productFlavor 就是它建的 —— 往那里写一句执行期钩子，
///     本条一个字都看不见，这一形今天**没有门**（2026-09-06 终止轮 R7 实测：把
///     `tasks.matching { … }.configureEach { onlyIf { false } }` 写进 `RustPlugin.kt`，
///     本条 12 项全绿）；
///  4. 字符串拼出来的方法名 / Groovy 动态派发。
///
/// 换句话说：本条把「显式写在那三份 gradle 脚本里的执行期钩子」变成必须过人眼的东西，
/// 其余那一层今天**没有门**，归属见 `gate-android-release-behavior.sh` 头注那张天花板表。
#[test]
fn execution_phase_hooks_must_be_registered() {
    for path in [GRADLE_APP, GRADLE_FACTS, GRADLE_ROOT] {
        let source = polaris_source_probe::mask_comments(&polaris_source_probe::repo_file!(path));
        // 取材面自检：净化面塌了（比如剥离器把整份文件吃掉）会让下面每一条计数恒为 0 = 恒绿。
        assert!(
            source.contains("tasks.register") || source.contains("android {"),
            "{path}：净化面上既没有 `tasks.register` 也没有 `android {{` —— \
             取材面塌了，下面的登记判定会在一份空文本上恒真。"
        );
        for hook in EXECUTION_PHASE_HOOKS {
            let hits = count_identifiers(&source, hook);
            let registered = REGISTERED_EXECUTION_HOOKS
                .iter()
                .find(|(file, token, _, _)| *file == path && token == hook)
                .map(|(_, _, count, _)| *count)
                .unwrap_or(0);
            assert_eq!(
                hits, registered,
                "{path}：执行期钩子 `{hook}` 出现 {hits} 次，登记的是 {registered} 次。\n\
                 这类钩子把事情推到**执行期**，而 release 路径的行为裁判跑的是配置期 —— \
                 `onlyIf {{ false }}` 之后任务仍在任务图里（裁判的 ⑤ 全绿）而真跑时被跳过；\n\
                 `doFirst {{ …writeText(\"\") }}` 之后配置期读到的规则清单与字节数全是对的，\
                 而 R8 到手的是一份空规则（裁判的 ⑦ 全绿）。\n\
                 确实需要它 ⇒ 在 REGISTERED_EXECUTION_HOOKS 里写一行，说清它在执行期做什么；\n\
                 不需要 ⇒ 删掉它。\n\
                 🔴 本条是**文本判据**：换个拼写（`setEnabled`）、挂到别处（`beforeTask`）、\
                 或写进取材面之外的文件（生成物 `tauri.build.gradle.kts`，以及**入库的** \
                 `buildSrc/`——终止轮实测把同一句写进 RustPlugin.kt，本条 12 项全绿），\
                 它都看不见。那一层归 android.yml 的 release-smoke 腿，而那条腿至今一次都没跑过\
                 （2026-09-06 实测：android.yml 根本没在远端注册过）。"
            );
        }
    }
}

/// 数 `needle` 作为**标识符**出现的次数：前一个字符不能是标识符字符。
///
/// 没有它，`enabled` 会被 `isMinifyEnabled` 之类顶上；有了它，判据数的才是那个 API 本身。
fn count_identifiers(haystack: &str, needle: &str) -> usize {
    let bytes = haystack.as_bytes();
    let mut count = 0usize;
    let mut from = 0usize;
    while let Some(offset) = haystack[from..].find(needle) {
        let at = from + offset;
        let boundary = at == 0 || {
            let previous = bytes[at - 1];
            !(previous.is_ascii_alphanumeric() || previous == b'_')
        };
        if boundary {
            count += 1;
        }
        from = at + needle.len();
    }
    count
}

#[test]
fn the_identifier_counter_has_teeth() {
    // 正：独立标识符数得到。
    assert_eq!(count_identifiers("enabled = false", "enabled"), 1);
    assert_eq!(count_identifiers("task.enabled = false", "enabled"), 1);
    // 反：拼在别的标识符里的不算 —— 没有这条，`isMinifyEnabled` 会把登记表逼成一张假表。
    assert_eq!(count_identifiers("isMinifyEnabled = true", "enabled"), 0);
    assert_eq!(count_identifiers("myOnlyIfHelper()", "onlyIf"), 0);
    assert_eq!(
        count_identifiers("onlyIf { false }\nonlyIf { true }", "onlyIf"),
        2
    );
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
