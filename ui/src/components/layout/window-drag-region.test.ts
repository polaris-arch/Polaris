import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { afterAll, describe, expect, it } from 'vitest';
import {
  ALL,
  contextOf,
  declaringSites,
  winnerValue,
} from '@/styles/css-cascade.test-support';
import { closeOracle, measure } from '@/styles/css-oracle.test-support';
import { inDesktopShell } from '@/styles/mount.test-support';

function read(relative: string): string {
  return readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8');
}

describe('W11 桌面窗口拖动带', () => {
  const appShell = read('./AppShell.tsx');
  const sidebar = read('./Sidebar.tsx');
  const settingsSidebar = read('../screens/settings/SettingsSidebar.tsx');

  it('复用两列现有的空 drag-region，不用覆盖层吞页面交互', () => {
    expect(appShell).toContain('<div className="main-chrome" data-tauri-drag-region />');
    expect(sidebar).toContain('<div className="side-chrome" data-tauri-drag-region />');
    expect(settingsSidebar).toContain('<div className="side-chrome" data-tauri-drag-region />');
    expect(appShell.indexOf('className="main-chrome"')).toBeLessThan(
      appShell.indexOf('className="main-scroll"')
    );
  });

  /*
   * 几何走**共享解析器**（`styles/css-cascade.test-support.ts`），不再拿正则去比对源码原文。
   *
   * 旧写法是 `/:root\[data-os="win"\] \.side-chrome,\s*…\{\s*height:16px;\s*margin-bottom:4px;\s*\}/`
   * 这样一条「逐字长这样」的正则，三个方向上都太窄（2026-09-05 复核）：
   *  · **写法一变即假红**：把选择器组换个顺序、把两条声明换行、在中间插一条无关声明，全都打红它，
   *    而渲染结果一格没动；
   *  · **同族覆写全盲**：在同一份文件后面补一句 `block-size: 24px` 或 `margin-block-end: 0`，
   *    浏览器里命中区当场变形，而这条正则仍然匹配得上 —— 它比对的是那一段原文还在不在；
   *  · **层叠全盲**：`index.css` 是最后一个进包的，它压得过这两份，而正则只看单文件。
   *
   * 现在三条一起判：**两份文件各自**都得有（原本那条"两份同源"的不变量），
   * 且**整条桌面层叠链上的胜出值**也得是它（新增的那一维）。
   */
  const FILES = ['../../styles/prototype.css', '../../styles/components.css'] as const;
  const perFile = new Map(
    FILES.map((f) => [f, contextOf([{ file: f, css: read(f) }])] as const),
  );
  /** 逐分量：`file` 里 `sel` 的 `prop` 胜出值；同时钉住整条桌面链上的胜出值。 */
  const geo = (sel: string, prop: string): void => {
    const wins = FILES.map((f) => winnerValue({ sel, prop, decls: perFile.get(f)! }));
    expect(new Set(wins).size, `\`${sel} { ${prop} }\` 在两份 CSS 里不同值：${wins.join(' vs ')}`).toBe(1);
    expect(
      winnerValue({ sel, prop, ctx: 'desktop' }),
      `\`${sel} { ${prop} }\` 在整条桌面层叠链上的胜出值与两份源文件不一致 —— ` +
        '有人在后面进包的文件里把它盖掉了（`index.css` 是最后一个）',
    ).toBe(wins[0]);
  };
  const value = (sel: string, prop: string): string =>
    winnerValue({ sel, prop, decls: perFile.get(FILES[0])! });

  it('Windows/Linux 统一为 16px 命中区 + 4px 间隔', () => {
    for (const os of ['win', 'lin'] as const)
      for (const chrome of ['side-chrome', 'main-chrome'] as const) {
        const sel = `:root[data-os="${os}"] .${chrome}`;
        geo(sel, 'height');
        geo(sel, 'margin-bottom');
        expect(value(sel, 'height'), `${sel} 的命中区高度不是 16`).toBe('16px');
        expect(value(sel, 'margin-bottom'), `${sel} 的间隔不是 4`).toBe('4px');
      }
    for (const [sel, prop, want] of [
      ['.winctl', 'position', 'absolute'],
      ['.winctl', 'top', '3px'],
      ['.winctl', 'padding-top', '2px'],
      ['.winctl', 'padding-right', '2px'],
      ['.winctl', 'padding-bottom', '2px'],
      ['.winctl', 'padding-left', '2px'],
      ['.winctl button', 'height', '26px'],
    ] as const) {
      geo(sel, prop);
      expect(value(sel, prop), `${sel} 的 ${prop} 变了`).toBe(want);
    }
    // 反面：win/lin 下不许给 `.phead` 补右内边距（窗控区已经靠上面那条 chrome 让出位置了）。
    // 按**语义分量**穷举，不是按属性名：`padding-inline-end` 与 `padding: a b` 都算数。
    const offenders = declaringSites('padding-right', { ctx: 'desktop', where: ALL })
      .flatMap((d) => d.rawSels.map((r) => `${d.file}:${d.line} ${r}`))
      .filter((x) => /data-os="(?:win|lin)"/.test(x) && x.includes('.phead'));
    expect(offenders, 'win/lin 下给 `.phead` 补了右内边距').toEqual([]);
  });

  it('macOS 原生标题栏高度保持 36px', () => {
    const sel = ':root[data-os="mac"] .main-chrome';
    geo(sel, 'height');
    expect(value(sel, 'height')).toBe('36px');
  });

  it('自检：同一套读法对着改坏的合成输入确实报得出来（不是恒绿）', () => {
    const bad = contextOf([
      { file: 'x.css', css: ':root[data-os="win"] .side-chrome{ height:16px; margin-bottom:4px }' },
      // 同族覆写：逻辑属性写法，旧正则完全看不见它。
      { file: 'y.css', css: ':root[data-os="win"] .side-chrome{ block-size: 24px }' },
    ]);
    expect(
      winnerValue({ sel: ':root[data-os="win"] .side-chrome', prop: 'height', decls: bad }),
      '同族覆写（`block-size`）没被折进同一个分量 —— 本门对它就是瞎的',
    ).toBe('24px');
    // 选择器一条都没命中 ⇒ 抛，不是返回空。
    expect(() => winnerValue({ sel: '.never-existed-chrome', prop: 'height', ctx: 'desktop' })).toThrow();
  });
});

/* ═══════════════════════════════════════════════════════════════════════════
 * **浏览器裁判**：拖动带的几何等式由 Chrome 对整条桌面链算。
 *
 * 上面那组读的是静态模型的胜出值。它在两处答不了：
 *  · **取材面缺一块**：`index.css:9` 的 `@import 'tailwindcss'` 在静态模型里被静默丢掉
 *    （裸说明符解析不到 ⇒ `cssOrder` 直接跳过），而它是**真的进包**的一大块（29 万字符，
 *    含 preflight 的 `*,::before,::after{box-sizing:border-box;margin:0;…}`）。
 *    裁判吃的是**产品自己那条 postcss 管线的产物**，那一块在里面。
 *  · **几何是结果不是声明**：`height:16px` + `margin-bottom:4px` 两条声明合起来是
 *    「20px 之后才轮到内容」。真正该钉的是那个 20，不是两句写法。
 *
 * ── 祖先链由**真组件**给（2026-09-05 第五轮 B02 起）────────────────────────
 * 上一版这里写着「`AppShell` 依赖 store / i18n / tauri invoke，node 环境跑不起 SSR」，于是按源码
 * 结构手搭了一条 `.stage > .win > .shell > {.side, .main}`。那句话是错的：`renderToStaticMarkup(
 * <AppShell/>)` 在 node 里跑得通（`useEffect` 不执行，`os` 停在 `detectPlatform()` 的兜底值，
 * 而档位本来就由 `rootAttrs` 的 `data-os` 驱动）。手搭的代价是实测出来的：往真 `AppShell.tsx` 的
 * `.main-scroll` **之前**插一个 40px 元素（生产里内容区从第 20px 起变成第 60px 起），整门 8/8 全绿 ——
 * 量的是手搭的那棵树，不是产品那棵。现在渲染真组件，并由 `css-oracle.test-support.ts` 的
 * `hostSkeleton('desktop')` 从 `index.html` 套上 `<html lang> / <body> / #root`
 * （桌面变异 `#root .main-chrome{height:24px}` 正是靠这一段才判得出来）。
 * 「喂进去的祖先链与生产一致」这条判据本身有门：`styles/mount-tree.test.tsx`。
 * ═══════════════════════════════════════════════════════════════════════════ */
describe('W11-b 浏览器裁判：拖动带几何（真链 + 真挂载树）', () => {
  // 收尾走 CDP 的 `Browser.close`，让 Chrome 自己退（全程不 kill 进程）。
  // 超时给到 30s：全量并行跑时机器争用，默认的 10s hookTimeout 会在收尾这一步假红。
  afterAll(closeOracle, 30_000);

  const appShellSrc = read('./AppShell.tsx');
  const sidebarSrc = read('./Sidebar.tsx');
  const titleBarSrc = read('./TitleBar.tsx');

  /** 产品那棵窗口外壳树（真组件 SSR；`#root` / `<html lang>` 由裁判按 `index.html` 套上）。 */
  const SHELL = inDesktopShell();

  const paint = (os: 'win' | 'lin' | 'mac', asks: Parameters<typeof measure>[1]) =>
    measure(
      {
        ctx: 'desktop',
        html: SHELL,
        rootAttrs: { 'data-os': os, 'data-theme': 'dark' },
        viewport: { width: 980, height: 740 },
      },
      asks,
    );

  it('自检：真组件渲染出来的祖先链与三份源码逐层对得上（结构改了先红这一条）', async () => {
    const order = (src: string, names: readonly string[]): number[] =>
      names.map((n) => {
        const at = src.indexOf(n);
        if (at < 0) throw new Error(`源码里找不到 \`${n}\` —— 重建的祖先链已经不是真的了`);
        return at;
      });
    const shellOrder = order(appShellSrc, [
      'className="stage"',
      'className="win"',
      'className="shell"',
      'className="main"',
      'className="main-chrome"',
      'className="main-scroll"',
    ]);
    expect([...shellOrder].sort((a, b) => a - b), 'AppShell 的层级次序变了').toEqual(shellOrder);
    const sideOrder = order(sidebarSrc, ["'side'", 'className="side-chrome"']);
    expect([...sideOrder].sort((a, b) => a - b), 'Sidebar 的层级次序变了').toEqual(sideOrder);
    expect(titleBarSrc, '`.winctl` 里不再是 button —— `.winctl button` 那条判据要重写').toMatch(
      /'winctl'[\s\S]{0,400}<button/,
    );
    // 真组件渲染出来的 DOM 上，被判元素一个不少（窗控三颗按钮 ⇒ 显式表态 `many: 'all'`）。
    const m = await paint('win', [
      { select: '.win .shell .side .side-chrome', props: ['height'] },
      { select: '.win .shell .main .main-chrome', props: ['height'] },
      { select: '.win .winctl button', props: ['height'], many: 'all' },
    ]);
    for (const sel of ['.win .shell .side .side-chrome', '.win .shell .main .main-chrome'])
      expect(m.count(sel), `\`${sel}\` 在真组件渲染的 DOM 上不存在`).toBe(1);
    expect(m.count('.win .winctl button'), '窗控按钮一颗都没渲染出来').toBeGreaterThan(0);
  }, 60_000);

  for (const os of ['win', 'lin'] as const) {
    it(`${os}：两条拖动带各 16px 高、之后让出 4px，内容从第 20px 起`, async () => {
      const m = await paint(os, [
        { select: '.side-chrome', props: ['height', 'margin-bottom'] },
        { select: '.main-chrome', props: ['height', 'margin-bottom'] },
        { select: '.main-scroll', props: ['height'] },
        { select: '.winctl', props: ['position', 'top', 'padding-top', 'padding-right', 'padding-bottom', 'padding-left'] },
        { select: '.winctl button', props: ['height'], many: 'all' },
      ]);
      for (const sel of ['.side-chrome', '.main-chrome'] as const) {
        expect(m.get(sel, 'height'), `${sel} 的命中区高度不是 16`).toBe('16px');
        expect(m.get(sel, 'margin-bottom'), `${sel} 的间隔不是 4`).toBe('4px');
        // 声明是两句，几何是一个数：真机上那条带子就是这么高。
        expect(m.rect(sel).height, `${sel} 渲染出来的命中区高度不是 16`).toBe(16);
      }
      // 几何等式：内容区的顶边 = 拖动带顶边 + 16 + 4。补一句 `block-size` 或
      // `margin-block-end` 就把这个数改了，而盯着两句声明原文的读法一个字都不会说。
      expect(
        m.rect('.main-scroll').y - m.rect('.main-chrome').y,
        '内容区不是从第 20px 起 —— 拖动带的高度或间隔被同族属性改掉了',
      ).toBe(20);
      expect(m.get('.winctl', 'position')).toBe('absolute');
      expect(m.get('.winctl', 'top')).toBe('3px');
      for (const side of ['top', 'right', 'bottom', 'left'] as const)
        expect(m.get('.winctl', `padding-${side}`), `.winctl 的 padding-${side} 变了`).toBe('2px');
      // 窗控是**三颗**按钮：逐颗断言，不是读第一颗（第五轮 M02 的形状）。
      const btns = m.getAll('.winctl button', 'height');
      expect(btns.length, '窗控按钮没渲染出来').toBeGreaterThan(0);
      for (const h of btns) expect(h, '窗控按钮高度变了').toBe('26px');
    }, 60_000);
  }

  /*
   * 🔴 第七轮 T6：**桌面这条链上此前一次都没采过 `prefers-color-scheme: dark`**。
   *
   * 上面几条问的全是几何（height / margin / padding / position），一个颜色属性都没有，
   * 于是 `assertColorAsksArePinned` 不响，`media` 落在 `MEDIA_DEFAULTS` 的 light 上。
   * 而 `SAMPLED_FACE` 是**按轴**声明的 —— 轴上那个 `dark` 来自移动端的调用点，
   * 读起来却像「裁判在 dark 档上也采过样」。这条腿把那句话变成真的：同一份几何在深色档再量一次。
   *
   * 它不是恒真的摆设 —— 本轮变异实测：往 `index.css` 末尾补一条
   * `@media (prefers-color-scheme: dark){ .main .main-scroll{ margin-top:40px } }`，
   * 本条的「内容区从第 20px 起」在深色档变成 60 而当场红，**本文件其余 8 条全绿**
   * （静态模型那一组看不见它 —— 它不问 `.main-scroll`；win/lin 两条浅色腿也量不到）。
   * 顺带一条反例，写下来免得下次照抄：`@media(dark){ .main-chrome{height:24px} }` 这种写法**打不红本条** ——
   * 它是 (0,1,0)，压不过生产里的 `:root[data-os="win"] .main-chrome` (0,2,0)，浏览器里根本没生效。
   * 钉住「这条腿还在」的判据在 `styles/css-oracle-sampling.test.ts`（「桌面链的 dark 腿还在」，读源码）。
   */
  it('深色档：拖动带几何与浅色档逐值相等（桌面链唯一一次 `prefers-color-scheme: dark` 采样）', async () => {
    // 🔴 这两次**故意不给 `data-theme`**：产品那族条件规则写的是
    // `@media (prefers-color-scheme:dark){ :root:not([data-theme="light"]):not([data-theme="dark"]) … }`，
    // 钉死 `data-theme` 会把整族关掉 ⇒ 深浅两档量到同一个 `color-scheme`，下面那条正向对照恒真
    // （本轮实测：带上 `data-theme:'dark'` 时它当场红，那才是这条对照该有的样子）。
    // 拖动带的几何规则只按 `data-os` 分档，不看 `data-theme`，所以少给它不影响被判的那几个值。
    //
    // 两档都**显式钉住** `prefers-color-scheme`，两次度量都在本 `it` 里逐字写出来 ——
    // 不走上面的 `paint`（它不给 `media`，落在默认的 light 上），也不抽成带参数的小 helper：
    // 抽出去会让 `'dark'` 这个字面量在本文件里出现两次，而 `css-oracle-sampling.test.ts` 那条
    // 「桌面链的 dark 腿还在」判据是按**本文件里的取值字面量**判的 —— 两处写就变成删掉一处也不红。
    const asks = [
      { select: '.side-chrome', props: ['height', 'margin-bottom'] },
      { select: '.main-chrome', props: ['height', 'margin-bottom'] },
      { select: '.main-scroll', props: ['height'] },
      { select: 'html', props: ['color-scheme'] },
    ];
    const light = await measure(
      {
        ctx: 'desktop',
        html: SHELL,
        rootAttrs: { 'data-os': 'win' },
        viewport: { width: 980, height: 740 },
        media: { 'prefers-color-scheme': 'light' },
      },
      asks,
    );
    const dark = await measure(
      {
        ctx: 'desktop',
        html: SHELL,
        rootAttrs: { 'data-os': 'win' },
        viewport: { width: 980, height: 740 },
        media: { 'prefers-color-scheme': 'dark' },
      },
      asks,
    );
    // 正向对照（先做）：两次度量确实落在**两个不同的条件档**上，不是同一次的两个别名。
    // 没有这一条，下面「逐值相等」在「两次其实是同一档」时也全绿 —— 那就是一条恒真的判据。
    expect(
      dark.get('html', 'color-scheme'),
      '深浅两档量到同一个 `color-scheme` —— 这条腿其实没换档，下面的相等断言恒真',
    ).not.toBe(light.get('html', 'color-scheme'));
    for (const sel of ['.side-chrome', '.main-chrome'] as const) {
      expect(dark.get(sel, 'height'), `深色档下 ${sel} 的命中区高度不是 16`).toBe('16px');
      expect(dark.get(sel, 'margin-bottom'), `深色档下 ${sel} 的间隔不是 4`).toBe('4px');
      expect(dark.get(sel, 'height'), `${sel} 的高度随 \`prefers-color-scheme\` 变了`).toBe(
        light.get(sel, 'height'),
      );
      expect(dark.get(sel, 'margin-bottom'), `${sel} 的间隔随 \`prefers-color-scheme\` 变了`).toBe(
        light.get(sel, 'margin-bottom'),
      );
    }
    // 几何等式在深色档同样成立：内容区仍然从第 20px 起。
    expect(
      dark.rect('.main-scroll').y - dark.rect('.main-chrome').y,
      '深色档下内容区不是从第 20px 起',
    ).toBe(20);
  }, 60_000);

  it('macOS：原生标题栏那一档 36px，且**不是** win/lin 那一档（证明 data-os 真的在分档）', async () => {
    const mac = await paint('mac', [{ select: '.main-chrome', props: ['height'] }]);
    expect(mac.get('.main-chrome', 'height'), 'mac 档的标题栏高度不是 36').toBe('36px');
    const win = await paint('win', [{ select: '.main-chrome', props: ['height'] }]);
    expect(win.get('.main-chrome', 'height'), 'win 与 mac 量到同一个值 —— data-os 没在分档').not.toBe(
      mac.get('.main-chrome', 'height'),
    );
  }, 60_000);
});
