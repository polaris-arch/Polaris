import { useTranslation } from 'react-i18next';
import { IdentityRepairFields } from '@/domain/identity-repair/IdentityRepairFields';
import { useIdentityRepair } from '@/domain/identity-repair/use-identity-repair';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

export function IdentityRepairPanel({ instanceId }: { instanceId: string }) {
  const { t } = useTranslation();
  const form = useIdentityRepair();
  return <FormSheet title={t('identityRepair.title')} closeLabel={t('common.close')} cancelLabel={t('common.close')}
    onRequestClose={() => { if (!form.busy) useMobileFormStore.getState().closeInstance(instanceId); }}
    closeLocked={form.busy} submitLabel={t('identityRepair.save')} submitDisabled={!form.canSave} onSubmit={() => void form.save()}>
    <IdentityRepairFields form={form} />
  </FormSheet>;
}
