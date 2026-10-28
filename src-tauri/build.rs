use std::path::Path;

/// 随包 `resources/data/` 必须携带的 `.srs` 数量 = `builtin_geo_rulesets()` 的条目数。
///
/// **这里是副本，不是真值**：build script 不引 config-engine（避免为一次打包断言给 host 侧多挂一条
/// 依赖边），故数量在此硬编码。漂移由 `runtime/geo_seed.rs` 的
/// `build_rs_expected_count_matches_builtin_table` 守着——它读本文件、正则出这个常量、与
/// `builtin_geo_rulesets().len()` 对账，不一致即 gate 转红。**改表必改这里，否则测试先炸。**
const EXPECTED_SRS_COUNT: usize = 28;

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_ARCH");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // NDK r27 still defaults Rust cdylibs to 4 KB. Keep every PT_LOAD segment
        // compatible with 16 KB devices, for both debug and release builds.
        println!("cargo:rustc-link-arg-cdylib=-Wl,-z,max-page-size=16384");
        println!("cargo:rustc-link-arg-cdylib=-Wl,-z,common-page-size=16384");
    }
    export_product_name();
    embed_validation_core_source();
    embed_selected_core_identity();
    assert_bundled_geo_data();
    assert_bundled_dashboard();
    embed_windows_manifest_for_msvc();
    if is_windows_msvc() {
        // The linker input below supplies the same Common Controls manifest to
        // the app and to the library unit-test executable. Keep winres' icon
        // and version resources, but do not embed a second app manifest there.
        tauri_build::try_build(
            tauri_build::Attributes::new()
                .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
        )
        .unwrap_or_else(|error| panic!("tauri-build failed: {error:#}"));
    } else {
        tauri_build::build();
    }
}

fn desktop_platform() -> Option<&'static str> {
    match (
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        std::env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
    ) {
        (Ok("linux"), Ok("x86_64")) => Some("linux"),
        (Ok("windows"), Ok("x86_64")) => Some("win"),
        (Ok("macos"), Ok("x86_64")) => Some("mac-x64"),
        (Ok("macos"), Ok("aarch64")) => Some("mac-arm64"),
        _ => None,
    }
}

fn temporary_desktop_default() -> bool {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("core-manifest.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("committed core selection is readable"))
            .expect("committed core selection is JSON");
    desktop_platform().is_some() && manifest.get("desktopTemporary").is_some()
}

fn upstream_desktop_default() -> bool {
    if desktop_platform().is_none() {
        return false;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("src-tauri/core-manifest.json")).expect("committed selection"),
    )
    .expect("committed selection JSON");
    let Some(selection) = manifest.get("desktopRelease") else {
        return false;
    };
    assert!(
        manifest.get("desktopTemporary").is_none(),
        "exclusive desktop selection required"
    );
    let path = selection["contractPath"]
        .as_str()
        .expect("formal contract path");
    assert!(
        matches!(
            path,
            "scripts/desktop-core/release-contract.json"
                | "scripts/desktop-core/pending-release-contract.json"
        ),
        "reviewed formal contract path required"
    );
    println!("cargo:rerun-if-changed={}", root.join(path).display());
    let raw = std::fs::read(root.join(path)).expect("formal contract is readable");
    assert_eq!(
        selection["contractSha256"].as_str(),
        Some(sha256_hex(&raw).as_str()),
        "formal contract digest differs"
    );
    let contract: serde_json::Value = serde_json::from_slice(&raw).expect("formal contract JSON");
    if contract["schema"] != "polaris-upstream-native-release-consumer-v1" {
        return false;
    }
    // Runtime and Windows source identity embed this canonical committed file.
    assert_eq!(
        path, "scripts/desktop-core/release-contract.json",
        "upstream installation must use the compiled canonical contract"
    );
    true
}

// Analysis-only builds have no frozen carrier. The explicit sentinel is never a
// usable installation and must not silently select the historical formal core.
const TEMPORARY_UNAVAILABLE: &[u8] = br#"{"schema":"polaris-temporary-desktop-unavailable-v1"}"#;
const FORMAL_UNAVAILABLE: &[u8] = br#"{"schema":"polaris-formal-desktop-unavailable-v1"}"#;

fn temporary_installation_pending(temporary_default: bool) -> bool {
    if !temporary_default
        || std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_some()
        || std::env::var_os("POLARIS_CORE_IDENTITY_FILE").is_some()
    {
        return false;
    }
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root")
        .join("resources")
        .join(desktop_platform().expect("desktop target"));
    for name in ["core-manifest.json", "candidate-carrier.json"] {
        let path = directory.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        if std::fs::symlink_metadata(&path).is_ok() {
            return false;
        }
        // Only true absence admits an unavailable analysis image. Permission or
        // I/O errors, and dangling symlinks above, must enter strict verification.
        if path
            .try_exists()
            .expect("inspect selected installation path")
        {
            return false;
        }
    }
    true
}

/// Technical candidate source facts are compiled only after the existing strict
/// consumer verifies the actual payloads and exact manual App/run context. This
/// is paired with a separately verified installation identity; neither authorizes managed launch.
fn embed_validation_core_source() {
    println!("cargo:rerun-if-env-changed=POLARIS_NATIVE_DNS_CANDIDATE");
    println!("cargo:rerun-if-env-changed=CORE_CANDIDATE");
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_ID");
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_ATTEMPT");
    for key in [
        "POLARIS_NO_KERNEL_RUN",
        "POLARIS_BUILD_ID",
        "CARGO_TARGET_DIR",
        "GITHUB_ACTIONS",
        "GITHUB_EVENT_NAME",
        "GITHUB_REPOSITORY",
        "GITHUB_REPOSITORY_ID",
        "GITHUB_REF",
        "GITHUB_SHA",
        "GITHUB_WORKFLOW_REF",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join("validation-core-source.json");
    let temporary_default = temporary_desktop_default();
    let mode = std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE");
    let raw = if temporary_installation_pending(temporary_default) {
        TEMPORARY_UNAVAILABLE.to_vec()
    } else if mode.is_none() && !temporary_default {
        b"null".to_vec()
    } else {
        assert!(
            desktop_platform().is_some(),
            "desktop candidate on unsupported target"
        );
        let candidate = if let Some(mode) = mode {
            assert_eq!(mode, "1", "explicit native DNS validation mode required");
            Some(std::env::var("CORE_CANDIDATE").expect("exact candidate App SHA"))
        } else {
            None
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repository root");
        for path in [
            "scripts/desktop-core/native-dns-candidate.mjs",
            "scripts/desktop-core/native-dns-candidate.json",
            "scripts/desktop-core/native-dns-source-binding.json",
            "resources",
            "scripts/desktop-core",
            "scripts/prepare-core-identity.mjs",
            "scripts/lib/core-identity.mjs",
        ] {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
        let mut command = std::process::Command::new("node");
        command
            .arg(root.join("scripts/desktop-core/native-dns-candidate.mjs"))
            .arg("source");
        if let Some(candidate) = &candidate {
            command.arg(format!("--candidate={candidate}"));
        }
        let verified = command
            .current_dir(root)
            .output()
            .expect("strict candidate source verifier must execute");
        assert!(
            verified.status.success(),
            "candidate source verification failed: {}",
            String::from_utf8_lossy(&verified.stderr)
        );
        let source: serde_json::Value =
            serde_json::from_slice(&verified.stdout).expect("verified candidate source JSON");
        if let Some(candidate) = candidate {
            assert_eq!(source["schema"], "polaris-native-dns-compiled-source-v1");
            assert_eq!(source["validationOnly"], true);
            assert_eq!(source["context"]["candidate"], candidate);
        } else {
            assert_eq!(source["schema"], "polaris-native-dns-compiled-source-v2");
            assert_eq!(source["validationOnly"], false);
            assert_eq!(source["purpose"], "temporary-default");
            assert_eq!(source["context"]["mode"], "temporary-default");
            assert_eq!(
                source["context"]["candidate"],
                std::env::var("POLARIS_BUILD_ID").expect("exact build identity")
            );
        }
        assert_eq!(source["AppAccepted"], false);
        assert_eq!(source["publicationEligible"], false);
        verified.stdout
    };
    std::fs::write(output, raw).expect("write compiled validation source identity");
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let mut hex = String::with_capacity(64);
    for byte in sha2::Sha256::digest(bytes) {
        write!(&mut hex, "{byte:02x}").expect("write digest to String");
    }
    hex
}

fn embed_selected_core_identity() {
    println!("cargo:rerun-if-env-changed=POLARIS_CORE_IDENTITY_FILE");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join("selected-core-manifest.json");
    let temporary_default = temporary_desktop_default();
    let formal_default = upstream_desktop_default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root");
    if temporary_installation_pending(temporary_default || formal_default) {
        std::fs::write(
            output,
            if formal_default {
                FORMAL_UNAVAILABLE
            } else {
                TEMPORARY_UNAVAILABLE
            },
        )
        .expect("write unavailable installation identity");
        return;
    }
    let selected_path = std::env::var_os("POLARIS_CORE_IDENTITY_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            (temporary_default || formal_default).then(|| {
                root.join("resources")
                    .join(desktop_platform().expect("desktop target"))
                    .join("core-manifest.json")
            })
        });
    let raw = match selected_path {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.display());
            let raw = std::fs::read(&path).expect("selected installation manifest is readable");
            let manifest =
                polaris_helper_proto::core_manifest::InstalledCoreManifest::parse(&raw, sha256_hex)
                    .expect("selected installation manifest has an exact content identity");
            let os = std::env::var("CARGO_CFG_TARGET_OS").expect("target OS");
            let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("target architecture");
            let platform = match (os.as_str(), arch.as_str()) {
                ("linux", "x86_64") => "linux",
                ("windows", "x86_64") => "win",
                ("macos", "x86_64") => "mac-x64",
                ("macos", "aarch64") => "mac-arm64",
                _ => panic!("selected desktop identity on unsupported target"),
            };
            if !temporary_default && std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_none() {
                let candidate_fingerprint = sha256_hex(include_bytes!(
                    "../scripts/desktop-core/native-dns-candidate.json"
                ));
                assert_ne!(
                    manifest.core.provider_fingerprint, candidate_fingerprint,
                    "candidate installation cannot be compiled as a formal/default identity"
                );
            }
            if temporary_default || std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_some() {
                let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .expect("repository root");
                let manual = std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_some();
                let directory = root
                    .join(if manual {
                        "resources/.native-dns-app"
                    } else {
                        "resources"
                    })
                    .join(platform);
                assert_eq!(
                    manifest.core.provider_fingerprint,
                    sha256_hex(include_bytes!(
                        "../scripts/desktop-core/native-dns-candidate.json"
                    )),
                    "selected temporary contract fingerprint differs"
                );
                assert_eq!(
                    path,
                    directory.join("core-manifest.json"),
                    "exact candidate identity path required"
                );
                let verified = std::process::Command::new("node")
                    .arg(root.join("scripts/prepare-core-identity.mjs"))
                    .arg(format!("--platform={platform}"))
                    .arg(format!("--directory={}", directory.display()))
                    .arg("--mode=verify")
                    .current_dir(root)
                    .output()
                    .expect("strict candidate App carrier verifier must execute");
                assert!(
                    verified.status.success(),
                    "candidate App identity verification failed: {}",
                    String::from_utf8_lossy(&verified.stderr)
                );
                assert_eq!(
                    raw,
                    std::fs::read(&path).expect("verified identity remains readable"),
                    "candidate identity changed during verification"
                );
            }
            if formal_default && std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_none() {
                let directory = root.join("resources").join(platform);
                assert_eq!(
                    path,
                    directory.join("core-manifest.json"),
                    "exact formal identity path required"
                );
                let verified = std::process::Command::new("node")
                    .arg(root.join("scripts/prepare-core-identity.mjs"))
                    .arg(format!("--platform={platform}"))
                    .arg(format!("--directory={}", directory.display()))
                    .arg("--mode=verify")
                    .current_dir(root)
                    .output()
                    .expect("strict formal App carrier verifier must execute");
                assert!(
                    verified.status.success(),
                    "formal App identity verification failed: {}",
                    String::from_utf8_lossy(&verified.stderr)
                );
                assert_eq!(
                    raw,
                    std::fs::read(&path).expect("verified formal identity readable"),
                    "formal identity changed during verification"
                );
            }
            assert_eq!(
                manifest.core.platform, platform,
                "selected core target differs"
            );
            assert!(
                manifest.core.helper_sha256.is_some(),
                "actual helper hash required"
            );
            if platform == "win" {
                assert!(
                    manifest.core.cleaner_sha256.is_some(),
                    "actual cleaner hash required"
                );
            }
            raw
        }
        // Library tests/mobile/debug source compilation do not create a desktop
        // installer. Actual installers always supply this frozen input in package.yml.
        None => {
            assert!(
                std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_none(),
                "candidate App requires a frozen actual helper/core installation identity"
            );
            b"null".to_vec()
        }
    };
    std::fs::write(output, raw).expect("write compiled selected core identity");
}

fn is_windows_msvc() -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
}

/// **把 `productName` 注入编译期** —— Linux deb/AppImage 的 FHS 资源目录名
/// （`/usr/lib/<productName>/_up_/resources`）在 Rust 侧的唯一真值来源。
///
/// 为什么要有：这个事实此前存**两份** —— `tauri.conf.json` 的 `productName`，与
/// `runtime/proxy/core_binary.rs` 里的 `LINUX_BUNDLE_PRODUCT_DIR` 字面量。因为存了两份，才需要
/// `verify-packaging.mjs confs` 拿正则去 Rust 源码里抓那个常量再跟 JSON 对拍；而那道对拍门
/// 把**整棵 `src-tauri/src/runtime/`** 变成了打包判据面（每个碰它的 PR 多跑一条 linux 打包腿），
/// 正则又硬锚在 `proxy.rs` 这一个文件上（该文件一拆就失锚）。注入之后两份塌成一份，
/// 对拍门连同它的成本一起没有存在意义。
///
/// **只读 base conf**：四份 per-platform conf 的顶层被 `verify-packaging.mjs confs` 钉死为
/// 只准 `$schema` + `bundle`（用 `--config` 按 RFC 7396 覆盖 `productName` 会当场转红，
/// 变异 M10 实测过），故 base 的这个键就是唯一来源，不必再合并四份。
///
/// **读不到 / 键缺失 / 不是合规字符串一律 panic**，不给 `unwrap_or("Polaris")` 之类的兜底：
/// 兜底的后果是构建照样成功、Linux 包的资源目录名却错 —— 运行期 core/helper/geo/dashboard
/// 四类消费者一起报「未找到」，正是本次要消灭的那类静默失效。
fn export_product_name() {
    let conf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    // 本 build.rs 已有显式 rerun-if-changed 声明 ⇒ cargo 只按声明路径重跑：漏了这条，改
    // productName 不会重编，注入值静默陈旧 —— 那又是一份「悄悄漂移的第二真值」，
    // 不能在消灭它的同时重新造一个。
    println!("cargo:rerun-if-changed={}", conf.display());

    let raw = std::fs::read_to_string(&conf).unwrap_or_else(|e| {
        panic!(
            "读不到 {}（{e}）—— productName 无真值来源，拒绝继续构建",
            conf.display()
        )
    });
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} 不是合法 JSON（{e}）", conf.display()));
    // 非空 + 不含换行：值要原样拼进 `cargo:rustc-env=` 这一行，换行会把后半截当成另一条
    // build 指令，是「构建成功但注入值不是你写的那个」的另一种形态。
    let name = match parsed.get("productName") {
        Some(serde_json::Value::String(s)) if !s.is_empty() && !s.contains(['\n', '\r']) => s,
        other => panic!(
            "{}: productName 必须是非空且不含换行的字符串，实为 {other:?} —— 拒绝取默认值继续：\n\
             注入值错会让 Linux 包的 /usr/lib/<productName>/ 资源目录名错，构建仍会成功，\n\
             运行期 core/helper/geo/dashboard 一起判成缺失。",
            conf.display()
        ),
    };
    println!("cargo:rustc-env=POLARIS_PRODUCT_NAME={name}");
}

/// W9：给 Windows MSVC 的可执行目标嵌入 Common-Controls v6 manifest。
///
/// 病理（run 32109642349 探针实证）：`tests/remote_webview_cannot_reach_app_commands` 的
/// 测试二进制导入 `comctl32.dll` 的 `TaskDialogIndirect / SetWindowSubclass /
/// RemoveWindowSubclass / DefSubclassProc`——这四个是 **v6 专属导出**。应用 exe 由
/// tauri-build 的 winres 只把默认 manifest 给应用 bin，**测试 exe 拿不到**，
/// 于是加载器把 comctl32 绑到 System32 的 v5.82 ⇒ 加载期
/// `STATUS_ENTRYPOINT_NOT_FOUND`（0xC0000139），一条测试都没执行——W9 登记的
/// 「零结论不是绿」的根因。
///
/// `cargo:rustc-link-arg-tests` 只覆盖独立的 `tests/*.rs` 目标，**不覆盖**
/// `cargo test --lib` 的 `src/lib.rs --test` 可执行目标；前者全绿、后者加载前退出。
/// `cargo:rustc-link-arg` 覆盖二者及应用 bin。MSVC 应用 bin 由上方
/// `new_without_app_manifest` 避免 winres 重复嵌 manifest。
/// manifest 内容与 tauri-build 2.6.3 自带的 `windows-app-manifest.xml` 等义（多一个可选
/// XML 声明头，无语义影响；Common-Controls 6.0.0.0 依赖），刻意不引它的私有路径。
fn embed_windows_manifest_for_msvc() {
    if !is_windows_msvc() {
        return;
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-test-manifest.xml");
    // 本 build.rs 已有显式 rerun-if-changed 声明 ⇒ cargo 只按声明路径重跑：manifest 编辑
    // 必须自己声明，否则测试二进制静默沿用旧嵌入内容（复审 F1）。
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}

/// **桌面随包 dashboard 完整性断言（打包期硬门）**。
///
/// 为什么从 `beforeBundleCommand` 搬到这里（2026-08-05，Windows 打包腿首次真跑时挂在那个 hook 上）：
///
/// Tauri 跑 hook 的 shell 是**按宿主平台**选的：Unix 走 `sh -c`，Windows 走 `cmd /C`。原 hook 是
///   `if [ -f scripts/ensure-dashboard.sh ]; then sh …; else sh ../scripts/…; fi`
/// —— 纯 sh 语法，在 linux/macOS 腿上成立，到 Windows 交给 `cmd` 必然 exit 1（实测 run
/// 30983415697：`beforeBundleCommand … failed with exit code 1`，且它挂在**整条 Rust 编译之后**，
/// 每验证一轮先烧 20 分钟 × 2x 计费）。
///
/// 那个 `if` 本身是为了绕开「hook 的 cwd 到底是仓库根还是 src-tauri」这个不确定性
/// （2026-07-20 的实测结论在 CLI 2.11.4 上已不成立，见 `scripts/ensure-dashboard.sh` 头注）。
///
/// 搬到 build script 后两个不确定性一起消失：**没有 shell**（不必同时满足 sh 与 cmd 两套语法），
/// **cwd 由 cargo 保证**（`CARGO_MANIFEST_DIR` 是编译期常量，与调用方 cwd 无关）。
///
/// [不选「per-platform conf 里给 Windows 覆盖一份 hook」：`verify-packaging.mjs` 的 conf 不变量
///  明写四份 per-platform conf **顶层只准 `$schema` + `bundle`、`bundle` 下只准 `resources`**
///  （变异 M10 实测过，防的是 `--config` 按 RFC 7396 覆盖 base 的 version/productName），
///  加 `build.beforeBundleCommand` 会撞这道门，而门槛不为了让改动通过而放宽]
/// [不选「hook 改成 `node scripts/…`」：打包机 mac 5.238 上 node **装了但不在 PATH 上** ——
///  它在 `/opt/homebrew/bin`，而该目录不在 ssh 会话（含 `bash -lc` 登录 shell）的 PATH 里。
///  见 `polaris-mac-deploy-recipe.md`「5.238 工具链状态（2026-07-20 订正）」：把这个现象误读成
///  「没装 Node」曾造成两轮部署失败 + 一次白编译。hook 由 tauri-cli 在**它自己的进程环境**下拉起，
///  拿到的就是那份缺目录的 PATH ⇒ `node` 仍会 not found。build script 不依赖任何解释器，绕开整类问题]
///
/// 判据用 **非空**而不是「文件存在」：0 字节 / 404 HTML 占位文件打进包里，与没有它同一后果 ——
/// 核只能回落联网下载面板（离线不可用；CWD 只读时还会刷 mkdir 报错）。
///
/// **只在 release 生效**。Android 的目标配置只打包 geo 和许可文本，不携带桌面 dashboard；
/// build script 在宿主执行，因此必须读取 Cargo 的 target OS，不能使用宿主 `cfg!`。
///
/// ⚠️ **不再顺手拉取**。原 hook 在有 node 时会替你跑一次 `fetch-dashboard.mjs`；build script 里不做
/// 这件事（build script 不该联网，且它在 sandbox / 离线构建下会变成硬失败）。CI 侧本来就有独立的
/// `Fetch sing-box dashboard` 步，不受影响；本机构建按下面 panic 文案里给的命令手动拉一次即可。
fn assert_bundled_dashboard() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        return;
    }
    let index = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("resources")
        .join("dashboard")
        .join("index.html");
    println!("cargo:rerun-if-changed={}", index.display());

    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }

    let size = std::fs::metadata(&index).map(|m| m.len()).unwrap_or(0);
    assert!(
        size > 0,
        "随包 dashboard 资源缺失或为 0 字节，拒绝出包：{}\n\
         后果：面板资源为空仍会被 bundler 原样打进安装包，核只能回落联网下载面板 —— \
         离线不可用，且 CWD 只读时还会刷 mkdir 报错。\n\
         修复（有 Node 的机器）：node scripts/fetch-dashboard.mjs\n\
         修复（无 Node 的打包机）：先在有 Node 的机器跑上面那条，再把 resources/dashboard/ rsync 过来。",
        index.display()
    );
}

/// **随包 geo 资源完整性断言（打包期硬门）**。
///
/// 为什么要有：`resources/data/` 的 28 个 `.srs` 是「智能分流能不能工作」的物理前提——缺了它们
/// `runtime_rules_dir` 种不满 → route builder fail-closed 剪掉全部 geo 规则 → 叠加回国模式即
/// **全量明文直连**（真机 2026-07-20）。此前唯一盯着这件事的是 `geo_seed.rs` 的 cargo test，而它读
/// **工作树**：本机工作树里有、但漏 `git add` 时本机 gate 照样全绿，只有 CI 干净 clone 才转红；
/// 而打包机拿的是工作树 rsync，于是「随包资源缺失」这条腿在真正出包的那台机器上**没有门**。
///
/// 另一条随包资源链（dashboard）的对等断言是本文件的 [`assert_bundled_dashboard`]
/// （2026-08-05 从 `beforeBundleCommand` 搬来，理由见那个函数的文档）。两条链现在守门强度一致、
/// 机制同源：同一个 build script、同样 release-only、同样按「内容有效」而非「文件存在」判定。
///
/// 校验用 **SRS 魔数**而非「文件存在」：0 字节 / 404 HTML 污染的占位文件打进包里，与没有它是同一个后果
/// （route builder 的 `is_valid_srs_fn` 判无效 → 照剪）。判据与运行时那侧逐字节同源。
///
/// **只在 release 生效**：debug（开发 / CI 单测）不阻断——那里由 `geo_seed.rs` 的
/// `real_bundled_resources_cover_every_builtin_tag` 按真值表逐 tag 覆盖。release ⟺ 会被打包分发的那份。
fn assert_bundled_geo_data() {
    let data_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("resources")
        .join("data");
    println!("cargo:rerun-if-changed={}", data_dir.display());

    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }

    let mut valid = 0usize;
    let mut bad: Vec<String> = Vec::new();
    match std::fs::read_dir(&data_dir) {
        Ok(entries) => {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".srs") {
                    continue;
                }
                if has_srs_magic(&e.path()) {
                    valid += 1;
                } else {
                    bad.push(name);
                }
            }
        }
        Err(e) => panic!(
            "随包 geo 资源目录读不到，拒绝出包：{} ({e})\n\
             后果：runtime rules 目录种不满 → route builder fail-closed 剪掉全部 geo 分流规则；\
             叠加回国模式即全量明文直连。",
            data_dir.display()
        ),
    }

    assert!(
        bad.is_empty(),
        "随包 geo 资源魔数校验失败（0 字节 / 下载污染），拒绝出包：{}\n目录：{}",
        bad.join(", "),
        data_dir.display()
    );
    assert!(
        valid >= EXPECTED_SRS_COUNT,
        "随包 geo 资源不足：{valid} / {EXPECTED_SRS_COUNT} 个有效 .srs，拒绝出包。\n\
         目录：{}\n\
         后果：runtime rules 目录种不满 → route builder fail-closed 剪掉引用它们的分流规则；\
         叠加回国模式即全量明文直连（真机 2026-07-20）。\n\
         修复：确认 resources/data/ 已随仓库提交（git status 查 untracked），\
         或从有完整资源的机器 rsync 该目录后重打包。",
        data_dir.display()
    );
}

/// SRS 魔数（`'S' 'R' 'S'`）判定。与 `config_engine::user_config::builtin_geo_rulesets::is_valid_srs_file`
/// 同判据——build script 不引 config-engine，故此处是 3 字节的独立实现（不值得为它加一条依赖边）。
fn has_srs_magic(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 3];
    f.read_exact(&mut buf).is_ok() && &buf == b"SRS"
}
