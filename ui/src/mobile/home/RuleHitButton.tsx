import type { ReactElement } from 'react';
import type { OutboundPolicy, RuleHit } from './aggregate';
import type { HomeScreenViewProps } from './view-model';

/** The overview and full list share labels, policy colors, counts and exact navigation keys. */
export function RuleHitButton({ hit, connections, t, onOpen }: {
  readonly hit: RuleHit;
  readonly connections: number;
  readonly t: HomeScreenViewProps['t'];
  readonly onOpen?: HomeScreenViewProps['onOpenRuleHit'];
}): ReactElement {
  const pct = connections > 0 ? hit.count / connections : 0;
  const policyLabel = t(hit.policy === 'direct' ? 'home.ruleDirect'
    : hit.policy === 'blocked' ? 'home.routingBlock'
      : hit.policy === 'proxied' ? 'home.ruleProxy' : 'common.unknown');
  const label = hit.kind === 'named' ? hit.rule : policyLabel;
  const policyClass: Record<OutboundPolicy, string> = {
    proxied: 'p-proxy', direct: 'p-direct', blocked: 'p-block', unknown: 'p-unknown',
  };
  return <button type="button" className="h-ringcell" onClick={() => onOpen?.({ key: hit.key, label })}
    disabled={!onOpen} aria-label={`${label} · ${hit.count}`}>
    <svg className={`h-ring ${policyClass[hit.policy]}`} viewBox="0 0 64 64" aria-hidden="true">
      <circle className="h-ringtrack" cx="32" cy="32" r="26" />
      <circle className="h-ringfill" cx="32" cy="32" r="26" strokeDasharray={`${(pct * 163.4).toFixed(2)} 163.4`} />
      <text x="32" y="32" dy=".35em" textAnchor="middle" className="h-ringval">{hit.count}</text>
    </svg>
    <span className="h-ringname">{label}</span>
    <span className="h-ringpolicy">{hit.kind === 'policy'
      ? (pct > 0 && pct < 0.01 ? '<1%' : `${Math.round(pct * 100)}%`) : policyLabel}</span>
  </button>;
}
