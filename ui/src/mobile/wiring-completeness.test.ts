/**
 * 移动端**接线完成度**门 —— 把「全部接线」从一句自述变成一个 rc=0/rc=1 的事实。
 *
 * # 为什么必须新开这一道门（根因，不是补丁）
 *
 * 上一批交付的失败形态不是「漏了」，而是「**诚实地标注了，却只对人可见**」：五屏都在、门全绿、
 * 真机装得上，每一个缺口都写着「移动端暂未接入 X，请在桌面端操作后同步」。那句话让缺口**对读代码
 * 的人可见**，却**对判据不可见** —— 没有任何断言在数这些句子，于是「还有几十条没接线」与「全部
 * 接线」在 CI 里长得一模一样。
 *
 * 本门做的就是把那些缺口变成**可数的项**，并要求这个数减去处置表后为空集。
 *
 * # 计量单位是「**渲染点**」，不是「写着缺席字样的键」（2026-09-06 复审 blocker，已修）
 *
 * 第一版按**键**去重：`mobileRules.formUnavailable` 在移动端有 8 个消费点，账上只算 1 条。
 * 于是「新增一处死界面时复用一条已有的缺席键」这条路的债务增量恒为 0 —— 不用进
 * 处置表、什么都不会红，而界面上多了一块死掉的功能。现在 i18n 面的项是 `键 × 消费文件 × 序号`，
 * 加一处渲染就加一条债。
 *
 * # 三个取材面（并集；每一面都各自做过切点自检，见「① 取材面自检」那一组）
 *
 * **A. i18n 面** —— `../i18n/locales/**.json`（主分区 + `auxiliary/`，**全部五个语种**）展平后，
 *    被**两条互相独立的缺席信号**之一认领的键，取**跨语种并集**（只扫一个语种会漏：`fa`/`ru`
 *    的翻译常规滞后，同一条缺口在 zh-CN 里写着而在 en-US 里还没同步）：
 *      · **A1 文案信号** `ABSENCE_PHRASES` —— 值读起来像「这里没有 / 去桌面做」。
 *      · **A2 键名信号** `ABSENCE_KEY_NAMES` —— 键名本身是缺席声明（`*Absent` / `*Unavailable` /
 *        `*Pending` / `*Unregistered` / `*PlatformNote` …），且这条键是**移动端自有**的
 *        （顶层命名空间在 `MOBILE_NAMESPACES` 里，或路径里带 `mobile` 段）。
 *    🔴 **为什么要两条信号**：第一版只有 A1，而 A1 是一张**措辞黑名单** —— 复审当天仓里就有两条
 *      如实标注的缺口整条掉在面外（`mobileSettings.general.privacyLockAbsent` 写的是「还没接」
 *      与 `is wired` 被 `neither…nor` 拆开；`mobileRules.resourceMetaUnregistered` 写的是
 *      「尚未登记进」），于是「29 条清零 = 全部接线」这句话当天就是假的。措辞可以无限换，
 *      靠人追永远追不上；两条独立信号意味着一条新缺口要同时**把键名写得人畜无害、把文案也写得
 *      人畜无害**才能溜过去 —— 那时它已经不是一条诚实的缺席标注了，剩下的由 B/C 面兜底。
 *    再按**移动端相关性**收窄（只对 A1；A2 按构造就是移动端自有的）：键的顶层命名空间属于
 *    `MOBILE_NAMESPACES`，**或**该键在移动端生产源码里有消费点（字面量，或拼键的父路径前缀）。
 *    ⚠️ 这一层收窄是**故意**的射程选择，不是偷懒：全仓还有一批「桌面自己也没接」的同形文案
 *    （`procPick.empty` / `resCatalog.errUnavailable` / `resources.urlDlgErrUnavailable` /
 *    `helper.*` / `settings.about.uninstallKindUnsupported`），
 *    它们是**桌面的**债，把它们算进「移动端还欠多少条」会让这个数字失去意义。
 *    ⚠️ `nodes/absence-register.ts` **不算消费文件**：那是 B 面的真值源，它引用一条 `reasonKey`
 *      是「登记」不是「渲染」。两面各数一遍同一件事只会把账做虚。B 面那侧另有一条正面断言
 *      钉着「登记表里每条非豁免处置的 `reasonKey` 都必须被缺席信号认领」，这条路没有被放开。
 *
 * **B. 登记表面** —— `nodes/absence-register.ts` 里处置不是 `ported` 的每一条，逐条成项
 *    （`ROW_ACTIONS` / `BATCH_ACTIONS` / `SUB_PENDING_FIELDS`）。这一面**直接 import 真值源**
 *    而不是正则扫源码：正则漏看一个对象字面量会**静默少测**，import 拿到的是运行期真的那张表。
 *    「全仓同形物」由下面「登记表唯一性」那条断言钉住 —— 判据是**结构性**的（处置字段名 ∈
 *    {kind,state,status,disposition}、取值 ∈ 缺席态词表、且同文件带 `reasonKey`），
 *    不再是第一版那个只认单引号 `kind: 'absent'` 的字面形状（换个引号或换个词就能在门外另起一张表）。
 *
 * **C. 控件面** —— 移动端可达的**非测试** `.ts/.tsx` 里，写操作控件被写死成不可用的**词法**形态。
 *    取材面在两处被复审打开过，都已修：
 *      · **注释先清零**（`blankComments`，行号保持不变）：第一版直接对原文分行，`// …disabled`
 *        这样一句注释会被算成一处死控件 —— 一条没有控件可接、只能靠改注释措辞消掉的幽灵待办。
 *      · **文件面按「移动端可达的模块闭包」取**，不再锁死在 `mobile/` 目录：`mobile/` 全部非测试
 *        文件，**加上**「从移动端可达、且从桌面任何入口都不可达」的 `ui/src` 模块。第一版锁目录，
 *        于是「把死按钮抽进 `components/PendingAction.tsx` 再由 BackupPage 渲染」当场离面。
 *        今天这类模块是 0 个（报告每次自己打这个数，不写死在注释里），这一面因此与第一版同形 ——
 *        但那条路已经堵上了。
 *        ⚠️ **没有**按复审建议的「从移动端出发遍历到的仓内文件全都纳面」：从移动端可达的非 mobile/
 *        模块有 120 个，全纳进来今天只多 1 条命中，而那 1 条是
 *        `hooks/use-network-interfaces.ts:72` 的 `{ …, disabled: true }` —— 网卡当前不可用这个
 *        **运行期状态**，不是未接线，进面就得为它写一条假处置。更要紧的是结构：那 120 个模块里
 *        桌面自己的死控件会记到**移动端**的账上，桌面加一颗、移动端一行没动，这道门就红，
 *        而完成门会叫移动端去认领别人的债。这与 A 面「桌面自己的债不算移动端的账」是同一条口径。
 *    🔴 **这一面的射程仍然被显式缩过，且缩到哪写在这里**：它认六种**词法**形态（见
 *      `CONTROL_PATTERNS`），其中 `disabled={FLAG}` 只做**同文件**常量折叠。它抓不到
 *      ① 跨文件/跨模块传进来的常量假开关；
 *      ② 渲染成 `<div>` / `<span>` 之类非交互元素、因而根本没有 `disabled` 属性的（`role="button"`
 *         + `pointer-events:none` 那一路）；
 *      ③ **整块没画出来**的功能（那在词法上什么都不是 —— 见下面「射程自曝」第 1 条）。
 *      不许把这一面说成「覆盖全部被禁用的控件」，它不是。
 *
 * # 处置表 `DISPOSITIONS` 的口径（唯一允许把一条从债务里拿掉的出口）
 *
 * 每条必须带 `{ id, face, kind, evidence, anchors, reviewed }`，`kind` 只有三种：
 *  · `platform-absent` —— **平台不提供这个能力**。依据是 `file:line`，或「桌面同一条键 /
 *    同一个函数上同样缺」的两端同缺证据。
 *  · `model-absent` —— **数据模型里没有这个对象，两端同缺**（2026-09-25 新增）。与平台无关：
 *    换一台设备也给不出。判据比另外两档多一条：必须有一条锚落在移动树（`ui/src/mobile/`）**之外**，
 *    证明「另一端也缺」—— 只有移动端的锚证明不了两端同缺。
 *  · `reachable-elsewhere` —— 这**不是缺席**：能力在移动端别处就有，这句文案是个指路牌。
 *    依据必须是移动端生产源码里那条真的渲染/路由。
 * 🔴 「我们还没做」「做起来麻烦」「依赖另一批」「排期里」「先不做」「要等」——一律不许进。
 *
 * 🔴 **`anchors` 是被机器打开核对的，不是形状检查**（2026-09-06 复审 major，已修）。
 * 第一版只用 `/\.(tsx?|rs):\d+/` 校验理由里**长得像** `file:line`，从不打开那个文件 ——
 * 于是白名单依据的事实可以整条消失而门不动：实测把移动端测速那一处 `onSelect: () => runSpeedTest(...)`
 * 改成 `() => undefined`（测速腿整条拔掉），或把桌面那颗「移动到分组」接上（两端同缺当场不成立），
 * 两个门都还是全绿。现在每条 anchor 是 `{ file, mustContain }`：文件必须存在、必须真的含那段文本，
 * 否则当场红。**不写行号**是有意的 —— 行号会被上方任何一次无关改动推走，那是脆而不是严。
 *
 * # 射程自曝（这道门**守不住**什么 —— 不许把它说得比实际宽）
 *
 * 1. **守得住「文案还在」，守不住「文案删了但功能也没接」。** 三个面全部是**词法/登记**面：
 *    把 `mobileSettings.backup.pickerPending` 那句话删掉、把两颗按钮也一起删掉，这道门会**变绿**，
 *    而备份导出依然没接。这是本门结构性的天花板，不是可以调参修好的疏漏。
 *    ⇒ 补这一格的是**另一道门**：`half-truth-facts.test.ts` 那种「断言那条能力今天确实在」的
 *      正面事实断言。本门与它是一对，缺任何一半都留着一条绕行路 —— 必须
 *      **两个文件一起跑**，Vitest rc=0 要求两边同时绿（早期报告壳只跑本文件，实测删掉 dialog 插件注册后
 *      半真门当场红而报告一个字不变、照样会在清零那天宣布「全部接线达成」）。
 * 2. **守不住「从来没画过的功能」**：桌面有、移动端一行都没写、也没人登记 —— 它不在任何一面上。
 *    兜这一格的是「拿桌面那一侧与登记表逐条对差」的另一族门，本门不重复实现、也**不假装**覆盖：
 *      · 节点屏 —— `nodes/nodes-screen.test.tsx` ⑤⑥（桌面按钮数 ↔ 登记表 ↔ 真渲染，三方对差）；
 *      · 其余四屏 —— `screen-parity.test.ts`（2026-09-06 批 6 落地）：动作面 / 数据块面逐条对差，
 *        全部豁免都带一条被机器打开核对的锚；
 *      · **移动端读的每一格 store 状态有没有写入方** —— 同一份门的 ⑤ 组（取材面在移动端这一侧）。
 *        `W-23` / `W-24` 正是死在这一格：控件都在、桌面对差全绿、界面在说假话。
 *    ⇒ 那批表里非豁免的每一条现在**逐条进本门的 B 面**（见 `scanParityRegisterFace`），
 *      债务因此从 44 涨到 70，复审把那道门的射程补完之后再涨到 122。
 *      涨的是能见度，不是死代码 —— 2026-09-14 已逐条接线清零。
 *      ⚠️ 这一格的**射程仍然由那道门自己的取材面决定**：它今天认「`ui/src/components/screens/`
 *      下每个含 `.tsx` 的目录都被某张表认领或显式交接」，认「`<button>` ∪ 带 `onClick=` /
 *      动作性 `role=` 的元素」，认「渲染一跳」。三条里任何一条再放宽，这个数字还会涨。
 * 3. **守不住译文质量**：只管「这句话在不在」，不管它说得对不对。
 * 4. **守不住后端**：三面全在 `ui/`。「前端接了、后端命令不存在」由 IPC 契约门管。
 * 5. **A1/A2 两条信号都是启发式**：同时把键名与文案写得人畜无害的一条缺席声明会溜过 A 面。
 *    兜底只剩 B/C 面（要么它登记了，要么它画了一颗死控件）；三样都躲开 = 功能整块没画 = 第 2 条。
 *
 * # 怎么跑（一条命令拿到「还剩几条、分别是哪些」）
 *
 *     cd ui
 *     POLARIS_WIRING_REPORT=1 pnpm exec vitest run --silent=false --reporter=verbose src/mobile/wiring-completeness.test.ts src/mobile/half-truth-facts.test.ts
 *
 * 直接保留 Vitest 的真实退出码；两份事实门与下方完成门共同判定，不再由报告壳翻译结果。
 *
 * # 完成门
 *
 * 「三个取材面减去处置表 === 空集」是一道硬门（2026-09-14 清零起）：再出现任何一条未接线项都会
 * 当场红，报错逐条列出剩余项；报告输出不代替断言。
 */
import { describe, expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { blankComments } from '@/contracts/comment-blanking.test-support';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';
import {
  ADD_ACTIONS,
  BATCH_ACTIONS,
  MESH_JOIN_ACTIONS,
  MESH_JOIN_CHOICES,
  ROW_ACTIONS,
  SUB_MENU_ACTIONS,
  SUB_PENDING_FIELDS,
  type BatchActionEntry,
  type MeshJoinChoiceEntry,
  type RowActionEntry,
} from './nodes/absence-register';
import { isDebt, type ParityEntry, type ScreenParityRegister } from './parity-registry.test-support';
import { HOME_PARITY } from './home/absence-register.test-support';
import { RULES_PARITY } from './screens/rules/absence-register.test-support';
import { CONNECTIONS_PARITY } from './connections/absence-register.test-support';
import { SETTINGS_PARITY } from './settings/absence-register.test-support';
import { MOBILE_FEEDS } from './feed-register.test-support';
import { nodeFormParityDebt } from './forms/node-form-parity.test-support';

/**
 * 四屏对差表。**不从 `screen-parity.test.ts` 转手 import** —— 那会在收集阶段之外执行它的
 * `describe`，vitest 当场抛。两处各自 import 同一批数据模块，真值源仍只有一份。
 */
const SCREEN_REGISTERS: readonly ScreenParityRegister[] = [
  HOME_PARITY,
  RULES_PARITY,
  CONNECTIONS_PARITY,
  SETTINGS_PARITY,
];

const MOBILE_DIR = fileURLToPath(new URL('.', import.meta.url));
const UI_SRC_DIR = fileURLToPath(new URL('..', import.meta.url));
const LOCALES_DIR = fileURLToPath(new URL('../i18n/locales', import.meta.url));
/** 本文件自身的绝对路径 —— 三个面**都**必须把它排除掉（它自己就写满了取材面上的词）。 */
const SELF = fileURLToPath(import.meta.url);
/**
 * B 面的真值源。A 面把它们排除在「消费文件」之外：登记 ≠ 渲染，两面各数一遍是把账做虚。
 *
 * 2026-09-06 从一张扩到五张：四屏各自的**与桌面逐块对差表**（`screen-parity.test.ts` 的取材面）
 * 与节点屏那张同形，逐条非豁免处置照样是一条债。**必须与「登记表唯一性」那条断言用同一个常量** ——
 * 第六张表长出来时，要么它被这里收进 B 面，要么那条断言红，不许两者都躲过去。
 */
const REGISTER_FILES: readonly string[] = [
  join(MOBILE_DIR, 'nodes', 'absence-register.ts'),
  join(MOBILE_DIR, 'parity-registry.test-support.ts'),
  join(MOBILE_DIR, 'home', 'absence-register.test-support.ts'),
  join(MOBILE_DIR, 'screens', 'rules', 'absence-register.test-support.ts'),
  join(MOBILE_DIR, 'connections', 'absence-register.test-support.ts'),
  join(MOBILE_DIR, 'settings', 'absence-register.test-support.ts'),
  join(MOBILE_DIR, 'feed-register.test-support.ts'),
  /* 2026-09-24（移动端 P1）：节点编辑器桌面 ↔ 移动端能力对差（指纹从 `NodeDialog.tsx` 源码派生）。 */
  join(MOBILE_DIR, 'forms', 'node-form-parity.test-support.ts'),
];

const rel = (absolute: string): string => relative(UI_SRC_DIR, absolute).replaceAll('\\', '/');

/* ────────────────────────── 公共工具 ────────────────────────── */

/**
 * 把注释内容抹成空格，**行数与列数都不变**（下游要按行号定位）。
 *
 * 为什么必须做：控件面第一版直接对原文分行，于是 `// 这颗按钮在核未运行时 disabled` 这样一句
 * 解释性注释会被算成一处「写死禁用的控件」—— 一条没有控件可接、只能靠改注释措辞消掉的幽灵待办。
 * 而这份代码里到处是解释「为什么这颗按钮 disabled」的中文注释。
 *
 * 🔴 实现**不在这里**：`contracts/comment-blanking.test-support.ts`，与 `screen-parity.test.ts`
 * 共用同一份。两道门需要同一个「什么算注释」，各留一份拷贝就会出现同一段文本在一道门里是注释、
 * 在另一道门里是代码。射程自曝写在那份文件的头注里（跨行模板字符串的续行会被当代码 —— 保守方向）。
 */

function walkTypeScript(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir).sort()) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walkTypeScript(full, out);
    else if (/\.tsx?$/.test(full)) out.push(full);
  }
  return out;
}

/* ────────────── 移动端可达的模块闭包（控件面 / 消费点面的文件面） ────────────── */

/**
 * 桌面的入口清单。闭包判据靠它区分「共享模块」与「只有移动端才用得到的模块」。
 * 少一个入口会让一大批共享模块被误判成移动端专属 ⇒ 下面有一条断言钉住它们都还在。
 */
const DESKTOP_ENTRIES: readonly string[] = [
  'main.tsx',
  'App.tsx',
  'components/layout/AppShell.tsx',
  'tray/main.tsx',
  'update-popup/main.ts',
];

function resolveImport(fromFile: string, specifier: string): string | null {
  let base: string;
  if (specifier.startsWith('@/')) base = join(UI_SRC_DIR, specifier.slice(2));
  else if (specifier.startsWith('.')) base = resolve(dirname(fromFile), specifier);
  else return null;
  for (const candidate of [
    base,
    `${base}.ts`,
    `${base}.tsx`,
    join(base, 'index.ts'),
    join(base, 'index.tsx'),
  ]) {
    if (existsSync(candidate) && statSync(candidate).isFile() && /\.tsx?$/.test(candidate)) {
      return candidate;
    }
  }
  return null;
}

function buildImportGraph(files: readonly string[]): Map<string, string[]> {
  const graph = new Map<string, string[]>();
  for (const file of files) {
    const text = blankComments(readFileSync(file, 'utf8'));
    const specifiers = [...text.matchAll(/(?:from|import)\s*\(?\s*['"]([^'"]+)['"]/g)].map((m) => m[1]!);
    graph.set(
      file,
      [...new Set(specifiers.map((s) => resolveImport(file, s)).filter((f): f is string => f !== null))],
    );
  }
  return graph;
}

function reachableFrom(roots: readonly string[], graph: ReadonlyMap<string, string[]>): Set<string> {
  const seen = new Set<string>();
  const stack = [...roots];
  while (stack.length > 0) {
    const file = stack.pop()!;
    if (seen.has(file)) continue;
    seen.add(file);
    for (const next of graph.get(file) ?? []) stack.push(next);
  }
  return seen;
}

/* ────────────────────────── A 面：i18n ────────────────────────── */

type Phrase = (text: string) => boolean;
/** 两条正则都要命中同一段文本 —— 用来给「提到桌面」加上「而且是在叫你去桌面做」这层限定。 */
const both =
  (a: RegExp, b: RegExp): Phrase =>
  (text) =>
    a.test(text) && b.test(text);
const matches =
  (pattern: RegExp): Phrase =>
  (text) =>
    pattern.test(text);

/**
 * 「缺席短语族」（A1 信号）。分族不是装饰：报告里要说得出**是哪一类**缺席，
 * 而反向对照要能证明**每一族**都真的认得出自己的形态、且**不**认那一族对应的良性文案。
 *
 * 英文式与中文式分开写、取并集，因为同一条缺口在五个语种里的落地进度不同步。
 *
 * 2026-09-06 复审后两个方向各改了一次，两个方向都有实测样本压在 ② 组里：
 *  · **放宽**（漏了真缺口）：`还没接` / `没接上` / `尚未登记` / `未登记进`，
 *    以及英文 `neither … is wired` / `not registered … yet` / `not offered on mobile yet`。
 *  · **收紧**（误伤良性文案）：裸 `桌面端`、裸 `不提供`、裸 `on the desktop`、裸 `desktop app`
 *    命中的是「提到桌面 / 提到不提供」，不是「因为平台缺席所以去桌面」。跨平台客户端里
 *    「设置已与桌面端实时同步」「免费套餐不提供高级节点」是常规写法，它们一旦落进 mobile*
 *    命名空间就变成一条永远修不掉、且在处置表口径下无处安放的假待办 —— 那会逼下一个人去削这道门。
 */
const ABSENCE_PHRASES: ReadonlyArray<readonly [string, Phrase]> = [
  [
    'zh-notwired',
    matches(
      /未接[线綫]|[暂暫]未接入|尚未接入|待接[线綫]|[还還][没沒]接|[没沒]接上|尚未登[记記]|未登[记記][进進]/,
    ),
  ],
  [
    'zh-godesktop',
    both(
      /桌面(?:端|版|客[户戶]端|系统|系統)?/,
      /[请請]|需(?:要|在)|得在|[才方](?:能|可)|只能|去桌面|上(?:操作|[设設]置|修改|完成|[删刪]除|新建|[编編][辑輯])|[后後]同步/,
    ),
  ],
  [
    'zh-noplatform',
    matches(
      /(?:本|此|[该該]|目前|[当當]前)平台(?:上)?(?:[暂暫])?不|(?:移[动動]端|行[动動]端|手机端|手機端)(?:上)?(?:[暂暫])?不(?:提供|支持|支援|可用)|(?:[暂暫])?不在(?:移[动動]端|行[动動]端)(?:提供|支持|支援)|不提供(?:此|[该該]|[这這]|本)?(?:操作|功能|能力|入口|[选選][项項])/,
    ),
  ],
  [
    'en-notwired',
    matches(
      /\bnot (?:yet )?(?:wired|hooked)(?: up)?\b|\bneither\b[^.]{0,90}\bis wired\b|\bnot registered\b[^.]{0,60}\byet\b|\bnot (?:offered|available|supported)\b[^.]{0,40}\bon mobile\b[^.]{0,12}\byet\b|\bnot implemented\b|\bcoming soon\b/i,
    ),
  ],
  [
    'en-godesktop',
    both(
      /\bdesktop\b/i,
      /\b(?:has to be|have to be|must be|do it|only|instead|go to the|use the desktop|(?:edit|delete|create|add|remove|set|change|manage|pick)(?: it)? on)\b/i,
    ),
  ],
  [
    'en-noplatform',
    matches(
      /this platform does not|not (?:available|supported|offered) on this platform|does not (?:provide|offer|support) (?:this|that|the|any)\b/i,
    ),
  ],
];

/**
 * 「缺席键名族」（A2 信号）—— 与文案完全独立的第二条判据。
 *
 * 只在**移动端自有**的键上生效（顶层命名空间在 `MOBILE_NAMESPACES`，或路径里带 `mobile` 段）。
 * 不放开到全仓是因为 `home.proxyExitUnavailable` / `logs.coreLevelPending` 这一批是**运行期状态**，
 * 不是平台缺席；把它们算进来会给账本灌一批永远修不掉的假债。
 */
const ABSENCE_KEY_NAMES =
  /Absent|Unavailable|Unsupported|NotWired|Unregistered|PlatformNote|Pending(?:Fields|Note)?$|NeedsDesktop|mobileNeeds/;

/** 移动端自有的顶层命名空间（`locales/en-US.json` 的顶层键里以 `mobile` 起头的那批）。 */
const MOBILE_NAMESPACES: ReadonlySet<string> = new Set([
  'mobileNav',
  'mobileHome',
  'mobileActions',
  'mobileConnections',
  'mobileRules',
  'mobileSettings',
  'mobileHelp',
  'mobileMeshInbound',
  'mobileMeshRouteEvidence',
  'mobileSpeedTest',
]);

/** 这条键是不是「移动端自有」的 —— A2 的作用域。 */
function isMobileOwnedKey(key: string): boolean {
  const segments = key.split('.');
  return (
    MOBILE_NAMESPACES.has(segments[0] ?? '') ||
    segments.some((segment) => /^mobile/i.test(segment) || /[Mm]obile/.test(segment))
  );
}

type FlatLocale = Record<string, string>;
interface LocaleFile {
  readonly file: string;
  readonly flat: FlatLocale;
}

function flattenLocale(node: unknown, prefix = '', out: FlatLocale = {}): FlatLocale {
  if (node !== null && typeof node === 'object') {
    for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
      flattenLocale(value, prefix ? `${prefix}.${key}` : key, out);
    }
  } else if (typeof node === 'string') {
    out[prefix] = node;
  }
  return out;
}

function readLocaleFiles(): LocaleFile[] {
  const dirs = [LOCALES_DIR, join(LOCALES_DIR, 'auxiliary')];
  const files: LocaleFile[] = [];
  for (const dir of dirs) {
    for (const entry of readdirSync(dir).sort()) {
      if (!entry.endsWith('.json')) continue;
      const full = join(dir, entry);
      files.push({
        file: rel(full),
        flat: flattenLocale(JSON.parse(readFileSync(full, 'utf8')) as unknown),
      });
    }
  }
  return files;
}

function matchedFamilies(text: string): string[] {
  return ABSENCE_PHRASES.filter(([, hit]) => hit(text)).map(([name]) => name);
}

/* ────────────────────────── 取材面的公共形状 ────────────────────────── */

type FaceName = 'i18n' | 'register' | 'control';

interface FaceItem {
  /** 全局唯一、可写进处置表的锚。腐烂检测拿它逐字对差。 */
  readonly id: string;
  readonly face: FaceName;
  /** 人读的位置（相对 `ui/src/`，或 i18n 键）。 */
  readonly where: string;
  /** 人读的证据行 —— 报告里跟在位置后面那一行。 */
  readonly detail: string;
  /**
   * 归并键：同一件工作在三个面上的影子共用一个值。
   * 报告靠它说出「N 条待办 / M 件工作」—— 29 条里有 11 条是另外 4 件工作的影子，
   * 平铺出来读的人得自己回去读 `reasonKey` 才知道哪几条是一件事。
   */
  readonly work: string;
}

/** 一条键在移动端源码里的消费点。 */
interface ConsumePoint {
  readonly file: string;
  readonly line: number;
  readonly kind: 'literal' | 'composed';
}

/**
 * 键在移动端生产源码里的消费点，**逐处**列出（注释已抹空，注释里提一句不算渲染）。
 *
 * 两种消费形态：
 *  · `literal` —— 带引号定界的字面量键（避免 `a.b` 撞上 `a.bc`）。
 *  · `composed` —— 拼键：`` t(`home.unlockStatus.${x}`) `` 这种模板串把父路径写在里面。
 *    本仓移动端有 8 处拼键，第一版只认字面量 ⇒ 「非 mobile* 命名空间 + 拼键消费」是一条静默通道。
 */
function consumersOf(key: string, sources: ReadonlyMap<string, string>): ConsumePoint[] {
  const needles = [`'${key}'`, `"${key}"`, `\`${key}\``];
  const parent = key.slice(0, key.lastIndexOf('.'));
  const composedNeedle = parent.length > 0 ? `${parent}.\${` : null;
  const points: ConsumePoint[] = [];
  for (const [file, text] of [...sources].sort(([a], [b]) => (a < b ? -1 : 1))) {
    text.split('\n').forEach((line, index) => {
      if (needles.some((needle) => line.includes(needle))) {
        points.push({ file: rel(file), line: index + 1, kind: 'literal' });
      } else if (composedNeedle !== null && line.includes(composedNeedle)) {
        points.push({ file: rel(file), line: index + 1, kind: 'composed' });
      }
    });
  }
  return points;
}

interface ClaimedKey {
  readonly key: string;
  readonly families: string[];
  readonly sample: string;
}

/** 被 A1 或 A2 认领的键（跨语种并集）。 */
function claimedKeys(locales: readonly LocaleFile[]): ClaimedKey[] {
  const byKey = new Map<string, { families: Set<string>; samples: string[] }>();
  for (const { file, flat } of locales) {
    for (const [key, value] of Object.entries(flat)) {
      const families = matchedFamilies(value);
      if (isMobileOwnedKey(key) && ABSENCE_KEY_NAMES.test(key)) families.push('key-name');
      if (families.length === 0) continue;
      const entry = byKey.get(key) ?? { families: new Set<string>(), samples: [] };
      families.forEach((family) => entry.families.add(family));
      entry.samples.push(`${file.split('/').pop()}: ${value}`);
      byKey.set(key, entry);
    }
  }
  return [...byKey]
    .sort(([a], [b]) => (a < b ? -1 : 1))
    .map(([key, entry]) => ({
      key,
      families: [...entry.families].sort(),
      sample: entry.samples[0] ?? '',
    }));
}

/**
 * i18n 面：**逐渲染点**成项。
 *
 * id 形如 `i18n:<键>@<文件>#<该文件内第几处>`。用序号而不是行号：行号会被上方任何一次
 * 无关改动推走，让处置表的腐烂检测把「只是挪了一行」误报成「已过期，删掉它」。
 * 没有任何消费点的键（命名空间归属、或只被登记表引用）落成一条无位置的项。
 */
function scanI18nFace(
  locales: readonly LocaleFile[],
  sources: ReadonlyMap<string, string>,
): FaceItem[] {
  const items: FaceItem[] = [];
  for (const { key, families, sample } of claimedKeys(locales)) {
    const consumers = consumersOf(key, sources);
    const namespaceOwned = MOBILE_NAMESPACES.has(key.split('.')[0] ?? '');
    const keyNameOwned = families.includes('key-name');
    if (!namespaceOwned && !keyNameOwned && consumers.length === 0) continue;
    const head = `[${families.join(',')}] ${sample}`;
    if (consumers.length === 0) {
      items.push({
        id: `i18n:${key}`,
        face: 'i18n',
        where: key,
        detail: `${head}\n            （命名空间/键名归属，无渲染点）`,
        work: key,
      });
      continue;
    }
    const seen = new Map<string, number>();
    for (const point of consumers) {
      const ordinal = (seen.get(point.file) ?? 0) + 1;
      seen.set(point.file, ordinal);
      items.push({
        id: `i18n:${key}@${point.file}#${ordinal}`,
        face: 'i18n',
        where: `${key} @ ${point.file}:${point.line}`,
        detail: `${head}\n            渲染点 ${point.kind}`,
        work: key,
      });
    }
  }
  return items;
}

/* ────────────────────────── B 面：登记表 ────────────────────────── */

function scanRegisterFace(
  rows: readonly RowActionEntry[],
  batch: readonly BatchActionEntry[],
  subPending: readonly string[],
  add: readonly RowActionEntry[],
  subMenu: readonly RowActionEntry[],
  meshJoin: readonly MeshJoinChoiceEntry[],
  meshJoinActions: readonly RowActionEntry[],
): FaceItem[] {
  const items: FaceItem[] = [];
  /* 三张 2026-09-06 新登记的表（「添加」四条 / 订阅「更多」五条 / 组网接入五个）与既有两张同形：
     处置不是 `ported` 的每一条逐条成项。它们此前**一条都不在任何面上** ——
     桌面多一项、移动端顺手删一项，两边都不会有东西红。 */
  const TABLE_NAME: Record<string, string> = {
    add: 'ADD_ACTIONS',
    'sub-menu': 'SUB_MENU_ACTIONS',
    /* 2026-09-06（批 3）新登记：组网接入面**卡片里的次动作**。上一版这 5 颗一颗都不在面上 ——
       那时两张卡片本身是灰的，次动作是「灰按钮底下的灰按钮」。卡片接通之后它们露出来了。 */
    'mesh-action': 'MESH_JOIN_ACTIONS',
  };
  for (const [prefix, table] of [
    ['add', add],
    ['sub-menu', subMenu],
    ['mesh-action', meshJoinActions],
  ] as const) {
    for (const entry of table) {
      if (entry.disposition.kind === 'ported') continue;
      items.push({
        id: `register:${prefix}:${entry.id}`,
        face: 'register',
        where: `absence-register.ts ${TABLE_NAME[prefix]}[${entry.id}]`,
        detail: `${entry.disposition.kind} · 理由键 ${entry.disposition.reasonKey}`,
        work: entry.disposition.reasonKey,
      });
    }
  }
  for (const entry of meshJoin) {
    if (entry.disposition.kind === 'ported') continue;
    items.push({
      id: `register:mesh-join:${entry.id}`,
      face: 'register',
      where: `absence-register.ts MESH_JOIN_CHOICES[${entry.id}]`,
      detail: `${entry.disposition.kind} · 理由键 ${entry.disposition.reasonKey}`,
      work: entry.disposition.reasonKey,
    });
  }
  for (const entry of rows) {
    if (entry.disposition.kind === 'ported') continue;
    items.push({
      id: `register:row:${entry.id}`,
      face: 'register',
      where: `absence-register.ts ROW_ACTIONS[${entry.id}]`,
      detail: `${entry.disposition.kind} · 理由键 ${entry.disposition.reasonKey}`,
      work: entry.disposition.reasonKey,
    });
  }
  for (const entry of batch) {
    if (entry.disposition.kind === 'ported') continue;
    items.push({
      id: `register:batch:${entry.id}`,
      face: 'register',
      where: `absence-register.ts BATCH_ACTIONS[${entry.id}]`,
      detail: `${entry.disposition.kind} · 理由键 ${entry.disposition.reasonKey}`,
      work: entry.disposition.reasonKey,
    });
  }
  for (const field of subPending) {
    items.push({
      id: `register:sub-pending:${field}`,
      face: 'register',
      where: `absence-register.ts SUB_PENDING_FIELDS[${field}]`,
      detail: '订阅摘要里没有登记数据源的位置（标为 pending 而不是画出来）',
      /* 这一格今天是空集（三处来源 2026-09-06 已登记进 data-contract）。`work` 是**分组标签**，
         不是 i18n 键 —— 旧值指着一条已经删掉的键，会让下一个人去找一句不存在的文案。
         下一条 pending 字段进来时，它自己的用户可见文案由那次改动一并给。 */
      work: 'register:sub-pending',
    });
  }
  return items;
}

/**
 * 四屏对差表的 B 面项：`disabled` / `absent` 两个码逐条成项，豁免四档（`ported` /
 * `reachable-elsewhere` / `platform-absent` / `adjudicated-out`）各自交了被机器打开的锚，不进债务。
 *
 * 🔴 **这一面进来那天债务会变大，而变大是对的**：这些条目一条都不是新造的死界面，
 * 它们是**此前没有任何一面看得见**的缺口（桌面有、移动端一行没写、也没人登记）——
 * 主门头注「射程自曝」第 2 条自己写着守不住这一档。数字变大 = 能见度变大。
 */
function scanParityRegisterFace(registers: readonly ScreenParityRegister[]): FaceItem[] {
  const items: FaceItem[] = [];
  const emit = (screen: string, kind: 'action' | 'block', entry: ParityEntry): void => {
    if (!isDebt(entry.disposition)) return;
    const reasonKey =
      entry.disposition.kind === 'disabled' || entry.disposition.kind === 'absent'
        ? entry.disposition.reasonKey
        : undefined;
    items.push({
      id: `parity:${screen}:${kind}:${entry.id}`,
      face: 'register',
      where: `${screen}/absence-register.ts ${kind}[${entry.id}]`,
      detail: `${entry.disposition.kind}${reasonKey ? ` · 理由键 ${reasonKey}` : ' · 界面上一个字都没提'}`,
      // 没有理由键的那一档**不并到任何一件已知工作上**：它就是自己一件工作，
      // 而且是最坏的一档（用户看不到任何提示）。并进去会让「N 件工作」把它藏起来。
      work: reasonKey ?? `parity:${screen}:${entry.id}`,
    });
  };
  for (const register of registers) {
    for (const entry of register.actions) emit(register.screen, 'action', entry);
    for (const entry of register.blocks) emit(register.screen, 'block', entry);
  }
  // 喂数腿面同口径：一格「读点在、写入方一个都没有」的状态就是一条债（W-23 的形状）。
  // 有写入方的（`ported`）与「刻意没有、恒初值即正确」的（`platform-absent`）各自交了锚，不进债务。
  for (const entry of MOBILE_FEEDS) {
    if (!isDebt(entry.disposition)) continue;
    const reasonKey =
      entry.disposition.kind === 'disabled' || entry.disposition.kind === 'absent'
        ? entry.disposition.reasonKey
        : undefined;
    items.push({
      id: `parity:feed:${entry.id}`,
      face: 'register',
      where: `feed-register.ts ${entry.id}`,
      detail: `${entry.disposition.kind} · 读点在、没有写入方${reasonKey ? ` · 理由键 ${reasonKey}` : '，界面上一个字都没提'}`,
      work: reasonKey ?? `parity:feed:${entry.id}`,
    });
  }
  return items;
}

/** 登记表里每条非 `ported` 处置的理由键（正面断言用：它们必须被缺席信号认领）。 */
function registerReasonKeys(): ReadonlyArray<readonly [string, string]> {
  const out: Array<readonly [string, string]> = [];
  for (const entry of ROW_ACTIONS) {
    if (entry.disposition.kind !== 'ported') out.push([`register:row:${entry.id}`, entry.disposition.reasonKey]);
  }
  for (const entry of BATCH_ACTIONS) {
    if (entry.disposition.kind !== 'ported') out.push([`register:batch:${entry.id}`, entry.disposition.reasonKey]);
  }
  for (const entry of ADD_ACTIONS) {
    if (entry.disposition.kind !== 'ported') out.push([`register:add:${entry.id}`, entry.disposition.reasonKey]);
  }
  for (const entry of SUB_MENU_ACTIONS) {
    if (entry.disposition.kind !== 'ported') out.push([`register:sub-menu:${entry.id}`, entry.disposition.reasonKey]);
  }
  for (const entry of MESH_JOIN_CHOICES) {
    if (entry.disposition.kind !== 'ported')
      out.push([`register:mesh-join:${entry.id}`, entry.disposition.reasonKey]);
  }
  for (const entry of MESH_JOIN_ACTIONS) {
    if (entry.disposition.kind !== 'ported')
      out.push([`register:mesh-action:${entry.id}`, entry.disposition.reasonKey]);
  }
  return out;
}

/* ────────────────────────── C 面：控件 ────────────────────────── */

type ControlMatcher = (line: string, constTrue: ReadonlySet<string>) => boolean;

/**
 * 六种「写死成不可用」的词法形态。每一条都在 ② 组的反向对照里各自证明过认得出，
 * 并各自证明过**不**认那个长得像但其实是动态的对照样本（否则它会把整仓的 `disabled={busy}` 全算成债）。
 *
 * 2026-09-06 复审后补的三种，都是实测「加一颗死控件而债务增量为 0」的现成走法：
 *  · `const-folded-disabled` —— `disabled={NEVER}` 配一行 `const NEVER = true;`（同文件常量折叠）；
 *  · `aria-disabled` —— 用 ARIA 而不是原生属性禁用；
 *  · `const-disabled-reason` —— `disabledReason: t('…')` 无条件常量理由，本仓行动作面的主力写法
 *    （`MobileNodesScreen.tsx` 那两处），而 `disabledReason: speedTestBlockedReason(...)` 是动态的，不认。
 * 同时 `inert-handler` 从只认 JSX 的 `onSelect={…}` 扩到也认对象字面量的 `onSelect: …`
 * —— 后者正是本仓行动作面的写法，第一版整条看不见。
 */
const CONTROL_PATTERNS: ReadonlyArray<readonly [string, ControlMatcher]> = [
  // JSX 上的裸 `disabled`：后面跟 `>`、`/>`、另一个属性名（含 `aria-*` 这种带连字符的），或行尾。
  ['bare-disabled', (line) => /(?:^|[\s{])disabled(?=\s*(?:\/?>|[A-Za-z_$][\w$-]*\s*=|$))/.test(line)],
  [
    'const-true-disabled',
    (line) => /\bdisabled\s*[:=]\s*(?:\{\s*true(?:\s+as\s+\w+)?\s*\}|true\b)/.test(line),
  ],
  [
    'const-folded-disabled',
    (line, constTrue) => {
      const hit = /\bdisabled\s*=\s*\{\s*([A-Za-z_$][\w$]*)\s*(?:as\s+\w+\s*)?\}/.exec(line);
      return hit !== null && constTrue.has(hit[1]!);
    },
  ],
  ['aria-disabled', (line) => /\baria-disabled\s*=\s*(?:"true"|'true'|\{\s*true\s*\})/.test(line)],
  [
    'inert-handler',
    (line) =>
      /\bon(?:Click|Select|Press|Tap)\s*[:=]\s*\{?\s*\(\s*\)\s*=>\s*(?:undefined|\{\s*\})\s*[},)]/.test(
        line,
      ),
  ],
  ['const-disabled-reason', (line) => /\bdisabledReason\s*:\s*(?:t\(\s*['"`]|['"`])/.test(line)],
];

/** 同文件里 `const X = true` 的名字集合 —— 给 `const-folded-disabled` 做常量折叠。 */
function constTrueNames(text: string): Set<string> {
  const names = new Set<string>();
  for (const hit of text.matchAll(/\bconst\s+([A-Za-z_$][\w$]*)\s*(?::\s*[\w<>\[\]|\s]+)?=\s*true\b/g)) {
    names.add(hit[1]!);
  }
  return names;
}

/**
 * 控件面的归并键：在命中行**附近**找一条被缺席信号认领的 i18n 键。
 *
 * 一颗死按钮与解释它为什么死的那句话通常隔着几行（`BackupPage.tsx` 的两颗按钮在 :74/:77，
 * 说明在 :81）。找得到就把这颗控件挂到那件工作上，找不到才退回文件级。
 * 这个字段**只进报告**（「N 条待办 / M 件工作」），不参与任何断言 —— 归并猜错不会让门误判。
 */
function nearbyAbsenceKey(
  lines: readonly string[],
  index: number,
  claimed: ReadonlySet<string>,
): string | null {
  for (let cursor = Math.max(0, index - 4); cursor < Math.min(lines.length, index + 11); cursor += 1) {
    for (const hit of (lines[cursor] ?? '').matchAll(/['"`]([a-z][\w]*(?:\.[\w-]+){1,5})['"`]/gi)) {
      if (claimed.has(hit[1]!)) return hit[1]!;
    }
  }
  return null;
}

function scanControlFace(
  sources: ReadonlyMap<string, string>,
  claimed: ReadonlySet<string> = new Set(),
): FaceItem[] {
  const items: FaceItem[] = [];
  for (const [file, text] of [...sources].sort(([a], [b]) => (a < b ? -1 : 1))) {
    const constTrue = constTrueNames(text);
    const ordinals = new Map<string, number>();
    const lines = text.split('\n');
    lines.forEach((line, index) => {
      // 一行同时命中多种形态（`<MobileButton disabled onClick={() => undefined}>` 就同时是
      // 裸 disabled 与空处理器）只算**一个控件**：这个数是「还欠几处接线」，不是「命中几次正则」。
      const names = CONTROL_PATTERNS.filter(([, hit]) => hit(line, constTrue)).map(([name]) => name);
      if (names.length === 0) return;
      const shape = names.join('+');
      // id 用「文件 + 命中形态 + 该行去空白后的短指纹」，**不带行号**，理由有两条：
      //  ① 带行号时，同文件上方任何一次无关改动都会让处置表的腐烂检测假红，而它给的指导是错的
      //     （条目没过期，只是挪了一行）；
      //  ② 只按「本文件里第几处」编号也不行：同文件同形态的一增一减会被序号吸收 ——
      //     把一颗死按钮改成动态（−1）、同时新交付另一颗死按钮（+1），id 集合逐字不变，
      //     一颗全新的未接线控件就这么静默进了仓。指纹跟着**内容**走，换了内容就换了 id。
      //  ③ 指纹取的是**命中行加它后面两行**，不是单行：本仓的死按钮长这样 ——
      //     `<button type="button" className="mr-btn" disabled>` 三颗逐字相同，区分它们的标签
      //     （`t('resources.catalog')` / `urlDownload` / `import`）在**下一行**。只取单行时，
      //     「把第一颗改成动态、同时新交付一颗」的一增一减仍然被序号吸收，实测全绿。
      //     代价如实登记：改一句按钮标签也会换 id ⇒ 处置表里那一条要跟着改。
      //     这是「宁可多问一次」的取向 —— 静默放行一颗新死按钮比多改一次处置表贵得多。
      const context = lines
        .slice(index, index + 3)
        .join(' ')
        .replace(/\s+/g, ' ')
        .trim();
      const fingerprint = createHash('sha1').update(context).digest('hex').slice(0, 8);
      const slot = `${shape}:${fingerprint}`;
      const ordinal = (ordinals.get(slot) ?? 0) + 1;
      ordinals.set(slot, ordinal);
      items.push({
        // 同一份文本在同一文件里出现多次时才加序号（两颗逐字相同的死按钮）。
        id: `control:${rel(file)}:${slot}#${ordinal}`,
        face: 'control',
        where: `${rel(file)}:${index + 1}`,
        detail: `[${shape}] ${line.trim()}`,
        work: nearbyAbsenceKey(lines, index, claimed) ?? `control:${rel(file)}`,
      });
    });
  }
  return items;
}

/* ────────────────────────── 真实取材 ────────────────────────── */

/** 未过滤的全量（只给切点自检用：要证明「本文件本来在这张单子上」）。 */
const ALL_UI_FILES = walkTypeScript(UI_SRC_DIR);
const NON_TEST_FILES = ALL_UI_FILES.filter((file) => !IS_TEST_ONLY_MODULE.test(file));
const IMPORT_GRAPH = buildImportGraph(NON_TEST_FILES);
const DESKTOP_ENTRY_FILES = DESKTOP_ENTRIES.map((entry) => join(UI_SRC_DIR, entry)).filter((file) =>
  existsSync(file),
);
const MOBILE_ROOTS = NON_TEST_FILES.filter((file) => file.startsWith(`${MOBILE_DIR}`));
const REACHABLE_FROM_MOBILE = reachableFrom(MOBILE_ROOTS, IMPORT_GRAPH);
const REACHABLE_FROM_DESKTOP = reachableFrom(DESKTOP_ENTRY_FILES, IMPORT_GRAPH);
/** 「从移动端可达、且桌面任何入口都够不着」的共享模块 —— 把死控件抽进去那条路的落点。 */
const MOBILE_ONLY_SHARED = [...REACHABLE_FROM_MOBILE]
  .filter((file) => !file.startsWith(`${MOBILE_DIR}`) && !REACHABLE_FROM_DESKTOP.has(file))
  .sort();

/** 真正的取材面：移动端可达的非测试生产源码。本文件自己被 `IS_TEST_ONLY_MODULE` 挡在外面。 */
const PRODUCTION_FILES = [...MOBILE_ROOTS, ...MOBILE_ONLY_SHARED].sort();
const PRODUCTION_SOURCES: ReadonlyMap<string, string> = new Map(
  PRODUCTION_FILES.map((file) => [file, blankComments(readFileSync(file, 'utf8'))]),
);
/** A 面的消费文件面：把 B 面的真值源摘掉（登记 ≠ 渲染）。 */
const CONSUMER_SOURCES: ReadonlyMap<string, string> = new Map(
  [...PRODUCTION_SOURCES].filter(([file]) => !REGISTER_FILES.includes(file)),
);
const LOCALE_FILES = readLocaleFiles();

/**
 * 登记表引用的理由键 —— A 面拿它做**去重**，不是拿它放行。
 *
 * 一条键如果在移动端源码里一个渲染点都没有、而它正是登记表的 `reasonKey`（`nodes.mobileNeedsDeleteLeg`
 * 就是这样），那它在 B 面已经逐条记着账了；A 面再记一条，同一件工作在账本上出现两次，
 * 「还欠几处」这个数就虚了。只在**无渲染点**这一档去重：一旦它真的被画到屏上，那处渲染点照记不误。
 * 而「登记一条缺席、理由文案却读不出缺席」那条静默通道由 ③ 组那条正面断言堵着。
 */
const REGISTER_REASON_KEYS: ReadonlySet<string> = new Set(
  registerReasonKeys().map(([, reasonKey]) => reasonKey),
);

const I18N_FACE = scanI18nFace(LOCALE_FILES, CONSUMER_SOURCES).filter(
  (item) => !(!item.id.includes('@') && REGISTER_REASON_KEYS.has(item.work)),
);
/* 三批各改过这一处：批 2 给节点屏加了三张表（ADD/SUB_MENU/MESH_JOIN），批 6 加了四屏的
   面级登记表，批 3（mesh）又加了组网卡片里的次动作表（MESH_JOIN_ACTIONS）。三者都要，故取并集。 */
const REGISTER_FACE = [
  ...scanRegisterFace(
    ROW_ACTIONS,
    BATCH_ACTIONS,
    SUB_PENDING_FIELDS,
    ADD_ACTIONS,
    SUB_MENU_ACTIONS,
    MESH_JOIN_CHOICES,
    MESH_JOIN_ACTIONS,
  ),
  ...scanParityRegisterFace(SCREEN_REGISTERS),
  /* 2026-09-24（移动端 P1）：节点编辑器的表单级能力（桌面 `NodeDialog.tsx` 消费、移动端
     `NodeFormPanel.tsx` 没消费也没登记的每一条能力指纹）。此前这一格不在任何面上：字段级新增
     两端共用 `ND_SPEC` 天然同到，表单级新增（Tailcat 密钥对 / 保存前同宽门 / 无地址协议…）
     各写一份，桌面加了、移动端没跟，没有任何东西红。口径与射程自曝见那份 support 文件头注。 */
  ...nodeFormParityDebt().map((item) => ({ ...item, face: 'register' as const })),
];
const CONTROL_FACE = scanControlFace(
  PRODUCTION_SOURCES,
  new Set(claimedKeys(LOCALE_FILES).map((entry) => entry.key)),
);
const ALL_FACE_ITEMS: readonly FaceItem[] = [...I18N_FACE, ...REGISTER_FACE, ...CONTROL_FACE];

/* ────────────────────────── 处置表 ────────────────────────── */

/** 被机器打开核对的锚：`file` 相对仓根，必须存在，且必须真的含 `mustContain`。 */
interface Anchor {
  readonly file: string;
  readonly mustContain: string;
}

interface DispositionEntry {
  /** 必须与某个取材面项的 `id` 逐字相同，否则腐烂检测转红。 */
  readonly id: string;
  readonly face: FaceName;
  /**
   * · `platform-absent` —— 平台不提供这个能力。
   * · `model-absent` —— 数据模型里没有这个对象，两端同缺（锚里必须有一条落在移动树之外）。
   * · `reachable-elsewhere` —— 这根本不是缺席：能力在移动端别处就有，这句文案是指路牌。
   */
  readonly kind: 'platform-absent' | 'model-absent' | 'reachable-elsewhere';
  /** 给人读的依据。措辞受 `EXCUSE_WORDS` 约束。 */
  readonly evidence: string;
  /** 给机器核对的依据。空数组不允许。 */
  readonly anchors: readonly Anchor[];
  /** 复核日期 `YYYY-MM-DD`。 */
  readonly reviewed: string;
}

const REPO_DIR = fileURLToPath(new URL('../../../', import.meta.url));

/**
 * 🔴 **只装「平台不提供」与「其实在别处就有」，不装「我们还没做」。**
 * 每一条的 `anchors` 都被下面的断言真的打开核对；写不出这样一条 anchor 的，就该留在债务里红着。
 */
const DISPOSITIONS: readonly DispositionEntry[] = [
  {
    id: 'i18n:mobileRules.customAppPackagesUnsupported@mobile/forms/AppAddPanel.tsx#1',
    face: 'i18n',
    kind: 'platform-absent',
    evidence:
      '这句仅在 iOS 的 Android 包选择区渲染：Android applicationId 枚举由 Android 原生桥提供，' +
      '非 Android 分支明确拒绝；生成器在 iOS 选择 ProcessName 而非 PackageName。' +
      '表单在 iOS 不调用该枚举，也不提供 Android 包选择控件，域名/IP 资源配置和保存入口保留。',
    anchors: [
      { file: 'src-tauri/src/runtime/proxy/android_bridge.rs', mustContain: '"已装应用枚举只有 Android 腿（桌面的对应物是进程名，见 system_list_processes）"' },
      { file: 'crates/config-engine/src/builder/route.rs', mustContain: 'Platform::Ios => AppOwnerLeg::ProcessName,' },
      { file: 'ui/src/mobile/forms/AppAddPanel.tsx', mustContain: "if (isIOS) return;\n    let alive = true;" },
      { file: 'ui/src/mobile/forms/AppAddPanel.tsx', mustContain: "<p className=\"m-form-hint\">{tr('mobileRules.customAppPackagesUnsupported')}</p>" },
    ],
    reviewed: '2026-11-02',
  },
  {
    id: 'i18n:mobileSettings.backup.systemBackupUnsupported@mobile/settings/BackupPage.tsx#1',
    face: 'i18n',
    kind: 'platform-absent',
    evidence:
      '所述对象是 Android BackupAgent/noBackupFilesDir 的运行期开关，并非通用手动备份。' +
      '非 Android 的执行侧查询明确拒绝；iOS 分支只显示不适用说明，不挂载会读写该开关的 SystemBackupGroup。' +
      'iOS 的分类导出、文件选择和恢复能力仍由同页独立入口提供，不在此处豁免。',
    anchors: [
      { file: 'src-tauri/src/runtime/proxy/android_bridge.rs', mustContain: 'Err("system backup toggle is not available on this platform".to_string())' },
      { file: 'ui/src/mobile/settings/BackupPage.tsx', mustContain: "desc={t('mobileSettings.backup.systemBackupUnsupported')} />" },
      { file: 'ui/src/mobile/settings/BackupPage.tsx', mustContain: ') : <SystemBackupGroup commit={commit} />}' },
    ],
    reviewed: '2026-11-02',
  },
  {
    id: 'i18n:mobileSettings.general.autoStartUnsupported@mobile/settings/GeneralPage.tsx#1',
    face: 'i18n',
    kind: 'platform-absent',
    evidence:
      '这句仅说明 iOS 不提供 Android BootReceiver 的开机恢复连接开关。' +
      'auto_start_set 对 mobile 且非 Android 的平台显式拒绝，界面在 iOS 不发该状态查询也不画开关；' +
      'Android 的执行侧标记读写及应用启动后自动连接是其它独立路径，不能一起记成缺席。',
    anchors: [
      { file: 'src-tauri/src/commands/misc/autostart.rs', mustContain: '#[cfg(all(mobile, not(target_os = "android")))]\n    {\n        let _ = (app, enabled);\n        ApiResponse::err("autostart is not available on this platform".to_string())' },
      { file: 'ui/src/mobile/settings/GeneralPage.tsx', mustContain: 'control={isIOS ? undefined : (' },
      { file: 'ui/src/mobile/settings/GeneralPage.tsx', mustContain: 'if (isIOS) return;' },
    ],
    reviewed: '2026-11-02',
  },
  {
    id: 'i18n:mobileSettings.general.bootConnectUnsupported@mobile/settings/GeneralPage.tsx#1',
    face: 'i18n',
    kind: 'platform-absent',
    evidence:
      '指向的是 Android 系统 VPN 列表及其 Always-on VPN 设置，不是泛称 iOS 不能建立 VPN。' +
      '原生跳转桥只安装于 Android MainActivity，并发送 Settings.ACTION_VPN_SETTINGS；' +
      'iOS 分支不渲染该跳转控件或 Android 手动路径，以说明行呈现此 Android 设置不适用。',
    anchors: [
      { file: 'src-tauri/gen/android/app/src/main/java/com/polaris2/app/MainActivity.kt', mustContain: 'Intent(Settings.ACTION_VPN_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)' },
      { file: 'ui/src/mobile/settings/GeneralPage.tsx', mustContain: '{!isIOS && canJump ? (' },
      { file: 'ui/src/mobile/settings/GeneralPage.tsx', mustContain: '{!isIOS && <SettingsNote id="boot-auto-connect-path">' },
    ],
    reviewed: '2026-11-02',
  },
  {
    id: 'control:mobile/forms/RuleFormPanel.tsx:const-true-disabled:4a3a57b1#1',
    face: 'control',
    kind: 'platform-absent',
    evidence:
      'Android/iOS 不能运行提权 DHCP 探测。仅为从其它平台导入的存量 DHCP 规则保留当前选项回显和纠正路径；' +
      '该选项禁选，新建首帧已过滤，用户可改选受支持的动作。',
    anchors: [
      { file: 'crates/config-engine/src/builder/network_env.rs', mustContain: 'Platform::Android | Platform::Ios => false,' },
      { file: 'ui/src/mobile/forms/RuleFormPanel.tsx', mustContain: "? [{ ...option, disabled: true, description: tr('mobileRules.networkProfile.reasonDhcpNoPermission') }]" },
    ],
    reviewed: '2026-09-28',
  },
  {
    id: 'register:batch:move-to-group',
    face: 'register',
    // 2026-09-25 从 `platform-absent` 改判（盘点 §4.4「标签不准」）：缺的是数据模型，不是平台 —— 理由自己就这么写。
    kind: 'model-absent',
    evidence:
      '两端同缺，与移动端无关：桌面 components/screens/nodes/NodesBatchBar.tsx:83 那颗 #batch-move ' +
      '永久置灰（disabled={!canMoveToGroup()}），用的与移动端登记表同一条键 nodes.batchMoveUnavailable。' +
      'Polaris 的分组由订阅归属与协议派生，数据模型里没有用户可自由分配的分组，' +
      '写进订阅分组会在下次刷新时被当作已下架删除。能力在数据模型层面就不存在，两端都给不出。',
    anchors: [
      // 桌面那颗一旦被接上（把 disabled 换成 onClick），「两端同缺」当场不成立 ⇒ 本条必须重判。
      { file: 'ui/src/components/screens/nodes/NodesBatchBar.tsx', mustContain: 'disabled={!canMoveToGroup()}' },
      { file: 'ui/src/mobile/nodes/absence-register.ts', mustContain: "reasonKey: 'nodes.batchMoveUnavailable'" },
    ],
    reviewed: '2026-09-25',
  },
  // 2026-09-06 摘除 `register:row:speed-test`：批 0 把它从 `disabled` 改判 `ported`。
  // 它不再是「缺席」的任何一种 —— 动作本身接着线（`onSelect: () => runSpeedTest(...)`），
  // 置灰由协议维度的结构性不可测决定，两端同源同函数。处置表回答的是「移动交付缺了什么」，
  // 一颗两端可做的动作完全相等的按钮留在这里只会稀释它的含义。腐烂检测当场报了这条，
  // 不是我主动想起来的 —— 那正是 anchors 那一层要的效果。
  {
    id: 'i18n:mobileRules.dnsSystemPaneAbsent@mobile/screens/rules/RulesScreen.tsx#1',
    face: 'i18n',
    kind: 'reachable-elsewhere',
    evidence:
      '这不是缺席，是指路牌：系统保护规则的参数在移动端**就有**，住在「设置 → DNS」那一页 ' +
      '（mobile/settings/MobileSettingsScreen.tsx:110 把 dns 路由到 DnsPage），' +
      '规则本身也在本屏列着（mobile/screens/rules/DnsSegment.tsx:107）。这句文案说的是' +
      '「本屏不内嵌那块面板」这条 IA 取舍，接任何东西都消不掉它，把它算成债会让账本永远清不了零。',
    anchors: [
      { file: 'ui/src/mobile/settings/MobileSettingsScreen.tsx', mustContain: 'dns: DnsPage,' },
      { file: 'ui/src/mobile/screens/rules/DnsSegment.tsx', mustContain: 'systemPaneAbsentReason' },
    ],
    reviewed: '2026-09-06',
  },
  // 2026-09-25 摘除 `i18n:mobileRules.appRoutingPlatformNote`（A5）：它登记的 platform-absent 依据
  // 「RouteRule 没有 package_name」指的是本仓结构体，不是平台 —— 上游 sing-box 有 `package_name`，
  // Kotlin `findConnectionOwner` 早已回填包名。本批 route 开出该键、Android 上「指定节点 / 阻断」按包名
  // 发射（`crates/config-engine/tests/android_app_owner_route.rs`），说明改名为 `appRoutingMatchNote`
  // 讲识别方式（含 Android 9 及以下的平台上界），不再是缺席声明。
];

/**
 * 理由里出现这些词 = 它讲的是「我们还没做」，不是「平台不提供」⇒ 不许进处置表。
 *
 * 第一版这张表用的是「还没做 / 尚未实现 / 排期 / TODO」这类**通用**托词，
 * 而全仓通篇用的是「未接线 / 尚未接入 / 还没接 / 先不做 / 要等」—— 一个都不在清单里，
 * 实测抄一句本仓自己的措辞（absence-register.ts:82 逐字写着「移动端还没有 toast 宿主」）
 * 就能把一条纯粹的「我们还没做」合法出账。
 */
const EXCUSE_WORDS: readonly RegExp[] = [
  /[还還][没沒]做/,
  /尚未[实實][现現]/,
  /做起来麻[烦煩]/,
  /成本高/,
  /依赖另一批/,
  /依賴另一批/,
  /排期/,
  /下一批/,
  /[暂暫][时時]/,
  /以后再/,
  /以後再/,
  /TODO/,
  /\blater\b/i,
  /未接[线綫]/,
  /[暂暫]未接入/,
  /尚未接入/,
  /[还還][没沒]接/,
  /[还還][没沒]有(?:接|做)/,
  /先不做/,
  /本批(?:先|不)/,
  /要等/,
  /等[^，。；]{0,8}就[绪緒]/,
  /下一步再/,
];

const DISPOSITION_IDS: ReadonlySet<string> = new Set(DISPOSITIONS.map((entry) => entry.id));
const DEBT: readonly FaceItem[] = ALL_FACE_ITEMS.filter((item) => !DISPOSITION_IDS.has(item.id));

function renderReport(): string {
  const works = new Set(DEBT.map((item) => item.work));
  const lines: string[] = [];
  lines.push('════════ 移动端接线完成度 ════════');
  lines.push(
    `取材面  i18n=${I18N_FACE.length}  register=${REGISTER_FACE.length}  control=${CONTROL_FACE.length}  合计=${ALL_FACE_ITEMS.length}`,
  );
  // 头注里「仅移动端可达的共享模块今天是 0 个」这句话不写死在注释里，让它每次跑都自己报一遍：
  // 这个数从 0 变成非 0，意味着有人把东西抽进了只有移动端才用得到的共享模块 —— 那正是本闭包守的形态。
  lines.push(
    `文件面  mobile=${MOBILE_ROOTS.length}  仅移动端可达的共享模块=${MOBILE_ONLY_SHARED.length}` +
      (MOBILE_ONLY_SHARED.length > 0
        ? `（${MOBILE_ONLY_SHARED.map((file) => rel(file)).join(' · ')}）`
        : ''),
  );
  lines.push(`处置表  ${DISPOSITIONS.length}（逐条见下；判据见 DISPOSITIONS）`);
  for (const entry of DISPOSITIONS) {
    lines.push(`        · [${entry.kind}] ${entry.id}`);
    lines.push(`          ${entry.evidence.split('：')[0]?.slice(0, 60) ?? ''}…`);
  }
  lines.push(`未接线  ${DEBT.length} 条待办 / ${works.size} 件工作`);
  if (process.env.POLARIS_WIRING_REPORT === '1') {
    lines.push('──────── 逐条（同一 work 的是同一件工作在不同面上的影子） ────────');
    DEBT.forEach((item, index) => {
      lines.push(`${String(index + 1).padStart(3)}. [${item.face}] ${item.where}`);
      lines.push(`            work=${item.work}`);
      lines.push(`            ${item.detail}`);
    });
  }
  return lines.join('\n');
}

/* ────────────────────────── 断言 ────────────────────────── */

describe('移动端接线完成度', () => {
  describe('① 取材面自检（判据不许被自己污染，也不许排成空集）', () => {
    it('本文件确实在 ui/src 的遍历结果里 —— 排除动作有对象可排', () => {
      expect(ALL_UI_FILES, 'walkTypeScript 没走到本文件，遍历口径变了').toContain(SELF);
    });

    it('本文件自身**会**命中取材面 —— 所以排除它是承重的，不是装饰', () => {
      // 注释也一并抹掉之后再看：这一条要证明的是「排除自身」承重，而不是「注释里有词」承重。
      const selfText = blankComments(readFileSync(SELF, 'utf8'));
      expect(
        matchedFamilies(selfText).length,
        '本文件（抹掉注释后）已经不含任何缺席短语了？那反向对照的样本也就没了',
      ).toBeGreaterThan(0);
      const constTrue = constTrueNames(selfText);
      const controlHits = CONTROL_PATTERNS.filter(([, hit]) =>
        selfText.split('\n').some((line) => hit(line, constTrue)),
      );
      expect(controlHits.length, '本文件已经不含任何控件形态样本了？').toBeGreaterThan(0);
    });

    it('排除之后本文件**不在**取材面里', () => {
      expect(PRODUCTION_FILES).not.toContain(SELF);
      expect(
        ALL_FACE_ITEMS.map((item) => item.id).filter((id) => id.includes('wiring-completeness')),
        '本文件自己漏进了取材面 —— 判据被自己污染',
      ).toEqual([]);
    });

    /**
     * 塌陷检测与**债务**解耦（复审 major，已修）。
     *
     * 第一版拿 `CONTROL_FACE.length > 0` 当控件面的塌陷判据 —— 那是一颗**装在完成路径上的地雷**：
     * 真的全部接线的那一天，写死禁用形态归零 ⇒ 这条断言以「控件面塌了」报红，
     * 而当时最省事的修法就是把塌陷检测删掉。塌陷要用**与债务无关的量**去测：扫了多少文件、
     * 多少行代码、locale 齐不齐；「认得出形态」由 ② 组的合成样本证明。
     */
    it('①-4 取材面没塌（量的是取材规模与谓词能力，不是债务条数）', () => {
      expect(PRODUCTION_FILES.length, '移动端可达的生产源码文件数掉了一大截').toBeGreaterThan(30);
      const scannedLines = [...PRODUCTION_SOURCES.values()].reduce(
        (sum, text) => sum + text.split('\n').filter((line) => line.trim().length > 0).length,
        0,
      );
      expect(scannedLines, '控件面扫到的代码行数掉了一大截 —— 取材面塌了').toBeGreaterThan(5000);
      const claimed = claimedKeys(LOCALE_FILES).length;
      // 判据与报错文案对齐（「掉到 10 条以下」= < 10 才红）。2026-09-25 A5 销掉 appRoutingPlatformNote 后恰 10 条。
      expect(claimed, '全仓被缺席信号认领的键掉到 10 条以下 —— 文案面塌了').toBeGreaterThanOrEqual(10);
      expect(ROW_ACTIONS.length + BATCH_ACTIONS.length, '登记表塌了').toBeGreaterThanOrEqual(10);
    });

    it('①-5 桌面入口清单没过期（闭包判据靠它区分共享模块与移动端专属模块）', () => {
      expect(
        DESKTOP_ENTRY_FILES.map((file) => rel(file)).sort(),
        '桌面入口文件被改名/移走 —— 闭包会把一大批共享模块误判成移动端专属',
      ).toEqual([...DESKTOP_ENTRIES].sort());
      expect(REACHABLE_FROM_DESKTOP.size, '从桌面入口可达的模块数掉了一大截 —— import 图解析口径变了').toBeGreaterThan(
        100,
      );
    });

    it('①-6 MOBILE_NAMESPACES 与 locale 顶层的 mobile* 键逐字相等', () => {
      const enUS = LOCALE_FILES.find((file) => file.file.endsWith('locales/en-US.json'));
      expect(enUS, 'locales/en-US.json 不在取材面里').toBeDefined();
      const topLevel = new Set(
        Object.keys(enUS!.flat)
          .map((key) => key.split('.')[0]!)
          .filter((segment) => /^mobile/.test(segment)),
      );
      expect(
        [...topLevel].sort(),
        '硬编码的 MOBILE_NAMESPACES 与 locale 失配 —— 新增一个 mobile* 命名空间会静默掉在面外',
      ).toEqual([...MOBILE_NAMESPACES].sort());
    });

    it('登记表唯一性：全仓只有 absence-register.ts 一张处置表', () => {
      // 射程是**整个 ui/src**：判据必须与它讲的话一样宽。
      // 判据是**结构性**的，不是第一版那个只认单引号 `kind: 'absent'` 的字面形状 ——
      // 实测换成双引号、或把字段名换成 `state:`、把取值换成 `'unavailable'`，第二张表就能在门外长大。
      // 🔴 取材面**不是** `NON_TEST_FILES`：那份清单走 `IS_TEST_ONLY_MODULE`，
      // 而它把 `.test-support.` 一起排掉了 —— 2026-09-06 批 6 新建的四张对差表正好是这个命名，
      // 于是这条「第六张表在门外无声长大」的检测器对**本批确立的新惯例**整体失明
      // （实测：新建一张 `.test-support.` 的处置表、不进 `REGISTER_FILES`、不被任何门 import，
      // 全仓测试照绿）。这里只排真正的门（`.test.` / `.spec.`）与本文件自己。
      /* 两个谓词各自具名，才能拿**合成样本**去证它们准不准（见下方切点自检）。
         内联在 filter 里时，只能靠「现实里恰好还有个样本」间接证明，而那正是会随销账消失的东西。 */
      const isRegistryCandidateName = (file: string): boolean =>
        !/\.(?:test|spec)\.tsx?$/.test(file) && file !== SELF;
      /* 🔴 2026-09-25 收窄第二个特征（`reasonKey`），并补一条等价的第三特征。
         起因：`domain/network-profile.ts`（网络场景的**运行期显示态**）同时含 `kind: 'unavailable'` /
         `state: 'missing'` 与 `reasonKey`，被认成第六张处置表 ⇒ 事实门失败。它不是表：
         它的 `reasonKey` 只以两种形态出现 —— **类型注解**（`reasonKey: string`）与**运行期算出来的值**
         （`reasonKey: probeReasonKey(hit.reason)`）。处置表的 `reasonKey` 是**登记**：写死的键
         （字面量 / 常量 / 简写属性）。故第二特征改成「`reasonKey` 以数据形态出现」，排掉那两种形态。
         「更通用 ≠ 更强」—— 输入对差表（旧 / 新判定）钉在下面的切点自检里，逐行有样本：
           ① 缺席态 + `reasonKey: '字面量'`                  T / T
           ② 缺席态 + `reasonKey: CONST` / 简写 `{ reasonKey }` T / T
           ③ 缺席态 + 只有 `reasonKey: string` 类型注解         T / **F**  ← 本次让出的一格（见下）
           ④ 缺席态 + 只有 `reasonKey: fn(x)` 运行期值          T / F     ← 本次要排掉的形态（network-profile.ts）
           ⑤ 缺席态 + 注册表类型（`ParityEntry` …）无 `reasonKey` F / **T**  ← 新增的第三特征
           ⑥ 只有缺席态 / 只有 `reasonKey`                     F / F
         ③ 是真实的让步：一张**自带类型声明**、且条目全是不带理由的 `{ kind: 'absent' }` 的新表，旧判据靠那行
         类型注解认得出，新判据认不出。⑤ 补的是它的主形态 —— 本仓每一张处置表都用注册表类型声明条目
         （`parity-registry.test-support.ts` 的 `ParityEntry` / `ScreenParityRegister`，节点屏那张的
         `Disposition`），而旧判据对「用注册表类型、条目全不带理由」的表**本来就是瞎的**（不含 `reasonKey` 一词）。
         剩下的盲区只有「自造类型名 + 条目全无理由」这一种，登记在此，不假装覆盖。 */
      const ABSENCE_STATE =
        /\b(?:kind|state|status|disposition)\s*:\s*["'](?:absent|disabled|unavailable|unsupported|pending|notWired|missing)["']/;
      /** `reasonKey` 以**登记数据**出现：后面不是 `: string` 类型注解，也不是 `: fn(…)` 运行期调用。 */
      const REASON_KEY_AS_DATA = /\breasonKey\b(?!\??\s*:\s*(?:string\b|[A-Za-z_$][\w$.]*\s*\())/;
      /** 条目用处置表的注册表类型声明（两处定义：`parity-registry.test-support.ts` 与 `nodes/absence-register.ts`）。 */
      const REGISTRY_TYPED = /\b(?:ParityDisposition|ParityEntry|ScreenParityRegister|Disposition)\b/;
      const looksLikeDispositionTable = (text: string): boolean =>
        ABSENCE_STATE.test(text) && (REASON_KEY_AS_DATA.test(text) || REGISTRY_TYPED.test(text));
      const registryFace = ALL_UI_FILES.filter(isRegistryCandidateName);
      const registries = registryFace
        .filter((file) => looksLikeDispositionTable(blankComments(readFileSync(file, 'utf8'))))
        .map((file) => rel(file))
        .sort();
      // 🔴 判据方向是**「探测到的都必须在 B 面射程里」**，不是双向恰等：
      //  · 这一侧（探测 ⊆ 已知）守的是「第六张表在门外无声长大」，那正是本条存在的理由；
      //  · 反向不成立且不该成立 —— 连接屏那张对差表今天**一条债都没有**（12 颗动作全有落点），
      //    于是它不含 `kind: 'absent'` 字面量、探测器看不见它。要求它被探测到，等于要求
      //    每张表都必须欠着点什么才算数。
      // 期望值取 B 面自己吃的那份清单（`REGISTER_FILES`），不另抄一份，避免两处各自漂。
      const known = REGISTER_FILES.map((file) => rel(file));
      expect(
        registries.filter((file) => !known.includes(file)),
        '出现了一张不在 B 面射程里的处置表 —— 扩 `REGISTER_FILES`，否则新表在门外无声长大',
      ).toEqual([]);
      // 反向那一半改成**结构**检查：清单里每一份都得真的存在、且真的是一张表（不是一个空壳路径）。
      for (const file of REGISTER_FILES) {
        expect(existsSync(file), `REGISTER_FILES 里的 ${rel(file)} 不存在`).toBe(true);
        const text = readFileSync(file, 'utf8');
        expect(
          /ScreenParityRegister|Disposition/.test(text),
          `${rel(file)} 不像一张处置表 —— B 面在吃一份空壳`,
        ).toBe(true);
      }
      // 正面对照：探测器本身没瞎（它至少认得出节点屏那张已知的表）。
      expect(registries, '探测器一张表都没认出来 ⇒ 上面那条恒绿').toContain(
        'mobile/nodes/absence-register.ts',
      );
      // 切点自检：放宽取材面之后，`.test-support.` 命名的那几张表真的被认出来了。
      // 少了这一条，上面那次放宽可能只是换了一个照样恒绿的口径。
      //
      // 🔴 判据是「**今天还有债的**那几张必须被认出来」，**不点名**某三张（2026-09-13 改）：
      // 探测器认的是缺席态字面量，一张表把债清零之后就不再含那种字面量 —— 上面那段注释里
      // 连接屏那张表正是这个形态，首页那张在批 12 销完账之后也进了同一档。点名 = 一个会随销账
      // 腐烂的常量，且它报的错（「取材面又把 .test-support. 排掉了」）会把人引向错误方向。
      // 名单从**登记数据本身**派生，与 B 面吃的是同一批对象，不另抄一份。
      const supportWithDebt = SCREEN_REGISTERS.filter((register) =>
        [...register.actions, ...register.blocks].some((entry) => isDebt(entry.disposition)),
      ).map((register) => `${register.mobileDir.replace(/^ui\/src\//, '')}/absence-register.test-support.ts`);
      /* 🔴 **切点换成合成样本**（2026-09-14）。
         上一版这里是 `expect(supportWithDebt.length).toBeGreaterThan(0)`，并在注释里预告了
         它自己的退役条件：「降到 0 个对象时正确的动作是另找切点（往取材面塞一份合成的
         `.test-support.` 表），不是让断言空转」。四张屏级对差表今天全部销清 ⇒ 那一天到了，
         按它自己写的办法换，不是把它删掉，也不是把下限调成 0（那就是空转）。

         这条要证的命题只有两句，两句都不需要现实供样本：
          ① `.test-support.` 这种命名**不被**取材面的排除规则挡掉（`.test.` / `.spec.` 才挡）；
          ② 探测器认的是「缺席态字面量 + `reasonKey`」这对特征，缺任一不算。
         各配一条反向对照，证明两个谓词不是恒真。 */
      expect(
        isRegistryCandidateName('mobile/zz/absence-register.test-support.ts'),
        '取材面把 `.test-support.` 挡掉了 —— 第六张处置表会在门外无声长大',
      ).toBe(true);
      expect(
        isRegistryCandidateName('mobile/zz/whatever.test.ts'),
        '取材面居然收了 `.test.` —— 那些是门本身，不是处置表',
      ).toBe(false);
      expect(
        looksLikeDispositionTable("const T = [{ kind: 'absent', reasonKey: 'zz.absent' }];"),
        '探测器认不出「缺席态字面量 + reasonKey」这对特征',
      ).toBe(true);
      expect(
        looksLikeDispositionTable("const T = [{ kind: 'absent' }];"),
        '缺 `reasonKey` 也被认成处置表 —— 那半个特征挡不住任何东西',
      ).toBe(false);
      expect(
        looksLikeDispositionTable("const T = [{ reasonKey: 'zz.absent' }];"),
        '缺缺席态字面量也被认成处置表 —— 同上',
      ).toBe(false);
      /* 输入对差表（见 `looksLikeDispositionTable` 上方注释）逐行一个样本，正反两侧都钉住。 */
      expect(
        looksLikeDispositionTable("const K = 'zz.absent';\nconst T = [{ kind: 'absent', reasonKey: K }];"),
        '② 常量形态的 reasonKey 认不出了 —— 收窄过头',
      ).toBe(true);
      expect(
        looksLikeDispositionTable("const reasonKey = 'zz';\nconst T = [{ state: \"disabled\", reasonKey }];"),
        '② 简写属性形态的 reasonKey 认不出了 —— 收窄过头',
      ).toBe(true);
      expect(
        looksLikeDispositionTable("const T: readonly ParityEntry[] = [{ id: 'x', disposition: { kind: 'absent' }, note: '…' }];"),
        '⑤ 注册表类型声明、条目全不带理由的表认不出 —— 第三特征没生效',
      ).toBe(true);
      expect(
        looksLikeDispositionTable(
          "export type D = { kind: 'unavailable'; reasonKey: string } | { kind: 'ok' };\n" +
            "export function f(h: H): D { return { kind: 'unavailable', reasonKey: probeReasonKey(h.reason) }; }\n" +
            "export type B = { state: 'missing' } | { state: 'unavailable'; reasonKey?: string };",
        ),
        '④ 运行期显示态（类型注解 + 调用算出来的 reasonKey）又被认成处置表',
      ).toBe(false);
      /* 反对照落到**真文件**上：network-profile.ts 确实同时含缺席态与 `reasonKey`（否则上一条样本说明不了它），
         且新判据不认它。 */
      {
        const npText = blankComments(readFileSync(join(UI_SRC_DIR, 'domain', 'network-profile.ts'), 'utf8'));
        expect(ABSENCE_STATE.test(npText) && /\breasonKey\b/.test(npText), 'network-profile.ts 不再同时含缺席态与 reasonKey —— 这条反对照失去了对象').toBe(true);
        expect(registries, 'network-profile.ts 又被认成处置表').not.toContain('domain/network-profile.ts');
      }
      for (const named of supportWithDebt) {
        expect(registries, `探测器看不见 ${named} —— 取材面又把 .test-support. 排掉了`).toContain(named);
      }
      // 且证明这条放宽**有对象可放**：那三张表确实被旧口径（`IS_TEST_ONLY_MODULE`）挡在外面。
      expect(
        NON_TEST_FILES.map((f) => rel(f)),
        '旧口径居然也看得见 .test-support. —— 那这次放宽是空操作，上面那三条自检说明不了什么',
      ).not.toContain('mobile/home/absence-register.test-support.ts');
    });
  });

  describe('② 反向对照（证明每一面都真的认得出它该认的东西）', () => {
    it('i18n 面：合成一条假的未接线文案，主判据必须逐条报出来', () => {
      const synthetic: LocaleFile[] = [
        {
          file: 'locales/__synthetic__.json',
          flat: {
            'mobileSettings.__synthetic__': '这条腿在移动端尚未接线，请在桌面端操作后同步',
            'mobileSettings.__benign__': '已连接',
          },
        },
      ];
      const items = scanI18nFace(synthetic, new Map());
      expect(items.map((item) => item.id)).toEqual(['i18n:mobileSettings.__synthetic__']);
    });

    it('i18n 面：**每一族**短语都各自认得出自己的形态（不是靠某一族撑住全部）', () => {
      const samples: Record<string, string> = {
        'zh-notwired': '这条腿在移动端尚未接线',
        'zh-godesktop': '请在桌面端修改后同步',
        'zh-noplatform': '本平台不按应用分流',
        'en-notwired': 'not wired on mobile yet',
        'en-godesktop': 'edit on the desktop client and sync',
        'en-noplatform': 'This platform does not route by application',
      };
      for (const [family, text] of Object.entries(samples)) {
        expect(matchedFamilies(text), `${family} 这一族认不出自己的样本`).toContain(family);
      }
    });

    /**
     * 复审 blocker 的**回放**：这两条文案 2026-09-06 当天就在仓里，各是一条真的「只是没接线」，
     * 而第一版六个短语族一个都不命中 ⇒ 债务数当天就偏小，「清零 = 全部接线」当天就不成立。
     * 这一组不是合成样本，是从 `locales/*.json` 逐字抄回来的历史真缺陷回放。
     */
    it('i18n 面：历史漏网的两条真缺口，现在必须被认领（回放，不是合成）', () => {
      const replays: Array<readonly [string, string]> = [
        [
          'privacyLockAbsent/en',
          'The idle privacy lock and its password are not offered on mobile yet: neither the idle timer nor the lock overlay is wired, so switching it on would lock nothing.',
        ],
        ['privacyLockAbsent/zh-CN', '自动隐私锁与锁屏密码暂不在移动端提供：闲置计时与锁屏遮罩这两条腿还没接，开着它不会锁任何东西。'],
        ['privacyLockAbsent/zh-TW', '自動隱私鎖與鎖屏密碼暫不在行動端提供：閒置計時與鎖屏遮罩這兩條腿還沒接，開著它不會鎖任何東西。'],
        [
          'resourceMetaUnregistered/en',
          'Size and last-updated are not registered in the mobile data contract yet, so they are not shown here — better absent than a number with no stated source.',
        ],
        ['resourceMetaUnregistered/zh-CN', '大小与更新时间尚未登记进移动端数据契约，故此处不显示 —— 宁可不显示，也不显示一个说不出来源的数字。'],
        ['resourceMetaUnregistered/zh-TW', '大小與更新時間尚未登記進行動端資料契約，故此處不顯示 —— 寧可不顯示，也不顯示一個說不出來源的數字。'],
      ];
      for (const [name, text] of replays) {
        expect(matchedFamilies(text), `${name} 又掉出取材面了`).not.toEqual([]);
      }
    });

    it('i18n 面：良性文案不许命中（判据不是「什么都算」）', () => {
      // 前四条是基础良性样本；后五条是复审实测会被第一版**误伤**的跨平台常规文案 ——
      // 它们一旦落进 mobile* 命名空间就是一条永远修不掉、且在处置表口径下无处安放的假待办。
      const benign = [
        '已连接',
        '智能分流',
        'Connected',
        'Speed test finished',
        '设置已与桌面端实时同步',
        '此配置在桌面端与移动端通用',
        '免费套餐不提供高级节点',
        'Settings sync with the desktop app automatically',
        'This profile also works on desktop',
      ];
      for (const text of benign) {
        expect(matchedFamilies(text), `良性文案被误判：${text}`).toEqual([]);
      }
    });

    it('i18n 面：键名信号（A2）与文案信号（A1）各自独立地认得出缺席', () => {
      // A2 认得出「文案人畜无害、键名是缺席声明」的形态 —— A1 对它是瞎的。
      const wordingDodges: LocaleFile[] = [
        {
          file: 'locales/__synthetic__.json',
          flat: { 'mobileSettings.__synthetic__Absent': '这项功能在此版本中的行为请参见帮助中心。' },
        },
      ];
      expect(matchedFamilies('这项功能在此版本中的行为请参见帮助中心。'), 'A1 居然认得出？那这条对照就不成立了').toEqual([]);
      expect(scanI18nFace(wordingDodges, new Map()).map((item) => item.id)).toEqual([
        'i18n:mobileSettings.__synthetic__Absent',
      ]);
      // 反过来：键名人畜无害、文案是缺席声明 —— A2 对它是瞎的，A1 接住。
      const nameDodges: LocaleFile[] = [
        {
          file: 'locales/__synthetic__.json',
          flat: { 'mobileSettings.__synthetic__Note': '这条腿在移动端尚未接线' },
        },
      ];
      expect(ABSENCE_KEY_NAMES.test('mobileSettings.__synthetic__Note'), 'A2 居然认得出？那这条对照就不成立了').toBe(
        false,
      );
      expect(scanI18nFace(nameDodges, new Map()).map((item) => item.id)).toEqual([
        'i18n:mobileSettings.__synthetic__Note',
      ]);
      // A2 只在移动端自有的键上生效：运行期状态那一批（`home.proxyExitUnavailable`）不许被拖进来。
      expect(isMobileOwnedKey('home.proxyExitUnavailable')).toBe(false);
      expect(isMobileOwnedKey('nodes.mobileMeshFormUnavailable')).toBe(true);
    });

    it('i18n 面：命中但与移动端无关的键必须被收窄掉（射程如实）', () => {
      const synthetic: LocaleFile[] = [
        {
          file: 'locales/__synthetic__.json',
          flat: { 'desktopOnly.__synthetic__': 'Download backend not wired up yet' },
        },
      ];
      expect(scanI18nFace(synthetic, new Map()), '桌面自己的债被算进了移动端的账').toEqual([]);
      // 同一条键一旦被移动端消费，立刻进面 —— 收窄靠的是消费点，不是命名空间黑名单。
      const consumed = new Map([[join(MOBILE_DIR, '__synthetic__.tsx'), "t('desktopOnly.__synthetic__')"]]);
      expect(scanI18nFace(synthetic, consumed).map((item) => item.id)).toEqual([
        'i18n:desktopOnly.__synthetic__@mobile/__synthetic__.tsx#1',
      ]);
    });

    it('i18n 面：逐渲染点计数（复用一条已有的缺席键不再是零增量）', () => {
      const synthetic: LocaleFile[] = [
        {
          file: 'locales/__synthetic__.json',
          flat: { 'mobileRules.__synthetic__': '这条腿在移动端尚未接线' },
        },
      ];
      const oneRender = new Map([
        [join(MOBILE_DIR, 'A.tsx'), "t('mobileRules.__synthetic__')"],
      ]);
      const threeRenders = new Map([
        [
          join(MOBILE_DIR, 'A.tsx'),
          ["t('mobileRules.__synthetic__')", "t('mobileRules.__synthetic__')"].join('\n'),
        ],
        [join(MOBILE_DIR, 'B.tsx'), "t('mobileRules.__synthetic__')"],
      ]);
      expect(scanI18nFace(synthetic, oneRender).map((item) => item.id)).toEqual([
        'i18n:mobileRules.__synthetic__@mobile/A.tsx#1',
      ]);
      expect(scanI18nFace(synthetic, threeRenders).map((item) => item.id)).toEqual([
        'i18n:mobileRules.__synthetic__@mobile/A.tsx#1',
        'i18n:mobileRules.__synthetic__@mobile/A.tsx#2',
        'i18n:mobileRules.__synthetic__@mobile/B.tsx#1',
      ]);
    });

    it('i18n 面：拼键也算消费点（模板串把父路径写在里面）', () => {
      const synthetic: LocaleFile[] = [
        {
          file: 'locales/__synthetic__.json',
          flat: { 'home.unlockStatus.notWiredOnMobile': '该检测在移动端尚未接入' },
        },
      ];
      const composed = new Map([
        [join(MOBILE_DIR, 'H.tsx'), 't(`home.unlockStatus.${s.result.status}`)'],
      ]);
      expect(scanI18nFace(synthetic, composed).map((item) => item.id)).toEqual([
        'i18n:home.unlockStatus.notWiredOnMobile@mobile/H.tsx#1',
      ]);
      // 父路径不同的拼键不许误伤。
      const unrelated = new Map([[join(MOBILE_DIR, 'H.tsx'), 't(`home.other.${x}`)']]);
      expect(scanI18nFace(synthetic, unrelated)).toEqual([]);
    });

    it('登记表面：ported 不进面，absent/disabled 逐条进面（七张表都在面上）', () => {
      const rows: RowActionEntry[] = [
        { id: 'a', desktopRef: 'x', disposition: { kind: 'ported' } },
        { id: 'b', desktopRef: 'y', disposition: { kind: 'absent', reasonKey: 'k1' } },
        { id: 'c', desktopRef: 'z', disposition: { kind: 'disabled', reasonKey: 'k2' } },
      ];
      const mesh: MeshJoinChoiceEntry[] = [
        { id: 'm1', label: 'M1', descKey: 'd', desktopRef: 'r', disposition: { kind: 'ported' } },
        { id: 'm2', label: 'M2', descKey: 'd', desktopRef: 'r', disposition: { kind: 'absent', reasonKey: 'k3' } },
      ];
      expect(
        scanRegisterFace(rows, [], ['f1'], rows, rows, mesh, rows).map((item) => item.id),
      ).toEqual([
        // 四张「新表」排在前面（构造顺序），每张各贡献那两条非 ported 的。
        'register:add:b',
        'register:add:c',
        'register:sub-menu:b',
        'register:sub-menu:c',
        'register:mesh-action:b',
        'register:mesh-action:c',
        'register:mesh-join:m2',
        'register:row:b',
        'register:row:c',
        'register:sub-pending:f1',
      ]);
      // 反向对照：四张新表全 `ported` 时一条都不进面（否则上面那条可能只是「表非空就记账」）。
      const allPorted: RowActionEntry[] = [{ id: 'a', desktopRef: 'x', disposition: { kind: 'ported' } }];
      expect(scanRegisterFace([], [], [], allPorted, allPorted, [mesh[0]!], allPorted)).toEqual([]);
    });

    it('控件面：六种写死形态各自认得出，动态形态不许误伤', () => {
      const hit = new Map([
        [
          join(MOBILE_DIR, '__synthetic__.tsx'),
          [
            'const NEVER = true;',
            '<button type="button" disabled>',
            '<MobileButton disabled onClick={() => undefined}>',
            '<button disabled aria-label={t("x")}>',
            '  disabled: true,',
            '  disabled={true}',
            '<button className="mr-btn" disabled={NEVER}>',
            '<span role="button" aria-disabled="true">x</span>',
            '  onSelect: () => undefined,',
            "  disabledReason: t('nodes.mobileMeshFormUnavailable'),",
          ].join('\n'),
        ],
      ]);
      const shapes = scanControlFace(hit).map((item) => item.detail.slice(0, item.detail.indexOf(']') + 1));
      expect(shapes.filter((s) => s.includes('bare-disabled')).length).toBe(3);
      expect(shapes.filter((s) => s.includes('const-true-disabled')).length).toBe(2);
      expect(shapes.filter((s) => s.includes('const-folded-disabled')).length).toBe(1);
      expect(shapes.filter((s) => s.includes('aria-disabled')).length).toBe(1);
      expect(shapes.filter((s) => s.includes('inert-handler')).length).toBe(2);
      expect(shapes.filter((s) => s.includes('const-disabled-reason')).length).toBe(1);
      // 同一行两种形态合成一项，不是两项 —— 这个数是「几处控件」不是「几次命中」。
      expect(shapes.filter((s) => s === '[bare-disabled+inert-handler]').length).toBe(1);
      expect(scanControlFace(hit).length).toBe(9);

      const miss = new Map([
        [
          join(MOBILE_DIR, '__synthetic__.tsx'),
          [
            'const BUSY = false;',
            '<button disabled={busy}>',
            '<button disabled={BUSY}>',
            '  disabled?: boolean;',
            '  disabled,',
            '  disabled={selectedCount === 0}',
            '  onCommit={() => undefined}',
            '  disabledReason: speedTestBlockedReason(t, proxyRunning, row),',
            '  disabledReason: visibleTestBlocked ?? undefined,',
            '  onSelect: () => runSpeedTest([row.server.id]),',
            '<span aria-disabled={busy}>x</span>',
          ].join('\n'),
        ],
      ]);
      expect(scanControlFace(miss), '动态禁用/类型声明/非点击处理器被误判成写死').toEqual([]);
    });

    it('控件面：注释里的形态不算债（幽灵待办对照）', () => {
      const commented = new Map([
        [
          join(MOBILE_DIR, '__synthetic__.tsx'),
          [
            '  // 这颗按钮在核未运行时 disabled',
            '  /* 核停时整块 disabled */',
            '  // TODO: 别再 disabled',
            '<button type="button" disabled>',
          ].join('\n'),
        ],
      ]);
      const blanked = new Map(
        [...commented].map(([file, text]) => [file, blankComments(text)] as const),
      );
      expect(scanControlFace(blanked).map((item) => item.where)).toEqual([
        'mobile/__synthetic__.tsx:4',
      ]);
      // blankComments 必须保住行号：第 4 行还是第 4 行。
      expect(blankComments(['a', '/* x\n y */', 'b'].join('\n')).split('\n').length).toBe(4);
    });

    it('控件面：抽进「只有移动端才用得到」的共享模块里也逃不掉（闭包对照）', () => {
      // 合成一张 import 图：mobile/X.tsx → components/PendingAction.tsx，桌面入口够不着它。
      const mobileFile = join(MOBILE_DIR, 'X.tsx');
      const sharedOnlyMobile = join(UI_SRC_DIR, 'components', '__PendingAction__.tsx');
      const sharedBoth = join(UI_SRC_DIR, 'components', '__Shared__.tsx');
      const desktopFile = join(UI_SRC_DIR, 'App.tsx');
      const graph = new Map<string, string[]>([
        [mobileFile, [sharedOnlyMobile, sharedBoth]],
        [desktopFile, [sharedBoth]],
        [sharedOnlyMobile, []],
        [sharedBoth, []],
      ]);
      const fromMobile = reachableFrom([mobileFile], graph);
      const fromDesktop = reachableFrom([desktopFile], graph);
      const mobileOnly = [...fromMobile]
        .filter((file) => !file.startsWith(`${MOBILE_DIR}`) && !fromDesktop.has(file))
        .map((file) => rel(file));
      expect(mobileOnly, '抽进共享目录、只有移动端 import 的模块必须仍在面内').toEqual([
        'components/__PendingAction__.tsx',
      ]);
      expect(fromDesktop.has(sharedBoth), '真共享模块不许被算成移动端专属').toBe(true);
    });
  });

  describe('③ 处置表纪律', () => {
    it('处置表非空（空 = 取材面塌了，或有人把它清空了）', () => {
      expect(DISPOSITIONS.length).toBeGreaterThan(0);
    });

    it('每条都仍能在取材面上被找到（腐烂检测）', () => {
      const faceIds = new Set(ALL_FACE_ITEMS.map((item) => item.id));
      const rotten = DISPOSITIONS.filter((entry) => !faceIds.has(entry.id)).map((entry) => entry.id);
      expect(
        rotten,
        '这些处置条目在取材面上找不到了：可能是那条缺口真的接上了（那就删掉本条），' +
          '也可能只是取材口径变了（那就先确认再动）。留着不管会让一个新出现的同名缺口被无声放行：\n' +
          rotten.join('\n'),
      ).toEqual([]);
    });

    it('每条的 face 与它在取材面上的实际归属一致', () => {
      const faceOf = new Map(ALL_FACE_ITEMS.map((item) => [item.id, item.face]));
      const mismatched = DISPOSITIONS.filter(
        (entry) => faceOf.get(entry.id) !== undefined && faceOf.get(entry.id) !== entry.face,
      ).map((entry) => `${entry.id}: 登记 ${entry.face}，实际 ${faceOf.get(entry.id)}`);
      expect(mismatched).toEqual([]);
    });

    it('判据只接受「平台不提供 / 其实在别处就有」：出现「我们还没做」类措辞当场红', () => {
      const excused = DISPOSITIONS.flatMap((entry) =>
        EXCUSE_WORDS.filter((word) => word.test(entry.evidence)).map(
          (word) => `${entry.id} 的理由里出现了 ${String(word)}`,
        ),
      );
      expect(
        excused,
        '「我们还没做」不是平台缺席 —— 把它从处置表里拿出来，让它红着',
      ).toEqual([]);
    });

    /**
     * 🔴 锚是被**打开核对**的，不是形状检查。
     * 第一版只校验理由里长得像 `file:line`，实测把测速腿整条拔掉、或把桌面那颗「移动到分组」
     * 接上（两端同缺当场不成立），两个门都还是全绿。
     */
    it('每条的 anchors 都真的打得开、且真的含那段文本（事实核对，不是形状检查）', () => {
      const broken: string[] = [];
      for (const entry of DISPOSITIONS) {
        expect(entry.anchors.length, `${entry.id} 一条 anchor 都没有`).toBeGreaterThan(0);
        for (const anchor of entry.anchors) {
          const full = join(REPO_DIR, anchor.file);
          if (!existsSync(full)) {
            broken.push(`${entry.id}: 文件不在了 → ${anchor.file}`);
            continue;
          }
          // 🔴 剥注释后再找：裸 `includes` 会被一句墓碑注释（「此处曾有 X，整块已移走」）喂饱，
          // 而本仓自己就有写墓碑注释的习惯（`contracts/types.ts:753`）。反向对照见 ② 组。
          if (!blankComments(readFileSync(full, 'utf8')).includes(anchor.mustContain)) {
            broken.push(`${entry.id}: ${anchor.file} 里找不到 ${JSON.stringify(anchor.mustContain)}`);
          }
        }
      }
      expect(
        broken,
        '处置表依据的事实变了 —— 要么那条能力被拿掉了（回归），要么两端同缺不再成立（该重判）：\n' +
          broken.join('\n'),
      ).toEqual([]);
    });

    it('每条都给得出复核日期，理由写得下一个人看得懂', () => {
      for (const entry of DISPOSITIONS) {
        expect(entry.reviewed, `${entry.id} 的复核日期格式不对`).toMatch(/^\d{4}-\d{2}-\d{2}$/);
        expect(entry.evidence.length, `${entry.id} 的理由太短，写不出可复核的依据`).toBeGreaterThan(40);
      }
    });

    it('处置表里的 id 不重复', () => {
      expect(new Set(DISPOSITIONS.map((e) => e.id)).size).toBe(DISPOSITIONS.length);
    });

    /**
     * 🔴 `model-absent` 说的是「两端同缺」，那就得有一条锚证明**另一端**也缺：
     * 锚全落在 `ui/src/mobile/` 下 = 只证明了移动端缺，那是债务，不是数据模型的边界。
     */
    it('`model-absent` 必须有一条锚落在移动树之外（证明两端同缺，不是只有移动端缺）', () => {
      const provesBothEnds = (entry: Pick<DispositionEntry, 'anchors'>): boolean =>
        entry.anchors.some((a) => !a.file.startsWith('ui/src/mobile/'));
      // 谓词自检：合成样本正反各一，证明它判得出（否则下面那条恒真 / 恒假）。
      expect(
        provesBothEnds({ anchors: [{ file: 'ui/src/mobile/nodes/absence-register.ts', mustContain: 'x' }] }),
        '只有移动端锚的样本被判成了「两端同缺」—— 谓词恒真',
      ).toBe(false);
      expect(
        provesBothEnds({
          anchors: [
            { file: 'ui/src/mobile/nodes/absence-register.ts', mustContain: 'x' },
            { file: 'ui/src/components/screens/nodes/NodesBatchBar.tsx', mustContain: 'y' },
          ],
        }),
        '带桌面锚的样本没被认出来 —— 谓词恒假',
      ).toBe(true);
      const modelAbsent = DISPOSITIONS.filter((e) => e.kind === 'model-absent');
      expect(modelAbsent.length, '一条 model-absent 都没有 ⇒ 本条恒真').toBeGreaterThan(0);
      const oneSided = modelAbsent.filter((e) => !provesBothEnds(e)).map((e) => e.id);
      expect(oneSided, '这些 model-absent 的锚全在移动树里，证明不了两端同缺：\n' + oneSided.join('\n')).toEqual([]);
    });

    /**
     * B 面那侧的正面断言：A 面把 `absence-register.ts` 从消费文件里摘掉了（登记 ≠ 渲染），
     * 这条断言保证那不是一条放开的口子 —— 登记表里每条**未被处置**的理由键都必须
     * 被缺席信号认领，否则「登记一条缺席、但理由文案写得人畜无害」就是一条静默通道。
     */
    it('登记表里每条未处置的理由键都被缺席信号认领（A 面摘掉登记表不是放开口子）', () => {
      const claimed = new Set(claimedKeys(LOCALE_FILES).map((entry) => entry.key));
      const unclaimed = registerReasonKeys()
        .filter(([id]) => !DISPOSITION_IDS.has(id))
        .filter(([, reasonKey]) => !claimed.has(reasonKey))
        .map(([id, reasonKey]) => `${id} 的理由键 ${reasonKey} 没有被任何缺席信号认领`);
      expect(
        unclaimed,
        '登记了一条缺席，而它的理由文案与键名都读不出缺席 —— 要么把文案写实，要么它根本不是缺席',
      ).toEqual([]);
    });
  });

  describe('④ 债务账本', () => {
    it('报告（取材面与债务摘要）', () => {
      // 摘要恒打印；逐条清单只在 POLARIS_WIRING_REPORT=1 时展开。
      console.log(renderReport());
      expect(ALL_FACE_ITEMS.length, '取材面为空 ⇒ 报告没有信息量').toBeGreaterThan(0);
    });

    /* 🔴 完成门：2026-09-14 起是硬门，再出现任何一条未接线项都会当场红。 */
    it(
      '全部接线：三个取材面减去处置表 === 空集',
      () => {
        const remaining = DEBT.map((item) => `[${item.face}] ${item.where} — ${item.detail}`);
        expect(remaining, `还欠 ${remaining.length} 条：\n${remaining.join('\n')}`).toEqual([]);
      },
    );
  });
});
