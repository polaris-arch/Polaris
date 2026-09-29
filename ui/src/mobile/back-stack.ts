/**
 * 「可关闭层」的 LIFO 登记表 —— 移动端系统返回键要按顺序关掉的那一叠东西。
 *
 * # 它存在的理由：今天按返回 = 退出 App
 *
 * `tauri-2.11.5/mobile/android/src/main/java/app/tauri/AppPlugin.kt:28-46`：系统返回经
 * `OnBackPressedDispatcher` 交给 AppPlugin 那个**唯一**回调，而它的分支是
 * 「没人监听 `back-button` ⇒ `canGoBack() ? goBack() : activity.onBackPressed()`」——
 * WebView 没有历史（本仓无路由库），于是 `finish()`，**任何一屏、任何一层弹层上按返回都直接退出**。
 * 一叠打开着的弹层、一个压着的二级页、一个批选态，全都没有机会拦下这一下。
 *
 * # 为什么是一张**集中**的表，而不是逐屏各接一个 `back-button` 监听
 *
 * `Plugin.trigger` 把事件**广播给所有 channel**：逐屏各注册一个监听，一次返回会同时关掉三层。
 * 且五个主目的地是 `MobileApp.tsx:31` 一个 `useState`（无栈无历史），22 个可返回面里
 * 只有设置屏二级页上报到外壳 —— 没有一个现成的地方能回答「最上面那一层是谁」。这张表就是那个答案。
 *
 * # 🔴 登记必须发生在**渲染期**，不许放进 `useEffect`
 *
 * 这条不是省事，是**判据决定的**（与 `MobileShell.tsx:56-66` 那段论证一字不差地同构）：
 * 本仓 vitest 跑 `environment: 'node'`、刻意不装 jsdom，判据面是 `renderToStaticMarkup`，
 * 而**它不跑 effect**。登记若挂在 effect 上，判据渲染完看到的永远是空栈 ⇒
 * 「这一层吃不吃返回」这件事**没有任何门守得住**，接线断了也不红。
 * 同理不用 zustand：v5 的 `useStore` 把 `getInitialState` 当 server snapshot，SSR 下读到的是
 * 建 store 那一刻的值，判据先 push 再渲染照样读回空栈。
 *
 * 代价如实记在这里：渲染期写模块级变量是「不纯渲染」。两条实际风险与它们的处置——
 *  · **StrictMode 双渲染**（`MobileMain.tsx` 开着）：`pushDismissable` 按 key 幂等，
 *    第二次渲染只就地换掉闭包，不会叠成两层；key 取 `useId()`，同一个组件实例上稳定。
 *  · **被丢弃的渲染**（渲染了却没提交）：那一层会留在表上，表现是「按一次返回什么都没发生」。
 *    今天移动端这棵树没有 Suspense 边界、没有 transition，但**有一条是真实可达的**：
 *    `MobileMain.tsx` 把 `MobileApp` 套在 `ErrorBoundary` 里，而被边界接住的那一趟渲染同样
 *    是被丢弃的渲染 —— 子树若在二级页/弹层开着时抛错，兜底页上第一下返回会被这个幽灵层吃掉。
 *    此时应用已经在故障态（`renderer-recovery` 随后 reload），实害仅止于此，故不为它加机制；
 *    真要收的话正确的修法是给 `useDismissableLayer` 补一条提交后校验，
 *    不是把登记挪回 effect（挪回去就同时失去上面那道门）。
 *
 * # 分工
 *
 * 本模块**只管「有哪些层、谁在最上面」**，不认识目的地、不认识原生桥。
 * 「关完了之后往哪走」（回首页 / 交还系统）住在 `MobileApp.tsx` —— 它是这棵树上唯一的应用级接线点。
 */

import { useEffect, useId, useRef, useSyncExternalStore } from 'react';

/**
 * 一层。`dismiss` 是**可变**字段：重渲染时就地换掉闭包，层序不动 ——
 * 换成整个对象重建会让「退订闭包按对象身份摘」这件事失效，而按 key 摘则会在
 * 「同一个 key 先退订、后又被别人登记」时误摘掉别人那一层。
 */
interface DismissableLayer {
  readonly key: string;
  dismiss: DismissFn;
}

/**
 * 一层的关闭闭包。**返回 `false` = 「我拒绝关闭，但这一下返回已被我吃掉」**。
 *
 * 这条返回值是 2026-09-06 三条复审同时点名的那个缺陷的修法：`dismissTop` **先出栈再调用**
 * （见下），而写操作进行中的表单（`FormSheet` 的 `closeLocked`）此前在闭包里直接 `return` ——
 * 层已经从表上摘了、表单还开着，于是第二下返回落到应用级分支（回首页），第三下退出 App，
 * 而那次创建/导入仍在后端跑且移动端没有恢复面。
 *
 * 返回 `false` 让「我没关」这件事**可表达**：`dismissTop` 据此把这一层压回原位。
 * 返回 `void` / `true` = 已关闭（绝大多数层，写成 `() => setOpen(false)` 即可，不必显式 return）。
 */
type DismissFn = () => boolean | void;

/** 栈底 → 栈顶。整体替换（不原地 push/pop），故 `useSyncExternalStore` 的快照可以直接取 `length`。 */
let layers: readonly DismissableLayer[] = [];

const layerListeners = new Set<() => void>();

/**
 * 唤醒订阅者。**排到微任务**再唤醒，不同步调。
 *
 * 登记与退订发生在**别人的渲染期**（见文件头注那条「必须发生在渲染期」）：同步唤醒
 * 等于「渲染 A 的过程中更新 B」。今天生产侧没有消费者所以撞不上，但 [`useDismissableDepth`]
 * 是导出的公共面，第一个消费者（比如「有层时给停靠区加遮罩」）出现那天就会吃到 React 的
 * `Cannot update a component while rendering a different component`，并且每次层数变化多排一轮渲染。
 * 判据读的是同步的 `getSnapshot`（SSR 下根本不订阅），故推迟唤醒不改变任何一道门的结论。
 */
function notifyLayers(): void {
  if (layerListeners.size === 0) return;
  queueMicrotask(() => {
    for (const listen of [...layerListeners]) listen();
  });
}

function subscribeLayers(onChange: () => void): () => void {
  layerListeners.add(onChange);
  return () => {
    layerListeners.delete(onChange);
  };
}

/**
 * 登记一层，返回**幂等**的退订闭包。
 *
 * 同 `key` 再登记 = 同一个组件重渲染：只就地刷新 `dismiss`（否则 `dismissTop` 调到的是上一帧的
 * `onClose`，闭包里捕获的是过期 state），**层序保持不变** —— 重渲染不该把一层顶到最上面。
 *
 * ⚠️ 由此得出的一条前提，记在这里免得下一个人踩：**层序 = 首次激活的时序**，不是渲染时序。
 * 今天两处「浅层后激活」的路径都自愈，靠的都是同一个偶然前提 —— 那层弹层在同一批里被关掉：
 * 规则屏 DNS 溢出面板走 `setDnsViewsSheet(false)` + `setPage(…)` 同批，节点屏「更多」面板里的
 * 批选入口走 `SheetRow`（先 `onSelect()` 再 `onDone()`）。哪天有人让弹层在推子页 / 进模式态时
 * **留着**，层序就反了：上面那层活着，返回却先关掉底下那层。真要收的话，收在
 * 「重新激活时置顶」这条语义上（本函数分不出「重渲染」与「重新激活」，故不能在这里判）。
 */
export function pushDismissable(key: string, dismiss: DismissFn): () => void {
  const existing = layers.find((l) => l.key === key);
  const layer = existing ?? { key, dismiss };
  if (existing !== undefined) {
    existing.dismiss = dismiss;
  } else {
    layers = [...layers, layer];
    notifyLayers();
  }
  return () => {
    if (!layers.includes(layer)) return; // 已经被 dismissTop 弹掉或退订过：幂等
    layers = layers.filter((l) => l !== layer);
    notifyLayers();
  };
}

/**
 * 弹掉最上面一层并执行它的 `dismiss`；空栈返 `false`。
 *
 * **先出栈再调用**：`dismiss` 会 `setState` → 重渲染 → 那一层的渲染期退订；先调用的话
 * 这两步会撞在一起。纯函数形态（不碰 DOM、不碰 React）是刻意的 —— 判据直接驱动得动它。
 *
 * # 🔴 拒绝关闭的那一层要**压回原位**（2026-09-06 三条复审同时点名）
 *
 * 「先出栈再调用」与「有些层此刻关不得」（写操作进行中的表单）撞在一起：闭包直接 `return`
 * 的话，层已经摘了而弹层还在屏上，第二下返回就落到应用级分支（回首页 → 退出 App）。
 * 故 `dismiss` 返回 `false` 时把它压回栈顶，**但本次仍返回 `true`** —— 这一下返回确实
 * 被这一层吃掉了（用户看到的是「按了没反应」，那正是「不许关」的正确表现），
 * 不该继续往下落到「回首页」。压回前先看它是不是已经自己回来了（`dismiss` 里若又渲染了一趟，
 * 渲染期登记会按 key 找到同一个对象），避免叠成两层。
 */
export function dismissTop(): boolean {
  const top = layers[layers.length - 1];
  if (top === undefined) return false;
  layers = layers.slice(0, -1);
  notifyLayers();
  if (top.dismiss() === false && !layers.includes(top)) {
    layers = [...layers, top];
    notifyLayers();
  }
  return true;
}

/**
 * 把登记表整个清空。
 *
 * **今天唯一的消费者是判据**，如实记在这里：`dismissTop` 之外没有别的出栈口，而
 * 「拒绝关闭的层会被压回原位」（见上）意味着一张含锁定层的表**排不空** —— 判据要在两个
 * 用例之间回到干净状态就必须有这个口。不写成 test-only 文件里的私货，是因为 `layers` 是本模块
 * 的私有状态，外面够不着；给它一个具名、有文档的复位口，好过让判据去猜内部结构。
 *
 * 生产侧不需要它：整棵树被丢弃的唯一路径是 `renderer-recovery` 的整页 reload，那时模块状态
 * 连同页面一起没了。哪天真出现「不 reload 的整树重建」，那条路径应当调用本函数。
 */
export function clearDismissables(): void {
  if (layers.length === 0) return;
  layers = [];
  notifyLayers();
}

/**
 * 当前叠了几层。
 *
 * 今天**生产侧没有消费者**（返回键的处理器直接调 `dismissTop`，没人需要因层数变化而重渲染），
 * 唯一的消费者是判据：它要一个**渲染期**读得到的口子，才能证明「登记确实发生在渲染期」这件事本身。
 * 写成 hook 而不是裸函数，是为了让判据走的是生产那条订阅路径；server snapshot 与 live 读法
 * 指向同一个函数，理由同 `MobileShell.usePushedPage`。
 *
 * 生产侧要用它是**可以的**（不是「测试专用」）：唤醒已经推迟到微任务（见 [`notifyLayers`]），
 * 渲染期登记不会去更新一个正在渲染的别的组件。
 */
export function useDismissableDepth(): number {
  const read = (): number => layers.length;
  return useSyncExternalStore(subscribeLayers, read, read);
}

/**
 * 组件侧的登记口：`active` 为真时这一层吃返回，`dismiss` 就是它已有的关闭闭包。
 *
 * 放在**弹层组件自己**身上（四套弹层各一处、二级页各一处），不是 19 个调用点上：
 * 以后新弹层复用同一个组件就自动带上，而「忘了登记」的表现降级成
 * 「这一层不吃返回」，不再是「整个 App 退出」。
 */
export function useDismissableLayer(active: boolean, dismiss: DismissFn): void {
  const key = useId();
  const latest = useRef(dismiss);
  latest.current = dismiss;
  /* 稳定的转发器：登记进表的函数身份不随重渲染变，真正调用的永远是最后一帧的闭包。
     **返回值必须透传**：吞掉它就等于把「我拒绝关闭」这条信息丢在这里，`dismissTop` 再也
     压不回那一层（见 [`DismissFn`]）。 */
  const forward = useRef<DismissFn | null>(null);
  forward.current ??= () => latest.current();
  const off = useRef<(() => void) | null>(null);

  // 渲染期登记 / 退订（见文件头注「必须发生在渲染期」）。
  if (active) {
    off.current = pushDismissable(key, forward.current);
  } else {
    off.current?.();
    off.current = null;
  }

  useEffect(() => {
    if (!active) return undefined;
    /* StrictMode 把 effect 跑成 setup → cleanup → setup：第一次 cleanup 会把渲染期登记的那一层摘掉，
       必须由第二次 setup 补回来，否则组件还挂着而层已经不在表上（返回键落空）。
       `pushDismissable` 幂等，故非 StrictMode 下这次调用只是刷新闭包。 */
    return pushDismissable(key, forward.current as DismissFn);
  }, [active, key]);
}
