/**
 * 层叠死规则门 —— 断点块里的声明不许被同文件后面的同名顶层规则盖回去。
 *
 * # 这道门守的是哪一类失效
 *
 * `@container` 与 `@media` 是**条件分组规则**，它们本身不贡献特异性。所以当源码里
 * 先写 `@container mscreen (min-width: 52.5em) { .row { min-height: 64px } }`、
 * 后写 `.row { min-height: 54px }` 时：
 *
 * 两条 `.row` 特异性同为 (0,1,0)、同源同重要度 ⇒ **只按源码顺序决胜**，后写的 54px 永远赢。
 * 断点块里那句 64px 是死代码：值写了、断点匹配了、层叠输了。渲染上完全看不出来，
 * 因为它退化成基础态 —— 一个「没有发生任何事」的失效形态。
 *
 * 这一类回归**已经在本仓发生过两次**（`nodes.css` 的 `.mn-row-main`、
 * `rules-screen.css` 的 `.mr-switch-row`），而当时两个屏各自的门都是绿的：
 * 那些 `declsOf` 式的判据是「用 `matchAll` 找选择器、命中第一条即 return」，
 * 断言的是「某处写过这个值」，不是「层叠之后胜出的是这个值」——
 * 死规则恰好就在文件靠前处，于是首条命中的就是它，门读到 64px，绿。
 *
 * # 判据
 *
 * 扫 `ui/src` 下走目录树发现的全部 `.css`（不写死清单 ⇒ 新建 CSS 文件自动进面）。
 * 对每条**处在 `@container` / `@media` 内**的规则，取它的选择器列表与它声明的属性集 P；
 * 若同文件**后面**存在一条**嵌套深度为 0 的顶层规则**，其选择器列表里有一项与前者的某一项
 * **归一化键相等**，且它也声明了 P 里的某个属性 ⇒ 断点块里那条对该属性是死规则 ⇒ 红。
 *
 * ## 为什么必须是「选择器列表逐项键相等」，不许子串
 *
 * 本门第一版用子串匹配，把 `#s-nodes.nodes-list-view .node-grid` 判成 `.node-grid` 的覆盖 ——
 * 那是特异性高得多的**另一个**选择器，谁胜由特异性决定、与源码顺序无关，判它是覆盖是假阳。
 * 键相等是本门唯一**可靠**的判据面：两个归一化后相同的选择器特异性必然相同，
 * 于是「后者胜」是无条件成立的事实，不需要实现一个特异性计算器（那才是新的错误来源）。
 * 代价是**射程窄**：特异性不同但实际互相覆盖的组合本门看不见（见下「盖不住什么」）。
 *
 * ## `!important` 的四格
 *
 * 决胜先比重要度再比顺序，所以只有「块内 important + 后面顶层非 important」这一格是块内胜出，
 * 不算死规则。其余三格仍是后者胜 ⇒ 仍然红。
 *
 * ## 这道门**盖不住**什么（失效形态，必须知道）
 *
 * 1. **跨文件覆盖**：层叠链由 `index.css` 把多份 CSS 串起来，后引入的文件可以盖前一份。
 *    本门逐文件判，跨文件那一维不在射程内 —— 需要的是构建产物面的判据，不是词法面的。
 * 2. **不同选择器、相同或更高特异性的覆盖**：`.a.b` 盖 `.b`、`#x .y` 盖 `.y`。见上，故意不做。
 *    注意这里说的是**真正不同的选择器**：`.a+.b` 与 `.a + .b`、`.a.b` 与 `.b.a`、`:before` 与
 *    `::before`、`[dir=rtl]` 与 `[dir="rtl"]` 这四种只是**同一个选择器的不同写法**，
 *    2026-09-05 起由 `selectorKey` 折成同一个键，不再是盲区（此前它们能一路绕过本门）。
 * 3. **块内规则在顶层规则之后**（即正确写法）本门放行 —— 它不检查断点值本身对不对，
 *    只检查它有没有机会生效。值对不对是各屏自己那道门的事。
 * 4. **继承 / 简写展开**：`.x{margin:0}` 之后 `@container{.x{margin-top:8px}}` 这种简写与
 *    长写的互相覆盖不在射程内（属性名不相等）。
 * 5. **胜出方只认深度 0 的顶层规则**：后面若是 `@supports{.row{…}}` 之类再被包一层的规则，
 *    真机上它同样会盖回来，本门放行。这是刻意收窄——判据面一旦扩到「任何后写的同名规则」，
 *    就得同时判两侧条件是否可能同时成立（`@media(max-width:600px)` 与
 *    `@container(min-width:840px)` 互斥时不构成覆盖），那是另一个量级的实现，
 *    而本仓两次真实回归都发生在「块 vs 顶层」这一格。
 */
import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const HERE = fileURLToPath(new URL('.', import.meta.url)); // …/ui/src/styles/
const UI_ROOT = join(HERE, '..', '..'); // …/ui/
const SRC_ROOT = join(UI_ROOT, 'src');

// ─────────────────────────────────────────────────────────────────────────────
// 剥噪与选择器归一化都走共享解析器 `css-cascade.test-support.ts`。
//
// 那是一个 `.test-support.ts` 而不是 `.test.ts`，所以 import 它**不会**把对方的 describe
// 在本文件的 suite 里再注册一遍 —— 本文件此前那份「同形而不 import」的副本因此可以删掉。
// 用的是**只剥注释**的那一份（`stripComments`）而不是连字符串一起抹平的 `stripNoise`：
// 后者会把 `[aria-selected='true']` 抹成 `[aria-selected=    ]`，选择器当场废掉。字符串原文留着，
// 由 `parseRules` 把它当**不透明记号**跳过（`content:"}"` 里的花括号不是结构）。
// 保长度（被剥掉的字符换成空格、换行原样保留）⇒ 偏移量与原文 1:1，行号可直接回算。
// ─────────────────────────────────────────────────────────────────────────────
import { selectorKey, selectorList, stripComments } from './css-cascade.test-support';

const normalize = (s: string): string => s.replace(/\s+/g, ' ').trim();

/** 一条样式规则的层叠相关事实。`important` 逐属性记，因为决胜先比重要度。 */
type Rule = {
  /** 选择器文本在剥噪后源码里的起始下标（含前导空白，仅用于排序与回算行号）。 */
  offset: number;
  /** 花括号嵌套深度（含 at-rule 层）。0 = 顶层规则。 */
  depth: number;
  /** 祖先里有没有 `@container` / `@media`。 */
  conditional: boolean;
  /**
   * 选择器的**归一化键**（`css-cascade.test-support.ts` 的 `selectorKey`）。
   *
   * 此前这里放的是只做了 `\s+ → ' '` 的原文，于是「逐项精确相等」被四种**字面差异**绕过：
   * 组合符两侧空白（`.a+.b` vs `.a + .b`）、复合内简单选择器顺序（`.a.b` vs `.b.a`）、
   * `:before` vs `::before`、属性选择器引号。这四种在浏览器里特异性完全相同、后者照样胜，
   * 而一个格式化器就能造出其中任何一种 —— 那是打穿本门的第五个盲区（文件头原本只列了四个）。
   */
  selectors: string[];
  /** 选择器原文（只进报错消息；判定一律用键）。 */
  raw: string[];
  /** 属性名 → 该属性是否带 `!important`（同规则内重复声明取最后一条，与浏览器一致）。 */
  decls: Map<string, boolean>;
};

const CONDITIONAL_AT = new Set(['container', 'media']);
/**
 * 内部块**不是层叠意义上的规则**的 at-rule。
 *
 * `@keyframes fade{ 0%{…} }` 里的 `0%` 是关键帧选择器不是 CSS 选择器：它既不参与作者来源的层叠，
 * 也归一化不了（送进选择器解析器会当场抛）。此前它被当成「一条选择器为 `0%` 的顶层规则」收进来，
 * 只是因为旧的归一化只做 `\s+ → ' '`、什么都不校验。
 */
const NON_CASCADE_AT = new Set(['keyframes', '-webkit-keyframes', 'counter-style', 'font-face', 'property']);

const parseRules = (css: string): Rule[] => {
  const rules: Rule[] = [];
  const stack: { atName: string; ruleIndex: number }[] = [];
  let buf = '';
  let bufStart = 0;
  let paren = 0;

  const conditional = () => stack.some((f) => CONDITIONAL_AT.has(f.atName));
  const currentRule = () => {
    for (let k = stack.length - 1; k >= 0; k--) if (stack[k].ruleIndex >= 0) return rules[stack[k].ruleIndex];
    return undefined;
  };
  const flushDecl = () => {
    const s = buf.trim();
    buf = '';
    if (!s || s.startsWith('@')) return;
    const k = s.indexOf(':');
    if (k < 0) return;
    const prop = normalize(s.slice(0, k));
    if (!prop) return;
    currentRule()?.decls.set(prop, /!\s*important\s*$/i.test(s.slice(k + 1).trim()));
  };

  for (let i = 0; i < css.length; i++) {
    const c = css[i];
    if (c === '"' || c === "'") {
      // 字符串是不透明记号：内容原样进 buf，里面的 `{` / `}` / `;` 不参与结构判定。
      let j = i + 1;
      while (j < css.length) {
        if (css[j] === '\\') {
          j += 2;
          continue;
        }
        if (css[j] === c) {
          j += 1;
          break;
        }
        if (css[j] === '\n') break; // CSS：字符串不得跨行
        j += 1;
      }
      buf += css.slice(i, j);
      i = j - 1;
      continue;
    }
    if (c === '(') paren++;
    else if (c === ')') paren = Math.max(0, paren - 1);

    if (paren === 0 && c === '{') {
      const prelude = normalize(buf);
      const atName = prelude.startsWith('@') ? (/^@([A-Za-z-]+)/.exec(prelude)?.[1] ?? '') : '';
      let ruleIndex = -1;
      if (!atName && !stack.some((f) => NON_CASCADE_AT.has(f.atName))) {
        ruleIndex = rules.length;
        rules.push({
          offset: bufStart,
          depth: stack.length,
          conditional: conditional(),
          selectors: selectorList(prelude).map(selectorKey),
          raw: selectorList(prelude),
          decls: new Map(),
        });
      }
      stack.push({ atName, ruleIndex });
      buf = '';
      bufStart = i + 1;
      continue;
    }
    if (paren === 0 && (c === '}' || c === ';')) {
      flushDecl();
      if (c === '}') stack.pop();
      bufStart = i + 1;
      continue;
    }
    buf += c;
  }
  return rules;
};

type Finding = { file: string; selector: string; prop: string; deadLine: number; winnerLine: number };

/** 行号从**选择器第一个非空白字符**回算 —— `offset` 含前导空白，直接回算会指到上一条规则的末行。 */
const lineOf = (css: string, offset: number): number => {
  let i = offset;
  while (i < css.length && /\s/.test(css[i])) i++;
  return css.slice(0, i).split('\n').length;
};

const deadRulesIn = (file: string, source: string): Finding[] => {
  const css = stripComments(source);
  const rules = parseRules(css);
  const tops = rules.filter((r) => r.depth === 0);
  const out: Finding[] = [];
  for (const dead of rules) {
    if (!dead.conditional) continue;
    for (const [si, selector] of dead.selectors.entries()) {
      for (const top of tops) {
        if (top.offset <= dead.offset) continue; // 顶层规则写在前面 ⇒ 断点块胜出，正确写法
        if (!top.selectors.includes(selector)) continue; // 逐项精确相等，不许子串
        for (const [prop, deadImportant] of dead.decls) {
          if (!top.decls.has(prop)) continue;
          // 块内 important 而后面顶层不 important ⇒ 块内仍胜出，不是死规则。
          if (deadImportant && !top.decls.get(prop)) continue;
          out.push({
            file,
            selector: dead.raw[si] ?? selector,
            prop,
            deadLine: lineOf(css, dead.offset),
            winnerLine: lineOf(css, top.offset),
          });
        }
      }
    }
  }
  return out;
};

// ─────────────────────────────────────────────────────────────────────────────
// 取材面
// ─────────────────────────────────────────────────────────────────────────────
const walkCss = (dir: string): string[] => {
  const out: string[] = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (['node_modules', 'dist', '.git'].includes(e.name)) continue;
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...walkCss(p));
    else if (e.name.endsWith('.css')) out.push(p);
  }
  return out.sort();
};

const SOURCES = walkCss(SRC_ROOT).map((p) => ({
  label: relative(UI_ROOT, p).split('\\').join('/'),
  source: readFileSync(p, 'utf8'),
}));

const ALL_RULES = SOURCES.flatMap((s) => parseRules(stripComments(s.source)));

describe('① 取材面自检（扫到 0 个文件 / 解析不出规则 ⇒ 必须红，不许空跑绿）', () => {
  it('目录树发现的 CSS 文件非空，且含移动端三个断点密集屏与桌面层叠链', () => {
    expect(SOURCES.length, 'ui/src 下一个 .css 都没扫到 —— 走查目录树的那条腿断了').toBeGreaterThan(0);
    for (const must of [
      'src/mobile/nodes/nodes.css',
      'src/mobile/screens/rules/rules-screen.css',
      'src/mobile/connections/connections.css',
      'src/mobile/home/home.css',
      'src/mobile/forms/forms.css',
      'src/mobile/mobile.css',
      'src/styles/index.css',
    ])
      expect(
        SOURCES.map((s) => s.label),
        `取材面缺 ${must}`,
      ).toContain(must);
  });

  it('解析器真的切出了规则，且断点块内的规则数非零（本门的作用面本体）', () => {
    expect(ALL_RULES.length, '一条规则都没解析出来').toBeGreaterThan(500);
    const inBreakpoint = ALL_RULES.filter((r) => r.conditional);
    expect(inBreakpoint.length, '@container/@media 里一条规则都没解析出来 ⇒ 本门没有作用面').toBeGreaterThan(50);
    // 声明也要真取到：只切规则不取属性的解析器同样会全绿。
    expect(inBreakpoint.filter((r) => r.decls.size > 0).length).toBeGreaterThan(50);
  });
});

describe('② 判据自身的对照组（合成输入，正反两面都钉）', () => {
  const F = (css: string) => deadRulesIn('probe.css', css).map((f) => `${f.selector}{${f.prop}}`);

  it('断点块在前、同名顶层规则在后 ⇒ 抓到', () => {
    expect(F('@container mscreen (min-width: 52.5em){.row{min-height:64px}}\n.row{min-height:54px}')).toEqual([
      '.row{min-height}',
    ]);
    expect(F('@media (min-width: 840px){.row{min-height:64px}}\n.row{min-height:54px}')).toEqual([
      '.row{min-height}',
    ]);
  });

  it('顶层规则在前、断点块在后（正确写法）⇒ 放行', () => {
    expect(F('.row{min-height:54px}\n@container mscreen (min-width: 52.5em){.row{min-height:64px}}')).toEqual([]);
  });

  it('后面那条是更高特异度的**另一个**选择器 ⇒ 放行（不许子串匹配）', () => {
    expect(F('@container mainc (min-width: 60em){.node-grid{gap:8px}}\n#s-nodes.nodes-list-view .node-grid{gap:4px}'))
      .toEqual([]);
    expect(F('@container mscreen (min-width: 52.5em){.row{min-height:64px}}\n.row.sel{min-height:54px}')).toEqual([]);
  });

  it('后面那条声明的是别的属性 ⇒ 放行', () => {
    expect(F('@container mscreen (min-width: 52.5em){.row{min-height:64px}}\n.row{color:red}')).toEqual([]);
  });

  it('选择器列表逐项判：只有真正同名的那一项算死规则', () => {
    expect(F('@container mscreen (min-width: 52.5em){.a,.b{gap:8px}}\n.b{gap:4px}')).toEqual(['.b{gap}']);
    expect(F('@container mscreen (min-width: 52.5em){.a{gap:8px}}\n.b,.a{gap:4px}')).toEqual(['.a{gap}']);
  });

  it('块内 !important 而后面顶层不带 ⇒ 放行；其余三格仍红', () => {
    expect(F('@media (min-width: 840px){.row{gap:8px !important}}\n.row{gap:4px}')).toEqual([]);
    expect(F('@media (min-width: 840px){.row{gap:8px !important}}\n.row{gap:4px !important}')).toEqual(['.row{gap}']);
    expect(F('@media (min-width: 840px){.row{gap:8px}}\n.row{gap:4px !important}')).toEqual(['.row{gap}']);
  });

  it('🔴 第五个盲区已堵：只是**写法**不同的同一个选择器仍算同名（一个格式化器就能造出这四种）', () => {
    // 组合符两侧空白：`.a+.b` ≡ `.a + .b`（2026-09-05 实测打穿本门的那一条，逐字复现）。
    expect(
      F(
        '@container mscreen (min-width: 37.5em){.h-diagrow + .h-diagrow::before{inset-inline-start:44px}}\n' +
          '.h-diagrow+.h-diagrow::before{inset-inline-start:35px}',
      ),
    ).toEqual(['.h-diagrow + .h-diagrow::before{inset-inline-start}']);
    // 复合段内简单选择器顺序：`.a.b` ≡ `.b.a`。
    expect(F('@media (min-width: 840px){.row.sel{gap:8px}}\n.sel.row{gap:4px}')).toEqual(['.row.sel{gap}']);
    // 单冒号旧式伪元素：`:before` ≡ `::before`。
    expect(F('@media (min-width: 840px){.row:before{gap:8px}}\n.row::before{gap:4px}')).toEqual(['.row:before{gap}']);
    // 属性选择器引号：`[dir=rtl]` ≡ `[dir="rtl"]`。
    expect(F('@media (min-width: 840px){[dir=rtl] .row{gap:8px}}\n[dir="rtl"] .row{gap:4px}')).toEqual([
      '[dir=rtl] .row{gap}',
    ]);
    // 反面：归一化不是「把一切拉平」—— 组合符**种类**不同仍是两个选择器，仍放行。
    expect(F('@media (min-width: 840px){.a .b{gap:8px}}\n.a > .b{gap:4px}')).toEqual([]);
  });

  it('注释里的假规则不算数（取材前已剥噪）', () => {
    expect(F('@container mscreen (min-width: 52.5em){.row{min-height:64px}}\n/* .row{min-height:54px} */')).toEqual([]);
  });

  // 射程边界 5 的钉子：这一格真机上会被盖回，本门刻意放行（理由见文件头）。
  // 钉住它是为了「射程收窄」是一次显式决策而不是解析器的意外行为——若哪天扩了射程，这条会红，
  // 逼一次显式改判，而不是让扩面悄悄发生。
  it('胜出方只认 depth=0 的顶层规则：后面再被包一层的同名规则，本门放行（已知盲区）', () => {
    expect(F('@container mscreen (min-width: 52.5em){.row{gap:8px}}\n@supports (display:grid){.row{gap:4px}}'))
      .toEqual([]);
  });
});

describe('③ 全仓：断点块里不许有被同名顶层规则盖回去的死规则', () => {
  it('ui/src 下每份 CSS 都没有死规则', () => {
    const findings = SOURCES.flatMap((s) => deadRulesIn(s.label, s.source));
    expect(
      findings.map((f) => `${f.file}:${f.deadLine} '${f.selector}' 的 ${f.prop} 被 :${f.winnerLine} 的同名顶层规则盖回`),
      '断点块写在同名顶层规则**之前** ⇒ 块内的值永远拿不到层叠胜出，是死代码。' +
        '把断点块整体移到该顶层规则之后（`home/home.css` 的 `.h-diagrow` 是同仓先例）。',
    ).toEqual([]);
  });
});
