/**
 * **喂进 CSS 裁判的祖先链，与生产那棵树一致** —— 本轮唯一一条钉住「进料口」的判据。
 *
 * # 为什么必须有它
 *
 * 2026-09-05 第五轮 B01/B02：裁判引擎本身一次都没说谎，被骗的全在它的**进料口**。
 * 上一版喂的是 `renderToStaticMarkup(<屏组件/>)` 与一段手搭的窗口外壳字符串，屏根之上一层都没有；
 * 而生产里屏住在 `#root > .m-shell > .m-scroll > .m-page` 底下、拖动带住在 `#root > .stage > .win`
 * 底下。于是把变异**往上抬一层**就整门全绿：
 *  · `mobile.css` 追加 `.m-page .mn-lat{ color: hsl(var(--err)) }` —— 生产里五个移动屏的每一条
 *    延迟读数全部变红 —— 全量 237 文件 / 3957 断言 rc=0；
 *  · `index.css` 追加 `#root .main-chrome{ height: 24px }`（id 特异度压过全部 `data-os` 档）—— 全绿；
 *  · 往真 `AppShell.tsx` 的 `.main-scroll` 之前插一个 40px 元素 —— 整门 8/8 全绿。
 * 修法是喂真挂载树；而**只修不钉，下次外壳一改这个洞原样回来**。本文件就是那颗钉子。
 * （另一半锁在类型上：`measure` 给了 `ctx` 就只收 `MountedMarkup`，只有
 * `styles/mount.test-support.tsx` 的挂载助手造得出来 —— 下一处新调用想喂裸字符串直接编译不过。）
 *
 * # 判据形状：两条，缺一条就漏掉本轮的一个 blocker
 *
 *  1. **整棵树相等**（守 B02）：裁判的取材面与**生产根组件**渲染出来的 markup **逐字节相同**
 *     （`inMobileShell(<MobileHomeScreen/>) === <MobileApp/>`；`<AppShell/> === <App/>`）。
 *     为什么不能只比祖先链：B02 那条变异（往 `AppShell.tsx` 的 `.main-scroll` **之前**插一个
 *     40px 元素）改的是**兄弟**不是祖先 —— 实测手搭的那段外壳与生产树的 `.main-chrome` 祖先链
 *     **逐层相同**，只比链是看不见它的。整树相等才把「兄弟少了一个」也接住。
 *  2. **浏览器实读的祖先链 = 生产链 + 宿主段**（守 B01 的另一半）：期望链由标签栈解析器从生产
 *     渲染里算，尾巴接上 `hostSkeleton` 从 `mobile.html` / `index.html` 解析出的挂载点 + `body` +
 *     `html`；实测链由 Chrome 顺着 `parentElement` 走。两条通路不共享代码，共享的只有源文件 ——
 *     宿主段漏掉时（上一版就没有 `#root`）第 1 条看不见，只有这一条看得见。
 *
 * 每条判据都配**反向对照**：喂上一版那种取材面（屏根即根 / 手搭外壳），本判据必须当场红 ——
 * 否则它只是又一条恒绿的断言。
 */
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, describe, expect, it } from 'vitest';
import App from '@/App';
import { MobileApp } from '@/mobile/MobileApp';
import { MobileHomeScreen } from '@/mobile/home/MobileHomeScreen';
import { inDesktopShell, inMobileShell, unsafeUnmountedMarkup } from './mount.test-support';
import { closeOracle, hostSkeleton, measure } from './css-oracle.test-support';
import type { CtxName } from './css-cascade.test-support';

afterAll(closeOracle, 30_000);

/** HTML 里不需要闭标签的元素 —— 栈解析器不能把它们压进去。 */
const VOID_ELEMENTS = new Set([
  'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input',
  'link', 'meta', 'param', 'source', 'track', 'wbr',
]);
/** 注释整段跳过；属性里的引号串整段吃掉（值里可能有 `>`）。 */
const TAG = /<!--[\s\S]*?-->|<(\/?)([a-zA-Z][\w:-]*)((?:[^>"']|"[^"]*"|'[^']*')*)(\/?)>/g;

/** 与裁判在浏览器里用的 `describe()` 同一套写法：`tag` + `#id` + `.cls1.cls2`。 */
function describeTag(tag: string, attrs: string): string {
  const id = /\bid="([^"]*)"/.exec(attrs)?.[1] ?? '';
  const cls = (/\bclass="([^"]*)"/.exec(attrs)?.[1] ?? '').trim();
  return (
    tag.toLowerCase() +
    (id === '' ? '' : `#${id}`) +
    (cls === '' ? '' : `.${cls.split(/\s+/).join('.')}`)
  );
}

/** `html` 里 `needle` 所在那个元素的祖先链，**自近及远**（与 `Measured.ancestorPath` 同序）。 */
export function ancestorChainOf(html: string, needle: string): string[] {
  const at = html.indexOf(needle);
  if (at < 0) throw new Error(`生产渲染里找不到 \`${needle}\` —— 期望链算不出来，别往下推。`);
  const start = html.lastIndexOf('<', at);
  const stack: string[] = [];
  TAG.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = TAG.exec(html)) !== null) {
    if (m.index >= start) break;
    if (m[0].startsWith('<!--')) continue;
    const [, closing, tag, attrs, selfClosing] = m;
    if (closing === '/') {
      stack.pop();
      continue;
    }
    if (selfClosing === '/' || VOID_ELEMENTS.has(tag.toLowerCase())) continue;
    stack.push(describeTag(tag, attrs));
  }
  return stack.reverse();
}

/** 宿主段：挂载点 → `body` → `html`，取自产品那份 `.html`。 */
const hostChain = (ctx: CtxName): string[] => {
  const h = hostSkeleton(ctx);
  return [`${h.mountTag}#${h.mountId}`, 'body', 'html'];
};

describe('移动端：裁判喂的挂载树 = `MobileApp` 那棵树 + `mobile.html` 的宿主段', () => {
  const production = renderToStaticMarkup(createElement(MobileApp));
  const expected = [...ancestorChainOf(production, 'class="h-screen"'), ...hostChain('mobile')];

  it('整棵树相等：`inMobileShell(<MobileHomeScreen/>)` 与生产根组件逐字节相同', () => {
    // `MobileApp` = `MobileShell(active=home)` 装 `SCREENS.home`，故默认档下两边必须一模一样。
    // 兄弟少一个、外壳多包一层、槽位换了位置 —— 只比祖先链都看不见，这一条看得见。
    const withoutReactResourceHints = (markup: string) => markup.replace(/<link rel="preload" as="image"[^>]*\/>/g, '');
    expect(
      withoutReactResourceHints(inMobileShell(renderToStaticMarkup(createElement(MobileHomeScreen)), 'home')),
      '裁判的取材面已经不是生产那棵树了',
    ).toBe(withoutReactResourceHints(production));
  });

  it('自检：期望链本身有内容，且外壳三层都在（算空了下面那条就恒真）', () => {
    expect(expected, '期望链算出来是空的').not.toEqual([]);
    for (const layer of ['div.m-page', 'div.m-scroll', 'div.m-screen-viewport', 'div.m-shell', 'div#root', 'body', 'html'])
      expect(expected, `生产树里没有 ${layer} —— 外壳装配变了，本文件的判据要跟着重写`).toContain(layer);
  });

  it('实测链（浏览器读的）与期望链逐层相等', async () => {
    const m = await measure(
      { ctx: 'mobile', html: inMobileShell(renderToStaticMarkup(createElement(MobileHomeScreen)), 'home') },
      [{ select: '.h-screen', props: ['display'] }],
    );
    expect(m.ancestorPath('.h-screen'), '喂进裁判的祖先链与生产那棵树对不上').toEqual(expected);
  }, 60_000);

  it('反向对照：喂「屏根即根」的取材面（上一版的形状）⇒ 本判据当场红', async () => {
    const m = await measure(
      { ctx: 'mobile', html: unsafeUnmountedMarkup(renderToStaticMarkup(createElement(MobileHomeScreen))) },
      [{ select: '.h-screen', props: ['display'] }],
    );
    const truncated = m.ancestorPath('.h-screen');
    expect(truncated, '屏根之上竟然还有外壳三层 —— 反向对照没造出上一版那个形状').toEqual(hostChain('mobile'));
    expect(truncated, '判据对「屏根即根」这种取材面是瞎的 —— 它守不住 B01').not.toEqual(expected);
  }, 60_000);
});

describe('桌面：裁判喂的挂载树 = `App` 那棵树 + `index.html` 的宿主段', () => {
  const production = renderToStaticMarkup(createElement(App));
  const expected = [...ancestorChainOf(production, 'class="main-chrome"'), ...hostChain('desktop')];

  it('整棵树相等：`<AppShell/>` 与生产根组件 `<App/>` 逐字节相同', () => {
    expect(inDesktopShell(), '裁判的取材面已经不是生产那棵树了').toBe(production);
  });

  it('自检：期望链本身有内容，且窗口外壳各层都在', () => {
    for (const layer of ['main.main', 'div.shell', 'div.win', 'div.stage', 'div#root', 'body', 'html'])
      expect(expected, `生产树里没有 ${layer} —— 窗口外壳装配变了，本文件的判据要跟着重写`).toContain(layer);
  });

  it('实测链（浏览器读的）与期望链逐层相等', async () => {
    const m = await measure({ ctx: 'desktop', html: inDesktopShell() }, [
      { select: '.main-chrome', props: ['height'] },
    ]);
    expect(m.ancestorPath('.main-chrome'), '喂进裁判的祖先链与生产那棵树对不上').toEqual(expected);
  }, 60_000);

  /** 逐字取自上一版 `window-drag-region.test.ts` 的 `SHELL` 常量。 */
  const HAND_BUILT = [
    '<div class="stage"><div class="win">',
    '<span class="winctl"><button type="button">x</button></span>',
    '<div class="shell">',
    '<nav class="side"><div class="side-chrome" data-tauri-drag-region></div></nav>',
    '<main class="main"><div class="main-chrome" data-tauri-drag-region></div>',
    '<div class="main-scroll"></div></main>',
    '</div></div></div>',
  ].join('');

  it('反向对照：上一版那段手搭外壳与生产树不是同一棵 ⇒ 整树判据当场红', () => {
    expect(HAND_BUILT, '手搭外壳竟然与生产树逐字节相同 —— 反向对照没有信息量').not.toBe(production);
  });

  it('🔴 只比祖先链**不够**：手搭外壳的 `.main-chrome` 祖先链与生产逐层相同', async () => {
    // 这一条把「为什么必须有整树判据」写成可执行的：B02 那条变异改的是**兄弟**
    // （`.main-scroll` 之前多一个 40px 元素），祖先链一层没变。
    const m = await measure({ ctx: 'desktop', html: unsafeUnmountedMarkup(HAND_BUILT) }, [
      { select: '.main-chrome', props: ['height'] },
    ]);
    expect(
      m.ancestorPath('.main-chrome'),
      '手搭外壳的祖先链与生产不同了 —— 那本条注释的前提要重写（整树判据仍然是必需的）',
    ).toEqual(expected);
  }, 60_000);
});
