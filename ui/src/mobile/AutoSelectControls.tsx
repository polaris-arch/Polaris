import { useAutoSelectStatus } from '@/hooks/use-auto-select-status';
import { AutoSelectStatusPanel } from './AutoSelectStatusPanel';
import type { AutoSelectT } from '@/lib/auto-select-actions';
import type { UserConfig } from '@/contracts/types';
import { useAutoSelectActions } from '@/hooks/use-auto-select-actions';

interface Props { config: UserConfig | null; accept: (config: UserConfig) => void | Promise<void>; t: AutoSelectT; query?: string; getCurrentConfig?: () => UserConfig | null; showStatus?: boolean }

/** Separate intent choices, never ServerConfig rows or bulk/test/export targets. */
export function AutoSelectControls({ config, accept, t, query = '', getCurrentConfig, showStatus = false }: Props) {
  const { busy, run, feedback } = useAutoSelectActions(config, accept, t, getCurrentConfig);
  const status = useAutoSelectStatus(showStatus ? config : null);
  if (!config) return null;
  const needle = query.trim().toLocaleLowerCase();
  const choices = (config.subscriptions ?? []).filter(sub => sub.name.toLocaleLowerCase().includes(needle));
  const intent = config.selectionIntent;
  const auto = intent?.mode === 'auto' && intent.scope === 'subscription';
  if (!choices.length && !auto && intent === undefined) return null;
  return <section data-auto-select-controls="" aria-label={t('autoSelect.choose')} aria-busy={busy}
    style={{ marginBlock: '0.75rem', padding: '0.75rem', border: '1px solid hsl(var(--line))', borderRadius: '0.75rem', overflowWrap: 'anywhere' }}>
    <strong>{t('autoSelect.choose')}</strong>
    <p>{t('autoSelect.choiceHelp')}</p>
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5rem' }}>
      {choices.map(sub => {
        const selected = auto && intent.subscriptionId === sub.id;
        const empty = !config.servers.some(server => server.subscriptionId === sub.id);
        return <div key={sub.id}><button type="button" data-auto-select-subscription={sub.id} data-exit-write="auto-select-enable"
          aria-pressed={selected} disabled={busy || empty || selected} aria-description={empty ? t('autoSelect.error.AUTO_SELECT_SUBSCRIPTION_EMPTY') : undefined}
          style={{ minHeight: '2.75rem', paddingInline: '0.75rem' }} onClick={() => run({ kind: 'enable', subscriptionId: sub.id })}>
          {t('autoSelect.choice', { subscription: sub.name })}
        </button>{empty && <p data-auto-select-empty={sub.id}>{t('autoSelect.error.AUTO_SELECT_SUBSCRIPTION_EMPTY')}</p>}</div>;
      })}
      {auto && <button type="button" data-exit-write="auto-select-now" disabled={busy}
        style={{ minHeight: '2.75rem', paddingInline: '0.75rem' }} onClick={() => run({ kind: 'switchNow' })}>{t('autoSelect.switchNow')}</button>}
      {intent !== undefined && <button type="button" data-exit-write="auto-select-manual" disabled={busy || !config.selectedServerId}
        style={{ minHeight: '2.75rem', paddingInline: '0.75rem' }} onClick={() => { if (config.selectedServerId) run({ kind: 'manual', serverId: config.selectedServerId }); }}>{t('autoSelect.keepManual')}</button>}
    </div>
    {showStatus && <AutoSelectStatusPanel {...status} t={t} servers={config.servers} subscriptions={config.subscriptions ?? []} />}
    {feedback && <p role="status" aria-live="polite" data-auto-select-feedback={feedback.tone}>{feedback.text}</p>}
  </section>;
}
