import { useTranslation } from 'react-i18next';
import { Modal } from './Modal';
import { restartReasonKeys, type RestartPrompt } from './restart-apply-reasons';

export type { RestartPrompt } from './restart-apply-reasons';

/** Desktop presentation of the shared restart decision. Dismissal never applies or saves. */
export function RestartApplyDialog({ prompt, onDismiss, onConfirm }: {
  prompt: RestartPrompt;
  onDismiss: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Modal titleId="restart-apply-title" title={t('home.restartApplyTitle')}
      icon={<span aria-hidden>!</span>} onClose={onDismiss}
      footer={<>
        <button type="button" className="btn ghost" onClick={onDismiss} data-autofocus>
          {t('home.restartApplyKeepConnection')}
        </button>
        <button type="button" className="btn flow" onClick={onConfirm}>
          {t('home.restartApplyConfirm')}
        </button>
      </>}>
      <p>{t('home.restartApplyDescription')}</p>
      <ul>{restartReasonKeys(prompt.reasons).map((key) => <li key={key}>{t(key)}</li>)}</ul>
    </Modal>
  );
}
