/**
 * Dynamic Type 的**消费端**门 —— 与 `mobile-entry.test.ts` ⑧（原生生产端）配对。
 *
 * # 这道门守的是什么
 *
 * 系统字号缩放这条链有三段，三段各有各的失效形态：
 *
 *  1. **生产端**（`MainActivity.kt` 读 `Configuration.fontScale` 写内联 `--font-scale`）——
 *     归 `mobile-entry.test.ts` ⑧。2026-09-05 之前这一段**整段不存在**：`mobile.css` 有接缝、
 *     有缺省 1，Android 侧一行代码都没有，用户在系统里调到 2.0x 本应用一动不动，而当时
 *     8/8 静态门全绿 —— 因为没有任何门问过"有没有人写这个变量"。
 *  2. **消费端**（`html{font-size:calc(16px * var(--font-scale))}` + 断点容器不写死字号）——
 *     本文件 ①。生产端写得再对，只要这条 `calc()` 被谁改成常数，或有人给 `.m-scroll`
 *     补一条 `font-size`，整条链**当场断且没有任何运行期报错**（CSS 不会为此抛异常）。
 *  3. **竖排回退**（三处横排行在字放大后改竖排）—— 本文件 ②③④。spec
 *     `mobile-screen-shell.md`「Horizontal clipping fallbacks」要求的三处，2026-09-05 之前
 *     **一条都没实现**。
 *
 * # 判据形态：按**层叠后的胜出值**判，不是按「文件里出现过这个字符串」判
 *
 * 与 `safe-area-consumption.test.ts` 同一口径，同一套解析器（进包次序按 ESM 求值序做后序遍历、
 * 声明带选择器/顺位/条件块）。理由那边已写透，这里只补一条本文件特有的：
 * **竖排回退的判据必须能分辨"哪个条件块"**。`.h-diagrow{flex-direction:column}` 写在
 * `@container mscreen (min-width: 52.5em)` 里是**反向的**错误（宽屏才竖排），而"文件里出现过
 * `flex-direction: column`"对这两种写法给出同一个答案。故本文件把条件块的 prelude 原文一起带出来，
 * 逐条比对阈值。
 *
 * ⚠️ 解析器是从 `safe-area-consumption.test.ts` **抄**过来的，不是 import：从一个 `.test.ts`
 *    import 会把它的 `describe` 一并收进本文件的套件（同一批用例跑两遍、报错报两处）。
 *    抽成共享模块是对的做法，但那要改 `safe-area-consumption.test.ts`，本轮它归另一条线。
 *    两份漂移的风险由 ⓪ 的自检兜：任一份的解析器塌了，那边的自检也会同时红。
 *
 * # 射程之外（别把绿读成「Dynamic Type 全对」）
 *
 *  · **iOS 侧没有生产端**（`preferredContentSizeCategory` 映射尚未落地）⇒ 那一端恒 1x，
 *    本文件与 ⑧ 都碰不到它。
 *  · **阈值以下的排版好不好看**：本门只判"回退有没有接上、阈值在不在合理带里"，
 *    竖排后的间距/对齐是真机验收的事（`~/docs/polaris/design/visual-pass-2026-09-05/dt-*.png`）。
 *  · **除这三处以外的横排行**：spec 只点了这三处，别的行（`.h-cardhead`、`.h-rates`、
 *    `.h-hostline` …）今天既不在登记表、也不在僵尸自检的取材面内 —— 不是判过不进门，是还没看。
 */

import { describe, it, expect } from 'vitest';
import {
  UNCONDITIONAL,
  context,
  contextOf,
  expandDecl,
  resolve,
  selectorKey,
  winnerValue,
  type Decl,
} from '@/styles/css-cascade.test-support';

// ════════════════════════════════════════════════════════════════════════════
// ① 解析器：全仓共用的那一份
// ════════════════════════════════════════════════════════════════════════════

/**
 * 本文件原先的解析器是从 `safe-area-consumption.test.ts` **抄**过来的（旧头注自陈：
 * 「抽成共享模块是对的做法，但那要改 safe-area，本轮它归另一条线」）。2026-09-05 那条债在这里结清：
 * 两份逐字副本一起删掉，统一到 `styles/css-cascade.test-support.ts`。
 *
 * 本文件特有的那点信息一条没丢：`conds` 仍是**由内到外的 at-rule prelude 原文**（判「竖排写在哪个
 * 条件块里」全靠它 —— `.h-diagrow{flex-direction:column}` 写在 `min-width` 块里是反向的错误，
 * 而一个 boolean 对两种写法给出同一个答案）。`where` 谓词形态照旧本地写。
 */
const MOBILE = context('mobile');
const CSS_ORDER = MOBILE.files;
const DECLS = MOBILE.decls;

/**
 * 某个选择器某个属性的**全部**贡献声明，按进包顺序（最后一条即同特异度下的胜出者）。
 *
 * 换到共享解析器后多出来的两件事：① 同族属性折到同一个语义分量（问 `flex-direction` 不受影响，
 * 但问 `min-height` 会一并收到 `min-block-size`）；② 「一条规则都没命中」直接抛，不再返回空数组。
 */
function chain(selector: string, prop: string, where: (d: Decl) => boolean): Decl[] {
  return resolve({ sel: selector, prop, ctx: 'mobile', where }).chain.map((c) => c.decl);
}

const winner = (selector: string, prop: string, where: (d: Decl) => boolean): Decl | null =>
  resolve({ sel: selector, prop, ctx: 'mobile', where }).winner?.decl ?? null;

const readsVar = (value: string, name: string) => new RegExp(`var\\(\\s*${name}\\b`).test(value);

/**
 * 登记表里的选择器要与 `Decl.sels` 同口径比对 —— 后者是**归一化键**。
 *
 * 不归一就会踩上打穿 `cascade-dead-rules` 的第五个盲区：`.h-act + .h-act::before`（登记表里带空格）
 * 与 CSS 里写的 `.h-act+.h-act::before` 字面不等，登记表当场失配、把真规则报成"未登记"。
 */
const K = (sel: string) => selectorKey(sel);

// ════════════════════════════════════════════════════════════════════════════
// ② 阈值：从 CSS 现场解析，不写死结论
// ════════════════════════════════════════════════════════════════════════════

/** `@container <name> (max-width: <n><unit>)` —— 回退块的条件。 */
const FALLBACK_COND = /@container\s+(\w+)\s*\(\s*max-width:\s*([\d.]+)(\w+)\s*\)/;
// The unlock tile grid has its own content-width query; it is not the shell's
// Dynamic Type fallback and must not be mistaken for a second shell system.
const isFallbackCond = (c: string) => FALLBACK_COND.test(c) && FALLBACK_COND.exec(c)?.[1] === 'mscreen';
const isInFallback = (d: Decl) => d.conds.some(isFallbackCond);

/** 全树出现过的 `max-width` 型容器条件（去重）。一条以上 = 出现了第二套阈值。 */
const FALLBACK_CONDS = [...new Set(DECLS.flatMap((d) => d.conds).filter(isFallbackCond))];

/** 解析出的阈值。取材面为空时这里是 null，⓪ 的自检会先红，下面每条不会恒绿。 */
const THRESHOLD = (() => {
  if (FALLBACK_CONDS.length !== 1) return null;
  const m = FALLBACK_COND.exec(FALLBACK_CONDS[0]);
  return m === null ? null : { container: m[1], value: parseFloat(m[2]), unit: m[3] };
})();

/** 根字号（`mobile.css` 的 `html{font-size:calc(16px * var(--font-scale))}` 里那个 16）。 */
const ROOT_PX = 16;
/**
 * 三个参考宽度，各自有出处，不是随手取的数：
 *  · `COMPACT_REF_PX` = `mobile-kit/tokens/core.json#referenceViewport.width`，
 *    也是 `../styles/text-fit.test.ts` 的 `MOBILE_REF_VIEWPORT_W`。
 *  · `DEVICE_PX` = 2026-09-05 真机/模拟器验收那台的 CSS 视口宽（1080 @2.625）。
 *  · `NARROWEST_1X_PX` = 在售 Android 竖屏里最窄的一档 CSS 宽度。阈值必须**严格小于**它的 1x
 *    em 值，否则 1x 下就有真机踩进回退 —— 那是窄屏回退，不是 Dynamic Type 回退。
 */
const COMPACT_REF_PX = 390;
const DEVICE_PX = 412;
const NARROWEST_1X_PX = 320;

/** 某个 CSS 宽度在某档字号下的容器 em 宽（容器查询里的 em 按容器自己的字号解析）。 */
const emWidth = (px: number, scale: number) => px / (ROOT_PX * scale);

// ════════════════════════════════════════════════════════════════════════════
// ⑥ 登记表
// ════════════════════════════════════════════════════════════════════════════

/**
 * spec 点名的三处横排行（`mobile-screen-shell.md`「Horizontal clipping fallbacks」）。
 * 每条的判据是三段：无条件下**不是**竖排（证明它确实是一行）、回退块里胜出值**是**竖排、
 * 而且那个回退块用的是登记在案的那一条阈值。
 */
const STACKED_ROWS: ReadonlyArray<readonly [string, string]> = [
  ['.h-acts', '节点状态卡的三按钮动作行'],
  ['.h-diagrow', '诊断行（标签 / 取值）'],
];

/**
 * 回退块里允许出现的**配套**声明 —— 不是 spec 点的行，但是让那三行竖排后仍然成立的必要条件。
 * 登记它们的意义在反向自检：回退块里冒出一条没登记的选择器时能指名道姓。
 */
const STACKED_SUPPORT: ReadonlyArray<readonly [string, string, string]> = [
  ['.h-act', 'flex', '竖排后按钮不再瓜分一行宽，`flex:1 1 0` 会在纵轴上把三颗高度绑成一份'],
  ['.h-act + .h-act::before', 'height', '分隔发丝线从内缩竖线翻成整幅横线'],
  ['.h-diagrow', 'align-items', '竖排后沿行首对齐（RTL 下 flex-start 自动翻边）'],
  ['.h-diagrow', 'justify-content', '同上'],
];

// ════════════════════════════════════════════════════════════════════════════

describe('⓪ 自检：解析器、进包次序、取材面都是活的（任一塌了下面全部恒绿）', () => {
  it('进包顺序解出来了，且移动端外壳与首页 CSS 都在链上', () => {
    expect(CSS_ORDER.length, '模块图里一份 CSS 都没走到 —— 解析器塌了').toBeGreaterThanOrEqual(6);
    expect(CSS_ORDER).toContain('src/mobile/mobile.css');
    expect(CSS_ORDER).toContain('src/mobile/home/home.css');
  });

  it('声明解析有量级，条件块 prelude 真的带出来了（双向对照）', () => {
    expect(DECLS.length, '一条声明都没解析出来').toBeGreaterThan(1000);
    // 正面：一条已知的**有条件**声明，且 prelude 原文里读得到容器名。
    const wide = chain('.h-flow', 'column-count', (d) => d.conds.length > 0);
    expect(wide.length, '`.h-flow` 的 `column-count` 断点声明没解析到 —— 条件面塌了').toBeGreaterThan(0);
    expect(wide[0].conds.join(' ')).toContain('@container mscreen');
    // 正面：一条已知的**无条件**声明。
    expect(winner('.h-acts', 'display', UNCONDITIONAL)?.value).toBe('flex');
    // 反面：不存在的选择器**抛**，不是返回空（空取材面上的否定断言恒真）。
    expect(() => chain('.h-acts-that-never-existed', 'display', () => true)).toThrow(
      /一条规则都没命中/,
    );
    // 反面：条件面筛错了会取到空 —— `UNCONDITIONAL` 真的把断点块挡在外面。
    expect(chain('.h-flow', 'column-count', UNCONDITIONAL)).toEqual([]);
  });

  it('剥注释真的把注释里的反面示例挡在外面（不剥的话删掉整个回退块本门照样绿）', () => {
    const synthetic =
      '/* @container mscreen (max-width: 18em) { .x { flex-direction: column } } */\n.y{color:red}';
    const parsed = contextOf([{ file: '<synthetic>', css: synthetic }]).decls;
    expect(parsed.map((d) => d.prop)).toEqual(['color']);
    // 正面对照：真实的 home.css 里，剥完之后回退块**仍在**（否则下面每条都是在读注释）。
    expect(
      DECLS.filter((d) => d.file === 'src/mobile/home/home.css' && d.conds.some((c) => c.startsWith('@container')))
        .length,
      'home.css 剥完注释后一条 `@container` 内声明都没有 —— 剥法把代码一起吃了',
    ).toBeGreaterThan(0);
  });

  it('`flex` 简写的收缩因子解析双向可辨（走共享解析器的 `flex-shrink` 分量）', () => {
    // 本文件此前自带一份 `flex` 简写读法。共享解析器把 `flex` 折成 grow/shrink/basis 之后
    // 那一份就是第二个读法了（同一件事两处实现 = 下一次只会改一处），故删掉、改吃分量。
    const shrinkOf = (value: string): string | undefined =>
      contextOf([{ file: 'm.css', css: `.x{ flex: ${value} }` }])
        .decls.flatMap((d) => [...expandDecl(d)])
        .find(([k]) => k === 'flex-shrink')?.[1];
    expect(shrinkOf('none')).toBe('0');
    expect(shrinkOf('0 0 auto')).toBe('0');
    expect(shrinkOf('0 1 auto')).toBe('1');
    expect(shrinkOf('1 1 auto')).toBe('1');
    expect(shrinkOf('1 1 0')).toBe('1');
    expect(shrinkOf('auto')).toBe('1');
    // `flex: <grow> <basis>` 两值形态：第二位不是数 ⇒ shrink 取初始值 1，别读成 0。
    expect(shrinkOf('1 200px')).toBe('1');
    // 反面：长写覆写看得见（这正是简写读法看不见的那一格）。
    const both = contextOf([{ file: 'm.css', css: '.x{ flex: none }\n.x{ flex-shrink: 1 }' }]);
    expect(winnerValue({ sel: '.x', prop: 'flex-shrink', decls: both })).toBe('1');
  });
});

describe('① 消费端接缝：`--font-scale` 真的被消费，且断点容器没有把 em 机制关掉', () => {
  it('`:root` 上有且只有一处 `--font-scale` 缺省，值为 1（生产端缺席时按 1x 正常工作）', () => {
    const all = DECLS.filter((d) => d.prop === '--font-scale');
    expect(
      all.map((d) => `${d.file} ${d.sels.join(',')}`),
      '`--font-scale` 的声明点不止一处 —— 原生写的是根元素**内联**样式，' +
        '多一处 `:root` 级声明不会被它盖掉的那一份就成了第二个真值源',
    ).toEqual(['src/mobile/mobile.css :root']);
    expect(all[0].value).toBe('1');
    expect(all[0].conds, '缺省写进了条件块 —— 某些条件下会整条缺失').toEqual([]);
  });

  it('`html` 的 `font-size` 胜出声明里确实读到了 `--font-scale`（正面断言，不是"不许是常数"）', () => {
    const win = winner('html', 'font-size', () => true);
    expect(win, '`html` 上一条 `font-size` 都没有 —— 根字号不再跟随系统字号').not.toBeNull();
    expect(
      readsVar(win?.value ?? '', '--font-scale'),
      `\`html\` 的 font-size 胜出声明是 \`${win?.file}\` 里的 \`${win?.value}\`，读不到 --font-scale ` +
        '⇒ 原生把值写进来了也没人用，整条 Dynamic Type 链断在消费端且不会报任何错',
    ).toBe(true);
    expect(win?.value).toContain(`${ROOT_PX}px`);
    expect(win?.conds, '根字号写进了条件块 —— 某些条件下字号缩放会静默失效').toEqual([]);
  });

  it('断点容器 `.m-screen-viewport` 没有任何 `font-size`（写死一条就静默关掉整条"大字自动降列"）', () => {
    // 正面：容器本身在，且真的是 inline-size 容器（登记一个不存在的容器 = 这一格空跑）。
    const container = winner('.m-screen-viewport', 'container', () => true);
    expect(container, '`.m-screen-viewport` 上找不到 `container` 声明 —— 断点容器没了').not.toBeNull();
    expect(container?.value).toContain('inline-size');
    expect(container?.value).toContain(THRESHOLD?.container ?? 'mscreen');
    // 反面：它身上不许有字号。em 阈值按容器自己的字号解析，写死 ⇒ 阈值不再随 Dynamic Type 收窄。
    expect(
      chain('.m-screen-viewport', 'font-size', () => true).map((d) => `${d.file}: ${d.value}`),
      '`.m-screen-viewport` 被写上了 `font-size` —— 容器查询的 em 从此不跟随根字号，' +
        '字放大时容器宽度（em）不再变窄，竖排回退与降列**双双静默失效**',
    ).toEqual([]);
  });

  it('屏内弹层的包含块不滚动，滚动正文不再建立嵌套断点包含块', () => {
    expect(winner('.m-screen-viewport', 'contain', UNCONDITIONAL)?.value).toBe('layout');
    expect(chain('.m-scroll', 'container', () => true)).toEqual([]);
    expect(chain('.m-scroll', 'contain', () => true)).toEqual([]);
  });
});

describe('② 竖排回退：spec 点名的三处横排行，在阈值以下的胜出值是竖排', () => {
  it('自检：登记的三个选择器在移动端 CSS 链上都真的存在（登记一个不存在的 = 那一格空跑）', () => {
    const known = new Set(DECLS.flatMap((d) => d.sels));
    const ghosts = STACKED_ROWS.map(([s]) => s).filter((s) => !known.has(K(s)));
    expect(ghosts, `登记表里这些选择器不存在：${ghosts.join(', ')}`).toEqual([]);
  });

  it.each(STACKED_ROWS.map((e) => [`${e[0]}（${e[1]}）`, e] as const))('%s', (_name, [sel, what]) => {
    // 正面之一：无条件下它是**横排**（默认值 row 或显式 row）——证明这确实是一处"横排行"，
    // 而不是本来就竖着、回退无从谈起。
    const base = winner(sel, 'flex-direction', UNCONDITIONAL);
    expect(
      base === null || base.value === 'row',
      `${sel}（${what}）在无条件下就是 \`${base?.value}\` —— 它不是横排行，登记表要重写`,
    ).toBe(true);
    expect(winner(sel, 'display', UNCONDITIONAL)?.value, `${sel} 不是 flex 行`).toBe('flex');

    // 正面之二：回退块里**有**这条声明，且胜出值是 column。
    const inFallback = chain(sel, 'flex-direction', isInFallback);
    expect(
      inFallback.length,
      `${sel}（${what}）在阈值回退块里一条 \`flex-direction\` 都没有 —— ` +
        'spec「Horizontal clipping fallbacks」要求的竖排回退没有实现，' +
        '字放大时这一行会横向溢出/截断（2026-09-05 登记的三条缺口之一）',
    ).toBeGreaterThan(0);
    const win = inFallback[inFallback.length - 1];
    expect(
      win.value,
      `${sel} 在 \`${win.conds.join(' / ')}\` 里的胜出值是 \`${win.value}\`，不是 column`,
    ).toBe('column');

    // 正面之三：它挂的那个条件就是登记在案的那一条阈值（写进 min-width 断点是**反向**的错误）。
    for (const c of win.conds) {
      expect(
        isFallbackCond(c),
        `${sel} 的竖排挂在 \`${c}\` 上 —— 那不是 Dynamic Type 阈值。` +
          '挂到 `min-width` 断点上等于"宽屏才竖排"，方向正好反了',
      ).toBe(true);
    }
  });
});

describe('③ 阈值：单位是 em、只有一条、且落在「Dynamic Type 才触发」的带里', () => {
  it('自检：回退条件恰好解析出一条（0 条 ⇒ 下面全部恒绿；≥2 条 ⇒ 出现了第二套阈值）', () => {
    expect(
      FALLBACK_CONDS,
      `全树 \`max-width\` 型容器条件 ${FALLBACK_CONDS.length} 条：\n${FALLBACK_CONDS.join('\n')}\n` +
        '0 条 = 回退整个不在；≥2 条 = spec 明写的 Do not introduce a second responsive system 被违反',
    ).toHaveLength(1);
    expect(THRESHOLD, '阈值解析不出来 —— 条件写法变了').not.toBeNull();
  });

  it('单位必须是 em（写成 px 就成了"窄屏回退"，与 Dynamic Type 完全无关）', () => {
    expect(
      THRESHOLD?.unit,
      `阈值单位是 \`${THRESHOLD?.unit}\`。容器查询里只有 em 会按容器自己的字号解析 ⇒ ` +
        '只有 em 能表达"字变大 ⇒ 容器相对变窄"。px 阈值在任何字号下都是同一个数，字放大时永不触发',
    ).toBe('em');
    expect(THRESHOLD?.container).toBe('mscreen');
  });

  it('数值落在带里：1x 下任何真机都不触发，且 compact 参考视口 1.5x 时必须已经触发', () => {
    const t = THRESHOLD?.value ?? NaN;
    // 上界：最窄的真机 1x 都不许踩进来（阈值的用意是 Dynamic Type，不是窄屏）。
    expect(
      t,
      `阈值 ${t}em ≥ ${emWidth(NARROWEST_1X_PX, 1)}em（${NARROWEST_1X_PX}px 真机在 1x 下的 em 宽）` +
        ' —— 1x 下就有真机踩进竖排回退，那是窄屏回退，不是 Dynamic Type 回退',
    ).toBeLessThan(emWidth(NARROWEST_1X_PX, 1));
    // 下界：compact 参考视口 390px 在 1.5x 下三等分槽已装不下最长动作文案 ⇒ 那时必须已经竖排。
    expect(
      t,
      `阈值 ${t}em ≤ ${emWidth(COMPACT_REF_PX, 1.5)}em —— ${COMPACT_REF_PX}px 在 1.5x 下 ` +
        '`.h-act` 的三等分槽只剩 108px，已装不下最长的动作文案，回退却还没触发',
    ).toBeGreaterThan(emWidth(COMPACT_REF_PX, 1.5));
  });

  it('对真机那台（412 CSS px）逐档结算：1x 不触发 / 2.0x 触发（阈值改成永不触发时这条红）', () => {
    const t = THRESHOLD?.value ?? NaN;
    const fires = (scale: number) => emWidth(DEVICE_PX, scale) <= t;
    expect(fires(1), `1x 下 ${DEVICE_PX}px = ${emWidth(DEVICE_PX, 1)}em，不该触发回退`).toBe(false);
    expect(
      fires(2),
      `2.0x 下 ${DEVICE_PX}px = ${emWidth(DEVICE_PX, 2)}em > 阈值 ${t}em —— ` +
        '把字放到 2 倍都不竖排，这条回退等于不存在（"永不触发的阈值"就是这个形态）',
    ).toBe(true);
    // 正面：真机验收那三档里，触发档与不触发档各至少一个（否则截图证明不了任何东西）。
    const matrix = [1, 1.3, 2].map((s) => [s, fires(s)] as const);
    expect(matrix.filter(([, f]) => f).length).toBeGreaterThan(0);
    expect(matrix.filter(([, f]) => !f).length).toBeGreaterThan(0);
  });
});

describe('④ 反向僵尸自检：回退块里的每一条声明都登记在案', () => {
  /** 回退块里的全部声明（选择器 × 属性）。 */
  const IN_BLOCK = DECLS.filter(isInFallback).flatMap((d) => d.sels.map((sel) => ({ sel, prop: d.prop, file: d.file })));

  it('自检：回退块里扫到的声明有量级（扫到 0 条会让下面那条恒绿）', () => {
    expect(IN_BLOCK.length, '回退块里一条声明都没扫到 —— 判据面塌了').toBeGreaterThanOrEqual(
      STACKED_ROWS.length + STACKED_SUPPORT.length,
    );
    // 正面：三处 spec 行确实都在这个集合里。
    for (const [sel] of STACKED_ROWS)
      expect(IN_BLOCK.some((x) => x.sel === K(sel) && x.prop === 'flex-direction')).toBe(true);
  });

  it('没有未登记的选择器 / 属性混进回退块（新增一条就要在这里记一笔）', () => {
    const registered = new Set([
      ...STACKED_ROWS.map(([s]) => `${K(s)}|flex-direction`),
      ...STACKED_SUPPORT.map(([s, p]) => `${K(s)}|${p}`),
    ]);
    // `.h-act + .h-act::before` 那条要翻整幅横线，一条规则里改了四五个盒模型属性；
    // 逐个登记只会把登记表变成规则的抄本，故按**选择器**放行、按属性登记其余。
    const hairline = STACKED_SUPPORT.find(([, p]) => p === 'height')?.[0];
    const orphans = [...new Set(IN_BLOCK.map((x) => `${x.sel}|${x.prop}`))].filter(
      (k) => !registered.has(k) && !k.startsWith(`${hairline === undefined ? '\u0000' : K(hairline)}|`),
    );
    expect(
      orphans,
      `回退块里出现了未登记的声明：\n${orphans.join('\n')}\n` +
        '要么补进 STACKED_ROWS / STACKED_SUPPORT，要么它本来就不该在这个块里',
    ).toEqual([]);
  });

  it('回退块里一个 `display` / `visibility` 都没有（竖向长高是免费的，回退不许藏内容）', () => {
    const hidden = IN_BLOCK.filter((x) => /^(?:display|visibility|content-visibility)$/.test(x.prop));
    expect(
      hidden.map((x) => `${x.file} ${x.sel} ${x.prop}`),
      'Dynamic Type 回退里出现了可见性属性 —— 那是"字大了就藏起来"，' +
        'spec 的裁定是 Vertical growth is free，且 `home-screen.test.tsx` 的同款判据也会红',
    ).toEqual([]);
  });
});

/**
 * ⑥ 底部导航在大字下不横向溢出（`bottom-navigation.md` 的两条明文）。
 *
 * spec 有两句直接相关的硬话：
 *   · 「Equal horizontal distribution; **labels never scroll horizontally**.」
 *   · 「`type.navigation`; **scaling may wrap to two lines only as a last resort**.」
 *
 * 2026-09-05 模拟器实测（412 CSS px / en-US / system font_scale = 2.0）：两条都被破了 ——
 * `Settings` 单格要 85px 而格子只有 82.4px，`Rules` / `Connections` / `Settings` 三格互相压字，
 * 文档 `scrollWidth` 413 > 视口 412（整页可横向滚动）。这**不是**本批引入的：Android WebView
 * 缺省就按系统字号缩排版，2.0x 下导航标签一直是 23px。但它是一条 Dynamic Type 缺陷，
 * 且两条 spec 明文都点到它，故与三处竖排回退同批收口。
 *
 * 判据钉两个点，缺哪个都救不回来：
 *  1. **`overflow-wrap: anywhere`**，不是 `break-word`。两者都能断超宽的词，但只有 `anywhere`
 *     把 **min-content 尺寸**压下去；标签是 `.m-nav-item`（`flex:1 1 0`）的 flex 子项、
 *     交叉轴按 fit-content 取宽，而 fit-content 的下限就是 min-content ⇒ `break-word`
 *     下格子仍按 max-content 互相挤，一个字都没救到。这条差别没有门就会在下一次"顺手改成
 *     更常见的 break-word"时静默回归。
 *  2. **导航高度是下限不是定值**。断词后标签会占两行，写死 `height: 64px` 会把第二行推到
 *     手势条上（`--safe-b` 加在 `.m-dock` 上，导航本体之外）。
 */
describe('⑥ 底部导航：大字下断得动词、且导航让得出那一行', () => {
  it('自检：导航的两个选择器都在树上，且几何仍是 px、排版仍是 rem（"框不动、字跟着走"）', () => {
    const known = new Set(DECLS.flatMap((d) => d.sels));
    for (const sel of ['.m-nav', '.m-nav-item', '.m-nav-label'])
      expect(known.has(K(sel)), `${sel} 不在移动端 CSS 链上 —— 判据面塌了`).toBe(true);
    // 正面：标签字号是 rem（跟随 `--font-scale`），导航高度是 px（不跟随）。
    expect(winner('.m-nav-label', 'font-size', () => true)?.value).toMatch(/rem$/);
    expect(winner(':root', '--nav-h', () => true)?.value).toMatch(/px$/);
  });

  it('`.m-nav-label` 的胜出 `overflow-wrap` 是 `anywhere`（`break-word` 压不下 min-content）', () => {
    const win = winner('.m-nav-label', 'overflow-wrap', () => true);
    expect(
      win,
      '`.m-nav-label` 没有 `overflow-wrap` —— 2.0x 下 `Connections` / `Settings` 要的宽度' +
        '超过五等分的格子，格子之间互相压字，且整页出现横向滚动' +
        '（spec：labels never scroll horizontally）',
    ).not.toBeNull();
    expect(
      win?.value,
      `\`.m-nav-label\` 的胜出值是 \`overflow-wrap: ${win?.value}\`。` +
        '`break-word` 只在渲染时断词、**不改 min-content 尺寸**，而标签是 `flex:1 1 0` 项的' +
        '子项、交叉轴按 fit-content 取宽（下限即 min-content）⇒ 格子仍按 max-content 互相挤，' +
        '换了等于没换。只有 `anywhere` 同时压 min-content',
    ).toBe('anywhere');
  });

  it('导航高度是**下限**不是定值（写死 = 断词后的第二行画到手势条上）', () => {
    // Only the >=1024 side rail may fill the viewport. Conditional bottom-nav rules
    // still need this guard, since a narrow-width height also clips enlarged labels.
    const isSideRail = (d: Decl): boolean => d.conds.includes('@media (min-width: 1024px)');
    const fixed = chain('.m-nav', 'height', (d) => !isSideRail(d));
    expect(
      fixed.map((d) => `${d.file}: ${d.value}`),
      '`.m-nav` 上出现了 `height` —— 标签换行后导航长不出去，第二行会压在 `.m-dock` 的' +
        '`--safe-b` 手势条留白上（spec 允许 wrap to two lines，前提是导航让得出那一行）',
    ).toEqual([]);
    expect(chain('.m-nav', 'height', isSideRail).map((d) => d.value)).toEqual([
      'var(--m-vv-height, 100%)',
    ]);
    // The >=1024 side rail intentionally has min-height:0; the bottom dock
    // retains the --nav-h floor at its unconditional/base width.
    const min = winner('.m-nav', 'min-height', UNCONDITIONAL);
    expect(min, '`.m-nav` 既没有 `height` 也没有 `min-height` —— 导航失去了它的基准高度').not.toBeNull();
    expect(
      readsVar(min?.value ?? '', '--nav-h'),
      `\`.m-nav\` 的 min-height 胜出值是 \`${min?.value}\`，读不到 --nav-h —— ` +
        '基准高度不再来自 `core.json` 的 layout 真值源',
    ).toBe(true);
  });
});
