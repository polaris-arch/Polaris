import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { MeshInboundGrant, MeshInboundPolicy, ServerConfig } from '@/contracts/types';
import { revealElement, useRevealAfterCommit } from '@/components/reveal';
import { MobileInfo } from '../MobileInfo';
import { MobileSelect } from '../MobileSelect';
import { FormGroup } from './FormGroup';

const split = (text: string): string[] => text.split(/[\n,]/).map((part) => part.trim());
const emptyGrant = (): MeshInboundGrant => ({ sourceCidrs: [], network: 'tcp', ports: [], target: 'local' });

/** Mobile presentation of the shared policy model; validation and normalization live in mesh-inbound-policy.ts. */
export function MeshInboundPolicyFields({ value, onChange, errorKey, errorVersion, idPrefix, protocol }: {
  value: MeshInboundPolicy | undefined;
  onChange: (next: MeshInboundPolicy | undefined) => void;
  errorKey: string | null;
  errorVersion: number;
  idPrefix: string;
  protocol: ServerConfig['protocol'];
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(value !== undefined);
  const section = useRef<HTMLDivElement>(null);
  const scheduleReveal = useRevealAfterCommit();
  useEffect(() => {
    if (!errorKey) return;
    if (open) {
      if (section.current) revealElement(section.current);
    } else {
      scheduleReveal(() => { if (section.current) revealElement(section.current); });
      setOpen(true);
    }
  }, [errorKey, errorVersion]);
  const mode = value?.mode ?? 'legacy';
  const modeKey = mode === 'legacy' ? 'meshInbound.legacy'
    : mode === 'block' ? 'meshInbound.block' : 'meshInbound.allowlist';
  const rules = value?.mode === 'allowlist' ? value.rules : [];
  const edit = (index: number, rule: MeshInboundGrant) => onChange({
    mode: 'allowlist', rules: rules.map((current, i) => i === index ? rule : current),
  });

  return <div ref={section}>
    <p className="m-form-hint">{t(protocol === 'tailscale' ? 'mobileMeshInbound.boundaryTs' : 'mobileMeshInbound.boundary')}</p>
    <FormGroup title={`${t('meshInbound.title')} · ${t(modeKey)}`} open={open}
      onToggle={() => {
        const willOpen = !open;
        if (willOpen) scheduleReveal(() => { if (section.current) revealElement(section.current); });
        setOpen(willOpen);
      }}>
      <div className="m-form-row">
        <label className="m-form-label" htmlFor={`${idPrefix}-mesh-mode`}>{t('meshInbound.title')}</label>
        <MobileSelect id={`${idPrefix}-mesh-mode`} className="m-form-select" value={mode}
          onChange={(event) => onChange(event.target.value === 'legacy' ? undefined
            : event.target.value === 'block' ? { mode: 'block' } : { mode: 'allowlist', rules: [] })}>
          <option value="legacy">{t('meshInbound.legacy')}</option>
          <option value="block">{t('meshInbound.block')}</option>
          <option value="allowlist">{t('meshInbound.allowlist')}</option>
        </MobileSelect>
        <div className="m-form-hint"><MobileInfo title={t('meshInbound.title')}
          summary={t('mobileMeshInbound.applySummary')} details={t('meshInbound.pendingApply')} /></div>
        {errorKey && <p className="m-form-err" role="alert">{t(errorKey)}</p>}
      </div>
      {mode === 'allowlist' && <>
        {rules.length === 0 && <p className="m-form-hint">{t('meshInbound.emptyBlocks')}</p>}
        {rules.map((rule, index) => <div key={index} className="m-form-row m-mesh-policy-rule">
          <div className="m-form-label">{t('meshInbound.rule', { number: index + 1 })}</div>
          <div className="m-mesh-policy-fields">
            <div className="m-mesh-policy-field full">
              <label className="m-form-label" htmlFor={`${idPrefix}-source-${index}`}>{t('meshInbound.source')}</label>
              <input id={`${idPrefix}-source-${index}`} className="m-form-input mono"
                value={rule.sourceCidrs.join(', ')} placeholder={t('meshInbound.sourcePlaceholder')}
                onChange={(event) => edit(index, { ...rule, sourceCidrs: split(event.target.value) })} />
            </div>
            <div className="m-mesh-policy-field">
              <label className="m-form-label" htmlFor={`${idPrefix}-network-${index}`}>{t('meshInbound.network')}</label>
              <MobileSelect id={`${idPrefix}-network-${index}`} className="m-form-select" value={rule.network}
                onChange={(event) => edit(index, { ...rule, network: event.target.value as MeshInboundGrant['network'] })}>
                <option value="tcp">TCP</option><option value="udp">UDP</option><option value="both">TCP + UDP</option>
              </MobileSelect>
            </div>
            <div className="m-mesh-policy-field">
              <label className="m-form-label" htmlFor={`${idPrefix}-ports-${index}`}>{t('meshInbound.ports')}</label>
              <input id={`${idPrefix}-ports-${index}`} className="m-form-input mono"
                value={rule.ports.join(', ')} placeholder={t('meshInbound.portsPlaceholder')}
                onChange={(event) => edit(index, { ...rule, ports: split(event.target.value) })} />
            </div>
            <div className="m-mesh-policy-field full">
              <label className="m-form-label" htmlFor={`${idPrefix}-target-${index}`}>{t('meshInbound.target')}</label>
              <MobileSelect id={`${idPrefix}-target-${index}`} className="m-form-select" value={rule.target}
                onChange={(event) => edit(index, { ...rule, target: event.target.value as MeshInboundGrant['target'], targetCidrs: undefined })}>
                <option value="local">{t('meshInbound.local')}</option><option value="forward">{t('meshInbound.forward')}</option>
              </MobileSelect>
            </div>
            {rule.target === 'forward' && <div className="m-mesh-policy-field full">
              <label className="m-form-label" htmlFor={`${idPrefix}-dest-${index}`}>{t('meshInbound.destCidrs')}</label>
              <input id={`${idPrefix}-dest-${index}`} className="m-form-input mono"
                value={(rule.targetCidrs ?? []).join(', ')} placeholder={t('meshInbound.destCidrsPlaceholder')}
                onChange={(event) => edit(index, { ...rule, targetCidrs: split(event.target.value) })} />
              <p className="m-form-hint">{t('meshInbound.forwardBoundary')}</p>
            </div>}
          </div>
          <button type="button" className="m-form-btn m-mesh-policy-action" onClick={() => onChange({ mode: 'allowlist', rules: rules.filter((_, i) => i !== index) })}>
            {t('meshInbound.remove')}
          </button>
        </div>)}
        <button type="button" className="m-form-btn m-mesh-policy-action" onClick={() => onChange({ mode: 'allowlist', rules: [...rules, emptyGrant()] })}>
          {t('meshInbound.add')}
        </button>
      </>}
    </FormGroup>
  </div>;
}
