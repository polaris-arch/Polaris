//! App 自更新的**安装计划决策 + 平台脚本生成**（移植 上游 `update-install-script.ts`）。
//!
//! # 结构纪律：决策与执行切开
//!
//! 本模块 99% 是**纯函数**（形态判定 / 计划决策 / 脚本文本生成），可在本机零副作用单测；
//! 真正的执行腿只有一个 [`spawn_detached_script`]（写脚本 + `spawn(detached)`），且**本机绝不跑**
//! （改写宿主应用本体 + 需提权，属真机门）。
//!
//! # 为什么是「自写脚本」而不是 updater 框架
//!
//! 上游 全程**不用任何 updater 框架、不用任何应用级签名密钥对**（`UpdateService.ts` 1212 行手写）。
//! Polaris 同理：不引 `tauri-plugin-updater`，因而**不需要 minisign 密钥对**。真伪由 OS 层负责
//! （见下方「代码签名：ad-hoc 的后果」）。
//!
//! # 代码签名：ad-hoc 的后果（**用户已拍板走 ad-hoc，不买证书**）
//!
//! 没有 Developer ID / Authenticode ⇒ OS 层的「真伪」校验会拦下来。两平台表现与对策：
//!
//! | 平台 | 症状 | 应用内能否消除 | 本模块的处置 |
//! |---|---|---|---|
//! | macOS | 下载件带 `com.apple.quarantine` → Gatekeeper「来自身份不明的开发者」，**装完打不开** | ✅ **能** | 安装脚本在替换成功后**必跑** `xattr -dr com.apple.quarantine`（对齐 上游 `update-install-script.ts:259`）+ 验证随包签名且保留冻结字节；失败保留安装件并恢复原文件 |
//! | Windows | SmartScreen「未知发布者」 | ❌ **不能**（唯一解是买证书 / 攒信誉） | 安装**前**经 [`install_advisory`] 返 [`InstallAdvisory::WindowsSmartScreen`]，UI 弹说明框告知「更多信息 → 仍要运行」的具体点法 |
//!
//! **诚实原则**：无法在应用内消除拦截的平台，**必须**在安装前把用户可执行的下一步说清楚
//! （不是装完了事）。该判定是纯函数 [`install_advisory`]，有单测 + 变异用例钉死。
//!
//! # 移植对照
//!
//! | 平台 | 上游 | 本模块 |
//! |---|---|---|
//! | Windows NSIS | `buildWindowsUpdateVbs`（UTF-16LE+BOM 的 `.vbs`，`wscript` 无窗口跑） | [`build_install_script`] → [`ScriptSpec`] |
//! | Windows 便携 | 上游便携版是单个自解压 exe，脚本把新 exe 拷进原目录 | **本仓便携版是 zip**（`Polaris_<version>_x64-win-Portable.zip`），没有脚本腿：[`InstallPlatform::WindowsPortable`] 由 command 层停核 → 打开压缩包与程序目录 → 退出，用户在应用退出后覆盖解压 |
//! | macOS | `buildMacUpdateScript`（`hdiutil attach` → `ditto` 私有暂存 → 签名核验 → 原子替换/本次失败恢复） | 同上 |
//! | Linux AppImage | `buildLinuxAppImageScript`（同目录暂存核验 + rename 替换/失败恢复） | 同上 |
//! | Linux deb | `buildLinuxDebScript`（`pkexec apt-get install`） | 同上 |
//! | Android | **上游没有这一腿**（ 上游 是桌面 Electron 应用） | [`InstallPlatform::Android`]：不生成脚本，经 FileProvider 交系统安装器（`android_bridge::hand_apk_to_system_installer`） |
//! | 形态错配 | 不强制 root，回退 `shell.openPath` 交系统（`UpdateService.ts:427-436`） | [`InstallReject::FormMismatch`]，command 层回退 `shell.open` |

use std::path::{Path, PathBuf};

// ── 运行形态 / 资产形态（纯判定）────────────────────────────────────────────

/// 应用运行形态（= 上游的 loose vs installed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunForm {
    /// 便携/免安装（Windows portable、Linux AppImage、macOS `.app`）。
    Loose,
    /// 安装态（Windows NSIS、Linux deb）。
    Installed,
}

/// 下载件的资产形态（由文件名后缀判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerKind {
    /// Windows 可执行安装器 / 便携包。
    WinExe,
    /// macOS 磁盘镜像。
    Dmg,
    /// Linux AppImage。
    AppImage,
    /// Linux Debian 包。
    Deb,
    /// Android 安装包。**它不走脚本腿** —— 落地方式是交系统安装器，见 [`InstallPlatform::Android`]。
    Apk,
    /// Windows 便携版压缩包（`Polaris_<version>_x64-win-Portable.zip`）。**它不走脚本腿**，见
    /// [`InstallPlatform::WindowsPortable`]。
    PortableZip,
}

/// 文件名是否是 Windows 便携版压缩包。
///
/// 判据与选包侧 `polaris_updater::github::find_suitable_update_asset` 的 loose 分支逐字同口径
/// （[`portable_zip_version`](polaris_updater::github::portable_zip_version) 的正式版本名 + 便携后缀，
/// **大小写敏感**）：选包侧选得出来的名字这里必须认得，别的 zip（内核包、规则包）一律不认。
#[must_use]
pub fn is_portable_zip_name(file_name: &str) -> bool {
    polaris_updater::github::portable_zip_version(file_name).is_some()
}

/// 由资产文件名判定形态（**纯函数**）。无法识别 → `None`。
///
/// 大小写：`.AppImage` 是官方命名（驼峰），但用户重命名/镜像改名很常见 → 一律按小写比较。
#[must_use]
pub fn classify_installer(file_name: &str) -> Option<InstallerKind> {
    if is_portable_zip_name(file_name) {
        return Some(InstallerKind::PortableZip);
    }
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".exe") {
        Some(InstallerKind::WinExe)
    } else if lower.ends_with(".dmg") {
        Some(InstallerKind::Dmg)
    } else if lower.ends_with(".appimage") {
        Some(InstallerKind::AppImage)
    } else if lower.ends_with(".deb") {
        Some(InstallerKind::Deb)
    } else if lower.ends_with(".apk") {
        Some(InstallerKind::Apk)
    } else {
        None
    }
}

/// 运行形态判定（**纯函数**：形态证据由调用方注入，本函数不读环境、不碰文件系统）。
///
/// - Linux：`APPIMAGE` 由 AppImage 运行时注入（真值，Electron/Tauri 通用）→ Loose；否则 deb 安装态。
/// - Windows：便携判据 = **exe 同级的 `portable.marker` 标记文件** —— 打包侧
///   `.github/workflows/package.yml` 的 `Build Windows portable zip` 步写进 zip 根，运行侧
///   `commands/updater::is_portable_layout` 探测。**判定只在那一处实现**，本函数只消费其结果：
///   `Some(便携 exe 路径)` → Loose；`None` → Installed（选 NSIS setup，保守且不会误覆盖）。
///   判据**不是** electron-builder 的 `PORTABLE_EXECUTABLE_FILE`（那是它自解压 stub 注入的，
///   Polaris 的便携版是纯 zip、无 stub ⇒ 该 env 恒不存在，成因详见 `is_portable_layout` 文档）。
/// - macOS：`.app` 恒 Loose（不分形态）。
/// - **Android：不写具名臂，落 `_ => Installed`，且那个值在 Android 上一次都不会被读**。
///   Android 应用只有一种形态（由系统包管理器装的），没有「便携 vs 安装态」这个轴；而
///   [`decide_install_plan`] 的 Android 臂根本不看 `run_form`（交系统安装器与运行形态无关）。
///   为一个不被消费的值新开一条具名臂，只会让 `platform_dispatch_exhaustive` 的字符串轴登记表
///   多一条没有信息量的记录。**这不是「忘了答」，这就是答案。**
///
/// ⚠️ **已知边界（未解决，如实登记）**：用户手动删掉 `portable.marker` 后判定退回 Installed，
/// 该用户会重新被推 NSIS 安装器。方向是失败安全的那一侧（安装器能装、不会砸掉便携副本），
/// 但**没有任何自动手段能守住它** —— 兜底只有标记文件内写明的「勿删」。
#[must_use]
pub fn detect_run_form(
    os: &str,
    appimage_env: Option<&Path>,
    portable_exe: Option<&Path>,
) -> RunForm {
    match os {
        "linux" => {
            if appimage_env.is_some() {
                RunForm::Loose
            } else {
                RunForm::Installed
            }
        }
        "windows" => {
            if portable_exe.is_some() {
                RunForm::Loose
            } else {
                RunForm::Installed
            }
        }
        "macos" => RunForm::Loose,
        _ => RunForm::Installed,
    }
}

// ── 安装计划 ────────────────────────────────────────────────────────────────

/// 安装路径（平台 × 形态的笛卡儿积收敛后的六种真实执行路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPlatform {
    /// Windows 便携（zip）：**不生成脚本**。command 层过退出准备门（停核）之后，用系统处理器
    /// 打开压缩包与程序目录并退出应用，由用户在应用退出后把压缩包内容覆盖解压进去。
    ///
    /// 不做自动解压替换：那需要一段在主程序退出后独立运行、逐项换入并能回滚的脚本，而它替换的是
    /// 用户自己放置、权限与占用情况都不可预知的目录。手动覆盖的前提只有一条——覆盖时应用与内核
    /// 都已退出——这条由 command 层的次序保证。
    WindowsPortable,
    /// Windows 安装态：跑 NSIS setup 原位升级。
    WindowsSetup,
    /// macOS：挂 DMG → 暂存 → mv-swap 原子替换 `.app` → 清 quarantine。
    Macos,
    /// Linux AppImage：覆盖 `$APPIMAGE` 原位。
    LinuxAppImage,
    /// Linux deb：`pkexec apt-get install` 原位升级。
    LinuxDeb,
    /// Android：**不生成脚本、不停代理、不退出应用** —— 经 FileProvider 把 APK 交给系统安装器，
    /// 之后由用户在系统 UI 里决定装不装。
    ///
    /// 与另外五种的结构性差别有三条，每条都能单独毁掉一次更新：
    ///  1. **没有脚本**（[`build_install_script`] 对它返 `None`）：Android 上没有 shell 腿可以
    ///     替换自己的 APK，替换是系统包管理器的事；
    ///  2. **不能先停代理**：停代理是为了让替换文件不被占用，而这里根本不替换文件；提前停了，
    ///     用户在系统确认框上按「取消」之后就只剩一条断掉的隧道；
    ///  3. **不能退出应用**：交出去 ≠ 装成了。退出等于把「用户还没决定」当成「已经装完」。
    Android,
}

/// 安装计划（[`build_install_script`] 的唯一输入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub platform: InstallPlatform,
    /// 已下载的安装件路径。
    pub installer_path: PathBuf,
    /// 当前应用可执行文件路径（重启用）。
    pub exe_path: PathBuf,
    /// Windows 便携：程序所在目录（用户把压缩包内容覆盖解压进去的目标）。
    pub portable_dir: Option<PathBuf>,
    /// macOS：`.app` 包路径；`None` = 定位不到 → 回退 `open` DMG 手动拖拽。
    pub app_bundle_path: Option<PathBuf>,
    /// Linux AppImage：`$APPIMAGE` 原位路径。
    pub appimage_target: Option<PathBuf>,
}

/// 安装计划被拒的原因（**不是错误，是安全兜底**：拒绝即回退交系统处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallReject {
    /// 资产形态与当前运行形态不匹配（如 AppImage 运行 + `.deb` 资产）。
    ///
    /// **绝不强制 root 安装**（= 上游 `UpdateService.ts:427-436`）：错配下自动提权装 deb
    /// 会在 AppImage 用户机器上装出第二份、且要 root，是最坏结果。回退 `shell.open` 交系统。
    FormMismatch {
        installer: String,
        os: String,
        form: RunForm,
    },
    /// 文件名无法识别为任何已知安装件形态。
    UnknownAsset { file_name: String },
}

impl InstallReject {
    /// 被拒之后是否把安装件交给系统默认处理器打开。
    ///
    /// 唯一不交的一格：**便携形态拿到 Windows 安装程序**。系统对 `.exe` 的「打开」就是运行它，
    /// 结果是在别处装出第二份程序，而用户正在用的这一份原样没动 —— 那正是判成错配要避免的事。
    /// 其余错配（如 AppImage 运行拿到 `.deb`）交出去只是让系统的包管理器界面接手，由用户决定。
    ///
    /// 判据只看资产形态与运行形态，不看平台名：`.exe` 安装程序只在 Windows 上有意义，别的平台
    /// 的便携形态拿到它，同样没有理由去运行。
    #[must_use]
    pub fn hands_to_system(&self) -> bool {
        !matches!(
            self,
            Self::FormMismatch { installer, form: RunForm::Loose, .. }
                if classify_installer(installer) == Some(InstallerKind::WinExe)
        )
    }
}

/// Probe destination-side admission before irreversibly draining the app.
/// The script must repeat ownership and lease checks; this is only preflight.
pub fn preflight_mac_admission(bundle: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("应用目录没有父目录")?;
    let lease = parent.join(".polaris-update.lease");
    match std::fs::symlink_metadata(&lease) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("更新租约不是普通文件".into());
            }
            std::fs::OpenOptions::new()
                .append(true)
                .open(&lease)
                .map_err(|error| error.to_string())?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let probe = parent.join(format!(
        ".polaris-admission-preflight-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    #[allow(unused_mut)] // Only Unix provides DirBuilderExt::mode.
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&probe).map_err(|error| error.to_string())?;
    std::fs::remove_dir(&probe).map_err(|error| error.to_string())
}

/// 从可执行路径推导 `.app` 包路径（**纯函数**，= 上游 `macAppBundleFromExe`）。
///
/// `/Applications/Polaris.app/Contents/MacOS/polaris` → `/Applications/Polaris.app`；不匹配返 `None`。
#[must_use]
pub fn mac_app_bundle_from_exe(exe_path: &Path) -> Option<PathBuf> {
    let s = exe_path.to_str()?;
    let idx = s.find(".app/Contents/MacOS/")?;
    let bundle = &s[..idx + 4];
    // 尾段必须是单层文件名（`/Contents/MacOS/<name>`，name 内无 `/`）。
    let tail = &s[idx + ".app/Contents/MacOS/".len()..];
    if tail.is_empty() || tail.contains('/') {
        return None;
    }
    Some(PathBuf::from(bundle))
}

/// 路径串的父目录（**纯字符串运算**，`\` 与 `/` 都当分隔符）。
///
/// 不用 `Path::parent`：它按**宿主**的分隔符切，Unix 宿主上 `C:\Apps\Polaris\polaris.exe` 整串是
/// 一个文件名，`parent()` 给出空串。本函数与宿主无关，故 Windows 形态的路径在任何宿主上都得到
/// 同一个答案。盘符根与 Unix 根保留结尾分隔符（`C:\polaris.exe` → `C:\`）；没有目录段返 `None`。
#[must_use]
pub fn parent_dir_of(path: &str) -> Option<&str> {
    let idx = path.rfind(['\\', '/'])?;
    let head = &path[..idx];
    if head.is_empty() || head.ends_with(':') {
        return Some(&path[..=idx]);
    }
    Some(head)
}

/// 路径串的末段文件名（**纯字符串运算**，`\\` 与 `/` 都当分隔符；与 [`parent_dir_of`] 成对）。
#[must_use]
pub fn file_name_of(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// 安装计划决策（**纯函数**：全部环境真值由参数注入 → 三平台真值表可在 Linux 上单测）。
///
/// # Errors
///
/// - [`InstallReject::UnknownAsset`]：文件名后缀不认识。
/// - [`InstallReject::FormMismatch`]：资产形态与 OS/运行形态错配。
pub fn decide_install_plan(
    os: &str,
    run_form: RunForm,
    installer_path: &Path,
    exe_path: &Path,
    appimage_env: Option<&Path>,
    portable_exe: Option<&Path>,
) -> Result<InstallPlan, InstallReject> {
    // 文件名按字符串取（理由同 [`parent_dir_of`]）：`Path::file_name` 在 Unix 宿主上把整条
    // Windows 路径当成一个文件名，前缀判据会因此失配。
    let file_name = file_name_of(&installer_path.to_string_lossy()).to_string();
    let Some(kind) = classify_installer(&file_name) else {
        return Err(InstallReject::UnknownAsset { file_name });
    };
    let mismatch = || InstallReject::FormMismatch {
        installer: file_name.clone(),
        os: os.to_string(),
        form: run_form,
    };

    let base = |platform: InstallPlatform| InstallPlan {
        platform,
        installer_path: installer_path.to_path_buf(),
        exe_path: exe_path.to_path_buf(),
        portable_dir: None,
        app_bundle_path: None,
        appimage_target: None,
    };

    match (os, kind) {
        ("windows", InstallerKind::WinExe) => {
            // 便携运行形态拿到安装器属错配：跑它会在别处装出第二份程序，而不是更新这一份。
            if run_form != RunForm::Installed {
                return Err(mismatch());
            }
            Ok(base(InstallPlatform::WindowsSetup))
        }
        ("windows", InstallerKind::PortableZip) => {
            // 安装态拿到便携压缩包同样是错配：安装目录不该被手动覆盖。
            if run_form != RunForm::Loose {
                return Err(mismatch());
            }
            // 程序目录取自便携 exe 路径；推不出来就不猜，按错配回退。
            let Some(dir) = portable_exe
                .and_then(|exe| parent_dir_of(&exe.to_string_lossy()).map(PathBuf::from))
            else {
                return Err(mismatch());
            };
            Ok(InstallPlan {
                portable_dir: Some(dir),
                ..base(InstallPlatform::WindowsPortable)
            })
        }
        ("macos", InstallerKind::Dmg) => Ok(InstallPlan {
            app_bundle_path: mac_app_bundle_from_exe(exe_path),
            ..base(InstallPlatform::Macos)
        }),
        ("linux", InstallerKind::AppImage) => {
            // AppImage 资产只在 AppImage 运行形态下原位覆盖；deb 安装态拿到 AppImage 属错配。
            if run_form != RunForm::Loose {
                return Err(mismatch());
            }
            let Some(target) = appimage_env else {
                return Err(mismatch());
            };
            Ok(InstallPlan {
                appimage_target: Some(target.to_path_buf()),
                ..base(InstallPlatform::LinuxAppImage)
            })
        }
        ("linux", InstallerKind::Deb) => {
            // **关键安全闸**：AppImage 运行形态拿到 .deb → 拒绝（绝不 pkexec 装 deb）。
            if run_form != RunForm::Installed {
                return Err(mismatch());
            }
            Ok(base(InstallPlatform::LinuxDeb))
        }
        // Android：`run_form` **不参与**（见 `detect_run_form` 文档的 Android 那一条）。
        // 交系统安装器与应用是怎么装上来的无关，而另外四条腿的 run_form 判据都在回答
        // 「该原位覆盖还是该跑安装器」——那个问题在 Android 上不存在。
        ("android", InstallerKind::Apk) => Ok(base(InstallPlatform::Android)),
        _ => Err(mismatch()),
    }
}

// ── 安装前告知（ad-hoc 签名 / 提权的用户可执行下一步）───────────────────────

/// 安装前必须告知用户的事项（**纯函数判定**，见模块文档「代码签名：ad-hoc 的后果」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAdvisory {
    /// Linux deb：即将弹 polkit 提权框（= 上游 `confirmDebElevation`）。
    ///
    /// **必须在停代理之前弹**：用户取消即真 no-op，不留「代理被停但没更新」坏态
    /// （上游 `UpdateService.ts:306-315` 明确注释）。
    DebElevation,
    /// Windows：无 Authenticode 证书 → SmartScreen「未知发布者」。
    ///
    /// **应用内无法消除**（唯一解是买证书或攒 reputation）⇒ 只能提前把「更多信息 → 仍要运行」讲清楚。
    WindowsSmartScreen,
    /// macOS：ad-hoc 签名 → 下载件带 quarantine。
    ///
    /// 脚本会自动 `xattr -dr com.apple.quarantine` 清除；**万一失败**要告诉用户「右键 → 打开」。
    MacosGatekeeper,
    /// Windows 便携：没有安装程序，更新要用户自己把压缩包覆盖解压到程序目录。
    ///
    /// 告知的内容是「接下来应用会停核并退出，请在它退出之后再覆盖」。它同样必须在停核之前给出：
    /// 用户不继续即真 no-op，连接不受影响。
    PortableManualReplace,
}

/// 该计划是否需要安装前告知用户（**纯函数**）。
///
/// 返回 `Some` 时 command 层必须先把说明交给 UI 确认，**确认后**才停代理 + 起脚本。
#[must_use]
pub fn install_advisory(plan: &InstallPlan) -> Option<InstallAdvisory> {
    match plan.platform {
        InstallPlatform::LinuxDeb => Some(InstallAdvisory::DebElevation),
        // 🔮 **前瞻登记（当前不成立，改 `installMode: perMachine` 时必须一并处理）**：
        // `WindowsSetup` 这条腿跑的是 NSIS setup。当前形态 `currentUser` 下模板发
        // `RequestExecutionLevel user` ⇒ 装 app 本身不弹 UAC，`windowsSmartScreen` 这条文案是完整的。
        // 若改成 per-machine，模板改发 `RequestExecutionLevel admin` ⇒ 用户点「继续安装」之后会
        // **先弹一次 UAC**，而该文案只讲了 SmartScreen、一个字没提管理员授权
        // （`ui/src/i18n/locales/*.json` 的 `settings.update.advisory.windowsSmartScreen.message`
        // 是该承诺的真值源）。取消 UAC 时安装不会发生，而此时代理已被停掉 —— 与 `DebElevation`
        // 那条注释指出的是同一个坏态，deb 腿靠「先弹告知再停代理」规避。
        // 届时改法是五语文案（新增一条 advisory 或扩写现有 message）+ 跑 ui 门。
        InstallPlatform::WindowsSetup => Some(InstallAdvisory::WindowsSmartScreen),
        InstallPlatform::WindowsPortable => Some(InstallAdvisory::PortableManualReplace),
        InstallPlatform::Macos => Some(InstallAdvisory::MacosGatekeeper),
        // AppImage 原位覆盖：无签名校验、无提权，装完直接跑 → 无需额外告知。
        InstallPlatform::LinuxAppImage => None,
        // 🔴 Android **刻意不出 advisory**，而它恰恰是最需要「告知用户下一步」的那个平台 ——
        // 理由是**判据的分辨率**，不是懒。
        //
        // advisory 是一次**预告**：它在动手之前猜「接下来 OS 会拦你」。Windows/macOS 上那个猜
        // 恒真（没有证书就一定被拦），所以预告是诚实的。Android 上不是：「安装未知应用」是
        // **按应用**授权且**可能早就给过了**（用户上次更新时授的）。给一个恒定的预告等于对
        // 一半用户说一句不成立的话，而那正是 `install_advisory` 这个机制最容易腐烂的方向。
        //
        // Android 有一个 advisory 拿不到的东西：`canRequestPackageInstalls()` —— 一个**当场可读的
        // 系统事实**。故这条腿把「要不要引导」推迟到真正交付的那一刻，由 Kotlin 侧按事实决定，
        // 并把结果作为 `ApkHandoff.reason` 带回来（没授权时它已经把用户送到那一页了）。
        // 「有真值可读就别猜」——这与本仓「没有登记来源的数据位不许编」是同一条口径。
        InstallPlatform::Android => None,
    }
}

impl InstallAdvisory {
    /// 前端 i18n key 后缀（UI 据此取 `settings.update.advisory.<key>` 文案）。
    ///
    /// **Rust 侧不建 i18n 框架**（YAGNI）：这里只出 key，文案全在 `ui/src/i18n/locales/*.json`。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::DebElevation => "debElevation",
            Self::WindowsSmartScreen => "windowsSmartScreen",
            Self::MacosGatekeeper => "macosGatekeeper",
            Self::PortableManualReplace => "portableManualReplace",
        }
    }
}

// ── 脚本生成（纯字符串）──────────────────────────────────────────────────────

/// 生成的安装脚本规格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSpec {
    /// 脚本文件名（落到临时目录）。
    pub file_name: String,
    /// 脚本字节（**Windows 是 UTF-16LE + BOM**，故是字节而非 String）。
    pub bytes: Vec<u8>,
    /// 解释器程序。
    pub program: String,
    /// 解释器参数（脚本路径由 command 层追加到末尾）。
    pub leading_args: Vec<String>,
}

/// POSIX shell 单引号转义（= 上游 `shq`）。
#[must_use]
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// VBS 字符串字面量：反斜杠双写（Windows 容忍双反斜杠路径，= 上游 `vbsPath`）。
#[must_use]
pub fn vbs_path(s: &str) -> String {
    s.replace('\\', r"\\")
}

/// UTF-16LE + BOM 编码（**Windows `.vbs` 必须**）。
///
/// # 为什么非它不可（上游的血泪注释，`UpdateService.ts:354-356`）
///
/// `wscript.exe` 按**系统代码页**解释无 BOM 的脚本。中文用户名路径
/// （`C:\Users\张三\AppData\Local\Temp\...`）在 UTF-8 字节被按 GBK 解释时会乱码 →
/// `fso.CopyFile` 找不到文件 → 更新静默失败。带 BOM 的 UTF-16LE 让 wscript 走 Unicode 路径。
#[must_use]
pub fn utf16le_with_bom(s: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE]; // BOM
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// 按计划生成安装脚本（**纯函数**：同一 plan 恒得同一字节序列，可快照断言）。
///
/// # 为什么返回 `Option`
///
/// [`InstallPlatform::Android`] 上**没有脚本这个东西**：落地方式是把 APK 交给系统安装器
/// （`android_bridge::hand_apk_to_system_installer`），一行 shell 都不跑。
/// [`InstallPlatform::WindowsPortable`] 同样没有：落地方式是用户手动覆盖解压。
///
/// 三种写法里选了 `Option`：`unreachable!()` 会把一个「本该在上游被分流掉」的接线错误变成
/// 安装路径上的 panic；造一个假的空 `ScriptSpec` 会让调用方 `spawn` 一个空脚本、拿到 rc=0、
/// 然后如实汇报「安装脚本已起」——一次**静默的**假成功。`None` 是唯一一种让调用方**必须**
/// 写下「那这里怎么办」的形状。
#[must_use]
pub fn build_install_script(plan: &InstallPlan) -> Option<ScriptSpec> {
    match plan.platform {
        // 这两条腿在 command 层各有各的落地方式，不经脚本。真走到这里也不许瞎造一个脚本。
        InstallPlatform::Android | InstallPlatform::WindowsPortable => None,
        InstallPlatform::WindowsSetup => {
            let text = build_windows_setup_vbs(plan);
            Some(ScriptSpec {
                file_name: "polaris-update.vbs".to_string(),
                bytes: utf16le_with_bom(&text),
                program: "wscript.exe".to_string(),
                leading_args: vec![],
            })
        }
        InstallPlatform::Macos => Some(ScriptSpec {
            file_name: "polaris-update.sh".to_string(),
            bytes: build_mac_script(plan).into_bytes(),
            program: "/bin/bash".to_string(),
            leading_args: vec![],
        }),
        InstallPlatform::LinuxAppImage => Some(ScriptSpec {
            file_name: "polaris-update.sh".to_string(),
            bytes: build_linux_appimage_script(plan).into_bytes(),
            program: "/bin/bash".to_string(),
            leading_args: vec![],
        }),
        InstallPlatform::LinuxDeb => Some(ScriptSpec {
            file_name: "polaris-update.sh".to_string(),
            bytes: build_linux_deb_script(plan).into_bytes(),
            program: "/bin/bash".to_string(),
            leading_args: vec![],
        }),
    }
}

/// Windows 安装态更新 VBS（移植 `buildWindowsUpdateVbs` 的安装器那一半）：等旧进程退出 → 跑 NSIS
/// setup 原位升级 → 删自身。行分隔符是 `\r\n`（wscript 要求）。
fn build_windows_setup_vbs(plan: &InstallPlan) -> String {
    let src = vbs_path(&plan.installer_path.to_string_lossy());
    [
        windows_exit_wait(std::process::id()),
        // 🔴 `/UPDATE`，**不是 上游的 `--updated`**（2026-08-05 修）：`--updated` 是
        // **electron-builder** 的约定（它的模板里由 `${isUpdated}` 消费），换到 Tauri 后不成立。
        // Tauri 的 NSIS 模板解析的是 `/UPDATE`（tauri-cli 2.11.4 内嵌模板逐字：
        // `${GetOptions} $CMDLINE "/UPDATE" $UpdateMode`，安装器与卸载器双侧各一处），
        // 且 `$UpdateMode` 有真实语义：
        //   · `${If} $UpdateMode = 1 → Goto reinst_done` —— 跳过「卸载旧版 / 重装」选择页
        //   · `${If} $UpdateMode <> 1` 才装 WebView2 —— 升级时不重跑 WebView2 安装
        //   · 卸载器侧三处 `${If} $UpdateMode <> 1` 清理闸 —— 升级时不做整卸清理
        // `${GetOptions}` 匹配不到只置 error flag、`$UpdateMode` 停在 0 ⇒ 传错 flag 不会报错，
        // 而是**静默降级成全新安装模式**。
        format!("WshShell.Run \"\"\"{src}\"\" /UPDATE\", 1, False"),
        "Set fso = CreateObject(\"Scripting.FileSystemObject\")".to_string(),
        "fso.DeleteFile WScript.ScriptFullName, True".to_string(),
    ]
    .join("\r\n")
}

/// Query absence and waiting on an acquired native process object are separate facts.
/// No query failure other than exact ArgumentException grants install permission.
fn windows_process_wait(pid: u32) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $process = $null\n\
         try {{\n\
           try {{ $process = [System.Diagnostics.Process]::GetProcessById({pid}) }}\n\
           catch {{\n\
             $queryError = $_.Exception\n\
             while ($null -ne $queryError.InnerException) {{ $queryError = $queryError.InnerException }}\n\
             if ($queryError.GetType() -eq [System.ArgumentException]) {{ exit 0 }}\n\
             exit 1\n\
           }}\n\
           try {{\n\
             $null = $process.Handle\n\
             if ($process.WaitForExit(120000)) {{ exit 0 }}\n\
             exit 2\n\
           }} catch {{ exit 1 }}\n\
         }} finally {{\n\
           if ($null -ne $process) {{ $process.Dispose() }}\n\
         }}"
    )
}

fn windows_exit_wait(pid: u32) -> String {
    let bytes: Vec<u8> = windows_process_wait(pid)
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let encoded = crate::runtime::mesh::base64_encode(&bytes);
    [
        "Set WshShell = CreateObject(\"WScript.Shell\")".to_string(),
        format!(
            "waitCommand = \"\"\"\" & WshShell.ExpandEnvironmentStrings(\"%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\") & \"\"\" -NoProfile -NonInteractive -EncodedCommand {encoded}\""
        ),
        "On Error Resume Next".to_string(),
        "Err.Clear".to_string(),
        "waitResult = WshShell.Run(waitCommand, 0, True)".to_string(),
        "waitError = Err.Number".to_string(),
        "On Error Goto 0".to_string(),
        "If waitError <> 0 Or waitResult <> 0 Then WScript.Quit 1".to_string(),
    ]
    .join("\r\n")
}

// The parent's write descriptor stays open until OS process exit, not Ready/commit/Drop.
// dd reads fd 0 through File and preserves read errors. Some cat implementations turn
// stdin EBADF into EOF. No count/noerror option may permit admission before real EOF.
const UNIX_EXIT_WAIT: &str =
    "[ -p /dev/fd/0 ] || exit 1\n/bin/dd bs=4096 of=/dev/null 2>/dev/null || exit 1";

// The executable is frozen by the living App into /tmp, not resolved after EOF
// from an AppImage mount or from an installed (possibly older) system service.
// Admission directories are distinct from the lease-protected recovery journals.
fn unix_update_admission(mac: bool) -> String {
    let owner_mode = if mac {
        "stat -f '%u:%Lp' \"$1\""
    } else {
        "stat -c '%u:%a' \"$1\""
    };
    let hash = if mac {
        "shasum -a 256 \"$1\" | awk '{print $1}'"
    } else {
        "sha256sum \"$1\" | awk '{print $1}'"
    };
    let original = if mac {
        "original.app"
    } else {
        "original.AppImage"
    };
    let original_kind = if mac { "-d" } else { "-f" };
    let candidate = if mac {
        "candidate.app"
    } else {
        "candidate.AppImage"
    };
    format!(
        r#"
set -u
PARENT=$(dirname "$DEST")
owner_mode() {{ {owner_mode}; }}
hash_file() {{ {hash}; }}
LOCK_HELPER="${{POLARIS_LOCK_HELPER:?frozen bundled lease helper required}}"
FROZEN_DIR=$(dirname "$LOCK_HELPER")
[ ! -L "$FROZEN_DIR" ] && [ -d "$FROZEN_DIR" ] && [ "$(owner_mode "$FROZEN_DIR")" = "$(id -u):700" ] || exit 1
[ -f "$LOCK_HELPER" ] && [ ! -L "$LOCK_HELPER" ] && [ "$(hash_file "$LOCK_HELPER")" = "${{POLARIS_LOCK_HELPER_SHA256:?}}" ] || exit 1
[ ! -e "$PARENT/.polaris-update.lock" ] && [ ! -L "$PARENT/.polaris-update.lock" ] || {{ echo "Legacy Polaris update recovery required" >&2; exit 1; }}
LOCK="$PARENT/.polaris-update.lease"
[ ! -L "$LOCK" ] || exit 1
if [ ! -e "$LOCK" ]; then (umask 077; set -C; : > "$LOCK") || [ -f "$LOCK" ] || exit 1; fi
[ -f "$LOCK" ] && [ ! -L "$LOCK" ] || exit 1
exec 9>>"$LOCK"
WORK=''
ADMISSION=''
LEASED=0
initial_cleanup() {{
  rc=$?
  trap - EXIT HUP INT TERM
  if [ "$rc" -ne 0 ] && command -v admission_failure >/dev/null 2>&1; then admission_failure; fi
  if [ -n "$ADMISSION" ]; then rm -rf "$ADMISSION"; fi
  if [ "$LEASED" -eq 1 ]; then "$LOCK_HELPER" lease-release --lock "$LOCK" --fd 9 || rc=1; fi
  exec 9>&-
  rm -f "$LOCK_HELPER"; rmdir "$FROZEN_DIR" 2>/dev/null || true
  exit "$rc"
}}
trap initial_cleanup EXIT
trap 'exit 1' HUP INT TERM
ADMISSION=$(mktemp -d "$PARENT/.polaris-admission.XXXXXX") || exit 1
chmod 0700 "$ADMISSION" || exit 1
printf '%s\n' polaris-update-v1 > "$ADMISSION/transaction-kind" || exit 1
printf '%s' "$DEST" > "$ADMISSION/transaction-destination" || exit 1
printf '%s\n' staging > "$ADMISSION/transaction-phase" || exit 1
"$LOCK_HELPER" lease-acquire --lock "$LOCK" --fd 9 || exit 1
LEASED=1
for OLD in "$PARENT"/.polaris-update.*; do
  [ "$OLD" = "$LOCK" ] && continue
  [ -e "$OLD" ] || [ -L "$OLD" ] || continue
  [ -d "$OLD" ] && [ ! -L "$OLD" ] && [ "$(owner_mode "$OLD")" = "$(id -u):700" ] || {{ echo "Polaris update recovery required: $OLD" >&2; exit 1; }}
  for OBJECT in "$OLD/transaction-kind" "$OLD/transaction-destination" "$OLD/transaction-phase"; do
    [ -f "$OBJECT" ] && [ ! -L "$OBJECT" ] || {{ echo "Polaris update recovery required: $OLD" >&2; exit 1; }}
  done
  [ "$(cat "$OLD/transaction-kind")" = polaris-update-v1 ] && printf '%s' "$DEST" | cmp -s - "$OLD/transaction-destination" || {{ echo "Polaris update recovery required: $OLD" >&2; exit 1; }}
  [ ! -e "$OLD/recovery-required" ] && [ ! -L "$OLD/recovery-required" ] || {{ echo "Polaris update recovery required: $OLD" >&2; exit 1; }}
  case "$(cat "$OLD/transaction-phase")" in
    staging)
      [ ! -e "$OLD/{original}" ] && [ ! -L "$OLD/{original}" ] && [ ! -e "$OLD/committed" ] && [ ! -L "$OLD/committed" ] || exit 1;;
    backup)
      # A missing destination plus the protected original is an unambiguous
      # interrupted first rename. Never overwrite an existing destination.
      if [ ! -e "$DEST" ] && [ ! -L "$DEST" ] && [ {original_kind} "$OLD/{original}" ] && [ ! -L "$OLD/{original}" ]; then
        mv "$OLD/{original}" "$DEST" || exit 1
      elif [ ! -e "$OLD/{original}" ] && [ ! -L "$OLD/{original}" ] && candidate_matches "$DEST" "$OLD/original-identity"; then
        : # The first rename did not happen, or exact rollback already finished.
      else echo "Polaris update recovery ambiguous: $OLD" >&2; exit 1; fi;;
    committed)
      candidate_matches "$DEST" "$OLD/candidate-identity" || exit 1;;
    *) echo "Polaris update recovery required: $OLD" >&2; exit 1;;
  esac
  rm -rf "$OLD" || exit 1
done
WORK="$PARENT/.polaris-update.${{ADMISSION##*.}}"
[ ! -e "$WORK" ] && [ ! -L "$WORK" ] || exit 1
mv "$ADMISSION" "$WORK" || exit 1
ADMISSION=''
STAGE="$WORK/{candidate}"
BAK="$WORK/{original}"
set_phase() {{ printf '%s\n' "$1" > "$WORK/transaction-phase.next" && mv -f "$WORK/transaction-phase.next" "$WORK/transaction-phase"; }}
"#
    )
}

/// macOS 更新脚本（移植 `buildMacUpdateScript`）。
///
/// # ad-hoc 签名的关键一步
///
/// 替换成功后**必跑** `xattr -dr com.apple.quarantine "$DEST"`：ad-hoc 签名的 `.app` 一旦带
/// quarantine 属性，Gatekeeper 直接拒绝启动（「来自身份不明的开发者」）——**用户点了更新、装完打不开**，
/// 比不做更新还糟。验证发行时已有的签名；失败保留候选并恢复本次操作前的原文件，不重签冻结的 helper/core。
fn build_mac_script(plan: &InstallPlan) -> String {
    let dmg = sh_quote(&plan.installer_path.to_string_lossy());
    let Some(bundle) = plan.app_bundle_path.as_ref() else {
        return format!("#!/bin/bash\n{UNIX_EXIT_WAIT}\nopen {dmg}\n");
    };
    let dest = sh_quote(&bundle.to_string_lossy());
    ["#!/bin/bash", UNIX_EXIT_WAIT, &format!("DMG={dmg}"), &format!("DEST={dest}"), r#"
admission_failure() {
  printf '%s\n' 'Polaris update admission failed; manual installation required.' > "$(dirname "$POLARIS_LOCK_HELPER")/admission-failure.txt"
  open "$DMG"
  if [ -d "$DEST" ]; then open "$DEST"; fi
}
trap 'admission_failure' EXIT
candidate_identity() {
  codesign -dv --verbose=4 "$1" 2>&1 | awk -F= '/^CDHash=/ {print $2}'
}
candidate_matches() {
  [ -f "$2" ] && [ ! -L "$2" ] && [ -d "$1" ] && [ ! -L "$1" ] && codesign --verify --deep --strict "$1" >/dev/null 2>&1 && [ -s "$2" ] && candidate_identity "$1" | cmp -s - "$2"
}
"#, &unix_update_admission(true), r#"
MNT=''
MOUNTED=0
cleanup() {
  if [ "$MOUNTED" -eq 1 ]; then hdiutil detach "$MNT" >/dev/null 2>&1 || return; fi
  if [ -n "$MNT" ]; then rmdir "$MNT" 2>/dev/null || true; fi
  if [ -e "$BAK" ] || [ -e "$WORK/recovery-required" ]; then
    echo "Polaris update recovery retained: $WORK" >&2
    return
  fi
  rm -rf "$WORK" || return
  "$LOCK_HELPER" lease-release --lock "$LOCK" --fd 9 || return
  exec 9>&-
  rm -f "$LOCK_HELPER"; rmdir "$FROZEN_DIR" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
MNT=$(mktemp -d /tmp/polaris-update-mount.XXXXXX) || { open "$DMG"; exit 1; }
if ! hdiutil attach "$DMG" -readonly -nobrowse -noautoopen -mountpoint "$MNT" >/dev/null 2>&1; then
  open "$DMG"; open "$DEST"; exit 1
fi
MOUNTED=1
APPS=("$MNT"/*.app)
if [ "${#APPS[@]}" -ne 1 ] || [ ! -d "${APPS[0]}" ] || [ -L "${APPS[0]}" ]; then
  open "$DMG"; open "$DEST"; exit 1
fi
if ! ditto "${APPS[0]}" "$STAGE"; then open "$DMG"; open "$DEST"; exit 1; fi
# Preserve every compiled helper/core byte. Quarantine removal changes xattrs;
# signature verification never repairs or rewrites a nested executable.
xattr -dr com.apple.quarantine "$STAGE" 2>/dev/null || true
if ! codesign --verify --deep --strict "$STAGE" >/dev/null 2>&1; then
  echo 'Polaris candidate signature invalid; frozen payload was not resigned' >&2
  open "$DMG"; open "$DEST"; exit 1
fi
if ! hdiutil detach "$MNT" >/dev/null 2>&1; then open "$DMG"; exit 1; fi
MOUNTED=0
candidate_identity "$DEST" > "$WORK/original-identity"
candidate_identity "$STAGE" > "$WORK/candidate-identity"
[ -s "$WORK/candidate-identity" ] || exit 1
replace() {
  [ ! -e "$BAK" ] || return 2
  set_phase backup || return 2
  # Durable backup phase precedes the first destination mutation.
  mv "$DEST" "$BAK" 2>/dev/null || return 1
  if mv "$STAGE" "$DEST" 2>/dev/null && codesign --verify --deep --strict "$DEST" >/dev/null 2>&1 && touch "$WORK/committed" && set_phase committed; then
    rm -rf "$BAK"
    return 0
  fi
  if [ -e "$DEST" ] && ! mv "$DEST" "$WORK/rejected.app"; then
    touch "$WORK/recovery-required"; return 2
  fi
  if ! mv "$BAK" "$DEST"; then touch "$WORK/recovery-required"; return 2; fi
  return 1
}
if ! replace; then
  # Elevation retries only this intact candidate after a permission failure.
  # A failed rollback retains its original bytes and is never retried blindly.
  if [ -d "$STAGE" ] && [ ! -e "$BAK" ] && [ ! -e "$WORK/recovery-required" ]; then
    ELEV="$WORK/replace.sh"
    {
      printf '%s\n' '#!/bin/bash' 'set -u'
      printf 'DEST=%q\nSTAGE=%q\nBAK=%q\nWORK=%q\n' "$DEST" "$STAGE" "$BAK" "$WORK"
      declare -f set_phase replace
      printf '%s\n' 'replace'
    } > "$ELEV"
    chmod 0700 "$ELEV"
    osascript - "$ELEV" <<'POLARIS_ELEVATE'
on run argv
  do shell script "/bin/bash " & quoted form of (item 1 of argv) with administrator privileges
end run
POLARIS_ELEVATE
  fi
fi
if [ -f "$WORK/committed" ] && [ -d "$DEST" ] && [ ! -d "$STAGE" ]; then
  rm -f "$DMG"
  open "$DEST" || open -R "$DEST"
else
  open "$DMG"
  if [ -d "$DEST" ]; then open "$DEST"; fi
  if [ -e "$BAK" ]; then open -R "$BAK"; fi
fi
"#].join("\n")
}

/// Linux AppImage 原位覆盖脚本（移植 `buildLinuxAppImageScript`）。
fn build_linux_appimage_script(plan: &InstallPlan) -> String {
    let src = sh_quote(&plan.installer_path.to_string_lossy());
    let dst = sh_quote(
        &plan
            .appimage_target
            .as_ref()
            .unwrap_or(&plan.exe_path)
            .to_string_lossy(),
    );
    [
        "#!/bin/bash",
        UNIX_EXIT_WAIT,
        &format!("NEW={src}"),
        &format!("DEST={dst}"),
        r#"
candidate_matches() {
  [ -f "$2" ] && [ ! -L "$2" ] && [ -f "$1" ] && [ ! -L "$1" ] && [ "$(hash_file "$1")" = "$(cat "$2")" ]
}
"#,
        &unix_update_admission(false),
        r#"
cleanup() {
  if [ -e "$BAK" ] || [ -e "$WORK/recovery-required" ]; then
    echo "Polaris update recovery retained: $WORK" >&2
  else
    rm -rf "$WORK" || return
    "$LOCK_HELPER" lease-release --lock "$LOCK" --fd 9 || return
    exec 9>&-
    rm -f "$LOCK_HELPER"; rmdir "$FROZEN_DIR" 2>/dev/null || true
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
if ! cp "$NEW" "$STAGE" || ! cmp -s "$NEW" "$STAGE" || ! chmod 0755 "$STAGE"; then
  nohup "$DEST" 9>&- >/dev/null 2>&1 &
  xdg-open "$(dirname "$NEW")" >/dev/null 2>&1 &
  exit 1
fi
hash_file "$DEST" > "$WORK/original-identity" || exit 1
hash_file "$STAGE" > "$WORK/candidate-identity" || exit 1
set_phase backup || exit 1
if ! mv "$DEST" "$BAK"; then nohup "$DEST" 9>&- >/dev/null 2>&1 & exit 1; fi
if mv "$STAGE" "$DEST" && candidate_matches "$DEST" "$WORK/candidate-identity" && set_phase committed; then
  rm -f "$BAK"
  rm -f "$NEW"
  nohup "$DEST" 9>&- >/dev/null 2>&1 &
else
  if [ -e "$DEST" ] || [ -L "$DEST" ]; then
    mv "$DEST" "$WORK/rejected.AppImage" || { touch "$WORK/recovery-required"; exit 1; }
  fi
  if ! mv "$BAK" "$DEST"; then
    touch "$WORK/recovery-required"
    xdg-open "$WORK" >/dev/null 2>&1 &
    exit 1
  fi
  nohup "$DEST" 9>&- >/dev/null 2>&1 &
  xdg-open "$(dirname "$NEW")" >/dev/null 2>&1 &
  exit 1
fi
"#,
    ]
    .join("\n")
}

/// Linux deb 原位升级脚本（移植 `buildLinuxDebScript`）。
fn build_linux_deb_script(plan: &InstallPlan) -> String {
    let deb = sh_quote(&plan.installer_path.to_string_lossy());
    let exe = sh_quote(&plan.exe_path.to_string_lossy());
    [
        "#!/bin/bash",
        UNIX_EXIT_WAIT,
        &format!("DEB={deb}"),
        &format!("EXE={exe}"),
        // apt-get install 本地 deb（apt 1.1+ 支持绝对路径）：解依赖 + 同包名版本升级。
        // Ubuntu 24.04+ 默认 .deb 处理器是 App Center，对本地 deb 只按包名判「已安装」、不比较版本
        // → `xdg-open` 是死路，故必须走 apt。
        "if pkexec apt-get install -y -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold \"$DEB\"; then",
        "  rm -f \"$DEB\" 2>/dev/null",
        "  nohup \"$EXE\" >/dev/null 2>&1 &",
        "else",
        // 用户取消授权 / apt 失败 → 打开下载目录让用户手动（**不**回退到 App Center 死路）。
        "  xdg-open \"$(dirname \"$DEB\")\" >/dev/null 2>&1 &",
        "fi",
        "",
    ]
    .join("\n")
}

// ── 唯一的执行腿（**真机门**：本机绝不调用）────────────────────────────────

/// 写脚本到临时目录并 `spawn(detached)`（**执行腿**）。
///
/// 调用方先完成受管进程的严格 drain，再生成脚本；脚本还须等待旧应用的 OS 退出。
///
/// # Errors
///
/// 写脚本 / spawn 失败。
pub fn spawn_detached_script(dir: &Path, spec: &ScriptSpec) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("建脚本目录失败 {}: {e}", dir.display()))?;
    let path = dir.join(&spec.file_name);
    std::fs::write(&path, &spec.bytes)
        .map_err(|e| format!("写安装脚本失败 {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    let mut cmd = std::process::Command::new(&spec.program);
    cmd.args(&spec.leading_args).arg(&path);
    #[cfg(unix)]
    let mut frozen_helper = if std::str::from_utf8(&spec.bytes)
        .is_ok_and(|text| text.contains("${POLARIS_LOCK_HELPER:?"))
    {
        let expected = crate::runtime::core_identity::selected_manifest(
            crate::runtime::core_identity::current_platform()?,
        )?
        .helper_sha256
        .ok_or("CORE_BUNDLE_MISMATCH: compiled lease helper hash unavailable")?;
        let frozen =
            freeze_lease_helper(&crate::runtime::helper::resolve_helper_binary()?, &expected)?;
        cmd.env("POLARIS_LOCK_HELPER", frozen.path())
            .env("POLARIS_LOCK_HELPER_SHA256", expected);
        Some(frozen)
    } else {
        None
    };
    // detached：父进程随即 exit(0)，脚本必须活下去。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // setsid：脱离本进程会话，父退出不带走脚本。
        // SAFETY: pre_exec 闭包不捕获借用、不分配也不持 Rust 锁；`setsid` 是单次 libc syscall，满足
        // fork 后到 exec 前只能做 async-signal-safe 操作的约束。失败按既有 best-effort 语义继续 exec。
        unsafe {
            cmd.pre_exec(|| {
                let _ = nix::unistd::setsid();
                Ok(())
            });
        }
    }
    #[cfg(unix)]
    {
        use std::os::fd::IntoRawFd;
        let (child, lifetime_writer) = spawn_with_lifetime_pipe(&mut cmd)
            .map_err(|e| format!("启动安装脚本失败 {}: {e}", spec.program))?;
        // No await or fallible publication after spawn: only OS process exit closes this fd.
        // CLOEXEC prevents subsequent child processes from extending this lifetime.
        let _process_lifetime_fd = lifetime_writer.into_raw_fd();
        if let Some(frozen) = frozen_helper.as_mut() {
            // The script now owns removal; this has no fallible publication.
            frozen.directory = None;
        }
        drop(child);
    }
    #[cfg(not(unix))]
    cmd.spawn()
        .map_err(|e| format!("启动安装脚本失败 {}: {e}", spec.program))?;
    Ok(path)
}

#[cfg(unix)]
struct FrozenLeaseHelper {
    directory: Option<PathBuf>,
    executable: PathBuf,
}
#[cfg(unix)]
impl FrozenLeaseHelper {
    fn path(&self) -> &Path {
        &self.executable
    }
}
#[cfg(unix)]
impl Drop for FrozenLeaseHelper {
    fn drop(&mut self) {
        if let Some(directory) = &self.directory {
            let _ = std::fs::remove_file(&self.executable);
            let _ = std::fs::remove_dir(directory);
        }
    }
}

#[cfg(unix)]
fn freeze_lease_helper(source: &Path, expected: &str) -> Result<FrozenLeaseHelper, String> {
    use std::os::unix::fs::MetadataExt;
    // Fixed, root-owned sticky temporary root. TMPDIR can point into AppImage's
    // short-lived mount, so it is deliberately not a staging authority here.
    let parent = Path::new("/tmp")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let metadata = std::fs::symlink_metadata(&parent).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o1000 == 0 {
        return Err("invalid independent update staging root".into());
    }
    freeze_lease_helper_in(&parent, source, expected)
}

#[cfg(unix)]
fn freeze_lease_helper_in(
    parent: &Path,
    source: &Path,
    expected: &str,
) -> Result<FrozenLeaseHelper, String> {
    use std::fs::{DirBuilder, OpenOptions};
    use std::io::Read;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    let input = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(source)
        .map_err(|error| error.to_string())?;
    let source_metadata = input.metadata().map_err(|error| error.to_string())?;
    // Match the existing immutable installation-file budget, without holding
    // a second helper binary in memory. No candidate is executed before sealing.
    if !source_metadata.is_file()
        || source_metadata.len() == 0
        || source_metadata.len() > 512 * 1024 * 1024
    {
        return Err("lease helper is not a bounded regular bundled file".into());
    }
    let directory = parent.join(format!("polaris-update-helper.{}", uuid::Uuid::new_v4()));
    DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|error| error.to_string())?;
    let frozen = FrozenLeaseHelper {
        executable: directory.join("lease-helper"),
        directory: Some(directory.clone()),
    };
    let metadata = std::fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
    if metadata.uid() != nix::unistd::getuid().as_raw() || metadata.mode() & 0o777 != 0o700 {
        return Err("invalid independent lease helper stage".into());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(frozen.path())
        .map_err(|error| error.to_string())?;
    let copied = std::io::copy(&mut input.take(source_metadata.len() + 1), &mut output)
        .map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    let actual = polaris_updater::verify::sha256_reader_hex(
        std::fs::File::open(frozen.path()).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if copied != source_metadata.len() || actual != expected {
        return Err("CORE_BUNDLE_MISMATCH: independent lease helper copy differs".into());
    }
    Ok(frozen)
}

#[cfg(unix)]
fn lifetime_pipe() -> std::io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    #[cfg(target_os = "linux")]
    let pipe = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC);
    #[cfg(not(target_os = "linux"))]
    let pipe = nix::unistd::pipe();
    let (reader, writer) = pipe.map_err(std::io::Error::from)?;
    #[cfg(not(target_os = "linux"))]
    {
        use nix::fcntl::{fcntl, FcntlArg, FdFlag};
        // Darwin lacks pipe2. A concurrent fork before both fcntls can only delay EOF.
        for fd in [&reader, &writer] {
            fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(std::io::Error::from)?;
        }
    }
    Ok((reader, writer))
}

#[cfg(unix)]
fn spawn_with_lifetime_pipe(
    cmd: &mut std::process::Command,
) -> std::io::Result<(std::process::Child, std::os::fd::OwnedFd)> {
    spawn_with_pipe(cmd, lifetime_pipe()?)
}

#[cfg(unix)]
fn spawn_with_pipe(
    cmd: &mut std::process::Command,
    (reader, writer): (std::os::fd::OwnedFd, std::os::fd::OwnedFd),
) -> std::io::Result<(std::process::Child, std::os::fd::OwnedFd)> {
    cmd.stdin(std::process::Stdio::from(reader));
    let child = cmd.spawn()?;
    Ok((child, writer))
}

#[cfg(test)]
mod tests;
