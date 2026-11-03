import { useTranslation } from 'react-i18next';
import { Modal } from './Modal';
import type { ImpactPrompt } from './use-impact-confirmation';
import { ImpactApplySummary, impactApplyDisclosure } from '@/shared/ImpactApplySummary';

export function ImpactApplyDialog({ prompt, onDismiss, onConfirm }: {
  prompt: ImpactPrompt;
  onDismiss: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const disclosure = impactApplyDisclosure(prompt);
  return <Modal titleId="impact-apply-title" title={t('home.impactApplyTitle')}
    icon={<span aria-hidden>!</span>} onClose={onDismiss}
    footer={<>
      <button type="button" className="btn ghost" onClick={onDismiss} data-autofocus>{t('common.cancel')}</button>
      <button type="button" className="btn flow" disabled={disclosure.blocked}
        onClick={() => { if (!disclosure.blocked) onConfirm(); }}>{t('home.impactApplyConfirm')}</button>
    </>}>
    <ImpactApplySummary prompt={{ coreImpacts: prompt.coreImpacts, hostEffects: prompt.hostEffects }} />
  </Modal>;
}
