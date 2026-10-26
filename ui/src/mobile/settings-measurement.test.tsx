import { renderToStaticMarkup } from 'react-dom/server';
import { Children, isValidElement } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
const fixture = vi.hoisted(() => ({ run: vi.fn(), busy: false, feedback: null as null | { tone: string; text: string }, statusFailed: false, statusLoading: false, refresh: vi.fn(), result: { state: 'expired', purpose: 'system' } as any }));
vi.mock('@/hooks/use-auto-select-actions', () => ({ useAutoSelectActions: () => ({ busy: fixture.busy, feedback: fixture.feedback, run: fixture.run }) }));
vi.mock('@/hooks/use-auto-select-status', () => ({ useAutoSelectStatus: () => ({ failed: fixture.statusFailed, loading: fixture.statusLoading, refresh: fixture.refresh, runtimeCurrent: false }) }));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/store/app-store', () => ({ useAppStore: (pick: any) => pick({ config: { servers: [] } }) }));
vi.mock('@/store/use-latency-store', () => ({ useLatencyStore: (pick: any) => pick({ currentProjections: { a: fixture.result }, projections: { a: fixture.result }, readError: false }) }));
import { SetRow, Switch } from '@/components/screens/settings/Primitives';
import { AutoSelectSettingsSwitch as desktopSwitch } from '@/components/screens/settings/AutoSelectSettingsSwitch';
import { AutoSelectControls } from './AutoSelectControls';
import { MeasurementLabel } from './MeasurementLabel';
const config = { autoSelectEnabled: true, servers: [], subscriptions: [] } as any;
beforeEach(() => { fixture.run.mockClear(); fixture.refresh.mockClear(); fixture.statusFailed = false; fixture.statusLoading = false; fixture.busy = false; fixture.feedback = null; fixture.result = { state: 'expired', purpose: 'system' }; });
it('settings Auto reuses ordinary settings row and switch instead of a separate card', () => {
  const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} variant="switch" renderSettingsSwitch={desktopSwitch} />);
  expect(html).toContain('class="set-row"'); expect(html).toContain('class="sr-ctl"');
  expect(html).toContain('role="switch"'); expect(html).toContain('aria-checked="true"');
  expect(html).toContain('class="set-row-group"'); expect(html).not.toContain('class="set-row-details"');
  expect(html).not.toContain('auto-choice-switch'); expect(html).toContain('data-exit-write="auto-select-mode"');
});
it('busy and feedback survive the shared settings presentation', () => {
  fixture.busy = true; fixture.feedback = { tone: 'error', text: 'safe feedback' };
  const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} variant="switch" renderSettingsSwitch={desktopSwitch} />);
  expect(html).toContain('aria-busy="true"'); expect(html).toContain('aria-disabled="true"');
  expect(html).toContain('class="set-row-details"'); expect(html).toContain('role="status"');
  expect(html).toContain('tabindex="-1"'); expect(html).toContain('aria-live="polite"'); expect(html).toContain('safe feedback');
});
it('compact measurement keeps the route in the tooltip, never in a fresh numeric substitute', () => {
  const html = renderToStaticMarkup(<MeasurementLabel serverId="a" purpose="current" compact />);
  expect(html).toContain('data-tip="measurement.expired · measurement.systemPath · measurement.expiredHint"');
  expect(html).toContain('>measurement.expired</span>'); expect(html).not.toContain(' ms');
});
it.each(['unmeasured', 'failed', 'paused'])('does not turn %s into expired', state => {
  fixture.result = { state };
  const html = renderToStaticMarkup(<MeasurementLabel serverId="a" purpose="current" compact />);
  expect(html).toContain(`>measurement.${state}</span>`); expect(html).not.toContain('measurement.expired');
});
it('fresh measurement retains its actual numeric value', () => {
  fixture.result = { state: 'success', latency: 42, purpose: 'system' };
  const html = renderToStaticMarkup(<MeasurementLabel serverId="a" purpose="current" compact />);
  expect(html).toContain('>42 ms</span>'); expect(html).toContain('42 ms · measurement.systemPath');
});
it('v2 FakeIP links to the existing DNS workspace without adding a second toggle', () => {
  const source = readFileSync(new URL('../components/screens/settings/SettingsDns.tsx', import.meta.url), 'utf8');
  const hint = source.split('{hasDnsPolicyV2 && <SetRow label="FakeIP"')[1].split('</SetRow>}')[0];
  expect(hint).toContain("navigate('dnsrules')"); expect(hint).not.toContain('<Switch');
  expect(source).toContain('{!hasDnsPolicyV2 && <SetRow label="FakeIP"');
});

it('mobile settings uses its existing touch-sized row and switch, not desktop-only CSS', () => {
  const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} variant="switch" settingsLayout="mobile" />);
  expect(html).toContain('data-setting="auto-select-mode"'); expect(html).toContain('min-width:var(--tap-min)');
  expect(html).not.toContain('class="set-row"'); expect(html).toContain('role="switch"');
});

it('desktop presentation delegates exactly one action to the existing Auto owner', () => {
  renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} variant="switch"
    renderSettingsSwitch={view => {
      const group = desktopSwitch(view);
      const row = Children.toArray(group.props.children).find(child => isValidElement(child) && child.type === SetRow);
      expect(isValidElement(row)).toBe(true);
      const control = isValidElement<{ children: import('react').ReactElement<{ onChange: (value: boolean) => void }> }>(row) ? row.props.children : undefined;
      expect(control?.type).toBe(Switch);
      expect(control?.props).toMatchObject({ checked: true, disabled: false, 'aria-label': 'autoSelect.choose' });
      (control?.props as { onChange: (value: boolean) => void }).onChange(false);
      return group;
    }} />);
  expect(fixture.run).toHaveBeenCalledExactlyOnceWith({ kind: 'setEnabled', enabled: false });
});
it('desktop settings supplies the presentation and mobile source has no desktop component dependency', () => {
  const desktop = readFileSync(new URL('../components/screens/settings/SettingsNetwork.tsx', import.meta.url), 'utf8');
  const mobile = readFileSync(new URL('./AutoSelectControls.tsx', import.meta.url), 'utf8');
  expect(desktop).toContain('renderSettingsSwitch={AutoSelectSettingsSwitch}');
  expect(desktop).toContain("import { AutoSelectSettingsSwitch } from './AutoSelectSettingsSwitch'");
  expect(mobile).not.toContain('@/components/screens/');
});


it('desktop disabled mode keeps the compact label and does not add an empty feedback area', () => {
  const html = renderToStaticMarkup(<AutoSelectControls config={{ ...config, autoSelectEnabled: false }} accept={() => {}} t={key => key} variant="switch" renderSettingsSwitch={desktopSwitch} />);
  expect(html).toContain('aria-checked="false"');
  expect(html).not.toContain('autoSelect.fixCurrentHint');
  expect(html).not.toContain('set-row-details');
});

it.each(['success', 'error'])('desktop %s action feedback remains live and separate from the switch row', tone => {
  fixture.feedback = { tone, text: 'action result' };
  const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} variant="switch" renderSettingsSwitch={desktopSwitch} />);
  expect(html).toContain('class="set-row-details"><span role="status" aria-live="polite"');
  expect(html).toContain(`data-auto-select-feedback="${tone}"`);
  expect(html).toContain('action result');
  expect(html).toContain('data-tip="autoSelect.fixCurrentHint"');
});

it.each([false, true])('desktop retry preserves the original handler with loading=%s', loading => {
  fixture.statusFailed = true;
  fixture.statusLoading = loading;
  let retry: import('react').ReactElement<{ onClick: () => void; disabled: boolean }> | undefined;
  const html = renderToStaticMarkup(<AutoSelectControls config={config} accept={() => {}} t={key => key} variant="switch" renderSettingsSwitch={view => {
    if (isValidElement<{ children: import('react').ReactNode }>(view.feedback)) {
      retry = Children.toArray(view.feedback.props.children).find(child => isValidElement(child) && child.type === 'button') as typeof retry;
    }
    return desktopSwitch(view);
  }} />);
  expect(html).toContain('class="set-row-details"><button');
  expect(retry?.props.disabled).toBe(loading);
  expect(retry?.props.onClick).toBe(fixture.refresh);
  expect(fixture.run).not.toHaveBeenCalled();
  if (!loading) retry?.props.onClick();
  expect(fixture.refresh).toHaveBeenCalledTimes(loading ? 0 : 1);
});
