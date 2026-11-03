import { useTranslation } from 'react-i18next';
import type { PendingApplyResult } from '@/contracts/types';

/** Shared read-only disclosure; confirmation tokens and UI owners stay in each surface. */
export interface ImpactDisclosure {
  coreImpacts: NonNullable<PendingApplyResult['coreImpacts']>;
  hostEffects: NonNullable<PendingApplyResult['hostEffectPreviews']>;
}

const HOST_LABEL = {
  dns: 'home.impactApplyDns',
  systemProxy: 'home.impactApplyProxy',
  nativeTunnel: 'home.impactApplyTunnel',
} as const;

// This finite producer vocabulary selects local copy; raw paths/reasons/session strings are never rendered.
const SESSION_LABEL = {
  'existing TUN TCP flows': 'home.impactApplyTunTcpReset',
  'existing TUN UDP associations': 'home.impactApplyTunUdpReset',
  'changed ingress admissions': 'home.impactApplyListenerAdmissions',
  'affected UDP associations': 'home.impactApplyListenerUdp',
  'dependent listener handoff': 'home.impactApplyListenerHandoff',
  'API streams on the replaced ingress': 'home.impactApplyApiStreams',
  api_generation_handoff_v1: 'home.impactApplyApiStreams',
  'dashboard updater': 'home.impactApplyDashboard',
  'new admissions': 'home.impactApplyNewAdmissions',
  'log emissions': 'home.impactApplyLogEmissions',
  'new TLS admissions': 'home.impactApplyTlsConsumers',
  'captured provider consumers': 'home.impactApplyTlsConsumers',
  'new clock consumers': 'home.impactApplyClockConsumers',
} as const;
const CONNECTION_CLASSES = new Set([
  'existing TUN TCP flows', 'existing TUN UDP associations', 'changed ingress admissions',
  'affected UDP associations', 'dependent listener handoff', 'API streams on the replaced ingress',
  'api_generation_handoff_v1', 'dashboard updater',
]);

export function impactApplyDisclosure(prompt: ImpactDisclosure): { keys: string[]; blocked: boolean } {
  const keys = new Set<string>();
  let blocked = prompt.coreImpacts.length === 0 && prompt.hostEffects.length === 0;
  for (const impact of prompt.coreImpacts) {
    // Unspecified, pending-adapter, restart-only and future classes cannot authorize this live-Apply choice.
    if (![1, 2, 3].includes(impact.updateClass) || impact.affectedSessionClasses.length === 0
      || (impact.updateClass === 1 && impact.reconnectRequired)) blocked = true;
    let connectionScope = false;
    for (const sessionClass of impact.affectedSessionClasses) {
      if (!Object.prototype.hasOwnProperty.call(SESSION_LABEL, sessionClass)) { blocked = true; continue; }
      keys.add(SESSION_LABEL[sessionClass as keyof typeof SESSION_LABEL]);
      connectionScope ||= CONNECTION_CLASSES.has(sessionClass);
    }
    if ((impact.reconnectRequired || impact.updateClass === 3) && !connectionScope) blocked = true;
  }
  if (keys.has('home.impactApplyTunTcpReset') || keys.has('home.impactApplyTunUdpReset')) keys.add('home.impactApplyTunRollback');
  for (const effect of prompt.hostEffects) {
    if (!Object.prototype.hasOwnProperty.call(HOST_LABEL, effect.kind)) { blocked = true; continue; }
    keys.add(HOST_LABEL[effect.kind]);
  }
  return { keys: [...keys], blocked };
}

export function ImpactApplySummary({ prompt }: { prompt: ImpactDisclosure }) {
  const { t } = useTranslation();
  const disclosure = impactApplyDisclosure(prompt);
  return <>
    <p>{t('home.impactApplyDescription')}</p>
    <ul>{disclosure.keys.map((key) => <li key={key}>{t(key)}</li>)}</ul>
    {disclosure.blocked && <p role="alert">{t('home.impactApplyUnknown')}</p>}
    <p>{t('home.impactApplySavedPending')}</p>
  </>;
}
