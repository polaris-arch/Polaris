#!/usr/bin/env node

/**
 * 把一次提交的路径差集分类为发布风险面。
 *
 * 输出是稳定 JSON：
 *   - kernel: 是否必须下载真实随包核并执行强制内核门；
 *   - platforms: 必须实际构建安装包的平台腿；
 *   - preflight: 是否需要静态打包/内核预检；
 *   - hasPackage: 是否存在安装包构建腿；
 *   - android: 是否必须跑 Android APK 腿（`.github/workflows/android.yml`）：登记根内的改动、表一命中，
 *     或已有内核门 / 安装包腿时为真（见 [`classifyImpact`]）；
 *   - unregisteredScopes: 落在登记根内、却在两张登记表里都查不到的 scope（fail-closed 的自曝面）。
 *
 * 路径判据由代码持有，workflow 只负责取得 diff 并消费输出。
 * 无法取得可靠 diff 时由调用方传 --full，故障关闭为四平台 + 内核门。
 *
 * ── 两级默认（2026-08-30）：为什么 `crates/` / `src-tauri/` / `resources/` 之内是 fail-closed ──
 *
 * 此前**全部**路径共用一条默认：「未命中任何判据 ⇒ 什么都不加」。它对 ui/、docs/ 是对的，
 * 对 `crates/`、`src-tauri/` 与 `resources/` 是 **fail-open**：新增一个 crate、新建一个 src 子树、
 * 往 `resources/` 加一个随包子目录，它就永久落在全部打包门之外，且没有任何东西会提醒。实测形态（2026-08-30）：改
 * `crates/system-integration/`、`crates/helper-client/`、`crates/stats-engine/`、
 * `src-tauri/src/runtime/` 得到 `kernel=false platforms=[] preflight=false hasPackage=false`
 * ⇒ release-risk.yml 的 preflight / package 两个 job 全 skip ⇒ gate 的三条断言在 required
 * 为 false 时全不生效 ⇒ **绿，但零信息量**。
 *
 * 修法是把「有没有判过」和「判成什么」拆开：登记根之内的每个 scope 必须显式出现在
 * [`PACKAGE_IMPACT_SCOPES`]（影响打包，附平台/内核门）或 [`NO_PACKAGE_IMPACT_SCOPES`]
 * （已判定不影响，附理由）之一。两张表都查不到 ⇒ 按内核门 + 四平台处理，并把 scope 写进
 * `unregisteredScopes` + stderr 自曝。完备性由
 * `ui/src/contracts/ci-impact-coverage-contract.test.ts` 在文件系统侧硬钉：新增 crate /
 * 新建 src 子树而没登记 ⇒ 该门红。
 *
 * 登记根之外（ui/、scripts/、packaging/、.github/、仓库根文件）仍是枚举表 + 未命中不加腿：
 * 那一侧没有可枚举的边界，改用两条反向断言兜（同一个契约测试）：打包 workflow 真正
 * `run:` 的每个仓库脚本、以及 verify-packaging 当判据读的每个源文件，都必须触发打包腿。
 */

import { appendFileSync, readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { appleRequired } from './apple-ci-impact.mjs';

export const ALL_PACKAGE_PLATFORMS = Object.freeze([
  'linux',
  'windows',
  'macos-arm64',
  'macos-x64',
]);

const ALL = ALL_PACKAGE_PLATFORMS;
const MACOS = Object.freeze(['macos-arm64', 'macos-x64']);
const NO_LEG = Object.freeze([]);

/**
 * fail-closed 的作用域根。这些树的内容全部编译进安装包、随包分发或参与打包契约，
 * 且 scope 数量有限可枚举 —— 所以「没登记」在这里是错误，不是默认放行。
 *
 * `resources/` 是 2026-08-30 补上的第三条姊妹腿：`resources/data/` 的 28 个 `.srs` **入库**
 * （`git ls-files resources/data` = 28；.gitignore 以 `/resources/*` + `!/resources/data/` 放行），
 * 四份 conf 的 `bundle.resources` 都含 `../resources/data/` ⇒ 改一个字节，四个安装包的字节都变，
 * 而此前分类器对 `resources/**` 输出 `kernel=false platforms=[] hasPackage=false`（实测），
 * 与改 `crates/`、改 `src-tauri/` 是同一个 fail-open。
 */
export const REGISTRY_ROOTS = Object.freeze(['crates/', 'src-tauri/', 'resources/', 'vendor/']);

/**
 * scope 粒度：`crates/<name>/`、`src-tauri/<entry>`、`src-tauri/src/<entry>`。
 * 顺序即优先级（长根先匹配）。
 */
const SCOPE_DEPTH = Object.freeze([
  ['src-tauri/src/', 3],
  ['src-tauri/', 2],
  ['crates/', 2],
  // `resources/<子树>/`：data/ dashboard/ 与四个平台目录各自一个 scope；
  // 根下的散文件（.gitkeep）按单文件 scope 归一。
  ['resources/', 2],
  ['vendor/', 2],
]);

/**
 * 把登记根内的任意路径归到它所属的 scope key。
 * 目录 scope 带尾斜杠，单文件 scope 就是文件路径本身；不在登记根内返回 null。
 *
 * 契约测试用同一个函数把文件系统条目翻成 key —— 两侧共用一份归一，避免
 * 「测试期望 `crates/x/`、表里写的是 `crates/x`」这种只在改判据时才暴露的错配。
 */
export function scopeOf(rawPath) {
  const path = normalized(rawPath);
  for (const [root, depth] of SCOPE_DEPTH) {
    if (!path.startsWith(root)) continue;
    const parts = path.split('/');
    if (parts.length < depth) return null;
    if (parts.length === depth) return parts.join('/');
    return `${parts.slice(0, depth).join('/')}/`;
  }
  return null;
}

/**
 * ── 表一：影响打包 ──
 *
 * `kernel` = 必须下载真核跑强制内核门；`platforms` = 必须真出安装包的腿；`why` = 判据。
 * 命中本表即点亮 Android APK 腿；`kernel=false` 且无桌面腿的条目只为它而登记。
 * key 比 scope 更细的条目（如 `crates/helper/src/platform/windows/`）按最长前缀胜出，
 * 用来把「整棵树四平台」收窄成单腿。
 */
export const PACKAGE_IMPACT_SCOPES = Object.freeze({
  'scripts/prepare-core-identity.mjs': {
    kernel: false, platforms: ALL,
    why: 'Pre-compilation helper/cleaner/core identity and final byte verification feed all four desktop carriers.',
  },
  'scripts/release-assets.mjs': {
    kernel: false,
    platforms: ALL,
    why: '正式发布资产命名与最终摘要清单由构建、聚合及更新器对拍消费；改动须重验全部桌面包的名称、资产集合和校验和。',
  },
  'scripts/windows-package-integrity.mjs': {
    kernel: false,
    platforms: ['windows'],
    why: 'Windows 打包使用该入口冻结输入、生成安装包并核对载荷。',
  },
  '.github/actions/install-windows-nasm/action.yml': {
    kernel: false,
    platforms: ['windows'],
    why: 'Windows 打包用该 action 安装 btls-sys 原生构建所需的 NASM。',
  },
  'scripts/mobile-core/source-contract.json': {
    kernel: false,
    platforms: ALL,
    why: 'NOTICE 的内核段落与该移动端发布合同对拍（verify-packaging confs），改它须重验全部桌面包。',
  },
  'scripts/mobile-core/': {
    kernel: false,
    platforms: NO_LEG,
    why: 'Formal zero-overlay mobile source, SDK producer, native-input/cache and final-carrier predicates feed the actual Android APK leg; host source tests also cover Apple binding/link controls.',
  },
  'scripts/build-android-apk.mjs': {
    kernel: false,
    platforms: NO_LEG,
    why: '标准 Android 构建入口统一 Rust NDK 并驱动一次 Tauri/Gradle 构建；改坏它会让 APK 腿跳过或混用工具链。',
  },
  'scripts/android-rust-ndk.version': {
    kernel: false,
    platforms: NO_LEG,
    why: 'Rust Android 构建默认使用的已验稳定 NDK 版本；改版本须重跑 APK 腿验证交叉编译与 Gradle。',
  },
  'scripts/strip-android-release-native.mjs': {
    kernel: false,
    platforms: NO_LEG,
    why: 'Release native producer strips copied libraries before APK packaging; changes require the actual Android APK and source-receipt checks.',
  },
  'scripts/strip-android-release-native.test.mjs': {
    kernel: false,
    platforms: NO_LEG,
    why: 'Mutation checks preserve runtime/JNI/Go metadata and source AAR bytes during release stripping; changes must retain actual Android package validation.',
  },
  'scripts/verify-apk.mjs': {
    kernel: false,
    platforms: NO_LEG,
    why:
      'APK 腿的产物级判据本体（三份许可文本逐字节对拍 / libbox.so 的 naive+cronet 指纹 / .srs 份数 / '
      + '死字节 / 剥符号 / **出厂权限集逐条全等**）。权限那条（判据 ⑦）是全仓唯一在产物侧问'
      + '「这个 APK 到底申请了哪些权限」的地方：源码侧那道门读的是本仓自有 manifest（8 条），'
      + '而 APK 带的是 manifest merger 合并库 manifest 之后的集合（本树实测 11 条）——'
      + '依赖注入一条敏感权限时，只有它会红。',
  },
  'scripts/verify-apk.test.mjs': {
    kernel: false,
    platforms: NO_LEG,
    why: '判据本体的变异测试：判据有没有牙由它守，改判据而不改它 = 那些否定用例可能已经不成立。',
  },
  'scripts/assert-r8-evidence.mjs': {
    kernel: false,
    platforms: NO_LEG,
    why:
      'release 冒烟腿的产物级判据本体：configuration.txt（剥注释后）证明各来源的规则文本到过 R8，'
      + 'seeds.txt 证明那些 keep 真的匹配到了类与成员。两份产物只在 APK 腿的 release 构建里存在。',
  },
  'scripts/assert-r8-evidence.test.mjs': {
    kernel: false,
    platforms: NO_LEG,
    why: '上一条判据的变异测试：判据有没有牙由它守（含把「针被本仓注释喂绿」那条真缺陷原样回放）。',
  },
  '.github/workflows/android.yml': {
    kernel: false,
    platforms: NO_LEG,
    why: 'APK 腿本体（工具链解析、libbox 来源、Tauri 构建、开箱验的调用点全在这一份里）。',
  },
  'vendor/swift-rs/': {
    kernel: false,
    platforms: MACOS,
    why: 'Swift 构建桥接仅用于 Apple 平台；移除旧路径依赖也要验证 macOS 打包，iOS 由 Apple 构建门覆盖。',
  },
  'crates/config-engine/': {
    kernel: true,
    platforms: NO_LEG,
    why:
      'sing-box config 的生成与 schema 真值；四道强制内核门（core_dep_fingerprint / core_schema_surface / '
      + 'core_build_matrix / kernel_accepts_outbounds）都是本 crate 的 tests，必须拿真核回放。不改包内容 ⇒ 不加腿。',
  },
  'crates/singbox-grpc/': {
    kernel: true,
    platforms: NO_LEG,
    why: '随包核的 gRPC(h2c) API 面，核换版即断 ⇒ 走内核门；纯 lib，不改包内容 ⇒ 不加腿。',
  },
  'crates/net-stack/src/singbox_import.rs': {
    kernel: true,
    platforms: NO_LEG,
    why:
      '本地 sing-box JSON 导入：`local_import_round_trips_through_the_bundled_core` 拿真核 check 导入产物，'
      + 'ci.yml 的 test job 不拉核会静默跳过，只有内核门那一步能让它真跑。不改包内容 ⇒ 不加腿。',
  },
  'crates/net-stack/src/singbox_import/': {
    kernel: true,
    platforms: NO_LEG,
    why: '同 `singbox_import.rs`：模块的测试实体与真核往返测试都在这里。',
  },
  'crates/net-stack/src/clash_parser.rs': {
    kernel: true,
    platforms: NO_LEG,
    why: 'mihomo 协议映射生成 sing-box 配置；远端订阅协议对齐门须拿随包核 check。不改包内容。',
  },
  'crates/net-stack/src/clash_parser/': {
    kernel: true,
    platforms: NO_LEG,
    why: '同 `clash_parser.rs`：协议映射与回归测试在这里。',
  },
  'crates/net-stack/tests/fixtures/assets/openvpn-test-ca.txt': {
    kernel: true,
    platforms: NO_LEG,
    why: '导入协议真核门使用的公开测试 CA；证书夹具变化同样须回放内核 check。',
  },
  'crates/helper/': {
    kernel: false,
    platforms: ALL,
    why:
      '随包特权 helper 二进制本体：package.yml 单独构建 polaris-helper（mac 显式 target；Linux/Windows 核对宿主后共享 App target）后铺进 '
      + 'resources/<平台>/，是包内资产（2026-08-10 三平台出过不含它的包）。',
  },
  'crates/helper/src/platform/linux/': {
    kernel: false,
    platforms: Object.freeze(['linux']),
    why: 'helper 的 linux 平台实现，只改 linux 那份二进制。',
  },
  'crates/windows-cleaner/': {
    kernel: false,
    platforms: ['windows'],
    why: '随 Windows 包分发/NSIS 嵌入的固定 native cleaner；SCM/支持目录卸载需要 Windows 原生与安装器验证，不执行真实网络内核。',
  },
  'crates/helper/src/platform/windows/': {
    kernel: false,
    platforms: Object.freeze(['windows']),
    why: 'helper 的 windows 平台实现，只改 windows 那份二进制。',
  },
  'crates/helper/src/platform/macos/': {
    kernel: false,
    platforms: MACOS,
    why: 'helper 的 macOS 平台实现，只改两条 mac 腿的二进制。',
  },
  'crates/helper-proto/': {
    kernel: false,
    platforms: ALL,
    why:
      'app ↔ 随包 helper 的线协议。协议改动要求包内 helper 与 app 同版（package.yml 用同一 checkout '
      + 'commit 做构建身份），只有真出一次包才能验到 staging 与协议同版。',
  },
  'crates/updater/': {
    kernel: false,
    platforms: ALL,
    why:
      'verify-packaging.mjs 的「产物命名 ↔ updater 选包」与体积门以 github.rs::find_suitable_update_asset '
      + '为口径 —— 判据源改了必须四平台重跑产物命名断言。',
  },

  'src-tauri/core-manifest.json': {
    kernel: true,
    platforms: ALL,
    why: '随包核版本与资产钉扎的唯一真值：既动内核（内核门）又动包内资产（四平台）。',
  },
  'src-tauri/Cargo.toml': {
    kernel: false,
    platforms: ALL,
    why: '主二进制的依赖与 feature 面，四平台的包都变。',
  },
  'rust-toolchain.toml': {
    kernel: false,
    platforms: ALL,
    why: '固定 Rust 编译器与组件，影响所有平台的原生构建。',
  },
  'src-tauri/build.rs': {
    kernel: false,
    platforms: ALL,
    why:
      '构建期钩子（随包 dashboard / geo 完整性断言、Windows manifest 嵌入），直接决定构建能否出包；'
      + '且它把 tauri.conf.json 的 productName 用 cargo:rustc-env 注入，是 Rust 侧那个值的唯一来源 '
      + '（Linux deb/AppImage 的 /usr/lib/<productName>/ 资源目录名靠它）。'
      + 'verify-packaging inventory 还读它的 EXPECTED_SRS_COUNT 当 .srs 份数判据。',
  },
  'src-tauri/tauri.conf.json': {
    kernel: false,
    platforms: ALL,
    why: 'base bundle 配置（resources / targets / 图标 / CSP），四平台 conf 都以它为底。',
  },
  'src-tauri/capabilities/': {
    kernel: false,
    platforms: ALL,
    why: 'Tauri ACL 能力清单，随包生效且构建期校验。',
  },
  'src-tauri/icons/': {
    kernel: false,
    platforms: ALL,
    why: '包内图标资产（含 NSIS installerIcon / icns / ico）。',
  },
  'src-tauri/permissions/': {
    kernel: false,
    platforms: ALL,
    why: '自定义权限定义，随 capabilities 参与构建期 ACL 校验。',
  },
  'src-tauri/tauri.linux.conf.json': {
    kernel: false,
    platforms: Object.freeze(['linux']),
    why: 'linux 腿的 conf（含该腿的内核目录），只影响 linux 包。',
  },
  'src-tauri/nsis-hooks.nsh': {
    kernel: false,
    platforms: Object.freeze(['windows']),
    why: 'NSIS 安装/卸载钩子，只进 windows 安装器。',
  },
  'src-tauri/nsis-installer.nsi': {
    kernel: false,
    platforms: Object.freeze(['windows']),
    why: 'NSIS 安装器模板，只进 windows 安装器。',
  },
  'src-tauri/nsis-languages/': {
    kernel: false,
    platforms: Object.freeze(['windows']),
    why:
      'tauri.conf.json 的 nsis.customLanguageFiles 指向本目录（Farsi.nsh），既是包内安装器语言资产，'
      + '又被 verify-packaging.mjs 的 confs 模式按 conf 解析后读取。',
  },
  'src-tauri/tauri.windows.conf.json': {
    kernel: false,
    platforms: Object.freeze(['windows']),
    why: 'windows 腿的 conf（含该腿的内核目录），只影响 windows 包。',
  },
  'src-tauri/Info.plist': {
    kernel: false,
    platforms: MACOS,
    why: 'macOS bundle 的 Info.plist，只进两条 mac 腿。',
  },
  'src-tauri/tauri.macos-arm64.conf.json': {
    kernel: false,
    platforms: MACOS,
    why:
      'mac 腿的 conf。两份 mac conf 一起选两条 mac 腿：产物命名/内核目录的对称性错配只有把另一条腿'
      + '一起打出来才看得见（收窄成单腿属独立取舍，改判据时一并改）。',
  },
  'src-tauri/tauri.macos-x64.conf.json': {
    kernel: false,
    platforms: MACOS,
    why: '同上（mac 两条腿成对验证）。',
  },
  // ── resources/：随包资源载荷树（2026-08-30 纳入登记根）──
  'resources/.release-receipts/': {
    kernel: true,
    platforms: ALL,
    why: '公开 release 消费的构建期原包、合同与 readback 收据；虽不直接入包，随核核验读取其绑定，变化必须触发四桌面打包门。',
  },
  'resources/data/': {
    kernel: true,
    platforms: ALL,
    why:
      '内置 geo 规则集的**入库**出厂种子（28 个 .srs，`git ls-files resources/data` 可查）。四份 conf 的 '
      + 'bundle.resources 都含 `../resources/data/` ⇒ 改/增/删一个文件，四个安装包的字节都变。观测面确实挂在打包腿上：'
      + 'verify-packaging inventory 的 `geo-srs` 规则按 min=max=build.rs::EXPECTED_SRS_COUNT 清点（多一个少一个都红），'
      + 'build.rs 的 release 断言再逐个校 SRS 魔数。缺失/损坏的后果是 runtime_rules_dir 种不满 → route builder '
      + 'fail-closed 剪掉全部 geo 规则 → 叠加回国模式即全量明文直连（真机 2026-07-20）。'
      + '完整配置真核门 `kernel_accepts_full_config` 逐份读取这些真实 `.srs` 并在 check 初始化 route.rule_set，'
      + '故必须 kernel=true：缺失、损坏或格式版本漂移会在 release-risk 的 Fetch core/cronet 后立刻转红。',
  },
  'resources/dashboard/': {
    kernel: false,
    platforms: ALL,
    why:
      '随包 sing-box 面板静态站，四份 conf 都 bundle 它。目录被 .gitignore（`/resources/*`），CI 由 '
      + 'scripts/fetch-dashboard.mjs 现拉 ⇒ 正常 diff 里不会出现；一旦出现（`git add -f` 往包里塞文件），'
      + '要判的正是四条腿的 inventory 白名单（dashboard-entry / assets / licenses / icons 四条规则）。',
  },
  'resources/linux/': {
    kernel: false,
    platforms: Object.freeze(['linux']),
    why:
      'linux 腿的随包内核 / libcronet / polaris-helper 落位目录（.gitignore，CI 由 fetch-core '
      + '与 helper 构建现铺）。只有 tauri.linux.conf.json 引它 ⇒ 只改 linux 包，由该腿的 inventory 白名单逐条对账。'
      + '核本身的版本真值在 core-manifest.json（已单列 kernel:true），这里只是落位目录 ⇒ 不重复挂内核门。',
  },
  'resources/win/': {
    kernel: false,
    platforms: Object.freeze(['windows']),
    why: '同 resources/linux/，windows 腿的随包二进制落位目录（sing-box.exe / libcronet.dll / polaris-helper.exe）。',
  },
  'resources/mac-arm64/': {
    kernel: false,
    platforms: Object.freeze(['macos-arm64']),
    why:
      '同 resources/linux/，macos-arm64 腿的落位目录。这里**不成对选两条 mac 腿**（与两份 mac conf 的取舍不同）：'
      + 'conf 之间有产物命名/内核目录的对称性要一起验，而资源目录各自只进各自那个包，inventory 也只清点本腿那棵树。',
  },
  'resources/mac-x64/': {
    kernel: false,
    platforms: Object.freeze(['macos-x64']),
    why: '同上，macos-x64 腿的落位目录。',
  },
});

/** 全 crate/子树共享的默认理由：纯应用逻辑，正确性由 ci.yml 三平台 fmt+clippy+build+test 覆盖。 */
const APP_LOGIC = (what) =>
  `${what}；纯 Rust 逻辑，编译进主二进制，不改安装包结构 / 包内资产 / 随包核契约，`
  + '正确性由 ci.yml 三平台 fmt+clippy+build+test 覆盖。';

/**
 * ── 表二：已判定不影响打包 ──
 *
 * value 是理由。在这里 = 「有人看过、判过」，不是「没人管过」。
 */
export const NO_PACKAGE_IMPACT_SCOPES = Object.freeze({
  'crates/tauri-plugin-polaris-ios/':
    'iOS 原生 VPN/viewport bridge；runtime 依赖及 Swift 构建均限 target_os=ios。当前桌面/Android包不链接它，'
    + '无 iOS CI 发布腿；共享纯 Rust lifecycle 测试由 workspace test 覆盖，Apple目标另以实际Xcode构建验证。',
  'src-tauri/Info.ios.plist':
    'iOS scene/bundle 配置，Tauri 仅在 iOS 构建消费；当前桌面与 Android 包不读取。无 iOS CI 发布腿，实际Xcode归档另验。',
  'crates/core-supervisor/': APP_LOGIC('sing-box 进程 spawn / readiness / 崩溃自愈；核路径由调用方传入'),
  'crates/helper-client/': APP_LOGIC(
    'app 侧的 helper 客户端（连接/装卸/token）。线协议真值在 helper-proto（已在影响表），本 crate 不产出包内二进制',
  ),
  'crates/log-budget/': APP_LOGIC('日志预算限流'),
  'crates/mesh/': APP_LOGIC('Tailscale / WARP 组网状态与出口路由；不随包任何第三方二进制'),
  'crates/net-stack/': APP_LOGIC('订阅拉取与分享链接/配置导入解析'),
  'crates/platform-events/': APP_LOGIC(
    '平台网络事件的解析与归一（macOS/Linux route monitor 文本、Linux ip monitor label、'
    + 'Windows IP Helper row → RoutePrefix / NetworkChangeImpact）与运行期绑定计划的数据模型；'
    + '从 src-tauri/src/runtime/ 下沉（E2②），纯 Rust 文本解析，不产出包内二进制、不碰随包核契约',
  ),
  'crates/stats-engine/': APP_LOGIC('连接统计聚合、诊断报告与脱敏'),
  'crates/source-probe/':
    '测试取材锚点 helper（按 CARGO_MANIFEST_DIR / workspace 根读源码）。**dev-only**：只被各 crate 的 '
    + '`[dev-dependencies]` 引用，不进任何 lib/bin 依赖图 ⇒ 不编译进主二进制、不产出包内资产、不碰随包核契约；'
    + '零第三方依赖（只用 std）⇒ Cargo.lock 与 THIRD-PARTY-LICENSES 均无新增。正确性由 ci.yml 三平台 '
    + '`cargo test --workspace` 覆盖。',
  'crates/store/': APP_LOGIC('用户配置持久化、迁移与备份（运行期用户目录，非包内资产）'),
  'crates/switch-engine/': APP_LOGIC('节点切换决策与热切执行'),
  'crates/system-integration/': APP_LOGIC(
    '系统代理 / DNS / 路由的平台操作（macos/windows/linux 子模块按 cfg 编译进主二进制，不是独立随包二进制）',
  ),
  'crates/unlock/': APP_LOGIC('流媒体解锁检测'),
  'crates/unlock-transport/': APP_LOGIC('解锁检测的传输端口抽象'),

  'src-tauri/tauri.android.conf.json':
    'Android 包的平台 conf（identifier 覆盖 + 按平台重筛 bundle.resources）。**不影响任何现有打包腿的产物**：'
    + 'Tauri 只按当前构建平台自动合并同名 conf，package.yml 的 linux/windows/mac 四条腿都显式传自己那份 '
    + '--config，构建过程从不读这一份 ⇒ kernel=false、platforms=[]（写 platforms 非空是给表本身撒谎：'
    + '后来人会据表推断「改它会重打某个桌面包」）。'
    + '🔴 **已知缺口**：release-risk.yml 的「Verify packaging conf invariants」步骤条件是 has_package==true，'
    + '因此只改本文件时 `verify-packaging.mjs confs`（含 NON_CORE_CONFS 的登记与内容判据）在 CI 上不跑。'
    + '缺口的合上点是 Android 真正进 package.yml 矩阵那一批：届时本条改为 PACKAGE_IMPACT_SCOPES 的 android 腿，'
    + '且 verify-packaging 的 NON_CORE_CONFS[...].ciLeg 从 null 改成那条腿的 label（该字段本身会在 CI 腿出现'
    + '而登记未更新时转红）。在那之前，本文件的判据由本机全量门（gates-wt.sh 的 01-pkgconfs）承担。',

  'src-tauri/tauri.ios.conf.json':
    'iOS 包的平台 conf（按平台重筛 bundle.resources；没有 identifier 覆盖，理由在 verify-packaging.mjs 的 '
    + 'NON_CORE_CONFS 里）。**不影响任何现有腿的产物**，且比 android 那份更彻底：本仓至今没有任何 workflow '
    + '构建 iOS（无 macOS/Xcode），Tauri 又只按当前构建平台自动合并同名 conf ⇒ 四条桌面腿与 android 腿都不会'
    + '读到它 ⇒ kernel=false、platforms=[]。'
    + '🔴 这一点由 verify-packaging 的 `NON_CORE_CONFS[\'tauri.ios.conf.json\'].ciLeg = null` **主动断言**：'
    + '那条判据按整个 .github/workflows/ 目录枚举「提到这份 conf 的文件集合」，null 要求该集合恰为空。'
    + '哪天有人加了 iOS 腿，那边先红，本条也要跟着改成 PACKAGE_IMPACT_SCOPES 的 ios 腿。'
    + '与 android 那条同一个已知缺口：只改本文件时 `verify-packaging.mjs confs` 在 CI 上不跑'
    + '（release-risk.yml 的条件是 has_package==true），判据由本机全量门承担。',

  'src-tauri/gen/':
    '平台生成子树不影响桌面安装包：`gen/schemas` 是 tauri-build 构建期重生成的 ACL/schema 产物，'
    + '不是包内资产（生成源 capabilities/ 与 permissions/ 已在影响表）；`gen/android` 是**入库的** Gradle 工程'
    + '（入库工程），四条桌面腿一个字节都不读它；`gen/apple` 是 iOS 源码自编译的 Xcode 工程，'
    + '同样不被桌面/Android腿读取，当前无 iOS CI 发布腿。'
    + '本条只说「不加桌面腿」：登记根内的改动一律点亮 Android APK 腿。',
  'src-tauri/tests/':
    'app crate 的集成测试，只进测试二进制、不进包；由 ci.yml 三平台 `cargo test` 覆盖。',
  'src-tauri/windows-test-manifest.xml':
    'build.rs 只用 `rustc-link-arg-tests` 把它嵌进**测试**二进制（应用 exe 的 manifest 由 tauri-build 的 winres 给），'
    + '不进包；由 ci.yml windows 腿覆盖。',

  'src-tauri/src/android_tls.rs':
    'Android 平台证书校验器（rustls-platform-verifier）的 JNI 初始化与「它到场了」的运行期断言。'
    + '整个模块是 `#[cfg(target_os = "android")]`，**桌面三平台一行都不编译它** ⇒ 四个桌面包的结构、'
    + '包内资产、随包核契约都不受影响。'
    + '🔴 所以这里刻意**不用** `APP_LOGIC()`：那句模板写着「正确性由 ci.yml 三平台 fmt+clippy+build+test 覆盖」，'
    + '对本文件是假话 —— 三平台的 build/test 根本走不到这段代码。它的覆盖面是另外三样：'
    + 'ci.yml 的 `aarch64-linux-android` 交叉 clippy、`src-tauri/tests/android_platform_verifier_wiring.rs` '
    + '这道跨 Rust/Kotlin/Gradle/ProGuard/manifest 的接线门，以及 android.yml 的 APK 腿'
    + '（登记根内的改动一律点亮那条腿）。',
  'src-tauri/src/bin/':
    'Current target native-admission-verifier is an explicit Cargo [[bin]] with required-features = '
    + '[native-production-verification] and rejects release builds. It links the normal production library, '
    + 'not cfg(test), but is absent from the default desktop/Android packages. Native CI remains required '
    + 'for these Rust paths; the controlled runtime step explicitly builds this feature/bin before execution. '
    + 'This is an acceptance carrier, not a package asset or a claim that ordinary default builds run it. '
    + 'The CI coverage contract binds this decision to the actual Cargo target and source declarations.',
  'src-tauri/src/native_verification/':
    'Desktop-only acceptance driver and cleanup shared with the controlled native-admission-verifier. '
    + 'Both lib.rs and runtime/proxy.rs require desktop plus the non-default native-production-verification '
    + 'feature; these files do not enter default App packages or mobile containers. Changes retain native CI '
    + 'while actual native acceptance separately requires the controlled-runner feature build/execution; a source-only/default build is not native '
    + 'acceptance. No bundle layout, packaged binary, Core selection or kernel schema is changed here.',
  'src-tauri/src/app_language.rs': APP_LOGIC('应用内语言与 macOS AppleLanguages 写入'),
  'src-tauri/src/app_tray.rs': APP_LOGIC('托盘装配'),
  'src-tauri/src/clean_exit.rs': APP_LOGIC('退出清理'),
  'src-tauri/src/commands/': APP_LOGIC('IPC command 层'),
  'src-tauri/src/commands.rs': APP_LOGIC('IPC command 模块根'),
  'src-tauri/src/events.rs': APP_LOGIC('事件通道定义'),
  'src-tauri/src/exit_lifecycle.rs': APP_LOGIC('退出生命周期'),
  'src-tauri/src/graphics_compat.rs': APP_LOGIC('图形后端兼容开关'),
  'src-tauri/src/i18n.rs': APP_LOGIC('主进程 i18n（include_str! 前端 locale，编译期内联）'),
  'src-tauri/src/icon_cache.rs': APP_LOGIC('应用图标缓存（运行期用户目录）'),
  'src-tauri/src/idle_lightweight.rs': APP_LOGIC('空闲轻量态'),
  'src-tauri/src/lib.rs': APP_LOGIC('app crate 装配根'),
  'src-tauri/src/logging.rs': APP_LOGIC('日志初始化'),
  'src-tauri/src/main.rs': APP_LOGIC('可执行入口'),
  'src-tauri/src/response.rs': APP_LOGIC('IPC 响应包装'),
  'src-tauri/src/runtime/':
    '运行期实现层（进程/网络/文件系统注入）；纯 Rust 逻辑，编译进主二进制，不改安装包结构。'
    + '**此前的例外已消失**：`proxy.rs` 的 LINUX_BUNDLE_PRODUCT_DIR 曾是 productName 的第二份字面量、'
    + '被 verify-packaging confs 正则抓来对拍，于是整棵子树被钉在打包判据面上。现在 productName 由 '
    + '`src-tauri/build.rs` 从 tauri.conf.json 读出并用 cargo:rustc-env 注入（Rust 侧是 `env!`），'
    + '事实只剩一份、对拍门已删 ⇒ 本子树诚实地退出打包判据面。'
    + '将来若又出现「被打包脚本读取的源码常量」，必须重新单列 —— '
    + 'ui/src/contracts/ci-impact-coverage-contract.test.ts 会按 verify-packaging 的实际读取面反向校验。',
  'src-tauri/src/runtime.rs': APP_LOGIC('runtime 模块根'),
  'src-tauri/src/session_restore.rs': APP_LOGIC(
    '结束会话恢复回执的有界等待与迟到交接；同名子目录仅含 cfg(test) 回归，不改变包结构、资产或随包核契约',
  ),
  'src-tauri/src/startup.rs': APP_LOGIC('启动编排'),
  'src-tauri/src/test_support.rs': APP_LOGIC('测试夹具（cfg(test) 面）'),
  'src-tauri/src/tests/':
    'app crate 根模块（`lib.rs` 的 `#[cfg(test)] mod tests;`）的测试实体。纯 `cfg(test)` 面：\n    不进任何构建产物、不碰随包资产与内核契约。**没有 `tests.rs` 兄弟**，故不吃模块别名，须显式登记。',
  'src-tauri/src/test_support/':
    '测试夹具的测试实体（`test_support/tests/`）。只在 `cfg(test)` 下编译，不进 lib/bin、不进包；'
    + '由 ci.yml 三平台 `cargo test` 覆盖。',
  'src-tauri/src/tray.rs': APP_LOGIC('托盘窗口'),
  'src-tauri/src/window_health.rs': APP_LOGIC('主窗白屏自愈'),
  'src-tauri/src/windows_single_instance.rs': APP_LOGIC('Windows 单实例'),
  'src-tauri/src/windows_session_end.rs':
    'Windows 结束会话的隐藏窗口与系统代理恢复编排，仅在 Windows 编译进 App，不改变包结构、资产或随包核契约；'
    + 'Windows GNU 交叉检查与 Windows 原生 CI 覆盖编译，平台中立回归覆盖回执及锁竞争；真实会话消息仍待设备验收。',
  'src-tauri/src/windows_file_id.rs':
    'Windows 文件身份读取只影响运行期 stop/run 引用判定，不改变安装包结构、包内资产或随包核契约；'
    + 'Windows GNU 交叉检查与 Windows CI 测试覆盖该平台编译及纯判定逻辑。',

  'resources/.gitkeep':
    '唯一入库的 resources 根文件（.gitignore 的 `!/resources/.gitkeep`），作用只是让 resources/ 目录在 clone 里存在。'
    + '四份 conf 的 bundle.resources 只列 data/ 、dashboard/ 与本平台目录三类条目，**resources/ 根下的散文件不进任何包** '
    + '⇒ 不影响打包（同理它也进不了 inventory 的清点面）。',
});

const CORE_PATHS = new Set(['scripts/fetch-core.mjs',
  // Direct release-risk runtime input inspection binds the actual core before
  // its traffic gate. Changes must retain kernel + desktop packaging signals.
  'scripts/native-payload-input.mjs']);

/**
 * 随包拉取脚本**被导入的共享模块**面。
 *
 * 取的是**前缀（整个目录）**而不是逐文件枚举：`scripts/lib/*.mjs` 全部由 `scripts/fetch-*.mjs`
 * import（extract-zip 解 dashboard/protoc 的 zip），
 * 改坏它们与改坏 fetch-core.mjs 本身的后果逐字相同。2026-08-31 前它们两张表都查不到、又不在
 * REGISTRY_ROOTS 内 ⇒ kernel=false platforms=[] 且不进 unregisteredScopes：改随包解包实现零信号，
 * 与本文件头注自称已封堵的 `scripts/fetch-protoc.mjs` fail-open 是同一形态的姊妹腿。
 *
 * 逐文件枚举会原样重造这个盲区（下一个抽出来的共享模块又落表外），故按目录整取；
 * 只被非内核脚本引用的将来模块因此被**从严**判为内核门 —— 方向是 fail-closed，可接受。
 */
const CORE_PATH_PREFIXES = ['scripts/lib/', 'scripts/desktop-core/'];

const SHARED_PACKAGE_PATHS = new Set([
  '.cargo/config.toml',
  'rust-toolchain.toml',
  '.github/workflows/package.yml',
  '.github/workflows/release-risk.yml',
  'Cargo.lock',
  'Cargo.toml',
  // 🔴 三份许可文本是**同一个判据面**，必须整组登记。此前只有 THIRD-PARTY-LICENSES.md 在表里，
  //    LICENSE / NOTICE 落表外 ⇒ hasPackage=false ⇒ release-risk 的
  //    `Verify packaging conf invariants` 整步 skip。2026-09-04 给 NOTICE 加了「内核段落必须与
  //    发布合同对拍」的门之后，这就成了「门守着 NOTICE，而改 NOTICE
  //    恰好不触发这道门」—— 门在但没牙。三份都随包分发、都被 verify-packaging.mjs confs 断言，
  //    故三份同进同出。
  'LICENSE',
  'NOTICE',
  'THIRD-PARTY-LICENSES.md',
  'scripts/classify-ci-impact.mjs',
  'scripts/gate-node-test.mjs',
  'scripts/assert-pc-runtime-release.mjs',
  'scripts/assert-pc-runtime-release.test.mjs',
  'scripts/fetch-dashboard.mjs',
  // package.yml 的 `Install protoc` 步骤真跑它（钉扎常量与版本依据的唯一真值）；
  // 它挂了整条打包链就编不出 singbox-grpc。2026-08-30 前它不在任何表里 = 同一 fail-open。
  'scripts/fetch-protoc.mjs',
  'scripts/verify-packaging.mjs',
  // Linux 与 macOS 打包腿共用的 Tauri 原始输出取证。
  'scripts/bundler-diagnostics.mjs',
  'scripts/macos-bundler-diagnostics.test.mjs',
  // macOS 在封印前和最终 DMG 开箱时分别校验内核与 helper 的签名。
  'scripts/macos-nested-code.mjs',
  'scripts/macos-nested-code.test.mjs',
  'scripts/nsis-core-sweep.test.mjs',
  'scripts/nsis-helper-cleanup.test.mjs',
  'scripts/tauri-cli.version',
  'ui/package.json',
  'ui/pnpm-lock.yaml',
]);

const LINUX_PACKAGE_PATHS = new Set(['scripts/postprocess-appimage.mjs']);

function normalized(path) {
  return String(path).replaceAll('\\', '/').replace(/^\.\//, '').trim();
}

function startsWithAny(path, prefixes) {
  return prefixes.some((prefix) => path.startsWith(prefix));
}

function isUiBuildConfig(path) {
  return (
    /^ui\/(vite|postcss|tailwind)\.config\.[^/]+$/.test(path) ||
    /^ui\/tsconfig(?:\.[^/]+)?\.json$/.test(path)
  );
}

/** 两张表里对 `path` 最具体的那条登记（最长 key 胜出）；没有则 null。 */
function directLookup(path) {
  let best = null;
  for (const table of [PACKAGE_IMPACT_SCOPES, NO_PACKAGE_IMPACT_SCOPES]) {
    for (const key of Object.keys(table)) {
      const hit = key.endsWith('/') ? path.startsWith(key) : path === key;
      if (!hit) continue;
      if (best === null || key.length > best.key.length) best = { key, table };
    }
  }
  return best;
}

/**
 * 同一个 Rust 模块的两种路径形态互为别名：`foo.rs` ↔ `foo/`。
 *
 * Rust 里模块 `foo` 的源码天然分布在 `foo.rs`（或 `foo/mod.rs`）**与** `foo/` 两处。把测试实体
 * 外移成 `foo/tests/mod.rs` 会凭空造出 `foo/` 这个 scope key —— 它不是新子系统，打包影响与
 * `foo.rs` 逐字相同。要求为它再登记一遍，等于每拆一次模块就往表里补一条同义条目：判断没变，
 * 表却在长，而每条同义条目都是一次可以填错的机会。
 *
 * **不放松 fail-closed**：别名只在同名兄弟**已登记**时生效。没有 `.rs` 兄弟的全新子树
 * （如 `src-tauri/src/tests/`）依旧两张表都查不到 ⇒ 内核门 + 四平台 + 自曝。
 */
export function moduleAliasesOf(scopeKey) {
  if (scopeKey.endsWith('/')) return [`${scopeKey.slice(0, -1)}.rs`];
  if (scopeKey.endsWith('.rs')) return [`${scopeKey.slice(0, -3)}/`];
  return [];
}

/** 直接登记优先；查不到时回落到同模块的另一种路径形态（见 [`moduleAliasesOf`]）。 */
function lookupScope(path) {
  const direct = directLookup(path);
  if (direct !== null) return direct;
  const scope = scopeOf(path);
  if (scope === null) return null;
  for (const alias of moduleAliasesOf(scope)) {
    const hit = directLookup(alias);
    if (hit !== null) return hit;
  }
  return null;
}

/**
 * 某个 scope key 是否已被两张表覆盖。完备性契约用它算「谁没登记」。
 *
 * 走的是 [`lookupScope`] 本身 —— 与 [`classifyImpact`] 判「要不要 fail-closed」用的**同一个**
 * 函数，而不是照着别名规则再抄一遍。抄一遍就会有一天两边不一致：门说「没登记」而分类器说
 * 「登记了」（假红），或者门说「登记了」而分类器 fail-closed（假绿，漏登记从此不再被门抓到）。
 */
export function isScopeRegistered(scopeKey) {
  const probe = scopeKey.endsWith('/') ? `${scopeKey}__scope_probe__.rs` : scopeKey;
  return lookupScope(probe) !== null;
}

export function classifyImpact(rawPaths, { forceFull = false } = {}) {
  const paths = [...new Set(rawPaths.map(normalized).filter(Boolean))].sort();
  const platforms = new Set();
  const unregistered = new Set();
  let kernel = forceFull;
  let android = forceFull;

  const addAll = () => {
    for (const platform of ALL_PACKAGE_PLATFORMS) platforms.add(platform);
  };
  const addMac = () => {
    platforms.add('macos-arm64');
    platforms.add('macos-x64');
  };

  if (forceFull) addAll();

  for (const path of paths) {
    // ── Android 腿复用本分类：APK 编入登记根内的 Rust、Gradle 工程与随包资源，
    // 故登记根内的改动一律点亮；登记根外跟随表一与内核门 / 安装包腿（见返回值）。
    if (startsWithAny(path, REGISTRY_ROOTS)) android = true;

    // ── 登记根内：表说了算，没登记就 fail-closed ──
    const scope = lookupScope(path);
    if (scope !== null) {
      if (scope.table === PACKAGE_IMPACT_SCOPES) {
        const decision = PACKAGE_IMPACT_SCOPES[scope.key];
        android = true;
        if (decision.kernel) kernel = true;
        for (const platform of decision.platforms) platforms.add(platform);
      }
      continue;
    }
    if (startsWithAny(path, REGISTRY_ROOTS)) {
      unregistered.add(scopeOf(path) ?? path);
      kernel = true;
      addAll();
      continue;
    }

    // ── 登记根外：枚举表；未命中交给 ci.yml / ui.yml ──
    if (CORE_PATHS.has(path) || startsWithAny(path, CORE_PATH_PREFIXES)) {
      kernel = true;
      addAll();
      continue;
    }
    if (SHARED_PACKAGE_PATHS.has(path)) {
      addAll();
      continue;
    }
    if (LINUX_PACKAGE_PATHS.has(path) || isUiBuildConfig(path)) {
      platforms.add('linux');
      continue;
    }
    if (path.startsWith('packaging/macos-')) {
      addMac();
      continue;
    }
    if (path.startsWith('packaging/')) addAll();
  }

  const orderedPlatforms = ALL_PACKAGE_PLATFORMS.filter((platform) => platforms.has(platform));
  return {
    kernel,
    platforms: orderedPlatforms,
    preflight: kernel || orderedPlatforms.length > 0,
    hasPackage: orderedPlatforms.length > 0,
    android: android || kernel || orderedPlatforms.length > 0,
    unregisteredScopes: [...unregistered].sort(),
    paths,
  };
}

function stdinPaths() {
  const input = readFileSync(0);
  if (input.length === 0) return [];
  const separator = input.includes(0) ? '\0' : '\n';
  return input.toString('utf8').split(separator);
}

// Separate from kernel/package impact: pure Rust logic often has no package
// impact but still requires native CI. Only existing Node-test content changes
// on the ordinary main push may reuse the full Linux Node/UI gates instead.
export function classifyNativeImpact(rawDiff) {
  const full = (reason) => ({ nativeRequired: true, reason });
  if (typeof rawDiff !== 'string' || !rawDiff || !rawDiff.endsWith('\0')) return full('missing-or-empty-diff');
  const fields = rawDiff.slice(0, -1).split('\0');
  if (fields.length % 2 !== 0) return full('malformed-raw-diff');
  const paths = [];
  for (let i = 0; i < fields.length; i += 2) {
    const row = /^:100644 100644 ([0-9a-f]{40}) ([0-9a-f]{40}) M$/.exec(fields[i]);
    const path = fields[i + 1];
    if (!row || row[1] === row[2] || [row[1], row[2]].some((oid) => /^0+$/.test(oid)))
      return full('not-existing-regular-file-content-only');
    // Match the visible, first-level gate-node-test.mjs glob without normalizing
    // a malformed raw Git path into a different, apparently safe path.
    if (!/^scripts\/[a-z0-9][a-z0-9.-]*\.test\.mjs$/.test(path)) return full('not-node-test-only');
    paths.push(path);
  }
  if (new Set(paths).size !== paths.length) return full('duplicate-raw-path');
  const impact = classifyImpact(paths);
  if (impact.kernel || impact.preflight || impact.hasPackage || impact.platforms.length
      || impact.android || impact.unregisteredScopes.length || appleRequired(paths))
    return full('existing-release-impact-requires-native');
  return { nativeRequired: false, reason: 'existing-node-test-content-only; full Release Risk Node and UI gates still required', paths };
}

// github context belongs to the caller in a reusable workflow. Checking the
// exact direct CI workflow_ref keeps Package/release calls full even if they
// inherit a push event. PR/merge queue retain their real three-host Node tests.
export function classifyNativeCiEvent(env, event, run = execFileSync) {
  const full = (reason) => ({ nativeRequired: true, reason });
  if (env.GITHUB_EVENT_NAME !== 'push' || env.GITHUB_REF !== 'refs/heads/main'
      || env.GITHUB_WORKFLOW !== 'CI'
      || env.GITHUB_WORKFLOW_REF !== `${env.GITHUB_REPOSITORY}/.github/workflows/ci.yml@refs/heads/main`)
    return full('not-ordinary-direct-main-push');
  if (!event || event.forced !== false || event.created !== false || event.deleted !== false)
    return full('forced-created-deleted-or-unknown-push');
  const sha = /^[0-9a-f]{40}$/;
  if (!sha.test(event.before) || !sha.test(event.after) || /^0+$/.test(event.before)
      || event.after !== env.GITHUB_SHA || event.before === event.after)
    return full('unreliable-push-endpoints');
  try {
    const options = { encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 };
    if (run('git', ['rev-parse', 'HEAD'], options).trim() !== event.after)
      return full('checkout-head-differs');
    run('git', ['cat-file', '-e', `${event.before}^{commit}`], options);
    run('git', ['merge-base', '--is-ancestor', event.before, event.after], options);
    const diff = run('git', ['diff', '--raw', '--abbrev=40', '--no-renames', '-z', event.before, event.after, '--'], options);
    return classifyNativeImpact(diff);
  } catch {
    // A shallow/missing base, history rewrite, failed command, oversized diff,
    // or unavailable objects never grants the exemption. No remote fetch/retry.
    return full('unreliable-history-or-diff');
  }
}

function main() {
  if (process.argv.includes('--native-ci')) {
    let result;
    try {
      result = classifyNativeCiEvent(process.env, JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8')));
    } catch {
      result = { nativeRequired: true, reason: 'missing-or-invalid-event' };
    }
    process.stdout.write(`${JSON.stringify(result)}\n`);
    if (process.env.GITHUB_OUTPUT)
      appendFileSync(process.env.GITHUB_OUTPUT, `native_required=${result.nativeRequired}\n`);
    if (process.env.GITHUB_STEP_SUMMARY)
      appendFileSync(process.env.GITHUB_STEP_SUMMARY, `Native CI required: ${result.nativeRequired}. ${result.reason}\n`
        + (result.paths ? `Changed Node tests: ${result.paths.join(', ')}\nNative checks are not executed for this test-only change; this is not a native test pass.\n` : ''));
    return;
  }
  const forceFull = process.argv.includes('--full');
  const paths = forceFull ? [] : stdinPaths().filter(Boolean);
  const result = classifyImpact(paths, { forceFull });
  // 自曝：未登记 scope 已经按内核门 + 四平台处理，但「为什么这次全量」必须能在日志里读出来。
  for (const scope of result.unregisteredScopes) {
    process.stderr.write(
      `未登记的影响面 scope：${scope} —— 已 fail-closed 为内核门 + 四平台。`
        + '请在 scripts/classify-ci-impact.mjs 的 PACKAGE_IMPACT_SCOPES 或 NO_PACKAGE_IMPACT_SCOPES 里显式判定。\n',
    );
  }
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) main();
