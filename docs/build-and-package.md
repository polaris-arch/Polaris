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
| Rust | stable（edition 2021） | 后端 + `crates/` 下的域 crate（另有 `source-probe` 是 dev-only，只进 `[dev-dependencies]`，不进任何 lib/bin 依赖图） |
| Node.js | 24+（CI 钉 26） | 前端构建 + fetch 脚本 |
| pnpm | 11.24.0（由 `ui/package.json` 钉扎） | 前端包管理（`ui/`）；`tauri build` 的 `beforeBuildCommand` 直接调用 `pnpm`，须在 PATH 上 |
| Go | 所选构建路径源码清单的 `goVersion` | 源码构建与二进制构建信息检查；桌面、移动端输入分开维护 |
| Python | 3（完整移动主机测试需 3.11+，CI 使用 3.12） | 桌面固定源码归档核验及移动构建器；Windows 命令名为 `python`，其它平台为 `python3` |
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
Windows：MSVC build tools（Visual Studio Build Tools 2022 + Windows 10/11 SDK），并准备 CMake 与 NASM（CI 固定 NASM 3.02，来源与摘要见 `.github/workflows/ci.yml`）。

原生 Rust 构建还需要 `protoc`（`crates/singbox-grpc/build.rs`）。运行 `node scripts/fetch-protoc.mjs` 获取脚本固定版本与摘要的工具；本地执行只打印 `bin` 目录，不会自动修改当前 shell 的 PATH。把打印出的目录加入 PATH，再用 `protoc --version` 核对（当前 `libprotoc 35.1`）。此步骤应在 `cargo` / `tauri` 构建前完成。

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

当前桌面与移动端输入均选择 [sing-box-polaris](https://github.com/polaris-arch/sing-box-polaris) 的固定 alpha.12.5 来源。桌面发布资产与移动端源码分别由下述契约锁定；仓库名、源码提交、tag 对象和摘要须一起核验，不能机械改写旧资产 URL 或复用历史收据。

当前桌面打包以 `src-tauri/core-manifest.json` 的 `desktopRelease` 为入口，按
`scripts/desktop-core/release-contract.json` 消费固定的 [sing-box-polaris](https://github.com/polaris-arch/sing-box-polaris)
发布资产。源码提交、tag 对象、资产索引与摘要由清单锁定；不要改用 fork 默认分支的最新提交，
也不要把旧 `sourceBuild` 或移动端 `scripts/mobile-core/source-contract.json` 当成当前桌面选择。
这些是构建输入约束，不代表该 App 候选已经完成原生验收或已发布。

```bash
# 本地 Linux 资源准备；其它平台为 win、mac-x64、mac-arm64
node scripts/fetch-core.mjs --platform=linux
node scripts/fetch-cronet.mjs --platform=linux
node scripts/fetch-dashboard.mjs
```

`fetch-core` 验证固定发布输入后放入 `resources/<平台>/`。Linux / Windows 的 Cronet 动态库也必须
符合清单摘要；macOS 使用静态输入。`fetch-dashboard` 从固定 commit 获取 sing-box 面板，验证归档字节数与 SHA256，
并在复用或落盘前核对清理后的完整文件清单摘要。
资源不入库；Tauri 打包配置引用的文件必须事先准备好，不能把获取资源视作运行期在线换核功能。

#### 发布打包与原生验收包

`.github/workflows/desktop-core.yml` 按冻结清单选择路径：发布选择模式在四个平台验证固定资产，
源码模式才从固定源码构建。原生输入汇总为候选专属内核包；`package.yml` 的消费形态为：

```bash
node scripts/fetch-core.mjs --bundle-dir=<内核包目录> --candidate="$(git rev-parse HEAD)"
```

`--bundle-dir` 与 `--candidate` 必须一起提供。当前 `desktopRelease` 路径先验证原生验收包，
再消费固定资产；不能用本机仅下载一个平台的结果替代发布验收。取包时使用实际 workflow 返回的
artifact 名称和候选提交，不复用其它提交的回执。当前清单存在 `desktopRelease`，不要照旧示例
直接使用 `--producer`；源码生产路径只在无此发布选择时适用。

`tauri.conf.json` 没有 `build.beforeBundleCommand`；`tauri build` 不会代替手动准备上述资源。
CI 在 fetch 后通过 `node scripts/verify-packaging.mjs confs` 检查资源存在且非空，
产物仍须通过后续 payload / assets 与原生验收。下载成功、摘要正确和运行验收是不同检查。

维护者调整桌面内核选择时须同步审查 `desktopRelease`、发布契约及其摘要，并核对实际固定发布资产。
移动端源码清单和历史 source-mode 输入分别维护，不能只改 `bundledCoreVersion` 或旧 `sourceBuild`
就宣称桌面选择已更新。内核随 App 配套交付；本说明不提供用户独立上传、在线替换或回滚内核的入口。

### 打包出安装包

本节的产物命名、资产表及下文三份 Android APK 描述的是当前 `main` 源码的构建与发布流程约定，供维护者准备候选包使用，不是现有公开下载清单，也不表示候选产物已经发布或通过安装验收。正式包发布后，下载请以 [README 安装说明](../README.md#安装) 和 [Releases](https://github.com/polaris-arch/Polaris/releases) 的实际资产为准。

以下为单平台本地完整准备顺序，须在对应操作系统、干净且已提交的源码 checkout 根目录执行，并先满足上文工具链与系统依赖。`platform` 只选当前目标，macOS 可显式选择 Intel 目标；Windows 使用已具备 MSVC 环境的 Git Bash。这是源码构建，不是正式发布命令。

```bash
# Bash; on Windows use Git Bash with the MSVC build environment available.
set -euo pipefail
platform=linux  # linux | win | mac-arm64 | mac-x64
case "$platform" in
  linux) target=x86_64-unknown-linux-gnu; conf=linux; helper=polaris-helper ;;
  win) target=x86_64-pc-windows-msvc; conf=windows; helper=polaris-helper.exe ;;
  mac-arm64) target=aarch64-apple-darwin; conf=macos-arm64; helper=polaris-helper ;;
  mac-x64) target=x86_64-apple-darwin; conf=macos-x64; helper=polaris-helper ;;
  *) echo "Unsupported platform" >&2; exit 1 ;;
esac
npm install -g "@tauri-apps/cli@$(cat scripts/tauri-cli.version)"
pnpm --dir ui install --frozen-lockfile
# protoc must already be on PATH as described under system dependencies.
protoc --version
export POLARIS_BUILD_ID="$(git rev-parse HEAD)"
node scripts/fetch-core.mjs --platform="$platform"
case "$platform" in
  linux|win) node scripts/fetch-cronet.mjs --platform="$platform" ;;
esac
node scripts/fetch-dashboard.mjs
args=(--target "$target")
out="target/$target/release"
case "$platform" in
  linux|win)
    test "$(rustc -vV | sed -n 's/^host: //p')" = "$target"
    args=(); out=target/release ;;
  mac-*) rustup target add "$target" ;;
esac
cargo build --release -p polaris-helper --bin polaris-helper "${args[@]}"
mkdir -p "resources/$platform"
cp "$out/$helper" "resources/$platform/$helper"
case "$platform" in
  mac-*)
    if ! codesign --verify --strict "resources/$platform/$helper"; then
      codesign --force --sign - --timestamp=none "resources/$platform/$helper"
    fi
    codesign --verify --strict "resources/$platform/$helper" ;;
  win)
    cargo build --release -p polaris-windows-cleaner --bin polaris-cleaner
    cp "$out/polaris-cleaner.exe" resources/win/polaris-cleaner.exe ;;
esac
node scripts/prepare-core-identity.mjs --platform="$platform"
export POLARIS_CORE_IDENTITY_FILE="$PWD/resources/$platform/core-manifest.json"
tauri build --config "src-tauri/tauri.$conf.conf.json" "${args[@]}"
node scripts/prepare-core-identity.mjs --platform="$platform" --mode=verify
```

helper、Windows cleaner 和 App 必须共享同一 `POLARIS_BUILD_ID`。macOS 的 helper 先做 ad-hoc 密封，再生成安装身份；这不是 Developer ID 签名或公证。`prepare-core-identity.mjs` 冻结实际随包字节，App 编译消费同一清单；后续改动 helper/内核字节必须重新准备并重编 App，不能只替换文件。缺少身份输入时分析构建可能产生不可用占位，不应把编译通过当作可安装运行的完整包。正式发布还需同一候选的原生验收包、payload/assets/inventory、签名与完整发布 DAG；本地步骤不替代这些门禁。`verify-packaging.mjs confs` 会检查四个平台的资源目录，需要全部四套资源准备齐全；它不是此单平台构建步骤的一部分。

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

### 移动端源码构建前置条件

桌面 sidecar 下载不能替代移动端绑定组件。当前共同输入见 [source-contract.json](../scripts/mobile-core/source-contract.json) 和 [移动内核构建说明](../scripts/mobile-core/README.md)：固定 sing-box-polaris 源码、Go 1.27.2、gomobile/gobind v0.1.13，先生成并核验组件及原始收据，再由 App 消费。

- **Android 绑定组件**：API 24 AAR，Java 17（CI 固定 17.0.20+101）、稳定 NDK r30（完整来源版本 30.0.16248370）。AAR 含 ARM64、ARMv7、x86、x86_64 四种 ABI；这不扩大正式 APK 的两种 ARM ABI 范围。
- **Android App**：Gradle 使用 JDK 21，SDK/Build Tools 当前为 36 / 36.0.0；Rust 使用 [android-rust-ndk.version](../scripts/android-rust-ndk.version) 固定的 27.3.13750724，不能与绑定组件 NDK 混用。App 最低 API 24。
- **组件准备顺序**：共享入口为 `bash scripts/build-mobile-core.sh android`，须显式提供已有 source、Go、gomobile、gobind、SDK、NDK、JDK、work/cache 路径（完整参数见移动内核说明）。随后由 `scripts/mobile-core/consume.py materialize-android` 核验组件和收据再写入 App 输入；CI 的 `ci_android.py prepare/build` 编排此过程。仅把任意 AAR 复制为 `libbox.aar` 不满足当前来源核验。
- **iOS / iPadOS**：源构建入口、固定 Xcode/SDK、ARM64 设备与模拟器目标及个人签名条件见 [iOS 源码构建](ios-source-build.md)。没有已发布 IPA 不影响源码构建文档的提供；编译/链接、签名、真机安装和运行验收是不同阶段。

### Android 当前源码签名发布构建

| ABI | 当前源码打包名（非已发布清单） |
|---|---|
| ARMv8 | `Polaris_<版本>_arm64-v8a-android.apk` |
| ARMv7 | `Polaris_<版本>_armeabi-v7a-android.apk` |
| ARMv8 + ARMv7（universal，不含 x86） | `Polaris_<版本>_universal-android.apk` |

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

这些不变量由 `node scripts/verify-packaging.mjs confs` 机器守住（CI 每次打包前跑；本机须先准备全部四个平台资源再复现）；
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

主要 workflow 分工（`.github/workflows/`）：

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
  只对受影响的面调用 `desktop-core.yml`、`package.yml`、`android.yml` 与 `apple.yml`。常规桌面与 Android 风险腿使用轻量构型，安装包 / APK 不上传、不可分发。独立的 `native_payload` 原生验收路径使用完整构型，可上传候选桌面包与证据，但不是公开 Release，也不自动证明安装或设备验收；应按具体 job 的构型与实际附件判断。
- **`desktop-core.yml`** — 只被调用：四个原生 runner 验证固定发布输入，或在源码模式下构建固定源码，再汇总候选内核包。
- **`package.yml`** — 发布工程：Linux / Windows / macOS arm64 / macOS x64 四条腿跑 fetch + `tauri build` + 产物校验。
  触发 = tag（`v*`）/ 手动 / 被 `release-risk.yml` 调用。发行构型用于构建可分发安装包；风险门轻量构型只验证打包链。tag 发布时建草稿 release、
  上传桌面资产，再调用 `android.yml` 上传签名 APK，全量对账后公开。
- **`android.yml`** — 只被调用或手动触发：构建 release-profile APK 并做产物级检查；发布调用时产出三份正式签名 APK。
- **`apple.yml`** — 由风险门选择或手动触发：默认核验设备 ARM64 App / PacketTunnel 的无签名编译与链接，模拟器 App 为显式选项；不发布 IPA，不证明设备安装、NE 授权或 VPN 运行。完整组件与证据约束见 [移动内核说明](../scripts/mobile-core/README.md)。

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

## 随包内核与提权助手

桌面正式包使用本版本选定的 sing-box-polaris 内核。Linux、Windows 的 Cronet 库与内核同包；macOS 使用静态输入。打包阶段先冻结实际 helper（Windows 还含 cleaner）字节，再生成 `core-manifest.json` 并作为 App 编译输入；最终包装必须与这份输入逐字节一致。正式版不接受用户上传内核或环境变量指定的替代内核。

TUN 首次使用或选定内容身份改变时，经管理员授权事务安装本版本配套内核。助手按 CoreID 存放不可变版本，启动请求只携带 CoreID，不上传文件。兼容的较新助手可保留；安装另一版本内核不会主动停止其他用户的活跃核。旧助手仍占用受管核或无法证明空槽时，升级会延期，不能把观察快照当作停止授权。

助手保留最近三个历史内容身份以及所有仍持有原生租约的版本。旧平铺核和用户目录遗留缓存只在未使用且观察完整时退役；无法确认时保留。公开发布及旧资产删除另按实际消费验收后的活跃引用迁移执行。

应用自更新保留发行时冻结的 helper/core 字节。macOS 对暂存与最终应用包校验签名，不做深度重签；macOS、AppImage 在目标旁暂存候选并串行替换，失败恢复本次操作前的原文件，恢复未完成时保留准确事务目录。失败不删除已下载安装件。这些源码路径仍需原生载体验证。
