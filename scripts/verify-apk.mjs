#!/usr/bin/env node
/**
 * verify-apk.mjs —— Android APK 的**开箱**检查（产物级判据）。
 *
 * 用法：`node scripts/verify-apk.mjs <apk 路径> [--abi <ABI>]`（`--abi` 缺省 arm64-v8a）
 * 退出码：0 = 全部判据成立；1 = 有违反（逐条打印）；2 = 用法错误 / 读不到输入。
 *
 * ── 存在理由：Android 上「构建全绿 + 产物是垃圾」有四条已实证的通路 ──
 *
 * ① **资源静默逃逸**（2026-09-04 实证，见 `polaris-android-first-apk-2026-09-04.md` §2.5）：
 *    AGP 传给 aapt2 的 `--ignore-assets` 默认串含 `<dir>_*`（忽略任何以 `_` 开头的 assets 子目录），
 *    而 Tauri 恰好把 `bundle.resources` 里每个 `../x` 条目铺成 `assets/_up_/x` ⇒ **所有以 `../` 写的
 *    资源整条不进包**。第一个 APK 就是这样出的：三份许可文本与 28 份 geo `.srs` 一条都没进去，
 *    磁盘上 tauri 铺得好好的，gradle `BUILD SUCCESSFUL`，`unzip -l` 里一个字节都没有。
 *    修复住在 `src-tauri/gen/android/app/build.gradle.kts` 的 `androidResources.ignoreAssetsPatterns`
 *    里 —— 一处**只有开箱才看得见**的修复，注释拦不住下一个人删它，本脚本才拦得住。
 *
 * ② **AGP 增量打包的死字节**（2026-09-05 实证）：
 *    AGP 更新一个**已存在**的 APK 时按 zip 增量写：被替换条目的旧数据**不回收**，只是不再被
 *    中央目录引用。反复重打同一个包，废字节按被换掉的条目大小一层层往上叠 —— 实测同一份内容
 *    从 578.8 MB 长到 1068.7 MB，**46% 是没有任何条目指向的死字节**；删掉 APK 重打一次即归零。
 *    而这期间 `unzip -t` 全绿、装得上、跑得起来，① 与 ③ 两条判据一条都不会说话：
 *    它们只问「该在的成员在不在、内容对不对」，不问「包本身是不是有一半是垃圾」。
 *
 * ③ **构建 tag 掉了**：`libbox.aar` 由 `scripts/build-libbox.sh` 现场构建，其坑② 明确记着
 *    「NDK 27 链 cronet 的预编译 .a 会报 unknown relocation ⇒ **不能靠摘掉 with_naive_outbound 绕过**」。
 *    摘掉之后一切照常：aar 出得来、APK 打得出、CI 全绿，只有真机上 naive/H3 节点静默连不上。
 *    判据形状抄 `scripts/build-libbox.sh` 尾部那段 aar 开箱验（四 ABI + cronet/naive 计数），
 *    射程从 aar 挪到**最终 APK**：aar 对了但 AGP 没把它的 jni 合并进包，同样只有这里看得见。
 *
 * ④ **原生库的 debug 信息回来了**（2026-09-05 实证）：Android 的剥符号开关是
 *    `.cargo/config.toml` 里四段 per-target `rustflags`（`-C strip=debuginfo`）。把它们删掉，
 *    `libpolaris_lib.so` 从 127652816 弹回 515511872、APK 从 219011984 弹回 606871040 ——
 *    而上面 ①②③ 与死字节那条**全绿**：死字节判据看的是「文件大小 vs 条目压缩和」的比值，
 *    这两个数是**一起**变大的，比值纹丝不动。整包翻三倍，没有一条判据说话。
 *    （`.cargo/config.toml` 已登记进 `classify-ci-impact.mjs` 的 `ANDROID_IMPACT_SCOPES`，
 *      改它会正确地把本腿拉起来 —— 但那只解决「被看见」，「被量」由本文件的判据 ⑥ 负责。）
 *
 * ⑤ **出厂权限集与本仓写的不是一回事**（2026-09-06 实证）：源码侧那道权限门读的是
 *    `src-tauri/gen/android/app/src/main/AndroidManifest.xml`（本仓自有 8 条），而 APK 带的是
 *    manifest merger 合并**库 manifest** 之后的集合 —— 本树实测 11 条，多出的
 *    `RECEIVE_BOOT_COMPLETED` / `WAKE_LOCK` 来自 `tauri-plugin-notification 2.3.3`、
 *    `com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION` 来自 `androidx.core:core:1.13.1`
 *    （出处 `app/build/outputs/logs/manifest-merger-arm64-debug-report.txt`）。
 *    于是「把一个 tauri 插件升一版 ⇒ APK 多一条 `QUERY_ALL_PACKAGES`」这一形，在本判据之前
 *    **整条链上没有一处会说话**：源码门绿（看不见库 manifest）、包可见性门绿（同上）、
 *    本文件此前六条判据一条都不涉及权限。判据 ⑦ 是那条链上唯一的落点。
 *
 * ── 判据必须含**正面**断言（本仓纪律）──
 *
 * 全部判据都写成「X 必须在、且内容必须是 Y」，没有一条是「不许出现 Z」：后者会被
 * 「什么都没发生」骗过 —— 一个空 zip、一次 `unzip` 变哑、一条读错的路径，在纯否定式判据下全是绿。
 * 三份许可文本不只判「在」，还与工作树里那三份**逐字节对拍**（截断 / 占位 / 改错版本都红）；
 * `.srs` 不只判「有」，还与工作树份数相等且要求工作树份数非零（否则 `0 == 0` 恒真）；
 * 死字节不只判「不许太大」，而是**算出实际比值并要求它落在带内** —— 带的下沿 `1.0` 守的正是
 * 「条目尺寸读成了 0 / 读错了包」那类取材面塌陷，纯上限判据在那种输入上是恒绿的；
 * 剥符号不只判「不许有 `.debug_*`」，而是先要求节表**解析得出、且含 `.text` 与 `.dynsym`**，
 * 再要求 debug 节数为 0 —— 少了前半句，「一个字节都没解析出来」就是一条免费的绿。
 *
 * 权限那条（判据 ⑦）不只判「不许有没登记的」，而是**两个方向的差集**加一条下限：
 * 只判「多的」会被「一份读不出来的输出」骗过（空集里没有多的），只判「少的」会漏掉
 * 悄悄多一条这一形；下限（本仓自有那 8 条）守的是解析口径塌陷 —— 在一份被截断的 dump 上，
 * 纯差集判据会报出一串「少了什么」，指向的地方离真因两跳远。
 *
 * ── 为什么不复用 `scripts/lib/extract-zip.mjs` ──
 *
 * 那个模块**刻意只提供全解**（见其头注：按成员选择性解压的 flag 语义在 bsdtar 上没有逐字对应物）。
 * 本脚本要从一个 debug APK（实测 150–600 MB）里只取 5 个成员，全解等于在 runner 上多写一遍
 * 整包。且本脚本的射程按定义只有 Linux（Android 腿只在 ubuntu 上跑，
 * 见 `.github/workflows/android.yml`），不需要那个模块为 Windows 付的复杂度 —— 直接用
 * `unzip -Z1` / `unzip -p`，两条命令都在 ubuntu runner 与本机实测过（80 MB 成员 0.3s）。
 */

import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/**
 * Tauri 把 `bundle.resources` 的每个 `../x` 条目铺成 `assets/_up_/x`（`_up_` = 一层 `..`）。
 * **实测得来**（`unzip -Z1` 于真实 APK），不是从 Tauri 文档抄的。
 */
export const ASSET_UP = 'assets/_up_/';

/**
 * 随包分发的三份许可文本。**这份清单是独立常量，不从 `tauri.android.conf.json` 派生**：
 * 从 conf 派生的话，「有人把 NOTICE 从 conf 里删掉」会同时删掉期望值，判据自己被自己污染，
 * 那条变异永远判不红（本脚本的变异 M1 验的正是这条）。
 */
export const LICENSE_FILES = Object.freeze(['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md']);

/**
 * 支持的 ABI → 该 ABI 下 `lib/<ABI>/*.so` 必须呈现的 ELF 形态。
 *
 * ── 为什么 ABI 是参数而不是常量（2026-09-05）──
 * APK 按 ABI 分包（`assembleArm64Debug` / `assembleX86_64Debug`），而**模拟器上跑的是
 * x86_64 那个包**。写死 arm64 时，x86_64 包在本仓一条产物级判据都没有：它照样可能
 * 资源逃逸、死字节、掉构建 tag、带着 DWARF —— 四条已实证的失效通路一条都没被量过。
 *
 * ── 为什么表里带 `elfClass` / `machine`，而不是只有一张 ABI 名单 ──
 * 参数化之后 `lib/<ABI>/` 退化成一个**路径前缀**，而路径前缀不校验内容：jniLibs 合并错、
 * splits 配错、把 arm64 的 `.so` 铺进 `lib/x86_64/`，判据 ②③⑥ 一条都不会说话
 * （「在不在」「有没有针」「剥没剥」在一份**架构错了的**库上全都成立），而它在模拟器上
 * 装得上、起不来。取值出自 ELF 规范的 `EI_CLASS`（1=ELF32 / 2=ELF64）与 `e_machine`。
 */
export const ABIS = Object.freeze({
  'arm64-v8a': Object.freeze({ elfClass: 2, machine: 0xb7, machineName: 'EM_AARCH64' }),
  'armeabi-v7a': Object.freeze({ elfClass: 1, machine: 0x28, machineName: 'EM_ARM' }),
  x86_64: Object.freeze({ elfClass: 2, machine: 0x3e, machineName: 'EM_X86_64' }),
  x86: Object.freeze({ elfClass: 1, machine: 0x03, machineName: 'EM_386' }),
});

/**
 * 缺省 ABI。不带 `--abi` 时取它，行为与参数化前逐字相同（既有调用因此不必改）。
 * android.yml 的 arm64 腿仍**显式**传 `--abi arm64-v8a`：那条腿打的是 arm64 包，而全部判据
 * 都挂在 ABI 上（原生库在不在 / 构建 tag / 剥符号 / ELF 机器类型与位宽）——靠缺省的话，
 * 换个 assemble 任务或改了缺省，门会安静地去量另外半个包，而收据里看不出来。
 */
export const DEFAULT_ABI = 'arm64-v8a';

/**
 * ABI 名 → 期望的 ELF 形态。**未知取值一律抛并列出支持的取值**，不静默回落到缺省：
 * 回落的后果不是「少验一点」而是**判据去量了另一个包**——`--abi x86-64`（该写下划线）
 * 这种手滑会按 arm64 判，而 arm64 那半个包在 x86_64 产物里根本不存在，
 * 报文却指向「取材面塌了」，离真因两跳远；更坏的形态是它恰好判绿。
 */
export function abiSpec(abi) {
  const spec = Object.prototype.hasOwnProperty.call(ABIS, abi) ? ABIS[abi] : undefined;
  if (!spec) {
    throw new Error(
      `未知 ABI：${JSON.stringify(abi)}。支持的取值：${Object.keys(ABIS).join(' / ')}`,
    );
  }
  return spec;
}

/**
 * CLI 参数：`<apk 路径> [--abi <ABI>]`（两者顺序不限）。抠成纯函数是为了让「缺省仍是
 * arm64-v8a」「未知 ABI 会抛」这两条能被测试直接钉住，而不是靠跑一遍 CLI 去推。
 */
export function parseArgs(argv) {
  let apk = null;
  let abi = DEFAULT_ABI;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--abi') {
      if (i + 1 >= argv.length) throw new Error('--abi 后面要跟一个 ABI 名');
      abi = argv[i + 1];
      i += 1;
    } else if (arg.startsWith('-')) {
      throw new Error(`未知参数：${arg}（用法：node scripts/verify-apk.mjs <apk 路径> [--abi <ABI>]）`);
    } else if (apk === null) {
      apk = arg;
    } else {
      throw new Error(`多余的位置参数：${arg}（只接受一个 apk 路径）`);
    }
  }
  abiSpec(abi); // 未知 ABI 在这里当场抛，不带着一个错的 abi 往下走
  return { apk, abi };
}

/**
 * `lib/<ABI>/` 下必须存在的原生库。两条都要：
 *  - `libbox.so` —— sing-box 内核（libbox.aar 的 jni 被 AGP 合并进来）；
 *  - `libpolaris_lib.so` —— 应用自己的 Rust 侧。缺它说明 tauri 那一步铺的 jniLibs 没进包，
 *    而那种情况下 `libbox.so` 可能仍在（它来自 aar 这条**另一条**路径）⇒ 必须分别断言。
 */
export const NATIVE_LIBS = Object.freeze(['libbox.so', 'libpolaris_lib.so']);

/**
 * `libbox.so` 里的构建 tag 指纹。阈值与形状抄 `scripts/build-libbox.sh` 尾部的 aar 开箱验，
 * 判据是 `count > min`（严格大于，与那边逐字一致）。
 *
 * 为什么用计数而不是「出现过就算」：符号表 / 路径串里蹭到一次 `naive` 是噪声，
 * 真链进 cronet-go 时这两个串是成百上千次量级（本机实测：cronet 1201、naive 223）。
 */
export const NATIVE_MARKERS = Object.freeze([
  { needle: 'cronet', min: 100, why: 'with_naive_outbound 掉了 ⇒ naive/H3 节点在真机上静默连不上' },
  { needle: 'naive', min: 10, why: '同上，naive outbound 未链入' },
]);

/** 工作树里 geo 规则集出厂副本的目录（`bundle.resources` 的 `../resources/data/`）。 */
export const SRS_DIR = 'resources/data';

/** APK 条目数下限：真实 APK 实测 965–1001 条。低于此说明清单没读全或读了个空壳。 */
const MIN_APK_ENTRIES = 100;

/**
 * ZIP 的结构开销**上界**用到的定长字段。每一项都能在 APPNOTE.TXT 4.3 里逐条查到，
 * 不是从某一个包上标定出来的经验值 —— 这是本判据不写成魔数的全部依据。
 *
 * `MAX_LOCAL_EXTRA` 是唯一一个不由定长字段给出的项：extra field 本身长度可变，
 * 而 APK 里它的**唯一**大额消费者是 zipalign 的对齐填充 —— AGP 把未压缩存放（`stor`）的
 * 原生库对齐到页边界，填充字节就塞在本地头的 extra field 里。按**一整页**计上界，
 * 且对每一条目都这么算（实际只有几条 `stor` 的 `.so` 会用到），故是一个真上界。
 */
export const ZIP = Object.freeze({
  LOCAL_HEADER: 30,        // 本地文件头定长部分（签名 4 + 26）
  CENTRAL_HEADER: 46,      // 中央目录头定长部分（签名 4 + 42）
  MAX_DATA_DESCRIPTOR: 24, // 每条目最多一个：签名 4 + CRC 4 + 两个 8 字节长度（zip64 形态）
  MAX_LOCAL_EXTRA: 4096,   // 见上：zipalign 的页对齐填充
  EOCD: 22,                // 整包一个（不含注释；注释另计在下面的实测余量里）
  ZIP64_EOCD: 56,          // 整包最多一份
  ZIP64_LOCATOR: 20,       // 整包最多一份
});

/**
 * 给定成员名清单，算出「文件大小 − 全部条目压缩后之和」里**合法**的那部分的上界。
 *
 * 文件名在 zip 里存**两遍**（本地头一遍、中央目录一遍），故按 `2 × 名字字节数` 计。
 *
 * ── 这个上界有多紧（本机实测，arm64 debug APK / 1001 条目 / 名字合计 47638 字节）──
 *   **干净包**（删掉旧 APK 重打）：文件 219011984 − 条目和 218812042 = **199942** 字节非条目
 *     字节，逐项拆开是本地头 30030 + 中央头 46046 + 文件名×2 95276 + 本地 extra 24472 +
 *     EOCD 22 = 195846，再加中央目录之前一段 4096 字节的对齐尾巴。
 *   **同一份清单上本函数给出的上界 = 4295570** ⇒ 干净包只用掉上界的 1/21，不会被判红。
 *   **脏包**（保留旧 APK 增量重打，同一次改动）：文件 606871040 − 条目和 218812042 =
 *     **388058998** 字节非条目字节，是上界的 **90 倍**。
 *   信号与噪声差两个数量级以上，这条判据不存在「卡在边上」的工况 —— 容差取多大都不改判定，
 *   所以才敢把它定成一个能逐项说清出处的上界，而不是一个标定出来的分位数。
 *
 * ── 射程与灵敏度下限（**如实登记，不是缺陷**）──
 *   本上界是「每条目都吃满一整页对齐填充」的最坏情况，因此它随**条目数**线性增长，
 *   与包的大小无关：1001 条目的包上它是 4.3 MB。也就是说这条判据看不见小于 4.3 MB 的死字节。
 *   落到本仓的射程上（Android debug APK，实测 150–600 MB）这个下限是整包的 0.7%–2%，
 *   而它要抓的 AGP 增量重打死字节实测是整包的 63% —— 中间隔着 30 倍以上，够用。
 *   不把上界收紧到「只给 `lib/**.so` 算页对齐」（那样 1001 条目的包上是 211766 字节，
 *   与实测的 199942 只差 1.06 倍）：AGP 实际填了 24472 字节本地 extra，而按「273 条 stored
 *   条目 ×4 字节 + 3 条 `.so` ×4096」只推得出 13107 —— 模型比现实窄，收紧就是给自己埋假红。
 */
export function zipOverheadBound(names) {
  const nameBytes = names.reduce((sum, n) => sum + Buffer.byteLength(n, 'utf8'), 0);
  const perEntry =
    ZIP.LOCAL_HEADER + ZIP.CENTRAL_HEADER + ZIP.MAX_DATA_DESCRIPTOR + ZIP.MAX_LOCAL_EXTRA;
  return names.length * perEntry + 2 * nameBytes + ZIP.EOCD + ZIP.ZIP64_EOCD + ZIP.ZIP64_LOCATOR;
}

/**
 * 死字节判据的**带**与实测值。判定与打印都从这一个函数取值 —— 分两处算，
 * 打印出来的收据在原理上可以与被判定的数不同。
 *
 * `ratio` = 文件大小 / 条目压缩和；带是 `[1, 1 + 上界/条目和]`。
 * `dead` = 非条目字节 − 结构开销上界（≤ 0 即全部非条目字节都能被 zip 结构解释）。
 *
 * @returns {{fileSize: number, entriesTotal: number, nonEntry: number,
 *            bound: number, dead: number, ratio: number, hi: number}}
 */
export function deadByteReport({ names, fileSize, entriesCompressedTotal }) {
  const bound = zipOverheadBound(names);
  const nonEntry = fileSize - entriesCompressedTotal;
  const ratio = entriesCompressedTotal > 0 ? fileSize / entriesCompressedTotal : NaN;
  const hi = entriesCompressedTotal > 0 ? 1 + bound / entriesCompressedTotal : NaN;
  const dead = nonEntry - bound;
  return { fileSize, entriesTotal: entriesCompressedTotal, nonEntry, bound, dead, ratio, hi };
}

/**
 * `.so` 里被认定为「调试信息」的节区名前缀。两种拼写都要：
 *  - `.debug_*` —— DWARF 的标准节名（未压缩，或 SHF_COMPRESSED 压缩后**节名不变**）；
 *  - `.zdebug_*` —— 压缩 DWARF 的**旧**拼写（Go 工具链历史上产出过，binutils 也认）。
 * 只写前者会被后者整条绕过：包大了、判据绿着，正是本仓「读回形态≠写入形态」那条坑。
 */
export const DEBUG_SECTION_PREFIXES = Object.freeze(['.debug_', '.zdebug_']);

/**
 * 一份**已链接**的共享库必须存在的节区。这是本判据的**正面**那一半：
 * 没有它，「`.debug_*` 数为 0」会被「一个字节都没解析出来」满足 —— 截断的读、
 * 读错的成员、解析器写反了，在纯否定式判据下全是绿。
 */
export const REQUIRED_ELF_SECTIONS = Object.freeze(['.text', '.dynsym']);


// ═══════════════════════════════════════════════════════════════════════════════════════════
// 判据 ⑦：出厂 APK 的权限集（uses-permission + 自建 permission，逐条全等一张登记表）
// ═══════════════════════════════════════════════════════════════════════════════════════════

/**
 * 出厂 APK 的 `uses-permission` 登记表：`{ name, from, why }`。
 *
 * ── 为什么这条判据必须住在**产物侧**（2026-09-06 实证）──
 *
 * `src-tauri/tests/android_native_surface_wiring.rs` 已经有一道逐条全等的权限门，但它读的是
 * **本仓自有**的 `AndroidManifest.xml`。APK 真正申请的权限集是 AGP 的 manifest merger 把
 * **库 manifest** 合并进来之后的结果 —— 两者今天就已经不等：本仓自己声明 8 条，
 * `app/build/intermediates/packaged_manifests/arm64Debug/…/AndroidManifest.xml` 是 11 条。
 * 多出来的三条来自依赖（见下面每条的 `from`，出处 `app/build/outputs/logs/manifest-merger-*.txt`）。
 *
 * 于是有一条完整的失效链，链上没有一处会说话：把任一 tauri 插件升一版、或加一个在自己
 * manifest 里声明 `QUERY_ALL_PACKAGES` / `READ_EXTERNAL_STORAGE` 的插件 ⇒ 出厂 APK 当场
 * 多一条敏感权限，而源码侧那道门绿（它看不见库 manifest）、包可见性门绿（同上）、
 * 本文件此前的六条判据一条都不涉及权限（`grep -n permission scripts/verify-apk.mjs` 零命中）。
 * 本判据就是那条链上唯一一处能说话的地方。
 *
 * ── 判据是**集合恰等**，不是「都在」──
 *
 * 多一条与少一条是同一类事故，且多的那一类更危险：权限是安全边界。membership 型判据对
 * 「悄悄多一条」是瞎的。
 *
 * ── `from` 不是装饰 ──
 *
 * `'self'` = 本仓 `AndroidManifest.xml` 自己写的；其余取值 = 注入它的那个库的坐标。
 * `src-tauri/tests/android_native_surface_wiring.rs` 的
 * `permission_registry_agrees_with_the_artifact_side_registry` 拿本表里 `from: 'self'` 的那批
 * 与它自己的登记表做集合恰等 —— 两张表因此不会各自漂移，而「这条权限从哪儿来」也就不是
 * 可以在登记表里改口的东西（把一条自有权限改标成「库注入」来消红，那道门当场红）。
 */
export const APK_PERMISSION_REGISTRY = Object.freeze([
  { name: 'android.permission.INTERNET', from: 'self', why: '内核出站与所有 HTTPS（订阅拉取、更新检查、测速）。没有它这个应用不存在。' },
  { name: 'android.permission.FOREGROUND_SERVICE', from: 'self', why: '隧道由 PolarisVpnService（前台服务）承载；Android 8+ 上缺它 startForegroundService 抛 SecurityException。' },
  { name: 'android.permission.FOREGROUND_SERVICE_SYSTEM_EXEMPTED', from: 'self', why: 'Android 14+ 要求前台服务声明类型化权限；VPN 是 systemExempted 的合法用例。缺它在 14+ 上起前台服务即抛。' },
  { name: 'android.permission.POST_NOTIFICATIONS', from: 'self', why: '前台服务的常驻通知在 Android 13+ 需要运行期授权；那条通知是用户唯一能看见「隧道在跑」的地方。' },
  { name: 'android.permission.ACCESS_NETWORK_STATE', from: 'self', why: 'DefaultNetworkMonitor 观察底层网络变化（切 WiFi/蜂窝时内核要重新绑出接口）。' },
  { name: 'android.permission.CHANGE_NETWORK_STATE', from: 'self', why: 'API 28..30 上「取真正的默认网络」只能用 requestNetwork，那条 API 要它；否则内核出站绕回自己的 tun。' },
  { name: 'android.permission.ACCESS_WIFI_STATE', from: 'self', why: 'readWIFIState()：内核的 wifi_ssid / wifi_bssid 路由规则要读当前 SSID/BSSID。' },
  { name: 'android.permission.REQUEST_INSTALL_PACKAGES', from: 'self', why: 'W-21 应用内自更新：把下载好的 APK 交给系统安装器。本仓经 GitHub Releases 分发、没有商店托管更新。射程只有「请求安装一个包」，每次仍由系统弹确认框。' },
  { name: 'android.permission.RECEIVE_BOOT_COMPLETED', from: 'self', why: '设置 → 通用「开机自动连接」：vpn.BootReceiver 开机后判准入再起核。normal 级、不弹窗。tauri-plugin-notification 也注入同一条（合并后仍是一条），但它现在是本仓自己声明且真的消费的。' },
  { name: 'android.permission.WAKE_LOCK', from: 'tauri-plugin-notification:2.3.3', why: '库注入（tauri-plugin-notification 为通知投递持锁声明）。本仓不消费它。' },
  { name: 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION', from: 'androidx.core:core:1.13.1', why: '库注入的**自建**权限（不是平台权限）：androidx 用它保护 ContextCompat.registerReceiver 注册的动态广播接收器，protectionLevel=signature ⇒ 只有同签名应用能拿到。射程只在本应用内，不向系统要任何东西。' },
]);

/**
 * 出厂 APK **自己声明**的 `<permission>`（不是它请求的）。同样集合恰等。
 *
 * 为什么也要判：自建权限的 `protectionLevel` 是一条真实的攻击面 —— 一条 `normal` 级的自建
 * 权限等于把它保护的组件对所有应用开放。今天这里只有 androidx 那一条（`signature`），
 * 而「今天只有一条」这件事本身要有人守，否则下一条进来时没有任何东西会说话。
 */
export const APK_DECLARED_PERMISSIONS = Object.freeze([
  { name: 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION', from: 'androidx.core:core:1.13.1', why: 'androidx.core 的动态广播接收器保护，protectionLevel=signature。' },
]);

/**
 * 解析 `aapt2 dump permissions <apk>` 的输出。
 *
 * 实测形态（本机 build-tools 36.0.0，2026-09-06）：
 * ```
 * package: com.polaris2.app
 * permission: com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION
 * uses-permission: name='android.permission.INTERNET'
 * ```
 * 两种行的前缀只差一个词，故**按行首锚定**：`permission:` 起笔的行绝不能被当成 uses-permission
 * （`includes`/`indexOf` 那种写法会把 `uses-permission:` 的尾巴也匹进去，或反过来把声明算成请求）。
 *
 * 读不出 `package:` 那一行 ⇒ 返回 `null`（**不是**空集）：空集会让「集合恰等」在一份读坏的
 * 输入上报成「少了 11 条」，把取材面塌陷伪装成产物缺陷；调用方对 `null` 的处理是红在取材面。
 */
export function parseAapt2Permissions(text) {
  const lines = String(text).split('\n').map((l) => l.trimEnd());
  if (!lines.some((l) => l.startsWith('package: '))) return null;
  const uses = [];
  const declared = [];
  for (const line of lines) {
    if (line.startsWith("uses-permission: name='")) {
      const value = line.slice("uses-permission: name='".length);
      const close = value.indexOf("'");
      if (close < 0) return null;
      uses.push(value.slice(0, close));
    } else if (line.startsWith('permission: ')) {
      declared.push(line.slice('permission: '.length).trim());
    }
  }
  return { uses, declared };
}

/** 集合恰等的差集报文（两个方向都报，且把登记表里的理由带上去）。 */
function permissionSetViolations(label, got, registry) {
  const errors = [];
  const want = new Set(registry.map((e) => e.name));
  const have = new Set(got);
  const extra = [...have].filter((n) => !want.has(n)).sort();
  const missing = [...want].filter((n) => !have.has(n)).sort();
  if (extra.length > 0) {
    errors.push(
      `出厂 APK 里有**没登记**的${label}：${extra.join(' ')}\n` +
        '  权限是安全边界，多一条与少一条是同一类事故。它可能不是本仓写的 —— manifest merger 会把\n' +
        '  库 manifest 里的权限合并进来（`app/build/outputs/logs/manifest-merger-*-report.txt` 有逐条出处）。\n' +
        `  确实需要它 ⇒ 加进 ${label === 'uses-permission' ? 'APK_PERMISSION_REGISTRY' : 'APK_DECLARED_PERMISSIONS'}` +
        '，写清 from（哪个库带进来的 / self）与 why；不需要 ⇒ 把带它的依赖换掉，或在本仓 manifest 里\n' +
        '  用 `tools:node="remove"` 显式移除。',
    );
  }
  if (missing.length > 0) {
    errors.push(
      `登记表里有出厂 APK 上**不存在**的${label}：${missing.join(' ')}\n` +
        '  要么某条腿的权限被误删（那条腿在真机上会恒失败），要么登记表腐烂了。',
    );
  }
  return errors;
}

/**
 * 判据 ⑦ 本体。`parsed` 为 `null` 表示取材面塌了（aapt2 找不到 / 输出读不出）——
 * 那必须红在取材面，而不是判绿，也不是报成「产物少了 11 条权限」。
 */
export function permissionViolations(parsed) {
  if (parsed === null) {
    return [
      '读不出出厂 APK 的权限集（aapt2 缺席，或 `aapt2 dump permissions` 的输出里没有 `package:` 行）——\n' +
        '  「集合恰等」在一份读不出来的输入上没有意义，本条红在取材面。\n' +
        '  aapt2 在 $ANDROID_HOME/build-tools/<版本>/aapt2；CI 上由 android.yml 的 SDK 步骤按 compileSdk 装。',
    ];
  }
  const errors = [];
  // 正面断言（FLOOR）：一个真实 APK 至少要有本仓自己那 8 条。扫到远少于此 ⇒ 解析口径塌了，
  // 而「集合恰等」在一份被截断的输入上会报成一串「少了什么」，指向的地方离真因两跳远。
  const floor = APK_PERMISSION_REGISTRY.filter((e) => e.from === 'self').length;
  if (parsed.uses.length < floor) {
    errors.push(
      `只从 APK 上读出 ${parsed.uses.length} 条 uses-permission（下限 ${floor}，即本仓自有那批）——\n` +
        '  解析口径塌了，而不是产物真的只剩这么点权限。',
    );
    return errors;
  }
  errors.push(...permissionSetViolations('uses-permission', parsed.uses, APK_PERMISSION_REGISTRY));
  errors.push(...permissionSetViolations('自建 permission', parsed.declared, APK_DECLARED_PERMISSIONS));
  return errors;
}

/** 节区数下限。实测：libpolaris_lib.so 28 / libbox.so 30 / libc++_shared.so 31。 */
const MIN_ELF_SECTIONS = 5;

/**
 * 从一份 ELF 的字节里读出节区名清单。
 *
 * ── 为什么自己解析，而不是调 `llvm-readelf` ──
 * 那要求 runner 上有 NDK（本脚本在 CI 上跑在 `开箱验` 步，那一步不设 NDK 环境），
 * 且要先把 127 MB 的成员落盘。而这里要的信息全在**文件头 + 节表 + shstrtab**里，
 * 三处加起来几 KB；字节已经在 `readMember` 的记忆里（判据 ② 为了验 ELF 魔数
 * 本来就把整份读进来了），**零额外 I/O**。
 *
 * ── 为什么两种 ELFCLASS 都支持 ──
 * 本仓四个 Android ABI 里 `armv7`/`i686` 是 ELF32、`arm64`/`x86_64` 是 ELF64。
 * 写死 64 位的话，换个 ABI 跑这条判据会**静默读到垃圾**（偏移全错），
 * 而垃圾解析出来的节名清单里当然没有 `.debug_` —— 又是一条假绿。字节序同理。
 *
 * @param {Buffer} buf
 * @returns {{ok: true, names: string[], bytes: number[]} | {ok: false, why: string}}
 */
/**
 * 读 ELF 的 `EI_CLASS` 与 `e_machine`——「这份 `.so` 真的是这个 ABI 的」由它取值。
 *
 * 与 `elfSectionNames` 分开：判据 ② 要在**节表读不出来**时也能说话（一份被截断的库仍有
 * 完整的 16 字节 e_ident + e_machine），而节表解析对这种输入只会返回「读不出来」。
 *
 * @param {Buffer} buf
 * @returns {{ok: true, elfClass: 1|2, machine: number} | {ok: false, why: string}}
 */
export function elfIdent(buf) {
  if (buf.length < 20) return { ok: false, why: `只有 ${buf.length} 字节，连 e_machine 都放不下` };
  if (buf.readUInt32BE(0) !== 0x7f454c46) return { ok: false, why: '前四字节不是 ELF 魔数' };
  const elfClass = buf[4]; // EI_CLASS：1 = ELF32，2 = ELF64
  const data = buf[5]; // EI_DATA：1 = 小端，2 = 大端
  if (elfClass !== 1 && elfClass !== 2) {
    return { ok: false, why: `EI_CLASS = ${elfClass}（既非 ELF32 也非 ELF64）` };
  }
  if (data !== 1 && data !== 2) return { ok: false, why: `EI_DATA = ${data}（字节序未知）` };
  // `e_machine` 在 ELF32 / ELF64 下偏移相同：e_ident 16 + e_type 2 = 18。
  const machine = data === 1 ? buf.readUInt16LE(18) : buf.readUInt16BE(18);
  return { ok: true, elfClass, machine };
}

/** All LOAD segments of a 64-bit Android library must support 16 KB pages. */
export function elfPageAlignment(buf) {
  const ident = elfIdent(buf);
  if (!ident.ok) return ident;
  if (ident.elfClass !== 2) return { ok: true }; // Android's 16 KB targets are 64-bit.
  if (buf.length < 64) return { ok: false, why: 'ELF64 header truncated' };
  const le = buf[5] === 1;
  const u16 = (p) => le ? buf.readUInt16LE(p) : buf.readUInt16BE(p);
  const u32 = (p) => le ? buf.readUInt32LE(p) : buf.readUInt32BE(p);
  const u64 = (p) => le ? buf.readBigUInt64LE(p) : buf.readBigUInt64BE(p);
  const start = Number(u64(32)), size = u16(54), count = u16(56);
  if (!Number.isSafeInteger(start) || start < 64 || size < 56 || count === 0 || start + size * count > buf.length) {
    return { ok: false, why: 'ELF program header table missing or truncated' };
  }
  let loads = 0;
  for (let i = 0; i < count; i++) {
    const p = start + i * size;
    if (u32(p) !== 1) continue;
    loads++;
    const alignment = u64(p + 48);
    if (alignment < 16384n || (alignment & (alignment - 1n)) !== 0n || u64(p + 8) % 16384n !== u64(p + 16) % 16384n) {
      return { ok: false, why: `LOAD[${i}] is not 16 KB aligned (p_align=${alignment})` };
    }
  }
  return loads ? { ok: true } : { ok: false, why: 'ELF has no LOAD segments' };
}

export function elfSectionNames(buf) {
  if (buf.length < 64) return { ok: false, why: `只有 ${buf.length} 字节，连 ELF 头都放不下` };
  // 魔数 / EI_CLASS / EI_DATA 三项与判据 ② 读的是**同一份**取值口：分两份写必然漂，
  // 而漂的方向是两条判据对同一份坏字节给出不同结论。
  const ident = elfIdent(buf);
  if (!ident.ok) return ident;
  const is64 = ident.elfClass === 2;
  const le = buf[5] === 1;

  const u16 = (o) => (le ? buf.readUInt16LE(o) : buf.readUInt16BE(o));
  const u32 = (o) => (le ? buf.readUInt32LE(o) : buf.readUInt32BE(o));
  const u64 = (o) => Number(le ? buf.readBigUInt64LE(o) : buf.readBigUInt64BE(o));
  const off = (o) => (is64 ? u64(o) : u32(o));

  // e_shoff / e_shentsize / e_shnum / e_shstrndx 在两种 class 下偏移不同
  const shoff = is64 ? off(40) : off(32);
  const shentsize = is64 ? u16(58) : u16(46);
  const shnumRaw = is64 ? u16(60) : u16(48);
  const shstrndxRaw = is64 ? u16(62) : u16(50);

  if (shoff === 0) return { ok: false, why: '节表偏移 e_shoff = 0（这份 ELF 没有节表）' };
  const minEnt = is64 ? 64 : 40;
  if (shentsize < minEnt) return { ok: false, why: `e_shentsize = ${shentsize}（小于 ${minEnt}）` };
  if (shoff + shentsize > buf.length) {
    return { ok: false, why: `节表起点 ${shoff} 超出文件长度 ${buf.length}（读到的字节不完整？）` };
  }

  const sh = (i, field) => {
    const base = shoff + i * shentsize;
    if (field === 'name') return u32(base);
    if (field === 'link') return u32(base + (is64 ? 40 : 24));
    if (field === 'size') return is64 ? u64(base + 32) : u32(base + 20);
    return is64 ? u64(base + 24) : u32(base + 16); // offset
  };

  // 溢出形态：节数 ≥ SHN_LORESERVE 时 e_shnum = 0，真值在 shdr[0].sh_size；
  // shstrndx ≥ SHN_LORESERVE 时 e_shstrndx = SHN_XINDEX(0xffff)，真值在 shdr[0].sh_link。
  // 现实里的 .so 撞不到，但**不处理就是静默读错**，代价只有这两行。
  const shnum = shnumRaw === 0 ? sh(0, 'size') : shnumRaw;
  const shstrndx = shstrndxRaw === 0xffff ? sh(0, 'link') : shstrndxRaw;

  if (!(shnum > 0)) return { ok: false, why: '节表条目数为 0' };
  if (shoff + shnum * shentsize > buf.length) {
    return { ok: false, why: `节表（${shnum} 条 × ${shentsize} 字节 @ ${shoff}）超出文件长度 ${buf.length}` };
  }
  if (shstrndx >= shnum) return { ok: false, why: `e_shstrndx = ${shstrndx} 越界（节数 ${shnum}）` };

  const strOff = sh(shstrndx, 'offset');
  const strSize = sh(shstrndx, 'size');
  if (strOff + strSize > buf.length) {
    return { ok: false, why: `节名字符串表 @${strOff}+${strSize} 超出文件长度 ${buf.length}` };
  }
  const strtab = buf.subarray(strOff, strOff + strSize);

  const names = [];
  const bytes = [];
  for (let i = 0; i < shnum; i += 1) {
    const nameOff = sh(i, 'name');
    if (nameOff >= strtab.length) {
      names.push('');
      bytes.push(0);
      continue;
    }
    const end = strtab.indexOf(0, nameOff);
    names.push(strtab.subarray(nameOff, end < 0 ? strtab.length : end).toString('latin1'));
    bytes.push(sh(i, 'size'));
  }
  return { ok: true, names, bytes };
}

/** 节名是不是调试信息节。 */
export function isDebugSection(name) {
  return DEBUG_SECTION_PREFIXES.some((p) => name.startsWith(p));
}

/**
 * 一份 `.so` 的「剥没剥过」判定。红时报文要能直接指向**改哪儿**，故按成员名给出处。
 *
 * @returns {{ok: boolean, why: string|null, sections: number, debug: string[], debugBytes: number}}
 */
export function strippedReport(buf) {
  const parsed = elfSectionNames(buf);
  if (!parsed.ok) return { ok: false, why: `节表读不出来：${parsed.why}`, sections: 0, debug: [], debugBytes: 0 };
  const { names, bytes } = parsed;
  // ── 正面那一半：解析出来的东西必须**像**一份真的共享库 ──
  if (names.length < MIN_ELF_SECTIONS) {
    return {
      ok: false,
      why: `只解析出 ${names.length} 个节区（实测真库 28–31 个）—— 解析到的不是一份完整节表`,
      sections: names.length,
      debug: [],
      debugBytes: 0,
    };
  }
  const missing = REQUIRED_ELF_SECTIONS.filter((n) => !names.includes(n));
  if (missing.length > 0) {
    return {
      ok: false,
      why:
        `节表里没有 ${missing.join(' / ')} —— 一份已链接的共享库必然有它们，` +
        '说明解析出来的不是这份库的节表（在这种输入上「没有 .debug_*」不构成任何证据）',
      sections: names.length,
      debug: [],
      debugBytes: 0,
    };
  }
  const debug = names.filter(isDebugSection);
  const debugBytes = names.reduce((sum, n, i) => (isDebugSection(n) ? sum + bytes[i] : sum), 0);
  return { ok: debug.length === 0, why: null, sections: names.length, debug, debugBytes };
}

/** 某份 `.so` 的调试信息该由谁负责 —— 红的时候直接把人送到改动点，不让他再去找。 */
function debugOwnerHint(lib) {
  if (lib === 'libpolaris_lib.so') {
    return '本仓 Rust 侧：`.cargo/config.toml` 的 `[target.*-linux-android*] rustflags = ["-C", "strip=debuginfo"]` 四段是否被删/改';
  }
  if (lib === 'libbox.so') {
    return 'sing-box 内核侧：`scripts/build-libbox.sh` 的 gomobile/Go 链接期参数（这份不经 rustc，与上面那四段无关）';
  }
  return 'NDK 预编译件（由 tauri 那一步 symlink 进 jniLibs）：只能在打包侧剥，或换一份 NDK';
}

/**
 * 全部判据。**纯函数**：输入靠注入，故可以喂合成输入做正/反向对照（见 `verify-apk.test.mjs`）。
 *
 * @param {object} input
 * @param {string[]} input.names       APK 内全部成员名（`unzip -Z1`）
 * @param {(name: string) => Buffer} input.readMember   读一个成员的字节
 * @param {(name: string) => Buffer|null} input.repoLicense 工作树里同名许可文本的字节（缺失为 null）
 * @param {number} input.repoSrsCount  工作树 `resources/data/` 下的 `.srs` 份数
 * @param {number} input.fileSize      APK 文件本身的字节数（`stat`）
 * @param {number} input.entriesCompressedTotal 全部条目**压缩后**字节数之和（`unzip -Zt`）
 * @param {string} input.abi          按哪个 ABI 判（`ABIS` 的键；未知取值当场抛，不静默按缺省走）
 * @returns {string[]} 违反项（空数组 = 全部判据成立）
 */
export function apkViolations({
  names,
  readMember,
  repoLicense,
  repoSrsCount,
  fileSize,
  entriesCompressedTotal,
  abi,
  permissions,
}) {
  // 未知 / 漏传的 abi 在这里抛（不是 fail 一条）：判据的**取材面参数**错了时，
  // 下面每一条的结论都无意义，报成「某某不在包里」等于把参数错误伪装成产物缺陷。
  const { elfClass, machine, machineName } = abiSpec(abi);
  const errors = [];
  const fail = (msg) => errors.push(msg);
  const present = new Set(names);

  // ── ⓪ 扫描面自检：清单塌了的话，下面每一条正面断言都会红，但报错会指向「许可文本不在包里」
  //     这种**离真因两跳远**的地方。先在这里把「读不到清单」与「包里真没有」分开。
  if (names.length < MIN_APK_ENTRIES) {
    fail(
      `APK 成员清单只有 ${names.length} 条（真实 APK 实测 965–998 条）—— ` +
        '清单没读全或读到了空壳，下面的判据在这种输入上没有意义',
    );
    return errors;
  }

  // ── ① 三份许可文本：在包里 + 与工作树逐字节相同 ──────────────────────────────
  //     「在」守的是 §2.5 那类整条静默逃逸；「逐字节相同」守的是截断 / 占位 / 版本漂移
  //     （NOTICE 里写着随包 sing-box 版本，与 core-manifest 对拍的门在 verify-packaging 侧）。
  for (const name of LICENSE_FILES) {
    const entry = `${ASSET_UP}${name}`;
    const want = repoLicense(name);
    if (want === null) {
      fail(`工作树里没有 ${name} —— 判据的**真值那一侧**缺失，无从对拍（不是 APK 的问题）`);
      continue;
    }
    if (want.length === 0) {
      fail(`工作树里的 ${name} 是 0 字节 —— 存在 ≠ 有内容，拿它当真值等于判据恒真`);
      continue;
    }
    if (!present.has(entry)) {
      fail(
        `${entry} 不在 APK 里 —— 许可文本没有随包分发（MIT 要求版权与许可声明随**所有副本**，APK 是副本）。\n` +
          `  最可能的成因：build.gradle.kts 的 androidResources.ignoreAssetsPatterns 那段被删/改了 ` +
          `（AGP 默认忽略 \`_\` 开头的 assets 子目录，而 Tauri 的资源全铺在 ${ASSET_UP} 下），` +
          `或 tauri.android.conf.json 的 bundle.resources 里少了 ../${name}`,
      );
      continue;
    }
    const got = readMember(entry);
    if (!got.equals(want)) {
      fail(
        `${entry} 与工作树 ${name} 不一致：包内 ${got.length} 字节 / 工作树 ${want.length} 字节 —— ` +
          '随包那份不是真值那份（截断、占位或改了却没重新打包）',
      );
    }
  }

  // ── ② 原生库：两条都在、都是真 ELF ───────────────────────────────────────────
  for (const lib of NATIVE_LIBS) {
    const entry = `lib/${abi}/${lib}`;
    if (!present.has(entry)) {
      fail(`${entry} 不在 APK 里 —— 该 ABI 的原生库缺席，装上也起不来`);
      continue;
    }
    const bytes = readMember(entry);
    const ident = elfIdent(bytes);
    if (!ident.ok) {
      const head = bytes.subarray(0, 4);
      fail(
        `${entry} 不是 ELF（前四字节 ${JSON.stringify([...head])}；${ident.why}）—— ` +
          '读到的不是一个真的共享库，下面的构建 tag 判据在它上面无意义',
      );
      continue;
    }
    // 「在 lib/<ABI>/ 下」只是路径，**不是**「这份库是这个 ABI 的」。分包配错 / jniLibs 合并错时
    // 它装得上而起不来，且 ③⑥ 两条判据在一份架构错了的库上照样成立 —— 只有这里看得见。
    if (ident.elfClass !== elfClass || ident.machine !== machine) {
      fail(
        `${entry} 的 ELF 形态与 ABI ${abi} 对不上：读到 EI_CLASS=${ident.elfClass}` +
          `（ELF${ident.elfClass === 2 ? 64 : 32}）/ e_machine=0x${ident.machine.toString(16)}，` +
          `应为 EI_CLASS=${elfClass}（ELF${elfClass === 2 ? 64 : 32}）/ ` +
          `e_machine=0x${machine.toString(16)}（${machineName}）—— ` +
          '这份原生库不是这个 ABI 的（jniLibs 合并错 / splits 配错），装得上但起不来',
      );
    }
  }

  // ── ③ libbox.so 的构建 tag 指纹（naive / cronet）─────────────────────────────
  const boxEntry = `lib/${abi}/libbox.so`;
  if (present.has(boxEntry)) {
    const so = readMember(boxEntry);
    for (const { needle, min, why } of NATIVE_MARKERS) {
      const count = countOccurrences(so, needle);
      if (count <= min) {
        fail(
          `${boxEntry} 里 '${needle}' 只出现 ${count} 次（要求 > ${min}）—— ${why}。\n` +
            '  判据来源：scripts/build-libbox.sh 尾部的 aar 开箱验，同一阈值同一形状。',
        );
      }
    }
  }

  // ── ④ geo `.srs` 出厂副本：份数与工作树相等，且工作树份数非零 ───────────────
  //     与 ① 同一类（都住在 `assets/_up_/` 下），但它是**计数**判据，必须先钉住真值非零，
  //     否则「工作树被清空 + 包里也没有」会以 `0 === 0` 判绿。
  const inApk = names.filter((n) => n.startsWith(`${ASSET_UP}${SRS_DIR}/`) && n.endsWith('.srs')).length;
  if (repoSrsCount === 0) {
    fail(`工作树 ${SRS_DIR}/ 下一份 .srs 都没有 —— 真值为零，份数判据退化成恒真`);
  } else if (inApk !== repoSrsCount) {
    fail(
      `APK 内 ${ASSET_UP}${SRS_DIR}/ 有 ${inApk} 份 .srs，工作树 ${SRS_DIR}/ 有 ${repoSrsCount} 份 —— ` +
        '内置 geo 规则集没有随包（缺失时 route builder fail-closed 剪掉全部 geo 规则，' +
        '叠加回国模式即全量明文直连）',
    );
  }

  // ── ⑤ 死字节：文件大小必须 ≈ 全部条目压缩后之和（+ 可解释的 zip 结构开销）─────────
  //     AGP 更新一个**已存在**的 APK 时按 zip 增量写，被替换条目的旧数据不回收、只是不再被
  //     中央目录引用。这条判据存在的理由不是「包大不好看」，而是：**它是唯一能看见这件事的地方**。
  //     本轮实证——同一次剥符号改动，`libpolaris_lib.so` 从 515511872 掉到 127652816（少 388 MB），
  //     而保留旧包增量重打出来的 APK **一个字节都没小**（606871040 → 606871040）：省下的 388 MB
  //     原样躺在文件里当死字节。上面 ①②③④ 四条判据在那个包上**全绿**，`unzip -t` 也全绿。
  //
  //     写成正面形态：算出实际比值并要求它落在带内。带的**下沿**不是摆设 —— `ratio < 1`
  //     意味着文件比它自己条目的压缩和还小，物理上不可能，只可能是条目尺寸读成了 0 / 读错了包；
  //     纯上限判据在那种输入上恒绿。
  if (!(fileSize > 0)) {
    fail(`APK 文件大小读到 ${fileSize} —— 取材面塌了，死字节判据在这种输入上没有意义`);
  } else if (!(entriesCompressedTotal > 0)) {
    fail(
      `全部条目压缩后之和读到 ${entriesCompressedTotal}，而文件有 ${fileSize} 字节 —— ` +
        '没能从 zip 读出条目尺寸：`unzip -Zt` 的汇总行没解析到，' +
        '或它报的条目数与 `unzip -Z1` 数出来的对不上（那说明匹到的不是汇总行）。' +
        '这种输入下死字节判据恒真，必须红在取材面，不许判绿。',
    );
  } else {
    const r = deadByteReport({ names, fileSize, entriesCompressedTotal });
    if (r.ratio < 1) {
      fail(
        `文件大小 ${r.fileSize} 比全部条目压缩后之和 ${r.entriesTotal} 还小（比值 ${r.ratio.toFixed(6)} < 1）—— ` +
          '物理上不可能，说明读到的尺寸不是这个包的（取材面错了，不是包有问题）',
      );
    } else if (r.ratio > r.hi) {
      fail(
        `APK 里有 ${r.dead} 字节（${mib(r.dead)}）没有任何条目指向 —— ` +
          `文件 ${r.fileSize}（${mib(r.fileSize)}）/ 条目压缩和 ${r.entriesTotal}（${mib(r.entriesTotal)}）/ ` +
          `非条目字节 ${r.nonEntry}（${mib(r.nonEntry)}）/ zip 结构开销上界 ${r.bound}（${mib(r.bound)}）；` +
          `比值 ${r.ratio.toFixed(6)} 超出带 [1.000000, ${r.hi.toFixed(6)}]，死字节占整包 ` +
          `${((r.dead / r.fileSize) * 100).toFixed(1)}%。\n` +
          '  成因：AGP 增量更新已存在的 APK 时不回收被替换条目的旧数据。' +
          '**删掉这个 APK 再打一次**即可归零（本机实测：606871040 → 219011984，死字节 388 MB → 0）。\n' +
          '  这不是「包大了点」——它意味着分发给用户的包里有一半是谁都不会读的字节，' +
          '且任何靠重打包省下来的体积都会被它吃掉而不被观测到。',
      );
    }
  }

  // ── ⑥ 原生库必须**已剥 debug 信息** ────────────────────────────────────────
  //     没有这一条时的失效链（2026-09-05 实证）：把 `.cargo/config.toml` 那四段 rustflags
  //     删掉 ⇒ `libpolaris_lib.so` 从 127652816 弹回 515511872、APK 从 219011984 弹回
  //     606871040 ⇒ 而上面 ①②③④⑤ **五条全绿**。死字节那条也不会红：文件大小与条目压缩和
  //     是**一起**变大的，比值纹丝不动。整包翻三倍，没有一条腿说话。
  //
  //     判据按**节区**判，不按体积上限判：
  //       · 「剥过」的定义就是「没有 `.debug_*` 节」—— 判据与被判的事同一个东西，不需要中间量；
  //       · 体积上限得随依赖增长一路上调，最后腐烂成一个没人敢动的魔数，且它对
  //         「依赖变多」与「debug 信息回来了」这两件完全不同的事给出同一个信号。
  //     代价本来是「要从 APK 里读 ELF 节表」，但那个代价是零：判据 ② 为了验魔数
  //     已经把整份 `.so` 读进 `readMember` 的记忆里了，节表就在同一个 Buffer 里。
  //
  //     取材面**派生**而不枚举（与判据 ② 相反，两者是不同的问题）：
  //       · ② 断言的是「该在的在不在」，那只能靠枚举 —— 缺席的东西扫不出来；
  //       · ⑥ 断言的是「在场的每一份都得剥过」，枚举就意味着**新加一份原生库自动免检**。
  //     故本条扫 `lib/<ABI>/` 下的每一个 `.so`，扫到几份就报几份。
  const soEntries = names.filter((n) => n.startsWith(`lib/${abi}/`) && n.endsWith('.so'));
  if (soEntries.length === 0) {
    fail(
      `APK 里 lib/${abi}/ 下一个 .so 都没扫到 —— 取材面塌了（ABI 前缀写错？分包出的不是这个 ABI？），` +
        '「每一份都剥过了」在空集上恒真，不许判绿',
    );
  }
  for (const entry of soEntries) {
    if (elfIdent(readMember(entry)).ok) {
      const alignment = elfPageAlignment(readMember(entry));
      if (!alignment.ok) fail(`${entry}: ${alignment.why}`);
    }
    const lib = entry.slice(entry.lastIndexOf('/') + 1);
    const r = strippedReport(readMember(entry));
    if (r.why !== null) {
      fail(`${entry} ${r.why}`);
      continue;
    }
    if (!r.ok) {
      fail(
        `${entry} 里还留着 ${r.debug.length} 个调试信息节（共 ${r.debugBytes} 字节 / ${mib(r.debugBytes)}）：` +
          `${r.debug.join(' ')} —— 这份原生库没有被剥过。\n` +
          `  该包按未压缩（\`stor\`）存放原生库，所以这 ${mib(r.debugBytes)} 一字节不少地进了 APK，` +
          '而它在真机上零消费者（`.debug_*` 不带 SHF_ALLOC，加载器根本不映射）。\n' +
          `  看这里：${debugOwnerHint(lib)}。`,
      );
    }
  }

  // ── ⑦ 出厂权限集：逐条全等登记表（成因与失效链见 APK_PERMISSION_REGISTRY 的文档）──
  //     这是本文件唯一一条**不看 zip 成员**的判据：它的取材面是 `aapt2 dump permissions`，
  //     因为 APK 里的 `AndroidManifest.xml` 是二进制 AXML，`unzip -p` 拿到的字节读不出名字。
  //     取材面塌了（aapt2 缺席 / 输出读不出）时红在取材面，不判绿 —— 见 permissionViolations。
  errors.push(...permissionViolations(permissions === undefined ? null : permissions));

  return errors;
}

/** 字节数的 MiB 形态，只用于报文可读性。 */
function mib(bytes) {
  return `${(bytes / 1048576).toFixed(2)} MiB`;
}

/** `haystack` 里 `needle`（ASCII）出现的次数。重叠不计（与 Python `bytes.count` 同语义）。 */
export function countOccurrences(haystack, needle) {
  const pat = Buffer.from(needle, 'latin1');
  let count = 0;
  let at = 0;
  for (;;) {
    const hit = haystack.indexOf(pat, at);
    if (hit < 0) return count;
    count += 1;
    at = hit + pat.length;
  }
}

// ───────────────────────────── CLI（把上面的纯函数接到真实输入上）─────────────────────────────

/** APK 成员清单。`-Z1` = zipinfo 模式只打印成员名，无表头无汇总行。 */
function apkNames(apk) {
  return execFileSync('unzip', ['-Z1', apk], { maxBuffer: 64 * 1024 * 1024 })
    .toString('utf8')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
}

/**
 * 全部条目**压缩后**字节数之和。`unzip -Zt` 只打一行汇总，形如
 * `1001 files, 230140609 bytes uncompressed, 218812042 bytes compressed:  4.9%`
 * —— 比逐条目解析 `-Zl` 的表格稳（成员名可以含空格，Name 是最后一列，切列很脆），
 * 也比自己走中央目录省掉一整个 zip 解析器。
 *
 * `expectedEntries`（由 `unzip -Z1` 数出来的成员数）做**交叉核对**：两条命令读的是同一份
 * 中央目录，条目数对不上就说明匹到的不是我以为的那一行 —— 这是「正则匹到了一个数」与
 * 「正则匹到了**对的**数」之间的差，而本函数的输入是另一个程序的文本输出，那个差是真实风险。
 *
 * 两种失败都返回 `null`（**不是** 0）：0 会让判据在「没读到」和「真的是 0」之间失去分辨，
 * 而调用方对 `null` 的处理是红在取材面。
 */
function apkCompressedTotal(apk, expectedEntries) {
  const out = execFileSync('unzip', ['-Zt', apk], { maxBuffer: 1024 * 1024 }).toString('utf8');
  const m = /^\s*(\d+) files?, \d+ bytes uncompressed, (\d+) bytes compressed/m.exec(out);
  if (!m) return null;
  if (Number(m[1]) !== expectedEntries) return null;
  return Number(m[2]);
}

/**
 * 读一个成员的字节。`-p` = 解到 stdout，不落盘、不建目录。
 *
 * 带**记忆**：判据与下面那份收据都要读同样几个成员，`libbox.so` 一次 80 MB。
 * 除了省两遍解压，更重要的是**判定与打印读的是同一份字节** —— 分两次读，
 * 打印出来的收据在原理上可以与被判定的内容不同。
 */
const memberCache = new Map();
function apkMember(apk, name) {
  const key = `${apk}\u0000${name}`;
  if (!memberCache.has(key)) memberCache.set(key, readMemberUncached(apk, name));
  return memberCache.get(key);
}

/**
 * 找 `aapt2`：`$ANDROID_HOME/build-tools/<最高版本>/aapt2`。找不到返回 `null`。
 *
 * 不猜 PATH 之外的位置、不静默降级到「跳过这条判据」—— `null` 会一路走到
 * [`permissionViolations`] 变成一条**红**，因为「没跑」与「跑过且通过」在退出码上必须可分辨。
 */
function aapt2Path() {
  const home = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
  if (!home) return null;
  const dir = join(home, 'build-tools');
  if (!existsSync(dir)) return null;
  const versions = readdirSync(dir)
    .filter((v) => existsSync(join(dir, v, 'aapt2')))
    // 版本号按段比大小，不按字典序：字典序会把 `9.0.0` 排在 `36.0.0` 后面。
    .sort((a, b) => {
      const pa = a.split('.').map(Number);
      const pb = b.split('.').map(Number);
      for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
        const d = (pb[i] || 0) - (pa[i] || 0);
        if (d !== 0) return d;
      }
      return 0;
    });
  return versions.length > 0 ? join(dir, versions[0], 'aapt2') : null;
}

/**
 * 问 aapt2 要这个 APK 的权限集。拿不到（工具缺席 / 调用失败 / 输出读不出）一律 `null`，
 * 由判据 ⑦ 红在取材面。
 */
function apkPermissions(apk) {
  const bin = aapt2Path();
  if (bin === null) return null;
  try {
    const out = execFileSync(bin, ['dump', 'permissions', apk], { maxBuffer: 8 * 1024 * 1024 });
    return parseAapt2Permissions(out.toString('utf8'));
  } catch {
    return null;
  }
}

function readMemberUncached(apk, name) {
  // debug APK 里未 strip 的 `.so` 实测 80 MB（libbox）/ 508 MB（libpolaris_lib）。
  // maxBuffer 给到 1 GiB：超了会抛 ENOBUFS，是**响亮的失败**，不是静默截断。
  return execFileSync('unzip', ['-p', apk, name], { maxBuffer: 1024 * 1024 * 1024 });
}

function main() {
  let apk;
  let abi;
  try {
    ({ apk, abi } = parseArgs(process.argv.slice(2)));
  } catch (e) {
    console.error(e.message);
    process.exit(2);
  }
  if (!apk) {
    console.error('用法：node scripts/verify-apk.mjs <apk 路径> [--abi <ABI>]');
    process.exit(2);
  }
  if (!existsSync(apk)) {
    console.error(`读不到 APK：${apk}`);
    process.exit(2);
  }

  const srsDir = join(ROOT, SRS_DIR);
  const repoSrsCount = existsSync(srsDir)
    ? readdirSync(srsDir).filter((n) => n.endsWith('.srs')).length
    : 0;

  const names = apkNames(apk);
  const fileSize = statSync(apk).size;
  // `null`（aapt2 缺席 / 输出读不出）原样传下去 —— 判据 ⑦ 会红在取材面而不是判绿。
  const permissions = apkPermissions(apk);
  // `null`（解析不出汇总行）原样传下去 —— `apkViolations` 会红在取材面而不是判绿。
  const entriesCompressedTotal = apkCompressedTotal(apk, names.length);
  const violations = apkViolations({
    names,
    readMember: (name) => apkMember(apk, name),
    repoLicense: (name) => {
      const path = join(ROOT, name);
      return existsSync(path) ? readFileSync(path) : null;
    },
    repoSrsCount,
    fileSize,
    entriesCompressedTotal,
    abi,
    permissions,
  });

  // 收据：不管红绿都把**实际数到的东西**打出来。只打印 "ok" 的门，红的时候没人知道它平时在看什么。
  // ABI 一并打出来：判据全挂在它上面，收据不说清按哪个 ABI 判，读的人无从知道量的是哪半个包。
  console.log(`APK：${apk}（${names.length} 条成员，按 ABI ${abi} 判）`);
  for (const name of LICENSE_FILES) {
    const entry = `${ASSET_UP}${name}`;
    console.log(
      `  ${names.includes(entry) ? '✔' : '✘'} ${entry.padEnd(40)} ` +
        `${names.includes(entry) ? apkMember(apk, entry).length : 0} 字节`,
    );
  }
  const boxEntry = `lib/${abi}/libbox.so`;
  if (names.includes(boxEntry)) {
    const so = apkMember(apk, boxEntry);
    const counts = NATIVE_MARKERS.map(({ needle }) => `${needle}=${countOccurrences(so, needle)}`);
    console.log(`  ✔ ${boxEntry.padEnd(40)} ${so.length} 字节，${counts.join(' ')}`);
  } else {
    console.log(`  ✘ ${boxEntry} 不在包里`);
  }
  const inApk = names.filter((n) => n.startsWith(`${ASSET_UP}${SRS_DIR}/`) && n.endsWith('.srs')).length;
  console.log(`  ${inApk === repoSrsCount && repoSrsCount > 0 ? '✔' : '✘'} .srs：APK ${inApk} 份 / 工作树 ${repoSrsCount} 份`);
  if (entriesCompressedTotal > 0) {
    const r = deadByteReport({ names, fileSize, entriesCompressedTotal });
    const ok = r.ratio >= 1 && r.ratio <= r.hi;
    console.log(
      `  ${ok ? '✔' : '✘'} 死字节：文件 ${mib(r.fileSize)} / 条目压缩和 ${mib(r.entriesTotal)} / ` +
        `非条目 ${r.nonEntry} 字节（结构开销上界 ${r.bound}）⇒ 死字节 ${Math.max(r.dead, 0)} 字节` +
        `（${((Math.max(r.dead, 0) / r.fileSize) * 100).toFixed(2)}%）；` +
        `比值 ${r.ratio.toFixed(6)} 落在带 [1.000000, ${r.hi.toFixed(6)}] ${ok ? '内' : '**外**'}`,
    );
  } else {
    console.log(`  ✘ 死字节：读不到条目压缩和（unzip -Zt 汇总行没解析到）`);
  }
  if (permissions === null) {
    console.log('  ✘ 出厂权限集：读不到（aapt2 缺席，或 dump 输出里没有 package: 行）');
  } else {
    const want = new Set(APK_PERMISSION_REGISTRY.map((e) => e.name));
    const okPerm = permissions.uses.length === want.size && permissions.uses.every((n) => want.has(n));
    console.log(
      `  ${okPerm ? '✔' : '✘'} 出厂权限集：uses-permission ${permissions.uses.length} 条 / ` +
        `登记 ${want.size} 条；自建 permission ${permissions.declared.length} 条 / ` +
        `登记 ${APK_DECLARED_PERMISSIONS.length} 条`,
    );
    for (const name of [...permissions.uses].sort()) {
      const entry = APK_PERMISSION_REGISTRY.find((e) => e.name === name);
      console.log(`      ${entry ? '✔' : '✘'} ${name.padEnd(56)} ${entry ? entry.from : '**没登记**'}`);
    }
  }
  const soEntries = names.filter((n) => n.startsWith(`lib/${abi}/`) && n.endsWith('.so'));
  console.log(`  ${soEntries.length > 0 ? '✔' : '✘'} 原生库剥符号：lib/${abi}/ 下扫到 ${soEntries.length} 份 .so`);
  for (const entry of soEntries) {
    const r = strippedReport(apkMember(apk, entry));
    const tail =
      r.why !== null
        ? r.why
        : `节区 ${r.sections} 个，调试信息节 ${r.debug.length} 个` +
          (r.debug.length > 0 ? `（${r.debug.join(' ')}，${mib(r.debugBytes)}）` : '');
    console.log(`      ${r.ok ? '✔' : '✘'} ${entry.padEnd(34)} ${tail}`);
  }

  if (violations.length > 0) {
    console.error(`\nverify-apk: ${violations.length} 条判据未通过：`);
    for (const v of violations) console.error(`  ✘ ${v}`);
    process.exit(1);
  }
  console.log('\nok: verify-apk 全部产物级判据成立。');
}

const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) main();
