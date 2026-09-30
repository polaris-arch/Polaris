/**
 * CSS 特性下限门 —— 断言进构建的 CSS 只用 Chromium 105 及以下的特性。
 *
 * # 为什么是 105，为什么这道门是唯一防线
 *
 * 移动端三档断点用 `@container`（§6.3.6 D-4），桌面 `@container mainc` 早已在用；`:has()` 与
 * `@container` 同在 Chrome 105 落地 ⇒ 下限是**一个数**，不是特性矩阵（§6.3.7）。
 * 手写 CSS 里 `@supports` 为 0 ⇒ 没有任何自动兜底，低版本 WebView 上失效的规则被**静默丢弃**。
 * 分发走 GitHub Releases APK、目标用户多半无 Play 服务 ⇒ WebView 停在出厂版，
 * 裁决是「启动提示 + 不硬拦」，代价是运行期不会报错。所以这道构建期的门是唯一能让越界自曝的地方。
 *
 * 失效后果是**降级不是白屏**：`@container` 失效 ⇒ 三档一并塌回 Compact，每张卡片都还在；
 * `:has()` 失效 ⇒ 3 条独立规则整条丢弃（`index.css` 早已如实记账过这种退化）。
 *
 * # 判据针对特性，不针对数量
 *
 * 「当前有 30 条 @container」这种计数断言毫无用处：第 31 条完全合法却会红，
 * 而 `@supports selector(:nth-child(of))` 这种真越界反而不红。本门断言的是**出现了哪些特性名**。
 *
 * # 白名单，不是黑名单 —— 以及为什么这个白名单不靠「我想起来要查什么」
 *
 * 黑名单（列出禁用特性）不用维护，但**新出的高版本特性静默通过** —— 那正是本门要防的失效形态，
 * 用一个同样会静默漏的机制去防静默失效，等于没防。
 *
 * 白名单要维护，代价是「写了新属性 → 门红 → 得去查基线 → 加进名单」。这个摩擦**就是收益本体**：
 * 它把「查 caniuse」这个动作从自觉变成强制。
 *
 * 关键在于这个白名单是**对源码的词法面**做的，不是对「我记得要检查的特性清单」做的：
 * 扫描器从 CSS 里穷举出 7 类可承载版本差异的标识符位置 —— at-rule 名、媒体/容器特性名、
 * 伪类伪元素、选择器位函数、属性名（含 at-rule 描述符）、值函数、单位 —— 任何一个不在名单里就红。
 * 因此「我没想到这个特性」不会漏，只会以「未登记的名字」形式红出来。
 *
 * ## 这个白名单**盖不住**什么（失效形态，必须知道）
 *
 * 1. **值关键字级的特性**：`display:masonry`、`overflow:overlay` 这类「老属性 + 新取值」。
 *    穷举值关键字会把每个颜色名、每个长度都拖进名单，噪声淹没信号，故不做。
 *    缓解：绝大多数值级新特性会连带一个新**函数**（`color-mix()`）、新**单位**（`dvh`）或
 *    新**属性**（`anchor-name`），这三条都在射程内。
 * 2. **选择器位置放宽**：`:nth-child(2 of .x)` 里的 `of`、`:has()` 可用位置的扩大 —— 名字没变，
 *    只有语法变了。本门看到的仍是已登记的 `:nth-child` / `:has`。
 * 3. **厂商前缀伪元素/属性**（`::-webkit-*` / `-moz-*`）按前缀整体放行：Chromium 不再新增
 *    `-webkit-` 伪元素，`-moz-` 的在 Chromium 里本就是无效选择器（整条规则被丢，与版本无关）。
 * 4. **自定义属性**（`--*`）不校验：作者自定义，与浏览器版本无关。
 *
 * # 射程边界
 *
 * 覆盖两个面，两面共用同一份名单（下面 ⓪–④ 与 ⑤–⑥）：
 *
 * - **手写面**：`ui/src` 下**走目录树发现**的全部 `.css`（不写死文件清单 —— 新建 CSS 文件自动进面）、
 *   `ui/*.html` 的 `<style>` 块、`brand-svgs.ts` 里第三方 SVG 内嵌的 `<style>` 块。
 *   已枚举确认**不含** CSS 特性的来源：React 内联 `style={{}}`（全仓只用到 `calc/hsl/max/min/var`）、
 *   CSS-in-JS（无 styled-components / emotion / vanilla-extract 依赖）、
 *   运行期 `insertRule` / `adoptedStyleSheets`（零命中）。
 * - **构建产物面**：`index.css` 经 Tailwind v4 编译后的实际输出。这一面不是可有可无的：
 *   v4 的 preflight 与工具类会**自己**生成 105 以上的特性（实测 `color-mix()`、`1lh`），
 *   只扫手写 CSS 的门在这条腿上完全没牙。
 *
 * 桌面独有的 `tray-overlay.css` / `update-popup/style.css` 也在面内。它们跑在桌面 WebView、
 * 下限本可更松，但 `index.css` 那条层叠链本就被三个入口共用，一个下限比三套下限简单，且不产生假红。
 *
 * # `@supports` 是唯一逃生口，且必须测它自己
 *
 * 包在 `@supports` 里的越界特性不算违规 —— 不支持时整块跳过，这正是「不静默失效」。
 * 但条件必须**测到这个特性本身**：`@supports (height:100dvh){ height:100dvh }` 放行，
 * `@supports (display:grid){ height:100dvh }` 仍然红。否则逃生口会退化成万能豁免。
 */
import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, relative } from 'node:path';

const HERE = fileURLToPath(new URL('.', import.meta.url)); // …/ui/src/styles/
const UI_ROOT = join(HERE, '..', '..'); // …/ui/
const SRC_ROOT = join(UI_ROOT, 'src');

// ─────────────────────────────────────────────────────────────────────────────
// 剥离器：注释与字符串
//
// 上一版统计把 `index.css` 一句「本仓不用 color-mix」的**注释**数成了一处真实用法。教训已登记：
// 扫描型判据取材前必须先剥注释与字符串。这里两者一遍扫完 —— 分两遍会互相误吞
// （字符串里的 `/*`、注释里的引号）。
//
// **保长度**：被剥掉的字符换成空格、换行原样保留 ⇒ 偏移量与原文 1:1，行号可直接回算。
// ─────────────────────────────────────────────────────────────────────────────
export const stripNoise = (src: string): string => {
  let out = '';
  let i = 0;
  const blank = (s: string) => s.replace(/[^\n]/g, ' ');
  while (i < src.length) {
    const c = src[i];
    if (c === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end === -1 ? src.length : end + 2;
      out += blank(src.slice(i, stop));
      i = stop;
      continue;
    }
    if (c === '"' || c === "'") {
      const q = c;
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
        if (src[j] === '\n') break; // CSS：字符串不得跨行，换行处终止
        j++;
      }
      out += blank(src.slice(i, j));
      i = j;
      continue;
    }
    out += c;
    i++;
  }
  return out;
};

// ─────────────────────────────────────────────────────────────────────────────
// 解析：括号/花括号配平，记录 @supports 守卫栈
// ─────────────────────────────────────────────────────────────────────────────
type Prelude = { text: string; at: boolean; supports: boolean; guards: string[]; index: number };
type Decl = { prop: string; value: string; sel: string; guards: string[]; index: number };

const parse = (css: string): { preludes: Prelude[]; decls: Decl[] } => {
  const preludes: Prelude[] = [];
  const decls: Decl[] = [];
  const openStack: { supports: boolean; sel: string }[] = [];
  const guards: string[] = [];
  let buf = '';
  let bufStart = 0;
  let paren = 0;

  const nearestSel = () => {
    for (let k = openStack.length - 1; k >= 0; k--) if (openStack[k].sel) return openStack[k].sel;
    return '';
  };
  const flushDecl = (text: string, index: number) => {
    const s = text.trim();
    if (!s) return;
    if (s[0] === '@') {
      // 以 `;` 收尾的 at-statement（@import / @config / @charset …）
      preludes.push({ text: s, at: true, supports: false, guards: [...guards], index });
      return;
    }
    const k = s.indexOf(':');
    if (k < 0) return;
    decls.push({
      prop: s.slice(0, k).trim(),
      value: s.slice(k + 1).trim(),
      sel: nearestSel(),
      guards: [...guards],
      index,
    });
  };

  for (let i = 0; i < css.length; i++) {
    const c = css[i];
    if (c === '(') paren++;
    else if (c === ')') paren = Math.max(0, paren - 1);

    if (paren === 0 && c === '{') {
      const s = buf.trim();
      const supports = /^@supports\b/.test(s);
      preludes.push({ text: s, at: s[0] === '@', supports, guards: [...guards], index: bufStart });
      openStack.push({ supports, sel: s[0] === '@' ? '' : s });
      if (supports) guards.push(s);
      buf = '';
      bufStart = i + 1;
      continue;
    }
    if (paren === 0 && c === '}') {
      flushDecl(buf, bufStart);
      const closed = openStack.pop();
      if (closed?.supports) guards.pop();
      buf = '';
      bufStart = i + 1;
      continue;
    }
    if (paren === 0 && c === ';') {
      flushDecl(buf, bufStart);
      buf = '';
      bufStart = i + 1;
      continue;
    }
    if (!buf) bufStart = i;
    buf += c;
  }
  flushDecl(buf, bufStart);
  return { preludes, decls };
};

// ─────────────────────────────────────────────────────────────────────────────
// 词法面：7 类可承载版本差异的标识符位置
// ─────────────────────────────────────────────────────────────────────────────
type Cat = 'at' | 'feature' | 'pseudo' | 'selfn' | 'prop' | 'fn' | 'unit';
type Hit = { cat: Cat; name: string; index: number; guards: string[] };

const FN = /(?<![\w.#-])([a-zA-Z][a-zA-Z0-9-]*)\(/g;
/** 数字紧跟字母 = 单位。`#0000`（色值）与 `translate3d`（标识符内）由 lookbehind 排除。 */
const UNIT = /(?<![\w.#%-])\d*\.?\d+([a-zA-Z]+)\b/g;

const collect = (css: string): Hit[] => {
  const { preludes, decls } = parse(css);
  const hits: Hit[] = [];
  const push = (cat: Cat, name: string, index: number, guards: string[]) =>
    hits.push({ cat, name, index, guards });

  for (const p of preludes) {
    // `@supports` 的条件本身永远安全：不支持时只是求值为假，不会有任何声明生效。
    if (p.supports) continue;
    if (p.at) {
      const at = p.text.match(/^@([a-zA-Z-]+)/);
      if (at) push('at', at[1], p.index, p.guards);
      for (const m of p.text.matchAll(/\(\s*(?:min-|max-)?([a-zA-Z-]+)\s*(?=[:<>=)])/g))
        push('feature', m[1], p.index, p.guards);
      for (const m of p.text.matchAll(FN)) push('selfn', m[1], p.index, p.guards);
    } else {
      for (const m of p.text.matchAll(/(::?)([a-zA-Z-]+)/g))
        push('pseudo', m[1] + m[2], p.index, p.guards);
      // 伪类函数（`:has(` / `:not(`）已按伪类记账，别再当成选择器位函数记一次。
      const noPseudo = p.text.replace(/::?[a-zA-Z-]+\(/g, ' (');
      for (const m of noPseudo.matchAll(FN)) push('selfn', m[1], p.index, p.guards);
    }
  }
  for (const d of decls) {
    push('prop', d.prop, d.index, d.guards);
    for (const m of d.value.matchAll(FN)) push('fn', m[1], d.index, d.guards);
    for (const m of d.value.matchAll(UNIT)) push('unit', m[1], d.index, d.guards);
  }
  return hits;
};

// ─────────────────────────────────────────────────────────────────────────────
// 名单
//
// 括号里的数字是该特性在 Chrome 的落地版本，全部 ≤105。**未标注版本的条目一律是**
// 「CSS1/CSS2 时代或 Chrome 早期（<40）」的老特性。加新条目时必须查基线再加，
// 不确定就别加 —— 让它红着，比猜一个版本号进名单危险度低一个量级。
//
// 名单里有一部分**当前未使用**（如 `env` / `clamp` / `@font-face` 的描述符 / `touch-action`）：
// 那是移动端 P1 一定会用到、且我已核对过基线的条目。预登记「已核实的」不削弱本门；
// 预登记「没核实的」才削弱，故一条都没有。
// ─────────────────────────────────────────────────────────────────────────────

/** 构建期指令，不是浏览器特性 —— Tailwind 编译时就消掉了，永远到不了 WebView。 */
const BUILD_DIRECTIVES = new Set([
  'config',
  'tailwind',
  'apply',
  'theme',
  'source',
  'utility',
  'variant',
  'custom-variant',
  'plugin',
  'reference',
]);

const BASELINE: Record<Cat, Set<string>> = {
  at: new Set([
    ...BUILD_DIRECTIVES,
    'charset',
    'import',
    'media',
    'page',
    'font-face',
    'keyframes', // 43
    'supports', // 28 —— 逃生口本身
    'counter-style', // 91
    'font-feature-values',
    'namespace',
    'property', // 85（Tailwind v4 输出用）
    'layer', // 99（同上）
    'container', // 105 ← 下限本体
  ]),
  feature: new Set([
    'width',
    'height',
    'aspect-ratio',
    'orientation',
    'resolution', // 29
    'color',
    'monochrome',
    'hover', // 38
    'any-hover', // 41
    'pointer', // 41
    'any-pointer', // 41
    'display-mode', // 42
    'prefers-reduced-motion', // 74
    'prefers-color-scheme', // 76
    'forced-colors', // 89
    'prefers-contrast', // 96
    'dynamic-range', // 98
    'inline-size', // 105（container query）
    'block-size', // 105（container query）
  ]),
  pseudo: new Set([
    ':root',
    ':hover',
    ':active',
    ':focus',
    ':link',
    ':visited',
    ':target',
    ':lang',
    ':not',
    ':empty',
    ':checked',
    ':disabled',
    ':enabled',
    ':indeterminate',
    ':default',
    ':required',
    ':optional',
    ':valid',
    ':invalid',
    ':in-range',
    ':out-of-range',
    ':read-only',
    ':read-write',
    ':first-child',
    ':last-child',
    ':only-child',
    ':first-of-type',
    ':last-of-type',
    ':only-of-type',
    ':nth-child',
    ':nth-last-child',
    ':nth-of-type',
    ':nth-last-of-type',
    ':scope', // 27
    ':any-link', // 50
    ':host', // 53
    ':host-context', // 53
    ':defined', // 54
    ':placeholder-shown', // 47
    ':focus-within', // 60
    ':fullscreen', // 71
    ':focus-visible', // 86
    ':is', // 88
    ':where', // 88
    ':has', // 105 ← 下限本体
    '::before',
    '::after',
    '::first-line',
    '::first-letter',
    '::selection',
    '::backdrop', // 37
    '::slotted', // 53
    '::placeholder', // 57
    '::part', // 73
    '::marker', // 86
    '::file-selector-button', // 89
  ]),
  selfn: new Set([]),
  prop: new Set([
    // 布局 / 盒模型
    'display',
    'position',
    'top',
    'right',
    'bottom',
    'left',
    'inset', // 87
    'inset-block-start', // 87
    'inset-block-end', // 87
    'inset-inline-start', // 87
    'inset-inline-end', // 87
    'width',
    'height',
    'min-width',
    'min-height',
    'max-width',
    'max-height',
    'aspect-ratio', // 88
    'box-sizing', // 10
    'margin',
    'margin-top',
    'margin-right',
    'margin-bottom',
    'margin-left',
    'margin-inline', // 87
    'margin-inline-start', // 87
    'margin-inline-end', // 87
    'margin-block', // 87
    'padding',
    'padding-top',
    'padding-right',
    'padding-bottom',
    'padding-left',
    'padding-inline', // 87
    'padding-inline-start', // 87
    'padding-inline-end', // 87
    'padding-block', // 87
    'overflow',
    'overflow-x',
    'overflow-y',
    'overflow-wrap', // 23
    'overscroll-behavior', // 63
    'overscroll-behavior-x', // 63
    'overscroll-behavior-y', // 63
    'contain', // 52
    'container', // 105 ← 下限本体（简写）
    'container-type', // 105
    'container-name', // 105
    'z-index',
    'visibility',
    'float',
    'clear',
    'resize',
    'object-fit', // 32
    'object-position', // 31
    'isolation', // 41
    'mix-blend-mode', // 41
    'clip-path', // 55
    // flex / grid
    'flex',
    'flex-basis',
    'flex-direction',
    'flex-flow',
    'flex-grow',
    'flex-shrink',
    'flex-wrap',
    'order',
    'align-content', // 29
    'align-items', // 52
    'align-self', // 21
    'justify-content',
    'justify-items', // 57
    'justify-self', // 57
    'place-items', // 59
    'place-content', // 59
    'place-self', // 59
    'gap', // 84（flexbox）
    'row-gap', // 84
    'column-gap',
    // 多列列流：两条都是 Chrome 50，远早于 105 —— 它们此前不在名单里只是因为**没人用过**。
    // F0 的设计文档 §7 第 4 条把「第一批真屏落地时连同白名单一起加」登记成一次有记录的决策；
    // `src/mobile/nodes/nodes.css` 是那第一批，medium 起的两列列表用的正是这一对。
    // 用列流而不是网格的理由写在那份 CSS 里：已排序的列表要按**列**读，网格按行填会把名次打散。
    'column-count', // 50
    'break-inside', // 50 // 84
    'grid-template-columns', // 57
    'grid-template-rows', // 57
    'grid-template-areas', // 57
    'grid-auto-columns', // 57
    'grid-auto-rows', // 57
    'grid-auto-flow', // 57
    'grid-area', // 57
    'grid-column', // 57
    'grid-row', // 57
    // 排版
    'color',
    'font',
    'font-family',
    'font-size',
    'font-weight',
    'font-style',
    'font-stretch',
    'font-variant-numeric', // 52
    'font-feature-settings', // 48
    'font-variation-settings', // 62
    'font-synthesis', // 97
    'font-display', // 60（@font-face 描述符）
    'src', // @font-face 描述符
    'unicode-range', // @font-face 描述符
    'size-adjust', // 92（@font-face 描述符）
    'ascent-override', // 87
    'descent-override', // 87
    'line-gap-override', // 87
    'line-height',
    'letter-spacing',
    'word-spacing',
    'word-break',
    'white-space',
    'text-align',
    'text-indent',
    'text-transform',
    'text-decoration',
    'text-decoration-line', // 57
    'text-decoration-color', // 57
    'text-decoration-style', // 57
    'text-decoration-thickness', // 89
    'text-underline-offset', // 87
    'text-overflow',
    'text-rendering',
    'text-shadow',
    'vertical-align',
    'writing-mode', // 48
    'direction',
    'hyphens', // 88
    'tab-size', // 21
    'list-style',
    'list-style-type',
    'list-style-position',
    'list-style-image',
    'quotes',
    'counter-reset',
    'counter-increment',
    'content',
    // 视觉
    'background',
    'background-color',
    'background-image',
    'background-position',
    'background-size', // 4
    'background-repeat',
    'background-clip',
    'background-origin',
    'background-attachment',
    'background-blend-mode', // 35
    'border',
    'border-top',
    'border-right',
    'border-bottom',
    'border-left',
    'border-width',
    'border-top-width',
    'border-right-width',
    'border-bottom-width',
    'border-left-width',
    'border-style',
    'border-color',
    'border-top-color',
    'border-right-color',
    'border-bottom-color',
    'border-left-color',
    'border-radius',
    'border-top-left-radius',
    'border-top-right-radius',
    'border-bottom-left-radius',
    'border-bottom-right-radius',
    'border-collapse',
    'border-spacing',
    'border-inline', // 87
    'border-inline-start', // 87
    'border-inline-end', // 87
    'border-block', // 87
    'box-shadow', // 10
    'outline',
    'outline-color',
    'outline-style',
    'outline-width',
    'outline-offset',
    'opacity',
    'filter', // 53
    'backdrop-filter', // 76
    'color-scheme', // 81
    'accent-color', // 93
    'caret-color', // 57
    'table-layout',
    'cursor',
    'pointer-events',
    'user-select', // 54
    'touch-action', // 36
    'appearance', // 84
    'scroll-behavior', // 61
    'scroll-snap-type', // 69
    'scroll-snap-align', // 69
    'scroll-margin', // 69
    'scroll-padding', // 69
    'content-visibility', // 85
    'contain-intrinsic-size', // 83
    // 动画 / 变换
    'transition',
    'transition-property',
    'transition-duration',
    'transition-timing-function',
    'transition-delay',
    'animation', // 43
    'animation-name',
    'animation-duration',
    'animation-timing-function',
    'animation-delay',
    'animation-iteration-count',
    'animation-direction',
    'animation-fill-mode',
    'animation-play-state',
    'transform', // 36
    'transform-origin',
    'transform-style',
    'perspective',
    'perspective-origin',
    'backface-visibility',
    'will-change', // 36
    // @property 描述符
    'syntax', // 85
    'inherits', // 85
    'initial-value', // 85
    // SVG 表现属性（brand-svgs 的 <style> 与 .sankey 用到）
    'fill',
    'fill-rule',
    'fill-opacity',
    'stroke',
    'stroke-width',
    'stroke-linecap',
    'stroke-linejoin',
    'stroke-miterlimit',
    'stroke-dasharray',
    'stroke-opacity',
    'clip-rule',
    'paint-order', // 35
    'rx', // 64（几何属性可作 CSS 属性）
    'ry', // 64
  ]),
  fn: new Set([
    'var', // 49
    'calc', // 26
    'min', // 79
    'max', // 79
    'clamp', // 79
    'env', // 69 ← safe-area 靠它
    'attr',
    'url',
    'counter',
    'rgb',
    'rgba',
    'hsl',
    'hsla',
    'linear-gradient', // 26
    'radial-gradient', // 26
    'conic-gradient', // 69
    'repeating-linear-gradient',
    'repeating-radial-gradient',
    'image-set', // 21
    'repeat', // 57
    'minmax', // 57
    'fit-content', // 57
    'blur', // 53
    'brightness', // 53
    'contrast', // 53
    'drop-shadow', // 53
    'grayscale', // 53
    'hue-rotate', // 53
    'invert', // 53
    'opacity', // 53
    'saturate', // 53
    'sepia', // 53
    'translate',
    'translateX',
    'translateY',
    'translateZ',
    'translate3d',
    'rotate',
    'rotateX',
    'rotateY',
    'rotateZ',
    'rotate3d',
    'scale',
    'scaleX',
    'scaleY',
    'scale3d',
    'skew',
    'skewX',
    'skewY',
    'matrix',
    'matrix3d',
    'perspective',
    'cubic-bezier',
    'steps',
    'path', // 46
    'circle', // 37
    'ellipse', // 37
    'polygon', // 37
    'inset', // 37
    'format', // @font-face src
    'local', // @font-face src
    'symbols',
  ]),
  unit: new Set([
    'px',
    'em',
    'rem',
    'ex',
    'ch',
    'cm',
    'mm',
    'in',
    'pt',
    'pc',
    'q',
    'vh',
    'vw',
    'vmin',
    'vmax',
    'fr', // 57
    'deg',
    'rad',
    'grad',
    'turn',
    's',
    'ms',
    'hz',
    'khz',
    'dpi',
    'dpcm',
    'dppx',
    'x',
    // 容器查询单位，全部 105 —— 与 @container 同批落地
    'cqw',
    'cqh',
    'cqi',
    'cqb',
    'cqmin',
    'cqmax',
  ]),
};

type Face = 'handwritten' | 'built';

/**
 * 越界但仍然放行的条目。每条都必须带：Chrome 版本、失效后果、以及**可断言的兜底**。
 * 下面 ④ / ⑥ 两组测试逐条验证这些兜底真的在，豁免因此不是空头支票。
 *
 * `faces` 把豁免限制在它成立的那个面上 —— `lh` 的理由只对 Tailwind preflight 成立，
 * 手写 CSS 里再出现 `2lh` 必须照样红。
 */
const EXEMPT: Record<string, { faces: Face[]; why: string }> = {
  'prop:scrollbar-width': {
    faces: ['handwritten', 'built'],
    why:
      'Chrome 121。仓内每一处都与同基选择器的 `::-webkit-scrollbar{display:none}` 成对出现（④ 逐处验证），' +
      'Chromium 105 走 webkit 那条腿，行为一致。',
  },
  'prop:app-region': {
    faces: ['handwritten', 'built'],
    why:
      'Chromium 私有属性，无前缀形态的落地版本**未核实**。射程与下限无关：仓内注释已记它仅 Windows ' +
      'WebView2 生效（拖动实际靠 HTML `data-tauri-drag-region`），移动 WebView 上本就不生效。' +
      '④ 断言它只出现在桌面窗口 chrome 选择器上，不会渗进移动端类名。',
  },
  'unit:lh': {
    faces: ['built'],
    why:
      'Chrome 133。**只在构建产物面**放行，来自 Tailwind v4 preflight 的 ' +
      '`::-webkit-date-and-time-value{min-height:1lh}`，未被 @supports 包住。' +
      'Chromium 105 丢弃该声明 ⇒ 日期/时间输入控件回落浏览器默认最小高度。' +
      '⑥ 断言：它只挂在该伪元素上，且本仓没有任何 date/time 类 input。',
  },
};

/** 厂商前缀整体放行（理由见文件头「盖不住什么」第 3 条）。 */
const vendorPrefixed = (name: string) => /^(::?)?-(webkit|moz|ms|o)-/.test(name);

const key = (h: Hit) => `${h.cat}:${h.name}`;

/** 守卫必须测到这个特性本身，否则 @supports 会退化成万能豁免。 */
const guardCovers = (h: Hit) =>
  h.guards.some((cond) => cond.toLowerCase().includes(h.name.toLowerCase()));

const lineOf = (css: string, index: number) => css.slice(0, index).split('\n').length;

type Violation = { key: string; where: string; guardedButUntested: boolean };

const violationsIn = (label: string, css: string, face: Face = 'handwritten'): Violation[] => {
  const out: Violation[] = [];
  const seen = new Set<string>();
  for (const h of collect(css)) {
    if (h.cat === 'prop' && h.name.startsWith('--')) continue; // 自定义属性
    if (vendorPrefixed(h.name)) continue;
    const k = key(h);
    if (BASELINE[h.cat].has(h.name)) continue;
    if (EXEMPT[k]?.faces.includes(face)) continue;
    if (h.guards.length && guardCovers(h)) continue;
    const dedup = `${k}@${label}`;
    if (seen.has(dedup)) continue;
    seen.add(dedup);
    out.push({
      key: k,
      where: `${label}:${lineOf(css, h.index)}`,
      guardedButUntested: h.guards.length > 0,
    });
  }
  return out;
};

const report = (vs: Violation[]) =>
  vs
    .map(
      (v) =>
        `  ${v.key}  @ ${v.where}` +
        (v.guardedButUntested ? '（在 @supports 里，但条件没测到它自己）' : ''),
    )
    .join('\n');

// ─────────────────────────────────────────────────────────────────────────────
// 取材面
// ─────────────────────────────────────────────────────────────────────────────
type Source = { label: string; css: string };

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

const STYLE_BLOCK = /<style[^>]*>([\s\S]*?)<\/style>/g;

const handwrittenSources = (): Source[] => {
  const srcs: Source[] = walkCss(SRC_ROOT).map((p) => ({
    label: relative(UI_ROOT, p),
    css: stripNoise(readFileSync(p, 'utf8')),
  }));
  // HTML 入口内联 <style>（index.html 的首帧主题兜底就住在这里）
  for (const f of readdirSync(UI_ROOT).filter((n) => n.endsWith('.html'))) {
    const html = readFileSync(join(UI_ROOT, f), 'utf8');
    let n = 0;
    for (const m of html.matchAll(STYLE_BLOCK))
      srcs.push({ label: `${f}#style[${n++}]`, css: stripNoise(m[1]) });
  }
  // 第三方品牌 SVG 里内嵌的 <style>：经 dangerouslySetInnerHTML 进 DOM，是真会生效的 CSS
  const brandPath = join(SRC_ROOT, 'components', 'brand-icons', 'brand-svgs.ts');
  const brand = readFileSync(brandPath, 'utf8');
  let n = 0;
  for (const m of brand.matchAll(STYLE_BLOCK))
    srcs.push({ label: `brand-svgs.ts#style[${n++}]`, css: stripNoise(m[1]) });
  return srcs;
};

const HANDWRITTEN = handwrittenSources();
const HANDWRITTEN_ALL = HANDWRITTEN.map((s) => s.css).join('\n');

/** Tailwind v4 的真实输出。构建失败必须炸出来 —— 静默跳过等于这条腿从来没跑过。 */
const buildTailwindCss = async (): Promise<string> => {
  const [{ default: postcss }, { default: tw }] = await Promise.all([
    import('postcss'),
    import('@tailwindcss/postcss'),
  ]);
  const from = join(HERE, 'index.css');
  const res = await postcss([tw()]).process(readFileSync(from, 'utf8'), { from });
  return res.css;
};

// ─────────────────────────────────────────────────────────────────────────────

describe('⓪ 剥离器与解析器自检（判据的前提，先证它自己是对的）', () => {
  it('注释里 3 处 + 代码里 1 处 color-mix ⇒ 剥完只剩 1（上一版数错的那次原样复现）', () => {
    const synthetic = `
/* 本仓至今没有用 color-mix()，也不打算用 color-mix() —— color-mix() 是 Chrome 111。 */
.x { color: color-mix(in oklab, red, blue); }
`;
    expect((synthetic.match(/color-mix\(/g) ?? []).length).toBe(4); // 剥之前：3 注释 + 1 代码
    const stripped = stripNoise(synthetic);
    expect((stripped.match(/color-mix\(/g) ?? []).length).toBe(1); // 剥之后：只剩代码里那处
    expect(violationsIn('synthetic', stripped).map((v) => v.key)).toEqual(['fn:color-mix']);
  });

  it('字符串里的特性名同样不算数，且 `/*` 在字符串里不会被当成注释起点', () => {
    const stripped = stripNoise(`.y::after { content: "/* @container :has( 100dvh */"; }`);
    expect(stripped).not.toMatch(/@container|:has\(|100dvh/);
    // 保长度：剥完行数与总长不变，行号才能直接回算
    const src = `a{}\n/* xx */\nb{}`;
    expect(stripNoise(src)).toHaveLength(src.length);
    expect(stripNoise(src).split('\n')).toHaveLength(3);
  });

  it('@supports 守卫栈：嵌套进出都记对', () => {
    const css = stripNoise(`
      .a { height: 100dvh; }
      @supports (height: 100dvh) { .b { height: 100dvh; } }
      .c { height: 100dvh; }
    `);
    const guarded = collect(css).filter((h) => h.cat === 'unit' && h.name === 'dvh');
    expect(guarded.map((h) => h.guards.length)).toEqual([0, 1, 0]);
  });
});

describe('① 取材面完整性（正面断言：CSS 被删光时本门必须红）', () => {
  it('目录树发现的 CSS 文件覆盖了层叠链的全部四层 + 两个独立入口', () => {
    const found = HANDWRITTEN.map((s) => s.label);
    for (const must of [
      'src/styles/tokens.css',
      'src/styles/components.css',
      'src/styles/screens.css',
      'src/styles/prototype.css',
      'src/styles/index.css',
      'src/tray/tray-overlay.css',
      'src/update-popup/style.css',
    ])
      expect(found, `取材面缺 ${must}`).toContain(must);
  });

  it('HTML 内联 <style> 与品牌 SVG 内嵌 <style> 都真的取到了内容', () => {
    const html = HANDWRITTEN.filter((s) => s.label.startsWith('index.html#'));
    expect(html.length, 'index.html 的首帧主题 <style> 没取到').toBeGreaterThan(0);
    expect(parse(html.map((s) => s.css).join('\n')).decls.length).toBeGreaterThan(0);

    const brand = HANDWRITTEN.filter((s) => s.label.startsWith('brand-svgs.ts#'));
    expect(brand.length, '品牌 SVG 的 <style> 没取到').toBeGreaterThan(0);
    expect(parse(brand.map((s) => s.css).join('\n')).decls.length).toBeGreaterThan(0);
  });

  it('解析出的声明量在合理量级（不是「什么都没扫到所以全绿」）', () => {
    const { preludes, decls } = parse(HANDWRITTEN_ALL);
    expect(decls.length, `只解析出 ${decls.length} 条声明 —— 扫描器坏了或 CSS 被删了`).toBeGreaterThan(
      5000,
    );
    expect(preludes.length).toBeGreaterThan(1500);
  });

  it('下限本体的三个特性确实被扫描器看见（它们消失 = 判据失去对象）', () => {
    const hits = collect(HANDWRITTEN_ALL);
    const n = (cat: Cat, name: string) =>
      hits.filter((h) => h.cat === cat && h.name === name).length;
    expect(n('at', 'container'), '@container 一条都没扫到').toBeGreaterThan(0);
    expect(n('pseudo', ':has'), ':has() 一处都没扫到').toBeGreaterThan(0);
    expect(n('prop', 'container'), 'container 简写一处都没扫到').toBeGreaterThan(0);
  });
});

describe('② 手写面：全部特性 ≤ Chromium 105', () => {
  for (const s of HANDWRITTEN) {
    it(`${s.label}`, () => {
      const vs = violationsIn(s.label, s.css);
      expect(
        vs,
        vs.length
          ? `以下标识符不在 ≤105 名单里 —— 查基线后要么加进 BASELINE，要么用 @supports 包起来：\n${report(vs)}`
          : '',
      ).toEqual([]);
    });
  }
});

describe('③ 反向对照：越界必红、合法必绿、守卫必须测它自己', () => {
  /**
   * 变异体里的标识符**一律拼接生成**，不写成完整字面量。
   *
   * 理由是本文件差点踩到的一个真坑（判据被自己污染）：`tailwind.config.js` 的 content 是
   * `./src/** /*.{ts,tsx}`，**测试文件（含其中的注释）也在内**。第一版把 Chrome 114 那个
   * 换行属性写成完整字面量后，Tailwind 的候选提取器把它当成同名工具类，往**生产 CSS**里生成了
   * 对应规则 —— 门自己制造了它要守的那个违规，并如实报红。拼接后字面量不再出现在源文本里，
   * 提取器无从命中；连本段注释也不能写出那个完整名字，否则同样会被提取。
   *
   * 顺带的仓级发现（不在本门射程内，故只记不修）：任何 `.ts/.tsx` 里**注释或字符串**提到的
   * Tailwind 工具类名都会被生成进生产 CSS。根治是把测试文件从 `tailwind.config.js` 的
   * content 里排除掉，那会动到打包产物，另开一批。
   */
  const j = (...xs: string[]) => xs.join('');
  const TEXT_WRAP = j('text-', 'wrap');
  const FIELD_SIZING = j('field-', 'sizing');
  const ANCHOR_NAME = j('anchor-', 'name');
  const POPOVER_OPEN = j('popover-', 'open');

  const MUTANTS: [string, string, string][] = [
    ['color-mix()（111）', '.x{ color: color-mix(in oklab, red, blue); }', 'fn:color-mix'],
    ['oklch()（111）', '.x{ color: oklch(0.7 0.1 200); }', 'fn:oklch'],
    ['dvh 单位（108）', '.x{ height: 100dvh; }', 'unit:dvh'],
    ['lh 单位（133）·手写面不豁免', '.x{ min-height: 2lh; }', 'unit:lh'],
    [`${TEXT_WRAP}（114）`, `.x{ ${TEXT_WRAP}: balance; }`, `prop:${TEXT_WRAP}`],
    [`${FIELD_SIZING}（123）`, `.x{ ${FIELD_SIZING}: content; }`, `prop:${FIELD_SIZING}`],
    ['@scope（118）', '@scope (.a) { .b{ color:red } }', 'at:scope'],
    ['@starting-style（117）', '@starting-style { .x{ opacity:0 } }', 'at:starting-style'],
    [`${ANCHOR_NAME}（125）`, `.x{ ${ANCHOR_NAME}: --a; }`, `prop:${ANCHOR_NAME}`],
    [`:${POPOVER_OPEN}（114）`, `.x:${POPOVER_OPEN}{ opacity:1 }`, `pseudo::${POPOVER_OPEN}`],
    ['@media scripting（120）', '@media (scripting: enabled){ .x{color:red} }', 'feature:scripting'],
  ];
  for (const [name, css, expected] of MUTANTS) {
    it(`${name} 判红`, () => {
      const vs = violationsIn('mutant', stripNoise(css));
      expect(vs.map((v) => v.key)).toContain(expected);
    });
  }

  it('合法输入判绿（下限本体的特性全用上，一条不红）', () => {
    const legal = `
      .win { container: mainc / inline-size; }
      @container mainc (max-width: 540px) { .grid { grid-template-columns: repeat(2, minmax(0,1fr)); } }
      body:has(.pending-bar.show) #toast-stack { bottom: calc(var(--statusbar-h) + 12px); }
      .card { width: 30cqw; backdrop-filter: blur(14px); padding: env(safe-area-inset-bottom); }
      @media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) .toast { background: hsl(var(--fg)); } }
    `;
    expect(violationsIn('legal', stripNoise(legal))).toEqual([]);
  });

  it('@supports 包住越界特性 ⇒ 放行；条件没测到它自己 ⇒ 仍然红', () => {
    const covered = '@supports (height: 100dvh) { .x { height: 100dvh; } }';
    expect(violationsIn('covered', stripNoise(covered))).toEqual([]);

    const bogus = '@supports (display: grid) { .x { height: 100dvh; } }';
    const vs = violationsIn('bogus', stripNoise(bogus));
    expect(vs.map((v) => v.key)).toContain('unit:dvh');
    expect(vs[0].guardedButUntested).toBe(true);
  });

  it('变异写进真实取材面同样判红（面是走目录树发现的，不是写死的文件清单）', () => {
    // 与 CLI 侧那次「真改文件跑门」互补：这里证明**新文件**会自动进面。
    const probe: Source = { label: 'src/styles/__probe.css', css: '.p{ color: oklch(0.5 0 0); }' };
    const faces = [...HANDWRITTEN, probe];
    const all = faces.flatMap((s) => violationsIn(s.label, s.css));
    expect(all.map((v) => v.key)).toContain('fn:oklch');
  });
});

describe('④ 手写面豁免项的兜底必须真的在（豁免不是空头支票）', () => {
  it('scrollbar-width 的每一处都有同基选择器的 ::-webkit-scrollbar 兜底', () => {
    const missing: string[] = [];
    for (const s of HANDWRITTEN) {
      const { preludes, decls } = parse(s.css);
      const sels = new Set(preludes.map((p) => p.text.replace(/\s+/g, '')));
      for (const d of decls) {
        if (d.prop !== 'scrollbar-width') continue;
        const base = d.sel.replace(/\s+/g, '');
        const ok = [...sels].some((x) => x.startsWith(`${base}::-webkit-scrollbar`));
        if (!ok) missing.push(`${s.label}: ${d.sel}`);
      }
    }
    expect(
      missing,
      `scrollbar-width（Chrome 121）少了 ::-webkit-scrollbar 兜底 ⇒ Chromium 105 上滚动条会显形：\n${missing.join('\n')}`,
    ).toEqual([]);
    // 正面对照：兜底检查本身有对象，不是「一处都没有所以空过」
    const uses = HANDWRITTEN.flatMap((s) =>
      parse(s.css).decls.filter((d) => d.prop === 'scrollbar-width'),
    );
    expect(uses.length, 'scrollbar-width 一处都没有 —— 豁免项可以从名单里删了').toBeGreaterThan(0);
  });

  it('app-region 只出现在桌面窗口 chrome 上，没渗进别的选择器', () => {
    const sels = HANDWRITTEN.flatMap((s) =>
      parse(s.css)
        .decls.filter((d) => d.prop === 'app-region')
        .map((d) => `${s.label}: ${d.sel}`),
    );
    expect(sels.length, 'app-region 一处都没有 —— 豁免项可以从名单里删了').toBeGreaterThan(0);
    for (const x of sels) expect(x, `app-region 出现在非窗口 chrome 的选择器上：${x}`).toMatch(/-chrome\b/);
  });
});

describe('⑤ 构建产物面：Tailwind v4 生成的 CSS 同样 ≤ Chromium 105', () => {
  it('构建输出里的全部特性都在名单内（v4 preflight 会自己生成 color-mix / 1lh）', async () => {
    const built = stripNoise(await buildTailwindCss());
    expect(parse(built).decls.length, '构建输出为空 —— 这条腿没跑').toBeGreaterThan(5000);
    const vs = violationsIn('tailwind-output', built, 'built');
    expect(
      vs,
      vs.length
        ? `Tailwind 输出里出现了未登记的标识符（升级 Tailwind 后最常见）：\n${report(vs)}`
        : '',
    ).toEqual([]);
  });

  it('反向对照：往构建输出里掺一条越界规则，同一判据必须红', async () => {
    const built = stripNoise(await buildTailwindCss()) + '\n.__mutant{ height: 100svh; }';
    expect(violationsIn('tailwind-output+mutant', built, 'built').map((v) => v.key)).toContain(
      'unit:svh',
    );
  });

  it('v4 自己生成的 color-mix() 确实被 @supports 包住（放行的理由要成立）', async () => {
    const built = stripNoise(await buildTailwindCss());
    const cm = collect(built).filter((h) => h.cat === 'fn' && h.name === 'color-mix');
    // 当前 preflight 就有一处；将来 `bg-x/50` 这类透明度工具类还会生成更多，也一律带守卫。
    expect(cm.length, 'color-mix 一处都没有 —— 这条断言失去对象，去掉或换锚点').toBeGreaterThan(0);
    for (const h of cm)
      expect(guardCovers(h), 'Tailwind 生成了不带 @supports 守卫的 color-mix()').toBe(true);
  });
});

describe('⑥ 构建产物面豁免项的兜底', () => {
  it('lh 单位只挂在 ::-webkit-date-and-time-value 上，且本仓没有 date/time 输入控件', async () => {
    const built = stripNoise(await buildTailwindCss());
    const { decls } = parse(built);
    const lh = decls.filter((d) => /(?<![\w.#%-])\d*\.?\d+lh\b/.test(d.value));
    expect(lh.length, 'lh 一处都没有 —— 豁免项可以从名单里删了').toBeGreaterThan(0);
    for (const d of lh)
      expect(d.sel, `lh（Chrome 133）跑到了别的选择器上：${d.sel}`).toMatch(
        /::-webkit-date-and-time-value/,
      );

    const tsx: string[] = [];
    const walk = (dir: string) => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        if (['node_modules', 'dist', '.git'].includes(e.name)) continue;
        const p = join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (/\.tsx?$/.test(e.name)) tsx.push(p);
      }
    };
    walk(SRC_ROOT);
    const withDateInput = tsx.filter((p) =>
      /type\s*=\s*["'{]?\s*["']?(date|time|datetime-local|month|week)["']/.test(
        readFileSync(p, 'utf8'),
      ),
    );
    expect(
      withDateInput.map((p) => relative(UI_ROOT, p)),
      'date/time 输入控件出现了 ⇒ lh 的豁免理由不再成立，需要另想兜底',
    ).toEqual([]);
  });
});
