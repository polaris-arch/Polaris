/**
 * 裁判自己的门 —— **正向对照组**。
 *
 * 这一组就是「它凭什么当裁判」的证据：喂一批**已知答案**的 CSS，逐条断言它给出的正是浏览器的答案。
 * 覆盖面按第四轮验收点名的那几类挑：特异度、`!important`、`:is()`、属性选择器、
 * 自定义属性的**值**被覆写、`@media screen` 恒真、at-rule 嵌进规则块、`@layer`。
 * 每一条都同时是静态模型 `css-cascade.test-support.ts` 四轮下来答错或答不了的那一类
 * （编号对应验收员的 R5-0x）。
 *
 * 另有两条**反向对照**：目标元素不存在 ⇒ 抛（不是返回空让否定断言恒真）；
 * 浏览器缺席 ⇒ 抛（不是 skip）。
 */
import { mkdtempSync, existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, describe, expect, it } from 'vitest';
import { contextOf, memoryFs, stripComments, winnerValue } from './css-cascade.test-support';
import {
  DRAIN,
  MEDIA_DEFAULTS,
  PLUGIN_LOADERS,
  abandon,
  assertColorAsksArePinned,
  assertNoOracleLeak,
  assertPluginRegistryMatchesConfig,
  browserWsUrl,
  buildCssUnit,
  buildDocument,
  closeOracle,
  cssBundleScan,
  cssBundleUnits,
  cssChain,
  declaredPluginsOf,
  hostSkeleton,
  inflightMeasures,
  measure,
  oracleHygiene,
  parseDevToolsEndpoint,
  productPostcssPlugins,
  resolveChromeBinary,
  spawnOwnedBrowser,
  waitForDevToolsEndpoint,
} from './css-oracle.test-support';
import type { SpawnedBrowser } from './css-oracle.test-support';
import { inMobileShell } from './mount.test-support';

// 收尾走 CDP 的 `Browser.close`，让 Chrome 自己退（全程不 kill 进程）。
// 超时给到 30s：全量并行跑时机器争用，默认的 10s hookTimeout 会在收尾这一步假红。
afterAll(closeOracle, 30_000);

/** 一次问一个属性的糖 —— 本文件的断言都是单点的。 */
const one = async (css: string[], html: string, select: string, prop: string): Promise<string> =>
  (await measure({ css, html }, [{ select, props: [prop] }])).get(select, prop);

describe('① 层叠：特异度 / 源序 / important', () => {
  it('同特异度后者胜（静态模型的「首条命中即返回」正是反的）', async () => {
    expect(
      await one(['.a{color:rgb(1,2,3)}', '.a{color:rgb(4,5,6)}'], '<i class="a">x</i>', '.a', 'color'),
    ).toBe('rgb(4, 5, 6)');
  });

  it('R5-01：互不包含但命中同一元素 —— `.mn-row .mn-lat` 压过 `.mn-lat.fast`', async () => {
    // 这正是第四轮 blocker 的原样：静态模型把这一族声明为「契约外」，
    // 而它的补偿控制 11 道门里 8 道一处都没接 ⇒ 门全绿、真机上 fast 档变红。
    const css = [
      '.mn-lat{color:rgb(1,2,3)}',
      '.mn-lat.fast{color:rgb(59,176,121)}',
      '.mn-row .mn-lat{color:rgb(239,67,67)}',
    ];
    const html = '<div class="mn-row"><span class="mn-lat fast">42</span></div>';
    expect(await one(css, html, '.mn-lat.fast', 'color')).toBe('rgb(239, 67, 67)');
    // 正向对照：把祖先拿掉，同一批 CSS 下 fast 档就是绿的 —— 判据不是恒红。
    expect(await one(css, '<span class="mn-lat fast">42</span>', '.mn-lat.fast', 'color')).toBe(
      'rgb(59, 176, 121)',
    );
  });

  it('`!important`：更宽 + important 必胜；两侧都 important 时回到特异度', async () => {
    expect(
      await one(
        ['.a.b{color:rgb(1,2,3)}', '.a{color:rgb(9,9,9)!important}'],
        '<i class="a b">x</i>',
        '.a.b',
        'color',
      ),
    ).toBe('rgb(9, 9, 9)');
    expect(
      await one(
        ['.a.b{color:rgb(1,2,3)!important}', '.a{color:rgb(9,9,9)!important}'],
        '<i class="a b">x</i>',
        '.a.b',
        'color',
      ),
    ).toBe('rgb(1, 2, 3)');
  });

  it('R5-05：`:is()` 包住的更宽选择器带 important —— 静态模型的纯语法超集判定漏它', async () => {
    expect(
      await one(
        ['.mr-ap-body.dim{color:rgb(1,2,3)}', ':is(.mr-ap-body,.zzz){color:rgb(9,9,9)!important}'],
        '<div class="mr-ap-body dim">x</div>',
        '.mr-ap-body.dim',
        'color',
      ),
    ).toBe('rgb(9, 9, 9)');
  });

  it('属性选择器包住的更宽选择器带 important —— 同上一族', async () => {
    expect(
      await one(
        ['.pane.dim{color:rgb(1,2,3)}', '[class~="pane"]{color:rgb(9,9,9)!important}'],
        '<div class="pane dim">x</div>',
        '.pane.dim',
        'color',
      ),
    ).toBe('rgb(9, 9, 9)');
  });
});

describe('② R5-02 / R5-03：更窄规则**新增**属性 / 更宽规则供着本选择器没写的属性', () => {
  it('R5-02：`.mr-screen .mr-ap-body.dim` 新增 `pointer-events` —— 本选择器自己没写过', async () => {
    // 静态模型的 narrower 桶只报「与本选择器重叠的分量」（`if (!won.has(key)) continue`），
    // 「更窄规则新增一个本选择器没写过的属性」整类被排除 —— 而否定判据守的正是这一类。
    expect(
      await one(
        ['.mr-ap-body.dim{opacity:.5}', '.mr-screen .mr-ap-body.dim{pointer-events:none}'],
        '<div class="mr-screen"><div class="mr-ap-body dim">x</div></div>',
        '.mr-ap-body.dim',
        'pointer-events',
      ),
    ).toBe('none');
  });

  it('R5-03：更宽且**非** important 的 `.mr-ap-body` 供着 `pointer-events` —— 更窄那条没声明它', async () => {
    // 静态模型无条件丢弃「更宽且非 important」的候选，注释把一个有前提的规则写成了不变量：
    // 前提是「更窄那条自己也声明了这个属性」。它没声明时，浏览器里生效的就是更宽那条。
    expect(
      await one(
        ['.mr-ap-body{pointer-events:none}', '.mr-ap-body.dim{opacity:.5}'],
        '<div class="mr-ap-body dim">x</div>',
        '.mr-ap-body.dim',
        'pointer-events',
      ),
    ).toBe('none');
    // 正向对照：更窄那条把它写回来，就轮到更窄的胜 —— 上一条不是恒 `none`。
    expect(
      await one(
        ['.mr-ap-body{pointer-events:none}', '.mr-ap-body.dim{opacity:.5;pointer-events:auto}'],
        '<div class="mr-ap-body dim">x</div>',
        '.mr-ap-body.dim',
        'pointer-events',
      ),
    ).toBe('auto');
  });
});

describe('③ R5-07：自定义属性的**值**被覆写', () => {
  it(':root 后一条 `--ok` 改掉了角色色 —— 模型只比字面量 `var(--ok)` 时全线无门', async () => {
    const css = [':root{--ok:142 51% 46%}', '.fast{color:hsl(var(--ok))}', ':root{--ok:0 84% 60%}'];
    // hsl(0 84% 60%) = rgb(239, 67, 67)。这个期望值第一次是手算写错的，裁判当场纠正 ——
    // 「别自己再算一遍引擎」这条教训在写这一行时就兑现了一次。
    expect(await one(css, '<i class="fast">x</i>', '.fast', 'color')).toBe('rgb(239, 67, 67)');
  });

  it('探针：角色色比对的是**两个 rgb**，不是源码里写着哪个令牌名', async () => {
    const m = await measure(
      {
        css: [':root{--ok:142 51% 46%;--err:0 84% 60%}', '.fast{color:hsl(var(--ok))}'],
        html: '<i class="fast">x</i>',
        probes: { ok: 'hsl(var(--ok))', err: 'hsl(var(--err))' },
      },
      [{ select: '.fast', props: ['color'] }],
    );
    expect(m.get('.fast', 'color')).toBe(m.probe('ok'));
    expect(m.probe('ok')).not.toBe(m.probe('err'));
  });
});

describe('④ R5-06 / R5-09：条件块', () => {
  it('R5-06：`@media screen` 在目标引擎上**恒真** —— 默认「只看无条件档」是给出反的答案', async () => {
    expect(
      await one(
        ['.h-connlabel.s-connected{color:rgb(1,2,3)}', '@media screen{.h-connlabel.s-connected{color:rgb(239,67,67)}}'],
        '<b class="h-connlabel s-connected">x</b>',
        '.h-connlabel.s-connected',
        'color',
      ),
    ).toBe('rgb(239, 67, 67)');
  });

  it('R5-09：at-rule **嵌进规则块** —— 两道门齐绿的那个形状', async () => {
    expect(
      await one(
        ['.a{color:rgb(1,2,3); @media screen{ color:rgb(4,5,6) }}'],
        '<i class="a">x</i>',
        '.a',
        'color',
      ),
    ).toBe('rgb(4, 5, 6)');
  });

  it('不成立的条件确实不生效 —— 上面两条不是「条件块一律赢」', async () => {
    expect(
      await one(
        ['.a{color:rgb(1,2,3)}', '@media print{.a{color:rgb(4,5,6)}}'],
        '<i class="a">x</i>',
        '.a',
        'color',
      ),
    ).toBe('rgb(1, 2, 3)');
  });

  it('视口是判据的一部分：同一份 CSS 在两个视口下答案不同', async () => {
    const css = ['.a{width:10px}', '@media (min-width:600px){.a{width:80px}}'];
    const html = '<i class="a" style="display:block">x</i>';
    const narrow = await measure({ css, html, viewport: { width: 390, height: 844 } }, [
      { select: '.a', props: ['width'] },
    ]);
    const wide = await measure({ css, html, viewport: { width: 1280, height: 800 } }, [
      { select: '.a', props: ['width'] },
    ]);
    expect(narrow.get('.a', 'width')).toBe('10px');
    expect(wide.get('.a', 'width')).toBe('80px');
  });
});

describe('⑤ R5-08：`@layer` 与构建期注入的 Tailwind', () => {
  it('无层的作者规则**永远**压过有层的 —— 与特异度无关', async () => {
    expect(
      await one(
        ['@layer utilities{ .a.b.c{color:rgb(9,9,9)} }', '.a{color:rgb(1,2,3)}'],
        '<i class="a b c">x</i>',
        '.a',
        'color',
      ),
    ).toBe('rgb(1, 2, 3)');
  });

  it('层间按声明序：后声明的层胜（静态模型对 `@layer` 是「遇到就抛」）', async () => {
    expect(
      await one(
        ['@layer one, two;', '@layer two{ .a{color:rgb(4,5,6)} }', '@layer one{ .a{color:rgb(1,2,3)} }'],
        '<i class="a">x</i>',
        '.a',
        'color',
      ),
    ).toBe('rgb(4, 5, 6)');
  });

  it('移动端进包单元与外部依赖都钉住 —— 取材面多一块少一块都会红', () => {
    const s = cssBundleScan('src/mobile/MobileMain.tsx');
    expect(s.units, '移动端进包 CSS 的清单或顺序变了').toEqual([
      'src/mobile/home/home.css',
      'src/mobile/connections/connections.css',
      'src/mobile/nodes/nodes.css',
      'src/mobile/screens/rules/rules-screen.css',
      'src/mobile/settings/settings.css',
      // 表单宿主（批 2）：排在五份屏 CSS **之后**、外壳之前 —— 它盖在屏上、不压外壳。
      // 次序由 `MobileApp.tsx` 里那一行 import 的位置决定（那里有一条别挪它的注释）。
      'src/mobile/forms/forms.css',
      'src/styles/tokens.resolved.css',
      'src/mobile/theme.css', // Explicit mobile dark layer; light remains the shared baseline.
      'src/mobile/mobile.css',
      'src/mobile/redesign.css',
      'src/mobile/screens/rules/rules-redesign.css',
      'src/mobile/connections/connections-redesign.css',
    ]);
    // 被跳过的裸说明符**自曝**：新添一个运行时依赖就红一次，逼人回答「它随包发 CSS 吗」。
    expect(s.externals, '移动端模块图里多了/少了一个 npm 依赖').toEqual([
      '@tauri-apps/api/core',
      '@tauri-apps/api/event',
      'i18next',
      'react',
      'react-dom', // MobileSelect portal；不自带 CSS，样式仍由 mobile.css 提供。
      'react-dom/client',
      'react-i18next',
      'zustand',
    ]);
  });

  it('TS 侧的说明符不再静默丢弃（R5-08 的另一半：M01）', () => {
    const entry = 'src/zz/entry.tsx';
    // ① 裸说明符带样式后缀 ⇒ 到 node_modules 解析并**进链**（上一版：静默丢弃）。
    //    实测过它的后果：给 `MobileMain.tsx` 加 `import 'tailwindcss/preflight.css'`，
    //    真 `vite build` 的 mobile chunk 从 59.66 kB 涨到 64.19 kB，而裁判的链里一条都没有。
    const withPkgCss = memoryFs({
      [entry]: "import 'somepkg/preflight.css';\nimport './a.css';\n",
      'node_modules/somepkg/preflight.css': '.p{ color: red }\n',
      'src/zz/a.css': '.a{ color: blue }\n',
    });
    expect(cssBundleScan(entry, withPkgCss).units).toEqual([
      'node_modules/somepkg/preflight.css',
      'src/zz/a.css',
    ]);
    // ② 同样的说明符在 node_modules 里找不到 ⇒ 抛，不静默跳过。
    const missingPkgCss = memoryFs({
      [entry]: "import 'somepkg/preflight.css';\nimport './a.css';\n",
      'src/zz/a.css': '.a{ color: blue }\n',
    });
    expect(() => cssBundleScan(entry, missingPkgCss)).toThrow(/真的进包的 CSS/);
    // ③ 相对说明符解析不到 ⇒ 抛。
    const brokenRelative = memoryFs({ [entry]: "import './nope';\nimport './a.css';\n", 'src/zz/a.css': '.a{}' });
    expect(() => cssBundleScan(entry, brokenRelative)).toThrow(/缺得无声无息/);
    // ④ 正向对照：普通 npm 包照常记进 externals，不抛（上面三条不是恒抛）。
    const plain = memoryFs({ [entry]: "import { x } from 'react';\nimport './a.css';\n", 'src/zz/a.css': '.a{ color: red }' });
    expect(cssBundleScan(entry, plain)).toEqual({ units: ['src/zz/a.css'], externals: ['react'] });
  });

  it('桌面链吃的是**构建产物**：`@import \'tailwindcss\'` 真展开了，产物里不剩构建期构造', async () => {
    const units = cssBundleUnits('src/main.tsx');
    expect(units, '桌面进包单元不是 index.css 一份 —— 打包形状变了').toEqual(['src/styles/index.css']);
    const built = await buildCssUnit(units[0]);
    expect(built.length, 'Tailwind 那一大块没进来').toBeGreaterThan(100_000);
    expect(built, '产物里还剩 @import —— 管线没真跑').not.toMatch(/@import\b/);
    expect(built, 'Tailwind 的层没建起来').toContain('@layer theme, base, components, utilities');
  }, 30_000);
});

describe('⑥ 反向对照：故障关闭', () => {
  it('目标元素在真 DOM 上不存在 ⇒ 抛（不是返回空让否定断言恒真）', async () => {
    await expect(
      measure({ css: ['.a{color:red}'], html: '<i class="a">x</i>' }, [
        { select: '.never-existed', props: ['color'] },
      ]),
    ).rejects.toThrow(/一个元素都没命中/);
  });

  it('浏览器缺席 ⇒ 抛（不是 skip）', () => {
    expect(() => resolveChromeBinary(['/nonexistent/chrome'])).toThrow(/找不到浏览器/);
    // 正向对照：本机这份候选表是找得到的 —— 上一条不是恒抛。
    expect(resolveChromeBinary()).toMatch(/chrom/i);
  });

  it('`ctx` 与 `css` 必须二选一，且不许一条都不问', async () => {
    await expect(
      // @ts-expect-error 故意两个都不给 —— 验的正是运行期那道闸（类型层现在也拦得住，故这行必须显式豁免）
      measure({ html: '<i></i>' }, [{ select: 'i', props: ['color'] }]),
    ).rejects.toThrow(/二选一/);
    await expect(measure({ css: ['.a{color:red}'], html: '<i class="a"></i>' }, [])).rejects.toThrow(/空判据/);
  });

  it('祖先链按**计算值**问：谁真的吸顶，由浏览器答（不是「源码里写过 sticky」）', async () => {
    const css = [
      '.mc-top{position:sticky;top:0}',
      // 某一档把吸顶取消掉 —— 静态模型的「无条件档穷举」腿对它是瞎的。
      '@media screen{.mc-top{position:static}}',
    ];
    const html = '<div class="mc-top"><input data-conn-search></div>';
    const m = await measure({ css, html }, [
      { select: '[data-conn-search]', props: ['position'], ancestorsWhere: { prop: 'position', value: 'sticky' } },
    ]);
    expect(m.ancestors('[data-conn-search]'), '条件档把吸顶取消了，祖先链里就不该再有吸顶元素').toEqual([]);
    // 正向对照：去掉那一档，同一条判据立刻找得到它 —— 不是恒空。
    const ok = await measure({ css: [css[0]], html }, [
      { select: '[data-conn-search]', props: ['position'], ancestorsWhere: { prop: 'position', value: 'sticky' } },
    ]);
    expect(ok.ancestors('[data-conn-search]')).toEqual(['div.mc-top']);
  });

  it('命中个数是可问的 —— 「祖先链存不存在」由真 DOM 说了算', async () => {
    const m = await measure(
      {
        css: ['.mn-lat{color:red}'],
        html: '<div class="mn-row"><span class="mn-lat">a</span><span class="mn-lat">b</span></div>',
      },
      [{ select: '.mn-row .mn-lat', props: ['color'], many: 'all' }],
    );
    expect(m.count('.mn-row .mn-lat')).toBe(2);
  });

  it('M02：命中多个而调用方没表态 ⇒ 抛（不是沉默地只读第一个）', async () => {
    // 三个 `.x`，**中间那个**被后一条规则改掉。上一版只读第一个 ⇒ 这个差异整条不可见，
    // 而紧挨着的 `count(...) > 0` 让人读成「这一族都查了」（traffic 夹具下 `.mr-grip` 就是 3 个）。
    const css = ['.x{color:rgb(1,2,3)}', '.bad{color:rgb(9,9,9)}'];
    const html = '<i class="x">a</i><i class="x bad">b</i><i class="x">c</i>';
    await expect(measure({ css, html }, [{ select: '.x', props: ['color'] }])).rejects.toThrow(
      /命中 3 个元素/,
    );
    const all = await measure({ css, html }, [{ select: '.x', props: ['color'], many: 'all' }]);
    expect(all.getAll('.x', 'color'), '中间那个的差异必须看得见').toEqual([
      'rgb(1, 2, 3)',
      'rgb(9, 9, 9)',
      'rgb(1, 2, 3)',
    ]);
    // `all` 的问法上单点读法禁用 —— 否则「只读第一个」这条又原样装回来了。
    expect(() => all.get('.x', 'color')).toThrow(/many: 'all'/);
    // `first` 是一句写出来的话：明说只读第一个，count 照常报全数。
    const first = await measure({ css, html }, [{ select: '.x', props: ['color'], many: 'first' }]);
    expect(first.get('.x', 'color')).toBe('rgb(1, 2, 3)');
    expect(first.count('.x')).toBe(3);
    // 反过来也堵上：`first` 的问法上全集读法禁用，否则「以为遍历了全族」原样装回来。
    expect(() => first.getAll('.x', 'color')).toThrow(/many: 'first'/);
    // 正向对照：命中恰好一个时不必表态（上面那条不是恒抛）。
    const one1 = await measure({ css, html: '<i class="x">a</i>' }, [{ select: '.x', props: ['color'] }]);
    expect(one1.get('.x', 'color')).toBe('rgb(1, 2, 3)');
  });
});

describe('⑦ 两侧对拍：静态模型不许静默给出与浏览器相反的答案', () => {
  /**
   * R5-05 的姊妹腿（第五轮 M05）：`:where()` 特异度为 0 但**参与匹配**。
   * 上一版静态模型把它当「恒真条件」忽略掉，于是 `.a:where(.b)` 既不进更窄桶也不进更宽桶 ⇒
   * 静默丢弃，`winnerValue` 给出 `rgb(1,2,3)`；而浏览器里它与 `.a` 同特异度、后来者胜。
   * 本条把两侧摆在一起：**浏览器给答案，静态模型给「我答不了」**。
   */
  const CSS = ['.a{color:rgb(1,2,3)}', '.a:where(.b){color:rgb(4,5,6)}'];

  it('浏览器：`.a:where(.b)` 压过 `.a`（同特异度、源序在后）', async () => {
    expect(await one(CSS, '<i class="a b">x</i>', '.a', 'color')).toBe('rgb(4, 5, 6)');
    // 正向对照：元素不带 `.b` 时 `:where(.b)` 不命中 —— 它是真条件，不是恒真。
    expect(await one(CSS, '<i class="a">x</i>', '.a', 'color')).toBe('rgb(1, 2, 3)');
  });

  it('静态模型：同一份链上当场抛，而不是给出反的 `rgb(1,2,3)`', () => {
    const decls = contextOf(CSS.map((css, i) => ({ file: `mem${i}.css`, css })));
    expect(() => winnerValue({ sel: '.a', prop: 'color', decls })).toThrow(/特异度打平/);
  });
});

describe('⑧ 进程与 profile 卫生：收尾不保证就得自曝（第五轮 M03）', () => {
  it('检漏器有牙：合成一个「还活着 / profile 还在」的快照 ⇒ 抛并点名', () => {
    expect(() =>
      assertNoOracleLeak({
        started: 2,
        liveBrowsers: [{ pid: 424242, dir: '/var/tmp/polaris-css-oracle-fake' }],
        leftProfiles: ['/var/tmp/polaris-css-oracle-fake'],
      }),
    ).toThrow(/424242/);
    // 正向对照：干净的快照不抛（上一条不是恒抛）。
    expect(() => assertNoOracleLeak({ started: 1, liveBrowsers: [], leftProfiles: [] })).not.toThrow();
  });

  it('启动抛出的路径会清 profile（进程都没起来时，目录不该留在 /var/tmp）', async () => {
    const dir = mkdtempSync('/var/tmp/polaris-css-oracle-abandon-');
    expect(existsSync(dir)).toBe(true);
    // 子进程与 ws 都还没建起来 —— 正是 `launch` 在等 `DevToolsActivePort` 之前抛掉的那一档。
    await abandon({ pid: undefined, dir, closed: false }, undefined, undefined);
    expect(existsSync(dir), '启动失败路径把 profile 留在了 /var/tmp').toBe(false);
  });

  it('收尾覆盖「启动在途」：不 await 的 measure + 立刻收尾 ⇒ 不留孤儿', async () => {
    // M03 的确定性复现：上一版 `closeOracle` 看到 `CONN === null` 就返回，而 `OPENING` 还在途中；
    // 浏览器随后才起来，配 `child.unref()` 就成了 ppid=1、请退不掉的孤儿（宿主上实测攒了 14 个）。
    await closeOracle(); // 先回到「一个实例都没有」的状态，否则走不到在途那条路
    // 处理器**当场**挂上：晚一个 tick 挂，这次度量的失败会先被 Node 记成 unhandled rejection。
    const inFlight = measure({ css: ['.a{color:red}'], html: '<i class="a">x</i>' }, [
      { select: '.a', props: ['color'] },
    ]).catch(() => undefined);
    await closeOracle();
    await inFlight; // 这次度量成不成功都行，要的是它别把浏览器落下
    const h = oracleHygiene();
    expect(h.started, '这一条根本没启动过浏览器 ⇒ 它证明不了什么').toBeGreaterThan(0);
    expect(h.liveBrowsers, '收尾撒手了：浏览器随后才起来，没人回收').toEqual([]);
    expect(h.leftProfiles, 'profile 没清干净').toEqual([]);
  }, 60_000);
});


/* ═══════════════════════════════════════════════════════════════════════════
 * ⑨ 第六轮：**进料口与收尾的每一条故障关闭分支，各自有门**。
 *
 * 上一轮 M04 的结论是「守 R5-08 的分支自己无声无息，是同一个缺陷换了个位置」。本轮验收在
 * **同一个文件里**又抓到三条同形的（探针 N2 / N6）：`BUILD_TIME_AT_RULE` 改成永不匹配的正则 ⇒ 全绿。
 * 所以这一组的规矩是：本模块每一条 `throw`，都要有一条喂**合成输入**把它打红的判据，
 * 外加一条正向对照证明它不是恒抛。
 *
 * 🔴 **第七轮 T7：这条规矩上一轮写在这里，而事实上不成立** —— 规矩留在头注里却做不到，
 * 比不写更坏。本轮逐条对过 `css-oracle.test-support.ts` 的全部 33 条 `throw`
 * （改前：32 条 `throw new Error` + 1 条 `throw err`；改后 31 + 1），处置如下：
 *  · 补上原先没判据的四条：`waitForDevToolsEndpoint` 的两条（Chrome 起不来 / 等超时）、
 *    `closeOracle` 的在途度量排空超时、`getAll` 问了没问过的属性 —— 都在下面第 ⑩ 组；
 *  · 删掉一条：`lastSampledPoint` 是死代码且头注是假话（第七轮 T3），连函数一起删；
 *  · 其余 27 条本轮逐条核对过消费方，各自都有一条打红它的判据（分布在 ⑤⑥⑧⑨⑩ 与
 *    `css-oracle-sampling.test.ts`）。
 *
 * 剩下**一条明确从规矩里划出去**，理由写在这里，而不是让规矩继续挂着一句做不到的话：
 *  · `launch()` 里的 `catch { await abandon(...); throw err; }`。它不是一条新的故障关闭分支 ——
 *    自己没有消息，只是把 `launchInto` 已经抛出的那个异常在清完 profile 之后原样上抛，
 *    打红它等于打红被它转抛的那一条。要造出这个输入必须让**真 Chrome 启动失败**，而
 *    `CHROME_BIN` 在模块加载期就定死、本模块任何路径都不 kill 进程、`launch` 也不导出 ——
 *    结构上造不出输入。它**有效果的那一半**（失败路径清 profile）有判据：⑧「启动抛出的路径会清 profile」。
 * ═══════════════════════════════════════════════════════════════════════════ */
describe('⑨ 第六轮：故障关闭分支逐条有门', () => {
  const MODULE_SRC = readFileSync(
    fileURLToPath(new URL('./css-oracle.test-support.ts', import.meta.url)),
    'utf8',
  );

  describe('A1 收尾：在途的度量必须等回来', () => {
    it('生产门路径（`ctx` + 真挂载树）上不 await 的 measure，收尾必须把它等回来', async () => {
      // 🔴 上一轮那条 M03 的门用的是**内存链**：`measure({css: …})` 同一个 tick 就走到 `conn()`，
      // 于是 `OPENING` 有值、收尾等得到。而五道生产门走的是 `ctx` 路径 —— 先 `await cssChain(ctx)`
      // 跑 postcss，收尾时 `CONN` 与 `OPENING` **都还是 null**，`closeOracle` 原样返回。
      // 实测那一档：vitest rc=0、宿主留 12 个 chrome + 1 个 profile、一个字都不打。
      await closeOracle(); // 先回到「一个实例都没有」，否则走不到「启动都还没开始」那条路
      let settled = false;
      const inFlight = measure(
        { ctx: 'mobile', html: inMobileShell('<span class="a1-leaf"></span>', 'home') },
        [{ select: '.a1-leaf', props: ['display'] }],
      ).then(
        () => (settled = true),
        () => (settled = true),
      );
      expect(inflightMeasures(), '这次度量根本没登记在册 ⇒ 下面那条恒真').toBe(1);
      expect(settled, '度量在收尾开始前就落地了 ⇒ 这一条没测到「在途」那一档').toBe(false);
      await closeOracle();
      expect(
        settled,
        '收尾返回时那次度量还在途中 —— 浏览器会在收尾之后才起来，而那时 `OWNED` 还是空的，' +
          '自查器对着空表说「干净」（第六轮 A1 实测：宿主留 12 个 chrome，vitest rc=0）',
      ).toBe(true);
      expect(inflightMeasures()).toBe(0);
      const h = oracleHygiene();
      expect(h.liveBrowsers, '收尾后还有活着的浏览器').toEqual([]);
      expect(h.leftProfiles, '收尾后还有没删的 profile').toEqual([]);
      await inFlight;
    }, 120_000);

    it('收尾 hook 由本模块自己注册 —— 消费方忘了写 `afterAll` 也覆盖得到', () => {
      // 「忘了写」上一版只剩 `beforeExit` 一条兜底：它晚于 vitest 报 rc，且只能打 stderr。
      const src = stripComments(MODULE_SRC);
      expect(src, '本模块不再自己挂收尾 hook —— 「谁 import 它谁就有收尾」这句话失效了').toMatch(
        /^afterAll\(closeOracle,/m,
      );
      // 正向对照：同一条扫描对着一份没挂 hook 的源码不报（上一条不是恒真）。
      expect(stripComments('const x = 1;\nexport function closeOracle() {}\n')).not.toMatch(
        /^afterAll\(closeOracle,/m,
      );
    });
  });

  describe('A2 `DevToolsActivePort`：等两行写全再读，且没有 ws 也请退得掉', () => {
    it('半截文件读不出端点（Chrome 是分两行写的）', () => {
      // 本轮 12 次全量跑里**自然触发 1 次**：`SyntaxError: TypeError: Invalid URL`，
      // 随后 `abandon` 因 `ws === undefined` 放弃请退 ⇒ 那个浏览器成了回收不了的孤儿（实测留 10 个）。
      expect(parseDevToolsEndpoint('44663'), '只有端口一行就当成写完了').toBeNull();
      expect(parseDevToolsEndpoint('44663\n'), '第二行还是空的').toBeNull();
      expect(parseDevToolsEndpoint('446'), '端口自己都还没写完').toBeNull();
      expect(parseDevToolsEndpoint('44663\ndevtools/browser/abc'), '第二行不是路径').toBeNull();
      // 正向对照：写全了就读得出来（上面四条不是恒 null）。
      expect(parseDevToolsEndpoint('44663\n/devtools/browser/3ed38210-dfcc\n')).toEqual({
        port: '44663',
        path: '/devtools/browser/3ed38210-dfcc',
      });
    });

    it('手里没有 ws 也请退得掉：起一个真浏览器，只把 profile 交给 `abandon`', async () => {
      const b = spawnOwnedBrowser();
      await waitForDevToolsEndpoint(b);
      expect(browserWsUrl(b.owned.dir), '端口文件写全了却拼不出 CDP 地址').toMatch(
        /^ws:\/\/127\.0\.0\.1:\d+\/devtools\/browser\//,
      );
      const before = oracleHygiene().liveBrowsers.filter((x) => x.dir === b.owned.dir);
      expect(before.length, '浏览器根本没起来 ⇒ 下面「请退掉了」这条恒真').toBeGreaterThan(0);
      // **ws 一律不给** —— 正是 `launchInto` 在建 ws 之前抛掉的那一档。
      await abandon(b.owned, b.child, undefined);
      expect(
        oracleHygiene().liveBrowsers.filter((x) => x.dir === b.owned.dir),
        '没有 ws 就请退不动了 —— 那一档留下的浏览器模块自己永远回收不了（第六轮 A2）',
      ).toEqual([]);
      expect(existsSync(b.owned.dir), '请退掉了却把 profile 留在 /var/tmp').toBe(false);
    }, 120_000);
  });

  describe('A3 `prefers-color-scheme`：由判据钉住，不由跑门这台机器决定', () => {
    const CSS = ['.a{color:rgb(1,2,3)}', '@media (prefers-color-scheme: dark){.a{color:rgb(239,67,67)}}'];
    const HTML = '<i class="a">x</i>';
    const at = async (media?: Record<string, string>): Promise<string> =>
      (await measure({ css: CSS, html: HTML, ...(media === undefined ? {} : { media }) }, [
        { select: '.a', props: ['color'] },
      ])).get('.a', 'color');

    it('两条腿各自明确，且**不给** media 时落在钉死的那一档上', async () => {
      expect(await at({ 'prefers-color-scheme': 'dark' })).toBe('rgb(239, 67, 67)');
      expect(await at({ 'prefers-color-scheme': 'light' })).toBe('rgb(1, 2, 3)');
      // 关键的一条：默认档不跟着机器走。上一版给 `setEmulatedMedia` 传空 features = 清空覆写，
      // 于是同一个 commit 在深色系统与浅色系统上得出不同结论（实测同一条变异的两半一红一绿）。
      expect(await at(), '默认档跟着跑门这台机器的系统主题走了').toBe(await at({ 'prefers-color-scheme': 'light' }));
      expect(MEDIA_DEFAULTS['prefers-color-scheme'], '默认表里没有钉住它').toBe('light');
    }, 60_000);

    it('没有钉住 `prefers-color-scheme` 就问颜色 ⇒ 抛', () => {
      const colorAsk = [{ select: '.a', props: ['color'] }];
      expect(() => assertColorAsksArePinned({}, colorAsk)).toThrow(/没有显式覆写/);
      expect(() => assertColorAsksArePinned({ 'prefers-reduced-motion': 'reduce' }, colorAsk)).toThrow(
        /没有显式覆写/,
      );
      // 正向对照两条：钉住了不抛；没钉住但问的不是颜色也不抛（上面不是恒抛）。
      expect(() => assertColorAsksArePinned({ 'prefers-color-scheme': 'dark' }, colorAsk)).not.toThrow();
      expect(() => assertColorAsksArePinned({}, [{ select: '.a', props: ['width', 'height'] }])).not.toThrow();
      // 颜色面要宽于 `color` 一个词：背景/边框/阴影/描边同样随主题变。
      for (const prop of ['background-color', 'border-color', 'box-shadow', 'fill', 'caret-color'])
        expect(() => assertColorAsksArePinned({}, [{ select: '.a', props: [prop] }]), prop).toThrow(
          /没有显式覆写/,
        );
    });
  });

  describe('A4 宿主 `.html` 的 `<head><style>` 在链上', () => {
    it('`mobile.html` 那段首帧兜底 `<style>` 被取进来了', () => {
      const skel = hostSkeleton('mobile');
      expect(skel.headStyles.length, '`<head>` 里那段 `<style>` 一段都没取到').toBeGreaterThan(0);
      expect(skel.headStyles.join('\n'), '取到的不是那段首帧主题兜底').toMatch(/html:not\(\[data-theme\]\)/);
    });

    it('它真的参与层叠：`<html>` 的背景由它给，且 `data-theme` 一落定就失效', async () => {
      // 🔴 第六轮 A4 的形状：上一版读了同一份 `.html` 的 `html`/`body` 属性与挂载点，**却跳过了
      // 同一个文件里的 `<style>`` —— 往那里加一行就能改掉屏上的颜色而门全绿，与已判死刑的 B01 同形。
      const leaf = inMobileShell('<span class="a4-leaf"></span>', 'home');
      const ask = [{ select: 'html', props: ['background-color'] as const }];
      const light = await measure({ ctx: 'mobile', html: leaf, media: { 'prefers-color-scheme': 'light' } }, ask);
      const dark = await measure({ ctx: 'mobile', html: leaf, media: { 'prefers-color-scheme': 'dark' } }, ask);
      expect(light.get('html', 'background-color'), '宿主 `<style>` 没进链 ⇒ `<html>` 背景是透明的').toBe(
        'rgb(242, 245, 248)',
      );
      expect(dark.get('html', 'background-color'), '那段里的 `@media` 腿没生效').toBe('rgb(9, 13, 21)');
      // 正向对照：`data-theme` 一落定这段就该整体失效（否则上面两条量的是别的东西）。
      const themed = await measure({ ctx: 'mobile', html: leaf, rootAttrs: { 'data-theme': 'dark' } }, ask);
      expect(themed.get('html', 'background-color'), '`:not([data-theme])` 没起作用').toBe('rgba(0, 0, 0, 0)');
    }, 60_000);
  });

  describe('A5 postcss 管线：插件表从产品那份配置读', () => {
    it('配置表 == 登记表，且两个方向都会说话', async () => {
      const declared = (await productPostcssPlugins()).map((d) => d.name);
      expect(declared, '`postcss.config.js` 里声明的插件变了').toEqual([
        '@tailwindcss/postcss',
        'autoprefixer',
      ]);
      expect(Object.keys(PLUGIN_LOADERS), '本模块的登记表与配置对不上').toEqual(declared);
      expect(() => assertPluginRegistryMatchesConfig(declared)).not.toThrow();
      // 反向对照两条：配置里少一个 / 多一个都抛。
      // 🔴 「少一个」正是本轮 A5 的探针：把 `'@tailwindcss/postcss': {}` 从配置里删掉
      //    （真实构建的 CSS 因此完全变形），上一版全量 rc=0 全绿。
      expect(() => assertPluginRegistryMatchesConfig(['autoprefixer'])).toThrow(/本模块要跑、配置里没有/);
      expect(() => assertPluginRegistryMatchesConfig([...declared, 'postcss-nested'])).toThrow(
        /配置里有、本模块不会加载/,
      );
    });

    it('那几个插件是**真的在跑**：autoprefixer 补出来的前缀在产物里', async () => {
      const built = await buildCssUnit('zz.css', memoryFs({ 'zz.css': '.a{ user-select: none }' }));
      expect(built, 'autoprefixer 没在链上 —— 「跑产品自己那条管线」又是一句没人管的话').toContain(
        '-webkit-user-select',
      );
      expect(built, '无前缀那条也得在').toContain('user-select: none');
    }, 30_000);
  });

  describe('A6 构建产物与宿主文档：三条故障关闭分支各自有门', () => {
    it('产物里还剩构建期构造 ⇒ 抛（探针 N2：这条改成永不匹配的正则，上一版全绿）', async () => {
      await expect(
        buildCssUnit('zz.css', memoryFs({ 'zz.css': '@tailwind base;\n.a{color:red}' })),
      ).rejects.toThrow(/还剩构建期构造 `@tailwind`/);
      await expect(
        buildCssUnit('zz.css', memoryFs({ 'zz.css': '@config "./x.js";\n.a{color:red}' })),
      ).rejects.toThrow(/还剩构建期构造 `@config`/);
      // 正向对照：干净的一份过得去（上面两条不是恒抛）。
      await expect(buildCssUnit('zz.css', memoryFs({ 'zz.css': '.a{color:red}' }))).resolves.toContain(
        'color:red',
      );
      // 判据自己不许被注释污染：整份都是注释时不该报「还剩 @import」。
      await expect(
        buildCssUnit('zz.css', memoryFs({ 'zz.css': '/* 本文件一个 @import 都不写 */\n.a{color:red}' })),
      ).resolves.toContain('color:red');
    }, 60_000);

    it('构建出来是空的 ⇒ 抛（空取材面上的否定断言恒真）', async () => {
      await expect(buildCssUnit('zz.css', memoryFs({ 'zz.css': '' }))).rejects.toThrow(/构建出来是空的/);
    }, 30_000);

    it('单元里出现 `</style` ⇒ 抛（注入会截断，本模块不猜它的意思）', async () => {
      await expect(
        measure({ css: ['.a{color:red}</style><b>'], html: '<i class="a">x</i>' }, [
          { select: '.a', props: ['color'] },
        ]),
      ).rejects.toThrow(/注入会截断/);
    }, 30_000);

    const hostFs = (html: string, entry = "createRoot(document.getElementById('root')!)") =>
      memoryFs({ 'mobile.html': html, 'src/mobile/MobileMain.tsx': entry });
    const DOC = (head: string, body: string) =>
      `<!doctype html><html lang="en-US" dir="ltr"><head>${head}</head><body>${body}</body></html>`;

    it('宿主文档：挂载点不是**恰好一个** / 与入口对不上 / 有外链样式表（含 `<body>` 里的）⇒ 逐条抛', () => {
      expect(() =>
        hostSkeleton('mobile', hostFs(DOC('', '<div id="root"></div><div id="other"></div>'))),
      ).toThrow(/带 id 的空元素有 2 个/);
      expect(() => hostSkeleton('mobile', hostFs(DOC('', '<p>没有挂载点</p>')))).toThrow(
        /带 id 的空元素有 0 个/,
      );
      expect(() =>
        hostSkeleton('mobile', hostFs(DOC('', '<div id="root"></div>'), 'createRoot(document.body)')),
      ).toThrow(/宿主文档与入口脚本对不上/);
      expect(() =>
        hostSkeleton(
          'mobile',
          hostFs(DOC('<link rel="stylesheet" href="/x.css">', '<div id="root"></div>')),
        ),
      ).toThrow(/外链样式表/);
      // 🔴 第七轮 T4：扫描面是**整份文档**，不只 `<head>`。上一版只扫 `<head>`，
      //    写在宿主 `<body>` 里的外链样式表静默放行 —— 而 `<body>` 段是**原样**注进喂给浏览器的
      //    那份文档的（`buildDocument` 的 `before` / `after`），浏览器照样把它算进层叠。
      expect(() =>
        hostSkeleton(
          'mobile',
          hostFs(DOC('', '<link rel="stylesheet" href="/x.css"><div id="root"></div>')),
        ),
      ).toThrow(/外链样式表/);
      expect(() => hostSkeleton('mobile', hostFs('<html><body><div id="root"></div></body></html>'))).toThrow(
        /找不到/,
      );
      // 正向对照：**注释里**引着一句 `<link rel=stylesheet>` 不算数（判据不许被自己的取材面污染）。
      expect(() =>
        hostSkeleton(
          'mobile',
          hostFs(DOC('<!-- 这里不要写 <link rel="stylesheet" href="/x.css"> -->', '<div id="root"></div>')),
        ),
      ).not.toThrow();
    });

    it('宿主文档：正向对照 —— 规矩的一份解析得出来，且**注释里的 `<style>` 不算数**', () => {
      const skel = hostSkeleton(
        'mobile',
        hostFs(DOC('<!-- 为什么是 <style> 不是 <script>：… --><style>.k{color:red}</style>', '<div id="root"></div>')),
      );
      expect(skel.headStyles, '注释里那段引文被当成真的样式取进来了').toEqual(['.k{color:red}']);
      expect(skel.mountId).toBe('root');
      expect(skel.rootAttrs).toEqual({ lang: 'en-US', dir: 'ltr' });
    });
  });

  describe('A6-b 取材面与读出口：余下几条故障关闭分支也各自有门', () => {
    it('模块图：入口解析不到 / 一份 CSS 都没进包 ⇒ 抛', () => {
      const entry = 'src/zz/entry.tsx';
      expect(() => cssBundleScan(entry, memoryFs({}))).toThrow(/入口模块解析不到/);
      expect(() =>
        cssBundleScan(entry, memoryFs({ [entry]: "import { x } from 'react';\nexport const y = x;\n" })),
      ).toThrow(/一份 CSS 都没进包/);
      // 正向对照：有 CSS 就不抛（上一条不是恒抛）。
      expect(
        cssBundleScan(entry, memoryFs({ [entry]: "import './a.css';\n", 'src/zz/a.css': '.a{color:red}' })).units,
      ).toEqual(['src/zz/a.css']);
    });

    it('postcss 配置里读不出 `plugins` ⇒ 抛（不是当成「一个插件都不跑」）', () => {
      expect(() => declaredPluginsOf({ default: {} })).toThrow(/读不到 `plugins`/);
      expect(() => declaredPluginsOf({})).toThrow(/读不到 `plugins`/);
      expect(() => declaredPluginsOf(null)).toThrow(/读不到 `plugins`/);
      // 正向对照：规矩的一份读得出来。
      expect(declaredPluginsOf({ default: { plugins: { autoprefixer: {} } } })).toEqual([
        { name: 'autoprefixer', options: {} },
      ]);
    });

    it('读出口：没问过的选择器 / 属性 / 探针 ⇒ 抛（不是返回 undefined 让断言恒真）', async () => {
      const m = await measure({ css: ['.a{color:rgb(1,2,3)}'], html: '<i class="a">x</i>' }, [
        { select: '.a', props: ['color'] },
      ]);
      expect(m.get('.a', 'color')).toBe('rgb(1, 2, 3)'); // 正向对照：问过的读得到
      expect(() => m.get('.never-asked', 'color')).toThrow(/没问过/);
      expect(() => m.get('.a', 'width')).toThrow(/没问过/);
      expect(() => m.probe('nope')).toThrow(/没注入或没算出来/);
    }, 30_000);

    it('页面里取值时抛了 ⇒ 抛（非法选择器不许被当成「没命中」）', async () => {
      // 「没命中」与「问法本身是坏的」必须可区分：后者退化成前者就成了空取材面上的恒真否定断言。
      await expect(
        measure({ css: ['.a{color:red}'], html: '<i class="a">x</i>' }, [{ select: 'a[', props: ['color'] }]),
      ).rejects.toThrow(/裁判取值时页面抛了/);
    }, 30_000);
  });

  describe('A7 角色探针：不许与被判元素住在同一个会被覆写的层里', () => {
    const CSS = [':root{--ok:142 51% 46%}', '.t{color:hsl(var(--ok))}'];
    const HTML = '<i class="t">x</i>';
    const paint = (extra: readonly string[]) =>
      measure({ css: [...CSS, ...extra], html: HTML, probes: { ok: 'hsl(var(--ok))' } }, [
        { select: '.t', props: ['color'] },
      ]);

    it('令牌被改在 `body` 上时，被判元素动而探针不动 ⇒ 对拍分叉', async () => {
      // 🔴 第六轮 A7：上一版探针宿主挂在 `<body>` 里、与挂载树并列，于是这一档两边**一起动**，
      // 「对拍探针挡得住角色对调」这句话恒真。实测 `body{ --ok-weak: 0 0% 100% }` ⇒ 全量全绿，
      // 而浏览器实算的「当前出口」角标背景已经是纯白。
      const m = await paint(['body{--ok:0 0% 100%}']);
      expect(m.get('.t', 'color'), '被判元素没吃到 `body` 上的覆写 ⇒ 这一条没造出那个形状').toBe(
        'rgb(255, 255, 255)',
      );
      expect(m.probe('ok'), '探针跟着 `body` 一起动了 —— 它还住在会被覆写的那一层里').not.toBe(
        'rgb(255, 255, 255)',
      );
      expect(m.get('.t', 'color'), '两边没分叉 ⇒ 角色被就地改掉时对拍腿是瞎的').not.toBe(m.probe('ok'));
    }, 30_000);

    it('正向对照：没有覆写时两边一致（上一条不是恒不等），且 `:root` 调色由色族腿承重', async () => {
      const clean = await paint([]);
      expect(clean.get('.t', 'color'), '干净链上探针与被判元素就该相等').toBe(clean.probe('ok'));
      // `:root` 上改令牌时两边仍然一起动 —— 这是探针腿的**设计**（令牌调色不误伤），
      // 那一档由各屏门里的色族腿承重（`LAT_FAMILY` / `isDanger` / 角标的主导通道）。
      const retinted = await paint([':root{--ok:0 84% 60%}']);
      expect(retinted.get('.t', 'color')).toBe(retinted.probe('ok'));
      expect(retinted.get('.t', 'color'), '`:root` 调色应当两边一起动').toBe('rgb(239, 67, 67)');
    }, 30_000);
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * ⑩ 第七轮：**段序**这条可判定的事实，以及 A6 那条规矩上还欠着的几条 `throw`。
 *
 * 上一轮的收敛方式是「给声明建门」，而验收发现那道门自己有和被守对象同一类的洞。
 * 本轮不再往上加层：**守不住的地方把话说窄**（见 `css-oracle.test-support.ts` 头注的
 * 「抓不到的形态」表新增的三行），**守得住的地方补真判据**（本组）。
 * ═══════════════════════════════════════════════════════════════════════════ */
describe('⑩ 第七轮：段序 + A6 规矩上欠着的那几条', () => {
  /**
   * 🔴 T5：**宿主段与打包段的段序**此前一条判据都没有。
   *
   * 把 `buildDocument` 里的 `[...hostUnits, ...chain]` 反过来写成 `[...chain, ...hostUnits]`，
   * 全量 4013 条仍然全绿（第七轮验收实测）—— 而段序反了会把「同特异度谁压谁」整类判反
   * （宿主那段首帧兜底在真实构建产物里排在打包 CSS 的 `<link>` 之前，见 `dist/mobile.html`）。
   * 本轮补上本条之后再做同一条变异：全量 239 文件 / 4019 条里**只红本条**，其余 4018 条全过 ——
   * 可见此前确实没有第二处看着段序，而本条就是那一处。
   *
   * 段序是**可判定的事实**，不是覆盖面问题，所以这一条补的是判据而不是话。
   * 量的是生产那个函数**产出的那份文档**，不是在门里再拼一遍段序（再拼一遍 = 判据被自己污染）。
   */
  it('T5 段序：喂给浏览器的那份文档里，宿主 `<head><style>` 在前、打包 CSS 在后', async () => {
    const chain = await cssChain('mobile');
    const host = hostSkeleton('mobile');
    expect(host.headStyles.length, '宿主段一段都没有 ⇒ 下面那条顺序断言恒真').toBeGreaterThan(0);
    expect(chain.length, '打包段一段都没有 ⇒ 同上').toBeGreaterThan(0);
    const doc = buildDocument(
      { ctx: 'mobile', html: inMobileShell('<span class="t5-leaf"></span>', 'home') },
      chain,
    );
    const units = [...doc.matchAll(/<style data-unit="([^"]+)"/g)].map((m) => m[1]);
    expect(units, '喂进浏览器的段序不是「宿主段 ++ 打包段」—— 同特异度谁压谁会整类判反').toEqual([
      ...host.headStyles.map((_, i) => `mobile.html#style${i}`),
      ...chain.map((u) => u.file),
    ]);
  }, 60_000);

  describe('T7 A6 规矩上欠着的那几条 `throw`', () => {
    /** 合成一个「浏览器」：`waitForDevToolsEndpoint` 只读 `owned.dir` / `child.exitCode` / `stderr`。 */
    const fakeBrowser = (exitCode: number | null, dir: string): SpawnedBrowser => ({
      owned: { pid: undefined, dir, closed: false },
      child: { exitCode } as unknown as SpawnedBrowser['child'],
      stderr: ['(合成 stderr：这一档没有真的浏览器)'],
    });

    it('`waitForDevToolsEndpoint`：进程已经退了 ⇒ 抛；等超时 ⇒ 抛；写全了 ⇒ 读得出来', async () => {
      const dir = mkdtempSync('/var/tmp/polaris-css-oracle-portfile-');
      try {
        // ① 子进程已经退了 —— 再等下去只会等到超时，死因在 stderr 里。
        await expect(waitForDevToolsEndpoint(fakeBrowser(3, dir), 5_000)).rejects.toThrow(
          /Chrome 起不来（退出码 3）/,
        );
        // ② 进程还活着但端口文件迟迟不来 ⇒ 超时抛（不是永远吊着）。
        await expect(waitForDevToolsEndpoint(fakeBrowser(null, dir), 1)).rejects.toThrow(
          /等 DevToolsActivePort 写全超时/,
        );
        // ③ 正向对照：两行写全了就读得出来（上面两条不是恒抛）。
        writeFileSync(join(dir, 'DevToolsActivePort'), '44663\n/devtools/browser/abc-123\n');
        await expect(waitForDevToolsEndpoint(fakeBrowser(null, dir), 5_000)).resolves.toEqual({
          port: '44663',
          path: '/devtools/browser/abc-123',
        });
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    }, 30_000);

    it('`closeOracle`：在途的度量迟迟不落地 ⇒ 抛（不是撒手继续收尾）', async () => {
      // 撒手的后果第六轮 A1 实测过：浏览器在收尾之后才起来，`OWNED` 还空着，
      // 自查器对着空表说「干净」，宿主上留 12 个 chrome + 1 个 profile 而 vitest rc=0。
      await closeOracle();
      expect(DRAIN.timeoutMs, '默认等待上限被改过了').toBe(60_000);
      try {
        DRAIN.timeoutMs = 0; // 一次都不等
        // 正向对照①：一个都不在途时，同一个「一次都不等」的收尾不抛。
        await expect(closeOracle()).resolves.toBeUndefined();
        const inFlight = measure(
          { ctx: 'mobile', html: inMobileShell('<span class="t7-leaf"></span>', 'home') },
          [{ select: '.t7-leaf', props: ['display'] }],
        );
        await expect(closeOracle()).rejects.toThrow(/次度量在途/);
        // 把它等回来 —— 判据自己不许把浏览器丢在宿主上。
        await inFlight;
      } finally {
        DRAIN.timeoutMs = 60_000;
      }
      // 正向对照②：落地之后，默认档的收尾照常过（上面那条不是恒抛）。
      await expect(closeOracle()).resolves.toBeUndefined();
    }, 120_000);

    it('读出口：`getAll` 问了没问过的属性 ⇒ 抛（`get` 那一半上一轮已有门，这一半没有）', async () => {
      const m = await measure(
        { css: ['.x{color:rgb(1,2,3)}'], html: '<i class="x">a</i><i class="x">b</i>' },
        [{ select: '.x', props: ['color'], many: 'all' }],
      );
      expect(m.getAll('.x', 'color'), '正向对照：问过的属性读得到').toEqual([
        'rgb(1, 2, 3)',
        'rgb(1, 2, 3)',
      ]);
      expect(() => m.getAll('.x', 'width')).toThrow(/没问过/);
    }, 30_000);

    /**
     * 头注里「任何路径都不 kill 进程」此前是一句**自陈**：既没有判据，也没进
     * 「本裁判抓不到的形态」表。它对应的是本仓的硬禁令（宿主上有用户自己长期在跑的
     * Chrome，误杀不可接受），所以这一条不该靠自觉——它是可判定的。
     *
     * 判据面 = 本模块源码**剥掉注释与字符串之后**的那一份。剥是必须的：
     * 头注里逐字写着「不 kill」「`process.kill(pid, 0)`」，不剥就会被自己的注释喂饱
     * （本会话已三次栽在同形上：nodes.css 的注释、configuration.txt 的注释、
     * 判据脚本自己的头注）。
     *
     * 放行**唯一**一处：`process.kill(pid, 0)` —— 信号 0 不投递，是 POSIX 的存在性探针，
     * 不是杀。多一处、或那一处的信号不是字面 0，都必须红。
     */
    it('🔴 「任何路径都不 kill 进程」不是自陈：源码里除信号 0 的存在性探针外没有第二处 kill', () => {
      const raw = readFileSync(
        fileURLToPath(new URL('./css-oracle.test-support.ts', import.meta.url)),
        'utf8',
      );
      // 只剥注释（保长度，偏移不变）。**刻意不剥字符串**：本文件里有含引号字符的正则字面量
      // （如 `/['\"]/`），朴素的字符串剥离器会把它当成字符串开头、一路吃到几百行外，
      // 把取材面掏空——那正是下面第一条自检当场抓到的形态（2026-09-06 实际踩过一次）。
      // 不剥字符串是安全的：本判据的针是 `kill(`，而实测剥完注释后全模块只剩那一处
      // 存在性探针；若将来有人在字符串里写出 `kill(`，末尾那条「恰好 1 处」会红。
      const code = raw
        .replace(/\/\*[\s\S]*?\*\//g, (m) => ' '.repeat(m.length))
        .replace(/\/\/[^\n]*/g, (m) => ' '.repeat(m.length));

      // 取材面自检：剥完之后必须还认得出本模块（否则剥过头了，下面的断言恒真）。
      expect(code, '取材面自检：剥注释之后应当还剩得下函数定义').toMatch(
        /export function measure\s*\(/,
      );
      expect(
        raw.includes('任何路径都不 kill'),
        '取材面自检：头注里那句自陈还在（它正是本判据要兑现的那句话）',
      ).toBe(true);

      const calls = [...code.matchAll(/\bkill\s*\(([^)]*)\)/g)].map((m) => m[1].trim());
      // 唯一放行项：存在性探针 `process.kill(<pid>, 0)`。
      const notProbes = calls.filter((args) => !/,\s*0\s*$/.test(args));
      expect(
        notProbes,
        `css-oracle.test-support.ts 里出现了信号 0 之外的 kill 调用：${notProbes.join(' | ')}\n` +
          '宿主上有用户自己长期在跑的 Chrome，本模块一律走 CDP `Browser.close` 请退。',
      ).toEqual([]);
      expect(calls.length, '正向对照：那一处存在性探针还在（它没了说明本判据在空跑）').toBe(1);
    });
  });
});
