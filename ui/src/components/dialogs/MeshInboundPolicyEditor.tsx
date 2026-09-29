/** Desktop mesh inbound policy editor; the pure submit boundary is shared with mobile. */
import { useTranslation } from 'react-i18next';
import type { MeshInboundGrant, MeshInboundPolicy } from '@/contracts/types';
import { Csel } from './Csel';
export { normalizeMeshInboundPolicy, meshInboundPolicyError, applyMeshInboundPolicy } from './mesh-inbound-policy';

const split = (text: string): string[] => text.split(/[\n,]/).map((part) => part.trim());
const emptyGrant = (): MeshInboundGrant => ({ sourceCidrs: [], network: 'tcp', ports: [], target: 'local' });

export function MeshInboundPolicyEditor({ value, onChange, idPrefix }: {
  value: MeshInboundPolicy | undefined;
  onChange: (next: MeshInboundPolicy | undefined) => void;
  idPrefix: string;
}) {
  const { t } = useTranslation();
  const mode = value?.mode ?? 'legacy';
  const rules = value?.mode === 'allowlist' ? value.rules : [];
  const edit = (index: number, next: MeshInboundGrant) => onChange({
    mode: 'allowlist', rules: rules.map((rule, i) => i === index ? next : rule),
  });
  return (
    <div className="fld">
      <div className="fld-l">{t('meshInbound.title')}</div>
      <Csel
        id={`${idPrefix}-mesh-inbound-mode`}
        ariaLabel={t('meshInbound.title')}
        value={mode}
        options={[
          { value: 'legacy', label: t('meshInbound.legacy') },
          { value: 'block', label: t('meshInbound.block') },
          { value: 'allowlist', label: t('meshInbound.allowlist') },
        ]}
        onChange={(next) => onChange(next === 'legacy' ? undefined : next === 'block' ? { mode: 'block' } : { mode: 'allowlist', rules: [] })}
      />
      <div className="card-sub form-inline-note">{t('meshInbound.systemBoundary')}</div>
      <div className="card-sub form-inline-note">{t('meshInbound.pendingApply')}</div>
      {mode === 'allowlist' && <>
        <div className="card-sub form-inline-note">{t('meshInbound.emptyBlocks')}</div>
        {rules.map((rule, index) => <div key={index} className="fld">
          <div className="fld-l">{t('meshInbound.rule', { number: index + 1 })}</div>
          <label className="fld-l" htmlFor={`${idPrefix}-mesh-source-${index}`}>{t('meshInbound.source')}</label>
          <input id={`${idPrefix}-mesh-source-${index}`} className="input mono" value={rule.sourceCidrs.join(', ')}
            onChange={(event) => edit(index, { ...rule, sourceCidrs: split(event.target.value) })}
            placeholder={t('meshInbound.sourcePlaceholder')} />
          <div className="fld-l">{t('meshInbound.network')}</div>
          <Csel id={`${idPrefix}-mesh-network-${index}`} ariaLabel={t('meshInbound.network')}
            value={rule.network} onChange={(network) => edit(index, { ...rule, network: network as MeshInboundGrant['network'] })}
            options={[{ value: 'tcp', label: 'TCP' }, { value: 'udp', label: 'UDP' }, { value: 'both', label: 'TCP + UDP' }]} />
          <label className="fld-l" htmlFor={`${idPrefix}-mesh-ports-${index}`}>{t('meshInbound.ports')}</label>
          <input id={`${idPrefix}-mesh-ports-${index}`} className="input mono" value={rule.ports.join(', ')}
            onChange={(event) => edit(index, { ...rule, ports: split(event.target.value) })}
            placeholder={t('meshInbound.portsPlaceholder')} />
          <div className="fld-l">{t('meshInbound.target')}</div>
          <Csel id={`${idPrefix}-mesh-target-${index}`} ariaLabel={t('meshInbound.target')}
            value={rule.target} onChange={(target) => edit(index, { ...rule, target: target as MeshInboundGrant['target'], targetCidrs: undefined })}
            options={[{ value: 'local', label: t('meshInbound.local') }, { value: 'forward', label: t('meshInbound.forward') }]} />
          {rule.target === 'forward' && <>
            <label className="fld-l" htmlFor={`${idPrefix}-mesh-dest-${index}`}>{t('meshInbound.destCidrs')}</label>
            <input id={`${idPrefix}-mesh-dest-${index}`} className="input mono" value={(rule.targetCidrs ?? []).join(', ')}
              onChange={(event) => edit(index, { ...rule, targetCidrs: split(event.target.value) })}
              placeholder={t('meshInbound.destCidrsPlaceholder')} />
            <div className="card-sub form-inline-note">{t('meshInbound.forwardBoundary')}</div>
          </>}
          <button type="button" className="btn ghost sm" onClick={() => onChange({ mode: 'allowlist', rules: rules.filter((_, i) => i !== index) })}>
            {t('meshInbound.remove')}
          </button>
        </div>)}
        <button type="button" className="btn ghost sm" onClick={() => onChange({ mode: 'allowlist', rules: [...rules, emptyGrant()] })}>
          {t('meshInbound.add')}
        </button>
      </>}
    </div>
  );
}
