use super::*;
use super::{
    app_update::*,
    core_info::{core_admission_info, core_version_info},
};

use std::sync::Arc;

use crate::test_support::{module_code, repo_file, TestDir};

use polaris_updater::github::{check_app_update, AppUpdateCheck, AssetArch, AssetPlatform};
use polaris_updater::popup::{UpdateErr, UpdateErrCode};
use polaris_updater::state::PopupPhase;
use serde_json::{json, Value};

mod release_pagination;
mod release_selection;

/// 本模块的**生产源码**（调用点守卫共用）。
///
/// 原先是 `concat!(include_str!("shared.rs"), …)` 手列四个文件——那份清单与
/// `src/commands/updater/` 的真实内容是两份事实：往这个模块里新加一个 `.rs`，下面
/// 二十多条调用点守卫会静默漏掉它（门不报错，只是不再看那个文件）。改成按目录取材后，
/// 新增文件自动进扫描面；`module_source` 排除 `tests/`，所以测试代码不会给生产扫描面充数。
fn src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| module_code("commands/updater")).as_str()
}

fn scratch(tag: &str) -> TestDir {
    TestDir::new(&format!("polaris-updater-test-{tag}-"))
}

// ── GitHub releases API 状态码 → 成因 ────────────────────────────────────

/// 🟡 **变异锁：404 = 更新源仓库不可见，且必须仍是错误。**
///
/// 真机成因（2026-07-28 Mac）：`update_check` 恒报「检查更新失败: GitHub API 返回错误: 404」。
/// GitHub 对不存在或当前请求不可见的仓库都以 404 掩盖存在性。裸状态码文案会把这一成因与
/// 「URL 写错 / owner-repo 迁移后仍指向旧地址」混为一谈，因此错误必须保留 owner/repo。
///
/// **变异探针**：删掉 `404 =>` 分支让它落回 `s =>` 兜底 ⇒ 第 2、3 条转红；
/// 把 404 改成返回 `None`（伪装成 2xx ⇒ 前端显示「已是最新」）⇒ 第 1 条 `expect` 转红；
/// 从文案里去掉 `{owner}/{repo}` ⇒ 第 3 条转红。
#[test]
fn github_404_reports_unreachable_repo_not_a_generic_status() {
    let e = github_status_error(404, "polaris-arch", "Polaris")
        .expect("404 必须是错误：更新通道整条不通，绝不能伪装成「无更新 / 已是最新」");
    assert!(e.contains("404"), "状态码要留在文案里供日志比对：{e}");
    assert!(
        e.contains("不存在") && e.contains("私有"),
        "releases **列表**端点上的 404 只意味着仓库不可见（零 release 是 `200 []`），\
             文案必须说准成因而不是甩一个裸状态码：{e}"
    );
    assert!(
        e.contains("polaris-arch/Polaris"),
        "app 腿与 core 腿共用同一个取数函数，不带 owner/repo 的日志分不清是哪条腿挂了：{e}"
    );
}

/// 🟡 **变异锁：2xx 放行 / 403 限流成因保留 / 其余状态码走兜底。**
///
/// 403 与 404 的处置**相反**（前者等一会儿或配加速就好，后者是仓库根本不可见），合并成一条
/// 文案会把用户引去做无用的重试。
///
/// **变异探针**：把 `200..=299` 改窄成 `200` ⇒ 第 2 条转红；删 `403 =>` 分支 ⇒ 第 3 条转红；
/// 把兜底 `s =>` 改成 `None` ⇒ 第 4 条 `expect` 转红。
#[test]
fn github_status_error_truth_table() {
    assert!(github_status_error(200, "o", "r").is_none(), "2xx 放行");
    assert!(
        github_status_error(204, "o", "r").is_none(),
        "放行的是整个 2xx 段（原实现即 `200..300`），不是只有 200"
    );
    let e403 = github_status_error(403, "o", "r").expect("403 必须是错误");
    assert!(
        e403.contains("频率限制"),
        "403 的处置是等/配加速，与 404 的「仓库不可见」不可合并：{e403}"
    );
    let e500 = github_status_error(500, "o", "r").expect("5xx 必须是错误");
    assert!(
        e500.contains("500"),
        "未特殊化的状态码走兜底并保留码值：{e500}"
    );
}

// ── #311：「查看更新日志」直达当前版本 release 页 ──────────────────────────

/// 🟡 **变异锁：有版本号 → 拼 `/releases/tag/v<version>`，且不重复补 `v`。**
///
/// **变异探针**：把 `format!` 里的 `v` 删掉 ⇒ 第 1 条转红；去掉
/// `trim_start_matches('v')` ⇒ 第 2 条转红（拼出 `vv0.1.0`）。
#[test]
fn releases_url_for_version_targets_tag_page() {
    assert_eq!(
        releases_url_for(Some("v0.2.0")),
        "https://github.com/polaris-arch/Polaris/releases/tag/v0.2.0"
    );
    // 调用方将来若改传裸 semver（无 `v`），也不能拼错——两种输入形态幂等。
    assert_eq!(
        releases_url_for(Some("0.2.0")),
        "https://github.com/polaris-arch/Polaris/releases/tag/v0.2.0"
    );
}

/// 🟡 **变异锁：version 为空/仅空白/`None` → 回落泛列表页，绝不拼出可能 404 的链接。**
///
/// **变异探针**：把 `filter(|v| !v.is_empty())` 删掉 ⇒ 第 2、3 条转红
/// （会拼出 `.../tag/v`）。
#[test]
fn releases_url_for_missing_version_falls_back_to_list_page() {
    assert_eq!(
        releases_url_for(None),
        "https://github.com/polaris-arch/Polaris/releases"
    );
    assert_eq!(
        releases_url_for(Some("")),
        "https://github.com/polaris-arch/Polaris/releases"
    );
    assert_eq!(
        releases_url_for(Some("   ")),
        "https://github.com/polaris-arch/Polaris/releases"
    );
}

// ── 内核信息（只读）──────────────────────────────────────────────────────

/// 载荷只有三格，且随包内核自报版本取自原始版本行（保留补丁集后缀）。
#[test]
fn core_version_info_reports_probe_bundled_and_patch_set() {
    let tree = "208f43bb64c3be80ffb10228a426186a7c2cba58";
    assert_eq!(
        core_version_info(
            "sing-box version 1.15.0-alpha.8.polaris.4",
            "1.15.0-alpha.8.polaris.4",
            Some(tree),
        ),
        json!({
            "packagedVersion": "1.15.0-alpha.8.polaris.4",
            "bundledVersion": "1.15.0-alpha.8.polaris.4",
            "patchSet": tree,
        })
    );
}

/// 读不到随包内核版本时那一格是空串，**不**拿清单声明的版本顶上；补丁集缺席是 `null`。
///
/// **变异探针**：把 `core_get_version_info` 的取材换成 `read_core_version()`（探测失败回落
/// 配套版本）⇒ 下面那条调用点守卫转红。
#[test]
fn core_version_info_never_substitutes_the_bundled_version_for_a_failed_probe() {
    let info = core_version_info("", "1.15.0-alpha.8.polaris.4", None);
    assert_eq!(info["packagedVersion"], "");
    assert_eq!(info["bundledVersion"], "1.15.0-alpha.8.polaris.4");
    assert_eq!(info["patchSet"], Value::Null);

    let body = crate::commands::guard_scan::top_level_fn_body(
        &crate::test_support::crate_code("commands/updater/core_info.rs"),
        "pub async fn core_get_version_info(",
    );
    assert!(body.contains("read_core_version_line()"));
    assert!(
        body.contains("spawn_blocking"),
        "读版本要起子进程，不得回到主线程同步执行"
    );
    assert!(
        !body
            .replace("read_core_version_line()", "")
            .contains("read_core_version()"),
        "内核信息卡的随包内核自报版本取自会回落清单声明版本的读法 —— 读不到会被显示成「就是清单声明的版本」"
    );
}

#[test]
fn core_info_retains_static_capability_refusal() {
    let refused = core_admission_info(
        Err(polaris_core_supervisor::admitted_payload::AdmissionError::unsupported_exact_object()),
        || panic!("static refusal must not attempt direct admission"),
    );
    assert_eq!(refused["state"], "rejected");
    assert_eq!(refused["kind"], "UnsupportedExactObject");
    assert_eq!(refused["recovery"], "none");
}

#[test]
fn direct_core_info_retains_actual_admission_result() {
    let refused = json!({"state":"rejected","kind":"Changed","recovery":"reinstallCurrentApp"});
    assert_eq!(core_admission_info(Ok(()), || refused.clone()), refused);
}

// ── 下载单飞（H2）───────────────────────────────────────────────────────

/// 同一个 dest 拿到同一把闸；不同 dest 互不影响（否则两个不相干的下载会互相排队）。
#[test]
fn download_gate_is_keyed_by_destination() {
    let a = std::path::Path::new("/cache/updates/a.dmg");
    let b = std::path::Path::new("/cache/updates/b.dmg");
    assert!(Arc::ptr_eq(&download_gate(a), &download_gate(a)));
    assert!(
        !Arc::ptr_eq(&download_gate(a), &download_gate(b)),
        "不同目标文件必须各有各的闸"
    );
}

/// 单飞表只拥有在途锁：旧目标的最后一个强引用释放后，下次取锁必须驱逐旧键。
#[test]
fn download_gate_map_prunes_finished_destinations() {
    let mut map = DownloadGateMap::new();
    let first_path = std::path::Path::new("/cache/updates/old.dmg");
    let first = keyed_download_gate(&mut map, first_path);
    assert_eq!(map.len(), 1);
    assert!(Arc::ptr_eq(
        &first,
        &keyed_download_gate(&mut map, first_path)
    ));

    drop(first);
    let current_path = std::path::Path::new("/cache/updates/current.dmg");
    let current = keyed_download_gate(&mut map, current_path);
    assert_eq!(map.len(), 1, "已结束目标不应在进程内永久占位");
    assert!(!map.contains_key(first_path));
    assert!(Arc::ptr_eq(
        &current,
        &keyed_download_gate(&mut map, current_path)
    ));
}

/// 🟡 **变异锁：同一个 dest 同时只允许一条下载腿在临界区内。**
///
/// 复现的是默认流程：`autoDownloadUpdate` 的后台下载腿 + 用户在 mini 弹窗点「更新」。
/// **变异探针**：把 `download_gate(...).lock().await` 从 `update_download` 里拿掉
/// （或让 [`download_gate`] 每次返回新 Arc）⇒ 本条转红。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_gate_serialises_concurrent_downloads_of_one_destination() {
    use std::sync::atomic::AtomicUsize;
    let dest = std::path::PathBuf::from("/cache/updates/concurrent.pkg");
    let inside = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (dest, inside, peak) = (dest.clone(), inside.clone(), peak.clone());
        tasks.push(tokio::spawn(async move {
            let gate = download_gate(&dest);
            let _permit = gate.lock().await;
            let n = inside.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            peak.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            inside.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(
        peak.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "同一个 dest 同时有多条下载腿在写 —— 两个进度游标互相顶，且白下一份流量"
    );
}

/// 复用只认 sha256：有摘要且相符才复用；不符 / 无摘要 / 文件不在 → 老老实实重下。
#[test]
fn cached_download_is_reusable_only_with_a_matching_digest() {
    let dir = scratch("reuse");
    let dest = dir.join("update.pkg");
    std::fs::write(&dest, b"package-bytes").unwrap();
    let good = polaris_updater::verify::sha256_hex(b"package-bytes");

    assert!(cached_download_is_reusable(&dest, Some(&good)));
    assert!(
        !cached_download_is_reusable(&dest, Some(&"a".repeat(64))),
        "摘要不符必须重下（磁盘上那份可能是上次被截断的）"
    );
    assert!(
        !cached_download_is_reusable(&dest, None),
        "没有摘要判据就不能声称「磁盘上这份就是你要的包」——不拿文件名当身份"
    );
    assert!(
        !cached_download_is_reusable(&dest, Some("")),
        "空摘要等同没有摘要"
    );
    assert!(!cached_download_is_reusable(
        &dir.join("missing.pkg"),
        Some(&good)
    ));
}

// ── App 更新包：写入体积闸（D4）─────────────────────────────────────────

/// 🟡 **`fileSize` 为 0 / 缺失时必须回落宽松上限，绝不能拿 0 当闸。**
///
/// `AppUpdateInfo.file_size` 在 GitHub asset 缺 `size` 字段时按 **0** 填
/// （`github.rs` 的 `#[serde(default)]`）。直接拿它当闸 ⇒ 闸值 = 0，
/// **任何包都过不去**，且失败长得像「下载超限」，成因（清单少个字段）无从追起。
///
/// **变异探针**：把 `declared.filter(|n| *n > 0)` 改回 `declared` ⇒ 后三条转红。
#[test]
fn app_update_size_limit_falls_back_when_the_declared_size_is_absent_or_zero() {
    // 声明值有效 → 闸就等于声明值（**无裕度**，2026-08-17 删）。
    //
    // 本条原是一对断言：`== declared + MARGIN` 与 `> declared`（「闸不得小于等于声明值本身」）。
    // 随裕度一并订正为**等值**：三处体积闸都是**严格大于**才拒（预检 `n > limit`、两条读侧
    // `已收 + 本 chunk > limit`），故 `limit == declared` 时一个恰好 `declared` 字节的包
    // **仍然过得去**。那条边界不是推断，由 `runtime::http` 的
    // `size_limit_boundary_admits_a_body_of_exactly_the_limit` 逐处拿等长响应体撞过。
    let declared = 40 * 1024 * 1024;
    assert_eq!(
        app_update_size_limit(Some(declared)),
        declared as usize,
        "有声明值时闸 = 声明值本身：等长包靠三处闸的严格大于语义放行，不靠裕度"
    );

    // 声明为 0（GitHub asset 缺 size 的真实形态）→ 回落宽松绝对上限。
    assert_eq!(
        app_update_size_limit(Some(0)),
        APP_UPDATE_MAX_BYTES as usize,
        "fileSize=0 是「清单没给」，不是「包是空的」——拿 0 当闸会拒掉一切"
    );
    // 字段整个缺失 → 同上。
    assert_eq!(app_update_size_limit(None), APP_UPDATE_MAX_BYTES as usize);
    // 回落上限必须容得下真实量级的安装包（几十 MiB）。
    assert!(
        app_update_size_limit(None) > 200 * 1024 * 1024,
        "回落上限卡得比真实安装包还紧 = 换了个姿势拒掉一切"
    );
}

/// 🟡 **声明分支必须有天花板：闸值绝不由服务端单方面顶到任意高。**
///
/// 本条**推翻了**它的前身（`app_update_size_limit_saturates_instead_of_wrapping`）：那条断言
/// `Some(u64::MAX) → usize::MAX`，即**把洞当成期望固化了下来** —— 只防「回绕成一个极小的闸」，
/// 完全不防「闸大到形同不设」。而 `APP_UPDATE_MAX_BYTES` 的文档写的正是「别让一个撒谎的
/// 服务端把盘写满」：`fileSize` 报 100 GiB ⇒ 闸值 100 GiB ⇒ Content-Length 预检放行 ⇒
/// 一路写到 ENOSPC，用户的系统盘被写满。
///
/// **变异探针**（2026-08-17 实测订正，原写「前三条逐条转红」是错的）：去掉
/// `.min(APP_UPDATE_MAX_BYTES)` ⇒ **第 1 条**转红（断言在此中止，故一次运行只看得到这一条）；
/// 单独摘掉第 1、2 条后重跑，**第 3、4 条仍绿** —— 第 3 条的声明值恰好等于天花板，`min`
/// 在那一点是恒等映射，本就落不到差异上；第 4 条在天花板之下同理。
/// 即本条真正的判据是**第 1、2 条**，第 3 条只钉 `min` 的等值分支不越界。
#[test]
fn app_update_size_limit_is_capped_by_the_absolute_ceiling() {
    // 极端声明值（前身守的那一半，保留）。裕度删掉后 `saturating_add` 也一并没了 ⇒
    // 「回绕成极小值」这条失效形态结构性不存在，只剩「顶到天上」要防。
    assert_eq!(
        app_update_size_limit(Some(u64::MAX)),
        APP_UPDATE_MAX_BYTES as usize,
        "u64::MAX 的声明值必须被压回绝对上限"
    );
    // 撒谎的服务端：100 GiB 的声明值必须被压回天花板。
    assert_eq!(
        app_update_size_limit(Some(100 * 1024 * 1024 * 1024)),
        APP_UPDATE_MAX_BYTES as usize,
        "声明值再大也不得把闸顶上去——那正是本闸要防的那件事"
    );
    // 边界：声明值恰好等于天花板 → 就取天花板（`min` 的等值分支）。
    assert_eq!(
        app_update_size_limit(Some(APP_UPDATE_MAX_BYTES)),
        APP_UPDATE_MAX_BYTES as usize
    );
    // 天花板之下的正常量级不受影响（封顶不得把正常包一起卡掉）。
    let normal = 40 * 1024 * 1024;
    assert_eq!(app_update_size_limit(Some(normal)), normal as usize);
}

// ── U2：打包体积门 ↔ 客户端写入闸的一致性（D5）───────────────────────────

/// 打包体积门源码（`scripts/verify-packaging.mjs`），只为**逐字读出它那行常量**。
fn packaging_gate_js() -> &'static str {
    static JS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    JS.get_or_init(|| repo_file("scripts/verify-packaging.mjs"))
        .as_str()
}

/// 打包体积门阈值在本仓的**第二份**（单位 MiB），真值在
/// `scripts/verify-packaging.mjs` 的 `MAX_UPDATE_ASSET_BYTES`。
///
/// D5 拍板的形态是「维护两份 + 一条一致性测试钉死」。两份不是冗余：Rust 侧这一份让本文件能
/// 提出一条 JS 侧**提不出**的断言 —— 打包阈值必须待在 [`APP_UPDATE_MAX_BYTES`] 之下。
///
/// 为什么不反过来「让 JS 去读 Rust 那份、只留一份」：本文件对 [`APP_UPDATE_MAX_BYTES`] 是
/// **按符号引用**的（改名 / 改类型 / 删掉都在编译期红），而 JS 侧只能拿正则去扒 Rust 源码，
/// 那种引用会**静默漂移**（正则扒不着就退化成「没变」）。把跨语言那一跳放在有编译器的这一侧，
/// 漂移面才最小 —— 也正因为如此，下面这个解析函数必须把「扒不准」一律判成失败。
const PACKAGING_MAX_UPDATE_ASSET_MIB: u64 = 200;

/// 从打包门源码里读出 `MAX_UPDATE_ASSET_BYTES = N * 1024 * 1024` 的 `N`。
///
/// 取不到就返回 `None` ⇒ 调用方**转红**。这是刻意的失败方向：判据取不到时若默认「没变」，
/// 一次 JS 侧改写形态（比如换成裸字面量）就会让这条一致性测试**静默失效**，
/// 而它看起来还是绿的 —— 那正是本测试要防的那类事。
///
/// # 「出现多次」同样是不可判定，必须一并判成失败（2026-08-17 复审修）
///
/// 原实现取**首个**匹配（`split_once`），于是「解析失败会红」防不住「解析到错的那一行」：
/// 本仓的注释风格就是大段引用代码 —— 调门时在常量文档里补一行
/// `const MAX_UPDATE_ASSET_BYTES = 96 * 1024 * 1024;  // 旧值` 作沿革记录、同时把真值升到 256，
/// 首个匹配读到注释里那个 96、与镜像相等 ⇒ **绿**，而两份常量已经漂开 160 MiB。
/// 故先数命中数，`!= 1` 直接 `None`：与 `checkSha256Sums` 对同名资产的处置同一条纪律
/// ——**不可判定就必须红，不能挑一个继续**。
///
/// # 数的是「以 marker 开头的**行**」，不是子串出现次数（2026-08-17 二次复审修）
///
/// 只数子串的话，「恰好 1 次、但那 1 次不是真声明」照样会被取值 —— 复审逐字复刻本函数跑了
/// 14 组输入，三组假绿全长一个样：**真声明因换行/改名不再匹配（0 次）+ 注释里那处同形（1 次）
/// = 合计 1 次**。最现实的一种是有人按更窄行宽把常量折成
/// `= \n  256 * 1024 * 1024;`，而文件里某处注释还留着一句一行形态的引用 ⇒ 读到注释里的旧值 96、
/// 与镜像相等 ⇒ 绿，两份实际已漂开 160 MiB。
///
/// 按行判把两侧一起堵上：注释行（`//` / ` * ` 开头）结构上不以 marker 起头，不再参与计数；
/// 真声明一旦折行就 0 命中 ⇒ `None` ⇒ 红（fail-closed，方向正确）。
/// `trim_start_matches("export ")` 是给「将来这行改成 `export const`」留的等价形态，不是通配。
fn packaging_gate_mib_from_js(src: &str) -> Option<u64> {
    const MARKER: &str = "const MAX_UPDATE_ASSET_BYTES = ";
    let hits: Vec<&str> = src
        .lines()
        .map(|l| l.trim_start().trim_start_matches("export "))
        .filter(|l| l.starts_with(MARKER))
        .collect();
    if hits.len() != 1 {
        return None;
    }
    hits[0]
        .strip_prefix(MARKER)?
        .split_once(';')?
        .0
        .trim()
        .strip_suffix(" * 1024 * 1024")?
        .trim()
        .parse()
        .ok()
}

/// 🟡 **解析函数对「扒不准」的三类输入都必须交白卷，而不是猜一个数出来。**
///
/// 上一条测试的全部效力都建立在「`js` 这个数确实是 CI 上拦包的那个数」之上。本条把这个前提
/// 本身钉住，覆盖三类：
///  - **形态变了**（裸字面量 / 改名 / 折行）⇒ `None`，调用方 `expect` 转红；
///  - **真声明出现多次**⇒ `None`（读哪一处不确定）；
///  - **注释里有同形文本**⇒ 不得被当成声明取值 —— 这一类是二次复审逐字复刻跑出来的假绿：
///    真声明折行后 0 命中、注释那处 1 命中，只数子串的实现合计得 1 ⇒ 静默读注释里的旧值。
///
/// **变异探针**（2026-08-17 实测）：把命中判据换回 `src.matches(MARKER).count()` ⇒ 注释组
/// ①②③ **三条各自都会转红**（断言在首条中止，故一次运行只看得到 ①）。三组在旧判据下逐字返回
/// `Some(96)` —— 即注释里那份旧值被当成了真值，而真值其实是 256。
/// 两条正向对照（唯一真声明 / `export` + 缩进）在新旧实现下都返回 `Some(96)`，
/// 故本条不是靠「把什么都判红」换来的。
#[test]
fn packaging_gate_parser_refuses_ambiguous_or_reshaped_sources() {
    // 正向对照：唯一一处、形态如约 ⇒ 读得出（否则下面几条「读不出」毫无信息量）。
    assert_eq!(
        packaging_gate_mib_from_js("const MAX_UPDATE_ASSET_BYTES = 96 * 1024 * 1024;\n"),
        Some(96)
    );
    // 形态变了 ⇒ 交白卷。
    assert_eq!(
        packaging_gate_mib_from_js("const MAX_UPDATE_ASSET_BYTES = 100663296;\n"),
        None,
        "裸字面量读不出，必须让调用方红"
    );
    assert_eq!(
        packaging_gate_mib_from_js("const MAX_UPDATE_ASSET_LIMIT = 96 * 1024 * 1024;\n"),
        None,
        "常量改名 = 判据消失"
    );
    // 真声明出现两次 ⇒ 读哪一处不确定，交白卷。
    assert_eq!(
        packaging_gate_mib_from_js(concat!(
            "const MAX_UPDATE_ASSET_BYTES = 96 * 1024 * 1024;\n",
            "const MAX_UPDATE_ASSET_BYTES = 256 * 1024 * 1024;\n"
        )),
        None,
        "两条真声明 ⇒ 不可判定；挑首个会把「已漂开」判成绿"
    );
    // ── 注释里的同形文本不得被当成声明（只数子串的实现在这三组上全是假绿）──
    // ① 真声明折行 ⇒ 应 0 命中；注释那处不参与计数 ⇒ 合计 0 ⇒ None。
    assert_eq!(
        packaging_gate_mib_from_js(concat!(
            "// 沿革：const MAX_UPDATE_ASSET_BYTES = 96 * 1024 * 1024;（旧值）\n",
            "const MAX_UPDATE_ASSET_BYTES =\n  256 * 1024 * 1024;\n"
        )),
        None,
        "真声明折行 ⇒ 判据取不到，绝不能回落去读注释里的旧值"
    );
    // ② 真声明改名 + 注释留旧形态。
    assert_eq!(
        packaging_gate_mib_from_js(concat!(
            " * 旧写法：const MAX_UPDATE_ASSET_BYTES = 96 * 1024 * 1024;\n",
            "const MAX_UPDATE_ASSET_LIMIT = 256 * 1024 * 1024;\n"
        )),
        None,
        "块注释里的同形行同样不得顶替判据"
    );
    // ③ 真声明换裸字面量并改名 + 注释留旧形态。
    assert_eq!(
        packaging_gate_mib_from_js(concat!(
            "// const MAX_UPDATE_ASSET_BYTES = 96 * 1024 * 1024;\n",
            "const ASSET_CEILING = 268435456;\n"
        )),
        None,
        "注释掉的旧行 + 换形态的新声明 ⇒ 必须交白卷"
    );
    // 反向对照：真声明前有缩进 / 将来改成 `export const` 仍读得出（本条不是「越严越好」，
    // 把等价形态一并判红会让门在无害改动上假红）。
    assert_eq!(
        packaging_gate_mib_from_js("  export const MAX_UPDATE_ASSET_BYTES = 96 * 1024 * 1024;\n"),
        Some(96)
    );
}

/// 🟡 **打包体积门的两份常量不得漂开，且它必须待在客户端绝对写入闸之下。**
///
/// 两条断言各防一件事：
///  - **漂开**：只改 JS 那一份 ⇒ 本条转红。没有它，「两份」就是两个各说各话的数；
///  - **越顶**：把打包门调到 [`APP_UPDATE_MAX_BYTES`] 之上 ⇒ 本条转红。那种配置下打包门会
///    放行一个**客户端结构性下不动**的包（写入闸在 512 MiB 处拒，用户侧表现为更新永远失败），
///    等于把 U1 那个缺陷换了个数量级重来一遍。
///
/// 还有一条**下界**：门不得低于真实产物量级（实测最大 122.58 MiB，linux AppImage，
/// run 32109475236；2026-08-18 二次定标前是 51.72 的 mac dmg），否则每次 linux 发布都假红
/// —— 一道恒红的门与一道没有的门，信息量一样是零。下界取 128：大于实测最大值的最小
/// 二进制整数档，给「AppImage 以后瘦身、想跟着降门」留一步余量，但降破它必须连同实测依据一起改。
///
/// **变异探针**：把 `MAX_UPDATE_ASSET_BYTES` 改成 `128 * 1024 * 1024` 而不动这里 ⇒ 第 1 条转红；
/// 改成 `1024 * 1024 * 1024` 并同步改这里 ⇒ 第 2 条转红；同步改成 `32` ⇒ 第 3 条转红。
#[test]
fn packaging_size_gate_is_mirrored_and_stays_under_the_client_write_gate() {
    let js = packaging_gate_mib_from_js(packaging_gate_js()).expect(
        "读不出 verify-packaging.mjs 的 MAX_UPDATE_ASSET_BYTES —— \
             判据取不到时必须红，不能默认「没变」（形态要求：`N * 1024 * 1024`）",
    );
    assert_eq!(
            js, PACKAGING_MAX_UPDATE_ASSET_MIB,
            "打包体积门的两份常量漂开了：JS 侧 {js} MiB / 本文件 {PACKAGING_MAX_UPDATE_ASSET_MIB} MiB。\
             D5 的形态是「维护两份 + 本条钉死」，改一处必须同步改另一处"
        );
    // 下面两条一律拿**从 JS 读出来的那个数**去判，不拿本文件的镜像：真正在 CI 上拦包的是
    // JS 那一份，镜像只是用来发现漂开。拿镜像判等于自己判自己（clippy 也会直接指出这是
    // 一条常量断言）。
    assert!(
        js * 1024 * 1024 < APP_UPDATE_MAX_BYTES,
        "打包门（{js} MiB）越过了客户端绝对写入闸（{} MiB）：\
             那样打包门会放行一个客户端根本下不动的包，用户侧表现为更新永远失败",
        APP_UPDATE_MAX_BYTES / 1024 / 1024
    );
    assert!(
        js >= 128,
        "打包门（{js} MiB）卡到了真实产物量级（实测最大 122.58 MiB，linux AppImage）附近：\
             那会让每次发布都假红，与没有这道门等价"
    );
}

// ── App 更新包：期望摘要的来源（D1）─────────────────────────────────────

/// 摘要来源按优先级挑；全无 → `Ok(None)`（**不拒装**，降级为弱校验并如实标记未校验）。
#[test]
fn expected_digest_is_resolved_by_source_priority_and_degrades_honestly() {
    let sha = "a".repeat(64);
    let got = resolve_expected_digest(&json!({ "sha256": sha }))
        .expect("合法字符串不该报错")
        .expect("应挑出 asset digest");
    assert_eq!(got.hex, sha);
    assert_eq!(got.source, DigestSource::GithubAssetDigest);
    assert_eq!(got.source.as_str(), "githubAssetDigest");

    // 无摘要 / 空串 / 纯空白 → Ok(None)（旧 release 的真实形态：`AppUpdateInfo.sha256` 带
    // `skip_serializing_if = "Option::is_none"`，故缺摘要时**字段整个不出现**）。
    assert_eq!(resolve_expected_digest(&json!({})), Ok(None));
    assert_eq!(resolve_expected_digest(&json!({ "sha256": "" })), Ok(None));
    assert_eq!(
        resolve_expected_digest(&json!({ "sha256": "   " })),
        Ok(None)
    );

    // 格式非法的摘要**不得**在此静默丢弃 —— 那会把「摘要写坏了」伪装成「本来就没摘要」而放行。
    // 它要一路走到校验步，才分得出「发布方写坏了」与「包被篡改」两种文案。
    let bad = resolve_expected_digest(&json!({ "sha256": "not-a-hash" }))
        .expect("非法 hex 是字符串，不在本函数报错")
        .expect("格式非法也要挑出来，交给校验步报错");
    assert_eq!(bad.hex, "not-a-hash");
}

/// 🟡 **字段在、但不是字符串 ⇒ 显式早退，绝不静默降级成「本来就没摘要」。**
///
/// 原实现三种形态（`123` / `null` / `["…"]`）全走 `Value::as_str` → `None` → 与「字段缺失」
/// 合流 → `verified:false` 放行。即一个把 `sha256` 序列化成数字的发布流程，会让**全体用户**
/// 静默地少一道校验，而返回体上只显示 `verified:false`（长得和旧 release 一模一样）。
/// 这与 [`resolve_expected_digest`] 自己文档写的「静默丢弃会把『摘要写坏了』伪装成
/// 『本来就没摘要』而放行」正相反。
///
/// **变异探针**：把非字符串分支改回 `continue`（或退回 `and_then(Value::as_str)`）⇒ 三条转红。
#[test]
fn a_non_string_digest_field_is_rejected_not_silently_dropped() {
    for bad in [json!(123), json!(null), json!(["a".repeat(64)]), json!({})] {
        let err = resolve_expected_digest(&json!({ "sha256": bad }))
            .expect_err("非字符串的 sha256 必须显式早退");
        assert!(err.contains("sha256"), "错误必须点名是哪个字段坏了：{err}");
    }
    // 反向对照：字段**整个缺失**仍是合法的「本来就没摘要」，不得被一起拒掉
    // （否则所有旧 release 立刻更新不了）。
    assert_eq!(resolve_expected_digest(&json!({ "other": 1 })), Ok(None));
}

/// 🟡 **清单声明的 `fileSize` 是等值判据（无摘要腿的主防线）。**
///
/// 无摘要腿此前**只有** Content-Length 兜底，而那个数是撒谎方自己给的：服务端/镜像返
/// `Content-Length: 1000` 且真发 1000 字节 ⇒ 完整性校验过 ⇒ 无摘要不校验 ⇒ 1000 字节的
/// 假包被 promote，返 `{success:true, verified:false}`，UI 给出安装入口。
/// 极端版 `Content-Length: 0` ⇒ 0 字节文件「下载成功」。
///
/// `fileSize` 来自 GitHub release 清单，镜像改不动 —— 零成本堵上这条。
///
/// **变异探针**：把 [`check_declared_size`] 改成恒 `Ok(())` ⇒ 第 2、3 条转红；
/// 去掉 `.filter(|n| *n > 0)` ⇒ 第 4 条转红（缺 size 的旧 release 会被全部拒掉）。
#[test]
fn declared_file_size_is_enforced_as_an_equality_criterion() {
    use polaris_updater::traits::DownloadError;

    // 声明值匹配 → 放行。
    assert!(check_declared_size(1000, Some(1000)).is_ok());

    // 不匹配 → Incomplete（结构化，带两个数）。收得少（截断）与收得多（掉包/注入）都算。
    assert!(matches!(
        check_declared_size(1000, Some(52_000_000)),
        Err(DownloadError::Incomplete {
            received: 1000,
            expected: 52_000_000
        })
    ));
    assert!(
        check_declared_size(0, Some(52_000_000)).is_err(),
        "`Content-Length: 0` + 真发 0 字节的假包必须被这一级拦下"
    );
    assert!(check_declared_size(52_000_001, Some(52_000_000)).is_err());

    // 声明为 0 / 缺失 = 「清单没给」，**不判**（旧 release 的正常形态；判了就全拒）。
    assert!(check_declared_size(1000, Some(0)).is_ok());
    assert!(check_declared_size(1000, None).is_ok());
}

// ── 随行事实：态与它依赖的数据同帧同行 ──────────────────────────────────

/// 🟡 **行为门：每一帧都带着它那个态所依赖的全部随行事实。**
///
/// `update:progress` 走 `events::broadcast` fan-out 给**所有**窗口 ⇒ 把设置页推进
/// downloading / downloaded / error 的路径大多**不是设置页发起的**（启动自动下载腿
/// `startup_tasks::spawn_auto_download`、弹窗「更新·重试」腿 `update_popup_action`），
/// 那几条腿上设置页拿不到任何 invoke 回包 ⇒ 这一帧是它**唯一**的事实来源。少一样就静默
/// 少一样：已核实的三条后果是「重启并安装」哑键（无 `filePath`）、「重试」哑键
/// （无 `updateInfo`）、卡片上的版本号与体积写的是上一次检查的另一个版本。
///
/// # 判据由**类型**穷尽，不由夹具点名
///
/// 「哪个变体该带哪些键」写在 `required` 的穷尽 `match` 里 ⇒ [`ProgressStage`] 新增变体
/// **编译不过**，作者必须回到这里显式回答它带得起哪些事实。
///
/// 「样本有没有覆盖到每个变体」则由**从源码派生的臂数**兜底，不是手写常数：前身写
/// `assert_eq!(samples.len(), VARIANTS)`（两个手写数字互比），2026-08-17 复审实测 ——
/// 加第 4 变体 + 补齐两处生产 match + **只**补一条 `required` 臂、不动样本 ⇒ 全量 4181 全绿，
/// 那个变体的载荷一次都没跑过。现改为数 [`stage_facts`] 那个**编译器强制穷尽**的 match
/// 有几条臂，臂数即变体数，两个数字都不再由人手维护。
/// 每格还反向断言「白名单之外不得夹带」——两个方向合起来是**逐变体的集合相等**，
/// 而不是「至少有这几个键」这种只挡得住删除的弱判据。
///
/// 跨语言那一半（Rust 键集 ↔ TS `UpdateProgress` 字段集）由
/// `ui/src/contracts/update-progress-payload.test.ts` 双向对拍，与本门正交。
///
/// **变异探针**：删掉 `payload["filePath"] = …` ⇒ Downloaded 那格转红；把
/// `"updateInfo": info` 换成 `Value::Null` ⇒ 三格同时转红；把 `Downloaded` 的百分比从
/// 100 改成别的 ⇒ 转红；多写一个键 ⇒ 「夹带」那条转红。
#[test]
fn progress_frame_carries_the_facts_its_state_depends_on() {
    /// 每个变体的帧里**必须**存在、且不得多于此的键（穷尽 match ⇒ 新增变体即编译错误）。
    ///
    /// 逐字段解构（不写 `..`）：给已有变体加字段时这里也必须表态，否则「加变体被挡住了、
    /// 加字段没有」那个不对称会从生产代码搬到门里来。
    const fn required(stage: ProgressStage<'_>) -> &'static [&'static str] {
        match stage {
            ProgressStage::Downloading {
                percentage: _,
                received: _,
            } => &["status", "percentage", "updateInfo", "receivedBytes"],
            ProgressStage::Downloaded {
                path: _,
                verified: _,
            } => &["status", "percentage", "updateInfo", "filePath", "verified"],
            ProgressStage::Failed { .. } => &[
                "status",
                "percentage",
                "errorCode",
                "errorDetail",
                "updateInfo",
            ],
        }
    }

    /// [`ProgressStage`] 的变体数，**从源码派生**：[`stage_facts`] 的 match 由编译器强制
    /// 穷尽 ⇒ 它有几条臂就有几个变体。数 `ProgressStage::` 而不数 `=>`：签名里的
    /// `ProgressStage<'a>` 不含冒号，不会被误计。
    ///
    /// 射程边界（如实登记，两条）：
    ///  1. 臂头若写成 or-pattern（`A::X { .. } | A::Y { .. } =>`），本计数会**高于**臂数
    ///     ⇒ 断言转红（安全方向：宁可误红也不放过没样本的变体）。
    ///  2. **给 `stage_facts` 与 `required` 同时补一条 `_ => …` 通配臂时，本门静默失效**：
    ///     计数停在通配那一刻的臂数，而 `match` 也不再被编译器强制穷尽 ⇒ 新变体既不编译红、
    ///     也不被本门抓到。今天两处都是穷举、无通配臂，故不可达；谁要加通配臂，请连同本门
    ///     一起重新设计判据（通配臂本身就是「新变体不必表态」的宣言）。
    fn variant_count() -> usize {
        // 锚取无泛型的前缀（U1 起 stage_facts 不再带生命周期参数；锚写死旧签名会在
        // 「锚消失」时掉进下方测试模块的字面量自匹配——本函数自己的锚串就在那）。
        let body = crate::commands::guard_scan::top_level_fn_body(src(), "const fn stage_facts(");
        let n = body.matches("ProgressStage::").count();
        assert!(
                n >= 3,
                "`stage_facts` 的臂形状变了（一条 ProgressStage:: 臂都没解析到）—— 本门已失去变体数的真值源"
            );
        n
    }

    let path = std::path::Path::new("/tmp/updates/polaris.dmg");
    let info = json!({
        "version": "v1.2.0",
        "fileSize": 52_000_000_u64,
        "isPrerelease": true,
        // 两个**必须被剥掉**的字段：样本里没有它们，下面那条「剥掉了」的断言就无信息量。
        "releaseNotes": "## v1.2.0\n- 大段更新说明……",
        "title": "Polaris v1.2.0",
        // 契约将来加字段时，这一格证明其余字段是**原样**带过去的，不是被逐字段抄了一遍。
        "futureField": "kept",
    });
    // 期望的投影：从样本清单里**逐个删掉**登记为剥除的键，其余一个不动。
    // 表驱动 ⇒ 剥除表加一项时，本断言的两个方向（该没的没了、该在的逐字还在）自动跟着长。
    let expected_manifest = {
        let mut m = info.as_object().expect("样本清单必须是对象").clone();
        for key in PROGRESS_MANIFEST_OMITTED {
            assert!(
                m.remove(key).is_some(),
                "样本清单里没有 `{key}` ⇒ 「它被剥掉了」这条断言无信息量（假绿）"
            );
        }
        Value::Object(m)
    };

    let samples = [
        ProgressStage::Downloading {
            percentage: 37,
            received: 19_240_000,
        },
        ProgressStage::Downloaded {
            path,
            verified: true,
        },
        ProgressStage::Failed(UpdateErr::with_detail(
            UpdateErrCode::DownloadFailed,
            "net down",
        )),
    ];
    let variants = variant_count();
    assert_eq!(
        samples.len(),
        variants,
        "样本表没跟上变体数（`stage_facts` 有 {variants} 条臂）—— 新增的那个变体的载荷一次都没跑过"
    );
    let tags: std::collections::BTreeSet<&str> =
        samples.iter().map(|s| stage_facts(*s).0).collect();
    assert_eq!(
        tags.len(),
        samples.len(),
        "样本里有重复变体 —— 有一格根本没被测到"
    );

    for stage in samples {
        let (status, percentage) = stage_facts(stage);
        let payload = progress_payload(&info, stage);
        let obj = payload.as_object().expect("载荷必须是 JSON 对象");
        for key in required(stage) {
            assert!(obj.contains_key(*key), "{status} 帧缺随行事实 `{key}`");
            assert!(!obj[*key].is_null(), "{status} 帧的 `{key}` 是 null");
        }
        let extra: Vec<&String> = obj
            .keys()
            .filter(|k| !required(stage).contains(&k.as_str()))
            .collect();
        assert!(extra.is_empty(), "{status} 帧夹带了未登记的键: {extra:?}");
        assert_eq!(obj["status"], json!(status));
        assert_eq!(obj["percentage"], json!(percentage));
        // 「剥掉的真没了」——先单独判，失败消息才指得出是哪个字段。
        for key in PROGRESS_MANIFEST_OMITTED {
            assert!(
                obj["updateInfo"].get(key).is_none(),
                "{status} 帧仍带着 `{key}` —— 它无上限且 progress 可达态一律不渲染，\
                     每帧广播给所有窗口是纯粹的主线程开销"
            );
        }
        // 「该在的逐字还在」——**整对象相等**，不是「有几个字段」：少带一个字段就是
        // 「重试」重新变哑键 / 卡片又开始显示别的版本，正是本批立项要修的那三条。
        assert_eq!(
            obj["updateInfo"], expected_manifest,
            "{status} 帧的清单投影与「原样减去剥除表」不符 —— 卡片会拿它渲染版本号/体积/档次，\
                 「重试」也要拿它重下"
        );
    }

    // 逐值对账（上面只管「在不在」，这里管「是不是那个数」）。
    let landed = progress_payload(
        &info,
        ProgressStage::Downloaded {
            path,
            verified: false,
        },
    );
    assert_eq!(landed["filePath"], json!(path.to_string_lossy()));
    assert_eq!(landed["verified"], json!(false));
    assert_eq!(landed["percentage"], json!(100), "落位帧的百分比由类型定死");
    let mid = progress_payload(
        &info,
        ProgressStage::Downloading {
            percentage: 37,
            received: 19_240_000,
        },
    );
    assert_eq!(
        mid["receivedBytes"],
        json!(19_240_000),
        "已收字节必须是回调原值 —— 从百分比反推的数每一帧都是错的"
    );
    let failed = progress_payload(
        &info,
        ProgressStage::Failed(UpdateErr::with_detail(
            UpdateErrCode::DownloadFailed,
            "net down",
        )),
    );
    assert_eq!(failed["errorCode"], json!("downloadFailed"));
    assert_eq!(failed["errorDetail"], json!("net down"));
    // 无细节的那发不夹带 errorDetail（None 不进载荷，前端按可选处理）。
    let bare = progress_payload(
        &info,
        ProgressStage::Failed(UpdateErr::new(UpdateErrCode::DownloadFailed)),
    );
    assert!(bare.get("errorDetail").is_none());
    assert_eq!(failed["percentage"], json!(0), "失败帧的百分比由类型定死");
}

// ── 残件所有权：RAII（H1）───────────────────────────────────────────────

/// 🟡 **运行时门：drop 即删；`disarm` 之后 drop 什么都不删。**
///
/// 这是把「失败即清理」从**源码计数**换成**类型**之后，真正有牙的那条门 ——
/// 它测的是行为（离开作用域时文件在不在），不是「代码里写没写那一行」。
///
/// **变异探针**：删掉 `impl Drop for PartialDownload` ⇒ 第 1 条转红；
/// 把 `disarm` 改成不置 `None`（或直接删掉这个方法体的赋值）⇒ 第 2 条转红
/// （落位成功后会把刚下好的包删掉）。
#[test]
fn partial_download_deletes_the_tmp_on_drop_and_keeps_it_after_disarm() {
    let dir = scratch("partial-raii");

    // ① 未解除 → drop 即删（= 每一条失败早退）。
    let armed = dir.join("armed.pkg.polaris-new-1-0");
    std::fs::write(&armed, b"half-written").unwrap();
    {
        let guard = PartialDownload::new(armed.clone());
        assert_eq!(guard.path(), armed, "守卫必须原样交出它持有的那条路径");
        assert!(armed.is_file(), "守卫存活期间不得提前删");
    }
    assert!(
        !armed.exists(),
        "守卫 drop 后残件必须消失 —— 否则每次下载失败都在缓存目录里攒一个几十 MiB 的垃圾"
    );

    // ② 已解除 → drop 什么都不做（= 落位成功，那个 inode 现在叫 dest 了）。
    let landed = dir.join("landed.pkg");
    std::fs::write(&landed, b"complete").unwrap();
    PartialDownload::new(landed.clone()).disarm();
    assert!(
        landed.is_file(),
        "disarm 之后 drop 绝不能删 —— 那删掉的是刚落位成功的更新包本身"
    );
    assert_eq!(std::fs::read(&landed).unwrap(), b"complete");

    // ③ 文件本来就不存在（「还没来得及建就失败」）→ drop 不得 panic。
    drop(PartialDownload::new(dir.join("never-created")));
}

// ── 落位：可注入的运行时判据（H2）──────────────────────────────────────

/// 🟡 **运行时门：rename 失败 ⇒ `Failed` 且 dest 不存在；成功 ⇒ `Landed` 且 tmp 消失。**
///
/// 这条替掉的是一条**表达不了自己声称之事**的文本守卫（`promote_at < downloaded_at` 的下标
/// 比较）：把落位失败的早退降级成「只 log 不早退」，文本序照样成立，而运行时会广播
/// `downloaded(100)` 外加一个根本不存在的 `filePath`。
///
/// [`land_payload`] 吃 `&dyn UpdateFs` ⇒ 可注入 `MockFailOp::Rename`，
/// 于是「rename 成功才算落位」变成一条真跑得起来的断言。
///
/// **变异探针**：把 [`land_payload`] 的 `Err(e) =>` 改成 `Ok(())` 一样返回
/// `LandingOutcome::Landed`（即「只 log 不早退」那个降级）⇒ 第 1 条转红。
#[test]
fn landing_reports_failure_and_leaves_dest_untouched_when_rename_fails() {
    use polaris_updater::traits::{MockFailOp, MockFs, StdFs};

    let dir = scratch("landing");

    // ① rename 失败 → Failed(_)，且 dest **不存在**（绝不广播一个不存在的 filePath）。
    let dest_fail = dir.join("fail.pkg");
    let tmp_fail = polaris_updater::verify::tmp_name(&dest_fail);
    std::fs::write(&tmp_fail, b"streamed").unwrap();
    let mut fs = MockFs::new(&dir);
    fs.fail_next(MockFailOp::Rename);
    let outcome = land_payload(&fs, &tmp_fail, &dest_fail);
    assert!(
        matches!(outcome, LandingOutcome::Failed(_)),
        "rename 失败必须报 Failed，实得 {outcome:?}"
    );
    assert!(
        !dest_fail.exists(),
        "落位失败时 dest 必须仍不存在 —— 否则 `update_install` 会拿一个半截文件去装"
    );
    if let LandingOutcome::Failed(msg) = outcome {
        assert!(
            msg.contains("rename to"),
            "失败文案须与同文件其它早退一致：{msg}"
        );
    }

    // ② 成功 → Landed，dest 是完整内容，tmp 消失。
    let dest_ok = dir.join("ok.pkg");
    let tmp_ok = polaris_updater::verify::tmp_name(&dest_ok);
    std::fs::write(&tmp_ok, b"streamed-bytes").unwrap();
    assert_eq!(
        land_payload(&StdFs, &tmp_ok, &dest_ok),
        LandingOutcome::Landed
    );
    assert_eq!(std::fs::read(&dest_ok).unwrap(), b"streamed-bytes");
    assert!(!tmp_ok.exists(), "落位成功后 tmp 必须已被 rename 掉");
}

/// 🟡 **运行时门：rename **之前**必须先 `fsync`；刷盘失败 ⇒ `Failed` 且 dest 不存在。**
///
/// 没有这一步时，rename 只保证目录项的原子替换，不保证那个 inode 的**数据**已离开 page
/// cache ⇒ 断电/内核崩溃后可能出现「dest 名字在、内容是零或半截」，而 dest 一旦存在
/// [`update_install`] 就会**直接拿去装**。
///
/// **变异探针**（2026-08-17 逐条实测，两条各红一处、互不重叠）：
///  - 删掉 `land_payload` 里的 `sync_file` 调用（或把失败改成「只 log 不早退」）
///    ⇒ **第 1 条**转红（注入的刷盘失败无处可发，结论成了 `Landed`）；
///  - 把 `sync_file` 挪到 `promote_staged` **之后** ⇒ **第 2 条**转红（结论仍是 `Failed`，
///    但 rename 已经发生 ⇒ dest 已存在）。顺序因此也在射程内，不是只靠注释声明。
///
/// **不是探针、是运行期后果**（本仓 CI 只跑 Linux/macOS/Windows 的 64 位构建，但**没有**
/// 任何门会因它转红，故不列在上面）：把
/// [`StdFs::sync_file`](polaris_updater::traits::StdFs) 的 `OpenOptions::write(true)`
/// 改成 `File::open`（只读句柄），Linux 上照常绿，**Windows 上落位会全线失败** ——
/// `FlushFileBuffers` 对只读句柄直接报错。这条只能靠 trait 文档里的那句约束守住。
#[test]
fn landing_fsyncs_the_payload_before_renaming_it_into_place() {
    use polaris_updater::traits::{MockFailOp, MockFs};

    let dir = scratch("landing-fsync");
    let dest = dir.join("synced.pkg");
    let tmp = polaris_updater::verify::tmp_name(&dest);
    std::fs::write(&tmp, b"streamed").unwrap();

    let mut fs = MockFs::new(&dir);
    fs.fail_next(MockFailOp::SyncFile);
    // 残件的清理归 RAII 守卫（`land_payload` 自己不删）——照生产的形态过一遍：
    // 失败分支**不** disarm，守卫在离开作用域时把 tmp 收掉。
    let outcome = {
        let partial = PartialDownload::new(tmp.clone());
        land_payload(&fs, partial.path(), &dest)
    };

    assert!(
        matches!(outcome, LandingOutcome::Failed(_)),
        "刷盘失败必须报 Failed（内容可能还在 page cache 里），实得 {outcome:?}"
    );
    assert!(
        !dest.exists(),
        "刷盘失败时绝不能已经 rename —— dest 一旦存在就会被 `update_install` 当成完整包"
    );
    assert!(
        !tmp.exists(),
        "失败路径上的残件必须由 RAII 守卫收掉（`land_payload` 不负责删）"
    );
}

// ── 孤儿 tmp 清扫（H3）──────────────────────────────────────────────────

/// 把文件的 mtime 往前拨 `age`（测跨进程残件的 24h 阈值；不引第三方 crate）。
fn backdate(path: &std::path::Path, age: std::time::Duration) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() - age).unwrap();
}

/// 🟡 **运行时门：本资产 + 本 pid 的孤儿直接收；其余一律只按 mtime 阈值收；无关文件不碰。**
///
/// 主触发器是「下载途中退出 App」：`update_download` 是 async command，tmp 建立后唯一的
/// await 点是 `spawn_blocking(...).await`，退出 App 会让 runtime **drop 掉那个 future** ⇒
/// RAII 守卫连同三条早退全被绕过，而 blocking 线程不可取消、可能把 tmp 写完。开着
/// `autoDownloadUpdate` 时每个「启动 → 后台下载 → 提前关 App」周期必留一个几十 MiB 的孤儿，
/// 而唯一的回收点是完全卸载。
///
/// # 第 ⑤ 条已**反向**（2026-08-17）
///
/// 它原先钉的是「别的资产的残件不归本次清扫管」—— 而那正是缺陷本身：App 更新的资产名带版本号，
/// 版本一换前缀就变，于是上面那个主触发器留下的残件（必然是**旧版本名**）永远收不回来。
/// 现在钉的是「别资产残件：新鲜保留、陈旧收」，并由第 ⑥ 条做真实缺陷回放。
///
/// **变异探针**（2026-08-17 逐条实测，末条原写「去掉它 ⇒ 第 4 条转红」是错的）：
/// 把本资产+本 pid 那一档也改成走 mtime 阈值 ⇒ 第 1 条转红；
/// 把匹配面缩回 `{file_name}.polaris-new-` ⇒ 第 6 条转红（旧版本名的陈旧残件又收不回来了）；
/// 把即时档改成只看 pid、不看资产名 ⇒ 第 5 条转红；
/// 把 `>= ORPHAN_TMP_MAX_AGE` 改成 `<` ⇒ 第 2、3、5、6 条同时转红。
///
/// `is_orphan_tmp_name` 无论**放宽成「只看中缀」还是整个去掉**，转红的都是**第 7 条**，
/// 不是第 4 条：第 4 条的 `dest` 夹具是新鲜写的，判据没了它只是落进 mtime 档 → 保留 → 仍绿。
/// 真正被这条判据护住的是「陈旧的非 tmp 文件」，而夹具里那个正是 `not_a_tmp`。
#[test]
fn orphan_sweep_collects_only_what_it_can_prove_is_abandoned() {
    use polaris_updater::traits::StdFs;

    let dir = scratch("orphan-sweep");
    let file_name = "polaris-0.2.0.dmg";
    let pid = std::process::id();

    // ① 本资产 + 本进程留下的残件：调用点在单飞闸内 ⇒ 此刻绝无第二条腿在写它 ⇒ 直接收（不看 mtime）。
    let mine = dir.join(format!("{file_name}.polaris-new-{pid}-7"));
    // ② 其它进程 + 陈旧（> 24h）：跨进程兜底，收。
    let stale = dir.join(format!("{file_name}.polaris-new-{}-0", pid.wrapping_add(1)));
    // ③ 其它进程 + 新鲜：可能正有另一个实例在写 ⇒ **不收**（失败安全的那一侧）。
    let fresh = dir.join(format!("{file_name}.polaris-new-{}-1", pid.wrapping_add(2)));
    // ④ 落位好的成品：绝不能碰。
    let dest = dir.join(file_name);
    // ⑤ 别的资产名 + 新鲜：**保留**。单飞闸只按 dest 串行，管不到别的资产名 ——
    //    本进程完全可能有另一条腿正在下它，故别资产不许走即时档（哪怕 pid 是自己的）。
    let other_fresh = dir.join(format!("polaris-0.1.9.dmg.polaris-new-{pid}-0"));
    // ⑥ **真实缺陷回放**：上次运行下到一半被关掉，留下的是**旧版本名**的残件（资产名含版本号，
    //    这次要下的是新版本 ⇒ 前缀必然不同）。陈旧了就必须收，否则每次版本更迭攒一个几十 MiB
    //    的垃圾，直到完全卸载才回收。
    let stale_old_version = dir.join(format!(
        "polaris-0.1.9.dmg.polaris-new-{}-4",
        pid.wrapping_add(3)
    ));
    // ⑦ 含中缀但**形态不符**（`{pid}-{seq}` 不是纯数字）：不是 `tmp_name` 的产物，一律不碰。
    let not_a_tmp = dir.join("notes.polaris-new-draft");

    for p in [
        &mine,
        &stale,
        &fresh,
        &dest,
        &other_fresh,
        &stale_old_version,
        &not_a_tmp,
    ] {
        std::fs::write(p, b"x").unwrap();
    }
    let over_age = ORPHAN_TMP_MAX_AGE + std::time::Duration::from_secs(60);
    backdate(&stale, over_age);
    backdate(&stale_old_version, over_age);
    backdate(&not_a_tmp, over_age); // 陈旧也不该被收：判据是命名形态，不是年龄
    backdate(&mine, std::time::Duration::from_secs(1)); // 新鲜也照收（判据是 pid 不是时间）

    sweep_orphan_downloads(&StdFs, &dir, file_name);

    assert!(
        !mine.exists(),
        "本资产 + 本进程留下的残件必须被收（闸已保证无人在写它）"
    );
    assert!(!stale.exists(), "超过 24h 的跨进程残件必须被收");
    assert!(
        fresh.exists(),
        "新鲜的跨进程残件必须保留 —— 另一个实例可能正在写它，而 pid 存活探测跨平台不可靠"
    );
    assert!(dest.exists(), "落位好的更新包绝不能被当成残件删掉");
    assert!(
        other_fresh.exists(),
        "别的资产名的**新鲜**残件必须保留 —— 单飞闸只按 dest 串行，管不到别的资产名"
    );
    assert!(
        !stale_old_version.exists(),
        "旧版本名的陈旧残件必须被收 —— 资产名含版本号，只收本次资产名等于这条腿永不回收"
    );
    assert!(
        not_a_tmp.exists(),
        "含 `.polaris-new-` 但不是 `{{pid}}-{{seq}}` 形态的文件不是清扫的对象"
    );

    // 交叉核对：上面五个夹具的名字是**手搓**的，与 `sweep_orphan_downloads` 的前缀判据同出一人
    // ⇒ 二者可以互相印证却**双双跑偏于真实命名**（`verify::tmp_name` 改个后缀就全盲）。
    // 故再用真产物过一遍：这条是「判据被自己污染」的唯一报警。
    let real_tmp = polaris_updater::verify::tmp_name(&dest);
    std::fs::write(&real_tmp, b"x").unwrap();
    sweep_orphan_downloads(&StdFs, &dir, file_name);
    assert!(
        !real_tmp.exists(),
        "`verify::tmp_name` 的真实产物落在清扫的前缀判据之外 —— 手搓夹具全绿也没有意义"
    );
    assert!(dest.exists(), "第二次清扫同样不得碰成品");

    // 目录不存在也不得 panic（best-effort，绝不改变本次下载的结论）。
    sweep_orphan_downloads(&StdFs, &dir.join("nope"), file_name);
}

// ── 弹窗邀请 ↔ 真正下载：同一个版本 ────────────────────────────────────

/// 两个 App 更新通道都由同一份 GitHub release 选择逻辑解释；稳定版过滤 prerelease，测试版纳入。
#[test]
fn app_update_channel_selects_the_expected_release_line() {
    const RELEASES: &str = r#"[
          {"tag_name":"v0.3.0-beta.1","prerelease":true,"published_at":"2024-06-01T00:00:00Z",
           "assets":[{"name":"Polaris_0.3.0-beta.1_aarch64-mac.dmg","browser_download_url":"https://x/beta","size":1}]},
          {"tag_name":"v0.2.0","prerelease":false,"published_at":"2024-05-01T00:00:00Z",
           "assets":[{"name":"Polaris_0.2.0_aarch64-mac.dmg","browser_download_url":"https://x/stable","size":1}]}
        ]"#;
    let pick = |include_pre: bool| match check_app_update(
        RELEASES,
        "0.1.0",
        include_pre,
        None,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .expect("样本 JSON 必须可解析")
    {
        AppUpdateCheck::Available(i) => i,
        AppUpdateCheck::NoUpdate => panic!("样本里有更新，不该判成无更新"),
    };

    assert_eq!(pick(false).version, "v0.2.0");
    assert_eq!(pick(true).version, "v0.3.0-beta.1");
}

/// 递归收集 `src-tauri/src` 下的全部 Rust 源码；读取失败必须 fail-loud。
fn rust_sources_under(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("读不到目录 {}: {e}", dir.display()));
    for entry in entries {
        let p = entry.expect("目录项读取失败").path();
        if p.is_dir() {
            rust_sources_under(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let s = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("读不到 {}: {e}", p.display()));
            out.push((p, s));
        }
    }
}

/// 从函数调用左括号取顶层实参；足以覆盖 `update_check` 当前四个简单实参。
fn call_args(src: &str, open: usize) -> Vec<String> {
    let mut depth = 0usize;
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_comment = false;
    let mut it = src[open..].chars().peekable();
    while let Some(c) = it.next() {
        if in_comment {
            if c == '\n' {
                in_comment = false;
            }
            continue;
        }
        if c == '/' && it.peek() == Some(&'/') {
            in_comment = true;
            it.next();
            continue;
        }
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                if depth > 1 {
                    cur.push(c);
                }
            }
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    if !cur.trim().is_empty() {
                        args.push(cur.split_whitespace().collect());
                    }
                    return args;
                }
                cur.push(c);
            }
            ',' if depth == 1 => {
                args.push(cur.split_whitespace().collect());
                cur.clear();
            }
            _ if depth >= 1 => cur.push(c),
            _ => {}
        }
    }
    panic!("update_check 调用括号未配平")
}

/// 启动和托盘从当前配置产出提醒；弹窗复查必须使用会话保存的口径。三条内部调用都不得启用
/// `include_current`，同版本重下只能由设置页的显式 IPC 动作触发。
#[test]
fn every_internal_update_check_preserves_channel_and_disables_current_version_resolution() {
    const NEEDLE: &str = "update_check(";
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources_under(&root, &mut files);

    let expected: std::collections::BTreeMap<&str, &str> = [
        ("runtime/startup_tasks.rs", "Some(include_prerelease)"),
        ("tray/commands.rs", "Some(include_prerelease)"),
        ("commands/updater/app_update.rs", "popup.include_prerelease"),
    ]
    .into_iter()
    .collect();
    let mut seen = std::collections::BTreeMap::new();

    for (path, raw) in &files {
        let src = crate::commands::guard_scan::strip_line_comments(raw);
        let mut from = 0usize;
        while let Some(rel) = src[from..].find(NEEDLE) {
            let at = from + rel;
            from = at + NEEDLE.len();
            let before = &src[..at];
            if before
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
                || before.trim_end().ends_with("fn")
            {
                continue;
            }
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            if before[line_start..].matches('"').count() % 2 == 1 {
                continue;
            }

            let rel_path = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let args = call_args(&src, at + "update_check".len());
            assert_eq!(
                args.len(),
                4,
                "{rel_path} 的 update_check 应有四个实参：{args:?}"
            );
            let expected_channel = expected
                .get(rel_path.as_str())
                .unwrap_or_else(|| panic!("出现未登记的内部 update_check 调用：{rel_path}"));
            assert_eq!(&args[2], expected_channel, "{rel_path} 的通道来源漂移");
            assert_eq!(args[3], "None", "{rel_path} 不得解析同版本安装包");
            assert!(
                seen.insert(rel_path, args[2].clone()).is_none(),
                "同一文件出现重复调用"
            );
        }
    }

    assert_eq!(
        seen.len(),
        expected.len(),
        "三条更新入口必须全部处于守卫射程"
    );
}
/// 🟡 **真值表：对账判定本身（`reconcile_recheck`）。**
///
/// **变异探针**：把 `advertised == Some(rechecked)` 改成 `!=`、或把空串那条早退删掉
/// ⇒ 逐条转红。
#[test]
fn reconcile_recheck_only_proceeds_on_a_verbatim_match() {
    // 逐字相同 ⇒ 照下（唯一放行档）。
    assert_eq!(
        reconcile_recheck(Some("v1.2.0"), "v1.2.0"),
        RecheckVerdict::Proceed
    );
    // 复查期间上游又发了一版（两端都是正式版，口径常量对此无能为力）⇒ 重新征求同意。
    assert_eq!(
        reconcile_recheck(Some("v1.2.0"), "v1.3.0"),
        RecheckVerdict::Renegotiate
    );
    // 大小写 / 前缀 / 空白差异一律不算「相同」——版本号是标识符，不是自然语言。
    assert_eq!(
        reconcile_recheck(Some("v1.2.0"), "V1.2.0"),
        RecheckVerdict::Renegotiate
    );
    assert_eq!(
        reconcile_recheck(Some("v1.2.0"), "1.2.0"),
        RecheckVerdict::Renegotiate
    );
    assert_eq!(
        reconcile_recheck(Some("v1.2.0"), " v1.2.0"),
        RecheckVerdict::Renegotiate
    );
    // 不知道当初承诺了什么 ⇒ 不能声称对上了（失败安全，不是 Proceed）。
    assert_eq!(
        reconcile_recheck(None, "v1.3.0"),
        RecheckVerdict::Renegotiate
    );
    // 复查回包没有版本号 = 契约破损：既不下也不弹空版本号。
    assert_eq!(
        reconcile_recheck(Some("v1.2.0"), ""),
        RecheckVerdict::Unusable
    );
    assert_eq!(reconcile_recheck(None, ""), RecheckVerdict::Unusable);
}

/// 这条是修复的另一半。修好选包器却让本函数恒返回 `false`（旧实现读的
/// `PORTABLE_EXECUTABLE_DIR` 在本仓恒不存在），便携用户照样被推安装器 ——
/// 变异探针：把判据改回任何一个 env 读取 ⇒ 「有标记」那条转红。
#[test]
fn portable_layout_is_decided_by_marker_file_next_to_exe() {
    let base = std::env::temp_dir().join(format!("polaris-portable-probe-{}", std::process::id()));
    let portable_dir = base.join("portable");
    let installed_dir = base.join("installed");
    std::fs::create_dir_all(&portable_dir).unwrap();
    std::fs::create_dir_all(&installed_dir).unwrap();
    std::fs::write(portable_dir.join(PORTABLE_MARKER), b"portable\n").unwrap();

    // 有标记 ⇒ 便携。
    assert!(
        is_portable_layout(&portable_dir.join("polaris.exe")),
        "exe 同级有 {PORTABLE_MARKER} 时必须判成便携形态，否则便携用户会被推 NSIS 安装器"
    );
    // 无标记 ⇒ 安装态（NSIS 装出来的目录里没有这个文件）。
    assert!(!is_portable_layout(&installed_dir.join("polaris.exe")));
    // 标记是**目录**不算数（`is_file` 而非 `exists`）。
    std::fs::create_dir_all(installed_dir.join(PORTABLE_MARKER)).unwrap();
    assert!(!is_portable_layout(&installed_dir.join("polaris.exe")));
    // 无父目录的退化路径不得 panic。
    assert!(!is_portable_layout(std::path::Path::new("polaris.exe")));

    std::fs::remove_dir_all(&base).unwrap();
}

// ── 下载进度（此前只有 0% / 100% 两点）───────────────────────────────────

/// 🟡 **进度百分比的三条规则各有牙**：无分母不发 / 夹在 1..=99 / 同值去重。
///
/// **变异探针**：去掉 `clamp(1, 99)` ⇒ 「0 与 100 不由中段发」两条断言转红；
/// 去掉 `pct != last_pct` ⇒ 「同百分比不重发」转红；
/// 把 `expected` 缺失时回落成 `received` ⇒ 「无 Content-Length 不发」转红。
#[test]
fn progress_percent_rules() {
    // 无 Content-Length / 为 0 → 一律不发（不拿已收字节凑假分母）。
    assert_eq!(progress_percent(5_000, None, 0), None);
    assert_eq!(progress_percent(5_000, Some(0), 0), None);

    // 正常中段。
    assert_eq!(progress_percent(50, Some(100), 0), Some(50));
    // 同百分比不重发（IPC 洪水防线）。
    assert_eq!(progress_percent(50, Some(100), 50), None);
    assert_eq!(progress_percent(509, Some(1000), 50), None, "50.9% 仍是 50");
    assert_eq!(progress_percent(510, Some(1000), 50), Some(51));

    // 下界：0% 不由中段发（那一发由下载开始前独占）。
    assert_eq!(
        progress_percent(0, Some(1000), 0),
        Some(1),
        "首个 chunk 前的 0 会被夹到 1；last_pct=0 故仍发一次"
    );
    assert_eq!(progress_percent(3, Some(1000), 1), None, "夹到 1，与上次同");
    // 上界：100% 不由中段发（那一发由 downloaded 独占，否则出现倒退帧）。
    assert_eq!(progress_percent(1000, Some(1000), 50), Some(99));
    // received 超过 total（服务端 Content-Length 撒谎）→ 仍夹在 99，不溢出。
    assert_eq!(progress_percent(9_999, Some(1000), 50), Some(99));
}

// ── 弹窗各档（此前只有 remind 一档可达）─────────────────────────────────

/// 🟡 **弹窗镜像与广播载荷读的是同一帧：三种帧各自带齐自己那一屏要的事实。**
///
/// 本函数此前吃的是压平后的 `(status, percentage, error)`，落位路径与版本号在那层压平里丢了
/// ⇒ `done` 只剩一个状态字。改吃 [`ProgressStage`] + 同一份清单后，两屏结构上不可能各说各话。
///
/// **变异探针**：把 `Downloaded` 那臂的 `path.to_string_lossy()` 换成常量串 ⇒ 落位路径那条转红；
/// 把 `info.get("fileSize")` 换成 `None` ⇒ 分母那条转红；
/// 把 `Downloading` 的 `Some(received)` 换成 `None` ⇒ 已收字节那条转红。
#[test]
fn progress_frames_map_to_popup_phases_with_their_facts() {
    let info = json!({ "version": "v1.2.0", "fileSize": 52_000_000_u64 });
    let path = std::path::Path::new("/tmp/updates/polaris.dmg");

    let downloading = popup_state_for(
        &info,
        ProgressStage::Downloading {
            percentage: 42,
            received: 19_240_000,
        },
    );
    assert_eq!(downloading.phase, PopupPhase::Progress);
    assert_eq!(downloading.percentage, Some(42));
    assert_eq!(
        downloading.received_bytes,
        Some(19_240_000),
        "已收字节必须是回调原值 —— 从百分比反推的数每一帧都是错的"
    );
    assert_eq!(
        downloading.total_bytes,
        Some(52_000_000),
        "分母取本帧随行清单的 fileSize（与设置页下载卡同源）"
    );

    let done = popup_state_for(
        &info,
        ProgressStage::Downloaded {
            path,
            verified: true,
        },
    );
    assert_eq!(done.phase, PopupPhase::Done);
    assert_eq!(
        done.file_path.as_deref(),
        Some("/tmp/updates/polaris.dmg"),
        "「完成」得说得出包落在哪儿 —— 没有它，它与「什么都没下」长得一模一样"
    );
    assert_eq!(
        done.version.as_deref(),
        Some("v1.2.0"),
        "得说得出下的是哪一版"
    );

    let err = popup_state_for(
        &info,
        ProgressStage::Failed(UpdateErr::with_detail(
            UpdateErrCode::DownloadFailed,
            "net down",
        )),
    );
    assert_eq!(err.phase, PopupPhase::Error);
    // U1：弹窗 error 态带码 + 诊断串，正文本地化在渲染端。
    assert_eq!(err.error_code.as_deref(), Some("downloadFailed"));
    assert_eq!(err.error_detail.as_deref(), Some("net down"));
    // 无细节 → 空 detail 而非缺键（弹窗渲染端按可选处理）。
    let bare = popup_state_for(
        &info,
        ProgressStage::Failed(UpdateErr::new(UpdateErrCode::DownloadFailed)),
    );
    assert_eq!(bare.error_code.as_deref(), Some("downloadFailed"));
    assert_eq!(bare.error_detail.as_deref(), Some(""));

    // 清单缺 `fileSize` / 给 0 ⇒ 分母未知，只报已收量，**不拿已收字节凑假分母**。
    for blind in [json!({}), json!({ "fileSize": 0 })] {
        let s = popup_state_for(
            &blind,
            ProgressStage::Downloading {
                percentage: 42,
                received: 19_240_000,
            },
        );
        assert_eq!(s.received_bytes, Some(19_240_000));
        assert_eq!(s.total_bytes, None, "分母未知时不得编一个出来：{blind}");
    }
    // 清单缺 `version` ⇒ 不编版本号（会话继承会补上弹窗邀请的那一版，见 PopupSession）。
    assert_eq!(
        popup_state_for(
            &json!({}),
            ProgressStage::Downloaded {
                path,
                verified: false,
            }
        )
        .version,
        None
    );
}

/// 🟡 **后台下载不得顶掉用户面前的 remind 提示。**
///
/// `autoDownloadUpdate` 开启时，启动检查会「弹 remind 窗」+「后台下载」并行；若进度事件无条件
/// 镜像进弹窗，用户**再也看不到**「要不要更新」那一屏。闸只放行 `Progress`（= 用户亲手点过
/// 「更新」的弹窗）。
///
/// 判据面**遍历 [`PopupPhase::ALL`]**，不点名几个 phase：新加一档而闸忘了表态时，点名式清单
/// 静默漏掉那一格（且全绿），遍历式必判。`ALL` 自己与枚举由 `state.rs` 的门对账。
///
/// **变异探针**：把闸改成恒 true / 放行任意一个非 Progress 档 ⇒ 本条转红并点名那一档。
#[test]
fn popup_only_follows_progress_it_was_put_into_by_the_user() {
    for phase in PopupPhase::ALL {
        let expected = phase == PopupPhase::Progress;
        assert_eq!(
            should_mirror_to_popup(phase),
            expected,
            "{phase} 档的跟随判定错了 —— 只有用户亲手推进 progress 的弹窗才跟随后台下载；\
                 remind 被顶掉用户就再也看不到「要不要更新」那一屏，终态被顶掉则是拿一次无关\
                 下载改写一个已经落定的结论"
        );
    }
    // 正向对照：闸不是恒 false（那样弹窗里点「更新」后窗内零反馈）。
    assert!(should_mirror_to_popup(PopupPhase::Progress));
}

// ── W8：「跳过此版本」存的与比的必须同口径 ─────────────────────────────

/// 🟡 **存储口径本体 = trim + strip_v，与比较侧同形。**
///
/// **变异探针**：把 helper 里的 `strip_v` 删掉 ⇒ 第 1 条红；把 `trim` 删掉 ⇒ 第 3 条红。
#[test]
fn stored_skip_version_matches_the_compare_side_form() {
    assert_eq!(stored_skip_version("v0.2.0"), "0.2.0");
    assert_eq!(
        stored_skip_version("0.2.0"),
        "0.2.0",
        "已归一化的输入必须幂等"
    );
    assert_eq!(
        stored_skip_version("  v0.2.0 "),
        "0.2.0",
        "顺带 trim：比较侧的 strip_v 不会碰空白"
    );
    assert_eq!(
        stored_skip_version("v1.2.3-beta.1"),
        "1.2.3-beta.1",
        "strip_v 只去前导一个 v，预发布后缀原样保留"
    );
    // 与比较侧的闭环另一半在 github.rs：check_app_update_skipped_version_is_no_update
    // 里带「原始 tag ⇒ Available」反例，钉死不许把比较侧改回原始 tag 来「修」跳过。
}

/// Detached spawn 失败不得提交退出；成功后消费持锁的 Ready，提交失败也不得报成功。
#[test]
fn complete_detached_install_owns_success_effects_and_leaves_errors_untouched() {
    let effects = std::cell::RefCell::new(Vec::new());
    let failed: Result<(), &str> = complete_detached_install(Err("spawn failed"), || {
        effects.borrow_mut().push("commit");
        Ok(())
    });
    assert_eq!(failed, Err("spawn failed"));
    assert!(effects.borrow().is_empty());
    let completed: Result<&str, &str> = complete_detached_install(Ok("detached-script"), || {
        effects.borrow_mut().push("commit");
        Ok(())
    });
    assert_eq!(completed, Ok("detached-script"));
    assert_eq!(*effects.borrow(), vec!["commit"]);
    let rejected: Result<&str, &str> =
        complete_detached_install(Ok("detached-script"), || Err("ready invalid"));
    assert_eq!(rejected, Err("ready invalid"));
}

// ── 更新进度快照槽：切走再切回来不丢进度 ────────────────────────────────

/// 本节共用的发布清单样本（字段取自真实 `update_check` 回包里被 `progress_manifest` 投影的那几个）。
fn progress_info_sample() -> Value {
    json!({
        "version": "v1.2.0",
        "fileSize": 52_000_000_u64,
        "isPrerelease": false,
        "fileName": "polaris-1.2.0.dmg",
    })
}

/// 🟢 **槽里放的就是那一帧本身，且后写覆盖前写。**
///
/// 缺陷形态（用户报告）：更新下到一半，窗口切走再切回来，卡片退回 idle 显示「检查更新」。成因是
/// 设置页的更新状态全在组件本地 state 里，重挂载后只能等下一帧 —— 而下载已下完时那一帧永不再来。
/// 本槽是回读腿（`update_get_progress`）的唯一数据源。
///
/// **变异探针**：`set_last_progress` 改成「已有值就不覆盖」⇒ 第 3 条转红（槽停在 30%，
/// 用户切回来看到的是早已过期的进度）；改成写入前先 `*g = None` 再判空 ⇒ 第 2 条转红。
#[test]
fn last_progress_slot_holds_the_frame_it_was_given_and_keeps_the_newest() {
    let tmp = scratch("last-progress");
    let rt = crate::runtime::updater::UpdaterRuntime::new(tmp.path().to_path_buf());
    let info = progress_info_sample();

    // 1. 一帧都没发过 ⇒ None。**不编造 idle 帧**：「没发过」与「处于 idle」是两回事，
    //    回一个假 idle 会让 UI 把「不知道」误当成「知道且没在下」。
    assert!(
        rt.last_progress().is_none(),
        "刚装配的运行时不得凭空有一帧进度"
    );

    // 2. 存进去的与同参 `progress_payload` 逐字段相等（emit_progress 存的正是这个值）。
    let at30 = ProgressStage::Downloading {
        percentage: 30,
        received: 15_600_000,
    };
    rt.set_last_progress(progress_payload(&info, at30));
    assert_eq!(
        rt.last_progress().expect("存过一帧就该读得到"),
        progress_payload(&info, at30),
        "槽里的值与广播出去的载荷必须是同一份 —— 回读到的与刚收到的事件对不上就是新的不一致"
    );

    // 3. 后写覆盖前写：连发 30 → 70 之后槽里是 70 那一帧。
    let at70 = ProgressStage::Downloading {
        percentage: 70,
        received: 36_400_000,
    };
    rt.set_last_progress(progress_payload(&info, at70));
    let held = rt.last_progress().expect("覆盖之后仍应有值");
    assert_eq!(held, progress_payload(&info, at70));
    assert_eq!(held["percentage"], json!(70));
    assert_eq!(
        held["receivedBytes"],
        json!(36_400_000),
        "随行事实要跟着最新那一帧走，不能停在上一帧"
    );
}

/// 🟢 **终态帧不清空。**
///
/// 这是本批的一条**决定**，不是疏漏：用户切走再切回来要看到的正是「下载完成了」。
/// 终态帧后清空槽 = 卡片被打回 idle，那恰恰是本槽要修的缺陷本身。失败帧同理：切回来该看到
/// 「失败了 + 重试」，而不是一个什么都没发生过的 idle。
///
/// **变异探针**：在 `set_last_progress` 里对 `Downloaded` / `Failed` 帧改存 `None` ⇒ 本条两格转红。
#[test]
fn last_progress_slot_keeps_terminal_frames_instead_of_clearing() {
    let tmp = scratch("last-progress-terminal");
    let rt = crate::runtime::updater::UpdaterRuntime::new(tmp.path().to_path_buf());
    let info = progress_info_sample();
    let path = std::path::Path::new("/var/tmp/updates/polaris-1.2.0.dmg");

    let landed = ProgressStage::Downloaded {
        path,
        verified: true,
    };
    rt.set_last_progress(progress_payload(&info, landed));
    let held = rt.last_progress().expect("落位帧之后槽里必须仍有值");
    assert_eq!(held, progress_payload(&info, landed));
    assert_eq!(held["status"], json!("downloaded"));
    assert_eq!(
        held["filePath"],
        json!(path.to_string_lossy()),
        "「重启并安装」拿的就是这个路径 —— 丢了它按钮就是哑键"
    );

    let failed = ProgressStage::Failed(UpdateErr::with_detail(
        UpdateErrCode::DownloadFailed,
        "net down",
    ));
    rt.set_last_progress(progress_payload(&info, failed));
    let held = rt.last_progress().expect("失败帧之后槽里必须仍有值");
    assert_eq!(held, progress_payload(&info, failed));
    assert_eq!(held["status"], json!("error"));
    assert_eq!(
        held["errorCode"],
        json!("downloadFailed"),
        "切回来要看到「失败了 + 可重试」，不是一个什么都没发生过的 idle"
    );
}

/// 🔴 **次序锁：Android 那条腿必须在「停代理」与「写脚本」之前整条早退。**
///
/// # 为什么次序是判据本体，而不是风格
///
/// `update_install` 的后半段做三件不可逆的事：停代理 → 写脚本 → `spawn` + 退出应用。
/// Android 上这三件一件都不该做（成因见 [`crate::runtime::update_install::InstallPlatform`] 的
/// `Android` 文档）。而**「交给系统安装器」返回之后用户还没决定装不装** —— 排在它后面的每一行
/// 都会把「用户还在看确认框」当成「已经装完」。
///
/// 最坏的一形是把 Android 分支写在 `proxy.stop()` 之后：功能看起来完全正常（安装器照样弹出来），
/// 只有在用户按「取消」时才暴露 —— 隧道已经被停了，而应用以为自己什么都没做。
/// 那种缺陷跑一次 happy path 是抓不到的，只有次序判据抓得到。
///
/// # 为什么是源码扫描
///
/// 这条不变式的两端（`State<'_, AppRuntime>` 与 Tauri `AppHandle`）在单测里构造不出来，
/// 而它守的是**控制流位置**，不是某个函数的返回值。
#[cfg(target_os = "linux")]
#[test]
fn update_install_hands_android_off_before_touching_the_proxy_or_scripts() {
    let body =
        crate::commands::guard_scan::top_level_fn_body(src(), "pub async fn update_install(");

    let android = body
        .find("if plan.platform == update_install::InstallPlatform::Android {")
        .expect("Android 分流不见了 —— 那条腿会掉进桌面的脚本路径");
    let prepare = body
        .find("crate::exit_lifecycle::prepare_desktop_exit(&app).await")
        .expect("严格退出准备门不见了 —— 本条次序判据失去了它要比的另一端");
    let script = body
        .find("update_install::build_install_script(&plan)")
        .expect("建脚本那一步不见了 —— 本条次序判据失去了它要比的另一端");

    assert!(
        android < prepare,
        "Android 分流必须在停代理**之前**：用户在系统确认框上按取消之后，\
         留下的不该是一条已经被停掉的隧道"
    );
    assert!(
        android < script,
        "Android 分流必须在建脚本**之前**：Android 上没有脚本腿，走到那里只能造一个假的"
    );

    // 早退必须是真的早退（`return`），不是算完一个值又往下走。
    let detach = body
        .find("update_install::spawn_detached_script(&dir, spec)")
        .expect("安装器启动不见了");
    assert!(prepare < detach, "安装器必须等待严格退出准备成功");
    let branch = &body[android..prepare];
    assert!(
        branch.contains("return Ok("),
        "Android 分支必须整条 return —— 落下去就会碰到后面那三件不可逆的事"
    );
    // 「交出去了」与「交不出去但知道为什么」必须分开回报：压成一句「安装失败」正是这条腿
    // 要消灭的形态（没授予「安装未知应用」的用户按一下开关就能继续）。
    assert!(
        branch.contains("\"awaitingSystemInstaller\": true")
            && branch.contains("\"awaitingSystemInstaller\": false"),
        "两种结局必须各有各的回包，不许折成一个"
    );
    assert!(
        branch.contains("\"reason\": reason"),
        "交不出去时必须把 Kotlin 侧的原因码原样带回来，不许吞掉"
    );
    // 🔴 **判据键不许与 `handedToSystem` 共用**（2026-09-06 补，复审抓到的真缺陷）。
    //
    // `handedToSystem` 在本仓只有一个产地：上面那条**失败**路径（形态错配 ⇒ 放弃安装、
    // 回退 `shell.open`，见 `\"handedToSystem\": opened`）。前端唯一的消费点
    // `ui/.../use-app-update.ts` 是 `if (result.handedToSystem || result.reason === 'form-mismatch')`
    // ⇒ 命中即 `settleInstall('error', formMismatch)`。Android **成功**回包如果也标它，
    // 用户会在系统安装器弹出来的同时看到「更新失败 / 形态错配」；而 Android **失败**回包
    // （`handedToSystem:false` + 一个 `unknown-sources-*` 码）两个条件都不成立 ⇒ 一个分支
    // 都不进 ⇒ 卡在「安装中」一个字都不说 —— 正是这条腿声称要消灭的那句话。
    //
    // 这条断言的射程只有 Android 那一段（`branch`），上面那条形态错配路径照旧用它。
    assert!(
        !branch.contains("handedToSystem"),
        "Android 分支复用了 `handedToSystem` —— 那个键在本仓已有唯一相反的语义（失败/回退），\
         前端的判据按失败处理。成功要用只属于本条腿的键（`awaitingSystemInstaller`）。"
    );
}

// ── Windows 便携版：停核先于交付，交付成功才退出 ─────────────────────────────

fn portable_plan() -> crate::runtime::update_install::InstallPlan {
    // 反斜杠样本：计划决策对路径只做字符串运算，故本条在任何宿主上给出同一个答案。
    let exe = std::path::Path::new("D:\\Tools\\Polaris\\polaris.exe");
    crate::runtime::update_install::decide_install_plan(
        "windows",
        crate::runtime::update_install::RunForm::Loose,
        std::path::Path::new("C:\\Cache\\updates\\Polaris_1.2.3_x64-win-Portable.zip"),
        exe,
        None,
        Some(exe),
    )
    .expect("便携运行形态 + 便携压缩包必须有计划")
}

/// 便携腿的「先告知」回包带着程序所在目录；别的腿不带这个键。
///
/// 前端拿不到程序目录，而那条告知要把「覆盖到哪个文件夹」写给用户看 —— 少了这个键，
/// 说明里那一处就是空的。
#[test]
fn confirm_request_carries_the_program_dir_only_for_the_portable_leg() {
    let plan = portable_plan();
    let advisory =
        crate::runtime::update_install::install_advisory(&plan).expect("便携腿必须先告知");
    assert_eq!(
        confirm_request(&plan, advisory),
        json!({
            "ok": false,
            "needConfirm": true,
            "advisory": "portableManualReplace",
            "programDir": "D:\\Tools\\Polaris",
        })
    );

    let setup = crate::runtime::update_install::decide_install_plan(
        "windows",
        crate::runtime::update_install::RunForm::Installed,
        std::path::Path::new("C:\\Cache\\updates\\polaris-1.2.3-win-setup.exe"),
        std::path::Path::new("C:\\Apps\\Polaris\\polaris.exe"),
        None,
        None,
    )
    .unwrap();
    let advisory = crate::runtime::update_install::install_advisory(&setup).unwrap();
    assert_eq!(
        confirm_request(&setup, advisory),
        json!({ "ok": false, "needConfirm": true, "advisory": "windowsSmartScreen" })
    );
}

/// 🔴 **便携腿：内核必须先停，压缩包与程序目录才许打开；打开成功才许退出。**
///
/// 内核直接从程序目录运行并占用自己的映像文件。窗口一打开用户就可能开始覆盖解压 —— 那一刻
/// 内核若还在跑，被占用的文件会被跳过，目录里就是新旧混装。故次序是
/// 「告知 → 退出准备门（停核）→ 打开 → 提交退出」，这里按调用点位置钉住。
///
/// 控制流位置在单测里构造不出两端（`AppHandle` / `State`），故扫源码。
///
/// 变异：把 `open_portable_update(` 那次调用挪到 `prepare_desktop_exit` 之前 ⇒ 本条红。
#[cfg(target_os = "linux")]
#[test]
fn update_install_stops_the_core_before_opening_the_portable_archive() {
    let body =
        crate::commands::guard_scan::top_level_fn_body(src(), "pub async fn update_install(");

    let confirm = body
        .find("return Ok(ApiResponse::ok(confirm_request(&plan, advisory)));")
        .expect("未确认即早退那一步不见了");
    let prepare = body
        .find("crate::exit_lifecycle::prepare_desktop_exit(&app).await")
        .expect("严格退出准备门不见了");
    let open = body
        .find("open_portable_update(&app, &installer, program_dir)")
        .expect("便携腿的交付调用不见了");
    let complete = body
        .find("complete_detached_install(delivered, || {")
        .expect("交付后的提交点不见了");
    let commit = body
        .find("crate::exit_lifecycle::commit_desktop_exit(")
        .expect("退出提交不见了");

    assert!(
        confirm < prepare,
        "未确认时不许碰内核：用户不继续即真 no-op"
    );
    assert!(
        prepare < open,
        "停核必须先于打开压缩包与程序目录 —— 反过来用户会在内核仍占用文件时开始覆盖"
    );
    assert!(
        open < complete && complete < commit,
        "打开成功才许提交退出：打不开就退出，用户连该覆盖到哪都看不到"
    );
    assert_eq!(
        body.matches("open_portable_update(").count(),
        1,
        "便携交付只许有这一个调用点（多出来的那个不受上面的次序约束）"
    );
    // 准备门失败必须整条 return，不许落下去照样打开；且带着「需重启应用」的码
    // （准备门一进去起核准入就永久关上了，见 `INSTALL_RESTART_REQUIRED`）。
    let gate = &body[prepare..open];
    assert!(
        gate.contains("Err(error) =>") && gate.contains("return Ok(ApiResponse::err_with_code("),
        "退出准备失败时必须早退，不能带着还在运行的内核继续交付"
    );
    assert!(
        gate.contains("INSTALL_RESTART_REQUIRED"),
        "退出准备失败的回包必须带「需重启应用」的码"
    );
    // 准备门之后的每一个失败出口都带这个码：门后没有无码的 `ApiResponse::err(`。
    let after_gate = &body[prepare..];
    assert!(
        !after_gate.contains("ApiResponse::err("),
        "准备门之后出现了无码错误：渲染端分不出它与「下载中断」"
    );
    assert_eq!(
        after_gate.matches("INSTALL_RESTART_REQUIRED").count(),
        2,
        "准备失败、交付或提交失败，两个出口各带一次"
    );
    // 两个位置在打开之后、提交退出之前留底（日志 + 更新状态文件）。
    let remember = body
        .find("remember_portable_update(&app, &state, &installer, program_dir)")
        .expect("便携腿退出前没有留下压缩包位置与程序目录");
    assert!(
        open < remember && remember < commit,
        "留底必须在交付之后、提交退出之前：早了记的是一次没发生的交付，晚了进程已经没了"
    );

    // Unsupported assets now fail closed. Only a receipt-bound staged DMG can be opened.
    let opens: Vec<usize> = body.match_indices(".open(").map(|(i, _)| i).collect();
    assert_eq!(opens.len(), 1);
    let reject = body.find("Err(_reject) => {").unwrap();
    let advisory = body
        .find("update_install::install_advisory(&plan)")
        .unwrap();
    assert!(!body[reject..advisory].contains(".open("));
    let stage = body.find("updater.custody.stage(&source)").unwrap();
    assert!(advisory < stage && stage < opens[0]);

    let mac = body
        .find("if plan.platform == update_install::InstallPlatform::Macos {")
        .expect("macOS 手动交付必须受平台分支约束");
    let admission = body[mac..]
        .find("if let Err(error) = update_install::preflight_mac_admission(bundle) {")
        .expect("只有准入失败才可手动交付")
        + mac;
    assert!(advisory < mac && mac < admission && admission < opens[0] && opens[0] < prepare);
    let manual = &body[mac..prepare];
    assert!(manual.contains("if let Some(bundle) = plan.app_bundle_path.as_deref() {"));
    assert!(manual.contains("app.shell().open(file_path.clone(), None)"));
    assert!(!body[admission..opens[0]].contains("} else {"));
    let manual_return = body[opens[0]..prepare]
        .find("return Ok(match opened {")
        .expect("手动交付必须在关闭 Start 准入前整条早退");
    assert!(manual_return > 0);
    assert!(manual.contains("awaitingManualInstall"));
    assert!(manual.contains("Err(open_error) => ApiResponse::err("));
    assert!(!manual.contains("commit_desktop_exit"));

    // 交付函数本身只做「打开」：两处打开都在，且它不自己停核、不自己退出。
    let handoff = crate::commands::guard_scan::top_level_fn_body(src(), "fn open_portable_update(");
    assert_eq!(
        handoff.matches(".open(").count(),
        2,
        "压缩包与程序目录各打开一次"
    );
    assert!(handoff.contains("archive.to_string_lossy()"));
    assert!(handoff.contains("program_dir.to_string_lossy()"));
    for forbidden in ["prepare_desktop_exit", "commit_desktop_exit", "app.exit("] {
        assert!(
            !handoff.contains(forbidden),
            "交付函数里不该出现 `{forbidden}`"
        );
    }
    // 整个模块里它只被 `update_install` 调用（定义 1 处 + 调用 1 处）。
    assert_eq!(src().matches("open_portable_update(").count(), 2);
}

/// 码的字面是跨语言协议：渲染端 `settings-logic.ts` 的 `installFailureKey` 按它取
/// `settings.update.installRestartRequired`。
#[test]
fn install_restart_required_code_is_the_wire_contract() {
    assert_eq!(INSTALL_RESTART_REQUIRED, "installRestartRequired");
    let hook = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ui/src/components/screens/settings/settings-logic.ts"),
    )
    .expect("读渲染端取文逻辑");
    assert!(
        hook.contains("code === 'installRestartRequired'"),
        "渲染端没有按这个码分支"
    );
}

/// 便携腿的交付复用同一个提交闸：打不开 ⇒ 不退出；打开了 ⇒ 恰好提交一次。
#[test]
fn portable_handoff_failure_never_commits_the_exit() {
    let commits = std::cell::Cell::new(0_u32);
    let failed: Result<(), String> =
        complete_detached_install(Err("打开程序所在目录失败".to_string()), || {
            commits.set(commits.get() + 1);
            Ok(())
        });
    assert!(failed.is_err());
    assert_eq!(commits.get(), 0);
    let delivered: Result<(), String> = complete_detached_install(Ok(()), || {
        commits.set(commits.get() + 1);
        Ok(())
    });
    assert_eq!(delivered, Ok(()));
    assert_eq!(commits.get(), 1);
}

/// 🔴 **Android 自更新资产：从「如实登记的缺口」变成「接上了，并且被钉住」**（2026-09-13）。
///
/// # 这条断言的历史
///
/// 它上一版叫 `android_has_no_update_asset_selector_yet_and_that_is_registered`，钉的是
/// 「今天 `AssetPlatform::from_os("android")` 返 `None`」这个事实，并在文档里列了一张
/// 「谁把它填上，谁要一起做的事」的解锁清单。其中与应用更新有关的几条已逐条做完：
///
///  1. **release 真的出 APK** —— `.github/workflows/android.yml` 的 `release-apk` job
///     （签名构建 + 开箱验 + 上传成 release 资产）。⚠️ 它要的四个签名 secret 在仓外，
///     且今天没有任何调用方传 `publish_release: true` ⇒ **这条腿还一次都没跑过**。
///     这不是「做完了」，是「代码侧接好了、外部前置还欠着」——两者必须分得清。
///  2. **`decide_install_plan("android", …, ".apk")`** —— `runtime::update_install::tests::
///     android_apk_plans_the_system_installer_regardless_of_run_form` 已在。
///  3. **前端那半** —— `ui/src/ipc/api/updater.ts` 的 `UpdateInstallResult` 现在带
///     `awaitingSystemInstaller`，`ui/src/mobile/settings/UpdatePage.tsx` 画出了下载→交系统
///     安装器那一跳，五个 `REASON_*` 码各有各的文案（判据在
///     `ui/src/mobile/settings/app-update-install.test.ts`）。
///
/// # 于是这条断言换了对象：从「缺口还在」换成「接上了的那几条不许悄悄退回去」
///
/// 一条只会在「被填上那天」响一次的断言，填上之后如果只是删掉，等于把这一族的观测面清零 ——
/// 而这一族恰恰是「改一个字符 ⇒ Android 上永远查不到更新，且一句错都不报」的那种静默故障。
#[test]
fn android_update_assets_are_wired() {
    // ① App 资产腿：Android 是一个正经变体，ARMv8 原生包可选；模拟器架构不可回落到 ARM 包。
    assert_eq!(
        AssetPlatform::from_os("android"),
        Some(AssetPlatform::Android),
        "Android 从 AssetPlatform 里消失了 —— 移动端的「检查更新」会退回结构性恒答「已是最新」"
    );
    let apk = polaris_updater::github::GithubAsset {
        name: "Polaris_9.9.9_arm64-v8a-android.apk".to_string(),
        browser_download_url: "https://x/apk".to_string(),
        size: 210,
        digest: None,
    };
    let picked = polaris_updater::github::find_suitable_update_asset(
        std::slice::from_ref(&apk),
        AssetPlatform::Android,
        AssetArch::Arm64,
        false,
    );
    assert!(
        picked.is_some(),
        "选包器选不中 CI 真会产出的那个资产名 —— 命名契约两端漂了（逐字对拍在 crates/updater）"
    );
    assert!(
        polaris_updater::github::find_suitable_update_asset(
            std::slice::from_ref(&apk),
            AssetPlatform::Android,
            AssetArch::X64,
            false
        )
        .is_none(),
        "x86_64（模拟器）也选出了包 —— 正式发布只含 ARMv8/ARMv7，发过去的必然装不上"
    );

    // 正面对照：三个桌面平台仍然认得出来 —— 否则上面那些 `is_none()` 可能只是因为
    // 选包器整个塌了（那时本条拿一个坏掉的判据冒充一条成立的结论）。
    for os in ["windows", "macos", "linux"] {
        assert!(
            AssetPlatform::from_os(os).is_some(),
            "{os} 也认不出来了 —— from_os 塌了，本条的每一条否定断言都不成立"
        );
    }
}

#[test]
fn full_uninstall_prepares_all_desktop_owners_before_any_destructive_step() {
    let source = crate::test_support::crate_code("commands/updater/uninstall.rs");
    let body =
        crate::commands::guard_scan::top_level_fn_body(&source, "pub async fn app_uninstall_all(");
    let ready = body.find("prepare_desktop_exit(&app).await").unwrap();
    let destructive = body
        .find("with_helper_service_mutation_core_guard")
        .unwrap();
    assert!(ready < destructive);
    let refusal = &body[ready..destructive];
    assert!(refusal.contains("return Ok(ApiResponse::err_with_code("));
    assert!(refusal.contains("UNINSTALL_RESTART_REQUIRED"));
    assert!(
        body.contains("let _exit_ready ="),
        "the Ready guard spans helper scripts and config removal"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn portable_reminder_is_read_only_and_confirmed_install_requires_fresh_custody() {
    let body =
        crate::commands::guard_scan::top_level_fn_body(src(), "pub async fn update_install(");
    let readonly = body.find("if confirmed == Some(false)").unwrap();
    let condition_end = body[readonly..].find('{').unwrap() + readonly;
    let condition: String = body[readonly..condition_end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(condition,
        "ifconfirmed==Some(false)&&os==\"windows\"&&portable.is_some()&&state.updater().custody.admits_install(&installer).is_err()");
    let authority = body[readonly..]
        .find("    if let Err(error) = state.updater().custody.admits_install")
        .unwrap()
        + readonly;
    let branch = &body[readonly..authority];
    assert!(branch.contains("requiresRedownload"));
    assert!(branch.contains("exe_path.parent()"));
    for forbidden in [
        ".open(",
        "std::fs::",
        "mutate_state",
        ".stage(",
        "remove_file",
        "remember_portable_update",
    ] {
        assert!(!branch.contains(forbidden), "{forbidden}");
    }
    let stage = body.find("updater.custody.stage(&source)").unwrap();
    let handoff = body.find("hand_apk_to_system_installer(").unwrap();
    assert!(authority < stage && stage < handoff);
    assert!(body[authority..stage].contains("updateTrustRequired"));
    assert!(body[stage..handoff].contains("plan.installer_path = installer.clone()"));
}

/// 🟡 **调用点守卫：`update_download` 的每一条失败早退都必须先发 `error` 进度事件。**
///
/// 弹窗被 `force progress(0)` 推进 Progress 后，只有 `error` / `downloaded` 能把它推出去；
/// 静默 return 会让它永远转圈（只剩 Cancel 可点）。原实现的三条前置校验早退正是这样。
///
/// 按**计数**锁而不是逐条锁：新增任何一条失败早退却忘了配一发 error 事件，
/// 两个计数立刻对不上 ⇒ 转红。
///
/// # 计数用**前缀** `ApiResponse::err`
///
/// 数 `ApiResponse::err(` 会**漏掉** `ApiResponse::err_with_code(`（`err` 后面是 `_` 不是 `(`），
/// 新增一条带 code 的早退时守卫全盲；故按前缀数。今天函数体内没有「一条早退里两个信封共用
/// 一发 error 事件」的分支，两个计数须**逐一相等**。
#[cfg(target_os = "linux")]
#[test]
fn every_failure_path_emits_an_error_progress_event() {
    let body =
        crate::commands::guard_scan::top_level_fn_body(src(), "pub async fn update_download(");
    let errors = body.matches("ApiResponse::err").count();
    let emits = body.matches("emit(ProgressStage::Failed(").count();
    assert!(errors > 0, "锚点消失：守卫已失去判据");
    assert_eq!(
        errors, emits,
        "失败信封 {errors} 处、error 进度事件 {emits} 发 —— 对不上的那条会让弹窗永远转圈"
    );
    // 配对由结构保证：全部早退经 fail 闭包，其体内 emit 与 err_with_code 同体。
    assert!(
        body.contains("let fail = |e: UpdateErr<'_>|"),
        "fail 统一出口被改形 —— 它体内 emit+信封的配对是本守卫的前提"
    );
}

/// 🟡 **调用点守卫：复查回来「没有可下的东西」时，既不广播假的 `downloaded`，也不对弹窗
/// 自己谎称「已下载」。**
///
/// # 这道门此前只守住了一半
///
/// 旧判据是「函数体里必须出现 `push_popup_state(&app, UpdatePopupState::done())`」+「函数体里
/// 不得出现 `ProgressStage::Downloaded`」。它守的是**广播给设置页的那一份**（后半条），而
/// 前半条**要求**把 `done()` 推给弹窗自己 —— 于是「一个字节都没下」在发起本次动作的那一屏上
/// 被渲染成 `updatePopup.downloaded`（「下载完成」）+ 满格进度条。门在，守的方向漏了发起方。
///
/// 新判据把「推出 progress」与「推的是哪一档」拆开：仍必须把弹窗推出 progress（否则永远转圈），
/// 但那一档必须是 `no_update`；`done` 在本臂**一次都不许出现**。`UpdatePopupState::done` 现在
/// 是必填落位路径的构造函数，本臂根本没有路径可传 —— 源码门与类型闸在此重合，两道都留着：
/// 类型挡「拿不出路径」，本门挡「随手编一个路径喂给它」。
///
/// # 判据必须收到**分支**，臂一级还不够
///
/// 射程收紧走过三级，每一级都是被同一形态咬出来的（「取材面宽于意图面」）：
///  1. **整函数**：把推送搬进别的臂就能替本臂作证 —— 本仓在同一个函数上栽过两次
///     （见 `guard_scan::match_arm_body` 文档）。
///  2. **臂**（`match_arm_body`）：仍不够。`Update | Retry` 臂内有 **5 条早退分支**，而
///     `arm.contains(…)` 只问「臂内某处有没有这句话」。实测：把本分支的推送**整个删掉**
///     （⇒ 弹窗永远停在 progress 转圈，正是本门自称要防的那件事）、把它挪进下面的
///     `Renegotiate` 分支 ⇒ **全仓 1428 passed / 0 failed，本门全绿**。
///  3. **分支**（现判据）：用**位置区间**钉死 —— 那两发推送的 offset 必须落在
///     「`let Some(info) = data.get("updateInfo")` 这一行」与「本分支自己的
///     `"hasUpdate": false` 早退」之间。同款先例见本文件
///     [`the_user_action_forces_the_popup_into_progress_before_rechecking`] 的 `force_at < check_at`。
///
/// 「我要判的是什么位置的什么形状」：形状 = 那两发推送，位置 = **那一条 `let-else` 的 else 体内**。
/// 计数不是位置（把推送搬进 helper 里计数纹丝不动），故这里判的是区间不是次数；但两个锚点
/// 各自的**唯一性**要单独断言，否则「区间」本身可以被第二处同名锚点重新划定。
///
/// 负向断言（不得广播 `ProgressStage::Downloaded`）**故意保持全函数**：任何一条臂都不许广播，
/// 那是更强的形态，与 `recheck_failures_settle_the_popup_without_broadcasting` 同一取向。
/// `done(` 的负向断言保持**臂级**：本臂任何一条分支都不许推 done，同样严于分支级。
///
/// **变异探针**：把 `no_update(...)` 改回 `done(popup.version.clone(), "")` ⇒ 转红；
/// 把两发推送挪进 `Renegotiate` 分支 ⇒ 区间判定转红（臂级判据在此**全绿**，见上面第 2 条）；
/// 把它挪进 `ViewLog` 臂 ⇒ 臂内找不到，转红。
#[cfg(target_os = "linux")]
#[test]
fn the_no_download_path_never_claims_a_download() {
    let body =
        crate::commands::guard_scan::top_level_fn_body(src(), "pub async fn update_popup_action(");
    let arm = crate::commands::guard_scan::match_arm_body(
        &body,
        "PopupAction::Update | PopupAction::Retry =>",
        "PopupAction::",
    );

    // ── 区间的两个端点：先证明它们各自唯一，否则「落在区间内」可以被第二处锚点重新划定 ──
    const BRANCH_HEAD: &str = "let Some(info) = data.get(\"updateInfo\")";
    const BRANCH_EXIT: &str = "\"hasUpdate\": false";
    // 🟡 #3（confirm 轮登记）：`BRANCH_EXIT` 曾要求**全臂恰 1**——正当的分支细分（把 no-update
    // 拆成「平台不受支持」与「其它」，两边都各自 `hasUpdate:false` 早退）会误红。安全方向，
    // 但留给后人的最省事修法就是把判据放宽跑 —— 这里先行改到**不随细分复制**的形态：
    // 头锚保持恰 1（let-else 那一行不该重样）；尾锚取**最后一次**出现且至少一次，区间 =
    // 头..最后一次。细分出的每一条子分支都被区间罩住，推送落在任一条里都判绿。
    // ⚠️ 残余缺口如实登记（相对「恰 1」版弱了一格，复审 F3 校正口径）：推送若被挪进
    // `updateInfo` **存在**的路径、且其后还存在更晚的 `hasUpdate:false`，旧判据会红、新判据
    // 不红。**没有自动姊妹门覆盖这个缺口**（本臂三道门都不钉「no_update 推送必须在该
    // 分支」），暴露途径只剩 code review 与真机（转圈窗）；补门方向 = 对 else 块子区间
    // 逐分支计数 no_update 推送 ≥1——真被咬到再做。
    // 锚点先收集后断言（🟢#7/F4 的修法，消灭 n=0 误归因与两条不可达 expect）。
    let heads: Vec<usize> = arm.match_indices(BRANCH_HEAD).map(|(i, _)| i).collect();
    assert!(
        !heads.is_empty(),
        "头锚 {BRANCH_HEAD:?} 在本臂里一次都没有 —— let-else 的形状变了，守卫失去判据"
    );
    assert_eq!(
        heads.len(),
        1,
        "头锚 {BRANCH_HEAD:?} 在本臂里出现 {} 处 —— let-else 那一行被复制了，\
             「落在分支内」这句话就不再指同一段代码",
        heads.len()
    );
    let head = heads[0];
    let exits: Vec<usize> = arm.match_indices(BRANCH_EXIT).map(|(i, _)| i).collect();
    assert!(
        !exits.is_empty(),
        "尾锚 {BRANCH_EXIT:?} 在本臂里一次都没有 —— no-update 早退的形状变了，守卫失去判据"
    );
    let exit = *exits.last().unwrap();

    // ── 那两发推送必须落在这条 `let-else` 的 else 体内 ──
    for (needle, why) in [
            (
                "push_popup_state(&app, UpdatePopupState::no_update(",
                "「没有可下的东西」仍须把弹窗推出 progress（否则永远转圈），且必须推 `no_update` 那一档",
            ),
            (
                "schedule_popup_auto_close(&app, NO_UPDATE_AUTO_CLOSE_MS)",
                "`noupdate` 终态没有排自动关窗，或沿用了 done 的 800ms —— 后者一闪而过，等于没说",
            ),
        ] {
            // 🔴 唯一性断言（confirm 轮 #2 的修法）：`top_level_fn_body` 只剥**整行**注释
            // （射程在 `commands.rs:47-50` 登记），**行尾注释携带同形串可以喂饱 `find`**。
            // 剥行尾注释要先认字符串字面量（`commands.rs:51-53` 既有裁决：不划算），故两层兜：
            //   ① `count == 1`：真调用 + 注释各一份 ⇒ 计数 2 ⇒ 红；
            //   ② 行级拒绝：命中的那一行若 needle 之前就有 `//`（= needle 落在行尾注释里）不算数
            //      ——「删掉真调用、只留一行注释」时计数仍 1，①抓不住，②让「找不到真调用」转红
            //      （confirm 轮原始收据正是这个形态）。
            // ② 的真实射程（复审 F2 校正口径，别照旧登记读反）：**未设防面 = 同形串出现在无先行
            // `//` 的任何非调用文本里**（典型：字符串字面量 `let s = "push_popup_state(…"` 且真调用
            // 已删 ⇒ ①计数 1、②当真 ⇒ 假绿——要防得认字符串=parser，不做）；串内带 `//` 且在
            // 同形串之前的形态反而会被 ② 拒绝转红（安全方向，panic 文案归因略偏但响亮）；真调用
            // 与行内更早的 `//` 同屏会被误拒（误红，安全）。
            let n = arm.matches(needle).count();
            assert_eq!(
                n, 1,
                "needle {needle:?} 在本臂里出现 {n} 次（须恰好 1）—— 大概率是行尾注释携带了\
                 同形串替真调用作证（`find` 命中哪一处全凭先后）：{why}"
            );
            // 命中行若在 needle 前就有 `//`，那是行尾注释不是代码——继续找代码行里的那一处。
            let mut real_at = None;
            let mut offset = 0usize;
            for line in arm.lines() {
                if line.contains(needle) {
                    let before = line.split(needle).next().unwrap_or("");
                    if !before.contains("//") {
                        real_at = Some(offset + before.len());
                        break;
                    }
                }
                offset += line.len() + 1;
            }
            let at = real_at.unwrap_or_else(|| {
                panic!("{why}（本臂里只剩行尾注释在携带这个串 —— 真调用没了，注释在替它作证）")
            });
            assert!(
                head < at && at < exit,
                "{why}。实得 offset：分支头={head} / 本句={at} / 分支早退={exit} —— \
                 它不在「复查回来没有 updateInfo」那条分支里。搬到臂内别的分支上，\
                 这条路径就一发都不推，而弹窗停在 progress 永远转圈"
            );
        }

    assert!(
        !arm.contains("UpdatePopupState::done("),
        "本臂一个字节都没下 —— 推 `done` 就是在发起本次动作的那一屏上谎称「下载完成」\
             （2026-08-17 前的现状）。落位路径是 `done` 的必填参数，本臂拿不出真的那一个"
    );
    assert!(
        !body.contains("ProgressStage::Downloaded"),
        "「没有可下的东西」不得广播 downloaded —— 无文件、无 filePath，设置页会显示假的「已下载」"
    );
}
