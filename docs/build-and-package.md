# 构建与打包

<div align="center">

**简体中文** · [English](build-and-package.en.md)

</div>

从 README 迁出（2026-08-13）：这些是工程内幕与不变量，读者是维护者，不是使用者。
README 只留「怎么装、怎么用」。

## 构建

实现按系统设计 §H 分批落地（B0 脚手架 → B10 发布工程）。

### 工具链要求

| 工具 | 版本 | 用途 |
|---|---|---|
| Rust | stable（edition 2021） | 后端 + `crates/` 下 18 个域 crate（另有 `source-probe` 是 dev-only，只进 `[dev-dependencies]`，不进任何 lib/bin 依赖图） |
| Node.js | 24+（CI 钉 26） | 前端构建 + fetch 脚本 |
| pnpm | 11.24.0（由 `ui/package.json` 钉扎） | 前端包管理（`ui/`）；`tauri build` 的 `beforeBuildCommand` 直接调用 `pnpm`，须在 PATH 上 |
| Go | `scripts/libbox-patches/source-manifest.json` 的 `goVersion` | 桌面内核源码构建，以及消费内核包时读取二进制构建信息 |
| [Tauri CLI 2](https://v2.tauri.app/) | `scripts/tauri-cli.version` 钉扎 | 全局 CLI 执行 `tauri build` 打包 |

在仓库根安装全局 CLI。CLI 已从 `ui/` devDependency 退场，桌面和 Android CI 均安装同一固定版本。

```bash
npm install -g "@tauri-apps/cli@$(cat scripts/tauri-cli.version)"
tauri --version
```

### 系统依赖

**Linux**（Tauri 2 WebKitGTK 4.1 栈，Debian/Ubuntu）：

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libglib2.0-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
  libdbus-1-dev pkg-config
```

**macOS**：13.0+（Ventura，BTM「允许后台」三级探测需要），Xcode Command Line Tools。
Windows：MSVC build tools（Visual Studio Build Tools 2022 + Windows 10/11 SDK）。

### Rust workspace（开发门禁）

```bash
cargo build --workspace        # 编译
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace         # 单测
cargo fmt --all -- --check     # 格式
```

### 前端工具链与 AST 测试

前端当前使用 React 19、TypeScript 7、Vite 8、Vitest 4 与 Tailwind CSS 4。安装、构建和测试统一使用
`ui/package.json` 的 `packageManager` 所钉 pnpm 11.24.0：

```bash
cd ui
npx pnpm@11.24.0 install --frozen-lockfile
npx pnpm@11.24.0 run build
npx pnpm@11.24.0 test
```

TypeScript 7 不再沿用旧的 `createSourceFile(..., ScriptTarget, ..., ScriptKind)` 测试入口。
仓库的结构化源码门统一经 `ui/src/test/ts-compiler.ts` 使用原生
`typescript/unstable/sync`、`typescript/unstable/fs` 与 `typescript/unstable/ast`；调用方只使用
`parseSourceFile(fileName, text)`。新增 AST 门不得重新引入旧编译器调用形态或直接维护另一套兼容层。

### 资源拉取（sing-box 核 / cronet / 面板）

三类资源的渠道与完整性合同不同，不能混称为「官方 release + manifest SHA」：

- `fetch-core` 不下载上游 Release 压缩包。桌面内核由 `scripts/libbox-patches/source-manifest.json` 钉扎的 sing-box
  源码提交加本仓补丁（Windows 另加 `scripts/core-patches/windows-dns-refresh.patch`）在四个平台各自原生构建，
  版本为 `core-manifest.json` 的 `sourceBuild.version`；脚本把校验过的四平台内核包落到 `resources/<平台>/`；
- `fetch-cronet` 从 Go module proxy 下载平台模块 zip，解压后以 `cronetLibrarySha256` 校验实际动态库；
- `fetch-dashboard` 取 sing-box `gh-pages` 面板产物，**当前没有 SHA256 pin**。

它们都按需拉取、不入库，且打包前必须跑：Tauri bundle `resources` 字段引用这些产物。core 与 cronet
缺 pin 都会 fail，绝不无校验拉取原生可执行资源。

```bash
# sing-box 四平台核：消费内核包（版本 = core-manifest.json 的 sourceBuild.version，勿在此重复钉）
node scripts/fetch-core.mjs --bundle-dir=<内核包目录> --candidate="$(git rev-parse HEAD)"
node scripts/fetch-cronet.mjs     # libcronet（仅 linux/windows；mac 静态编入核心）
node scripts/fetch-dashboard.mjs  # sing-box 面板（gh-pages 产物）
```

`node scripts/fetch-cronet.mjs --platform=linux` 或 `--platform=win` 可只拉当前打包腿；版本始终从
`bundledCoreVersion` 对应 sing-box tag 的 `go.mod` 解析，Linux 与 Windows 可各自使用上游实际 require 的模块版本。
`--check-only` 不下载库，但会校验 tag 可读、两个精确 require 均存在、以及两平台的 SHA-256 pin 完整且格式有效。

内核包由 `.github/workflows/desktop-core.yml` 产出：Linux / Windows / macOS x64 / macOS arm64 四个原生 runner 各跑
`node scripts/fetch-core.mjs --producer --platform=<linux|win|mac-x64|mac-arm64> --bundle-dir=<目录> --candidate=<提交 SHA>`，
汇总后由 `--assemble` 写出 `bundle.json`。`--producer` 只接受恰好一个平台，不做跨平台构建。消费形态要求
`--candidate` 等于当前 `HEAD` 且工作树没有已跟踪文件的改动；即使用 `--platform` 只落其中一个平台，也会先校验
全部四份二进制与回执。缺内核包、或 `sourceBuild` 钉扎不完整时直接失败，没有回落到官方 Release 资产或旧缓存的路径。

#### 获取内核包的两种方式

**完整的四平台包（打包与发布用）。** `desktop-core.yml` 只能被其它 workflow 调用。在自己有权限的仓库（或 fork）里手动触发
`release-risk.yml` 或 `package.yml`，它们会调用 `desktop-core.yml`；运行结束后下载名为
`desktop-core-bundle-<提交 SHA>-<运行号>-<重试次数>` 的产物，解压出的目录就是 `--bundle-dir`。产物保留 7 天，
且只对产出它的那个提交有效。

```bash
gh run download <运行号> --name 'desktop-core-bundle-<提交 SHA>-<运行号>-<重试次数>' --dir <内核包目录>
```

**只构建本机平台的核（本地开发用）。** 没有四平台包时，可以在本机只产出当前平台的核，再手动放到资源目录。
这条路径跳过了四平台包的一致性校验，只用于本地开发与调试，不能用来打发布包。

```bash
# 需要 Go（source-manifest.json 的 goVersion）、Python 3、git，并能访问 github.com 取上游源码
# HEAD 必须等于 --candidate，且工作树没有已跟踪文件的改动
node scripts/fetch-core.mjs --producer --platform=linux --bundle-dir=<目录> --candidate="$(git rev-parse HEAD)"
mkdir -p resources/linux && cp <目录>/linux/sing-box resources/linux/sing-box
```

`--platform` 取 `linux`、`win`、`mac-x64`、`mac-arm64` 之一，必须与本机一致（不做跨平台构建）；Windows 的文件名是
`sing-box.exe`。本机若装的不是清单要求的 Go 版本，可设 `GOTOOLCHAIN=go<版本>` 让 Go 自行取用对应工具链。

上面三条都必须手动跑：`tauri.conf.json` **没有** `build.beforeBundleCommand`，
`tauri build` 不会替你 fetch 任何资源。
（此前本节描述过一套 `beforeBundleCommand` 兜底，实际那个键从未存在，
`scripts/verify-dashboard-resources.mjs` 因此是永不执行的孤儿脚本——已删除。）

兜底改由 `node scripts/verify-packaging.mjs confs` 提供，它在 CI 的 fetch 之后、Rust 构建之前跑
（`.github/workflows/package.yml`），断言 conf 引用的每一条资源路径**存在且有内容**：
空目录与 0 字节文件一律转红（存在 ≠ 有内容；fetch / 解压失败的典型形态正是留下这两种）。
任意开发机可复现，纯静态、无构建依赖。

升级核心时，先更新 `bundledCoreVersion`、`scripts/libbox-patches/source-manifest.json` 与 `core-manifest.json` 的
`sourceBuild` / `windowsBuild` 钉扎（`coreArchiveSha256` 现只保留其键集合作为随包平台枚举），再跑
`node scripts/fetch-cronet.mjs --check-only`。若上游 `go.mod` 的 Cronet 依赖变化，只更新受影响平台的
`cronetLibrarySha256`，最后以 `--force --platform=<linux 或 win>` 重拉并验证该平台库；版本不写回 manifest。

### 打包出安装包

```bash
# 1) 拉资产（见上）
# 2) 前端构建 + Rust 编译 + 安装包（Tauri CLI 自动编排 beforeBuildCommand）
#    从**仓库根**跑，并显式传本平台的 config（见下方「按平台筛内核」）
tauri build --config src-tauri/tauri.linux.conf.json          # Linux
tauri build --config src-tauri/tauri.windows.conf.json        # Windows
tauri build --config src-tauri/tauri.macos-arm64.conf.json    # macOS Apple Silicon
tauri build --config src-tauri/tauri.macos-x64.conf.json --target x86_64-apple-darwin  # macOS Intel
```

产物落在**仓库根**的 `target/release/bundle/`（本仓是 cargo workspace，workspace 根在仓库根，
故**不是** `src-tauri/target/`）；传了 `--target <triple>` 时再下沉一层：`target/<triple>/release/bundle/`。

| 平台 | 产物 | 形态 |
|---|---|---|
| Linux | `Polaris_<版本>_amd64-linux.deb` / `Polaris_<版本>_amd64-linux.AppImage` | deb 包 + AppImage（单文件免装） |
| macOS | `Polaris_<版本>_aarch64-mac.dmg` / `Polaris_<版本>_x64-mac.dmg` | **分架构单出**（不再出 universal，未签名）。⚠️ 这是 **release 资产名**，不是本地产物名 —— 见下 |
| Windows | `Polaris_<版本>_x64-win-setup.exe` | NSIS 安装器（WebView2 downloadBootstrapper，不内嵌 Runtime） |
| Windows | `Polaris_<版本>_x64-win-Portable.zip` | 免安装绿色版（解压即用，自带 `resources/` + `portable.marker` 形态标记） |

portable 由 `package.yml` 在 Windows 腿从 `target/release/polaris.exe` + `resources/` 额外打 zip，
本地单跑上面那条 `tauri build` 不会有。

正式发布文件名统一为 `Polaris_<版本>_<架构>-<平台>[-<形态>].<扩展名>`。
`package.yml` 在产物校验前规范化 macOS、Linux 与 Windows 的文件名；本地单跑 `tauri build`
仍得到 Tauri 默认文件名。版本来自 `src-tauri/tauri.conf.json`，不使用 CI 运行标签代替版本。
桌面草稿先核六份资产；最终发布必须包含六份桌面资产、下列三份 Android APK 与 `SHA256SUMS`。
`scripts/release-assets.mjs` 定义命名及集合，`verify-packaging.mjs assets --label release-all`
核对完整集合、版本、体积与逐项摘要，旧名副本与额外文件均拒绝。

### Android 正式 release

| ABI | 发布名 |
|---|---|
| ARMv8 | `Polaris_<版本>_arm64-v8a-android.apk` |
| ARMv7 | `Polaris_<版本>_armeabi-v7a-android.apk` |
| ARMv8 + ARMv7 | `Polaris_<版本>_universal-android.apk` |

`android.yml` 在桌面草稿通过后构建三个正式签名 APK：

```bash
bash scripts/build-android-apk.sh --apk --split-per-abi --target aarch64 armv7 --ci --config src-tauri/tauri.android.conf.json
bash scripts/build-android-apk.sh --apk --target aarch64 armv7 --ci --config src-tauri/tauri.android.conf.json
```

两次调用分别产出 `arm64Release` / `armRelease` 与 `universalRelease`；全部使用完整 Rust release
构型、R8、资源裁剪、压缩 native libraries 与复制后的原生库安全剥符号。两条命令只构建
`arm64-v8a` 和 `armeabi-v7a`；release JNI 打包再排除 x86/x86_64 库，保留各 split 自己的 ABI 集合。
universal 包含两个 ARM ABI；debug 开发构型保留模拟器 ABI。
三个包逐一核 manifest 版本/应用 ID、真实 ABI 集合、每份 ELF 的架构/符号/压缩方式、R8 证据、
内核 AAR 来源原字节与正式签名证书 pin。任何签名凭据缺失即失败；风险门的未签名轻量包不能发布。
最终清单使用从草稿回读且与 Android job 固定已验签 SHA 对拍的字节，公开紧前再核完整远端 digest。
本轮不产出 iOS release 包。三份 APK 的实际构建、签名、体积与安装验收须由候选发布验证。

#### 按平台筛内核（`--config` 不可省）

`src-tauri/tauri.conf.json` 的 `bundle.resources` **不含任何平台内核目录**，四个平台内核分别由
`tauri.{linux,windows,macos-arm64,macos-x64}.conf.json` 指定。理由与纪律：

- 四平台内核全塞 ⇒ 每个包白背 ~210MB 死重（运行期只按 `env::consts::OS/ARCH` 取一份）。
- 合并按 RFC 7396，**数组整体替换不合并** ⇒ 往 base 加公共资源必须同步到四份，否则四个包全部静默丢失。
- **不吃 Tauri 的「按平台名隐式合并」**：隐式只认固定文件名，文件一改名就静默停止合并，
  包里没有内核、bundler 照常出包，只在用户机器上 `resolve_core_binary → Err` 才暴露。显式
  `--config` 下同样的改名会得到 `failed to read configuration file` 硬失败。
  （macOS 那份原名 `tauri.macos.conf.json` 会被隐式合并，实为 arm64 专用 ⇒ 在 Intel Mac 上裸
  `tauri build` 会打进 arm64 内核；已改名为 `tauri.macos-arm64.conf.json` 消除该隐式默认。）

这些不变量由 `node scripts/verify-packaging.mjs confs` 机器守住（CI 每次打包前跑，本机可直接复现）；
构建后再由 `payload` / `assets` 两个模式断言产物里恰好一份本平台内核、且产物名满足更新器选包契约。

上面这些问的都是「**该在**的东西在不在」。反方向由第四个模式 `inventory` 守 ——
**包内容白名单**：枚举资源载荷树的全部文件，与逐条登记的白名单对账，**多一个文件即红**，
且每条登记都要写明「为什么它该在包里」（登记表见 `verify-packaging.mjs` 的 `payloadAllowRules()`）。

```bash
node scripts/verify-packaging.mjs inventory --label linux --static                 # 无需产物：conf × 工作树静态推导
node scripts/verify-packaging.mjs inventory --label linux --root target/release/bundle   # 产物口径
```

补它的起因是一批**已经出货**的夹带（2026-08-29）：`resources/data/README*.md`（开发文档，已移到
`docs/geo-rulesets*.md`）、0 字节 `.gitkeep`、面板的 gh-pages/PWA 残留（`.nojekyll` / `sw.js` /
`registerSW.js` / `workbox-*.js` / `manifest.webmanifest`，已在 `scripts/fetch-dashboard.mjs` 解压后即剔）——
当时全链零转红。同期给 `confs` 补的**不变量 E**（per-platform conf 不得含未登记条目）堵的是同一类的配置侧入口：
此前往四份 conf 各加一条 `"../ui/src/"`，`confs` 仍 rc=0，整份 `ui/src` 会随四个安装包出货。

`inventory` 的射程如实标注：清点的是 bundler 铺 `bundle.resources` 的那一片，不含 bundler /
linuxdeploy 自己铺的 ELF 与宿主共享库（那些不由 `bundle.resources` 决定），输出里会打印射程外的文件数。
Windows 腿因 NSIS 无 bundle 侧副本，退化为 cargo staging 清点并在输出里标注「staging 检查（不是产物验证）」。

## 持续集成

六个 workflow 分工（`.github/workflows/`）：

- **`ci.yml`** — Rust 门禁，职责 = 「改动是否正确」。三个并行 job：
  - `fmt + clippy (<os>)`：格式与 clippy（`-D warnings`）；Linux 腿另跑 rustdoc 与 Cronet 来源解析。
  - `cross-target clippy + android face`（只在 Linux）：对 Windows / macOS / Android / iOS 四个交叉目标跑
    clippy，检查 `vendor/` 下随仓第三方副本在这些目标上零诊断，并核对 Android 影响面登记表与交叉豁免表。
    带 C 依赖的包在部分目标上被豁免（见 `scripts/cross-target-exempt.json`），它不能代替原生腿。
  - `build + test (<os>)`：`cargo build` 与 `cargo test`。

  触发与平台：
  - 推送 `main` 只跑 Linux 原生测试加交叉 clippy（纯文档改动不触发）。
  - Windows 与 macOS 原生 lint 与测试在 pull_request、手动触发、发布时跑（发布经 `package.yml` 调用，
    三平台不绿不发布）。
  - 含平台分支的改动合入前须手动触发三平台：`gh workflow run ci.yml --ref <分支>`；只确认一个平台时加
    `-f os=windows-2022` 或 `-f os=macos-14`。直接推送 `main` 不会跑这两个平台，原生腿上的失败要到下一次
    手动触发或发布时才会出现。

  本机镜像是 `scripts/gate-rust.sh`（加 `--with-cross` 才跑交叉那几条），与 `ci.yml` 由
  `scripts/gate-rust-ci-parity.test.mjs` 对拍。建议把三平台的 `fmt + clippy (<os>)`、`build + test (<os>)`
  与 `cross-target clippy + android face` 设为 required checks。
- **`ui.yml`** — 前端门禁：`pnpm run build`（tsc + vite build）、vitest、Playwright；PR / push 到 main 触发。
- **`release-risk.yml`** — 发布风险门：PR / merge queue / push 到 main 均触发，在 job 内按改动路径分类，
  只对受影响的面调用 `desktop-core.yml`、`package.yml`（不上传产物）与 `android.yml`。
- **`desktop-core.yml`** — 只被调用：四个原生 runner 从源码构建桌面内核并汇总成内核包。
- **`package.yml`** — 发布工程：Linux / Windows / macOS arm64 / macOS x64 四条腿跑 fetch + `tauri build` + 产物校验。
  触发 = tag（`v*`）/ 手动 / 被 `release-risk.yml` 调用。职责 = 「能否产出可分发安装包」。tag 发布时建草稿 release、
  上传桌面资产，再调用 `android.yml` 上传签名 APK，全量对账后公开。
- **`android.yml`** — 只被调用或手动触发：构建 release-profile APK 并做产物级检查；发布调用时产出签名 APK。

## Windows 安装器与 WebView2

Tauri 2 依赖 WebView2 Runtime。Windows 只发布一个 **`Polaris_<版本>_x64-win-setup.exe`**，使用
`tauri.conf.json` 的 `downloadBootstrapper`：普通 Win10/11 通常已预装，缺失时安装器联网获取微软
Runtime。Polaris 不内嵌、不镜像 WebView2 Runtime，也不维护第二套 Windows 安装包。

精简版 / LTSC 或便携版用户若缺少 Runtime，需要先从微软官方下载并安装
[Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。设备若完全
离线，也无法下载 Polaris 或获取订阅，因此发行链不再为该场景维护第二套安装包与校验工作流。

命名不是随意的：更新器（`crates/updater/src/github.rs`）Windows 侧**按运行形态分成两条互不相交的
选包规则**：

| 运行形态 | 选包判据 | 命中 |
|---|---|---|
| 安装态（NSIS 装的） | `Polaris_` 前缀 + `_x64-win-setup.exe` 后缀 | `Polaris_<版本>_x64-win-setup.exe` |
| 便携（解压 zip 跑的） | `Polaris_` 前缀 + `_x64-win-Portable.zip` 后缀 | `Polaris_<版本>_x64-win-Portable.zip` |

故安装器显式带 `win`，便携版是 zip，与 `.exe` 分属不相交的命名空间，两条规则各自无歧义。
这些「恰好一个」由 `verify-packaging.mjs assets` 在 CI 里机器守住。

**便携形态怎么被认出来**：便携 zip 里与 `polaris.exe` 同级有一个 `portable.marker` 文件，
应用启动检查更新时读它（`commands/updater::is_portable_layout`）。不用 `PORTABLE_EXECUTABLE_DIR`
那类环境变量——那是 electron-builder 便携目标（自解压 stub）特有的注入，本仓的便携版是
`Compress-Archive` 打的纯 zip、没有 stub，该变量恒不存在。`package.yml` 打完 zip 会开包核验
标记确实在里面，缺了就让整条腿硬失败。

⚠️ **便携版的更新是「下载 + 手动覆盖」，不是全自动**：便携用户拿到的是 zip，没有安装程序。在更新卡上点
「重启并安装」后，应用先给出说明（更新压缩包与程序所在文件夹两个位置），此时连接不受影响；用户再点
「退出并打开文件夹」，应用才按下面的次序执行（`update_install` 的便携腿）：

1. 断开连接并停止内核（与正常退出同一道准备门；内核未确认停止则应用保持运行并提示重试）；
2. 用系统打开更新压缩包与程序所在文件夹（任一个打不开则不退出）；
3. 退出应用。

随后由用户把压缩包里的全部内容解压到程序所在文件夹并选择覆盖，再重新启动。内核 `sing-box.exe` 直接从
便携目录的 `resources\win\` 运行，Windows 不允许改写正在运行的映像文件，所以停核与退出必须先于覆盖——
这一次序由应用保证，不依赖用户记得先退出。自动解压替换未做：它需要一段在主程序退出后独立运行、逐项换入
并能回滚的脚本，而被替换的是用户自己放置、权限与占用情况都不可预知的目录。
⚠️ **应用管不到的情形**：用户绕开更新卡、在应用仍运行时自己把压缩包解压进程序目录，被占用的文件会被跳过，
目录里会是新旧文件混杂。便携版不经过安装器，安装器里「先结束本安装目录内的内核」那一步在这里不存在。
⚠️ 便携运行形态下拿到安装器（或安装态拿到便携压缩包）按形态错配处理：不执行安装，交系统打开并在更新卡上
报错，不会在背后装出第二份程序。
⚠️ 用户若删掉 `portable.marker`，应用会重新把自己当成安装态、改推安装器——**没有自动手段能守住这点**，
故该文件内写明了「勿删」。
