/** 本地入站授权的三处节点表单共用编辑器与提交门；不含协议 settings wire 字段。 */
import { useTranslation } from 'react-i18next';
import type { MeshInboundGrant, MeshInboundPolicy, ServerConfig } from '@/contracts/types';
import { isValidIpCidr, validateRuleValue } from '@/domain/rules';
import { Csel } from './Csel';

const split = (text: string): string[] => text.split(/[\n,]/).map((part) => part.trim());
const clean = (values: string[]): string[] => values.map((v) => v.trim()).filter(Boolean);
const validCidr = (cidr: string): boolean => cidr.includes('/') && isValidIpCidr(cidr);
const loopbackOnly = (cidr: string): boolean => {
  const [address, mask] = cidr.split('/');
  const prefix = Number(mask);
  if (address?.includes('.') && !address.includes(':')) {
    return (address.split('.')[0] === '127' && prefix >= 8) ||
      (address === '0.0.0.0' && prefix === 32);
  }
  if (!address?.includes(':')) return false;
  // URL 使用浏览器原生 IPv6 解析，把展开式与 dotted mapped 地址归到同一规范表示。
  let host: string;
  try { host = new URL(`http://[${address}]/`).hostname.slice(1, -1); }
  catch { return false; }
  if ((host === '::1' || host === '::') && prefix === 128) return true;
  const mapped = host.match(/^::ffff:([0-9a-f]+):([0-9a-f]+)$/);
  if (!mapped) return false;
  const first = Number.parseInt(mapped[1], 16) >> 8;
  return (first === 127 && prefix >= 104) ||
    (mapped[1] === '0' && mapped[2] === '0' && prefix === 128);
};

export function normalizeMeshInboundPolicy(policy: MeshInboundPolicy | undefined): MeshInboundPolicy | undefined {
  if (!policy || policy.mode === 'block') return policy;
  return {
    mode: 'allowlist',
    rules: policy.rules.map((rule) => ({
      ...rule,
      sourceCidrs: clean(rule.sourceCidrs),
      ports: clean(rule.ports),
      targetCidrs: rule.target === 'forward' ? clean(rule.targetCidrs ?? []) : undefined,
    })),
  };
}

/** 返回本地化错误键；后端仍是最终授权门。 */
export function meshInboundPolicyError(
  policy: MeshInboundPolicy | undefined,
  protocol: ServerConfig['protocol'],
  wgAddresses: string[] = [],
): string | null {
  if (!policy) return null;
  if (!['wireguard', 'tailscale', 'masque-client', 'openconnect', 'openvpn-client'].includes(protocol)) {
    return 'meshInbound.unsupported';
  }
  if (policy.mode === 'block') return null;
  if (protocol === 'wireguard' && (!wgAddresses.length || wgAddresses.some((cidr) =>
    !validCidr(cidr) || !cidr.endsWith(cidr.includes(':') ? '/128' : '/32')
  ))) return 'meshInbound.wgHostPrefix';
  for (const rule of policy.rules) {
    if (!rule.sourceCidrs.length || rule.sourceCidrs.some((cidr) => !validCidr(cidr))) {
      return 'meshInbound.sourceInvalid';
    }
    if (!rule.ports.length || rule.ports.some((port) => !validateRuleValue('port', port))) {
      return 'meshInbound.portInvalid';
    }
    if (rule.target === 'forward' && (!rule.targetCidrs?.length ||
      rule.targetCidrs.some((cidr) => !validCidr(cidr) || loopbackOnly(cidr)))) {
      return 'meshInbound.targetInvalid';
    }
    if (rule.network !== 'tcp' && rule.ports.some((port) => {
      const [start, end = start] = port.split('-').map(Number);
      return start <= 53 && end >= 53;
    })) return 'meshInbound.udp53';
  }
  return null;
}

export function applyMeshInboundPolicy<T extends ServerConfig>(server: T, policy: MeshInboundPolicy | undefined): T {
  const normalized = normalizeMeshInboundPolicy(policy);
  if (normalized) server.meshInboundPolicy = normalized;
  else delete server.meshInboundPolicy;
  return server;
}

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
