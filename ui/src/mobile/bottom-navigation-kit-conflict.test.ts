/**
 * 底部导航上「kit 的 md 说 A、原型说 B」的**四个值** —— 代码侧那半门。
 *
 * 规格权威：`~/docs/polaris/design/mobile-kit/README.md`「Source of truth」——
 * 「Where this kit and the prototype disagree about a measurement, **the prototype wins and the
 * kit is wrong**」。原型是 `~/docs/polaris/design/prototype/mobile-home-breakpoints.html`。
 *
 * ── 这道门要挡的是什么 ──────────────────────────────────────────────────────
 * `component-specs/bottom-navigation.md` 的 Dimensions 两行（Icon visual / Active indicator）与
 * Colors/tokens 两行（Top divider / Inactive）写着四个**被判为错**的值。**引法不带行号**：那份 md
 * 不进版本控制，一次编辑就让行号漂掉，而漂掉的引用会让下一个人对不上、转头相信 md 本身。
 * 那四行已在 kit 里就地标注了「与原型冲突，以原型为准」，但 kit 住在 `~/docs` 的 vault 里、
 * **不进本仓的版本控制**：标注是一段文字，对下一个人没有强制力。本仓的判据纪律恰恰是
 * 「判据由代码持有 —— 文档里的表在不同天会被执行成不同结果」。
 *
 * 于是回改路径是敞开的：下一个人（或下一次 AI 复核）按规范权威顺序去读 component-spec，读到
 * 「Icon visual: 24」而代码是 23，判定为实现偏离，把 `--nav-icon` 改回 24px —— 全量测试
 * 1400+ 条照样全绿，PR 合入，四条里任意一条都能这么被翻回去。**本文件就是那道会当场转红的门。**
 *
 * ── 为什么要收全部声明，而不是取"第一条规则" ────────────────────────────────
 * 本轮复核在别处抓到过这个形态：`@container` 块写在**同名顶层规则之前**，被后面那条盖回去 ⇒
 * 断点覆盖永远不生效；而只取选择器首条规则的判据看不见第二条。故这里对每个 (选择器, 属性)
 * 收**全部**声明（含 `@media` / `@container` 体内的），并把条数钉成 1：
 *   · 0 条 = 取材面塌了或规则被删；
 *   · ≥2 条 = 有人加了覆写 —— 无论它排在前还是后、有没有条件，都必须过一次人眼。
 * ⓪ 里用一段合成 CSS 正面证明"块在前、顶层在后"这一形态确实被收成了两条。
 *
 * ── 射程如实登记 ────────────────────────────────────────────────────────────
 * 本文件读的是 `ui/src` 下**全部** `.css` 的源码文本，判的是"声明写成什么样"，不是
 * "浏览器算出来是什么"。故它挡得住「把值改回 md 那一版」与「另起一条规则盖掉它」，
 * 挡不住「换一个更高特异度的选择器另画一套」——那要真机或 headless 量计算样式，本批未做。
 *
 * "这几个类真的有 .tsx 在挂"不在本文件里再钉一遍：`mobile-entry.test.ts` ⑥ 已经断言外壳 CSS
 * 的每个类都有模块图内的 .tsx 消费它。两处各写一份的话，两份谓词分叉时没有门看得见。
 */
import { describe, expect, it } from 'vitest';
import { ALL, contextOf, resolve, type Contribution } from '@/styles/css-cascade.test-support';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

/** …/ui/ */
const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
const SRC_ROOT = join(UI_ROOT, 'src');
const rel = (abs: string) => relative(UI_ROOT, abs).split(sep).join('/');

/** `ui/src` 下全部 `.css`，已排序。走目录树而不是写死清单：新加的 CSS 自动进取材面。 */
function cssFiles(): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir).sort()) {
      const full = join(dir, entry);
      if (statSync(full).isDirectory()) walk(full);
      else if (entry.endsWith('.css')) out.push(full);
    }
  };
  walk(SRC_ROOT);
  return out;
}

/**
 * 取材面 = `ui/src` 下全部 `.css`，走**共享解析器** `styles/css-cascade.test-support.ts`。
 *
 * 换掉的那份本地解析器有两个洞，2026-09-05 复核实测都能溜过去：
 *  · **按属性名精确配对**：`.m-dock` 那条争议值登记的是简写 `border-top`，而判据是
 *    `d.prop === 'border-top'` ⇒ 补一句 `.m-dock{ border-top-color: hsl(var(--hair)) }`
 *    就能把这条线换成 kit 那个被否决的值，而门数出来仍是「恰好 1 条、值没变」，全绿。
 *    同理 `.m-nav-icon{ width }` 看不见 `inline-size`。共享解析器把简写/长写/逻辑属性折成
 *    同一个**语义分量**键，登记表随之按分量拆开（`border-top` → width/style/color 三条）。
 *  · **注释靠正则剥**：本地那份用 `/\/\*[\s\S]*?\*\//g`，字符串里的 `/*` 会被当成注释起点。
 *    共享解析器把注释与字符串一起走词法，且**保长度** ⇒ 每条声明还带得出 `file:line`。
 *
 * 取材面**一格没缩**：仍是走目录树发现的全部 `.css`（新加的自动进面），只是喂给了共享解析器。
 */
const FILES = cssFiles();
const CSS = contextOf(
  FILES.map((abs) => ({ file: rel(abs), css: readFileSync(abs, 'utf8') })),
  'ui/src/**/*.css',
);
const DECLS = CSS.decls;

/**
 * 某 (选择器, **语义分量**) 的全部贡献声明，按进包顺序。条件块里的一样收（`where: ALL`）。
 *
 * 返回的是**贡献**不是声明：`value` 是那条声明为这个分量贡献的值（简写已展开到分量），
 * `decl` 是它出自哪一条。`border-top: 1px solid …` 对 `border-top-color` 的贡献是
 * `hsl(var(--line))`，不是整句简写 —— 登记表钉的是前者。
 *
 * 选择器在整个取材面里一条规则都没命中 ⇒ **抛**（旧写法返回空数组，其上的否定断言恒真）。
 */
const chain = (sel: string, prop: string): Contribution[] =>
  resolve({ sel, prop, decls: CSS, where: ALL }).chain;

// ════════════════════════════════════════════════════════════════════════════
// 登记表：四个争议值 + 三个消费点
// ════════════════════════════════════════════════════════════════════════════

/** 一条争议：kit 的 md 写 `md`，原型写 `proto`，代码必须是 `value`。 */
interface Disputed {
  readonly sel: string;
  readonly prop: string;
  readonly value: string;
  /** md 那一行（写明行号，好让下一个人当场对上）。 */
  readonly md: string;
  /** 原型那一行 —— 判据的真值源。 */
  readonly proto: string;
  /** 采原型这一侧的旁证；没有旁证就写清"为什么原型是唯一真值源"。 */
  readonly why: string;
}

const DISPUTED: readonly Disputed[] = [
  {
    sel: ':root',
    prop: '--nav-icon',
    value: '23px',
    md: 'bottom-navigation.md 的 Dimensions →「Icon visual: 24」',
    proto: 'mobile-home-breakpoints.html:163 `.nav svg{width:23px;height:23px;}`',
    why:
      'kit 自己的机读侧也是 23（`ui-manifest.json` bottom-navigation `dimensions.icon: 23`、' +
      '`tokens/core.json` `layout.iconVisualSize: 23`）—— md 的 24 是 kit 里唯一的孤证',
  },
  {
    sel: ':root',
    prop: '--nav-indicator-w',
    value: '28px',
    md: 'bottom-navigation.md 的 Dimensions →「Active indicator: 2 high and approximately 40 wide」',
    proto: 'mobile-home-breakpoints.html:162 `.nav a.on::before{…width:28px;height:2px;…}`',
    why:
      '`ui-manifest.json` 只登记了 `indicatorHeight: 2`，**没有登记宽度** ⇒ 指示条宽度的唯一' +
      '真值源就是原型；md 那个 “approximately 40” 连近似值都不是同一档',
  },
  /*
   * 顶分隔线：`border-top: 1px solid hsl(var(--line))` 逐**分量**登记，不登记那句简写。
   * 只盯 `border-top` 的旧写法补一句 `border-top-color: hsl(var(--hair))` 就能把这条线换成
   * kit 那个被否决的值 —— 门照旧数出「恰好 1 条、值没变」。三个分量各一条，颜色那一格补得动的
   * 只有它自己，而它正是这条争议的本体。
   */
  {
    sel: '.m-dock',
    prop: 'border-top-width',
    value: '1px',
    md: 'bottom-navigation.md 的 Colors/tokens →「Top divider: `divider.default`」（= `--hair`）',
    proto: 'mobile-home-breakpoints.html:156 `border-top:1px solid hsl(var(--line))`（= `border.subtle`）',
    why: '线宽是这条争议的另一半：kit 与原型都写 1px，此处钉住它以免连粗细一起漂',
  },
  {
    sel: '.m-dock',
    prop: 'border-top-style',
    value: 'solid',
    md: 'bottom-navigation.md 的 Colors/tokens →「Top divider: `divider.default`」（= `--hair`）',
    proto: 'mobile-home-breakpoints.html:156 `border-top:1px solid hsl(var(--line))`（= `border.subtle`）',
    why: '线型同上；`none` / `dashed` 会让这条分隔线消失或换质感，与颜色是两件独立的事',
  },
  {
    sel: '.m-dock',
    prop: 'border-top-color',
    value: 'hsl(var(--line))',
    md: 'bottom-navigation.md 的 Colors/tokens →「Top divider: `divider.default`」（= `--hair`）',
    proto: 'mobile-home-breakpoints.html:156 `border-top:1px solid hsl(var(--line))`（= `border.subtle`）',
    why:
      '取 `--hair` 会让这条线重一档（深色 L 28% vs 23%、浅色 L 83% vs 87%），' +
      '导航区与内容区之间的切分比设计稿更硬',
  },
  {
    sel: '.m-nav-item',
    prop: 'color',
    value: 'hsl(var(--fg-faint))',
    md: 'bottom-navigation.md 的 Colors/tokens →「Inactive: `text.secondary`」（= `--fg-dim`）',
    proto: 'mobile-home-breakpoints.html:160 `.nav a{…color:hsl(var(--fg-faint))}`（= `text.tertiary`）',
    why:
      '取 `--fg-dim` 会把四个未选中项抬亮一档（深色 L 66% vs 48%），选中项那抹 `--flow` 与其余' +
      '四项的明度差被压窄 ——「现在在哪一屏」正是靠这个差一眼读出来的',
  },
];

/**
 * 消费点：token 定对了、消费点却写死数值，等于门在但没牙。
 * `(选择器, 属性, 必须读的变量)`。
 */
const CONSUMERS: readonly (readonly [string, string, string])[] = [
  ['.m-nav-icon', 'width', '--nav-icon'],
  ['.m-nav-icon', 'height', '--nav-icon'],
  ['.m-nav-item::before', 'width', '--nav-indicator-w'],
];

// ════════════════════════════════════════════════════════════════════════════

describe('⓪ 自检：解析器、剥注释、条件块都是活的（任一塌了下面全部恒绿）', () => {
  it('取材面非空，且 `mobile.css` 真的在里面', () => {
    expect(FILES.length, '`ui/src` 下一个 .css 都没扫到 —— 取材面塌了').toBeGreaterThan(8);
    expect(FILES.map(rel)).toContain('src/mobile/mobile.css');
    expect(DECLS.length, '一条声明都没解析出来 —— 解析器塌了').toBeGreaterThan(1000);
    // 反面：不存在的选择器**抛**，不是返回空数组 —— 空取材面上的否定断言恒真。
    expect(() => chain('.m-dock-that-never-existed', 'border-top-color')).toThrow(
      /一条规则都没命中/,
    );
    // 反面：同族但没人声明的分量，链是空的（而不是恰好返回别的分量的值）。
    expect(chain('.m-dock', 'border-bottom-color')).toEqual([]);
  });

  it('注释真的被掏掉了（`divider.default` / `text.secondary` 只活在 `mobile.css` 的注释里）', () => {
    const raw = readFileSync(join(SRC_ROOT, 'mobile', 'mobile.css'), 'utf8');
    const parsed = contextOf([{ file: 'mobile.css', css: raw }]).decls;
    const text = parsed.map((d) => `${d.prop}: ${d.value}`).join('\n');
    expect(parsed.length, 'mobile.css 一条声明都没解析出来 —— 下面那条恒真').toBeGreaterThan(100);
    for (const needle of ['divider.default', 'text.secondary']) {
      // 正面对照：这个字样确实在原文里 —— 少了它，下面那条 `not.toContain` 是恒真，不是证据。
      expect(raw, `\`${needle}\` 从 mobile.css 的注释里消失了 —— 这条对照失效，剥法没被真正验过`).toContain(
        needle,
      );
      expect(
        text,
        `\`${needle}\` 解析完还在 = 注释没剥干净：本文件"没采用 kit 那个值"的断言可能是注释在充数`,
      ).not.toContain(needle);
    }
  });

  it('条件块里的声明照收，且"块在前、顶层在后"这一形态收得成两条', () => {
    // 正面：树上真的解析出了 `@media` / `@container` 体内的声明。
    expect(
      DECLS.filter((d) => d.conds.length > 0).length,
      '`@media` / `@container` 里的声明一条都没标出来 —— 覆写对本门不可见',
    ).toBeGreaterThan(10);
    // 合成夹具：`@container` 块写在同名顶层规则**之前**（本轮复核抓到的那个形态）。
    // 这条证明 `chain()` 不是"只取首条规则"，而是把两条都收下、按顺序排好。
    const fixture = `
      @container (min-width: 30em) { .x { color: red; } }
      .x { color: blue; }
    `;
    const got = contextOf([{ file: 'fixture.css', css: fixture }]).decls.filter(
      (d) => d.sels.includes('.x') && d.prop === 'color',
    );
    expect(got.map((d) => [d.value, d.conds.length > 0])).toEqual([
      ['red', true],
      ['blue', false],
    ]);
  });
});

describe('① 四个争议值：原型胜，kit 的 md 是错的（改回 md 那一版即转红）', () => {
  for (const d of DISPUTED) {
    it(`${d.sel} { ${d.prop} } = ${d.value}`, () => {
      const got = chain(d.sel, d.prop);
      const wideRailToken = d.sel === ':root' && (d.prop === '--nav-icon' || d.prop === '--nav-indicator-w');
      expect(
        got.length,
        `\`${d.sel} { ${d.prop} }\` 解析到 ${got.length} 条，预期恰好 1 条。\n` +
          `0 条 = 规则被删或取材面塌了；≥2 条 = 有人加了覆写（条件块里的也算，且写在顶层规则` +
          `之前一样会被这里数出来），那条覆写必须过一次人眼：\n` +
          got
            .map(
              (x) =>
                `  · ${x.decl.file}:${x.decl.line}${x.decl.conds.length > 0 ? '（条件块内）' : ''}: ` +
                `${x.decl.prop}: ${x.decl.value}  ⇒ ${x.value}`,
            )
            .join('\n'),
      ).toBe(wideRailToken ? 2 : 1);
      if (wideRailToken) {
        expect(got[1].decl.file).toContain('redesign.css');
        expect(JSON.stringify(got[1].decl.conds)).toContain('1024px');
        expect(got[1].value).toBe(d.prop === '--nav-icon' ? '18px' : '2px');
      }
      expect(
        got[0].value,
        `这个分量被改成了 \`${got[0].value}\`（出自 ${got[0].decl.file}:${got[0].decl.line} 的 ` +
          `\`${got[0].decl.prop}: ${got[0].decl.value}\`）。\n` +
          `· kit 的 ${d.md} —— **是错的**，已在 kit 里就地标注；\n` +
          `· 真值源是原型 ${d.proto}，依据 kit README「Source of truth」：` +
          `kit 与原型分歧时原型胜、kit 是错的；\n` +
          `· ${d.why}。\n` +
          `要改它，先改原型并让 README 那条改判 —— 不是照 component-spec 的 md 回改。`,
      ).toBe(d.value);
    });
  }
});

describe('② token 定对了还得真的被用（写死数值 = 门在但没牙）', () => {
  for (const [sel, prop, variable] of CONSUMERS) {
    it(`${sel} { ${prop} } 读的是 var(${variable})`, () => {
      const got = chain(sel, prop);
      expect(
        got.length,
        `\`${sel} { ${prop} }\` 解析到 ${got.length} 条，预期恰好 1 条 —— 见 ① 里同一条理由`,
      ).toBe(1);
      expect(
        got[0].value,
        `\`${sel}\` 的 ${prop} 写成了 \`${got[0].value}\`，没有读 \`var(${variable})\`：` +
          `token 改对了也传不到这里，① 那条会变成一条没人消费的声明`,
      ).toMatch(new RegExp(`var\\(\\s*${variable}\\b`));
    });
  }
});
