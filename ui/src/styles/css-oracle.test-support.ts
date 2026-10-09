/**
 * CSS 裁判 —— 把**浏览器**当层叠的判据，而不是再手写一个层叠。
 *
 * # 为什么有这一层
 *
 * `css-cascade.test-support.ts` 是一个**静态模型**：它自己实现了一部分层叠。四轮验收下来严重度
 * 没降（二轮 3 major → 三轮 2 blocker + 7 major → 四轮 4 blocker + 8 major），而每一轮红的
 * 都是**同一个根因的下一条尾巴**：`:is()` / 属性选择器 / 自定义属性的**值**被覆写 / `@layer` /
 * 构建期注入的 Tailwind / at-rule 嵌进规则块 / 跨任意选择器的特异度……层叠的尾巴是无穷的，
 * 手写实现只能一条一条补，而补的速度赶不上冒出来的速度。
 *
 * 验收员自己用的却是正确的裁判 —— 他们的报告里反复出现「Chrome 152 实测：`.mn-lat.fast` 的
 * color 由 rgb(59,176,121) 变 rgb(239,67,67)」。**判据别重新实现引擎，去问那个引擎。**
 * 同一条教训在本会话另一条线上也成立：Android 专属文件集的判据补了两轮正则不收敛，
 * 第三轮换成问编译器（`cargo --emit=dep-info` 两 target 对差）之后一次判定。
 *
 * # 契约
 *
 * > 输入 = 要渲染的**真实组件 HTML**（或夹具 markup）+ 该上下文的**整条 CSS 链**（按真实进包顺序）
 * > + 目标元素选择器 + 属性名
 * > 输出 = 浏览器算出的 `getComputedStyle` 值。
 *
 * **为什么喂真实组件 HTML 而不是手搓夹具**：这些门本来就在用 `renderToStaticMarkup` 渲染真组件。
 * 把真 DOM 喂进去，顺带把「祖先链到底存不存在」这个问题一并答了 —— 第四轮好几条 blocker 的前提
 * 正是「`.mn-lat` 确实住在 `.mn-row` 里」，那本来就该由真 DOM 说了算，不该由人在评审里断言。
 *
 * 🔴 **真 DOM 必须从屏根一路真到 `<html>`**（2026-09-05 第五轮 B01/B02）。上一版只喂屏组件的
 * `renderToStaticMarkup`，而生产里屏之上还压着
 * `#root > .m-shell > .m-scroll > .m-page`（移动端）/ `#root > .stage > .win > .shell`（桌面）。
 * 于是把变异**往上抬一层**就全绿：`mobile.css` 追加 `.m-page .mn-lat{color:hsl(var(--err))}`
 * 让五个屏的每一条延迟读数在真机上变红，全量 237 文件 / 3957 断言 rc=0；桌面 `#root .main-chrome{height:24px}`
 * 同样全绿。红绿分界线正好落在屏根上 —— 断的不是引擎，是**进料口**。
 * 现在祖先链两段都由代码给：宿主段（`<html lang>` / `<body>` / 挂载点）从产品那份 `.html` 解析
 * （[`hostSkeleton`]），外壳段从产品自己的外壳组件渲染（`styles/mount.test-support.tsx` 的
 * `inMobileShell` / `inDesktopShell`）。两道锁：「喂进去的祖先链与生产一致」有门
 * （`styles/mount-tree.test.tsx`），而「给了 `ctx` 就只收挂载过的 markup」是**类型**
 * （[`MountedMarkup`]）—— 裸字符串编译不过，下一处新调用绕不开。
 *
 * # 零新依赖
 *
 *  · 浏览器 = 系统 `google-chrome`（本机 Chrome 152），**不装 puppeteer / playwright 的浏览器**；
 *  · 传输 = Node 原生 `fetch` / `WebSocket`（Node 26 全局可用）+ CDP，**不装 CDP 客户端**；
 *  · CSS 打包 = 仓内既有 devDependency `postcss` + 产品 `postcss.config.js` 里声明的那几个插件。
 *    **插件表是从那份配置读出来的**（[`productPostcssPlugins`]），本模块只登记「这个名字怎么变成
 *    插件实例」（[`PLUGIN_LOADERS`]）；两张表的键集必须逐个相等，对不上就抛。
 *    🔴 第六轮 A5：上一版这里写着「跑的是产品自己那条管线」，实际是
 *    `postcss([tailwindcss({optimize:false})])` 一张**硬编表** —— 实测把 `'@tailwindcss/postcss': {}`
 *    从 `ui/postcss.config.js` 删掉（真实构建的 CSS 因此完全变形），全量 rc=0 全绿。
 *    一条不受判据管辖的声明，比没有这句话更坏。
 *    所以 `@import 'tailwindcss'` 与全部相对 `@import` 都按真实构建展开 —— 静态模型第四轮 R5-08
 *    那条「取材面缺了实际进包的 Tailwind 且缺失是静默的」在这里不成立：本模块吃的就是构建产物本身。
 *
 * # 故障关闭
 *
 *  · **Chrome 缺席 ⇒ 模块加载期抛，不 skip**。「没检查」与「检查通过」的输出必须可区分
 *    （仓内先例：`lib/prototype-confirm-parity.test.ts` 文件缺席时模块加载期 throw）。
 *  · 目标选择器在真 DOM 上**一个元素都没命中 ⇒ 抛**（空取材面上的否定断言恒真）。
 *  · CSS 链里任一份**构建不出来、或产物里还剩 `@import` / `@theme` / `@utility` 这类构建期构造
 *    ⇒ 抛**（说明管线没真跑，喂进浏览器的不是产品那一份）。
 *  · **postcss 插件表 != 产品配置里的插件表 ⇒ 抛**（第六轮 A5）。
 *  · 宿主 `.html` 里出现**外链样式表** ⇒ 抛（本模块解析不了它，静默跳过 = 取材面缺一块）。
 *    扫描面是**整份文档**，不只 `<head>`：`<body>` 段是原样注进去的，写在那里的 `<link>` 一样进层叠（第七轮 T4）。
 *  · 一个测试文件**一个浏览器实例**：`measure()` 惰性开、`closeOracle()` 收尾。收尾走 CDP 的
 *    `Browser.close` 让它自己退 —— 本模块任何路径上都**不 kill 进程**（宿主上跑着用户自己的
 *    Chrome，碰坏它是不可接受的事故），`--remote-debugging-port=0` + **等 `DevToolsActivePort`
 *    两行写全**再读（第六轮 A2：Chrome 分两行写，读到一半 ⇒ `Invalid URL` ⇒ 那个浏览器成孤儿），
 *    不写死端口号撞车。
 *  · 🔴 **收尾覆盖每一条路径**（第六轮 A1）：`closeOracle` 先把**在途的 `measure`** 等回来
 *    （上一版只等「启动在途」，而生产门走的 `ctx` 路径在 `await cssChain()` 里还没走到启动
 *    ⇒ 收尾直接返回、浏览器随后才起来 ⇒ 实测宿主留 12 个 chrome + profile 而 vitest rc=0、
 *    一个字都不打），再请退、再 [`assertNoOracleLeak`] 自查并点名。
 *    收尾 hook 由**本模块自己**在 import 期 `afterAll` 注册 ⇒ 调用方忘了写也覆盖得到。
 *  · 请退**不依赖手里那条 ws**：ws 建不起来时按 profile 里的端口**重连一次**再请退
 *    （第六轮 A2：上一版 `abandon` 在 `ws === undefined` 时直接放弃，浏览器就成了自己回收不了的孤儿）。
 *  · 选择器命中**多个**元素而调用方没表态 ⇒ 抛。「只读第一个」是一句要写出来的话（第五轮 M02）。
 *  · TS 侧的 import 说明符解析不到 ⇒ 抛；裸说明符里带样式后缀的到 `node_modules` 解析并进链；
 *    其余裸说明符记进 `externals` 自曝（第五轮 M01）。
 *  · 🔴 **没有显式钉住 `prefers-color-scheme` 就不许问颜色 ⇒ 抛**（第六轮 A3：上一版给
 *    `setEmulatedMedia` 传空 features = 清空覆写 ⇒ 同一个 commit 在深色系统与浅色系统上得出
 *    不同结论，实测同一条变异的两半一红一绿）。默认值见 [`MEDIA_DEFAULTS`]。
 *
 * # 🔴 采样面：本裁判**只在这些条件档上采样**（一句声明；两段自检的强度见本节末尾）
 *
 * 视口 × 方向 × 主题 × `data-*` × `prefers-*` 是一个**组合面**，追它没有终点。本模块不假装覆盖它，
 * 而是把采到的那几个点写成一张**明写出来**的表（[`SAMPLED_FACE`]）。
 * 🔴 不写「受判据管辖」——上一版这么写，而两段自检各有一个洞（见下）。采样的**轴**就这五条：
 *
 *  1. **视口**（`Emulation.setDeviceMetricsOverride` 的宽×高@dpr）；
 *  2. **书写方向 `dir`**（来自宿主 `.html` 的 `<html dir>`；调用点今天一处都没覆写它）；
 *  3. **主题 `data-theme`**（调用点在 `<html>` 上给）；
 *  4. **其它 `data-*`**（今天只有 `data-os`）；
 *  5. **`prefers-*` / `forced-colors`**（`Emulation.setEmulatedMedia`，默认表 [`MEDIA_DEFAULTS`]）。
 *
 * 🔴 **每条轴上到底采了哪几个取值，在 [`SAMPLED_FACE`] 里，不在这段话里** —— 一份写在注释里的
 * 取值副本会漂，而漂了没人会红，那正是本仓判过死刑的「不受判据管辖的声明」。
 *
 * 那张表两个方向各有一段自检，**两段的强度必须写出来**（第七轮 T1 / T2）：
 *  · **实际 ⊆ 声明**：`measureOnce` 发命令之前过一遍 [`assertPointDeclared`]。这是一条**运行期自检**，
 *    **它自己没有判据** —— 函数本身有正反对照（`css-oracle-sampling.test.ts`「有牙」那条），
 *    但「`measureOnce` 真的调用了它」这条**接线**没人守：把那一行删掉，全量照样全绿。
 *  · **声明 ⊆ 实际**：`css-oracle-sampling.test.ts` 判的是**调用点源码里有没有那个字面量**，
 *    不是运行期有没有真采过。采样者被 `skip` 掉、或那条 `measure(...)` 断言被注掉一半，
 *    只要 `viewport: {…}` / `media: {…}` 这几行还在源码里，声明照旧全绿。
 *
 * 所以这里**不写**「两个方向都有门」—— 上一版就是这么写的，而那两扇门之间的缝正好是生产路径。
 * 两条都进了下面那张「抓不到的形态」表。
 *
 * 🔴 这张表是**按轴**的，不是按（链 × 轴）的：它答不出「桌面那条链采过 dark 吗」。
 * 今天桌面链确实采了 `prefers-color-scheme` 的两个取值（`window-drag-region.test.ts` 的 dark 腿，
 * 第七轮 T6 补的），钉住这一点的判据只有一条、且读的是那个文件的**源码**
 * （`css-oracle-sampling.test.ts`「桌面链的 dark 腿还在」）。其余（链 × 轴）格子没有判据。
 *
 * 采样面之外的条件档 **今天没有判据**（不是「通过了」）。哪些形态谁都抓不到 ⇒ 见下面那张表。
 *
 * # 🔴 本裁判**抓不到**的形态（每条都写清谁来守；没人守就写没人守）
 *
 * | 形态 | 谁来守 |
 * |------|--------|
 * | 只在**未采样条件档**下才出现的覆写（`@media (max-width:380px)`、`dir=rtl`、`data-*` 的其它取值、`prefers-contrast` …） | **没人守**。静态模型按定义只看无条件档；本裁判只看 [`SAMPLED_FACE`] 那几个点。两者的**并集**才是覆盖面，并集之外今天是空的。 |
 * | 「实际 ⊆ 声明」那条**接线**：`measureOnce` 里的 `assertPointDeclared(point)` 这一行 | **没人守**（第七轮 T1）。函数自己有正反对照，接线没有 —— 本轮实测把那一行注掉，全量 239 文件 / 4019 条照样 rc=0 全绿，没有任何东西会说话。处置是把这句话说窄，不是再给它造一层门。 |
 * | 「声明 ⊆ 实际」那条门的**判定对象**：它读的是调用点**源码里的字面量**，不是运行期真采过 | **没人守**（第七轮 T2）。本轮实测把桌面那整个 `describe` 改成 `describe.skip`（一次度量都不发），`css-oracle-sampling.test.ts` 9/9 照旧全绿 —— 只要那几行对象字面量还留在源码里。 |
 * | （链 × 轴）这一格：**某条链**在某条轴上到底采了哪几个取值 | 只有一条：「桌面链的 dark 腿还在」（读源码，第七轮 T6）。[`SAMPLED_FACE`] 本身是按轴声明的，其余格子**没人守**。 |
 * | 构建期注入、而 **TS/CSS 模块图看不见**的 CSS（运行期 `<link>`、宿主 `.html` 之外注入的 `<style>`、平台侧注入） | 宿主 `.html` 里的 `<head><style>` 由 [`hostSkeleton`] 接住并进链（第六轮 A4）；`<link rel=stylesheet>` 出现在宿主 `.html` 的**任何位置**（`<head>` 与 `<body>` 都算）⇒ **抛**（第七轮 T4 把扫描面从 `<head>` 扩到整份文档）。**其余没人守** —— 运行期注入的样式本模块看不见。 |
 * | 运行期 **JS 改的内联样式 / class**（`useEffect`、事件处理器、动画帧） | **没人守**。本模块喂的是 `renderToStaticMarkup` 的静态 HTML，JS 不跑。组件级行为断言（各屏 `*.test.tsx` 的非 CSS 组）守的是 props→markup，不是 CSS 结果。 |
 * | 交互态 `:hover` / `:focus` / `:active` / `:focus-visible` | **没人守**。CDP 有 `CSS.forcePseudoState`，本模块没接。 |
 * | 动画/过渡的**中间帧**、`@keyframes` 的实际轨迹 | **没人守**。读的是 `getComputedStyle` 的静态一帧。 |
 * | 真机引擎差异（Android WebView / WKWebView 与 Chrome 152） | **没人守**，只有真机能答（见「明确不保证」）。 |
 * | 前缀属性本身（`-webkit-*`）算出什么 | autoprefixer 现在**真的在链上跑**（第六轮 A5），但断言问的都是无前缀属性名 ⇒ 前缀分支**没有判据**。 |
 *
 * 🔴 「没人守」是一句要写出来的话。上一轮出过一次把一整类推给一个**够不着它的**守卫的 caveat ——
 * 那比不写更坏，因为它让读的人以为有人看着。
 *
 * # 明确不保证
 *
 *  · **不跑 JS**：喂进去的是 `renderToStaticMarkup` 的静态 HTML，`useEffect` / 事件不会发生。
 *    要测的是层叠，不是运行时。
 *  · **不跑压缩**：`@tailwindcss/postcss` 的 `optimize` 由 `NODE_ENV` 决定，测试进程里是 `test` ⇒ 关。
 *    真 `vite build` 里它是开的（lightningcss 降级/压缩）。这是一条**收窄**，不是保证。
 *  · **不是真机**：本机 Chrome 152 与 Android WebView / macOS WKWebView 不是同一个引擎。
 *    本模块把「层叠算错」这一类从门里除掉，剩下的引擎差异仍然只有真机能答。
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve as resolvePath } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { afterAll } from 'vitest';
import postcss, { type AcceptedPlugin } from 'postcss';
import tailwindcss from '@tailwindcss/postcss';
import autoprefixer from 'autoprefixer';
import { DISK_FS, ENTRY_MODULE, stripComments, type CtxName, type SourceFs } from './css-cascade.test-support';

// ════════════════════════════════════════════════════════════════════════════
// ① Chrome 定位 —— 缺席即抛（模块加载期）
// ════════════════════════════════════════════════════════════════════════════

/** 按顺序探这些路径；一个都没有就抛。**没有环境变量后门** —— 后门等于一个静默的 skip。 */
export const CHROME_CANDIDATES: readonly string[] = [
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
];

/** 纯函数形态，便于对「缺席 ⇒ 抛」这条本身做正向对照（见 `css-oracle.test.ts` ⑥）。 */
export function resolveChromeBinary(candidates: readonly string[] = CHROME_CANDIDATES): string {
  for (const c of candidates) if (existsSync(c) && statSync(c).isFile()) return c;
  throw new Error(
    'CSS 裁判找不到浏览器 —— 探过：\n' +
      candidates.map((c) => `    ${c}`).join('\n') +
      '\n本模块**不 skip**：「没检查」与「检查通过」的输出必须可区分。装一个 Chrome，或改这份候选表。',
  );
}

/** 模块加载期就抛：import 得到本模块 ⇒ 浏览器一定在。 */
const CHROME_BIN = resolveChromeBinary();

// ════════════════════════════════════════════════════════════════════════════
// ② CSS 链 —— 跑产品自己那条 postcss 管线，喂的是构建产物
// ════════════════════════════════════════════════════════════════════════════

/** …/ui/ */
const UI_ROOT = fileURLToPath(new URL('../../', import.meta.url));
const abs = (relToUi: string) => resolvePath(UI_ROOT, relToUi);
const rel = (absPath: string) => relative(UI_ROOT, absPath).split('\\').join('/');

const TS_SPEC = /\bfrom\s*['"]([^'"]+)['"]|\bimport\s*\(\s*['"]([^'"]+)['"]|^[ \t]*import\s+['"]([^'"]+)['"]/gm;

/** 相对 / `@/` 说明符 ⇒ 文件路径。裸说明符（npm 包）返回 `null`，由调用方分流。 */
const resolveLocal = (spec: string, importerAbs: string, fs: SourceFs): string | null => {
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
    `${base}.js`,
    `${base}.jsx`,
    `${base}.mjs`,
    `${base}.css`,
    `${base}.json`,
    `${base}/index.ts`,
    `${base}/index.tsx`,
  ])
    if (fs.isFile(cand)) return cand;
  return null;
};

const isBareSpec = (spec: string) => !spec.startsWith('.') && !spec.startsWith('@/') && !spec.startsWith('/');
/** 样式文件后缀 —— 裸说明符带这些后缀 ⇒ 它是**真的进包的一块 CSS**，必须解析出来。 */
const STYLE_EXT = /\.(?:css|scss|sass|less|styl|pcss)$/i;

/**
 * 一个层叠上下文的**打包单元**清单，按真实进包顺序。
 *
 * 与 `css-cascade.test-support.ts` 的 [`cssOrder`] 是**两种切法，同一个事实**：
 *  · `cssOrder` 连 CSS 内部的 `@import` 一起展开成文件清单，因为静态模型要逐文件解析；
 *  · 本函数**只走 TS/TSX 模块图**，CSS 内部的 `@import` 留给 postcss 管线原样展开 ——
 *    产品构建里就是这么做的（Vite 把每个被 JS import 的 `.css` 当一个单元交给 postcss）。
 *    这也是 `@import 'tailwindcss'` 唯一能被正确处理的形态：它不是一个文件路径。
 */
export function cssBundleUnits(entryModule: string): string[] {
  return cssBundleScan(entryModule).units;
}

/**
 * [`cssBundleUnits`] + **被跳过的裸说明符清单**。
 *
 * 🔴 2026-09-05 第五轮 M01：上一版对解析不到的 TS 说明符是**静默丢弃**（`if (next !== null) walk(next)`），
 * 而本文件头注把 R5-08「取材面缺一块且缺得无声无息」声明为「在这里不成立」。实测它成立：
 * 给 `MobileMain.tsx` 加一行 `import 'tailwindcss/preflight.css';`，真 `vite build` 的 mobile chunk
 * 从 59.66 kB 涨到 64.19 kB（产物里 grep 得到 `text-size-adjust`），而裁判的链里一条都没有 ——
 * R5-08 补的是 CSS 侧的 `@import`，**TS 侧的说明符是同一个洞的另一半**。
 *
 * 现在三条分流，一条都不静默：
 *  · 相对 / `@/` 说明符解析不到 ⇒ **抛**（那是坏 import，构建本来就过不去）；
 *  · 裸说明符带样式后缀（`tailwindcss/preflight.css`）⇒ 到 `ui/node_modules` 里解析并**当作进包单元**；
 *    解析不到 ⇒ 抛；
 *  · 其余裸说明符（npm 的 JS 包）⇒ 记进 `externals` **自曝**，由 `css-oracle.test.ts` ⑤ 逐条钉住 ——
 *    新添一个运行时依赖就会红一次，逼人回答「它随包发 CSS 吗」。
 */
export function cssBundleScan(
  entryModule: string,
  fs: SourceFs = DISK_FS,
): { units: string[]; externals: string[] } {
  const done = new Set<string>();
  const visiting = new Set<string>();
  const out: string[] = [];
  const externals = new Set<string>();
  const walk = (fileAbs: string): void => {
    if (done.has(fileAbs) || visiting.has(fileAbs)) return;
    visiting.add(fileAbs);
    if (/\.tsx?$/.test(fileAbs)) {
      const body = stripComments(fs.read(fileAbs));
      for (const m of body.matchAll(TS_SPEC)) {
        const spec = m[1] ?? m[2] ?? m[3];
        if (!isBareSpec(spec)) {
          const next = resolveLocal(spec, fileAbs, fs);
          if (next === null)
            throw new Error(
              `${rel(fileAbs)} 的 \`import '${spec}'\` 解析不到文件 —— 静默跳过它就等于取材面缺一块` +
                `且缺得无声无息（R5-08 的形状）。修好这条 import，或把它变成可解析的路径。`,
            );
          walk(next);
          continue;
        }
        if (STYLE_EXT.test(spec)) {
          const inPkg = resolvePath(UI_ROOT, 'node_modules', spec);
          if (!fs.isFile(inPkg))
            throw new Error(
              `${rel(fileAbs)} 的 \`import '${spec}'\` 是一块**真的进包的 CSS**，而 ` +
                `\`node_modules/${spec}\` 解析不到 —— 本模块不静默跳过它。`,
            );
          walk(inPkg);
          continue;
        }
        externals.add(spec);
      }
    }
    visiting.delete(fileAbs);
    done.add(fileAbs);
    if (/\.(?:css|scss|sass|less|styl|pcss)$/i.test(fileAbs)) out.push(rel(fileAbs));
  };
  const entryAbs = abs(entryModule);
  if (!fs.isFile(entryAbs)) throw new Error(`入口模块解析不到：${entryModule}`);
  walk(entryAbs);
  if (out.length === 0) throw new Error(`入口 \`${entryModule}\` 一份 CSS 都没进包 —— 空取材面。`);
  return { units: out, externals: [...externals].sort() };
}

// ── postcss 管线：插件表**从产品那份配置读**（第六轮 A5）─────────────────────

/** 产品那份 postcss 配置（相对 `ui/`）。 */
export const POSTCSS_CONFIG_FILE = 'postcss.config.js';

/**
 * 本模块知道**怎么把一个插件名变成插件实例**的登记表。
 *
 * 🔴 它不是「要跑哪几个插件」的真值源 —— 那个真值源是 `ui/postcss.config.js`，由
 * [`productPostcssPlugins`] 读。本表只回答「这个名字怎么加载」。两张表的键集**必须逐个相等**：
 *  · 配置里多一个 ⇒ 本模块跑的管线比产品**少**一步（第六轮 A5 的形状：删掉
 *    `'@tailwindcss/postcss': {}` 后真实构建的 CSS 完全变形，而全量 rc=0 全绿）；
 *  · 本表里多一个 ⇒ 本模块跑的比产品**多**一步，喂进浏览器的同样不是产品那一份。
 * 判据是 [`assertPluginRegistryMatchesConfig`]，在 [`buildCssUnit`] 的生产路径上每次都过。
 */
export const PLUGIN_LOADERS: Readonly<Record<string, (options: unknown) => AcceptedPlugin>> = {
  '@tailwindcss/postcss': (options) => tailwindcss(options as Parameters<typeof tailwindcss>[0]),
  autoprefixer: (options) => autoprefixer(options as Parameters<typeof autoprefixer>[0]),
};

export interface DeclaredPlugin {
  name: string;
  options: unknown;
}

let CONFIG_PLUGINS: Promise<DeclaredPlugin[]> | null = null;

/**
 * 产品 `postcss.config.js` 里声明的插件，**按声明序**。
 *
 * 用运行期 `import()` 真读那份文件（它就是一份 ESM 模块），不写第二个解析器 ——
 * 「别自己再实现一遍，去问那个引擎」在配置这一层同样成立。
 */
export function productPostcssPlugins(): Promise<DeclaredPlugin[]> {
  CONFIG_PLUGINS ??= (async (): Promise<DeclaredPlugin[]> => {
    const href = pathToFileURL(abs(POSTCSS_CONFIG_FILE)).href;
    return declaredPluginsOf(await import(/* @vite-ignore */ href));
  })();
  return CONFIG_PLUGINS;
}

/** 一份 postcss 配置模块 ⇒ 它声明的插件。读不出 `plugins` ⇒ 抛（纯函数，故这条分支自己有门）。 */
export function declaredPluginsOf(mod: unknown): DeclaredPlugin[] {
  const plugins = (mod as { default?: { plugins?: unknown } } | null)?.default?.plugins;
  if (plugins === null || plugins === undefined || typeof plugins !== 'object')
    throw new Error(
      `\`${POSTCSS_CONFIG_FILE}\` 里读不到 \`plugins\` —— 产品那条管线跑的是哪几个插件，本模块不猜。`,
    );
  return Object.entries(plugins as Record<string, unknown>).map(([name, options]) => ({ name, options }));
}

/** 配置表 == 登记表（**逐个相等，含顺序**），对不上就抛。 */
export function assertPluginRegistryMatchesConfig(
  configNames: readonly string[],
  registryNames: readonly string[] = Object.keys(PLUGIN_LOADERS),
): void {
  const missing = configNames.filter((n) => !registryNames.includes(n));
  const extra = registryNames.filter((n) => !configNames.includes(n));
  if (missing.length === 0 && extra.length === 0) return;
  throw new Error(
    `本模块跑的 postcss 插件表与 \`${POSTCSS_CONFIG_FILE}\` 对不上：\n` +
      (missing.length > 0 ? `    配置里有、本模块不会加载：${missing.join('、')}\n` : '') +
      (extra.length > 0 ? `    本模块要跑、配置里没有：${extra.join('、')}\n` : '') +
      '喂进浏览器的就不是产品那一份 CSS 了（第六轮 A5：上一版是一张硬编表，' +
      '把 `@tailwindcss/postcss` 从配置里删掉全量照样 rc=0 全绿）。',
  );
}

/** 构建期构造：出现在产物里 ⇒ 管线没真跑，喂进浏览器的不是产品那一份。 */
const BUILD_TIME_AT_RULE = /@(?:import|tailwind|apply|theme|utility|source|plugin|config|variant|custom-variant)\b/;

const BUILT = new Map<string, string>();

/**
 * 跑产品那条 postcss 管线，产出该单元的**最终 CSS**。
 *
 * `fs` 可注入 —— 下面那两条故障关闭分支（产物里还剩构建期构造 / 构建出来是空的）因此**自己有门**
 * （`css-oracle.test.ts` ⑨；第六轮 A6：上一版这两条一改成永不触发，全量照样全绿）。
 * 注入了自定义 `fs` 就不走缓存，免得合成输入污染真链的缓存。
 */
export async function buildCssUnit(fileRelToUi: string, fs: SourceFs = DISK_FS): Promise<string> {
  const cacheable = fs === DISK_FS;
  if (cacheable) {
    const hit = BUILT.get(fileRelToUi);
    if (hit !== undefined) return hit;
  }
  const from = abs(fileRelToUi);
  const src = fs.read(from);
  const declared = await productPostcssPlugins();
  assertPluginRegistryMatchesConfig(declared.map((d) => d.name));
  const res = await postcss(declared.map((d) => PLUGIN_LOADERS[d.name](d.options))).process(src, { from });
  const css = res.css;
  // 判据取材面**先剥注释**：本仓的 CSS 头注里满是「本文件一个 `@import` 都不写」这类引文，
  // 不剥就会把注释当成真实构造报红（判据被自己的取材面污染，2026-09-05 写这一行时当场撞到）。
  const left = BUILD_TIME_AT_RULE.exec(stripComments(css));
  if (left !== null) {
    throw new Error(
      `\`${fileRelToUi}\` 过完 postcss 管线后产物里还剩构建期构造 \`${left[0]}\` —— ` +
        '管线没真跑，喂进浏览器的不是产品那一份。（R5-08 的形状：`@import \'tailwindcss\'` ' +
        '在静态模型里被静默丢掉，取材面缺了实际进包的那一大块。）',
    );
  }
  if (css.trim().length === 0) throw new Error(`\`${fileRelToUi}\` 构建出来是空的 —— 空取材面。`);
  if (cacheable) BUILT.set(fileRelToUi, css);
  return css;
}

// ════════════════════════════════════════════════════════════════════════════
// ②-b 宿主文档 —— 挂载点之上的祖先链由产品那份 `.html` 给，不由人手写
// ════════════════════════════════════════════════════════════════════════════

/** 四个层叠上下文各自的宿主文档（与 `vite.config.ts` 的 `rollupOptions.input` 逐条对得上）。 */
export const HOST_DOCUMENT: Readonly<Record<CtxName, string>> = {
  desktop: 'index.html',
  mobile: 'mobile.html',
  tray: 'tray.html',
  popup: 'update-popup.html',
};

export interface HostSkeleton {
  /** 宿主文档 `<html>` 上的属性（`lang` / `dir`）。调用方的 `rootAttrs` 覆盖同名项。 */
  rootAttrs: Readonly<Record<string, string>>;
  /** 宿主文档 `<body>` 上的属性。 */
  bodyAttrs: Readonly<Record<string, string>>;
  /**
   * 宿主文档 `<head>` 里那几段 `<style>` 的**原文**，按文档序。
   *
   * 🔴 2026-09-05 第六轮 A4：上一版读了同一份 `.html` 的 `html`/`body` 属性与挂载点，**却跳过了
   * 同一个文件里的 `<style>`**。那两段不是装饰 —— `mobile.html` / `index.html` 的 `<head><style>`
   * 是首帧主题兜底（`html:not([data-theme]){ background: … }`），在真实构建产物里它排在打包 CSS 的
   * `<link>` **之前**（`dist/mobile.html` 实测：`<style>` 在第 21 行、`<link rel=stylesheet>` 在第 32 行），
   * 也就是说它是这条层叠链的**第一段**。跳过它 ⇒ 往那里加一条规则就能改掉屏上的颜色而门全绿，
   * 与已判过死刑的 B01 逐字同形（「进料口漏了一段真实存在的祖先/样式」）。
   */
  headStyles: string[];
  /** 挂载元素的标签名与 id（`<div id="root">` / `<div id="tray-root">` …）。 */
  mountTag: string;
  mountId: string;
  /** 挂载点之前 / 之后的 `<body>` 原文（含挂载元素自己的开闭标签）。 */
  before: string;
  after: string;
}

const ATTR_PAIR = /([a-zA-Z_:][-\w:.]*)\s*=\s*"([^"]*)"/g;
const parseAttrs = (raw: string): Record<string, string> => {
  const out: Record<string, string> = {};
  for (const m of raw.matchAll(ATTR_PAIR)) out[m[1]] = m[2];
  return out;
};

const HOSTS = new Map<CtxName, HostSkeleton>();

/**
 * 产品那份 `.html` 的骨架：`<html>` 属性 + `<body>` 属性 + **挂载点**（入口脚本 mount 进去的那个元素）。
 *
 * 判据：`<body>` 里剥掉注释与 `<script>` 之后，**恰好一个**带 `id` 的空元素 —— 那就是挂载点。
 * 多于一个或一个都没有 ⇒ 抛（猜哪个是挂载点等于在评审里断言祖先链）。
 * 再与入口模块对拍一次 `getElementById('<id>')`：宿主文档与入口脚本说的必须是同一个挂载点。
 */
export function hostSkeleton(ctx: CtxName, fs: SourceFs = DISK_FS): HostSkeleton {
  const cacheable = fs === DISK_FS;
  if (cacheable) {
    const hit = HOSTS.get(ctx);
    if (hit !== undefined) return hit;
  }
  const file = HOST_DOCUMENT[ctx];
  const src = fs.read(abs(file));
  const html = /<html([^>]*)>/i.exec(src);
  const body = /<body([^>]*)>([\s\S]*)<\/body>/i.exec(src);
  const head = /<head([^>]*)>([\s\S]*?)<\/head>/i.exec(src);
  if (html === null || body === null || head === null)
    throw new Error(`\`${file}\` 里找不到 \`<html>\` / \`<head>\` / \`<body>\`。`);
  // 注释先剥掉：本仓这两份 `.html` 的 `<head>` 注释里逐字引着 `<style>` / `<script>`，
  // 不剥就会把注释里的引文当成真的样式段取进来（判据被自己的取材面污染）。
  const headInner = head[2].replace(/<!--[\s\S]*?-->/g, '');
  // 🔴 外链样式表的扫描面 = **整份文档**，不只 `<head>`（第七轮 T4）。上一版只扫 `<head>`，
  //    于是写在宿主 `<body>` 里的 `<link rel=stylesheet>` 静默放行：浏览器照样把它算进层叠，
  //    而本模块的链里没有它 ⇒ 取材面缺一块且缺得无声无息（R5-08 的形状）。
  //    `<body>` 段是**原样**注进文档的（见 [`buildDocument`] 的 `before` / `after`），所以这条不是理论。
  if (/<link\b[^>]*\brel\s*=\s*["']?stylesheet/i.test(src.replace(/<!--[\s\S]*?-->/g, '')))
    throw new Error(
      `\`${file}\` 里有**外链样式表** —— 本模块解析不了它，而静默跳过就等于` +
        '取材面缺一块且缺得无声无息（R5-08 的形状）。把它接进链里，或改用 `@import`。',
    );
  const headStyles = [...headInner.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/gi)].map((m) => m[1]);
  const inner = body[2].replace(/<!--[\s\S]*?-->/g, '').replace(/<script\b[\s\S]*?<\/script>/gi, '');
  const mounts = [...inner.matchAll(/<([a-zA-Z][\w-]*)([^>]*\bid="([^"]+)"[^>]*)>\s*<\/\1>/g)];
  if (mounts.length !== 1)
    throw new Error(
      `\`${file}\` 的 \`<body>\` 里带 id 的空元素有 ${mounts.length} 个，不是 1 个 —— ` +
        '挂载点是谁不能靠猜（猜错 = 祖先链又变回人写的）。',
    );
  const m = mounts[0];
  const entrySrc = fs.read(abs(ENTRY_MODULE[ctx]));
  if (!entrySrc.includes(`getElementById('${m[3]}')`))
    throw new Error(
      `\`${file}\` 的挂载点是 \`#${m[3]}\`，而入口 \`${ENTRY_MODULE[ctx]}\` 里没有 ` +
        `\`getElementById('${m[3]}')\` —— 宿主文档与入口脚本对不上，喂进裁判的祖先链就不是生产那条。`,
    );
  const skeleton: HostSkeleton = {
    rootAttrs: parseAttrs(html[1]),
    bodyAttrs: parseAttrs(body[1]),
    headStyles,
    mountTag: m[1],
    mountId: m[3],
    before: inner.slice(0, m.index) + `<${m[1]}${m[2]}>`,
    after: `</${m[1]}>` + inner.slice(m.index + m[0].length),
  };
  if (cacheable) HOSTS.set(ctx, skeleton);
  return skeleton;
}

/** 一个层叠上下文的整条 CSS 链（已构建），按真实进包顺序。 */
export async function cssChain(ctx: CtxName): Promise<{ file: string; css: string }[]> {
  const units = cssBundleUnits(ENTRY_MODULE[ctx]);
  const out: { file: string; css: string }[] = [];
  for (const f of units) out.push({ file: f, css: await buildCssUnit(f) });
  return out;
}

// ════════════════════════════════════════════════════════════════════════════
// ③ CDP 客户端 —— 原生 fetch / WebSocket，一个测试文件一个实例
// ════════════════════════════════════════════════════════════════════════════

interface Conn {
  child: ChildProcess;
  ws: WebSocket;
  dir: string;
  send: (method: string, params?: Record<string, unknown>, sessionId?: string) => Promise<Record<string, unknown>>;
}

let CONN: Conn | null = null;
let OPENING: Promise<Conn> | null = null;

/**
 * **在途的 `measure`**。收尾必须把它们等回来，否则「浏览器在收尾之后才起来」这一档没人管。
 *
 * 🔴 2026-09-05 第六轮 A1：上一版收尾只等「启动在途」（`OPENING`）。而生产门走的是 `ctx` 路径 ——
 * `measure` 先 `await cssChain(ctx)`（要跑 postcss，几百毫秒起步）**才**走到 `conn()`，
 * 于是收尾时 `CONN` 与 `OPENING` **都还是 null**，`closeOracle` 原样返回、`assertNoOracleLeak`
 * 对着一张空的 `OWNED` 说「干净」。浏览器随后才起来，配 `child.unref()` 就成了 ppid=1 的孤儿。
 * 实测（生产门路径，`void measure({ctx:'mobile', html: inMobileShell(…)}, …)` + 常规
 * `afterAll(closeOracle)`）：vitest **rc=0**，宿主留 **12 个 chrome** + 1 个 profile，**一个字都不打**。
 * 上一轮那条 M03 的门用的是内存链（`css` 路径），它同一个 tick 就走到 `conn()` ⇒ `OPENING` 有值 ⇒
 * 那条门是绿的 —— **检漏器只在某些路径上生效**，而五道生产门走的正是不生效的那条。
 */
const INFLIGHT = new Set<Promise<void>>();

/** 登记一次度量：无论成败都从簿上销号（异常由调用方自己接，这里只吞给簿用的那一份）。 */
function track<T>(p: Promise<T>): Promise<T> {
  const settled = p.then(
    () => undefined,
    () => undefined,
  );
  INFLIGHT.add(settled);
  void settled.then(() => INFLIGHT.delete(settled));
  return p;
}

/** 还有几次度量在途 —— 让「收尾把在途等回来了」这条自己可断言（`css-oracle.test.ts` ⑨）。 */
export const inflightMeasures = (): number => INFLIGHT.size;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ── 进程与 profile 的收尾自查 ───────────────────────────────────────────────
//
// 🔴 2026-09-05 第五轮 M03：上一轮全量跑完，宿主上留下 **14 个 ppid=1 的孤儿 headless Chrome**
// 与两个 53M profile。按设计任何路径都不 kill ⇒ 无人回收，而且**没有任何输出说漏了**。
// 确定性复现：`measure(...)` 不 await → 立刻 `await closeOracle()` → 再 await 那次 measure ——
// 收尾时 `CONN` 还是 null、`OPENING` 在途，收尾直接返回，浏览器随后才起来，配 `child.unref()`
// 就成了收不回来的孤儿。三件一起补：① 收尾覆盖「启动在途」；② `launch` 抛出的路径也清 profile；
// ③ **留下了就自曝** —— 收尾时按本次运行**自己启动的那几个**逐个查活性与 profile，
// 没清干净就抛并点名。「悄悄漏一个」与「清干净了」的输出因此可区分。

export interface Owned {
  pid: number | undefined;
  dir: string;
  closed: boolean;
}
const OWNED: Owned[] = [];

/**
 * 这个 profile 目录底下**还活着几个进程**（返回它们的 pid，按 pid 序）。
 *
 * 判据是 `/proc/<pid>/cmdline` 里带着我们那个 `--user-data-dir` —— 只读 `/proc`，**不发任何信号**
 * （宿主上跑着用户自己的 Chrome，本模块任何路径都不 kill），顺带避开 pid 复用的误报，
 * 也**不用会自匹配的 `ps | grep`**。
 *
 * 🔴 第六轮：上一版只查 `owned.pid` 那**一个**根进程。一个 headless Chrome 实测是 **12 个进程**
 * （browser / zygote / gpu / network / renderer …），根退了子进程一般跟着退，但「根退了而子进程还在」
 * 这一档上一版是瞎的。现在按 profile 扫全树 —— 报出来的数就是宿主上真的还剩几个。
 */
function pidsUnder(dir: string): number[] {
  if (!existsSync('/proc')) return [];
  const out: number[] = [];
  for (const entry of readdirSync('/proc')) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      if (readFileSync(`/proc/${entry}/cmdline`, 'utf8').includes(dir)) out.push(Number(entry));
    } catch {
      /* 进程刚退 / 没权限 —— 都算不在 */
    }
  }
  return out.sort((a, b) => a - b);
}

/** 没有 `/proc` 的平台退化成存在性探针（信号 0 不投递，只做权限/存在检查）。 */
function stillRunning(o: Owned): boolean {
  if (existsSync('/proc')) return pidsUnder(o.dir).length > 0;
  if (o.pid === undefined) return false;
  try {
    process.kill(o.pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** 本次运行启动过的浏览器 / 还没清掉的 profile。`liveBrowsers` 是**逐进程**的，不是逐实例的。 */
export function oracleHygiene(): {
  started: number;
  liveBrowsers: { pid: number | undefined; dir: string }[];
  leftProfiles: string[];
} {
  const live: { pid: number | undefined; dir: string }[] = [];
  for (const o of OWNED) {
    if (existsSync('/proc')) for (const pid of pidsUnder(o.dir)) live.push({ pid, dir: o.dir });
    else if (stillRunning(o)) live.push({ pid: o.pid, dir: o.dir });
  }
  return {
    started: OWNED.length,
    liveBrowsers: live,
    leftProfiles: OWNED.filter((o) => existsSync(o.dir)).map((o) => o.dir),
  };
}

/**
 * 没清干净 ⇒ 抛并点名。收尾时自己调一次，故「漏了」不会是一次沉默。
 * `h` 可传入合成快照 —— 让「检漏器本身有没有牙」这条自己有正向对照（`css-oracle.test.ts` ⑧）。
 */
export function assertNoOracleLeak(h = oracleHygiene()): void {
  if (h.liveBrowsers.length === 0 && h.leftProfiles.length === 0) return;
  throw new Error(
    `CSS 裁判收尾没清干净（本次启动 ${h.started} 个）：\n` +
      h.liveBrowsers.map((b) => `    还活着：pid=${b.pid} profile=${b.dir}`).join('\n') +
      (h.liveBrowsers.length > 0 && h.leftProfiles.length > 0 ? '\n' : '') +
      h.leftProfiles.map((d) => `    profile 还在：${d}`).join('\n') +
      '\n本模块任何路径都不 kill 进程 —— 请退不掉就必须报出来，' +
      '否则宿主上会攒下 ppid=1 的孤儿 headless Chrome（2026-09-05 实测 14 个）。',
  );
}

/** CDP 的请退报文。id 取一个不会与 [`Conn.send`] 的自增序号撞车的大数。 */
const CLOSE_FRAME = JSON.stringify({ id: 1_000_000, method: 'Browser.close', params: {} });

/**
 * `DevToolsActivePort` 的内容 ⇒ 端口 + 路径。**两行都齐全才算数**，否则返回 `null`。
 *
 * 🔴 2026-09-05 第六轮 A2：Chrome 是**分两行写**这个文件的（先端口、再 `/devtools/browser/<uuid>`），
 * 上一版 `existsSync` 一为真就 `readFileSync(...).trim().split('\n')` ⇒ 读到半截时 `path` 是
 * `undefined`，拼出来的 `ws://127.0.0.1:44663undefined` 让 `new WebSocket` 抛
 * `SyntaxError: TypeError: Invalid URL`。本轮 12 次全量跑里**自然触发 1 次**，随后 `abandon` 因
 * `ws === undefined` 放弃请退 ⇒ 那个浏览器成了模块自己回收不了的孤儿（实测留 10 个进程）。
 * 纯函数形态是为了让「半截文件读不出东西」这条自己有门（`css-oracle.test.ts` ⑨）。
 */
export function parseDevToolsEndpoint(raw: string): { port: string; path: string } | null {
  const lines = raw.split('\n');
  if (lines.length < 2) return null;
  const port = lines[0].trim();
  const path = lines[1].trim();
  if (!/^\d{1,5}$/.test(port) || !path.startsWith('/')) return null;
  return { port, path };
}

/** profile 目录里那个端口文件说的 CDP 地址；文件缺席或**还没写全**时返回 `null`。 */
export function browserWsUrl(dir: string): string | null {
  const portFile = join(dir, 'DevToolsActivePort');
  if (!existsSync(portFile)) return null;
  let raw: string;
  try {
    raw = readFileSync(portFile, 'utf8');
  } catch {
    return null;
  }
  const ep = parseDevToolsEndpoint(raw);
  return ep === null ? null : `ws://127.0.0.1:${ep.port}${ep.path}`;
}

/** 轮询到该 profile 底下一个进程都不剩，或超时。**只读 `/proc`，不发信号**。 */
async function waitGone(dir: string, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (pidsUnder(dir).length === 0) return true;
    if (Date.now() > deadline) return false;
    await sleep(50);
  }
}

/**
 * **手里没有 ws 也要能请退**：按 profile 里的端口自己重连一次，发 `Browser.close`。
 *
 * 🔴 第六轮 A2 的另一半：上一版 `abandon` 在 `ws === undefined` 时**直接放弃**，于是「ws 还没建起来
 * 就抛了」这一档（端口文件半截、握手失败、`Target.createTarget` 报错…）留下的浏览器，模块自己
 * 永远回收不了。请退成功返回 `true`。全程不发信号。
 */
async function requestExitViaProfile(dir: string): Promise<boolean> {
  const url = browserWsUrl(dir);
  if (url === null) return false;
  let ws: WebSocket;
  try {
    ws = new WebSocket(url);
  } catch {
    return false;
  }
  try {
    await new Promise<void>((res, rej) => {
      ws.onopen = () => res();
      ws.onerror = () => rej(new Error('重连 CDP 失败'));
      setTimeout(() => rej(new Error('重连 CDP 超时')), 10_000);
    });
    ws.send(CLOSE_FRAME);
    await waitGone(dir, 10_000);
    return true;
  } catch {
    return false;
  } finally {
    try {
      ws.close();
    } catch {
      /* 已断 */
    }
  }
}

/**
 * 启动失败路径：尽力让它自己退，退掉了就清 profile；退不掉就留给 [`assertNoOracleLeak`] 点名。
 * 导出是为了让「启动抛出时 profile 会被清掉」与「没有 ws 也请退得掉」两半各自有门
 * （`css-oracle.test.ts` ⑧ / ⑨）。**任何路径都不 kill 进程。**
 */
export async function abandon(o: Owned, child: ChildProcess | undefined, ws: WebSocket | undefined): Promise<void> {
  const alive = child === undefined ? pidsUnder(o.dir).length > 0 : child.exitCode === null;
  if (alive) {
    const exited = child === undefined ? null : new Promise<void>((res) => child.once('exit', () => res()));
    let sent = false;
    if (ws !== undefined && ws.readyState === ws.OPEN) {
      try {
        ws.send(CLOSE_FRAME);
        sent = true;
      } catch {
        /* 连接已断 —— 落到下面的重连 */
      }
    }
    // 没有 ws（或 ws 已断）⇒ 按 profile 里的端口重连一次再请退（第六轮 A2）。
    if (!sent) sent = await requestExitViaProfile(o.dir);
    if (sent) await Promise.race([exited ?? waitGone(o.dir, 10_000), sleep(10_000)]);
  }
  try {
    ws?.close();
  } catch {
    /* 已断 */
  }
  // 🔴 删 profile 之前**等整棵进程树散干净**，不是瞄一眼就下结论。子进程比直接子进程晚退几十毫秒
  // 是常态（并行跑时更明显），瞄一眼判成「还活着」就跳过删除，紧接着的自查却看到「进程没了、
  // 目录还在」⇒ 报一次假漏。等，或者等到超时后如实留给自查点名。
  if (await waitGone(o.dir, 15_000)) {
    rmSync(o.dir, { recursive: true, force: true });
    o.closed = true;
  }
}

/** 启动参数。导出是为了让「没有 ws 也请退得掉」那条门用**同一份**参数起浏览器，而不是抄一份。 */
const CHROME_ARGS: readonly string[] = [
  '--headless',
  '--disable-gpu',
  '--no-first-run',
  '--no-default-browser-check',
  '--disable-extensions',
  '--disable-background-networking',
  '--disable-component-update',
  '--disable-sync',
  '--disable-default-apps',
  '--mute-audio',
  '--hide-scrollbars',
  // 端口交给内核挑（写死端口号会与宿主上跑着的其它 Chrome 撞车），从 DevToolsActivePort 读回来。
  '--remote-debugging-port=0',
];

export interface SpawnedBrowser {
  owned: Owned;
  child: ChildProcess;
  /** 最近若干行 stderr（起不来时的死因）。 */
  stderr: string[];
}

/**
 * 起一个**登记在册**的浏览器：profile 目录 + `Owned` 记录都进 [`oracleHygiene`] 的视野。
 *
 * 导出给「`abandon` 在没有 ws 时也请退得掉」那条门用（`css-oracle.test.ts` ⑨）——
 * 它必须跟生产路径起同一种浏览器，抄一份参数就成了「测的不是生产那条腿」。
 */
export function spawnOwnedBrowser(): SpawnedBrowser {
  const dir = mkdtempSync('/var/tmp/polaris-css-oracle-');
  const owned: Owned = { pid: undefined, dir, closed: false };
  OWNED.push(owned);
  const child = spawn(CHROME_BIN, [...CHROME_ARGS, `--user-data-dir=${dir}`, 'about:blank'], {
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  owned.pid = child.pid;
  const stderr: string[] = [];
  child.stderr?.on('data', (b: Buffer) => {
    stderr.push(String(b));
    if (stderr.length > 40) stderr.shift();
  });
  return { owned, child, stderr };
}

/**
 * 等 `DevToolsActivePort` **写全两行**再读（第六轮 A2）。
 *
 * 「文件存在」不等于「写完了」：Chrome 分两行写，中间那一瞬读出来的 path 是 `undefined`。
 * 这里的循环条件是 [`parseDevToolsEndpoint`] 给出非 `null`，不是 `existsSync`。
 */
export async function waitForDevToolsEndpoint(
  b: SpawnedBrowser,
  timeoutMs = 30_000,
): Promise<{ port: string; path: string }> {
  const portFile = join(b.owned.dir, 'DevToolsActivePort');
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (existsSync(portFile)) {
      const ep = parseDevToolsEndpoint(readFileSync(portFile, 'utf8'));
      if (ep !== null) return ep;
    }
    if (b.child.exitCode !== null)
      throw new Error(`Chrome 起不来（退出码 ${b.child.exitCode}）：\n${b.stderr.join('')}`);
    if (Date.now() > deadline)
      throw new Error(`等 DevToolsActivePort 写全超时：\n${b.stderr.join('')}`);
    await sleep(25);
  }
}

async function launch(): Promise<Conn> {
  const spawned = spawnOwnedBrowser();
  let ws: WebSocket | undefined;
  try {
    return await launchInto(spawned, (w) => (ws = w));
  } catch (err) {
    await abandon(spawned.owned, spawned.child, ws);
    throw err;
  }
}

async function launchInto(spawned: SpawnedBrowser, keepWs: (w: WebSocket) => void): Promise<Conn> {
  const { owned, child } = spawned;
  const dir = owned.dir;
  const { port, path } = await waitForDevToolsEndpoint(spawned);

  const ws = new WebSocket(`ws://127.0.0.1:${port}${path}`);
  keepWs(ws);
  await new Promise<void>((res, rej) => {
    ws.onopen = () => res();
    ws.onerror = () => rej(new Error('CDP 的 WebSocket 握手失败'));
  });

  let nextId = 0;
  const pending = new Map<number, { res: (v: Record<string, unknown>) => void; rej: (e: Error) => void }>();
  ws.onmessage = (ev: MessageEvent) => {
    const msg = JSON.parse(String(ev.data)) as { id?: number; error?: unknown; result?: Record<string, unknown> };
    if (msg.id === undefined) return;
    const slot = pending.get(msg.id);
    if (slot === undefined) return;
    pending.delete(msg.id);
    if (msg.error !== undefined) slot.rej(new Error(`CDP 报错：${JSON.stringify(msg.error)}`));
    else slot.res(msg.result ?? {});
  };
  // 连接断掉时把在途请求**结清**。不写这一段，`Browser.close` 那条请求会永远吊着 ——
  // 它的应答常常随连接一起消失（浏览器先关连接再退），于是收尾 hook 卡到超时（实测：
  // 全量并行跑时四个文件的 afterAll 一起 Hook timed out，而 3957 条断言全绿）。
  ws.onclose = () => {
    for (const [, slot] of pending) slot.rej(new Error('CDP 连接已断开'));
    pending.clear();
  };
  const send: Conn['send'] = (method, params = {}, sessionId) =>
    new Promise((res, rej) => {
      const id = ++nextId;
      pending.set(id, { res, rej });
      ws.send(JSON.stringify({ id, method, params, ...(sessionId === undefined ? {} : { sessionId }) }));
    });

  // 子进程句柄不再钉住事件循环：收尾仍然只走 `Browser.close`（本模块任何路径都不 kill），
  // 但万一它退得慢，测试进程不必陪着它一起吊死。
  child.unref();
  return { child, ws, dir, send };
}

async function conn(): Promise<Conn> {
  if (CONN !== null) return CONN;
  OPENING ??= launch().then((c) => {
    CONN = c;
    OPENING = null;
    return c;
  });
  return OPENING;
}

/**
 * 收尾时**排空在途度量**的等待上限（毫秒）。
 *
 * 做成可写的一格，只为给 [`closeOracle`] 那条超时分支一条判据（第七轮 T7：A6 自己立的规矩
 * 「每条 `throw` 都要有判据」在这一条上此前不成立）。为什么不做成 `closeOracle` 的形参 ——
 * 那个函数被 `afterAll(closeOracle, …)` 直接注册，vitest 会把第一个形参当 fixture 解析，
 * 加参数会让全部消费文件 `FixtureParseError`（本轮实测）。判据用完必须写回 60_000。
 */
export const DRAIN = { timeoutMs: 60_000 };

/**
 * 收尾。**走 CDP 的 `Browser.close` 让 Chrome 自己退** —— 本模块任何路径上都不 kill 进程。
 * 每个用到裁判的测试文件都要 `afterAll(closeOracle)`。
 *
 * 三件事按顺序做，缺一件就会留下 ppid=1 的孤儿（第五轮 M03 实测 14 个）：
 *  1. **等启动落地**：`OPENING` 在途时直接返回 ⇒ 浏览器随后才起来而收尾已撒手；
 *  2. 请退并**确认它真的退了**才删 profile（还活着就删 = 拿掉一个活进程的家）；
 *  3. 收尾后 [`assertNoOracleLeak`] 自查一遍，没清干净就抛并点名。
 *
 * 🔴 **本函数不许有形参**：它被 `afterAll(closeOracle, …)` 直接注册，而 vitest 会把第一个形参
 * 当成 fixture 解析 —— 加一个 `opts` 参数，全部消费文件当场 `FixtureParseError`（本轮实测）。
 * 所以第 0 步那条超时上限放在 [`DRAIN`] 这一格里（见那里的理由）。
 */
export async function closeOracle(): Promise<void> {
  // 0. **在途的度量**先等回来（第六轮 A1）。不等它 ⇒ 浏览器在收尾之后才起来，
  //    而那时 `OWNED` 还是空的，自查器对着空表说「干净」。
  const drainDeadline = Date.now() + DRAIN.timeoutMs;
  while (INFLIGHT.size > 0) {
    // `>=`（不是 `>`）：让 `DRAIN.timeoutMs = 0` 有确定含义 —— 一次都不等，当场报在途几个。
    if (Date.now() >= drainDeadline)
      throw new Error(
        `CSS 裁判收尾时还有 ${INFLIGHT.size} 次度量在途，等了 ${DRAIN.timeoutMs}ms 没落地 —— ` +
          '继续收尾就等于把它们启动的浏览器丢在宿主上（第六轮 A1）。',
      );
    await Promise.race([Promise.all([...INFLIGHT]), sleep(200)]);
  }
  // 1. 启动在途 ⇒ 等它落地。失败路径 `launch` 自己清过 profile，这里只吞异常。
  if (OPENING !== null) {
    try {
      await OPENING;
    } catch {
      /* 启动失败：profile 已由 `abandon` 处理，残留由下面的自查点名 */
    }
  }
  const c = CONN;
  CONN = null;
  OPENING = null;
  if (c !== null) {
    const owned = OWNED.find((o) => o.dir === c.dir);
    const gone = new Promise<void>((res) => c.child.once('exit', () => res()));
    if (c.ws.readyState === c.ws.OPEN) {
      // **不等应答**：`Browser.close` 的回包常常随连接一起消失。真正的完成信号是子进程退出。
      void c.send('Browser.close').catch(() => undefined);
      await Promise.race([gone, sleep(10_000)]);
    }
    try {
      c.ws.close();
    } catch {
      /* 已断 */
    }
    // ws 已经断了（或请退没起效）而进程还在 ⇒ 按 profile 里的端口重连一次再请退（第六轮 A2）。
    if (pidsUnder(c.dir).length > 0) await requestExitViaProfile(c.dir);
    // 2. 确认退干净了再删 profile —— 还活着就把它的 profile 删掉，等于给一个活进程拆家。
    //    **等整棵树散干净**再判（子进程比直接子进程晚退是常态，瞄一眼会判出一次假漏）。
    if (await waitGone(c.dir, 15_000)) {
      rmSync(c.dir, { recursive: true, force: true });
      if (owned !== undefined) owned.closed = true;
    }
    // 3. 「清干净了」也要有输出，否则它与「悄悄漏一个」在日志上长得一模一样。
    const h = oracleHygiene();
    if (h.liveBrowsers.length === 0 && h.leftProfiles.length === 0)
      console.log(`[css-oracle] 收尾自查：本次启动 ${h.started} 个浏览器，已全部退出、profile 已删。`);
  }
  assertNoOracleLeak();
}

/**
 * 🔴 **收尾 hook 由本模块自己挂**（第六轮 A1）。
 *
 * 上一版靠每个消费文件自己写 `afterAll(closeOracle)`。「忘了写」只剩 `beforeExit` 一条兜底，
 * 而兜底路径既晚（vitest 早就把 rc 报出去了）又只能打 stderr。本模块在 import 期注册一次：
 * 谁 import 它，谁的那个文件就一定有收尾 —— 「哪条路径都不漏」的前提是这道 hook 不能靠人记得写。
 * `closeOracle` 幂等，消费文件里已有的 `afterAll(closeOracle, 30_000)` 保留着也不会重复请退。
 */
afterAll(closeOracle, 60_000);

// 兜底：进程正常退出前再关一次（`beforeExit` 允许再排异步活）。
// 兜底路径不能靠抛异常自曝（没人接），故把残留打到 stderr 并把退出码染红。
process.once('beforeExit', () => {
  void closeOracle().catch((err: unknown) => {
    console.error(String(err));
    process.exitCode = 1;
  });
});

// ════════════════════════════════════════════════════════════════════════════
// ④ 度量
// ════════════════════════════════════════════════════════════════════════════

/**
 * **挂在生产树上的** markup —— 只有 `styles/mount.test-support.tsx` 的挂载助手造得出来。
 *
 * 🔴 为什么要一个品牌类型：第五轮 B01/B02 的根因是「喂了一棵不是生产的树」，而**修好一处不等于
 * 堵住这条路** —— 下一个人再写一处 `measure({ ctx: 'mobile', html: renderToStaticMarkup(<屏/>) })`
 * 洞就原样回来，且没有任何东西会红（新路径绕开旧闸门）。把它做成类型：给了 `ctx` 就只收
 * [`MountedMarkup`]，裸字符串**编译不过**。反向对照要故意喂一棵残缺树时，走挂载助手里那个名字
 * 很难看的 `unsafeUnmountedMarkup()` —— 绕得过去，但绕的时候必须写出来。
 */
export type MountedMarkup = string & { readonly __mountedOnProductionTree: unique symbol };

/** 不给 `viewport` 时用的视口。**是采样面的一部分**，见 [`SAMPLED_FACE`]。 */
export const DEFAULT_VIEWPORT = Object.freeze({ width: 1280, height: 800, dpr: 1 });

/**
 * 🔴 **默认必须显式覆写的媒体特性**（第六轮 A3）。
 *
 * 上一版给 `Emulation.setEmulatedMedia` 传的是 `features: []` —— 那是**清空覆写**，于是这些特性
 * 由**跑门这台机器的系统设置**决定。产品 CSS 里 `@media (prefers-color-scheme:dark){ :root:not([data-theme="light"]) … }`
 * 这一族在 `data-theme="dark"` 下**照样命中**，所以同一个 commit 在深色系统与浅色系统上得出不同结论；
 * 实测同一条变异的两半一红一绿。判据不许由跑它的那台机器决定。
 *
 * 取值选 `light` 的理由：它是 CSS 规范里这三项的初始值，也是 `tokens.resolved.css` / `mobile.css`
 * 的默认腿（`:root{ color-scheme: light }`）。要 dark 那条腿的调用方**显式写出来**
 * （`media: { 'prefers-color-scheme': 'dark' }`），两条腿各自明确，不靠机器。
 */
export const MEDIA_DEFAULTS: Readonly<Record<string, string>> = Object.freeze({
  'prefers-color-scheme': 'light',
  'prefers-reduced-motion': 'no-preference',
  'forced-colors': 'none',
});

/** 实际发给浏览器的媒体特性 = 默认表 + 调用方覆写。 */
export const effectiveMedia = (media?: Readonly<Record<string, string>>): Record<string, string> => ({
  ...MEDIA_DEFAULTS,
  ...(media ?? {}),
});

/**
 * 🔴 **声明的采样面**（第六轮 B1）—— 本裁判到底在哪几个条件档上采过样。
 *
 * 视口 × 方向 × 主题 × `data-*` × `prefers-*` 是组合面，追它没有终点。本模块**不追**，
 * 只把采到的那几个点写成一张表，两个方向各有一段自检，**强度各不相同**（第七轮 T1 / T2）：
 *  · **实际 ⊆ 声明**：`measureOnce` 拿实际发出去的点过 [`assertPointDeclared`]。运行期自检，
 *    **这条接线自己没有判据** —— 删掉那一行不会有任何东西说话。
 *  · **声明 ⊆ 实际**：`css-oracle-sampling.test.ts` 从**全部调用点的源码**里把字面量提出来与本表对拍。
 *    它判的是**源码**，不是运行期：采样者被 `skip` 掉而源码原样留着 ⇒ 照旧全绿。
 *
 * 本表按**轴**声明，不按（链 × 轴）：它答不出「桌面链采过 dark 吗」。
 * 表外的条件档 **今天没有判据**。哪些形态谁都抓不到，见本文件头注那张表。
 */
export const SAMPLED_FACE = Object.freeze({
  /** `Emulation.setDeviceMetricsOverride` 的取值，写成 `宽x高@dpr`。 */
  viewport: Object.freeze(['1280x800@1', '320x740@1', '390x844@1', '980x740@1']),
  /** `Emulation.setEmulatedMedia` 的特性 → 取值集。**未列出的特性一律不覆写、也不许被问到颜色**。 */
  media: Object.freeze({
    'prefers-color-scheme': Object.freeze(['light', 'dark']),
    'prefers-reduced-motion': Object.freeze(['no-preference']),
    'forced-colors': Object.freeze(['none']),
  }) as Readonly<Record<string, readonly string[]>>,
  /** `<html>` 上的属性 → 取值集（`lang` / `dir` 来自宿主 `.html`，其余来自调用点）。 */
  rootAttrs: Object.freeze({
    lang: Object.freeze(['en-US']),
    dir: Object.freeze(['ltr']),
    'data-theme': Object.freeze(['dark', 'light']),
    'data-os': Object.freeze(['win', 'lin', 'mac']),
  }) as Readonly<Record<string, readonly string[]>>,
  /** `<body>` 上的属性 → 取值集。今天一个都没有（两份宿主 `.html` 的 `<body>` 都是光的）。 */
  bodyAttrs: Object.freeze({}) as Readonly<Record<string, readonly string[]>>,
});

/** 视口点的规范写法，两侧对拍用同一个函数，免得一边写 `@1` 一边写 `@1.0`。 */
export const viewportKey = (v: { width: number; height: number; dpr?: number }): string =>
  `${v.width}x${v.height}@${v.dpr ?? DEFAULT_VIEWPORT.dpr}`;

/** 实际采样点在声明的采样面之内吗？不在就抛并点名是哪一轴的哪个值。 */
export function assertPointDeclared(point: SampledPoint, face = SAMPLED_FACE): void {
  const bad: string[] = [];
  const vk = viewportKey(point.viewport);
  if (!face.viewport.includes(vk)) bad.push(`视口 \`${vk}\``);
  const axis = (
    kind: string,
    actual: Readonly<Record<string, string>>,
    declared: Readonly<Record<string, readonly string[]>>,
  ): void => {
    for (const [name, value] of Object.entries(actual)) {
      const values = declared[name];
      if (values === undefined) bad.push(`${kind} \`${name}\`（整条轴都没声明）`);
      else if (!values.includes(value)) bad.push(`${kind} \`${name}\` 的取值 \`${value}\``);
    }
  };
  axis('媒体特性', point.media, face.media);
  axis('`<html>` 属性', point.rootAttrs, face.rootAttrs);
  axis('`<body>` 属性', point.bodyAttrs, face.bodyAttrs);
  if (bad.length === 0) return;
  throw new Error(
    `这次度量落在**没有声明过**的条件档上：\n${bad.map((b) => `    ${b}`).join('\n')}\n` +
      '本裁判的采样面是一张**明写出来的**表（`SAMPLED_FACE`）—— 多采一个点而不声明，' +
      '就等于让「本裁判在哪些档上采样」这句话变成假话。\n' +
      '要采这一档：把取值补进 `SAMPLED_FACE`。\n' +
      '⚠️ 别把这条自检读成「采样面被守着」：本函数自己有正反对照，但\n' +
      '   ·「`measureOnce` 真的调用了它」这条接线**没有判据**（注掉那一行，全量照样全绿）；\n' +
      '   ·`css-oracle-sampling.test.ts` 判的是**调用点源码里有没有那个字面量**，不是运行期真采过\n' +
      '     （把采样者 `describe.skip` 掉，那道门仍然全绿）。\n' +
      '   两条都在头注「🔴 本裁判抓不到的形态」表里，别在这里读出比那张表更强的结论。',
  );
}

/** 取值会随主题变的属性 —— 问它们之前，`prefers-color-scheme` 必须已经被钉住。 */
const COLOR_PROP =
  /(?:^|-)color$|^background(?:-image)?$|^border(?:-[a-z]+)?$|^outline$|^box-shadow$|^text-shadow$|^fill$|^stroke$|^color-scheme$/;

/**
 * **没有显式钉住 `prefers-color-scheme` 就不许问颜色** ⇒ 抛。
 *
 * 这条判据是 [`MEDIA_DEFAULTS`] 的牙：那张表只要有人抽掉 `prefers-color-scheme` 这一行，
 * 生产路径上每一条颜色断言都会当场红，而不是安静地回到「由机器决定」。
 * 纯函数形态是为了让它自己有正向/反向对照（`css-oracle.test.ts` ⑨）。
 */
export function assertColorAsksArePinned(
  media: Readonly<Record<string, string>>,
  asks: readonly { select: string; props: readonly string[] }[],
): void {
  if ('prefers-color-scheme' in media) return;
  const asked = asks.flatMap((a) => a.props.filter((prop) => COLOR_PROP.test(prop)).map((prop) => `${a.select} 的 ${prop}`));
  if (asked.length === 0) return;
  throw new Error(
    `没有显式覆写 \`prefers-color-scheme\` 就问了颜色：${asked.join('、')}。\n` +
      '不覆写 = 由**跑门这台机器的系统主题**决定，同一个 commit 换台机器就换个结论' +
      '（第六轮 A3 实测：同一条变异的两半一红一绿）。把它写进 `media`，或写回 `MEDIA_DEFAULTS`。',
  );
}

interface OracleCommon {
  /** `<html>` 上的属性（`data-os` / `data-theme` / `dir` …）。会与宿主文档的 `lang` / `dir` 合并。 */
  rootAttrs?: Readonly<Record<string, string>>;
  /** `<body>` 上的属性。 */
  bodyAttrs?: Readonly<Record<string, string>>;
  /** 视口（`@media` / `@container` 的判据）。默认 [`DEFAULT_VIEWPORT`]。 */
  viewport?: { width: number; height: number; dpr?: number };
  /** 媒体特性覆写，如 `{ 'prefers-color-scheme': 'dark' }`。与 [`MEDIA_DEFAULTS`] 合并。 */
  media?: Readonly<Record<string, string>>;
  /**
   * 角色色探针：名字 → 一段 CSS 值（如 `hsl(var(--ok))`）。
   * 以**行内样式**注入一个隐藏元素并读它的 `color` ⇒ 拿到该令牌在这条链上算出来的真值。
   * 断言「延迟 fast 档 = status.success」时比对的是两个 rgb，而不是「源码里写着 `--ok`」——
   * 令牌调色不误伤，角色对调必红。
   *
   * 🔴 **探针宿主挂在 `<head>` 里**（第六轮 A7）。上一版把它放在 `<body>` 中、与挂载树**并列** ——
   * 于是令牌被改在 `body`（或更外层）时，探针与被判元素**一起移动**，「对拍探针挡得住角色对调」
   * 这句话在那一档上恒真。实测：`body{ --ok-weak: 0 0% 100% }` ⇒ 全量全绿，而浏览器实算的
   * 「当前出口」角标背景已经变成纯白。
   * 现在探针是 `<head>` 里的 `<meta>`（HTML 解析器允许它留在 head，`<span>` 会被提到 body 去），
   * 自定义属性从 `<html>` 继承下来 ⇒ **读到的是设计系统在 `:root` 上给的那个值**。
   * `body` 及以下的任何覆写都只动被判元素、不动探针 ⇒ 两边分叉 ⇒ 红。
   * 令牌被改在 `:root` 自己身上时两边仍然一起动 —— 那一档由**色族**腿承重（见各屏门里的
   * `LAT_FAMILY` / `isDanger`），色族腿答的是「用户看到的是不是绿/红」。
   */
  probes?: Readonly<Record<string, string>>;
}

/** 问「产品在某个层叠上下文里长什么样」：整条真链 + **生产那棵挂载树**。 */
export interface OracleContextInput extends OracleCommon {
  ctx: CtxName;
  css?: undefined;
  html: MountedMarkup;
}

/** 问「这几条 CSS 在这段 markup 上算出什么」：合成链 + 合成夹具（正向/反向对照用）。 */
export interface OracleMemoryInput extends OracleCommon {
  ctx?: undefined;
  /** 直接给 CSS 链（按进包顺序）。 */
  css: readonly string[];
  html: string;
}

export type OracleInput = OracleContextInput | OracleMemoryInput;

export interface Ask {
  /** `document.querySelector` 的选择器。 */
  select: string;
  props: readonly string[];
  /** 伪元素（`::before` / `::after`）。 */
  pseudo?: string;
  /**
   * 顺带问**祖先链**：从该元素向上，computed 的 `prop` 命中 `value` 的祖先都是谁。
   *
   * 「粘在 header 下」「面板不在置灰包裹里」这类判据的真值是**祖先关系 + 计算值**，
   * 不是「源码里写过 `position: sticky`」。静态模型两样都答不了：它既没有 DOM，
   * 也判不了某一档下这句话还成不成立。
   */
  ancestorsWhere?: { prop: string; value: string };
  /**
   * 该选择器在真 DOM 上命中**多个**元素时的表态。**不给 ⇒ 命中必须恰好一个**，否则抛。
   *
   * 🔴 2026-09-05 第五轮 M02：上一版 `get` / `rect` / `ancestors` 只回答**第一个**命中元素，
   * 而紧挨着的 `count(...) > 0` 让人读成「这一族都查了」。实测 traffic 夹具下 `.mr-grip` 的
   * `count = 3`，门只读了第一个的 `touch-action`：三个里中间那个改坏，门照绿。
   * 现在只剩两条路，**都要写出来**：`'all'` 全量返回、由调用方对全集断言（[`Measured.getAll`]）；
   * `'first'` 明说「我只读第一个」。沉默不再是一个选项。
   */
  many?: 'first' | 'all';
}

export interface Measured {
  /** 浏览器算出的 computed 值。没问过 / 没命中 ⇒ 抛；`many: 'all'` 的问法 ⇒ 抛（改用 `getAll`）。 */
  get(select: string, prop: string, pseudo?: string): string;
  /** 该选择器命中的**每一个**元素的 computed 值，按文档序。 */
  getAll(select: string, prop: string, pseudo?: string): string[];
  /** 探针色。 */
  probe(name: string): string;
  /** 该选择器在真 DOM 上命中的元素个数。 */
  count(select: string): number;
  /** 目标元素的边框盒（`getBoundingClientRect`）。 */
  rect(select: string): { x: number; y: number; width: number; height: number };
  /** 每一个命中元素的边框盒。 */
  rectAll(select: string): { x: number; y: number; width: number; height: number }[];
  /** [`Ask.ancestorsWhere`] 命中的祖先，自近及远，形如 `div.mc-top`。 */
  ancestors(select: string): string[];
  /**
   * 目标元素**完整的**祖先链，自近及远直到 `<html>`，形如 `div.m-page` / `div#root` / `body`。
   *
   * 「喂进裁判的祖先链与生产一致」这条判据要的就是它（`styles/mount-tree.test.tsx`）——
   * 上一版裁判喂的是屏组件的 `renderToStaticMarkup`，屏根之上一层都没有，
   * 于是把变异往上抬一层就全绿（第五轮 B01/B02）。
   */
  ancestorPath(select: string): string[];
}

const attrs = (a: Readonly<Record<string, string>> | undefined): string =>
  a === undefined ? '' : Object.entries(a).map(([k, v]) => ` ${k}="${v.replace(/"/g, '&quot;')}"`).join('');

/**
 * 组装喂给浏览器的整份文档。
 *
 * 给了 `ctx` ⇒ **宿主段由产品那份 `.html` 提供**（`<html lang dir>`、`<head>` 里那几段 `<style>`、
 * `<body>`、挂载元素），调用方的 `html` 注进挂载点里。给的是内存链 `css` ⇒ 不套宿主
 * （合成对照本来就不该有生产祖先）。
 *
 * **段序 = 生产段序**：宿主 `<head><style>` 在前、打包 CSS 在后。真实构建产物就是这个顺序
 * （`dist/mobile.html`：`<style>` 第 21 行、`<link rel=stylesheet>` 第 32 行），
 * 顺序反了会把「同特异度谁压谁」整类判反。
 *
 * 🔴 **段序有判据了**（第七轮 T5）：`css-oracle.test.ts` ⑩ 读本函数**产出的那份文档**里
 * `<style data-unit>` 的出现序，与「宿主段 ++ 打包段」逐个对拍。上一版这句话没人管 ——
 * 第七轮验收把两段顺序反过来，全量 4013 条仍然全绿；本轮补门后再做同一条变异，
 * 全量 239 文件 / 4019 条里**恰好红这一条**（其余 4018 条全过），可见此前没有第二处看着它。
 * 导出本函数就是为了让那条判据量到生产这一份，而不是在门里再拼一遍段序（拼一遍 = 判据被自己污染）。
 */
export function buildDocument(input: OracleInput, chain: readonly { file: string; css: string }[]): string {
  const host = input.ctx === undefined ? null : hostSkeleton(input.ctx);
  const hostUnits = (host?.headStyles ?? []).map((css, i) => ({
    file: `${input.ctx === undefined ? '<none>' : HOST_DOCUMENT[input.ctx]}#style${i}`,
    css,
  }));
  const all = [...hostUnits, ...chain];
  for (const u of all)
    if (/<\/style/i.test(u.css))
      throw new Error(`\`${u.file}\` 里出现 \`</style\` —— 注入会截断，本模块不猜它的意思。`);
  const styles = all.map((u) => `<style data-unit="${u.file}">\n${u.css}\n</style>`).join('\n');
  // 🔴 探针住在 `<head>` 里（第六轮 A7）：`<meta>` 是 HTML 解析器允许留在 head 的少数元素之一
  //    （`<span>` 会被提到 `<body>` 去，那就又与挂载树同层了）。它从 `<html>` 继承自定义属性，
  //    故 `body` 及以下的覆写动不了它 —— 两边分叉才看得见「角色被就地改掉」。
  const probes = Object.entries(input.probes ?? {})
    .map(([n, v]) => `<meta data-probe="${n}" content="" style="color:${v}">`)
    .join('');
  const rootAttrs = { ...(host?.rootAttrs ?? {}), ...(input.rootAttrs ?? {}) };
  const bodyAttrs = { ...(host?.bodyAttrs ?? {}), ...(input.bodyAttrs ?? {}) };
  const mounted = host === null ? input.html : `${host.before}${input.html}${host.after}`;
  return (
    `<!doctype html><html${attrs(rootAttrs)}><head><meta charset="utf-8">\n${styles}\n${probes}\n</head>` +
    `<body${attrs(bodyAttrs)}>${mounted}</body></html>`
  );
}

interface Hit {
  props: Record<string, string>;
  rect: { x: number; y: number; width: number; height: number };
  ancestors: string[];
  path: string[];
}
interface RawAsk {
  count: number;
  hits: Hit[];
}
interface RawResult {
  asks: RawAsk[];
  probes: Record<string, string | null>;
}

/**
 * 渲染一次、问一批。
 *
 * 浏览器实例整个测试文件复用；每次度量持有独立 target/session/frame，最后关闭 target。
 * Vitest 的 timeout 不取消 callback，因此旧度量仍在途时也不能覆盖下一份 DOM 或媒体档。
 */
export function measure(input: OracleInput, asks: readonly Ask[]): Promise<Measured> {
  // 🔴 第六轮 A1：**登记在途**。收尾要靠这张簿子把「还没走到 `conn()` 的那次度量」等回来。
  return track(measureOnce(input, asks));
}

/**
 * 一次度量**实际发给浏览器**的采样点：视口 + 媒体特性 + `<html>` / `<body>` 属性。
 *
 * 只有一个消费方 —— [`assertPointDeclared`]，`measureOnce` 在发命令之前拿它过一遍。
 * 🔴 上一版这里还挂着一个 `lastSampledPoint()`，头注写着「B1 那道门拿它从**运行期**取实际值、
 * 而不是从源码里正则」。那是假话：`css-oracle-sampling.test.ts` 做的正是后者（读源码），
 * 而且全仓零消费方。假声明 + 死代码，两条理由各自都够，本轮删掉。
 */
export interface SampledPoint {
  viewport: { width: number; height: number; dpr: number };
  media: Record<string, string>;
  rootAttrs: Record<string, string>;
  bodyAttrs: Record<string, string>;
}

let coldMeasureReported = false;

async function measureOnce(input: OracleInput, asks: readonly Ask[]): Promise<Measured> {
  const reportCold = !coldMeasureReported;
  coldMeasureReported = true;
  const began = performance.now();
  if ((input.ctx === undefined) === (input.css === undefined))
    throw new Error('裁判必须显式给 `ctx`（desktop/mobile/tray/popup）**或** `css`（内存链），二选一。');
  if (asks.length === 0) throw new Error('一条都没问 —— 空判据。');
  // A3：颜色断言必须跑在一个**钉住**了 `prefers-color-scheme` 的档上，否则由机器决定。
  const media = effectiveMedia(input.media);
  assertColorAsksArePinned(media, asks);
  const chain =
    input.css !== undefined
      ? input.css.map((css, i) => ({ file: `<memory>#${i}`, css }))
      : await cssChain(input.ctx);
  const cssReady = performance.now();
  const c = await conn();
  const connected = performance.now();
  const vp = {
    width: input.viewport?.width ?? DEFAULT_VIEWPORT.width,
    height: input.viewport?.height ?? DEFAULT_VIEWPORT.height,
    dpr: input.viewport?.dpr ?? DEFAULT_VIEWPORT.dpr,
  };
  const host = input.ctx === undefined ? null : hostSkeleton(input.ctx);
  const point: SampledPoint = {
    viewport: vp,
    media,
    rootAttrs: { ...(host?.rootAttrs ?? {}), ...(input.rootAttrs ?? {}) },
    bodyAttrs: { ...(host?.bodyAttrs ?? {}), ...(input.bodyAttrs ?? {}) },
  };
  // B1 的一半：**实际采样点必须在声明的采样面之内**。加了一档而忘了声明 ⇒ 这里当场抛。
  // 🔴 这一行的**接线**自己没有判据：删掉它全量照样全绿（见头注「抓不到的形态」表）。
  assertPointDeclared(point);
  const spec = JSON.stringify({
    asks: asks.map((a) => ({
      select: a.select,
      props: a.props,
      pseudo: a.pseudo ?? null,
      ancestorsWhere: a.ancestorsWhere ?? null,
      all: a.many === 'all',
    })),
    probes: Object.keys(input.probes ?? {}),
  });
  const expression = `(() => {
    const spec = JSON.parse(${JSON.stringify(spec)});
    const out = { asks: [], probes: {} };
    const describe = (el) => el.tagName.toLowerCase()
      + (el.id ? '#' + el.id : '')
      + (el.className && typeof el.className === 'string' && el.className.trim()
          ? '.' + el.className.trim().split(/\\s+/).join('.') : '');
    for (const a of spec.asks) {
      const all = document.querySelectorAll(a.select);
      const targets = a.all ? [...all] : all.length > 0 ? [all[0]] : [];
      const hits = [];
      for (const el of targets) {
        const cs = getComputedStyle(el, a.pseudo);
        const props = {};
        for (const p of a.props) props[p] = cs.getPropertyValue(p);
        const r = el.getBoundingClientRect();
        const ancestors = [];
        const path = [];
        for (let p = el.parentElement; p !== null; p = p.parentElement) {
          path.push(describe(p));
          if (a.ancestorsWhere !== null
              && getComputedStyle(p).getPropertyValue(a.ancestorsWhere.prop) === a.ancestorsWhere.value)
            ancestors.push(describe(p));
        }
        hits.push({ props, rect: { x: r.x, y: r.y, width: r.width, height: r.height }, ancestors, path });
      }
      out.asks.push({ count: all.length, hits });
    }
    for (const n of spec.probes) {
      const el = document.querySelector('head meta[data-probe="' + n + '"]');
      out.probes[n] = el === null ? null : getComputedStyle(el).getPropertyValue('color');
    }
    return JSON.stringify(out);
  })()`;
  const html = buildDocument(input, chain);
  const { targetId } = (await c.send('Target.createTarget', { url: 'about:blank' })) as { targetId: string };
  const targetReady = performance.now();
  let raw: RawResult;
  try {
    const { sessionId } = (await c.send('Target.attachToTarget', { targetId, flatten: true })) as { sessionId: string };
    await c.send('Page.enable', {}, sessionId);
    const tree = (await c.send('Page.getFrameTree', {}, sessionId)) as { frameTree: { frame: { id: string } } };
    await c.send(
      'Emulation.setDeviceMetricsOverride',
      { width: vp.width, height: vp.height, deviceScaleFactor: vp.dpr, mobile: false },
      sessionId,
    );
    await c.send(
      'Emulation.setEmulatedMedia',
      { features: Object.entries(media).map(([name, value]) => ({ name, value })) },
      sessionId,
    );
    await c.send('Page.setDocumentContent', { frameId: tree.frameTree.frame.id, html }, sessionId);
    const evaluated = (await c.send('Runtime.evaluate', { expression, returnByValue: true }, sessionId)) as {
      result?: { value?: string };
      exceptionDetails?: unknown;
    };
    if (evaluated.exceptionDetails !== undefined)
      throw new Error(`裁判取值时页面抛了：${JSON.stringify(evaluated.exceptionDetails)}`);
    raw = JSON.parse(String(evaluated.result?.value)) as RawResult;
  } finally {
    // This request alone owns the page. Browser shutdown still drains every in-flight measure.
    const closeBegan = performance.now();
    const closed = await c.send('Target.closeTarget', { targetId });
    if (reportCold) console.log('[css-oracle] cold measure stages (ms):', {
      css: Math.round(cssReady - began),
      connection: Math.round(connected - cssReady),
      target: Math.round(targetReady - connected),
      measurement: Math.round(closeBegan - targetReady),
      close: Math.round(performance.now() - closeBegan),
    });
    if (closed.success !== true) throw new Error('CSS 裁判没有关闭本次度量的页面。');
  }

  const missing = asks.filter((_, i) => raw.asks[i].count === 0).map((a) => a.select);
  if (missing.length > 0)
    throw new Error(
      `真 DOM 上一个元素都没命中：${missing.map((s) => `\`${s}\``).join('、')}。\n` +
        '取材面为空 ⇒ 其上的否定断言恒真。喂进来的 markup 里确实有这一层吗？' +
        '（祖先链存不存在，正是本裁判要替评审回答的那个问题。）',
    );
  // M02：命中多个而调用方没表态 ⇒ 抛。沉默地只读第一个，等于把「这一族都查了」写成了假话。
  const ambiguous = asks
    .map((a, i) => ({ a, n: raw.asks[i].count }))
    .filter(({ a, n }) => n > 1 && a.many === undefined);
  if (ambiguous.length > 0)
    throw new Error(
      ambiguous.map(({ a, n }) => `\`${a.select}\` 在真 DOM 上命中 ${n} 个元素`).join('；') +
        '。\n只读第一个是一句要写出来的话：给 `many: \'all\'` 全量返回并对全集断言（`getAll`），' +
        '或 `many: \'first\'` 明说只读第一个。',
    );

  const byAsk = new Map<string, { ask: Ask; raw: RawAsk }>();
  asks.forEach((a, i) => byAsk.set(`${a.select}\u0000${a.pseudo ?? ''}`, { ask: a, raw: raw.asks[i] }));
  const at = (select: string, pseudo?: string) => {
    const hit = byAsk.get(`${select}\u0000${pseudo ?? ''}`);
    if (hit === undefined) throw new Error(`没问过 \`${select}\`${pseudo ?? ''} —— 先把它写进 asks。`);
    return hit;
  };
  /** 单点读法：`many: 'all'` 的问法上禁用 —— 那种问法的答案是一组，取头一个就是把 M02 又装回来。 */
  const only = (select: string, pseudo?: string): Hit => {
    const { ask, raw: r } = at(select, pseudo);
    if (ask.many === 'all')
      throw new Error(
        `\`${select}\` 是按 \`many: 'all'\` 问的（命中 ${r.count} 个）—— 单点读法会只看第一个。` +
          '用 `getAll` / `rectAll` 对全集断言。',
      );
    return r.hits[0];
  };
  /**
   * 全集读法：`many: 'first'` 的问法上禁用 —— 那种问法浏览器只量了第一个，
   * 让 `getAll` 在它上面返回一个元素的数组，就是把「以为遍历了全族」这个误读原样装回来。
   */
  const every = (select: string, pseudo?: string): RawAsk => {
    const { ask, raw: r } = at(select, pseudo);
    if (ask.many === 'first')
      throw new Error(
        `\`${select}\` 是按 \`many: 'first'\` 问的（真 DOM 上有 ${r.count} 个）—— ` +
          '全集读法在它上面只会拿到第一个。要对全族断言就改成 `many: \'all\'`。',
      );
    return r;
  };
  return {
    get(select, prop, pseudo) {
      const rec = only(select, pseudo).props;
      if (!(prop in rec)) throw new Error(`没问过 \`${select}\` 的 \`${prop}\` —— 先把它写进 asks[].props。`);
      return rec[prop];
    },
    getAll(select, prop, pseudo) {
      const r = every(select, pseudo);
      return r.hits.map((h) => {
        if (!(prop in h.props)) throw new Error(`没问过 \`${select}\` 的 \`${prop}\` —— 先把它写进 asks[].props。`);
        return h.props[prop];
      });
    },
    probe(name) {
      const v = raw.probes[name];
      if (v === null || v === undefined) throw new Error(`探针 \`${name}\` 没注入或没算出来。`);
      return v;
    },
    count: (select) => at(select).raw.count,
    rect: (select) => only(select).rect,
    rectAll: (select) => every(select).hits.map((h) => h.rect),
    ancestors: (select) => only(select).ancestors,
    ancestorPath: (select) => only(select).path,
  };
}
