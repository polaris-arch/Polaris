/**
 * 一个**可折叠的字段分组** —— 移动端表单里页签的等价物。
 *
 * # 为什么是折叠段而不是页签（唯一一处刻意的形态偏离，四张表共用同一条理由）
 *
 * 桌面对字段多的表开页签（`FormTabs`）。手机上页签把一张本来就要纵向滚的表切成
 * 「看不见另一半」—— 校验失败要先跳页签再滚动，而错误就在同一根滚动轴上。这里改成逐组折叠：
 * 初始展开由各表单根据必填主路径、已配置选项决定；校验失败时由调用方
 * **自动展开出错的那一组**。本组件不猜字段是否必填，也不持有展开状态。
 * 页签与折叠段承载的是同一份分区（`node-spec#nodeFormGroups` / `mesh-form-layout#groupWgFields`
 * / `#groupTsFields`），没有第二套分组真值。
 *
 * # `open` 由调用方持有
 *
 * 不是本组件的内部 state：校验失败时要能**从外面掰开它**。自持状态的折叠段做不到这件事，
 * 而那正是这个形态存在的理由。
 *
 * `onToggle` 收到的是**这一段的容器元素**（不是那颗组头）：展开后要滚进视区的是整段。
 * 「点的那一刻字段还没渲染」这条由调用方的 `useRevealAfterCommit` 负责，不在这里。
 */

import { useRef, type ReactElement, type ReactNode } from 'react';

export function FormGroup({
  title,
  groupId,
  open,
  onToggle,
  children,
}: {
  readonly title: string;
  /** 可选稳定定位键，供校验失败后在提交完成时滚到这一组。 */
  readonly groupId?: string;
  readonly open: boolean;
  readonly onToggle: (section: HTMLElement | null) => void;
  readonly children: ReactNode;
}): ReactElement {
  const box = useRef<HTMLElement | null>(null);
  return (
    <section className="m-form-group" data-form-group={groupId} ref={box}>
      <button
        type="button"
        className="m-form-group-h"
        aria-expanded={open}
        onClick={() => onToggle(box.current)}
      >
        <span>{title}</span>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
          <path d={open ? 'M6 15l6-6 6 6' : 'M6 9l6 6 6-6'} />
        </svg>
      </button>
      {open && <div className="m-form-group-b">{children}</div>}
    </section>
  );
}
