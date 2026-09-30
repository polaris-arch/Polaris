/**
 * `mobile-token-parity` —— §6.3.5 的门。
 *
 * 契约与门规格：`~/docs/polaris/design/polaris-mobile-platform-evaluation-2026-08-29.md` §6.3
 * （§6.3.1 桌面视觉真值链 / §6.3.2 缺口 / §6.3.3 四条契约 / §6.3.5 门的五条）。
 * 落地设计：`~/docs/polaris/design/polaris-mobile-token-entry-2026-09-04.md`。
 *
 * ── 已落地范围（2026-09-04 补齐左半边与第 2 条）────────────────────────────
 * ✅ 第 1 条**两侧**：右半边是桌面 token 实跑终值基线（下方 `BASELINE`），左半边是移动端唯一
 *    token 入口 `./tokens.resolved.css`（契约 A1）解析出的同一组腿，两侧逐值对拍。
 * ✅ 第 2 条：入口相对 `BASELINE` **多出**的 token 必须命中契约 C 白名单前缀
 *    （`--tap-*` / `--safe-*` / `--shadow-sheet`），且入口必须**确实**带上 `MOBILE_REQUIRED`
 *    那几个——否则「没有多余项」会让白名单那条成为恒真断言。
 * ✅ 第 3 条：**四条腿**，比 §6.3.5 原文的三条多一条「跟随系统深色」。多这条不是凑数：
 *    §6.3.1 实测**只有那一条腿是 `tokens.css` 赢**（它的 media 腿多一个 `:not` ⇒ (0,3,0) >
 *    prototype 的 (0,2,0)），方向与其余三条相反，恰恰是最该钉住的一条。
 * ✅ 第 4 条：字体栈（`--disp` 首选族随包、字重区间覆盖在用字重、`--sans` 含 Android 可用 CJK 族）。
 *    2026-09-04 前这两条是 `it.fails` 标着的已知缺口，本轮随契约 B 落地转为正式断言。
 * ✅ 第 5 条：反向对照。桌面基线段、入口段、字体段各自带一条 `⓪`，用**同一套解析器**吃合成的
 *    违约输入，证明每条断言真的报得出错。**未证明有牙的门不计入验收**（§9 验收清单原话）。
 *
 * ── 为什么要一份「终值」基线，而不是直接读 tokens.css ───────────────────────
 * 桌面 token 的最终取值**不在 `tokens.css`**（§6.3.1）。`index.css` 的 @import 序为
 * tailwindcss → fonts.css → tokens.css → components.css → screens.css → prototype.css，
 * 其后才是 index.css 自有规则；`prototype.css` 自带一整套同选择器的 `:root` token 压过
 * `tokens.css`，`index.css` 的 `:root, :root[data-theme='light']` 又把四个语义色压回来，
 * 另一条 `:root` 把 `--sans` 压回来（契约 B 补的 CJK 族）。移动端按直觉只 `@import tokens.css`
 * 会拿到中间层的值，**没有任何报错**。基线钉的就是浏览器里真正生效的那一层。
 * `fonts.css` 只有一条 @font-face、不声明任何 token，故不进 `CASCADE`（它由第 4 条那段扫描）。
 *
 * ── 解析器为什么不复用 style-invariants.test.ts 的 `flat()` ─────────────────
 * `read` / `stripComments` 与它同义（那两个是模块私有的 const，不能 import，只能同形复制）。
 * 但 `flat()` 把 `@media` 拍平了 —— 而 @media 上下文正是本门最容易翻车的地方：
 * `tokens.css` 的跟随系统腿是 `:root:not([data-theme='light']):not([data-theme='dark'])`（0,3,0），
 * `prototype.css` 的对应腿是 `:root:not([data-theme="light"])`（0,2,0）——这一腿是 tokens.css 赢，
 * 与其它腿方向相反。若丢掉 @media 归属，prototype 的 (0,2,0) 深色腿会盖过 `:root` 的 (0,1,0)，
 * 浅色基线会整张变成深色值。故这里必须用带块嵌套的走查，而不是正则拍平。
 *
 * 另：声明必须**按 `;` 切**，不能按行切。`prototype.css` 是多声明同行的紧凑格式
 * （`--bg:210 30% 96%; --surface:0 0% 100%; …` 在同一行），`grep -m1` 到该行再用 sed 剥掉冒号前缀，
 * 会静默取到该行**最后一个**声明的值（实测踩过，得到过一整张错误的对照表）。
 *
 * ── 射程边界 ────────────────────────────────────────────────────────────────
 * 基线只覆盖 Polaris 自有的五个样式文件在**根元素**上声明的自定义属性。
 * `@import 'tailwindcss'` 注入的 Tailwind theme 变量不在射程内（不是设计 token，移动端也不靠它对齐视觉）。
 * 后代选择器（`:root[data-theme="dark"] .toast{…}`）里的自定义属性同样不在射程内——它们不作用于根元素。
 */
import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve as resolvePath } from 'node:path';

const read = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const abs = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));
/** 去掉 CSS 注释，避免注释里的示例声明被当成真实声明命中。 */
const stripComments = (src: string) =>
  src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');

// ── 层叠链 ────────────────────────────────────────────────────────────────────
/** 源序 = index.css 的 @import 顺序 + index.css 自身（自有规则在全部 @import 之后）。 */
const CASCADE = ['./tokens.css', './components.css', './screens.css', './prototype.css', './index.css'] as const;

// ── CSS 走查（保留 @media 归属）───────────────────────────────────────────────
type Rule = { file: string; sel: string; body: string; at: string[]; order: number };

/** 按花括号配对走查，规则带上它所处的 at-rule 前奏栈（`@media …` 等）。 */
function walk(css: string, file: string, out: Rule[], at: string[], seq: { n: number }): void {
  let i = 0;
  let buf = '';
  while (i < css.length) {
    const c = css[i];
    if (c === '{') {
      const prelude = buf.trim().replace(/\s+/g, ' ');
      let depth = 1;
      let j = i + 1;
      while (j < css.length && depth > 0) {
        if (css[j] === '{') depth++;
        else if (css[j] === '}') depth--;
        j++;
      }
      const inner = css.slice(i + 1, j - 1);
      if (prelude.startsWith('@')) {
        // 条件组：递归并把前奏压栈。@font-face / @keyframes 不含嵌套规则，不递归。
        if (/^@(media|supports|layer|container)\b/i.test(prelude)) walk(inner, file, out, at.concat(prelude), seq);
      } else {
        out.push({ file, sel: prelude, body: inner, at, order: seq.n++ });
      }
      buf = '';
      i = j;
    } else if (c === '}' || c === ';') {
      // `;` 收尾的 at-rule（@import / @charset / @config）不产生块，清掉缓冲避免被并入下条选择器。
      buf = '';
      i++;
    } else {
      buf += c;
      i++;
    }
  }
}

/**
 * 一组「文件名 → CSS 源文」走查成规则表。`order` 跨源连号 ⇒ 同特异性时源序在后者胜。
 *
 * 之所以吃**源文**而不是只吃路径：反向对照要把「变异过的入口」喂进**同一个**解析器
 * （见各段 `⓪`）。若变异只能靠改磁盘上的文件来做，那门就得写盘再回滚，一旦中途失败
 * 就把违约的取值留在了工作区。
 */
type Src = { file: string; css: string };
function rulesOf(srcs: readonly Src[]): Rule[] {
  const out: Rule[] = [];
  const seq = { n: 0 };
  for (const s of srcs) walk(s.css, s.file, out, [], seq);
  return out;
}
const srcOf = (rel: string): Src => ({ file: rel, css: stripComments(read(rel)) });

const ALL_RULES: Rule[] = rulesOf(CASCADE.map(srcOf));

/** 声明按 `;` 切（不是按行切），后声明覆盖同块内的前声明。 */
function declsOf(body: string): Record<string, string> {
  const m: Record<string, string> = {};
  const re = /(?:^|;)\s*(--[A-Za-z0-9_-]+)\s*:\s*([^;]*)/g;
  let x: RegExpExecArray | null;
  while ((x = re.exec(body))) m[x[1]] = x[2].replace(/\s+/g, ' ').trim();
  return m;
}

// ── 三条腿 ────────────────────────────────────────────────────────────────────
type Env = { theme: null | 'dark' | 'light'; prefers: 'light' | 'dark' };
/**
 * 门覆盖的**四条腿**（§6.3.5 第 3 条要求三条，这里多钉一条，理由见文件头）。
 * `:root` 腿取「无 data-theme + 系统浅色」，即浅色默认；`:root@系统深色` 是同一个选择器
 * 在系统偏好为深色时的另一条路径 —— 用户 uiTheme 选 'system' 时走的就是它。
 */
const LEGS: Record<string, Env> = {
  ':root': { theme: null, prefers: 'light' },
  ':root@prefers-color-scheme:dark': { theme: null, prefers: 'dark' },
  ":root[data-theme='dark']": { theme: 'dark', prefers: 'dark' },
  ":root[data-theme='light']": { theme: 'light', prefers: 'light' },
};

/** 只认「作用于根元素」的单复合选择器：`:root` + 任意个 `[data-theme=x]` / `:not([data-theme=x])`。 */
const ROOT_COMPOUND = /^:root(?:(?::not\(\[data-theme=["'][a-z]+["']\]\))|(?:\[data-theme=["'][a-z]+["']\]))*$/;

function parseRootCompound(sel: string): { spec: number; eq: string[]; ne: string[] } | null {
  if (!ROOT_COMPOUND.test(sel)) return null;
  const eq: string[] = [];
  const ne: string[] = [];
  // `:root` 本身贡献 1（伪类）；每个属性选择器 / `:not([attr])` 各贡献 1（:not() 取其参数的特异性）。
  let spec = 1;
  for (const p of sel.matchAll(/:not\(\[data-theme=["']([a-z]+)["']\]\)|\[data-theme=["']([a-z]+)["']\]/g)) {
    spec++;
    if (p[1] !== undefined) ne.push(p[1]);
    else eq.push(p[2]);
  }
  return { spec, eq, ne };
}

/** at-rule 栈是否在该环境下命中。不认识的条件组一律抛错（宁可自曝，不要静默算错）。 */
function atRuleApplies(at: string[], env: Env): boolean {
  for (const q of at) {
    const m = q.match(/prefers-color-scheme\s*:\s*(dark|light)/);
    if (!m) throw new Error(`token 声明落在本门未建模的条件组里，解析器需要扩展：${q}`);
    if (env.prefers !== m[1]) return false;
  }
  return true;
}

/** 解析某条腿上全部根级自定义属性的终值（特异性优先，同特异性取源序在后者）。 */
function resolveLeg(env: Env, rules: readonly Rule[] = ALL_RULES): Record<string, string> {
  const win: Record<string, { spec: number; order: number; val: string }> = {};
  for (const r of rules) {
    const d = declsOf(r.body);
    if (Object.keys(d).length === 0) continue;
    if (!atRuleApplies(r.at, env)) continue;
    let spec = -1;
    for (const part of r.sel.split(',').map((s) => s.trim())) {
      // 后代/兄弟选择器不作用于根元素（`:root[data-theme="dark"] .toast` 之类），跳过。
      if (/[ >+~]/.test(part)) continue;
      const c = parseRootCompound(part);
      if (!c) {
        // 单复合、又声明了自定义属性、还认不出来 ⇒ 可能作用于根元素而被漏算。必须自曝，不能静默跳过。
        if (/^(:root|html\b|\*|:where|:is)/.test(part))
          throw new Error(`根级 token 选择器无法归类，解析器需要扩展：${r.file} "${part}"`);
        continue; // `.foo{--x}` 之类：不是根元素，正常跳过。
      }
      if (c.eq.every((v) => env.theme === v) && c.ne.every((v) => env.theme !== v)) spec = Math.max(spec, c.spec);
    }
    if (spec < 0) continue;
    for (const [k, v] of Object.entries(d)) {
      const cur = win[k];
      if (!cur || spec > cur.spec || (spec === cur.spec && r.order >= cur.order)) win[k] = { spec, order: r.order, val: v };
    }
  }
  return Object.fromEntries(
    Object.entries(win)
      .sort(([a], [b]) => (a < b ? -1 : 1))
      .map(([k, v]) => [k, v.val]),
  );
}

// ── checked-in 基线：桌面实跑终值 ─────────────────────────────────────────────
/**
 * 生成方式：由本文件的解析器算出，逐值人工核对过 —— 四个语义色与 §6.3.1 表格的「实跑」列一致
 * （`--ok` 152 60% 27% / `--warn` 32 84% 31% / `--err` 356 68% 44% / `--dn` 197 80% 33%），
 * 都不是 `tokens.css` 或 `prototype.css` 的声明值。
 *
 * 取值形态就是**胜出那条声明的原文**（空白折叠后）：`--disp` / `--mono` 是 `prototype.css` 的
 * 双引号紧凑写法，不是 `tokens.css` 的单引号折行写法 —— 因为浏览器里生效的是前者。
 * `--sans` 是**第三种**形态：契约 B 补 CJK 族后，胜出的那条在 `index.css` 的 @import 之后
 * （单引号、逗号后带空格）。三个字体栈 token 三种写法，正是「取值形态跟着胜出者走」的实证。
 *
 * 改基线的唯一正当理由：桌面设计 token 有意演进，且改动已在桌面侧落地。
 * 「移动端对不上所以把基线调过去」是契约 C 的违约（只许加，不许改）。
 */
const LIGHT: Record<string, string> = {
  '--aurora': '162 78% 33%',
  '--aurora-hi': '163 82% 27%',
  '--aurora-weak': '162 55% 92%',
  '--bg': '210 30% 96%',
  '--disp': '"Space Grotesk","Avenir Next","SF Pro Display","Segoe UI",var(--sans)',
  '--dn': '197 80% 33%',
  '--err': '356 68% 44%',
  '--err-weak': '356 74% 96%',
  '--fg': '220 32% 13%',
  '--fg-dim': '216 15% 40%',
  '--fg-faint': '214 14% 44%',
  '--flow': '197 88% 40%',
  '--flow-hi': '198 92% 33%',
  '--flow-weak': '196 70% 93%',
  '--hair': '213 22% 83%',
  '--indigo': '234 60% 40%',
  '--line': '213 22% 87%',
  '--logo-tile': '0 0% 100%',
  '--logo-tile-bd': '214 24% 80%',
  '--mono': 'ui-monospace,"SF Mono","JetBrains Mono","Cascadia Code",Menlo,Consolas,monospace',
  '--ok': '152 60% 27%',
  '--ok-weak': '152 48% 92%',
  '--pending-bar-h': '36px',
  '--r': '11px',
  '--r-lg': '14px',
  '--r-md': '10px',
  '--r-sm': '8px',
  '--r-xs': '6px',
  '--ring': '197 88% 44%',
  '--sans':
    "-apple-system, 'Segoe UI Variable', 'Segoe UI', 'PingFang SC', 'Hiragino Sans GB', 'Noto Sans CJK SC', 'Microsoft YaHei', system-ui, sans-serif",
  '--shadow': '216 45% 20%',
  '--shadow-pop': '0 18px 40px -16px hsl(var(--shadow)/0.6)',
  '--sp-1': '4px',
  '--sp-2': '8px',
  '--sp-3': '12px',
  '--sp-4': '16px',
  '--sp-5': '20px',
  '--sp-6': '24px',
  '--sp-7': '32px',
  '--statusbar-h': '32px',
  '--surface': '0 0% 100%',
  '--surface-2': '210 28% 94%',
  '--surface-3': '210 22% 89%',
  '--toast-gap': 'var(--sp-4)',
  '--unlock-dot-err-light': '356 82% 52%',
  '--unlock-dot-ok-light': '152 75% 36%',
  '--unlock-dot-warn-light': '32 92% 42%',
  '--up': '262 52% 54%',
  '--warn': '32 84% 31%',
  '--warn-weak': '38 84% 92%',
};

const DARK: Record<string, string> = {
  ...LIGHT,
  '--aurora': '161 60% 45%',
  '--aurora-hi': '162 68% 55%',
  '--aurora-weak': '162 45% 13%',
  '--bg': '220 40% 6%',
  '--dn': '195 82% 62%',
  '--err': '356 74% 66%',
  '--err-weak': '356 42% 17%',
  '--fg': '210 30% 95%',
  '--fg-dim': '214 16% 66%',
  '--fg-faint': '214 13% 48%',
  '--flow': '195 90% 56%',
  '--flow-hi': '194 96% 68%',
  '--flow-weak': '200 60% 15%',
  '--hair': '217 17% 28%',
  '--indigo': '236 64% 32%',
  '--line': '217 19% 23%',
  '--logo-tile': '213 26% 88%',
  '--logo-tile-bd': '214 18% 72%',
  '--ok': '152 50% 46%',
  '--ok-weak': '154 40% 14%',
  '--ring': '195 90% 60%',
  '--shadow': '224 80% 1%',
  '--surface': '219 32% 10.5%',
  '--surface-2': '218 26% 14%',
  '--surface-3': '217 22% 19.5%',
  '--up': '265 70% 74%',
  '--warn': '36 88% 60%',
  '--warn-weak': '34 45% 14%',
};

/**
 * 两条浅色腿共用同一张表、两条深色腿共用同一张表，**本身就是四条断言**：
 * 显式浅色 ≡ 浅色默认，显式深色 ≡ 跟随系统深色（`tokens.css` 的「主题两档同步」注释、
 * `index.css` 的双选择器写法都建立在这个前提上）。哪天它们真分叉了，这里会红，
 * 届时该拆表的是人，不是让门闭嘴。
 *
 * 深色两条腿共表这一条尤其值钱：它们在桌面上分别由 `tokens.css`(0,3,0) 与
 * `prototype.css`(0,2,0) 赢下，来源不同却必须同值 —— §6.3.1 说的「四条主题路径里
 * tokens.css 只赢一条」正是在这里被钉住。
 */
const BASELINE: Record<string, Record<string, string>> = {
  ':root': LIGHT,
  ':root@prefers-color-scheme:dark': DARK,
  ":root[data-theme='dark']": DARK,
  ":root[data-theme='light']": LIGHT,
};

describe('§6.3.5 第 1 条右半边：桌面 token 终值基线（四条腿逐值对拍）', () => {
  it('index.css 的 @import 序与解析器假设的层叠链一致（基线的前提，序一变整张表失真）', () => {
    const imports = [...stripComments(read('./index.css')).matchAll(/@import\s+['"]([^'"]+)['"]/g)].map((m) => m[1]);
    // tailwindcss 是裸包名、fonts.css 只带一条 @font-face，两者都不在 CASCADE 里（射程边界见文件头）；
    // 其余四个必须按 CASCADE 顺序出现，且中间不许插入别的 @import（整表 toEqual，不是子序列匹配）。
    expect(imports).toEqual([
      'tailwindcss',
      './fonts.css',
      './tokens.css',
      './components.css',
      './screens.css',
      './prototype.css',
    ]);
  });

  for (const [leg, env] of Object.entries(LEGS)) {
    it(`${leg} 的全部根级 token 终值等于基线`, () => {
      expect(resolveLeg(env)).toEqual(BASELINE[leg]);
    });
  }

  /**
   * ⓪ 反向对照（§6.3.5 第 5 条）—— 证明上面那四条不是「解析器把什么都算成基线」。
   *
   * 两个变异靶各打一处**不同的层**，且都先证明「变异确实打上了」（needle 在剥注释后的源文里
   * 恰好命中一次），再看门的反应：
   *   ① 覆盖层（index.css 的 @import 后 `:root`）：改一个明度点 ⇒ `:root` 腿必须红。
   *      这条钉的是「基线读的是覆盖层」——若解析器错读成 tokens.css 或 prototype.css 的声明值，
   *      改覆盖层就不会有反应，门也就一直绿着放行一个丢掉无障碍校准的移动端。
   *   ② 声明层的**跟随系统深色腿**（tokens.css 的 @media 块）：改一个明度点 ⇒
   *      `:root@系统深色` 腿变、`[data-theme='dark']` 腿**不变**。
   *      这条钉的是 @media 归属真的被解析。文件头写过：若把 @media 拍平，prototype 的 (0,2,0)
   *      深色腿会盖过 `:root` 的 (0,1,0)，浅色基线会整张变成深色值 —— 那种失效模式下这两条腿
   *      会一起动，一眼可辨。
   */
  it('⓪ 反向对照：两处层叠变异分别打红对应的腿，且不误伤别的腿', () => {
    // ── ① 覆盖层 ──
    const indexCss = stripComments(read('./index.css'));
    const okNeedle = '--ok: 152 60% 27%;';
    expect(indexCss.split(okNeedle).length - 1, `index.css 剥注释后 \`${okNeedle}\` 不是恰好 1 处 —— 变异靶变形了`).toBe(1);
    const mutIndex = rulesOf([...CASCADE.slice(0, -1).map(srcOf), { file: './index.css', css: indexCss.replace(okNeedle, '--ok: 152 60% 28%;') }]);
    expect(resolveLeg(LEGS[':root'], mutIndex)['--ok'], '改了覆盖层的 --ok，`:root` 腿却没跟着变 —— 基线读的不是覆盖层').toBe('152 60% 28%');
    expect(resolveLeg(LEGS[':root'], mutIndex)).not.toEqual(LIGHT);

    // ── ② 声明层的跟随系统深色腿 ──
    const tokensCss = stripComments(read('./tokens.css'));
    const mediaAt = tokensCss.indexOf('@media (prefers-color-scheme: dark)');
    expect(mediaAt, 'tokens.css 的跟随系统深色腿不见了 —— 变异靶没了，本条不构成证据').toBeGreaterThan(-1);
    const bgNeedle = '--bg: 220 40% 6%;';
    const tail = tokensCss.slice(mediaAt);
    expect(tail.split(bgNeedle).length - 1, `tokens.css 的 @media 块里 \`${bgNeedle}\` 不是恰好 1 处`).toBe(1);
    const mutTokens = rulesOf([
      { file: './tokens.css', css: tokensCss.slice(0, mediaAt) + tail.replace(bgNeedle, '--bg: 220 40% 7%;') },
      ...CASCADE.slice(1).map(srcOf),
    ]);
    expect(resolveLeg(LEGS[':root@prefers-color-scheme:dark'], mutTokens)['--bg'], '改了 tokens.css 的 @media 深色腿，跟随系统腿却没变 —— 那条腿的胜出关系没被解析对').toBe('220 40% 7%');
    expect(resolveLeg(LEGS[":root[data-theme='dark']"], mutTokens)['--bg'], '只改了 @media 腿，显式深色腿却跟着变了 —— @media 归属被拍平了').toBe('220 40% 6%');
    expect(resolveLeg(LEGS[':root'], mutTokens), '只改了深色腿，浅色基线却动了').toEqual(LIGHT);
  });

  it('四个语义色的浅色终值来自 index.css 的 @import 后覆盖，不是 tokens.css / prototype.css 的声明', () => {
    const light = resolveLeg(LEGS[':root']);
    // §6.3.1：prototype.css 的浅色语义色明度高 5–9 点，它进不了浏览器。
    // 直接钉「终值 ≠ prototype 声明值」，比只钉数值更能解释这道门在防什么。
    const protoLightRoot = ALL_RULES.find((r) => r.file === './prototype.css' && r.sel === ':root' && r.at.length === 0);
    expect(protoLightRoot, 'prototype.css 的浅色 :root token 块不见了？').toBeDefined();
    const proto = declsOf(protoLightRoot!.body);
    for (const k of ['--ok', '--warn', '--err', '--dn']) {
      expect(proto[k], `${k} 不在 prototype.css 的浅色 :root 块里，前提校验失败`).toBeDefined();
      expect(light[k], `${k} 的终值退回到了 prototype.css 的中间层取值（无障碍校准被吃掉）`).not.toBe(proto[k]);
    }
    expect([light['--ok'], light['--warn'], light['--err'], light['--dn']]).toEqual([
      '152 60% 27%',
      '32 84% 31%',
      '356 68% 44%',
      '197 80% 33%',
    ]);
  });
});

// ── §6.3.5 第 1 条左半边 + 第 2 条：移动端 token 入口（契约 A1 / C）──────────────
/** 移动端唯一 token 入口。契约 A1：既不是 `tokens.css`，也不是整条 `index.css` 层叠链。 */
const MOBILE_ENTRY = './tokens.resolved.css';
/** 契约 C 白名单前缀（§6.3.3）：移动端只许新增这三类，改任何既有 token 的取值即违约。 */
const MOBILE_ONLY = /^--tap-|^--safe-|^--shadow-sheet$/;
/**
 * 入口**必须**带上的移动端专属 token（§6.3.2③ 点名的两类缺口：触控目标与 safe-area）。
 * 没有这条正面断言，白名单那条会被「一个多余 token 都没有」骗过去 —— 空集永远满足白名单，
 * 那样第 2 条就成了零信息量的恒真断言。
 * 这里用「必须含」而不是「恰好等于」：将来按契约 C 合法新增 `--shadow-sheet` 之类不该逼人改门。
 */
const MOBILE_REQUIRED = ['--tap-min', '--safe-t', '--safe-r', '--safe-b', '--safe-l'];

/** 把一条腿的解析结果切成「基线里有的」与「多出来的」两半。 */
function split(resolved: Record<string, string>, base: Record<string, string>) {
  const shared: Record<string, string> = {};
  const extra: Record<string, string> = {};
  for (const [k, v] of Object.entries(resolved)) (k in base ? shared : extra)[k] = v;
  return { shared, extra };
}
/** 解析入口的某条腿。反向对照传入变异过的源文，走的是**同一条**解析路径。 */
const entryLeg = (leg: string, css: string = srcOf(MOBILE_ENTRY).css) =>
  resolveLeg(LEGS[leg], rulesOf([{ file: MOBILE_ENTRY, css }]));

describe('§6.3.5 第 1 条左半边 + 第 2 条：移动端 token 入口（契约 A1 / C）', () => {
  it('自检：入口在、解析得出规则、四条腿都不是空集（空集会让下面每条断言恒绿）', () => {
    expect(existsSync(abs(MOBILE_ENTRY)), `${MOBILE_ENTRY} 不存在 —— 契约 A1 的入口没了`).toBe(true);
    expect(rulesOf([srcOf(MOBILE_ENTRY)]).length, '入口一条规则都没解析出来').toBeGreaterThan(0);
    for (const leg of Object.keys(LEGS)) {
      expect(Object.keys(entryLeg(leg)).length, `${leg} 腿解析出的 token 太少 —— 十有八九是选择器写法没被解析器认出来`).toBeGreaterThan(40);
    }
  });

  it('契约 A：入口不得走桌面层叠链，只许 @import 随包字体面', () => {
    const imports = [...stripComments(read(MOBILE_ENTRY)).matchAll(/@import\s+['"]([^'"]+)['"]/g)].map((m) => m[1]);
    // 为什么要单独钉这一条：入口若 `@import './tokens.css'`，四条腿的**终值仍然对**
    //（后面的扁平声明压过它），门照绿 —— 但移动端包里从此躺着一份声明层，且下一个人会以为
    // 那份才是真值源。这是一条只在打包体积与阅读路径上出问题、在取值上完全不出声的违约。
    expect(imports, '入口的 @import 集合变了 —— 只许有随包字体面 fonts.css').toEqual(['./fonts.css']);
  });

  for (const leg of Object.keys(LEGS)) {
    it(`${leg}：入口解析出的既有 token 逐值等于桌面实跑终值`, () => {
      expect(split(entryLeg(leg), BASELINE[leg]).shared).toEqual(BASELINE[leg]);
    });
  }

  it('第 2 条：多出的 token 全部命中契约 C 白名单，且必备的移动端 token 确实在', () => {
    for (const leg of Object.keys(LEGS)) {
      const extra = Object.keys(split(entryLeg(leg), BASELINE[leg]).extra).sort();
      expect(
        extra.filter((k) => !MOBILE_ONLY.test(k)),
        `${leg} 上出现了契约 C 白名单（--tap-* / --safe-* / --shadow-sheet）之外的新 token`,
      ).toEqual([]);
    }
    const rootExtra = Object.keys(split(entryLeg(':root'), LIGHT).extra);
    for (const k of MOBILE_REQUIRED) {
      expect(rootExtra, `入口缺 ${k} —— §6.3.2③ 点名的移动端必需 token 没落地`).toContain(k);
    }
  });

  /**
   * ⓪ 反向对照（§6.3.5 第 5 条）。六个变异靶，每个都先证明**变异确实打上了**
   * （needle 在剥注释后的源文里命中次数已知），再看门的反应。没有这一条，上面那堆 `toEqual`
   * 只能证明「没崩」，证明不了「有牙」。
   */
  it('⓪ 反向对照：六类违约喂进同一套解析/比对，各自报得出错', () => {
    const css = srcOf(MOBILE_ENTRY).css;
    const shared = (mut: string, leg = ':root') => split(entryLeg(leg, mut), BASELINE[leg]).shared;
    const extra = (mut: string, leg = ':root') => split(entryLeg(leg, mut), BASELINE[leg]).extra;
    /** 变异收据：needle 必须命中恰好 n 次，且替换后源文确实变了。 */
    const mutate = (needle: string, next: string, n = 1) => {
      expect(css.split(needle).length - 1, `入口剥注释后 \`${needle}\` 不是恰好 ${n} 处 —— 变异靶变形了，本条不构成证据`).toBe(n);
      const out = css.replace(needle, next);
      expect(out, `\`${needle}\` 的替换没生效`).not.toBe(css);
      return out;
    };

    // 前提：未变异的入口本身是绿的。少了这句，下面每条「红了」都可能只是「本来就红」。
    expect(shared(css), '未变异的入口就对不上基线 —— 下面的红不构成证据').toEqual(LIGHT);

    // ① 改一个明度点。契约 C 明写的典型违约（「只是稍微调亮一点以适应户外强光」）。
    const m1 = mutate('--ok: 152 60% 27%;', '--ok: 152 60% 28%;');
    expect(shared(m1)['--ok']).toBe('152 60% 28%');
    expect(shared(m1)).not.toEqual(LIGHT);

    // ② 漏搬一个 token（不是改值）。`toEqual` 对缺键同样要红。
    const m2 = mutate('--warn-weak: 38 84% 92%;', '');
    expect(Object.keys(shared(m2))).not.toContain('--warn-weak');
    expect(shared(m2)).not.toEqual(LIGHT);

    // ③ 字体栈把单引号换成双引号：CSS 语义等价，但**不是同一份声明**。
    //    门比的是胜出声明的原文，这一条钉的正是文件头那段「取值形态要逐字照抄」。
    const m3 = mutate("'Noto Sans CJK SC'", '"Noto Sans CJK SC"');
    expect(shared(m3)).not.toEqual(LIGHT);

    // ④ 白名单外的新 token。
    const m4 = mutate('--tap-min: 48px;', '--tap-min: 48px;\n  --brand-outdoor-boost: 1.2;');
    expect(Object.keys(extra(m4)).filter((k) => !MOBILE_ONLY.test(k))).toEqual(['--brand-outdoor-boost']);

    // ⑤ 删掉必备的 `--tap-min` ⇒ 第 2 条的正面断言必须失守（证明它不是恒真）。
    const m5 = mutate('--tap-min: 48px;', '');
    expect(Object.keys(extra(m5))).not.toContain('--tap-min');

    // ⑥ 两条深色腿逐字重复，改一条漏一条必须被抓到：动**显式**深色腿（源文里最后那处 `--bg`），
    //    显式腿红、跟随系统腿绿。若门只对拍一条深色腿，这里会有一条判不出来。
    const at = css.lastIndexOf('--bg: 220 40% 6%;');
    expect(at, '入口里找不到深色 --bg —— 变异靶没了').toBeGreaterThan(-1);
    const m6 = css.slice(0, at) + '--bg: 220 40% 7%;' + css.slice(at + '--bg: 220 40% 6%;'.length);
    expect(m6).not.toBe(css);
    expect(shared(m6, ":root[data-theme='dark']")['--bg'], '改了显式深色腿却没被抓到').toBe('220 40% 7%');
    expect(shared(m6, ':root@prefers-color-scheme:dark')['--bg'], '只改了显式深色腿，跟随系统腿却跟着变了').toBe('220 40% 6%');
  });
});

// ── §6.3.5 第 4 条：字体栈 ────────────────────────────────────────────────────
/** 顶层逗号切分（括号与引号内的逗号不算），用于拆 font-family 列表。 */
function splitTopLevel(value: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote = '';
  let cur = '';
  for (const ch of value) {
    if (quote) {
      if (ch === quote) quote = '';
      cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      cur += ch;
    } else if (ch === '(') {
      depth++;
      cur += ch;
    } else if (ch === ')') {
      depth--;
      cur += ch;
    } else if (ch === ',' && depth === 0) {
      out.push(cur.trim());
      cur = '';
    } else cur += ch;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}
const unquote = (s: string) => s.replace(/^['"]|['"]$/g, '').trim();
/** font-family 名比较：忽略大小写与多余空白（CSS 里族名大小写不敏感）。 */
const sameFamily = (a: string, b: string) => a.toLowerCase().replace(/\s+/g, ' ') === b.toLowerCase().replace(/\s+/g, ' ');

type FontFace = { family: string; srcs: string[]; weight: string; dir: string };
/** 抽出 @font-face 的族名、src url 列表与字重声明（含所在目录，用于判定「随包」而非远端拉取）。 */
function fontFaces(css: string, dir: string): FontFace[] {
  return [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].flatMap((m) => {
    const body = m[1];
    const fam = body.match(/font-family\s*:\s*([^;]+)/);
    if (!fam) return [];
    return [
      {
        family: unquote(fam[1].trim()),
        srcs: [...body.matchAll(/url\(\s*(['"]?)([^'")]+)\1\s*\)/g)].map((u) => u[2].trim()),
        weight: (body.match(/font-weight\s*:\s*([^;]+)/)?.[1] ?? '').replace(/\s+/g, ' ').trim(),
        dir,
      },
    ];
  });
}
/** 「随包」= 本地相对路径且文件真实存在；远端 URL（Google Fonts 之类）不算。 */
const isBundled = (f: FontFace) =>
  f.srcs.some((u) => !/^(https?:)?\/\//.test(u) && !u.startsWith('data:') && existsSync(resolvePath(f.dir, u.split('?')[0])));
/**
 * `@font-face` 的字重区间。单值（静态 face 的写法）返回 `[w, w]`，缺省按 CSS 规范算 `[400, 400]`。
 * 为什么要判它：设计交接包 `~/Code/polaris/design/mobile/tokens/core.json` 的
 * `type.cardTitle` / `type.pill` / `type.ringValue` 用的是 **640**，不是标准静态字重；
 * 且 `index.css` 的 `:root{font-synthesis:none}` 关掉了合成 ⇒ 静态 face 会把 500/600/640
 * 静默塌到同一档。D-9 选 variable 的**全部理由**就是这一条，不钉住它，将来换成静态 face 无人可知。
 */
function weightRange(f: FontFace): [number, number] | null {
  if (!f.weight) return [400, 400];
  const n = f.weight.split(/\s+/).map(Number);
  if (n.length < 1 || n.length > 2 || n.some((v) => !Number.isFinite(v))) return null;
  return [n[0], n[n.length - 1]];
}

/**
 * 取材面：`ui/src` 下**全部** .css（styles/ 五个 + tray-overlay.css + update-popup/style.css）。
 * `resources/dashboard/` 是第三方 dashboard 打包产物（IBM Plex / Schibsted Grotesk），不是应用外壳字体，不在面内。
 */
function appCssFiles(): string[] {
  const root = abs('..');
  return readdirSync(root, { recursive: true, encoding: 'utf8' })
    .filter((p) => p.endsWith('.css'))
    .map((p) => resolvePath(root, p))
    .sort();
}
const ALL_FACES: FontFace[] = appCssFiles().flatMap((p) => fontFaces(stripComments(readFileSync(p, 'utf8')), dirname(p)));

/** Android（含 WebView）自带或事实可用的中文族。`--sans` 至少要命中其一，否则安卓上掉栈。 */
const ANDROID_CJK = [
  'Noto Sans CJK SC',
  'Noto Sans SC',
  'Noto Sans CJK',
  'Source Han Sans SC',
  'Source Han Sans CN',
  'HarmonyOS Sans SC',
  'Droid Sans Fallback',
];

describe('§6.3.5 第 4 条：字体栈', () => {
  const light = resolveLeg(LEGS[':root']);

  /**
   * ⓪ 反向对照（§6.3.5 第 5 条）——证明下面三条正式断言是「真的在判」，而不是扫描器恒真。
   *
   * 2026-09-04 之前这两条是 `it.fails` 标着的**已知缺口**（全仓 0 个 @font-face、0 个字体文件，
   * `--sans` 里三个中文族在 Android 上全部缺席）。契约 B 落地后标记已删除、断言转正。
   * 保留这段历史是为了说明：`it.fails` 那种写法一旦真的通过就会报「expected to fail」转红，
   * 强制来人删标记 —— 缺口自曝、修复也自曝。绝不能反过来放宽断言让它变绿。
   *
   * 四个靶点，都用合成输入而非改盘上的文件：
   *   · 本地存在的 src   → 随包（正向）
   *   · 远端 URL          → 不随包（否则一个 Google Fonts 链接就能骗过第 1 条）
   *   · 本地但**文件不在** → 不随包（「有人删了 woff2、CSS 还留着」是最可能的退化路径，
   *                          它和远端 URL 是两种失效模式，只测远端会漏掉这一种）
   *   · 静态字重 / 无字重 → 区间退化成一点（否则第 2 条抓不到「换成静态 face」）
   */
  it('⓪ 反向对照：@font-face 扫描器对合成输入能分辨随包 / 不随包 / 静态字重', () => {
    expect(splitTopLevel(light['--disp']).map(unquote)[0]).toBe('Space Grotesk');
    expect(splitTopLevel(light['--sans']).map(unquote)).toContain('PingFang SC');

    const synth = (body: string) => fontFaces(`@font-face{${body}}`, abs('.'))[0];

    const local = synth(`font-family:'Space Grotesk';src:url("./tokens.css") format("woff2");font-weight:300 700;`);
    expect(sameFamily(local.family, 'Space Grotesk')).toBe(true);
    expect(isBundled(local), '本地存在的 src 应判为「随包」').toBe(true);
    expect(weightRange(local)).toEqual([300, 700]);

    expect(isBundled(synth(`font-family:'X';src:url(https://fonts.gstatic.com/x.woff2);`)), '远端 src 必须判为「不随包」').toBe(false);
    expect(isBundled(synth(`font-family:'X';src:url('./no-such-font.woff2') format('woff2');`)), '本地路径但文件不存在，必须判为「不随包」').toBe(false);

    // 静态 face 的两种写法：显式单值、以及压根不写（CSS 规范默认 400）。两者都必须落成一个点。
    expect(weightRange(synth(`font-family:'X';src:url(x.woff2);font-weight:600;`))).toEqual([600, 600]);
    expect(weightRange(synth(`font-family:'X';src:url(x.woff2);`))).toEqual([400, 400]);

    expect(ANDROID_CJK.some((f) => sameFamily(f, 'noto sans cjk sc'))).toBe(true);
  });

  it('§6.3.2① / 契约 B：--disp 的首选族 Space Grotesk 有随包的 @font-face', () => {
    const first = splitTopLevel(light['--disp']).map(unquote)[0];
    const hit = ALL_FACES.filter((f) => sameFamily(f.family, first));
    expect(hit.length, `${first} 没有任何 @font-face（当前 ui/src 下共 ${ALL_FACES.length} 个 @font-face）`).toBeGreaterThan(0);
    expect(hit.some(isBundled), `${first} 的 @font-face 没有随包的本地 src —— 远端拉取不算随包`).toBe(true);
  });

  it('D-9：随包的那份 Space Grotesk 是 variable，字重区间覆盖在用字重（含非标准的 640）', () => {
    const first = splitTopLevel(light['--disp']).map(unquote)[0];
    const bundled = ALL_FACES.filter((f) => sameFamily(f.family, first) && isBundled(f));
    expect(bundled.length, `${first} 没有随包的 face —— 上一条应已先红`).toBeGreaterThan(0);
    // 交接包 core.json 的在用字重：500 / 600 / 640 / 700。取 [500,700] 作判据，640 落在区间内。
    const covers = bundled.some((f) => {
      const r = weightRange(f);
      return r !== null && r[0] <= 500 && r[1] >= 700;
    });
    expect(
      covers,
      `${first} 的随包 face 字重区间为 [${bundled.map((f) => f.weight || '(缺省 400)').join(' | ')}]，` +
        '未覆盖 500–700。静态 face 会把 500/600/640 静默塌到同一档（index.css 关了 font-synthesis），' +
        '这正是 D-9 选 variable 的理由。',
    ).toBe(true);
  });

  it('§6.3.2② / 契约 B：--sans 含至少一个 Android 可用的 CJK 族', () => {
    const fams = splitTopLevel(light['--sans']).map(unquote);
    const hit = fams.filter((f) => ANDROID_CJK.some((c) => sameFamily(c, f)));
    expect(hit, `--sans 当前为 [${fams.join(', ')}]，无 Android 可用 CJK 族`).not.toHaveLength(0);
    // 位置也判：具名族排在通用族 `sans-serif` 之后，实践中不会再被走到（通用族一定解析得出）。
    const generic = fams.findIndex((f) => f === 'sans-serif');
    const cjkAt = fams.findIndex((f) => ANDROID_CJK.some((c) => sameFamily(c, f)));
    expect(generic, '--sans 末尾的通用族 sans-serif 不见了').toBeGreaterThan(-1);
    expect(cjkAt, `Android CJK 族排在通用族 sans-serif 之后，等于没写：[${fams.join(', ')}]`).toBeLessThan(generic);
  });
});
