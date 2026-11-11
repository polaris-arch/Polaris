//! Android **原生面**的源码级门：包可见性 / 安装器不静默失败（权限画像由 `scripts/verify-apk.mjs`
//! 判据 ⑦ 在产物侧守）。
//!
//! # 守的是什么（三条各自的失效形态，都没有运行期表征）
//!
//! 本批（W-09b 已装应用枚举、W-21 交系统安装器）新增了两条只有 Kotlin 才写得出来的腿，
//! 它们各自带来一类**在 CI 上完全看不见**的事故：
//!
//! | # | 事故 | 症状 | 谁能抓 |
//! |---|---|---|---|
//! | 1 | 权限画像无声漂移（多一条 / 少一条） | 多：安全边界被悄悄放宽；少：某条腿在真机上恒失败 | 产物侧 `verify-apk.mjs` ⑦ |
//! | 2 | 包可见性声明与代码走的路不一致 | 真机上「已装应用」是**空列表**，不报错、不抛异常 | 只有源码 |
//! | 3 | 「安装未知应用」没授权那一支被吞掉 | 点了更新**什么都没发生** | 只有源码 |
//!
//! 三条的共同形状是**沉默**：APK 照样打得出、Kotlin 照样编得过、`cargo test` 照样全绿、
//! `verify-apk.mjs` 的六条判据一条都不响。第 2、3 条甚至在模拟器上也不一定复现
//! （包可见性过滤只在 targetSdk ≥ 30 且真的装了第三方应用的机器上才显形；
//! 「安装未知应用」在开发机上往往早就授过了）。
//!
//! # 🔴 射程自曝：这三道门**证明不了**什么
//!
//! 逐条写清楚，不许把它们说得比实际宽：
//!
//! - **本批一次模拟器都没起过，一台真机都没验过。** 下面每一条断言的对象都是**源码文本**：
//!   manifest 里写了什么、Kotlin 里的调用顺序是什么。它们证明「代码是这么写的」，
//!   证明不了「系统是这么响应的」。具体地：
//!   · `<queries>` 声明**真的**让 `queryIntentActivities` 返回非空 —— 没验过；
//!   · `REQUEST_INSTALL_PACKAGES` + `canRequestPackageInstalls()` 在 API 26/30/36 上的实际取值 —— 没验过；
//!   · `FileProvider.getUriForFile` 对 `cacheDir/updates/*.apk` 真的解析得出 URI —— 没验过。
//!   [`file_provider_exposure_matches_the_registry_exactly`] 只对 `res/xml/file_paths.xml` 的
//!   `<paths>` 子标签集合做源码级断言，那只证明**声明**在，不证明**解析**成功；
//!   · 系统安装器真的被拉起来 —— 没验过。
//! - **守得住「判据在」，守不住「判据是对的」**：第 3 条断言的是「`canRequestPackageInstalls`
//!   在 `getUriForFile` 之前被调用过、且没授权那一支有可读回报」。它守不住那个回报的**内容**
//!   对不对（码写错一个字母仍然绿），但**方向**是守得住的（[`call_site_is_negated`]：`!` 被删掉
//!   必须红）。也守不住 Kotlin 之外的链路（Rust 侧把 reason 吞掉这一形，
//!   由 `commands/updater/tests` 的次序锁与本文件的 [`rust_side_passes_the_reason_through`] 成对守）。
//! - 🔴 **只看本仓自有的 `AndroidManifest.xml`，看不见 manifest 合并**：APK 真正申请的权限集是
//!   AGP 的 manifest merger 把**库 manifest** 合并进来之后的结果，而本文件的取材面是合并**之前**
//!   那一份。2026-09-06 实测两者已经不等：本仓自己声明 8 条，
//!   `app/build/intermediates/packaged_manifests/arm64Debug/**/AndroidManifest.xml` 是 11 条 ——
//!   多出的 `RECEIVE_BOOT_COMPLETED` / `WAKE_LOCK` 来自 `tauri-plugin-notification 2.3.3`、
//!   `com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION` 来自 `androidx.core:core:1.13.1`
//!   （出处见 `app/build/outputs/logs/manifest-merger-arm64-debug-report.txt`）。
//!   出厂集由 `scripts/verify-apk.mjs` 的判据 ⑦ 在**产物侧**（`aapt2 dump permissions <apk>`）守。
//! - **不覆盖 `MainActivity.kt` 与 `StatsBridge.kt`**：取材面只有 `PolarisVpnPlugin.kt` 与
//!   `AndroidManifest.xml`。别的 Kotlin 文件里另写一份包枚举/安装腿，本门看不见
//!   （`scripts/check-android-bridge.mjs` 的 A11d 会拦「`@Command` 搬出插件类文件」这一形，
//!   但拦不住「不是 `@Command` 的普通函数」）。
//!
//! # 取材与自污染
//!
//! 本文件的头注与被审的两份文件里都逐字写满了 `QUERY_ALL_PACKAGES`、
//! `REQUEST_INSTALL_PACKAGES`、`canRequestPackageInstalls` 这些针 —— 不剥注释，判据会被自己
//! 和被审对象的说明文字一起喂饱。故：
//!
//! - manifest 走 [`polaris_source_probe::mask_html_comments`]（XML 注释 = HTML 注释）；
//! - Kotlin 走 [`polaris_source_probe::mask_comments`]（剥注释、**留字符串** —— 本门的针有一半
//!   就是字符串字面量，如 `PACKAGE_VISIBILITY_ROUTE` 的取值与 intent action 的全名）。

// 源码文本门：三平台源码逐字一致（`.gitattributes` eol=lf），只在必跑的 Linux 腿执行一次。
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;

/// 被审的 Android manifest（相对 workspace 根）。
const MANIFEST: &str = "src-tauri/gen/android/app/src/main/AndroidManifest.xml";
/// 被审的 Tauri Android 插件类（两条新腿的实现所在，相对 workspace 根）。
const PLUGIN_KT: &str =
    "src-tauri/gen/android/app/src/main/java/com/polaris2/app/vpn/PolarisVpnPlugin.kt";
/// FileProvider 的授权面声明（相对 workspace 根）。
const FILE_PATHS_XML: &str = "src-tauri/gen/android/app/src/main/res/xml/file_paths.xml";
/// Rust 侧的桥（reason 透传那一条断言的取材，相对 workspace 根）。
const BRIDGE_RS: &str = "src-tauri/src/runtime/proxy/android_bridge.rs";

/// Edge-to-edge 下必须显式接收 IME insets；它不证明 WebView 布局视口已随键盘缩小。
#[test]
fn main_activity_explicitly_requests_ime_resize() {
    let masked =
        polaris_source_probe::mask_html_comments(&polaris_source_probe::repo_file!(MANIFEST));
    let activity = masked
        .split("<activity")
        .skip(1)
        .map(|tail| tail.split('>').next().expect("closed activity tag"))
        .find(|tag| tag.contains("android:name=\".MainActivity\""))
        .expect("MainActivity declaration");
    assert!(activity.contains("android:windowSoftInputMode=\"adjustResize\""));
}

// ═══════════════════════════════════════════════════════════════════════════
// 门 1：**本仓自有** manifest 的 uses-permission（逐条全等一张登记表）
// ═══════════════════════════════════════════════════════════════════════════

/// 从（已剥注释的）manifest 里抠 `uses-permission` 的 `android:name`。
///
/// 判据不做 XML 解析（不引依赖）：按 `<uses-permission` 起、到最近的 `/>` 或 `>` 止，
/// 在这一段里找 `android:name="…"`。抠不出名字 ⇒ panic（判据塌了必须转红，不许静默少取）。
fn declared_permissions(masked_manifest: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = masked_manifest;
    while let Some(at) = rest.find("<uses-permission") {
        let tail = &rest[at..];
        let end = tail
            .find('>')
            .unwrap_or_else(|| panic!("`<uses-permission` 标签没有闭合 —— manifest 被截断了？"));
        let tag = &tail[..end];
        let name_at = tag
            .find("android:name=\"")
            .unwrap_or_else(|| panic!("`<uses-permission` 标签里没有 android:name：{tag}"));
        let value = &tag[name_at + "android:name=\"".len()..];
        let close = value
            .find('"')
            .unwrap_or_else(|| panic!("android:name 的引号没有闭合：{tag}"));
        out.insert(value[..close].to_string());
        rest = &tail[end..];
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// 门 2：包可见性（声明的路线 ⇄ manifest 事实 ⇄ 代码真的走的那条）
// ═══════════════════════════════════════════════════════════════════════════

/// 走 `<queries>` 那条路线的取值（Kotlin 侧 `PACKAGE_VISIBILITY_ROUTE` 的合法值之一）。
const ROUTE_QUERIES: &str = "queries-launcher";
/// 走 `QUERY_ALL_PACKAGES` 权限那条路线的取值。
const ROUTE_QUERY_ALL: &str = "query-all-packages";
/// 敏感权限的全名（只在这里写一次）。
const QUERY_ALL_PERMISSION: &str = "android.permission.QUERY_ALL_PACKAGES";

/// 读（已剥注释的）Kotlin 里 `const val <name> = "…"` 的取值；读不到就 panic。
fn kotlin_str_const(masked_kt: &str, name: &str) -> String {
    let needle = format!("const val {name} = \"");
    let at = masked_kt.get_or_panic(&needle);
    let tail = &masked_kt[at + needle.len()..];
    let close = tail
        .find('"')
        .unwrap_or_else(|| panic!("`const val {name}` 的字符串没有闭合"));
    tail[..close].to_string()
}

/// `str::find` 的「找不到就点名 panic」版本（判据塌了必须转红，不许 `unwrap_or(0)`）。
trait FindOrPanic {
    fn get_or_panic(&self, needle: &str) -> usize;
}
impl FindOrPanic for str {
    fn get_or_panic(&self, needle: &str) -> usize {
        self.find(needle).unwrap_or_else(|| {
            panic!(
                "取材面上找不到 `{needle}` —— 判据塌了（改名 / 文件不对 / 剥离器吃掉了它），\
                 在修好之前不许把它读成「检查通过」"
            )
        })
    }
}

/// 从（已剥注释的）manifest 里切出 `<queries>` … `</queries>` 之间那一段；没有块就 `None`。
///
/// # 🔴 为什么必须切块，不能在整份 manifest 上 `contains`
///
/// 第一版正是在整份文件上 `contains("android.intent.action.MAIN")`，而**每个 Android 应用
/// 都必然有**一个带 `MAIN` + `LAUNCHER` 的启动器 `<intent-filter>`（本仓在
/// `AndroidManifest.xml` 的 `.MainActivity` 里）。那对针于是被应用自己的启动器恒真喂饱，
/// 「`<queries>` 里真的声明了那对 intent」这一向**从来没有被判过**。
///
/// 2026-09-06 实测的两条假绿（两种都让真机上 `queryIntentActivities` 返回空列表、
/// 不报错不抛异常，「自定义应用」选择器恒空）：
///  · 把 `<queries>` 体换成 `<package android:name="com.example.nothing" />` ⇒ 旧判据 12 条全绿；
///  · 把块里的 intent 换成 `ACTION_SEND` + `CATEGORY_DEFAULT` ⇒ 旧判据 12 条全绿。
fn queries_block(masked_manifest: &str) -> Option<&str> {
    let open = masked_manifest.find("<queries>")? + "<queries>".len();
    let close = masked_manifest[open..].find("</queries>")?;
    Some(&masked_manifest[open..open + close])
}

/// `<queries>` 块里是否真的声明了「带启动器图标的 activity」这一类。
///
/// 三件事同时成立才算：块在、块里有 `<intent`、且那段里同时出现 `MAIN` 与 `LAUNCHER`。
/// 少任何一件都判假 —— 方向是 fail-closed（空块 / 只有 `<package>` / 换成别的 intent 全部为假）。
fn declares_launcher_query(masked_manifest: &str) -> bool {
    let Some(block) = queries_block(masked_manifest) else {
        return false;
    };
    block.contains("<intent")
        && block.contains("android.intent.action.MAIN")
        && block.contains("android.intent.category.LAUNCHER")
}

/// 声明的包可见性路线必须与 manifest 的事实一致，**并且与代码真的走的那条一致**。
///
/// # 三向，而不是两向
///
/// 只对「常量 ⇄ manifest」两向是不够的：两边都写着「走 queries」，而代码里调的是
/// `getInstalledPackages()` —— 那条 API 在 targetSdk ≥ 30 上同样被包可见性过滤，
/// 但它拿到的是**另一个集合**（所有可见包，而不是有启动器图标的），且 `<queries>` 的 intent
/// 声明对它不起作用。两向门在这一形上全绿，而真机上列表是空的。
///
/// 故第三向是：代码必须真的按声明的路线取材。
#[test]
fn package_visibility_route_agrees_across_constant_manifest_and_code() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let manifest = polaris_source_probe::mask_html_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(MANIFEST),
        MANIFEST,
        ".vpn.PolarisVpnService",
    ));

    let route = kotlin_str_const(&kt, "PACKAGE_VISIBILITY_ROUTE");
    // **封闭词表**：认不出来的取值一律红，不猜一个默认。方向是 fail-closed。
    assert!(
        route == ROUTE_QUERIES || route == ROUTE_QUERY_ALL,
        "PACKAGE_VISIBILITY_ROUTE = `{route}` 不是本门认识的两条路线之一\
         （`{ROUTE_QUERIES}` / `{ROUTE_QUERY_ALL}`）。新开一条路线要先来这里答题。"
    );

    let declares_query_all = declared_permissions(&manifest).contains(QUERY_ALL_PERMISSION);
    // 🔴 取材面是 `<queries>` **块内**，不是整份 manifest：应用自己的启动器 `<intent-filter>`
    // 里逐字写着同一对针（成因与两条实测假绿见 [`queries_block`]），在整份文件上判等于没判。
    let launcher_intent = declares_launcher_query(&manifest);

    if route == ROUTE_QUERIES {
        assert!(
            launcher_intent,
            "声明走 `{ROUTE_QUERIES}`，但 manifest 里没有带 MAIN + LAUNCHER 的 <queries> 块。\n\
             targetSdk ≥ 30 上这不会报错 —— 真机上 queryIntentActivities 会返回**空列表**，\
             而「自定义应用」的选择器于是永远挑不出东西。"
        );
        assert!(
            !declares_query_all,
            "声明走 `{ROUTE_QUERIES}`，manifest 却同时要了 {QUERY_ALL_PERMISSION}。\n\
             两条路线只能选一条：留着那个权限等于把窄路线的全部理由作废，\
             而权限画像门也会因为它没登记而红。"
        );
        // 第三向：代码真的按「带 LAUNCHER 的 intent」取材。
        // 取材面是**枚举腿的函数体**，不是整份 Kotlin：整文件 `contains` 会被一份搬进死函数、
        // 或写在别的命令里的同名调用喂饱，而那时 `listInstalledApps` 走的可能是另一条 API。
        let launcher_fn = kotlin_fn_body(&kt, "launcherApps");
        assert!(
            launcher_fn.contains("queryIntentActivities("),
            "声明走 `{ROUTE_QUERIES}`，而枚举腿 launcherApps 里没有 queryIntentActivities —— \
             路线声明与实现对不上"
        );
        assert!(
            launcher_fn.contains("Intent.ACTION_MAIN")
                && launcher_fn.contains("Intent.CATEGORY_LAUNCHER"),
            "queryIntentActivities 喂的不是 MAIN + LAUNCHER 那对 intent —— \
             <queries> 里声明的过滤器与代码问的问题必须是同一个，否则声明放行不了这次查询"
        );
        assert!(
            !kt.contains("getInstalledPackages(") && !kt.contains("getInstalledApplications("),
            "声明走 `{ROUTE_QUERIES}`，代码却用了 getInstalledPackages/getInstalledApplications。\n\
             那两条 API 拿的是**另一个集合**，且 <queries> 的 intent 声明对它们不起作用 —— \
             两边都写着「走 queries」而真机上列表是空的，正是这条断言要抓的形态。"
        );
    } else {
        assert!(
            declares_query_all,
            "声明走 `{ROUTE_QUERY_ALL}`，manifest 里却没有 {QUERY_ALL_PERMISSION}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 门 3：安装器不静默失败
// ═══════════════════════════════════════════════════════════════════════════

/// Kotlin 里 `fun <name>(` 的函数体（含首尾大括号），按大括号配平取；取不到就 panic。
///
/// 取材面必须是**剥过注释**的源码：本仓那几个函数的 KDoc 里写满了 `canRequestPackageInstalls`、
/// `startActivity`、`REASON_*`，不剥就是拿说明书当实现来审。
fn kotlin_fn_body(masked_kt: &str, name: &str) -> String {
    let needle = format!("fun {name}(");
    let at = masked_kt.get_or_panic(&needle);
    let open = masked_kt[at..]
        .find('{')
        .unwrap_or_else(|| panic!("`fun {name}` 后面找不到函数体的左花括号"))
        + at;
    // 按字节扫 `{`/`}`：UTF-8 多字节序列里不会出现 ASCII 字节，故不会误命中，
    // 返回的下标也落在 ASCII 字符上、是合法的切片边界。
    let mut depth = 0usize;
    for (i, byte) in masked_kt.as_bytes().iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return masked_kt[open..=i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("`fun {name}` 的函数体大括号没有配平 —— 取材塌了");
}

/// 某个调用点在这段代码里是不是**取反**的（调用点前紧邻一个 `!`，跳过空白）。
///
/// # 🔴 为什么位置判据必须配一条极性判据
///
/// [`installer_checks_permission_before_handing_the_package_over`] 判的是「授权判据出现在
/// 交付动作之前」。位置对了、方向反了，是同一段代码里另一件事：把
/// `!activity.packageManager.canRequestPackageInstalls()` 的 `!` 删掉，那三条位置/文本断言
/// **逐条仍然成立**（2026-09-06 实测：`cargo test --test android_native_surface_wiring` 12 passed）。
///
/// 而删掉那个 `!` 的运行期后果，正是这道门被委托要消灭的那句话：没授权的用户直接落到
/// `getUriForFile` + `startActivity`，而 `startActivity` 在没授权时**不抛异常** ⇒ Kotlin 返
/// `Handoff(true, null)` ⇒ 界面说「已交系统安装器」而屏幕上什么都没发生；反过来，已经授过权的
/// 用户被弹去「安装未知应用」设置页，永远拿不到安装器。
fn call_site_is_negated(code: &str, call: &str) -> bool {
    let Some(at) = code.find(call) else {
        return false;
    };
    code[..at].trim_end().ends_with('!')
}

/// 🔴 **授权判据必须在交付动作之前跑，且没授权那一支要有可读回报。**
///
/// # 为什么次序是判据本体
///
/// `startActivity` 在没授予「安装未知应用」时**不抛异常**：系统只是不装（或弹一个不说原因的框）。
/// 于是「先交付、失败了再看」这条写法在代码上完全成立，而用户看到的是**什么都没发生**。
/// 唯一能让它自曝的位置是交付**之前**问一次 `canRequestPackageInstalls()`。
///
/// 断言取的是 `handOffToSystemInstaller` 函数体内两个位置的先后：权限判据必须早于
/// `FileProvider.getUriForFile`（交付动作的第一步）。
#[test]
fn installer_checks_permission_before_handing_the_package_over() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let body = kotlin_fn_body(&kt, "handOffToSystemInstaller");

    let gate = body.get_or_panic("canRequestPackageInstalls");
    let handoff = body.get_or_panic("FileProvider.getUriForFile");
    // 🔴 极性先判：位置对而方向反，是一条会让本门全绿的真缺陷（成因见 [`call_site_is_negated`]）。
    assert!(
        call_site_is_negated(&body, "activity.packageManager.canRequestPackageInstalls()"),
        "授权判据没有取反 —— 交付那一支被「已经授权」以外的所有情形走到：\
         没授权的用户拿不到任何回报（startActivity 不抛异常，系统只是不装），\
         已授权的用户反而被弹去设置页。判据在位置上成立、在方向上反了。"
    );
    assert!(
        gate < handoff,
        "「安装未知应用」的授权判据排在交付动作之后 —— 那等于没有判据：\
         startActivity 在没授权时不抛异常，系统只是不装，用户看到的是「点了更新什么都没发生」。"
    );

    // 设置由独立授权命令完成；授权被撤销时交付必须拒绝，不能再次打开设置或继续安装。
    let denied = &body[gate..handoff];
    assert!(
        denied.contains("REASON_UNKNOWN_SOURCES_DENIED")
            && !denied.contains("openUnknownSourcesSettings("),
        "安装前授权已经撤销时必须拒绝，不得隐式再开设置或继续交付"
    );
    assert!(
        denied.contains("Handoff(false,"),
        "没授权那一支没有构造一个带原因的结局"
    );
}

/// 没授权时必须把用户**送到那一页**，且「送不过去」是另一个码。
#[test]
fn unknown_sources_branch_guides_the_user_and_distinguishes_a_dead_end() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let body = kotlin_fn_body(&kt, "openUnknownSourcesSettings");

    assert!(
        body.contains("Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES"),
        "引导腿没有打开「安装未知应用」那一页 —— 可读的引导就是这一跳本身"
    );
    assert!(
        body.contains("startActivityForResult(invoke, intent, \"apkInstallPermissionResult\")"),
        "引导腿没有真的起跳"
    );
    assert!(
        body.contains("package:${activity.packageName}"),
        "授权必须定向到本应用的安装来源设置"
    );
    let request = kotlin_fn_body(&kt, "requestApkInstallPermission");
    assert!(request.contains("REASON_UNKNOWN_SOURCES_NO_SETTINGS"));
    assert!(request.contains("openUnknownSourcesSettings(invoke)"));
    assert!(request.contains("installActivityResumed()"));
    assert!(request.contains("remaining <= 0"));
    assert!(request.contains("apkInstallFlow.beginPermission("));
    assert!(request.contains("apkInstallFlow.expire(invoke,"));
    assert!(!request.contains("handOffToSystemInstaller("));
}

#[test]
fn installation_permission_callbacks_cannot_install_or_accept_expired_receipts() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let complete = kotlin_fn_body(&kt, "completeApkInstallPermission");
    let expiry = complete.get_or_panic("SystemClock.elapsedRealtime() >= pending.deadline");
    let query = complete.get_or_panic("canInstallPackages()");
    assert!(expiry < query, "迟到授权回执必须先拒绝，再查询当前许可");
    assert!(complete.contains("apkInstallFlow.takeReturned(invoke)"));
    assert!(complete.contains("REASON_UNKNOWN_SOURCES_DENIED"));
    for name in [
        "apkInstallPermissionResult",
        "completeApkInstallPermission",
        "onResume",
    ] {
        let body = kotlin_fn_body(&kt, name);
        assert!(!body.contains("handOffToSystemInstaller("));
        assert!(!body.contains("FileProvider.getUriForFile"));
        assert!(!body.contains("startActivity("));
    }
    let destroy = kotlin_fn_body(&kt, "onDestroy");
    assert!(destroy.contains("apkInstallFlow.destroy()?.reject("));
}

#[test]
fn installer_rejects_expired_dispatch_and_system_dialogs_are_mutually_exclusive() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let install = kotlin_fn_body(&kt, "installApk");
    assert!(
        install.get_or_panic("System.currentTimeMillis() >= args.expiresAtEpochMs")
            < install.get_or_panic("handOffToSystemInstaller(")
    );
    assert!(install.contains("pendingVpnPermission != null"));
    assert!(install.contains("apkInstallFlow.beginInstall()"));
    let handoff = kotlin_fn_body(&kt, "handOffToSystemInstaller");
    assert!(
        handoff.get_or_panic("check(System.currentTimeMillis() < expiresAtEpochMs)")
            < handoff.get_or_panic("activity.startActivity(intent)")
    );
    let vpn = kotlin_fn_body(&kt, "requestVpnPermission");
    assert!(
        vpn.get_or_panic("apkInstallFlow.busy()")
            < vpn.get_or_panic("VpnService.prepare(activity)")
    );
    let permission = kotlin_fn_body(&kt, "requestApkInstallPermission");
    assert!(permission.contains("pendingVpnPermission != null"));
}

/// 结局必须一路走到回包：`installApk` 的 `@Command` 体里必须把 `reason` 发出去。
///
/// 这条与上面两条**成对**：那两条证明「Kotlin 算出了原因」，这条证明「原因没有停在 Kotlin」。
/// 缺了它，把 `result.put("reason", …)` 那一行删掉，前两条照样全绿，而 Rust 侧只会收到
/// 一个孤零零的 `handedOff:false` —— 界面又只剩「安装失败」一句话可说。
#[test]
fn the_reason_reaches_the_response() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let body = kotlin_fn_body(&kt, "installApk");
    assert!(
        body.contains("result.put(\"handedOff\", outcome.handedOff)"),
        "installApk 的回包没有发 handedOff"
    );
    assert!(
        body.contains("result.put(\"reason\", outcome.reason)"),
        "installApk 的回包没有把原因发出去 —— 原因停在 Kotlin 里等于没算过"
    );
}

/// 🔴 **没有一个 catch 是空的。**
///
/// 吞异常是「静默失败」最直接的实现方式，而它在 Kotlin 里只要两个字符（`{}`）。
/// 判据：枚举的授权/安装函数体里每一个 `catch (…) {` 都必须有可观测结果 ——
/// 记日志、拒掉这次调用、或返回一个带原因的结局。
///
/// 射程如实登记：本条只覆盖下面枚举的函数，且只按文本判「块体里有没有那几样东西」。
/// 它抓不到「记了日志但吞掉了控制流」（那由上面两条次序/回包断言覆盖），
/// 也抓不到别的文件里的空 catch。
#[test]
fn no_swallowed_exceptions_on_the_install_legs() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));

    let mut checked = 0usize;
    for name in [
        "installApk",
        "listInstalledApps",
        "handOffToSystemInstaller",
        "openUnknownSourcesSettings",
        "requestApkInstallPermission",
        "completeApkInstallPermission",
    ] {
        let body = kotlin_fn_body(&kt, name);
        let mut rest = body.as_str();
        while let Some(at) = rest.find("catch (") {
            let tail = &rest[at..];
            let open = tail
                .find('{')
                .unwrap_or_else(|| panic!("{name} 里的 catch 没有块体"));
            // 从 catch 的块体起按大括号配平，取整块。
            let block = {
                let mut depth = 0usize;
                let mut end = None;
                for (i, byte) in tail.as_bytes().iter().enumerate().skip(open) {
                    match byte {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(i);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                &tail[open..=end.unwrap_or_else(|| panic!("{name} 里的 catch 块没有配平"))]
            };
            assert!(
                block.contains("Log.")
                    || block.contains("invoke.reject")
                    || block.contains("Handoff(")
                    || block.contains("REASON_"),
                "{name} 里有一个把异常吞掉的 catch：{block}\n\
                 吞掉异常 = 用户按了按钮什么都没发生，而日志里也没有任何东西可查。"
            );
            checked += 1;
            rest = &tail[open..];
        }
    }
    // 原安装/枚举三处 + 授权请求两处 + 返回回查一处。不得因拆分授权漏掉错误路径。
    // 而「一个空 catch 都没找到」与「一个 catch 都没找到」在没有这条时不可区分。
    assert!(
        checked >= 6,
        "只审到 {checked} 个 catch（下限 6）—— 取材面塌了，不是真的没有 catch"
    );
}

/// Kotlin 侧声明的 `REASON_*` 码必须**条条有出口**。
///
/// 声明一个码却从不返回它，等于在文档里许诺了一种可分辨的失败而实现里没有 —— 界面按那个码
/// 写的分支永远走不到，而真正发生的事被折进了别的码。
#[test]
fn every_declared_reason_code_has_a_return_path() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let declared: Vec<String> = kt
        .match_indices("const val REASON_")
        .map(|(at, _)| {
            let tail = &kt[at + "const val ".len()..];
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len());
            tail[..end].to_string()
        })
        .collect();
    assert!(
        declared.len() >= 5,
        "只抠到 {} 个 REASON_* 常量（下限 5）—— 抠取口径塌了",
        declared.len()
    );

    let impl_face = format!(
        "{}{}{}{}",
        kotlin_fn_body(&kt, "handOffToSystemInstaller"),
        kotlin_fn_body(&kt, "openUnknownSourcesSettings"),
        kotlin_fn_body(&kt, "requestApkInstallPermission"),
        kotlin_fn_body(&kt, "completeApkInstallPermission"),
    );
    for code in &declared {
        assert!(
            impl_face.contains(code.as_str()),
            "{code} 声明了却没有任何一条实现路径返回它 —— \
             那是一个许诺过、但永远不会发生的可分辨失败"
        );
    }
}

/// Rust 侧必须把 Kotlin 的 `reason` **原样**透传，不折叠、不兜底。
///
/// 与门 3 的 Kotlin 侧三条成对：那三条证明原因被算出来并发出去了，这条证明它在 Rust 侧
/// 没有被 `unwrap_or("failed")` 之类的东西抹平。抹平之后，「按一下开关就能继续」与
/// 「本机根本装不了」在界面上又变成同一句话。
#[test]
fn rust_side_passes_the_reason_through() {
    let rs = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(BRIDGE_RS),
        BRIDGE_RS,
        "pub(crate) async fn hand_apk_to_system_installer",
    ));
    assert!(
        reason_is_forwarded_verbatim(&rs),
        "桥没有把 Kotlin 的 reason **原样**带出来。\n\
         判据是整行恰为 `reason: r.reason,`——在后面接任何东西（`.map(…)` / `.or_else(…)` /\n\
         `.unwrap_or(…)`）都会把 Kotlin 侧五个 REASON_* 码抹成同一个串，\n\
         而这段代码在 `#[cfg(target_os = \"android\")]` 里，本机 cargo test 连类型都不检查它。"
    );
}

/// Kotlin 的 `reason` 在 Rust 侧是不是**整行原样**转出去的。
///
/// # 🔴 为什么是「整行恰等」，不是 `contains`
///
/// 第一版写的是 `rs.contains("reason: r.reason")`。2026-09-06 实测两条合法 Rust 变异
/// （`Option<String>` 进 `Option<String>`，编译得过）在那一版上**全绿**：
///  · `reason: r.reason.map(|_| "failed".to_string()),`
///  · `reason: r.reason.or_else(|| Some("failed".to_string())),`
/// 而这道门的失败信息自己写着「在这里补一个默认值，会让一个本侧从未见过的码静默变成同一句话」
/// —— 补默认值恰恰是它拦不住的那一类。
///
/// 判据因此改成：整份取材面上以 `reason: r.reason` 起笔的行**恰好一条**，且那一行去空白后
/// 恰为 `reason: r.reason,`。折行写法（`reason: r\n    .reason,`）判假，方向是 fail-closed。
fn reason_is_forwarded_verbatim(masked_rs: &str) -> bool {
    let hits: Vec<&str> = masked_rs
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("reason: r.reason"))
        .collect();
    hits.len() == 1 && hits[0] == "reason: r.reason,"
}

/// FileProvider 的授权面登记表：`(标签, name, path)`，**逐条全等**。
///
/// # 🔴 这张表**不是**「APK 落点在不在授权面内」，比那句话宽
///
/// `res/xml/file_paths.xml` 是 Tauri 的 Android 模板带来的，里面除了 APK 交付要用的
/// `<cache-path>`，还有一条 `<external-path name="my_images" path="." />` ——
/// 那把**整个共享外部存储根**也声明进了同一个 provider 的授权面。所以：
///
/// - `FileProvider` 的配置本身**不构成**「只有私有目录的文件才交得出去」这条约束
///   （`/sdcard/Download/x.apk` 落在 `<external-path>` 的覆盖面内，`getUriForFile` 正常返回
///   一个 `content://` URI，不会抛 `IllegalArgumentException`）；
/// - 唯一构成那条约束的是 `PolarisVpnPlugin.handOffToSystemInstaller` 里的
///   `startsWith(cacheRoot)` 前缀判据，由 [`installer_refuses_packages_outside_the_private_cache`] 守。
///
/// 本条登记表守的是**授权面不许悄悄变宽**：往这份文件里加一条 `<root-path path="/" />`
/// （整个文件系统变成可授权面）在「cache-path 在且等于 `.`」那种 membership 判据下是绿的，
/// 而它守的正是同一条边界。故判据是集合恰等。
///
/// `<external-path>` 留着不动：它是模板既有物、本批没有新增外部存储交付面，
/// 而删它属于另一件事的射程（要先证明 Tauri 的文件选择/分享腿不依赖它）。登记在案 ≠ 认可。
const FILE_PROVIDER_PATHS: &[(&str, &str, &str)] = &[
    ("cache-path", "my_cache_images", "."),
    ("external-path", "my_images", "."),
];

/// 从（已剥注释的）`file_paths.xml` 里抠 `<paths>` 的全部子标签：`(标签名, name, path)`。
///
/// 抠不出 `name` / `path` ⇒ panic（判据塌了必须转红，不许静默少取）。
fn file_provider_paths(masked_xml: &str) -> BTreeSet<(String, String, String)> {
    let body_at = masked_xml.get_or_panic("<paths");
    let body = &masked_xml[body_at..];
    let mut out = BTreeSet::new();
    // 逐个 `<` 起笔的标签扫；跳过 `<paths` 自己与 `</…>` 收尾标签。
    let mut rest = &body["<paths".len()..];
    while let Some(at) = rest.find('<') {
        let tail = &rest[at + 1..];
        let end = tail
            .find('>')
            .unwrap_or_else(|| panic!("file_paths.xml 里有标签没闭合：{tail:.60}"));
        let tag = &tail[..end];
        rest = &tail[end..];
        if tag.starts_with('/') || tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        let label = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_string();
        let attr = |key: &str| -> String {
            let needle = format!("{key}=\"");
            let at = tag.find(&needle).unwrap_or_else(|| {
                panic!("file_paths.xml 的 <{label}> 没有 {key} 属性：{tag} —— 取材塌了")
            });
            let value = &tag[at + needle.len()..];
            let close = value
                .find('"')
                .unwrap_or_else(|| panic!("<{label}> 的 {key} 引号没闭合：{tag}"));
            value[..close].to_string()
        };
        let (name, path) = (attr("name"), attr("path"));
        out.insert((label, name, path));
    }
    out
}

/// FileProvider 的授权面必须**逐条全等**登记表，且 APK 落点（应用私有 cache 根）在里面。
///
/// 🔴 **射程**：这条只证明**声明**在，不证明解析成功（那要真机）。`path="."` 是「该根及其
/// 全部子目录」，故 `updates/` 在 cache 那条的覆盖面内 —— 这一句是对 FileProvider 语义的引用，
/// 不是实测。
#[test]
fn file_provider_exposure_matches_the_registry_exactly() {
    let xml = polaris_source_probe::mask_html_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(FILE_PATHS_XML),
        FILE_PATHS_XML,
        "<paths",
    ));
    let declared = file_provider_paths(&xml);
    let registered: BTreeSet<(String, String, String)> = FILE_PROVIDER_PATHS
        .iter()
        .map(|(t, n, p)| ((*t).to_string(), (*n).to_string(), (*p).to_string()))
        .collect();
    assert_eq!(
        declared, registered,
        "FileProvider 的授权面与登记表对不上。\n\
         file_paths.xml 里：{declared:?}\n\
         登记表里：{registered:?}\n\
         多一条 = 授权面被悄悄放宽（`<root-path path=\"/\" />` 就是整个文件系统）；\n\
         少一条 = 某条腿的 getUriForFile 会在真机上抛 IllegalArgumentException。"
    );

    // 正面断言：APK 交付要用的那条**必须**在，且授权的是 cache 根而不是某个子目录。
    // 2026-09-06 实测（M10）：把它收窄成 `thumbnails/` 时，「整份文件里含 path=\".\"」这种
    // membership 判据是绿的 —— 因为 `<external-path>` 也带着一模一样的串。
    assert!(
        registered
            .iter()
            .any(|(tag, _, path)| tag == "cache-path" && path == "."),
        "登记表里没有覆盖 cache 根的 <cache-path> —— APK 落在 cacheDir/updates/ 下，\
         收窄到别的子路径会让它掉出授权面 ⇒ getUriForFile 抛 IllegalArgumentException"
    );

    // 与 Kotlin 侧的 authority 对齐：manifest 声明的是 `${applicationId}.fileprovider`，
    // 代码里拼的必须是同一个后缀，否则 getUriForFile 找不到 provider。
    let manifest = polaris_source_probe::mask_html_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(MANIFEST),
        MANIFEST,
        ".vpn.PolarisVpnService",
    ));
    assert!(
        manifest.contains("${applicationId}.fileprovider"),
        "manifest 里 FileProvider 的 authority 不是 `${{applicationId}}.fileprovider`"
    );
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    assert!(
        kt.contains("\"${activity.packageName}.fileprovider\""),
        "代码里拼的 authority 与 manifest 声明的对不上 —— 这一对失配只在运行到那一行时才炸"
    );
}

/// 🔴 **只有应用私有 cache 目录下的包交得出去** —— 这条前缀判据是唯一的落点约束。
///
/// # 为什么它不能被 FileProvider 的配置替代
///
/// 见 [`FILE_PROVIDER_PATHS`] 的第一段：`file_paths.xml` 的授权面**宽于** APK 落点
/// （`<external-path>` 把整个共享外部存储根也声明了进去），`getUriForFile` 对
/// `/sdcard/...` 不会抛异常。`installApk` 的入参最终来自一次 IPC，接受任意路径 =
/// 把「给系统安装器授哪个文件的读权限」交给调用方决定。
///
/// 射程如实登记：本条只断言那两行判据**在**（前缀比较 + `canonicalFile` 归一化），
/// 断不了它们在真机上的取值。
#[test]
fn installer_refuses_packages_outside_the_private_cache() {
    let kt = polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(PLUGIN_KT),
        PLUGIN_KT,
        "class PolarisVpnPlugin",
    ));
    let body = kotlin_fn_body(&kt, "handOffToSystemInstaller");

    // `canonicalFile` 必须在两端都做：只归一化一侧，`..` 与符号链接的逃逸仍然成立。
    assert!(
        body.contains("File(apkPath).canonicalFile"),
        "入参路径没有 canonicalFile 归一化 —— `..` 逃逸会直接穿过前缀判据"
    );
    assert!(
        body.contains("activity.cacheDir.canonicalFile"),
        "比较的另一端没有 canonicalFile 归一化 —— /data/user/0 与 /data/data 是同一个目录的\
         两个名字，不归一化时合法路径会被误拒（而误拒会逼后人把判据改宽）"
    );
    let guard = body.get_or_panic("startsWith(cacheRoot.path + File.separator)");
    let handoff = body.get_or_panic("FileProvider.getUriForFile");
    assert!(
        guard < handoff,
        "落点判据排在交付动作之后 —— 那等于没有判据"
    );
    // 判据必须**取反**后拒掉：`if (apk.path.startsWith(...))` 少一个 `!` 会把语义整个翻过来。
    assert!(
        call_site_is_negated(
            &body,
            "apk.path.startsWith(cacheRoot.path + File.separator)"
        ),
        "落点判据没有取反 —— 方向反了：私有目录的包被拒、外部路径反而交得出去"
    );
    assert!(
        body[guard..handoff].contains("REASON_NOT_APP_PRIVATE"),
        "落点判据没有给出一个可分辨的原因码 —— 「接线错误」与「用户没授权」在界面上会变成同一句话"
    );
}
