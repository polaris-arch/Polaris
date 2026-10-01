#!/usr/bin/env node
// assert-r8-evidence.mjs —— release 冒烟腿的**产物级**判据本体：R8 到底吃进了什么、又保住了什么。
//
// ── 为什么是一份脚本而不是 workflow 里的一段内联 shell ────────────────────────────
//
// 内联那版有两个洞，都是 2026-09-05 验收实测出来的：
//
//   ① **针被本仓自己的注释污染**。R8 的 `-printconfiguration` 把每一份规则文件的**原文**
//      （含 `#` 注释）逐段抄进 `configuration.txt`。而本仓 `proguard-rules.pro` 的注释里
//      恰好写着 `libbox.aar 的 proguard.txt（go.** / io.nekohasekai.**）` —— 于是
//      「`io.nekohasekai` 在不在 configuration.txt 里」这条针**永远命中**，哪怕 libbox 的
//      consumer 规则一条都没进 R8。取材必须先剥注释，这是本仓记过多次的形态。
//   ② **`configuration.txt` 只证明规则文本到过 R8，不证明它匹配到了任何东西**。
//      一条 `-keep class org.rustls.platformverifier.NoSuchThing` 同样会原样出现在
//      configuration.txt 里，而 `CertificateVerifier` 此时一个 keep 都没有、被 R8 整包剪掉。
//      「保住了什么」的判据是 `seeds.txt`（R8 列出每条 keep 规则**实际命中**的类与成员），
//      那份文件此前已经在上传清单里，却一条断言都没有。
//
//   ③ **规则文本到过 R8 ≠ 它是从该来的那份文件来的**（2026-09-05 收官轮 A11 补）。
//      第 5 根针原本是 `com.polaris2.app.RustWebView`，登记的意思是「本批为 wry 那两个 JNI
//      方法补的 keep 块」。而 Tauri CLI 生成的 `proguard-wry.pro` 里本来就有一条
//      `-keep class com.polaris2.app.RustWebView {` —— 那根针**永远由别人喂绿**，
//      本批那个块整块删掉照样命中，实际零观测面。
//      今天每根针都带一个**期望来源**，判定面收窄到 R8 段落标记划出来的那一段。
//
// 判据落成脚本还有第三个理由：内联 shell 的判据没法单测。本文件的每一条都有
// `scripts/assert-r8-evidence.test.mjs` 的正反用例钉着，包括把上面①原样回放。
//
// ── 用法 ────────────────────────────────────────────────────────────────────────
//
//   node scripts/assert-r8-evidence.mjs <mapping 目录> [规则文件]
//
//   mapping 目录默认 src-tauri/gen/android/app/build/outputs/mapping/arm64Release
//   规则文件默认 src-tauri/gen/android/app/proguard-rules.pro（只用来做「注释确实被剥掉了」
//   的正向对照，不参与任何针的判定）。
//
// 文件缺席 / 为空 ⇒ 退出码 1 并点名。不静默跳过：R8 没跑与 R8 跑过且通过必须可分辨。

import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_MAPPING_DIR = "src-tauri/gen/android/app/build/outputs/mapping/arm64Release";
const DEFAULT_OWN_RULES = "src-tauri/gen/android/app/proguard-rules.pro";

/** R8 段落标记的两句原文（`-printconfiguration` 每一段规则前后各写一行）。 */
const SECTION_START = "The proguard configuration file for the following section is";
const SECTION_END = "End of content from";

/** 本仓自己那三份规则文件的文件名 —— 「外部来源」的判定就是「不是这三份里的任何一份」。 */
export const OWN_RULE_FILE_NAMES = Object.freeze([
  "proguard-rules.pro",
  "proguard-tauri.pro",
  "proguard-wry.pro",
]);

/** 期望来源写成它时，表示「随便哪一份**不是本仓自己写的**规则文件」。 */
export const EXTERNAL_SOURCE = "<外部来源>";

/**
 * 「这条规则文本真的进了 R8，**而且是从该来的那一份文件进来的**」的针。
 *
 * 每项是 `[针, 期望来源, 它证明什么]`。判定面是**该来源那一段**、**剥掉 `#` 注释之后**的正文。
 *
 * ── 为什么要按来源归因（2026-09-05 收官轮 A11）────────────────────────────────
 *
 * 上一版第 5 根针是 `com.polaris2.app.RustWebView`，登记的意思是「本批为 wry 那两个 JNI
 * 方法补的 keep 块」。而 Tauri CLI 生成的 `proguard-wry.pro` 里本来就有一条
 * `-keep class com.polaris2.app.RustWebView {`（成员是 loadUrlMainThread / evalScript…）——
 * 于是那根针**永远由别人喂绿**，本批那个 keep 块实际零观测面：整块删掉它照样命中。
 *
 * 同一形也适用于另外两根「无源码观测面」的针：`io.nekohasekai` 与 TauriPlugin 要证明的是
 * **aar 里的 consumer 规则真的被 AGP 喂给了 R8**。如果哪天有人把同一条规则抄进本仓自己的
 * `.pro`，针照样绿，而那一跳（AGP 内部把 consumer 规则并进来）其实已经断了。
 * 故这两根针的期望来源是 `EXTERNAL_SOURCE`：出现在本仓自己那三份里的**不算数**。
 */
export const CONFIGURATION_NEEDLES = Object.freeze([
  [
    "-keep class org.rustls.platformverifier",
    "proguard-rules.pro",
    "本仓 proguard-rules.pro 里那条 rustls keep",
  ],
  [
    "io.nekohasekai",
    EXTERNAL_SOURCE,
    "libbox.aar 内的 consumer proguard.txt（无源码观测面）。它必须来自 aar 那一段：\n" +
      "      抄进本仓自己的 .pro 也能让针命中，而那时「AGP 把 consumer 规则并进 R8」这一跳已经断了。",
  ],
  [
    "@app.tauri.annotation.TauriPlugin",
    EXTERNAL_SOURCE,
    "tauri-android 的 consumer 规则（无源码观测面），同上。",
  ],
  [
    "-dontwarn java.beans.ConstructorProperties",
    "proguard-rules.pro",
    "本批为 jackson 补的 -dontwarn",
  ],
  [
    "void clearAllBrowsingData();",
    "proguard-rules.pro",
    "本批为 wry 那两个 JNI 方法补的 keep 块。针钉在**成员**上而不是类名上：" +
      "类名 com.polaris2.app.RustWebView 在 Tauri 生成的 proguard-wry.pro 里也有一条 keep，" +
      "钉类名等于让别人替本批那个块作证。",
  ],
]);

/**
 * 「这条 keep 规则真的命中了东西」的针：每项是一组必须**同时出现在 seeds.txt 同一行**上的片段。
 *
 * seeds.txt 的行形态是 `<类全名>` 或 `<类全名>: <成员签名>`。这里刻意只要求「同一行里都出现」，
 * 不逐字比对整行：R8 版本之间成员签名的排版会变，而本判据要问的是「命中没命中」，
 * 不是「排版对不对」——把排版写进判据只会逼后人把判据改宽。
 */
export const SEED_NEEDLES = Object.freeze([
  [
    ["org.rustls.platformverifier.CertificateVerifier"],
    "Rust 侧按名字 loadClass 的那个类。keep 规则写成一个不存在的类名时，configuration.txt " +
      "照样有那行文本，而这里会是 0 —— R8 已经把它整包剪掉，release 每次 TLS 握手都报证书错。",
  ],
  [
    ["com.polaris2.app.PolarisTls"],
    "native 方法名就是 JNI 符号名的一部分，被改名即 UnsatisfiedLinkError。",
  ],
  [
    ["com.polaris2.app.RustWebView", "clearAllBrowsingData"],
    "wry 在 Rust 侧按方法名 JNI 调它；Kotlin 侧零调用点，R8 看不见引用。",
  ],
  [
    ["com.polaris2.app.RustWebView", "getCookies"],
    "同上；AGP 默认档那条 View getter 规则够不到它（那条限定无参 getter）。",
  ],
]);

/**
 * 反向对照：这个类不存在，seeds.txt 里必须**一次都没有**。
 *
 * 没有它，上面那组「必须命中」的断言无法与「匹配逻辑恒真」区分开 —— 一条永远返回命中的
 * 匹配器会让整组针全绿。
 */
export const SEED_ABSENT_CONTROL = "org.rustls.platformverifier.NoSuchThingEverAbsent";

/** 剥掉 `#` 行注释（ProGuard / R8 配置的唯一注释形态），保行结构。 */
export function stripHashComments(source) {
  return source
    .split("\n")
    .map((line) => {
      const at = line.indexOf("#");
      return at === -1 ? line : line.slice(0, at);
    })
    .join("\n");
}

/**
 * 把 configuration.txt 按 R8 的段落标记切成 `[{ source, body }]`。
 *
 * `source` 是那一段规则的来源文件路径（R8 写在 `# The proguard configuration file for the
 * following section is <路径>` 里）；没有标记罩着的内容 `source` 为 `null`。
 * `body` 是那一段**剥掉 `#` 注释之后**的正文 —— 注释里写着的同一句话不许给针作证。
 */
export function parseConfigurationSections(configuration) {
  const sections = [];
  let current = { source: null, lines: [] };
  const flush = () => {
    if (current.lines.length > 0) sections.push(current);
  };
  for (const line of configuration.split("\n")) {
    const trimmed = line.trim();
    if (trimmed.startsWith("#")) {
      const body = trimmed.replace(/^#+\s*/, "");
      if (body.startsWith(SECTION_START)) {
        flush();
        current = { source: body.slice(SECTION_START.length).trim(), lines: [] };
        continue;
      }
      if (body.startsWith(SECTION_END)) {
        flush();
        current = { source: null, lines: [] };
        continue;
      }
    }
    current.lines.push(line);
  }
  flush();
  return sections.map((section) => ({
    source: section.source,
    body: stripHashComments(section.lines.join("\n")),
  }));
}

/** 这一段的来源符不符合某根针的期望来源。 */
export function sourceMatches(source, expected) {
  if (source === null) return false;
  if (expected === EXTERNAL_SOURCE) {
    return !OWN_RULE_FILE_NAMES.some((own) => source.includes(own));
  }
  return source.includes(expected);
}

/** 从规则文件里取出「只由注释构成」的行（去掉 `#` 与首尾空白后仍足够长的那些）。 */
export function commentOnlyLines(source, minLength = 24) {
  return source
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("#"))
    .map((line) => line.replace(/^#+\s*/, "").trim())
    .filter((line) => line.length >= minLength);
}

/**
 * 判据本体。返回 `{ problems, notes }`；`problems` 非空即失败。
 *
 * @param {{configuration: string, seeds: string, ownRules: string}} input
 */
export function assertR8Evidence({ configuration, seeds, ownRules }) {
  const problems = [];
  const notes = [];

  // ── 取材面自检：空面上任何「含某串」的断言都恒假、任何「不含」都恒真 ──
  if (configuration.trim() === "") {
    problems.push("configuration.txt 是空的 —— R8 没写出配置，下面全部判据作废。");
  }
  if (seeds.trim() === "") {
    problems.push("seeds.txt 是空的 —— R8 一条 keep 都没命中，或产物路径变了。");
  }
  if (problems.length > 0) return { problems, notes };

  const configFace = stripHashComments(configuration);
  if (!configFace.includes("-keep") && !configFace.includes("-dontwarn")) {
    problems.push(
      "剥掉注释之后的 configuration.txt 里一条 `-keep` / `-dontwarn` 都没有 —— " +
        "取材面塌了（剥离器吃过头，或这份文件根本不是 R8 的配置转储）。",
    );
    return { problems, notes };
  }

  // ── ① 注释确实被剥掉了：拿本仓自己的注释行做正向对照 ──
  //
  // 这不是可有可无的一步。本判据存在的直接原因就是「针命中的是本仓注释里的同一句话」，
  // 而「剥离到底有没有发生」在结果上与「R8 本来就不带注释」长得一模一样。
  const ourComments = commentOnlyLines(ownRules);
  const carried = ourComments.filter((line) => configuration.includes(line));
  if (carried.length === 0) {
    notes.push(
      "R8 这一版的 configuration.txt 里找不到本仓规则文件的注释原文 —— " +
        "剥注释这一步这次没有实际作用（留着：它守的是 R8 换实现后又开始抄注释这一形）。",
    );
  } else {
    const survivors = carried.filter((line) => configFace.includes(line));
    if (survivors.length > 0) {
      problems.push(
        `剥注释没生效：本仓 proguard-rules.pro 的 ${survivors.length} 行注释原文仍留在判定面上，` +
          `例如「${survivors[0].slice(0, 60)}」。注释里写着的同一句话会把针喂绿。`,
      );
    } else {
      notes.push(
        `R8 把本仓 ${carried.length} 行注释原文抄进了 configuration.txt，判定面已把它们剥掉。`,
      );
    }
  }

  // ── ② 规则文本到没到 R8，**而且是从该来的那一份文件来的** ──
  const sections = parseConfigurationSections(configuration);
  const attributed = sections.filter((section) => section.source !== null);
  if (attributed.length === 0) {
    problems.push(
      "configuration.txt 里一个段落标记都没有 —— 规则归不到来源文件上，下面五根针只能退化成" +
        "「全文里有没有这串」，而那正是 A11 那条真缺陷（本批的 keep 块被 Tauri 生成的 " +
        "proguard-wry.pro 喂绿）。R8 的 -printconfiguration 每段前会写 " +
        `\`# ${SECTION_START} <路径>\`；这份文件里没有 ⇒ 要么它不是 R8 的转储，` +
        "要么 R8 换了输出格式（换格式时来改这里的解析，别把归因判据摘掉）。",
    );
    return { problems, notes };
  }
  notes.push(
    `configuration.txt 的来源段：${attributed.map((section) => section.source).join(" | ")}`,
  );
  for (const [needle, expectedSource, why] of CONFIGURATION_NEEDLES) {
    const matching = attributed.filter((section) => sourceMatches(section.source, expectedSource));
    const hits = matching.filter((section) => section.body.includes(needle));
    if (hits.length === 0) {
      const elsewhere = sections.filter((section) => section.body.includes(needle));
      const hint =
        elsewhere.length > 0
          ? `（它出现在别的段里：${elsewhere.map((s) => s.source ?? "<无归属>").join(" | ")} ——` +
            "那不算数，见下面这条针的登记理由）"
          : "（整份文件的任何一段里都没有它）";
      problems.push(
        `configuration.txt 里来源为 \`${expectedSource}\` 的段落（剥注释后）中没有 ` +
          `\`${needle}\`${hint} —— 它证明的是：${why}`,
      );
    } else {
      notes.push(
        `configuration.txt  ${needle} ← ${hits.map((section) => section.source).join(" | ")}`,
      );
    }
  }

  // ── ③ 规则到底保住了什么：seeds.txt ──
  const seedLines = seeds.split("\n").map((l) => l.trim()).filter(Boolean);
  if (seedLines.length < 10) {
    problems.push(
      `seeds.txt 只有 ${seedLines.length} 行 —— 一次真实的 release 构建会保住成百上千个符号，` +
        "这么少多半是 R8 没跑完或文件被截断，下面的命中断言在这种面上没有信息量。",
    );
    return { problems, notes };
  }
  for (const [parts, why] of SEED_NEEDLES) {
    const matched = seedLines.filter((line) => parts.every((p) => line.includes(p)));
    if (matched.length === 0) {
      problems.push(
        `seeds.txt 里没有同时含 ${parts.map((p) => `\`${p}\``).join(" 与 ")} 的行 —— ` +
          `这条 keep 规则一个东西都没保住。它守的是：${why}`,
      );
    } else {
      notes.push(`seeds.txt        ${parts.join(" + ")} × ${matched.length}`);
    }
  }
  const controlHits = seedLines.filter((line) => line.includes(SEED_ABSENT_CONTROL)).length;
  if (controlHits !== 0) {
    problems.push(
      `反向对照失败：seeds.txt 里出现了 \`${SEED_ABSENT_CONTROL}\`（${controlHits} 次）。` +
        "这个类根本不存在 —— 匹配逻辑恒真，上面那组「必须命中」的断言全部没有信息量。",
    );
  } else {
    notes.push(`反向对照      ${SEED_ABSENT_CONTROL} × 0（匹配器不是恒真的）`);
  }

  return { problems, notes };
}

function readOrDie(path, why) {
  if (!existsSync(path)) {
    console.error(`::error::assert-r8-evidence: 取材点不在场：${path}\n  ${why}`);
    process.exit(1);
  }
  return readFileSync(path, "utf8");
}

function main(argv) {
  const mappingDir = resolve(REPO, argv[0] ?? DEFAULT_MAPPING_DIR);
  const ownRulesPath = resolve(REPO, argv[1] ?? DEFAULT_OWN_RULES);
  const configuration = readOrDie(
    join(mappingDir, "configuration.txt"),
    "R8 的 `-printconfiguration` 转储。不在 = R8 没跑，或 AGP 改了产物路径。",
  );
  const seeds = readOrDie(
    join(mappingDir, "seeds.txt"),
    "R8 的 `-printseeds` 转储（每条 keep 规则实际命中的类与成员）。",
  );
  const ownRules = readOrDie(ownRulesPath, "本仓入库的规则文件，只用作剥注释的正向对照。");

  const { problems, notes } = assertR8Evidence({ configuration, seeds, ownRules });
  for (const note of notes) console.log(`  · ${note}`);
  if (problems.length === 0) {
    console.log(`assert-r8-evidence: ${mappingDir} 的产物级判据全部通过。`);
    return 0;
  }
  for (const problem of problems) console.error(`::error::assert-r8-evidence: ${problem}`);
  return 1;
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))) {
  process.exit(main(process.argv.slice(2)));
}
