import { useTranslation } from 'react-i18next';
import { IdentityRepairFields } from '@/domain/identity-repair/IdentityRepairFields';
import { useIdentityRepair } from '@/domain/identity-repair/use-identity-repair';
import { Modal } from './Modal';
import { useDialogStore } from './dialog-store';

export function IdentityRepairDialog({ instanceId }: { instanceId: string }) {
  const { t } = useTranslation();
  const form = useIdentityRepair();
  const close = () => { if (!form.busy) useDialogStore.getState().closeInstance(instanceId); };
  return <Modal icon={<svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M7 7h10v10H7zM3 3h6M3 3v6M21 21h-6M21 21v-6" /></svg>} titleId={`identity-repair-${instanceId}`} title={t('identityRepair.title')} onClose={close}
    footer={<><button type="button" className="btn ghost" disabled={form.busy} onClick={close}>{t('common.close')}</button>
      {!form.done && <button type="button" className="btn flow" disabled={!form.canSave} onClick={() => void form.save()}>{t('identityRepair.save')}</button>}</>}>
    <IdentityRepairFields form={form} />
  </Modal>;
}
