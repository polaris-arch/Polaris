/**
 * 安全区**消费侧**的门 —— 与 `mobile-entry.test.ts` ⑦ 配对，补掉那两扇门之间的缝。
 *
 * # 这道门为什么必须单独存在
 *
 * ⑦ 证明的是**原生侧把值送进来了**（`MainActivity.kt` 从 `WindowInsetsCompat` 读四个方向、
 * 两条通道投递）。它一个字都没说**贴边容器有没有真的用这个值**。缝就在中间，而 2026-09-05
 * 真机缺陷①的成因恰好落在缝里：注入是新加的、消费是旧代码，未来任何一次样式重排都能把消费侧
 * 删掉而八道门全绿。
 *
 * 实测过的复发形态（协调者设计的变异 `V-M3`）：只把 `mobile.css` 的
 * `.m-dock { padding-bottom: var(--safe-b) }` 改成 `padding-bottom: 0px`，缺陷①原样复发，
 * 而 `src/mobile/` + `src/styles/` 共 12 文件 516 条**全绿**。本文件就是为这条变异写的。
 *
 * # 判据形态：按**层叠后的胜出值**判，不是按「文件里出现过这个字符串」判
 *
 * grep 字符串会被两件事骗过，两件本仓都亲手踩过：
 *
 *  1. **写了但被后面的规则覆盖掉。** 2026-09-05 缺陷② 就是 `.m-section` 靠**进包顺序**赢下
 *     `.mn` 那条同特异度的取消规则 —— 文件里两条都在，真机上只有一条算数。
 *  2. **写在错误的分量里。** `rules-screen.css` 的 `.mr-top` 用的是 4 值 `padding` 简写
 *     （`var(--sp-3) calc(…--safe-r) 0 calc(…--safe-l)`）。「这条声明里出现过 `--safe-r`」
 *     对右边距而言是**弱证据** —— 把两个变量对调，字符串判据照样绿，真机上左右安全区互换。
 *     故本文件把简写按 1/2/3/4 值展开到 TRBL，逐**分量**判。
 *
 * 层叠顺序不是猜的：从 `MobileMain.tsx` 起按 ESM 求值序（依赖先于自身、同层按源码顺序）做一次
 * 后序遍历，得出 CSS 进包的真实次序 —— 与 Vite 的 CSS 发射同源。这条顺序本身就是缺陷②的成因，
 * 把它写成代码而不是写进注释，是让那类缺陷下次能被机器看见的唯一办法。
 *
 * # 取材面的两个方向都要自检（单向会烂）
 *
 *  · **正向**：登记表里的选择器必须在树上真的存在，且胜出声明确实读到了那个变量。
 *    登记一个不存在的选择器 = 那一格空跑，是最典型的「门在但没牙」。
 *  · **反向（僵尸自检）**：树上每一处 `var(--safe-*)` 消费点都必须在登记表里。
 *    新增一个贴边容器却忘了登记 ⇒ 当场红，而不是等下一次真机验收。
 *
 * ⚠️ **本门测不到的**（登记，别把绿读成"安全区全对"）：一个**新的**贴边容器**从一开始就没写**
 * 安全区 —— 它既不在登记表里、也不在消费点集合里，两个方向都碰不到它。CSS 里没有可机械判定的
 * 「这个元素贴着屏幕边」谓词（`.mn-top` 是 `position:sticky;top:0` 却**不该**吃 `--safe-t`，
 * 因为 `.m-shell` 已经在外层让过了）。新弹层必须先审包含块与安全区归属，再登记消费边。
 */

import { describe, it, expect } from 'vitest';
import {
  ALL,
  context,
  expandDecl,
  explain,
  resolve,
  selectorKey,
  shorthandComponent,
  type Decl,
  type Dir,
  type Edge,
} from '@/styles/css-cascade.test-support';

// ════════════════════════════════════════════════════════════════════════════
// ① 解析器：全仓共用的那一份
// ════════════════════════════════════════════════════════════════════════════

/**
 * 剥噪 / 进包顺序 / 规则解析 / 简写展开原本在本文件里各写一份（且与 `dynamic-type.test.ts`
 * **逐字**重复两份），2026-09-05 起统一到 `styles/css-cascade.test-support.ts`。
 *
 * 换过去多出来的东西正是本文件此前判不了的形状：同族属性（`padding-inline` 认得出
 * `padding-left`）、`!important` 决胜、选择器归一化键、以及「更窄选择器」自曝。
 * `shorthandComponent` 与它那组 1/2/3/4 值 + calc 分量 + 双向错位单测一并搬进
 * `css-cascade.test.ts` ⑧，一行没改。
 *
 * 一处**判据变严**：旧的 `edgeChain('.不存在的选择器', …)` 返回 `[]`（然后其上的否定断言恒真），
 * 新的 `resolve` 对「一条规则都没命中」直接抛 —— 那正是本轮全部缺陷的同一个源头。
 */
const MOBILE = context('mobile');
const CSS_ORDER = MOBILE.files;
const DECLS = MOBILE.decls;

type Family = 'padding' | 'margin';
const EDGES: readonly Edge[] = ['top', 'right', 'bottom', 'left'] as const;

/** 某条声明为 `family` 的 `edge` 这条边贡献的值；不贡献则 null（简写与逻辑属性都已展开）。 */
const edgeValueOf = (d: Decl, family: Family, edge: Edge): string | null =>
  expandDecl(d).get(`${family}-${edge}`) ?? null;

/**
 * 该选择器该条边的层叠结果。
 *
 * `where: ALL` —— 条件块里的覆写**要看**：某个断点下把安全区悄悄丢掉，正是本门要抓的形状之一。
 * 边仍是物理边；现在 iOS rail 有真实 `[dir]` 覆写，显式度量 ltr/rtl，不能让默认环境隐去它。
 */
const edgeResolution = (selector: string, family: Family, edge: Edge, dir: Dir = 'ltr') =>
  resolve({ sel: selector, prop: `${family}-${edge}`, ctx: 'mobile', where: ALL, env: { dir } });

const readsVar = (value: string, name: string) =>
  new RegExp(`var\\(\\s*${name}\\b`).test(value);

const ios = (selector: string, attributes = '') => selectorKey(`:root[data-mobile-os="ios"]${attributes} ${selector}`);
const iosBar = selectorKey(':root[data-mobile-os="ios"]:is([data-ios-vertical-bar-side="left"], [data-ios-vertical-bar-side="right"]) .m-nav');

// ════════════════════════════════════════════════════════════════════════════
// ② 登记表：(贴边容器, 边, 盒属性族, 必须消费的变量)
// ════════════════════════════════════════════════════════════════════════════

/**
 * 全树 `var(--safe-*)` 消费点的全集。反向自检钉着这个「全集」。
 *
 * 三类：
 *  · **外壳**（`mobile.css`）：`.m-shell` 顶 / `.m-page` 左右 / `.m-dock` 底与左右 ——
 *    契约「安全区归 shell 管」的落点，缺陷①就在 `.m-dock` 那一格。
 *  · **满幅吸顶块**（三个屏各一）：负外边距把自己拉到滚动区边沿，再用等量内边距把内容推回原位
 *    ⇒ **margin 与 padding 两族都得读同一个变量**，只改一边会让吸顶块自己错位。
 *  · **底部 sheet**（三个屏各一）：`position:fixed;bottom:0` 或落在 fixed 容器的 flex-end，
 *    盖在 `.m-dock` 之上 ⇒ 各自吃一份 `--safe-b`，不是与外壳重复计。
 */
const REGISTRY: ReadonlyArray<readonly [string, Edge, Family, string]> = [
  // ── 外壳：安全区归 shell 管 ──
  ['.m-shell', 'top', 'padding', '--safe-t'],
  ['.m-page', 'left', 'padding', '--safe-l'],
  ['.m-page', 'right', 'padding', '--safe-r'],
  ['.m-dock', 'bottom', 'padding', '--safe-b'], // ← 缺陷① / 变异 V-M3 的那一格
  ['.m-dock', 'left', 'padding', '--safe-l'],
  ['.m-dock', 'right', 'padding', '--safe-r'],
  ['.m-nav', 'top', 'padding', '--safe-t'], // wide rail still clears the top display inset
  // ── 满幅吸顶块：margin 与 padding 成对 ──
  ['.mn-top', 'left', 'margin', '--safe-l'],
  ['.mn-top', 'right', 'margin', '--safe-r'],
  ['.mn-top', 'left', 'padding', '--safe-l'],
  ['.mn-top', 'right', 'padding', '--safe-r'],
  ['.mr-top', 'left', 'margin', '--safe-l'],
  ['.mr-top', 'right', 'margin', '--safe-r'],
  ['.mr-top', 'left', 'padding', '--safe-l'], // ← 4 值简写，逐分量判
  ['.mr-top', 'right', 'padding', '--safe-r'], // ←
  ['.mc-top', 'left', 'margin', '--safe-l'],
  ['.mc-top', 'right', 'margin', '--safe-r'],
  ['.mc-top', 'left', 'padding', '--safe-l'],
  ['.mc-top', 'right', 'padding', '--safe-r'],
  // ── 底部 sheet ──
  ['.h-sheet', 'bottom', 'padding', '--safe-b'],
  ['.h-sheet', 'left', 'padding', '--safe-l'],
  ['.h-sheet', 'right', 'padding', '--safe-r'],
  ['.mn-sheet', 'bottom', 'padding', '--safe-b'],
  ['.mn-sheet', 'left', 'padding', '--safe-l'],
  ['.mn-sheet', 'right', 'padding', '--safe-r'],
  ['.mc-sheet', 'bottom', 'padding', '--safe-b'],
  ['.mc-sheet', 'left', 'padding', '--safe-l'],
  ['.mc-sheet', 'right', 'padding', '--safe-r'],
  ['.mr-sheet', 'left', 'padding', '--safe-l'],
  ['.mr-sheet', 'right', 'padding', '--safe-r'],
  /* 表单宿主的脚（2026-09-06 批 2）。吃安全区的是**脚**而不是面板本身：面板是三段式，
     中间那段滚、脚是 sticky 贴底 —— 安全区加在面板上会在滚动区末尾多出一段空白，
     而手势条底下那颗提交按钮照旧被压住。 */
  ['.m-form-foot', 'bottom', 'padding', '--safe-b'],
  // Read-only details have no footer; their scrolling body owns the bottom inset.
  ['.m-info-layer .m-info-body', 'bottom', 'padding', '--safe-b'],
  ['.m-form-layer', 'top', 'padding', '--safe-t'],
  ['.m-form-layer', 'left', 'padding', '--safe-l'],
  ['.m-form-layer', 'right', 'padding', '--safe-r'],
  ['.m-select-layer', 'top', 'padding', '--safe-t'],
  ['.m-select-panel', 'bottom', 'padding', '--safe-b'],
  ['.m-select-panel', 'left', 'padding', '--safe-l'],
  ['.m-select-panel', 'right', 'padding', '--safe-r'],
  ['.m-lock-content', 'top', 'padding', '--safe-t'],
  ['.m-lock-content', 'right', 'padding', '--safe-r'],
  ['.m-lock-content', 'bottom', 'padding', '--safe-b'],
  ['.m-lock-content', 'left', 'padding', '--safe-l'],
  // iOS-specific consumers: the shell owns bottom, panels own side insets.
  [ios('.mn-sheet'), 'left', 'padding', '--safe-l'],
  [ios('.mn-sheet'), 'right', 'padding', '--safe-r'],
  [ios('.m-form-panel'), 'left', 'padding', '--safe-l'],
  [ios('.m-form-panel'), 'right', 'padding', '--safe-r'],
  [ios('.m-shell'), 'bottom', 'padding', '--safe-b'],
  [ios('.m-pending-slot'), 'left', 'padding', '--safe-l'],
  [ios('.m-pending-slot'), 'right', 'padding', '--safe-r'],
  [ios('.m-nav'), 'left', 'padding', '--safe-l'],
  [ios('.m-nav'), 'right', 'padding', '--safe-r'],
  [iosBar, 'right', 'padding', '--safe-r'],
  [ios('.m-nav', '[data-ios-vertical-bar-side="left"]'), 'left', 'padding', '--safe-l'],
  [ios('.m-nav', '[data-ios-vertical-bar-side="left"][dir]'), 'left', 'padding', '--safe-l'],
  [ios('.m-nav', '[data-ios-vertical-bar-side="right"][dir]'), 'right', 'padding', '--safe-r'],
];

const key = (s: string, e: Edge, f: Family) => `${s}|${f}-${e}`;

// A narrower iOS rule may transfer an inset to a specific containing block or
// to the rail's physical outer edge. Pin both the exact replacement value and
// the receiving owner. Unlisted overrides (including Android) still fail.
type Transfer = readonly [selector: string, prop: string, value: string, owner: string, ownerProp: string, variable: string];
const TRANSFERS: Transfer[] = [];
const transfer = (sel: string, prop: string, value: string, owner: string, ownerProp: string, variable: string) =>
  TRANSFERS.push([sel, prop, value, owner, ownerProp, variable]);
const railTraits = ['[data-ios-horizontal-size-class="regular"]', '[data-ios-vertical-size-class="compact"]'];
const leftRail = ios('.m-nav', '[data-ios-vertical-bar-side="left"]');
const leftRailDir = ios('.m-nav', '[data-ios-vertical-bar-side="left"][dir]');
const rightRailDir = ios('.m-nav', '[data-ios-vertical-bar-side="right"][dir]');
for (const sel of [...railTraits.map((a) => ios('.m-nav', a)), iosBar, leftRail, leftRailDir, rightRailDir]) {
  transfer(sel, 'padding-top', 'var(--sp-3)', '.m-shell', 'padding-top', '--safe-t');
}
transfer(ios('.m-nav'), 'padding-top', '0', '.m-shell', 'padding-top', '--safe-t');
for (const attr of railTraits) {
  const rail = ios('.m-nav', attr);
  transfer(rail, 'padding-left', '0', rail, 'padding-right', '--rail-edge-safe');
  transfer(rail, 'padding-right', 'var(--rail-edge-safe)', ios('.m-shell'), '--rail-edge-safe', '--safe-r');
  const rtl = ios('.m-nav', attr + '[dir="rtl"]');
  transfer(rtl, 'padding-left', 'var(--rail-edge-safe)', ios('.m-shell', '[dir="rtl"]'), '--rail-edge-safe', '--safe-l');
  transfer(rtl, 'padding-right', '0', rtl, 'padding-left', '--rail-edge-safe');
}
transfer(iosBar, 'padding-left', '0', iosBar, 'padding-right', '--safe-r');
for (const sel of [leftRail, leftRailDir]) {
  transfer(sel, 'padding-right', '0', sel, 'padding-left', '--safe-l');
}
transfer(rightRailDir, 'padding-left', '0', rightRailDir, 'padding-right', '--safe-r');
transfer(ios('.mn-sheet'), 'padding-bottom', 'var(--card-padding)', ios('.m-shell'), 'padding-bottom', '--safe-b');
transfer(ios('.m-form-layer'), 'padding-top', 'var(--ios-form-safe-top)', ios('.m-form-layer'), '--ios-form-safe-top', '--safe-t');
for (const [edge, variable] of [['left', '--safe-l'], ['right', '--safe-r']] as const) {
  transfer(ios('.m-form-layer'), `padding-${edge}`, '0', ios('.m-form-panel'), `padding-${edge}`, variable);
}
transfer(ios('.m-form-foot'), 'padding-bottom', 'var(--sp-2)', ios('.m-form-layer'), '--ios-form-safe-bottom', '--safe-b');
transfer(ios('.m-form-foot'), 'padding-bottom', '0', ios('.m-form-layer'), '--ios-form-safe-bottom', '--safe-b');
transfer(ios('.m-form-panel[data-ios-reserved-short] .m-form-foot'), 'padding-bottom', '0', ios('.m-form-layer'), '--ios-form-safe-bottom', '--safe-b');

const assertNarrowConsumption = (decl: Decl, value: string, family: Family, edge: Edge, variable: string) => {
  const prop = `${family}-${edge}`;
  for (const sel of decl.sels) {
    expect(sel, `较窄覆盖必须明确只匹配 iOS：${decl.file}:${decl.line}`).toContain('[data-mobile-os="ios"]');
    if (readsVar(value, variable)) continue;
    const known = TRANSFERS.find(([s, p, v]) => s === sel && p === prop && v === value);
    expect(known, `未裁定的较窄覆盖：${sel} ${prop}: ${value}`).toBeDefined();
    if (!known) throw new Error('Missing safe-area ownership transfer');
    if (sel === ios('.m-form-foot') && value === '0') {
      expect(decl.conds, 'footer 的零 inset 只属于 ios-form 短高容器，不能扩成常规档').toEqual(['@container ios-form (max-height: 15em)']);
    }
    const [, , , owner, ownerProp, ownerVariable] = known;
    const r = resolve({ sel: owner, prop: ownerProp, ctx: 'mobile', where: ALL,
      env: { dir: owner.includes('[dir="rtl"]') ? 'rtl' : 'ltr' } });
    expect(r.winner && readsVar(r.winner.value, ownerVariable),
      `${sel} 把 ${prop} 转给 ${owner}，但接收方没有消费 ${ownerVariable}:\n${explain(r)}`).toBe(true);
    for (const c of r.narrower) {
      for (const receivingSel of c.decl.sels) {
        const physicalRtlRail = ownerProp === '--rail-edge-safe'
          && receivingSel === ios('.m-shell', '[dir="rtl"]') && c.value === 'var(--safe-l)';
        const provedTransfer = TRANSFERS.some(([s, p, v]) => s === receivingSel && p === ownerProp && v === c.value);
        expect(readsVar(c.value, ownerVariable) || physicalRtlRail || provedTransfer,
          `接收安全区的 owner 又被未裁定覆盖：${receivingSel} ${ownerProp}: ${c.value}`).toBe(true);
      }
    }
  }
};

// ════════════════════════════════════════════════════════════════════════════

describe('⓪ 自检：解析器、顺序、取材面都是活的（任一塌了下面全部恒绿）', () => {
  it('进包顺序解出来了，且外壳 CSS 排在各屏 CSS **之后**（缺陷②的成因，钉住它）', () => {
    expect(CSS_ORDER.length, '模块图里一份 CSS 都没走到 —— 解析器塌了').toBeGreaterThanOrEqual(6);
    expect(CSS_ORDER).toContain('src/mobile/mobile.css');
    expect(CSS_ORDER).toContain('src/mobile/nodes/nodes.css');
    const shell = CSS_ORDER.indexOf('src/mobile/mobile.css');
    for (const screen of [
      'src/mobile/nodes/nodes.css',
      'src/mobile/home/home.css',
      'src/mobile/connections/connections.css',
      'src/mobile/screens/rules/rules-screen.css',
    ]) {
      expect(
        CSS_ORDER.indexOf(screen),
        `${screen} 排到了 mobile.css 之后 —— 进包顺序变了，本文件"外壳先于屏"的胜负判断要重推`,
      ).toBeLessThan(shell);
    }
  });

  it('声明解析有量级，且选择器 / 条件块都归位了', () => {
    expect(DECLS.length, '一条声明都没解析出来').toBeGreaterThan(1000);
    // 正面：一条已知的无条件声明。
    const dock = edgeResolution('.m-dock', 'padding', 'bottom').chain;
    // 钉死 1 条：0 = 解析器塌了；≥2 = 有人给它加了覆写，那条覆写可能正是把安全区顶掉的那一条
    // （V-M4 变异实测：追加一条 `.m-dock{padding-bottom:0}` 时这里由 1 变 2），必须过一次人眼。
    expect(dock.length, `.m-dock 的 padding-bottom 解析到 ${dock.length} 条，预期恰好 1 条`).toBe(1);
    expect(dock[0].decl.conds).toEqual([]);
    // 正面：条件块里的声明确实带上了 at-rule prelude（`@container` 真的解析进来了）。
    const conditional = DECLS.filter((d) => d.conds.length > 0);
    expect(conditional.length, '@container / @media 里的声明一条都没标出来').toBeGreaterThan(10);
    // 反面：不存在的选择器**抛**，不是返回空。
    // 旧读法在这里返回 `[]` ⇒ 其上的否定断言恒真；那正是 2026-09-05 七条 major 的同一个源头。
    expect(() => edgeResolution('.m-dock-that-never-existed', 'padding', 'bottom')).toThrow(
      /一条规则都没命中/,
    );
  });

  it('同族属性认得出来（简写与逻辑属性都折到同一条边）', () => {
    // 旧的 `edgeValueOf` 只认 `padding` / `padding-<edge>` 两形态：补一句 `padding-inline-start`
    // 就能把左边距改掉而本门全绿。搬到共享解析器之后这一族整个进模型。
    // `shorthandComponent` 自己那组 1/2/3/4 值 + calc + 双向错位单测搬去了 `css-cascade.test.ts` ⑧。
    const probe = (prop: string, value: string) =>
      expandDecl({
        file: '<probe>',
        line: 0,
        order: 0,
        sels: ['.x'],
        rawSels: ['.x'],
        prop,
        value,
        important: false,
        conds: [],
        registered: false,
      });
    expect(probe('padding', '1px 2px 3px 4px').get('padding-left')).toBe('4px');
    expect(probe('padding-inline-start', '9px').get('padding-left')).toBe('9px');
    expect(probe('margin-inline', '5px 6px').get('margin-right')).toBe('6px');
    expect(shorthandComponent('1px 2px 3px 4px', 'left')).toBe('4px');
  });
});

describe('① 正向：登记的每个贴边容器，胜出声明里确实读到了那个变量', () => {
  it('登记表里的选择器在树上都真的存在（登记一个不存在的 = 那一格空跑）', () => {
    const known = new Set(DECLS.flatMap((d) => d.sels));
    const ghosts = [...new Set(REGISTRY.map(([s]) => s))].filter((s) => !known.has(s));
    expect(ghosts, `登记表里这些选择器在移动端 CSS 链上不存在：${ghosts.join(', ')}`).toEqual([]);
  });

  it.each(REGISTRY.flatMap((e) => (['ltr', 'rtl'] as const).map((dir) => [`${e[0]} ${e[2]}-${e[1]} ← ${e[3]} (${dir})`, e, dir] as const)))(
    '%s',
    (_name, [selector, edge, family, variable], dir) => {
      const r = edgeResolution(selector, family, edge, dir);
      // 正面断言之一：这条边**有**声明。全没有 ⇒ 被整条删掉了。
      expect(
        r.chain.length,
        `${selector} 的 ${family}-${edge} 一条声明都没有 —— 该贴边容器不再消费 ${variable}，` +
          `真机上这条边会压在系统栏/手势条底下（2026-09-05 缺陷①的原样复发）\n${explain(r)}`,
      ).toBeGreaterThan(0);
      // 正面断言之二：**胜出的那一条**读到了变量（不是"文件里出现过"）。
      const win = r.winner;
      expect(
        win !== null && readsVar(win.value, variable),
        `${selector} 的 ${family}-${edge} 胜出声明里读不到 ${variable}：\n${explain(r)}`,
      ).toBe(true);
      // 条件块里的覆写也不许把变量丢掉（某个断点下悄悄失效）。
      for (const c of r.chain.filter((x) => x.decl.conds.length > 0)) {
        expect(
          readsVar(c.value, variable),
          `${selector} 在条件块（${c.decl.file}:${c.decl.line}）里把 ${family}-${edge} 覆写成 ` +
            `\`${c.value}\`，丢掉了 ${variable} —— 某些断点/媒体条件下安全区会静默失效\n${explain(r)}`,
        ).toBe(true);
      }
      // Android/unlisted overrides still fail. iOS must keep this physical
      // variable or match an exact, proved ownership transfer above.
      for (const c of r.narrower) assertNarrowConsumption(c.decl, c.value, family, edge, variable);
    },
  );

  it('较窄覆盖不能扩成 Android、未知 iOS 分支或把短高 footer 的零 inset 扩成常规档', () => {
    const original = DECLS.find((d) => d.sels.includes(ios('.m-form-foot')) && d.conds.length > 0 && d.prop === 'padding-bottom');
    expect(original).toBeDefined();
    if (!original) throw new Error('Missing real short-height footer override');
    for (const sels of [['.m-form-foot'], [ios('.m-form-foot[data-unreviewed]')]]) {
      expect(() => assertNarrowConsumption({ ...original, sels }, '0', 'padding', 'bottom', '--safe-b')).toThrow();
    }
    expect(() => assertNarrowConsumption({ ...original, conds: [] }, '0', 'padding', 'bottom', '--safe-b')).toThrow();
  });
});

describe('② 反向僵尸自检：树上每一处安全区消费点都必须在登记表里', () => {
  /** 全树消费点 =（选择器, 族, 边, 变量名），从**分量**读出来，不是从整条声明。 */
  const CONSUMERS = (() => {
    const out = new Map<string, { sel: string; family: Family; edge: Edge; variable: string; file: string }>();
    for (const d of DECLS) {
      if (!d.value.includes('var(--safe-')) continue;
      for (const family of ['padding', 'margin'] as const) {
        for (const edge of EDGES) {
          const v = edgeValueOf(d, family, edge);
          if (v === null) continue;
          const m = /var\(\s*(--safe-[trbl])\b/.exec(v);
          if (m === null) continue;
          for (const sel of d.sels) {
            out.set(`${key(sel, edge, family)}|${m[1]}`, {
              sel,
              family,
              edge,
              variable: m[1],
              file: d.file,
            });
          }
        }
      }
    }
    return [...out.values()];
  })();

  it('自检：消费点总数与登记表逐数相等（扫到 0 个会让下面那条恒绿）', () => {
    expect(
      CONSUMERS.length,
      CONSUMERS.length === 0
        ? '全树一处 var(--safe-*) 消费点都没扫到 —— 判据面塌了'
        : `全树消费点 ${CONSUMERS.length} 处，登记表 ${REGISTRY.length} 条，对不上：` +
          '少了 = 有贴边容器不再消费安全区（缺陷①复发）；多了 = 新增了没登记的消费点',
    ).toBe(REGISTRY.length);
    // 正面：底部导航那一格确实在集合里（它是缺陷①的那一格）。
    expect(
      CONSUMERS.some((c) => c.sel === '.m-dock' && c.edge === 'bottom' && c.variable === '--safe-b'),
    ).toBe(true);
  });

  it('每个消费点都登记在案（新增贴边容器忘了登记 ⇒ 表在腐烂 ⇒ 当场红）', () => {
    const registered = new Set(REGISTRY.map(([s, e, f, v]) => `${key(s, e, f)}|${v}`));
    const orphans = CONSUMERS.filter((c) => !registered.has(`${key(c.sel, c.edge, c.family)}|${c.variable}`)).map(
      (c) => `${c.sel} ${c.family}-${c.edge} ← ${c.variable} (${c.file})`,
    );
    expect(
      orphans,
      `这些安全区消费点不在登记表里：\n${orphans.join('\n')}\n` +
        '要么补进 REGISTRY（新的贴边容器），要么它读错了变量（左右对调之类）',
    ).toEqual([]);
  });
});
