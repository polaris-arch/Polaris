/**
 * 移动端屏外壳（spec：`~/docs/polaris/design/mobile-kit/component-specs/mobile-screen-shell.md`）。
 *
 * 它拥有五件事，**每一屏都不该再自己做一遍**：
 *  1. 安全区（状态栏 / 手势条 / 横屏两侧刘海）；
 *  2. 纵向滚动（内容滚，底部导航不滚）；
 *  3. 断点（容器查询选出 compact / medium / expanded，并把 `--page-inset` `--section-gap`
 *     `--card-padding` 三个响应标量解析一次，卡片继承，卡片**不得**自己写死 16）；
 *  4. section 节奏（`.m-page` 是 flex 列，间距 = `--section-gap`）；
 *  5. 底部导航停靠 + 待应用/待保存条槽位；
 *  6. **压栈深度**（哪个目的地正停在二级页）—— 它决定导航出不出现，见下面的 [`usePushedPage`]。
 *
 * # 三处结构是语义的一部分，挪了就变味
 *
 * **① 底部导航在滚动区之外。** spec「Bottom navigation docking」写明 docked outside the scroll area。
 * 放进滚动区就会随内容滚走，"固定导航"名不副实；放在外面还让滚动区的可用高度天然扣掉导航，
 * 内容不需要再补一段"导航高度 + 安全区"的假底部内边距（那种补法在导航高度变化时必然漂）。
 *
 * **② 待应用/待保存条挂外壳，不挂某一屏**（IA 裁定 #2）。移动端三个产生方俱在（节点变更 / 订阅刷新 /
 * 设置改动）。挂单屏 = 把桌面已经修过一次的 bug 重新引进来：用户在 A 屏改完切到 B 屏，待应用状态从
 * 视野里消失。桌面的实况是 `ui/src/components/layout/AppShell.tsx:234` 把 `<PendingChangesBar />`
 * 挂在全局 chrome 上，而且那是**迁移的结果**（它原来在首页）。
 *
 * **③ 槽位在导航之上、滚动区之下。** 与桌面同构（那边是 `.main-scroll` 与 `<StatusBar/>` 之间）。
 *
 * # 槽位里的两件东西由**外部注入**（2026-09-06 接线）
 *
 * `pendingBar` / `toastHost` 是两个 `ReactNode` 入参，本组件对它们的内容一无所知 —— 外壳不认识
 * store、不认识 `api`、不认识 `error-handler`，这条边界是判据能把整棵外壳单独渲染的前提。
 * 实体由 `MobileApp` 装配（`MobilePendingBar` / `MobileToaster`），理由与那两条腿为什么必须挂在
 * `MobileApp` 一致：它是这棵树上唯一既是组件、又在五个屏之上的位置。
 *
 * 桌面的 `PendingChangesBar` / `Toaster` **不能直接复用**：它们的外观全部落在 `.pending-bar` /
 * `.btn flow` / `.toast` 这些类上，而这些类住在 `prototype.css` / `components.css` —— 即桌面那条
 * 五层层叠链。移动入口只许走 `tokens.resolved.css`（契约 A1），`import` 它就等于把整条桌面 CSS
 * 拖进移动包，`mobile-entry.test.ts` 会当场红。**逻辑**则原样复用（`pending-bar-logic.ts` 的两条
 * 正交维度合成、`toast-queue.ts` 的队列语义），缺的只是一套移动端的呈现。
 *
 * # `TsExitWarning` 的挂点（IA 裁定 #3）：本批不实现，但结构得留得住
 *
 * 要移植的不变量是"**紧贴每一个能产生该状态的控件**"，不是"落在某一屏"。外壳因此**不**给它设全局槽位
 * ——设了就等于把它钉死成"全应用一处"，正好是裁定否掉的那个形态。屏内容区是自由的普通流，
 * 控件旁边随时插得进一条警示，这就是它需要的全部结构条件。
 */

import { useRef, useSyncExternalStore, type ReactElement, type ReactNode } from 'react';
import { BottomNavigation } from './BottomNavigation';
import type { DestinationId } from './destinations';

/*
 * ═══════════ 压栈状态：**每个目的地一格**，值 = 该栈顶上的二级页 id ═══════════
 *
 * # 为什么外壳要知道它（spec 的硬话）
 *
 * `mobile-screen-shell.md#Variants`：a pushed detail screen has a platform back affordance and
 * **no duplicate bottom navigation**；IA §2.4「Fixed and scrolling」再说一遍（bottom navigation on
 * the root, **and not on pushed pages**）。导航归外壳、压栈状态归屏，两者之间必须有一条通道 ——
 * 没有的话八个二级页上「返回」与五格导航同时在场，而点导航还会连栈位一起丢。
 *
 * # 为什么不是 context 上报、不是屏内 `useState`、也不是 zustand
 *
 *  · **context 只能父 → 子**，这里要的是子告诉父；
 *  · **effect 上报**（子挂载后回调外壳 `setState`）在本仓**没有任何门守得住**：vitest 跑 node 环境、
 *    刻意不装 jsdom，而 `renderToStaticMarkup` 不跑 effect ⇒ 判据里导航恒在，接线断了也不红；
 *  · **zustand** 差一格：v5 的 `useStore` 把 `getInitialState` 当 server snapshot
 *    （`node_modules/zustand/esm/react.mjs`），SSR 下读到的是**建store 那一刻**的值 ⇒ 判据先 `push`
 *    再渲染，读回来的仍是空栈，整条判据恒绿。这里用 React 自带的 `useSyncExternalStore`，
 *    并把 server snapshot 指成**同一个** live 读法：本仓的「SSR」只有判据在用，它要的正是当前值。
 *
 * 两条合起来 ⇒ 压栈状态在 render 期可读，判据能把栈写成 `dns` 再渲染**整棵生产树**，直接看导航在不在。
 * `MobileApp.tsx` 头注早写了提升条件：「等第二个消费者出现（底部导航要读该栈）再谈提升」——
 * 第二个消费者就是这条。栈住在外壳这一层还顺手消掉「切 tab 回来栈位复位」。
 *
 * 值是裸 `string`：外壳不认识、也不该认识任何一屏的页 id 枚举（认识就等于把屏的内容契约搬进外壳），
 * 读回时由屏自己收窄（设置屏走 `settings/settings-pages.ts` 的 `isSettingsPageId`）。
 */

let pushStack: Readonly<Partial<Record<DestinationId, string | null>>> = {};
const pushListeners = new Set<() => void>();

function subscribePushStack(onChange: () => void): () => void {
  pushListeners.add(onChange);
  return () => {
    pushListeners.delete(onChange);
  };
}

/** 屏上报自己停在哪一页。`null` = 回到根页（底部导航随之回来）。 */
export function setPushedPage(destination: DestinationId, page: string | null): void {
  pushStack = { ...pushStack, [destination]: page };
  for (const listen of [...pushListeners]) listen();
}

/** 读某个目的地的栈位。外壳与屏共用这一个入口，故两边不可能各持一份会漂移的真相。 */
export function usePushedPage(destination: DestinationId): string | null {
  const read = (): string | null => pushStack[destination] ?? null;
  return useSyncExternalStore(subscribePushStack, read, read);
}

export function MobileShell({
  active,
  onSelect,
  children,
  pendingBar,
  toastHost,
}: {
  active: DestinationId;
  onSelect: (id: DestinationId) => void;
  children: ReactNode;
  /** 待应用/待保存条。缺省 `undefined` ⇒ 槽位空 ⇒ `.m-pending-slot:empty` 零高度零 DOM 影响。 */
  pendingBar?: ReactNode;
  /** toast 宿主。停靠区的 `absolute` 子元素，压栈的二级页上照常在场（见下面那处注释）。 */
  toastHost?: ReactNode;
}): ReactElement {
  const scrollRef = useRef<HTMLDivElement>(null);
  /** 当前目的地是否停在二级页。只看**当前**这一格：别的目的地还压着栈不影响本屏的导航。 */
  const pushed = usePushedPage(active) !== null;

  /**
   * spec「Interaction」：Re-tapping the active item returns that stack to its root **or** scrolls it
   * to top（`bottom-navigation.md`「Interaction」）。两半之间是 **or**，而「回根页」那一半
   * **从导航上够不着**：同一份 spec 的「Variants」规定压栈的二级页不画底部导航，本组件下面那行
   * `!pushed && <BottomNavigation …>` 正是它的落实 ⇒ 栈非空时这个回调根本没有触发入口。
   * 故这里兑现的「滚到顶」就是可达的全部，不是半截（此前的注释把它记成了欠账，是误记）。
   * 在这条路上补一句 `setPushedPage(active, null)` 只会得到一次恒为 no-op 的写 + 一轮多余的订阅通知。
   *
   * 「退掉栈顶那一层」今天有自己的入口：系统返回键（`back-stack.ts` → `MobileApp` 的返回处理器）。
   * 滚动容器归外壳、屏自己拿不到它，故滚到顶这条只能落在这里。
   */
  const handleSelect = (id: DestinationId): void => {
    if (id === active) {
      scrollRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
      return;
    }
    onSelect(id);
  };

  return (
    <div className="m-shell">
      {/* 不滚动的边界承接断点和屏内 fixed 弹层；内层只滚动正文。字号继续继承根缩放。 */}
      <div className="m-screen-viewport">
        <div className="m-scroll" ref={scrollRef}>
          <div className="m-page">{children}</div>
        </div>
      </div>
      {/* 停靠区（不滚）：待应用条槽位在上、底部导航在下。 */}
      <div
        className="m-dock"
        /* 压栈页上导航不渲染，但停靠区**本体留着**：`--safe-b` 那段手势条留白是它给的，
           整块拿掉会让二级页的最后一行压在手势条底下。它此时不该再画成一根条 ⇒ 底色与顶分隔
           线就地取消（外壳 CSS 已定稿，这两笔只能内联；`.m-pending-slot:empty` 那条仍生效）。 */
        style={pushed ? { background: 'none', borderTop: 'none' } : undefined}
      >
        {/* toast 宿主。**停靠区的 `absolute` 子元素**（`bottom:100%`，见 `mobile.css`）：
            ① 不占布局 ⇒ toast 进出场不会把滚动区挤得跳一下；
            ② 压栈的二级页上导航不渲染、**停靠区本体仍在**（`--safe-b` 那段留白是它给的），
               故宿主跟着留下，此时贴的是手势条上沿而不是导航上沿 —— 不需要第二套定位。 */}
        {toastHost}
        {/* 待应用/待保存条槽位（IA 裁定 #2）。空槽零 DOM 零高度，与桌面那条"差集为空即 return null"
            同一口径 —— 条自己在无事可做时返 null，槽位随之 `:empty`。 */}
        <div className="m-pending-slot" data-slot="pending-changes">
          {pendingBar}
        </div>
        {/* spec「Variants」/ IA §2.4：压栈的二级页上不重复画底部导航（栈状态见 [`usePushedPage`]）。 */}
        {!pushed && <BottomNavigation active={active} onSelect={handleSelect} />}
      </div>
    </div>
  );
}
