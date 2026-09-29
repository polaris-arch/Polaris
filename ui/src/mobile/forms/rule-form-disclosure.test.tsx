import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { UserConfig } from '@/contracts/types';
import i18n, { i18nReady } from '@/i18n';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { RuleFormPanel } from './RuleFormPanel';

beforeAll(async () => { await i18nReady; });
beforeEach(() => {
  useAppStore.setState({ servers: [], rules: [], dnsRules: [], config: { servers: [], subscriptions: [], customRules: [] } as unknown as UserConfig });
  useStagedConfigStore.setState({ entries: [] });
  // Zustand SSR hooks read the initial snapshot rather than the current browser snapshot.
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
});

describe('rule form disclosure', () => {
  it.each(['route', 'dns'] as const)('shows name, conditions and %s effect on first open; leaves matching test folded', (initialPlane) => {
    const markup = renderToStaticMarkup(<RuleFormPanel instanceId={`rule-${initialPlane}`} initialPlane={initialPlane} />);
    expect([...markup.matchAll(/class="m-form-group-h" aria-expanded="(true|false)"/g)].map((match) => match[1])).toEqual(['true', 'true', 'true', 'false']);
    expect(markup).toContain(i18n.t('rules.name'));
    expect(markup).toContain(i18n.t('rules.conditions'));
    expect(markup).toContain(i18n.t(initialPlane === 'route' ? 'rules.target' : 'rules.dnsAction'));
    expect(markup).toContain(`data-form-group="rule-${initialPlane}:rule:basic"`);
    expect(markup).toContain(`data-form-group="rule-${initialPlane}:rule:cond"`);
  });

});
