#!/usr/bin/env node

/**
 * 「Android 专属 Rust 源文件」的**产物级**完备性门（2026-09-05）。
 *
 * # 为什么换真值源
 *
 * `ANDROID_IMPACT_SCOPES` 是 APK 腿（`.github/workflows/android.yml`，无 push/PR 触发）的唯一触发面：
 * 新增一个 Android 专属文件而不登记 ⇒ 那条腿永远不为它跑。此前两轮完备性门的取材面都是
 * **源码级正则推导**（扫 `target_os = "android"`，再补「cfg 写在父模块 `mod` 声明上」），
 * 每补一种形态，下一轮验收就找出下一种：
 *
 *   ① cfg 写在父模块 `mod` 声明上（正是 `src-tauri/src/android_tls.rs` 自己的形态）
 *   ② **两层嵌套**：已登记文件里再 `mod` 出一个子文件 ⇒ 全新 Android 专属文件完全隐形
 *   ③ `#[cfg_attr(target_os = "android", path = "…")]` 让门指到**非 android 的那份**文件上
 *   ④ 内联 Rust 夹具的 raw string 里写 `mod` ⇒ 判据被自己污染
 *   ⑤ `#[cfg(target_os = r"android")]`（与不带 `r` 语义等价，rustc E0428 反证过）绕过正则
 *   ⑥ `#[cfg(...)]mod foo;` 零空白
 *
 * 共同根因是**一个**：在用正则重新实现 rustc 的模块解析与 cfg 求值。继续补形态是打地鼠。
 *
 * 本门改问编译器：`--emit=dep-info`（`cargo check` 默认下发，不需要动 RUSTFLAGS）产出的 `.d`
 * 是 rustc 在**解析完 `mod` / `#[path]` / `include!` 并求值完 cfg 之后**给出的源文件清单。
 *
 *     Android 专属文件集 = （android target 的编译面） − （host target 的编译面）
 *
 * 上面六种形态一个都骗不了它（隔离工程实证，六种一次写全，六条全部落在差集里；④ 的 raw string
 * 夹具两侧 `.d` 都没有它——rustc 解析的是真模块图，字符串里的文本结构上就进不来）。
 *
 * # 用法
 *
 *     node scripts/check-android-only-face.mjs --android <a.json> --host <h.json> [--inject <相对路径>]…
 *
 * 两份 JSON 是 `cargo check … --message-format=json-render-diagnostics` 的 **stdout**（用 `>` 重定向，
 * 不许接管道：管道会让 cargo 自己的 rc 失真）。`--inject` 往 android 面里塞一个假想文件，用于反向对照。
 *
 * rc：0 = 全部已登记；1 = 有 Android 专属文件没登记；2 = **自曝**（取材面塌了，红的不是仓库而是这道门）。
 *
 * # 🔴 为什么必须按 unit 定向取 `.d`，不许 glob `deps/*.d`
 *
 * `.d` 会腐烂：`target/` 被 `Swatinem/rust-cache` 跨 run 缓存，同一个 crate 在 deps/ 下会同时躺着
 * check / build / clippy / test 四种 mode、以及历史 config 留下的孤儿 `.d`。本机实测撞到过一份
 * `polaris_lib-*.d` 里挂着**磁盘上早已不存在**的 `src-tauri/src/android_tls/ax3_child.rs`，
 * 直到下一次 check 才被改写。glob 的后果是 **fail-open**：host 侧多收一条陈旧记录，差集就少一条。
 * 故取材走 `compiler-artifact` 消息的 `filenames` —— 那是**本次调用实际参与的编译单元**。
 *
 * # 这门抓不到什么（如实登记，别把 rc=0 读成「Android 影响面登记表整体完备」）
 *
 *  1. **共享文件里的 android 条件代码**：`#[cfg(target_os = "android")] fn f()` 写在一个两侧都编的
 *     文件里 ⇒ 它不在差集里。这一类**必须再拆一半**，两半今天的守法不同（2026-09-05 改写：上一版
 *     整类推给 B 组，而 B 组的针够不着其中一半）：
 *      · 文件**自己含** `target_os = "android"` 字样 —— `android_bridge.rs`（27 处）、
 *        `stats/source.rs`（27 处）都是这一类 ⇒ 归
 *        `ui/src/contracts/android-impact-coverage-contract.test.ts` 的 B 组：那一组的针就是这段
 *        文本（`/target_os\s*=\s*(?:r#*)?"android"/`，两个剥离面合取），扎得到。
 *      · 文件**自己不含**该字样、只在 android 调用点被消费（`#[cfg]` 写在**调用方**）⇒ **两门皆盲**：
 *        B 组的针一处都扎不到它，本门也看不见它（两侧都编 ⇒ 不进差集），分类器同样不会为它点亮
 *        APK 腿。它今天的实际覆盖只有 ci.yml 的 aarch64 交叉腿兜住的**编译面**
 *        （`cargo clippy --target aarch64-linux-android --all-targets` 那个循环，外加
 *        `cargo check --target aarch64-linux-android -p polaris`）；**产物面本腿没有任何判据**。
 *        🔴 如实登记，**不为它再造一道门**：要扎到这一类，取材面就是整棵 Rust 树（「谁被 android
 *        调用点消费」= 跨文件可达性分析），那是一条比它所守的东西重得多的腿。这条的价值在于
 *        **不假装守住了**。
 *  2. **build script**：`--target` 交叉时 build script 仍编给 host，两侧同源 ⇒ `src-tauri/build.rs`
 *     永远不可能落进差集；build script 内部 `if target_os == "android"` 的运行期分支本门全盲。
 *     `cargo:rerun-if-changed=` 声明的文件也不进 dep-info（实测：`src-tauri/build.rs` 声明了
 *     tauri.conf.json，而全部 `.d` 里 `tauri.conf` 零命中）。进 dep-info 的只有 **rustc 自己读过**的
 *     文件：`mod` / `#[path]` / `include!` / `include_str!` / `include_bytes!`（含宏展开产生的）。
 *
 *     🔴 **后果与口径**（2026-09-05 补，上一版只登记了机制、没登记后果）：一个**仓内 `.rs` 源文件**
 *     若只在 android 构型下被 build script 读走、写进 `OUT_DIR`、再被 android-only 模块 `include!`
 *     进来，它的代码**真的编进 android 产物**，而它的**模板路径**结构上进不了 dep-info —— rustc 读到
 *     的是 OUT_DIR 里那份生成物，而 OUT_DIR 在 `targetDir` 之下、被 `sourcesOf()` 整类剔掉。
 *     故口径是：**新增这类代码生成时，必须手工把模板路径登记进 `ANDROID_IMPACT_SCOPES`** ——
 *     本门不会替你发现它。
 *
 *     **今天不活**（2026-09-05 实测）：全仓只有两个 build script（`src-tauri/build.rs`、
 *     `crates/singbox-grpc/build.rs`），OUT_DIR 代码生成全仓只有一处 ——
 *     `crates/singbox-grpc/src/lib.rs` 的 `include!(concat!(env!("OUT_DIR"), "/daemon.rs"))`，
 *     模板是 `.proto` 而不是 `.rs`，且那个 crate 两侧都编、结构上不在差集里。`src-tauri/build.rs`
 *     一处 OUT_DIR 代码生成都没有（`OUT_DIR` / `fs::write` / `File::create` 零命中；它只发
 *     `cargo:rerun-if-changed=` / `cargo:rustc-env=` / `cargo:rustc-link-arg-tests=` 三类指令）。
 *     附带一条要记住的：那份 `.proto` 之所以**仍然**出现在 `.d` 里，靠的是 `proto_wire_check.rs`
 *     里一条独立的 `include_str!("proto/started_service.proto")`，**不是**靠 build script 的
 *     `rerun-if-changed` —— 删掉那条 `include_str!`，它当场从 dep-info 里消失。
 *  3. **只覆盖 aarch64 一个 ABI**：`gen/android/buildSrc` 的 RustPlugin 默认打四个 ABI，
 *     `#[cfg(all(target_os = "android", target_arch = "arm"))]` 这类文件本门看不见。今天的仓里
 *     没有这种形态，但这是**已知上限，不是「验过了」**。要补就是四个 target 的并集，代价 ×4。
 *  4. **两侧选择器必须逐字对称**（见 ci.yml 那一步的注释）：host 侧一旦比 android 侧宽（例如为了
 *     省钱去复用 `cargo clippy --workspace --all-targets` 的输出），`#[cfg(any(target_os = "android",
 *     test))]` 的文件就会同时出现在 host 的 test 单元里 ⇒ 从差集里掉出去。方向是 **fail-open**。
 *     故两侧都是 `cargo check --workspace`（lib + bins），代价是**测试目标里的 android 专属代码
 *     不在射程内**。
 *  5. **feature 门控**（而非 cfg 门控）的 android 专属文件结构上不可见：对差变的是 target，不是
 *     feature 集。今天不活（android.yml 与 tauri.android.conf.json 都不传额外 `--features`）。
 *  6. **proc macro 在展开期自己 `fs::read` 的文件**不进 dep-info（`proc_macro::tracked_path` 至今
 *     unstable）。**这条未实测**，从 rustc 机制推得，置信度 medium。
 *
 * # 🔴 别名不生效：`android_tls/child.rs` 不会因为 `android_tls.rs` 已登记而算「登记过」
 *
 * 本门用的是 `androidRegistrationOf()`（精确键 / 目录前缀键），**不走** `moduleAliasesOf()` 的
 * `foo.rs` ↔ `foo/` 别名。这是刻意的：桌面轴的别名规则说「同一个模块换个路径形态、判断没变」，
 * 而 Android 轴上「已登记文件里 `mod` 出一个新子文件」恰恰是要人来重判一次的形态（它可能是
 * 全新的 JNI 导出点 / 全新的 android-only 依赖消费点）。方向是 fail-closed：逼一条显式登记。
 */

import { readFileSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname, basename, join, resolve, relative, isAbsolute } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, '..');

/** 自曝：取材面塌了。rc=2 与「有文件没登记」(rc=1) 分开，日志里一眼能分清红的是谁。 */
function selfExposeFail(lines) {
  for (const line of lines) console.error(`::error::${line}`);
  process.exit(2);
}

function parseArgs(argv) {
  const out = { android: null, host: null, inject: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const key = argv[i];
    if (key === '--android') out.android = argv[++i];
    else if (key === '--host') out.host = argv[++i];
    else if (key === '--inject') out.inject.push(argv[++i]);
    else selfExposeFail([`认不出的参数：${key}`]);
  }
  if (!out.android || !out.host) {
    selfExposeFail(['用法：--android <a.json> --host <h.json> [--inject <相对路径>]…']);
  }
  return out;
}

/**
 * workspace 成员名与 target 目录的真值来自 `cargo metadata`，不从路径字符串猜。
 *
 * `target/` 前缀不许写死：`CARGO_TARGET_DIR` 可以指到仓外，写死就会把 OUT_DIR 生成物
 * （如 `…/out/daemon.rs`）当成仓内源文件收进面里。
 */
function cargoMetadata() {
  let raw;
  try {
    raw = execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--offline'], {
      cwd: REPO_ROOT,
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    });
  } catch (err) {
    selfExposeFail([`cargo metadata 跑不起来，成员清单取不到：${err.message}`]);
  }
  const meta = JSON.parse(raw);
  const members = new Map();
  for (const pkg of meta.packages) members.set(resolve(pkg.manifest_path), pkg.name);
  return { members, targetDir: resolve(meta.target_directory) };
}

/**
 * 一份 cargo JSON 日志里**本次实际参与**的编译单元 → 它们各自的 `.d`。
 *
 * `custom-build` 单元整类跳过：交叉编译时 build script 仍编给 host，两侧同源，结构上不可能落进
 * 差集；它的产物 `build-script-build` 还没有扩展名（`.d` 却叫 `build_script_build-<hash>.d`），
 * 留着只会让「.d 找不到」这条自曝恒红。跳过的条数照样计数并打印，不做静默。
 */
function unitsOf(logPath, members) {
  if (!existsSync(logPath)) selfExposeFail([`cargo JSON 日志不存在：${logPath}`]);
  const dFiles = new Set();
  const packages = new Set();
  const missing = [];
  let artifacts = 0;
  let buildScripts = 0;
  let foreign = 0;
  for (const line of readFileSync(logPath, 'utf8').split('\n')) {
    if (!line.startsWith('{')) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      continue;
    }
    if (msg.reason !== 'compiler-artifact') continue;
    const name = members.get(resolve(msg.manifest_path ?? ''));
    if (name === undefined) {
      foreign += 1; // registry / path 依赖：不可能是仓内源文件的来源
      continue;
    }
    if ((msg.target?.kind ?? []).includes('custom-build')) {
      buildScripts += 1;
      continue;
    }
    artifacts += 1;
    packages.add(name);
    // 只认 `deps/` 下那份产物：uplift 出来的副本不带 hash、也没有配套 `.d`。
    let hit = false;
    for (const file of msg.filenames ?? []) {
      const dir = dirname(file);
      if (basename(dir) !== 'deps') continue;
      const stem = basename(file).replace(/\.[^.]+$/, '');
      for (const candidate of [stem, stem.replace(/^lib/, '')]) {
        const dPath = join(dir, `${candidate}.d`);
        if (existsSync(dPath)) {
          dFiles.add(dPath);
          hit = true;
          break;
        }
      }
      if (hit) break;
    }
    if (!hit) missing.push(`${name} / ${msg.target?.name} [${(msg.target?.kind ?? []).join(',')}]`);
  }
  return { dFiles, packages, missing, artifacts, buildScripts, foreign };
}

/**
 * 一批 `.d` 的 phony 段 → 仓内源文件的相对路径集合。
 *
 * `.d` 是 Makefile 规则：前两行 `<产物>: dep dep …`，其后每个前置文件一条空规则 `dep:`，
 * 末尾可能有 `# env-dep:…`。取 phony 段最省事（长规则行里的转义空格不必再拆一遍）。
 *
 * 路径**未规范化**（实见 `src-tauri/src/../../ui/src/i18n/locales/auxiliary/zh-CN.json`、
 * `crates/singbox-grpc/src/../proto/started_service.proto`）⇒ 必须 `resolve` + `relative` 之后再比，
 * 直接与 `git ls-files` 的字符串比会漏。
 */
function sourcesOf(dFiles, targetDir) {
  const out = new Set();
  for (const dPath of dFiles) {
    for (const rawLine of readFileSync(dPath, 'utf8').split('\n')) {
      const line = rawLine.trimEnd();
      if (line === '' || line.startsWith('#')) continue; // `# env-dep:` 段
      if (!line.endsWith(':')) continue; // 规则行（`<产物>: dep …`）不是 phony 行
      const path = line.slice(0, -1).replace(/\\ /g, ' ');
      if (path === '' || path.includes(': ')) continue;
      const abs = isAbsolute(path) ? path : resolve(REPO_ROOT, path);
      if (abs === targetDir || abs.startsWith(`${targetDir}/`)) continue; // OUT_DIR 生成物
      const rel = relative(REPO_ROOT, abs);
      if (rel === '' || rel.startsWith('..')) continue; // registry / sysroot
      out.add(rel);
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
const { members, targetDir } = cargoMetadata();
const memberNames = new Set(members.values());
const android = unitsOf(args.android, members);
const host = unitsOf(args.host, members);

const androidFiles = sourcesOf(android.dFiles, targetDir);
const hostFiles = sourcesOf(host.dFiles, targetDir);
for (const path of args.inject) androidFiles.add(path);

const androidOnly = [...androidFiles].filter((p) => !hostFiles.has(p)).sort();
const hostOnly = [...hostFiles].filter((p) => !androidFiles.has(p)).sort();

console.log(
  `android：unit ${android.dFiles.size}（包 ${android.packages.size}，build script 跳过 ${android.buildScripts}，`
    + `仓外 ${android.foreign}）→ 仓内源文件 ${androidFiles.size}`,
);
console.log(
  `host   ：unit ${host.dFiles.size}（包 ${host.packages.size}，build script 跳过 ${host.buildScripts}，`
    + `仓外 ${host.foreign}）→ 仓内源文件 ${hostFiles.size}`,
);

/* ───────────────────────────── 自曝（rc=2）───────────────────────────── */
//
// 「派生塌了」与「今天真的没有 Android 专属文件」长得一模一样，两者都是**空差集**。
// 下面五条把它们分开：任何一条不成立，红的是这道门自己，不是仓库。
const broken = [];

for (const [side, unit] of [['android', android], ['host', host]]) {
  if (unit.missing.length > 0) {
    broken.push(
      `${side} 侧有 ${unit.missing.length} 个编译单元找不到配套 .d（取材面在这里断了，不许静默跳过）：`
        + unit.missing.slice(0, 5).join(' | '),
    );
  }
  const absent = [...memberNames].filter((n) => !unit.packages.has(n)).sort();
  if (absent.length > 0) {
    broken.push(
      `${side} 侧的编译面漏了 ${absent.length} 个 workspace 成员：${absent.join(', ')}`
        + ' —— 选择器不是 --workspace，或者 cargo 半途失败只留下了局部单元集。',
    );
  }
}
if (androidFiles.size < 100 || hostFiles.size < 100) {
  broken.push(
    `源文件面太小（android ${androidFiles.size} / host ${hostFiles.size}，下限 100）——`
      + ' .d 解析器塌了，或者 cargo 只跑了一小撮包。',
  );
}
// 正面哨兵：全仓唯一的 JNI 导出点，必然只在 android 上编译。它掉出差集 ⇒ 对差机器坏了。
const SENTINEL = 'src-tauri/src/android_tls.rs';
if (!androidOnly.includes(SENTINEL)) {
  broken.push(
    `哨兵 ${SENTINEL} 不在 android 专属集合里 —— 它是全仓唯一的 JNI 导出点、cfg 写在 lib.rs 的`
      + ' `#[cfg(target_os = "android")] mod android_tls;` 上，必然只在 android 编译面内。'
      + '它掉出去只有两种可能：对差机器坏了，或者那个文件真的变成两侧都编（那就得换一个哨兵并重判本门射程）。',
  );
}
// 反向对照：cfg 求值必须在**两个**方向上都起作用。host-only 为空 ⇒ 两侧极可能算成了同一个 target。
if (hostOnly.length === 0) {
  broken.push(
    'host 专属集合为空 —— 反向对照塌了。今天它应当至少含 crates/helper/src/platform/linux/**，'
      + '为空说明两侧编的是同一个 target（`--target` 没生效 / 两份日志同源）。',
  );
}
if (broken.length > 0) selfExposeFail(broken);

/* ───────────────────────────── 主断言（rc=1）───────────────────────────── */

const { androidRegistrationOf } = await import(
  pathToFileURL(join(REPO_ROOT, 'scripts/classify-ci-impact.mjs')).href
);

console.log(`\nAndroid 专属源文件 ${androidOnly.length} 条：`);
const unregistered = [];
for (const path of androidOnly) {
  const hit = androidRegistrationOf(path);
  const ok = hit !== null && hit.table === 'ANDROID_IMPACT_SCOPES';
  console.log(`  ${ok ? '✓' : '✗'} ${path}  ${hit ? `${hit.table}[${hit.key}]` : '两张表都查不到'}`);
  if (!ok) unregistered.push({ path, hit });
}
console.log(`host 专属源文件 ${hostOnly.length} 条（反向对照，头 3 条）：`);
for (const path of hostOnly.slice(0, 3)) console.log(`  · ${path}`);

if (unregistered.length > 0) {
  for (const { path, hit } of unregistered) {
    console.error(
      `::error::${path} 只在 aarch64-linux-android 上编译，却${
        hit === null ? '在两张 Android 表里都查不到' : `被登记在 ${hit.table}（应为 ANDROID_IMPACT_SCOPES）`
      }`,
    );
  }
  console.error(
    '::error::Android 专属文件必须登记进 scripts/classify-ci-impact.mjs 的 ANDROID_IMPACT_SCOPES（附 why）——'
      + ' 那张表是 .github/workflows/android.yml 的唯一触发面（它没有 push/PR 触发），'
      + '不登记 ⇒ APK 腿永远不为这个文件跑。'
      + '判成「不影响 APK 产物」也不行：本门的射程是「只在 android 上编译的文件」，'
      + '那正是链接期与随包 `.so` 内容的来源。',
  );
  process.exit(1);
}

console.log('\n✓ 全部 Android 专属源文件都已登记进 ANDROID_IMPACT_SCOPES');
