/**
 * **四屏 × 桌面逐块对差门**（首页 / 规则 / 连接 / 设置）+ **全树喂数腿门**。
 *
 * # 它补的是哪一格
 *
 * `wiring-completeness.test.ts` 的头注「射程自曝」第 2 条自己写着守不住什么：
 *
 * >「守不住**从来没画过的功能**：桌面有、移动端一行都没写、也没人登记 —— 它不在任何一面上。
 * >  今天能兜住这一格的是 `nodes-screen.test.tsx` 那类『拿桌面按钮数与登记表逐颗对差』的门，
 * >  本门不重复实现它，也**不假装**覆盖了它。」
 *
 * 而那类对差门**此前只有节点屏有**。本门把它铺到其余四屏，形态照 `nodes/absence-register.ts` +
 * `nodes/nodes-screen.test.tsx` ⑤⑥ 那一对（表在各屏目录里，门在这里）。
 *
 * 这不是假想的风险：`W-23`（Tailscale 状态）/ `W-24`（订阅进度）/ `W-27`（反向分流指示器）
 * 三条真缺陷就是从这些缝里漏掉的 —— 不在任何缺席登记表上、也没有任何门看得见，
 * 所以「五屏都在」的批次可以带着它们收口。其中 W-27 是 2026-07-20 一次真机事故的修复产物。
 *
 * ── 桌面那一侧的取材面：**先说清一个移动屏对着哪几个桌面目录** ──────────────
 *
 * 🔴 **一屏 ≠ 一个目录**。IA §1.1 把八个桌面屏收进五个移动目的地：桌面 `logs` 折进移动连接屏的
 * `logs` 段，桌面 `app-policy` / `resources` 折进移动规则屏的两个分段。所以取材面的根是
 * `desktopDirs`（**复数**），逐屏写在自己的表里：
 *
 *   home        ← `components/screens/home`
 *   rules       ← `components/screens/rules` + `app-policy` + `resources`
 *   connections ← `components/screens/connections` + `logs`
 *   settings    ← `components/screens/settings`
 *   （nodes 另有 `nodes-screen.test.tsx` ⑤⑥ 那一对门，见 `HANDED_OFF_DESKTOP_DIRS`）
 *
 * 上一版这里是单数 `desktopDir`，那三个折进来的桌面目录**不在任何一面上** ——
 * `logs.archiveLegacy` / `logs.deleteLegacy`（归档 / 删除旧日志）因此既没接、也没登记、
 * 也没有任何门看得见，正是本门自称补上的那一格。⓪ 组现在有一条**面外自曝**断言：
 * `ui/src/components/screens/` 下每个含 `.tsx` 的目录都必须被某张表认领，或在
 * `HANDED_OFF_DESKTOP_DIRS` 上（那张表逐条要求接手门真的存在且含指定那组断言）。
 *
 * ── 四个取材面，逐个写明怎么枚举桌面那一侧 ──────────────────────────────────
 *
 * **A 动作面**（`actionFaces`，可以叠，各屏在自己的表里声明）
 *  · `controls`（四屏都有）—— 桌面这一屏**取材文件**里的每一个**动作元素**：
 *    `<button>`，或任何带 `onClick=` / 动作性 `role=`（button / switch / tab / menuitem / link）
 *    的开标签。剥注释与字符串后按 `{}` 配平取到开标签末尾，再取到对应闭标签。
 *    **取材文件 = `desktopDirs` 下全部非测试 `.tsx/.ts` ∪ 渲染一跳**（见下）。
 *    每颗的身份 = `id=` 属性 → 体内 `t('…')` 键集合 → `className` 字面量 →
 *    `onClick` 里第一个裸标识符 → 开标签 sha1。**先命中先用**，同文件同身份加序号。
 *    这个顺序是按「改动时最不容易漂」排的：`id=` 与 i18n 键是契约，className 是样式，
 *    handler 名是实现，sha1 是最后的兜底（它会因为任何一次改写而变 —— 那时红的是「请重新判一次」）。
 *    ⚠️ 上一版只认字面 `<button`：桌面出口面板里的「按延迟排序」是一枚 `role="switch"` 的
 *    `<span>`，移动端整块移植了却漏了这一维，而集合恒等让人连手工补登都做不到。
 *  · `config-fields`（设置屏，**叠**在 `controls` 上）—— 桌面这一屏 `update({ … })` 写出去的
 *    每一个配置字段，按括号配平取整个对象字面量、收**全部顶层键**（含计算键与展开一层的子键）。
 *    两个面都要：只数配置字段答不出「复制 / 清理 / 重试那些不写配置的按钮在不在」
 *    （`SettingsNetwork` 的终端环境变量复制与 `proxy.clear` 就是从这条缝漏掉的），
 *    只数动作元素又答不出「它能改哪些配置」。
 *
 * **渲染一跳（结构性，不是词法）** —— 一屏的动作取材面不止于它自己的目录：解析这一屏各文件的
 *    具名 import，凡被当作 JSX 标签用过的符号，就把它的来源文件也拉进动作面。
 *    连接屏右键面板里的 `RuleSubjectMenuItems`（新建规则 / 合并进已有规则两颗）住在
 *    `components/RuleSubjectMenuItems.tsx`，上一版整条掉在面外。
 *    这一跳的每一个文件还必须被**显式分类**成 `extraBlockSources`（这一屏的内容块）或
 *    `sharedPrimitives`（通用原语），⓪ 组按恰等断言 —— 新 import 一个会渲染的组件进来，
 *    必须当场判一次，不判就红。
 *
 * **B 数据块面** —— 桌面这一屏渲染的**内容块**：`desktopDirs` 各文件里 `function Foo` /
 *    `const Foo =` **声明**出来的组件名 ∪ 那些文件的基名 ∪ `extraBlockSources` 里声明出来的组件名，
 *    再与「这一屏真渲染过的 JSX 标签」求交，最后减掉 `primitiveModules` 里声明的那些。
 *    ⚠️ 上一版的判据是「标签名 == 同目录某个**文件名**」，于是**同文件内定义并渲染**的块整条
 *    不可见 —— W-27 被这一面抓到纯粹因为 `ReverseRoutingBadge` 恰好有独立文件，而
 *    `UnlockBadge` / `RuleMetaCounts` / `TerminalEnvBlock` 三个同形的真数据块当时全在面外
 *    （`TerminalEnvBlock` 在移动端至今零处、零登记）。
 *    `extraBlockSources` 补的是另一半：**这一屏的内容块住在别的目录**（规则屏逐条规则的详情面
 *    在 `components/hover-cards/RuleHoverCard.tsx`）。`primitiveModules` 是反向豁免，
 *    它不是白名单：⓪ 组要求那个文件在自有目录里、且**自己一句 `t('…')` 都不渲染**。
 *
 * **C 具名数据槽面** —— 桌面这一屏（全部 `desktopDirs`）的 `id="…"` 字面量，
 *    与各表的 `slotBaseline` **恰等**。
 *    🔴 **这一面是变更探测器，不是对差面**（口径写在 `parity-registry.ts` 的字段注释里）：
 *    它回答「桌面这一屏的数据槽集合有没有在没人看的情况下变过」，**答不出**移动端画没画其中某一个。
 *
 * **D 喂数腿面** —— 见 `feed-register.ts` 头注（取材面在移动端这一侧，判据是写入方存在）。
 *
 * ── 射程自曝：本门**守不住**什么，以及那些格由谁兜 ─────────────────────────
 *
 * 1. **逐个数据槽的对差**（C 面只做基线，不做落点核对）。兜它的是各屏自己的渲染门，
 *    四屏各自把接手门写进 `slotHandoffGates`，⑥ 组**逐条打开核对**（文件在 + 那一组 describe 的
 *    字面标题在）——上一版这句交接只落在四分之一上（只断言了 `MobileSettings.test.tsx`）。
 *    设置屏另有一条更强的正面断言：桌面九张设置子页必须恰好等于
 *    `MOBILE_SETTINGS_PAGES ∪ MOBILE_ABSENT_SETTINGS_PAGE` —— 那两个常量正是
 *    `MobileSettings.test.tsx ①` 的真值源。交接处对不上就红，不是「相信它兜住了」。
 * 2. **一次 `update()` 里键名要到运行期才知道的那一类写腿**（`update({ [key]: v })`）。
 *    本门把它记成 `field:[key]` 这一条**必须被显式判一次**的登记，但它**答不出**那个调用点
 *    今天承载哪几个字段。今天那一条跨 `mixedPort` / `controlPort`，两者的缺席由
 *    `MobileSettings.test.tsx ③` 逐值钉死（那条登记的锚指的就是那道门）。
 * 3. **「登记说移植了、真的画出来没有」的渲染证明**。本门用的是**被打开核对的源码锚**
 *    （`{file, mustContain}`，逐字同 `wiring-completeness.test.ts` 的 `Anchor` 口径），
 *    不是渲染断言。锚证明「那条腿的源码今天还在代码行上」，**不证明**它渲染在一个会为真的分支里。
 *    那一格归各屏的渲染门。⚠️ 锚有三条机器判据（都在 ④ 组，逐条带反向对照）：
 *    **剥注释后**核对（墓碑注释喂不饱它）、`ported`/`reachable-elsewhere` 必须指 `ui/src/mobile/`、
 *    `platform-absent` 必须指一条平台判定。三条都是 2026-09-06 复审实测出来的假绿路径。
 * 4. **动作元素的判据仍是词法的**：认的是 `<button>` / `onClick=` / 动作性 `role=`。
 *    把点击装在自定义组件的**自有 prop** 上（`<Row onActivate={…}>`）仍然看不见 ——
 *    与 `wiring-completeness.test.ts` C 面第 ② 条同一条边界。今天四屏取材面上这类形态的实况：
 *    非 `<button>` 的动作元素共 47 处（首页 2 / 规则 2+4 / 连接 1 / 设置 38），全部已进面并逐条登记；
 *    `onActivate` 这类自定义 prop **零处**（一旦出现，它带来的动作会静默掉在面外）。
 *
 * ── 判据不许被自己污染 ────────────────────────────────────────────────────
 *
 * ④ 组有一条反向对照要证明「只出现在注释里的文本喂不饱锚」，它需要一段**确实只在注释里**的文本。
 * 就是下面这个探针，别删、别在代码里写成一整个字面量（那条断言会拼出它来找）：__墓碑注释反向对照探针__
 *
 * 三个桌面面全部**排除测试文件**（`.test.tsx` / `.spec.tsx`）：这些门的测试文件里满是
 * `<button` 夹具与 `id="…"` 断言，纳进来会让桌面面凭空长大、逼着登记表去登记一批夹具。
 * ⓪ 组带一条**切点自检**：证明排除之后每一面都还剩得下真取材面（不能排成空集），
 * 且证明排除动作**有对象可排**（那些测试文件确实在目录里）。
 */
import { describe, expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { blankComments } from '@/contracts/comment-blanking.test-support';
import { IS_TEST_ONLY_MODULE } from '@/contracts/test-only-modules';

import {
  MOBILE_ABSENT_SETTINGS_PAGE,
  MOBILE_SETTINGS_PAGES,
} from './settings/settings-pages';
import {
  exemptionAnchor,
  isDebt,
  type Anchor,
  type ParityDisposition,
  type ParityEntry,
  type ScreenParityRegister,
} from './parity-registry.test-support';
import { MOBILE_FEEDS } from './feed-register.test-support';
import { HOME_PARITY } from './home/absence-register.test-support';
import { RULES_PARITY } from './screens/rules/absence-register.test-support';
import { CONNECTIONS_PARITY } from './connections/absence-register.test-support';
import { SETTINGS_PARITY } from './settings/absence-register.test-support';

/** 仓根（同 `wiring-completeness.test.ts` 的算法，两处的锚因此可以互相搬）。 */
const REPO = fileURLToPath(new URL('../../../', import.meta.url));
const UI_SRC = fileURLToPath(new URL('..', import.meta.url));
const MOBILE_DIR = fileURLToPath(new URL('.', import.meta.url));

export const REGISTERS: readonly ScreenParityRegister[] = [
  HOME_PARITY,
  RULES_PARITY,
  CONNECTIONS_PARITY,
  SETTINGS_PARITY,
];

/**
 * `ui/src/components/screens/` 下**不在四张表射程内、但确实另有一道门在对差**的桌面屏。
 *
 * 节点屏是本仓这类门的先例（`nodes/absence-register.ts` + `nodes-screen.test.tsx` ⑤⑥
 * 那一对），本门不重复实现它。锚指的就是那一组断言 —— 那道门被删、或那一组被摘掉，⓪ 组当场红。
 * 🔴 想往这里加一行之前先想清楚：这张表是**豁免**，写一行就等于说「那一屏另有人逐颗对差」。
 * 写不出那道门的，就把目录加进某张表的 `desktopDirs`，老实把动作面铺开。
 */
const HANDED_OFF_DESKTOP_DIRS: readonly { readonly dir: string; readonly gate: Anchor }[] = [
  {
    dir: 'ui/src/components/screens/nodes',
    gate: {
      file: 'ui/src/mobile/nodes/nodes-screen.test.tsx',
      mustContain: '⑥ 能力缺席登记表对得上桌面',
    },
  },
];

/* ────────────────────────── 词法工具 ────────────────────────── */

/**
 * 剥注释、**保长度**（剥掉的字符换成空格、换行原样）⇒ 偏移量与原文 1:1，
 * 于是「命中在第几行」可以直接现场算。实现与射程自曝在
 * `contracts/comment-blanking.test-support.ts` —— 那一份是**全仓共用**的：三道门需要同一个
 * 「什么算注释」，不许各留一份拷贝。
 *
 * 🔴 本文件上一版留了自己的拷贝（一个识别字符串的逐字符状态机），它在两类真实源码上会**失步**，
 * 且失步之后是静默吞掉大段代码：JSX 文本里的 `https://223.5.5.5/dns-query` 被当成行注释起点；
 * 一个落单的撇号开出一段假字符串、一直吃到下一个撇号，之后的块注释起点被当成真的开头 ——
 * 实测 `connections-screen.test.tsx` 从第 349 行起 114 行代码被整块抹白，
 * 一条指向那里的锚因此报「找不到」。共用那一份是保守实现：方向只会让判据少认，不会多认。
 *
 * 剥注释是必须的：本仓的头注里满是 `<button` / `id="…"` 这类反面示例与引文，
 * 不剥就会把文档说明当成真实代码抓进取材面。
 */
const strip = blankComments;

/**
 * 「这是一道门」的判据 —— **严格**只认 `.test.` / `.spec.`。
 * `adjudicated-out` 那条断言拿它判「锚指的是不是一道真门」，放宽到 `.test-support.` 会让一份
 * 数据表冒充成门（那个码要的是「有人在持续证明它不出现」，数据表证明不了任何事）。
 */
const IS_GATE_FILE = /\.(?:test|spec)\.tsx?$/;
/**
 * 「这不是产品代码」的判据 —— 用仓里那份**共享**谓词（`contracts/test-only-modules.ts` 头注：
 * 三道门需要同一个概念，不许各留一份拷贝）。
 *
 * 🔴 **本门自己的取材面必须用它**：四屏对差登记表是 `*.test-support.ts`，里面写满了
 * `t('nodes.mobileNeedsFormLayer')` 这样的**锚文本**与理由键。用严格口径扫，⑦ 组那条
 * 「理由键必须在移动端生产源码里真有渲染点」会被**本表自己**喂饱 —— 判据被自己污染的教科书形态。
 * 同理，⑤ 组的 store 读点面也不该把登记表里的 `useAppStore.getState().save()` 类锚文本算成读点。
 */
const IS_TEST = IS_TEST_ONLY_MODULE;

/** 目录下的非测试 `.ts/.tsx`（不递归 —— 桌面一屏就是一个平目录）。 */
function screenFiles(dirAbs: string): string[] {
  return readdirSync(dirAbs)
    .filter((name) => /\.tsx?$/.test(name) && !IS_TEST.test(name))
    .sort();
}

/** 目录下的非测试 `.ts/.tsx`，**递归**（移动端一屏可能有子目录）。 */
function walk(dirAbs: string, out: string[] = []): string[] {
  for (const name of readdirSync(dirAbs).sort()) {
    const full = join(dirAbs, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.tsx?$/.test(full) && !IS_TEST.test(full)) out.push(full);
  }
  return out;
}

/** `index` 落在第几行（1 起）。锚报告里那个 `file:line` 就是这么算的，不写死。 */
function lineOf(text: string, index: number): number {
  return text.slice(0, index).split('\n').length;
}

/* ────────────────────────── A 面：动作 ────────────────────────── */

interface DesktopAction {
  readonly file: string;
  readonly id: string;
  readonly tag: string;
}

/** onClick 里跳过的词：语言关键字与形参名，它们不是「这颗按钮做什么」。 */
const HANDLER_NOISE = new Set([
  'void', 'return', 'new', 'typeof', 'await', 'async', 'if', 'else', 'const', 'let',
  'true', 'false', 'null', 'undefined', 'function', 'e', 'event',
]);

/** 动作性 `role=` —— 无障碍契约里「这东西点得动」的那几档。 */
const ACTION_ROLE = /\brole="(?:button|switch|tab|menuitem|link)"/;

/**
 * 一个元素从开标签到它**自己那个**闭标签的整段（同名嵌套逐层配平；自闭合就只有开标签）。
 *
 * 「配平」不是讲究：`<span role="switch">` 外面常包着别的 `<span>`，裸 `indexOf('</span>')`
 * 会把邻居的 `t('…')` 键吞进身份里，于是改邻居的文案会让这颗控件的 `id` 漂掉。
 */
function elementBody(body: string, name: string, start: number, tagEnd: number): string {
  if (body[tagEnd - 1] === '/') return body.slice(start, tagEnd + 1);
  const open = new RegExp(`<${name}(?=[\\s/>])`, 'g');
  const close = `</${name}>`;
  let depth = 1;
  let i = tagEnd + 1;
  while (i < body.length) {
    const nextClose = body.indexOf(close, i);
    if (nextClose === -1) return body.slice(start, tagEnd + 1);
    open.lastIndex = i;
    let nested = 0;
    for (let m = open.exec(body); m !== null && m.index < nextClose; m = open.exec(body)) {
      // 自闭合的同名标签不增加深度
      let j = m.index;
      let d = 0;
      while (j < body.length) {
        const c = body[j];
        if (c === '{') d += 1;
        else if (c === '}') d -= 1;
        else if (c === '>' && d === 0) break;
        j += 1;
      }
      if (body[j - 1] !== '/') nested += 1;
    }
    depth += nested - 1;
    if (depth <= 0) return body.slice(start, nextClose);
    i = nextClose + close.length;
  }
  return body.slice(start, tagEnd + 1);
}

/**
 * 一个文件里的全部**动作元素**：任何开标签（按 `{}` 配平取到 `>`）满足
 * 「标签名是 `button`」**或**「带 `onClick=`」**或**「带动作性 `role=`」，再取到对应闭标签。
 *
 * 🔴 上一版只认字面 `<button`。2026-09-06 复审 major 的实测：桌面出口面板里的「按延迟排序」
 * 是一枚 `<span className="swt" role="switch" onClick={toggleLatencySort}>`，移动端把整块面板
 * 移植了却漏了这一维 —— 那颗既不在面上（看不见），又因为 ① 是**集合恒等**而不允许人手补登。
 * 今天四屏上这类非 `<button>` 控件实存 4 处（`NodeMenu.tsx` 的排序开关、`GeoCard.tsx` 与
 * `RuleItem.tsx` 的行内开关、`ConnectionsScreen.tsx` 的列排序 `<th>`），其中一处就是活缺口。
 */
function actionElementsOf(body: string): { tag: string; seg: string; index: number }[] {
  const out: { tag: string; seg: string; index: number }[] = [];
  for (const m of body.matchAll(/<([a-zA-Z][A-Za-z0-9.]*)(?=[\s/>])/g)) {
    const start = m.index;
    let i = start + m[0].length;
    let depth = 0;
    let tagEnd = -1;
    while (i < body.length) {
      const c = body[i];
      if (c === '{') depth += 1;
      else if (c === '}') depth -= 1;
      else if (c === '>' && depth === 0) {
        tagEnd = i;
        break;
      }
      i += 1;
    }
    if (tagEnd === -1) continue;
    const tag = body.slice(start, tagEnd + 1);
    const name = m[1]!;
    if (name !== 'button' && !/\bonClick=/.test(tag) && !ACTION_ROLE.test(tag)) continue;
    out.push({ tag: tag.replace(/\s+/g, ' '), seg: elementBody(body, name, start, tagEnd), index: start });
  }
  return out;
}

/** 一颗按钮的身份（口径见文件头注 A 面那一段）。 */
export function actionId(button: { tag: string; seg: string }): string {
  const byId = /\bid="([^"{}]+)"/.exec(button.tag);
  if (byId) return `#${byId[1]}`;
  const keys = [...new Set([...button.seg.matchAll(/\bt\(\s*'([^']+)'/g)].map((m) => m[1]!))].sort();
  if (keys.length > 0) return `k:${keys.join('+')}`;
  const byClass = /className="([^"{}]+)"/.exec(button.tag);
  if (byClass) return `c:${byClass[1]}`;
  const onClick = /\bonClick=\{([\s\S]*?)\}\s*(?:[A-Za-z_$-]+=|\/?>)/.exec(button.tag);
  if (onClick) {
    for (const ident of onClick[1]!.matchAll(/[A-Za-z_$][\w$]*/g)) {
      if (!HANDLER_NOISE.has(ident[0])) return `f:${ident[0]}`;
    }
  }
  return `h:${createHash('sha1').update(button.tag).digest('hex').slice(0, 8)}`;
}

/**
 * 一屏**渲染一跳**够得到的仓内文件：解析这一屏各文件的具名 import，凡被当作 JSX 标签用过的符号，
 * 就把它的来源文件拉进来。返回 `相对仓根路径 → 用到的组件名集合`（不含本屏自己的目录）。
 *
 * 🔴 这一跳是 2026-09-06 复审 major 逼出来的：动作面上一版钉死在「桌面这一屏自己的目录」，
 * 于是**这一屏渲染、组件住在别处**的动作整条掉在面外 —— 连接屏右键面板里的
 * `RuleSubjectMenuItems`（新建规则 / 合并进已有规则两颗）就是活的一处。判据因此从**词法**
 * （数这个目录里的 `<button`）换成**结构**（顺着 import 边走一跳），与 `app-wiring.test.tsx` ⑫
 * 第二个面同一条路子。类型 import 与 React 内建天然在外（前者不产 JSX 标签，后者不是仓内文件）。
 */
function renderedImports(dirsAbs: readonly string[]): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  const inOwnDirs = (abs: string): boolean => dirsAbs.some((d) => abs.startsWith(`${d}/`));
  const resolveSpec = (fromFile: string, spec: string): string | null => {
    let base: string;
    if (spec.startsWith('@/')) base = join(UI_SRC, spec.slice(2));
    else if (spec.startsWith('.')) base = resolve(dirname(fromFile), spec);
    else return null; // 包名 —— 不是仓内文件
    for (const ext of ['.tsx', '.ts', '/index.tsx', '/index.ts']) {
      if (existsSync(base + ext)) return base + ext;
    }
    return existsSync(base) && /\.tsx?$/.test(base) ? base : null;
  };
  for (const dirAbs of dirsAbs) {
    for (const name of screenFiles(dirAbs)) {
      const full = join(dirAbs, name);
      const body = strip(readFileSync(full, 'utf8'));
      const tags = new Set(
        [...body.matchAll(/(^|[^A-Za-z0-9_$>\]])<([A-Z][A-Za-z0-9]*)(?=[\s/>])/g)].map((m) => m[2]!),
      );
      for (const im of body.matchAll(/import\s+(type\s+)?\{([^}]*)\}\s*from\s*'([^']+)'/g)) {
        if (im[1]) continue; // `import type { … }` 不产标签
        const used = im[2]!
          .split(',')
          .map((piece) => piece.trim().split(/\s+as\s+/).pop()!.trim())
          .filter((n) => n !== '' && tags.has(n));
        if (used.length === 0) continue;
        const target = resolveSpec(full, im[3]!);
        if (target === null || inOwnDirs(target)) continue;
        const rel = relative(REPO, target);
        const bucket = out.get(rel) ?? new Set<string>();
        for (const n of used) bucket.add(n);
        out.set(rel, bucket);
      }
    }
  }
  return out;
}

/**
 * 一份源码里每个顶层组件声明覆盖的字节区间（到下一个顶层声明为止）。
 *
 * 渲染一跳只该带进**这一屏真渲染的那几个组件**的动作 —— 规则屏从 `settings/Primitives.tsx`
 * 里 import 的只有 `Switch`，把那份原语库里 `Button` / `Select` 的动作也算进规则屏的面，
 * 数出来的不是这一屏的能力。
 */
function componentSpans(body: string): Map<string, [number, number]> {
  const decls: { name: string; at: number }[] = [];
  for (const m of body.matchAll(/(?:^|\n)(?:export\s+)?(?:default\s+)?(?:function|const|class)\s+([A-Za-z_$][\w$]*)\b/g)) {
    decls.push({ name: m[1]!, at: m.index });
  }
  const out = new Map<string, [number, number]>();
  decls.forEach((d, i) => {
    out.set(d.name, [d.at, i + 1 < decls.length ? decls[i + 1]!.at : body.length]);
  });
  return out;
}

/** 一屏动作面的取材文件：自有目录（整份文件）∪ 渲染一跳（只取被渲染的那几个组件的区间）。 */
interface ActionSource {
  readonly abs: string;
  /** `null` = 整份文件都算；否则只算这几个组件声明覆盖的区间。 */
  readonly onlyComponents: ReadonlySet<string> | null;
}

function actionSources(dirsAbs: readonly string[]): ActionSource[] {
  const out: ActionSource[] = dirsAbs.flatMap((d) =>
    screenFiles(d).map((n) => ({ abs: join(d, n), onlyComponents: null })),
  );
  for (const [rel, used] of renderedImports(dirsAbs)) {
    out.push({ abs: join(REPO, rel), onlyComponents: used });
  }
  return out.sort((a, b) => (a.abs < b.abs ? -1 : a.abs > b.abs ? 1 : 0));
}

function desktopActions(sources: readonly ActionSource[]): DesktopAction[] {
  const out: DesktopAction[] = [];
  const seen = new Map<string, number>();
  for (const source of sources) {
    const name = source.abs.slice(source.abs.lastIndexOf('/') + 1);
    const body = strip(readFileSync(source.abs, 'utf8'));
    let inScope: (index: number) => boolean = () => true;
    if (source.onlyComponents !== null) {
      const spans = componentSpans(body);
      const ranges = [...source.onlyComponents].flatMap((n) => {
        const span = spans.get(n);
        return span ? [span] : [];
      });
      inScope = (index) => ranges.some(([from, to]) => index >= from && index < to);
    }
    for (const element of actionElementsOf(body)) {
      if (!inScope(element.index)) continue;
      const base = actionId(element);
      const key = `${name}|${base}`;
      const ordinal = (seen.get(key) ?? 0) + 1;
      seen.set(key, ordinal);
      out.push({ file: name, id: `${key}${ordinal > 1 ? `#${ordinal}` : ''}`, tag: element.tag });
    }
  }
  return out;
}

/** `{` / `(` / `[` 的配平位置；起点必须是三者之一，否则返回 -1。 */
function matchBracket(src: string, open: number): number {
  const want = { '{': '}', '(': ')', '[': ']' }[src[open] as '{' | '(' | '['];
  if (want === undefined) return -1;
  let depth = 0;
  for (let i = open; i < src.length; i += 1) {
    const c = src[i]!;
    if (c === '{' || c === '(' || c === '[') depth += 1;
    else if (c === '}' || c === ')' || c === ']') {
      depth -= 1;
      if (depth === 0) return c === want ? i : -1;
    }
  }
  return -1;
}

/** 从 `i` 起跳到本层的下一个 `,`（或本层结束）。 */
function skipToComma(src: string, from: number): number {
  let depth = 0;
  for (let i = from; i < src.length; i += 1) {
    const c = src[i]!;
    if (c === '{' || c === '(' || c === '[') depth += 1;
    else if (c === '}' || c === ')' || c === ']') {
      if (depth === 0) return i;
      depth -= 1;
    } else if (c === ',' && depth === 0) return i;
  }
  return src.length;
}

interface ObjectEntry {
  readonly key: string;
  /** 值在 `inner` 里的起点；简写属性（`{ foo }`）为 -1。 */
  readonly valueStart: number;
}

/**
 * 一个对象字面量**内容**（不含最外层大括号）里的全部顶层键。
 *
 * 认四种键形：标识符、字符串字面量、计算键 `[expr]`（记作 `[expr]` 本身）、简写属性。
 * `...spread` 跳过（它不是这次写的字段，是被复制的旧值）。
 */
function objectEntries(inner: string): ObjectEntry[] {
  const out: ObjectEntry[] = [];
  let i = 0;
  const skipWs = (): void => {
    while (i < inner.length && /\s/.test(inner[i]!)) i += 1;
  };
  while (i < inner.length) {
    const before = i;
    skipWs();
    if (i >= inner.length) break;
    if (inner.startsWith('...', i)) {
      i = skipToComma(inner, i + 3);
    } else {
      let key: string | null = null;
      if (inner[i] === '[') {
        const close = matchBracket(inner, i);
        if (close === -1) break;
        key = `[${inner.slice(i + 1, close).trim()}]`;
        i = close + 1;
      } else if (inner[i] === "'" || inner[i] === '"') {
        const quote = inner[i]!;
        let j = i + 1;
        while (j < inner.length && inner[j] !== quote) j += inner[j] === '\\' ? 2 : 1;
        key = inner.slice(i + 1, j);
        i = j + 1;
      } else {
        const m = /^[A-Za-z_$][\w$]*/.exec(inner.slice(i));
        if (m) {
          key = m[0];
          i += m[0].length;
        }
      }
      if (key === null) i = skipToComma(inner, i);
      else {
        skipWs();
        if (inner[i] === ':') {
          i += 1;
          skipWs();
          out.push({ key, valueStart: i });
          i = skipToComma(inner, i);
        } else {
          out.push({ key, valueStart: -1 });
          i = skipToComma(inner, i);
        }
      }
    }
    skipWs();
    if (inner[i] === ',') i += 1;
    if (i <= before) i = before + 1; // 进度保险：任何一条分支都不许原地打转
  }
  return out;
}

/**
 * `update({ … })` 写出去的配置字段。设置屏两个动作面之一。
 *
 * 🔴 上一版是一条正则 `update\s*\(\s*\{\s*([A-Za-z_$][\w$]*)`：只取**第一个字面量键**。
 * 2026-09-06 复审两条 major 的实测：`update({ dnsDefaults: …, unmatchedAction })` 这类一次写两格的
 * 调用只被数了一半（`dnsDefaults` 与 `ruleResourceUpdateIntervalHours` 两个字段整条掉在面外，
 * 删掉移动端对应的写入门照绿），而 `update({ [key]: next })` 这类计算键连第一个都匹配不上。
 * 现在按括号配平取整个对象字面量，收**全部顶层键**：
 *  · 计算键记作 `[key]` 本身 —— 它是一条真实的写腿，只是键名要到运行期才知道，
 *    故它**必须被登记表显式判一次**（今天那一条承载 `mixedPort` / `controlPort`）。
 *  · 值本身又是对象字面量时，**再展开一层**（`update({ dnsConfig: { x: 1 } })` ⇒ `dnsConfig.x`）。
 *    今天这一层在桌面上产出 **3 条**（`dnsConfig.enableFakeIp` / `dnsConfig.fakeIpTunAutoEnable` /
 *    `dnsDefaults.unmatchedAction`），其中后一条正是一处真缺口：移动端读得出、画得出，但改不了。
 *    ⚠️ **只展开一层**，且只在调用点写成内联字面量时才看得见 —— 桌面传算好的变量
 *    （`update({ dnsConfig: next })`）时，那次写进去的子字段本面看不见。那一格由
 *    `MobileSettings.test.tsx ①` 的九子页逐项对差兜（交接由 ⑥ 组那条恰等断言钉死）。
 *    展开一层与计算键的行为对照见 ⓪ 组合成用例。
 */
export function updatedConfigFieldsIn(source: string): Set<string> {
  const body = strip(source);
  const fields = new Set<string>();
  for (const m of body.matchAll(/\bupdate\s*\(/g)) {
    let i = m.index + m[0].length;
    while (i < body.length && /\s/.test(body[i]!)) i += 1;
    if (body[i] !== '{') continue;
    const close = matchBracket(body, i);
    if (close === -1) continue;
    const inner = body.slice(i + 1, close);
    for (const entry of objectEntries(inner)) {
      fields.add(entry.key);
      if (entry.valueStart >= 0 && inner[entry.valueStart] === '{') {
        const nested = matchBracket(inner, entry.valueStart);
        if (nested !== -1) {
          for (const sub of objectEntries(inner.slice(entry.valueStart + 1, nested))) {
            fields.add(`${entry.key}.${sub.key}`);
          }
        }
      }
    }
  }
  return fields;
}

export function updatedConfigFields(dirsAbs: readonly string[], recursive = false): Set<string> {
  const files = dirsAbs.flatMap((dirAbs) =>
    recursive ? walk(dirAbs) : screenFiles(dirAbs).map((n) => join(dirAbs, n)),
  );
  const fields = new Set<string>();
  for (const abs of files) {
    for (const field of updatedConfigFieldsIn(readFileSync(abs, 'utf8'))) fields.add(field);
  }
  return fields;
}

/* ────────────────────────── B 面：数据块 ────────────────────────── */

/** 一份源码里 `function Foo` / `const Foo =` 声明出来的大写开头符号（组件候选）。 */
function declaredComponents(body: string): Set<string> {
  const out = new Set<string>();
  for (const m of body.matchAll(/(?:^|\n)\s*(?:export\s+)?(?:default\s+)?(?:function|const)\s+([A-Z][A-Za-z0-9]*)\b/g)) {
    out.add(m[1]!);
  }
  return out;
}

/** 一份源码里被当作 JSX 标签用过的大写开头符号。 */
function renderedTags(body: string): Set<string> {
  const out = new Set<string>();
  // `(^|[^A-Za-z0-9_$>\]])` 那个前缀是**把泛型排掉**：`useRef<HTMLDivElement>` /
  // `useState<SortKey>` / `Partial<UserConfig>` 里的 `<` 前面一定是标识符字符、`>` 或 `]`，
  // 而 JSX 的 `<` 前面一定不是。没有这一条，取材面会混进一批类型名（实测 5 个）。
  for (const m of body.matchAll(/(^|[^A-Za-z0-9_$>\]])<([A-Z][A-Za-z0-9]*)(?=[\s/>])/g)) out.add(m[2]!);
  return out;
}

/**
 * 桌面这一屏渲染的**内容块**。
 *
 * 取材面 = 「这一屏 `desktopDirs` 里各文件**声明**出来的组件名 ∪ 那些文件的基名
 * ∪ `extraBlockSources` 里声明出来的组件名」∩「这一屏各文件里真被当作 JSX 标签渲染的名字」，
 * 再减掉 `primitiveModules` 里声明的那些。
 *
 * 🔴 上一版的 `own` 是**文件基名集合**，于是「同目录、同文件内定义并渲染」的块整条不可见 ——
 * W-27 之所以被这一面抓到，纯粹因为 `ReverseRoutingBadge` 恰好有独立文件。2026-09-06 复审
 * 三条 finding 记的实测：桌面今天就有 `HomeScreen.tsx` 的 `UnlockBadge`、`RuleItem.tsx` 的
 * `RuleMetaCounts` / `CondCount`、`SettingsNetwork.tsx` 的 `TerminalEnvBlock` 四个同形的真数据块
 * 掉在面外（其中 `TerminalEnvBlock` 在移动端全树零处、零登记），而在 `HomeScreen.tsx` 里新增
 * 一个 `KillSwitchBadge` 并渲染它，全仓 248 个测试文件照绿。
 *
 * `extraBlockSources` 补的是另一半：**这一屏的内容块住在别的目录**（规则屏逐条规则的详情面
 * `components/hover-cards/RuleHoverCard.tsx` 就是，`rules.noConditions` 那句「（无条件）」
 * 从这条缝漏掉）。它与 `sharedPrimitives` 合起来必须**恰好**覆盖这一屏的渲染一跳，见 ⓪ 组。
 */
export function ownBlocks(
  dirsAbs: readonly string[],
  primitiveModulesAbs: readonly string[] = [],
  extraBlockSourcesAbs: readonly string[] = [],
): string[] {
  const own = new Set<string>();
  const primitive = new Set<string>();
  const tags = new Set<string>();
  for (const dirAbs of dirsAbs) {
    for (const name of screenFiles(dirAbs)) {
      const abs = join(dirAbs, name);
      const body = strip(readFileSync(abs, 'utf8'));
      const declared = declaredComponents(body);
      if (primitiveModulesAbs.includes(abs)) {
        for (const n of declared) primitive.add(n);
      } else {
        for (const n of declared) own.add(n);
        own.add(name.replace(/\.tsx?$/, ''));
      }
      for (const n of renderedTags(body)) tags.add(n);
    }
  }
  for (const abs of extraBlockSourcesAbs) {
    for (const n of declaredComponents(strip(readFileSync(abs, 'utf8')))) own.add(n);
  }
  return [...tags].filter((n) => own.has(n) && !primitive.has(n)).sort();
}

/* ────────────────────────── C 面：具名数据槽 ────────────────────────── */

export function namedSlots(dirsAbs: readonly string[]): string[] {
  const slots = new Set<string>();
  for (const dirAbs of dirsAbs) {
    for (const name of screenFiles(dirAbs)) {
      const body = strip(readFileSync(join(dirAbs, name), 'utf8'));
      for (const m of body.matchAll(/\bid="([^"{}]+)"/g)) slots.add(m[1]!);
    }
  }
  return [...slots].sort();
}

/* ────────────────────────── D 面：喂数腿 ────────────────────────── */

const STORE_HOOK = /Store$|^useEffectiveConfig$|^useEffectiveServers$|^useSystemProxyLive$/;

function closingParen(src: string, openIndex: number): number {
  let depth = 0;
  for (let i = openIndex; i < src.length; i += 1) {
    const c = src[i];
    if (c === '(') depth += 1;
    else if (c === ')') {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/**
 * 把**模块内的 store 别名**还原成它指向的那个 store，再交给下面三种形态去认。
 *
 * 形态：`const <名字> = () => useXStore.getState();`（声明到 `getState()` 为止，后面没有 `.字段`
 * —— `const cfg = () => useAppStore.getState().config;` 是别名到**一格**，不是别名到 store，
 * 认它会把 `cfg()` 的每一次调用都算成对同名字段的读）。还原后 `<名字>()` 逐字变成
 * `useXStore.getState()`。
 *
 * 🔴 **为什么必须有这一步**（2026-09-13 实测出来的洞）：`mobile/app-wiring.ts` 顶上就有
 * `const store = () => useAppStore.getState();`，而那个文件里十余条应用级订阅**主流写法**正是
 * `store().<写腿>(…)`。取材面只认 `useXStore.getState().<字段>` 时，经别名的读点在这张面上
 * **隐形**：同一条订阅换个写法（功能、真机行为、其余全部测试都不变）就会从面上消失，
 * 于是喂数腿登记表里对应那行变成「只在表里」而红 —— 而那时最顺手的「修法」是**把登记删掉**，
 * 一笔已付的账就这么退回成「自己看不见的写入方」。
 *
 * 这一步只**放宽**取材面（多认一种等价写法），不减少任何既有覆盖：别名不存在时它逐字节不改动源码。
 */
function resolveStoreAliases(body: string): string {
  const alias = /\bconst\s+([A-Za-z_$][\w$]*)\s*=\s*\(\s*\)\s*=>\s*(use[A-Z][A-Za-z0-9]*)\s*\.\s*getState\s*\(\s*\)\s*;/g;
  let out = body;
  for (const m of body.matchAll(alias)) {
    const [, name, hook] = m;
    out = out.replace(new RegExp(`\\b${name}\\s*\\(\\s*\\)`, 'g'), `${hook}.getState()`);
  }
  return out;
}

/** 三种词法形态（口径见 `feed-register.ts` 头注）。返回 `id → 读点文件集合`。 */
export function storeReads(sources: ReadonlyMap<string, string>): Map<string, Set<string>> {
  const reads = new Map<string, Set<string>>();
  const add = (id: string, file: string): void => {
    if (!STORE_HOOK.test(id.split('.')[0]!)) return;
    const bucket = reads.get(id) ?? new Set<string>();
    bucket.add(file);
    reads.set(id, bucket);
  };
  for (const [file, raw] of sources) {
    const body = resolveStoreAliases(raw);
    for (const m of body.matchAll(/\b(use[A-Z][A-Za-z0-9]*)\s*\.\s*getState\s*\(\s*\)\s*\.\s*([A-Za-z_$][\w$]*)/g)) {
      add(`${m[1]}.${m[2]}`, file);
    }
    for (const m of body.matchAll(/\b(use[A-Z][A-Za-z0-9]*)\s*\(/g)) {
      const open = m.index + m[0].length - 1;
      const end = closingParen(body, open);
      if (end === -1) continue;
      const arg = body.slice(open + 1, end).trim();
      if (arg === '') {
        add(`${m[1]}.*`, file);
        continue;
      }
      const arrow = /^\(?\s*([A-Za-z_$][\w$]*)\s*\)?\s*=>/.exec(arg);
      if (!arrow) continue;
      const param = arrow[1]!;
      const bodyExpr = arg.slice(arrow[0].length);
      const re = new RegExp(`\\b${param}\\s*\\??\\.\\s*([A-Za-z_$][\\w$]*)`, 'g');
      let hit = false;
      for (const pm of bodyExpr.matchAll(re)) {
        hit = true;
        add(`${m[1]}.${pm[1]}`, file);
      }
      if (!hit) add(`${m[1]}.*`, file);
    }
  }
  return reads;
}

/* ────────────────────────── 真实取材 ────────────────────────── */

const MOBILE_FILES = walk(MOBILE_DIR);
const MOBILE_SOURCES: ReadonlyMap<string, string> = new Map(
  MOBILE_FILES.map((f) => [relative(UI_SRC, f), strip(readFileSync(f, 'utf8'))]),
);
const FEED_FACE = storeReads(MOBILE_SOURCES);

interface ScreenFace {
  readonly register: ScreenParityRegister;
  readonly desktopDirsAbs: readonly string[];
  /** `controls` 面（元素动作），空数组表示这一屏没声明这个面。 */
  readonly controls: readonly string[];
  /** `config-fields` 面（`field:` 前缀），空数组同上。 */
  readonly configFields: readonly string[];
  /** 两个面的并集 —— 登记表要与它恰等的那一个。 */
  readonly actions: readonly string[];
  readonly actionDetail: ReadonlyMap<string, string>;
  readonly actionSources: readonly ActionSource[];
  readonly oneHop: ReadonlyMap<string, Set<string>>;
  readonly blocks: readonly string[];
  readonly slots: readonly string[];
}

const FACES: readonly ScreenFace[] = REGISTERS.map((register) => {
  const desktopDirsAbs = register.desktopDirs.map((d) => join(REPO, d));
  const detail = new Map<string, string>();
  const sources = actionSources(desktopDirsAbs);
  let controls: string[] = [];
  if (register.actionFaces.includes('controls')) {
    const found = desktopActions(sources);
    for (const a of found) detail.set(a.id, a.tag);
    controls = found.map((a) => a.id);
  }
  let configFields: string[] = [];
  if (register.actionFaces.includes('config-fields')) {
    configFields = [...updatedConfigFields(desktopDirsAbs)].sort().map((f) => `field:${f}`);
    for (const id of configFields) detail.set(id, `update({ ${id.slice('field:'.length)}: … })`);
  }
  return {
    register,
    desktopDirsAbs,
    controls,
    configFields,
    actions: [...controls, ...configFields],
    actionDetail: detail,
    actionSources: sources,
    oneHop: renderedImports(desktopDirsAbs),
    blocks: ownBlocks(
      desktopDirsAbs,
      register.primitiveModules.map((f) => join(REPO, f)),
      register.extraBlockSources.map((f) => join(REPO, f)),
    ),
    slots: namedSlots(desktopDirsAbs),
  };
});

/** 移动端设置树真的写出去的配置字段 —— `config-fields` 面的「已接」信号（是**写**，不是文案）。 */
const MOBILE_WRITTEN_FIELDS = updatedConfigFields([join(REPO, SETTINGS_PARITY.mobileDir)], true);

/* ────────────────────────── 锚 ────────────────────────── */

interface ResolvedAnchor {
  readonly owner: string;
  readonly anchor: Anchor;
  readonly exists: boolean;
  readonly hit: number;
}

/**
 * 锚在**剥掉注释之后**的源码上核对。
 *
 * 🔴 上一版是对原文裸 `indexOf`。2026-09-06 复审 major 的实测：把移动端首页那块
 * `{props.reverseRouting && (…)}` 整块删掉、只留一行「此处曾有 `t('home.reverseRoutingBadge')`
 * 那一行常驻指示器，整块已移走」的墓碑注释 ⇒ screen-parity 33/33 全绿，**包括 W-27 那条专门的
 * 回放断言**。而本仓自己就有写墓碑注释的习惯（`contracts/types.ts:753`）。
 * `strip()` 保长度，所以 `lineOf()` 照样算得出真行号；反向对照见 ④ 组。
 */
function resolveAnchor(owner: string, anchor: Anchor): ResolvedAnchor {
  const abs = join(REPO, anchor.file);
  if (!existsSync(abs)) return { owner, anchor, exists: false, hit: -1 };
  const text = strip(readFileSync(abs, 'utf8'));
  const at = text.indexOf(anchor.mustContain);
  return { owner, anchor, exists: true, hit: at === -1 ? -1 : lineOf(text, at) };
}

/**
 * 平台判定的词法标记 —— `platform-absent` 的锚**自己那段文本里**必须有一处。
 *
 * 🔴 判据是「锚文本自身含平台判定」，**不是**「命中点附近有平台判定」。后者试过，太弱：
 * `app_tray.rs` 这种整份文件都挂满 `#[cfg(desktop)]` 的源码里，随便指一行 `use …;` 都能通过
 * （实测：把托盘那条锚改成指文件顶部的一句 `use std::sync::atomic::Ordering;`，邻域判据全绿）。
 * 现在要求锚**把那条守卫连同被守的那一行一起引下来** —— 读锚的人一眼看见平台分叉，
 * 而删掉守卫、留着被守的代码时锚当场失配。
 *
 * 收的是 cfg 属性 / 宏、`target_os` / `target_family`、`platform != "android"` 这类运行期平台
 * 比较，以及 Android 侧的平台对象（`VpnService` / `@RequiresApi`）。故意**不**收「出现过
 * android / mobile / desktop 这些词」：那样随便一句提到 Android 的代码都能当依据。
 */
const PLATFORM_TOKEN = /#\[cfg\(|cfg!\(|target_os|target_family|platform\s*[!=]==?|Platform::|VpnService|@RequiresApi/;

/** 平台专属的源码树 —— 整棵树只为一个平台存在时，路径本身就是那条判定。 */
const PLATFORM_PATH = /(?:^|\/)(?:android|ios|apple|windows|macos|linux)(?:\/|\.)/i;

const ALL_ENTRIES: ReadonlyArray<readonly [string, ParityEntry]> = REGISTERS.flatMap((r) => [
  ...r.actions.map((e) => [`${r.screen}/action/${e.id}`, e] as const),
  ...r.blocks.map((e) => [`${r.screen}/block/${e.id}`, e] as const),
]);

const ALL_ANCHORS: readonly ResolvedAnchor[] = [
  ...ALL_ENTRIES.flatMap(([owner, entry]) => {
    const anchor = exemptionAnchor(entry.disposition);
    return anchor ? [resolveAnchor(owner, anchor)] : [];
  }),
  ...MOBILE_FEEDS.flatMap((entry) => {
    const anchor = exemptionAnchor(entry.disposition);
    return anchor ? [resolveAnchor(`feed/${entry.id}`, anchor)] : [];
  }),
];

/** 排期措辞 —— 逐字同 `app-wiring.test.tsx` ⑫：排期不是判定。 */
const EXCUSE = /本批|下一批|以后|排期|暂时不做|先不做|还没做|待接线/;

const ZH_CN = JSON.parse(readFileSync(join(UI_SRC, 'i18n', 'locales', 'zh-CN.json'), 'utf8')) as Record<string, unknown>;
function localeHas(key: string): boolean {
  return (
    typeof key.split('.').reduce<unknown>((cur, seg) => (cur as Record<string, unknown>)?.[seg], ZH_CN) ===
    'string'
  );
}

/* ────────────────────────── 报告 ────────────────────────── */

if (process.env.POLARIS_WIRING_REPORT === '1') {
  const lines: string[] = ['════════ 四屏 × 桌面对差 ════════'];
  for (const face of FACES) {
    const debts = [...face.register.actions, ...face.register.blocks].filter((e) => isDebt(e.disposition));
    lines.push(
      `${face.register.screen.padEnd(12)} 动作面(${face.register.actionFaces.join('+')})=${face.actions.length}` +
        `  数据块=${face.blocks.length}  数据槽基线=${face.slots.length}  登记=${
          face.register.actions.length + face.register.blocks.length
        }  其中债务=${debts.length}`,
    );
  }
  for (const face of FACES) {
    lines.push(
      `  ${face.register.screen} 取材面：目录 ${face.register.desktopDirs.join(' + ')}` +
        `  一跳 ${[...face.oneHop.keys()].join(' , ') || '（无）'}`,
    );
  }
  lines.push(`喂数腿面   读点=${FEED_FACE.size}  登记=${MOBILE_FEEDS.length}`);
  lines.push('──────── 锚（现场算出来的 file:line，不是写死的常量） ────────');
  for (const a of ALL_ANCHORS) {
    lines.push(`  ${a.owner}\n      ${a.anchor.file}:${a.hit}  «${a.anchor.mustContain.slice(0, 60)}»`);
  }
  console.log(lines.join('\n'));
}

/* ────────────────────────── 断言 ────────────────────────── */

describe('⓪ 自检：四个取材面都是活的，且判据没被自己污染', () => {
  it('桌面各屏的目录都在，且排除测试文件这件事**有对象可排**', () => {
    for (const face of FACES) {
      let excludedTotal = 0;
      for (const dirAbs of face.desktopDirsAbs) {
        expect(existsSync(dirAbs), `桌面目录不见了：${relative(REPO, dirAbs)}`).toBe(true);
        excludedTotal += readdirSync(dirAbs).filter((n) => /\.tsx?$/.test(n) && IS_TEST.test(n)).length;
        // 切点自检：排完之后还剩得下真取材面（不能排成空集）。
        expect(
          screenFiles(dirAbs).length,
          `${face.register.screen}/${relative(REPO, dirAbs)}：排除测试文件后没有源码剩下`,
        ).toBeGreaterThan(0);
      }
      expect(
        excludedTotal,
        `${face.register.screen}：一个测试文件都没排掉 —— 那条排除规则可能已经失效（或者目录里真没有测试，` +
          '两种情况都要看一眼)',
      ).toBeGreaterThan(0);
    }
  });

  it('`ui/src/components/screens/` 下每个桌面屏目录都被认领了（新开一个屏不许掉在面外）', () => {
    // 🔴 这一条是 2026-09-06 复审 blocker 的落点：上一版一张表只对一个桌面目录，而移动端 IA §1.1
    // 把八个桌面屏折进五个目的地 —— `logs` 折进移动连接屏、`app-policy` / `resources` 折进移动规则屏。
    // 那三个目录（28 颗动作）不在任何一面上，`logs.archiveLegacy` / `logs.deleteLegacy` 因此
    // 既没接、也没登记、也没人看得见。今天补的不只是那三个目录，还有这条**面外自曝**：
    // 桌面新开一个屏，要么进某张表的 `desktopDirs`，要么显式写明由谁兜，否则当场红。
    const root = join(UI_SRC, 'components', 'screens');
    const dirs = readdirSync(root)
      .filter((n) => statSync(join(root, n)).isDirectory())
      .sort();
    expect(dirs.length, '桌面屏目录一个都没枚举到 ⇒ 本条恒绿').toBeGreaterThan(5);
    const claimed = new Set(REGISTERS.flatMap((r) => r.desktopDirs.map((d) => d.split('/').pop()!)));
    const handedOff = new Set(HANDED_OFF_DESKTOP_DIRS.map((h) => h.dir.split('/').pop()!));
    const orphans: string[] = [];
    for (const name of dirs) {
      if (claimed.has(name) || handedOff.has(name)) continue;
      // 唯一的自动豁免：那个目录里**一个非测试 `.tsx` 都没有**（不是屏，是纯逻辑）。
      // 这是可被推翻的事实，不是一句话：往里放一个 `.tsx`，它立刻掉出豁免。
      const surfaces = screenFiles(join(root, name)).filter((f) => f.endsWith('.tsx'));
      if (surfaces.length === 0) continue;
      orphans.push(`${name}（${surfaces.length} 个 .tsx）`);
    }
    expect(
      orphans,
      '这些桌面屏目录既不在任何一张表的 `desktopDirs` 上，也不在「另有门兜」表上 ——\n' +
        '正是「桌面有、移动端一行没写、也没人登记」那一格：\n' +
        orphans.join('\n'),
    ).toEqual([]);
    // 正面对照：豁免那一档今天确实有对象（`shared` 是纯逻辑目录），且「另有门兜」的那道门真的存在。
    expect(screenFiles(join(root, 'shared')).filter((f) => f.endsWith('.tsx'))).toEqual([]);
    for (const handoff of HANDED_OFF_DESKTOP_DIRS) {
      expect(existsSync(join(REPO, handoff.dir)), `${handoff.dir} 不在了`).toBe(true);
      const resolved = resolveAnchor(`handoff/${handoff.dir}`, handoff.gate);
      expect(resolved.exists, `${handoff.dir} 的接手门 ${handoff.gate.file} 不存在 —— 交接给一个不存在的门 = 没交接`).toBe(true);
      expect(
        resolved.hit,
        `${handoff.dir} 的接手门里找不到 «${handoff.gate.mustContain}» —— 那一组断言被摘掉了`,
      ).toBeGreaterThan(0);
    }
  });

  it('每一屏的**渲染一跳**都被显式分类过（内容块 / 通用原语，恰等）', () => {
    // 动作面吃「自有目录 ∪ 渲染一跳」的全部文件；数据块面只吃自有目录 + `extraBlockSources`。
    // 两者的差额就是 `sharedPrimitives`。这条断言逼的是：新 import 一个会渲染的组件进来时，
    // 必须当场判一次「它是这一屏的内容，还是通用原语」——不判就红，不许静默落在块面之外。
    let totalHops = 0;
    for (const face of FACES) {
      const hops = [...face.oneHop.keys()].sort();
      totalHops += hops.length;
      const classified = [
        ...new Set([...face.register.extraBlockSources, ...face.register.sharedPrimitives]),
      ].sort();
      expect(
        classified,
        `${face.register.screen} 的渲染一跳没有被逐个分类。\n` +
          `只在代码里（新 import 了却没判）：${hops.filter((h) => !classified.includes(h)).join(' , ') || '（无）'}\n` +
          `只在表里（僵尸分类）：${classified.filter((h) => !hops.includes(h)).join(' , ') || '（无）'}\n` +
          `本屏一跳用到的组件：${[...face.oneHop].map(([f, n]) => `${f}[${[...n].join(',')}]`).join(' ; ')}`,
      ).toEqual(hops);
      for (const file of face.register.extraBlockSources) {
        expect(existsSync(join(REPO, file)), `${face.register.screen}：extraBlockSources 里的 ${file} 不存在`).toBe(true);
      }
    }
    expect(totalHops, '四屏一跳一个都没抓到 ⇒ 上面那条恒绿').toBeGreaterThan(5);
  });

  it('「这是通用原语，不是这一屏的内容」必须**挣来**：它自己不渲染任何屏级文案', () => {
    // 两处豁免共用这一条判据：
    //  · `primitiveModules` —— 自有目录里的原语库，它声明的组件不进数据块面；
    //  · `sharedPrimitives` —— 渲染一跳里的通用件，同上（**动作照进动作面**，见头注）。
    // 判据是可被推翻的事实：**原语渲染的是传进来的内容，自己不持有屏级文案**。
    // 只放行 `common.*`（确定 / 取消 / 上一页 / 加载中这类通用词）；出现任何一个带屏级命名空间的
    // 键（`rules.*` / `logs.*` / `settings.*` …），它就是内容，必须进 `extraBlockSources` 或自有目录。
    // 🔴 这条判据不是摆设：`components/RuleSubjectMenuItems.tsx`（新建规则 / 合并进已有规则）
    // 正是靠它从「通用原语」里被赶出来的 —— 2026-09-06 复审 major 记的那条活缺口就住在那儿。
    const screenKeysOf = (file: string): string[] => {
      const body = strip(readFileSync(join(REPO, file), 'utf8'));
      return [...new Set([...body.matchAll(/\bt\(\s*'([^']+)'/g)].map((m) => m[1]!))]
        .filter((key) => !key.startsWith('common.'))
        .sort();
    };
    const listed = REGISTERS.flatMap((r) =>
      [...r.primitiveModules, ...r.sharedPrimitives].map((f) => [r, f] as const),
    );
    expect(listed.length, '一条原语豁免都没有 ⇒ 本条恒真').toBeGreaterThan(8);
    for (const [register, file] of listed) {
      expect(existsSync(join(REPO, file)), `${register.screen}：原语豁免里的 ${file} 不存在`).toBe(true);
      const keys = screenKeysOf(file);
      expect(
        keys,
        `${register.screen}：${file} 自己渲染了屏级文案（${keys.join(' , ')}）—— 它不是原语，是内容。\n` +
          '把它放进 `extraBlockSources`（或搬进这一屏的目录），让它声明的块进数据块面逐个判。',
      ).toEqual([]);
    }
    for (const [register, file] of REGISTERS.flatMap((r) => r.primitiveModules.map((f) => [r, f] as const))) {
      expect(
        register.desktopDirs.some((d) => file.startsWith(`${d}/`)),
        `${register.screen}：${file} 不在这一屏的 desktopDirs 里 —— 那不是「本屏的原语库」`,
      ).toBe(true);
    }
    // 反向对照：一份**真的是内容**的文件必须被这条判据判成不合格（证明它不是恒真）。
    expect(
      screenKeysOf('ui/src/components/hover-cards/RuleHoverCard.tsx').length,
      'RuleHoverCard 居然一条屏级文案都不渲染 —— 那这条反向对照选错了样本，判据也就没被证明过',
    ).toBeGreaterThan(5);
  });

  it('动作面取材文件的基名唯一（否则两份文件的动作会被合并进同一串序号）', () => {
    for (const face of FACES) {
      const names = face.actionSources.map((f) => f.abs.slice(f.abs.lastIndexOf('/') + 1));
      const dup = names.filter((n, i) => names.indexOf(n) !== i);
      expect(
        [...new Set(dup)],
        `${face.register.screen}：取材面里有同名文件 —— \`id\` 用基名做前缀，重名会让两份文件的动作` +
          '静默合并（第二份的第一颗变成第一份的 `#2`）',
      ).toEqual([]);
    }
  });

  it('三个桌面面都有量级（任一塌成空集会让下面的等式恒绿）', () => {
    const actions = FACES.reduce((n, f) => n + f.actions.length, 0);
    const blocks = FACES.reduce((n, f) => n + f.blocks.length, 0);
    const slots = FACES.reduce((n, f) => n + f.slots.length, 0);
    // 下限贴着今天的真值（动作 208 / 块 49 / 槽 123）留一成余量：任一面**部分**塌掉
    // （比如某个 `desktopDirs` 掉了一项、或某个提取器不再认某种形态）也要红，不是只在塌成空集时才红。
    expect(actions, `四屏动作面一共只抓到 ${actions} 条`).toBeGreaterThan(180);
    expect(blocks, `四屏数据块面一共只抓到 ${blocks} 个`).toBeGreaterThan(40);
    expect(slots, `四屏数据槽面一共只抓到 ${slots} 个`).toBeGreaterThan(110);
    expect(FEED_FACE.size, `移动端 store 读点只抓到 ${FEED_FACE.size} 条`).toBeGreaterThan(45);
    expect(MOBILE_SOURCES.size, '移动端生产文件数掉了一大截').toBeGreaterThan(30);
  });

  it('分析器自身的行为对照：四个提取器各喂一份合成源码，正反都判对', () => {
    // A 面：身份优先级四档 + 泛型不误吞。
    const withId = actionElementsOf(strip('<div><button id="x" onClick={go}>hi</button></div>'))[0]!;
    expect(actionId(withId)).toBe('#x');
    const withKey = actionElementsOf(strip("<button className=\"a\" onClick={go}>{t('ns.k')}</button>"))[0]!;
    expect(actionId(withKey)).toBe('k:ns.k');
    const withClass = actionElementsOf(strip('<button className="only-class" onClick={go}/>'))[0]!;
    expect(actionId(withClass)).toBe('c:only-class');
    const withHandler = actionElementsOf(strip('<button onClick={() => void doThing(a)} />'))[0]!;
    expect(actionId(withHandler)).toBe('f:doThing');
    // 注释里的按钮不算（判据被自己污染的那一档）。
    expect(actionElementsOf(strip('/* <button id="ghost"> */ <button id="real"/>')).length).toBe(1);
    // A 面第二档：非 `<button>` 的动作元素（复审 major：桌面「按延迟排序」是 role=switch 的 span）。
    const roleSwitch = actionElementsOf(strip('<span className="swt" role="switch" onClick={toggleLatencySort}/>'));
    expect(roleSwitch.length, 'role="switch" 的控件必须进面').toBe(1);
    expect(actionId(roleSwitch[0]!)).toBe('c:swt');
    expect(
      actionElementsOf(strip('<div onClick={openSheet}><span>x</span></div>')).length,
      '带 onClick 的任意元素都算动作',
    ).toBe(1);
    // 而**不带**动作属性的普通元素不进面（否则整棵 DOM 都是动作面，等式恒不成立）。
    expect(actionElementsOf(strip('<div className="card"><span>{v}</span></div>')).length).toBe(0);
    // B 面：块面认「本文件里声明的组件」，不只认文件名（复审 major：内联块整条不可见）。
    expect([...declaredComponents('function UnlockBadge(props) {}\nconst RuleMetaCounts = () => null;')].sort()).toEqual([
      'RuleMetaCounts',
      'UnlockBadge',
    ]);
    expect([...renderedTags(strip('<UnlockBadge x={1}/> useRef<HTMLDivElement>(null)'))]).toEqual(['UnlockBadge']);
    // `config-fields` 面：一次调用写多格 + 计算键 + 展开一层，三种都认。
    expect([...updatedConfigFieldsIn('void update({ a: 1, b, [key]: c, nested: { deep: 2 } });')].sort()).toEqual([
      '[key]',
      'a',
      'b',
      'nested',
      'nested.deep',
    ]);
    // 展开与旧口径的差：旧正则只取第一个字面量键（`a`），后面三格全掉在面外。
    expect([...updatedConfigFieldsIn('void update({ ...prev, dnsDefaults: d, unmatchedAction: u });')].sort()).toEqual([
      'dnsDefaults',
      'unmatchedAction',
    ]);
    // D 面：三种形态各认一条，且不认非 store 的 hook。
    const probe = storeReads(
      new Map([
        ['p.ts', 'useAppStore((s) => s.alpha); useAppStore.getState().beta; useEffectiveConfig(); useMemo(() => x.gamma);'],
      ]),
    );
    expect([...probe.keys()].sort()).toEqual([
      'useAppStore.alpha',
      'useAppStore.beta',
      'useEffectiveConfig.*',
    ]);
    // 一次选择器里读两格 ⇒ 两条（第一版只取第一格，`s.a && s.b` 那种会漏掉一半）。
    const two = storeReads(new Map([['p.ts', 'useAppStore((s) => (s.one ? s.two : null));']]));
    expect([...two.keys()].sort()).toEqual(['useAppStore.one', 'useAppStore.two']);

    /* D 面第四档：**模块内 store 别名**（`resolveStoreAliases`）。
       `mobile/app-wiring.ts` 顶上就有 `const store = () => useAppStore.getState();`，那个文件里
       十余条订阅的主流写法是 `store().<写腿>(…)` —— 不解别名，经它的读点在这张面上隐形，
       同一条订阅换个写法就会从面上消失（功能与真机行为一字不变），而登记表对应那行随之
       变成「只在表里」。 */
    const aliased = storeReads(
      new Map([['p.ts', 'const store = () => useAppStore.getState();\nstore().setAlpha(1); store().beta;']]),
    );
    expect(
      [...aliased.keys()].sort(),
      '模块内 store 别名没有被还原 —— 经它的读点会整批掉在面外',
    ).toEqual(['useAppStore.beta', 'useAppStore.setAlpha']);

    /* 反向对照之一：别名到**一格**（不是到 store）不许被当成 store 别名 ——
       认了它，`cfg()` 的每一次调用都会被算成对 `config` 的一次读。 */
    const fieldAlias = storeReads(
      new Map([['p.ts', 'const cfg = () => useAppStore.getState().config;\ncfg(); cfg();']]),
    );
    expect([...fieldAlias.keys()].sort(), '别名到一格被误认成别名到 store').toEqual([
      'useAppStore.config',
    ]);

    /* 反向对照之二：别名**只在本文件内**生效。同名标识符出现在另一个没有那条声明的文件里时，
       不许被还原（文件粒度是刻意的：跨文件解析需要真 parser）。 */
    const otherFile = storeReads(new Map([['q.ts', 'store().setAlpha(1);']]));
    expect([...otherFile.keys()], '别名泄漏到了没有声明它的文件').toEqual([]);
  });

  it('反向对照：合成一颗桌面没有的按钮 / 一格没人读的 store，提取器必须报得出来', () => {
    expect(actionElementsOf(strip('<button id="synthetic-never-shipped" />')).length).toBe(1);
    expect([...storeReads(new Map([['p.ts', 'useAppStore((s) => s.__synthetic__);']])).keys()]).toEqual([
      'useAppStore.__synthetic__',
    ]);
    // 而真实取材面里不许出现这两个合成物（证明上面那两条不是在读真面）。
    expect(FACES.flatMap((f) => f.actions)).not.toContain('HomeScreen.tsx|#synthetic-never-shipped');
    expect([...FEED_FACE.keys()]).not.toContain('useAppStore.__synthetic__');
  });
});

describe('① 动作面：桌面每一颗都判过，且判词与移动端事实不打架', () => {
  it.each(REGISTERS.map((r) => [r.screen] as const))('%s：桌面动作面 ≡ 登记表（多一条少一条都红）', (screen) => {
    const face = FACES.find((f) => f.register.screen === screen)!;
    const registered = face.register.actions.map((e) => e.id);
    // 两个面各自判，判定方向不同（`controls` 逐颗登记，`config-fields` 的 `ported` 由门自己算）。
    if (face.register.actionFaces.includes('controls')) {
      const registeredControls = registered.filter((id) => !id.startsWith('field:'));
      expect(
        registeredControls.slice().sort(),
        `桌面 ${screen} 屏的动作面与登记表对不上。\n` +
          '桌面多一颗而这里没登记 ⇒ 那颗就是「悄无声息地没了」的形态；\n' +
          '这里多一条而桌面没有 ⇒ 登记表变僵尸了。逐条判：\n' +
          `只在桌面：${face.controls.filter((id) => !registeredControls.includes(id)).join(' , ') || '（无）'}\n` +
          `只在表里：${registeredControls.filter((id) => !face.controls.includes(id)).join(' , ') || '（无）'}`,
      ).toEqual(face.controls.slice().sort());
    }
    if (face.register.actionFaces.includes('config-fields')) {
      // `config-fields` 面：`ported` 由门自己算（移动端写了同一个字段），表里只登记缺口。
      const registeredFields = registered.filter((id) => id.startsWith('field:'));
      const ported = face.configFields.filter((id) => MOBILE_WRITTEN_FIELDS.has(id.slice('field:'.length)));
      const union = [...new Set([...ported, ...registeredFields])].sort();
      expect(
        union,
        `桌面 ${screen} 屏写的配置字段，既没在移动端写、也没登记的有：\n` +
          face.configFields.filter((id) => !ported.includes(id) && !registeredFields.includes(id)).join('\n'),
      ).toEqual(face.configFields.slice().sort());
      expect(
        registeredFields.filter((id) => ported.includes(id)),
        '这些字段登记着「缺」，移动端却真的在写 —— 僵尸登记',
      ).toEqual([]);
      expect(ported.length, '一个字段都没算成已接 ⇒ 「已接」这一侧的信号塌了').toBeGreaterThan(10);
    }
    // 没有第三种面：每条登记都必须落在这一屏声明过的某个面上（`field:` 前缀 ↔ `config-fields`）。
    const stray = registered.filter((id) =>
      id.startsWith('field:')
        ? !face.register.actionFaces.includes('config-fields')
        : !face.register.actionFaces.includes('controls'),
    );
    expect(stray, `${screen}：这些登记不属于本屏声明的任何一个动作面`).toEqual([]);
  });

  it('每一屏都至少有一条豁免，且四张表加起来的**条目总量**没有塌', () => {
    /*
     * 🔴 **2026-09-13 换了下限的对象**：原来钉的是「债务条数 > 10」，理由写的是
     * 「一张全 ported 的表说明不了任何事」。那个数把两件互相矛盾的事挤在一个阈值上 ——
     * 「判据塌了」（登记表丢了对象、提取器坏了）与「活真干完了」（四个批次真把债销到个位数）
     * 在债务计数上**同向**，于是它只能靠运气分辨。今天它挡的正是后者：债务降到 9，门红了，
     * 而红的原因是进展。
     *
     * 会随判据塌陷而缩、且**不随销账而缩**的量是**条目总量**（豁免 + 债务）：一条能力被接上
     * 只是把它的处置码从 `absent` 翻成 `ported`，条目仍在表上；真正让总量掉下去的是
     * 「登记表被清空 / 桌面取材面读不到 / 这个文件整个没被加载」。故下限改钉它。
     *
     * 取 200：今日实况 230（首页 32 / 连接 33 / 规则 63 / 设置 102）。留 30 条余量给正常增删，
     * **不给「少了一整张表」留余量** —— 最小的那张 32 条，掉任何一张都会跌破 200。
     *
     * 「活真干完了」那一档由别处答，不由这里：`wiring-completeness.test.ts` 的
     * `COMPLETION_GATE_MODE` 清零那天从 `known-red` 改成 `hard`。一个门答一个问题。
     */
    const all = REGISTERS.flatMap((r) => [...r.actions, ...r.blocks]);
    expect(
      all.length,
      '四张对差表的条目总量塌了 —— 那不是接完了（接完只会让处置码从 absent 翻成 ported，' +
        '条目还在），是某张表没被加载 / 被清空 / 桌面取材面读不到',
    ).toBeGreaterThan(200);
    for (const r of REGISTERS) {
      const entries = [...r.actions, ...r.blocks];
      expect(
        entries.some((e) => !isDebt(e.disposition)),
        `${r.screen}：一条豁免都没有`,
      ).toBe(true);
    }
  });
});

describe('② 数据块面：桌面这一屏自己的每一块都判过（W-27 住在这一面上）', () => {
  it.each(REGISTERS.map((r) => [r.screen] as const))('%s：桌面数据块 ≡ 登记表', (screen) => {
    const face = FACES.find((f) => f.register.screen === screen)!;
    const registered = face.register.blocks.map((e) => e.id).sort();
    expect(
      registered,
      `桌面 ${screen} 屏的数据块与登记表对不上。\n` +
        `只在桌面：${face.blocks.filter((b) => !registered.includes(b)).join(' , ') || '（无）'}\n` +
        `只在表里：${registered.filter((b) => !face.blocks.includes(b)).join(' , ') || '（无）'}`,
    ).toEqual(face.blocks.slice().sort());
  });

  it('四屏的块面都非空，且**空集报得出来**（任一面塌成空集会让上一条恰等恒绿）', () => {
    for (const face of FACES) {
      expect(
        face.blocks.length,
        `${face.register.screen} 的数据块面是空的 —— 先确认那是事实，不是提取器塌了`,
      ).toBeGreaterThan(0);
    }
    // 连接屏是这一面上最薄的一屏：两个桌面目录各自一个 `*Screen.tsx` 画完，没有把任何呈现块
    // 拆成组件，唯一那一块是渲染一跳带进来的 `RuleSubjectMenuItems`。它一度**真的是空集**，
    // 所以「空集」这个取值必须报得出来，不能是提取器永远给不出的东西 —— 合成一个只有纯逻辑
    // 文件的目录喂进去，必须得到 `[]`。
    expect(ownBlocks([join(UI_SRC, 'contracts')]), '块面提取器对一个没有组件的目录居然不返回空集').toEqual([]);
    expect(FACES.find((f) => f.register.screen === 'connections')!.blocks).toEqual(['RuleSubjectMenuItems']);
  });

  it('W-27 回放的落点在场：反向分流指示器登记着 ported，且它的锚指向移动端首页的真渲染', () => {
    const badge = HOME_PARITY.blocks.find((b) => b.id === 'ReverseRoutingBadge');
    expect(badge, 'ReverseRoutingBadge 不在首页登记表里 —— 那正是 W-27 漏掉时的状态').toBeDefined();
    expect(badge!.disposition.kind).toBe('ported');
    const anchor = exemptionAnchor(badge!.disposition)!;
    expect(anchor.file).toBe('ui/src/mobile/home/HomeScreenView.tsx');
    const resolved = resolveAnchor('W-27', anchor);
    expect(resolved.exists).toBe(true);
    expect(resolved.hit, '移动端首页不再渲染反向分流指示器 —— 这正是 2026-07-20 那次真机事故的形态').toBeGreaterThan(0);
  });
});

describe('③ 具名数据槽：桌面这一屏的槽集合没有在没人看的情况下变过', () => {
  it.each(REGISTERS.map((r) => [r.screen] as const))('%s：桌面 id= 全集 ≡ slotBaseline', (screen) => {
    const face = FACES.find((f) => f.register.screen === screen)!;
    expect(
      face.slots,
      `桌面 ${screen} 屏的具名数据槽变了。这一面是**变更探测器**：它不知道移动端画没画，\n` +
        '只负责逼一次「移动端要不要有这一格」的显式决定。判完把基线推到新集合。\n' +
        `新增：${face.slots.filter((s) => !face.register.slotBaseline.includes(s)).join(' , ') || '（无）'}\n` +
        `消失：${face.register.slotBaseline.filter((s) => !face.slots.includes(s)).join(' , ') || '（无）'}`,
    ).toEqual(face.register.slotBaseline.slice().sort());
  });
});

describe('④ 锚：每一条豁免都被真的打开核对（不是形状检查）', () => {
  it('自检：确实有一批锚要核对，且它们分布在多个文件上', () => {
    expect(ALL_ANCHORS.length, '一条锚都没有 ⇒ 下面那条恒真').toBeGreaterThan(190);
    expect(new Set(ALL_ANCHORS.map((a) => a.anchor.file)).size).toBeGreaterThan(15);
  });

  it('每一条锚指向的文件都存在，且真的含那段文本', () => {
    const missingFile = ALL_ANCHORS.filter((a) => !a.exists).map((a) => `${a.owner} → ${a.anchor.file}`);
    expect(missingFile, `这些锚指向的文件不存在：\n${missingFile.join('\n')}`).toEqual([]);
    const missingText = ALL_ANCHORS.filter((a) => a.exists && a.hit === -1).map(
      (a) => `${a.owner} → ${a.anchor.file} «${a.anchor.mustContain}»`,
    );
    expect(
      missingText,
      '这些锚指向的**依据本身已经不在了**。豁免随之作废：要么改锚（事实挪了位置），\n' +
        `要么把那条登记退回债务（事实没了）：\n${missingText.join('\n')}`,
    ).toEqual([]);
  });

  it('反向对照：一条指向不存在文本的锚必须报得出来（证明上一条不是恒真）', () => {
    const fake = resolveAnchor('synthetic', {
      file: 'ui/src/mobile/parity-registry.test-support.ts',
      mustContain: '__这段文本不可能存在__',
    });
    expect(fake.exists).toBe(true);
    expect(fake.hit).toBe(-1);
    const gone = resolveAnchor('synthetic', { file: 'ui/src/__no_such_file__.ts', mustContain: 'x' });
    expect(gone.exists).toBe(false);
  });

  it('反向对照：**只出现在注释里**的文本喂不饱锚（墓碑注释那一档）', () => {
    // 2026-09-06 复审 major 的实测形态：把一整块 JSX 删掉、只留一行「此处曾有 X」的注释，
    // 上一版的裸 `indexOf` 照样命中 ⇒ 那条豁免连同它的回放断言一起变成假绿。
    //
    // 🔴 探针文本**必须拼出来**，不许写成一整个字面量：字符串是被保留的，
    // 一整个字面量会让这条断言拿本行自己当命中点（判据被自己污染的教科书形态）。
    const commentOnly = ['__墓碑注释', '反向对照探针__'].join('');
    const self = 'ui/src/mobile/screen-parity.test.ts';
    const raw = readFileSync(join(REPO, self), 'utf8');
    expect(raw.split(commentOnly).length - 1, '探针在本文件里必须恰好出现一次（就是头注里那一处）').toBe(1);
    expect(resolveAnchor('synthetic', { file: self, mustContain: commentOnly }).hit, '一段只在注释里出现过的文本被判成了命中 —— 剥注释那一步没生效').toBe(-1);
    // 正面对照：同一份文件里一段**代码**上的文本必须命中（证明上一条不是「什么都找不到」）。
    expect(resolveAnchor('synthetic', { file: self, mustContain: 'const PLATFORM_TOKEN =' }).hit).toBeGreaterThan(0);
  });

  it('`ported` / `reachable-elsewhere` 的锚必须落在 `ui/src/mobile/` 下（不许拿桌面源码当移植证据）', () => {
    // 字段名叫 `mobile`，上一版却不校验它指哪儿。复审 minor 的实测：把首页连接键那条的锚换成桌面
    // 自己的 `id="connect-btn"`，门 33/33 全绿 —— 而移植时从桌面文件里复制一段字面量是最省事的写法。
    // （喂数腿表不受这条约束：那张表按读点取材，写入方合法地可以住在共享模块里。）
    const claims = ALL_ENTRIES.filter(
      ([, e]) => e.disposition.kind === 'ported' || e.disposition.kind === 'reachable-elsewhere',
    );
    expect(claims.length, '一条 ported / reachable-elsewhere 都没有 ⇒ 本条恒真').toBeGreaterThan(20);
    const offside = claims
      .map(([owner, e]) => [owner, exemptionAnchor(e.disposition)!] as const)
      .filter(([, a]) => !a.file.startsWith('ui/src/mobile/'))
      .map(([owner, a]) => `${owner} → ${a.file}`);
    expect(
      offside,
      '这些登记声称「移动端有」，锚却指在移动树之外 —— 那不是移植的证据：\n' + offside.join('\n'),
    ).toEqual([]);
  });

  it('`platform-absent` 的锚必须指向一条**平台判定**，不是一句写过的话', () => {
    // 复审 minor 的实测形态：三条锚全指着 `lib.rs` 里的中文注释 —— 给 Android 落一个 autostart 实现、
    // 把 `#[cfg(desktop)]` 删掉而注释原样留着，豁免继续成立。剥注释已经堵住「指注释」那一半，
    // 这一条堵另一半：命中处必须真的是一条平台判定（或那个文件本身就是平台专属的）。
    const platformAbsent = [
      ...ALL_ENTRIES.filter(([, e]) => e.disposition.kind === 'platform-absent'),
      ...MOBILE_FEEDS.filter((e) => e.disposition.kind === 'platform-absent').map(
        (e) => [`feed/${e.id}`, e] as const,
      ),
    ];
    expect(platformAbsent.length, '一条 platform-absent 都没有 ⇒ 本条恒真').toBeGreaterThan(5);
    for (const [owner, entry] of platformAbsent) {
      const anchor = exemptionAnchor(entry.disposition)!;
      const resolved = resolveAnchor(owner, anchor);
      expect(resolved.hit, `${owner}：锚 ${anchor.file} 已在上一组报过，这里只做平台判定`).toBeGreaterThan(0);
      expect(
        PLATFORM_TOKEN.test(anchor.mustContain) || PLATFORM_PATH.test(anchor.file),
        `${owner}：${anchor.file}:${resolved.hit} 这条锚的文本里没有平台判定` +
          '（`#[cfg(` / `cfg!(` / `target_os` / `platform != "…"` / `VpnService`，也不在平台专属源码树里）——\n' +
          '「平台上没有这个对象」这句话得**把那条守卫引下来**，不能指着守卫附近的一行代码说。',
      ).toBe(true);
    }
  });

  it('反向对照：一条指着与平台无关的代码的锚，必须被上一条判成不合格', () => {
    const benign: Anchor = {
      file: 'ui/src/contracts/test-only-modules.ts',
      mustContain: 'export const IS_TEST_ONLY_MODULE =',
    };
    expect(resolveAnchor('synthetic', benign).hit, '这段样本得真的在（否则下面那条恒真）').toBeGreaterThan(0);
    expect(
      PLATFORM_TOKEN.test(benign.mustContain) || PLATFORM_PATH.test(benign.file),
      '一段与平台无关的代码被判成了平台判定 —— 那条断言恒绿',
    ).toBe(false);
    // 第二条反向对照，针对上一版真栽过的那个形态：**指在一堆 cfg 守卫中间、但自己不是守卫**的一行。
    const nearGuards: Anchor = {
      file: 'src-tauri/src/app_tray.rs',
      mustContain: 'use tauri::Manager;',
    };
    expect(resolveAnchor('synthetic', nearGuards).hit, '这段样本得真的在').toBeGreaterThan(0);
    expect(
      PLATFORM_TOKEN.test(nearGuards.mustContain) || PLATFORM_PATH.test(nearGuards.file),
      '一行只是「住在 cfg 守卫附近」的代码被判成了平台判定 —— 那正是邻域判据的洞',
    ).toBe(false);
  });
});

describe('⑤ 喂数腿：移动端读的每一格 store 状态都有写入方（W-23 / W-24 住在这一面上）', () => {
  const registered = MOBILE_FEEDS.map((e) => e.id);

  it('移动端读点面 ≡ 喂数腿登记表', () => {
    const face = [...FEED_FACE.keys()].sort();
    expect(
      registered.slice().sort(),
      '移动端 store 读点与喂数腿登记表对不上。\n' +
        '读点在、表里没有 ⇒ 有一格状态没人问过「谁写它」（W-23 / W-24 就是这么漏的）；\n' +
        '表里有、读点没有 ⇒ 登记表变僵尸了。\n' +
        `只在代码里：${face.filter((id) => !registered.includes(id)).join(' , ') || '（无）'}\n` +
        `只在表里：${registered.filter((id) => !face.includes(id)).join(' , ') || '（无）'}`,
    ).toEqual(face);
  });

  it('每一格 `state` 的写入方都在（锚已在 ④ 组被打开核对，这里钉的是「它必须有一条」）', () => {
    const unfed = MOBILE_FEEDS.filter(
      (e) => e.role === 'state' && exemptionAnchor(e.disposition) === null && !isDebt(e.disposition),
    );
    expect(unfed.map((e) => e.id), '这些状态既没有写入方、也没被判成债务').toEqual([]);
    const stateCount = MOBILE_FEEDS.filter((e) => e.role === 'state').length;
    expect(stateCount, '一格 state 都没有 ⇒ 上面那条恒真').toBeGreaterThan(15);
  });

  it('每一格 `action` 读的确实是那个 store 声明出来的东西', () => {
    const actions = MOBILE_FEEDS.filter((e) => e.role === 'action');
    expect(actions.length, '一条 action 都没有 ⇒ 下面那条恒真').toBeGreaterThan(10);
    for (const entry of actions) {
      const anchor = exemptionAnchor(entry.disposition);
      expect(anchor, `${entry.id} 没给 store 声明锚`).not.toBeNull();
      const store = readFileSync(join(REPO, anchor!.file), 'utf8');
      const name = entry.id.split('.')[1]!;
      expect(
        new RegExp(`\\b${name}\\b`).test(store),
        `${entry.id}：${anchor!.file} 里找不到 \`${name}\` —— 读的是一个 store 已经不提供的东西`,
      ).toBe(true);
    }
  });

  it('W-23 / W-24 的写入方逐条在场（这两条腿被摘掉时，上面那条对差会把它们报成没有写入方）', () => {
    const byId = new Map(MOBILE_FEEDS.map((e) => [e.id, e] as const));
    for (const id of ['useAppStore.tailscaleStatuses', 'useAppStore.tailscaleLoginStates']) {
      const anchor = exemptionAnchor(byId.get(id)!.disposition)!;
      expect(resolveAnchor(id, anchor).hit, `W-23：${id} 的写入方不在了`).toBeGreaterThan(0);
    }
    const subProgress = exemptionAnchor(byId.get('useSubscriptionProgressStore.progress')!.disposition)!;
    expect(resolveAnchor('W-24', subProgress).hit, 'W-24：订阅进度的写入方不在了').toBeGreaterThan(0);
  });

  /*
   * 🔴 `invalidNodes` —— 本表落地当天抓到的那条活的，2026-09-13（批 18）接上写入方。
   *
   * 上一版这条 it 断言的是 `kind === 'absent'`（把「今天还欠着」钉住）。现在它反过来钉「已经接上」，
   * 三句缺一不可：
   *  ① 读点仍在节点屏上 —— 否则这是一条僵尸登记，账销给了一个没人读的格子；
   *  ② 写入方（那条订阅）仍在 —— 由锚打开 `app-wiring.ts` 核对；
   *  ③ **写入方自己也在读点面上**。第 ③ 句是反向对照的着力点：`setInvalidNodes` 走的是
   *    `useAppStore.getState().…` 这种本表认得出的形态，故把那条订阅从 `app-wiring.ts` 里
   *    摘掉时，上面那条「移动端读点面 ≡ 喂数腿登记表」会当场红（表里多出一条没有读点的僵尸）。
   *    若写调用改走本地 `store()` 速记，三句里只有 ② 会红，而 ② 是可以被一行墓碑注释之外的
   *    任意改写绕过的；③ 让「摘掉能力」这件事在**面**上留下缺口，不只在锚上。
   */
  it('🔴 `invalidNodes` 的写入方在场，且它自己也落在读点面上（反向对照的着力点）', () => {
    const entry = MOBILE_FEEDS.find((e) => e.id === 'useAppStore.invalidNodes')!;
    expect(entry.disposition.kind).toBe('ported');
    expect(FEED_FACE.get('useAppStore.invalidNodes')).toBeDefined();
    // ① 那个读点真的在节点屏上（不是一条早就删干净了的僵尸登记）。
    expect([...FEED_FACE.get('useAppStore.invalidNodes')!]).toContain('mobile/nodes/MobileNodesScreen.tsx');
    // ② 写入方在场。
    const anchor = exemptionAnchor(entry.disposition)!;
    expect(
      resolveAnchor('useAppStore.invalidNodes', anchor).hit,
      '无效节点的写入方不在了 ⇒「这个节点已失效」那一格退回恒不出现',
    ).toBeGreaterThan(0);
    // ③ 写入方自己也在读点面上。
    expect(
      FEED_FACE.get('useAppStore.setInvalidNodes'),
      '写调用没落在本表的取材面上（多半是改走了 `store()` 速记）—— 一条自己看不见的写入方等于没有写入方',
    ).toBeDefined();
  });
});

describe('⑥ 交接：被本门显式缩掉的那两格，接手的那道门与本门读的是同一份真值', () => {
  it('桌面九张设置子页 ≡ `MOBILE_SETTINGS_PAGES` ∪ `MOBILE_ABSENT_SETTINGS_PAGE`', () => {
    const settings = FACES.find((f) => f.register.screen === 'settings')!;
    const subPages = settings.blocks
      .filter((b) => /^Settings[A-Z]/.test(b))
      .map((b) => b.replace(/^Settings/, '').toLowerCase())
      .sort();
    expect(subPages.length, '桌面设置子页一个都没抓到 ⇒ 这条交接断言恒绿').toBe(9);
    const mobileSide = [...MOBILE_SETTINGS_PAGES, MOBILE_ABSENT_SETTINGS_PAGE].sort();
    expect(
      subPages,
      '桌面设置子页与移动端那份真值源对不上 —— 本门把「逐行设置」这一格交给 ' +
        '`MobileSettings.test.tsx`，交接口就是这两个常量。对不上时那一格谁都没在守。',
    ).toEqual(mobileSide);
  });

  it('**四屏**逐槽粒度的接手门逐条在场（交接给一个不存在的门 = 没交接）', () => {
    // 2026-09-06 复审 minor：射程自曝第 1 条把逐槽粒度点名交给四道门，上一版却只断言了
    // `MobileSettings.test.tsx` 一道 —— 另外三道被删、或那几组 describe 被摘掉，
    // 头注那句交接静默变成假的而门全绿。现在四屏各自把接手门写进 `slotHandoffGates`，走同一条锚核对。
    let total = 0;
    for (const register of REGISTERS) {
      expect(
        register.slotHandoffGates.length,
        `${register.screen}：一道接手门都没填 —— 射程自曝第 1 条对这一屏是空头支票`,
      ).toBeGreaterThan(0);
      for (const gate of register.slotHandoffGates) {
        total += 1;
        expect(
          IS_GATE_FILE.test(gate.file),
          `${register.screen}：接手门 ${gate.file} 不是一道门（只认 .test. / .spec.）`,
        ).toBe(true);
        const resolved = resolveAnchor(`${register.screen}/handoff`, gate);
        expect(resolved.exists, `${register.screen}：接手门 ${gate.file} 不见了 —— 缩掉的那一格当场无人接手`).toBe(true);
        expect(
          resolved.hit,
          `${register.screen}：${gate.file} 里找不到 «${gate.mustContain}» —— 那一组断言被摘掉了`,
        ).toBeGreaterThan(0);
      }
    }
    expect(total, '四屏加起来一条接手门都没有 ⇒ 本条恒真').toBeGreaterThan(4);
  });
});

describe('⑦ 判词纪律：理由不许是排期，缺席理由键必须真的在 locale 里', () => {
  it('每条登记的 `note` 都够长、且不是排期', () => {
    for (const [owner, entry] of ALL_ENTRIES) {
      expect(entry.note.length, `${owner} 的 note 太短，说不清射程`).toBeGreaterThan(15);
      expect(entry.note, `${owner} 的 note 写的是排期，不是判定`).not.toMatch(EXCUSE);
    }
    for (const entry of MOBILE_FEEDS) {
      expect(entry.note.length, `feed/${entry.id} 的 note 太短`).toBeGreaterThan(5);
      expect(entry.note, `feed/${entry.id} 的 note 写的是排期`).not.toMatch(EXCUSE);
    }
  });

  it('每个 `reasonKey` 都在 zh-CN 里存在，且**在移动端生产源码里真有渲染点**', () => {
    // 取材面里已经没有登记表了（`IS_TEST` = 共享的 test-only 谓词，`*.test-support.ts` 整批在外），
    // 但这里再显式排一次并**自检排完非空** —— 两道防线，因为这条断言一旦被登记表自己喂饱就恒绿。
    const consumers = [...MOBILE_SOURCES].filter(
      ([file]) => !/absence-register|feed-register|parity-registry/.test(file),
    );
    // 切点自检：排除动作**有对象可排**（节点屏那张表是产品命名的，确实在取材面里），
    // 且排完之后还剩得下真取材面（不能排成空集）。少任何一半，这条断言都可能恒绿。
    const dropped = [...MOBILE_SOURCES.keys()].filter((f) => !consumers.some(([c]) => c === f));
    expect(dropped, '一份登记表都没排掉 —— 那条排除规则失效了，⑦ 会被本表自己的锚文本喂饱').not.toEqual([]);
    expect(consumers.length, '排掉登记表之后消费文件面空了 —— 切点把真取材面一起排掉了').toBeGreaterThan(30);
    /** 一条登记的理由键（只有债务那两档带，且 `absent` 那档还是可选的）。 */
    const reasonKeyOf = (d: ParityDisposition): string | undefined =>
      d.kind === 'absent' || d.kind === 'disabled' ? d.reasonKey : undefined;
    /*
     * 🔴 **谓词自检代替「至少要有 N 条」的下限**（2026-09-13 / 批 17 改，与下一条用例同日那次
     * 逐字同构，理由也是同一条）。
     *
     * 原来这里写的是 `expect(withReason.length).toBeGreaterThan(5)`，理由是「一条带理由键的登记都
     * 没有 ⇒ 下面那个 for 恒真」。那句话对，但它把**判据塌了**与**这一档真的在清空**压在同一个
     * 数上：批 17 把 DNS 默认策略那两条 `absent`（共用 `mobileRules.formUnavailable`）销掉之后
     * 这条下限当场红 —— 红的原因是进展，而它报的错（「一条带理由键的登记都没有」）会把人引向
     * 「判据坏了」，正是上一次同形教训点名要避免的那个误导。
     *
     * 一条判据有没有牙，可以不靠「现实里恰好还剩几个样本」来证：下面用**合成样本**把抽取器
     * 两个方向各证一次 —— 带理由的两档各取得出、不带理由的档一条都不许混进来。
     * 证完之后，真实集合哪怕是空的也无所谓：空集时这条用例证明的是「这把尺子是准的」。
     */
    expect(reasonKeyOf({ kind: 'absent', reasonKey: 'probe.absent' })).toBe('probe.absent');
    expect(
      reasonKeyOf({ kind: 'disabled', reasonKey: 'probe.disabled', mobile: { file: 'x', mustContain: 'y' } }),
    ).toBe('probe.disabled');
    // 不带理由的两种形态：`absent` 省略理由键、以及豁免族（它们交的是锚，不是理由）。
    expect(reasonKeyOf({ kind: 'absent' })).toBeUndefined();
    expect(reasonKeyOf({ kind: 'ported', mobile: { file: 'x', mustContain: 'y' } })).toBeUndefined();
    expect(
      reasonKeyOf({ kind: 'platform-absent', evidence: { file: 'x', mustContain: 'y' } }),
    ).toBeUndefined();

    const withReason = [...ALL_ENTRIES, ...MOBILE_FEEDS.map((e) => [`feed/${e.id}`, e] as const)]
      .map(([owner, entry]) => {
        const key = reasonKeyOf(entry.disposition);
        return key ? ([owner, key] as const) : null;
      })
      .filter((x): x is readonly [string, string] => x !== null);
    /*
     * 🔴 **谓词自检代替「至少要有 N 条」的下限**（2026-09-13 改，同下一条用例的先例）。
     *
     * 原来这里是 `toBeGreaterThan(5)`，理由是「一条带理由键的登记都没有 ⇒ 下面恒真」。
     * 那句话对，但它把「判据塌了」与「这一档真的接完了」压在同一个数上：批次一条条把带理由键的
     * `absent` 接通之后（批 15 一次销掉五条），这条开始红 —— 红的原因是进展，而它报的错会把人
     * 引向「登记表坏了」。
     *
     * 下面用**合成样本**两个方向各证一次这把尺子是准的：locale 里没有的键必须判假、
     * 生产源码里没人渲染的键必须判假。证完之后真实集合哪怕是空的也无所谓 ——
     * 那时这条用例证明的是「尺子准」，而不是「什么都没发生所以通过」。
     */
    expect(localeHas('mobileSettings.__never_shipped__'), '尺子坏了：locale 里不存在的键判成了存在').toBe(
      false,
    );
    expect(
      consumers.some(([, text]) => text.includes("'mobileSettings.__never_shipped__'")),
      '尺子坏了：生产源码里不存在的键判成了有渲染点',
    ).toBe(false);
    // 正面对照：一条真的在场的键两侧都判真（否则上面两条否定判定可能只是因为谓词恒假）。
    expect(localeHas('mobileSettings.update.appDesc')).toBe(true);
    expect(consumers.some(([, text]) => text.includes("'mobileSettings.update.appDesc'"))).toBe(true);

    for (const [owner, key] of withReason) {
      expect(localeHas(key), `${owner} 的理由键 ${key} 在 zh-CN 里不存在`).toBe(true);
      expect(
        consumers.some(([, text]) => text.includes(`'${key}'`)),
        `${owner} 的理由键 ${key} 在移动端生产源码里一个渲染点都没有 —— 登记了一句用户看不到的理由`,
      ).toBe(true);
    }
  });

  it('`disabled`（在场置灰）的理由必须渲染在**它自己那一屏**，不许借别屏的一句话', () => {
    // 这一条与上一条的差别是**分辨率**：上一条只问「全树有没有人画过这句」，
    // 而「在场置灰」的语义是「那颗控件就在眼前、灰着、旁边写着为什么」——
    // 理由渲染在另一屏等于那颗灰控件旁边什么都没有。`absent` 不受这条约束：
    // 它那条腿根本不在这一屏上，理由自然可能落在别处（首页「添加节点」借节点屏那句就是这个形态）。
    /** 某个理由键在某一屏的**自有**源码里有没有渲染点（登记表本身不算 —— 那是账本不是界面）。 */
    const renderedOnOwnScreen = (mobileDir: string, key: string): boolean =>
      [...MOBILE_SOURCES]
        .filter(
          ([file]) =>
            file.startsWith(relative(UI_SRC, join(REPO, mobileDir))) && !/absence-register/.test(file),
        )
        .some(([, text]) => text.includes(`'${key}'`));

    /*
     * 🔴 **谓词自检代替「至少要有一条」的下限**（2026-09-13 改）。
     *
     * 原来这里写的是 `expect(disabled.length).toBeGreaterThan(0)`，理由是「一条 disabled 都没有
     * ⇒ 本条恒真」。那句话对，但它把「判据塌了」与「这一档真的清空了」压在同一个数上：
     * 2026-09-13 四个批次把仅有的几条 `disabled` 全部接通或改判之后，这条断言开始红 ——
     * 红的原因是进展，而它报的错会把人引向「判据坏了」。
     *
     * 一条判据有没有牙，可以不靠「现实里恰好有个样本」来证。下面用**合成样本**两个方向各证一次：
     * 理由渲染在别屏 ⇒ 必须判假；渲染在本屏 ⇒ 必须判真。证完之后，真实集合哪怕是空的也无所谓 ——
     * 空集时这条用例证明的是「这把尺子是准的」，而不是「什么都没发生所以通过」。
     */
    /*
     * 探针键只需满足两件事：在设置屏有渲染点、在首页没有。
     *
     * 🔴 2026-09-13（批 17）换过一次：原来用的是 `mobileRules.formUnavailable`（DNS 页那条默认
     * 策略缺席说明）。那两格接通之后那句说明整条消失、键也随之从 locale 删掉 ⇒ 上面那条自检会
     * 因为「探针本身没了」而红，而它报的错会把人引向「判定失效」。换成同一页那句 FakeIP 风险
     * 提示：它是**常驻**文案（不是缺席理由），不会随任何一批接线而消失。
     */
    const PROBE_KEY = 'mobileSettings.dns.fakeIpOffRisk'; // DNS 页那句常驻的关 FakeIP 风险提示
    expect(
      renderedOnOwnScreen('ui/src/mobile/settings', PROBE_KEY),
      '自检失效：探针键在它真的被渲染的那一屏上都找不到 —— 下面的判定全部无意义',
    ).toBe(true);
    expect(
      renderedOnOwnScreen('ui/src/mobile/home', PROBE_KEY),
      '自检失效：探针键在一个**没有**渲染它的屏上也判成了有 —— 这把尺子量不出「借了别屏的一句话」',
    ).toBe(false);

    const disabled = REGISTERS.flatMap((r) =>
      [...r.actions, ...r.blocks]
        .filter((e) => e.disposition.kind === 'disabled')
        .map((e) => [r, e] as const),
    );
    for (const [register, entry] of disabled) {
      const key = entry.disposition.kind === 'disabled' ? entry.disposition.reasonKey : '';
      expect(
        renderedOnOwnScreen(register.mobileDir, key),
        `${register.screen}/${entry.id}：理由键 ${key} 在本屏源码里一个渲染点都没有 —— ` +
          '「在场置灰 + 理由」这句话在这一屏上是假的',
      ).toBe(true);
    }
  });

  it('`adjudicated-out` 的锚必须指向一道**正面钉着「它不出现」**的门，不是一句说明', () => {
    const adjudicated = ALL_ENTRIES.filter(([, e]) => e.disposition.kind === 'adjudicated-out');
    expect(adjudicated.length, '一条 adjudicated-out 都没有 ⇒ 本条恒真').toBeGreaterThan(0);
    for (const [owner, entry] of adjudicated) {
      const anchor = exemptionAnchor(entry.disposition)!;
      expect(
        IS_GATE_FILE.test(anchor.file),
        `${owner}：adjudicated-out 的锚指的是 ${anchor.file}，那不是一道门。` +
          '这个码要的是「有人在持续证明它不出现」，写不出这样一道门的就老实留在债务里。',
      ).toBe(true);
    }
  });
});
