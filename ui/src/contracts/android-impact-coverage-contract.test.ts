/**
 * Android 影响面的**完备性门**（2026-09-05）。
 *
 * # 背景：两条轴不对称
 *
 * `scripts/classify-ci-impact.mjs` 上有两条正交的判定轴：
 *
 *  - 「**包影响**」轴有完备性门（`ci-impact-coverage-contract.test.ts`）：每个 crate、每个
 *    `src-tauri` 顶层子树都必须显式落进 `PACKAGE_IMPACT_SCOPES` 或 `NO_PACKAGE_IMPACT_SCOPES`；
 *  - 「**Android 影响**」轴此前**一条完备性门都没有**。`ANDROID_IMPACT_SCOPES` 是这条腿
 *    （`.github/workflows/android.yml`，没有 push/PR 触发，只由 release-risk 按 `android == 'true'`
 *    调用）的**唯一**触发面，而它只有一条单向断言（「表里每条都必须真的点亮腿」，在隔壁文件）——
 *    那条抓的是装饰性登记，抓不到**漏登记**。
 *
 * 实测形态（2026-09-05）：全仓 11 个含真代码 `cfg(target_os = "android")` 的 Rust 文件里，一个都不
 * 点亮 APK 腿，其中包括 VpnService 桥 `src-tauri/src/runtime/proxy/android_bridge.rs`。它们**是不是**
 * 该点亮是另一件事（本批逐个判过，全判 NO，理由见 `NO_ANDROID_IMPACT_SCOPES`）；缺陷在于
 * 「没有任何东西要求有人判一次」——下一个 Android 专属文件加进来时，同样静默落在腿外。
 *
 * # 本门守什么
 *
 *  - **A（机械层，无需判断）**：声明了 JNI 导出的 Rust 文件，必须被 `ANDROID_IMPACT_SCOPES` 覆盖。
 *    JNI 符号是链接期发射的，而 `polaris` 在 android 上只有一条不 codegen 的 `cargo check`
 *    （见 D 组），APK 腿是全仓唯一真链 `libpolaris_lib.so` 的地方。
 *  - **B（完备性，射程已收窄）**：**含 android 条件代码的共享文件**必须落进两张 Android 表之一 ——
 *    即「有人显式判过一次」。取材面只剩一条腿：文件内含**真代码** `cfg(target_os = "android")`。
 *    🔴 2026-09-05 删掉了第二条腿（「cfg 写在父模块 `mod` 声明上 ⇒ 推导出子文件」）。那条腿问的是
 *    「哪些文件**只在 android 上编译**」，而它是在用正则重新实现 rustc 的 `mod` 解析与 cfg 求值 ——
 *    两轮里被连续绕过五种形态：已登记文件里再 `mod` 出一个子文件（两层嵌套 ⇒ 全新文件完全隐形）、
 *    `#[cfg_attr(target_os = "android", path = "x_android.rs")]`（门点名的是**非 android 的那份**，
 *    照它自己给的修法登记之后就 fail-open）、`#[cfg(target_os = r"android")]`、`#[cfg(...)]mod foo;`
 *    零空白、内联夹具的 raw string 里写 `mod`（判据被自己污染）。补形态是打地鼠。
 *    那个子问题整体移交产物级真值门 `scripts/check-android-only-face.mjs`（ci.yml 的 dep-info 对差
 *    那一步，D4 组钉住），真值源是 rustc 自己的 `--emit=dep-info`。
 *    **两者互补、不是替代**：dep-info 结构上看不见共享文件里的 android 条件代码（两侧都编 ⇒
 *    不进差集），那正是本组守的东西；本组也看不见「文件自己不含 target_os 字样」的 android 专属
 *    文件，那正是 dep-info 门守的东西。
 *  - **C（哨兵 / 取材面自检）**：枚举面为空或明显偏小 ⇒ 红；剥离器被注释或字符串喂饱 ⇒ 红。
 *  - **D（替补覆盖还在）**：`NO_ANDROID_IMPACT_SCOPES` 的理由几乎都落在 ci.yml 的
 *    `aarch64-linux-android` 交叉腿上，而那条腿今天**没有任何门钉住**它自己（`cross_target_coverage.rs`
 *    只钉 clippy 主行字面量与从 `rustup target add` 行**派生**的 triple 集）。在 ci.yml 与豁免表两侧
 *    同时动手，就能让上面那些理由集体变成假话而全仓无一处转红。D 组把它们钉成正面断言。
 *    D2 按 **step** 取材而不是整文件 grep（子串存在性对「命令还在但不生效 / 换了包」全盲），
 *    D3 把**整包**豁免与 **scope 型**豁免一并编进比对面（只数前者时，一条 `"scope": "tests"`
 *    就能静默摘掉两条 NO 理由点名的唯一覆盖）。
 *  - **D4（dep-info 对差门的执行自曝）**：「哪些文件只在 android 上编译」已整体移交 ci.yml 的
 *    `scripts/check-android-only-face.mjs` 那一步（见 B 组）。那一步被删掉 / 被关掉 / 命令被中和 /
 *    两侧选择器被改成不对称，本文件其余各组一条都不会红 —— D4 把「没执行」变成自曝。
 *
 * # 🔴 取材必须先剥注释**与字符串**（本仓记过的「判据被自己污染」）
 *
 * `src-tauri/tests/android_platform_verifier_wiring.rs` 里 `jni_mangle` 出现 18 次，**全在注释与字符串
 * 字面量里**（它是那道接线门的取材代码，第 109 行是 `let attr = "#[jni::jni_mangle(";` —— 一条**真代码行**
 * 上的字符串）。按原文反查会把一道门误判成一个 JNI 导出点。
 *
 * 「只剥整行注释」的方案对本门是**错的**：方向是「宁可多取」，而这里多取 ⇒ 假红（逼后人把判据改宽
 * = 门被磨钝）。故本门自带一个词法级剥离器（照 `crates/source-probe` 的 `mask_comments` /
 * `mask_comments_and_strings` 两档移植；`scripts/check-android-bridge.mjs` 的 `stripComments()`
 * 是同一份移植的 `keep-literals` 档），并在 C 组用**内联夹具 + 仓内真实文件**两路证明它对那个文件
 * 给出正确结果。
 *
 * 两档不是「剥得多一点少一点」，是**判据的针不同**（同 source-probe 的头注）：
 *  - A 组的针是 `#[jni::jni_mangle(` / `extern … fn Java_`，是**符号**面 ⇒ 连字符串一起剥（`code-only`）；
 *  - B 组的针 `target_os = "android"` **本身含字符串字面量** ⇒ 在只剥注释的面（`keep-literals`）上找，
 *    剥了字符串针就永远命中不到。
 *
 * 🔴 但**只用 `keep-literals` 会被内联 Rust 夹具喂饱**：`src-tauri/tests/` 下一个只含
 * `const FIXTURE: &str = r####"…#[cfg(target_os = "android")] mod fixture_only;…"####;` 的文件
 * 会被判成「含 Android 专属代码」而把门打红。修法是**两个面合取**（[`hasRealAndroidCfg`]）：
 * 命中位置由 `keep-literals` 面给出，再要求**同一偏移**在 `code-only` 面上仍是 `target_os` 这个
 * 标识符。两档剥离器都换行守恒、只把被抹的字节换成空格 ⇒ 偏移逐字对齐，可以直接合取。
 *  - 真代码 `#[cfg(target_os = "android")]`：`code-only` 抹掉 `"android"`，`target_os` 是标识符、留着 ⇒ 命中；
 *  - 夹具 raw string 里的同一串：`code-only` 把整段连同里面的 `target_os` 一起抹掉 ⇒ 不命中。
 *
 * # 这门抓不到什么（如实登记）
 *
 * - **判定内容的对错**：本门只保证「有人显式判过并留了理由」。把 `android_bridge.rs` 判成 NO 而其实
 *   该 YES，本门不红 —— 那由 review 与两张表里的理由本身承担。
 * - **只在字符串里出现的 `target_os = "android"` 不进面**（两个面合取的直接后果）：那是「判据被
 *   自己污染」，不是漏检；C 组 ③b 有正反两向对照。真代码里的 `"android"` 与 `r"android"` 都算数。
 * - **「只在 android 上编译」的那一整类文件本门管不到**：文件自己不含 `target_os` 字样时（cfg 写在
 *   别处的 `mod` 声明上 / `#[cfg_attr(…, path = …)]` 派发 / 两层嵌套），本门的针一个都扎不到它。
 *   那一类归 ci.yml 的 dep-info 对差门（`scripts/check-android-only-face.mjs`，D4 钉住）。
 * - **枚举面只有 `.rs` 与 `Cargo.toml`**：Kotlin / Gradle / manifest 侧的 Android 专属文件全部住在
 *   `src-tauri/gen/android/`，那棵树整树登记在 `ANDROID_IMPACT_SCOPES`，不需要逐文件枚举。
 * - **「这条腿真的会被调用」**：那要跑一次真实 CI 才能确认，本门只断言分类器的输出。
 * - **D2 / D4 的「这一步会不会跑」只判到 step 级**：`if:` 是常量假 / `continue-on-error: true` /
 *   执行到那条命令时 errexit 已被 `set +e` 关掉 / 命令被 `|| true` 中和，四种它都红；但**语义上永不成立**的条件
 *   （如把 `runner.os == 'Linux'` 改成一个永不为真的判断）、以及 job 级 `if:` 与 workflow 触发
 *   条件被关掉，它看不见 —— 那要么得实跑一次 CI，要么得把 GitHub 的表达式求值搬进本门。
 */
import { describe, expect, it } from 'vitest';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const REPO_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

type Impact = { android: boolean };

type Classifier = {
  classifyImpact: (paths: string[], options?: { forceFull?: boolean }) => Impact;
  androidRegistrationOf: (path: string) => { key: string; table: string } | null;
  ANDROID_IMPACT_SCOPES: Record<string, { why: string }>;
  NO_ANDROID_IMPACT_SCOPES: Record<string, string>;
};

const classifier: Classifier = await import(
  pathToFileURL(join(REPO_ROOT, 'scripts/classify-ci-impact.mjs')).href
);

const { classifyImpact, androidRegistrationOf, NO_ANDROID_IMPACT_SCOPES } = classifier;

/* ────────────────────────── 词法级剥离器 ────────────────────────── */

/**
 * 把 Rust 源码里的注释（`//` 行注释、嵌套块注释）抹成空格；
 * `mode === 'code-only'` 时**连字符串 / 原始字符串 / 字符与字节字面量一起抹**。
 *
 * 换行守恒（行号不漂），只把被抹的字节换成 ASCII 空格。移植自
 * `crates/source-probe/src/lib.rs` 的 `mask()`——判据的针是符号就用 `code-only`，
 * 针本身是字符串字面量就用 `keep-literals`（理由见文件头）。
 *
 * 已知边界（与 source-probe 同）：生命周期标注 `'a` 与字符字面量 `'a'` 词法上只差一个收尾引号，
 * 判不出就当生命周期放过 —— 放过的后果是该处字面量**留在面上**（方向是少剥，不是误剥）。
 */
function maskRust(source: string, mode: 'code-only' | 'keep-literals'): string {
  const maskLiterals = mode === 'code-only';
  const out = source.split('');
  const n = source.length;
  const at = (i: number) => (i < n ? source.charCodeAt(i) : -1);
  const blank = (from: number, to: number) => {
    for (let k = from; k < Math.min(to, n); k += 1) if (out[k] !== '\n') out[k] = ' ';
  };
  const isIdentStart = (c: number) =>
    c === 0x5f || (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a) || c > 127;
  const isIdentCont = (c: number) => isIdentStart(c) || (c >= 0x30 && c <= 0x39);

  /** 开引号之后的 `from` 起，普通字符串的结束偏移（开区间右端；未闭合则到文件尾）。 */
  const normalStringEnd = (from: number) => {
    let j = from;
    while (j < n) {
      if (at(j) === 0x5c) {
        j += 2;
        continue;
      }
      if (at(j) === 0x22) return j + 1;
      j += 1;
    }
    return n;
  };
  /** `r` / `b` / `br` 前缀起的原始字符串结束偏移；不是原始字符串则 null。 */
  const rawStringEnd = (start: number) => {
    let j = start;
    while (j < n && isIdentCont(at(j))) j += 1;
    let hashes = 0;
    while (at(j) === 0x23) {
      hashes += 1;
      j += 1;
    }
    if (at(j) !== 0x22) return null;
    j += 1;
    const close = `"${'#'.repeat(hashes)}`;
    const found = source.indexOf(close, j);
    return found < 0 ? n : found + close.length;
  };
  /** `'` 起的字符字面量结束偏移；判不出（多半是生命周期）则 null。 */
  const charLiteralEnd = (start: number) => {
    let j = start + 1;
    if (at(j) === 0x5c) {
      j += 1;
      while (j < n && at(j) !== 0x27) {
        if (j - start > 12) return null;
        j += 1;
      }
      return j < n ? j + 1 : null;
    }
    if (j < n && at(j) !== 0x27 && at(j + 1) === 0x27) return j + 2;
    return null;
  };

  let i = 0;
  while (i < n) {
    if (at(i) === 0x2f && at(i + 1) === 0x2f) {
      let end = i;
      while (end < n && at(end) !== 0x0a) end += 1;
      blank(i, end);
      i = end;
      continue;
    }
    if (at(i) === 0x2f && at(i + 1) === 0x2a) {
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (at(j) === 0x2f && at(j + 1) === 0x2a) {
          depth += 1;
          j += 2;
        } else if (at(j) === 0x2a && at(j + 1) === 0x2f) {
          depth -= 1;
          j += 2;
        } else j += 1;
      }
      blank(i, j);
      i = j;
      continue;
    }
    // 标识符整体消费：`r` / `b` / `br` 只有作为**独立**标识符时才是字面量前缀，
    // 逐字符扫会把 `foo_r"…"` 里的 `r"` 读成原始字符串起点、从此整段偏移错位。
    if (isIdentStart(at(i))) {
      let j = i;
      while (j < n && isIdentCont(at(j))) j += 1;
      const ident = source.slice(i, j);
      if (ident === 'r' || ident === 'b' || ident === 'br') {
        const raw = rawStringEnd(i);
        if (raw !== null) {
          if (maskLiterals) blank(i, raw);
          i = raw;
          continue;
        }
        if ((ident === 'b' || ident === 'br') && at(j) === 0x22) {
          const end = normalStringEnd(j + 1);
          if (maskLiterals) blank(i, end);
          i = end;
          continue;
        }
        if (ident === 'b' && at(j) === 0x27) {
          const end = charLiteralEnd(j);
          if (end !== null) {
            if (maskLiterals) blank(i, end);
            i = end;
            continue;
          }
        }
      }
      i = j;
      continue;
    }
    if (at(i) === 0x22) {
      const end = normalStringEnd(i + 1);
      if (maskLiterals) blank(i, end);
      i = end;
      continue;
    }
    if (at(i) === 0x27) {
      const end = charLiteralEnd(i);
      if (end !== null) {
        if (maskLiterals) blank(i, end);
        i = end;
        continue;
      }
    }
    i += 1;
  }
  return out.join('');
}

/** TOML 只有 `#` 行注释、没有块注释；整行注释抹成空行，字符串原样留（针在字符串里）。 */
function maskTomlComments(source: string): string {
  return source
    .split('\n')
    .map((line) => (line.trimStart().startsWith('#') ? '' : line))
    .join('\n');
}

/* ────────────────────────── 枚举面 ────────────────────────── */

/** 构建产物 / 依赖树 / 版本库内部目录：里面的东西不是本仓源码。 */
const SKIP_DIRS = new Set(['.git', '.codegraph', 'node_modules', 'target', 'dist', 'build']);

function walk(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_DIRS.has(entry.name)) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, acc);
    else if (entry.isFile()) acc.push(full);
  }
  return acc;
}

const ALL_FILES = walk(REPO_ROOT)
  .map((p) => relative(REPO_ROOT, p).split(/[\\/]/).join('/'))
  .sort();

const RUST_FILES = ALL_FILES.filter((p) => p.endsWith('.rs'));
const CARGO_TOMLS = ALL_FILES.filter((p) => p === 'Cargo.toml' || p.endsWith('/Cargo.toml'));

/**
 * 判据的针。
 *
 * `extern` 那一支写成「ABI 字符串可有可无」：`code-only` 面上 `"system"` 已经被抹成空格，
 * 留下 `extern      fn Java_…`；不写成可选就会在自己的净化面上永远命中不到（判据不是变弱，是消失）。
 */
const JNI_EXPORT = /#\s*\[\s*(?:jni\s*::\s*)?jni_mangle\s*\(|\bextern\s+(?:"[^"\n]*"\s+)?fn\s+Java_/;
/**
 * `target_os = "android"` 的针。
 *
 * `(?:r#*)?` 收 raw string 形态：`#[cfg(target_os = r"android")]` 与不带 `r` 的**语义等价**
 * —— 两条同时写 rustc 报 E0428「重复定义」，反证它们是同一个 cfg。上一版的针只认普通字符串，
 * 换成 raw string 就绕过去了。
 */
const ANDROID_CFG = /target_os\s*=\s*(?:r#*)?"android"/;

/**
 * 文件里有没有**真代码**的 `target_os = "android"`（两个面合取，理由见文件头）。
 *
 * 命中位置在 `keep-literals` 面上找（针本身含字符串字面量，剥了字符串就永远命中不到），
 * 再要求**同一偏移**在 `code-only` 面上仍是 `target_os` 这个标识符。两档剥离器都换行守恒、
 * 只把被抹的字节换成空格 ⇒ 偏移逐字对齐，可以直接拿来合取。
 *
 * 后果：内联 Rust 夹具 `const FIXTURE: &str = r####"…#[cfg(target_os = "android")]…"####;`
 * 在 `code-only` 面上整段被抹成空格 ⇒ 不算数（那是判据被自己污染，不是漏检）。
 */
function hasRealAndroidCfg(raw: string): boolean {
  // 粗筛：剥离只删不增，原文没有的净化面上也不会有（737 个 `.rs` 全部逐字符剥两遍没有必要）。
  if (!ANDROID_CFG.test(raw)) return false;
  const keep = maskRust(raw, 'keep-literals');
  const code = maskRust(raw, 'code-only');
  for (const m of keep.matchAll(new RegExp(ANDROID_CFG.source, 'g'))) {
    if (code.startsWith('target_os', m.index ?? 0)) return true;
  }
  return false;
}

// 先在原文上过一遍正则、命中了才做词法剥离：剥离只删不增，原文没有的净化面上也不会有
// （737 个 `.rs` 全部逐字符剥离没有必要，取材面不因此变窄）。
const jniFace = RUST_FILES.filter((path) => {
  const raw = readFileSync(join(REPO_ROOT, path), 'utf8');
  if (!JNI_EXPORT.test(raw)) return false;
  return JNI_EXPORT.test(maskRust(raw, 'code-only'));
});

const androidCfgFace = [
  ...RUST_FILES.filter((path) => hasRealAndroidCfg(readFileSync(join(REPO_ROOT, path), 'utf8'))),
  ...CARGO_TOMLS.filter((path) => {
    const raw = readFileSync(join(REPO_ROOT, path), 'utf8');
    if (!ANDROID_CFG.test(raw)) return false;
    return ANDROID_CFG.test(maskTomlComments(raw));
  }),
].sort();

/*
 * ── 取材面**第二条腿已删除**（2026-09-05）──
 *
 * 那条腿扫「cfg 写在父模块 `mod` 声明上」的模块，再把 `mod foo;` 解析成 `foo.rs` / `foo/mod.rs`，
 * 问的是「哪些文件**只在 android 上编译**」。它在用正则重新实现 rustc 的模块解析与 cfg 求值，
 * 两轮里被连续绕过五种形态：已登记文件里再 `mod` 出一个子文件（两层嵌套，全新文件完全隐形）、
 * `#[cfg_attr(target_os = "android", path = "x_android.rs")]`（门点名的是**非 android 的那份**，
 * 照它自己给的修法登记之后就 fail-open）、`#[cfg(target_os = r"android")]`、`#[cfg(...)]mod foo;`
 * 零空白、以及内联夹具的 raw string 里写 `mod`（判据被自己污染）。补形态是打地鼠。
 *
 * 那个子问题整体移交 `scripts/check-android-only-face.mjs`：真值源换成 rustc 自己的
 * `--emit=dep-info`，Android 专属文件集 = android 编译面 − host 编译面，上面五种形态一个都骗不了它
 * （隔离工程实证）。挂载点是 ci.yml 的「Android impact face must be registered (dep-info 对差)」
 * 那一步，由本文件 D4 组钉住 —— **别在这里重新长出第二条正则腿**。
 */

/* ────────────────────── workflow 取材：按 step 切，而不是整文件 grep ────────────────────── */

/**
 * 把一个 workflow 切成 step（`- name:` 起、到同缩进的下一个 `- ` 或更浅的行止）。
 *
 * D2 需要的不是「文件里有没有这串」，而是「**哪一步**跑它、那一步会不会跑、那条命令有没有被中和」
 * ——三样都只有按 step 切开才问得出来。这里不引 YAML 解析器：判据只用到「步的文本块 + 它的
 * `if:` / `continue-on-error:` 两个键」，为此在 CI 契约里加一条运行时依赖不划算（简约阶梯）。
 */
type WorkflowStep = { name: string; indent: string; text: string };

function parseWorkflowSteps(yaml: string): WorkflowStep[] {
  const lines = yaml.split('\n');
  const steps: WorkflowStep[] = [];
  for (let i = 0; i < lines.length; i += 1) {
    const head = /^(\s*)- name:\s*(.*)$/.exec(lines[i]);
    if (!head) continue;
    const indent = head[1];
    const body = [lines[i]];
    for (let j = i + 1; j < lines.length; j += 1) {
      if (lines[j].trim() === '') {
        body.push(lines[j]);
        continue;
      }
      const lead = /^\s*/.exec(lines[j])![0];
      if (lead.length < indent.length) break;
      if (lead.length === indent.length && /^\s*-\s/.test(lines[j])) break;
      body.push(lines[j]);
    }
    steps.push({ name: head[2].trim(), indent, text: body.join('\n') });
  }
  return steps;
}

/** 一步里**真跑**的行：剥掉整行 YAML/shell 注释（本仓两者都是 `#` 起头）。 */
const runnableLines = (step: WorkflowStep): string[] =>
  step.text.split('\n').filter((l) => !l.trimStart().startsWith('#'));

/** 「在 android target 上 `cargo check`」这条命令的形状（包名不写进判据，由 token 断言单独判）。 */
const CARGO_CHECK_ANDROID = /\bcargo\s+check\b.*--target\s+aarch64-linux-android\b/;

/**
 * 执行到**第一条**满足 `predicate` 的命令时，`errexit` / `pipefail` 各自还在不在；
 * 那条命令不在这一步里则 `null`。
 *
 * 🔴 上一版只问「run 体里有没有 `set -e`」，那对下面这种形态全盲（2026-09-05 实测）：在目标命令
 * **之前**插一行 `set +e`、之后补一行 `echo`，YAML 合法、`set -e` 的字样还留在文件里，而多行
 * `run:` 只把**最后一条**命令的 rc 当步 rc ⇒ 那条 cargo 红了不再让步骤红，门却照样 rc=0。
 * 要判的是「执行到那条命令时 errexit 还在不在」，不是「文件里有没有这串」。
 *
 * 🔴 **`pipefail` 与 errexit 同权**（2026-09-05 补）。上一版的 token 正则是 `^([-+])([a-zA-Z]+)$`，
 * `set -euo pipefail` 里 `pipefail` 这个 token 匹配不上 ⇒ 被整个忽略。于是「把那两条 cargo 接进
 * 管道、同时把 `pipefail` 摘掉」——**恰好是 ci.yml 那一步的注释点名禁止的形态**（「JSON 用 `>`
 * 重定向、不接管道：管道会让 cargo 自己的 rc 失真」）——一条断言都碰不到：管道里 cargo 的 rc 被
 * 最后一环吞掉，errexit 看到的永远是 0，errexit 判定照样 true。
 * 故 `-o <名字>` 走通用长选项解析（`-o` 的参数是**下一个** token）：`set -euo pipefail` /
 * `set -o pipefail` / `set +o pipefail` / `set -o errexit` 四种写法同一条代码路径。
 *
 * 两个初始状态都取 `false`（比 GitHub `shell: bash` 默认的 `-eo pipefail` 严一格）：本仓这几步
 * 都显式写了 `set -euo pipefail`，把它当成必须，方向是宁可假红不可假绿。
 * 已知上限：`set` 一律按整段线性生效，不解析子 shell / 函数体 / `if` 块的作用域。
 */
function shellStateAt(
  step: WorkflowStep,
  predicate: (line: string) => boolean,
): { errexit: boolean; pipefail: boolean } | null {
  let errexit = false;
  let pipefail = false;
  for (const line of runnableLines(step)) {
    const set = /^\s*set\s+(.+)$/.exec(line);
    if (set !== null) {
      const tokens = set[1].trim().split(/\s+/);
      for (let i = 0; i < tokens.length; i += 1) {
        const flag = /^([-+])([a-zA-Z]*)$/.exec(tokens[i]);
        if (flag === null) continue; // `-o` 的选项名参数，已被下面的 `i += 1` 吃掉
        const on = flag[1] === '-';
        if (flag[2].includes('e')) errexit = on;
        if (flag[2].includes('o')) {
          i += 1;
          if (tokens[i] === 'pipefail') pipefail = on;
          else if (tokens[i] === 'errexit') errexit = on;
        }
      }
      continue;
    }
    if (predicate(line)) return { errexit, pipefail };
  }
  return null;
}

/**
 * 这一行有没有接管道。`||` 不算 —— 它由「被中和」那条断言单独判，两条各报各的原因。
 *
 * 已知上限：判的是行内**字面**的 `|`，引号里带 `|` 的命令会假红（方向是宁可假红不可假绿；
 * 本门今天点名的那三行都不含引号内管道）。
 */
const isPiped = (line: string): boolean => line.replace(/\|\|/g, '').includes('|');

const HOW_TO_FIX =
  '\n修法二选一：确实影响 APK 腿的产物 / 链接 ⇒ 写进 scripts/classify-ci-impact.mjs 的 ' +
  'ANDROID_IMPACT_SCOPES（附 why）；不影响 ⇒ 写进 NO_ANDROID_IMPACT_SCOPES，理由里必须点名它' +
  '**今天真实**的覆盖面（aarch64-linux-android 交叉 clippy / 某道源码级门 / check-android-bridge.mjs），' +
  '不许套 APP_LOGIC() 那句「三平台 fmt+clippy+build+test 覆盖」—— 对 Android 专属代码那是假话。';

describe('Android 影响面的完备性（APK 腿的触发面 fail-open 根治）', () => {
  it('A：声明了 JNI 导出的 Rust 文件必须被 ANDROID_IMPACT_SCOPES 覆盖', () => {
    // JNI 符号在 codegen / 链接期发射，而 `polaris` 在 android 上的替补是不 codegen 的
    // `cargo check`（D 组钉住那一行）⇒ 「符号发不出来」这一类只有真编真链的 APK 腿看得见。
    expect(
      jniFace,
      'JNI 导出面是空的 —— 剥离器或枚举器坏了（`src-tauri/src/android_tls.rs` 必然在面里）',
    ).toContain('src-tauri/src/android_tls.rs');

    const uncovered = jniFace.filter(
      (path) => androidRegistrationOf(path)?.table !== 'ANDROID_IMPACT_SCOPES',
    );
    expect(
      uncovered,
      '这些文件声明了 JNI 导出，却不在 ANDROID_IMPACT_SCOPES 里：\n' +
        uncovered.map((p) => `  · ${p}`).join('\n') +
        '\n改坏一个 JNI 导出（符号名漂 / cdylib 链不出来）今天只有 android.yml 的真编真链看得见，' +
        '而它不被点亮就一次都不跑。',
    ).toEqual([]);
  });

  it('B：含真代码 cfg(target_os = "android") 的每个文件都落在两张 Android 表之一', () => {
    // 🔴 本组的射程只有「**共享文件**里有 android 条件代码 ⇒ 必须有人判过一次」。
    //    「只在 android 上编译的文件」那一整类归 ci.yml 的 dep-info 对差门
    //    （`scripts/check-android-only-face.mjs`，D4 钉住），别在这里重新长出一条正则腿。
    const unregistered = androidCfgFace.filter((path) => androidRegistrationOf(path) === null);
    expect(
      unregistered,
      '这些文件含 Android 专属代码，却在 ANDROID_IMPACT_SCOPES 与 NO_ANDROID_IMPACT_SCOPES 里都查不到：\n' +
        unregistered.map((p) => `  · ${p}`).join('\n') +
        HOW_TO_FIX,
    ).toEqual([]);

    // 正面断言：面里的每一条都真的解析到了一条登记（上面那条只说「没有漏网」，
    // 面若为空它同样成立）。
    for (const path of androidCfgFace) {
      const hit = androidRegistrationOf(path);
      expect(hit, `${path} 的登记解析不出来`).not.toBeNull();
      expect(
        ['ANDROID_IMPACT_SCOPES', 'NO_ANDROID_IMPACT_SCOPES'],
        `${path} 命中的表名不认识：${hit?.table}`,
      ).toContain(hit?.table);
    }
  });

  it('C：枚举面与剥离器自检 —— 空跑 / 被注释喂饱都必须红', () => {
    // ① 枚举器还在扫整棵树（readdir 变哑 / SKIP_DIRS 写错 ⇒ 在此红，而不是让上面的 filter 恒真）
    expect(RUST_FILES.length, `扫到的 .rs 太少（${RUST_FILES.length}）—— 枚举器坏了，门在裸奔`)
      .toBeGreaterThan(400);
    expect(CARGO_TOMLS.length, `扫到的 Cargo.toml 太少（${CARGO_TOMLS.length}）`).toBeGreaterThan(10);

    // ② 「必然在面里」的已知文件（两条面各钉一个 + 一批高密度文件）
    expect(jniFace.length, 'JNI 导出面为空').toBeGreaterThan(0);
    expect(androidCfgFace.length, `android cfg 面太小（${androidCfgFace.length}）`).toBeGreaterThan(6);
    for (const known of [
      'src-tauri/src/runtime/proxy/android_bridge.rs',
      'src-tauri/src/runtime/proxy/startup.rs',
      'src-tauri/src/runtime/stats/source.rs',
      'src-tauri/Cargo.toml',
    ]) {
      expect(
        androidCfgFace,
        `枚举结果里没有 ${known} —— 要么取材面漏了整棵树，要么它真的不再含 android cfg（那么本哨兵该跟着改）`,
      ).toContain(known);
    }

    // ②b `src-tauri/src/android_tls.rs` **不该**在本面里 —— 它自己一处 `target_os` 都没有
    //     （cfg 写在 lib.rs 的 `mod android_tls;` 上）。它归 A 组（JNI 导出）与 ci.yml 的
    //     dep-info 对差门管。这条是**射程边界的正面对照**：哪天它出现在这里，说明有人又把
    //     「父模块 cfg 推导」那条正则腿加回来了（或者那个文件自己长出了 cfg，那要另判）。
    expect(
      androidCfgFace,
      'src-tauri/src/android_tls.rs 出现在了 B 组的取材面里 —— 它自己零处 target_os，' +
        '本组的针扎不到它。要么第二条正则腿被加回来了（那正是 2026-09-05 删掉的东西，' +
        '理由见文件头），要么它自己长出了 cfg（那么本对照该换样本）。',
    ).not.toContain('src-tauri/src/android_tls.rs');

    // ③ 剥离器的内联夹具：同一串分别放进行注释 / 块注释 / 字符串 / 真代码，只有真代码那份算数。
    const fixture = [
      '// #[jni::jni_mangle("a","b")]',
      '/* #[jni::jni_mangle("c","d")] */',
      'let attr = "#[jni::jni_mangle(";',
      '// #[cfg(target_os = "android")]',
      'const X: u8 = 1;',
    ].join('\n');
    expect(
      JNI_EXPORT.test(maskRust(fixture, 'code-only')),
      '剥离器被注释或字符串喂饱了：夹具里没有任何一处真代码 JNI 导出',
    ).toBe(false);
    expect(
      ANDROID_CFG.test(maskRust(fixture, 'keep-literals')),
      '注释剥离失效：夹具里的 android cfg 只出现在注释里',
    ).toBe(false);
    const positive = `${fixture}\n#[jni::jni_mangle("e","f")]\n#[cfg(target_os = "android")]\nfn f() {}`;
    expect(
      JNI_EXPORT.test(maskRust(positive, 'code-only')),
      '剥离器把真代码里的 JNI 导出一起抹掉了 —— 判据消失（不是变弱）',
    ).toBe(true);
    expect(
      ANDROID_CFG.test(maskRust(positive, 'keep-literals')),
      'keep-literals 档把字符串也抹了 —— `"android"` 这根针在自己的净化面上永远命中不到',
    ).toBe(true);

    // ③b 两个面**合取**的正反对照（[`hasRealAndroidCfg`]）。
    //     单靠 keep-literals 会被内联 Rust 夹具喂饱：`src-tauri/tests/` 下一个只含
    //     `const FIXTURE: &str = r####"…#[cfg(target_os = "android")] mod fixture_only;…"####;`
    //     的文件会被判成「含 Android 专属代码」而把门打红 —— 假红逼后人把判据改宽 = 门被磨钝。
    const inlineFixture =
      'const FIXTURE: &str = r####"\n#[cfg(target_os = "android")]\nmod fixture_only;\n"####;\n';
    expect(
      ANDROID_CFG.test(maskRust(inlineFixture, 'keep-literals')),
      '本对照失去意义：keep-literals 面上已经看不到夹具里的那一串了（剥离档位改了？）',
    ).toBe(true);
    expect(
      hasRealAndroidCfg(inlineFixture),
      '判据被自己污染：raw string 夹具里的 `target_os = "android"` 被当成了真代码。' +
        'code-only 面上那整段应当被抹成空格，合取因此不成立。',
    ).toBe(false);
    // 正面：真代码里的两种写法（普通字符串 / raw string）都必须命中。
    //      `r"android"` 与 `"android"` 语义等价（同时写会 E0428），针不认它就是一条绕过通道。
    expect(
      hasRealAndroidCfg('#[cfg(target_os = "android")]\nfn a() {}\n'),
      '真代码里的 `#[cfg(target_os = "android")]` 命中不到 —— 合取把判据判没了（不是变弱）',
    ).toBe(true);
    expect(
      hasRealAndroidCfg('#[cfg(target_os = r"android")]\nfn a() {}\n'),
      'raw string 形态 `#[cfg(target_os = r"android")]` 绕过了针 —— 它与不带 r 的语义等价' +
        '（rustc 判重复定义 E0428 反证过），针必须两种都认。',
    ).toBe(true);
    expect(
      hasRealAndroidCfg('#[cfg(target_os = "linux")]\nfn a() {}\n'),
      '针把 `target_os = "linux"` 也算进来了 —— 合取只该要求 `target_os` 是真代码，不该丢掉 "android" 这半边',
    ).toBe(false);

    // ④ 仓内真实文件的正反对照（本仓记过的「判据被自己污染」的两个实例）
    const wiring = 'src-tauri/tests/android_platform_verifier_wiring.rs';
    const wiringRaw = readFileSync(join(REPO_ROOT, wiring), 'utf8');
    expect(
      (wiringRaw.match(/jni_mangle/g) ?? []).length,
      `${wiring} 里 jni_mangle 的出现次数掉到 0 —— 本对照失去意义，先确认那道门是不是搬家了`,
    ).toBeGreaterThan(5);
    expect(
      jniFace,
      `${wiring} 被判成了 JNI 导出点 —— 它那 18 处 jni_mangle 全在注释与字符串里（第 109 行是` +
        '一条真代码行上的字符串 `let attr = "#[jni::jni_mangle(";`）。这是判据被自己污染。',
    ).not.toContain(wiring);

    const mesh = 'src-tauri/src/runtime/mesh.rs';
    const meshRaw = readFileSync(join(REPO_ROOT, mesh), 'utf8');
    expect(
      ANDROID_CFG.test(meshRaw),
      `${mesh} 原文里已经没有 target_os = "android" —— 本对照失去意义，该换一个注释假阳性样本`,
    ).toBe(true);
    expect(
      androidCfgFace,
      `${mesh} 被判成含 Android 专属代码 —— 它那处 target_os = "android" 只在行注释里，注释剥离失效了`,
    ).not.toContain(mesh);
  });

  it('D：NO_ANDROID_IMPACT_SCOPES 不是装饰，且它依赖的替补覆盖今天还在', () => {
    // ── D1：表本身 ──
    // 哨兵贴着当前值（2026-09-05 实测 11 条），只留一格余量：它守的是「表在缩水」——
    // 有人把某条判定删掉、那个文件从此两张表都查不到（而 B 组只在**文件还在**时才会红）。
    // 🔴 **加条目时要顺手把这个数抬上来**，否则它会一路退化成「> 8」那种起不到作用的下限。
    expect(
      Object.keys(NO_ANDROID_IMPACT_SCOPES).length,
      `NO_ANDROID_IMPACT_SCOPES 只剩 ${Object.keys(NO_ANDROID_IMPACT_SCOPES).length} 条（下限 10，` +
        '当前 11）—— 有判定被删掉了，先确认那个文件是真的没了，还是判定被顺手删了。',
    ).toBeGreaterThanOrEqual(10);
    for (const [key, why] of Object.entries(NO_ANDROID_IMPACT_SCOPES)) {
      expect(
        classifyImpact([key]).android,
        `NO_ANDROID_IMPACT_SCOPES['${key}'] 声称不触发 APK 腿，分类器却给它点亮了 —— 两者必须同真值` +
          '（多半是它同时落在 ANDROID_IMPACT_SCOPES 的某个前缀里，那条前缀说了算）',
      ).toBe(false);
      expect(why.length, `NO_ANDROID_IMPACT_SCOPES['${key}'] 的理由太短，等于没判`).toBeGreaterThan(40);
      expect(
        why.includes('三平台 fmt+clippy+build+test 覆盖'),
        `NO_ANDROID_IMPACT_SCOPES['${key}'] 套用了 APP_LOGIC() 那句模板 —— 对 Android 专属代码那是假话，` +
          '三平台的 build/test 走不到 `#[cfg(target_os = "android")]` 的那一支',
      ).toBe(false);
    }
    // 反向对照：没有它，上面那条「不点亮」会被「分类器对什么都不点亮」满足。
    expect(classifyImpact(['src-tauri/src/android_tls.rs']).android).toBe(true);
    expect(classifyImpact(['ui/pnpm-lock.yaml']).android).toBe(true);

    // ── D1b：别名轴两侧对称（2026-09-05）──
    // `moduleAliasesOf` 的 `foo.rs` ↔ `foo/` 此前只有桌面轴的 lookupScope 在用，Android 轴不用 ⇒
    // 已登记文件里 `mod` 出来的子文件 `android = false`，且因为桌面轴把它别名到已登记的
    // `android_tls.rs`、它也不进 `unregisteredScopes` —— 分类器那一侧完全静默。
    // 上面 NO 表那个循环（每条都必须 `android === false`）就是本条的反向对照：
    // 它不是被「分类器对什么都点亮」满足的。
    expect(
      classifyImpact(['src-tauri/src/android_tls/child.rs']).android,
      '已登记的 `src-tauri/src/android_tls.rs` 里 `mod` 出来的子文件没点亮 APK 腿 —— ' +
        'isAndroidImpact 又不走 moduleAliasesOf 了（同一张别名表，两条轴只有一条在用）。',
    ).toBe(true);
    expect(
      classifyImpact(['src-tauri/src/android_tls/deep/nested.rs']).android,
      '两层嵌套的子文件没点亮 APK 腿 —— scopeOf 把 `src-tauri/src/<entry>` 之下的整棵子树归到' +
        '同一个 scope，别名应当照样命中。',
    ).toBe(true);
    // 🔴 负向对照：别名**只查 ANDROID_IMPACT_SCOPES**，不是「凡有同名兄弟就点亮」。
    //    `crates/net-stack/` 在桌面表里已登记（⇒ 不走 fail-closed 那条路），而它在 Android 两张表里
    //    一条都没有 ⇒ 别名扎不到，必须仍是 false。没有这条，上面两条会被「别名对什么都命中」满足。
    expect(
      classifyImpact(['crates/net-stack/src/some_child.rs']).android,
      '别名把一个 ANDROID_IMPACT_SCOPES 里根本没有兄弟键的路径也点亮了 —— 别名的查表面变宽了' +
        '（它只该查 ANDROID_IMPACT_SCOPES 这一张）。',
    ).toBe(false);

    // ── D2：那些理由靠的 ci.yml 交叉腿还在，**且真的生效** ──
    //
    // 上一版这两条是纯子串存在性判定（`ci.includes('cargo check --target … -p polaris')`），
    // 对两类形态全盲：
    //   · **换了包**：`-p polaris` → `-p polaris-helper`，子串仍然命中（它是前缀），而
    //     `src-tauri/src/**` 在 android 上的唯一编译面当场消失；
    //   · **命令还在但不生效**：末尾接 `|| true`、步上挂 `continue-on-error: true`、`if: false`、
    //     或 run 体里没有 errexit（多行 `run:` 只把**最后一条**命令的 rc 当步 rc）。
    // 故改成：定位到那一步 → 断言它会跑 → 断言那条命令按 **token** 是那条命令。
    const ciRaw = readFileSync(join(REPO_ROOT, '.github/workflows/ci.yml'), 'utf8');
    const ciSteps = parseWorkflowSteps(ciRaw);
    expect(ciSteps.length, `ci.yml 只解析到 ${ciSteps.length} 个 step —— 解析器塌了，D2 在裸奔`)
      .toBeGreaterThan(10);

    // ci.yml 上跑 `cargo check --target aarch64-linux-android` 的步**恰好两个**，各有各的职责：
    //   · `-p <pkg>` 那条 = 本表理由指着的交叉编译面（`polaris` 被整包豁免出交叉 clippy 后的替补）；
    //   · `--workspace` 那条 = dep-info 对差门的 android 半边（D4 组）。
    // 少一个 / 多一个都要有人来重判，别让它退化成「反正有一条 check」。
    const androidCheckSteps = ciSteps.filter((s) =>
      runnableLines(s).some((l) => CARGO_CHECK_ANDROID.test(l)),
    );
    expect(
      androidCheckSteps.map((s) => s.name),
      'ci.yml 里跑 `cargo check --target aarch64-linux-android` 的步不是恰好两个 —— ' +
        '一条是 `-p polaris`（`polaris` 在 android 上的唯一编译面，NO_ANDROID_IMPACT_SCOPES 里 ' +
        '6 条理由都指着它），一条是 dep-info 对差门的 `--workspace`。',
    ).toHaveLength(2);
    const crossStep = androidCheckSteps.find((s) =>
      runnableLines(s).some((l) => CARGO_CHECK_ANDROID.test(l) && /\s-p\s/.test(l)),
    );
    const depInfoStep = androidCheckSteps.find((s) =>
      runnableLines(s).some((l) => CARGO_CHECK_ANDROID.test(l) && /--workspace\b/.test(l)),
    );
    expect(crossStep, '找不到带 `-p <pkg>` 的那条 android cargo check（交叉编译面没了）').toBeDefined();
    expect(depInfoStep, '找不到带 `--workspace` 的那条 android cargo check（dep-info 门的 android 半边没了）')
      .toBeDefined();
    expect(
      crossStep!.name !== depInfoStep!.name,
      '两条 android cargo check 被判成了同一步 —— 说明有人把 dep-info 门折进了交叉编译步，' +
        '两者的失败原因从此分不开（编不过 vs 有文件没登记）。',
    ).toBe(true);

    // ① 切片自检：这句只在这一步的整行注释里出现，剥干净了下面几条才是「真跑的命令」。
    expect(
      runnableLines(crossStep!).join('\n').includes('覆盖面从 workspace 成员'),
      'ci.yml 的 YAML 注释剥离失效：整行注释仍留在取材面上（判据会被自己的说明文字喂饱）',
    ).toBe(false);

    // ② 这一步真的会跑：`if` 不是常量假、没有 continue-on-error、run 体开了 errexit。
    const stepIf = /^\s*if:\s*(.*)$/m.exec(crossStep!.text)?.[1]?.trim() ?? '';
    expect(
      /^(?:false|'false'|"false"|\$\{\{\s*false\s*\}\})$/.test(stepIf),
      `ci.yml 的「${crossStep!.name}」被 \`if: ${stepIf}\` 关掉了 —— 命令还在文件里，但一次都不跑。`,
    ).toBe(false);
    expect(
      /^\s*continue-on-error:\s*(?:true|'true'|"true")\s*$/m.test(crossStep!.text),
      `ci.yml 的「${crossStep!.name}」挂了 continue-on-error: true —— 交叉编译红了也不拦合入，` +
        '那些「由 android 交叉腿覆盖」的理由随之作废。',
    ).toBe(false);
    // errexit / pipefail 都判到**用它的那一行**：在那条命令之前插 `set +e`、之后补一行 echo，
    // 「run 体里有没有 set -e」这种判法完全看不出来（见 [`shellStateAt`] 的两条 🔴）。
    const crossShell = shellStateAt(crossStep!, (l) => CARGO_CHECK_ANDROID.test(l));
    expect(
      crossShell?.errexit,
      `ci.yml 的「${crossStep!.name}」执行到那条 cargo check 时 errexit 不在（没开，或被 \`set +e\` 关掉）` +
        '—— 多行 run 只把**最后一条**命令的 rc 当作步 rc，中途那条 cargo check 红了会被静默吞掉。',
    ).toBe(true);
    expect(
      crossShell?.pipefail,
      `ci.yml 的「${crossStep!.name}」执行到那条 cargo check 时 pipefail 不在 —— 把它接进管道（\`| tee\` 之类）` +
        '之后，cargo 自己的 rc 会被管道最后一环吞掉，errexit 看到的永远是 0，这一步照样绿。',
    ).toBe(true);

    // ③ 交叉 target 真的装了。**按集合判、不按字面顺序判**：上一版钉死了三元组的书写顺序，
    //    调换顺序会假红（那是格式，不是语义）。
    const rustupLine = runnableLines(crossStep!).find((l) => /\brustup\s+target\s+add\b/.test(l));
    expect(
      rustupLine,
      `ci.yml 的「${crossStep!.name}」里没有 \`rustup target add\` —— 交叉 target 装不上，` +
        'NO_ANDROID_IMPACT_SCOPES 里「由 android 交叉 clippy 覆盖」的每一条理由当场变成假话。',
    ).toBeDefined();
    const installedTargets = rustupLine!
      .slice(rustupLine!.indexOf('rustup'))
      .split(/\s+/)
      .slice(3)
      .filter(Boolean)
      .sort();
    // 2026-09-06 多出 `aarch64-apple-ios` —— 按本条自己的要求「有人来确认它是不是该进本门的射程」，
    // 确认结论：**进射程，但不改变本表任何一条理由**。
    //   · NO_ANDROID_IMPACT_SCOPES 的理由指的是「由 android 交叉 clippy 覆盖」，那一格没动
    //     （android 的豁免面由下面 D3 逐字钉住，加 ios 腿不改变它：豁免按 target 生效，
    //     `polaris-unlock-transport` 多出的 ios 条目不进 android 那张比对面）。
    //   · ios 那一格覆盖的是另外 17 个纯 Rust 包的 iOS 编译面，`polaris`(src-tauri) 在 ios 上
    //     整包豁免且**补不了** `cargo check`（build script 撞 `xcrun` 缺席，实测），
    //     故它一条 android 理由都替代不了、也一条都不削弱。
    expect(
      installedTargets,
      `ci.yml 装的交叉 target 集合变了（现在是 [${installedTargets.join(', ')}]）。` +
        '少了 aarch64-linux-android = 本表的理由集体作废；多了一个也要有人来确认它是不是该进本门的射程。',
    ).toEqual([
      'aarch64-apple-ios',
      'aarch64-linux-android',
      'x86_64-apple-darwin',
      'x86_64-pc-windows-msvc',
    ]);

    // ④ 那条 check 按 **token** 是「check aarch64-linux-android 上的 polaris」，且没被中和。
    //    子串判定在这里恰好全盲：`-p polaris` 是 `-p polaris-helper` 的前缀。
    const checkLine = runnableLines(crossStep!).find((l) => CARGO_CHECK_ANDROID.test(l))!;
    const tokens = checkLine.trim().split(/\s+/);
    expect(
      tokens[tokens.indexOf('--target') + 1],
      `那条 cargo check 的 --target 变了：${checkLine.trim()}`,
    ).toBe('aarch64-linux-android');
    expect(
      tokens[tokens.indexOf('-p') + 1],
      'ci.yml 那条 `cargo check --target aarch64-linux-android -p <pkg>` 换了包 —— 只有 `polaris`' +
        '（src-tauri）在 android 上被整包豁免出交叉 clippy，换成别的包等于把 `src-tauri/src/**` 的' +
        'android 编译面整块摘掉，而子串判定看不出来（`-p polaris` 是 `-p polaris-helper` 的前缀）。' +
        `实际那一行：${checkLine.trim()}`,
    ).toBe('polaris');
    expect(
      /\|\||;\s*true\b|&\s*$/.test(checkLine),
      `那条 cargo check 被中和了（\`|| true\` / \`; true\` / 后台 \`&\`）：${checkLine.trim()}`,
    ).toBe(false);
    expect(
      isPiped(checkLine),
      `那条 cargo check 接了管道 —— 管道会让 cargo 自己的 rc 失真（rc 归最后一环），` +
        `errexit 与步 rc 都不再拦得住它编不过：${checkLine.trim()}`,
    ).toBe(false);

    // ── D3：android 的豁免面必须逐字等于已判过的那一份，**两种豁免形态都认** ──
    // 精准豁免掉某个 android 关键包，就等于悄悄摘掉上面那些理由里的交叉 clippy，
    // 而现有护栏（逐 target 的派生数下限、豁免条数 ≤3）两条都只挡「豁免面暴涨」。
    //
    // 🔴 上一版只数**整包**豁免（`scope === undefined`），于是 scope 型豁免可以静默摘掉覆盖面：
    // 给某个包补一条 `"scope": "tests"`（甚至任意别的 scope 值），它在 clippy 循环里的
    // `--all-targets` 就没了，而本条哨兵一声不吭 —— `crates/helper-proto/src/tests/mod.rs` 与
    // `crates/system-integration/src/dns_flush/tests/mod.rs` 两条 NO 理由点名的「唯一覆盖」正是
    // 那个 `--all-targets`。故改成把 scope 一并编进比对面：`<包名>` / `<包名>#<scope>`。
    const exempt: Record<string, { targets?: string[]; scope?: string }> = JSON.parse(
      readFileSync(join(REPO_ROOT, 'scripts/cross-target-exempt.json'), 'utf8'),
    );
    const androidExempt = Object.entries(exempt)
      .filter(([, e]) => (e.targets ?? []).includes('aarch64-linux-android'))
      .map(([name, e]) => (e.scope === undefined ? name : `${name}#${e.scope}`))
      .sort();
    expect(
      androidExempt,
      '在 aarch64-linux-android 上被豁免出交叉 clippy 的面变了（整包豁免写成 `<包名>`，' +
        'scope 型写成 `<包名>#<scope>`）。多一条 = 那个包的 Android 专属代码从此少一档 lint 检出，' +
        '而 NO_ANDROID_IMPACT_SCOPES 里指着交叉 clippy 的理由随之失真；少一条（豁免清完）是好事，' +
        '但同样要回来把这条哨兵和相关理由改一遍。',
    ).toEqual(['polaris', 'polaris-helper#tests']);
  });

  it('D4：dep-info 对差门那一步真的会跑，且两侧选择器逐字对称', () => {
    // 「哪些文件只在 android 上编译」这个子问题的**唯一**执行点是 ci.yml 里那一步
    // （`scripts/check-android-only-face.mjs`）。它被删掉 / 被 `if:` 关掉 / 命令被中和 /
    // 两侧选择器被改成不对称，本文件其余各组一条都不会红 —— 本组把「没执行」变成自曝。
    const ciRaw = readFileSync(join(REPO_ROOT, '.github/workflows/ci.yml'), 'utf8');
    const steps = parseWorkflowSteps(ciRaw);
    const gateSteps = steps.filter((s) =>
      runnableLines(s).some((l) => l.includes('scripts/check-android-only-face.mjs')),
    );
    expect(
      gateSteps.map((s) => s.name),
      'ci.yml 里跑 scripts/check-android-only-face.mjs 的步不是恰好一个 —— 0 个 = Android 专属源文件' +
        '的完备性今天没有门在守（B 组只管共享文件里的 android 条件代码，射程不重叠）；多个 = 有人复制了' +
        '这一步，两份判据会各自漂移。',
    ).toHaveLength(1);
    const gateStep = gateSteps[0];

    const gateIf = /^\s*if:\s*(.*)$/m.exec(gateStep.text)?.[1]?.trim() ?? '';
    expect(
      /^(?:false|'false'|"false"|\$\{\{\s*false\s*\}\})$/.test(gateIf),
      `ci.yml 的「${gateStep.name}」被 \`if: ${gateIf}\` 关掉了 —— 命令还在文件里，但一次都不跑。`,
    ).toBe(false);
    expect(
      /^\s*continue-on-error:\s*(?:true|'true'|"true")\s*$/m.test(gateStep.text),
      `ci.yml 的「${gateStep.name}」挂了 continue-on-error: true —— 门红了也不拦合入。`,
    ).toBe(false);

    const androidLine = runnableLines(gateStep).find((l) => CARGO_CHECK_ANDROID.test(l));
    const hostLine = runnableLines(gateStep).find(
      (l) => /\bcargo\s+check\b/.test(l) && !/--target\b/.test(l),
    );
    const nodeLine = runnableLines(gateStep).find((l) => l.includes('check-android-only-face.mjs'));
    expect(androidLine, `ci.yml 的「${gateStep.name}」里没有 android 侧的 cargo check`).toBeDefined();
    expect(hostLine, `ci.yml 的「${gateStep.name}」里没有 host 侧的 cargo check —— 只剩一侧就没有对差了`)
      .toBeDefined();

    // 三条命令逐条：执行到它们时 errexit **与 pipefail** 必须都还在，且既没被
    // `|| true` / `; true` / 后台 `&` 中和，也没有接管道。
    // 「run 体里有没有 set -e」判不出「那条命令之前被 set +e 关掉了」（见 [`shellStateAt`]）；
    // 而本步注释明写「JSON 用 `>` 重定向、**不接管道**：管道会让 cargo 自己的 rc 失真」——
    // 那句禁令在此之前没有任何一条断言在守（接管道 + 摘 pipefail，errexit 判定照样 true）。
    for (const [what, line] of [
      ['android 侧 cargo check', androidLine!],
      ['host 侧 cargo check', hostLine!],
      ['node 断言', nodeLine!],
    ] as const) {
      const shell = shellStateAt(gateStep, (l) => l === line);
      expect(
        shell?.errexit,
        `ci.yml 的「${gateStep.name}」执行到${what}那一行时 errexit 不在 —— 它红了不会让步骤红。`,
      ).toBe(true);
      expect(
        shell?.pipefail,
        `ci.yml 的「${gateStep.name}」执行到${what}那一行时 pipefail 不在 —— 一旦有人把它接进管道，` +
          '真正的 rc 会被管道最后一环吞掉（本步注释点名禁止的正是这个形态）。',
      ).toBe(true);
      expect(
        /\|\||;\s*true\b|&\s*$/.test(line),
        `ci.yml 的「${gateStep.name}」的${what}被中和了（\`|| true\` / \`; true\` / 后台 \`&\`）：${line.trim()}`,
      ).toBe(false);
      expect(
        isPiped(line),
        `ci.yml 的「${gateStep.name}」的${what}接了管道 —— 本步注释明写「JSON 用 \`>\` 重定向、不接管道：` +
          `管道会让 cargo 自己的 rc 失真」：${line.trim()}`,
      ).toBe(false);
    }
    expect(
      nodeLine!.includes('--android') && nodeLine!.includes('--host'),
      `dep-info 门的调用少了 --android / --host 之一：${nodeLine!.trim()}`,
    ).toBe(true);

    // 🔴 两侧选择器**逐字对称**。host 侧一旦比 android 侧宽（例如为省钱去复用
    //    `cargo clippy --workspace --all-targets` 的输出），`#[cfg(any(target_os = "android", test))]`
    //    的文件就会同时落进 host 的 test 单元、从差集里掉出去 —— 方向是 **fail-open**，
    //    而这个 pattern 仓里真的在用（src-tauri/src/runtime/stats/source.rs 的三个通道常量）。
    for (const [side, line] of [['android', androidLine!], ['host', hostLine!]] as const) {
      const tokens = line.trim().split(/\s+/);
      expect(
        tokens.includes('--workspace'),
        `dep-info 门的 ${side} 侧不是 \`--workspace\`：${line.trim()}` +
          '—— 两侧必须同一个选择器，且 `-p <pkg>` 会漏掉不被该包依赖的 workspace 成员。',
      ).toBe(true);
      expect(
        tokens.includes('--all-targets'),
        `dep-info 门的 ${side} 侧带了 --all-targets —— 两侧对称性被破坏（host 更宽 ⇒ 差集偏小 ⇒ ` +
          'fail-open），android 侧带它还会让 polaris-helper 撞 4 条 E0433。',
      ).toBe(false);
      expect(
        tokens.some((t) => t.startsWith('--message-format=json')),
        `dep-info 门的 ${side} 侧没有 --message-format=json… —— 取材要靠 compiler-artifact 定位本次` +
          '实际参与的编译单元；glob deps/*.d 会收进陈旧 .d（fail-open）。',
      ).toBe(true);
    }
    expect(
      hostLine!.includes('--target'),
      `dep-info 门的 host 侧带了 --target —— 它必须是宿主编译面，否则两侧算的是同一个 target：${hostLine!.trim()}`,
    ).toBe(false);
  });
});
