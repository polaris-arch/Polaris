import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import type { useIdentityRepair } from './use-identity-repair';

export function IdentityRepairFields({ form }: { form: ReturnType<typeof useIdentityRepair> }) {
  const { t } = useTranslation();
  const instance = useId();
  return <>
    <p>{t('identityRepair.explanation')}</p>
    {form.message && <p role="status">{t(form.message)}</p>}
    {form.preview?.groups.length === 0 && <p>{t('identityRepair.none')}</p>}
    {!form.done && form.preview?.groups.map((group, groupIndex) => <fieldset key={group.indices.join(',')} disabled={form.busy || !group.repairable}>
      <legend>{t('identityRepair.group', { count: groupIndex + 1 })}</legend>
      {!group.repairable && <p>{t('identityRepair.unsupported')}</p>}
      {group.records.map(record => <label key={record.index} style={{ display: 'block', padding: '8px 0', overflowWrap: 'anywhere' }}>
        <input type="radio" name={`${instance}-identity-keeper-${groupIndex}`} value={record.index}
          checked={form.keepers[groupIndex] === record.index}
          onChange={() => { form.setKeepers(old => ({ ...old, [groupIndex]: record.index })); form.setAcknowledged(false); }} />
        {t('identityRepair.record', { index: record.index + 1 })} {record.name} · {record.protocol}
        {record.address ? ` · ${record.address}${record.port ? `:${record.port}` : ''}` : ''}
      </label>)}
    </fieldset>)}
    {!form.done && Boolean(form.preview?.groups.length) && <label style={{ display: 'block', padding: '12px 0' }}>
      <input type="checkbox" checked={form.acknowledged} disabled={form.busy} onChange={event => form.setAcknowledged(event.target.checked)} />
      {t('identityRepair.confirmReferences')}
    </label>}
    {form.done && form.message === 'identityRepair.savedRefreshFailed' && <button type="button" disabled={form.busy} onClick={() => void form.retryRefresh()}>{t('identityRepair.reload')}</button>}
    {!form.preview && !form.busy && !form.done && <button type="button" onClick={() => void form.load()}>{t('identityRepair.reload')}</button>}
  </>;
}
