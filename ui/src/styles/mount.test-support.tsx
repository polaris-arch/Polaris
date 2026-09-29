/**
 * **真挂载树**：把要判的那段 markup 装进产品自己的外壳里，再交给 CSS 裁判。
 *
 * # 为什么要有这一层
 *
 * 2026-09-05 第五轮 B01/B02：裁判引擎本身一次都没说谎，被骗的全在它的**进料口**。
 * 上一版喂的是 `renderToStaticMarkup(<屏组件/>)` 与一段手搭的窗口外壳字符串，屏根之上一层都没有；
 * 而生产里屏住在 `#root > .m-shell > .m-scroll > .m-page` 底下、拖动带住在 `#root > .stage > .win`
 * 底下。于是把变异**往上抬一层**就整门全绿：
 *  · `mobile.css` 追加 `.m-page .mn-lat{ color: hsl(var(--err)) }` —— 生产里五个移动屏的每一条
 *    延迟读数（fast/mid/slow/dead/none）全部变红 —— 全量 237 文件 / 3957 断言 rc=0；
 *  · `index.css` 追加 `#root .main-chrome{ height: 24px }` —— 全绿；
 *  · 往真 `AppShell.tsx` 的 `.main-scroll` 之前插一个 40px 元素 —— 整门 8/8 全绿。
 * 正向对照证明手法不钝：同一条变异写在屏**内部**祖先上（`.mn-row .mn-lat`）⇒ 当场红 4 条。
 * **红绿分界线正好落在屏根上。**
 *
 * # 两道锁，缺一道洞就会回来
 *
 *  1. **祖先链由代码给**：外壳段直接渲染产品自己的外壳组件（`MobileShell` / `AppShell`），
 *     不手搭字符串；宿主段（`<html lang>` / `<body>` / `#root`）由裁判的 `hostSkeleton` 从
 *     `mobile.html` / `index.html` 解析。判据本身有门：`styles/mount-tree.test.tsx`
 *     （整树逐字节对拍生产根组件 + 浏览器实读祖先链对拍）。
 *  2. **裸字符串编译不过**：本模块的返回值是品牌类型 [`MountedMarkup`]，而 `measure` 在给了 `ctx`
 *     时只收它。修好一处不等于堵住这条路 —— 下一个人再写一处
 *     `measure({ ctx: 'mobile', html: renderToStaticMarkup(<屏/>) })`，洞就原样回来且没有任何东西会红。
 *     现在那样写 `tsc` 直接报错。要故意喂一棵残缺树（反向对照）就走
 *     [`unsafeUnmountedMarkup`] —— 绕得过去，但绕的时候必须写出来。
 */
import { renderToStaticMarkup } from 'react-dom/server';
import AppShell from '@/components/layout/AppShell';
import { DEFAULT_DESTINATION, type DestinationId } from '@/mobile/destinations';
import { MobileShell } from '@/mobile/MobileShell';
import type { MountedMarkup } from './css-oracle.test-support';

/** 屏的插入位。渲染外壳时先放这个标记，再把屏的 markup 原样换进去。 */
const SLOT = '<div data-mobile-screen-slot="1"></div>';

/**
 * 屏 markup → 生产外壳里的那份（`.m-shell > .m-scroll > .m-page > 屏`）。
 *
 * `active` 决定底部导航的当前项 —— 与被判的屏对齐，别让门跑在一个「导航停在别处」的状态上。
 */
export function inMobileShell(
  screenHtml: string,
  active: DestinationId = DEFAULT_DESTINATION,
): MountedMarkup {
  const shell = renderToStaticMarkup(
    <MobileShell active={active} onSelect={() => undefined}>
      <div data-mobile-screen-slot="1" />
    </MobileShell>,
  );
  const at = shell.indexOf(SLOT);
  if (at < 0)
    throw new Error(
      `外壳渲染里找不到屏槽位 \`${SLOT}\` —— \`MobileShell\` 的 children 装配方式变了，` +
        '这里的注入切法已经失效（继续跑下去喂进裁判的就不是生产那棵树了）。',
    );
  return (shell.slice(0, at) + screenHtml + shell.slice(at + SLOT.length)) as MountedMarkup;
}

/**
 * 桌面窗口外壳（`.stage > .win > .shell > {.side, .main}`）。
 *
 * 上一版这里写着「`AppShell` 依赖 store / i18n / tauri invoke，node 环境跑不起 SSR」，于是手搭了
 * 一段。那句话是错的：`useEffect` 在 SSR 里不执行，`os` 停在 `detectPlatform()` 的兜底值，
 * 而 per-os 档位本来就由裁判的 `rootAttrs['data-os']` 驱动，与这里渲染出的是哪一档无关。
 *
 * 内容区走生产默认的 `ScreenRouter`（`AppShell` 不给 children 时的那一支）—— 与 `App.tsx` 一样，
 * 故 `mount-tree.test.tsx` 能拿 `<App/>` 与它做逐字节对拍。要判某个屏内部的东西时再加参数，
 * 现在没有第二个消费者，不预留。
 */
export function inDesktopShell(): MountedMarkup {
  return renderToStaticMarkup(<AppShell />) as MountedMarkup;
}

/**
 * 🔴 **只给反向对照用**：把一段没挂在生产树上的 markup 强行标成「挂过了」。
 *
 * 存在的唯一理由是让「喂残缺树会被判据抓住」这件事本身可测（`styles/mount-tree.test.tsx` 的
 * 两条反向对照）。名字难看是故意的：它出现在生产判据里就是一个要被问的红旗。
 */
export const unsafeUnmountedMarkup = (html: string): MountedMarkup => html as MountedMarkup;
