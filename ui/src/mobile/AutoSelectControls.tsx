import { MobileSwitch, SettingsRow } from './settings/SettingsChrome';
import type { ReactNode } from 'react';

export interface AutoSelectSwitchView { label: string; tip?: string; feedback: ReactNode; checked: boolean; disabled: boolean; onChange: (enabled: boolean) => void }
import { useAutoSelectStatus } from '@/hooks/use-auto-select-status';
import type { AutoSelectT } from '@/lib/auto-select-actions';
import type { UserConfig } from '@/contracts/types';
import type { AutoSelectTarget } from '@/contracts/auto-select';
import { useAutoSelectActions } from '@/hooks/use-auto-select-actions';
import { autoSelectTargetMatches } from '@/domain/auto-select-target';
import { groupServersBySubscription } from '@/domain/server-grouping';

export interface AutoSelectControlsProps { config: UserConfig | null; accept: (config: UserConfig) => void | Promise<void>; t: AutoSelectT; query?: string; getCurrentConfig?: () => UserConfig | null; showStatus?: boolean; target?: AutoSelectTarget; settingsLayout?: 'desktop' | 'mobile'; renderSettingsSwitch?: (view: AutoSelectSwitchView) => ReactNode; variant?: 'card' | 'row' | 'switch' }

/** Auto is a routing reference, never a real server, export row, or bulk-test target. */
export function AutoSelectControls({ config, accept, t, getCurrentConfig, target = { scope: 'global' }, variant = 'row', renderSettingsSwitch }: AutoSelectControlsProps) {
  const { busy, run, feedback } = useAutoSelectActions(config, accept, t, getCurrentConfig);
  const status = useAutoSelectStatus(config);
  if (!config) return null;
  const featureEnabled = config.autoSelectEnabled === true;
  const selected = autoSelectTargetMatches(config.selectionIntent, target);
  const groups = groupServersBySubscription(config.servers, config.subscriptions ?? []);
  const members = target.scope === 'global' ? config.servers : groups.find(group => target.scope === 'subscription'
    ? !group.isManual && !group.isMesh && group.id === target.subscriptionId
    : target.scope === 'manual' ? group.isManual : group.isMesh)?.servers ?? [];
  if (variant !== 'switch' && (!featureEnabled || !members.length)) return null;
  const group = status.report?.nativeAutoGroups.find(group => group.target.scope === target.scope
    && group.target.subscriptionId === target.subscriptionId);
  const actual = status.runtimeCurrent && group?.confirmed ? members.find(server => server.id === group.actualServerId) : undefined;
  if (variant === 'switch') return <div data-auto-select-controls="" data-auto-select-scope={target.scope}
    data-auto-select-subscription={target.subscriptionId} aria-busy={busy} data-exit-write="auto-select-mode">
    {renderSettingsSwitch ? renderSettingsSwitch({
      label: t('autoSelect.choose'), tip: featureEnabled ? t('autoSelect.fixCurrentHint') : undefined,
      checked: featureEnabled, disabled: busy, onChange: enabled => { void run({ kind: 'setEnabled', enabled }); },
      feedback: status.failed || feedback ? <>{status.failed && <button type="button" onClick={status.refresh} disabled={status.loading} data-tip={t('autoSelect.loadFailed')}>{t('autoSelect.refresh')}</button>}
        {feedback && <span role="status" aria-live="polite" data-auto-select-feedback={feedback.tone}>{feedback.text}</span>}</> : undefined,
    }) : <SettingsRow id="auto-select-mode" first label={t('autoSelect.choose')}
      desc={featureEnabled ? t('autoSelect.fixCurrentHint') : undefined}
      hint={<>{status.failed && <button type="button" onClick={status.refresh} disabled={status.loading}>{t('autoSelect.refresh')}</button>}
        {feedback && <span role="status" aria-live="polite" data-auto-select-feedback={feedback.tone}>{feedback.text}</span>}</>}
      control={<MobileSwitch checked={featureEnabled} disabled={busy} ariaLabel={t('autoSelect.choose')}
        onChange={enabled => run({ kind: 'setEnabled', enabled })} />} />}
  </div>;
  return <section className={`auto-choice auto-choice-${variant}${selected ? ' on' : ''}`} data-auto-select-controls="" data-auto-select-scope={target.scope} data-auto-select-subscription={target.subscriptionId}
    aria-label={t('autoSelect.auto')} aria-busy={busy}>
    <div className="auto-choice-head">
      <button type="button" className="auto-choice-pick" data-auto-select-subscription={target.subscriptionId} data-exit-write="auto-select-enable" aria-pressed={selected} disabled={busy}
        onClick={() => run({ kind: 'enable', ...target })}>
        <strong>{t('autoSelect.auto')}</strong>
        <span data-auto-select-actual="">{actual ? `${actual.name} · ${t('autoSelect.tcpNode')}` : t('autoSelect.pendingNode')}</span>
      </button>
    </div>
    {status.failed && <button type="button" onClick={status.refresh} disabled={status.loading} data-tip={t('autoSelect.loadFailed')}>{t('autoSelect.refresh')}</button>}
    {feedback && <p role="status" aria-live="polite" data-auto-select-feedback={feedback.tone}>{feedback.text}</p>}
  </section>;
}
