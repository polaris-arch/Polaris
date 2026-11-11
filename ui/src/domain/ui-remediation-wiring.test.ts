import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import { createElement } from 'react';
import { Segmented } from '@/components/screens/settings/Primitives';
const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

describe('UI remediation boundary wiring', () => {
  it('both rule surfaces share bounded asynchronous preview with revision ownership', () => {
    expect(read('hooks/use-rule-test.ts')).toContain('computeTestMatchAsync(query.conds, query.logic, query.test)');
    expect(read('hooks/use-rule-test.ts')).toContain('reply?.query === query');
    expect(read('components/dialogs/RuleDialog.tsx')).toContain('useRuleTestFold(conds, logic, test)');
    expect(read('mobile/forms/RuleFormPanel.tsx')).toContain('useRuleTestFold(conds, logic, test)');
  });
  it('disabled segmented control emits native disabled buttons', () => {
    const html = renderToStaticMarkup(createElement(Segmented, {
      id: 'capability', ariaLabel: 'Capability', value: 'off', disabled: true,
      options: [{ value: 'off', label: 'Off' }, { value: 'on', label: 'On' }], onChange: () => {},
    }));
    expect(html.match(/disabled=""/g)).toHaveLength(2);
  });
  it('mobile WireGuard uses the shared worker validator before persistence', () => {
    const source = read('mobile/forms/WgPanel.tsx');
    expect(source).toContain('workersInputInvalid(draft.workers)');
    expect(source.indexOf('workersInputInvalid(draft.workers)')).toBeLessThan(source.indexOf('await api.server'));
  });
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

it('shared mobile form logic stays outside desktop hosts without hiding it from the audit surface', () => {
  expect(read('mobile/forms/RuleFormPanel.tsx')).toContain("from '@/hooks/use-rule-test'");
  for (const path of ['mobile/forms/VpnAuthPanel.tsx', 'components/dialogs/VpnAuthDialog.tsx']) {
    expect(read(path)).toContain("from '@/shared/vpn/VpnAuthForm'");
  }
  const shared = read('shared/vpn/VpnAuthForm.tsx');
  expect(shared).toContain('return renderFrame(');
  expect(shared).not.toMatch(/from ['"][^'"]*(?:Modal|FormSheet|components\/screens)/);
  expect(shared).not.toMatch(/import ['"][^'"]+\.css/);
  expect(read('mobile/nodes/nodes-screen.test.tsx')).toContain("join(SRC, 'shared/vpn/VpnAuthForm.tsx')");
});

it('claimed startup errors retain localized resolver and speed URL recovery hints', () => {
  const home = read('components/screens/home/HomeScreen.tsx');
  expect(home).toContain('code === ProxyErrorCode.NODE_RESOLVER_UNSUPPORTED || code === ProxyErrorCode.SPEED_TEST_URL_INVALID');
  expect(home).toContain('reportConnectionFailure(proxyErrorText({ errorCode: code }, key => t(key)))');
  expect(home).toContain("const reportConnectionFailure = (detail: string) => toast.error(");
  expect(home).toContain("t(action === 'stop' ? 'feedback.stopFailed' : 'feedback.connectionFailed'), detail");
});

