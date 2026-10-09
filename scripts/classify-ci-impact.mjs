#!/usr/bin/env node

/**
 * 把一次提交的路径差集分类为发布风险面。
 *
 * 输出是稳定 JSON：
 *   - kernel: 是否必须下载真实随包核并执行强制内核门；
 *   - platforms: 必须实际构建安装包的平台腿；
 *   - preflight: 是否需要静态打包/内核预检；
 *   - hasPackage: 是否存在安装包构建腿；
 *   - android: 是否必须跑 Android APK 腿（`.github/workflows/android.yml`，见 [`ANDROID_IMPACT_SCOPES`]）；
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

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

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
  // 根下的散文件（.gitkeep / .fetch-stamp.json）按单文件 scope 归一。
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
 * key 比 scope 更细的条目（如 `crates/helper/src/platform/windows/`）按最长前缀胜出，
 * 用来把「整棵树四平台」收窄成单腿。
 */
export const PACKAGE_IMPACT_SCOPES = Object.freeze({
  'scripts/release-assets.mjs': {
    kernel: false,
    platforms: ALL,
    why: '正式发布资产命名与最终摘要清单由构建、聚合及更新器对拍消费；改动须重验全部桌面包的名称、资产集合和校验和。',
  },
  'vendor/swift-rs/': {
    kernel: false,
    platforms: ALL,
    why: 'Cargo crates.io patch 使用的本地 Swift/Rust 桥源码；库及构建脚本可能改变桌面 Tauri 产物，须保留四平台打包门。',
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
      '随包特权 helper 二进制本体：package.yml 单独 `cargo build -p polaris-helper --target …` 后铺进 '
      + 'resources/<平台>/，是包内资产（2026-08-10 三平台出过不含它的包）。',
  },
  'crates/helper/src/platform/linux/': {
    kernel: false,
    platforms: Object.freeze(['linux']),
    why: 'helper 的 linux 平台实现，只改 linux 那份二进制。',
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
    why: '随包核版本与资产钉扎的唯一真值：既换核（内核门）又换包内资产（四平台）。',
  },
  'src-tauri/Cargo.toml': {
    kernel: false,
    platforms: ALL,
    why: '主二进制的依赖与 feature 面，四平台的包都变。',
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
      'linux 腿的随包内核 / libcronet / polaris-helper 落位目录（.gitignore，CI 由 fetch-core、fetch-cronet '
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
  'crates/dns-race/': APP_LOGIC('节点域名解析竞速 sidecar（UDP server + DNS wire 编解码）'),
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
    + '🔴 但 `gen/android/` **不是没人管**：它是 Android APK 腿的主要触发面，登记在 [`ANDROID_IMPACT_SCOPES`]，'
    + '改它会点亮 `android=true`。本条只说「不加桌面腿」，别据此推断「改它没有门」。',
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
    + '（本文件同时登记在 ANDROID_IMPACT_SCOPES，改它会点亮那条腿）。',
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
  'resources/.fetch-stamp.json':
    'scripts/fetch-*.mjs 的本机拉取戳，被 .gitignore 覆盖 ⇒ 永远不会出现在 diff 里、分类器实际判不到它。'
    + '登记它是为了让完备性门在**已 fetch 过的开发机**上枚举到它时不误红；同样不在任何 bundle.resources 条目内。',
});

/**
 * ── 表三：Android APK 腿的触发面 ──
 *
 * 与上面两张表**正交**：那两张答的是「要不要重打桌面安装包 / 跑真核门」，本表答的是
 * 「要不要跑 `.github/workflows/android.yml`」。同一个路径可以两边都真（`resources/data/`）、
 * 也可以只在本表里（`src-tauri/gen/android/`）。
 *
 * ── 取材面怎么定的：从**这条腿自己的判据**反推，不从「哪些文件跟 Android 沾边」反推 ──
 *
 * 这条腿的产物级判据只有三族（`scripts/verify-apk.mjs`）：三份许可文本进包且与工作树逐字节相同、
 * `libbox.so` 里有 naive/cronet、geo `.srs` 份数相等。凡是**能改变这三条判定**或**能让 APK 根本打不出来**
 * 的输入都在表里；改不了它们的不在。
 *
 * 🔴 **明确不在表里的（是取舍，不是遗漏）**：`src-tauri/src/**`、`crates/**`、`ui/**`。
 *    它们确实编译进 `libpolaris_lib.so`（前端还被编进二进制），但**动不了上面任何一条判定** ——
 *    动的是「Android 上编不编得过」，那是**编译面**，与本腿守的**产物内容面**是两件事。
 *    把整棵 Rust 树塞进本表，等于用一条**每次都要交叉编译 + 打包 + 可能现建 libbox** 的重腿，
 *    去兜一个 `cargo clippy --target` 就能兜的面。那条门该单独加 —— **2026-09-04 已经加了**：
 *    `ci.yml` 的 `Cross-check platform targets` 步多了 `aarch64-linux-android` 一格
 *    （18 个包 `clippy --all-targets -D warnings`，外加一条 `cargo check -p polaris` 覆盖 src-tauri
 *    的编译面；`polaris` / `polaris-helper` 在该 target 上的豁免与逐条理由在
 *    `scripts/cross-target-exempt.json`）。**编译面从此归它，本腿仍然只守产物内容面。**
 *    ci.yml 没有对 `src-tauri/**` / `crates/**` 的 paths 过滤 ⇒ 那条门在这些改动上必跑，
 *    所以本表不收它们不再留缺口。细节见 `polaris-android-ci-leg-2026-09-04.md` §8。
 *
 * 🔴 **上一段的例外：只在 android 上编译的那一部分**（2026-09-05 补 —— 不写这一句，上一段与
 *    dep-info 对差门就是自相矛盾的两条口径）。上一段说的「不在表里」指的是那棵 Rust 树里
 *    **两侧都编**的部分：它们的 android 面是**编译面**，归交叉 clippy / check，本表不收。
 *    而**只在 `aarch64-linux-android` 上编译**的 Rust 文件是例外 —— `ci.yml` 的 dep-info 对差门
 *    （`scripts/check-android-only-face.mjs`）对它们**一律要求写进本表**，错误信息里明写
 *    「判成『不影响 APK 产物』也不行」：那一类正是**链接期**与随包 `.so` 内容的来源，而替补的
 *    `cargo check` 不 codegen、不链接，交叉腿结构上兜不到它。
 *    这不等于把整棵 Rust 树塞进来：这个集合由编译器的 dep-info 定（android 编译面 − host 编译面），
 *    今天全仓**恰好一个** —— `src-tauri/src/android_tls.rs`，本表已收，理由见下面那一条。
 *
 * fail-closed 方向与另外两张表一致：登记根内**未登记**的 scope 一律点亮本腿（见 [`classifyImpact`]），
 * `--full` 亦然。默认后果是多跑一条腿，不是静默放行。
 */
export const ANDROID_IMPACT_SCOPES = Object.freeze({
  'scripts/release-assets.mjs': {
    why: 'Android 正式三包构建与聚合直接消费资产名、实际 ABI 集合及已验签字节摘要判据，须重验 APK 来源、内容与发布清单。',
  },
  'src-tauri/src/runtime/proxy/android_bridge/tailscale_store.rs': {
    why: '原 android_bridge.rs 的 scoped store 模块外移后，父声明 cfg(any(android,test)) 使本文件在 '
      + 'production 中只参与 Android 编译；原实际 dep-info 的 Android − host 差集已确认该精确文件。'
      + '同父模块、同 public API、原 decoder/body 与 Android 调用保持，文件字节进入 APK 的 Rust cdylib，'
      + '必须点亮 Android 编译/链接与打包腿。whole Android crossClippy/check 覆盖目标编译，host decoder '
      + 'tests 与 check-android-bridge.mjs 覆盖原 37 个命令；这些门不签实际 JNI 调用、设备或 NoOwner 语义。',
  },
  'scripts/core-source-provision.py': {
    why: '共同 core/dependency source provider 须点亮 Android AAR/APK 来源腿，避免共同来源变更静默沿用桌面豁免。'
      + 'Android builder 直接消费原 provider checkout，prelookup 与 cached/fresh 共享完整来源和实际工具/SDK 谓词。'
      + 'Host Node fixtures 覆盖 provider receipt 与来源准入；本登记不证明真实 Android consumer 接线、AAR/APK 重构建或设备行为。',
  },
  'src-tauri/src/runtime/proxy/android_capacity.rs': {
    why: 'Android native admission capacity failures keep their typed identity through the Rust bridge, '
      + 'login checker and speedtest spawn error. APK compilation must include these Android cfg branches '
      + 'and their linked Rust cdylib; desktop tests cannot validate the packaged Android error adapter.',
  },
  'src-tauri/src/runtime/proxy.rs': {
    why: 'Android Debug observer/probe modules and original Start-input ownership are wired by Android cfg in this module. '
      + 'A fresh APK must compile and link these Rust cdylib branches; source registration does not attest runtime attribution.',
  },
  'src-tauri/src/runtime/proxy/android_probe_loan.rs': {
    why: 'Android Debug lends the original generation, birth, revision, digest and authenticated probe-input metadata through the native bridge. '
      + 'APK Rust/JNI compilation verifies the typed adapter is packaged; actual ingress and UID attribution still require device evidence.',
  },
  'src-tauri/src/runtime/proxy/debug_pc_echo.rs': {
    why: 'Android Debug validates the private authenticated PC-ready target against the original probe loan and erases its owned nonce. '
      + 'This Android-only native admission code enters the APK Rust cdylib; dep-info and fresh APK compilation must cover it. '
      + 'Source registration and compilation do not prove an installed package or live LAN witness.',
  },
  'src-tauri/src/runtime/proxy/hot_switch.rs': {
    why: 'Android Debug binds an accepted reload revision to the original probe owner in Android cfg branches. '
      + 'These linked Rust inputs require a fresh APK; successful compilation does not prove runtime generation ownership.',
  },
  'src-tauri/src/runtime/speedtest.rs': {
    why: 'Android 临时测速核由独立 libbox 实例承载；改动会进入 APK 的 Rust cdylib。'
      + '真编 Android APK 可验证该 cfg 模块与 libbox/NDK 的链接，测速的运行期成败仍需设备验证。',
  },
  'src-tauri/src/runtime/speedtest/android.rs': {
    why: 'Android-only stopped-state speedtests check and start the same authenticated in-memory config through the independent libbox bridge. '
      + 'The typed checker/spawner/child are packaged in the Rust cdylib and require Android dep-info registration and fresh APK compilation; '
      + 'this registration does not attest runtime cleanup or device speedtest results.',
  },
  'src-tauri/src/runtime/proxy/route_replan.rs': {
    why: 'Android route observation consumes the native physical-Network bridge and distinguishes a real empty snapshot from failure; synchronous selector locks skip only the OS inventory precheck. Fresh APK Rust/JNI compilation and device binding verification cover this Android-specific production path.',
  },
  'src-tauri/src/commands/system.rs': {
    why: 'Android 网卡枚举使用 Kotlin ConnectivityManager 桥，与具名 socket 绑定共享物理 Network 判据；'
      + '必须重新编译 APK 的 Rust/JNI 桥并验证回包字段、权限和实际 Android ABI，不能用桌面 getifaddrs 代验。',
  },
  'src-tauri/src/commands/speedtest.rs': {
    why: 'Android 的临时核测速/系统接口拒绝分支会进入 APK 中的新 Rust cdylib；'
      + '需真编 Android 目标与重包，确认 cfg 分支、错误码和测速回执随产物发布，不能靠桌面测速测试代替。',
  },
  'src-tauri/gen/android/': {
    why:
      '入库的 Gradle 工程本体。`app/build.gradle.kts` 的 androidResources.ignoreAssetsPatterns 正是'
      + '「三份许可文本进不进包」的开关（AGP 默认忽略 `_` 开头的 assets 子目录，而 Tauri 把 `../x` 铺成 '
      + '`assets/_up_/x`），删掉它构建照样全绿、包里一条资源都没有；`app/libs/libbox.aar` 的引用行'
      + '（`implementation(files("libs/libbox.aar"))`）决定 libbox.so 进不进包。',
  },
  'src-tauri/src/runtime/tailscale_login_core.rs': {
    why: 'Android-only production adapter starts and closes an independent libbox login instance through the native bridge; APK compilation verifies this cfg branch and Java factory ABI.',
  },
  'src-tauri/src/runtime/mesh.rs': {
    why: 'Android Logout dispatch and strict auth-state retirement consume the original native store custody and held target reservation. '
      + 'These Android production branches enter the linked Rust cdylib and require fresh APK cfg/bridge compilation; '
      + 'host FileStore tests cannot verify packaged JNI behavior or actual writer retirement.',
  },
  'src-tauri/src/runtime/proxy/prerequisite.rs': {
    why: 'Android credential actions bind the original Main raw-config digest, complete claim scope, selected current runtime run and exact Stop. '
      + 'These cfg branches enter the APK Rust cdylib and must compile against the packaged native bridge; '
      + 'host selection negatives do not attest Android SDK execution or native cleanup.',
  },
  'src-tauri/src/runtime/tailscale_login_core/attempts.rs': {
    why: 'The original Android Attempt retains private Main/Login custody or an exact Warm tuple and same-Entry action reservation across cancellation and unknown completion. '
      + 'These production cfg fields and consumers enter the APK Rust cdylib; fresh APK compilation covers their native adapter types, '
      + 'while host bookkeeping tests cannot prove on-device terminal or reservation release.',
  },
  'src-tauri/src/android_tls.rs': {
    why:
      '平台证书校验器的初始化本体，也是**全仓唯一的 JNI 导出点**（本文件 `#[jni::jni_mangle(...)]`）。'
      + '本腿对它的独有射程是**链接期**，不是运行期：ci.yml 给 `polaris` 的 android 替补是 '
      + '`cargo check --target aarch64-linux-android -p polaris`（`polaris` 被 cross-target-exempt.json '
      + '整包豁免出交叉 clippy），而 `cargo check` 不 codegen、不链接 ⇒「导出符号发不出来 / cdylib 链不出来」'
      + '这一类只有本腿真编真链 `libpolaris_lib.so` 时才看得见；它又是 android-only 依赖 '
      + '`rustls-platform-verifier` 的唯一消费者，那份 AAR 能不能被 `app/build.gradle.kts` 的 '
      + '`rustlsPlatformVerifierAar()` 真解析出来，同样只有 Gradle 跑一遍才知道。'
      + '🔴 **旧理由（「让接线门与开箱判据都跑一遍」）是假话，别照抄**：接线门 '
      + '`src-tauri/tests/android_platform_verifier_wiring.rs` 是纯读文件的集成测试，ci.yml 三平台 '
      + '`cargo test --workspace` 每次都跑，与本腿是否被调用无关；`verify-apk.mjs` 对本文件零判据。'
      + '「不初始化 ⇒ 每次 HTTPS panic 掉一个 tokio worker」是**运行期**语义，而本腿只打包不运行 APK，'
      + '那个形态它一条判据都看不见。',
  },
  'src-tauri/tauri.android.conf.json': {
    why:
      '`bundle.resources` 就是「哪些资源会被铺进 assets/_up_/」的真值；少一条许可文本 = 分发义务落空，'
      + '而 verify-packaging confs 只能判「conf 里写没写」，判不了「包里到底有没有」。',
  },
  'scripts/build-libbox.sh': {
    why:
      'libbox.aar 的唯一来源。它的坑② 明写「不能靠摘掉 with_naive_outbound 绕过 NDK 报错」—— '
      + '摘了之后 aar/APK/CI 全绿，只有真机上 naive/H3 静默连不上，唯一能抓到的就是本腿的 so 指纹判据。',
  },
  'scripts/libbox-patches/': {
    why: 'libbox 固定来源、工具链、补丁、构建与收据核验同属 AAR 来源；任何改动都须重跑 Android APK 腿。',
  },
  'scripts/build-android-apk.sh': {
    why: '标准 Android 构建入口统一 Rust NDK 并驱动一次 Tauri/Gradle 构建；改坏它会让 APK 腿跳过或混用工具链。',
  },
  'scripts/android-rust-ndk.version': {
    why: 'Rust Android 构建默认使用的已验稳定 NDK 版本；改版本须重跑 APK 腿验证交叉编译与 Gradle。',
  },
  'scripts/strip-android-release-native.mjs': {
    why: 'Release native producer strips copied libraries before APK packaging; changes require the actual Android APK and source-receipt checks.',
  },
  'scripts/strip-android-release-native.test.mjs': {
    why: 'Mutation checks preserve runtime/JNI/Go metadata and source AAR bytes during release stripping; changes must retain actual Android package validation.',
  },
  'scripts/verify-apk.mjs': {
    why:
      '本腿的产物级判据本体（三份许可文本逐字节对拍 / libbox.so 的 naive+cronet 指纹 / .srs 份数 / '
      + '死字节 / 剥符号 / **出厂权限集逐条全等**）。权限那条（判据 ⑦）是全仓唯一在产物侧问'
      + '「这个 APK 到底申请了哪些权限」的地方：源码侧那道门读的是本仓自有 manifest（8 条），'
      + '而 APK 带的是 manifest merger 合并库 manifest 之后的集合（本树实测 11 条）——'
      + '依赖注入一条敏感权限时，只有它会红。',
  },
  'scripts/verify-apk.test.mjs': {
    why: '判据本体的变异测试：判据有没有牙由它守，改判据而不改它 = 那些否定用例可能已经不成立。',
  },
  'scripts/verify-wry-keep-rules.mjs': {
    why:
      'RustWebView 那条 keep 规则与它事实源的对拍判据。事实源是 `tauri android build` 现场生成、'
      + '被 app/.gitignore 忽略的 `generated/RustWebView.kt` ⇒ 只有本腿跑到那一步时它才在树上，'
      + '裸 checkout 上的 `cargo test` 读不到 ⇒ 这条判据没有别的落点。'
      + 'wry 升级把方法改名或改签名时它是唯一会说话的东西：规则不再匹配任何成员，R8 静默剪掉方法，'
      + '而 debug 包与全部源码门都是绿的。',
  },
  'scripts/gate-android-release-behavior.sh': {
    why:
      'release 路径的**行为**裁判：跑 gradle 配置期，断言零凭据必红、逃生门只认命令行实参、'
      + 'R8 在 release 任务图里恰好一次而 debug 里零次、AGP 手里那份 proguardFiles 清单三份都在。'
      + '此前这些判据全是「build.gradle.kts 里含某串」，实测 24 种变异下一条不红而行为已经变了 ⇒ '
      + '这条腿是它们今天唯一的观测面（判据要 Android SDK，ci.yml 的 lint / cross / test 都跑不了）。',
  },
  'scripts/assert-r8-evidence.mjs': {
    why:
      'release 冒烟腿的产物级判据本体：configuration.txt（剥注释后）证明各来源的规则文本到过 R8，'
      + 'seeds.txt 证明那些 keep 真的匹配到了类与成员。两份产物只在本腿的 release 构建里存在。',
  },
  'scripts/assert-r8-evidence.test.mjs': {
    why: '上一条判据的变异测试：判据有没有牙由它守（含把「针被本仓注释喂绿」那条真缺陷原样回放）。',
  },
  '.github/workflows/android.yml': {
    why: '本腿本体（工具链解析、libbox 来源、Tauri 构建、开箱验的调用点全在这一份里）。',
  },
  '.github/workflows/release-risk.yml': {
    why: '本腿的派发处：`android` job 的 `if` 与 gate job 里「被选中就必须 success」那条断言。',
  },
  'src-tauri/core-manifest.json': {
    why:
      '`bundledCoreVersion` 是 build-libbox.sh 读的内核版本真值，也是 libbox 缓存键的一部分：'
      + '换版本 = 换一份 aar，必须重建并重验 so 指纹。',
  },
  'src-tauri/tauri.conf.json': {
    why: 'base conf：identifier / frontendDist / beforeBuildCommand，android conf 以它为底做 RFC 7396 合并。',
  },
  'src-tauri/build.rs': { why: '构建期资源校验与 productName 注入，决定 Android 侧能否走完 tauri 那一步。' },
  'src-tauri/Cargo.toml': { why: 'app crate 的依赖与 feature 面，决定 aarch64-linux-android 能否交叉编译出 .so。' },
  'Cargo.lock': { why: '同上，且它是 THIRD-PARTY-LICENSES.md 的生成输入（本腿逐字节对拍那份）。' },
  // ── 四条**冗余保留**的资源键（2026-09-05 重写理由；键不动，理由改成实话）──
  //
  // 旧理由（「本腿逐字节对拍这份，改它而不重打包 = 包里那份成了旧值」）是**本机语义、在 CI 上不成立**：
  // verify-apk 的判据 ①④ 是**相对**判据 —— 拿包内那份与**同一次 checkout 的工作树**那份比，
  // 而 CI 每次都是先 checkout 再打包，内容变更两侧同动 ⇒ 恒等判绿。「包里那份成了旧值」是本机
  // 增量构建才有的形态。
  //
  // 🔴 **仍然保留这四个键**，三条依据：
  //  ① 移除是 **fail-open 方向**的改动，而「相对判据恒绿」这个分析**未经 CI 实证**（本机推演，
  //     置信度 medium）；押注一个没实证的分析去放宽触发面，代价不对称。
  //  ② 这四个键今天真实的**检出力上限**是两件事：**资源整条逃逸**（`bundle.resources` /
  //     `ignoreAssetsPatterns` 被改坏 ⇒ 包里干脆没有这份文件，绝对判据，判据 ① 的存在性那一半
  //     与判据 ④ 的份数那一半都会红）与**工作树那份被删空**。前者的触发面另有登记
  //     （`src-tauri/tauri.android.conf.json` / `src-tauri/gen/android/`），后者更早被桌面
  //     `verify-packaging` 抓到 —— 那是**冗余**，不是错。
  //  ③ 多跑一次昂贵的腿，代价远低于漏跑一次。
  //
  // **什么时候可以删**：在 CI 上实证了「只改这四份文件的内容、APK 腿的判据 ①④ 恒绿」之后
  //（跑一次真实 run，不是再推一遍），届时把这四个键挪进 NO_ANDROID_IMPACT_SCOPES 并写明实证。
  LICENSE: {
    why:
      '**冗余保留**（2026-09-05 重判）。verify-apk 判据 ① 拿 APK 里的 assets/_up_/LICENSE 与'
      + '**同一次 checkout 的工作树**那份逐字节对拍 —— 相对判据，改内容两侧同动 ⇒ 在 CI 上恒等判绿；'
      + '「包里那份成了旧值」只在本机增量构建里成立。它真正的检出力上限是**这份文件整条没进包**'
      + '（资源铺设被改坏 / 工作树那份被删空），而前者的触发面另有登记、后者更早被桌面 '
      + 'verify-packaging 抓到。不删键的理由：移除是 fail-open 方向，而上面这段分析未经 CI 实证；'
      + '多跑一次腿的代价远低于漏跑一次。CI 上实证了恒绿再挪表。',
  },
  NOTICE: {
    why:
      '同 LICENSE：**冗余保留**，判据 ① 是相对判据、在 CI 上恒等判绿，检出力上限是「整条没进包」。'
      + 'Android 上 libbox 与主二进制同进程，NOTICE 的分发义务比桌面更重 —— 这是**保留它的动机**，'
      + '不是「本腿能量到内容变更」的依据，两者别混。CI 实证相对判据恒绿之后可挪进 NO 表。',
  },
  'THIRD-PARTY-LICENSES.md': {
    why:
      '同 LICENSE：**冗余保留**，判据 ① 相对、恒等判绿。2.1 MB，是链进 libpolaris_lib.so 的 Rust '
      + '依赖的许可与版权声明；它的**内容**真值由 Cargo.lock 决定（那条键自己在表内，且是生成输入），'
      + '本键守的只是「这份文件整条没进包」。CI 实证后可挪进 NO 表。',
  },
  'resources/data/': {
    why:
      '**冗余保留**：本腿判据 ④ 断言 APK 内 `.srs` 份数与**同一次 checkout 的工作树**相等 —— 同样是'
      + '相对判据，增删一份两侧同动 ⇒ 恒等判绿。检出力上限是「随包 geo 整条逃逸」（资源铺设被改坏），'
      + '而那条的触发面另有登记。不删键的理由同 LICENSE：移除是 fail-open 方向、分析未经 CI 实证。',
  },
  '.cargo/config.toml': {
    why:
      'Android 的 `.so` 剥符号开关住在这里（per-target `rustflags` 的四个 `*-android` 段，2026-09-05）。'
      + '摘掉那四行，`libpolaris_lib.so` 从 127652816 弹回 515511872、APK 从 219011984 弹回 606871040，'
      + '而**桌面四条腿一个字节都不变**——它此前只登记在 SHARED_PACKAGE_PATHS（桌面面），'
      + 'Android 腿不点亮 ⇒ 改坏它之后，唯一会量这件事的门恰好不跑。'
      + '同一形态的姊妹坑参见本表 LICENSE/NOTICE 那条：门守着某个文件，而改那个文件不触发这道门。',
  },
  // ── 本腿 workflow 真正 `run:` 的脚本，一条都不能落表外 ──
  // 与 SHARED_PACKAGE_PATHS 那一侧同一个道理：真跑它的腿必须在改它时亮起来，否则
  // 「改坏打包链的一环、合入前零信号」在 Android 这条腿上原样复发。
  // 反向断言由 ui/src/contracts/ci-impact-coverage-contract.test.ts 按 android.yml 的实际 `run:` 面钉住。
  'scripts/fetch-protoc.mjs': {
    why:
      '本腿真跑它：`crates/singbox-grpc` 的 build.rs 在 aarch64-linux-android 上同样要 protoc'
      + '（那条依赖不按平台 cfg 掉），protoc 拉不到 = APK 编不出来。',
  },
  'scripts/lib/': {
    why:
      'fetch-protoc.mjs 解 zip 用的共享模块（extract-zip / fetch-stamp），改坏它与改坏 fetch-protoc 后果逐字相同。'
      + '按目录整取，理由同 CORE_PATH_PREFIXES：逐文件枚举会让下一个抽出来的共享模块又落表外。',
  },
  'scripts/tauri-cli.version': {
    why: '桌面与 Android 构建使用的全局 Tauri CLI 精确版本；CLI 改动可能改变资源与原生库铺设，须跑 APK 门。',
  },
  'ui/package.json': {
    why: 'Android beforeBuildCommand 编译前端；依赖或构建命令变化会改变 APK 内的 UI 产物。',
  },
  'ui/pnpm-lock.yaml': {
    why: 'Android 前端通过 pnpm install --frozen-lockfile 安装；锁文件决定实际打进 APK 的前端依赖。',
  },
});

/**
 * ── 表四：已判定**不**触发 Android APK 腿 ──
 *
 * 与 [`NO_PACKAGE_IMPACT_SCOPES`] 同形：value 是理由字符串，「在这里 = 有人看过、判过」。
 * 本表**不参与分类**（不抑制任何 fail-closed），只承载判定与理由；强制力来自两道完备性门：
 * `ui/src/contracts/android-impact-coverage-contract.test.ts`（共享文件里的 android 条件代码）
 * 与 `scripts/check-android-only-face.mjs`（只在 android 上编译的文件，dep-info 对差）。
 *
 * ── 🔴 理由怎么写才不是假话 ──
 *
 * 不许套 [`APP_LOGIC`]。那句模板结尾是「正确性由 ci.yml 三平台 fmt+clippy+build+test 覆盖」，
 * 对 **Android 专属代码**是假的：三平台的 build/test 根本走不到 `#[cfg(target_os = "android")]`
 * 的那一支。每条理由必须点名它**今天真实**的覆盖面，能落到具体那一行最好：
 *
 *  - `ci.yml` 的 `Cross-check platform targets` 步：`cargo clippy --target aarch64-linux-android
 *    --all-targets -p <pkg> -- -D warnings`（豁免见 `scripts/cross-target-exempt.json`）；
 *  - `ci.yml` 同一步末尾那条 `cargo check --target aarch64-linux-android -p polaris`
 *    ——`polaris` 被整包豁免出 clippy 后**唯一**的 android 编译面，且它**不带 `--all-targets`**
 *    ⇒ `src-tauri/tests/**` 的 android 分支从来没在 android target 上编译过；
 *  - `ui.yml` 的 `pnpm run build` → `scripts/check-android-bridge.mjs` 的 A1–A10 跨语言对拍
 *    （ui.yml 在 PR 上无任何路径过滤，纯 Rust 改动照跑）；
 *  - `ci.yml` 三平台 `cargo test --workspace` 覆盖的源码级门与非 android 分支。
 *
 * 覆盖有缺口就**如实写缺口**，别拿一条抓不到它的门凑数（本表 `android_bridge.rs` 那条就是实例）。
 *
 * ── 本表的取材面（= 完备性门的枚举面）──
 *
 * 剥掉注释之后**仍含**真代码 `cfg(target_os = "android")` 的文件（`target_os` 这个标识符本身必须
 * 在剥掉字符串的面上也还在，否则那是内联夹具的字符串在喂判据），加上声明了 JNI 导出的文件。
 * 「只在 android 上编译、而文件自己不含 `target_os` 字样」的那一类**不在本表的取材面上** ——
 * 那是 dep-info 对差门的射程，两者互补。
 * 只在注释里提过 android 的文件**不进表** —— 把它们登记进来等于承认判据被自己的注释喂饱，
 * 那正是完备性门 C 组固定 comment-only 合成负例要证伪的形态；原 mesh.rs 已有真实 Android 分支。
 */
export const NO_ANDROID_IMPACT_SCOPES = Object.freeze({
  'src-tauri/src/runtime/tailscale_login_core/tests/attempt_lifecycle.rs':
    '原 cfg(test) 登录动作回归，只由 ci.yml 的 host cargo test --workspace 执行纯 Attempt/CAS 与源码绑定负门，测试代码不进入 APK。'
    + 'Android cargo check -p polaris 不带 --all-targets，APK 构建也不构建 test target，因此其中 Android test cfg 编译/运行没有被 APK 腿保证；'
    + '点亮 APK 腿不能补成这些测试的实际 Android 覆盖或 scoped writer 证明。',
  'src-tauri/src/runtime/tailscale_login_core/tests/process_exit.rs':
    '原 cfg(test) Child/supervisor 回归在 ci.yml 的 host cargo test --workspace 核真实 mock close/retry 与保留 Entry/config 的负门，不随 APK 发布。'
    + 'Android cargo check -p polaris 未包含测试目标，APK 构建同样不编 test target；Android test cfg 的编译/运行缺口仍如实保留，'
    + 'host close fixture 不能签 JNI terminal、实际 native retirement 或设备验收。',
  'src-tauri/src/commands/android_batch_qa.rs':
    '单一 Debug Android 批次入口；android+debug_assertions 分支走已有预算 plugin 桥，其他构型零资源 disabled stub。'
    + 'Android cargo check 守 cfg 类型，check-android-bridge A15 守 release guard/实际 BoxService 观察接线；'
    + 'APK 构建不会执行 socket、SDK snapshot、OEM guardian 或独立 peer 验收。',
  'src-tauri/src/runtime/updater.rs':
    'Frozen desktop sourceBuild selects the compiled desktop baseline only; the Android branch keeps bundledCoreVersion unchanged. '
    + 'cargo check --target aarch64-linux-android -p polaris type-checks that branch, while host updater/tests exercises explicit Android '
    + 'baseline compatibility and incomplete-source negatives. No JNI, APK resource, Android updater flow or on-device cleanup is verified by this change.',
  'scripts/fetch-core.mjs':
    'Desktop source producers and four-artifact consumption are invoked by package/release-risk, not android.yml. '
    + 'Android build-libbox is separately owned and does not invoke desktop fetch-core. Node source-graph fixture tests '
    + 'cover this desktop control flow; four native source producers and actual embedded buildInfo remain required before packaging.',
  'scripts/desktop-core/':
    'Desktop-only source producers and four-artifact receipt consumption preserve existing feature/CGO faces. '
    + 'This directory is not imported by fetch-protoc or Android build-libbox; Android source provisioning is separately owned. '
    + 'Required host gate-node-test fixtures cover its rejection boundaries, and native desktop producers must provide real embedded buildInfo before packaging.',
  'scripts/build-desktop-core.test.mjs':
    'Host Node fixtures for desktop source graph admission and artifact consumption are registered as required in gate-node-test. '
    + 'The opt-in tiny Go build only verifies embedded row parsing; neither these tests nor an APK build verify full desktop source binaries.',
  'src-tauri/src/commands/window.rs':
    'app_restart 的共享 Rust 平台派发：Android 保留 QuitState/RestartState 与 request_restart 腿，'
    + '桌面才进入四 producer 的 prepare/commit 门；未新增 JNI 导出、Gradle 或包内资产契约。'
    + 'ci.yml 的 cargo check --target aarch64-linux-android -p polaris 编译 Android 分支，'
    + 'exit_lifecycle/tests/mod.rs 在 host cargo test 上守桌面 prepare/commit 接线。'
    + 'APK 构建不执行应用重启，Android 实际重启行为仍需设备验证。',
  'src-tauri/src/exit_lifecycle.rs':
    '桌面退出 admission/drain 与 Android 原 run_android_exit_once 腿通过 cfg 分派，'
    + '不新增 APK 资产或外部 ABI；Android 不消费桌面 DesktopExitReady。'
    + 'ci.yml 的 cargo check --target aarch64-linux-android -p polaris 覆盖 Android cfg 编译，'
    + 'exit_lifecycle/tests/mod.rs 的 host 单测与源码门覆盖桌面协调和退出入口。'
    + 'APK 构建不执行退出清理，Android 的运行期停止/退出结果未由这些 host 门验证。',
  'src-tauri/src/runtime/speedtest/tests/mod.rs':
    'cfg(test) 内新增的 pc_custody 模块与桌面退出源码门受 cfg(not android) 约束，测试代码不进入 APK。'
    + 'ci.yml 的 host cargo test --workspace 实际执行这些桌面回归；'
    + 'Android cargo check -p polaris 不带 --all-targets，未编译本测试目标的 Android 构型。'
    + 'APK 构建同样不构建测试目标，点亮 APK 腿不能补成 Android 测试覆盖。',
  'src-tauri/src/commands/misc/backup.rs':
    'Android 导入选择器不带扩展过滤，以便 SAF 显示 .polaris-backup；导出仍走保存对话框。'
    + '`cargo check --target aarch64-linux-android -p polaris` 覆盖 cfg 编译，'
    + '`half-truth-facts.test.ts` 守住本仓接线；APK 构建不执行系统选择器，真实 SAF 回执需设备验证。',
  'src-tauri/src/commands/subscription.rs':
    'Android 本地订阅导入选择器不带扩展过滤，由正文解析继续校验格式和大小；'
    + 'Android 目标 cargo check 覆盖条件编译，APK 构建不执行选择器或验证 SAF 可见性。',
  'src-tauri/src/commands/updater/core_update.rs':
    '非 Android 的核心换代在旧核可停期间持有 legacy admission lease；Android 走独立的随包核更新路径。'
    + '桌面生命周期测试覆盖 lease，Android 目标 cargo check 覆盖 cfg 分支；APK 构建不执行更新事务。',
  'src-tauri/src/runtime/proxy/lifecycle.rs':
    'Android 不申请桌面 managed marker 的 legacy lease，起停仍经 Android 核桥；'
    + 'Android 目标 cargo check 覆盖条件编译，桌面 lifecycle 单测覆盖 lease 准入，'
    + 'APK 构建不执行起停竞态，真机状态转移需设备验证。',
  'src-tauri/src/runtime/proxy/process_supervision/direct_stop.rs':
    'S4 本地 Child 精确停止桥在 Android 入口明确 Unsupported；cfg! 两侧在桌面也参加类型检查，'
    + 'direct_stop 单测覆盖拒绝与精确 Child CAS，Android 目标 cargo check 复核构型；APK 不调用 dormant 桥。',
  'src-tauri/src/runtime/proxy/tests/lifecycle.rs':
    '桌面-only managed marker/legacy lease 生命周期测试，cfg(not android) 不进入 APK；'
    + '桌面 cargo test -p polaris --lib 实际执行，Android 交叉 cargo check 不构建此测试目标。',
  'src-tauri/src/runtime/proxy/tests/process_supervision.rs':
    '桌面-only helper attempt/lease 负例属于 cfg(test) 测试目标，不进入 APK；'
    + '桌面 cargo test -p polaris --lib 实际执行，Android 交叉 cargo check 不构建此测试目标。',
  'src-tauri/src/runtime/proxy/tests/recovery.rs':
    '桌面-only legacy admission 与 crash recovery 负例属于 cfg(test) 测试目标，不进入 APK；'
    + '桌面 cargo test -p polaris --lib 实际执行，Android 交叉 cargo check 不构建此测试目标。',
  'src-tauri/src/runtime/proxy/mesh_apply/pc_owner_census.rs':
    'PC producer 成员封存，暂无运行期消费者（模块带 dead_code 豁免）；Android 没有 temp producer，'
    + '`member_counts` 的 android 臂恒返 0。Android 目标 cargo check 覆盖条件编译，'
    + '桌面 cargo test -p polaris --lib 覆盖非 android 臂；APK 构建不执行成员封存。',
  'src-tauri/src/runtime/proxy/mesh_apply/pc_owner_census/tests/mod.rs':
    '桌面-only 成员封存与暂停负例属于 cfg(test) 测试目标，不进入 APK；'
    + '桌面 cargo test -p polaris --lib 实际执行，Android 交叉 cargo check 不构建此测试目标。',
  'src-tauri/src/runtime/startup_tasks.rs':
    '桌面自动连接在起核前复核 legacy admission，Android 走独立启动接管腿；'
    + '桌面测试覆盖 marker 拒绝，Android 目标 cargo check 覆盖 cfg 编译，APK 构建不执行自动连接。',
  'src-tauri/src/lib.rs':
    'app crate 装配根，三处 android cfg：`mod android_tls;`、`android_tls::assert_ready_before_any_https()` '
    + '调用点、`builder.plugin(runtime::proxy::android_bridge::init())`。前两处都有更早更便宜的门 ——'
    + '删 `mod` ⇒ 调用点编不过 ⇒ `cargo check --target aarch64-linux-android -p polaris` 当场红；'
    + '删调用点 ⇒ 接线门 `src-tauri/tests/android_platform_verifier_wiring.rs` 在三平台 `cargo test` 上红。'
    + '🔴 第三处（插件注册行）**今天没有任何门**，但 APK 腿同样抓不到它：本腿只打包、不运行 APK，'
    + '`register_android_plugin` 是运行期反射。把本文件塞进 ANDROID_IMPACT_SCOPES 不会让那条缝被量到，'
    + '补法是往 check-android-bridge.mjs 加一条「`init()` 必须出现在 `run()` 的 builder 链上」的断言。',
  'src-tauri/src/commands/misc/autostart.rs':
    '`auto_start_set` / `auto_start_get_status` 的三条平台腿（2026-09-25 加 android 腿：经起停核桥'
    + '写/读 Kotlin 侧「开机自动连接」标记）。android 腿只是转调 `android_bridge::set_boot_auto_connect` / '
    + '`boot_auto_connect`，编译面 → `cargo check --target aarch64-linux-android -p polaris`；两条命令名 / '
    + '载荷 / 回包面 → ui.yml 的 check-android-bridge.mjs A1/A2/A3/A10。桌面腿由三平台 clippy/build/test 覆盖。'
    + '不改 APK 的结构与包内资产。🔴 开关在真机上「开机时真的连上」要真机验证，CI 与 APK 腿都量不到。',
  'src-tauri/src/commands/misc/logs.rs':
    'Android Debug 故障报告把原生日志并入脱敏报告并交给系统分享页；改动只在 Rust cdylib 语义，'
    + 'APK 腿不运行分享流程，也不检查报告内容。Android 交叉 check 覆盖条件编译，报告脱敏单测覆盖文本，'
    + '原生分享的成败需安装真机验收；不靠一次静态打包冒充运行验证。',
  'src-tauri/src/commands/updater/app_update.rs':
    'version_get_info 的 debugReportAvailable 是 Android Debug 构型标志，仅改变 IPC 载荷；'
    + '两侧编译与 UI 契约覆盖字段，APK 腿不会执行命令或验证构型语义。',
  'src-tauri/src/commands/updater/shared.rs':
    'Android 内核随 APK 分发，此处拒绝独立可写核心目录；目标编译检查覆盖 cfg 分支，'
    + 'APK 腿不执行更新命令，真实拒绝行为仍由 Android 端回归检查。',
  'src-tauri/src/runtime/geo_seed.rs':
    'Android 从 Application 预先提取的 bundled-geo 目录播种规则；Rust 单测覆盖路径映射与播种，'
    + '交叉 check 覆盖 Android 条件编译。APK 腿只打包 28 个资源，不运行播种；资产完整性由'
    + ' Kotlin BundledRules 与 verify-apk 各自检查，设备实测验证最终文件。',
  'src-tauri/src/runtime/measurement_scheduler.rs':
    '周期测速调度器。唯一的 android cfg 是 `android_conditions` 的一对腿：android 腿只把设备状况'
    + '（活动网络是否计费、是否省电）的查询转给 `android_bridge::device_conditions` 并折成三态，'
    + '非 android 腿返回「不可得」。调度、裁决与状态投影都是与平台无关的纯逻辑，由宿主 `cargo test` 覆盖；'
    + 'android 腿的编译面落在 `cargo check --target aarch64-linux-android -p polaris`，'
    + '命令名与回包字段的跨语言面由 check-android-bridge.mjs 对拍。APK 腿只打包不运行，不为它新增判据；'
    + '真机上的计费识别与离开前台即暂停属于设备验收项。',
  'src-tauri/src/runtime/proxy/android_bridge.rs':
    'Android 起停核桥（Rust → Kotlin plugin）的 Rust 半边，35 处 android cfg（2026-09-06 由 27 增至 35：'
    + 'W-09b 已装应用枚举与 W-21 交系统安装器各带来一条命令腿 + 超时常量 + 非 Android 桩）。改它只改 '
    + '`libpolaris_lib.so` 的字节，而 verify-apk 对那份 `.so` 只问「在不在 + 有没有 `.debug_*` 节」——'
    + '本腿不新增任何判据。今天的覆盖：编译面 → `cargo check --target aarch64-linux-android -p polaris`'
    + '（cross-target-exempt.json 里 `polaris` 那条豁免的理由明写这条 check 单独留着就是为了抓这座桥）；'
    + '命令 / 载荷 / 回包面 → ui.yml 的 `pnpm run build` 跑 check-android-bridge.mjs 的 A1/A2/A3/A10；'
    + '四档超时的相对大小由文件内 `const _: ()` 编译期断言守，同样落在那条 check 上。'
    + '插件身份面（`PLUGIN_IDENTIFIER` / `PLUGIN_CLASS`）→ 同一道门的 A11 三向对拍（Rust 常量 ⇄ Kotlin '
    + '`@TauriPlugin` 类的 package/类名 ⇄ 该文件路径，2026-09-05 补；此前这三个词在门里一次都不出现，'
    + '改错一个字母只在真机 `register_android_plugin` 反射那一刻炸，而 APK 腿同样看不见 —— 它只打包、不运行）。'
    + '🔴 **如实登记残余缺口**：`PLUGIN_NAME` 在 Kotlin 侧没有对应物（全仓 `polaris-vpn` 只此一处常量），'
    + 'A11 只能断言它被 `Builder::new` 真的用上且形状合法，断不了跨语言一致性。'
    + '本批新增的两条腿（`installed_apps` / `hand_apk_to_system_installer`）同样落在上面那几条覆盖里：'
    + '回包元素面多一条 A12（Kotlin 的 `installedApp()` 工厂 ⇄ Rust `InstalledApp`，同源命名配对）；'
    + '🔴 而「Android 权限画像 / 包可见性 / 安装器不静默失败」三件事**不在这条腿的射程内**，'
    + '由 `src-tauri/tests/android_native_surface_wiring.rs` 在三平台 `cargo test` 上守（源码级）——'
    + 'APK 腿同样量不到它们：它只打包、不装、不运行，`canRequestPackageInstalls()` 与 `<queries>` '
    + '的实际效果都要真机才看得见，本批**没有真机证据**。',
  'src-tauri/src/runtime/proxy/startup.rs':
    '起停核编排，9 处 android cfg，多数是 `if cfg!(target_os = "android")` 的运行期分支 —— 那种写法两支在'
    + '**所有** target 上都参与类型检查，桌面三平台的 clippy/build/test 直接覆盖；余下 `#[cfg]` 的'
    + '占位常量与 binary 分叉由 `cargo check --target aarch64-linux-android -p polaris` 覆盖。'
    + '不改 APK 的结构与包内资产（verify-apk 不解析 `.so` 的语义）。破坏它的后果是「Android 真机上'
    + '起核走错腿」，那要模拟器才能观测，本腿一样观测不到。',
  'src-tauri/src/runtime/proxy/process_supervision.rs':
    '两处 `if cfg!(target_os = "android")` 早退：停核腿到 `android_bridge::stop_core()`，孤儿清扫腿到 '
    + '`stop_system_started_core()`（2026-09-25，停掉系统拉起的核再起）；文件内注释明写用 `cfg!` '
    + '而非 `#[cfg]` 正是为了让下面整段在 Android 编译单元里仍然可达 ⇒ 桌面与 android 两侧都在编译面内'
    + '（桌面三平台 clippy/build/test + android 那条 `cargo check`）。不改 APK 内容。',
  'src-tauri/src/runtime/stats/source.rs':
    '连接/流量统计的数据源，27 处 android cfg（与 android_bridge.rs 并列全仓第一）。承重的三个通道常量'
    + '写成 `#[cfg(any(target_os = "android", test))]` ⇒ **桌面测试构型里就编译得到**，且由 ui.yml 的 '
    + 'check-android-bridge.mjs A6/A7/A8/A9 与 Kotlin 侧、前端 `ipc-channels.ts` 三向逐字对拍；'
    + '其余是平台分支，两侧都在编译面内。不改 APK 内容。',
  'src-tauri/src/runtime/stats/gate.rs':
    '统计闸门，2 处 android cfg：`probe_main_window_visible` 的 android 臂恒返 `Ok(true)`。android 臂的编译面'
    + '由 `cargo check --target aarch64-linux-android -p polaris` 覆盖，非 android 臂由三平台 `cargo test` 覆盖。'
    + '它的**行为**正确性（「窗口可见性在 Android 上不可观测」）是真机事实，CI 无任何腿能验，APK 腿也不能。',
  'src-tauri/src/runtime/proxy/android_bridge/tests/mod.rs':
    '`#[cfg(test)]` 面，不进任何构建产物、不改 APK 一个字节。两条断言都写成「非 Android 上必须诚实失败」，'
    + '在三平台 `cargo test --workspace` 上真跑。APK 腿根本不构建 test target'
    + '（`tauri android build` 只出 cdylib）⇒ 进 ANDROID_IMPACT_SCOPES 是零收益。',
  'src-tauri/tests/remote_webview_cannot_reach_app_commands.rs':
    'app crate 的集成测试，永不进包。`cfg!(any(windows, target_os = "android"))` 的两支在桌面编译时都要'
    + '过类型检查，windows 那支还在 windows 腿上被真跑。'
    + '🔴 **如实登记**：它的 android 取值从未在 android target 上编译过 —— `cargo check -p polaris` 不含 '
    + 'test target，而 `polaris` 又被整包豁免出 `--all-targets` clippy。但 APK 腿同样不构建 test target，'
    + '进表补不上这条缝；补法是给那条豁免收窄 scope 或给 check 加 `--all-targets`。',
  'crates/helper-proto/src/lib.rs':
    '`Platform::current()` 的 android 分支。本包**不在** `scripts/cross-target-exempt.json` 里 ⇒ 落进 ci.yml 的'
    + '`cargo clippy --target aarch64-linux-android --all-targets -p polaris-helper-proto -- -D warnings`，'
    + '编译面（含测试目标）完整覆盖，判据比 src-tauri 那条 `cargo check` 还严。helper 二进制在 Android 上'
    + '根本不随包（APK 里没有它），本文件的 android cfg 只是把桌面路径 cfg 掉 ⇒ 不改 APK 内容。',
  'crates/helper-proto/src/tests/mod.rs':
    '同上包的 `cfg(test)` 面：`--all-targets` 的 android 交叉 clippy 覆盖其编译面，三平台 `cargo test` 跑其'
    + '桌面分支。`platform_current_matches_compile_target` 的 android 臂自己在注释里写明「本机跑不到，靠交叉门'
    + '覆盖编译面」，那道交叉门今天真在（本表 D 组断言钉住它）。测试代码不进包。',
  'crates/system-integration/src/dns_flush/tests/mod.rs':
    '`polaris-system-integration` 不在豁免表 ⇒ android 交叉 clippy `--all-targets` 覆盖其编译面，'
    + '三平台 `cargo test` 跑其余断言。DNS flush 是系统集成的平台操作，编进主二进制而不是随包二进制；'
    + '测试代码不进包，APK 腿不构建它。',
  'src-tauri/src/commands/proxy.rs':
    'IPC command 层，单处 `if cfg!(target_os = "android")`：custom 协议兼容性探测（`kernel_probe_outbound`）'
    + '在 Android 上改问 `android_bridge::check_config`（libbox `CheckConfig`），不再走恒失败的子进程 check。'
    + '`cfg!` 运行期分支 ⇒ 两支在**所有** target 上都参与类型检查，桌面三平台 clippy/build/test 直接覆盖；'
    + '三态翻译是纯函数 `probe_check_from_gate`，由 `commands/proxy/tests/probe_tests.rs` 在三平台 `cargo test` 上真跑；'
    + 'android 编译面由 `cargo check --target aarch64-linux-android -p polaris` 覆盖。不改 APK 结构与包内资产。'
    + '🔴 **如实登记**：libbox `CheckConfig` 对**单个探测 outbound** 的真实回话形态没有真机样本，'
    + '键路径拆分按起核闸门同一个 `parse_kernel_rejection` 口径推定 —— 要模拟器才能观测，本腿一样观测不到。',
});

/** 表内命中 `path` 的**最长** key（目录键按前缀、文件键按全等）；没有则 null。 */
function longestKeyMatch(path, table) {
  let best = null;
  for (const key of Object.keys(table)) {
    const hit = key.endsWith('/') ? path.startsWith(key) : path === key;
    if (hit && (best === null || key.length > best.length)) best = key;
  }
  return best;
}

/**
 * 路径是否落在 Android 腿的触发面上。
 *
 * 🔴 **走 [`moduleAliasesOf`] 的 `foo.rs` ↔ `foo/` 别名**（2026-09-05 补）。此前只有桌面轴的
 * [`lookupScope`] 在用那张别名表，Android 轴一侧不用 —— 同一张表，两条轴只有一条在使用。实测后果：
 * `src-tauri/src/android_tls/child.rs`（在已登记的 `android_tls.rs` 里 `mod` 出来的子文件）
 * `android = false`，而它又因为桌面轴把它别名到了已登记的 `android_tls.rs`、**也不进**
 * `unregisteredScopes` ⇒ 分类器这一侧完全静默。
 *
 * 方向是 fail-closed：别名只查 [`ANDROID_IMPACT_SCOPES`] 一张表（`NO_` 表不参与），故它只可能让
 * 本腿**多**亮一次，不可能抑制任何已有的点亮。
 *
 * 🔴 与 [`androidRegistrationOf`] 的不对称是**刻意的**，两者问的不是同一件事：本函数问「这次改动
 * 要不要跑 APK 腿」（同一个模块换个路径形态，该跑还是要跑）⇒ 走别名；`androidRegistrationOf` 问
 * 「有没有人显式判过这个文件一次」⇒ 不走别名，逼一条新登记键。
 *
 * **别名只在同名兄弟已登记时生效**（[`moduleAliasesOf`] 的既有语义）：全新子树照旧两张表都查不到，
 * 由 dep-info 门与 [`classifyImpact`] 的登记根 fail-closed 兜。
 */
function isAndroidImpact(path) {
  if (longestKeyMatch(path, ANDROID_IMPACT_SCOPES) !== null) return true;
  const scope = scopeOf(path);
  if (scope === null) return false;
  return moduleAliasesOf(scope).some(
    (alias) => longestKeyMatch(alias, ANDROID_IMPACT_SCOPES) !== null,
  );
}

/**
 * `path` 在 Android 两张表里的登记（最长 key 胜出，与 [`directLookup`] 同一条消歧规则）；
 * 两张表都查不到返回 `null`。
 *
 * 两道完备性门都用它算「谁没登记」，且与 [`isAndroidImpact`] 共用**同一份** key 匹配 —— 在门那边
 * 另写一份 `Object.keys(...).some(...)` 就是同一事实两处实现，总有一天两边不一致：门说「登记了」
 * 而分类器不点腿（静默放行），或者门说「没登记」而分类器点了（假红）。两个消费者：
 *  - `ui/src/contracts/android-impact-coverage-contract.test.ts`：**共享文件**里的 android 条件代码；
 *  - `scripts/check-android-only-face.mjs`：**只在 android 上编译**的文件（真值源是 rustc 的
 *    `--emit=dep-info`，android 编译面 − host 编译面；ci.yml 的 dep-info 对差那一步）。
 *
 * 🔴 本函数**不走** [`moduleAliasesOf`] 的 `foo.rs` ↔ `foo/` 别名，这是刻意的（2026-09-05）。
 * 桌面轴的别名规则说「同一个模块换个路径形态、打包影响没变」；而 Android 轴上「已登记文件里
 * `mod` 出一个新子文件」恰恰是要人**重判一次**的形态（它可能是全新的 JNI 导出点、全新的
 * android-only 依赖消费点）。方向是 fail-closed：dep-info 门会为它转红，逼一条显式登记。
 *
 * 🔴 **与 [`isAndroidImpact`] 的不对称是刻意的，别顺手抹平**（2026-09-05 修正）。那个函数**走**
 * 别名，本函数不走 —— 因为两者问的不是同一件事（见那边的头注）。
 * 此前是**两侧都不走**，那才是缺陷：`src-tauri/src/android_tls/child.rs` 这类路径
 * `android = false`，且因为桌面轴的 [`lookupScope`] 把它别名到已登记的 `android_tls.rs`、
 * 它也**不进** `unregisteredScopes` ⇒ 分类器那一侧完全静默，唯一的堵口只剩 dep-info 门。
 * 而那道门自己登记了三条盲区（只覆盖 aarch64 一个 ABI、两侧都不带 `--all-targets`、feature 门控），
 * 落进盲区的 android-only 子文件两道门都扎不到。现在分类器一侧由别名兜住（只多亮腿），
 * 本函数仍不走别名 ⇒ 该补的显式登记键还是要补。
 */
export function androidRegistrationOf(rawPath) {
  const path = normalized(rawPath);
  const impact = longestKeyMatch(path, ANDROID_IMPACT_SCOPES);
  const none = longestKeyMatch(path, NO_ANDROID_IMPACT_SCOPES);
  if (impact === null && none === null) return null;
  if (none === null || (impact !== null && impact.length >= none.length)) {
    return { key: impact, table: 'ANDROID_IMPACT_SCOPES' };
  }
  return { key: none, table: 'NO_ANDROID_IMPACT_SCOPES' };
}

const CORE_PATHS = new Set(['scripts/fetch-core.mjs', 'scripts/fetch-cronet.mjs', 'scripts/build-desktop-core.test.mjs', 'scripts/core-source-provision.py',
  '.github/workflows/desktop-core.yml', 'scripts/desktop-core-ci-wiring.test.mjs']);

/**
 * 随包拉取脚本**被导入的共享模块**面。
 *
 * 取的是**前缀（整个目录）**而不是逐文件枚举：`scripts/lib/*.mjs` 全部由 `scripts/fetch-*.mjs`
 * import（extract-zip 解随包内核/cronet/dashboard/protoc 的 zip，fetch-stamp 决定要不要重拉），
 * 改坏它们与改坏 fetch-core.mjs 本身的后果逐字相同。2026-08-31 前它们两张表都查不到、又不在
 * REGISTRY_ROOTS 内 ⇒ kernel=false platforms=[] 且不进 unregisteredScopes：改随包解包实现零信号，
 * 与本文件头注自称已封堵的 `scripts/fetch-protoc.mjs` fail-open 是同一形态的姊妹腿。
 *
 * 逐文件枚举会原样重造这个盲区（下一个抽出来的共享模块又落表外），故按目录整取；
 * 只被非内核脚本引用的将来模块因此被**从严**判为内核门 —— 方向是 fail-closed，可接受。
 */
const CORE_PATH_PREFIXES = ['scripts/lib/', 'scripts/core-patches/', 'scripts/desktop-core/'];

const SHARED_PACKAGE_PATHS = new Set([
  '.cargo/config.toml',
  '.github/workflows/package.yml',
  '.github/workflows/release-risk.yml',
  'Cargo.lock',
  'Cargo.toml',
  // 🔴 三份许可文本是**同一个判据面**，必须整组登记。此前只有 THIRD-PARTY-LICENSES.md 在表里，
  //    LICENSE / NOTICE 落表外 ⇒ hasPackage=false ⇒ release-risk 的
  //    `Verify packaging conf invariants` 整步 skip。2026-09-04 给 NOTICE 加了「版本号必须与
  //    core-manifest 的 bundledCoreVersion 对拍」的门之后，这就成了「门守着 NOTICE，而改 NOTICE
  //    恰好不触发这道门」—— 门在但没牙。三份都随包分发、都被 verify-packaging.mjs confs 断言，
  //    故三份同进同出。
  'LICENSE',
  'NOTICE',
  'THIRD-PARTY-LICENSES.md',
  'scripts/classify-ci-impact.mjs',
  'scripts/classify-ci-impact.test.mjs',
  'scripts/gate-node-test.sh',
  'scripts/assert-pc-runtime-release.mjs',
  'scripts/assert-pc-runtime-release.test.mjs',
  'scripts/fetch-dashboard.mjs',
  // package.yml 的 `Install protoc` 步骤真跑它（钉扎常量与版本依据的唯一真值）；
  // 它挂了整条打包链就编不出 singbox-grpc。2026-08-30 前它不在任何表里 = 同一 fail-open。
  'scripts/fetch-protoc.mjs',
  'scripts/verify-packaging.mjs',
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
    // ── Android 腿的触发面与桌面两张表正交，故先独立判一次，再走下面的桌面分支 ──
    // 放在所有 `continue` 之前：登记根内的分支会 `continue` 掉，写在后面等于对
    // `src-tauri/gen/android/**`、`resources/data/**` 这些两边都真的路径失明。
    if (isAndroidImpact(path)) android = true;

    // ── 登记根内：表说了算，没登记就 fail-closed ──
    const scope = lookupScope(path);
    if (scope !== null) {
      if (scope.table === PACKAGE_IMPACT_SCOPES) {
        const decision = PACKAGE_IMPACT_SCOPES[scope.key];
        if (decision.kernel) kernel = true;
        for (const platform of decision.platforms) platforms.add(platform);
      }
      continue;
    }
    if (startsWithAny(path, REGISTRY_ROOTS)) {
      unregistered.add(scopeOf(path) ?? path);
      kernel = true;
      // Android 腿一并 fail-closed：未登记的 scope 有可能正是新加的 Android 面
      // （`src-tauri/gen/ios/`、新的随包资源子树…），默认多跑一条腿，不默认放行。
      android = true;
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
    android,
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

function main() {
  const forceFull = process.argv.includes('--full');
  const result = classifyImpact(forceFull ? [] : stdinPaths(), { forceFull });
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
