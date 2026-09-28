#!/usr/bin/env node
/**
 * Android 起停核桥的**跨语言契约门**：Rust 调用面 ⇄ Kotlin `@Command` / `@InvokeArg` 双向对拍。
 *
 * # 这道门守的是什么
 *
 * **不是**配置 JSON 的形状 —— Kotlin 侧根本不解析配置，它只是把字符串转手给 libbox。
 * 真正会静默漂的是**命令面**：改个 Kotlin 方法名、少个 `@InvokeArg` 字段，两侧都编得过
 * （Rust 传的是 JSON、Kotlin 靠反射查方法），**只在真机运行到那一行时才炸**
 * （`InvalidPluginMethodException` / Jackson 反序列化异常）。这与 `ui/scripts/check-ipc-args.mjs`
 * 守的 `generate_handler![]` ⇄ 前端 `invoke` 面是同一类缺陷、同一个手法：
 * **两侧源码互为真值**（不维护映射表）、**只锁 crash 族**、**带 FLOOR**（判据自己塌掉时转红而不是空跑绿）。
 *
 * # 断言总表
 *
 * 起停核桥（K4）：
 *
 * | # | 断言 | 抓的缺陷 |
 * |---|---|---|
 * | A1 | Rust 每个 `run_mobile_plugin_async::<_>("<cmd>", …)` / `call_with_budget::<_,_>(plugin, "<cmd>", …)` 的 `<cmd>`，在 Kotlin 里必有同名 `@Command fun` | Kotlin 侧改名/删方法 ⇒ 运行期 `InvalidPluginMethodException` |
 * | A2 | Rust 载荷结构体的 serde 字段名 ⊇ 对应 `@InvokeArg` 类的**非空**字段 | 少传必需键 ⇒ Jackson 反序列化炸 |
 * | A2b | 每个**有载荷**的 `@Command` 必须走 `invoke.parseArgs(<X>::class.java)`，不得用无类型的 `getArgs()`/`getRawArgs()` | 无类型取参 ⇒ A2 无从可查，等于在门上开个洞 |
 * | A3 | Kotlin 每个 `@Command` 都被 Rust 调到（反向） | Rust 侧断掉调用 ⇒ 门红 |
 * | A4 | Kotlin 源码树里**不得存在第二份配置真值**：任何 `.kt` 同时含 `"inbounds"` 与 `"outbounds"` 字面量即红 | `BootstrapConfig` 复活 / 有人又硬编一份 |
 * | A5 | FLOOR：命令数 ≥ 10、Kotlin 语料非空 | 正则塌了 ⇒ 空集合恒绿 |
 * | A10 | **回包**字段面：Kotlin 就地 `put("k")` 的键集 ⇄ Rust 回包类型的 serde 字段（多发/少必填都红）+ FLOOR ≥ 2 | 无载荷命令此前只被「名字」这一条守着 —— Kotlin 把回包键改个名、Rust 照旧读旧名，两侧都编得过，真机上「状态永远读不到」 |
 * | A12 | **回包元素**字段面：Kotlin 的 JSObject 工厂（函数名 ⇄ 同名 Rust 结构体，不维护映射表）的 `put(` 键集 ⇄ 该结构体 serde 字段（多发/少必填都红）+ FLOOR ≥ 1 | A10 只看回包顶层。回包里装数组时，元素的键在别处造 —— 改名两侧都编得过、A1/A3/A10 全绿，真机上「那一列整列是空的」 |
 * | A13 | **不经桥的起核**有配置来源：`startKernel` 交给内核的是「桥配置 ?: SystemStart.load(…)」；读的路径两段与 Rust 落盘逐字对拍；准入摘要住 noBackupFilesDir；三个用户断开入口都撤准入；开机接收器先判准入再起服务 | 系统（always-on / 开机 / 重拉）起服务时配置为 null ⇒ 自停；A1–A12 全绿 |
 * | A14 | **系统自动备份**默认关、用户可开：manifest 挂 backupAgent + fullBackupOnly + allowBackup="true"；`onFullBackup` 在开关为关/缺失时不调 super（super 恰好一处且排在闸门后）；开关住 noBackupFilesDir；缺省 = 关；插件读写的是 agent 那一份 | 不写 allowBackup ⇒ 凭据默认随 Google 云备份上传、随换机迁移带走；A1–A13 全绿 |
 * | A11 | **插件身份**三向对拍：Rust `PLUGIN_IDENTIFIER` / `PLUGIN_CLASS` ⇄ Kotlin `@TauriPlugin` 类的 package/类名 ⇄ 该文件路径；外加「常量真的被 `register_android_plugin` / `Builder::new` 用上」「`@Command` 全在插件类文件里」两条 | 改一侧不改另一侧 ⇒ 编译器与 CI 全绿（A1–A10 一条都不红、APK 打得出、verify-apk 六条判据全绿），**只在真机 setup 反射那一刻炸** |
 *
 * 数据面（K5，连接列表 + 流量统计）：
 *
 * | # | 断言 | 抓的缺陷 |
 * |---|---|---|
 * | A6 | 每个 Rust `const <PREFIX>_LIBBOX_COMMAND = "CommandX"` ⇔ Kotlin `val <prefix>Stream` 的 `addCommand(Libbox.CommandX)`（双向、逐字） | **摘掉某条 `addCommand`**（或订错通道）⇒ 那条流的数据在 Android 上安静消失 |
 * | A6b | Rust `TOPIC_SOURCE` 的 topic 集合 == 前端 `STATS_TOPIC_EVENT` 的键集合；每个 topic 声明的通道必须真的被某条 Kotlin 流订上 | 前端加了第六条 topic 而 Android 没有数据源；或某条 topic 的来源被摘掉 ⇒ 那个屏永远空着 |
 * | A7 | 已订通道对应的 `CommandClientHandler` 回调（`CommandStatus→writeStatus` / `CommandConnections→writeConnectionEvents`）必须在**那条流自己的声明块里**被 override，且方法体含 `enqueue(` | **订阅接上了、回调把数据静默扔掉** —— A6 只守订阅，守不住数据 |
 * | A8 | Kotlin 编码器 `encodeConnection` / `encodeStatus` 的顶层 `put("键")` 集合 == Rust `BridgeConnection` / `BridgeStatus` 的 serde 线上字段集 | 少发一个键 ⇒ 整条流反序列化即炸（响亮，但门更早）；多发一个 ⇒ 两侧字段面开始漂 |
 * | A9 | Rust `BridgeConnection` / `BridgeStatus` 的字段集 == 桌面 `daemon_conn_to_engine` / `daemon_status_to_engine` **映射字面量**里的键集 | 桌面加/减一个映射字段而 Android 没跟 ⇒ **同一个界面在两个平台显示不同的东西** |
 *
 * A9 是这一组里最值的一条：它把「Android 的字段语义与桌面一致」从一句承诺变成一条可执行判据，
 * 且判据取的是桌面**真正在用**的那段映射（不是类型定义 —— 类型里的字段桌面本就刻意不全映）。
 *
 * A4 是本门里**最便宜、最值**的一条：`BootstrapConfig.kt` 是 K2 留的临时最小配置，用完即删，
 * 而「删掉了」这件事此前没有任何东西守着。一条 grep 断言就把「第二份真值」这一整类挡在门外
 * ——配置的唯一来源必须是 Rust 侧 config-engine 的产出，经 `start` 命令的 `configContent` 进来。
 *
 * # 已知盲区（写出来，不假装覆盖）
 *
 * A10 只覆盖「回包在 `@Command` 方法体里就地 `put(` 出来」的命令（今天是 `checkConfig` 与
 * `vpnAuthStatus`）。`statsPoll` / `connectionsPoll` 的帧由 `StatsBridge` 的编码器造，方法体里一个
 * `put(` 都没有 —— 那一族由 A8 逐键对拍。分流按「方法体里有没有 `put(`」做，不按命令名维护一张会烂的表；
 * 代价是「本该就地造回包却一个 put 都没写」这种形态 A10 看不见（它会先在 A1/A3 或运行期显性失败）。
 *
 * 命令名若被提成 Rust 常量（`const CMD_START: &str = "start"`），A1/A3 的正则会漏掉那一条 ——
 * 但 A5 的 FLOOR 会因为命令数掉到 3 以下而转红，故失效方式是**响亮**的，不是静默的。这是刻意
 * 接受的形态：把常量解引用也做进正则，会让门本身变成需要维护的第二份 Rust 解析器。
 *
 * A11 断不了 `PLUGIN_NAME`：Kotlin 侧根本没有它的对应物（全仓 `polaris-vpn` 只出现在 Rust 那个常量里，
 * Kotlin 那条 `"polaris-vpn-auth"` 是线程名、字面上的子串巧合）。插件名只走 Rust 内部，故 A11e 只判形状。
 *
 * 无新增依赖（纯 node:fs + 正则）。挂载点：`ui` 的 `pnpm run build`（同 `check-ipc-args.mjs` 的既有做法）。
 */

import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, sep } from 'node:path';

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const ROOT = join(SCRIPT_DIR, '..');
const RUST_SRC = join(ROOT, 'src-tauri', 'src');
const KOTLIN_SRC = join(
  ROOT,
  'src-tauri',
  'gen',
  'android',
  'app',
  'src',
  'main',
  'java'
);

const failures = [];
const fail = (msg) => failures.push(msg);

/** 判据塌了就直接死，绝不「扫到 0 条于是 0 个断言全绿」。 */
function hardFail(msg) {
  console.error(`✗ check-android-bridge: ${msg}`);
  process.exit(1);
}

/** 递归收集某后缀的文件。 */
function collect(dir, ext) {
  const out = [];
  const walk = (d) => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, entry.name);
      if (entry.isDirectory()) walk(p);
      else if (entry.isFile() && entry.name.endsWith(ext)) out.push(p);
    }
  };
  walk(dir);
  return out.sort();
}

/**
 * 剥掉注释再取材：判据看的是**代码**。
 *
 * 本文件与两侧源码的头注里都逐字引用了 `run_mobile_plugin_async(` / `@Command` 这些形状，
 * 不剥就会红在自己的说明文字上 —— 那是真阳性的判定逻辑打在假阳性的目标上。
 *
 * 🔴 上一版只剥**整行**注释（`//` / `*` / `/*` 起头的行），**行尾注释喂得饱它**（2026-09-05 实测）：
 *
 *     pub(crate) fn f() {} // const PLUGIN_CLASS: &str = "Poisoned";
 *
 * 行首是代码 ⇒ 整行留在取材面上。而 A11 的 `rustStrConst()` 是在**全语料拼接**上取第一处命中，
 * 排序在前的文件里塞一行这样的注释，插件身份三向对拍量的就成了注释里的假值 —— 判据被自己污染。
 *
 * 故改成词法级：按字符扫，`//` / 块注释只在**不在字符串里**时才算注释。跨行状态三样都认：
 * 块注释（Rust 允许嵌套）、可跨行的普通字符串、raw string（`r#*"…"#*`）——本仓
 * `src-tauri/src/tray/*.rs` 与 `window_health.rs` 里的内联 JS 就是多行 raw string，
 * 按行切会把它们腰斩、连带把针一起切掉（那是**静默少取**，比假红更坏）。
 *
 * 这是 `crates/source-probe` 的 `mask_comments()` 的第三份移植（另两份：那个 crate 本体、
 * `ui/src/contracts/android-impact-coverage-contract.test.ts` 的 `maskRust(…, 'keep-literals')`）。
 * 三份的档位语义必须一致；哪天要合并，合并点是 source-probe。
 *
 * 已知上限（如实登记）：
 *  · Kotlin 的 `"""…"""` 三引号串不认（`command grep '"""'` 确认本仓 Kotlin 树今天一处都没有）；
 *  · 字符字面量与生命周期标注词法上只差一个收尾引号，判不出就当生命周期放过 —— 方向是**少剥**
 *    （那一处仍留在面上），不是误剥；
 *  · 换行守恒：被剥的字节换成空格、行数不变，报错里的行号仍然可用。
 */
function stripComments(src) {
  const out = src.split('');
  const n = src.length;
  const at = (i) => (i < n ? src.charCodeAt(i) : -1);
  const blank = (from, to) => {
    for (let k = from; k < Math.min(to, n); k += 1) if (out[k] !== '\n') out[k] = ' ';
  };
  const isIdentStart = (c) =>
    c === 0x5f || (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a) || c > 127;
  const isIdentCont = (c) => isIdentStart(c) || (c >= 0x30 && c <= 0x39);
  /** 开引号之后的 `from` 起，普通字符串的结束偏移；未闭合则到文件尾。 */
  const normalStringEnd = (from) => {
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
  /** `r` / `b` / `br` 前缀起的 raw string 结束偏移；不是 raw string 则 null。 */
  const rawStringEnd = (start) => {
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
    const found = src.indexOf(close, j);
    return found < 0 ? n : found + close.length;
  };
  /** `'` 起的字符字面量结束偏移；判不出（多半是生命周期）则 null。 */
  const charLiteralEnd = (start) => {
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
    // 逐字符扫会把 `foo_r"…"` 里的 `r"` 读成 raw string 起点、从此整段偏移错位。
    if (isIdentStart(at(i))) {
      let j = i;
      while (j < n && isIdentCont(at(j))) j += 1;
      const ident = src.slice(i, j);
      if (ident === 'r' || ident === 'b' || ident === 'br') {
        const raw = rawStringEnd(i);
        if (raw !== null) {
          i = raw;
          continue;
        }
        if ((ident === 'b' || ident === 'br') && at(j) === 0x22) {
          i = normalStringEnd(j + 1);
          continue;
        }
        if (ident === 'b' && at(j) === 0x27) {
          const end = charLiteralEnd(j);
          if (end !== null) {
            i = end;
            continue;
          }
        }
      }
      i = j;
      continue;
    }
    if (at(i) === 0x22) {
      i = normalStringEnd(i + 1);
      continue;
    }
    if (at(i) === 0x27) {
      const end = charLiteralEnd(i);
      if (end !== null) {
        i = end;
        continue;
      }
    }
    i += 1;
  }
  return out.join('');
}

/** snake_case → lowerCamelCase（对齐 serde 的 `rename_all = "camelCase"`）。 */
function camel(name) {
  const parts = name.split('_').filter(Boolean);
  if (parts.length === 0) return '';
  return (
    parts[0] +
    parts
      .slice(1)
      .map((p) => p.charAt(0).toUpperCase() + p.slice(1))
      .join('')
  );
}

// ════════════════════════════════════════════════════════════════════════════
// Rust 侧：调用面 + 载荷结构体
// ════════════════════════════════════════════════════════════════════════════

if (!existsSync(RUST_SRC)) hardFail(`Rust 源码树不存在：${RUST_SRC}`);
const rustFiles = collect(RUST_SRC, '.rs').filter(
  // 生产调用面才是真值；`tests/` 里的调用是别的东西（今天为空，写在这里是为了让射程显式）。
  (f) => !f.includes(`${sep}tests${sep}`)
);
if (rustFiles.length < 50) {
  hardFail(`Rust 语料只有 ${rustFiles.length} 个文件 —— 收集器失效，本门在裸奔`);
}

/** `{ command, payloadType, responseType, file }`。`null` 表示「无」/「不是具名裸结构体」。 */
const rustCalls = [];
/** 载荷结构体：名字 → { wireFields: string[], file } */
const rustStructs = new Map();

for (const file of rustFiles) {
  const src = stripComments(readFileSync(file, 'utf8'));

  // 调用面。rustfmt 会把长调用折成多行，故 `(` 之后允许任意空白。
  //
  // 🔴 **两种调用形状，不是二选一**（K6b 2026-09-04）：起停核桥那三处改走
  // `android_bridge::call_with_budget(plugin, "<cmd>", <载荷>, <预算>)` —— 它把调用交给一条分离
  // task 持有到底，避免超时腿 drop 掉 tauri 内部的 `oneshot::Receiver`（迟到回执会在 JNI 线程
  // `send().unwrap()` panic ⇒ 进程 abort）。数据面五处仍是裸 `run_mobile_plugin_async`。
  // 两条都留着，**不是**用新形状替换旧的：替换会让数据面那五处静默掉出取材面，而 FLOOR 只查
  // 总数、查不出「换了一批」。改动前后逐条对差过：命令 × 载荷类型两列完全相同（11 处调用、
  // 9 个命令），故本次改写没有放宽判据。
  //
  // 🟡 turbofish 的**第一个类型实参**是**回包**类型（A10 用它）。捕获组改成具名的，是为了让「命令 /
  // 载荷 / 回包」三列在正则里各自有名字 —— 位置组在加一列之后会静默错位。匹配的语言没变，
  // 改写前后 (命令 × 载荷类型) 两列逐条对差过（11 处调用、9 个命令，完全相同）。
  for (const re of [
    /run_mobile_plugin_async::<(?<resp>[^>]*)>\(\s*"(?<cmd>[A-Za-z0-9_]+)"\s*,\s*(?<payload>[A-Za-z_][A-Za-z0-9_]*)?/g,
    /call_with_budget::<(?<resp>[^,>]*)\s*,[^>]*>\(\s*[A-Za-z_][A-Za-z0-9_]*\s*,\s*"(?<cmd>[A-Za-z0-9_]+)"\s*,\s*(?<payload>[A-Za-z_][A-Za-z0-9_]*)?/g,
  ]) {
    for (const m of src.matchAll(re)) {
      const g = m.groups;
      rustCalls.push({
        command: g.cmd,
        // 紧跟命令名的那个标识符（载荷结构体名）。`()` 空载荷时匹配不到 ⇒ null。
        payloadType: g.payload ?? null,
        // 回包类型。`()` / `Batch<X>` 这类不是**具名的裸结构体**，A10 对它们不适用 ⇒ null。
        responseType: /^[A-Za-z_][A-Za-z0-9_]*$/.test(g.resp.trim()) ? g.resp.trim() : null,
        file,
      });
    }
  }

  // 载荷结构体：`#[serde(rename_all = "camelCase")]` + `struct X { a: T, b: T }`。
  // 🔴 `(?:pub…)?` 不是顺手加的：`#[serde(rename_all = "camelCase")]` 与 `struct` 之间只要隔着
  // 一个可见性修饰符（`pub` / `pub(crate)`），旧正则就匹配不到 `attr` 组 ⇒ **camelCase 静默失效**
  // ⇒ A10/A12 拿 snake_case 的字段面去比 Kotlin 的驼峰键，两边逐条对不上。
  // 失效方向是假红（吵，能查），但它逼着后人把判据改宽，那才是真代价。
  // 模块内的载荷/回包类型今天都写在函数体里（无 `pub`），跨模块用的（`InstalledApp`）必须带
  // `pub(crate)` —— 后者正是这一改要接住的形态。
  for (const m of src.matchAll(
    /(#\[serde\((?<attr>[^)]*)\)\]\s*)?(?:pub\s*(?:\([^)]*\)\s*)?)?struct\s+(?<name>[A-Za-z_][A-Za-z0-9_]*)(?:<[^>]*>)?\s*\{(?<body>[^}]*)\}/g
  )) {
    const { attr, name, body } = m.groups;
    const camelCase = (attr ?? '').includes('camelCase');
    // 第二个捕获组是字段类型（到行尾/逗号为止）——A10 用它区分「必填」与「可空」：
    // `Option<T>` 收不到也不炸，非 `Option` 收不到就是真机上的反序列化错误。
    const decls = [...body.matchAll(/(?:^|\n)\s*(?:pub\s+)?([a-z_][a-z0-9_]*)\s*:([^,\n]*)/g)];
    const fields = decls.map((f) => f[1]);
    if (fields.length === 0) continue;
    const wire = (f) => (camelCase ? camel(f) : f);
    rustStructs.set(name, {
      fields,
      wireFields: fields.map(wire),
      requiredWireFields: decls
        .filter(([, , type]) => !/^\s*Option\s*</.test(type))
        .map(([, f]) => wire(f)),
      file,
    });
  }
}

// ════════════════════════════════════════════════════════════════════════════
// Kotlin 侧：`@Command` / `@InvokeArg`
// ════════════════════════════════════════════════════════════════════════════

if (!existsSync(KOTLIN_SRC)) hardFail(`Kotlin 源码树不存在：${KOTLIN_SRC}`);
const kotlinFiles = collect(KOTLIN_SRC, '.kt');
if (kotlinFiles.length < 5) {
  hardFail(`Kotlin 语料只有 ${kotlinFiles.length} 个文件 —— 收集器失效，本门在裸奔`);
}

/** 命令名 → { body, file } */
const kotlinCommands = new Map();
/** `@InvokeArg` 类名 → { requiredFields: string[], file } */
const kotlinArgClasses = new Map();

for (const file of kotlinFiles) {
  const raw = readFileSync(file, 'utf8');
  const src = stripComments(raw);

  // A4：第二份配置真值。判据取**原文**（含注释）：一份被注释掉的整配置同样是「留在树里的第二份
  // 真值」，下一个人取消注释就复活了。
  if (raw.includes('"inbounds"') && raw.includes('"outbounds"')) {
    fail(
      `A4 ${file.slice(ROOT.length + 1)}：Kotlin 树里出现了第二份配置真值（同时含 "inbounds" 与 ` +
        `"outbounds" 字面量）。配置的唯一来源必须是 Rust 侧 config-engine 的产出，经 start 命令的 ` +
        `configContent 进来 —— 硬编一份在这里，诊断包里那份与内核实际吃的那份就会漂。`
    );
  }

  // `@Command fun name(invoke: Invoke) { … }`：从 `fun` 名字往后按大括号配平取方法体。
  for (const m of src.matchAll(/@Command\s+fun\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)) {
    const name = m[1];
    const open = src.indexOf('{', m.index + m[0].length);
    let body = '';
    if (open >= 0) {
      let depth = 0;
      for (let i = open; i < src.length; i += 1) {
        if (src[i] === '{') depth += 1;
        else if (src[i] === '}') {
          depth -= 1;
          if (depth === 0) {
            body = src.slice(open, i + 1);
            break;
          }
        }
      }
    }
    kotlinCommands.set(name, { body, file });
  }

  // `@InvokeArg class X { lateinit var a: String; var b: Int }`
  for (const m of src.matchAll(
    /@InvokeArg\s+(?:internal\s+|public\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{([^}]*)\}/g
  )) {
    const [, name, body] = m;
    const required = [...body.matchAll(/(?:lateinit\s+)?va[lr]\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^\n=]+)/g)]
      // 可空（`String?`）与带默认值的字段不是「必需键」：Jackson 收不到它们也不会炸。
      .filter(([, , type]) => !type.trim().endsWith('?'))
      .map(([, field]) => field);
    kotlinArgClasses.set(name, { requiredFields: required, file });
  }
}

// ════════════════════════════════════════════════════════════════════════════
// A5 FLOOR：判据自己塌掉时转红，而不是空跑绿
// ════════════════════════════════════════════════════════════════════════════

// start / stop / checkConfig / vpnAuthStatus（K4 起停核 + 授权状态）
// + {stats,connections}×{Open,Poll,Close}（K5 数据面）
const COMMAND_FLOOR = 10;
const rustCommandNames = new Set(rustCalls.map((c) => c.command));
if (rustCommandNames.size < COMMAND_FLOOR) {
  hardFail(
    `A5 FLOOR：Rust 调用面只解析到 ${rustCommandNames.size} 个命令（下限 ${COMMAND_FLOOR}）。` +
      `要么起停核桥的某条腿被删了，要么调用写法变了（如命令名被提成常量）——两种都不许静默放行。`
  );
}
if (kotlinCommands.size < COMMAND_FLOOR) {
  hardFail(
    `A5 FLOOR：Kotlin 只解析到 ${kotlinCommands.size} 个 @Command（下限 ${COMMAND_FLOOR}）`
  );
}

// ════════════════════════════════════════════════════════════════════════════
// A1 / A2 / A2b / A3
// ════════════════════════════════════════════════════════════════════════════

for (const call of rustCalls) {
  const kt = kotlinCommands.get(call.command);
  if (!kt) {
    fail(
      `A1 ${call.file.slice(ROOT.length + 1)}：Rust 调 "${call.command}"，Kotlin 侧没有同名 ` +
        `@Command fun —— 两侧都编得过，真机运行到那一行才 InvalidPluginMethodException。` +
        `（Kotlin 现有命令：${[...kotlinCommands.keys()].join(', ') || '无'}）`
    );
    continue;
  }

  if (call.payloadType === null) continue; // 无载荷命令（`()`）：A2/A2b 不适用

  // A2b：必须用有类型的 parseArgs 取参。
  const parsed = /invoke\.parseArgs\(\s*([A-Za-z_][A-Za-z0-9_]*)::class\.java\s*\)/.exec(kt.body);
  if (/invoke\.get(Raw)?Args\(/.test(kt.body)) {
    fail(
      `A2b ${kt.file.slice(ROOT.length + 1)}：@Command ${call.command} 用了无类型的 ` +
        `invoke.getArgs()/getRawArgs() —— A2 从此无从可查，等于在门上开个洞。`
    );
  }
  if (!parsed) {
    fail(
      `A2b ${kt.file.slice(ROOT.length + 1)}：@Command ${call.command} 有载荷（Rust 传 ` +
        `${call.payloadType}）却没有 invoke.parseArgs(X::class.java)`
    );
    continue;
  }

  const argClass = kotlinArgClasses.get(parsed[1]);
  if (!argClass) {
    fail(
      `A2 ${kt.file.slice(ROOT.length + 1)}：@Command ${call.command} 解析到 ${parsed[1]}，` +
        `但树里没有对应的 @InvokeArg class`
    );
    continue;
  }
  const rustStruct = rustStructs.get(call.payloadType);
  if (!rustStruct) {
    fail(
      `A2 ${call.file.slice(ROOT.length + 1)}：Rust 载荷 ${call.payloadType} 的结构体定义没解析到 ` +
        `—— 判据塌了，不许当成「字段都对得上」`
    );
    continue;
  }
  const missing = argClass.requiredFields.filter((f) => !rustStruct.wireFields.includes(f));
  if (missing.length > 0) {
    fail(
      `A2 命令 ${call.command}：Kotlin ${parsed[1]} 的必需字段 [${missing.join(', ')}] ` +
        `不在 Rust ${call.payloadType} 的 serde 字段面 [${rustStruct.wireFields.join(', ')}] 里 ` +
        `—— 少传必需键，Jackson 在真机上反序列化即炸。`
    );
  }
}

// A3 反向：Kotlin 有、Rust 不调 ⇒ 起停核的某条腿被断掉了。
for (const [name, kt] of kotlinCommands) {
  if (!rustCommandNames.has(name)) {
    fail(
      `A3 ${kt.file.slice(ROOT.length + 1)}：@Command ${name} 没有任何 Rust 调用点 —— ` +
        `要么 Rust 侧的调用被删了（起停核的一条腿断了、且没有任何编译错误），要么它是死代码。` +
        `（Rust 现有调用：${[...rustCommandNames].join(', ') || '无'}）`
    );
  }
}

// ════════════════════════════════════════════════════════════════════════════
// A10：**回包**字段面（Kotlin 就地构造的那些命令）
//
// A1/A3 只对拍命令**名**，A2/A2b 只对拍**入参**。一个无载荷命令（如 `vpnAuthStatus`）因此只被
// 名字这一条守着 —— 而它真正会静默漂的地方在**回包**：Kotlin 把键改成 `granted`、Rust 还在读
// `authorized`，两侧都编得过，真机上表现为「状态永远读不到」，且没有任何一条既有断言会红。
//
// 判据取两侧源码互为真值，双向：
//   · Kotlin 发的每个键都必须在 Rust 回包类型的 serde 字段面里（多发 ⇒ 两侧字段面开始漂）；
//   · Rust 回包类型的每个**必填**（非 `Option<_>`）字段都必须被 Kotlin 发出（少发 ⇒ 反序列化即炸）。
//
// **射程写明**：只覆盖「回包在 `@Command` 方法体里就地 `put(` 出来」的命令。`statsPoll` /
// `connectionsPoll` 的帧由 `StatsBridge` 的编码器造（方法体里一个 `put(` 都没有），那一族归 A8 ——
// 故这里按「方法体里有没有 `put(`」分流，而不是按命令名维护一张会烂的表。
// ════════════════════════════════════════════════════════════════════════════

/** 集合对差（A10 与数据面 A6/A8/A9 共用，故声明在两组断言之前）。 */
const sameSet = (a, b) => a.length === b.length && a.every((x) => b.includes(x));
const missing = (want, have) => want.filter((x) => !have.includes(x));

/** 一段 Kotlin 里**全部**的 `put("键")`（不分嵌套深度）。 */
function allPutKeys(block) {
  return [...block.matchAll(/put\(\s*"([A-Za-z0-9_]+)"/g)].map((m) => m[1]);
}

let inlineResponseContracts = 0;
for (const call of rustCalls) {
  if (call.responseType === null) continue;
  const kt = kotlinCommands.get(call.command);
  if (!kt) continue; // A1 已经报过了
  const keys = [...new Set(allPutKeys(kt.body))];
  if (keys.length === 0) continue; // 回包不在方法体里造（`StatsBridge` 那一族）⇒ 归 A8
  inlineResponseContracts += 1;

  const resp = rustStructs.get(call.responseType);
  if (!resp) {
    fail(
      `A10 ${call.file.slice(ROOT.length + 1)}：命令 ${call.command} 的回包类型 ` +
        `${call.responseType} 没解析到 —— 判据塌了，不许当成「字段都对得上」`
    );
    continue;
  }
  const extra = missing(keys, resp.wireFields);
  if (extra.length > 0) {
    fail(
      `A10 命令 ${call.command}：Kotlin 发的键 [${extra.join(', ')}] 不在 Rust ${call.responseType} ` +
        `的 serde 字段面 [${resp.wireFields.join(', ')}] 里 —— Rust 侧会静默丢掉它们，` +
        `两侧的回包字段面从此各说各话（改名 Kotlin 不改 Rust 就是这个形态）。`
    );
  }
  const notSent = missing(resp.requiredWireFields, keys);
  if (notSent.length > 0) {
    fail(
      `A10 命令 ${call.command}：Rust ${call.responseType} 的必填字段 [${notSent.join(', ')}] ` +
        `在 Kotlin 的 @Command 方法体里没有对应的 put("…") —— 少发必填键，真机上反序列化即炸。` +
        `（Kotlin 现发：[${keys.join(', ') || '无'}]）`
    );
  }
}
// FLOOR 用 `fail` 而不是 `hardFail`：它照样让本门 rc=1（空集合恒绿被挡住），但**不提前退出**。
// 用 hardFail 实测过一次代价：把 Kotlin 的 `@Command fun vpnAuthStatus` 改个名，A1/A3 本该点名
// 「Rust 调 vpnAuthStatus，Kotlin 侧没有同名方法」，却被这条先一步 exit 掉 —— 判据说了话，
// 但说的是**离真因最远**的那句。本条自己不为下游供数，没有「继续跑会产出垃圾」的理由。
if (inlineResponseContracts < 2) {
  fail(
    `A10 FLOOR：只对拍到 ${inlineResponseContracts} 个「就地造回包」的命令（下限 2：checkConfig 的 ` +
      `{error}、vpnAuthStatus 的 {authorized}）。要么其中一条腿被删了，要么 put(/turbofish 的解析 ` +
      `塌了 —— 两种都不许静默放行。`
  );
}

// ════════════════════════════════════════════════════════════════════════════
// 数据面（K5）：A6 / A6b / A7 / A8 / A9
//
// 取材面是**两侧源码 + 前端契约**三处，一处都不维护映射表：
//   · Rust  `runtime/stats/source.rs`   —— 通道声明（`*_LIBBOX_COMMAND`）、topic→通道表、桥载荷类型
//   · Kotlin `vpn/StatsBridge.kt`       —— 每条流订哪个通道、回调怎么落帧、帧怎么编码
//   · 前端  `domain/ipc-channels.ts`    —— 五条 topic 的事件通道表
// 唯一一张常量表是 libbox 自己的 API 事实（通道 → 回调方法名），libbox 换了那张表，Kotlin 侧
// 先编译不过。
// ════════════════════════════════════════════════════════════════════════════

/** 从 `index` 之后的第一个 `{` 起按大括号配平取块体（不含首尾大括号）。 */
function braceBlock(src, index) {
  const open = src.indexOf('{', index);
  if (open < 0) return null;
  let depth = 0;
  for (let i = open; i < src.length; i += 1) {
    if (src[i] === '{') depth += 1;
    else if (src[i] === '}') {
      depth -= 1;
      if (depth === 0) return src.slice(open + 1, i);
    }
  }
  return null;
}

/** 结构体字面量里**深度 1**的字段名（跳过 `..Default::default()` 与嵌套字面量内部）。 */
function topLevelRustFields(literalBody) {
  const segs = [];
  let depth = 0;
  let seg = '';
  for (const ch of literalBody) {
    if (ch === '{' || ch === '(' || ch === '[') depth += 1;
    else if (ch === '}' || ch === ')' || ch === ']') depth -= 1;
    if (ch === ',' && depth === 0) {
      segs.push(seg);
      seg = '';
    } else seg += ch;
  }
  segs.push(seg);
  return segs
    .map((x) => x.trim())
    .filter((x) => x && !x.startsWith('..'))
    .map((x) => (x.includes(':') ? x.slice(0, x.indexOf(':')) : x).trim())
    .filter((x) => /^[a-z_][a-z0-9_]*$/.test(x));
}

/** Kotlin 编码器里**大括号深度 0**的 `put("键")`（嵌套 `apply { … }` 里的不算顶层键）。 */
function topLevelPutKeys(block) {
  const keys = [];
  let depth = 0;
  const re = /\{|\}|put\(\s*"([A-Za-z0-9_]+)"/g;
  let m;
  while ((m = re.exec(block)) !== null) {
    if (m[0] === '{') depth += 1;
    else if (m[0] === '}') depth -= 1;
    else if (depth === 0) keys.push(m[1]);
  }
  return keys;
}

/** `fn <sig>` 的函数体里第一个 `<Literal> {` 字面量的深度 1 字段名。 */
function mappedFields(src, fnSignature, literalName) {
  const i = src.indexOf(fnSignature);
  if (i < 0) return null;
  const body = braceBlock(src, i + fnSignature.length);
  if (body === null) return null;
  const j = body.indexOf(`${literalName} {`);
  if (j < 0) return null;
  const lit = braceBlock(body, j + literalName.length);
  return lit === null ? null : topLevelRustFields(lit);
}

const rustSrcAll = rustFiles.map((f) => stripComments(readFileSync(f, 'utf8'))).join('\n');
const kotlinSrcAll = kotlinFiles.map((f) => stripComments(readFileSync(f, 'utf8'))).join('\n');

// ════════════════════════════════════════════════════════════════════════════
// A11：**插件身份**三向对拍（Rust 常量 ⇄ Kotlin package/类名 ⇄ 文件路径）
//
// `register_android_plugin(PLUGIN_IDENTIFIER, PLUGIN_CLASS)` 是**运行期反射**：Tauri 把这两个串
// 拼成 `com/polaris2/app/vpn/PolarisVpnPlugin` 去 JVM 里找类。改一侧不改另一侧 ——
// 两侧都编得过、A1–A10 全绿、APK 打得出、verify-apk 六条判据全绿，**只在真机 setup 那一刻炸**。
// 本门此前对这三个常量零覆盖（全仓提到 `PolarisVpnPlugin` 的只有 Rust 那一处常量、Kotlin 两份、
// 一句注释）。
//
// 判据**从 Rust 侧常量提取**，不在本文件里另写一份字面量：三处一起改名照常绿，只有不一致才红。
// 取材面是剥过注释的源码（`android_bridge.rs` 的头注里逐字写着 `com/polaris2/app/vpn/PolarisVpnPlugin`，
// 不剥就会拿注释当真值）。任何一个取材点找不到 ⇒ `hardFail` 自曝，绝不当成「都对得上」。
//
// 🔴 **PLUGIN_NAME 没有 Kotlin 侧对应物**（如实登记，别假装三向）：全仓 `polaris-vpn` 只出现在
// Rust 那个常量里；Kotlin 侧唯一的字面命中是 `PolarisVpnPlugin.kt` 的线程名 `"polaris-vpn-auth"`
// —— 那是**子串巧合**，拿 `kotlinSrcAll.includes(PLUGIN_NAME)` 对拍它会得到一条恒绿的假判据。
// 插件名只走 Rust 内部（`Builder::new` 与 `plugin:<name>|<cmd>` 的 IPC 路径），故这里只断言
// 它**被真的用上**（不是死常量）且形状合法。
// ════════════════════════════════════════════════════════════════════════════

/** 取一个 Rust 侧 `const <NAME>: &str = "…";` 的字面量；取不到就自曝。 */
function rustStrConst(name) {
  const m = new RegExp(`const\\s+${name}\\s*:\\s*&\\s*(?:'[A-Za-z_][A-Za-z0-9_]*\\s+)?str\\s*=\\s*"([^"]*)"`).exec(
    rustSrcAll
  );
  if (!m) {
    hardFail(
      `A11：Rust 侧取不到 \`const ${name}: &str = "…"\` —— 判据塌了，不许当成「插件身份对得上」。` +
        `它今天住在 src-tauri/src/runtime/proxy/android_bridge.rs；改了写法就把本条的取材一起改。`
    );
  }
  return m[1];
}

const PLUGIN_IDENTIFIER = rustStrConst('PLUGIN_IDENTIFIER');
const PLUGIN_CLASS = rustStrConst('PLUGIN_CLASS');
const PLUGIN_NAME = rustStrConst('PLUGIN_NAME');

// A11a：这三个常量必须是**真的被传进去**的那三个。少了这条，把常量改个名、在调用点硬写一份
// 字面量，本门照样绿 —— 判据就成了「某处有个常量」而不是「注册用的是它」。
if (!new RegExp(`register_android_plugin\\s*\\(\\s*PLUGIN_IDENTIFIER\\s*,\\s*PLUGIN_CLASS\\s*\\)`).test(rustSrcAll)) {
  hardFail(
    'A11a：Rust 侧找不到 `register_android_plugin(PLUGIN_IDENTIFIER, PLUGIN_CLASS)` —— 要么注册点没了，' +
      '要么改成了硬写字面量。后者会让本门的三向对拍从此量的是一对没人用的常量。'
  );
}
if (!/Builder::new\s*\(\s*PLUGIN_NAME\s*\)/.test(rustSrcAll)) {
  hardFail(
    'A11a：Rust 侧找不到 `Builder::new(PLUGIN_NAME)` —— PLUGIN_NAME 成了死常量，或插件名改成了硬写字面量。'
  );
}

// A11b：Kotlin 侧被反射到的那个类。取材是 `@TauriPlugin` 注解本身（Tauri 的插件入口标记），
// 不是按 PLUGIN_CLASS 去搜 —— 按名字搜就成了「找得到同名类」，找不到时还分不清是改名还是没了。
const tauriPluginClasses = [];
for (const file of kotlinFiles) {
  const src = stripComments(readFileSync(file, 'utf8'));
  if (!/@TauriPlugin\b/.test(src)) continue;
  const m = /@TauriPlugin(?:\s*\([^)]*\))?\s*(?:@\w+(?:\([^)]*\))?\s*)*(?:(?:public|internal|open|final|abstract)\s+)*class\s+([A-Za-z_][A-Za-z0-9_]*)/.exec(
    src
  );
  if (!m) {
    hardFail(
      `A11b：${file.slice(ROOT.length + 1)} 里有 @TauriPlugin 注解，却解析不到它标注的类名 —— 判据塌了。`
    );
  }
  const pkg = /(?:^|\n)\s*package\s+([A-Za-z_][A-Za-z0-9_.]*)/.exec(src);
  if (!pkg) {
    hardFail(`A11b：${file.slice(ROOT.length + 1)} 解析不到 \`package\` 声明 —— 判据塌了。`);
  }
  tauriPluginClasses.push({ file, className: m[1], pkg: pkg[1] });
}
if (tauriPluginClasses.length !== 1) {
  hardFail(
    `A11b FLOOR：Kotlin 树里 @TauriPlugin 标注的类有 ${tauriPluginClasses.length} 个（期望恰好 1）。` +
      `0 个 = 插件入口没了（真机 register_android_plugin 反射直接失败）；多于 1 个 = 本门无从判断 Rust ` +
      `那对常量指的是哪一个，得有人来重判。（找到：${tauriPluginClasses.map((c) => `${c.pkg}.${c.className}`).join(', ') || '无'}）`
  );
}
const pluginKt = tauriPluginClasses[0];
const pluginKtRel = pluginKt.file.slice(KOTLIN_SRC.length + 1).split(sep).join('/');

// A11c：类名 / package / 文件路径三样都必须与 Rust 常量逐字一致。
if (pluginKt.className !== PLUGIN_CLASS) {
  fail(
    `A11 ${pluginKtRel}：Rust \`PLUGIN_CLASS = "${PLUGIN_CLASS}"\`，而 @TauriPlugin 标注的类叫 ` +
      `\`${pluginKt.className}\` —— 两侧都编得过，真机 setup 反射时 ClassNotFound。`
  );
}
if (pluginKt.pkg !== PLUGIN_IDENTIFIER) {
  fail(
    `A11 ${pluginKtRel}：Rust \`PLUGIN_IDENTIFIER = "${PLUGIN_IDENTIFIER}"\`，而该文件的 package 是 ` +
      `\`${pluginKt.pkg}\` —— Tauri 把这两个串拼成 \`${PLUGIN_IDENTIFIER.replace(/\./g, '/')}/${PLUGIN_CLASS}\` ` +
      `去 JVM 里找类，对不上就是真机启动即失败。`
  );
}
const expectedKtPath = `${PLUGIN_IDENTIFIER.replace(/\./g, '/')}/${PLUGIN_CLASS}.kt`;
if (pluginKtRel !== expectedKtPath) {
  fail(
    `A11 插件类的文件路径是 \`${pluginKtRel}\`，按 Rust 常量应为 \`${expectedKtPath}\`` +
      `（相对 ${KOTLIN_SRC.slice(ROOT.length + 1)}）。Kotlin 不强制目录与 package 对齐，但 AGP 的源码集与` +
      `本仓的既有布局都按它走 —— 路径与 package 漂开之后，下一个人按路径找类会找错文件。`
  );
}

// A11d：`@Command` 必须全部住在那个被反射到的类的文件里。A1/A3 扫的是整棵 Kotlin 树 ——
// 把一个 `@Command` 搬到别的类里，A1/A3 照常绿，而 PluginManager 只在插件类上查方法 ⇒
// 真机 InvalidPluginMethodException。这是把 PLUGIN_CLASS 与命令面绑在一起的那条腿。
const strayCommands = [...kotlinCommands.entries()].filter(([, kt]) => kt.file !== pluginKt.file);
if (strayCommands.length > 0) {
  fail(
    `A11 这些 @Command 不在插件类 \`${PLUGIN_CLASS}\` 的文件里：` +
      strayCommands.map(([name, kt]) => `${name} @ ${kt.file.slice(ROOT.length + 1)}`).join('、') +
      ` —— PluginManager 只在 ${PLUGIN_IDENTIFIER}.${PLUGIN_CLASS} 上按名字查方法，别处的 @Command ` +
      `永远调不到（A1/A3 扫整棵树，看不出这个形态）。`
  );
}

// A11e：插件名走 `plugin:<name>|<cmd>` 的 IPC 路径 ⇒ 形状必须是 kebab（不含 `:` / `|` / 空白）。
if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(PLUGIN_NAME)) {
  fail(
    `A11 \`PLUGIN_NAME = "${PLUGIN_NAME}"\` 不是合法的 Tauri 插件名（kebab、无 \`:\` / \`|\` / 空白）——` +
      `它要拼进 \`plugin:<name>|<command>\` 的调用路径。`
  );
}

// ════════════════════════════════════════════════════════════════════════════
// A12：**回包元素**的字段面（Kotlin 的 JSObject 工厂 ⇄ 同名 Rust 结构体）
//
// A10 只对拍 `@Command` 方法体里的 `put(`，也就是回包的**顶层**。回包里装着一个数组时
// （`listInstalledApps` 的 `{"apps": [...]}`），元素的键是在别处造的 —— 而元素的键同样会漂：
// Kotlin 把 `packageName` 改成 `pkg`、Rust 照旧读 `packageName`，两侧都编得过，
// A1/A3（名字）与 A10（顶层只有一个 `apps`）全绿，真机上表现为「应用列表整列是空的」。
//
// # 配对怎么来的：**同源命名，不是映射表**
//
// Kotlin 侧的工厂函数名与 Rust 侧的结构体名是同一个词（`installedApp` ⇄ `InstalledApp`）。
// 判据据此自动配对，不维护第三份清单 —— 清单会腐烂，而腐烂时它是静默的。
// 任一侧改名 ⇒ 配对数掉到 0 ⇒ 下面的 FLOOR 转红（**响亮**地失效，不是安静地少测）。
//
// # 射程（如实登记）
//
// 取材面只有**插件类那个文件**（A11b 认领的那一份）。别处的 `.kt` 里写一个同名工厂不在面内 ——
// 那与 A10 的射程（`@Command` 方法体）是同一条边界：A11d 已经钉死 `@Command` 必须住在插件类文件里。
// 本条也只覆盖「工厂函数体里就地 `put(`」的形态；用 `JSObject(jsonString)` 之类拼出来的元素看不见。
// ════════════════════════════════════════════════════════════════════════════

/** Kotlin `fun <name>(` 的函数体（含首尾大括号）；找不到返回 null。 */
function kotlinFnBody(src, name) {
  const at = src.indexOf(`fun ${name}(`);
  if (at < 0) return null;
  const open = src.indexOf('{', at);
  if (open < 0) return null;
  let depth = 0;
  for (let i = open; i < src.length; i += 1) {
    if (src[i] === '{') depth += 1;
    else if (src[i] === '}') {
      depth -= 1;
      if (depth === 0) return src.slice(open, i + 1);
    }
  }
  return null;
}

const pluginKtSrc = stripComments(readFileSync(pluginKt.file, 'utf8'));
let elementContracts = 0;
for (const [structName, rust] of rustStructs) {
  const fnName = structName.charAt(0).toLowerCase() + structName.slice(1);
  const body = kotlinFnBody(pluginKtSrc, fnName);
  if (body === null) continue;
  const keys = [...new Set(allPutKeys(body))];
  // 同名但不造 JSObject 的普通函数不在面内（同 A10 的分流口径：按「有没有 put(」判，
  // 不按名字维护一张会烂的表）。
  if (keys.length === 0) continue;
  elementContracts += 1;

  const extra = missing(keys, rust.wireFields);
  if (extra.length > 0) {
    fail(
      `A12 ${pluginKtRel}：工厂 ${fnName}() 发的键 [${extra.join(', ')}] 不在 Rust ` +
        `${structName} 的 serde 字段面 [${rust.wireFields.join(', ')}] 里 —— Rust 侧会静默丢掉它们。`
    );
  }
  const notSent = missing(rust.requiredWireFields, keys);
  if (notSent.length > 0) {
    fail(
      `A12 ${pluginKtRel}：Rust ${structName} 的必填字段 [${notSent.join(', ')}] 在工厂 ` +
        `${fnName}() 里没有对应的 put("…") —— 少发必填键，真机上反序列化即炸。` +
        `（Kotlin 现发：[${keys.join(', ') || '无'}]）`
    );
  }
}
if (elementContracts < 1) {
  fail(
    `A12 FLOOR：一个「工厂 ⇄ 同名结构体」配对都没对拍到（下限 1：installedApp ⇄ InstalledApp）。` +
      `要么那条腿被删了，要么两侧之一改了名 —— 同源命名是本条的配对依据，改名等于把判据摘掉。`
  );
}

// ── A6：Rust 通道声明 ⇄ Kotlin `addCommand` ─────────────────────────────────
const rustChannels = new Map(); // 前缀（小写） → libbox 通道常量名
for (const m of rustSrcAll.matchAll(
  /const\s+([A-Z][A-Z0-9_]*)_LIBBOX_COMMAND\s*:\s*&str\s*=\s*"([A-Za-z0-9_]+)"/g
)) {
  rustChannels.set(m[1].toLowerCase(), m[2]);
}
if (rustChannels.size < 2) {
  hardFail(
    `A6 FLOOR：Rust 侧只解析到 ${rustChannels.size} 条 *_LIBBOX_COMMAND 通道声明（下限 2：` +
      `CommandStatus 供 stats、CommandConnections 供四条连接需求）。要么数据面的一条腿被删了，` +
      `要么声明写法变了 —— 两种都不许静默放行。`
  );
}

const kotlinStreams = new Map(); // 前缀（小写） → 该流声明块的源码
for (const m of kotlinSrcAll.matchAll(
  /val\s+([a-z][A-Za-z0-9]*)Stream\s*=\s*object\s*:\s*CommandStream\(/g
)) {
  kotlinStreams.set(m[1].toLowerCase(), braceBlock(kotlinSrcAll, m.index + m[0].length) ?? '');
}

/** libbox 的 API 事实：订了哪个通道，帧就从哪个回调进来。 */
const CHANNEL_HANDLER = {
  CommandStatus: 'writeStatus',
  CommandConnections: 'writeConnectionEvents',
};

for (const [prefix, channel] of rustChannels) {
  const block = kotlinStreams.get(prefix);
  if (block === undefined) {
    fail(
      `A6：Rust 声明 ${prefix.toUpperCase()}_LIBBOX_COMMAND = "${channel}"，Kotlin 侧却没有 ` +
        `\`val ${prefix}Stream = object : CommandStream(…)\` —— 这条 topic 在 Android 上没有供数流。` +
        `（Kotlin 现有流：${[...kotlinStreams.keys()].join(', ') || '无'}）`
    );
    continue;
  }
  const added = [...block.matchAll(/addCommand\(\s*Libbox\.([A-Za-z0-9_]+)\s*\)/g)].map((x) => x[1]);
  if (added.length !== 1 || added[0] !== channel) {
    fail(
      `A6 ${prefix}Stream：Rust 声明它订 \`${channel}\`，Kotlin 侧实际 addCommand 的是 ` +
        `[${added.join(', ') || '（一个都没有）'}] —— 订阅面与声明面漂了。摘掉 addCommand 两侧都编得过，` +
        `真机上表现为「那个屏永远空着、且没有任何错误」。`
    );
    continue;
  }
  // ── A7：订了通道，就必须真的把帧落进缓冲 ────────────────────────────────
  const handler = CHANNEL_HANDLER[channel];
  if (!handler) {
    fail(
      `A7 ${prefix}Stream：通道 \`${channel}\` 不在本门认识的 libbox 回调映射表里 ` +
        `（${Object.keys(CHANNEL_HANDLER).join(' / ')}）—— 新订了一条通道就要一并说明它的帧从哪个回调来，` +
        `否则 A7 对它是空跑。`
    );
    continue;
  }
  const at = block.indexOf(`override fun ${handler}(`);
  if (at < 0) {
    fail(
      `A7 ${prefix}Stream：订了 \`${channel}\` 却没有在这条流自己的声明块里 override ` +
        `\`${handler}\` —— 会落到基类的空实现上，帧收到即扔，且没有任何错误。`
    );
    continue;
  }
  const handlerBody = braceBlock(block, at + `override fun ${handler}(`.length) ?? '';
  if (!handlerBody.includes('enqueue(')) {
    fail(
      `A7 ${prefix}Stream：\`${handler}\` 的方法体里没有 \`enqueue(\` —— 订阅接上了，数据却没进缓冲。` +
        `这正是「门只守了订阅、没守数据」那个洞。`
    );
  }
}
for (const prefix of kotlinStreams.keys()) {
  if (!rustChannels.has(prefix)) {
    fail(
      `A6 反向：Kotlin 有 \`${prefix}Stream\`，Rust 侧却没有 ${prefix.toUpperCase()}_LIBBOX_COMMAND ` +
        `声明 —— 要么它是死代码，要么 Rust 侧的消费腿被删了。`
    );
  }
}

// ── A6b：topic 集合 ⇄ 前端 STATS_TOPIC_EVENT ────────────────────────────────
const topicSourceRaw = /const\s+TOPIC_SOURCE\s*:[^=]*=\s*&\[([\s\S]*?)\]\s*;/.exec(rustSrcAll);
if (!topicSourceRaw) {
  hardFail('A6b：Rust 侧解析不到 TOPIC_SOURCE —— 判据塌了，不许当成「topic 都有来源」');
}
const topicSource = [...topicSourceRaw[1].matchAll(/\(\s*"([a-z]+)"\s*,\s*([A-Z][A-Z0-9_]*)\s*\)/g)].map(
  (m) => [m[1], m[2].replace(/_LIBBOX_COMMAND$/, '').toLowerCase()]
);

const IPC_CHANNELS_TS = join(ROOT, 'ui', 'src', 'domain', 'ipc-channels.ts');
if (!existsSync(IPC_CHANNELS_TS)) hardFail(`A6b：前端契约文件不存在：${IPC_CHANNELS_TS}`);
const topicEventBlock = braceBlock(
  readFileSync(IPC_CHANNELS_TS, 'utf8'),
  readFileSync(IPC_CHANNELS_TS, 'utf8').indexOf('const STATS_TOPIC_EVENT')
);
if (topicEventBlock === null) {
  hardFail('A6b：前端 STATS_TOPIC_EVENT 的表体没解析到 —— 判据塌了');
}
const frontendTopics = [...topicEventBlock.matchAll(/(?:^|\n)\s*([a-z][A-Za-z0-9]*)\s*:/g)].map((m) => m[1]);
if (frontendTopics.length < 5) {
  hardFail(
    `A6b FLOOR：前端只解析到 ${frontendTopics.length} 条 topic（下限 5：stats / aggregate / ` +
      `topology / detail / closed）`
  );
}
const declaredTopics = topicSource.map(([t]) => t);
if (!sameSet(declaredTopics, frontendTopics)) {
  fail(
    `A6b：Rust TOPIC_SOURCE 的 topic 集合 [${declaredTopics.join(', ')}] 与前端 ` +
      `STATS_TOPIC_EVENT 的 [${frontendTopics.join(', ')}] 不一致。前端多出来的那条在 Android 上` +
      `没有任何数据源（屏会一直空着且不报错）；Rust 多出来的那条没有消费者。`
  );
}
for (const [topic, prefix] of topicSource) {
  if (!kotlinStreams.has(prefix)) {
    fail(
      `A6b：topic \`${topic}\` 声明由 ${prefix.toUpperCase()}_LIBBOX_COMMAND 供数，` +
        `而 Kotlin 侧没有对应的 \`${prefix}Stream\` —— 这个屏在 Android 上拿不到数据。`
    );
  }
}

// ── A8 / A9：帧字段面（Kotlin 编码器 ⇄ Rust 桥载荷 ⇄ 桌面映射字面量）────────
const FRAME_CONTRACTS = [
  {
    what: '连接',
    encoder: 'fun encodeConnection(',
    bridgeStruct: 'BridgeConnection',
    desktopFn: 'fn daemon_conn_to_engine(',
    desktopLiteral: 'SingBoxConnection',
    floor: 10,
  },
  {
    what: 'Status',
    encoder: 'fun encodeStatus(',
    bridgeStruct: 'BridgeStatus',
    desktopFn: 'fn daemon_status_to_engine(',
    desktopLiteral: 'SingBoxStatus',
    floor: 8,
  },
];

for (const c of FRAME_CONTRACTS) {
  const bridge = rustStructs.get(c.bridgeStruct);
  if (!bridge) {
    hardFail(`A8：Rust 桥载荷 ${c.bridgeStruct} 的结构体没解析到 —— 判据塌了，不许当成「字段都对得上」`);
  }
  if (bridge.fields.length < c.floor) {
    hardFail(
      `A8 FLOOR：${c.bridgeStruct} 只解析到 ${bridge.fields.length} 个字段（下限 ${c.floor}）—— 正则塌了`
    );
  }

  const at = kotlinSrcAll.indexOf(c.encoder);
  if (at < 0) {
    fail(`A8：Kotlin 侧找不到 \`${c.encoder}\` —— ${c.what}帧的编码器没了，或改了名而 Rust 侧没跟`);
  } else {
    const keys = topLevelPutKeys(braceBlock(kotlinSrcAll, at + c.encoder.length) ?? '');
    if (!sameSet(keys, bridge.wireFields)) {
      fail(
        `A8 ${c.what}帧：Kotlin 编码器发的键 [${keys.join(', ')}] 与 Rust ${c.bridgeStruct} 的线上` +
          `字段 [${bridge.wireFields.join(', ')}] 不一致（Kotlin 少发：` +
          `[${missing(bridge.wireFields, keys).join(', ') || '无'}]；Kotlin 多发：` +
          `[${missing(keys, bridge.wireFields).join(', ') || '无'}]）。少发即真机上整条流反序列化报错，` +
          `多发则两侧字段面开始漂。`
      );
    }
  }

  const desktop = mappedFields(rustSrcAll, c.desktopFn, c.desktopLiteral);
  if (desktop === null) {
    hardFail(
      `A9：解析不到桌面映射 \`${c.desktopFn}\` 里的 ${c.desktopLiteral} 字面量 —— 判据塌了`
    );
  }
  if (desktop.length < c.floor) {
    hardFail(`A9 FLOOR：桌面 ${c.desktopLiteral} 字面量只解析到 ${desktop.length} 个字段（下限 ${c.floor}）`);
  }
  if (!sameSet(bridge.fields, desktop)) {
    fail(
      `A9 ${c.what}帧：Android 桥的 ${c.bridgeStruct} 字段 [${bridge.fields.join(', ')}] 与桌面 ` +
        `${c.desktopFn.replace('fn ', '')} 实际映射的 [${desktop.join(', ')}] 不一致（Android 少映：` +
        `[${missing(desktop, bridge.fields).join(', ') || '无'}]；Android 多映：` +
        `[${missing(bridge.fields, desktop).join(', ') || '无'}]）。这两组字段决定 aggregate / detail 的` +
        `输出内容 —— 不一致就是同一个界面在两个平台显示不同的东西。`
    );
  }
}

// ════════════════════════════════════════════════════════════════════════════
// A13：**不经桥的起核**有配置来源（always-on / 开机自动连接 / 系统重拉）
//
// 缺陷形态（2026-09-25 盘点）：配置只在进程内存里，`BoxService.startKernel` 在没有桥调用时拿到 null
// ⇒ 抛错自停 ⇒ 设置页引导用户去开的「始终开启的 VPN」**一定**起不来。两侧都编得过，A1–A12 全绿。
//
//  A13a 正面：`startKernel` 交给 `startOrReloadService` 的那个配置变量，必须是
//       「桥配置 ?: SystemStart.load(…)」—— 桥那一侧取自 `VpnBridge.currentConfig()`。
//  A13b 同一份字节：`SystemStart` 读的路径 = `context.dataDir` / <子目录> / <文件名>，两段字面量与
//       Rust `lib.rs` 的 `app_config_dir()…join("<子目录>")`、`ProxyRuntime::runtime_config_path` 的
//       `join("<文件名>")` 逐字相等（Tauri Android 的 app_config_dir 就是 dataDir）。
//  A13c 存放面：准入摘要住 `noBackupFilesDir`；`SystemStart.kt` 里不许出现外部存储 / 可备份目录 API。
//  A13d 用户意图：`SystemStart.forget(` 必须出现在三个「用户要断开」的入口（插件 `stop` 命令且排在
//       幂等早退 `beginStop` 之前、BoxService 的停机广播接收器、`onRevoke`）；`SystemStart.remember(`
//       必须出现在 `startKernel` 里。
//  A13e 开机接收器：manifest 注册 `.vpn.BootReceiver` 收 BOOT_COMPLETED；`onReceive` 里
//       开关 / VPN 授权 / 准入三道判定都排在 `startForegroundService(` 之前。
//
// 取材：代码结构类断言在「剥注释 + 抹字符串内容」之后的面上判（注释或日志串里写一句
// `?: SystemStart.load(` 喂不饱它）；路径字面量类断言在「只剥注释」的面上取值。两种面的切点
// 各有一条合成输入自检（见下方 a13SelfCheck）。
// ════════════════════════════════════════════════════════════════════════════

/** 在已剥注释的源码上再把字符串字面量的**内容**抹成空格（保留引号与换行，偏移不变）。 */
function blankStrings(src) {
  const out = src.split('');
  let i = 0;
  while (i < src.length) {
    if (src[i] === '"') {
      let j = i + 1;
      while (j < src.length && src[j] !== '"') {
        if (src[j] === '\\') j += 1;
        else if (src[j] !== '\n') out[j] = ' ';
        j += 1;
      }
      for (let k = i + 1; k < Math.min(j, src.length); k += 1) if (out[k] !== '\n') out[k] = ' ';
      i = j + 1;
      continue;
    }
    i += 1;
  }
  return out.join('');
}

/** A13a 的判定本体（抽出来给自检复用）。返回 null = 通过，否则是失败原因。 */
function a13aVerdict(startKernelBody) {
  if (!startKernelBody) return '切不出 BoxService.startKernel 的函数体';
  const passed = /startOrReloadService\(\s*([A-Za-z_][A-Za-z0-9_]*)/.exec(startKernelBody);
  if (!passed) return 'startKernel 里没有 startOrReloadService(<变量>, …) 调用';
  const v = passed[1];
  const decl = new RegExp(`val\\s+${v}\\s*=\\s*([^\\n]+)`).exec(startKernelBody);
  if (!decl) return `交给内核的变量 ${v} 没有在 startKernel 里声明`;
  const m = /^([A-Za-z_][A-Za-z0-9_.()]*)\s*\?:\s*SystemStart\.load\(/.exec(decl[1].trim());
  if (!m) return `交给内核的 ${v} = ${decl[1].trim()} —— 不是「桥配置 ?: SystemStart.load(…)」，无桥调用时没有配置来源`;
  const left = m[1];
  if (left === 'VpnBridge.currentConfig()') return null;
  const leftDecl = new RegExp(`val\\s+${left}\\s*=\\s*VpnBridge\\.currentConfig\\(\\)`).exec(startKernelBody);
  return leftDecl ? null : `「?:」左侧 ${left} 不是取自 VpnBridge.currentConfig()`;
}

/** 切点自检：注释/字符串里的同形物不许喂饱 A13a；代码里的真形态必须被认出来。 */
function a13SelfCheck() {
  const code = (t) => blankStrings(stripComments(t));
  const good = kotlinFnBody(
    code('private fun startKernel() {\n val b = VpnBridge.currentConfig()\n val config = b ?: SystemStart.load(service)\n server.startOrReloadService(config, o)\n}'),
    'startKernel'
  );
  const inComment = kotlinFnBody(
    code('private fun startKernel() {\n // val config = b ?: SystemStart.load(service)\n val config = VpnBridge.currentConfig() ?: error("x")\n server.startOrReloadService(config, o)\n}'),
    'startKernel'
  );
  const inString = kotlinFnBody(
    code('private fun startKernel() {\n val config = VpnBridge.currentConfig() ?: error("?: SystemStart.load(")\n server.startOrReloadService(config, o)\n}'),
    'startKernel'
  );
  if (a13aVerdict(good) !== null) hardFail(`A13 自检：合成的正确形态没被认出来（${a13aVerdict(good)}）—— 判据塌了`);
  if (a13aVerdict(inComment) === null) hardFail('A13 自检：注释里的 `?: SystemStart.load(` 喂饱了判据 —— 剥注释失效');
  if (a13aVerdict(inString) === null) hardFail('A13 自检：字符串里的 `?: SystemStart.load(` 喂饱了判据 —— 抹字符串失效');
}
a13SelfCheck();

{
  const VPN_DIR = join(KOTLIN_SRC, 'com', 'polaris2', 'app', 'vpn');
  const readKt = (name) => {
    const f = join(VPN_DIR, name);
    if (!existsSync(f)) hardFail(`A13：找不到 ${f}`);
    const noComments = stripComments(readFileSync(f, 'utf8'));
    return { noComments, code: blankStrings(noComments) };
  };
  const box = readKt('BoxService.kt');
  const sys = readKt('SystemStart.kt');
  const boot = readKt('BootReceiver.kt');
  const plugin = { code: blankStrings(pluginKtSrc) };

  // A13a
  const startKernel = kotlinFnBody(box.code, 'startKernel');
  const a = a13aVerdict(startKernel);
  if (a !== null) fail(`A13a BoxService.startKernel：${a}（系统发起的起核会以「桥没有交来配置」自停）`);

  // A13b
  const ktConst = (name) =>
    new RegExp(`const\\s+val\\s+${name}\\s*=\\s*"([^"]+)"`).exec(sys.noComments)?.[1] ?? null;
  const ktSubdir = ktConst('RUST_CONFIG_SUBDIR');
  const ktFile = ktConst('RUST_RUNTIME_CONFIG_FILE');
  const rustRuntimePath = (() => {
    const at = rustSrcAll.indexOf('fn runtime_config_path(');
    if (at < 0) return null;
    return /\.join\(\s*"([^"]+)"\s*\)/.exec(braceBlock(rustSrcAll, at) ?? '')?.[1] ?? null;
  })();
  const rustSubdir =
    /app_config_dir\(\)\s*\.map\(\s*\|\s*p\s*\|\s*p\.join\(\s*"([^"]+)"\s*\)\s*\)/.exec(rustSrcAll)?.[1] ?? null;
  if (!ktSubdir || !ktFile) fail('A13b SystemStart.kt：抠不出 RUST_CONFIG_SUBDIR / RUST_RUNTIME_CONFIG_FILE 常量');
  if (!rustRuntimePath) fail('A13b Rust：抠不出 runtime_config_path 的 join("<文件名>") —— 判据塌了');
  if (!rustSubdir) fail('A13b Rust：抠不出 lib.rs 里 app_config_dir().map(|p| p.join("<子目录>")) —— 判据塌了');
  if (ktFile && rustRuntimePath && ktFile !== rustRuntimePath) {
    fail(`A13b 配置文件名：Kotlin 读 "${ktFile}"，Rust 写 "${rustRuntimePath}" —— 系统起核读不到 Rust 落的那一份`);
  }
  if (ktSubdir && rustSubdir && ktSubdir !== rustSubdir) {
    fail(`A13b 配置子目录：Kotlin 读 "${ktSubdir}"，Rust 写 "${rustSubdir}" —— 系统起核读不到 Rust 落的那一份`);
  }
  // 表达式体函数（`fun runtimeConfig(…): File = File(…)`）没有大括号块：取到下一个空行为止。
  const rcAt = sys.code.indexOf('fun runtimeConfig(');
  const runtimeConfigFn = rcAt < 0 ? '' : sys.code.slice(rcAt, sys.code.indexOf('\n\n', rcAt));
  if (!/context\.dataDir/.test(runtimeConfigFn) || !/RUST_CONFIG_SUBDIR/.test(runtimeConfigFn) || !/RUST_RUNTIME_CONFIG_FILE/.test(runtimeConfigFn)) {
    fail('A13b SystemStart.runtimeConfig 没有由 context.dataDir + 两个对拍常量拼出（Tauri Android 的 app_config_dir = dataDir）');
  }
  if (!/runtimeConfig\(/.test(kotlinFnBody(sys.code, 'load') ?? '')) {
    fail('A13b SystemStart.load 没有读 runtimeConfig(…) —— 读的不是 Rust 落的那一份');
  }

  // A13c
  if (!/File\(\s*context\.noBackupFilesDir\s*,\s*STARTED_DIGEST_FILE\s*\)/.test(sys.code)) {
    fail('A13c 准入摘要不在 noBackupFilesDir（它决定系统能否不经用户起隧道，不得随备份恢复到别的设备）');
  }
  for (const banned of ['getExternalFilesDir', 'externalCacheDir', 'Environment.getExternal', 'filesDir,', '.filesDir)']) {
    if (sys.code.includes(banned)) fail(`A13c SystemStart.kt 出现了 ${banned} —— 标记只许放 noBackupFilesDir`);
  }

  // A13d
  const stopCmd = kotlinFnBody(plugin.code, 'stop') ?? '';
  const forgetAt = stopCmd.indexOf('SystemStart.forget(');
  const beginStopAt = stopCmd.indexOf('VpnBridge.beginStop(');
  if (forgetAt < 0 || beginStopAt < 0 || forgetAt > beginStopAt) {
    fail('A13d 插件 stop 命令：SystemStart.forget( 缺失或排在 beginStop 的幂等早退之后 —— 用户断开后系统仍会把隧道连回去');
  }
  const receiverAt = box.code.indexOf('object : BroadcastReceiver()');
  if (!(braceBlock(box.code, receiverAt) ?? '').includes('SystemStart.forget(')) {
    fail('A13d BoxService 停机广播接收器（通知栏「断开」）里没有 SystemStart.forget(');
  }
  if (!(kotlinFnBody(box.code, 'onRevoke') ?? '').includes('SystemStart.forget(')) {
    fail('A13d BoxService.onRevoke 里没有 SystemStart.forget(');
  }
  if (!(startKernel ?? '').includes('SystemStart.remember(')) {
    fail('A13d BoxService.startKernel 里没有 SystemStart.remember( —— 经桥起核成功后从不记下，系统起核恒被拒');
  }

  // A13e
  const manifest = readFileSync(
    join(ROOT, 'src-tauri', 'gen', 'android', 'app', 'src', 'main', 'AndroidManifest.xml'),
    'utf8'
  ).replace(/<!--[\s\S]*?-->/g, '');
  const receiverTag = /<receiver[^>]*android:name="\.vpn\.BootReceiver"[\s\S]*?<\/receiver>/.exec(manifest)?.[0] ?? '';
  if (!receiverTag.includes('android.intent.action.BOOT_COMPLETED')) {
    fail('A13e manifest 没有注册 .vpn.BootReceiver 收 BOOT_COMPLETED —— 开机自动连接开关是个摆设');
  }
  if (!/android:exported="false"/.test(receiverTag)) {
    fail('A13e .vpn.BootReceiver 不是 exported="false" —— 别的应用能伪造一次开机替用户连上隧道');
  }
  const onReceive = kotlinFnBody(boot.code, 'onReceive') ?? '';
  const fgsAt = onReceive.indexOf('startForegroundService(');
  if (fgsAt < 0) fail('A13e BootReceiver.onReceive 里没有 startForegroundService(');
  for (const gate of ['SystemStart.bootAutoConnect(', 'VpnService.prepare(', 'SystemStart.load(']) {
    const at = onReceive.indexOf(gate);
    if (at < 0 || (fgsAt >= 0 && at > fgsAt)) {
      fail(`A13e BootReceiver.onReceive：准入判定 ${gate} 缺失或排在起服务之后`);
    }
  }
}

// ════════════════════════════════════════════════════════════════════════════
// A14：**系统自动备份**默认关、用户可开（2026-09-25 决策）
//
// 缺陷形态：清单不写 allowBackup ⇒ 平台默认开启 Auto Backup，`dataDir/polaris/` 下含节点凭据的
// user config 与 sing-box 运行配置随 Google 云备份上传、随换机迁移（D2D）带走。allowBackup 是静态
// 属性，做不成「用户可开」⇒ 由自定义 BackupAgent 在 `onFullBackup` 里读用户开关做运行期闸门。
// 云备份与 D2D 走的是同一个入口（`FullBackupDataOutput.getTransportFlags()` 带
// `FLAG_DEVICE_TO_DEVICE_TRANSFER`），平台自己的规则判定也在 `super.onFullBackup` 里。
//
//  A14a 清单：`<application>` 挂 `android:backupAgent`（解析到 Kotlin 树里一个 `: BackupAgent()` 的类）、
//       `fullBackupOnly="true"`（走文件级 Auto Backup），`allowBackup` 显式为 true（写 false 会把
//       「用户可开」整条堵死，且在 targetSdk ≥ 31 的部分设备上并不关 D2D）。
//  A14b 闸门：`onFullBackup` 里先有 `if (!isEnabled(this)) { … return … }`，`super.onFullBackup(`
//       **恰好一处**且排在闸门块之后（关 / 缺省 ⇒ 不调 super；开 ⇒ 调 super）。
//  A14c 存放面：开关文件 = `File(context.noBackupFilesDir, …)`（不随备份走，恢复不会拿旧值覆盖本机选择）；
//       agent 文件里不许出现 filesDir / 外部存储等可备份目录 API。
//  A14d 默认关：`isEnabled` ≡ `flagFile(context).exists()`（文件不存在 = 关）；`setEnabled` 开 ⇒ 写文件、
//       关 ⇒ 删文件。
//  A14e 同一份真值：插件 `setSystemBackup` / `systemBackupStatus` 读写的正是 agent 的 setEnabled / isEnabled。
//
// 取材：结构类断言在「剥注释 + 抹字符串内容」的面上判；切点各有合成输入自检（a14SelfCheck）。
// ════════════════════════════════════════════════════════════════════════════

/** A14b 的判定本体（抽出来给自检复用）。返回 null = 通过，否则是失败原因。 */
function a14bVerdict(onFullBackupBody) {
  if (!onFullBackupBody) return '切不出 onFullBackup 的函数体';
  const gate = /if\s*\(\s*!\s*isEnabled\(\s*this\s*\)\s*\)\s*\{/.exec(onFullBackupBody);
  if (!gate) return '没有 `if (!isEnabled(this)) {` 闸门 —— 开关为关时也会备份';
  const gateOpen = gate.index + gate[0].length - 1;
  const gateBody = braceBlock(onFullBackupBody, gateOpen);
  if (gateBody === null) return '闸门块配不平';
  if (!/\breturn\b/.test(gateBody)) return '闸门块里没有 return —— 关时仍会落到 super';
  const gateEnd = gateOpen + gateBody.length + 1;
  const supers = [...onFullBackupBody.matchAll(/super\.onFullBackup\(/g)].map((m) => m.index);
  if (supers.length !== 1) return `super.onFullBackup( 出现 ${supers.length} 处（期望恰好 1 处，排在闸门之后）`;
  if (supers[0] < gateEnd) return 'super.onFullBackup( 排在闸门块之前或之内 —— 开关为关时也会备份';
  return null;
}

/** 切点自检：注释/字符串里的闸门同形物不许喂饱 A14b；代码里的真形态必须被认出来。 */
function a14SelfCheck() {
  const code = (t) => blankStrings(stripComments(t));
  const body = (t) => kotlinFnBody(code(t), 'onFullBackup');
  const good = body('override fun onFullBackup(data: X) {\n if (!isEnabled(this)) {\n Log.i(T, "x")\n return\n }\n super.onFullBackup(data)\n}');
  const gateInComment = body('override fun onFullBackup(data: X) {\n // if (!isEnabled(this)) { return }\n super.onFullBackup(data)\n}');
  const gateInString = body('override fun onFullBackup(data: X) {\n Log.i(T, "if (!isEnabled(this)) { return }")\n super.onFullBackup(data)\n}');
  const superInGate = body('override fun onFullBackup(data: X) {\n if (!isEnabled(this)) {\n super.onFullBackup(data)\n return\n }\n}');
  if (a14bVerdict(good) !== null) hardFail(`A14 自检：合成的正确形态没被认出来（${a14bVerdict(good)}）—— 判据塌了`);
  if (a14bVerdict(gateInComment) === null) hardFail('A14 自检：注释里的闸门喂饱了判据 —— 剥注释失效');
  if (a14bVerdict(gateInString) === null) hardFail('A14 自检：字符串里的闸门喂饱了判据 —— 抹字符串失效');
  if (a14bVerdict(superInGate) === null) hardFail('A14 自检：闸门块里调 super 没被认出来 —— 位置判定失效');
}
a14SelfCheck();

{
  const manifestRaw = readFileSync(
    join(ROOT, 'src-tauri', 'gen', 'android', 'app', 'src', 'main', 'AndroidManifest.xml'),
    'utf8'
  ).replace(/<!--[\s\S]*?-->/g, '');
  const appTag = /<application\b[^>]*>/.exec(manifestRaw)?.[0] ?? '';
  if (!appTag) hardFail('A14a：manifest 里切不出 <application …> 开标签 —— 判据塌了');
  const attr = (name) => new RegExp(`android:${name}="([^"]*)"`).exec(appTag)?.[1] ?? null;

  // A14a
  const agentName = attr('backupAgent');
  if (attr('allowBackup') !== 'true') {
    fail(`A14a <application> 的 android:allowBackup = ${attr('allowBackup') ?? '（未写，平台默认开且无闸门）'}，期望显式 "true" 并由 backupAgent 做运行期闸门`);
  }
  if (attr('fullBackupOnly') !== 'true') {
    fail('A14a <application> 没有 android:fullBackupOnly="true" —— 挂了 backupAgent 后平台改走 key-value，Auto Backup 的闸门形同虚设');
  }
  let agent = null;
  if (!agentName) {
    fail('A14a <application> 没有 android:backupAgent —— 系统备份没有运行期闸门，凭据默认随备份上云 / 随换机迁移');
  } else {
    const ns = /namespace\s*=\s*"([^"]+)"/.exec(
      readFileSync(join(ROOT, 'src-tauri', 'gen', 'android', 'app', 'build.gradle.kts'), 'utf8')
    )?.[1];
    if (!ns) hardFail('A14a：build.gradle.kts 里抠不出 namespace —— 判据塌了');
    const fqcn = agentName.startsWith('.') ? `${ns}${agentName}` : agentName;
    const file = join(KOTLIN_SRC, ...fqcn.split('.')) + '.kt';
    if (!existsSync(file)) {
      fail(`A14a android:backupAgent="${agentName}" 解析到 ${file.slice(ROOT.length + 1)}，文件不存在`);
    } else {
      const noComments = stripComments(readFileSync(file, 'utf8'));
      agent = { rel: file.slice(ROOT.length + 1), cls: fqcn.split('.').pop(), noComments, code: blankStrings(noComments) };
      if (!new RegExp(`class\\s+${agent.cls}\\s*:\\s*BackupAgent\\(\\)`).test(agent.code)) {
        fail(`A14a ${agent.rel} 里没有 \`class ${agent.cls} : BackupAgent()\``);
      }
    }
  }

  if (agent) {
    // A14b
    const b = a14bVerdict(kotlinFnBody(agent.code, 'onFullBackup'));
    if (b !== null) fail(`A14b ${agent.rel} onFullBackup：${b}`);

    // A14c
    if (!/File\(\s*context\.noBackupFilesDir\s*,\s*[A-Z_]+\s*\)/.test(agent.code)) {
      fail(`A14c ${agent.rel}：开关文件不在 noBackupFilesDir —— 它会随备份走，恢复时旧值覆盖新设备上的选择`);
    }
    for (const banned of ['getExternalFilesDir', 'externalCacheDir', 'Environment.getExternal', 'filesDir,', '.filesDir)', 'dataDir', 'SharedPreferences', 'getSharedPreferences']) {
      if (agent.code.includes(banned)) fail(`A14c ${agent.rel} 出现了 ${banned} —— 开关只许放 noBackupFilesDir`);
    }

    // A14d
    const isEnabledDecl = /fun\s+isEnabled\(\s*context\s*:\s*Context\s*\)\s*:\s*Boolean\s*=\s*([^\n]+)/.exec(agent.code)?.[1]?.trim();
    if (isEnabledDecl !== 'flagFile(context).exists()') {
      fail(`A14d ${agent.rel} isEnabled = ${isEnabledDecl ?? '（找不到表达式体声明）'} —— 期望 flagFile(context).exists()（文件不存在 = 关 = 默认）`);
    }
    const setBody = kotlinFnBody(agent.code, 'setEnabled') ?? '';
    const onBranch = /if\s*\(\s*enabled\s*\)\s*\{/.exec(setBody);
    const onBody = onBranch ? braceBlock(setBody, onBranch.index + onBranch[0].length - 1) ?? '' : '';
    const offPart = onBranch ? setBody.slice(onBranch.index + onBranch[0].length + onBody.length) : '';
    if (!onBranch || !/writeText\(/.test(onBody) || !/\.delete\(\)/.test(offPart)) {
      fail(`A14d ${agent.rel} setEnabled：不是「开 ⇒ 写开关文件、关 ⇒ 删开关文件」`);
    }

    // A14e
    const plug = blankStrings(pluginKtSrc);
    if (!(kotlinFnBody(plug, 'setSystemBackup') ?? '').includes(`${agent.cls}.setEnabled(`)) {
      fail(`A14e 插件 setSystemBackup 没有写 ${agent.cls}.setEnabled( —— 设置页的开关写不到 agent 读的那一份`);
    }
    if (!(kotlinFnBody(plug, 'systemBackupStatus') ?? '').includes(`${agent.cls}.isEnabled(`)) {
      fail(`A14e 插件 systemBackupStatus 没有读 ${agent.cls}.isEnabled( —— 设置页显示的不是 agent 用的那一份`);
    }
  }
}

// ════════════════════════════════════════════════════════════════════════════
if (failures.length > 0) {
  console.error('✗ check-android-bridge：Rust ⇄ Kotlin 起停核桥契约不一致\n');
  for (const f of failures) console.error(`  · ${f}\n`);
  process.exit(1);
}
console.log(
  `✓ check-android-bridge：插件身份 ${PLUGIN_IDENTIFIER}.${PLUGIN_CLASS}（插件名 ${PLUGIN_NAME}）` +
    `⇄ ${pluginKtRel} 三向对拍通过；` +
    `${rustCommandNames.size} 个命令双向对拍通过 ` +
    `(${[...rustCommandNames].sort().join(', ')})；Kotlin 树内无第二份配置真值；` +
    `不经桥起核有配置来源（A13）；系统备份默认关且有运行期闸门（A14）；` +
    `数据面 ${rustChannels.size} 条 libbox 通道 ⇄ ${frontendTopics.length} 条前端 topic 双向对拍通过 ` +
    `(${topicSource.map(([t, p]) => `${t}←${rustChannels.get(p)}`).join(', ')})`
);
