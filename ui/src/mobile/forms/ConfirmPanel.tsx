/**
 * 破坏性动作的二次确认 —— 表单宿主的**地基**层（同桌面 `ConfirmDialog`）。
 *
 * 桌面节点屏用的是**原地二次确认**（`confirmTwice`：按钮翻红 + 「再点一次」），移动端换成叠一层
 * 确认面板，理由是触屏没有 hover 状态、翻红那一下在拇指底下被自己的手指挡住，而「再点一次」
 * 的第二击极易被读成误触重复。一层带标题与正文的面板把「要删的是哪一个、删了会怎样」说清楚。
 *
 * `onConfirm` **由回调自行关闭**（`dialog-store.ts#ConfirmPayload` 的既定语义）：不在这里自动 pop，
 * 否则「确认框里又开了一层」时 pop 顶层 ≠ pop 目标。
 */

import { type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import type { MobileConfirmPayload } from './form-store';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

export function ConfirmPanel({
  instanceId,
  payload,
}: {
  instanceId: string;
  payload: MobileConfirmPayload;
}): ReactElement {
  const { t } = useTranslation();
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  return (
    <FormSheet
      compact
      title={payload.title}
      onRequestClose={() => closeInstance(instanceId)}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={payload.confirmLabel ?? t('common.confirm')}
      submitDanger={payload.danger}
      onSubmit={() => void payload.onConfirm()}
    >
      <p className="m-form-msg">{payload.message}</p>
    </FormSheet>
  );
}
