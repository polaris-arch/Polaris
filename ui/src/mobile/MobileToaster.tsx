/**
 * 移动端的 **toast 宿主** —— `lib/error-handler` 那个门面在移动入口上的出口。
 *
 * # 它解锁的不是一个新能力，是 24 处**已经写好的**调用
 *
 * `lib/error-handler.ts:62-71` 的 `toast` 门面在**未注入实现时落 console**，而接线之前全仓唯一的
 * `setToastImpl` 注入点是 `components/layout/Toaster.tsx`，它只挂在桌面 `AppShell` 上。
 * 于是移动端生产源码里那 24 处 `toast.success/info/warning/error`（全在
 * `screens/rules/RulesScreen.tsx`：规则重排成功、保存失败、复制、删除、地区切换、
 * 反向分流开关、规则资源更新/取消/重置…）**一条都到不了用户眼前**。
 * 那不是「没写反馈」，是「写了反馈但它是静音的」—— 比没写更难被发现。
 * 数量走精确棘轮，见 `half-truth-facts.test.ts` C 组。
 *
 * # 为什么不直接复用桌面 `components/layout/Toaster.tsx`
 *
 * 不是排期，是**契约冲突**（同 `MobileShell` 头注对待应用条写的那条）：它的外观全部落在
 * `.toast` / `.toast-desc` / `.toast-action` 这些类上，而这些类住在 `prototype.css` /
 * `index.css` —— 即桌面那条五层层叠链。移动入口只许走 `tokens.resolved.css`（契约 A1），
 * `import` 它就等于把整条桌面 CSS 拖进移动包，`mobile-entry.test.ts` 会当场红。
 * 它还 portal 进最顶层 `<dialog>`（桌面弹窗层的 top-layer 语义），而移动端没有那一层。
 *
 * **队列语义则原样复用**（`components/layout/toast-queue.ts`）：同 key 原地更新、栈上限 2、
 * 溢出优先挤非 sticky、有动作必有出路。那三条判定是纯逻辑、与呈现无关，另起一份只会让
 * 「一轮 50 个节点的测速刷 50 条」这种退化在移动端再发生一次。
 *
 * # 停靠位置：**底部导航之上**，且不占布局
 *
 * 宿主是 `.m-dock` 的 `position:absolute; bottom:100%` 子元素 —— 贴在停靠区上沿向上生长。
 * 两条后果都是要的：
 *  · **压栈的二级页上照常可见**。`MobileShell` 在压栈时不渲染 `BottomNavigation`，但**停靠区本体
 *    留着**（`--safe-b` 那段手势条留白是它给的），故宿主跟着留下，只是此时贴的是手势条上沿而不是
 *    导航上沿 —— 位置随停靠区高度自动走，不需要第二套定位。
 *  · **不占布局**：`absolute` 不参与 flex 排布 ⇒ toast 进出场不会把滚动区挤得跳一下。
 *
 * `pointer-events` 与桌面同口径：栈整体 `none`（不挡操作），带动作/关闭的那一条自己收回来。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { setToastImpl, type ToastOptions } from '@/lib/error-handler';
import {
  autoDismissMs,
  LEAVE_MS,
  toastListKey,
  upsertToast,
  type ToastEntry,
  type ToastKind,
} from '@/components/layout/toast-queue';

/** 一次入栈请求。抽成类型是为了让 [`installMobileToastHost`] 的注入面能被单测直接驱动。 */
export type ToastPush = (
  msg: string,
  kind: ToastKind,
  desc?: string,
  opts?: ToastOptions,
) => void;

/**
 * 把门面接到给定的入栈函数上，返回**恢复 console 兜底**的闭包。
 *
 * 抽成不吃 React 的纯函数，理由与 `config-sync.ts` 拆两个导出完全相同：本仓 vitest 是
 * `environment:'node'`、无 jsdom ⇒ effect 体在 `renderToStaticMarkup` 下不执行，写在组件里的注入
 * 就一条**行为**判据都写不出来。拆开之后判据可以真的调 `toast.error(...)`，看它有没有走到这里。
 *
 * 四条通道的映射逐字同桌面 `Toaster.tsx:130-136`（含 `error` 的第二参 description 要接上），
 * 差别只有：这里没有 `''` 这一档的第四种配色可造 —— 原型只有三档，`info`/`warning` 走基础样式。
 */
export function installMobileToastHost(push: ToastPush): () => void {
  setToastImpl({
    success: (m, o) => push(m, 'ok', undefined, o),
    error: (m, d, o) => push(m, 'err', d, o),
    info: (m, o) => push(m, '', undefined, o),
    warning: (m, o) => push(m, '', undefined, o),
  });
  // 门面是模块级单例；卸载后仍保留 push 闭包会写进一个已卸载组件。恢复 console 兜底，
  // StrictMode 重挂载时下一轮 effect 立即重新注入当前实例（同桌面）。
  return () => setToastImpl({});
}

export function MobileToaster(): ReactElement | null {
  const [items, setItems] = useState<ToastEntry[]>([]);
  const seq = useRef(0);
  /** 卸载时清掉全部在飞定时器（StrictMode 双挂载 / 热重载下防泄漏）。 */
  const timers = useRef<Set<ReturnType<typeof setTimeout>>>(new Set());

  /** 显式关闭：走与自动消失相同的离场动画，再按本次 entry id 精确移除。 */
  const dismiss = (id: number): void => {
    setItems((prev) => prev.map((it) => (it.id === id ? { ...it, leaving: true } : it)));
    const timer = setTimeout(() => {
      timers.current.delete(timer);
      setItems((prev) => prev.filter((it) => it.id !== id));
    }, LEAVE_MS);
    timers.current.add(timer);
  };

  useEffect(() => {
    const later = (fn: () => void, ms: number): void => {
      const id = setTimeout(() => {
        timers.current.delete(id);
        fn();
      }, ms);
      timers.current.add(id);
    };

    const push: ToastPush = (msg, kind, desc, opts) => {
      const id = (seq.current += 1);
      const entry: ToastEntry = {
        id,
        dedupeKey: opts?.key,
        msg,
        desc: (desc ?? opts?.description)?.trim() || undefined,
        kind,
        sticky: opts?.sticky === true,
        actions: opts?.actions,
        dismiss: opts?.dismiss,
        // 无 jsdom 的判据环境里没有 rAF 可依赖，且移动端进场动画由 CSS `animation` 一次性播完，
        // 不靠「下一帧加 .show」那套两帧语义 ⇒ 直接置 true。桌面那套是为了兼容原型的 transition。
        shown: true,
        leaving: false,
      };
      setItems((prev) => upsertToast(prev, entry));
      // `null` = sticky，不起淡出定时器。判定在 `toast-queue`（策略必须可单测）。
      const ttl = autoDismissMs(entry);
      if (ttl === null) return;
      later(() => {
        setItems((prev) => prev.map((it) => (it.id === id ? { ...it, leaving: true } : it)));
        later(() => setItems((prev) => prev.filter((it) => it.id !== id)), LEAVE_MS);
      }, ttl);
    };

    const restore = installMobileToastHost(push);
    const pending = timers.current;
    return () => {
      restore();
      pending.forEach(clearTimeout);
      pending.clear();
    };
  }, []);

  if (items.length === 0) return null;

  return (
    <div className="m-toast-host" role="status" aria-live="polite">
      {items.map((it) => (
        <div
          /* 不是 `it.id`：同 key 更新会换新 id，用 id 作 React key 等于每次刷新都卸载重挂
             ⇒ 进场动画重播。见 `toast-queue.ts` 的 `toastListKey`。 */
          key={toastListKey(it)}
          className={`m-toast${it.kind === 'ok' ? ' m-toast-ok' : it.kind === 'err' ? ' m-toast-err' : ''}${
            it.leaving ? ' m-toast-leaving' : ''
          }`}
        >
          <div className="m-toast-body">
            <div className="m-toast-msg">{it.msg}</div>
            {it.desc !== undefined && <div className="m-toast-desc">{it.desc}</div>}
          </div>
          {it.actions !== undefined && it.actions.length > 0 && (
            <div className="m-toast-actions">
              {it.actions.map((action) => (
                <button
                  key={action.label}
                  type="button"
                  className="m-toast-action"
                  onClick={action.onClick}
                >
                  {action.label}
                </button>
              ))}
            </div>
          )}
          {it.dismiss !== undefined && (
            <button
              type="button"
              className="m-toast-close"
              aria-label={it.dismiss.label}
              onClick={() => dismiss(it.id)}
            >
              <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                <path d="M18 6 6 18M6 6l12 12" />
              </svg>
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
