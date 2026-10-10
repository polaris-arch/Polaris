//! 核二进制解析 owner：随包资源候选布局、开发树 manifest 目录、现役核解析、
//! sing-box 官方面板 serve 目录。
//!
//! 纯自由函数，零 [`super::ProxyRuntime`] 状态依赖（L0，`proxy` 依赖拓扑的叶）。符号被
//! `proxy` 外部消费（`speedtest.rs` / `tailscale_login_core.rs` / `geo_seed.rs` /
//! `commands/misc/dashboard.rs` / `lib.rs`），façade 必须 `pub(crate) use` 再导出。

use std::path::{Path, PathBuf};

use super::platform_contracts::core_platform_dirs;

/// Linux deb/AppImage 的 FHS 资源目录名 —— 就是 `tauri.conf.json` 的 `productName`，
/// 由 `src-tauri/build.rs::export_product_name` 用 `cargo:rustc-env` 在编译期注入。
///
/// **这里刻意不再存第二份字面量**：两份存在过（本常量 + conf），于是需要
/// `verify-packaging.mjs confs` 拿正则去本文件里抓常量再跟 JSON 对拍 —— 代价是整棵
/// `src-tauri/src/runtime/` 变成打包判据面，且正则硬锚在本文件上（拆分即失锚）。
/// 塌成一份后那道门已删除，仅存的保险是 `injected_product_name_matches_tauri_conf`
/// （对着 conf 逐字核注入值，改 conf 不改注入链即转红）。
pub(super) const LINUX_BUNDLE_PRODUCT_DIR: &str = env!("POLARIS_PRODUCT_NAME");

/// bundled 资源二进制候选路径（sing-box 核 / polaris-helper 共用）。抽纯函数便于**钉 `_up_` 布局回归**。
///
/// 布局兜底顺序：① **`exe/_up_/resources/`（Windows NSIS 装机权威布局）**：tauri-utils 的
/// `resource_relpath` 把 `../` 段改名 `_up_`，NSIS 装机后资源在 `<exe目录>\_up_\resources\`——W10 根因，
/// 漏掉则装机态核/helper 解析双双落空（2026-08-19 真机 toast 首曝）。②
/// **`exe/../Resources/_up_/resources/`（macOS .app 权威布局）**：同一 `_up_` 改名机制在 .app 里
/// 实际落 `Contents/Resources/_up_/resources/`，漏掉则打包态 mac 上核/helper 恒找不到。③
/// **`usr/bin/../lib/Polaris/_up_/resources/`（Linux deb/AppImage 权威布局）**：Tauri 的 Debian/AppImage
/// 数据树把应用资源落在 `/usr/lib/<productName>/`，可执行文件则在 `/usr/bin/`；只从 exe 同级找会得到
/// 「包里明明有核/规则，运行期却全部报未找到」的假坏包。④ exe 同级 `resources/`（portable 权威布局 /
/// NSIS legacy 兜底）。⑤ `exe/../Resources/resources/`（mac legacy 兜底）。④⑤必须排在三条权威布局
/// **之后**：安装升级不会删除旧版遗留目录，反过来会让新 app 静默首选旧 core/helper
/// （2026-08-23 `.207` 真机实证）。⑥ `CARGO_MANIFEST_DIR/../resources/`（开发态）。
///
/// `exe_dir` = `current_exe().parent()`（None=取不到）；`manifest_dir` = `CARGO_MANIFEST_DIR`。
pub(crate) fn bundle_resource_candidates(
    exe_dir: Option<&std::path::Path>,
    dev_manifest_dir: Option<&std::path::Path>,
    platform_dirs: &[&str],
    filename: &str,
) -> Vec<PathBuf> {
    let prefixes = bundle_resource_roots(exe_dir, dev_manifest_dir);

    let mut candidates: Vec<PathBuf> = Vec::new();
    for prefix in &prefixes {
        for pdir in platform_dirs {
            candidates.push(prefix.join(pdir).join(filename));
        }
    }
    candidates
}

/// [`bundle_resource_candidates`] 的**前缀腿**：随包资源目录本身（不含平台子目录与文件名）。
fn bundle_resource_roots(
    exe_dir: Option<&std::path::Path>,
    dev_manifest_dir: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe_dir {
        // Windows NSIS 装机布局（W10 根因，2026-08-19 真机 toast 首曝）：tauri-utils 的
        // `resource_relpath` 把 `../` 段改名 `_up_`（与 bundler 无关，NSIS 同样生效），装机后资源
        // 在 `<exe目录>\_up_\resources\`——此前候选表只有 mac 的一种 `_up_` 形态（`../Resources/
        // _up_/resources`），Windows 装机态的核 / helper 解析双双落空（helper 安装 toast「未找到
        // polaris-helper 二进制」，核解析同函数同病）。它必须排在裸 `resources/` 前：后者是 portable
        // 权威布局，但在 NSIS 装机态也可能是升级前残件；裸目录先行会把新包静默降级成旧 payload。
        prefixes.push(dir.join("_up_").join("resources"));
        prefixes.push(
            dir.join("..")
                .join("Resources")
                .join("_up_")
                .join("resources"),
        );
        // Linux deb / AppImage 的 Tauri 权威布局：exe 在 `<root>/usr/bin`，资源在
        // `<root>/usr/lib/Polaris/_up_/resources`。只对组件后缀恰为 `usr/bin` 的路径加这条，避免
        // Windows/macOS 的失败文案里混入不属于它们的 FHS 猜测路径。
        if dir.ends_with(Path::new("usr").join("bin")) {
            prefixes.push(
                dir.join("..")
                    .join("lib")
                    .join(LINUX_BUNDLE_PRODUCT_DIR)
                    .join("_up_")
                    .join("resources"),
            );
        }
        prefixes.push(dir.join("resources"));
        prefixes.push(dir.join("..").join("Resources").join("resources"));
    }
    // 开发树候选：**release 里恒不存在**（`dev_manifest_dir()` 返 `None`），见其文档。
    if let Some(manifest_dir) = dev_manifest_dir {
        prefixes.push(manifest_dir.join("..").join("resources"));
    }
    prefixes
}

/// 开发树的 crate 根（`CARGO_MANIFEST_DIR`）。**release 构建里恒为 `None`**。
///
/// # 为什么是 `#[cfg]` 而不是 `cfg!()`
///
/// `cfg!(debug_assertions)` 是**运行期**布尔，两条腿都会被编译 ⇒ `env!("CARGO_MANIFEST_DIR")`
/// 那个字面量照样进 `.rodata`。2026-08-30 对发行产物实测：`strip = "symbols"` 之后仍有
/// **143 处** `/home/sway/Code/polaris` 字样，其中开发者仓库路径正是这么泄出去的
/// （余下来自 `btls-sys` 编译 BoringSSL 时 C 编译器写进去的 `__FILE__`）。
/// `#[cfg]` 是**编译期**分叉，release 那条腿里根本没有这个 `env!`。
///
/// # 它在开发态还有用
///
/// `cargo run` / `cargo test` 时二进制在 `target/debug/`，随包资源并不在 exe 旁边，
/// 只能靠 `<crate 根>/../resources` 找到。测试构建 `debug_assertions` 恒开，故取材面不变。
#[cfg(debug_assertions)]
pub(crate) fn dev_manifest_dir() -> Option<&'static std::path::Path> {
    Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// [`dev_manifest_dir`] 的 release 腿：没有开发树，也**不留下那个字面量**。
#[cfg(not(debug_assertions))]
pub(crate) fn dev_manifest_dir() -> Option<&'static std::path::Path> {
    None
}

/// 从按权威度排序的 bundled 候选中取第一个真实文件（core / helper 共用同一选择语义）。
pub(crate) fn first_existing_bundle_candidate(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

/// 内核路径的开发态超驰。**只在 debug / test 构型编译**；release 构型里这个函数不存在，
/// 发行包对进程环境里的任何内核路径变量都不读取。
#[cfg(any(debug_assertions, test))]
/// 读 `POLARIS_SINGBOX_PATH`：命中即用，指向不存在的文件即 `Err`（不静默回落）。
///
/// 为什么 release 不保留「路径须落在可信目录内」那一档：应用应当运行与本应用版本配套的
/// 内核，而「路径在某个目录里」不等于「内容配套」。能让发行包换一个内核文件的入口，只要
/// 存在，就是绕开配套约束的入口。`tests/release_escape_hatches.rs` 钉着 release 侧零读取。
fn dev_core_binary_override() -> Result<Option<PathBuf>, String> {
    let Ok(raw) = std::env::var("POLARIS_SINGBOX_PATH") else {
        return Ok(None);
    };
    let path = PathBuf::from(raw);
    if path.is_file() {
        Ok(Some(path))
    } else {
        Err(format!(
            "POLARIS_SINGBOX_PATH 指向的文件不存在：{}",
            path.display()
        ))
    }
}

/// 应用侧的内核解析：只认随包核（[`resolve_bundled_core_binary`]）。
///
/// debug / test 构型在它之前多一级开发态超驰（见 `dev_core_binary_override`，release 不编译）。
/// 用户配置目录、`PATH`、配置字段都不参与解析。本函数给出的是路径；那个路径上的文件是否
/// 仍是出厂内容、经提权助手运行时实际执行的是哪一份，不由本函数保证。
///
/// 找不到 → Err（**不静默回落 PATH**：误起系统里别的 sing-box 比起不来更糟）。
pub(crate) fn resolve_core_binary() -> Result<PathBuf, String> {
    #[cfg(any(debug_assertions, test))]
    {
        if let Some(core) = dev_core_binary_override()? {
            return Ok(core);
        }
    }
    resolve_bundled_core_binary()
}

/// 随包核：只解析打进安装包的资源目录。
fn resolve_bundled_core_binary() -> Result<PathBuf, String> {
    let filename = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };
    let platform_dirs = core_platform_dirs(std::env::consts::OS, std::env::consts::ARCH);

    let exe = std::env::current_exe().ok();
    let candidates = bundle_resource_candidates(
        exe.as_deref().and_then(std::path::Path::parent),
        dev_manifest_dir(),
        &platform_dirs,
        filename,
    );
    if let Some(core) = first_existing_bundle_candidate(&candidates) {
        return Ok(core);
    }
    Err(format!(
        "未找到 sing-box 二进制（尝试过：{}）。开发态请先跑 `node scripts/fetch-core.mjs`。",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    ))
}

/// 内核文件是否可执行（Unix：任一可执行位；其它平台没有这个概念，恒 `Ok`）。
///
/// 内核直接从安装包里执行，应用不改写安装包内的文件，所以权限位不对时这里只报告、不修复。
///
/// # Errors
///
/// 读不到文件元数据，或文件没有任何可执行位。
pub(crate) fn ensure_core_executable(core: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(core)
            .map_err(|e| format!("读取内核文件属性失败 {}: {e}", core.display()))?;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "安装包内的内核文件没有可执行权限，无法启动：{}。请重新安装应用。",
                core.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = core;
    Ok(())
}

/// sing-box 官方面板运行时下载覆盖目录名（`<config_dir>/singbox-dashboard`）。
/// 与 `commands/misc.rs` 的 `SINGBOX_DASHBOARD_DIR` 同名同义：「刷新面板资源」清此目录 → 核下次启动回落随包内置。
const SINGBOX_DASHBOARD_DIR_NAME: &str = "singbox-dashboard";

/// 解析 sing-box 官方面板 `services[].dashboard.path`（对齐 上游 `resolveDashboardServeDir`）。
///
/// 优先级：**运行时下载覆盖**（`<config_dir>/singbox-dashboard` 含 `index.html`）→ **随包内置**
/// （`resources/dashboard/index.html`，`scripts/fetch-dashboard.mjs` 落地、tauri.conf `resources` 打包）→
/// 两者皆无返 `None`。
///
/// `None` 时 config-engine 省略 `path` → 核回落**联网下载**兜底（保「异常打包不 brick」）；该下载会在进程 CWD 下
/// 相对 mkdir `dashboard`，故必须配合起核 `.current_dir(<可写目录>)`（见 spawner `working_dir` / helper spawn）
/// 避免 CWD=`/` 下的只读 mkdir 噪音。命中（有 `path`）时核直接 serve 本地文件、**零联网下载、打开即时离线可用**
/// ——根治噪音的首选路径。
pub(crate) fn resolve_dashboard_serve_dir(config_dir: &std::path::Path) -> Option<String> {
    // 1) 运行时下载覆盖优先。
    let override_dir = config_dir.join(SINGBOX_DASHBOARD_DIR_NAME);
    if override_dir.join("index.html").is_file() {
        return Some(override_dir.to_string_lossy().into_owned());
    }
    // 2) 随包内置 resources/dashboard（非平台特定 → 借 bundle_resource_candidates 以 "dashboard" 作子目录、
    //    "index.html" 作探针；命中即取其父目录 = serve 根）。
    let exe = std::env::current_exe().ok();
    bundle_resource_candidates(
        exe.as_deref().and_then(std::path::Path::parent),
        dev_manifest_dir(),
        &["dashboard"],
        "index.html",
    )
    .iter()
    .find(|c| c.is_file())
    .and_then(|c| c.parent())
    .map(|p| p.to_string_lossy().into_owned())
}
