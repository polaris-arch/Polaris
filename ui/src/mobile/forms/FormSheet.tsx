/**
 * 移动端表单的**呈现外壳** —— 底部弹层（bottom sheet），不是居中弹窗。
 *
 * # 为什么是底部锚定
 *
 * 居中弹窗在手机上有两个具体后果，都不是审美问题：
 *  ① 软键盘一弹起，可视视口只剩上半屏，居中的弹窗被顶得只露出一条边，而正在编辑的那个输入框
 *     恰好在被遮住的那一半；
 *  ② 单手握持时拇指够得到的是屏幕下三分之一，而弹窗的主按钮在正中偏上。
 * 底部锚定把「主按钮」与「拇指」放在同一处，且键盘弹起时**面板自己变矮**（下面那条 `max-height`
 * 在移动根发布的可见视口内解析）而不是被推出屏幕。
 *
 * # 三段式：头（不滚）· 身（滚）· 脚（不滚）
 *
 * 脚里放主/次动作。它**不许跟着内容滚走** —— 一张 30 个字段的节点表单，提交键滚到看不见的地方
 * 等于没有提交键。故 `.m-form-foot` 是 sticky；底部只保留可见的手势条安全区，
 * 键盘遮住布局视口底端时不把原生 `--safe-b` 再叠在键盘上方。
 *
 * # 返回键
 *
 * 本组件**自己**登记进 `back-stack`（`useDismissableLayer`），与 `nodes/NodesScreenView.tsx#Sheet`
 * 同形：登记放在弹层组件身上而不是每个调用点上，以后新表单复用本组件就自动带上。
 * `onRequestClose` 与遮罩点击、右上角关闭是**同一个闭包** —— 三条离开路径必须落到同一处，
 * 否则「按返回」与「点关闭」会在脏态确认这件事上给出两种行为。
 */

import type { ReactElement, ReactNode } from 'react';
import { useDismissableLayer } from '../back-stack';
import { SheetHeading } from '../SheetHeading';

export interface FormSheetProps {
  readonly title: string;
  /** 短确认用内容高度；长编辑表单仍保留满高和固定头/脚。 */
  readonly compact?: boolean;
  /** 关闭意图的**唯一**出口：返回键 / 遮罩 / 关闭键三条路径共用它（脏态确认在调用方）。 */
  readonly onRequestClose: () => void;
  /** 关闭被锁（写操作进行中、关掉会留下孤儿 continuation）。锁上时遮罩与关闭键都不响应。 */
  readonly closeLocked?: boolean;
  readonly closeLabel: string;
  /** 已本地化的次动作（取消）。 */
  readonly cancelLabel: string;
  /** 已本地化的主动作。省略 ⇒ 只画取消（选择器类的面板没有提交动作）。 */
  readonly submitLabel?: string;
  readonly onSubmit?: () => void;
  readonly submitDisabled?: boolean;
  readonly submitDanger?: boolean;
  /** 表单级错误/提示，落在脚上方（不是 toast：一屏表单里「哪一格错了」要留在原地）。 */
  readonly notice?: { readonly tone: 'ok' | 'info' | 'err'; readonly text: string };
  readonly children: ReactNode;
}

export function FormSheet(props: FormSheetProps): ReactElement {
  /* 本组件只在「这一层开着」时被渲染 ⇒ 恒 `true`（同 `NodesScreenView#Sheet`）。
     🔴 登记必须发生在**渲染期**，理由见 `back-stack.ts` 头注：判据面是 `renderToStaticMarkup`，
     它不跑 effect，登记挪进 effect 会让「这一层吃不吃返回」没有任何门守得住。 */
  useDismissableLayer(true, () => {
    /* 🔴 锁住时返回 `false`（**不是**裸 `return`）。`dismissTop` 先出栈再调用，裸 `return`
       等于「这一层被静默摘掉、表单还开着」：下一次返回就落到应用级分支（回首页），再一次退出
       App，而那次写操作仍在后端跑、移动端没有恢复面。返回 `false` 让 `dismissTop` 把这一层
       压回原位，本次返回仍算被它吃掉 —— 用户看到「按了不关」，那正是「锁住」该有的表现。 */
    if (props.closeLocked === true) return false;
    props.onRequestClose();
    return true;
  });

  const locked = props.closeLocked === true;
  return (
    <div className="m-form-layer" data-compact={props.compact === true ? '' : undefined} role="dialog" aria-modal="true" aria-label={props.title}>
      <button
        type="button"
        className="m-form-scrim"
        aria-label={props.closeLabel}
        disabled={locked}
        onClick={props.onRequestClose}
      />
      <div className="m-form-panel">
        <div className="m-form-grip" aria-hidden />
        <SheetHeading title={props.title} onClose={props.onRequestClose} closeLabel={props.closeLabel} closeDisabled={locked}
          className="m-form-head" titleClassName="m-form-title" closeClassName="m-form-x" />
        <div className="m-form-body">{props.children}</div>
        {props.notice !== undefined && (
          <p className={`m-form-notice ${props.notice.tone}`} role="status">
            {props.notice.text}
          </p>
        )}
        <div className="m-form-foot">
          <button type="button" className="m-form-btn" disabled={locked} onClick={props.onRequestClose}>
            {props.cancelLabel}
          </button>
          {props.submitLabel !== undefined && (
            <button
              type="button"
              className={`m-form-btn primary${props.submitDanger === true ? ' danger' : ''}`}
              disabled={props.submitDisabled === true}
              onClick={props.onSubmit}
            >
              {props.submitLabel}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
