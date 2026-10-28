import { useTranslation } from 'react-i18next';
import { toast } from '@/lib/error-handler';

/**
 * 「全局周期测速已关闭，此订阅暂不会按周期测速」+ 就地打开全局开关的按钮（桌面订阅弹窗）。
 *
 * 出现条件由调用方判（全局关着、而本订阅的周期测速开着）。订阅自己的开关保持可操作、选择照常
 * 保存：这一行只是说明「为什么开着却不测」，并让人不必离开弹窗就能改掉。
 * 警示色沿用弹窗已有的 `.warn-line`。移动端表单有自己的一份（`mobile/forms/`）。
 */
export function PeriodicGlobalOffHint({ busy, onEnable }: { busy: boolean; onEnable: () => Promise<void> }) {
  const { t } = useTranslation();
  return (
    <div className="warn-line" data-periodic-global-off="">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}>
        <path d="M12 9v4M12 17h.01M10.3 3.9L2 18a2 2 0 001.7 3h16.6a2 2 0 001.7-3L13.7 3.9a2 2 0 00-3.4 0z" />
      </svg>
      <span>{t('sub.periodicGlobalOff')}</span>
      <button
        type="button"
        className="btn ghost sm"
        disabled={busy}
        onClick={() => {
          onEnable().catch((error) => {
            console.error('[SubDialog] enable periodic speed test failed:', error);
            toast.error(t('common.saveFailed'));
          });
        }}
      >
        {t('sub.periodicGlobalOn')}
      </button>
    </div>
  );
}
