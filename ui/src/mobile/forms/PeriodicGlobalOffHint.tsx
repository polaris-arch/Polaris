import type { ReactElement } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * 「全局周期测速已关闭，此订阅暂不会按周期测速」+ 就地打开全局开关的按钮（移动端订阅表单）。
 *
 * 出现条件由调用方判（全局关着、而本订阅的周期测速开着）。订阅自己的开关保持可操作、选择照常
 * 保存。警示色沿用表单里自动更新提示那一条的样式；桌面弹窗有自己的一份。
 * 保存失败怎么回显由表单自己定（它有常驻的提示行）。
 */
export function PeriodicGlobalOffHint({
  busy,
  onEnable,
}: {
  busy: boolean;
  onEnable: () => void;
}): ReactElement {
  const { t } = useTranslation();
  return (
    <div className="m-form-row" data-periodic-global-off="">
      <p className="m-form-hint m-form-auto-update-notice">{t('sub.periodicGlobalOff')}</p>
      <button type="button" className="m-form-btn" disabled={busy} onClick={onEnable}>
        {t('sub.periodicGlobalOn')}
      </button>
    </div>
  );
}
