import { renderToStaticMarkup } from 'react-dom/server';
import { Children, isValidElement } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
const fixture = vi.hoisted(() => ({ run: vi.fn(), busy: false, feedback: null as null | { tone: string; text: string }, statusFailed: false, statusLoading: false, refresh: vi.fn(), result: { state: 'expired', latency: 42, purpose: 'system' } as any, snapshot: null as any }));
vi.mock('@/hooks/use-auto-select-actions', () => ({ useAutoSelectActions: () => ({ busy: fixture.busy, feedback: fixture.feedback, run: fixture.run }) }));
vi.mock('@/hooks/use-auto-select-status', () => ({ useAutoSelectStatus: () => ({ failed: fixture.statusFailed, loading: fixture.statusLoading, refresh: fixture.refresh, runtimeCurrent: false }) }));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/store/app-store', () => ({ useAppStore: (pick: any) => pick({ config: { servers: [{ id: 'a', subscriptionId: 'sub' }] } }) }));
vi.mock('@/store/use-latency-store', () => ({ useLatencyStore: (pick: any) => pick({ currentProjections: { a: fixture.result }, projections: { a: fixture.result }, readError: false, snapshot: fixture.snapshot }) }));
import { SetRow, Switch } from '@/components/screens/settings/Primitives';
import { AutoSelectSettingsSwitch as desktopSwitch } from '@/components/screens/settings/AutoSelectSettingsSwitch';
import { AutoSelectControls } from './AutoSelectControls';
import { MeasurementLabel } from './MeasurementLabel';
import { UNMEASURED_PROJECTION } from '@/domain/measurement-display';
const config = { autoSelectEnabled: true, servers: [], subscriptions: [] } as any;
beforeEach(() => { fixture.run.mockClear(); fixture.refresh.mockClear(); fixture.statusFailed = false; fixture.statusLoading = false; fixture.busy = false; fixture.feedback = null; fixture.result = { state: 'expired', latency: 42, purpose: 'system' }; fixture.snapshot = null; });
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
it('expired measurement retains its old value and keeps route details in the tooltip', () => {
  const html = renderToStaticMarkup(<MeasurementLabel serverId="a" purpose="current" compact />);
  expect(html).toContain('data-tip="42 ms · measurement.expired · measurement.systemPath · measurement.expiredHint"');
  expect(html).toContain('>42 ms · measurement.expired</span>');
});
it.each(['unmeasured', 'failed', 'paused'])('does not turn %s into expired', state => {
  fixture.result = { state };
  const html = renderToStaticMarkup(<MeasurementLabel serverId="a" purpose="current" compact />);
  expect(html).toContain(`>measurement.${state}</span>`); expect(html).not.toContain('measurement.expired');
});
it.each([false, true])('fresh measurement shows only its value with compact=%s', compact => {
  fixture.result = { state: 'success', latency: 42, purpose: 'system', disconnected: true };
  const html = renderToStaticMarkup(<MeasurementLabel serverId="a" purpose="current" compact={compact} />);
  expect(html).toContain('>42 ms</span>'); expect(html).toContain('measurement.systemPath');
  expect(html).not.toContain('measurement.disconnected');
});
it('pending native quality does not inherit an App subscription pause', () => {
  fixture.result = UNMEASURED_PROJECTION;
  fixture.snapshot = { schedule: { state: 'paused', reason: 'metered', subscriptions: { sub: {} } }, rows: { a: { nativeHttp: null } } };
  const native = renderToStaticMarkup(<MeasurementLabel serverId="a" />);
  expect(native).toContain('>measurement.unmeasured</span>'); expect(native).not.toContain('measurement.paused');
  delete fixture.snapshot.rows.a.nativeHttp;
  expect(renderToStaticMarkup(<MeasurementLabel serverId="a" />)).toContain('>measurement.paused</span>');
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
