#!/usr/bin/env node
// gate-android-release-behavior.mjs —— Android release 路径的**行为**裁判（配置期）。
//
// ── 为什么存在（根因）────────────────────────────────────────────────────────────
//
// release 路径此前的全部判据都是源码级的：「`build.gradle.kts` 里含某串」。2026-09-05 的
// 逐行变异验收在 30 多条探针里打出 **24 条门全绿**，每一条的形状都一样 ——
// **文本里那串还在，行为已经变了**：
//
//   · `proguardFiles(...)` 的实参只喂三份规则里的一份 ⇒ 另两份整体失效，门全绿；
//   · 守卫谓词 `||` 改 `&&` ⇒ 零凭据跑 release 的 rc 由 1 变 0，守卫永久闭嘴，门全绿；
//   · 守卫体顶部插一句 `return@Action` ⇒ 被断言的串一个不少，守卫既不判也不报，门全绿；
//   · `isMinifyEnabled = true` 后面追一行 `false` ⇒ `minifyArm64ReleaseWithR8` 从任务图消失，
//     本批全部 keep / dontwarn 一次都不生效，门全绿；
//   · 逃生门读取点后面接 `|| System.getenv(...)` ⇒ 永久开关从另一扇门回来，门全绿。
//
// 判据形态选错了：要守的是构建的**行为**，判据却在断言构建脚本的**文本**。
// 本脚本换裁判 —— 跑一次 gradle 配置期，问 Gradle 自己。
//
// ── 判据都问同一个人：gradle 的配置期 ───────────────────────────────────────────
//
//   ./gradlew --offline --no-daemon :app:assembleArm64Release --dry-run     # 看 rc / 任务图 / 报错原文
//   ./gradlew --offline --no-daemon :app:printPolarisReleaseFacts           # 看 AGP 手里的事实
//
// `--dry-run` 只跑到任务图就停：`gradle.taskGraph.whenReady` 的守卫照常开口（它挂在图上，
// 不挂在执行上），而一个字节的代码都不用编。本机实测一轮约 12 秒。
// `printPolarisReleaseFacts` 住在 `app/polaris-release-facts.gradle.kts`（**不是**被审判的
// `app/build.gradle.kts`），把 AGP 已经装配好的 `proguardFiles` 清单、签名配置的 `storeFile`、
// 逃生门的三个观察值原样打出来 —— 判断在这里，构建脚本不给自己判分。
//
// ═══════════════════════════════════════════════════════════════════════════════
// 🔴 能力边界（天花板声明，2026-09-05 收官轮立；2026-09-06 终止轮逐句复核并收窄）
// —— 这一段是判据的一部分，不是免责声明
// ═══════════════════════════════════════════════════════════════════════════════
//
// 本裁判跑的是 gradle 的**配置期**（`--dry-run` 到任务图为止 + 一个只读任务）。
// 而 release 构建的另一半发生在**执行期**：任务真的跑起来、R8 真的读文件、产物真的写出来。
// 配置期看得见的与看不见的，界线是硬的：
//
// ┌── 看得见（本裁判守，逐条对应下面的 ①–⑩）────────────────────────────────────┐
// │ · 任务图里有哪些任务（`minifyArm64ReleaseWithR8` 在 release 图里 1 次 / debug 图里 0 次）│
// │ · `gradle.taskGraph.whenReady` 守卫开不开口、报的是哪一句、点不点名到变量            │
// │ · AGP 装配好的 `proguardFiles`：release **构建类型**那一份的路径、份数、逐份字节数、 │
// │   集合是否恰等（⑦）；以及 `defaultConfig` 与 `productFlavor` 两处必须为空（⑦b）——   │
// │   AGP 口径是三处合并后喂给 R8（**本轮没在 R8 输入那一层实测**，那要真跑一次构建）；  │
// │   实测到的是**取材面**：往 `defaultConfig` 挂一份 `.pro`，⑦b 之前的判据零失败 rc=0。 │
// │   两处为空时，buildType 那一份就是本仓自有规则的全集，⑦ 的恰等才名副其实            │
// │ · 逃生门的来源，**只覆盖枚举得到的这四形**：命令行 `-P` 认、`gradle.properties` 不认、│
// │   `ORG_GRADLE_PROJECT_polarisAllowUnsigned` 不认、`POLARIS_ALLOW_UNSIGNED` 不认      │
// │ · 签名的**容器侧**：`signingConfigs` 里那份 `release` 配置的 `storeFile` 指向哪儿、  │
// │   在不在盘上、读不读得动、是不是 `POLARIS_KEYSTORE_FILE` 指的那一份                  │
// └───────────────────────────────────────────────────────────────────────────────┘
//
// ┌── 看不见（**结构上**看不见，本裁判不假装守）───────────────────────────────────┐
// │ ① 任务被挂上执行期开关。`tasks.matching { … }.configureEach { onlyIf { false } }`   │
// │    或 `enabled = false`：任务**仍在任务图里**（`--dry-run` 照样打印它），只是真跑时  │
// │    被跳过。本裁判的 ⑤ 数的是任务图，数不出「它到时候会不会真跑」。                   │
// │ ② `doFirst` / `doLast` 在执行期改文件。`doFirst { proguard-rules.pro.writeText("") }`│
// │    ⇒ 配置期读到的清单与字节数全是对的，R8 到手的却是一份空规则。⑦ 量的是配置期那一刻。│
// │ ③ 任务真正执行后的**产物**：R8 吃进去的规则全文、每条 keep 命中了什么、包里还剩什么。 │
// │ ④ **签名这件事本裁判只看到容器那一层**（2026-09-06 终止轮 R4 —— 上一版把「签名」整个 │
// │    写进「看得见」，那句话过宽）。今天看得见的只有：`signingConfigs` 容器里那份配置的  │
// │    `storeFile` 指哪儿、在不在、读不读得动。看不见的是另外两层：                      │
// │    · **接线**：release 构建类型到底有没有把那份配置接上（`buildType.signingConfig`）。│
// │      ⑩ 读的是 `signingConfigs.findByName("release")`，也就是**容器**；守卫读的       │
// │      `releaseSigningConfig` 同样是容器。两条判据都不看 buildType 那一侧。            │
// │      实测（终止轮 R4）：在 release 块里那行接线之后补一句 `signingConfig = null`，    │
// │      本裁判零失败 rc=0，源码侧 12 项全绿 —— 因为源码侧数的是那行接线**出现         │
// │      几次**，而它一次没少。                                                         │
// │    · **有效性**：storeFile 是不是一个合法的密钥库、口令对不对、别名存不存在           │
// │      （那三件只有 apksigner / jarsigner / keytool 真跑才知道）。                     │
// │ ⑤ 逃生门**换一个新变量名**那一形（2026-09-06 终止轮 R3）。两侧门各守一头，中间有缝：  │
// │    · 本裁判的 ④/④b 是一张**枚举表**：只试上面列出的那四个来源，新名字它不试。         │
// │    · 源码侧那条形状断言钉死的是**读取点那一条绑定**（`val allowUnsignedRelease` 的整  │
// │      条右值逐字相等），管不到守卫**体内**别处新写的读取。                            │
// │    缝在中间：在守卫体里、逃生门分支**之前**插一句                                    │
// │    `if (System.getenv("POLARIS_XXX") == "true") return@Action`，绑定一字未动、        │
// │    枚举表里没有这个名字 ⇒ 两侧同时全绿，而这是一个从环境继承、永久生效的逃生门。      │
// │    实测（终止轮 R3）：源码侧 12 项全绿，本裁判零失败 rc=0。                          │
// │    **这一形今天没有门。** 不再加第三道门去堵它 —— 判据表的边界就写在这里。            │
// └───────────────────────────────────────────────────────────────────────────────┘
//
// ┌── 当前执行期验证归属（按本树 android.yml 的实际 job）─────────────────────────
// │ ①②③ 由 release_check 和 release-apk 构建真实 release APK 后，运行             │
// │ assert-r8-evidence.mjs 校验 configuration.txt / seeds.txt，并运行 verify-apk.mjs │
// │ 检查本次最终 APK。release_check 在 publish_release != true 时执行；               │
// │ release-apk 在 publish_release 为真时执行。两条腿均保留真实产物检查。              │
// │ release_check 使用显式命令行 unsigned 开关，其结果不能证明签名有效性。             │
// │                                                                              │
// │ ④ 正式 release-apk 从所选 keystore/alias 导出公钥证书摘要，随后用 apksigner        │
// │ verify --verbose --print-certs 检查每份发布 APK，核对实际签名证书摘要。              │
// │ 本配置期裁判仍只观察签名容器；实际 APK 的签名接线与有效性由上述产物步骤验证。         │
// │                                                                              │
// │ ⑤ 新环境变量名绕过守卫仍超出本裁判枚举范围，见上方能力边界。                       │
// └───────────────────────────────────────────────────────────────────────────────
//
// 2026-09-06 的历史审计曾记录 release-smoke 默认关闭、无自动调用方和无 apksigner。
// 那是当时源码/运行观察，已不适用于本树当前维护说明。上面描述当前源码中的接线，
// 不据此推断某次远端运行已经发生或通过；实际运行结论必须引用对应 run 的日志/收据。
//
// 本仓成文口径：**一条不受判据管辖的声明，比没有这句话更坏。** 上面这张表就是按这条写的：
// 能补的补进 ①–⑩，补不了的指名归属，并区分源码接线与实际运行证据。
//
//
// ── 本机怎么跑（与 CI 逐字同一条命令）──────────────────────────────────────────
//
//   JAVA_HOME=<JDK 21> ANDROID_HOME=<Android SDK> node scripts/gate-android-release-behavior.mjs
//
// 判据要 Android SDK，故 `ci.yml` 的 Rust 各 job（lint / cross / test）跑不了它（那里只有 Rust 工具链）。
// 它挂在 `.github/workflows/android.yml` 的 release_check 与 release-apk 两条腿，
// 都在 SDK/NDK 与生成的规则文件准备好后执行，验证配置期行为；后续真实 APK/R8/签名验证
// 由各 job 的产物级步骤负责。当前源码接线不等于某次 CI 已运行或已通过。
//
// 2026-09-06 的只读 API 观察曾记录默认分支缺少 android.yml；这是历史观察，
// 不能用来判断当前远端分支或执行状态。需要运行证据时应检查目标 run，而不是沿用旧断言。
//
// ── 硬要求 ──────────────────────────────────────────────────────────────────────
//
//   · gradle 跑不起来（缺 JDK / 缺 SDK / 缺生成物 / 超时）⇒ **抛并点名**，不许 skip、
//     不许当作通过。「没跑」与「跑过且通过」必须可分辨。
//   · 一律 offline、禁 configuration-cache；本地 no-daemon，CI 显式选择本批独立 daemon，退出必 stop。
//   · 禁 kill/pkill。
//   · 每条断言取材前先自检取材面非空；rc 一律取子进程自己的退出状态，不经管道。
//
// ── 裁判自己也要有门（2026-09-05 收官轮 A1；2026-11-26 迁 node 后按同一组变异复测）────────
//
// 实测（bash 版）：把 `bad()` 里的失败计数删掉 ⇒ 裁判先打印 `✗ minifyArm64ReleaseWithR8 命中 0 次`，
// 紧接着打印「全部行为断言通过」并 **rc=0**。接管四条判据的东西自己是空的。今天三层守它：
//
//   · **失败计数对账**：每条失败打印到 stdout、抄进 `transcript` 并计入 `failed`。
//     收尾时 `transcript` 中的 `✗` 行数必须等于 `failed`，不等一律 rc=2（裁判坏了）。
//   · **哨兵自检**：正式判据之前，先用 `POLARIS_GATE_SELFCHECK=fail` 把自己再跑一遍，
//     那一轮**只有一条必然失败的断言**。它的 rc 必须非零、必须打出那条 `✗`。
//     反向对照是 `POLARIS_GATE_SELFCHECK=pass`（只有一条必然通过的断言）：rc 必须为 0。
//     两轮都不碰 gradle、不碰工作树，隔离运行。
//     `bad()` 整个被掏空时，`fail` 那轮会 rc=0 ⇒ 当场 die。
//   · **出口盖章**（2026-09-06 终止轮 R1）：上面两层都要 `finish()` **跑起来**才说话。
//     删掉末尾那一句 `finish()`（node 从末尾自然落地，rc=0）、或在 `selfCheck()` 之后插一句
//     `process.exit(0)` —— 两条都会让 rc=0 而判据一条都没定罪。今天 rc 只许从 `die()` /
//     `finish()` 两个出口出来，两者退出前给 `sealed` 盖章，`process.on('exit')` 上验章：
//     没盖章一律改判 rc=2。未捕获的异常（node 默认 rc=1，与「判据红了」同码）也走这条，改判 rc=2。
//     🔴 它守不住的：把 `process.on('exit', …)` 本身删掉。那是把门拆了，
//     不是从门缝里溜过去 —— 本裁判的自守到此为止，再往上一层今天没有门。
//
// ── 临时状态：三条断言必须现场造出「另一种世界」，用完还原 ────────────────────
//
//   ③ 要往 `gen/android/gradle.properties` 写一行；⑧ 要让一份 `.pro` 暂时不在；
//   ⑨ 要往 `app/` 放一份没登记的 `.pro`。
//   三处都是备份 → 改 → 跑 → 还原 → 逐字节比对，并挂在 `process.on('exit')`（含中断路径：
//   SIGINT / SIGTERM 让正在跑的子进程先结束，再按 130 / 143 退出并走同一段还原）。
//   还原不到位时本脚本自己会红（退出时的比对与「探针文件不许留下」那条），而且 `cargo test`
//   那条「gradle.properties 里不许有 polarisAllowUnsigned」的断言也会红 —— 两层都能自曝。
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import {
  accessSync, appendFileSync, closeSync, constants as fsConstants, copyFileSync, existsSync, mkdirSync,
  mkdtempSync, openSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { constants, homedir } from 'node:os';
import { basename, delimiter, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT = fileURLToPath(import.meta.url);
const REPO = resolve(dirname(SCRIPT), '..');
const GRADLE_DIR = join(REPO, 'src-tauri/gen/android');
const PROPS = join(GRADLE_DIR, 'gradle.properties');
// ⑧ 拿它做「缺一份规则文件」的实验对象：三份里它是生成物（gitignored），
// 且不是本仓入库的那份 —— 实验失败时不会碰到入库内容。
const SACRIFICIAL_RULE = join(GRADLE_DIR, 'app/proguard-tauri.pro');
// ⑨ 「多一份没登记的规则文件」的实验对象。名字带双下划线前后缀，且只在本脚本里出现，
// 不与任何真规则文件同名；退出时一定被删掉。
const PROBE_RULE = join(GRADLE_DIR, 'app/__gate_probe__.pro');
const RULES = [
  'app/proguard-rules.pro',
  'app/proguard-tauri.pro',
  'app/src/main/java/com/polaris2/app/generated/proguard-wry.pro',
];

const WORK = mkdtempSync('/var/tmp/polaris-release-oracle.');
const PROPS_BAK = join(WORK, 'gradle.properties.bak');
const RULE_BAK = join(WORK, 'proguard-tauri.pro.bak');
const TRANSCRIPT = join(WORK, 'transcript');
writeFileSync(TRANSCRIPT, '');

const SELFCHECK = process.env.POLARIS_GATE_SELFCHECK ?? '';

// 前提不成立（工具链缺席、取材面为空、裁判自身对账不平）一律走这里：rc=2，与「判据红了」的
// rc=1 刻意分开。「没跑」与「跑过且通过」必须可分辨 —— 本裁判在任何前提不成立时都不当作通过。
function die(message) {
  sealed = 'die';
  console.error(`::error::gate-android-release-behavior: ${message}`);
  console.error();
  console.error('rc=2 表示**前提不成立**（跑不起来 / 裁判自身坏了），不是判据失败（那是 rc=1）。');
  process.exit(2);
}

const same = (a, b) => {
  try {
    return readFileSync(a).equals(readFileSync(b));
  } catch {
    return false;
  }
};

function restore() {
  // 幂等：两份备份都在时无条件写回（改没改过都写，省得判断状态）。每一步单独兜住，
  // 一步失败不挡后面的还原；失败由退出时的逐字节比对判红。
  const steps = [
    () => existsSync(PROPS_BAK) && copyFileSync(PROPS_BAK, PROPS),
    () => existsSync(RULE_BAK) && copyFileSync(RULE_BAK, SACRIFICIAL_RULE),
    // ⑨ 的探针文件从不属于工作树：中断路径上也必须消失，否则它会永久改掉 R8 的规则集。
    () => fixturesOwned && rmSync(PROBE_RULE, { force: true }),
  ];
  for (const step of steps) {
    try {
      step();
    } catch (error) {
      console.error(error.message);
    }
  }
}

// ── 出口盖章：「本裁判跑完了」这件事本身必须可判（2026-09-06 终止轮 R1）────────
//
// 失败计数对账（A1）要 finish() **跑起来**才说话 —— 守不住「根本没走到 finish」：
// 删掉末尾那一句 `finish()` ⇒ 脚本从末尾自然落地，rc=0；在 `selfCheck()` 之后插一句
// `process.exit(0)` ⇒ gradle 一次都不跑，rc=0。判据一条都没定罪，而 rc 与「全部通过」相同。
//
// 今天把 rc 的**产生**收到两个出口：`die()`（前提不成立，rc=2）与 `finish()`（判据结论，
// rc=0/1/2）。两者都在这里盖一个章，退出时验章：没盖章就走到退出 ⇒ 退出码是**别的东西**
// 给的（含未捕获异常的 rc=1），一律改判 rc=2（裁判坏了，不是判据红了 —— 与 A1 的对账不平同一档）。
let fixturesOwned = false;
let daemonActive = false;
let invocations = 0;
let sealed = '';
let interrupted = 0;
process.on('exit', (code) => {
  let rc = code;
  let cleanupFailed = false;
  restore();
  if (existsSync(PROPS_BAK) && !same(PROPS_BAK, PROPS)) cleanupFailed = true;
  if (existsSync(RULE_BAK) && !same(RULE_BAK, SACRIFICIAL_RULE)) cleanupFailed = true;
  if (fixturesOwned && existsSync(PROBE_RULE)) cleanupFailed = true;
  if (daemonActive) {
    // This user home owns only this invocation's daemon registry. Never stop another build's daemon.
    const log = openSync(join(WORK, 'daemon-stop.log'), 'w');
    const stop = spawnSync('./gradlew', ['--offline', '--stop'], { cwd: GRADLE_DIR, stdio: ['ignore', log, log] });
    closeSync(log);
    if (stop.status !== 0) cleanupFailed = true;
  }
  if (sealed === '') {
    console.error(`::error::gate-android-release-behavior: 裁判没跑完（rc=${rc}）`);
    rc = 2;
  }
  if (cleanupFailed) {
    console.error(`::error::fixture restore or isolated daemon stop failed; evidence: ${WORK}`);
    // A cleanup failure may turn success red, but must never overwrite the original failure.
    if (rc === 0) rc = 2;
  }
  if (rc === 0) rmSync(WORK, { recursive: true, force: true });
  process.exitCode = rc;
});
// SIGINT / SIGTERM：正在跑的前台子进程先跑完（与 bash trap 的时机相同），随后按 130 / 143 退出，
// 退出时照常还原；下一个子进程启动前也先检查一次，不再开新的一轮。
process.on('SIGINT', () => { interrupted ||= 130; });
process.on('SIGTERM', () => { interrupted ||= 143; });
const stopIfInterrupted = () => {
  if (interrupted) {
    sealed = 'signal';
    process.exit(interrupted);
  }
};

// 子进程 stdout/stderr 合并落盘；rc 取它自己的退出状态（被信号终止按 shell 惯例记 128+N）。
async function run(command, args, options, log) {
  await new Promise(setImmediate);
  stopIfInterrupted();
  const fd = openSync(log, 'w');
  const child = spawn(command, args, { ...options, stdio: ['inherit', fd, fd] });
  closeSync(fd);
  const [status, signal] = await once(child, 'exit');
  stopIfInterrupted();
  return status ?? 128 + constants.signals[signal];
}

let failed = 0;
// 每一行判据结论同时走两条通道：stdout（给人看）与 transcript（给对账看）。
const emit = (line) => {
  console.log(line);
  appendFileSync(TRANSCRIPT, `${line}\n`);
};
const ok = (label) => emit(`  ✓ ${label}`);
// 压平失败信息里的换行，使每条失败在 transcript 中只占一行。
const bad = (label) => {
  emit(`  ✗ ${label.replaceAll('\n', ' ⏎ ')}`);
  failed += 1;
};

// ── 收尾：先给裁判自己对账，再给判据结论定 rc ────────────────────────────────
//
// 顺序是刻意的：「裁判坏了」必须比「判据红了」先说话，也必须用不同的 rc（2 vs 1）——
// 两者混在一起的话，一个坏掉的裁判可以用一条真失败把自己藏起来。
function finish() {
  sealed = 'finish';
  const recorded = readFileSync(TRANSCRIPT, 'utf8').split('\n').filter((line) => line.startsWith('  ✗ ')).length;

  if (recorded !== failed) {
    console.error();
    console.error('::error::gate-android-release-behavior: **裁判自身坏了**（失败计数对账不平）：');
    console.error(`  transcript 记录的 ✗ ：${recorded} 条`);
    console.error(`  计数器 failed：${failed}`);
    console.error();
    console.error('这不是判据红了，是判据的裁决**带不动退出码** —— rc=2 与判据失败的 rc=1 刻意分开。');
    if (SELFCHECK !== '') rmSync(WORK, { recursive: true, force: true });
    process.exit(2);
  }

  console.log();
  // 自检轮不碰 gradle，也就没有 gradle 日志可留 —— 报一个不存在的路径同样是不实之词。
  if (SELFCHECK !== '') {
    rmSync(WORK, { recursive: true, force: true });
    if (failed === 0) {
      console.log('gate-android-release-behavior: 全部行为断言通过（裁判自检轮，未跑 gradle）');
      process.exit(0);
    }
    console.log(`::error::gate-android-release-behavior: ${failed} 条行为断言失败（裁判自检轮）`);
    process.exit(1);
  }
  if (failed === 0) {
    console.log(`gate-android-release-behavior: 全部行为断言通过（日志在 ${WORK}）`);
    process.exit(0);
  }
  console.log(`::error::gate-android-release-behavior: ${failed} 条行为断言失败；gradle 日志留在 ${WORK}`);
  process.exit(1);
}

// ══ 裁判自检：正式判据之前，先证明「✗ 一定带得动 rc」════════════════════════════
//
// 隔离模式：不碰 gradle、不碰工作树，只跑一条必然失败（或必然通过）的哨兵断言，
// 走的是与真判据**同一段** bad()/ok()/finish() 代码。
const SELFCHECK_FAIL_LABEL = '哨兵断言：这一条必然失败（裁判自检用，不是真判据）';
const SELFCHECK_PASS_LABEL = '哨兵断言：这一条必然通过（裁判自检的反向对照）';
if (SELFCHECK === 'fail') {
  bad(SELFCHECK_FAIL_LABEL);
  finish();
} else if (SELFCHECK === 'pass') {
  ok(SELFCHECK_PASS_LABEL);
  finish();
} else if (SELFCHECK !== '') {
  die(`POLARIS_GATE_SELFCHECK 只认 fail / pass，实得：${SELFCHECK}`);
}

async function selfCheck() {
  const selfRun = (mode) => run(process.execPath, [SCRIPT],
    { env: { ...process.env, POLARIS_GATE_SELFCHECK: mode } }, join(WORK, `selfcheck-${mode}.log`));
  const failLog = join(WORK, 'selfcheck-fail.log');
  const rc = await selfRun('fail');
  if (rc === 0) {
    die('裁判自检（fail 轮）rc=0 —— 一条必然失败的断言没能把退出码带成非零。'
      + `失败通道与 rc 脱节 ⇒ 本裁判此刻接管的判据全部没有信息量。日志：${failLog}`);
  }
  if (rc === 2) {
    const lines = readFileSync(failLog, 'utf8').split('\n').filter((line) => /transcript 记录的|计数器/.test(line));
    die(`裁判自检（fail 轮）rc=2 —— 裁判自身坏了。日志：${failLog}。原文：${lines.map((line) => `${line} `).join('')}`);
  }
  if (rc !== 1) die(`裁判自检（fail 轮）rc=${rc}，期望 1。日志：${failLog}`);
  const failText = readFileSync(failLog, 'utf8');
  if (!failText.includes(`✗ ${SELFCHECK_FAIL_LABEL}`)) {
    die('裁判自检（fail 轮）rc 对了，但那条 ✗ 一个字都没打出来 —— 失败通道与退出码脱节了。');
  }
  if (failText.includes('全部行为断言通过')) {
    die('裁判自检（fail 轮）在失败之后仍然打印了「全部行为断言通过」—— 收尾逻辑与计数脱节。');
  }

  // 反向对照：没有它，上面三条会被一个「永远 rc≠0」的坏收尾满足。
  const passLog = join(WORK, 'selfcheck-pass.log');
  const passRc = await selfRun('pass');
  if (passRc !== 0) {
    die(`裁判自检（pass 轮）rc=${passRc}，期望 0。只有一条必然通过的断言却判成了失败 ⇒ `
      + `本裁判恒红，红也没有信息量。日志：${passLog}`);
  }
  if (!readFileSync(passLog, 'utf8').includes('全部行为断言通过')) {
    die('裁判自检（pass 轮）rc=0 却没打印通过行 —— 收尾逻辑没走到。');
  }
  console.log('══ ⓪ 裁判自检：失败通道带得动 rc（fail 轮 rc=1）、通过通道也走得通（pass 轮 rc=0）══');
}
await selfCheck();

// ══ 前置：工具链必须在场，缺了当场死 ══════════════════════════════════════════
const executable = (path) => {
  try {
    accessSync(path, fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
};
if (!executable(join(GRADLE_DIR, 'gradlew'))) die(`找不到 ${GRADLE_DIR}/gradlew —— Android 工程没铺出来。`);
const javaHome = process.env.JAVA_HOME;
if (javaHome) {
  if (!executable(join(javaHome, 'bin/java'))) die(`JAVA_HOME=${javaHome} 下没有 bin/java。`);
  process.env.PATH = join(javaHome, 'bin') + delimiter + process.env.PATH;
}
if (spawnSync('which', ['java'], { stdio: 'ignore' }).status !== 0) {
  die('PATH 上没有 java，且 JAVA_HOME 未指向一个 JDK。AGP 8.11 要 JDK 17–21。');
}
const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
if (!sdk || !statSync(sdk, { throwIfNoEntry: false })?.isDirectory()) {
  die('ANDROID_HOME / ANDROID_SDK_ROOT 没指向一个存在的 Android SDK。');
}
process.env.ANDROID_HOME = sdk;
// 三份 `.pro` 里有两份是 `tauri android build` 的生成物：不在场时下面 ⑦⑧ 的取材面是空的，
// 「三份都在」会退化成对空清单恒真 —— 故在这里当场死，而不是让断言去撞。
for (const rel of RULES) {
  if (!(statSync(join(GRADLE_DIR, rel), { throwIfNoEntry: false })?.size > 0)) {
    die(`${rel} 不在场或为 0 字节 —— 先跑一次 \`tauri android build --debug --apk --target aarch64\` 把生成物铺出来。`);
  }
}
// ⑨ 的探针文件必须从「不在场」出发：上一轮中断留下来的话，⑦ 的集合恰等会红在一个假原因上。
if (existsSync(PROBE_RULE)) die(`${PROBE_RULE} 已经在场 —— 上一轮的探针没清干净，先删掉它再跑。`);

fixturesOwned = true;
try {
  copyFileSync(PROPS, PROPS_BAK);
} catch {
  die('备份 gradle.properties 失败');
}
try {
  copyFileSync(SACRIFICIAL_RULE, RULE_BAK);
} catch {
  die('备份 proguard-tauri.pro 失败');
}

// CI opt-in only. Share dependency/distribution bytes, not daemon registries or configuration cache.
// Keep the host's Gradle worker/JVM policy; do not introduce an unbounded parallelism override.
let daemonFlags = ['--no-daemon'];
const ciDaemon = process.env.POLARIS_GATE_CI_DAEMON || '0';
if (ciDaemon === '1') {
  if (process.env.CI !== 'true' || process.env.GITHUB_ACTIONS !== 'true') die('daemon reuse is CI-only');
  let originalGradleHome;
  try {
    originalGradleHome = realpathSync(process.env.GRADLE_USER_HOME || join(homedir(), '.gradle'));
  } catch {
    die('Gradle user home missing');
  }
  const gradleHome = join(WORK, 'gradle-home');
  process.env.GRADLE_USER_HOME = gradleHome;
  try {
    mkdirSync(gradleHome, { recursive: true });
  } catch {
    die('cannot create isolated Gradle user home');
  }
  for (const part of ['caches', 'wrapper', 'jdks']) {
    if (statSync(join(originalGradleHome, part), { throwIfNoEntry: false })?.isDirectory()) {
      try {
        symlinkSync(join(originalGradleHome, part), join(gradleHome, part));
      } catch {
        die(`cannot reuse Gradle ${part}`);
      }
    }
  }
  if (statSync(join(originalGradleHome, 'gradle.properties'), { throwIfNoEntry: false })?.isFile()) {
    try {
      copyFileSync(join(originalGradleHome, 'gradle.properties'), join(gradleHome, 'gradle.properties'));
    } catch {
      die('cannot preserve Gradle host policy');
    }
  }
  daemonFlags = ['--daemon'];
  daemonActive = true;
} else if (ciDaemon !== '0') {
  die('POLARIS_GATE_CI_DAEMON must be 0 or 1');
}

// A per-invocation nonce proves configuration actually ran with this process's environment.
// Facts also need an execution marker; UP-TO-DATE / FROM-CACHE / SKIPPED is not evidence.
const ORACLE_INIT = join(WORK, 'oracle.init.gradle');
writeFileSync(ORACLE_INIT, `gradle.settingsEvaluated {
    println("POLARIS_ORACLE_CONFIG\\t" + System.getenv("POLARIS_ORACLE_INVOCATION"))
}
gradle.beforeProject { project ->
    if (project.path == ':app') {
        project.tasks.matching { it.name == 'printPolarisReleaseFacts' }.configureEach {
            doLast {
                println("POLARIS_ORACLE_EXEC\\t" + System.getenv("POLARIS_ORACLE_INVOCATION"))
            }
        }
    }
}
`);
const GRADLE_COMMON = ['./gradlew', '--offline', ...daemonFlags, '--no-configuration-cache', '--init-script', ORACLE_INIT];
const lines = (log) => readFileSync(log, 'utf8').split('\n');

// ══ 跑一轮 gradle；rc 直接取，日志落盘供断言取材 ═══════════════════════════════
//
// 每一轮都从**同一个干净环境**出发：把四个签名环境变量与全部 ORG_GRADLE_PROJECT_* 摘掉，
// 免得跑这条门的人自己机器上的凭据把「零凭据」这个前提悄悄改掉；场景需要的变量再显式加回。
let RC = 0;
async function gradleRun(log, command, extraEnv = {}) {
  invocations += 1;
  const nonce = `${basename(WORK)}-${invocations}`;
  const env = { ...process.env, POLARIS_ORACLE_INVOCATION: nonce };
  for (const name of ['POLARIS_KEYSTORE_FILE', 'POLARIS_KEYSTORE_PASSWORD', 'POLARIS_KEY_PASSWORD',
    'POLARIS_KEY_ALIAS', 'POLARIS_ALLOW_UNSIGNED', 'ORG_GRADLE_PROJECT_polarisAllowUnsigned']) delete env[name];
  RC = await run(command[0], command.slice(1), { cwd: GRADLE_DIR, env: { ...env, ...extraEnv } }, log);
  const output = lines(log);
  if (!output.includes(`POLARIS_ORACLE_CONFIG\t${nonce}`)) die(`fresh configuration missing: ${log}`);
  if (command.includes(':app:printPolarisReleaseFacts') && !output.includes(`POLARIS_ORACLE_EXEC\t${nonce}`)) {
    die(`facts task did not execute: ${log}`);
  }
}

// ⑩ 专用：凭据**齐全**那条分支。密钥库用的是一个现造的普通文件（不是密钥，也不需要是）——
// 配置期从不读它的内容，只问「在不在、读不读得动、是不是本工程认的那一份」。
// 口令/别名是三个显式的占位串，写在这里就是要让人一眼看出它们不是秘密。
const FAKE_KEYSTORE = join(WORK, 'not-a-real-keystore.bin');
const FAKE_SECRET = 'gate-probe-placeholder-not-a-secret';
const signed = (keystore) => ({
  POLARIS_KEYSTORE_FILE: keystore,
  POLARIS_KEYSTORE_PASSWORD: FAKE_SECRET,
  POLARIS_KEY_PASSWORD: FAKE_SECRET,
  POLARIS_KEY_ALIAS: FAKE_SECRET,
});

// 取材前自检：日志为空 = gradle 根本没说话，下面全部断言都在空面上作证。
function countIn(log, needle) {
  if (!(statSync(log, { throwIfNoEntry: false })?.size > 0)) die(`取材面为空：${log} 一个字节都没有 —— gradle 没跑起来。`);
  return lines(log).filter((line) => line.includes(needle)).length;
}
function expectRc(label, want) {
  if (want === 0) {
    if (RC === 0) ok(`${label}（rc=${RC}）`);
    else bad(`${label}：期望 rc=0，实得 rc=${RC}`);
  } else if (RC !== 0) {
    ok(`${label}（rc=${RC}）`);
  } else {
    bad(`${label}：期望 rc≠0，实得 rc=${RC}`);
  }
}
function expectHas(label, log, needle) {
  if (countIn(log, needle) > 0) ok(label);
  else bad(`${label}：${log} 里找不到 \`${needle}\``);
}
function expectAbsent(label, log, needle) {
  const n = countIn(log, needle);
  if (n === 0) ok(label);
  else bad(`${label}：${log} 里出现了 \`${needle}\`（${n} 次），本不该有`);
}
function expectCount(label, log, needle, want) {
  const n = countIn(log, needle);
  if (n === want) ok(`${label}（${n} 次）`);
  else bad(`${label}：\`${needle}\` 命中 ${n} 次，期望 ${want} 次`);
}

const DRY = [...GRADLE_COMMON, ':app:assembleArm64Release', '--dry-run'];
const DRY_DEBUG = [...GRADLE_COMMON, ':app:assembleArm64Debug', '--dry-run'];
const FACTS = [...GRADLE_COMMON, ':app:printPolarisReleaseFacts'];
const HATCH = '-PpolarisAllowUnsigned=true';
const log = (name) => join(WORK, `${name}.log`);

// 三条“指名到点”的针。它们是失败信息的原文片段，不是变量名 —— 守的是「说了什么」。
const MSG_REFUSE = 'Android release 签名凭据缺失';
const MSG_HATCH = '已生效，本次 release 产物';
const MSG_RULES = 'release 的 R8 规则文件缺席';
const MSG_EXTRA = 'release 的 R8 规则集里有没登记的规则文件';
const MSG_STORE = 'storeFile 不是本工程认的那一份';

// 事实行的前缀（含制表符）：`POLARIS_FACT<TAB>proguardFile<TAB><路径><TAB><字节数>`。
const FACT_FILE_PREFIX = 'POLARIS_FACT\tproguardFile\t';

console.log('══ ① 零凭据 + 不带逃生门 ⇒ 必须硬失败并点名 ══');
await gradleRun(log('1-bare'), DRY);
expectRc('零凭据跑 release 被拒绝', 'nonzero');
expectHas('失败信息点名了「拒绝静默产出未签名包」', log('1-bare'), MSG_REFUSE);
for (const v of ['POLARIS_KEYSTORE_PASSWORD', 'POLARIS_KEY_PASSWORD', 'POLARIS_KEY_ALIAS']) {
  expectHas(`失败信息逐条点名 ${v}`, log('1-bare'), v);
}
// 反向对照：这一轮不该出现逃生门的警告 —— 证明上面那几条不是被一段「什么都印」的输出喂绿的。
expectAbsent('本轮没有逃生门警告（反向对照）', log('1-bare'), MSG_HATCH);

console.log('══ ② 命令行逃生门 ⇒ 放行、出警告、R8 仍在任务图里 ══');
await gradleRun(log('2-hatch'), [...DRY, HATCH]);
expectRc('带 -PpolarisAllowUnsigned=true 放行', 0);
expectHas('逃生门打印了未签名警告', log('2-hatch'), MSG_HATCH);
expectAbsent('放行时不再报「凭据缺失」（反向对照）', log('2-hatch'), MSG_REFUSE);

console.log('══ ⑤ release 任务图里 R8 恰好跑一次 ══');
expectCount('minifyArm64ReleaseWithR8 在任务图里', log('2-hatch'), 'minifyArm64ReleaseWithR8', 1);
expectHas('任务图确实是 release 那条（取材自检）', log('2-hatch'), ':app:assembleArm64Release');

console.log('══ ⑤b ARMv7 与 universal 同走完整 release/R8 任务图 ══');
await gradleRun(log('5b-arm-flavors'), [...GRADLE_COMMON,
  ':app:assembleArmRelease', ':app:assembleUniversalRelease', '--dry-run',
  HATCH, '-PtargetList=aarch64,armv7', '-ParchList=arm64,arm', '-PabiList=arm64-v8a,armeabi-v7a']);
expectRc('ARMv7/universal release 任务图可解析', 0);
expectCount('ARMv7 的 R8 在图中恰好一次', log('5b-arm-flavors'), 'minifyArmReleaseWithR8', 1);
expectCount('universal 的 R8 在图中恰好一次', log('5b-arm-flavors'), 'minifyUniversalReleaseWithR8', 1);

console.log('══ ②b 命令行来源的正向对照：这条路确实把开关送到了逃生门手里 ══');
await gradleRun(log('2b-facts'), [...FACTS, HATCH]);
expectRc('事实任务跑得动', 0);
expectHas('命令行属性表里有它', log('2b-facts'), 'hatchCmdlineKeys\tpolarisAllowUnsigned');
expectHas('逃生门确实被打开', log('2b-facts'), 'hatchOpen\ttrue');

console.log('══ ③ gradle.properties 写一行 ⇒ 仍然必须硬失败 ══');
appendFileSync(PROPS, '\npolarisAllowUnsigned=true\n');
await gradleRun(log('3-props'), DRY);
expectRc('gradle.properties 打不开逃生门', 'nonzero');
expectHas('仍然报「凭据缺失」', log('3-props'), MSG_REFUSE);
expectAbsent('没有出逃生门警告', log('3-props'), MSG_HATCH);
// 正向对照：证明这一行**确实被 gradle 读到了**（否则 rc≠0 只是「什么都没发生」）。
await gradleRun(log('3b-facts'), FACTS);
expectHas('gradle 确实读到了这一行（正向对照）', log('3b-facts'), 'hatchAllPropertyKeys\tpolarisAllowUnsigned');
expectHas('但它没进命令行属性表', log('3b-facts'), 'hatchCmdlineKeys\t<none>');
expectHas('逃生门保持关闭', log('3b-facts'), 'hatchOpen\tfalse');
copyFileSync(PROPS_BAK, PROPS);

console.log('══ ④ ORG_GRADLE_PROJECT_ 环境变量 ⇒ 仍然必须硬失败 ══');
const envHatch = { ORG_GRADLE_PROJECT_polarisAllowUnsigned: 'true' };
await gradleRun(log('4-env'), DRY, envHatch);
expectRc('ORG_GRADLE_PROJECT_ 打不开逃生门', 'nonzero');
expectHas('仍然报「凭据缺失」', log('4-env'), MSG_REFUSE);
expectAbsent('没有出逃生门警告', log('4-env'), MSG_HATCH);
await gradleRun(log('4b-facts'), FACTS, envHatch);
expectRc('事实任务跑得动', 0);
expectHas('gradle 确实收到了这个属性（正向对照）', log('4b-facts'), 'hatchAllPropertyKeys\tpolarisAllowUnsigned');
expectHas('逃生门保持关闭', log('4b-facts'), 'hatchOpen\tfalse');

// Transition negative control: ORG_GRADLE_PROJECT_* must not leak to the next request.
await gradleRun(log('4-reset-facts'), FACTS);
expectRc('环境属性撤销后事实任务仍执行', 0);
expectHas('环境属性未串到下一场景', log('4-reset-facts'), 'hatchAllPropertyKeys\t<none>');
expectHas('逃生门保持关闭', log('4-reset-facts'), 'hatchOpen\tfalse');

console.log('══ ④b 别的环境变量也不是逃生门的来源 ══');
// 验收员的 M01 变异就是给读取点接一句 `|| System.getenv("POLARIS_ALLOW_UNSIGNED") == "true"`。
// 这条断言只覆盖**这一个名字**。上一版这里写着「新变量名由源码侧那条形状断言守，两条一起
// 就封住了这一形」——**那句话过宽**（2026-09-06 终止轮 R3）：源码侧钉死的是
// `val allowUnsignedRelease` **那一条绑定**的整条右值，管不到守卫**体内**别处新写的读取。
// 两侧各守一头，中间那条缝（在逃生门分支之前插一句
// `if (System.getenv("POLARIS_XXX") == "true") return@Action`）**今天没有门**，
// 实测两侧同时全绿。登记在头注天花板表的「看不见 ⑤」，本处不复写。
await gradleRun(log('4c-env'), DRY, { POLARIS_ALLOW_UNSIGNED: 'true' });
expectRc('POLARIS_ALLOW_UNSIGNED 环境变量打不开逃生门', 'nonzero');
expectHas('仍然报「凭据缺失」', log('4c-env'), MSG_REFUSE);

console.log('══ ⑥ debug 侧不受影响：跑得通，且任务图里没有 R8 ══');
await gradleRun(log('6-debug'), DRY_DEBUG);
expectRc('debug 侧照常放行', 0);
expectHas('任务图确实是 debug 那条（取材自检）', log('6-debug'), ':app:assembleArm64Debug');
expectCount('debug 任务图里没有 R8', log('6-debug'), 'WithR8', 0);
expectAbsent('debug 侧不该被签名守卫拦', log('6-debug'), MSG_REFUSE);

console.log('══ ⑦ R8 实际吃进的规则集：集合恰等（多一份少一份都红）、且逐份非空 ══');
await gradleRun(log('7-facts'), FACTS);
expectRc('事实任务跑得动', 0);
expectHas('逃生门默认关闭', log('7-facts'), 'hatchOpen\tfalse');
expectHas('命令行属性表为空（默认态自检）', log('7-facts'), 'hatchCmdlineKeys\t<none>');
expectHas('release 资源裁剪已开启', log('7-facts'), 'POLARIS_FACT\treleaseShrinkResources\ttrue');
for (const variant of ['arm64Release', 'armRelease', 'universalRelease']) {
  expectHas(`${variant} 排除 x86/x86_64 原生库`, log('7-facts'),
    `POLARIS_FACT\treleaseJniExcludes\t${variant}\t**/x86/*.so,**/x86_64/*.so`);
}

// 把 AGP 手里那份清单原样取下来（每行一个绝对路径），下面按**集合**判，不按份数判。
//
// 上一版判的是 `listed >= 4`。那是 membership 不是闭合：往 app/ 放一份没人登记的
// `extra.pro`（内容 `-keep class ** { *; }` = 把整包混淆关掉），listed 变成 5，
// 「≥ 4」照样绿。规则集是集合，多一份与少一份是同一类事故。
const listed = lines(log('7-facts')).filter((line) => line.includes(FACT_FILE_PREFIX));
if (listed.length === 0) die('事实任务一行 proguardFile 都没打出来 —— 取材面是空的，⑦ 全部断言作废。');
const actualPaths = listed.map((line) => line.split('\t')[2] ?? '').sort();
const expectedPaths = RULES.map((rel) => join(GRADLE_DIR, rel)).sort();
// AGP 默认档的路径带着 AGP 版本号（`…/proguard-android-optimize.txt-8.11.0`），
// 不能写死；把它从实得集合里摘出来单独判，剩下的必须与登记的三份**逐字相等**。
const defaultHits = actualPaths.filter((path) => path.includes('proguard-android-optimize.txt')).length;
const actualOwn = actualPaths.filter((path) => !path.includes('proguard-android-optimize.txt'));
if (defaultHits === 1) {
  ok('AGP 默认档在清单里，且恰好一份');
} else {
  bad(`AGP 默认档在清单里出现 ${defaultHits} 次（应恰好 1 次）—— 0 次 = native 方法、@JavascriptInterface、enum values/valueOf 这批全局兜底规则整体缺席`);
}
if (actualOwn.join('\n') === expectedPaths.join('\n')) {
  ok('AGP 手里的本仓规则文件集合与登记的三份**恰等**');
} else {
  bad(`AGP 手里的本仓规则文件集合与登记的三份不等（期望 / 实得）：${expectedPaths.join(', ')} / ${actualOwn.join(', ')}`);
}
for (const rel of RULES) {
  const line = lines(log('7-facts')).filter((entry) => entry.includes(`${FACT_FILE_PREFIX}${join(GRADLE_DIR, rel)}\t`)).join('\n');
  if (line === '') {
    bad(`⑦ ${rel} 不在 AGP 装配好的 proguardFiles 清单里 —— 这份规则整体不生效，而构建照常绿`);
    continue;
  }
  const size = line.slice(line.lastIndexOf('\t') + 1);
  if (/^\d+$/.test(size) && Number(size) > 0) {
    ok(`⑦ ${rel} 进了 R8（${size} 字节）`);
  } else {
    bad(`⑦ ${rel} 在清单里但是空文件（${size} 字节）—— 规则文本一条都没有`);
  }
}

console.log('══ ⑦b R8 输入集的另外两处来源必须为空（defaultConfig / productFlavor）══');
//
// ⑦ 的「集合恰等」判的是 release **构建类型**那一份清单，而 AGP 是把
// `defaultConfig` + 选中的 `productFlavor` + `buildType` 三处**合并**后喂给 R8 的
// （AGP 口径；本轮没去 R8 的输入那一层实测，那要真跑一次 release 构建）。
// 实测（2026-09-06 终止轮 R2）：往 `defaultConfig` 上挂一份 `../__probe__.pro`
// —— 它在 app/ 的 fileTree 扫描面之外，守卫那条「多一份没登记的规则文件」也看不见它 ——
// 本裁判 ⑦ 零失败、rc=0，事实表里连它的影子都没有。取材面只有三分之一，恰等就名不副实。
//
// 今天把另外两处也摊开，并钉死为空：两处为空 ⇒ buildType 那一份**就是**本仓自有规则的全集，
// ⑦ 的恰等才等于「R8 到手的就是登记的那三份」。
// 🔴 这两条是**空集断言**，不是白名单：真要给 defaultConfig / flavor 挂规则，
//    这条会红，那时该做的是把登记面搬过去（连同 ⑦ 的期望集），不是把这条改宽。
expectHas('defaultConfig 上没挂 proguardFiles', log('7-facts'), 'POLARIS_FACT\tproguardFilesDefaultConfig\t<none>');
expectHas('五个 productFlavor 上都没挂 proguardFiles', log('7-facts'), 'POLARIS_FACT\tproguardFilesFlavors\t<none>');

console.log('══ ⑧ 少一份规则文件 ⇒ 必须点名那一份并硬失败 ══');
rmSync(SACRIFICIAL_RULE, { force: true });
await gradleRun(log('8-missing'), [...DRY, HATCH]);
expectRc('缺规则文件时拒绝构建', 'nonzero');
expectHas('失败信息说的是「规则文件缺席」', log('8-missing'), MSG_RULES);
expectHas('并点名缺的是哪一份', log('8-missing'), 'proguard-tauri.pro');
// 反向对照：这一轮带着逃生门，所以红的原因必须是规则文件，不是签名。
expectAbsent('红的原因不是签名（反向对照）', log('8-missing'), MSG_REFUSE);
copyFileSync(RULE_BAK, SACRIFICIAL_RULE);

console.log('══ ⑨ 多一份没登记的 .pro ⇒ 事实表必须跟着变，且构建必须点名硬失败 ══');
//
// 这一节同时是两条判据：
//   · **闭合**（收官轮 A9）：`-keep class ** { *; }` 一行就把整包混淆关掉，而「三份都在」
//     这条 membership 判定对它完全是瞎的。集合恰等在 ⑦，硬失败在这里。
//   · **事实任务读的是不是 AGP 那份清单**（收官轮 A2）：事实任务已经搬出被审判的
//     `build.gradle.kts`，但「它读的到底是 AGP 装配好的清单，还是一份写死的常量」
//     不能靠读代码来答。差分实验答：AGP 的扫描面变了，它打出来的清单必须跟着变。
//     打不出这一行 = 它读的是别的东西，⑦ 的全部结论随之作废。
writeFileSync(PROBE_RULE, '-keep class ** { *; }\n');
await gradleRun(log('9-facts'), FACTS);
expectRc('多一份规则文件时事实任务仍跑得动', 0);
expectHas('事实表跟着 AGP 的扫描面变了（证明它读的就是 AGP 装配好的清单，不是写死的常量）',
  log('9-facts'), `${FACT_FILE_PREFIX}${PROBE_RULE}\t`);
await gradleRun(log('9-dry'), [...DRY, HATCH]);
expectRc('多一份没登记的规则文件时拒绝构建', 'nonzero');
expectHas('失败信息说的是「有没登记的规则文件」', log('9-dry'), MSG_EXTRA);
expectHas('并点名多出来的是哪一份', log('9-dry'), '__gate_probe__.pro');
expectAbsent('红的原因不是签名（反向对照）', log('9-dry'), MSG_REFUSE);
expectAbsent('红的原因也不是缺规则文件（反向对照）', log('9-dry'), MSG_RULES);
rmSync(PROBE_RULE, { force: true });

console.log('══ ⑩ 凭据在场那条分支：storeFile 必须存在、可读、且就是本工程认的那一份 ══');
//
// 这条分支此前**零观测**：裁判每一轮都跑在零凭据下，`storeFile` 指向哪儿没有任何东西看一眼。
// 最坏的一形不是指向空气（AGP 执行期还会报一句 `Keystore file not set`），而是指向**另一份
// 存在的**密钥库：构建全绿、包也签得出来，只是签它的不是本工程的密钥。
//
// 🔴 这里用的不是密钥：`FAKE_KEYSTORE` 是现造的一个普通文件，口令/别名是三个写死的占位串。
//    配置期从不读密钥库的内容 —— 它只被 `isFile` / `canRead` / 路径比对碰到。
//    「这份密钥库是不是合法、口令对不对」不在本裁判射程内（见头注天花板表第 ④ 条）。
writeFileSync(FAKE_KEYSTORE, 'not a keystore; created by gate-android-release-behavior.mjs as a path probe\n');
await gradleRun(log('10-facts'), FACTS, signed(FAKE_KEYSTORE));
expectRc('凭据齐全时事实任务跑得动', 0);
expectHas('AGP 手里的 storeFile 就是 POLARIS_KEYSTORE_FILE 指的那一份，且存在、可读',
  log('10-facts'), `POLARIS_FACT\tsigningStoreFile\t${FAKE_KEYSTORE}\ttrue\ttrue`);
await gradleRun(log('10-dry'), DRY, signed(FAKE_KEYSTORE));
expectRc('凭据齐全时 release 放行', 0);
expectAbsent('凭据齐全时不再报「凭据缺失」', log('10-dry'), MSG_REFUSE);
expectAbsent('凭据齐全时不该走逃生门（反向对照：这一轮的绿不是逃生门给的）', log('10-dry'), MSG_HATCH);
expectAbsent('storeFile 与本工程认的那一份一致（没报错位）', log('10-dry'), MSG_STORE);

console.log('══ ⑩b 反向对照：密钥库不在盘上时，凭据齐不齐都必须硬失败并点名那条路径 ══');
// 没有这条，⑩ 的「存在且可读」可能是被一条恒真的判定喂绿的。
const MISSING_KEYSTORE = join(WORK, 'there-is-no-such-keystore.jks');
if (existsSync(MISSING_KEYSTORE)) die(`反向对照的前提被破坏：${MISSING_KEYSTORE} 居然在场。`);
await gradleRun(log('10c-dry'), DRY, signed(MISSING_KEYSTORE));
expectRc('密钥库不在盘上时拒绝构建', 'nonzero');
expectHas('失败信息点名了「密钥库文件不存在」', log('10c-dry'), '密钥库文件不存在');
expectHas('并点名是哪一条路径', log('10c-dry'), MISSING_KEYSTORE);
await gradleRun(log('10c-facts'), FACTS, signed(MISSING_KEYSTORE));
expectRc('事实任务仍跑得动', 0);
expectHas('此时 AGP 手里的 storeFile 是空的（空壳配置，正向对照）',
  log('10c-facts'), 'POLARIS_FACT\tsigningStoreFile\t<none>\tfalse\tfalse');

// Transition negative controls: signed -> completely absent credentials, on the same daemon.
await gradleRun(log('10-reset-dry'), DRY);
expectRc('撤销凭据后 release 再次拒绝', 'nonzero');
expectHas('签名环境未串到下一场景', log('10-reset-dry'), MSG_REFUSE);
expectAbsent('撤销凭据不是通过逃生门', log('10-reset-dry'), MSG_HATCH);
await gradleRun(log('10-reset-facts'), FACTS);
expectRc('撤销凭据后事实任务仍执行', 0);
expectHas('签名容器已恢复空壳', log('10-reset-facts'), 'POLARIS_FACT\tsigningStoreFile\t<none>\tfalse\tfalse');
expectHas('所有临时属性已清除', log('10-reset-facts'), 'hatchAllPropertyKeys\t<none>');
expectAbsent('额外规则文件未串到最后场景', log('10-reset-facts'), `${FACT_FILE_PREFIX}${PROBE_RULE}`);
const restoredRules = lines(log('10-reset-facts')).filter((line) => line.includes(FACT_FILE_PREFIX)).sort();
if (restoredRules.join('\n') === [...listed].sort().join('\n')) {
  ok('规则文件清单与字节数已恢复，未串到后续场景');
} else {
  bad('规则文件清单或字节数在变异后没有恢复');
}
if (invocations !== 21) die(`expected 18 original scenarios plus 3 transition controls; got ${invocations}`);

console.log('══ 还原自检 ══');
if (same(PROPS_BAK, PROPS)) ok('gradle.properties 已还原');
else bad('gradle.properties 没还原干净');
if (same(RULE_BAK, SACRIFICIAL_RULE)) ok('proguard-tauri.pro 已还原');
else bad('proguard-tauri.pro 没还原干净');
if (existsSync(PROBE_RULE)) bad(`⑨ 的探针文件还留在树上：${PROBE_RULE}`);
else ok('⑨ 的探针文件已清掉');

finish();
