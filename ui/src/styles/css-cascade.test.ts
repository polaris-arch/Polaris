/**
 * `css-cascade.test-support.ts` 的自测 —— 解析器自己的门。
 *
 * # 为什么这份必须存在
 *
 * 全仓的 CSS 断言迁到同一个解析器之后，它就成了**单点**：它判错一次，所有门一起判错，
 * 而失效形态是「给出一个反的答案」而不是报错。所以这里每一条都带**双向对照**：
 * 正面证明它读得对，反面喂一个已知应该被抓住的输入，证明它真的报得出错。
 *
 * # 取材面
 *
 * 大部分用例吃**内存源**（`contextOf`），不写盘 —— 这棵树是多线共用的，写盘变异会互相吃掉快照
 * （本轮已发生两次）。只有 ⓪ 与 ⑩ 两组吃真实 corpus：前者钉进包顺序，后者钉族表的完备性。
 */
import { describe, it, expect } from 'vitest';
import {
  ALL,
  INITIAL,
  UNCONDITIONAL,
  context,
  contextOf,
  declaringSites,
  expandDecl,
  explain,
  isBroader,
  isNarrower,
  keyOf,
  parseCss,
  parseSelector,
  resolve,
  selectorKey,
  shorthandComponent,
  topLevelParts,
  winnerValue,
  winners,
  winnersText,
  OUT_OF_FAMILY_MODEL,
  UNRESOLVED_CSS_IMPORTS,
  cssOrderWithGaps,
  selectorAlternatives,
  memoryFs,
  narrowsWithoutSpecificity,
  type CtxName,
  type Decl,
} from './css-cascade.test-support';

const CTXS: readonly CtxName[] = ['desktop', 'mobile', 'tray', 'popup'];
const mem = (css: string, file = 'mem.css') => contextOf([{ file, css }]);
const readsVar = (value: string, name: string) => new RegExp(`var\\(\\s*${name}\\b`).test(value);

// ════════════════════════════════════════════════════════════════════════════
// ⓪ 上下文：四个入口各自推导得出进包顺序，且顺序是从模块图来的
// ════════════════════════════════════════════════════════════════════════════

describe('⓪ 四个层叠上下文', () => {
  for (const ctx of CTXS) {
    it(`${ctx} 解析得出、且有量级（取材面为空 ⇒ 其上的否定断言恒真）`, () => {
      const c = context(ctx);
      expect(c.files.length, `${ctx} 一份 CSS 都没进来`).toBeGreaterThan(0);
      expect(c.decls.length, `${ctx} 一条声明都没解析出来`).toBeGreaterThan(50);
    });
  }

  it('桌面链的顺序 = index.css 的 @import 序 + index.css 自身在最后', () => {
    // 与 `mobile-token-parity.test.ts` 那条「@import 整表逐字对拍」互为前提：
    // 那边钉住 @import 清单，这边钉住「本解析器确实照那个清单排」。
    expect(context('desktop').files).toEqual([
      'src/styles/fonts.css',
      'src/styles/tokens.css',
      'src/styles/components.css',
      'src/styles/screens.css',
      'src/styles/prototype.css',
      'src/styles/index.css',
    ]);
  });

  it('移动端：各屏 CSS 整体先于外壳 mobile.css 进包（外壳压屏，不是屏压外壳）', () => {
    const files = context('mobile').files;
    const shell = files.indexOf('src/mobile/mobile.css');
    expect(shell).toBeGreaterThanOrEqual(0);
    for (const screen of [
      'src/mobile/home/home.css',
      'src/mobile/nodes/nodes.css',
      'src/mobile/connections/connections.css',
      'src/mobile/screens/rules/rules-screen.css',
      'src/mobile/forms/forms.css',
    ]) {
      expect(files.indexOf(screen), `${screen} 排到了 mobile.css 之后 —— 进包顺序变了`).toBeLessThan(
        shell,
      );
    }
  });

  it('托盘链 = 桌面链 + tray-overlay.css 压在最后；弹窗链只有它自己', () => {
    expect(context('tray').files).toEqual([...context('desktop').files, 'src/tray/tray-overlay.css']);
    expect(context('popup').files).toEqual(['src/update-popup/style.css']);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ① 剥注释：注释喂不饱判据（SEAM-3 的根因）
// ════════════════════════════════════════════════════════════════════════════

describe('① 注释与字符串', () => {
  it('注释里的规则不进模型（改坏真规则 + 注释里留一份旧的正确规则 ⇒ 骗不过）', () => {
    const c = mem(`/* 旧版： .mn-lat.fast { color: hsl(var(--ok)) } */\n.mn-lat.fast{ color: hsl(var(--err)) }`);
    expect(winnerValue({ sel: '.mn-lat.fast', prop: 'color', decls: c })).toBe('hsl(var(--err))');
  });

  it('字符串留原文：属性选择器的值抹不得，`content` 里的花括号也不是结构', () => {
    const c = mem(`.a::before{ content: "}" ; color: red }\n:root[data-theme='dark'] .a{ color: blue }`);
    expect(winnerValue({ sel: '.a::before', prop: 'content', decls: c })).toBe('"}"');
    expect(selectorKey(`:root[data-theme='dark'] .a`)).toBe(selectorKey(':root[data-theme="dark"] .a'));
  });

  it('行号从保长度剥噪回算，与原文一致', () => {
    const css = ['/* 一', '   二 */', '.a{', '  color: red;', '}'].join('\n');
    const decls = parseCss('t.css', css, 0).decls;
    expect(decls).toHaveLength(1);
    expect(decls[0].line).toBe(4);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ② 选择器键归一：打穿 cascade-dead-rules 的第五个盲区
// ════════════════════════════════════════════════════════════════════════════

describe('② 选择器归一化键', () => {
  const same = (a: string, b: string) => expect(selectorKey(a)).toBe(selectorKey(b));
  const differ = (a: string, b: string) => expect(selectorKey(a)).not.toBe(selectorKey(b));

  it('组合符两侧空白不影响键（一个格式化器就能造出这种差异）', () => {
    same('.h-diagrow+.h-diagrow::before', '.h-diagrow + .h-diagrow::before');
    same('.a>.b', '.a > .b');
    same('.a~.b', '.a  ~  .b');
  });

  it('复合段内简单选择器排序：`.a.b` ≡ `.b.a`', () => {
    same('.mn-lat.fast', '.fast.mn-lat');
    same('.x .a.b:hover', '.x :hover.b.a');
  });

  it('`:before` ≡ `::before`；伪类名大小写不敏感', () => {
    same('.a:before', '.a::before');
    same('.a:HOVER', '.a:hover');
  });

  it('属性选择器引号与 `:not()` 参数递归归一', () => {
    same('[dir=rtl]', '[dir="rtl"]');
    same("[dir='rtl']", '[dir="rtl"]');
    same(":root:not([data-theme='light'])", ':root:not([data-theme="light"])');
  });

  it('反面：不同的选择器不许折成同一个键（归一化不是"把一切拉平"）', () => {
    differ('.a .b', '.a > .b');
    differ('.a.b', '.a .b');
    differ('.a::before', '.a::after');
    differ('[dir="rtl"]', '[dir="ltr"]');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ③ 层叠：后者胜（本轮三条 major 的根因回放）
// ════════════════════════════════════════════════════════════════════════════

describe('③ 同键决胜：收全部再取最后一条', () => {
  it('末尾追加一条同选择器规则 ⇒ 胜出值跟着变（旧读法在这里返回首条，正好反了）', () => {
    const before = mem('.h-diagdot{ margin-inline: 7.5px }');
    expect(winnerValue({ sel: '.h-diagdot', prop: 'margin-left', decls: before })).toBe('7.5px');
    const after = mem('.h-diagdot{ margin-inline: 7.5px }\n.h-diagdot{ margin-inline: 0 }');
    expect(winnerValue({ sel: '.h-diagdot', prop: 'margin-left', decls: after })).toBe('0');
    expect(resolve({ sel: '.h-diagdot', prop: 'margin-left', decls: after }).chain).toHaveLength(2);
  });

  it('跨文件按进包顺序，不按字母序', () => {
    const c = contextOf([
      { file: 'z-first.css', css: '.a{ color: red }' },
      { file: 'a-last.css', css: '.a{ color: blue }' },
    ]);
    expect(winnerValue({ sel: '.a', prop: 'color', decls: c })).toBe('blue');
  });

  it('归一化后同键的两条规则参与同一条链（`.a+.b` 与 `.a + .b`）', () => {
    const c = mem('.a+.b{ color: red }\n.a + .b{ color: blue }');
    const r = resolve({ sel: '.a + .b', prop: 'color', decls: c });
    expect(r.chain).toHaveLength(2);
    expect(r.winner?.value).toBe('blue');
  });

  it('`!important` 优先于源序；两条 important 之间仍按源序', () => {
    const c = mem('.a{ color: red !important }\n.a{ color: blue }');
    expect(winnerValue({ sel: '.a', prop: 'color', decls: c })).toBe('red');
    const two = mem('.a{ color: red !important }\n.a{ color: green !important }\n.a{ color: blue }');
    expect(winnerValue({ sel: '.a', prop: 'color', decls: two })).toBe('green');
  });

  it('同键上有条件声明而调用方没给 `where` ⇒ **抛**（BLK-2：默认沉默会给出反的答案）', () => {
    const css = '.a{ color: red }\n@container mscreen (min-width: 37.5em){ .a{ color: blue } }';
    const c = mem(css);
    // 失败关闭：报文必须点名那条声明的 file:line 与 prelude，读的人才知道要对什么表态。
    expect(() => winnerValue({ sel: '.a', prop: 'color', decls: c })).toThrow(/没有显式给/);
    expect(() => winnerValue({ sel: '.a', prop: 'color', decls: c })).toThrow(/mem\.css:2/);
    expect(() => winnerValue({ sel: '.a', prop: 'color', decls: c })).toThrow(/min-width: 37\.5em/);
    // 两个方向的显式表态都放行，且给出各自该给的答案。
    expect(winnerValue({ sel: '.a', prop: 'color', decls: c, where: UNCONDITIONAL })).toBe('red');
    expect(winnerValue({ sel: '.a', prop: 'color', decls: c, where: ALL })).toBe('blue');
    const cond = resolve({ sel: '.a', prop: 'color', decls: c, where: ALL }).chain[1].decl.conds;
    expect(cond).toEqual(['@container mscreen (min-width: 37.5em)']);
    // 反向对照：同键上**没有**条件声明时不抛（否则这道闸门等于把默认查询全禁了）。
    const plain = mem('.a{ color: red }');
    expect(winnerValue({ sel: '.a', prop: 'color', decls: plain })).toBe('red');
    // 条件声明落在**别的**选择器上也不抛 —— 本闸门守的是「本查询的答案被滤掉了」。
    const other = mem('.a{ color: red }\n@media screen{ .b{ color: blue } }');
    expect(winnerValue({ sel: '.a', prop: 'color', decls: other })).toBe('red');
  });

  it('`@keyframes` 里的关键帧声明不进作者来源的层叠', () => {
    const c = mem('.a{ color: red }\n@keyframes fade{ 0%{ color: blue } 100%{ color: green } }');
    expect(resolve({ sel: '.a', prop: 'color', decls: c }).chain).toHaveLength(1);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ④ 同族：简写 / 长写 / 逻辑属性折成同一个语义分量
// ════════════════════════════════════════════════════════════════════════════

describe('④ 属性族', () => {
  it('`gap` 认得出 `column-gap`（text-fit 那条「连追加都不用」的绕过路径）', () => {
    const c = mem('.tray-i{ gap: 10px; column-gap: 40px }');
    expect(winnerValue({ sel: '.tray-i', prop: 'column-gap', decls: c })).toBe('40px');
    expect(winnerValue({ sel: '.tray-i', prop: 'row-gap', decls: c })).toBe('10px');
  });

  it('`border-top` 认得出 `border-top-color`（bottom-navigation 那条同族覆写）', () => {
    const c = mem('.m-dock{ border-top: 1px solid hsl(var(--line)) }\n.m-dock{ border-top-color: transparent }');
    expect(winnerValue({ sel: '.m-dock', prop: 'border-top-color', decls: c })).toBe('transparent');
    expect(winnerValue({ sel: '.m-dock', prop: 'border-top-width', decls: c })).toBe('1px');
    expect(winnerValue({ sel: '.m-dock', prop: 'border-top-style', decls: c })).toBe('solid');
  });

  it('`border-color: a b c d` 与 `border-inline-start` 都归到同一组分量', () => {
    const c = mem('.a{ border-color: red blue green teal; border-inline-start: 2px dashed pink }');
    expect(winnerValue({ sel: '.a', prop: 'border-bottom-color', decls: c })).toBe('green');
    // ltr 下 inline-start = left，且边简写压过前面那条 border-color 的 left 分量。
    expect(winnerValue({ sel: '.a', prop: 'border-left-color', decls: c })).toBe('pink');
    expect(winnerValue({ sel: '.a', prop: 'border-left-width', decls: c })).toBe('2px');
    // rtl 下同一条声明落到 right。
    expect(winnerValue({ sel: '.a', prop: 'border-right-color', decls: c, env: { dir: 'rtl' } })).toBe('pink');
  });

  it('`margin` 简写 / `margin-inline` / `margin-inline-start` 同链', () => {
    const c = mem('.a{ margin: 1px 2px 3px 4px }\n.a{ margin-inline: 8px }\n.a{ margin-inline-start: 9px }');
    const r = resolve({ sel: '.a', prop: 'margin-left', decls: c });
    expect(r.chain.map((x) => x.value)).toEqual(['4px', '8px', '9px']);
    expect(r.winner?.value).toBe('9px');
    // 同一批声明在 bottom 上只有简写贡献。
    expect(winnerValue({ sel: '.a', prop: 'margin-bottom', decls: c })).toBe('3px');
  });

  it('轴映射：`inline-size` ≡ `width`、`min-block-size` ≡ `min-height`', () => {
    const c = mem('.a{ width: 10px }\n.a{ inline-size: 20px }\n.b{ min-height: 48px }\n.b{ min-block-size: 24px }');
    expect(winnerValue({ sel: '.a', prop: 'width', decls: c })).toBe('20px');
    expect(winnerValue({ sel: '.b', prop: 'min-height', decls: c })).toBe('24px');
  });

  it('`top/right/bottom/left` 与 `inset` / `inset-inline-*` 同链', () => {
    const c = mem('.a{ inset: 0 1px 2px 3px }\n.a{ inset-inline-start: 35px }');
    expect(winnerValue({ sel: '.a', prop: 'left', decls: c })).toBe('35px');
    expect(winnerValue({ sel: '.a', prop: 'inset-inline-end', decls: c })).toBe('1px');
  });

  it('简写查询有歧义 ⇒ 抛并点名分量（`cssValue(.x,"gap")` 正是同族全盲的入口）', () => {
    expect(() => keyOf('gap')).toThrow(/歧义/);
    expect(() => keyOf('padding')).toThrow(/歧义/);
    expect(() => keyOf('border-top')).toThrow(/border-top-width/);
  });

  it('值不求解：`var()` / `calc()` / `inherit` / `currentColor` 逐字进出（头注那条「不保证」的判据）', () => {
    const c = mem(
      '.a{ color: hsl(var(--ok)); padding-top: calc(var(--x) + 2px); border-top-color: currentColor }\n' +
        '.b{ color: inherit }',
    );
    expect(winnerValue({ sel: '.a', prop: 'color', decls: c })).toBe('hsl(var(--ok))');
    expect(winnerValue({ sel: '.a', prop: 'padding-top', decls: c })).toBe('calc(var(--x) + 2px)');
    expect(winnerValue({ sel: '.a', prop: 'border-top-color', decls: c })).toBe('currentColor');
    // `inherit` 按字面进模型，不追溯来源 —— 拿到的是「CSS 里写的那句」不是「浏览器算出来的那个」。
    expect(winnerValue({ sel: '.b', prop: 'color', decls: c })).toBe('inherit');
  });

  it('未登记的属性走原样直判，行为与迁移前一致', () => {
    const c = mem('.a{ position: sticky; color: red }');
    expect(winnerValue({ sel: '.a', prop: 'position', decls: c })).toBe('sticky');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑤ 更窄选择器：自曝而非裁决
// ════════════════════════════════════════════════════════════════════════════

describe('⑤ narrower 桶', () => {
  it('多一个简单选择器 / 多一段祖先 ⇒ 进桶（不给值，只报"有人从更窄处压你"）', () => {
    const c = mem('.fld-fold{ min-height: 0 }\n.dlg .fld-fold{ min-height: 40px }\n.tagchip{ color: red }\n.rv-pick .tagchip.off-pool{ color: blue }');
    const a = resolve({ sel: '.fld-fold', prop: 'min-height', decls: c });
    expect(a.narrower.map((x) => x.decl.rawSels[0])).toEqual(['.dlg .fld-fold']);
    const b = resolve({ sel: '.tagchip', prop: 'color', decls: c });
    expect(b.narrower.map((x) => x.decl.rawSels[0])).toEqual(['.rv-pick .tagchip.off-pool']);
  });

  it('`:root[dir="rtl"] .main` 对 `.main` 是更窄覆写（本仓真实存在两处）', () => {
    const c = mem('.main{ border-radius: 8px }\n:root[dir="rtl"] .main{ border-radius: 0 }');
    const r = resolve({ sel: '.main', prop: 'border-radius', decls: c });
    expect(r.narrower).toHaveLength(1);
  });

  it('更宽的候选直接丢弃，不制造噪音；同键的进 chain 不进 narrower', () => {
    const c = mem('.b{ color: red }\n.a.b{ color: blue }\n.b.a{ color: green }');
    const wide = resolve({ sel: '.a.b', prop: 'color', decls: c });
    expect(wide.narrower).toEqual([]);
    expect(wide.chain.map((x) => x.value)).toEqual(['blue', 'green']); // `.a.b` ≡ `.b.a`
  });

  it('组合符不一致的候选不进桶（`.x > .a` 不是 `.x .a` 的更窄）', () => {
    const c = mem('.x .a{ color: red }\n.x > .a{ color: blue }');
    expect(resolve({ sel: '.x .a', prop: 'color', decls: c }).narrower).toEqual([]);
  });

  it('🔴 `:where()` 不是恒真条件：特异度打平而匹配集更小 ⇒ 故障关闭（第五轮 M05）', () => {
    // 上一版这条写的是「多一段祖先本身就够严格 ⇒ 仍进 narrower 桶；同段里补一个 `:where()` 不进桶」。
    // 两句都不成立：`:where(.x) .a` 与 `.a:where(.x)` 的特异度都与 `.a` **打平**，
    // narrower 桶的结论（「在候选命中的元素上必胜，与源序无关」）对它们是假的；
    // 而后者上一版两个桶都不进 ⇒ 静默丢弃，浏览器里它按源序压过 `.a`（对拍见 `css-oracle.test.ts` ⑦）。
    const c = mem('.a{ color: red }\n:where(.x) .a{ color: blue }');
    expect(() => resolve({ sel: '.a', prop: 'color', decls: c })).toThrow(/特异度打平/);
    const same = mem('.a{ color: red }\n.a:where(.x){ color: blue }');
    expect(() => resolve({ sel: '.a', prop: 'color', decls: same })).toThrow(/特异度打平/);
    // 正向对照：`*` 是真的恒真 —— `*{ … !important }` 仍然走「更宽」那条腿，没被新桶抢走。
    const star = mem('.a{ color: red }\n*{ color: blue !important }');
    expect(() => resolve({ sel: '.a', prop: 'color', decls: star })).toThrow(/更宽的选择器/);
  });

  it('🔴 边界钉子：互不包含但可能命中同一元素的跨选择器，resolver 一句话都不说', () => {
    // `#mc-alt-top` 与 `.mc-top` 谁命中谁静态面上判不了 ⇒ 既不进 chain 也不进 narrower。
    // 这条不是含糊其辞，是契约外：那一族的权威判据是**裁判**（`css-oracle.test-support.ts`，
    // 真 DOM + 整条链）；`declaringSites` 的穷举腿只提供「有几处在写它」这一半。
    // 有人把「resolver 没报」读成「没有」时，这条测试是那份反驳。
    const c = mem('.mc-top{ position: sticky }\n#mc-alt-top{ position: sticky }');
    const r = resolve({ sel: '.mc-top', prop: 'position', decls: c });
    expect(r.chain).toHaveLength(1);
    expect(r.narrower).toEqual([]);
    // 穷举腿看得见它。
    expect(declaringSites('position', { decls: c }).map((d) => d.rawSels[0]).sort()).toEqual([
      '#mc-alt-top',
      '.mc-top',
    ]);
  });
});

describe('⑤-b winners：选择器最终长什么样', () => {
  it('逐分量取胜出值；同族与 important 一并折算', () => {
    const c = mem('.mn-pill{ padding: 2px 6px; border: 1px solid hsl(var(--surface-3)) }\n.mn-pill{ padding-inline-end: 9px }');
    const w = winners({ sel: '.mn-pill', decls: c }).won;
    expect(w.get('padding-top')?.value).toBe('2px');
    expect(w.get('padding-right')?.value).toBe('9px'); // 同族覆写胜出
    expect(w.get('border-left-width')?.value).toBe('1px');
    expect(winnersText({ sel: '.mn-pill', decls: c })).toContain('padding-right: 9px;');
  });

  it('注释里的旧规则喂不饱它（SEAM-3 那条变异的形状）', () => {
    const c = mem('/* .mn-lat.fast{ color: hsl(var(--ok)) } */\n.mn-lat.fast{ color: hsl(var(--err)) }');
    expect(winnersText({ sel: '.mn-lat.fast', decls: c })).not.toContain('--ok');
  });

  it('选择器一条都没命中 ⇒ 抛', () => {
    expect(() => winners({ sel: '.nope', decls: mem('.a{color:red}') })).toThrow(/一条规则都没命中/);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑥ 故障关闭：空取材面上的否定断言恒真，是本轮全部缺陷的同一个源头
// ════════════════════════════════════════════════════════════════════════════

describe('⑥ 故障关闭', () => {
  it('选择器一条都没命中 ⇒ 抛（不是返回空、不是恒真）', () => {
    const c = mem('.a{ color: red }');
    expect(() => resolve({ sel: '.never-existed', prop: 'color', decls: c })).toThrow(/一条规则都没命中/);
  });

  it('选择器命中但没人声明这个分量 ⇒ winnerValue 抛并附全表', () => {
    const c = mem('.a{ color: red }');
    expect(() => winnerValue({ sel: '.a', prop: 'padding-top', decls: c })).toThrow(/没有任何声明贡献/);
  });

  it('`@layer` / `@scope` ⇒ 抛（它们改写"同键后者胜"这条前提）', () => {
    expect(() => mem('@layer base{ .a{ color: red } }')).toThrow(/@layer/);
    expect(() => mem('@scope (.a){ .b{ color: red } }')).toThrow(/@scope/);
  });

  it('CSS 嵌套 `&` ⇒ 抛', () => {
    expect(() => mem('.a{ color: red; & .b{ color: blue } }')).toThrow(/嵌套/);
  });

  it('`writing-mode` / `direction` 声明 ⇒ 抛（逻辑↔物理映射的前提被改写了）', () => {
    expect(() => mem('.a{ writing-mode: vertical-rl }')).toThrow(/writing-mode/);
    expect(() => mem('.a{ direction: rtl }')).toThrow(/direction/);
  });

  it('属性落在已建模族的命名空间内却分不了类 ⇒ 抛并给 file:line 与原属性名', () => {
    const d: Decl = {
      file: 'x.css',
      line: 7,
      order: 0,
      sels: ['.a'],
      rawSels: ['.a'],
      prop: 'padding-blokc-end',
      value: '1px',
      important: false,
      conds: [],
      registered: false,
    };
    expect(() => expandDecl(d)).toThrow(/x\.css:7/);
    expect(() => expandDecl(d)).toThrow(/padding-blokc-end/);
  });

  it('dir 敏感查询 + 链里出现 `[dir=` 选择器 + 没给 env ⇒ 抛（本仓 RTL 是活的）', () => {
    const c = mem('.main{ padding-inline-start: 4px }\n:root[dir="rtl"] .main{ padding-inline-start: 9px }');
    expect(() => resolve({ sel: '.main', prop: 'padding-left', decls: c })).toThrow(/dir/);
    expect(resolve({ sel: '.main', prop: 'padding-left', decls: c, env: { dir: 'ltr' } }).chain).toHaveLength(1);
    // 与方向无关的键不受影响（轴映射在 horizontal-tb 下恒等）。
    const axis = mem('.main{ inline-size: 4px }\n:root[dir="rtl"] .main{ inline-size: 9px }');
    expect(resolve({ sel: '.main', prop: 'width', decls: axis }).chain).toHaveLength(1);
  });

  it('入口模块解析不到 ⇒ 抛', () => {
    expect(() => contextOf([])).not.toThrow(); // 空源合法（调用方自己的事）
    const empty = contextOf([]);
    expect(() => resolve({ sel: '.a', prop: 'color', decls: empty })).toThrow(/一条规则都没命中/);
  });

  it('查询既没给 ctx 也没给 decls ⇒ 抛（取材面必须显式选）', () => {
    expect(() => resolve({ sel: '.a', prop: 'color' })).toThrow(/显式/);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑦ 穷举腿与 @property 注册（MobileSettings 那条 M4）
// ════════════════════════════════════════════════════════════════════════════

describe('⑦ declaringSites', () => {
  it('`@property` 的 initial-value 算"声明过"（注册一次就让全树的 var() 回落值失效）', () => {
    const c = mem('@property --row-min{ syntax: "<length>"; inherits: true; initial-value: 64px }');
    const sites = declaringSites('--row-min', { decls: c });
    expect(sites).toHaveLength(1);
    expect(sites[0].registered).toBe(true);
    expect(sites[0].value).toBe('64px');
  });

  it('注册初值不进层叠链（它是初值不是声明）', () => {
    const c = mem('@property --row-min{ syntax: "<length>"; initial-value: 64px }\n:root{ --row-min: 54px }');
    expect(winnerValue({ sel: ':root', prop: '--row-min', decls: c })).toBe('54px');
    expect(declaringSites('--row-min', { decls: c })).toHaveLength(2);
  });

  it('穷举腿认同族：查 `min-height` 收得到 `min-block-size`', () => {
    const c = mem('.a{ min-height: 48px }\n.b{ min-block-size: 24px }');
    expect(declaringSites('min-height', { decls: c }).map((d) => d.rawSels[0]).sort()).toEqual(['.a', '.b']);
  });

  it('where 谓词能把条件块滤掉', () => {
    const c = mem('.a{ position: sticky }\n@container mscreen (min-width: 37.5em){ .b{ position: sticky } }');
    expect(declaringSites('position', { decls: c, where: UNCONDITIONAL })).toHaveLength(1);
    expect(declaringSites('position', { decls: c, where: ALL })).toHaveLength(2);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑧ 简写展开（逐字搬自 safe-area-consumption.test.ts:381-391）
// ════════════════════════════════════════════════════════════════════════════

describe('⑧ 简写按 1/2/3/4 值展开到 TRBL（写在错误分量里必须判得出来）', () => {
  it('四种值形态 + calc 分量 + 双向错位对照', () => {
    expect(shorthandComponent('1px', 'left')).toBe('1px');
    expect(shorthandComponent('1px 2px', 'left')).toBe('2px');
    expect(shorthandComponent('1px 2px 3px', 'bottom')).toBe('3px');
    expect(shorthandComponent('1px 2px 3px 4px', 'left')).toBe('4px');
    // `calc()` 里的空格不是分量分隔符 —— 切错了会把 3 值读成 5 值。
    expect(topLevelParts('var(--sp-2) var(--card-padding) calc(var(--x) + var(--y))').length).toBe(3);
    // 双向：分量错位判得出来。
    expect(readsVar(shorthandComponent('0 calc(var(--safe-r)) 0 0', 'right') ?? '', '--safe-r')).toBe(true);
    expect(readsVar(shorthandComponent('0 calc(var(--safe-r)) 0 0', 'left') ?? '', '--safe-r')).toBe(false);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑨ 完备性：族表写漏了必须当场红
// ════════════════════════════════════════════════════════════════════════════

describe('⑨ 族表完备性（真实 corpus）', () => {
  it('四个上下文的每一条声明都分得了类（分不了类的当场报 file:line）', () => {
    const bad: string[] = [];
    for (const ctx of CTXS) {
      for (const d of context(ctx).decls) {
        try {
          expandDecl(d);
        } catch (e) {
          bad.push(`${d.file}:${d.line} \`${d.prop}\` — ${(e as Error).message}`);
        }
      }
    }
    expect(bad, `族表写漏了：\n${bad.join('\n')}`).toEqual([]);
  });

  it('`OUT_OF_FAMILY_MODEL` 每条都在 corpus 里恰好命中至少一次（0 次 = 免死金牌）', () => {
    const hits = new Map<string, number>();
    for (const ctx of CTXS) {
      for (const d of context(ctx).decls) {
        const p = d.prop.toLowerCase();
        if (OUT_OF_FAMILY_MODEL.includes(p)) hits.set(p, (hits.get(p) ?? 0) + 1);
      }
    }
    const dead = OUT_OF_FAMILY_MODEL.filter((p) => !hits.has(p));
    expect(dead, `白名单里这些条目守的东西已经没了，删掉它们：${dead.join(', ')}`).toEqual([]);
  });

  it('反面对照：往命名空间里塞一个没登记的属性 ⇒ 立刻红', () => {
    // ⑨ 第一条的牙从这里来：如果族表对任何输入都不抛，那条断言就是恒真的。
    expect(() => mem('.a{ padding-diagonal: 3px }')).not.toThrow(); // 解析阶段不判属性
    const c = mem('.a{ padding-diagonal: 3px }');
    expect(() => expandDecl(c.decls[0])).toThrow(/padding-diagonal/);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑩ explain：迁移后的红必须一眼可辨是"真覆盖"还是"解析器判过头了"
// ════════════════════════════════════════════════════════════════════════════

describe('⑩ explain', () => {
  it('渲染 chain 与 narrower 全表，带 file:line、值、important 与条件', () => {
    const c = contextOf([
      { file: 'a.css', css: '.h-diagdot{ margin-inline: 7.5px }' },
      { file: 'b.css', css: '.h-diagdot{ margin-inline: 0 !important }\n.row .h-diagdot{ margin-left: 3px }' },
    ]);
    const text = explain(resolve({ sel: '.h-diagdot', prop: 'margin-left', decls: c }));
    expect(text).toContain('a.css:1');
    expect(text).toContain('b.css:1');
    expect(text).toContain('!important');
    expect(text).toContain('← 胜出');
    expect(text).toContain('b.css:2');
    expect(text).toContain('更窄选择器');
  });

  it('链为空时明说"空"，不是印一张假装有内容的表', () => {
    const c = mem('.a{ color: red }');
    expect(explain(resolve({ sel: '.a', prop: 'padding-top', decls: c }))).toContain('空');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑪ 解析器结构自检
// ════════════════════════════════════════════════════════════════════════════

describe('⑪ 解析器结构', () => {
  it('选择器列表按顶层逗号切，括号内的逗号不切', () => {
    const c = mem('.a , .b:not(.c, .d) , [x="a,b"]{ color: red }');
    expect(c.decls[0].rawSels).toEqual(['.a', '.b:not(.c, .d)', '[x="a,b"]']);
    expect(winnerValue({ sel: '.b:not(.c,.d)', prop: 'color', decls: c })).toBe('red');
  });

  it('条件块 prelude 按由内到外记录原文（判"哪个条件块"要靠它）', () => {
    const c = mem('@media (min-width: 40em){ @container mscreen (max-width: 18em){ .a{ color: red } } }');
    expect(c.decls[0].conds).toEqual([
      '@container mscreen (max-width: 18em)',
      '@media (min-width: 40em)',
    ]);
  });

  it('`parseSelector` 的复合段与组合符切分正确', () => {
    const p = parseSelector('.x > .a.b + .c');
    expect(p.compounds).toHaveLength(3);
    expect(p.combinators).toEqual(['>', '+']);
    expect(p.hasDir).toBe(false);
    expect(parseSelector(':root[dir="rtl"] .main').hasDir).toBe(true);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑫ 跨选择器的失败关闭（2026-09-05 第三轮验收的 BLK-1 / BLK-2 / MAJ-3）
// ════════════════════════════════════════════════════════════════════════════

describe('⑫ 跨选择器：更宽 + important 抛，更窄进桶', () => {
  it('`isBroader` 与 `isNarrower` 方向相反，且 `*` / `:where()` 在「更宽」方向上是恒真条件', () => {
    const P = (x: string) => parseSelector(x);
    expect(isBroader(P('.mn-lat'), P('.mn-lat.fast'))).toBe(true);
    expect(isNarrower(P('.mn-lat'), P('.mn-lat.fast'))).toBe(false);
    expect(isBroader(P('*'), P('.tray-i'))).toBe(true); // `* { … !important }` 本仓真有两条
    expect(isBroader(P('.mn-lat.fast'), P('.mn-lat.fast'))).toBe(false); // 同键不算更宽
    expect(isBroader(P('.mn-row .mn-lat'), P('.mn-lat'))).toBe(false); // 更窄不是更宽
    expect(isBroader(P('.pill'), P('.tray-i'))).toBe(false); // 互不包含：一句话都不说
  });

  it('🔴 BLK-1：更宽选择器带 `!important` ⇒ 抛（层叠里 important 排在特异度之前）', () => {
    const c = mem('.mn-lat.fast{ color: hsl(var(--ok)) }\n.mn-lat{ color: hsl(var(--err)) !important }');
    for (const run of [
      () => winnerValue({ sel: '.mn-lat.fast', prop: 'color', decls: c }),
      () => winnersText({ sel: '.mn-lat.fast', decls: c }),
    ]) {
      expect(run).toThrow(/更宽的选择器/);
      expect(run).toThrow(/mem\.css:2/);
    }
    // 反向对照之一：**同一条规则去掉 important** ⇒ 更宽必输 ⇒ 不抛，胜出仍是 `--ok`。
    const noBang = mem('.mn-lat.fast{ color: hsl(var(--ok)) }\n.mn-lat{ color: hsl(var(--err)) }');
    expect(winnerValue({ sel: '.mn-lat.fast', prop: 'color', decls: noBang })).toBe('hsl(var(--ok))');
    // 反向对照之二：important 在**互不包含**的选择器上 ⇒ 静态面判不了，契约外，不抛也不进桶。
    const apart = mem('.mn-lat.fast{ color: hsl(var(--ok)) }\n#other{ color: red !important }');
    expect(winnerValue({ sel: '.mn-lat.fast', prop: 'color', decls: apart })).toBe('hsl(var(--ok))');
    // 反向对照之三：更宽 + important 但**贡献的是别的分量** ⇒ 与本查询无关，不抛。
    const otherKey = mem('.mn-lat.fast{ color: hsl(var(--ok)) }\n.mn-lat{ padding-top: 1px !important }');
    expect(winnerValue({ sel: '.mn-lat.fast', prop: 'color', decls: otherKey })).toBe('hsl(var(--ok))');
  });

  it('🔴 MAJ-3：`winners` 也算 narrower 桶（此前只有 `resolve` 有，两道最大的门够不着）', () => {
    const c = mem('.mn-lat.fast{ color: hsl(var(--ok)) }\n.mn-row .mn-lat.fast{ color: hsl(var(--err)) }');
    const w = winners({ sel: '.mn-lat.fast', decls: c });
    expect(w.won.get('color')?.value).toBe('hsl(var(--ok))');
    expect(w.narrower.map((x) => x.decl.rawSels.join(', '))).toEqual(['.mn-row .mn-lat.fast']);
    // 🔴 R5-02（2026-09-05 第四轮）：更窄规则碰的是**别的键**时也要报。
    // 此前这里断言的是 `[]`，理由写着「它没覆写任何东西，报它只是噪音」——
    // 那句话对正面取值判据成立，对**否定**判据正好相反：否定判据守的就是「更窄规则**新增**
    // 一个本选择器没写过的属性」这一类（`.mr-screen .mr-ap-body.dim{ pointer-events: none }`）。
    const apart = mem('.mn-lat.fast{ color: red }\n.mn-row .mn-lat.fast{ padding-top: 1px }');
    expect(
      winners({ sel: '.mn-lat.fast', decls: apart }).narrower.map((x) => x.decl.rawSels.join(', ')),
    ).toEqual(['.mn-row .mn-lat.fast']);
    // 反向对照：**不**更窄的选择器（互不包含）仍然不进桶 —— 上一条不是「谁都报」。
    expect(
      winners({
        sel: '.mn-lat.fast',
        decls: mem('.mn-lat.fast{ color: red }\n#other .zz{ padding-top: 1px }'),
      }).narrower,
    ).toEqual([]);
    // BLK-2 与 MAJ-3 叠在一起的那条缝：**条件档里的**更窄覆写会被默认谓词滤掉 ⇒ 也要自曝。
    const cond = mem(
      '.mn-lat.fast{ color: red }\n@media screen{ .mn-row .mn-lat.fast{ color: blue } }',
    );
    expect(() => winners({ sel: '.mn-lat.fast', decls: cond })).toThrow(/跨选择器覆写/);
    expect(() => resolve({ sel: '.mn-lat.fast', prop: 'color', decls: cond })).toThrow(/没有显式给/);
    // 显式表态后照常给答案：`ALL` 下它进 narrower 桶，`UNCONDITIONAL` 下明说不看它。
    expect(
      winners({ sel: '.mn-lat.fast', decls: cond, where: ALL }).narrower.map((x) =>
        x.decl.rawSels.join(', '),
      ),
    ).toEqual(['.mn-row .mn-lat.fast']);
    expect(winners({ sel: '.mn-lat.fast', decls: cond, where: UNCONDITIONAL }).narrower).toEqual([]);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑬ 族表补齐与简写复位（MAJ-5 / MAJ-6）
// ════════════════════════════════════════════════════════════════════════════

describe('⑬ 未登记族与简写复位', () => {
  it('🔴 MAJ-5：`background` 与 `background-color` 折成同一个分量（此前是两个互不相干的键）', () => {
    const c = mem('.mr-geo-regions{ background: hsl(var(--surface-2)) }\n.mr-geo-regions{ background-color: hsl(var(--flow-weak)) }');
    expect(winnerValue({ sel: '.mr-geo-regions', prop: 'background-color', decls: c })).toBe(
      'hsl(var(--flow-weak))',
    );
    // 同一形状的另外几族。
    const fam = mem(
      '.a{ flex: 1 0 auto; overflow: hidden; outline: 2px solid red; text-decoration: underline dotted; list-style: none; place-items: center; font: 13px/1.5 sans-serif }',
    );
    const w = winners({ sel: '.a', decls: fam }).won;
    for (const [k, v] of [
      ['flex-grow', '1'],
      ['flex-shrink', '0'],
      ['flex-basis', 'auto'],
      ['overflow-x', 'hidden'],
      ['overflow-y', 'hidden'],
      ['outline-width', '2px'],
      ['outline-style', 'solid'],
      ['outline-color', 'red'],
      ['text-decoration-line', 'underline'],
      ['text-decoration-style', 'dotted'],
      ['list-style-type', 'none'],
      ['align-items', 'center'],
      ['justify-items', 'center'],
      ['font-size', '13px'],
      ['line-height', '1.5'],
      ['font-family', 'sans-serif'],
    ] as const) {
      expect(w.get(k)?.value, `${k} 没折算出来`).toBe(v);
    }
  });

  it('🔴 MAJ-6：简写把缺席分量复位成 `initial`（`border: none` 不再让「宽度有定义」恒真）', () => {
    const c = mem('.mr-geo-regions{ border: 1px solid hsl(var(--line)) }\n.mr-geo-regions{ border: none }');
    expect(winnerValue({ sel: '.mr-geo-regions', prop: 'border-top-width', decls: c })).toBe(INITIAL);
    expect(winnerValue({ sel: '.mr-geo-regions', prop: 'border-top-style', decls: c })).toBe('none');
    // 反向对照：写全的简写不复位（否则这条闸门会把正常写法一起打红）。
    const full = mem('.a{ border: 1px solid red }');
    expect(winnerValue({ sel: '.a', prop: 'border-top-width', decls: full })).toBe('1px');
    // 同一形状：`background: none` 复位底色、`outline: none` 复位宽度。
    const bg = mem('.a{ background-color: red }\n.a{ background: none }');
    expect(winnerValue({ sel: '.a', prop: 'background-color', decls: bg })).toBe(INITIAL);
    const ol = mem('.a{ outline-width: 2px }\n.a{ outline: none }');
    expect(winnerValue({ sel: '.a', prop: 'outline-width', decls: ol })).toBe(INITIAL);
  });

  it('未建模的简写形态**抛**，不静默丢分量（族表写漏了必须当场红）', () => {
    // `background` 的位置/尺寸本模块不建模。
    expect(() => expandDecl(mem('.a{ background: url(x.png) 50% 50% / cover }').decls[0])).toThrow(
      /分不了类/,
    );
    // 系统字体关键字（`font: menu`）同理。
    expect(() => expandDecl(mem('.a{ font: menu }').decls[0])).toThrow(/font/);
    // 命名空间新登记的前缀：`text-decoration-*` 里塞一个没登记的当场红，而 `text-align` 不受影响。
    expect(() => expandDecl(mem('.a{ text-decoration-skip: none }').decls[0])).toThrow(
      /text-decoration-skip/,
    );
    expect(() => expandDecl(mem('.a{ text-align: center }').decls[0])).not.toThrow();
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑭ 解析层：postcss（MAJ-7 —— 不带 `&` 的原生嵌套此前被静默拍平）
// ════════════════════════════════════════════════════════════════════════════

describe('⑭ postcss 解析层', () => {
  it('🔴 MAJ-7：不带 `&` 的原生嵌套 ⇒ 抛（此前被拍平成顶层规则，两个方向都错）', () => {
    // 判据是「非 at-rule 块里还有块」，不是「有没有 `&`」。
    expect(() => mem('.wrap{ color: inherit; .m-dock{ padding-bottom: 0 } }')).toThrow(/嵌套/);
    expect(() => mem('.wrap{ color: inherit; .m-dock{ padding-bottom: 0 } }')).toThrow(/\.wrap/);
    expect(() => mem('@media screen{ .a{ .b{ color: red } } }')).toThrow(/嵌套/);
    expect(() => mem('.a{ & .b{ color: red } }')).toThrow(/嵌套/);
    // 反向对照：`@media` / `@keyframes` 里的规则**不是**嵌套，照旧解析。
    expect(() => mem('@media screen{ .a{ color: red } }')).not.toThrow();
    expect(() => mem('@keyframes f{ 0%{ opacity: 0 } }')).not.toThrow();
  });

  it('未闭合花括号 ⇒ 抛（手写扫描器对它是静默容忍的，后面整批规则错位而无人报）', () => {
    expect(() => mem('.a{ color: red ')).toThrow(/解析失败/);
    expect(() => mem('.a{ color: red }')).not.toThrow();
  });

  it('注释在选择器里 / 值里都不进模型，行号仍与原文一致', () => {
    const c = mem('.a /* x */ , .b{ color: /* y */ red }');
    expect(c.decls[0].rawSels).toEqual(['.a', '.b']);
    expect(c.decls[0].value).toBe('red');
    const lines = parseCss('t.css', ['/* 一', '   二 */', '.a{', '  color: red;', '}'].join('\n'), 0).decls;
    expect(lines[0].line).toBe(4);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑯ 2026-09-05 第四轮：跨选择器三桶、`:is()` 展开、取材面的窟窿要出声
// ════════════════════════════════════════════════════════════════════════════

describe('⑯ R5-03：更宽且非 important 的选择器**供着**本选择器没写过的分量', () => {
  it('本选择器没声明该分量 ⇒ 链为空，但更宽那条进 `broaderSupplied`，报错里点名它', () => {
    // 浏览器里：`.mr-ap-body.dim` 这个元素的 `pointer-events` 就是 `none`，
    // 因为它自己没声明、更宽的 `.mr-ap-body` 声明了。
    // 「`.mr-ap-body.dim` 的声明面里没有 pointer-events」与「置灰的正文点得动」是两件事。
    const c = mem('.mr-ap-body{ pointer-events: none }\n.mr-ap-body.dim{ opacity: .5 }');
    const r = resolve({ sel: '.mr-ap-body.dim', prop: 'pointer-events', decls: c });
    expect(r.chain, '同键链应当为空 —— 本选择器确实没声明它').toEqual([]);
    expect(r.broaderSupplied.map((x) => x.decl.rawSels.join(', '))).toEqual(['.mr-ap-body']);
    expect(() => winnerValue({ sel: '.mr-ap-body.dim', prop: 'pointer-events', decls: c })).toThrow(
      /更宽的选择器供着它/,
    );
    // `winners` 那一侧同样自曝。
    expect(
      winners({ sel: '.mr-ap-body.dim', decls: c }).broaderSupplied.map((x) => x.decl.rawSels.join(', ')),
    ).toEqual(['.mr-ap-body']);
  });

  it('反向对照：本选择器自己声明了该分量 ⇒ 更宽那条特异度必输，照常丢弃不制造噪音', () => {
    const c = mem('.mr-ap-body{ pointer-events: none }\n.mr-ap-body.dim{ pointer-events: auto }');
    expect(winnerValue({ sel: '.mr-ap-body.dim', prop: 'pointer-events', decls: c })).toBe('auto');
    expect(winners({ sel: '.mr-ap-body.dim', decls: c }).broaderSupplied).toEqual([]);
  });
});

describe('⑰ R5-05：`:is()` 先拆分支再判更宽/更窄', () => {
  it('拆分支：`:is(.a,.b) .c` → 两条', () => {
    expect(selectorAlternatives(':is(.a, .b) .c').sort()).toEqual(['.a .c', '.b .c']);
    expect(selectorAlternatives('.plain')).toEqual(['.plain']);
    // 嵌套两层也拆得开。
    expect(selectorAlternatives(':is(.a,:is(.b,.c))').sort()).toEqual(['.a', '.b', '.c']);
  });

  it('`:is()` 包住的更宽选择器带 important ⇒ 抛（此前原样溜过去，给出反的答案）', () => {
    const c = mem('.mr-ap-body.dim{ color: red }\n:is(.mr-ap-body, .zzz){ color: blue !important }');
    expect(() => winnerValue({ sel: '.mr-ap-body.dim', prop: 'color', decls: c })).toThrow(
      /更宽的选择器\*\*带/,
    );
    // 反向对照：分支里**没有**更宽的那一支 ⇒ 互不包含，照旧丢弃，不制造噪音。
    const apart = mem('.mr-ap-body.dim{ color: red }\n:is(.qqq, .zzz){ color: blue !important }');
    expect(winnerValue({ sel: '.mr-ap-body.dim', prop: 'color', decls: apart })).toBe('red');
  });

  it('`:is()` 包住的更**窄**选择器同样判得出来（方向对称）', () => {
    const c = mem('.mn-lat.fast{ color: red }\n:is(.mn-row, .zz) .mn-lat.fast{ color: blue }');
    expect(
      winners({ sel: '.mn-lat.fast', decls: c }).narrower.map((x) => x.decl.rawSels.join(', ')),
    ).toEqual([':is(.mn-row, .zz) .mn-lat.fast']);
  });
});

describe('⑱ R5-08：取材面的窟窿必须出声', () => {
  it('桌面链里解析不到的裸 `@import` 恰好是白名单里那一个，且它确实在 index.css 上', () => {
    const { unresolved } = cssOrderWithGaps('src/main.tsx');
    expect(unresolved.map((u) => u.spec)).toEqual(['tailwindcss']);
    expect(unresolved[0].importer).toBe('src/styles/index.css');
    expect(context('desktop').unresolvedImports?.map((u) => u.spec)).toEqual(['tailwindcss']);
    // 白名单本身钉死：多一个就是「又有一块 CSS 悄悄不在取材面里」。
    expect([...UNRESOLVED_CSS_IMPORTS]).toEqual(['tailwindcss']);
  });

  it('没在白名单上的裸 `@import` ⇒ 抛（此前是静默跳过）', () => {
    // 合成一份带裸说明符的入口来验这条路 —— 真树上只有 `tailwindcss` 一个，
    // 光靠它证明不了「不在白名单上的会抛」。
    expect(() => cssOrderWithGaps('src/styles/__never-existed.css')).toThrow(/入口模块解析不到/);
    // 直接验判据本体：白名单是个字符串数组，`includes` 就是那道闸。
    expect(UNRESOLVED_CSS_IMPORTS.includes('bootstrap')).toBe(false);
  });

  it('移动端链没有窟窿（契约 A1：移动端不走 index.css 那条桌面层叠链）', () => {
    expect(cssOrderWithGaps('src/mobile/MobileMain.tsx').unresolved).toEqual([]);
  });
});

describe('⑲ R5-09：at-rule 嵌进规则块是**建模**的，不是「遇到就抛」', () => {
  it('`.a{ color:x; @media screen{ color:y } }` 解析成两条声明，内层带条件标签', () => {
    const c = mem('.a{ color: red; @media screen{ color: blue } }');
    const rows = c.decls.map((d) => ({ sel: d.rawSels.join(','), v: d.value, conds: d.conds }));
    expect(rows).toEqual([
      { sel: '.a', v: 'red', conds: [] },
      { sel: '.a', v: 'blue', conds: ['@media screen'] },
    ]);
    // 于是它照常触发 BLK-2：没显式给 `where` 就抛，不会给出「只有 red」这个反的答案。
    expect(() => winnerValue({ sel: '.a', prop: 'color', decls: c })).toThrow(/没有显式给/);
    expect(winnerValue({ sel: '.a', prop: 'color', decls: c, where: ALL })).toBe('blue');
  });

  it('反向对照：**规则**嵌进规则块仍然抛（那一类才是本模块不建模的）', () => {
    expect(() => mem('.a{ color: red; .b{ color: blue } }')).toThrow(/CSS 嵌套/);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ⑳ R5-08 的**修复本体**自己有没有门（第五轮 M04）
//
// 上一轮把「解析不到的裸 `@import` 不在白名单 ⇒ 抛」写进 `cssOrderWithGaps`，可是没人看着它：
// 2026-09-05 实测把 `css-cascade.test-support.ts:774` 的条件改成 `if (false)`
// （逐字等于恢复修复前那条静默跳过），全量 237 文件 / 3957 断言**全绿**。
// 守「取材面缺一块且缺得无声无息」的分支自己无声无息，是同一个缺陷换了个位置。
//
// 真树上只有 `tailwindcss` 一个裸说明符、且它在白名单里，所以这条分支在真 corpus 上**走不到** ——
// 判据必须喂合成模块图。[`memoryFs`] 就是为此存在的（不写盘：这棵树是多线共用的）。
// ════════════════════════════════════════════════════════════════════════════

describe('⑳ 取材面窟窿的闸门本身：不在白名单上的裸 `@import` ⇒ 抛', () => {
  const entry = 'src/zz/entry.tsx';
  const graph = (importSpec: string) =>
    memoryFs({
      [entry]: "import './a.css';\n",
      'src/zz/a.css': `@import '${importSpec}';\n.a{ color: red }\n`,
    });

  it('白名单外的裸说明符 ⇒ 抛并点名（把这条改成 `if (false)` 时本条转红）', () => {
    expect(() => cssOrderWithGaps(entry, graph('bootstrap'))).toThrow(/解析不到文件/);
    expect(() => cssOrderWithGaps(entry, graph('bootstrap'))).toThrow(/无声无息/);
  });

  it('正向对照：白名单内的照常记进 `unresolved`，不抛（上一条不是恒抛）', () => {
    expect(UNRESOLVED_CSS_IMPORTS).toContain('tailwindcss');
    const r = cssOrderWithGaps(entry, graph('tailwindcss'));
    expect(r.files).toEqual(['src/zz/a.css']);
    expect(r.unresolved).toEqual([{ importer: 'src/zz/a.css', spec: 'tailwindcss' }]);
  });

  it('正向对照：能解析的相对 `@import` 照常展开（合成图本身是活的）', () => {
    const fs = memoryFs({
      [entry]: "import './a.css';\n",
      'src/zz/a.css': "@import './b.css';\n.a{ color: red }\n",
      'src/zz/b.css': '.b{ color: blue }\n',
    });
    // 后序：被 import 的先进包。
    expect(cssOrderWithGaps(entry, fs).files).toEqual(['src/zz/b.css', 'src/zz/a.css']);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ㉑ R5-05 的姊妹腿：`:where()` 特异度为 0 但**参与匹配**（第五轮 M05）
//
// 上一版把 `*` 与 `:where(X)` 揉成一个「零特异度 ⇒ 恒真条件」，于是 `.a:where(.b)` 对着查询 `.a`
// 既判不出更窄（特异度没涨）也判不出更宽（它多了一个真条件），两个桶都不进 ⇒ **静默丢弃**。
// 而 Chrome 152 实测 `['.a{color:rgb(1,2,3)}', '.a:where(.b){color:rgb(4,5,6)}']` + `<i class="a b">`
// 的 computed 是 `rgb(4, 5, 6)` —— 模型给的是反的答案。两侧对拍见 `css-oracle.test.ts` ⑦。
// ════════════════════════════════════════════════════════════════════════════

describe('㉑ `:where()` 不再被当成恒真条件', () => {
  const P = (sel: string) => parseSelector(sel);

  it('`.a:where(.b)` 对着 `.a`：不是更窄、不是更宽，是「特异度打平而匹配集更小」', () => {
    expect(isNarrower(P('.a:where(.b)'), P('.a')), '特异度没涨，不该进「必胜」那一桶').toBe(false);
    expect(isBroader(P('.a:where(.b)'), P('.a')), '它多了一个真条件，不比 `.a` 宽').toBe(false);
    expect(narrowsWithoutSpecificity(P('.a:where(.b)'), P('.a'))).toBe(true);
  });

  it('这一族在 `resolve` 上是**故障关闭**的：抛，不给一个反的答案', () => {
    const c = contextOf([
      { file: 'a.css', css: '.a{ color: rgb(1,2,3) }' },
      { file: 'b.css', css: '.a:where(.b){ color: rgb(4,5,6) }' },
    ]);
    expect(() => winnerValue({ sel: '.a', prop: 'color', decls: c })).toThrow(/特异度打平/);
  });

  it('反向对照一：`*` 仍然是恒真条件（`* .a` 是收窄，`*{…}` 不是）', () => {
    expect(isBroader(P('*'), P('.a')), '`*{ … !important }` 那一族还得判得出更宽').toBe(true);
    expect(narrowsWithoutSpecificity(P('* .a'), P('.a')), '`* .a` 要求有父元素 ⇒ 收窄').toBe(true);
  });

  it('反向对照二：带特异度的更窄候选仍归 `isNarrower`，没被新桶抢走', () => {
    expect(isNarrower(P('.x .a'), P('.a'))).toBe(true);
    expect(narrowsWithoutSpecificity(P('.x .a'), P('.a'))).toBe(false);
    expect(isNarrower(P('.a.b'), P('.a'))).toBe(true);
    expect(narrowsWithoutSpecificity(P('.a.b'), P('.a'))).toBe(false);
  });

  it('反向对照三：没有 `:where()` 时同键照常裁决（新闸门不是恒抛）', () => {
    const c = contextOf([
      { file: 'a.css', css: '.a{ color: rgb(1,2,3) }' },
      { file: 'b.css', css: '.a{ color: rgb(4,5,6) }' },
    ]);
    expect(winnerValue({ sel: '.a', prop: 'color', decls: c })).toBe('rgb(4,5,6)');
  });
});
