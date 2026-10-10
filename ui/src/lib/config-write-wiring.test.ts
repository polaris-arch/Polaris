/**
 * UserConfig 写入口接线守卫 —— 钉死「**每一个**写 config 的调用点都被显式分过类」。
 *
 * # 为什么必须是源码结构守卫（逻辑单测替代不了）
 *
 * `lib/staged-config.test.ts` 那一批一直是绿的：`editRoute` 的三张表、重放、撤销、冲突全都对。
 * 而 `~/docs/polaris/design/polaris-userconfig-write-entrypoints-2026-07-29.md` 盘出来的事实是
 * **100+ 个写 config 的调用点里只有 1 个查过 `editRoute`**——判据正确与判据被调用是两件事，
 * 前者有测、后者一条门都没有。该文最后一段点名的风险正是这条：
 *
 * > 新增任何一个写入口都会**静默**漏掉闸门（没有任何门会因此转红）。
 *
 * 本文件就是那盏红灯。判据面 = 所有能把字节写进 `config.json` 的前端调用形态（下方 `PATTERNS`），
 * 每一个扫到的 `(文件, 被调方法)` 必须在 `SITES` 表里有一行，并带一个**说得出因由**的去向。
 * 表里有、树上没有 ⇒ 也红（陈旧登记同样危险：它会让人以为某个入口还在被管辖）。
 *
 * # 为什么闸门挂调用点、而不是像 commit `3db36c7` 那样挂广播回声腿
 *
 * 那份 handoff 的结论「闸门不能挂调用点」管的是**路由判定**——它举的 `SettingsNetwork.tsx:179`
 * 一个调用点跨 `mixedPort`(Class B) / `controlPort`(Class A) 两个 class，确实不能在调用点手写 if。
 * 那条已经解决了：路由判定收口在 `editRoute` 单点，设置页那 46 个调用点全部经 `useConfig().update`
 * 漏斗**按键**在运行期判（`config-patch-route.ts`），调用点一个 class 判定都不写。
 *
 * 剩下的另一半——**生成一条有标签的 `StagedEntry`**（「编辑节点 香港 IEPL 01」）——只有编辑点知道：
 * Rust 侧的收口点看得见的只是「一份新 config」，造不出「用户刚才想干什么」这条意图。
 * 所以逐入口接是必要的，代价就是本文件。
 *
 * # 守的是形态不是措辞
 *
 * 断言落在「哪个文件调了哪个写方法 / **这一处调用点**有没有被暂存闸门罩住」这类结构事实上；
 * 改注释、改文案、改变量命名不会误伤，新增/挪走一个写入口则必然转红。
 *
 * 2026-09-05：T3 的粒度从**按文件**收到**按调用点**（旧判据「该文件里有过一处 `editRoute` 分流」
 * 让同文件的第二条写腿可以整条绕过暂存而全绿，F3 线的资源删除就是这么漏的）。判据本体见下方
 * 「闸门判据（AST 侧）」的长注释。
 */
import { describe, it, expect } from 'vitest';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import * as ts from '@/test/ts-compiler';
import { aliasWriteLegs, ALIAS_FIXTURE, ALIAS_FIXTURE_LEGS } from '@/test/write-leg-aliases';

import { STAGED_CONFIG_ENABLED, editRoute } from '@/lib/staged-config';
import { USER_CONFIG_FIELDS } from '@/contracts/user-config-fields';

const SRC = fileURLToPath(new URL('..', import.meta.url));

// ─────────────────────────────── 扫描器 ───────────────────────────────

/**
 * 去掉注释、**但保留行号**（把注释体换成等量空白，不是删掉）——报错要能指回真实行。
 *
 * 两个方向都必要：本仓注释习惯逐字引用被禁的旧形态（本文件头就写着 `SettingsNetwork.tsx:179`），
 * 扫原文会被说明文字误伤；反过来只在注释里提一句 `editRoute` 就能让正向断言变绿，那是假绿。
 * `[^:]` 前瞻避免把 `https://` 当行注释切掉。
 */
function code(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/(^|[^:])\/\/.*$/gm, (m, p1: string) => p1 + ' '.repeat(m.length - p1.length));
}

/**
 * 递归收集产品代码（跳过测试自身，否则本文件里的示例串会污染扫描面）。
 *
 * `src/test/` 整个目录同样排除：那里住的是**门自己的适配层**（`ts-compiler.ts` 的 AST 适配、
 * `write-leg-aliases.ts` 的别名解析 + 合成夹具），不是产品代码。不排它会当场自污染 —— 别名夹具里
 * 逐字写着 `void plain.update({ … })`，扫进来就是 5 条查无此人的「未登记写入口」（实测过，
 * 本轮加这条排除正是被下面那条自检抓出来的）。
 * 代价是「有人把产品代码塞进 `src/test/`」会静默逃出判据面 ⇒ 由下面「适配层清单逐字对拍」兜住。
 */
const TEST_ADAPTER_DIR = join('src', 'test') + '/';

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) sourceFiles(p, out);
    // 共享谓词（`contracts/test-only-modules.ts` 头注：三道门需要同一个概念，不许各留一份拷贝）。
    // `.test-support.` 同样不进产物，且产品代码不许 import 它们（`i18n-coverage` G0-b 锁着）——
    // 把它们留在产品面上，判据会被别的判据的**锚文本**喂饱（2026-09-06 在 app-wiring ⑩/⑫ 实测过一次假绿）。
    else if (/\.tsx?$/.test(e.name) && !IS_TEST_ONLY_MODULE.test(e.name) && !p.includes(TEST_ADAPTER_DIR))
      out.push(p);
  }
  return out;
}

/**
 * 判据面 —— 前端能把字节写进 `config.json` 的**全部**形态，按 handoff 的 W-a..W-d 分组。
 *
 * `W-e`（Rust 侧自主写：自动换节点 / 备份导入 / 订阅自动更新）结构上不经前端 config 对象，
 * 且**全部不是「用户在 UI 上的编辑意图」**，故不在本守卫射程；它们由 Rust 侧自己的门管。
 */
const PATTERNS: ReadonlyArray<readonly [string, RegExp]> = [
  // W-a：设置页漏斗。只认对象字面量实参 —— `update(idx, next)` 那种同名的列表编辑器局部函数不是写 config。
  ['W-a', /(?<![.\w$])update\s*\(\s*\{/g],
  // W-b：通用 config 写（版本化整份 save / 原子 patch / 实体事务 / store 包装）。
  ['W-b', /(?:\b(?:api\.config|configApi)\s*\.\s*(?:save|patch|mutateEntities)|\b(?:saveConfig|mutateConfigEntities))\s*\(/g],
  // W-d：专用 IPC 写腿 —— 前端发的是「加一个节点」而不是「写 servers 键」，但落盘的是同一份 config。
  [
    'W-d',
    /\b(?:api\.(?:server|rules|subscription|ruleResources)|(?:server|rules|subscription|ruleResources)Api)\s*\.\s*(?:add|addBulk|createStart|update|updateServers|delete|deleteBatch|reorder|switch|registerWarp|applyWarpLicense|download|redownload|cancel|resetBuiltin|setAutoUpdate|updateAll|tailscaleLogin|tailscaleLogout|tailscaleLoginCancel)\s*\(/g,
  ],
];

interface Hit {
  readonly file: string;
  /** 被调方法的归一化写法（去空白），如 `api.server.add(`。`(文件, 方法)` 是本守卫的登记粒度。 */
  readonly callee: string;
  readonly line: number;
  /**
   * 命中点在**原文**里的字符偏移。`code()` 把注释体换成等量空白而不是删掉，故去注释文本与原文
   * 逐字符对齐 —— 这个偏移可以直接拿去和 AST 的节点区间比，无需第二套定位。T3 的「这一处调用点
   * 有没有闸门管辖」就架在它上面。
   */
  readonly index: number;
  readonly tag: string;
}

/**
 * 本门关心的 store 写方法（喂 `aliasWriteLegs` 的选择器别名那一支）。
 *
 * 只列**写 config** 的那几个：`switchServer` / `startProxy` / `stopProxy` 会失败、要回显，但它们
 * 一个字节的 config 都不写，归跨屏门 `mobile/write-failure-visibility.test.ts` 管，不进本门判据面。
 */
const STORE_CONFIG_WRITES = ['saveConfig', 'mutateConfigEntities'] as const;

/**
 * 登记粒度取 `(文件, 方法)` 而**不是** `文件:行号`：行号随任何无关编辑漂移，会把守卫变成每次改动
 * 都要重刷的噪音表（噪音表的下场是被人整体重刷，等于没门）。代价是「同一文件里第二次调同一个写方法」
 * 在**登记表**上不转红 —— 那一格由「同文件同方法归属同一去向」兜住：真要换去向必然得改这一行登记。
 * 而「第二处调用点有没有真的过闸门」不吃这个代价：T3 逐 `Hit` 判（每一处命中都带自己的 `index`）。
 */
function scan(): Hit[] {
  const hits: Hit[] = [];
  for (const p of sourceFiles(SRC)) {
    // api-client（barrel + 按域拆分的 ipc/api/ 目录）是这些方法的**定义**处，不是调用点。
    if (p.endsWith(join('ipc', 'api-client.ts')) || p.includes(join('ipc', 'api') + '/')) continue;
    const src = code(readFileSync(p, 'utf8'));
    // 绑定别名腿（句柄成员访问 / 解构改名 / 选择器改名）与静态判据面同口径，见 `@/test/write-leg-aliases`。
    const aliased: Array<readonly [string, RegExp]> = aliasWriteLegs(src, STORE_CONFIG_WRITES).map(
      (re) => ['W-a', re] as const
    );
    for (const [tag, re] of [...PATTERNS, ...aliased]) {
      for (const m of src.matchAll(re)) {
        hits.push({
          file: p.slice(SRC.length).split(/[\\/]/).join('/'),
          callee: m[0].replace(/\s+/g, ''),
          line: src.slice(0, m.index).split('\n').length,
          index: m.index,
          tag,
        });
      }
    }
  }
  return hits;
}

const FILES = sourceFiles(SRC);
const HITS = scan();

/**
 * **自曝纪律**（模块加载期就抛，不留给断言）：扫空了 / 过滤过头 ⇒ 下面每一条断言都会变成空跑恒绿，
 * 而那正是本守卫要防的那种「没有任何门会转红」。抛出来比绿着更诚实。
 */
if (FILES.length < 100) throw new Error(`接线守卫扫不到源码（只收到 ${FILES.length} 个文件）`);
if (HITS.length < 90) throw new Error(`接线守卫只扫到 ${HITS.length} 个写入口，判据面已失配`);

const SOURCE = new Map(FILES.map((p) => [p.slice(SRC.length).split(/[\\/]/).join('/'), code(readFileSync(p, 'utf8'))]));

// ─────────────────────────── 闸门判据（AST 侧） ───────────────────────────

/**
 * 「这一处 staged 调用点，是不是真的在某道暂存闸门的辖区内」——**按调用点**判，不按文件判。
 *
 * # 旧判据漏在哪
 *
 * 原 T3 断言的是「每个登记为 `staged` 的**文件**里，至少有一处 `editRoute(…) === 'staged'`」。
 * 同一文件里的第二条写腿因此可以整条绕过暂存而**没有任何门会转红**：F3 线的资源删除
 * （`mobile/screens/rules/RulesScreen.tsx` 的 `api.ruleResources.delete`）正是从这条缝里漏的
 * ——登记表写着 staged、实现直连后端、T3 全绿，靠人眼复审才抓到
 * （`~/docs/polaris/design/polaris-mobile-screen-rules-2026-09-04.md` §8-2）。
 *
 * # 为什么不能用「闸门数 vs 站点数」计数法
 *
 * 计数法会误报，本仓两个反例都是真的：
 *  - `NodeDialog.handleSubmit` 里**一道** `editRoute('servers', …) === 'staged'` 同时罩住
 *    `api.server.add(` 与 `api.server.update(` 两个 callee ⇒ 一闸两站合法；
 *  - `use-node-deletion.ts` 全文**一处 `editRoute` 都没有**，它走的是批量分区形态。
 *
 * # 闸门有两种形态，归一成同一条性质
 *
 *  - **F-逐键**：`editRoute(<键>, stagingEnabled[, op]) === 'staged'`（可先落成一个局部常量再分支）。
 *  - **F-批量**：`splitStagedOnly(…)` / `partitionNodeDeleteRoutes(…)` 产出分区，再按分区分支
 *    （`routes.staged.length > 0` / `routes.directIds.length > 0`）。
 *
 * 两者的共同性质是：**存在一个分支，它的条件表达式读到了暂存层的判定结果**。判据就断言这条性质，
 * 并要求该分支**在调用点之前**、且调用点落在**该分支所在函数**的函数体内（含嵌套回调）：
 *  - 辖区按闸门所在函数算 ⇒ 「隔壁那个 handler 有闸门」不再替这个 handler 背书（F3 那条漏法）；
 *  - 允许嵌套 ⇒ 本仓三处实腿是「先判闸门、再 `void (async () => { … })()` 发 IPC」，
 *    只认最内层函数会把它们误报成漏；
 *  - 在之前 ⇒ 闸门必须先于写发生，写在闸门前面的腿不算被管辖。
 *
 * 残余面（如实记，不假装没有）：判定「读到了暂存层结果」用的是**文件级**的派生名集合，不做作用域
 * 解析；同名局部变量理论上能骗过它。代价与收益权衡后接受 —— 真要绕过得**故意**造一个同名变量并
 * 用它开一个分支，而漏掉的那一类（隔壁函数的闸门背书）是**无意**就会发生的。
 */
const DECISION_FNS = new Set(['editRoute', 'splitStagedOnly', 'partitionNodeDeleteRoutes']);

function collectNodes(root: ts.Node, pred: (n: ts.Node) => boolean): ts.Node[] {
  const out: ts.Node[] = [];
  const walk = (n: ts.Node): void => {
    if (pred(n)) out.push(n);
    ts.forEachChild(n, walk);
  };
  walk(root);
  return out;
}

/** 被调函数的名字：`f()` → `f`；`a.b()` → `b`。走 AST ⇒ 换行写的点号照样认得。 */
function calleeNameOf(call: ts.CallExpression): string | null {
  const e = call.expression;
  if (ts.isIdentifier(e)) return e.text;
  if (ts.isPropertyAccessExpression(e) && ts.isIdentifier(e.name)) return e.name.text;
  return null;
}

const isFnLike = (n: ts.Node): boolean =>
  ts.isFunctionDeclaration(n) ||
  ts.isFunctionExpression(n) ||
  ts.isArrowFunction(n) ||
  ts.isMethodDeclaration(n);

interface Span {
  readonly start: number;
  readonly end: number;
}

interface GateMap {
  /** 文件里全部函数体的区间（用来取「最内层函数」）。 */
  readonly fns: readonly Span[];
  /** 每道闸门的位置 = 它那个**条件表达式**的结束偏移。 */
  readonly gates: readonly number[];
}

function analyzeGates(sf: ts.SourceFile): GateMap {
  const isDecisionCall = (n: ts.Node): boolean =>
    ts.isCallExpression(n) && DECISION_FNS.has(calleeNameOf(n) ?? '');
  const hasDecision = (n: ts.Node): boolean => collectNodes(n, isDecisionCall).length > 0;

  // 由决策调用派生出来的名字：`const stageRule = editRoute(…) === 'staged'`、
  // `const routes = partitionNodeDeleteRoutes(…)`。分支条件读到它们，等价于读到判定结果。
  const derived = new Set<string>();
  for (const d of collectNodes(sf, ts.isVariableDeclaration) as ts.VariableDeclaration[]) {
    if (d.initializer && ts.isIdentifier(d.name) && hasDecision(d.initializer)) derived.add(d.name.text);
  }
  const readsDecision = (test: ts.Node): boolean =>
    hasDecision(test) ||
    collectNodes(test, (n) => ts.isIdentifier(n) && derived.has(n.text)).length > 0;

  const gates: number[] = [];
  for (const n of collectNodes(sf, (x) => ts.isIfStatement(x) || ts.isConditionalExpression(x))) {
    const test = ts.isIfStatement(n)
      ? (n as ts.IfStatement).expression
      : (n as ts.ConditionalExpression).condition;
    if (readsDecision(test)) gates.push(test.end);
  }
  const fns = collectNodes(sf, isFnLike).map((n) => ({ start: n.getStart(sf), end: n.end }));
  return { fns, gates };
}

/** 偏移所处的**最内层**函数体；不在任何函数里 ⇒ null（那本身是一条违规，见自检）。 */
function innermostFn(fns: readonly Span[], at: number): Span | null {
  let best: Span | null = null;
  for (const f of fns) {
    if (f.start <= at && at < f.end && (best === null || f.end - f.start < best.end - best.start)) {
      best = f;
    }
  }
  return best;
}

/**
 * 判据本体：吐出「没有闸门管辖」的偏移。
 *
 * 写成吃 `SourceFile` 的纯函数，是为了能直接喂**合成源码**做正反对照 —— 判据自己得证明它报得出错，
 * 且报的正是那一处（同 `renderer-recovery.test.ts` 的先例）。
 */
function ungatedOffsets(sf: ts.SourceFile, offsets: readonly number[]): number[] {
  const { fns, gates } = analyzeGates(sf);
  /**
   * 每道闸门连同**它自己所在的那个函数** —— 那就是它的辖区：闸门管得到本函数体内的写腿，
   * 含嵌套在里面的回调（`void (async () => { … })()` / `runWrite(…, async () => { … })`
   * 是本仓判完闸门再发 IPC 的常见形态，三处实腿如此），但管不到隔壁那个 handler。
   * 闸门不在任何函数里（模块顶层）⇒ 不认：它的辖区会退化成整份文件，等于把旧判据请回来。
   */
  const scoped = gates
    .map((at) => ({ at, fn: innermostFn(fns, at) }))
    .filter((g): g is { at: number; fn: Span } => g.fn !== null);
  return offsets.filter((at) => {
    if (innermostFn(fns, at) === null) return true;
    return !scoped.some((g) => g.at <= at && g.fn.start <= at && at < g.fn.end);
  });
}

// ─────────────────────────────── 去向登记表 ───────────────────────────────

/**
 * - `funnel` —— 经 `useConfig().update` 漏斗，路由在漏斗里**按键**判（调用点不写任何 class 判定）。
 * - `staged` —— 调用点自身经 `editRoute(...) === 'staged'` 分流，命中即只产生一条 `StagedEntry`。
 * - `direct` —— **豁免或绕过**：Class A（键 ∉ `UserConfig`）或 W-1/W-2/W-3。理由必须点名谓词。
 * - `blocked` —— **既不豁免也不绕过，本该进暂存但现契约表达不了**。与 `direct` 分开正是为了不让
 *   「做不到」伪装成「不该做」：这几行是欠账清单，不是白名单。
 * - `primitive` —— 写原语/暂存层自身的落盘腿，闸门在它上游。
 */
type Route = 'funnel' | 'staged' | 'direct' | 'blocked' | 'primitive';

interface Site {
  readonly file: string;
  readonly callee: string;
  readonly route: Route;
  /** 为什么是这个去向。`direct` 必须点名 W-0/Class A 或 W-1/W-2/W-3；`blocked` 必须写清卡在哪。 */
  readonly why: string;
}

const SITES: readonly Site[] = [
  // ── W-a：设置子页与 DNS 资源工作区共用 `useConfig().update` 漏斗 ──
  // 漏斗按键判是硬要求而非风格：`SettingsNetwork` 的 `update({ [key]: next })` 一个调用点跨
  // `mixedPort`(Class B) / `controlPort`(Class A) 两个 class，静态判必错一半。
  ...(
    [
      'components/screens/settings/SettingsDisplay.tsx',
      'components/screens/settings/SettingsDns.tsx',
      'components/screens/settings/SettingsGeneral.tsx',
      'components/screens/settings/SettingsNetwork.tsx',
      'components/screens/settings/SettingsTun.tsx',
      'components/screens/settings/SettingsUpdate.tsx',
      'components/screens/settings/AppUpdateCard.tsx',
      'components/dialogs/DnsResourceDialog.tsx',
      'components/screens/rules/DnsPolicyWorkspace.tsx',
      'components/screens/rules/NetworkProfilePanel.tsx',
      // 移动端设置屏（F4）。走的是**同一个** `useConfig().update` 漏斗 —— 移动端没有第二套写腿，
      // 故去向与桌面那批逐字相同；分开列是因为登记粒度是「文件 + 方法」。
      'mobile/home/MobileHomeScreen.tsx',
      'mobile/settings/DisplayPage.tsx',
      'mobile/settings/DnsPage.tsx',
      'mobile/settings/GeneralPage.tsx',
      'mobile/settings/NetworkPage.tsx',
      'mobile/settings/TunPage.tsx',
      'mobile/settings/UpdatePage.tsx',
      // 移动端 DNS 服务器 / 分组表单（批 10）。与桌面 `DnsResourceDialog.tsx` 那一行是同一件事
      // 的两端：同一个 `useConfig().update` 漏斗、同一条暂存/写盘/回滚路径，呈现层重写而已。
      'mobile/forms/DnsResourceFormPanel.tsx',
      // 移动端网络场景表单（2026-09-25）。与桌面 `NetworkProfilePanel.tsx` 那一行是同一件事的两端。
      'mobile/forms/NetworkProfileFormPanel.tsx',
    ] as const
  ).map((file): Site => ({
    file,
    callee: 'update({',
    route: 'funnel',
    why: '经 useConfig().update 漏斗，逐键走 splitPatchByRoute → editRoute；调用点不做 class 判定',
  })),
  {
    // 同一个漏斗，只是解构时改了名（`const { update: updateConfig } = useConfig()`）。
    // 2026-09-05 补登记：此前 W-a 只认 `update({` 字面形态，这条腿整条在判据面之外。
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'updateConfig(',
    route: 'funnel',
    why: '经 useConfig().update 漏斗（解构改名为 updateConfig），dnsServers / dnsServerGroups / networkProfiles 逐键走 splitPatchByRoute → editRoute；调用点不做 class 判定',
  },

  // ── W-b：通用 config 写 ──
  {
    file: 'hooks/use-periodic-speed-test-fields.ts',
    callee: 'saveConfig(',
    route: 'direct',
    why: 'W-0/Class A：订阅表单里「开启全局周期测速」的就地操作，只写 periodicSpeedTestEnabled。它是原始配置的顶层键（∉ UserConfig），调度器下一拍即读到，不需要重启内核，也就没有可暂存的东西',
  },
  {
    file: 'components/screens/home/HomeScreen.tsx',
    callee: 'saveConfig(',
    route: 'direct',
    why: 'W-1 切节点（直连哨兵写 selectedServerId）+ W-2 系统代理接管（proxyModeType 有 OS 活态回读）；同批被 applyFakeIpTunEntry 连带纠正的 dnsConfig 是这次原子模式切换的一半，拆开暂存会让 UI 与 OS 活态分叉',
  },
  {
    file: 'components/screens/logs/LogsScreen.tsx',
    callee: 'saveConfig(',
    route: 'staged',
    why: 'logLevel 是 UserConfig 字段（Class B，喂 sing-box log.level 需重启核）',
  },
  {
    file: 'mobile/connections/MobileConnectionsScreen.tsx',
    callee: 'saveConfig(',
    route: 'staged',
    why: '同桌面日志屏：logLevel 是 UserConfig 字段（Class B）；移动端日志分段的级别选择走同一条 editRoute 闸门',
  },
  {
    file: 'components/screens/rules/RulesScreen.tsx',
    callee: 'saveConfig(',
    route: 'staged',
    why: 'regionRouting 是 UserConfig 字段（Class B）',
  },
  // ── 移动端「规则」屏（四合一）：与桌面同名函数逐条同形，去向判定因此逐条相同 ──
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'saveConfig(',
    route: 'staged',
    why: 'regionRouting 与 appRoutingEnabled 都是 UserConfig 字段（Class B）；两处标量均经原子顶层 patch 提交',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'mutateConfigEntities(',
    route: 'staged',
    why: 'appRules Class B（应用行策略选择器落一档）；按 appId 寻址的实体事务，与桌面 AppPolicyScreen 同一条腿、同一个去向',
  },
  {
    file: 'mobile/forms/AppAddPanel.tsx',
    callee: 'mutateConfigEntities(',
    route: 'staged',
    why: 'customAppPresets Class B（新增一个自定义应用预设）；调用点自身经 editRoute(\'customAppPresets\', stagingEnabled) 分流，与桌面 AppAddDialog 同一条腿、同一个去向',
  },
  {
    file: 'mobile/forms/ResourceAddPanel.tsx',
    callee: 'api.ruleResources.download(',
    route: 'direct',
    why: 'W-3：真下载并把 .srs 落盘，「重置」删不掉已经写下的文件；与桌面 ResCatalogDialog / ResUrlDialog 两条同一口径（本文件同时承载目录多选与按 URL 单条两支）',
  },
  {
    file: 'components/screens/settings/use-config.ts',
    callee: 'configApi.patch(',
    route: 'funnel',
    why: '漏斗自身的直写腿：只把 direct 顶层字段原子合入后端最新配置',
  },
  {
    file: 'store/app-store.ts',
    callee: 'api.config.patch(',
    route: 'primitive',
    why: '即时补丁原语（saveConfig 本体），只提交调用点明确变动的顶层字段',
  },
  {
    file: 'store/app-store.ts',
    callee: 'api.config.mutateEntities(',
    route: 'primitive',
    why: '集合实体事务原语：在后端最新数组上按主键提交，并让同批多实体全成或全不成',
  },
  {
    file: 'store/app-store.ts',
    callee: 'saveConfig(',
    route: 'direct',
    why: 'W-1 分流模式切换复用即时顶层 patch，并由主页 routingBusy 保持用户动作顺序',
  },
  {
    file: 'store/staged-config-store.ts',
    callee: 'api.config.save(',
    route: 'primitive',
    why: '暂存层自己的「保存」腿：把 staged 重放到磁盘现值后落盘',
  },
  {
    file: 'tray/TrayMenu.tsx',
    callee: 'api.config.patch(',
    route: 'direct',
    why: 'W-1 模式/出口与 W-2 系统代理接管：只提交本动作字段；托盘虽是独立 webview，未提交字段仍由后端锁内保留',
  },

  {
    file: 'components/dialogs/AppAddDialog.tsx',
    callee: 'mutateConfigEntities(',
    route: 'staged',
    why: 'customAppPresets Class B；暂存关闭时仍按 id 在后端最新集合上追加，避免旧数组覆盖',
  },
  {
    file: 'components/screens/app-policy/AppPolicyScreen.tsx',
    callee: 'mutateConfigEntities(',
    route: 'staged',
    why: 'appRules / customAppPresets 均为 Class B；删除预设与关联规则在同一实体事务内原子提交',
  },
  {
    file: 'components/screens/app-policy/AppPolicyScreen.tsx',
    callee: 'saveConfig(',
    route: 'staged',
    why: 'appRoutingEnabled 是 Class B；暂存关闭时标量经原子顶层 patch 提交',
  },

  // 旧 W-c（裸 setValue/updateMode）已清零：标量统一走 patch，集合统一走实体事务。

  // ── W-d：专用 IPC 写腿 —— servers 族 ──
  {
    file: 'components/dialogs/NodeDialog.tsx',
    callee: 'api.server.add(',
    route: 'staged',
    why: 'servers 是 UserConfig 首字段（Class B），表单提交的就是整个 ServerConfig，天然满足重放要求的幂等整体替换',
  },
  {
    file: 'components/dialogs/NodeDialog.tsx',
    callee: 'api.server.update(',
    route: 'staged',
    why: 'servers Class B；编辑提交的同样是整个 ServerConfig（base 起底保全非模型字段）',
  },
  {
    file: 'components/dialogs/WgDialog.tsx',
    callee: 'api.server.add(',
    route: 'staged',
    why: 'servers Class B；手填 / 粘贴 .conf 两条来源都无远端副作用',
  },
  {
    file: 'components/dialogs/WgDialog.tsx',
    callee: 'api.server.update(',
    route: 'staged',
    why: 'servers Class B；编辑 WG 节点同样无远端副作用',
  },
  {
    file: 'components/dialogs/ImportDialog.tsx',
    callee: 'api.server.addBulk(',
    route: 'staged',
    why: 'servers Class B；解析在 localImport.parse 已完成，addBulk 本身是纯 servers 写 ⇒ 逐节点一条条目',
  },
  {
    file: 'components/dialogs/TsSettingsDialog.tsx',
    callee: 'api.server.update(',
    route: 'staged',
    why: 'servers Class B；写的是该节点的 tailscaleSettings，没有远端副作用（弹窗里从活态回读的只是出口候选列表，不是被写的字段）。'
      + '「清除 Auth Key」复用本条腿：它交出的 next 与保存完全同形，只是少了 authKey 这个键（buildTsSettings 的 clearAuthKey 入参），'
      + '本地删一个 config 键没有任何远端副作用 ⇒ 与其余字段同去向，不另开写入口',
  },
  {
    // cloneServer（含这个调用点）2026-08-30 随 5B 拆分外提到 use-node-actions.ts，登记表跟着落点走。
    file: 'components/screens/nodes/use-node-actions.ts',
    callee: 'api.server.add(',
    route: 'staged',
    why: 'servers Class B（克隆 = 造一个新节点，无副作用）',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.add(',
    route: 'direct',
    why: 'W-3：紧接着的 tailscaleLogin 是有远端效应的登录流程，且它按节点 id 寻址 —— 节点必须先真落盘',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.update(',
    route: 'direct',
    why: 'W-3：登录流程按节点 id 寻址，编辑态同样要求节点已经在磁盘上',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLogin(',
    route: 'direct',
    why: 'W-3：发起远端登录会话（拿授权 URL），暂存一个「还没发生」的登录无语义',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLoginCancel(',
    route: 'direct',
    why: 'W-3：撤销已发起的远端登录会话，副作用已经在远端发生',
  },
  {
    file: 'App.tsx',
    callee: 'api.server.tailscaleLoginCancel(',
    route: 'direct',
    why: 'W-0/Class A：控制面返回无效登录URL时，立即取消本次授权并等待进程收割；取消进程的会话命令不能暂存',
  },
  {
    file: 'mobile/app-wiring.ts',
    callee: 'api.server.tailscaleLoginCancel(',
    route: 'direct',
    why: 'W-0/Class A：移动端收到无效授权URL立即取消对应attempt并等待进程回收，失败由全局toast回报；会话取消不可暂存',
  },
  {
    file: 'components/dialogs/TsSettingsDialog.tsx',
    callee: 'api.server.tailscaleLogout(',
    route: 'direct',
    why: 'W-3：清 tailscale state 目录不可逆（BYPASS_TABLE 的 deleteTailscaleNode 同族）',
  },
  {
    file: 'components/dialogs/ts-logout-confirm.ts',
    callee: 'api.server.tailscaleLogout(',
    route: 'direct',
    why: 'W-3：主连接持有该节点时用户确认断开后的第二次登出，与 TsSettingsDialog / NodesScreen 的第一次登出同一动作、同样不可逆；两个调用点都已先过 splitStagedOnly 前置拦截',
  },
  {
    file: 'components/dialogs/TsLoginDialog.tsx',
    callee: 'api.server.tailscaleLogout(',
    route: 'direct',
    why: 'W-3：切 auth_key 前必须先清 state 目录（不清则 tsnet 继续用旧 node key，新 key 不生效），\
          与 TsSettingsDialog 的登出同族、同样不可逆；且必须在落盘/起核之前发生，进暂存就晚了',
  },
  {
    file: 'components/screens/nodes/NodesScreen.tsx',
    callee: 'api.server.tailscaleLogout(',
    route: 'direct',
    why: 'W-3：清 tailscale state 目录不可逆，「重置」只能退回一个空壳坏节点',
  },
  {
    file: 'components/dialogs/WarpDialog.tsx',
    callee: 'api.server.registerWarp(',
    route: 'direct',
    why: 'W-3：向 Cloudflare 真注册设备',
  },
  {
    file: 'components/dialogs/WarpDialog.tsx',
    callee: 'api.server.add(',
    route: 'direct',
    why: 'W-3：registerWarp 的远端设备已经建好，暂存这条节点会留下「远端有设备、本地无节点」的不可回滚状态',
  },
  { file: 'components/dialogs/WarpDialog.tsx', callee: 'api.server.update(', route: 'direct', why: 'W-3：applyWarpLicense 已改过远端账户绑定' },
  {
    file: 'components/dialogs/WarpDialog.tsx',
    callee: 'api.server.applyWarpLicense(',
    route: 'direct',
    why: 'W-3：向 Cloudflare 提交 license，远端账户等级当场改变，「重置」退不回去',
  },
  {
    file: 'components/screens/nodes/use-node-deletion.ts',
    callee: 'api.server.delete(',
    route: 'staged',
    why: 'servers Class B；全部节点先产生 nextValue:null，TS state/WARP 注销由后端持久删除意图延后到 Apply；仅暂存未启用时调用本 IPC',
  },
  {
    file: 'components/screens/nodes/use-node-deletion.ts',
    callee: 'api.server.deleteBatch(',
    route: 'staged',
    why: 'servers Class B；批量删除同样统一暂存，远端/文件副作用只在 Apply 消费；未知 id 或暂存未启用才交后端即时路径',
  },

  /* ── 移动端表单宿主与节点面（2026-09-06 批 2）。每一条的去向都与桌面同形腿**逐字相同** ——
        两端对「这次写该不该进暂存」给出两种答案，会让同一份配置在两个客户端上落成两种形状。 ── */
  {
    file: 'mobile/forms/NodeFormPanel.tsx',
    callee: 'api.server.add(',
    route: 'staged',
    why: 'servers Class B；表单提交的就是整个 ServerConfig，天然满足重放要求的幂等整体替换（同 NodeDialog）',
  },
  {
    file: 'mobile/forms/NodeFormPanel.tsx',
    callee: 'api.server.update(',
    route: 'staged',
    why: 'servers Class B；编辑提交的同样是整个 ServerConfig（base 起底保全非模型字段）',
  },
  {
    file: 'mobile/forms/ImportFormPanel.tsx',
    callee: 'api.server.addBulk(',
    route: 'staged',
    why: 'servers Class B；解析在 localImport.parse 已完成，addBulk 本身是纯 servers 写 ⇒ 逐节点一条条目',
  },
  {
    file: 'mobile/forms/TsExitPanel.tsx',
    callee: 'api.server.update(',
    route: 'staged',
    why: 'servers Class B；写的是该节点的 tailscaleSettings，没有远端副作用（本面板从活态回读的只是出口候选列表，不是被写的字段）',
  },
  /* ── 移动端组网三张表（2026-09-06 批 3）。去向与桌面同形腿**逐字相同** —— 两端对
        「这次写该不该进暂存」给出两种答案，会让同一份配置在两个客户端上落成两种形状。 ── */
  {
    file: 'mobile/forms/WgPanel.tsx',
    callee: 'api.server.add(',
    route: 'staged',
    why: 'servers Class B；手填与粘贴 .conf 两条来源都没有远端副作用，提交的是整个 ServerConfig（同 WgDialog）',
  },
  {
    file: 'mobile/forms/WgPanel.tsx',
    callee: 'api.server.update(',
    route: 'staged',
    why: 'servers Class B；编辑提交的同样是整个 ServerConfig（buildWgServer 以 base 起底保全非模型字段）',
  },
  {
    file: 'mobile/forms/TsSettingsPanel.tsx',
    callee: 'api.server.update(',
    route: 'staged',
    why: 'servers Class B；写的是该节点的 tailscaleSettings，没有远端副作用（面板里从活态回读的只是出口候选列表，不是被写的字段）——同 TsSettingsDialog',
  },
  {
    file: 'mobile/forms/TsSettingsPanel.tsx',
    callee: 'api.server.tailscaleLogout(',
    route: 'direct',
    why: 'W-3：清 tailscale state 目录不可逆（同 TsSettingsDialog；另有 splitStagedOnly 前置拦截）',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.registerWarp(',
    route: 'direct',
    why: 'W-3：向 Cloudflare 真注册设备（同 WarpDialog）',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.add(',
    route: 'direct',
    why: 'W-3：registerWarp 的远端设备已经建好，暂存这条节点会留下「远端有设备、本地无节点」的不可回滚状态（同 WarpDialog）',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.applyWarpLicense(',
    route: 'direct',
    why: 'W-3：向 Cloudflare 提交 license 当场改远端账户等级',
  },
  {
    file: 'mobile/forms/WarpPanel.tsx',
    callee: 'api.server.update(',
    route: 'direct',
    why: 'W-3：applyWarpLicense 已改过远端账户绑定（同 WarpDialog）',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.add(',
    route: 'direct',
    why: 'W-3：紧接着的 tailscaleLogin 是有远端效应的登录流程，且它按节点 id 寻址 —— 节点必须先真落盘（同 TsLoginDialog）',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.update(',
    route: 'direct',
    why: 'W-3：登录流程按节点 id 寻址，编辑态同样要求节点已经在磁盘上',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleLogin(',
    route: 'direct',
    why: 'W-3：发起远端登录会话（拿授权 URL），暂存一个「还没发生」的登录无语义',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleLoginCancel(',
    route: 'direct',
    why: 'W-3：撤销已发起的远端登录会话，副作用已经在远端发生',
  },
  {
    file: 'mobile/forms/TsLoginPanel.tsx',
    callee: 'api.server.tailscaleLogout(',
    route: 'direct',
    why: 'W-3：切 auth_key 前清掉旧 state 目录，不可逆；且它必须在紧随其后的 tailscaleLogin 之前真的发生 —— 暂存一个「还没清」的登出会让 tsnet 继续用旧 node key（表现是「提交成功、身份一动不动」）',
  },
  /* ── 移动端首页的规则写腿（2026-09-13 批 12）。桌面同形腿分别是
        `ConnectionTopology#addSubjectRule` 与 `RuleSubjectMenuItems#append`，
        去向**逐字相同** —— 两端对「这次写该不该进暂存」给出两种答案，会让同一份配置在两个
        客户端上落成两种形状。 ── */
  {
    file: 'mobile/home/MobileHomeScreen.tsx',
    callee: 'api.rules.add(',
    route: 'staged',
    why: 'trafficRules Class B；快速规则与「新建规则」提交的都是一条完整规则、无远端副作用 ⇒ 默认腿（同桌面 ConnectionTopology#addSubjectRule）；新增时前端自铸 id，条目才有稳定的实体寻址键',
  },
  {
    file: 'mobile/home/MobileHomeScreen.tsx',
    callee: 'api.rules.update(',
    route: 'staged',
    why: 'trafficRules Class B；`appendSubjectToRule` 产出的是整条规则的幂等替换，满足重放要求（同桌面 RuleSubjectMenuItems#append 那条全仓唯一的追加腿）',
  },
  {
    file: 'mobile/nodes/MobileNodesScreen.tsx',
    callee: 'api.server.add(',
    route: 'staged',
    why: 'servers Class B（克隆 = 造一个新节点，无副作用），同桌面 use-node-actions.ts 那条克隆腿',
  },
  {
    file: 'mobile/nodes/node-deletion.ts',
    callee: 'api.server.delete(',
    route: 'staged',
    why: 'servers Class B；全部节点先产生 nextValue:null，TS state/WARP 注销由后端持久删除意图延后到 Apply；仅暂存未启用时调用本 IPC',
  },
  {
    file: 'mobile/nodes/node-deletion.ts',
    callee: 'api.server.deleteBatch(',
    route: 'staged',
    why: 'servers Class B；批量删除同样统一暂存，远端/文件副作用只在 Apply 消费；未知 id 或暂存未启用才交后端即时路径',
  },
  {
    file: 'mobile/nodes/node-deletion.ts',
    callee: 'api.subscription.delete(',
    route: 'staged',
    why: 'subscriptions 与其下 servers 以同一 groupId 形成原子删除意图；重放只走全量 config 保存，不再调用后端级联腿',
  },
  {
    file: 'mobile/forms/SubFormPanel.tsx',
    callee: 'api.subscription.update(',
    route: 'direct',
    why: 'W-3：改 URL 会触发后端重拉；与桌面 SubDialog 同一条，按副作用绕过而非按 Class A 豁免',
  },
  {
    file: 'store/app-store.ts',
    callee: 'api.server.switch(',
    route: 'direct',
    why: 'W-1 switchServer（BYPASS_TABLE 明列）：首页出口框 / 状态栏节点名 / willRestartOnSelect 都实时回显它',
  },
  {
    file: 'tray/TrayMenu.tsx',
    callee: 'api.server.switch(',
    route: 'direct',
    why: 'W-1 switchServer；且托盘是独立 webview，够不着主窗的暂存 store',
  },

  // ── W-d：customRules 族 ──
  // 提交逻辑（含这两个 IPC 调用）2026-08-30 随 5C 拆分外提到 rule-submit.ts（RuleDialog 只剩一个
  // 薄封装调用 submitRule(...)），登记表跟着落点走。
  { file: 'components/dialogs/rule-submit.ts', callee: 'api.rules.add(', route: 'staged', why: 'customRules 是 UserConfig 字段（Class B），提交的是完整 Rule' },
  {
    file: 'components/dialogs/rule-submit.ts',
    callee: 'api.rules.update(',
    route: 'staged',
    why: 'customRules Class B；编辑提交的同样是完整 Rule（base 起底保全 tlsSpoof 等非模型字段）',
  },
  {
    // 新建带网络场景的规则插到最前（spec §3.4-4）：暂存腿写 `order:<orderKey>` 整序列条目，
    // 直写腿在 rules.add 之后调既有 rules.reorder —— 与 RulesScreen 拖拽排序同一对去向。
    file: 'components/dialogs/rule-submit.ts',
    callee: 'api.rules.reorder(',
    route: 'staged',
    why: 'routeRuleOrder / dnsRuleOrder 是 UserConfig 字段（Class B）；新建带场景规则的插首位，暂存腿与拖拽同形',
  },
  {
    // 规则删除的唯一执行腿（列表行内垃圾桶 + 规则弹窗 footer 共用），2026-07-30 从 RuleDialog 抽出。
    file: 'lib/use-rule-delete.ts',
    callee: 'api.rules.delete(',
    route: 'staged',
    why: 'customRules Class B；删规则无不可逆副作用（不同于删节点），集合实体删除 = nextValue 取 null',
  },
  {
    file: 'components/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.add(',
    route: 'staged',
    why: 'customRules Class B（行内复制，载荷来自 duplicateRulePayload，无副作用）',
  },
  {
    file: 'components/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.update(',
    route: 'staged',
    why: 'customRules Class B（行内启停开关，提交的是整条 Rule）',
  },
  {
    file: 'components/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.reorder(',
    route: 'staged',
    why: 'customRules Class B；顺序条目 entityPath=[集合] 单段、nextValue=主键序列，replay 分两趟（实体在前、顺序在后）⇒ 与同批增删改可交换',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.add(',
    route: 'staged',
    why: 'customRules Class B（行内复制，载荷来自 duplicateRulePayload，无副作用）',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.update(',
    route: 'staged',
    why: 'customRules Class B（行内启停开关，提交的是整条 Rule）',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.rules.reorder(',
    route: 'staged',
    why: 'customRules Class B；顺序条目 entityPath=[集合] 单段、nextValue=主键序列，replay 分两趟（实体在前、顺序在后）⇒ 与同批增删改可交换',
  },
  {
    file: 'components/screens/home/ConnectionTopology.tsx',
    callee: 'api.rules.add(',
    route: 'staged',
    why: 'customRules Class B（拓扑图右键「为其加规则」，纯新增无副作用）',
  },
  {
    file: 'components/RuleSubjectMenuItems.tsx',
    callee: 'api.rules.update(',
    route: 'staged',
    why: 'customRules Class B；「加入已有规则」追加腿写的是**整条** Rule（appendSubjectToRule 的返回值，{...base} 起底 + 镜像同步）⇒ 幂等整体替换。拓扑与连接页两个菜单共用本文件这一条腿',
  },
  {
    // 移动端连接屏的同一件事（批 13）。**判据层与桌面共用**（`dialogs/rule-append.ts` 的
    // `appendSubjectToRule`），拦路的只有组件：桌面那条腿住在 `RuleSubjectMenuItems.tsx` 里，
    // 而它渲染 `.ctx-i` 菜单按钮 = 桌面层叠链，契约 A1 禁入移动端 ⇒ 调用点各有一处，登记各占一行。
    file: 'mobile/connections/MobileConnectionsScreen.tsx',
    callee: 'api.rules.update(',
    route: 'staged',
    why: 'customRules Class B；与桌面那条追加腿同形（写 appendSubjectToRule 返回的整条 Rule，幂等整体替换），闸门同为 editRoute(\'trafficRules\')',
  },

  // ── W-d：subscriptions 族 ──
  {
    file: 'store/subscription-create-operation-store.ts',
    callee: 'api.subscription.createStart(',
    route: 'direct',
    why: 'W-3：新增订阅启动后端持有的 fetch/parse/commit operation；commit 才原子写 config，renderer 重建可重附，暂存「重置」仍不能回退该网络副作用',
  },
  {
    file: 'components/dialogs/SubDialog.tsx',
    callee: 'api.subscription.update(',
    route: 'direct',
    why: 'W-3：改 URL 会触发后端重拉；subscriptions 已因订阅级网卡策略进入 UserConfig，但这里按副作用绕过而非按 Class A 豁免',
  },
  {
    file: 'components/screens/nodes/use-node-subscription-actions.ts',
    callee: 'api.subscription.delete(',
    route: 'staged',
    why: 'subscriptions 与其下 servers 以同一 groupId 形成原子删除意图；重放只走全量 config 保存，不再调用后端级联腿，TS/WARP 副作用由 Apply 消费',
  },
  {
    file: 'domain/subscription-refresh.ts',
    callee: 'api.subscription.updateServers(',
    route: 'direct',
    why: 'W-3 refreshSubscription（BYPASS_TABLE 明列）：已发网络请求 + 已拿到新节点集',
  },

  // ── W-d：ruleResources 族（下载/覆盖即时；删除则由 Apply 事务延迟物理 unlink）──
  {
    file: 'components/dialogs/ResCatalogDialog.tsx',
    callee: 'api.ruleResources.download(',
    route: 'direct',
    why: 'W-3：真下载并把 .srs 落盘，「重置」删不掉已经写下的文件',
  },
  {
    file: 'components/dialogs/ResUrlDialog.tsx',
    callee: 'api.ruleResources.download(',
    route: 'direct',
    why: 'W-3：按 URL 真下载并落盘，「重置」删不掉已经写下的文件',
  },
  {
    file: 'components/screens/resources/ResourcesScreen.tsx',
    callee: 'api.ruleResources.updateAll(',
    route: 'direct',
    why: 'W-3：批量重新下载并覆盖本地规则文件，「重置」退不回旧内容',
  },
  {
    file: 'components/screens/resources/ResourcesScreen.tsx',
    callee: 'api.ruleResources.redownload(',
    route: 'direct',
    why: 'W-3：重新下载并覆盖该资源的本地文件，「重置」退不回旧内容',
  },
  {
    file: 'components/screens/resources/ResourcesScreen.tsx',
    callee: 'api.ruleResources.delete(',
    route: 'staged',
    why: 'ruleResources Class B；先暂存 config 实体删除，保存只写持久删除意图，Apply/冷启动才 unlink 本地规则文件',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.ruleResources.updateAll(',
    route: 'direct',
    why: 'W-3：批量重新下载并覆盖本地规则文件，「重置」退不回旧内容',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.ruleResources.redownload(',
    route: 'direct',
    why: 'W-3：重新下载并覆盖该资源的本地文件，「重置」退不回旧内容',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.ruleResources.delete(',
    route: 'staged',
    why: 'ruleResources Class B；先暂存 config 实体删除，保存只写持久删除意图，Apply/冷启动才 unlink 本地规则文件',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.ruleResources.cancel(',
    route: 'direct',
    why: 'Class A：中止在途下载不写 config（被取消的下载不落盘不入册）',
  },
  {
    file: 'mobile/screens/rules/RulesScreen.tsx',
    callee: 'api.ruleResources.resetBuiltin(',
    route: 'direct',
    why: 'W-3：重置内置集 = 重新下载并覆盖本地文件',
  },
  {
    file: 'components/screens/resources/ResourcesScreen.tsx',
    callee: 'api.ruleResources.resetBuiltin(',
    route: 'direct',
    why: 'W-3：重置内置集 = 重新下载并覆盖本地文件',
  },
  {
    file: 'components/screens/resources/ResourcesScreen.tsx',
    callee: 'api.ruleResources.cancel(',
    route: 'direct',
    why: 'Class A：中止在途下载不写 config（被取消的下载不落盘不入册）',
  },
];

// ─────────────────────────────── 断言 ───────────────────────────────

const key = (s: { file: string; callee: string }) => `${s.file} | ${s.callee}`;

describe('守卫自检：扫到的确实是源码（防扫空 / 过滤过头 → 断言恒真）', () => {
  it('测试文件与 api-client 定义处（barrel + ipc/api/ 目录）都被排除在扫描面外', () => {
    expect(FILES.some((p) => /\.(test|spec)\.tsx?$/.test(p))).toBe(false);
    expect(HITS.some((h) => h.file.endsWith('ipc/api-client.ts') || h.file.includes('ipc/api/'))).toBe(
      false
    );
  });

  it('每组判据面都有实扫命中（任一组归零 = 该组正则已失配）', () => {
    for (const [tag] of PATTERNS) {
      expect(HITS.filter((h) => h.tag === tag).length, `${tag} 一个调用点都没扫到`).toBeGreaterThan(0);
    }
  });

  it('去注释后仍是可断言的代码（防 code() 把源码整段吃掉）', () => {
    for (const s of SITES) {
      const src = SOURCE.get(s.file);
      expect(src, `${s.file} 不在扫描面里（路径写错 / 文件已删）`).toBeDefined();
      const raw = readFileSync(join(SRC, s.file), 'utf8');
      expect(src!.replace(/\s+/g, '').length, `${s.file} 去注释后几乎空了`).toBeGreaterThan(raw.length / 8);
    }
  });

  /**
   * 绑定别名判据面（`@/test/write-leg-aliases`）的正反对照。
   *
   * 四种绑定形态里，**句柄成员访问**与**选择器改名**在真实代码上今天是零实例 —— 判据在真实代码上
   * 空跑，它是绿的还是死的分不出来。故用合成夹具证明它确实报得出，并证明它不是恒报
   * （同名绑定与不改名的解构**不得**被别名腿重复计一次，否则同一处写腿会在错误清单里出现两遍）。
   */
  it('绑定别名判据面：句柄成员访问 / 解构改名 / 选择器改名三种都抓得到（真实代码上零实例的那两种靠它证明）', () => {
    const legs = aliasWriteLegs(ALIAS_FIXTURE, STORE_CONFIG_WRITES);
    const hit = legs
      .flatMap((re) => [...ALIAS_FIXTURE.matchAll(re)].map((m) => ({ t: m[0].replace(/\s+/g, ''), i: m.index })))
      .sort((a, b) => a.i - b.i)
      .map((x) => x.t);
    expect(hit, '别名解析没抓全四种绑定形态 —— P2 那类洞会重新张开').toEqual([...ALIAS_FIXTURE_LEGS]);
  });

  it('反向对照：不改名的解构与同名选择器绑定**不**产生别名腿（否则同一处写腿会被重复计数）', () => {
    const same = ["const { update } = useConfig();", "const saveConfig = useAppStore((s) => s.saveConfig);"].join(
      '\n'
    );
    expect(aliasWriteLegs(same, STORE_CONFIG_WRITES)).toEqual([]);
  });

  it('门自己的适配层被排除在判据面外，且那个目录里只有适配层（清单逐字对拍）', () => {
    // 排除是必要的（夹具会自污染），但排除一个目录也意味着放走它里面的任何东西 ——
    // 故清单逐字钉住：往 `src/test/` 里塞新文件必须先来这里登记，顺手塞产品代码会当场红。
    // `call-sites.test-support.ts`（2026-09-06）：「谁调了这个符号」的共用谓词，两道接线门共用，
    // 一个写腿都不含（它只读源码文本），故过审。
    expect(HITS.filter((h) => h.file.startsWith('test/')).map(key)).toEqual([]);
    expect(
      readdirSync(join(SRC, 'test')).filter((f) => /\.tsx?$/.test(f)).sort(),
      '`src/test/` 的内容变了 —— 它被整个排除在判据面外，新增文件必须显式过一次审'
    ).toEqual(['call-sites.test-support.ts', 'ts-compiler.ts', 'write-leg-aliases.ts']);
  });

  it('锚点仍在：已知的两个基准调用点必须被扫到', () => {
    const found = new Set(HITS.map(key));
    // NodeDialog 是实体写入口锚点；app-store 是即时补丁原语。两者任一消失都说明判据面失配。
    expect(found.has('components/dialogs/NodeDialog.tsx | api.server.add(')).toBe(true);
    expect(found.has('store/app-store.ts | api.config.patch(')).toBe(true);
  });
});

describe('T1：写入口全登记（新增写 config 的路径 ⇒ 必须显式选一个去向）', () => {
  it('树上扫到的每一个 (文件, 写方法) 都在登记表里', () => {
    const registered = new Set(SITES.map(key));
    const unregistered = [...new Set(HITS.map(key))].filter((k) => !registered.has(k)).sort();
    expect(
      unregistered,
      `以下写 config 的调用点未登记 —— 先按 spec §2.5 Q3/Q3-b 判去向，再补进 SITES：\n${unregistered.join('\n')}`
    ).toEqual([]);
  });

  it('登记表里没有陈旧行（登记了却在树上找不到）', () => {
    const found = new Set(HITS.map(key));
    const stale = SITES.map(key).filter((k) => !found.has(k)).sort();
    expect(stale, `以下登记已陈旧（调用点被删/改名），删掉或改对：\n${stale.join('\n')}`).toEqual([]);
  });

  it('登记表自身不重复（同一 (文件, 方法) 不得有两个去向）', () => {
    const seen = new Set<string>();
    const dup = SITES.map(key).filter((k) => (seen.has(k) ? true : (seen.add(k), false)));
    expect(dup).toEqual([]);
  });
});

describe('T2：白名单是数据不是借口（每条 direct/blocked 都得点名判据）', () => {
  it('direct 的理由必须点名 W-0/Class A 或 W-1/W-2/W-3', () => {
    for (const s of SITES.filter((x) => x.route === 'direct')) {
      expect(s.why, `${key(s)} 的理由没点名任何绕过/豁免谓词`).toMatch(/W-1|W-2|W-3|W-0|Class A/);
    }
  });

  /**
   * 欠账已清零（原 4 行：appRules / customAppPresets 两行靠把条目模型的主键从写死的 `id` 改成
   * 「集合 → 主键字段」映射解决；`rules.reorder` 靠新增整集合顺序条目解决；`subscription.delete`
   * 是错分类 —— 它本就是 W-3，已归位 `BYPASS_TABLE`）。
   *
   * 断言按本组原注释的字面要求从 `toBeGreaterThan(0)` 改成 `toBe(0)`：**这不是放松**。
   * 门的牙从「欠账必须可见」换成了更紧的「欠账必须为零」—— 再想登记一条 `blocked`，
   * 就必须先动这一行，那正是要逼出来的那次显式决定（而不是往表里悄悄多加一行）。
   * `Route` 保留 `'blocked'` 成员与它的语义文档：真出现表达不了的入口时，它仍是唯一诚实的去向。
   */
  it('契约阻塞欠账为零（再开一条必须先改本断言，不得悄悄加行）', () => {
    const blocked = SITES.filter((x) => x.route === 'blocked');
    expect(blocked.map(key)).toEqual([]);
  });

  it('每条登记都有非空理由', () => {
    for (const s of SITES) expect(s.why.length, key(s)).toBeGreaterThan(10);
  });
});

describe('T3：每一处 staged 调用点都必须在暂存闸门辖区内（按调用点判，不按文件判）', () => {
  const stagedKeys = new Set(SITES.filter((s) => s.route === 'staged').map(key));
  /** 树上真实扫到的、登记为 `staged` 的**每一处**调用点（同文件同方法的第二处也在内）。 */
  const stagedHits = HITS.filter((h) => stagedKeys.has(key(h)));
  const stagedFiles = [...new Set(stagedHits.map((h) => h.file))].sort();

  const parsed = new Map<string, ts.SourceFile>();
  const parse = (f: string): ts.SourceFile => {
    let sf = parsed.get(f);
    if (!sf) {
      const abs = join(SRC, f);
      sf = ts.parseSourceFile(abs, readFileSync(abs, 'utf8'));
      parsed.set(f, sf);
    }
    return sf;
  };
  const gatesOf = new Map<string, GateMap>();
  const analyze = (f: string): GateMap => {
    let g = gatesOf.get(f);
    if (!g) {
      g = analyzeGates(parse(f));
      gatesOf.set(f, g);
    }
    return g;
  };

  it('自检：取材面非空且**含多腿文件**（登记被整体降级 / 扫描器失配 ⇒ 下面每条恒绿）', () => {
    expect(stagedHits.length, `只扫到 ${stagedHits.length} 处 staged 调用点`).toBeGreaterThan(25);
    expect(stagedFiles.length, `只扫到 ${stagedFiles.length} 个 staged 文件`).toBeGreaterThan(14);
    // 「同一文件里两处以上 staged 调用点」必须真实存在 —— 那一格正是旧判据看不见的地方，
    // 它归零就说明本轮收紧无处发力（判据在，但射程空）。
    const multi = stagedFiles.filter((f) => stagedHits.filter((h) => h.file === f).length > 1);
    expect(multi.length, '没有任何一个文件带两处以上 staged 调用点').toBeGreaterThan(3);
  });

  it('自检：每一处 staged 调用点都落在某个函数体内（提到模块顶层 ⇒ 无从谈闸门）', () => {
    const homeless = stagedHits
      .filter((h) => innermostFn(analyze(h.file).fns, h.index) === null)
      .map((h) => `${h.file}:${h.line} ${h.callee}`);
    expect(homeless, '这些写腿不在任何函数体内 —— 判据的「同函数」前提对它们不成立').toEqual([]);
  });

  it('每一处 staged 调用点都被一道先于它、且辖区罩住它的暂存闸门管辖', () => {
    const escaped: string[] = [];
    for (const f of stagedFiles) {
      const hits = stagedHits.filter((h) => h.file === f);
      const ungated = new Set(
        ungatedOffsets(
          parse(f),
          hits.map((h) => h.index)
        )
      );
      for (const h of hits) if (ungated.has(h.index)) escaped.push(`${h.file}:${h.line} ${h.callee}`);
    }
    expect(
      escaped.sort(),
      '这些调用点登记为 staged，但没有任何一道暂存闸门的辖区先于它罩住它 —— ' +
        '登记表说「进暂存」，实现却直连后端写盘。要么修实现，要么把这一行的 route 改成实话'
    ).toEqual([]);
  });

  it('闸门形态固定：editRoute(<键>, stagingEnabled[, op]) —— 键可以是字面量或局部常量，开关不许是别的', () => {
    for (const f of stagedFiles) {
      const src = SOURCE.get(f)!;
      for (const g of src.matchAll(/editRoute\(([^)]*)\)/g)) {
        expect(g[1], `${f} 的闸门实参形态不对（开关必须是 stagingEnabled）`).toMatch(
          /^\s*(?:'[A-Za-z][A-Za-z0-9.]*'|[A-Za-z_$][\w$]*)\s*,\s*stagingEnabled\s*(?:,\s*'[A-Za-z]+'\s*)?$/
        );
      }
    }
  });

  it('**不得**给 editRoute 传死开关（传 true 就绕过了总开关，本轮的零变化承诺当场作废）', () => {
    for (const [f, src] of SOURCE) {
      // `staged-config.ts` 自身的文档/实现不在此列（它定义 editRoute），测试文件已被排除在扫描面外。
      if (f === 'lib/staged-config.ts') continue;
      expect(src, `${f} 给 editRoute 传了字面量开关`).not.toMatch(/editRoute\([^)]*,\s*(true|false)\b/);
    }
  });

  it('每个 staged 文件都从 @/lib/staged-config 取分流判据（两种形态各认一个入口）', () => {
    for (const f of stagedFiles) {
      expect(SOURCE.get(f)!, `${f} 既没 import editRoute 也没 import splitStagedOnly`).toMatch(
        /import\s*\{[^}]*\b(?:editRoute|splitStagedOnly)\b[^}]*\}\s*from\s*'@\/lib\/staged-config'/
      );
    }
  });

  /**
   * `stage` 由调用方注入的文件：不持有 `useStagedConfigStore`，闸门本体仍在文件内（上面那条按调用点
   * 的断言照样罩着它们）。豁免表用 `toEqual` 与实况**逐字对拍**：多一个（某文件开始自持 store 了）
   * 或少一个（新文件悄悄不持有）都转红 —— 白名单不许变成僵尸。
   */
  const STAGE_INJECTED_FILES = [
    'components/dialogs/rule-submit.ts',
    'components/screens/nodes/use-node-actions.ts',
    'components/screens/nodes/use-node-deletion.ts',
  ] as const;

  it('只有登记过的那几个文件可以不自持 staged store（僵尸豁免 ⇒ 红）', () => {
    const noStore = stagedFiles.filter(
      (f) =>
        !/import\s*\{[^}]*\buseStagedConfigStore\b[^}]*\}\s*from\s*'@\/store\/staged-config-store'/.test(
          SOURCE.get(f)!
        )
    );
    expect(noStore, '「stage 由调用方注入」的豁免表与实况对不上').toEqual([...STAGE_INJECTED_FILES]);
    // 正面断言：豁免的前提是它**确实还在造暂存条目**，只是不持有 store。
    for (const f of STAGE_INJECTED_FILES) {
      expect(SOURCE.get(f)!, `${f} 不再引 StagedEntry —— 豁免的前提没了`).toMatch(/\bStagedEntry\b/);
    }
  });

  it('批量形态的路由器仍由暂存开关驱动（F-批量 的来源锚点）', () => {
    const src = SOURCE.get('components/screens/nodes/node-delete-fallback.ts');
    expect(src, 'node-delete-fallback.ts 不见了 —— F-批量 的闸门来源塌了').toBeDefined();
    expect(src!, '`partitionNodeDeleteRoutes` 不见了（改名了？）').toMatch(
      /export function partitionNodeDeleteRoutes/
    );
    expect(
      src!,
      '它不再吃 `EditRoute` —— 那 `routes.staged` 就不是暂存判定的结果了，把它当闸门是假绿'
    ).toMatch(/import\s+type\s*\{[^}]*\bEditRoute\b[^}]*\}\s*from\s*'@\/lib\/staged-config'/);
  });

  /* ── 判据自证：它报得出错，且报的正是那一处 ──────────────────────────────────
   *
   * 合成源码复刻 F3 那条真缺陷的形态：**同一个文件、同一个组件**，一条腿过闸门、另一条腿整条绕过。
   * 旧判据（按文件查有没有 editRoute）对它是绿的；新判据必须点名绕过的那一处、且只点名那一处。 */
  const FIXTURE_HEAD = `
function useRules(stagingEnabled: boolean, stage: (e: unknown) => void) {
  const toggleRule = async (next: unknown) => {
    if (editRoute('trafficRules', stagingEnabled) === 'staged') {
      stage({ id: 'rule:1', nextValue: next });
      return;
    }
    await api.rules.update(next, 'route');
  };
  const deleteResource = async (id: string) => {
`;
  const FIXTURE_TAIL = `
    await api.ruleResources.delete(id, true);
  };
  return { toggleRule, deleteResource };
}
`;
  const offsetsIn = (text: string): number[] =>
    ['api.rules.update(', 'api.ruleResources.delete('].map((c) => text.indexOf(c));

  it('正向对照：同文件里第二条腿绕过闸门 ⇒ 判据点名它（且只点名它）', () => {
    const text = FIXTURE_HEAD + FIXTURE_TAIL;
    const sf = ts.parseSourceFile('synthetic-staged-bypass.ts', text);
    const [gated, ungated] = offsetsIn(text);
    expect(ungatedOffsets(sf, [gated, ungated])).toEqual([ungated]);
  });

  it('反向对照：把那条腿也接进闸门 ⇒ 判据不再报（证明它不是恒红）', () => {
    const text =
      FIXTURE_HEAD +
      `
    if (editRoute('ruleResources', stagingEnabled) === 'staged') {
      stage({ id: 'res:' + id, nextValue: null });
      return;
    }` +
      FIXTURE_TAIL;
    const sf = ts.parseSourceFile('synthetic-staged-gated.ts', text);
    expect(ungatedOffsets(sf, offsetsIn(text))).toEqual([]);
  });
});

describe('T4：`enabled=false` 时的行为契约（关闭态必须等价于暂存层不存在）', () => {
  /**
   * 原标题是「总开关关着 ⇒ 本轮改动在产品行为上零变化」，那是开关翻开前的框架，已过时：
   * 开关 2026-07-29 起为 `true`，产品默认行为就是「默认进暂存」。
   *
   * 本组保留的价值在于**关闭态仍是一条必须成立的退路**（回滚手段 = 把常量翻回 `false`），
   * 故下面各条一律显式传 `enabled=false` 测函数契约，不再依赖编译期默认值。
   */
  it('编译期开关为开（翻回 false 是产品行为变更，必须显式改本断言）', () => {
    expect(STAGED_CONFIG_ENABLED).toBe(true);
  });

  it('开关关时每一个 UserConfig 字段都路由到 direct（含全部 Class B 键）', () => {
    for (const k of USER_CONFIG_FIELDS) expect(editRoute(k, false), k).toBe('direct');
    // 子键路径与 Class A 键同样恒 direct —— 入口侧不该出现第二处 if。
    expect(editRoute('dnsConfig.enableFakeIp', false)).toBe('direct');
    expect(editRoute('autoStart', false)).toBe('direct');
  });
});

/**
 * T5：组网单例节点（Tailscale / WARP）的远端注册/登录写腿**恒 `direct`**。
 *
 * # 为什么这条门属于本文件，又为什么它是给别处用的
 *
 * `lib/entity-action-wiring.test.ts` 里有四行登记（TS 登出 ×2 / WARP applyLicense / WARP update）
 * 走 `block` 策略，因为它们触达 state 目录或远端设备。这里钉住其创建/登录来源仍走 W-3 直写，
 * 防止把“已发起远端会话”拆成一条可重置的本地配置意图。
 *
 * 手填/导入可能产生 staged-only 的 endpoint，这不削弱本门：对那类节点，远端动作由
 * `ENTITY_ACTION_TABLE` 明确 block；普通配置删除则按节点类型另行分流。
 */
describe('T5：TS / WARP 的远端注册/登录写腿恒 direct', () => {
  const MESH_SINGLETON_FILES = [
    'components/dialogs/TsLoginDialog.tsx',
    'components/dialogs/WarpDialog.tsx',
  ] as const;

  const rows = SITES.filter((s) => (MESH_SINGLETON_FILES as readonly string[]).includes(s.file));

  it('两个弹窗都真的有写腿被登记（扫空 ⇒ 下面那条恒真）', () => {
    for (const f of MESH_SINGLETON_FILES) {
      expect(
        rows.filter((s) => s.file === f).length,
        `${f} 一条写腿都没登记 —— 判据面已失配，下面那条断言会空跑`
      ).toBeGreaterThan(0);
    }
  });

  it('这两个文件里没有任何一条写腿走 staged', () => {
    const staged = rows.filter((s) => s.route !== 'direct').map((s) => `${s.file} | ${s.callee} → ${s.route}`);
    expect(
      staged,
      'TS / WARP 的远端注册/登录写腿不再恒直落盘 ⇒ 已发生的远端副作用会被伪装成可重置配置'
    ).toEqual([]);
  });
});
