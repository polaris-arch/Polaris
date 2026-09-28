/**
 * CSS 层叠解析器 —— 全仓测试读 CSS 的**静态**入口。
 *
 * # 🔴 先说射程：本模块之上有一个裁判
 *
 * 2026-09-05 走到第四轮验收：二轮 3 major → 三轮 2 blocker + 7 major → 四轮 4 blocker + 8 major。
 * 严重度不降，而每一轮红的都是**同一个根因的下一条尾巴** —— 我们在手写实现浏览器的层叠，
 * 而它的尾巴是无穷的（`:is()` / 属性选择器 / 自定义属性的**值**被覆写 / `@layer` /
 * 构建期注入的 Tailwind / at-rule 嵌进规则块 / 跨任意选择器的特异度……）。
 *
 * 所以从这一轮起，**「这个元素最终是什么样」这个问题不归本模块答**，归
 * [`css-oracle.test-support.ts`]：系统 Chrome + CDP，喂真组件 DOM 与产品那条 postcss 管线的产物，
 * 读 `getComputedStyle`。凡是「错了用户看得见」的判据都迁到那边。
 *
 * 本模块留下来干**它真正擅长且裁判答不了**的事：
 *  · **源码面的穷举**：谁声明过这个分量、声明在哪个文件哪一行、在哪个条件档里
 *    （[`declaringSites`] / [`Decl`]）—— 裁判只看得见结果，看不见「有几处在写它」。
 *  · **写法**判据：用的是令牌还是硬编码、用的是逻辑属性还是物理属性、断点块里有没有这一档。
 *  · **进包顺序**：[`cssOrder`] 从入口模块图推导，缺陷「外壳 CSS 反而后进包」只有它看得见。
 *
 * ⚠️ 读到本模块的绿时，先问一句「这条判据守的东西，错了用户看得见吗」。看得见 ⇒ 它该在裁判上。
 *
 * # 为什么当初要有这层（仍然成立的那部分）
 *
 * 2026-09-05 第二轮抓到 7 条 major，全部是「门本身太窄」：非全局 `match` 取首条规则
 * （浏览器是「同特异度后者胜」，正好相反）、按属性名精确配对（`border-top` 的断言被
 * `border-top-color` 绕过）、同根因的姊妹腿没扫。共同形状是**空取材面上的否定断言恒真**。
 * 本仓此前有**至少四套各写各的** CSS 读法，本模块把它们归一到一份。
 *
 * # 为什么是 `.test-support.ts` 而不是 `.test.ts`
 *
 * 从一个 `.test.ts` import 会把对方的 `describe` 一并收进本文件的套件（同一批用例跑两遍、报错报两处）
 * —— 仓内两处自陈过这件事（`src/mobile/dynamic-type.test.ts` 头注、`src/styles/cascade-dead-rules.test.ts`）。
 * `.test-support.` 是本仓已成立的类别：`contracts/test-only-modules.ts` 的 `IS_TEST_ONLY_MODULE` 认它、
 * i18n G1 豁免其中文注释、G0-b 禁生产代码 import 它、`file-naming` 接受该中缀。
 *
 * # 输入：两条腿，都必须显式选
 *
 *  · [`context(ctx)`] —— 四个层叠上下文各自从入口模块图按 **ESM 求值序后序遍历**推导进包顺序，
 *    不写死文件清单。顺序写成代码，「外壳 CSS 反而后进包」那类缺陷才有人看得见。
 *  · [`contextOf(sources)`] —— 吃内存源。反向对照/变异自检不必写盘：这棵树是多线共用的，
 *    写盘变异会互相吃掉快照。
 *
 * # 保证（每条都在 `css-cascade.test.ts` 里有对应的双向对照）
 *
 *  1. **注释与结构走 `postcss`**（`ui/package.json` 的既有 devDependency）：注释不进模型、
 *     选择器列表与 at-rule prelude 按真解析器切、`file:line` 取自 postcss 的 source map。
 *  2. 同选择器键内的胜负是**事实不是估计**：字面相同 ⇒ 特异度必然相同 ⇒ `!important` → 源序决胜无条件成立。
 *  3. 选择器键归一：组合符两侧空白（`.a+.b` ≡ `.a + .b`）、复合内简单选择器排序（`.a.b` ≡ `.b.a`）、
 *     `:before` ≡ `::before`、属性选择器引号（`[dir=rtl]` ≡ `[dir="rtl"]`）。
 *  4. `!important` 逐属性进模型（corpus 实测 23 处）。
 *  5. **故障关闭**（每条都实测过一次能溜过去的变异）：
 *     · CSS 解析失败 / 花括号不配平 ⇒ 抛；
 *     · 选择器一条都没命中 ⇒ 抛（空取材面上的否定断言恒真）；
 *     · 属性落在已建模族的**前缀**内却分不了类 ⇒ 抛；
 *     · 未建模构造 ⇒ 抛：`@layer` / `@scope` / **样式规则嵌进样式规则**（`.a{ .b{…} }`，
 *       判据是「非 at-rule 块里还有**样式规则**块」）/ `writing-mode`、`direction` 声明 /
 *       含 `[dir=` 的选择器而 dir 敏感查询未给 `env` / 未建模的简写形态；
 *     · 调用方**没显式给 `where`** 而同键上还有条件块里的声明 ⇒ 抛并点名 `file:line` + prelude
 *       （`@media screen` / `@supports (display:flex)` / `@media (min-width:0)` 在目标 WebView 里恒真，
 *       默认谓词把它们滤掉就是给出反的答案）。
 *  6. 带复位语义的简写把**缺席分量**写成 [`INITIAL`]（`border: none` 之后 `border-*-width` 是 `initial`）。
 *  7. **at-rule 嵌进规则块是建模的**（`.a{ color:x; @media screen{ color:y } }` → 两条声明，
 *     内层带条件标签）。⚠️ 上一版头注把它写进「⇒ 抛」那一列，与代码直接冲突 —— postcss 早就
 *     把它解析对了。判据见 `css-cascade.test.ts` ⑲。
 *
 * # 明确不保证 —— 每条后面写着**它归谁判**
 *
 * 🔴 一条**不受判据管辖**的「不保证」声明，比没有这句话更坏 —— 读的人会以为有人想过。
 * 上一版这一段里有三条正是如此（R5-01 / R5-03 / R5-05），下面逐条改成实话。
 *
 *  · **不判两个条件块能否同时成立**（`@media` 与 `@container` 的交集）。
 *    管辖：`where` 必须显式给（保证 5 最后一条），且 [`ALL`] 的语义是**并集不是交集**。
 *    要问「在某一档下最终是多少」⇒ 走裁判，给它一个视口 / 媒体特性，让浏览器自己判。
 *  · 🔴 **不判互不包含但可能命中同一元素的跨选择器**（`.mn-row .mn-lat` vs `.mn-lat.fast`；
 *    `#mc-alt-top` vs `.mc-top`）。**本模块对这一族一句话都不说，也不会抛。**
 *    管辖：**裁判**。上一版这里写的是「由 `declaringSites` 穷举腿 + 登记表接住」——
 *    那是假的：11 道消费门里 8 道一处都没接（R5-01 实测），补一条
 *    `.mn-row .mn-lat{ color: hsl(var(--err)) }` 全量绿而真机上 fast 档变红。
 *    现在这一族的权威判据是 `nodes-screen.test.tsx` ⑱ / `rules-screen.test.tsx` ⑮ 等处的裁判组。
 *  · **值不求解**：`var()` 不展开、`calc()` 不求值、`inherit` / `currentColor` 不追溯来源。
 *    管辖：③/⑬ 逐字对拍返回值。**推论**：自定义属性的**值**被覆写（`:root{--ok: 0 84% 60%}`）
 *    本模块看不见 —— `hsl(var(--ok))` 进去还是 `hsl(var(--ok))` 出来，而屏幕上已经变红了（R5-07）。
 *    管辖：裁判的「色族」腿（`nodes-screen.test.tsx` ⑱）。
 *  · **不建模 DOM 与继承**：谁是谁的祖先、某个元素最终命中哪几条规则，本模块不知道。
 *    跨选择器只报三个**可判定**子类：更窄（[`isNarrower`]）、更宽 + important（[`isBroader`] ⇒ 抛）、
 *    更宽且供着本选择器没写过的分量（[`WinnerSet.broaderSupplied`]）。
 *    ⚠️ 这三类的判定是**纯语法**的：`:is()` / `:matches()` / `:any()` 会先拆成分支再判
 *    （[`selectorAlternatives`]，R5-05），但**属性选择器不拆** —— `[class~="pane"]` 与 `.pane.dim`
 *    的包含关系本模块判不了，它落进上面那条「互不包含」，归裁判。
 *  · 🔴 **「更宽的非 important 选择器必输」是一条有前提的规则，不是不变量**：前提是
 *    **本选择器自己也声明了那个分量**。没声明时，浏览器里生效的就是更宽那条
 *    （`.mr-ap-body{pointer-events:none}` vs `.mr-ap-body.dim`，R5-03）。
 *    管辖：这类候选进 [`Resolution.broaderSupplied`] / [`WinnerSet.broaderSupplied`]，
 *    且 [`winnerValue`] 链为空时的报错会点名它。上一版的注释把这条写成了无条件的不变量。
 *  · 🔴 **取材面只有能解析成文件的 `.css`**。`@import 'tailwindcss'` 是包名不是路径，本模型
 *    展不开它（那是构建期构造）。现在它必须登记在 [`UNRESOLVED_CSS_IMPORTS`] 上、由
 *    `css-cascade.test.ts` ⑱ 钉住，**没登记的裸说明符当场抛**。上一版写着「文件解析不到 ⇒ 抛」
 *    而代码是 `if (next !== null) walk(next)` —— 静默跳过，桌面取材面里少了实际进包的 29 万字符（R5-08）。
 *    管辖：裁判 —— 它吃的是产品自己那条 postcss 管线的产物，Tailwind 与 `@layer` 都在里面。
 *    UA / 用户样式仍然不在任何一面内；`@keyframes` 等非层叠 at-rule 的块体不进作者来源的层叠。
 *  · 🔴 **[`winnerValue`] / [`winnersText`] / [`winners().won`] 不强制调用方面对跨选择器三桶**。
 *    上一版 [`WinnerSet`] 的注释写着「拿不到这个桶就拿不到 `won`，调用方**必须**对它表态」——
 *    那只是把桶放进了返回值，取值的四个入口一个都没碰它（R5-10）。真正的类型强制没有实现，
 *    也不打算实现（改造面 = 全部消费门）。**替代方案就是这一轮做的事**：把承重判据迁到裁判上。
 *  · 本仓 vitest `environment: 'node'`。裁判用的是本机 Chrome 152，**不是**目标 WebView
 *    （Android WebView / WKWebView）。裁判把「层叠算错」这一类从门里除掉，剩下的引擎差异仍然只有真机能答。
 *
 * # 🔴 条件档：本模块只看**无条件档**，条件档归裁判 —— 而并集之外今天**没有判据**
 *
 * 2026-09-05 第六轮 B1/B2。把两边的射程摆在一起说一次，免得读的人以为「两边合起来就全了」：
 *
 *  · **本模块**：默认谓词只收无条件声明；同键上还有条件块里的声明而调用方没显式给 `where` ⇒ 抛
 *    （保证 5 最后一条）。也就是说，「某个条件档下最终是多少」这个问题本模块**从不回答**。
 *  · **裁判**（`css-oracle.test-support.ts`）：回答的是「在**它采过样的那几个点**上最终是多少」。
 *    那几个点写在 `SAMPLED_FACE` 里。那张表的两段自检强度不同（一段的接线没判据、一段判的是源码
 *    字面量而非运行期），逐条写在裁判模块头注的「抓不到的形态」表里 —— 别当成「那张表全被守着」。
 *  · **两边的并集才是覆盖面**。并集之外 —— 裁判没采样、而本模块按定义也不看的那些条件档
 *    （别的视口、`dir=rtl`、别的 `data-*` 取值、`prefers-contrast` …）—— **今天一条判据都没有**。
 *    不是「通过了」，是**没人看**。视口 × 方向 × 主题 × `data-*` × `prefers-*` 是组合面，
 *    追它没有终点；本轮的口径是把上限写成实话，并给「声明 == 实际」建门，而不是做一个
 *    覆盖不全却看起来很全的东西。谁都抓不到的那些形态列在裁判模块头注那张表里。
 *
 * # 特异度：不算，只判可判定的子类
 *
 * 见 [`isNarrower`] / [`isBroader`] 的注释。一句话：算对了也不够（还得判「两个选择器命中同一元素」，
 * 静态面上判不了），算错时是**静默翻转胜出者**。所以 `chain` 只在选择器键相同时断言胜负，
 * 跨选择器只把上面那三个可判定子类自曝给调用方，其余一律丢弃或抛。
 */
import { existsSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, relative, resolve as resolvePath } from 'node:path';
import postcss from 'postcss';
import type { ChildNode, Root } from 'postcss';

/** …/ui/ */
const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
const abs = (relToUi: string) => resolvePath(UI_ROOT, relToUi);
const rel = (absPath: string) => relative(UI_ROOT, absPath).split('\\').join('/');

// ════════════════════════════════════════════════════════════════════════════
// ① 剥注释（**保长度**）—— 只给模块图走查与选择器列表切分用；规则取材走 postcss
// ════════════════════════════════════════════════════════════════════════════

/**
 * 只剥注释、**保留字符串原文**（保长度：剥掉的字符换成空格、换行原样留着）。
 *
 * 两个用处：本模块的**模块图走查**（`cssOrder()` 要读 import 说明符，而它就住在字符串里 ——
 * 连字符串一起抹平的剥法会让上下文变成空），以及 `cascade-dead-rules.test.ts` 的取材。
 * **规则取材不用它**：那一层 2026-09-05 起走 postcss，注释由真解析器摘掉（见 [`parseCss`]）。
 */
export const stripComments = (src: string): string => {
  let out = '';
  let i = 0;
  while (i < src.length) {
    if (src[i] === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end === -1 ? src.length : end + 2;
      out += src.slice(i, stop).replace(/[^\n]/g, ' ');
      i = stop;
      continue;
    }
    if (src[i] === '"' || src[i] === "'") {
      const q = src[i];
      let j = i + 1;
      while (j < src.length) {
        if (src[j] === '\\') {
          j += 2;
          continue;
        }
        if (src[j] === q) {
          j++;
          break;
        }
        if (src[j] === '\n') break;
        j++;
      }
      out += src.slice(i, j);
      i = j;
      continue;
    }
    out += src[i];
    i++;
  }
  return out;
};

const collapse = (s: string): string => s.replace(/\s+/g, ' ').trim();

/** 按**顶层**逗号切选择器列表：括号/方括号内的逗号（`:is(a, b)`、`[x="a,b"]`）不切。 */
export const selectorList = (prelude: string): string[] => {
  const parts: string[] = [];
  let depth = 0;
  let buf = '';
  for (const ch of prelude) {
    if (ch === '(' || ch === '[') depth++;
    else if (ch === ')' || ch === ']') depth = Math.max(0, depth - 1);
    if (ch === ',' && depth === 0) {
      if (collapse(buf)) parts.push(collapse(buf));
      buf = '';
      continue;
    }
    buf += ch;
  }
  if (collapse(buf)) parts.push(collapse(buf));
  return parts;
};

// ════════════════════════════════════════════════════════════════════════════
// ② 选择器归一化键
// ════════════════════════════════════════════════════════════════════════════

/** 组合符：`' '` 后代 / `>` 子 / `+` 相邻 / `~` 兄弟。 */
type Combinator = ' ' | '>' | '+' | '~';

export interface ParsedSelector {
  /** 原文（已折叠空白）。 */
  raw: string;
  /** 归一化键：字面相同 ⇒ 特异度必然相同。 */
  key: string;
  /** 各复合段的简单选择器（已归一 + 排序）。 */
  compounds: string[][];
  /** `combinators[i]` = `compounds[i]` 与 `compounds[i+1]` 之间的组合符。 */
  combinators: Combinator[];
  /** 选择器里出现过 `[dir=` ⇒ 该规则的胜负与书写方向有关。 */
  hasDir: boolean;
}

/** 旧式单冒号伪元素 —— 归一到双冒号，`:before` 与 `::before` 是同一件事。 */
const LEGACY_PSEUDO_ELEMENTS = new Set(['before', 'after', 'first-line', 'first-letter']);
/** 参数本身是选择器列表的函数式伪类：参数要递归归一，否则 `:not([a='b'])` 与 `:not([a="b"])` 不同键。 */
const SELECTOR_ARG_PSEUDOS = new Set(['not', 'is', 'where', 'has', 'matches', 'any']);
/**
 * **恒真**的简单选择器：`*`。它既不为特异度出分量，也不对匹配集设条件。
 *
 * 🔴 与 `:where(X)` 不是一回事。上一版把两者揉成一个 `isZeroSpecificity` 当「恒真条件」用，
 * 而 `:where(X)` 的特异度虽然是 0，它**参与匹配**：`.a:where(.b)` 只命中同时带 `.b` 的元素。
 * 后果是 `.a:where(.b)` 对着查询 `.a` 既判不出更窄（特异度没涨）也判不出更宽（它多了一个真条件），
 * 两个桶都不进 ⇒ **静默丢弃**；而浏览器里它与 `.a` 特异度相同、后来者胜，
 * 模型给出的正是反的答案（2026-09-05 R5-05 的姊妹腿，两侧对拍见 `css-oracle.test.ts` ⑦）。
 * 现在这一族走 [`narrowsWithoutSpecificity`] ⇒ [`resolve`] 当场抛。
 */
const isAlwaysTrue = (simple: string) => simple === '*';
/** 零特异度但**参与匹配**的简单选择器：`:where(X)`。 */
const isZeroSpecificityMatcher = (simple: string) => simple.startsWith(':where(');
/** 不为特异度出分量的简单选择器（恒真的 `*` 与参与匹配的 `:where(X)` 都算）。 */
const carriesNoSpecificity = (simple: string) => isAlwaysTrue(simple) || isZeroSpecificityMatcher(simple);

const ATTR = /^\[\s*([^\s~^$*|=\]]+)\s*(?:([~^$*|]?=)\s*("[^"]*"|'[^']*'|[^\]\s]+)\s*)?([iIsS]\s*)?\]$/;

/** `[dir=rtl]` ≡ `[dir="rtl"]` ≡ `[dir='rtl']`；大小写标志保留并小写。 */
function normalizeAttribute(simple: string): string {
  const m = ATTR.exec(simple);
  if (m === null) throw new Error(`属性选择器解析不了（未建模写法）：\`${simple}\``);
  const name = m[1];
  if (m[2] === undefined) return `[${name}]`;
  let value = m[3];
  if (
    (value.startsWith('"') && value.endsWith('"')) ||
    (value.startsWith("'") && value.endsWith("'"))
  ) {
    value = value.slice(1, -1);
  }
  const flag = m[4] === undefined ? '' : ` ${m[4].trim().toLowerCase()}`;
  return `[${name}${m[2]}"${value}"${flag}]`;
}

/** 把一个复合段切成简单选择器（`.a.b:hover::before` → 四个）。认不出的字符**抛**。 */
function splitCompound(compound: string): string[] {
  const out: string[] = [];
  let i = 0;
  const eatWord = () => {
    while (i < compound.length) {
      if (compound[i] === '\\') {
        i += 2;
        continue;
      }
      if (/[\w-]/.test(compound[i])) {
        i += 1;
        continue;
      }
      break;
    }
  };
  const eatBalanced = (open: string, close: string) => {
    let depth = 0;
    while (i < compound.length) {
      if (compound[i] === open) depth += 1;
      else if (compound[i] === close) {
        depth -= 1;
        if (depth === 0) {
          i += 1;
          return;
        }
      }
      i += 1;
    }
    throw new Error(`选择器括号不配平：\`${compound}\``);
  };
  while (i < compound.length) {
    const start = i;
    const c = compound[i];
    if (c === '.' || c === '#') {
      i += 1;
      eatWord();
    } else if (c === '[') {
      eatBalanced('[', ']');
    } else if (c === ':') {
      i += 1;
      if (compound[i] === ':') i += 1;
      eatWord();
      if (compound[i] === '(') eatBalanced('(', ')');
    } else if (c === '*') {
      i += 1;
    } else if (/[\w\\|-]/.test(c)) {
      eatWord();
    } else if (c === '&') {
      throw new Error(
        `选择器里出现 CSS 嵌套 \`&\`（\`${compound}\`）—— 本解析器不建模嵌套。` +
          `嵌套的展开与层叠序需要另一个量级的实现，静默按字面处理会给出反的答案。`,
      );
    } else {
      throw new Error(`选择器里出现未建模字符 \`${c}\`：\`${compound}\``);
    }
    if (i === start) throw new Error(`选择器解析原地打转：\`${compound}\``);
    out.push(compound.slice(start, i));
  }
  return out;
}

function normalizeSimple(simple: string): string {
  if (simple.startsWith('[')) return normalizeAttribute(simple);
  if (!simple.startsWith(':')) return simple;
  const m = /^(::?)([-\w]+)(?:\(([\s\S]*)\))?$/.exec(simple);
  if (m === null) throw new Error(`伪类/伪元素解析不了（未建模写法）：\`${simple}\``);
  const name = m[2].toLowerCase();
  if (m[3] === undefined) {
    const doubled = m[1] === '::' || LEGACY_PSEUDO_ELEMENTS.has(name);
    return `${doubled ? '::' : ':'}${name}`;
  }
  const args = SELECTOR_ARG_PSEUDOS.has(name)
    ? selectorList(m[3])
        .map((s) => parseSelector(s).key)
        .join(',')
    : collapse(m[3]);
  return `${m[1]}${name}(${args})`;
}

/** 复合段内的简单选择器**排序后**拼接 ⇒ `.a.b` 与 `.b.a` 同键。 */
const compoundKey = (simples: readonly string[]) => [...simples].sort().join('');

/** 把一条复合选择器切成「复合段 + 组合符」。组合符两侧空白不影响结果。 */
function splitComplex(sel: string): { compounds: string[]; combinators: Combinator[] } {
  const compounds: string[] = [];
  const combinators: Combinator[] = [];
  let depth = 0;
  let cur = '';
  let pending: Combinator | null = null;
  for (const ch of sel) {
    if (ch === '(' || ch === '[') depth += 1;
    else if (ch === ')' || ch === ']') depth = Math.max(0, depth - 1);
    if (depth === 0) {
      if (/\s/.test(ch)) {
        if (cur !== '') {
          compounds.push(cur);
          cur = '';
          pending = ' ';
        }
        continue;
      }
      if (ch === '>' || ch === '+' || ch === '~') {
        if (cur !== '') {
          compounds.push(cur);
          cur = '';
        }
        pending = ch;
        continue;
      }
    }
    if (cur === '' && pending !== null) {
      combinators.push(pending);
      pending = null;
    }
    cur += ch;
  }
  if (cur !== '') compounds.push(cur);
  return { compounds, combinators };
}

/**
 * 把 `:is(a, b)` / `:matches()` / `:any()` 拆成**分支**（`:is(.x,.y) .z` → `.x .z` 与 `.y .z`）。
 *
 * 2026-09-05 R5-05：[`isBroader`] / [`isNarrower`] 是**纯语法**超集判定，`:is()` 包一层就判不出来 ——
 * 实测 `:is(.mr-ap-body, .zzz){ color: … !important }` 从 `.mr-ap-body.dim` 的查询底下原样溜过去，
 * 而它在浏览器里必胜（important 排在特异度之前）。拆成分支之后 `.mr-ap-body` 这一支是可判定的更宽者，
 * BLK-1 当场抛。
 *
 * `:where()` **不拆**：拆开会把「零特异度」这条信息丢掉。它自己那一族（特异度打平、匹配集收窄 ⇒
 * 胜负回到源序 ⇒ 本模型给不出一个值）由 [`narrowsWithoutSpecificity`] 判，[`resolve`] 当场抛。
 *
 * 分支数封顶 [`MAX_ALTERNATIVES`]：超了当场抛，不静默截断（截断 = 少判一支 = 静默放行）。
 */
const MAX_ALTERNATIVES = 64;
const IS_PSEUDO = /:(?:is|matches|any)\(/i;

export function selectorAlternatives(sel: string): string[] {
  const m = IS_PSEUDO.exec(sel);
  if (m === null) return [sel];
  const open = m.index + m[0].length - 1;
  let depth = 0;
  let close = -1;
  for (let i = open; i < sel.length; i += 1) {
    if (sel[i] === '(' || sel[i] === '[') depth += 1;
    else if (sel[i] === ')' || sel[i] === ']') {
      depth -= 1;
      if (depth === 0) {
        close = i;
        break;
      }
    }
  }
  if (close === -1) throw new Error(`选择器 \`${sel}\` 里的 \`:is(\` 没有收口 —— 解析不了就不判。`);
  const head = sel.slice(0, m.index);
  const tail = sel.slice(close + 1);
  const out: string[] = [];
  for (const branch of selectorList(sel.slice(open + 1, close)))
    for (const rest of selectorAlternatives(`${head}${branch}${tail}`)) {
      out.push(rest);
      if (out.length > MAX_ALTERNATIVES)
        throw new Error(
          `选择器 \`${sel}\` 展开出的 \`:is()\` 分支超过 ${MAX_ALTERNATIVES} 条 —— ` +
            `本解析器不静默截断（少判一支 = 静默放行一条可能压过本查询的规则）。`,
        );
    }
  return out;
}

/** 候选的**任一** `:is()` 分支严格更窄 ⇒ 判为更窄。更宽方向同理。 */
const anyAlternative = (
  raw: string,
  query: ParsedSelector,
  test: (c: ParsedSelector, q: ParsedSelector) => boolean,
): boolean => selectorAlternatives(raw).some((alt) => test(parseSelector(alt), query));

const SELECTOR_CACHE = new Map<string, ParsedSelector>();

/** 解析并归一一条（不含逗号的）复合选择器。解析不了一律抛。 */
export function parseSelector(sel: string): ParsedSelector {
  const raw = collapse(sel);
  const hit = SELECTOR_CACHE.get(raw);
  if (hit !== undefined) return hit;
  if (raw === '') throw new Error('空选择器');
  const { compounds, combinators } = splitComplex(raw);
  if (compounds.length === 0) throw new Error(`选择器切不出复合段：\`${raw}\``);
  const parsed = compounds.map((c) => splitCompound(c).map(normalizeSimple));
  const key = parsed
    .map(compoundKey)
    .reduce((acc, k, i) => (i === 0 ? k : `${acc}${combinators[i - 1]}${k}`), '');
  const out: ParsedSelector = {
    raw,
    key,
    compounds: parsed,
    combinators,
    hasDir: /\[\s*dir\s*[~^$*|]?=/.test(raw),
  };
  SELECTOR_CACHE.set(raw, out);
  return out;
}

/** 归一化键（等价于 `parseSelector(sel).key`）。 */
export const selectorKey = (sel: string) => parseSelector(sel).key;

/**
 * 候选选择器是否**严格更窄**于查询选择器。
 *
 * 判据（无算术、可判定、方向单调）：候选末尾 n 段与查询的 n 段逐段对齐，段间组合符逐一相等，
 * 每一段候选的简单选择器集合 ⊇ 查询的，且「候选多出至少一个非零特异度的简单选择器」或
 * 「候选多出祖先/兄弟段」。此时：
 *
 *  · **匹配集单调收缩**：多一个简单选择器 = 多一个必须同时成立的条件；多一段祖先/兄弟 = 多一个
 *    结构约束 ⇒ 候选的匹配集 ⊆ 查询的匹配集。
 *  · **三元特异度分量严格增加**：多出来的那一个必须**带特异度**（`:is()`/`:has()`/`:not()` 取参数
 *    特异度，非负；`:where()` 与 `*` 归零）。🔴 上一版这里只要「段数变多」就判 strict，于是
 *    `:where(.x) .a` / `* .a` 这类**匹配集收窄而特异度打平**的候选被塞进本桶，而本桶的结论
 *    （「必胜、与源序无关」）对它们不成立。它们现在归 [`narrowsWithoutSpecificity`]。
 *
 * 两条合起来 ⇒ **在候选命中的元素上候选必胜，与源序无关**。所以这一桶不给值，只报「有人从更窄处压你」，
 * 由调用方 `expect(narrower).toEqual([])` 或维护一份「已知更窄覆盖」白名单。
 *
 * 反过来，**更宽的候选**（查 `.a.b` 时的 `.b`）—— 2026-09-05 BLK-1 起这句话是**条件式**的：
 * 层叠的比较顺序是「来源与层 → `!important` → 特异度 → 源序」，`!important` 排在特异度**之前**。
 * 所以只有**两侧 `!important` 相同**时「更宽 ⇒ 特异度严格更低 ⇒ 必输」才成立；
 * **更宽 + `!important`** 反而必胜（它命中的元素集 ⊇ 本查询的 ⇒ 在本查询命中的每个元素上它都在场）。
 * 故 [`isBroader`] 单独判这一族：更宽且 important ⇒ 模型给不出对的答案 ⇒ **抛**。
 * 更宽且非 important ⇒ **有前提地**丢弃 —— 前提是本选择器自己也声明了那个分量（那时它特异度更高必胜）。
 * 本选择器没声明时，浏览器里生效的就是更宽那条，故它进 [`Resolution.broaderSupplied`]（2026-09-05 R5-03）。
 * **互不包含**的候选（`.mn-row .mn-lat` vs `.mn-lat.fast`、`[class~="pane"]` vs `.pane.dim`）
 * 静态面上不可判定 ⇒ 丢弃，**本模块对它一句话都不说**。那一族归裁判
 * （`css-oracle.test-support.ts`）—— 上一版这里写的「由 `declaringSites` 穷举腿 + 登记表接住」是假的，
 * 11 道消费门里 8 道一处都没接（R5-01 实测）。
 */
export function isNarrower(candidate: ParsedSelector, query: ParsedSelector): boolean {
  const shape = alignNarrowing(candidate, query);
  return shape !== null && shape.addsSpecificity;
}

/**
 * 候选**匹配集严格更小、特异度却打平**的那一族：`.a:where(.b)` 对着 `.a`、`* .a` 对着 `.a`。
 *
 * 这一族在浏览器里的胜负回到**源序**（同特异度后者胜），而且只在候选也命中的那部分元素上翻转 ——
 * 查询 `.a` 的答案于是不是一个值，是两个。本模型只在同一个选择器键内裁决，给不出这个答案，
 * 故 [`resolve`] 对它**当场抛**（与「更宽 + important」同一条口径），把这一面交给裁判
 * `css-oracle.test-support.ts`：喂真 DOM 与整条链，问浏览器。
 */
export function narrowsWithoutSpecificity(candidate: ParsedSelector, query: ParsedSelector): boolean {
  const shape = alignNarrowing(candidate, query);
  return shape !== null && !shape.addsSpecificity && shape.addsCondition;
}

/**
 * 候选的末尾若干段能否与查询逐段对齐（组合符相等、每段的简单选择器 ⊇ 查询的）。
 * 对齐得上时顺带报两件事：**多出来的条件里有没有带特异度的**、**有没有多出条件**。
 */
function alignNarrowing(
  candidate: ParsedSelector,
  query: ParsedSelector,
): { addsSpecificity: boolean; addsCondition: boolean } | null {
  const n = query.compounds.length;
  const m = candidate.compounds.length;
  if (m < n) return null;
  const off = m - n;
  for (let i = 0; i < n - 1; i += 1) {
    if (candidate.combinators[off + i] !== query.combinators[i]) return null;
  }
  let addsSpecificity = false;
  // 多出来的祖先/兄弟段：整段都不带特异度（`* .a` / `:where(.x) .a`）时，它只收窄匹配集。
  for (let i = 0; i < off; i += 1) {
    if (candidate.compounds[i].some((s) => !carriesNoSpecificity(s))) addsSpecificity = true;
  }
  let addsCondition = off > 0;
  for (let i = 0; i < n; i += 1) {
    const cs = candidate.compounds[off + i];
    const qs = query.compounds[i];
    for (const s of qs) if (!cs.includes(s)) return null;
    for (const s of cs) {
      if (qs.includes(s)) continue;
      if (!carriesNoSpecificity(s)) {
        addsSpecificity = true;
        addsCondition = true;
      } else if (isZeroSpecificityMatcher(s)) {
        // `:where(X)` 特异度为 0 但**参与匹配** ⇒ 只收窄匹配集。
        addsCondition = true;
      }
    }
  }
  return { addsSpecificity, addsCondition };
}

/**
 * 候选是否**严格更宽**于查询：候选命中的元素集 ⊇ 查询命中的元素集。
 *
 * 判据与 [`isNarrower`] 同形，只是方向反过来，外加一条：候选里的**恒真**简单选择器（`*`）
 * 可以直接忽略 —— 否则 `* { … !important }` 这种「宽到极致」的候选会被判成「互不包含」而丢掉
 * （本仓 `styles/index.css:278-279` 真有两条）。🔴 `:where(X)` **不在此列**：它是一个真条件，
 * 忽略它会把 `.a:where(.b)` 误判成「不比 `.a` 窄」（见 [`isAlwaysTrue`] 的注释）。
 * 「候选比查询宽」的严格性也按**匹配集**判而不按特异度判：更宽 + important 的必胜与特异度无关。
 *
 * 为什么要单独有它：`!important` 在层叠里排在特异度**之前**，
 * **更宽 + important 必胜**。判不到它 ⇒ 一条 `.mn-lat{ color: … !important }` 就能在
 * 十几道共用本解析器的门底下把任意被守的值改掉，而全量门全绿（2026-09-05 BLK-1 实测）。
 */
export function isBroader(candidate: ParsedSelector, query: ParsedSelector): boolean {
  const n = candidate.compounds.length;
  const m = query.compounds.length;
  if (m < n) return false;
  const off = m - n;
  for (let i = 0; i < n - 1; i += 1) {
    if (query.combinators[off + i] !== candidate.combinators[i]) return false;
  }
  let strict = m > n;
  for (let i = 0; i < n; i += 1) {
    const cs = candidate.compounds[i];
    const qs = query.compounds[off + i];
    // 候选的每个**有效**条件，查询都得有（只有恒真的 `*` 不算条件）。
    for (const c of cs) if (!isAlwaysTrue(c) && !qs.includes(c)) return false;
    if (qs.some((q) => !cs.includes(q) && !isAlwaysTrue(q))) strict = true;
  }
  return strict;
}

// ════════════════════════════════════════════════════════════════════════════
// ③ 声明模型与解析
// ════════════════════════════════════════════════════════════════════════════

export interface Decl {
  /** 相对 `ui/` 的路径；内存源则是调用方给的名字。 */
  file: string;
  /** 1 起的行号（保长度剥噪 ⇒ 与原文一致）。 */
  line: number;
  /** 跨文件连号的全局顺位；越大越靠后。 */
  order: number;
  /** 归一化后的选择器键（逗号列表逐项）。 */
  sels: string[];
  /** 选择器原文（报错消息用）。 */
  rawSels: string[];
  prop: string;
  /** 已剥掉 `!important` 的值。 */
  value: string;
  important: boolean;
  /** 由内到外的 at-rule prelude 原文；`[]` = 无条件。 */
  conds: string[];
  /**
   * 来自 `@property { initial-value }` 的**注册初值**，不是层叠里的声明。
   * 不进 [`resolve`] 的链，只进 [`declaringSites`] —— 它正是 `MobileSettings` 那条 M4 的入口：
   * 注册一个 `--row-min` 就能让全树的 `var(--row-min, 54px)` 处处解析成注册初值，
   * 而按「`--row-min:` 这串字面量在不在」判的登记表一条都不命中。
   */
  registered: boolean;
}

export interface CssContext {
  /** CSS `@import` 里解析不到文件的裸说明符（见 [`UNRESOLVED_CSS_IMPORTS`]）。本模型看不见它们。 */
  unresolvedImports?: { importer: string; spec: string }[];
  /** `desktop` / `mobile` / `tray` / `popup`，或内存源的 `<memory>`。 */
  name: string;
  /** 按进包顺序的文件列表。 */
  files: string[];
  decls: Decl[];
}

export interface Src {
  file: string;
  css: string;
}

const UNMODELED_AT = /^@(layer|scope)\b/;
/**
 * 内部块**不是层叠意义上的规则**的 at-rule。
 *
 * `@keyframes fade { 0% { opacity: 0 } }` 里的 `0%` 是关键帧选择器不是 CSS 选择器，
 * 它的声明属于动画来源、不进作者来源的层叠（文件头「不建模 transition/animation 来源」）。
 * 不排掉它，`0%` 会被送进选择器解析器然后当场抛。
 */
const NON_CASCADE_AT = /^@(?:-\w+-)?(keyframes|counter-style|font-feature-values|page|viewport)\b/;

/**
 * 一份 CSS 的全部声明。
 *
 * **解析层走 `postcss`**（`ui/package.json` 里既有的 devDependency，离线可用）。此前这里是一个
 * 手写的花括号/引号平衡扫描器，它在**原生 CSS 嵌套**上给出反的答案：`.a { .b { … } }` 不带 `&`，
 * 扫描器把 `.b` 当成一条独立顶层选择器 push 进栈、祖先整段丢掉 —— 两个方向都错（既能把不相干规则
 * 塞进被守选择器的链造假红，也能让被守规则挪进一个不存在的父级而模型毫无察觉造假绿）。
 * 嵌套 / 注释 / 选择器列表 / at-rule prelude 这一层交给 postcss；层叠语义（特异度 / `!important` /
 * 源序 / 简写复位 / 族）仍是本模块自己的，只是面积小了很多。
 *
 * 结构：最内层的**非 at-rule** prelude 才是选择器；`@property` 块是唯一的例外（见 [`Decl.registered`]）。
 * **嵌套的判据是「非 at-rule 块里还有块」**，不是「prelude 里有没有 `&`」—— 遇到即抛。
 */
export function parseCss(file: string, css: string, order0: number): { decls: Decl[]; next: number } {
  const decls: Decl[] = [];
  let order = order0;
  let root: Root;
  try {
    root = postcss.parse(css, { from: file });
  } catch (e) {
    const err = e as { line?: number; reason?: string; message?: string };
    throw new Error(
      `${file}${err.line === undefined ? '' : `:${err.line}`} CSS 解析失败：` +
        `${err.reason ?? err.message ?? String(e)}。` +
        `此前的手写扫描器对未闭合花括号是**静默容忍**的（后面的规则整批错位而无人报），故这里当场抛。`,
    );
  }
  const lineOf = (n: ChildNode): number => n.source?.start?.line ?? 0;

  const visit = (node: ChildNode, stack: readonly string[]): void => {
    if (node.type === 'comment') return;

    if (node.type === 'atrule') {
      const prelude = collapse(node.params === '' ? `@${node.name}` : `@${node.name} ${node.params}`);
      if (UNMODELED_AT.test(prelude)) {
        throw new Error(
          `${file}:${lineOf(node)} 出现未建模的 at-rule \`${prelude}\`。` +
            `\`@layer\` / \`@scope\` 会改写「同键后者胜」这条前提（本解析器全部结论都站在它上面），` +
            `静默放行等于给出反的答案。`,
        );
      }
      for (const child of node.nodes ?? []) visit(child, [...stack, prelude]);
      return;
    }

    if (node.type === 'rule') {
      const prelude = collapse(node.selector);
      const outer = [...stack].reverse().find((p) => !p.startsWith('@'));
      if (outer !== undefined) {
        throw new Error(
          `${file}:${lineOf(node)} 出现 CSS 嵌套：\`${outer}\` 的块体里还有一个块 \`${prelude}\` —— ` +
            `本解析器不建模嵌套。嵌套的展开与层叠序需要另一个量级的实现，静默按字面处理会给出反的答案` +
            `（把内层当顶层规则、祖先整段丢掉，既能造假红也能造假绿）。` +
            `另注：原生嵌套是 Chrome 112/120 的能力，远超本仓声明的 ≤105 下限。`,
        );
      }
      if (prelude.includes('&')) {
        throw new Error(
          `${file}:${lineOf(node)} 出现 CSS 嵌套 \`${prelude}\` —— 本解析器不建模嵌套。`,
        );
      }
      for (const child of node.nodes ?? []) visit(child, [...stack, prelude]);
      return;
    }

    const prop = node.prop;
    // 自定义属性的值 postcss 保留原文尾随空白（`--x: 54px ` → `'54px '`）。规范说自定义属性的
    // 值是「去掉首尾空白后的记号序列」，故这里 trim —— 不 trim 的话每条 `toBe('54px')` 都会假红。
    const value = node.value.trim();
    const important = node.important === true;
    const line = lineOf(node);
    if (/^(writing-mode|direction)$/i.test(prop)) {
      throw new Error(
        `${file}:${line} 出现 \`${prop}: ${value}\` —— 本解析器的逻辑属性↔物理属性映射` +
          `建立在 \`horizontal-tb\` + 由 \`[dir]\` 决定书写方向之上。声明式改写书写模式会让那张映射表` +
          `静默给出反的答案，故当场抛。要建模它，先把 \`LOGICAL\` 那张表升成按元素求 writing-mode。`,
      );
    }
    const owner = [...stack].reverse().find((p) => !p.startsWith('@'));
    const conds = [...stack].reverse().filter((p) => p.startsWith('@'));
    if (conds.some((c) => NON_CASCADE_AT.test(c))) return;
    if (owner === undefined) {
      const inner = stack[stack.length - 1] ?? '';
      const reg = /^@property\s+(--[\w-]+)$/.exec(inner);
      if (reg === null || prop.toLowerCase() !== 'initial-value') return;
      decls.push({
        file,
        line,
        order: order++,
        sels: ['@property'],
        rawSels: [inner],
        prop: reg[1],
        value,
        important,
        conds: conds.slice(1),
        registered: true,
      });
      return;
    }
    const list = selectorList(owner);
    decls.push({
      file,
      line,
      order: order++,
      sels: list.map((s) => parseSelector(s).key),
      rawSels: list,
      prop,
      value,
      important,
      conds,
      registered: false,
    });
  };

  for (const node of root.nodes) visit(node, []);
  return { decls, next: order };
}

// ── 进包顺序：入口模块图的后序遍历 ───────────────────────────────────────────

const TS_SPEC = /\bfrom\s*['"]([^'"]+)['"]|\bimport\s*\(\s*['"]([^'"]+)['"]|^[ \t]*import\s+['"]([^'"]+)['"]/gm;
const CSS_SPEC = /@import\s+['"]([^'"]+)['"]/g;

const specifiersOf = (src: string, isCss: boolean): string[] => {
  const out: string[] = [];
  const body = stripComments(src);
  for (const m of body.matchAll(isCss ? CSS_SPEC : TS_SPEC)) out.push(m[1] ?? m[2] ?? m[3]);
  return out;
};

/**
 * 模块图取材面的**文件系统**。默认就是真磁盘；给一份内存实现即可对着合成模块图做正向/反向对照。
 *
 * 🔴 为什么要能注入：[`cssOrderWithGaps`] 里那条「解析不到的裸 `@import` 不在白名单 ⇒ 抛」
 * 是 R5-08 的修复本体，而它此前**没有任何门看着**（2026-09-05 实测：把它改成 `if (false)`，
 * 逐字等于恢复修复前那条静默跳过，全量 237 文件 / 3957 断言全绿）。守「取材面缺一块且缺得
 * 无声无息」的分支自己无声无息，是同一个缺陷换了个位置。判据见 `css-cascade.test.ts` ⑳。
 */
export interface SourceFs {
  read: (absPath: string) => string;
  isFile: (absPath: string) => boolean;
}

/** 真磁盘。 */
export const DISK_FS: SourceFs = {
  read: (absPath) => readFileSync(absPath, 'utf8'),
  isFile: (absPath) => existsSync(absPath) && statSync(absPath).isFile(),
};

/** 内存模块图：键是**相对 `ui/` 的路径**，值是文件内容。用于对着合成输入验判据本身。 */
export function memoryFs(files: Readonly<Record<string, string>>): SourceFs {
  const byAbs = new Map(Object.entries(files).map(([f, css]) => [abs(f), css]));
  return {
    read: (absPath) => {
      const hit = byAbs.get(absPath);
      if (hit === undefined) throw new Error(`内存模块图里没有 ${rel(absPath)}`);
      return hit;
    },
    isFile: (absPath) => byAbs.has(absPath),
  };
}

const resolveSpec = (spec: string, importerAbs: string, fs: SourceFs): string | null => {
  const base = spec.startsWith('@/')
    ? resolvePath(UI_ROOT, 'src', spec.slice(2))
    : spec.startsWith('.')
      ? resolvePath(dirname(importerAbs), spec)
      : null;
  if (base === null) return null;
  for (const cand of [
    base,
    `${base}.ts`,
    `${base}.tsx`,
    `${base}.css`,
    `${base}.json`,
    `${base}/index.ts`,
    `${base}/index.tsx`,
  ]) {
    if (fs.isFile(cand)) return cand;
  }
  return null;
};

/**
 * CSS 进包顺序 = 入口模块图的**后序**遍历（依赖先于自身，同层按源码里的 import 次序）。
 *
 * 这正是 `MobileMain.tsx` 那条缺陷的机理：`import { MobileApp } from './MobileApp'` 排在
 * `import './mobile.css'` **之前**，于是各屏 CSS 整体先于外壳 CSS 进包，外壳的 `.m-section`
 * 便压过了 `nodes.css` 的 `.mn`。顺序写成代码，那类缺陷才有人看得见 —— 写死一份文件清单则不然。
 */
export function cssOrder(entryModule: string): string[] {
  return cssOrderWithGaps(entryModule).files;
}

/**
 * CSS `@import` 里**解析不到文件**的裸说明符白名单。
 *
 * 🔴 2026-09-05 R5-08：`index.css:9` 的 `@import 'tailwindcss'` 是一个**包名**，不是路径 ——
 * [`resolveSpec`] 对它返回 `null`，而此前的 `if (next !== null) walk(next)` 是**静默跳过**。
 * 于是静态模型的桌面取材面里，实际进包的那 29 万字符 Tailwind 产物一条规则都没有，
 * 而文件头写着「文件解析不到 ⇒ 抛」。那句话与代码直接冲突，是本轮最典型的
 * 「一条不受判据管辖的声明」。
 *
 * 现在的口径是：**没在这张表上的裸说明符 ⇒ 抛**；在表上的记进
 * [`CssContext.unresolvedImports`]，由 `css-cascade.test.ts` ⑱ 逐条钉住。
 * 而 `tailwindcss` 这一份**在本模型里本来就展不开**（它是构建期构造，要跑 Tailwind 编译器）——
 * 那一面的权威判据是 `css-oracle.test-support.ts`：它吃的是产品自己那条 postcss 管线的产物。
 */
export const UNRESOLVED_CSS_IMPORTS: readonly string[] = ['tailwindcss'];

/** [`cssOrder`] + 解析不到的裸 `@import` 说明符清单。 */
export function cssOrderWithGaps(
  entryModule: string,
  fs: SourceFs = DISK_FS,
): {
  files: string[];
  unresolved: { importer: string; spec: string }[];
} {
  const done = new Set<string>();
  const visiting = new Set<string>();
  const out: string[] = [];
  const unresolved: { importer: string; spec: string }[] = [];
  const walk = (fileAbs: string): void => {
    if (done.has(fileAbs) || visiting.has(fileAbs)) return;
    visiting.add(fileAbs);
    if (/\.(?:tsx?|css)$/.test(fileAbs)) {
      const isCss = fileAbs.endsWith('.css');
      for (const spec of specifiersOf(fs.read(fileAbs), isCss)) {
        const next = resolveSpec(spec, fileAbs, fs);
        if (next !== null) {
          walk(next);
          continue;
        }
        if (!isCss) continue; // TS 侧的 npm 包与本模块无关（CSS 才进层叠）。
        if (!UNRESOLVED_CSS_IMPORTS.includes(spec))
          throw new Error(
            `${rel(fileAbs)} 的 \`@import '${spec}'\` 解析不到文件 —— 它是**真的进包**的一块 CSS，` +
              `而本模型看不见它 ⇒ 取材面缺一块且缺得无声无息。要么把它变成可解析的路径，` +
              `要么写进 \`UNRESOLVED_CSS_IMPORTS\` 并在 \`css-cascade.test.ts\` ⑱ 里说明那一面归谁判` +
              `（裁判 \`css-oracle.test-support.ts\` 吃的是产品那条 postcss 管线的产物，它看得见）。`,
          );
        unresolved.push({ importer: rel(fileAbs), spec });
      }
    }
    visiting.delete(fileAbs);
    done.add(fileAbs);
    if (fileAbs.endsWith('.css')) out.push(rel(fileAbs));
  };
  const entryAbs = abs(entryModule);
  if (!fs.isFile(entryAbs)) throw new Error(`入口模块解析不到：${entryModule}`);
  walk(entryAbs);
  return { files: out, unresolved };
}

export type CtxName = 'desktop' | 'mobile' | 'tray' | 'popup';

/** 四个层叠上下文的入口模块（实测自源码，不是约定俗成的清单）。 */
export const ENTRY_MODULE: Readonly<Record<CtxName, string>> = {
  desktop: 'src/main.tsx',
  mobile: 'src/mobile/MobileMain.tsx',
  tray: 'src/tray/main.tsx',
  popup: 'src/update-popup/main.ts',
};

const CONTEXTS = new Map<CtxName, CssContext>();

/** 某个层叠上下文的全部声明（缓存）。 */
export function context(ctx: CtxName): CssContext {
  const hit = CONTEXTS.get(ctx);
  if (hit !== undefined) return hit;
  const { files, unresolved } = cssOrderWithGaps(ENTRY_MODULE[ctx]);
  if (files.length === 0) {
    throw new Error(`上下文 \`${ctx}\` 一份 CSS 都没解析出来 —— 空取材面上的否定断言恒真。`);
  }
  const built = contextOf(
    files.map((f) => ({ file: f, css: readFileSync(abs(f), 'utf8') })),
    ctx,
  );
  built.unresolvedImports = unresolved;
  CONTEXTS.set(ctx, built);
  return built;
}

/** 从内存源建上下文 —— 反向对照与变异自检用，不必写盘（这棵树是多线共用的）。 */
export function contextOf(sources: readonly Src[], name = '<memory>'): CssContext {
  const decls: Decl[] = [];
  let order = 0;
  for (const s of sources) {
    const r = parseCss(s.file, s.css, order);
    decls.push(...r.decls);
    order = r.next;
  }
  return { name, files: sources.map((s) => s.file), decls };
}

// ════════════════════════════════════════════════════════════════════════════
// ④ 属性族：简写 / 长写 / 逻辑属性折成同一个「语义分量」键
// ════════════════════════════════════════════════════════════════════════════

export type Edge = 'top' | 'right' | 'bottom' | 'left';
export type Dir = 'ltr' | 'rtl';
export interface Env {
  dir: Dir;
}

const EDGES: readonly Edge[] = ['top', 'right', 'bottom', 'left'];
const DEFAULT_ENV: Env = { dir: 'ltr' };

/** 按**顶层**空白切分（`calc(a + b)` 里的空格不算）。逐字取自 `safe-area-consumption.test.ts`。 */
export function topLevelParts(value: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let cur = '';
  for (const ch of value) {
    if (ch === '(') depth += 1;
    if (ch === ')') depth -= 1;
    if (depth === 0 && /\s/.test(ch)) {
      if (cur !== '') parts.push(cur);
      cur = '';
    } else {
      cur += ch;
    }
  }
  if (cur !== '') parts.push(cur);
  return parts;
}

/** 简写值里属于某条边的那一个分量（1/2/3/4 值形态，与 CSS 规范同）。 */
export function shorthandComponent(value: string, edge: Edge): string | null {
  const p = topLevelParts(value);
  if (p.length === 0 || p.length > 4) return null;
  const idx: Record<Edge, number> =
    p.length === 1
      ? { top: 0, right: 0, bottom: 0, left: 0 }
      : p.length === 2
        ? { top: 0, right: 1, bottom: 0, left: 1 }
        : p.length === 3
          ? { top: 0, right: 1, bottom: 2, left: 1 }
          : { top: 0, right: 1, bottom: 2, left: 3 };
  return p[idx[edge]];
}

/** 逻辑侧 → 物理边。轴映射与方向无关；`inline-*` 两条依赖 `dir`（本仓 RTL 是活的）。 */
function physicalEdge(side: string, env: Env): Edge {
  switch (side) {
    case 'block-start':
      return 'top';
    case 'block-end':
      return 'bottom';
    case 'inline-start':
      return env.dir === 'rtl' ? 'right' : 'left';
    case 'inline-end':
      return env.dir === 'rtl' ? 'left' : 'right';
    default:
      throw new Error(`未建模的逻辑侧：\`${side}\``);
  }
}

/** 与书写方向有关的键（查询命中这些键、且链里出现 `[dir=` 选择器却没显式给 env ⇒ 抛）。 */
const isDirSensitiveKey = (key: string) => /-(left|right)$|-(left|right)-/.test(key);

/**
 * 简写**没写到**的分量记这个记号 —— 它是「复位到初值」，不是「这条声明没说」。
 *
 * 2026-09-05 MAJ-6：`border: none` 只贡献 style，此前模型里 `border-*-width` 仍由前一条声明供着
 * ⇒ 模型上有边框、浏览器上没有，而盯着「宽度有定义」的判据恒绿。凡带复位语义的简写
 * （`border` / `outline` / `background` / `font` / `flex` / `text-decoration` / `list-style`）
 * 一律把缺席分量写成本记号。
 */
export const INITIAL = 'initial';

/** CSS 全局关键字：整条简写换成它时，每个分量都取它。 */
const CSS_WIDE = new Set(['inherit', 'initial', 'unset', 'revert', 'revert-layer']);

/** 按**顶层**逗号切（`hsl(a, b)` / `:is(a, b)` 里的逗号不切）。 */
function topLevelCommaParts(value: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let cur = '';
  for (const ch of value) {
    if (ch === '(' || ch === '[') depth += 1;
    else if (ch === ')' || ch === ']') depth = Math.max(0, depth - 1);
    if (ch === ',' && depth === 0) {
      out.push(cur.trim());
      cur = '';
      continue;
    }
    cur += ch;
  }
  out.push(cur.trim());
  return out.filter((x) => x !== '');
}

const BORDER_SUBS = ['width', 'style', 'color'] as const;
type BorderSub = (typeof BORDER_SUBS)[number];
const BORDER_STYLES = new Set([
  'none',
  'hidden',
  'dotted',
  'dashed',
  'solid',
  'double',
  'groove',
  'ridge',
  'inset',
  'outset',
]);
const BORDER_WIDTH_KEYWORDS = new Set(['thin', 'medium', 'thick']);
const isLength = (v: string) => /^[-+]?(\d*\.)?\d+(px|em|rem|%|vh|vw|cqw|cqh|pt|ch|ex)?$/.test(v) || /^calc\(/i.test(v) || /^var\(/i.test(v);

/** 一望可知是颜色的记号。`var()` 单独判（它既可能是颜色也可能是别的，见各调用点的注释）。 */
const isColorToken = (v: string) =>
  /^(#|hsla?\(|rgba?\(|color\(|oklch\(|oklab\(|lab\(|lch\()/i.test(v) ||
  /^(transparent|currentcolor)$/i.test(v);
/** 一望可知是图片的记号。 */
const isImageToken = (v: string) =>
  /^none$/i.test(v) || /^(url|image-set|-webkit-image-set)\(/i.test(v) || /(^|-)gradient\(/i.test(v);

/**
 * `border: 1px solid hsl(var(--x))` / `outline: 2px solid currentColor` → `{width, style, color}`。
 *
 * 分不了类的分量**抛** —— 静默丢掉一个分量就是「同族属性全盲」那条缺陷的原样复制。
 * `var()` 既可能是宽度也可能是颜色 ⇒ 按位置：能判成 style / 长度关键字的先摘走，剩下的按
 * 「先 width 后 color」的书写惯例分配；两个都判不了时抛。
 * **缺席的分量由调用方补 [`INITIAL`]**（简写复位语义，见该常量的注释）。
 */
function parseLineShorthand(
  value: string,
  where: string,
  label: string,
  extraStyles: ReadonlySet<string> = new Set(),
): Partial<Record<BorderSub, string>> {
  const parts = topLevelParts(value);
  if (parts.length === 0 || parts.length > 3) {
    throw new Error(`${where} 的 \`${label}\` 简写分量数 ${parts.length} 未建模：\`${value}\``);
  }
  const out: Partial<Record<BorderSub, string>> = {};
  const rest: string[] = [];
  for (const p of parts) {
    const low = p.toLowerCase();
    if ((BORDER_STYLES.has(low) || extraStyles.has(low)) && out.style === undefined) out.style = p;
    else if (BORDER_WIDTH_KEYWORDS.has(low) && out.width === undefined) out.width = p;
    else rest.push(p);
  }
  for (const p of rest) {
    if (out.width === undefined && isLength(p) && !/^var\(|^hsl|^rgb|^#/i.test(p)) out.width = p;
    else if (out.color === undefined) out.color = p;
    else {
      throw new Error(`${where} 的 \`${label}\` 简写分量 \`${p}\` 分不了类：\`${value}\``);
    }
  }
  return out;
}

/**
 * 已建模族的命名空间：属性名落在这些**前缀**下、却分不了类 ⇒ **抛**。
 *
 * 这条比表本身重要：表一定会写漏，漏了必须当场红而不是静默放行 —— 2026-09-05 两轮验收里
 * 全部 major 的共同形状就是「表里没有 ⇒ 判据静默失明」。
 *
 * **判据是前缀不是首段**（`prop === P` 或 `prop` 以 `P-` 开头）。按首段判够不着 `text-decoration`
 * （首段 `text`）、`list-style`（首段 `list`）、`place-items`（首段 `place`）这一族 —— 那正是
 * MAJ-5 的成因：`background` 与 `background-color` 落成两个互不相干的键，兜底守卫一句话都不说。
 * 按首段登记 `text` 又会把 `text-align` / `text-overflow` 这些无关属性一起拖进来。
 */
const MODELED_NAMESPACE: readonly string[] = [
  'padding',
  'margin',
  'border',
  'inset',
  'gap',
  'width',
  'height',
  'min',
  'max',
  'inline',
  'block',
  'row',
  'column',
  'top',
  'right',
  'bottom',
  'left',
  // ── 2026-09-05 MAJ-5 补齐的带简写的族 ──────────────────────────────────
  'background',
  'flex',
  'font',
  'overflow',
  'outline',
  'list-style',
  'text-decoration',
  'place',
  'align',
  'justify',
];

const inModeledNamespace = (lower: string) =>
  MODELED_NAMESPACE.some((ns) => lower === ns || lower.startsWith(`${ns}-`));

/**
 * 落在已建模命名空间、但**明确不进族模型**的属性。
 *
 * 白名单纪律（抄 `contracts/test-only-exports.test.ts`）：每条**必须恰好在 corpus 里命中至少一次**，
 * 命中 0 次 = 守的东西没了、条目成免死金牌 —— 由 `css-cascade.test.ts` 的自检钉住。
 */
export const OUT_OF_FAMILY_MODEL: readonly string[] = [
  'border-radius',
  'border-top-left-radius',
  'border-top-right-radius',
  'border-bottom-left-radius',
  'border-bottom-right-radius',
  'border-collapse',
  'border-spacing',
  'column-count',
  // `overflow` 简写不含它（名字像而已）：它管的是换行，不是溢出。
  'overflow-wrap',
  // `outline` 简写不含它（CSS UI 3 明列在简写之外）。
  'outline-offset',
  // 本模块的 `font` 简写只折算 Fonts 3 的那几个分量；Fonts 4 才把它列进复位面。
  // 明说不建模 ⇒ 它与 `font` 简写之间的互覆本模块**不报**，那一族归 `declaringSites` 穷举腿。
  'font-synthesis',
];
const OUT_OF_FAMILY = new Set(OUT_OF_FAMILY_MODEL);

type KeyMap = Map<string, string>;

const put = (m: KeyMap, key: string, value: string | null) => {
  if (value !== null) m.set(key, value);
};

/**
 * 一条声明贡献的**全部**语义分量 → 值。
 *
 * 键是语义分量不是 CSS 属性名：`padding-<edge>` / `margin-<edge>` / `inset-<edge>` /
 * `border-<edge>-<width|style|color>` / `size-<inline|block>` / `min-size-*` / `max-size-*` /
 * `gap-<row|column>`；未登记的属性名走**原样直判**（键 = 属性名），保持既有行为不变。
 */
export function expandDecl(decl: Decl, env: Env = DEFAULT_ENV): KeyMap {
  const m: KeyMap = new Map();
  const { prop, value } = decl;
  const at = `${decl.file}:${decl.line}`;
  if (prop.startsWith('--')) {
    m.set(prop, value);
    return m;
  }
  const head = prop.split('-')[0];
  const lower = prop.toLowerCase();

  // ── padding / margin ───────────────────────────────────────────────────────
  const boxFamily = /^(padding|margin)(?:-(.+))?$/.exec(lower);
  if (boxFamily !== null) {
    const fam = boxFamily[1];
    const tail = boxFamily[2];
    if (tail === undefined) {
      for (const e of EDGES) put(m, `${fam}-${e}`, shorthandComponent(value, e));
      return m;
    }
    if (EDGES.includes(tail as Edge)) {
      m.set(`${fam}-${tail}`, value);
      return m;
    }
    if (tail === 'block' || tail === 'inline') {
      const p = topLevelParts(value);
      if (p.length < 1 || p.length > 2) throw new Error(`${at} \`${prop}\` 分量数未建模：\`${value}\``);
      m.set(`${fam}-${physicalEdge(`${tail}-start`, env)}`, p[0]);
      m.set(`${fam}-${physicalEdge(`${tail}-end`, env)}`, p[p.length - 1]);
      return m;
    }
    if (/^(block|inline)-(start|end)$/.test(tail)) {
      m.set(`${fam}-${physicalEdge(tail, env)}`, value);
      return m;
    }
    throw new Error(unmodeled(at, prop));
  }

  // ── inset（含 top/right/bottom/left 长写） ─────────────────────────────────
  if (EDGES.includes(lower as Edge)) {
    m.set(`inset-${lower}`, value);
    return m;
  }
  const insetFamily = /^inset(?:-(.+))?$/.exec(lower);
  if (insetFamily !== null) {
    const tail = insetFamily[1];
    if (tail === undefined) {
      for (const e of EDGES) put(m, `inset-${e}`, shorthandComponent(value, e));
      return m;
    }
    if (tail === 'block' || tail === 'inline') {
      const p = topLevelParts(value);
      if (p.length < 1 || p.length > 2) throw new Error(`${at} \`${prop}\` 分量数未建模：\`${value}\``);
      m.set(`inset-${physicalEdge(`${tail}-start`, env)}`, p[0]);
      m.set(`inset-${physicalEdge(`${tail}-end`, env)}`, p[p.length - 1]);
      return m;
    }
    if (/^(block|inline)-(start|end)$/.test(tail)) {
      m.set(`inset-${physicalEdge(tail, env)}`, value);
      return m;
    }
    throw new Error(unmodeled(at, prop));
  }

  // ── border ────────────────────────────────────────────────────────────────
  if (head === 'border' && !OUT_OF_FAMILY.has(lower)) {
    const tail = lower === 'border' ? '' : lower.slice('border-'.length);
    const sub = BORDER_SUBS.find((s) => tail === s || tail.endsWith(`-${s}`));
    const sideText = sub === undefined ? tail : tail.slice(0, tail.length - sub.length).replace(/-$/, '');
    const sides: Edge[] | null =
      sideText === ''
        ? [...EDGES]
        : EDGES.includes(sideText as Edge)
          ? [sideText as Edge]
          : /^(block|inline)-(start|end)$/.test(sideText)
            ? [physicalEdge(sideText, env)]
            : sideText === 'block' || sideText === 'inline'
              ? [physicalEdge(`${sideText}-start`, env), physicalEdge(`${sideText}-end`, env)]
              : null;
    if (sides === null) throw new Error(unmodeled(at, prop));
    const axisPair = sideText === 'block' || sideText === 'inline';
    if (sub === undefined) {
      // 边简写：`border` / `border-top` / `border-inline-start` → width/style/color
      // 缺席分量写 `initial`：`border: none` 只贡献 style，宽度**复位**，不是「没说」。
      const got = parseLineShorthand(value, at, prop);
      for (const e of sides) for (const s of BORDER_SUBS) m.set(`border-${e}-${s}`, got[s] ?? INITIAL);
      return m;
    }
    if (sideText === '') {
      // `border-color: a b c d` 之类：四值 TRBL
      for (const e of EDGES) put(m, `border-${e}-${sub}`, shorthandComponent(value, e));
      return m;
    }
    if (axisPair) {
      const p = topLevelParts(value);
      if (p.length < 1 || p.length > 2) throw new Error(`${at} \`${prop}\` 分量数未建模：\`${value}\``);
      m.set(`border-${sides[0]}-${sub}`, p[0]);
      m.set(`border-${sides[1]}-${sub}`, p[p.length - 1]);
      return m;
    }
    m.set(`border-${sides[0]}-${sub}`, value);
    return m;
  }

  // ── background（2026-09-05 MAJ-5） ────────────────────────────────────────
  // corpus 里 1551 条全是简写、一条长写都没有 ⇒ 迁移前 `background` 与 `background-color`
  // 落成两个互不相干的键：补一句 `background-color` 就能把盯着 `background` 的判据绕过去。
  const BG_SUBS = [
    'background-image',
    'background-position',
    'background-size',
    'background-repeat',
    'background-attachment',
    'background-origin',
    'background-clip',
    'background-color',
  ] as const;
  if (lower === 'background') {
    if (CSS_WIDE.has(value.toLowerCase())) {
      for (const k of BG_SUBS) m.set(k, value);
      return m;
    }
    const layers = topLevelCommaParts(value);
    const images: string[] = [];
    const got = new Map<string, string>();
    for (const [i, layer] of layers.entries()) {
      const layerImages: string[] = [];
      for (const part of topLevelParts(layer)) {
        const low = part.toLowerCase();
        if (isImageToken(part)) layerImages.push(part);
        else if (/^(repeat|repeat-x|repeat-y|no-repeat|space|round)$/.test(low)) got.set('background-repeat', part);
        else if (/^(scroll|fixed|local)$/.test(low)) got.set('background-attachment', part);
        else if (/^(border-box|padding-box|content-box|text)$/.test(low))
          got.set(got.has('background-origin') ? 'background-clip' : 'background-origin', part);
        else if (isColorToken(part) || /^var\(/i.test(part)) {
          // `var()` 在单分量 background 里按颜色算（corpus 实测 `background: var(--surface)` 就是底色）。
          // 判错的代价是把一条底色当图片；判不了的形态（多分量里混 `var()`）走下面的抛。
          if (i !== layers.length - 1 || got.has('background-color'))
            throw new Error(
              `${at} 的 \`background\` 里出现第二个颜色分量或颜色写在非末层：\`${value}\`。` +
                `层与颜色的对应关系判不了 ⇒ 抛，别给出反的答案。`,
            );
          got.set('background-color', part);
        } else {
          throw new Error(
            `${at} 的 \`background\` 简写分量 \`${part}\` 分不了类：\`${value}\`。` +
              `位置/尺寸（\`center / 50% 50% / cover\`）本模块不建模 —— 登记进族表或拆成长写。`,
          );
        }
      }
      if (layerImages.length > 0) images.push(layerImages.join(' '));
    }
    if (images.length > 0) got.set('background-image', images.join(', '));
    for (const k of BG_SUBS) m.set(k, got.get(k) ?? INITIAL);
    return m;
  }
  if (BG_SUBS.includes(lower as (typeof BG_SUBS)[number])) {
    m.set(lower, value);
    return m;
  }

  // ── flex / flex-flow ──────────────────────────────────────────────────────
  if (lower === 'flex') {
    const low = value.toLowerCase();
    if (CSS_WIDE.has(low)) {
      for (const k of ['flex-grow', 'flex-shrink', 'flex-basis']) m.set(k, value);
      return m;
    }
    // 简写的缺省不是各分量的初值，是规范另给的一组（`flex: 1` ≡ `1 1 0%`）—— 照规范写。
    if (low === 'none') {
      m.set('flex-grow', '0');
      m.set('flex-shrink', '0');
      m.set('flex-basis', 'auto');
      return m;
    }
    const p = topLevelParts(value);
    if (p.length < 1 || p.length > 3) throw new Error(`${at} \`flex\` 分量数未建模：\`${value}\``);
    const isNum = (v: string) => /^[-+]?(\d*\.)?\d+$/.test(v);
    if (p.length === 1) {
      if (isNum(p[0])) {
        m.set('flex-grow', p[0]);
        m.set('flex-shrink', '1');
        m.set('flex-basis', '0%');
      } else {
        m.set('flex-grow', '1');
        m.set('flex-shrink', '1');
        m.set('flex-basis', p[0]);
      }
      return m;
    }
    if (!isNum(p[0])) throw new Error(`${at} \`flex\` 的第一个分量不是 grow：\`${value}\``);
    m.set('flex-grow', p[0]);
    if (p.length === 3) {
      if (!isNum(p[1])) throw new Error(`${at} \`flex\` 的第二个分量不是 shrink：\`${value}\``);
      m.set('flex-shrink', p[1]);
      m.set('flex-basis', p[2]);
      return m;
    }
    if (isNum(p[1])) {
      m.set('flex-shrink', p[1]);
      m.set('flex-basis', '0%');
    } else {
      m.set('flex-shrink', '1');
      m.set('flex-basis', p[1]);
    }
    return m;
  }
  if (lower === 'flex-grow' || lower === 'flex-shrink' || lower === 'flex-basis') {
    m.set(lower, value);
    return m;
  }
  const FLEX_DIRS = /^(row|row-reverse|column|column-reverse)$/;
  const FLEX_WRAPS = /^(nowrap|wrap|wrap-reverse)$/;
  if (lower === 'flex-flow') {
    const p = topLevelParts(value);
    if (p.length < 1 || p.length > 2) throw new Error(`${at} \`flex-flow\` 分量数未建模：\`${value}\``);
    const got = new Map<string, string>();
    for (const x of p) {
      if (FLEX_DIRS.test(x.toLowerCase())) got.set('flex-direction', x);
      else if (FLEX_WRAPS.test(x.toLowerCase())) got.set('flex-wrap', x);
      else throw new Error(`${at} \`flex-flow\` 分量 \`${x}\` 分不了类：\`${value}\``);
    }
    for (const k of ['flex-direction', 'flex-wrap']) m.set(k, got.get(k) ?? INITIAL);
    return m;
  }
  if (lower === 'flex-direction' || lower === 'flex-wrap') {
    m.set(lower, value);
    return m;
  }

  // ── overflow ──────────────────────────────────────────────────────────────
  const OVERFLOW_VALUES = /^(visible|hidden|clip|scroll|auto|overlay)$/;
  if (lower === 'overflow') {
    const p = topLevelParts(value);
    if (p.length < 1 || p.length > 2) throw new Error(`${at} \`overflow\` 分量数未建模：\`${value}\``);
    for (const x of p)
      if (!OVERFLOW_VALUES.test(x.toLowerCase()) && !CSS_WIDE.has(x.toLowerCase()))
        throw new Error(`${at} \`overflow\` 分量 \`${x}\` 未建模：\`${value}\``);
    m.set('overflow-x', p[0]);
    m.set('overflow-y', p[p.length - 1]);
    return m;
  }
  if (lower === 'overflow-x' || lower === 'overflow-y') {
    m.set(lower, value);
    return m;
  }
  // 逻辑写法：horizontal-tb 下 inline ≡ x、block ≡ y（与 size 那族同一条轴映射）。
  if (lower === 'overflow-inline' || lower === 'overflow-block') {
    m.set(lower === 'overflow-inline' ? 'overflow-x' : 'overflow-y', value);
    return m;
  }

  // ── outline（与 border 同形，但 `auto` 也是合法 style） ────────────────────
  if (lower === 'outline' && !OUT_OF_FAMILY.has(lower)) {
    if (CSS_WIDE.has(value.toLowerCase())) {
      for (const sub of BORDER_SUBS) m.set(`outline-${sub}`, value);
      return m;
    }
    const got = parseLineShorthand(value, at, prop, new Set(['auto']));
    for (const sub of BORDER_SUBS) m.set(`outline-${sub}`, got[sub] ?? INITIAL);
    return m;
  }
  if (BORDER_SUBS.some((sub) => lower === `outline-${sub}`)) {
    m.set(lower, value);
    return m;
  }

  // ── text-decoration ───────────────────────────────────────────────────────
  const TD_SUBS = [
    'text-decoration-line',
    'text-decoration-style',
    'text-decoration-color',
    'text-decoration-thickness',
  ] as const;
  if (lower === 'text-decoration') {
    if (CSS_WIDE.has(value.toLowerCase())) {
      for (const k of TD_SUBS) m.set(k, value);
      return m;
    }
    const got = new Map<string, string>();
    const lines: string[] = [];
    for (const part of topLevelParts(value)) {
      const low = part.toLowerCase();
      if (/^(none|underline|overline|line-through|blink|spelling-error|grammar-error)$/.test(low))
        lines.push(part);
      else if (/^(solid|double|dotted|dashed|wavy)$/.test(low)) got.set('text-decoration-style', part);
      else if (/^(auto|from-font)$/.test(low) || isLength(part))
        got.set('text-decoration-thickness', part);
      else if (isColorToken(part) || /^var\(/i.test(part)) got.set('text-decoration-color', part);
      else throw new Error(`${at} \`text-decoration\` 分量 \`${part}\` 分不了类：\`${value}\``);
    }
    if (lines.length > 0) got.set('text-decoration-line', lines.join(' '));
    for (const k of TD_SUBS) m.set(k, got.get(k) ?? INITIAL);
    return m;
  }
  if (TD_SUBS.includes(lower as (typeof TD_SUBS)[number])) {
    m.set(lower, value);
    return m;
  }

  // ── list-style ────────────────────────────────────────────────────────────
  const LS_SUBS = ['list-style-type', 'list-style-position', 'list-style-image'] as const;
  if (lower === 'list-style') {
    if (CSS_WIDE.has(value.toLowerCase())) {
      for (const k of LS_SUBS) m.set(k, value);
      return m;
    }
    const got = new Map<string, string>();
    for (const part of topLevelParts(value)) {
      const low = part.toLowerCase();
      if (low === 'none') {
        // 规范：`list-style: none` 里第一个 `none` 同时落到 type 与 image。
        got.set('list-style-type', part);
        got.set('list-style-image', part);
      } else if (/^(inside|outside)$/.test(low)) got.set('list-style-position', part);
      else if (isImageToken(part)) got.set('list-style-image', part);
      else if (/^[\w-]+$/.test(part) || /^["']/.test(part)) got.set('list-style-type', part);
      else throw new Error(`${at} \`list-style\` 分量 \`${part}\` 分不了类：\`${value}\``);
    }
    for (const k of LS_SUBS) m.set(k, got.get(k) ?? INITIAL);
    return m;
  }
  if (LS_SUBS.includes(lower as (typeof LS_SUBS)[number])) {
    m.set(lower, value);
    return m;
  }

  // ── place-* / align-* / justify-* ─────────────────────────────────────────
  const placeFamily = /^place-(items|content|self)$/.exec(lower);
  if (placeFamily !== null) {
    const p = topLevelParts(value);
    if (p.length < 1 || p.length > 2) throw new Error(`${at} \`${prop}\` 分量数未建模：\`${value}\``);
    m.set(`align-${placeFamily[1]}`, p[0]);
    m.set(`justify-${placeFamily[1]}`, p[p.length - 1]);
    return m;
  }
  if (/^(align|justify)-(items|content|self)$/.test(lower)) {
    m.set(lower, value);
    return m;
  }

  // ── font ──────────────────────────────────────────────────────────────────
  const FONT_SUBS = [
    'font-style',
    'font-weight',
    'font-stretch',
    'font-size',
    'line-height',
    'font-family',
    'font-variant-numeric',
  ] as const;
  if (lower === 'font') {
    if (CSS_WIDE.has(value.toLowerCase())) {
      for (const k of FONT_SUBS) m.set(k, value);
      return m;
    }
    const got = new Map<string, string>();
    const parts = topLevelCommaParts(value);
    const head = topLevelParts(parts[0]);
    let i = 0;
    for (; i < head.length; i += 1) {
      const low = head[i].toLowerCase();
      if (/^(normal|italic|oblique)$/.test(low)) got.set('font-style', head[i]);
      else if (/^(bold|bolder|lighter|[1-9]00|\d{2,3})$/.test(low)) got.set('font-weight', head[i]);
      else if (/^(ultra-|extra-|semi-)?(condensed|expanded)$/.test(low)) got.set('font-stretch', head[i]);
      else break;
    }
    if (i >= head.length) {
      throw new Error(
        `${at} \`font\` 简写里读不出 \`<size>\`：\`${value}\`。` +
          `系统字体关键字（\`caption\` / \`menu\` …）本模块不建模 —— 遇到即抛，不给出错误答案。`,
      );
    }
    const sizePart = head[i];
    i += 1;
    const slash = sizePart.indexOf('/');
    if (slash >= 0) {
      got.set('font-size', sizePart.slice(0, slash));
      got.set('line-height', sizePart.slice(slash + 1));
    } else {
      got.set('font-size', sizePart);
    }
    const family = [head.slice(i).join(' '), ...parts.slice(1)].filter((x) => x !== '').join(', ');
    if (family === '') throw new Error(`${at} \`font\` 简写里读不出 \`<family>\`：\`${value}\``);
    got.set('font-family', family);
    for (const k of FONT_SUBS) m.set(k, got.get(k) ?? INITIAL);
    return m;
  }
  if (FONT_SUBS.includes(lower as (typeof FONT_SUBS)[number])) {
    m.set(lower, value);
    return m;
  }

  // ── gap ───────────────────────────────────────────────────────────────────
  if (lower === 'gap') {
    const p = topLevelParts(value);
    if (p.length < 1 || p.length > 2) throw new Error(`${at} \`gap\` 分量数未建模：\`${value}\``);
    m.set('gap-row', p[0]);
    m.set('gap-column', p[p.length - 1]);
    return m;
  }
  if (lower === 'row-gap' || lower === 'column-gap') {
    m.set(`gap-${lower.slice(0, lower.indexOf('-'))}`, value);
    return m;
  }

  // ── size（轴映射：horizontal-tb 下 inline≡width / block≡height） ───────────
  const size = /^(?:(min|max)-)?(width|height|inline-size|block-size)$/.exec(lower);
  if (size !== null) {
    const axis = size[2] === 'width' || size[2] === 'inline-size' ? 'inline' : 'block';
    m.set(`${size[1] === undefined ? '' : `${size[1]}-`}size-${axis}`, value);
    return m;
  }

  if (inModeledNamespace(lower) && !OUT_OF_FAMILY.has(lower)) throw new Error(unmodeled(at, prop));

  m.set(prop, value);
  return m;
}

function unmodeled(at: string, prop: string): string {
  return (
    `${at} 的属性 \`${prop}\` 落在已建模族的命名空间内却分不了类。` +
    `静默放行 = 盯着这一族的判据当场失明（本轮 7 条 major 就是这个形状）。` +
    `要么把它登记进 \`expandDecl\` 的族表，要么写进 \`OUT_OF_FAMILY_MODEL\` 明说它不进族模型。`
  );
}

/**
 * 把**查询用**的属性名折成语义分量键。
 *
 * 跨多个分量的简写（`padding` / `gap` / `border-top` …）拿来查是**有歧义**的 ⇒ 抛并点名可选分量，
 * 逼调用方显式选一个。这不是刁难：`cssValue('.x','gap')` 这种写法正是「同族全盲」那条缺陷的入口。
 */
export function keyOf(prop: string, env: Env = DEFAULT_ENV): string {
  // 探针值要能让简写把**全部**分量都展开出来，否则歧义判不出来：
  // `border-top` 用 `0` 探只会展开出一个 width 分量，而它在语义上恒跨 width/style/color 三格
  //（写不全的那几格回落到 initial —— 本解析器明说不做 initial 补齐，所以它仍然是歧义查询）。
  const lower = prop.toLowerCase();
  const isBorderEdgeShorthand =
    lower.split('-')[0] === 'border' &&
    !OUT_OF_FAMILY.has(lower) &&
    !BORDER_SUBS.some((sub) => lower === `border-${sub}` || lower.endsWith(`-${sub}`));
  // 各族的探针值：拿 `0` 探 `background` 只会当场「分不了类」而抛，报文指错方向 ——
  // 那些简写的**歧义**才是要报的东西，故各给一个能把全部分量展开出来的合法值。
  const PROBE: Readonly<Record<string, string>> = {
    background: '#000',
    flex: '1 1 auto',
    'flex-flow': 'row wrap',
    font: '1px x',
    overflow: 'hidden',
    outline: '1px solid red',
    'text-decoration': 'underline solid red 1px',
    'list-style': 'none inside',
    'place-items': 'center',
    'place-content': 'center',
    'place-self': 'center',
  };
  const probe: Decl = {
    file: '<query>',
    line: 0,
    order: 0,
    sels: [],
    rawSels: [],
    prop,
    value: isBorderEdgeShorthand ? '1px solid red' : (PROBE[lower] ?? '0'),
    important: false,
    conds: [],
    registered: false,
  };
  const m = expandDecl(probe, env);
  const keys = [...m.keys()];
  if (keys.length === 1) return keys[0];
  throw new Error(
    `\`${prop}\` 是跨 ${keys.length} 个语义分量的简写，拿来查有歧义。` +
      `显式选一个分量：${keys.join(' / ')}（例如 \`gap\` → \`row-gap\` / \`column-gap\`，` +
      `\`border-top\` → \`border-top-width\` / \`border-top-style\` / \`border-top-color\`）。`,
  );
}

// ════════════════════════════════════════════════════════════════════════════
// ⑤ 查询
// ════════════════════════════════════════════════════════════════════════════

export interface Contribution {
  decl: Decl;
  /** 该声明为查询键贡献的值（简写已展开到分量）。 */
  value: string;
}

export interface Resolution {
  /** 查询选择器（原文）。 */
  sel: string;
  /** 查询选择器的归一化键。 */
  selKey: string;
  /** 查询属性原文。 */
  prop: string;
  /** 折算后的语义分量键。 */
  key: string;
  env: Env;
  /** 同键下对该分量有贡献的**全部**声明，按进包顺序。 */
  chain: Contribution[];
  /** `!important` 优先、其次源序的最后一条。 */
  winner: Contribution | null;
  /** 「有人从更窄处压你」——不给值，只自曝。 */
  narrower: Contribution[];
  /**
   * 「更宽的选择器**供着**一个本选择器没声明的分量」（R5-03）。
   *
   * `chain` 为空而本桶非空 ⇒ 浏览器里这个元素**确实有**这个属性，值来自更宽那条规则。
   * 「`.mr-ap-body.dim` 没声明 `pointer-events`」与「置灰的正文点得动」是两件事。
   */
  broaderSupplied: Contribution[];
}

export interface Query {
  sel: string;
  prop: string;
  /** 四个层叠上下文之一；与 `decls` 二选一，必须显式选。 */
  ctx?: CtxName;
  decls?: CssContext;
  env?: Env;
  /** 条件面谓词，默认 [`UNCONDITIONAL`]。 */
  where?: (d: Decl) => boolean;
}

/** 只看无条件声明（默认）。条件块内的声明只打标签，不参与无条件查询的胜负。 */
export const UNCONDITIONAL = (d: Decl) => d.conds.length === 0;
/** 全看（含 `@media` / `@container` 内）。注意：解析器**不判两个条件块能否同时成立**。 */
export const ALL = () => true;
/** 只看条件 prelude 命中 `re` 的那批。 */
export const inCondition = (re: RegExp) => (d: Decl) => d.conds.some((c) => re.test(c));

function ctxOf(q: Query): CssContext {
  if (q.decls !== undefined) return q.decls;
  if (q.ctx !== undefined) return context(q.ctx);
  throw new Error('查询必须显式给 `ctx`（desktop/mobile/tray/popup）或 `decls`（内存上下文）。');
}

/**
 * 解析一个选择器某个语义分量的层叠结果。
 *
 * 故障关闭三条（2026-09-05 第三轮验收，全部实测过能溜过去的变异）：
 *  1. 选择器在整个上下文里一条规则都没命中 ⇒ 抛。空取材面上的否定断言恒真。
 *  2. **更宽的选择器带 `!important`** 压下来 ⇒ 抛。层叠里 important 排在特异度**之前**，
 *     更宽 + important 在本查询命中的每个元素上必胜 ⇒ 本函数给不出对的答案（BLK-1）。
 *  3. 调用方**没显式给 `where`**、而同键上还有**条件块里的声明** ⇒ 抛并点名 file:line + prelude。
 *     默认谓词 [`UNCONDITIONAL`] 会把 `@media screen` / `@supports (display:flex)` /
 *     `@media (min-width:0)` 这类**恒真条件**整块滤掉，浏览器按源序后者胜 ⇒ 模型给出反的答案（BLK-2）。
 *     要放行就显式传 `where: UNCONDITIONAL`（「我只看基础档」是一句要写出来的话，不是默认沉默）。
 */
export function resolve(q: Query): Resolution {
  const ctx = ctxOf(q);
  const env = q.env ?? DEFAULT_ENV;
  const whereGiven = q.where !== undefined;
  const where = q.where ?? UNCONDITIONAL;
  const query = parseSelector(q.sel);
  const key = keyOf(q.prop, env);

  const everMatched = ctx.decls.some((d) => d.sels.includes(query.key));
  if (!everMatched) {
    throw new Error(
      `选择器 \`${q.sel}\`（键 \`${query.key}\`）在上下文 \`${ctx.name}\` 里一条规则都没命中。` +
        `取材面为空 ⇒ 其上的否定断言恒真。检查选择器是否写错、或该规则是否已被搬走/改名。`,
    );
  }

  const chain: Contribution[] = [];
  const narrower: Contribution[] = [];
  const broaderImportant: Contribution[] = [];
  const broaderSupplied: Contribution[] = [];
  const equalSpecificityNarrower: Contribution[] = [];
  const hiddenConditional: Contribution[] = [];
  let sawDirSelector = false;
  for (const d of ctx.decls) {
    if (d.registered) continue;
    const sameKey = d.sels.includes(query.key);
    // `:is()` 先拆成分支再判（R5-05）：纯语法超集判定对 `:is(.a,.b)` 是瞎的。
    const narrowSels = sameKey ? [] : d.rawSels.filter((s) => anyAlternative(s, query, isNarrower));
    const broadSels =
      sameKey || narrowSels.length > 0
        ? []
        : d.rawSels.filter((s) => anyAlternative(s, query, isBroader));
    // 特异度打平而匹配集收窄的那一族（`.a:where(.b)` 对着 `.a`）：既不更窄也不更宽，
    // 上一版在这里被 `continue` 静默丢掉，而它在浏览器里按源序翻转胜出者。
    const tieSels =
      sameKey || narrowSels.length > 0 || broadSels.length > 0
        ? []
        : d.rawSels.filter((s) => anyAlternative(s, query, narrowsWithoutSpecificity));
    if (!sameKey && narrowSels.length === 0 && broadSels.length === 0 && tieSels.length === 0) continue;
    const v = expandDecl(d, env).get(key);
    if (v === undefined) continue;
    if (!where(d)) {
      // 被默认谓词滤掉的条件声明：同键的、以及**更窄/更宽 important** 的候选都要自曝 ——
      // 只看同键的话，`@media screen{ .mn-row .mn-lat.fast{ … } }` 这种「条件档里的更窄覆写」
      // 会同时从 narrower 桶和本闸门底下溜过去（BLK-2 与 MAJ-3 叠在一起的那条缝）。
      if (
        !whereGiven &&
        d.conds.length > 0 &&
        (sameKey || narrowSels.length > 0 || tieSels.length > 0 || d.important)
      )
        hiddenConditional.push({ decl: d, value: v });
      continue;
    }
    if (tieSels.length > 0) {
      equalSpecificityNarrower.push({ decl: d, value: v });
      continue;
    }
    if (broadSels.length > 0) {
      // 更宽且 important ⇒ 必胜、模型给不出对的答案。
      if (d.important) broaderImportant.push({ decl: d, value: v });
      // 更宽且非 important ⇒ **有前提地**丢弃：前提是本选择器自己也声明了这个分量
      //（那时特异度更高必胜）。本选择器没声明时，浏览器里生效的就是更宽那条（R5-03：
      // `.mr-ap-body{ pointer-events: none }` vs `.mr-ap-body.dim`）—— 那条注释此前把一个
      // 有前提的规则写成了不变量。收集起来，链为空时由 `winnerValue` 点名。
      else broaderSupplied.push({ decl: d, value: v });
      continue;
    }
    if (d.rawSels.some((s) => parseSelector(s).hasDir)) sawDirSelector = true;
    (sameKey ? chain : narrower).push({ decl: d, value: v });
  }
  failClosedOnBroaderImportant(q.sel, q.prop, key, broaderImportant);
  failClosedOnEqualSpecificityNarrower(q.sel, q.prop, key, equalSpecificityNarrower);
  failClosedOnHiddenConditional(q.sel, q.prop, hiddenConditional);
  if (sawDirSelector && q.env === undefined && isDirSensitiveKey(key)) {
    throw new Error(
      `查询 \`${q.sel}\` 的 \`${q.prop}\`（键 \`${key}\`）与书写方向有关，而链里出现了含 \`[dir=\` 的选择器 —— ` +
        `本仓 RTL 是活的（\`styles/components.css\` 与 \`styles/prototype.css\` 各有一条 \`:root[dir="rtl"] .main\`）。` +
        `显式给 \`env: { dir: 'ltr' }\` 或 \`{ dir: 'rtl' }\`，别让 \`inline-start → left\` 变成无条件常量。`,
    );
  }

  chain.sort((a, b) => a.decl.order - b.decl.order);
  narrower.sort((a, b) => a.decl.order - b.decl.order);
  const important = chain.filter((c) => c.decl.important);
  const pool = important.length > 0 ? important : chain;
  return {
    sel: query.raw,
    selKey: query.key,
    prop: q.prop,
    key,
    env,
    chain,
    winner: pool.length === 0 ? null : pool[pool.length - 1],
    narrower,
    broaderSupplied,
  };
}

/** 贡献的落点渲染（报错消息共用）。 */
const siteOf = (c: Contribution) =>
  `${c.decl.file}:${c.decl.line}  ${c.decl.rawSels.join(', ')} { ${c.decl.prop}: ${c.decl.value}` +
  `${c.decl.important ? ' !important' : ''} }  ⇒ ${c.value}` +
  `${c.decl.conds.length === 0 ? '' : `   [${c.decl.conds.join(' « ')}]`}`;

/** BLK-1：更宽的选择器带 `!important` 压下来 ⇒ 本查询给不出对的答案。 */
function failClosedOnBroaderImportant(
  sel: string,
  prop: string,
  key: string,
  hits: readonly Contribution[],
): void {
  if (hits.length === 0) return;
  throw new Error(
    `查询 \`${sel}\` 的 \`${prop}\`（键 \`${key}\`）上有**更宽的选择器**带 \`!important\` 压下来：\n` +
      hits.map((c) => `    ${siteOf(c)}`).join('\n') +
      `\n层叠的比较顺序是「来源与层 → \`!important\` → 特异度 → 源序」—— important 排在特异度**之前**，` +
      `而更宽的候选命中的元素集 ⊇ 本查询的 ⇒ 在本查询命中的每个元素上它都必胜。` +
      `本解析器只在**同一个选择器键内**裁决胜负，故这里当场抛，不给一个反的答案。` +
      `要么去掉那条 \`!important\`，要么改查那个更宽的选择器。`,
  );
}

/**
 * R5-05 姊妹腿：候选**特异度与本查询打平、匹配集却更小**（`.a:where(.b)`、`* .a`）。
 *
 * 同特异度 ⇒ 胜负回到源序；匹配集更小 ⇒ 只在它也命中的那部分元素上翻转。查询的答案于是不是一个值。
 * 本模型只在同一个选择器键内裁决 ⇒ 当场抛，不给一个反的答案。
 */
function failClosedOnEqualSpecificityNarrower(
  sel: string,
  prop: string,
  key: string,
  hits: readonly Contribution[],
): void {
  if (hits.length === 0) return;
  throw new Error(
    `查询 \`${sel}\` 的 \`${prop}\`（键 \`${key}\`）上有**特异度打平、匹配集更小**的候选：\n` +
      hits.map((c) => `    ${siteOf(c)}`).join('\n') +
      `\n\`:where(X)\` 的特异度是 0 但它**参与匹配**（\`* X\` 同理）⇒ 与本查询同特异度、` +
      `胜负回到源序，且只在它也命中的那部分元素上翻转 ⇒ 本查询的答案不是一个值。` +
      `本解析器只在同一个选择器键内裁决，故这里当场抛，不给一个反的答案。` +
      `要问「某个具体元素最终是多少」⇒ 走裁判 \`css-oracle.test-support.ts\`（真 DOM + 整条链）。`,
  );
}

/** BLK-2：调用方没显式给 `where`，而同键上还有条件块里的声明。 */
function failClosedOnHiddenConditional(sel: string, prop: string, hits: readonly Contribution[]): void {
  if (hits.length === 0) return;
  throw new Error(
    `查询 \`${sel}\` 的 \`${prop}\` 没有显式给 \`where\`，而同键上还有**条件块里的声明**：\n` +
      hits.map((c) => `    ${siteOf(c)}`).join('\n') +
      `\n默认谓词 \`UNCONDITIONAL\` 会把它们整块滤掉。而 \`@media screen\` / ` +
      `\`@supports (display:flex)\` / \`@media (min-width:0)\` 这类条件在目标 WebView 里**恒真**，` +
      `浏览器按源序后者胜 ⇒ 滤掉它们就是给出反的答案。` +
      `「只看基础档」是一句要写出来的话：显式传 \`where: UNCONDITIONAL\` 放行，` +
      `或按判据性质改 \`where: ALL\`（否定/穷举判据）/ \`inCondition(re)\`（只看某一档）。`,
  );
}

/** 胜出值。链为空（谁都没声明过这个分量）⇒ 抛。 */
export function winnerValue(q: Query): string {
  const r = resolve(q);
  if (r.winner === null) {
    throw new Error(
      `\`${r.sel}\` 没有任何声明贡献 \`${r.prop}\`（键 \`${r.key}\`）。\n${explain(r)}` +
        (r.broaderSupplied.length === 0
          ? ''
          : `\n  但**更宽的选择器供着它**（浏览器里这个元素确实有这个属性，值来自下面这条）：\n` +
            r.broaderSupplied.map((c) => `    ${siteOf(c)}`).join('\n')),
    );
  }
  return r.winner.value;
}

export interface WinnerSet {
  /** 键 → 胜出贡献（**只收本选择器键自己的声明**）。 */
  won: Map<string, Contribution>;
  /**
   * 「有人从更窄处压你」—— 更窄选择器对**本选择器也声明过的分量**的覆写，不给值，只自曝。
   *
   * 2026-09-05 MAJ-3：这个桶此前只有 [`resolve`] 有，而两道最大的迁移门（nodes ⑫ / rules ⑧）
   * 全程只调 `winners` ⇒ 那个桶对它们等于不存在。现在 `winners` 也算它。
   *
   * ⚠️ **但这不是「类型强制」**：`won` 是 `WinnerSet` 的一个字段，取值的四个入口
   * （[`winnerValue`] / [`winnersText`] / `winners().won` / [`declaringSites`]）
   * 谁都能绕开本桶（R5-10）。承重判据的正解是迁到裁判上，不是指望调用方自觉。
   *
   * 2026-09-05 R5-02 起**不再**要求「与 `won` 重叠」：更窄规则**新增**一个本选择器没写过的属性
   * （`.mr-screen .mr-ap-body.dim{ pointer-events: none }`）正是否定判据守的那一类。
   */
  narrower: Contribution[];
  /**
   * 「更宽的选择器**供着**本选择器没有胜出值的分量」（R5-03）。
   *
   * `.mr-ap-body{ pointer-events: none }` 之于 `.mr-ap-body.dim`：更窄那条自己不声明该属性 ⇒
   * 浏览器里生效的是更宽那条。`won` 里没有它**不等于**元素上没有它 —— 这两句话此前被混成一句。
   */
  broaderSupplied: Contribution[];
}

/**
 * 一个选择器上**每个语义分量的胜出值**。
 *
 * 与 [`resolve`] 的分工：resolve 回答「这一个分量最终是多少」，本函数回答「这个选择器最终长什么样」。
 * 迁移前各门是拿正则切出**首条**同名规则的**体**，然后在那段原文上 `toContain('var(--ok)')` /
 * `not.toContain('var(--flow)')` —— 两个洞叠在一起：首条命中（末尾追加一条就改得回去）
 * 加上「写过就算」（注释里留一份旧的正确规则就能把门喂饱）。改读胜出值之后两个洞一起堵上。
 *
 * 故障关闭与 [`resolve`] 同口径：更宽选择器带 `!important` ⇒ 抛（BLK-1）；
 * 没显式给 `where` 而本选择器上还有条件块里的声明 ⇒ 抛（BLK-2）。
 * 「更窄选择器从旁边压下来」进 [`WinnerSet.narrower`]，由调用方表态。
 */
export function winners(opts: {
  sel: string;
  ctx?: CtxName;
  decls?: CssContext;
  env?: Env;
  where?: (d: Decl) => boolean;
}): WinnerSet {
  const ctx = ctxOf(opts as Query);
  const env = opts.env ?? DEFAULT_ENV;
  const whereGiven = opts.where !== undefined;
  const where = opts.where ?? UNCONDITIONAL;
  const query = parseSelector(opts.sel);
  const hits = ctx.decls.filter((d) => !d.registered && d.sels.includes(query.key));
  if (hits.length === 0) {
    throw new Error(
      `选择器 \`${opts.sel}\`（键 \`${query.key}\`）在上下文 \`${ctx.name}\` 里一条规则都没命中。` +
        `取材面为空 ⇒ 其上的否定断言恒真。`,
    );
  }
  const won = new Map<string, Contribution>();
  const hiddenConditional: Contribution[] = [];
  for (const d of hits) {
    if (!where(d)) {
      if (!whereGiven && d.conds.length > 0)
        hiddenConditional.push({ decl: d, value: `${d.prop}: ${d.value}` });
      continue;
    }
    for (const [key, value] of expandDecl(d, env)) {
      const cur = won.get(key);
      const better =
        cur === undefined ||
        (d.important && !cur.decl.important) ||
        (d.important === cur.decl.important && d.order > cur.decl.order);
      if (better) won.set(key, { decl: d, value });
    }
  }
  failClosedOnHiddenConditional(opts.sel, '<整个选择器>', hiddenConditional);

  // 跨选择器三桶：更窄的进 `narrower`、更宽且 important 的抛、更宽且供着本选择器没写过的分量的
  // 进 `broaderSupplied`。`:is()` 先拆分支再判（R5-05）。
  //
  // 2026-09-05 R5-02：此前这里有一句 `if (!won.has(key)) continue` —— 只报与本选择器**重叠**的
  // 分量，理由写的是「一条只碰别的分量的更窄规则没有覆写任何东西，报它只是噪音」。那句话对
  // **正面取值**判据成立，对**否定**判据正好相反：`.mr-screen .mr-ap-body.dim{ pointer-events: none }`
  // 覆写的正是一个本选择器没写过的属性，而否定判据守的就是这一类。现在更窄的一律报。
  const narrower: Contribution[] = [];
  const broaderSupplied: Contribution[] = [];
  const broaderImportant: Contribution[] = [];
  const hiddenCross: Contribution[] = [];
  for (const d of ctx.decls) {
    if (d.registered || d.sels.includes(query.key)) continue;
    const narrow = d.rawSels.some((r) => anyAlternative(r, query, isNarrower));
    const broad = !narrow && d.rawSels.some((r) => anyAlternative(r, query, isBroader));
    if (!narrow && !broad) continue;
    const expanded = [...expandDecl(d, env)];
    // 更宽且非 important：只在它供着一个本选择器**没有**胜出值的分量时才有话说（R5-03）。
    const pick = narrow || d.important ? expanded[0] : expanded.find(([k]) => !won.has(k));
    if (pick === undefined) continue;
    const value = pick[1];
    // 条件档里的跨选择器覆写被默认谓词滤掉 ⇒ 与同键那一族同样自曝（见 `resolve` 里同名的那段注释）。
    if (!where(d)) {
      if (!whereGiven && d.conds.length > 0) hiddenCross.push({ decl: d, value });
      continue;
    }
    if (narrow) narrower.push({ decl: d, value });
    else if (d.important) broaderImportant.push({ decl: d, value });
    else broaderSupplied.push({ decl: d, value });
  }
  failClosedOnHiddenConditional(opts.sel, '<跨选择器覆写>', hiddenCross);
  failClosedOnBroaderImportant(opts.sel, '<整个选择器>', '<全部分量>', broaderImportant);
  narrower.sort((a, b) => a.decl.order - b.decl.order);
  broaderSupplied.sort((a, b) => a.decl.order - b.decl.order);
  return { won, narrower, broaderSupplied };
}

/** [`WinnerSet.won`] 的文本形态：`键: 值;` 逐条拼接，给 `toContain` / `not.toMatch` 这类判据用。 */
export const winnersText = (opts: Parameters<typeof winners>[0]): string =>
  [...winners(opts).won]
    .map(([k, c]) => `${k}: ${c.value}${c.decl.important ? ' !important' : ''};`)
    .join(' ');

/**
 * **穷举腿**：谁声明过这个分量（不问谁胜出）。
 *
 * 与 [`resolve`] 的分工：resolve 回答「同一个选择器上最终是多少」，declaringSites 回答
 * 「整个上下文里有谁碰过它」—— 后者接住的正是 resolve 契约外的那一族：互不包含但可能命中
 * 同一元素的跨选择器（`#mc-alt-top` vs `.mc-top`）。
 *
 * 自定义属性一并收 `@property` 的 `initial-value`（[`Decl.registered`]）。
 */
export function declaringSites(
  prop: string,
  opts: { ctx?: CtxName; decls?: CssContext; env?: Env; where?: (d: Decl) => boolean },
): Decl[] {
  const ctx = ctxOf(opts as Query);
  const env = opts.env ?? DEFAULT_ENV;
  const where = opts.where ?? ALL;
  const key = keyOf(prop, env);
  const out: Decl[] = [];
  for (const d of ctx.decls) {
    if (!where(d)) continue;
    if (d.registered) {
      if (d.prop === prop) out.push(d);
      continue;
    }
    if (expandDecl(d, env).has(key)) out.push(d);
  }
  return out;
}

/**
 * 渲染 chain / narrower 全表（`file:line` + 值 + important + 条件）。
 *
 * 所有门的报错消息都走它 —— 迁移后各门的红必须**一眼可辨**是「真覆盖」还是「解析器判过头了」。
 */
export function explain(r: Resolution): string {
  const site = siteOf;
  const lines = [
    `选择器 \`${r.sel}\`（键 \`${r.selKey}\`）的 \`${r.prop}\` → 语义分量 \`${r.key}\`，dir=${r.env.dir}`,
    `  链（按进包顺序，important 优先、其次最后一条胜出）：`,
    ...(r.chain.length === 0
      ? ['    （空 —— 没有任何声明贡献这个分量）']
      : r.chain.map((c, i) => `    #${i + 1} ${site(c)}${c === r.winner ? '   ← 胜出' : ''}`)),
  ];
  if (r.narrower.length > 0) {
    lines.push(
      `  更窄选择器（匹配集 ⊆ 本查询、特异度分量单调不减 ⇒ 在它命中的元素上它必胜，与源序无关）：`,
      ...r.narrower.map((c) => `    ${site(c)}`),
    );
  }
  return lines.join('\n');
}
