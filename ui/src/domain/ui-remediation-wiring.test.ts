import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import { createElement } from 'react';
import { Segmented } from '@/components/screens/settings/Primitives';
import { proxyErrorReason } from './proxy-error-text';
const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

describe('UI remediation boundary wiring', () => {
  it('native resolver rejection has an actionable localized code, ignoring diagnostic text', () => {
    expect(proxyErrorReason({ errorCode: 'NODE_RESOLVER_UNSUPPORTED', message: 'private diagnostic' }, key => key))
      .toBe('errors.nodeResolverUnsupported');
  });
  it('both rule surfaces share bounded asynchronous preview with revision ownership', () => {
    expect(read('components/dialogs/RuleTestFold.tsx')).toContain('computeTestMatchAsync(query.conds, query.logic, query.test)');
    expect(read('components/dialogs/RuleTestFold.tsx')).toContain('reply?.query === query');
    expect(read('components/dialogs/RuleDialog.tsx')).toContain('useRuleTestFold(conds, logic, test)');
    expect(read('mobile/forms/RuleFormPanel.tsx')).toContain('useRuleTestFold(conds, logic, test)');
  });
  it('desktop About reads the actual coreVersion field', () => {
    const source = read('components/screens/settings/SettingsAbout.tsx');
    expect(source).toContain('info.coreVersion');
    expect(source).not.toContain('info.singBoxVersion');
  });
  it('disabled segmented control emits native disabled buttons', () => {
    const html = renderToStaticMarkup(createElement(Segmented, {
      id: 'capability', ariaLabel: 'Capability', value: 'off', disabled: true,
      options: [{ value: 'off', label: 'Off' }, { value: 'on', label: 'On' }], onChange: () => {},
    }));
    expect(html.match(/disabled=""/g)).toHaveLength(2);
    expect(read('components/screens/settings/SettingsNetwork.tsx')).toContain('disabled={webrtcDisabled}');
  });
  it('mobile WireGuard uses the shared worker validator before persistence', () => {
    const source = read('mobile/forms/WgPanel.tsx');
    expect(source).toContain('workersInputInvalid(draft.workers)');
    expect(source.indexOf('workersInputInvalid(draft.workers)')).toBeLessThan(source.indexOf('await api.server'));
  });
});

it('rule rows resolve explicit automatic targets and desktop toggles use entity CAS', () => {
  for (const path of ['components/screens/rules/RulesScreen.tsx', 'mobile/screens/rules/RulesScreen.tsx']) {
    const source = read(path);
    expect(source).toContain('autoSelectTargetLabel(config, route.targetAutoSelect,');
  }
  expect(read('components/screens/rules/RulesScreen.tsx')).toContain('api.rules.update(next, plane, rule)');
});


it('both settings surfaces derive warnings from the adopted visible draft', () => {
  const desktop = read('components/screens/settings/SettingsNetwork.tsx');
  const mobile = read('mobile/settings/NetworkPage.tsx');
  expect(desktop).toContain('const speedUrlErr = invalidSpeedUrl(speedUrlDraft)');
  expect(mobile).toContain('const speedUrlInvalid = invalidSpeedUrl(speedUrl)');
  expect(desktop).toContain('adoptSpeedTestUrlDraft(cur, prev.speedUrl, snap.speedUrl)');
  expect(mobile).toContain('adoptSpeedTestUrlDraft(cur, previous, next)');
  expect(mobile).toContain('const previous = seeded.current');
});
