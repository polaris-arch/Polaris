#!/usr/bin/env node
// verify-wry-keep-rules.mjs —— 把 proguard-rules.pro 里那条 RustWebView keep 块与它的**事实源**对拍。
//
// ── 守的是什么（根因）────────────────────────────────────────────────────────────
//
// `proguard-rules.pro` 有一条按**方法签名**写死的 keep：
//
//     -keep class com.polaris2.app.RustWebView {
//       void clearAllBrowsingData();
//       java.lang.String getCookies(java.lang.String);
//     }
//
// 它盯的两个方法住在 `gen/android/app/src/main/java/com/polaris2/app/generated/RustWebView.kt`
// —— 一份 **gitignored 的 Tauri CLI 生成物**，内容随 wry 版本走。wry 一升级、方法改名或改签名，
// 规则就不再匹配任何成员，R8 静默把方法剪掉，而：五条 Rust 源码门全绿（它们只查规则文本在不在）、
// CI 全绿、debug 包全绿（debug 不 minify）。故障要到 release 包真机上按到某条路径才现形。
//
// ProGuard 对「keep 了一个不存在的成员」不报错，这是它的设计（规则是模式，不是引用）。
// 所以这条腿必须自己去事实源里取签名回来对拍，不能指望 R8 说话。
//
// ── 为什么是一份独立脚本，而不是 Rust 门里的第六条断言 ────────────────────────────
//
// 取材点 `generated/` 被 `app/.gitignore` 忽略：裸 checkout 上它不存在，而 `cargo test` 正是在裸
// checkout 上跑（ci.yml）。让 Rust 门去读它，只有两种结局 —— 在 CI 上必然 panic，或者写成
// 「文件不在就跳过」而变成一条**静默缺席**的假门。
//
// 出路是把判据放到生成物**保证在场**的那一刻：`.github/workflows/android.yml` 里
// `tauri android build` 铺完生成物的下一步。Rust 门那边改为守「这一步还在不在 android.yml 里」
// （`android_release_signing_wiring.rs` 的 ⑥），于是删掉这条腿会在每次 `cargo test` 上红。
// 判据只有这一份实现，两处调用。
//
// ── 用法 ────────────────────────────────────────────────────────────────────────
//
//   node scripts/verify-wry-keep-rules.mjs            # 仓库根执行
//
// 生成物不在场 ⇒ 退出码 1 并点名（**不静默跳过**：这条腿的全部价值就在于取材点在场时的对拍，
// 「没跑」必须与「跑过且通过」可分辨）。

import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Tauri CLI 生成的 wry WebView 子类（gitignored，`tauri android build` 现场铺出来）。 */
const KOTLIN = "src-tauri/gen/android/app/src/main/java/com/polaris2/app/generated/RustWebView.kt";
/** 唯一入库的 R8 规则文件。 */
const PROGUARD = "src-tauri/gen/android/app/proguard-rules.pro";

/**
 * Kotlin 类型 → ProGuard 规则里该写的 JVM 类型名。
 *
 * 刻意只列本文件今天真的用到的几个，**不做通用映射**：碰到没登记的类型一律硬失败并点名，
 * 让人回来看一眼，而不是让一个猜错的映射把对拍悄悄放过去。
 */
const TYPE_MAP = new Map([
  ["Unit", "void"],
  ["", "void"], // Kotlin 无返回类型标注 = Unit
  ["String", "java.lang.String"],
  ["Int", "int"],
  ["Boolean", "boolean"],
]);

/** 读文件；不在场就点名退出（取材点缺席必须自曝，不能静默跳过）。 */
function read(rel, why) {
  const path = resolve(REPO, rel);
  if (!existsSync(path)) {
    fail(`取材点不在场：${rel}\n  ${why}`);
  }
  return readFileSync(path, "utf8");
}

function fail(message) {
  console.error(`::error::verify-wry-keep-rules: ${message}`);
  process.exit(1);
}

/** 剥掉 `#` 行注释（ProGuard 的唯一注释形态），保行结构。 */
function stripHashComments(source) {
  return source
    .split("\n")
    .map((line) => {
      const at = line.indexOf("#");
      return at === -1 ? line : line.slice(0, at);
    })
    .join("\n");
}

// 剥掉 Kotlin 的行注释与块注释；字符串字面量原样留着（本门的针是符号，不是字面量）。
// 写成行注释而不是文档注释：这段的说明里必须出现块注释的收尾定界符，写进文档注释会把它提前关掉。
function stripKotlinComments(source) {
  let out = "";
  let i = 0;
  while (i < source.length) {
    const two = source.slice(i, i + 2);
    if (two === "//") {
      const end = source.indexOf("\n", i);
      i = end === -1 ? source.length : end;
    } else if (two === "/*") {
      const end = source.indexOf("*/", i + 2);
      i = end === -1 ? source.length : end + 2;
    } else if (source[i] === '"') {
      // 整段跳过字符串，免得里面的 `//` 被当注释起笔
      let j = i + 1;
      while (j < source.length && source[j] !== '"') {
        j += source[j] === "\\" ? 2 : 1;
      }
      out += source.slice(i, Math.min(j + 1, source.length));
      i = j + 1;
    } else {
      out += source[i];
      i += 1;
    }
  }
  return out;
}

/** 取 `marker` 之后第一个 `{` 起的花括号配平块（不含首尾大括号）。切不出来当场死。 */
function balancedBlock(source, marker, origin) {
  const at = source.indexOf(marker);
  if (at === -1) {
    fail(`${origin}：找不到 \`${marker}\` —— 取材切片失败，本腿下游判据全部作废。`);
  }
  const rest = source.slice(at);
  const open = rest.indexOf("{");
  if (open === -1) fail(`${origin}：\`${marker}\` 之后没有 \`{\`，块结构变了。`);
  let depth = 0;
  for (let i = open; i < rest.length; i += 1) {
    if (rest[i] === "{") depth += 1;
    else if (rest[i] === "}") {
      depth -= 1;
      if (depth === 0) return rest.slice(open + 1, i);
    }
  }
  return fail(`${origin}：\`${marker}\` 起的花括号没有配平 —— 取材切片失败。`);
}

// ══ ① 事实源：从生成的 Kotlin 里把「类的全限定名」与「全部方法签名」抠出来 ══

const kotlin = stripKotlinComments(
  read(
    KOTLIN,
    "它由 `tauri android build` 现场生成、被 app/.gitignore 忽略。" +
      "本腿必须在生成物铺好之后跑（见 .github/workflows/android.yml 的 wry keep 对拍步）。",
  ),
);

const packageMatch = kotlin.match(/^\s*package\s+([\w.]+)/m);
if (!packageMatch) fail(`${KOTLIN}：读不到 \`package\` 声明 —— 全限定名反推不出来。`);

const classMatch = kotlin.match(/\bclass\s+(\w+)\s*\(/);
if (!classMatch) fail(`${KOTLIN}：读不到 \`class <名>(\` 声明 —— 类名反推不出来。`);

const fqcn = `${packageMatch[1]}.${classMatch[1]}`;

// ── 取材面收窄到**那个类的类体**，而不是整份文件 ────────────────────────────────
//
// 2026-09-05 验收实测的 M03：把 `clearAllBrowsingData` / `getCookies` 两个方法从
// `RustWebView` 搬到同一份文件里的**另一个类**下，本脚本与全部源码门一起绿 —— 因为提取器
// 当时是按整份文件收 `fun`，搬去哪个类它都看得见；而 R8 那边这条 keep 指名的是
// `com.polaris2.app.RustWebView`，方法已经不在那个类里，照剪不误。
//
// 对拍的两侧必须是**同一个类**：规则里写的是 `<fqcn> { <成员> }`，事实源那侧就得是
// `<fqcn>` 的类体。下面先把类体切出来，再在类体里收 `fun`。
//
// 残留（明写）：`RustWebView` 类体内的**嵌套类**也在这个切片里，把方法搬进嵌套类仍能骗过
// 本条。下面「同名方法在整份文件里必须恰好声明一次」那条是为它准备的第二层 —— 嵌套类里
// 的同名方法会让计数变成 2 而红；而把方法**只**放进嵌套类则会让下面 `-keep` 成员对拍
// 找得到签名，故那一形由 seeds.txt 那条产物级判据兜（scripts/assert-r8-evidence.mjs）。
const classBody = balancedBlock(kotlin, `class ${classMatch[1]}`, KOTLIN);

/**
 * 收集 `fun <名>(<形参>): <返回>` 声明，归一成 ProGuard 成员签名的形状。
 *
 * `override` 的那些一并收（判据只问「规则里那条在不在事实源里」，不问它是不是覆写）。
 */
const declared = new Map(); // 归一签名 → 方法名
const funRe = /\bfun\s+(\w+)\s*\(([^)]*)\)\s*(?::\s*([\w.<>, ?]+?))?\s*\{/g;
for (const m of classBody.matchAll(funRe)) {
  const [, name, rawParams, rawReturn] = m;
  const params = rawParams
    .split(",")
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      const t = p.split(":").slice(1).join(":").trim();
      // 泛型（`Map<String, String>`）在 ProGuard 侧是擦除后的裸类型；本门今天不需要它们，
      // 登记为不可映射，由下面的 TYPE_MAP 查表统一硬失败。
      return t;
    });
  const ret = (rawReturn ?? "").trim();
  declared.set(`${name}(${params.join(",")})->${ret}`, name);
}

if (declared.size === 0) {
  fail(
    `${KOTLIN}：\`${fqcn}\` 的类体里一个 \`fun\` 都没抠出来 —— ` +
      `要么提取器与生成物的形态对不上了，要么方法整批搬去了别的类。两种情况判据都作废。`,
  );
}

// ══ ② 规则侧：把那条 keep 块里的成员签名抠出来 ══

const proguard = stripHashComments(read(PROGUARD, "它是入库文件，不该缺席。"));

const keepMarker = `-keep class ${fqcn} {`;
if (!proguard.includes(keepMarker)) {
  fail(
    `${PROGUARD}：找不到 \`${keepMarker}\`。\n` +
      `  类的全限定名是从 ${KOTLIN} 的 package + class 反推出来的：` +
      `生成物改了包名/类名，这条 keep 就不再指向任何东西，而 R8 不会为此报错。`,
  );
}
const keepBlock = balancedBlock(proguard, keepMarker, PROGUARD);

const members = keepBlock
  .split(";")
  .map((s) => s.trim())
  .filter(Boolean);

if (members.length === 0) {
  fail(`${PROGUARD}：\`${fqcn}\` 的 keep 块是空的 —— 那条规则一个成员都没保，等于没写。`);
}

// ══ ③ 对拍：规则里的每一条成员，事实源里必须真有一个同名同签名的方法 ══

/** 把 Kotlin 类型翻成 ProGuard 该写的名字；没登记的类型硬失败（不猜）。 */
function toJvm(kotlinType, context) {
  const key = kotlinType.replace(/\?$/, "");
  if (!TYPE_MAP.has(key)) {
    fail(
      `${KOTLIN}：类型 \`${kotlinType}\`（出现在 ${context}）没有登记的 ProGuard 映射。\n` +
        `  在 scripts/verify-wry-keep-rules.mjs 的 TYPE_MAP 里补一条再跑 —— ` +
        `猜一个映射会把对拍悄悄放过去，那正是本腿要防的事。`,
    );
  }
  return TYPE_MAP.get(key);
}

/** 事实源的签名集合，翻成 ProGuard 形状。 */
const factSignatures = new Map(); // "void clearAllBrowsingData()" → 方法名
for (const [normalized, name] of declared) {
  const paramsPart = normalized.slice(normalized.indexOf("(") + 1, normalized.lastIndexOf(")"));
  const retPart = normalized.slice(normalized.indexOf("->") + 2);
  const params = paramsPart ? paramsPart.split(",") : [];
  // 泛型形参的方法（loadUrlMainThread(Map<..>)）本门今天不对拍，跳过而不是硬失败：
  // 规则里没有它们，跳过不会让任何一条规则失去对照；真要 keep 它们时 TYPE_MAP 会当场喊。
  if (params.some((p) => p.includes("<")) || retPart.includes("<")) continue;
  const ret = toJvm(retPart, `${name} 的返回类型`);
  const args = params.map((p) => toJvm(p, `${name} 的形参`));
  factSignatures.set(`${ret} ${name}(${args.join(",")})`, name);
}

const problems = [];
for (const member of members) {
  // 规则成员形如 `void clearAllBrowsingData()` / `java.lang.String getCookies(java.lang.String)`
  const normalized = member.replace(/\s+/g, " ").replace(/\s*,\s*/g, ",").replace(/\s*\(\s*/, "(");
  if (!factSignatures.has(normalized)) {
    problems.push(normalized);
  }
}

// ── 反向：规则点名的每个方法名，在**整份文件**里必须恰好声明一次 ──────────────
//
// 上面那条只问「规则里的成员在类体里找不找得到」。它对「同名方法在别的类里也有一份」是瞎的，
// 而那正是把方法搬走时最省事的走法（留一个同名壳、真正的实现搬去别处）。
// 这条计数把「有且只有这一处」变成正面断言。
const duplicated = [];
for (const [signature, name] of factSignatures) {
  if (!members.some((m) => m.includes(`${name}(`))) continue;
  const declarations = kotlin.match(new RegExp(`\\bfun\\s+${name}\\s*\\(`, "g")) ?? [];
  if (declarations.length !== 1) {
    duplicated.push(`${signature}（整份文件里声明了 ${declarations.length} 次）`);
  }
}
if (duplicated.length > 0) {
  fail(
    `${KOTLIN}：下面这些被 keep 规则点名的方法，在整份文件里不是恰好声明一次：\n` +
      duplicated.map((d) => `  - ${d}`).join("\n") +
      `\n\n0 次 = 类体里那份是切片切出来的幻觉；多次 = 另一个类里也有同名方法，` +
      `而 keep 规则只指向 \`${fqcn}\` —— 对拍会对上错的那一个。`,
  );
}

if (problems.length > 0) {
  fail(
    `${PROGUARD} 的 \`${fqcn}\` keep 块里，下面的成员在事实源里**找不到对应方法**：\n` +
      problems.map((p) => `  - ${p}`).join("\n") +
      `\n\n事实源 ${KOTLIN} 现有的可对拍签名：\n` +
      [...factSignatures.keys()].map((s) => `  · ${s}`).join("\n") +
      `\n\n这条 keep 的成员清单是按 wry 在 Rust 侧 JNI 调用的方法名写的（wry 0.55.1 的\n` +
      `main_pipe.rs 用 call_method 按名字调 clearAllBrowsingData / getCookies）。\n` +
      `wry 升级把方法改名或改了签名 ⇒ 规则不再匹配任何成员 ⇒ R8 静默剪掉 ⇒ release 包上\n` +
      `NoSuchMethodError，而 debug 包与全部源码门都是绿的。规则与 wry 一起改，别只改一边。`,
  );
}

console.log(
  `verify-wry-keep-rules: ${fqcn} 的 ${members.length} 条 keep 成员全部在 ${KOTLIN} 里对得上：`,
);
for (const member of members) console.log(`  ✓ ${member.replace(/\s+/g, " ")}`);
