import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import en from '@/i18n/locales/en-US.json';
import type { CoexSnapshot } from '@/contracts/coex-snapshot';
const t = (key: string) => {
  let value: unknown = en;
  for (const part of key.split('.')) value = (value as Record<string, unknown>)[part];
  if (typeof value !== 'string') throw Error(`missing translation ${key}`);
  return value;
};
const collect = vi.hoisted(() => vi.fn());
vi.mock('@/ipc', () => ({ api: { system: { coexReadonlySnapshot: collect } } }));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t }) }));
const { CoexSnapshotBlock, renderCoexFacts } = await import('./CoexSnapshotBlock');
const known = <T,>(value: T): { status: 'known'; value: T } => ({ status: 'known', value });
const unknown = (reason: string): { status: 'unknown'; reason: string } => ({ status: 'unknown', reason });
const fixture = (): CoexSnapshot => ({
  schemaVersion: 1, platform: 'linux', observation: known({ elapsedMillis: 17, atomic: false }), commandCleanup: unknown('retained command cleanup unresolved'),
  context: { observationPhase: unknown('phase missing'), ownInterfaces: unknown('owner missing'), criteria: unknown('criteria missing'), repairHistory: unknown('history missing') },
  classification: unknown('classification not performed'),
  objects: known([{ interface: '<fixture>&tun', tunnel: known(false), virtualization: unknown('<script>unreadable</script>'), stableIdentity: known(null),
    addresses: known([{ address: '10.77.2.9', prefixLen: 24 }]), routes: unknown('route permission denied'), policyRules: known([]) }]),
});
const render = (snapshot: CoexSnapshot, expanded: number | null = 0) => renderToStaticMarkup(<>{renderCoexFacts(snapshot, t, expanded)}</>);
describe('COEX fact projection on the actual settings surface', () => {
  it('does not invoke on initial render and starts explicitly uncollected', () => {
    expect(renderToStaticMarkup(<CoexSnapshotBlock />)).toContain(en.settings.coex.notCollected);
    expect(collect).not.toHaveBeenCalled();
    const source = readFileSync(new URL('./SettingsTun.tsx', import.meta.url), 'utf8');
    expect(source).toContain("import { CoexSnapshotBlock } from './CoexSnapshotBlock'"); expect(source).toContain('<CoexSnapshotBlock />');
    expect(source).not.toContain('coexReadonlySnapshot(');
  });
  it('retains addresses when routes are unreadable, distinguishes false/null/empty/unknown and escapes raw strings', () => {
    const html = render(fixture());
    for (const value of ['10.77.2.9/24', 'route permission denied', en.settings.coex.false, en.settings.coex.noIdentity,
      en.settings.coex.observedEmpty, en.settings.coex.unknown, 'history missing', 'classification not performed',
      '&lt;script&gt;unreadable&lt;/script&gt;', '&lt;fixture&gt;&amp;tun']) expect(html).toContain(value);
    expect(html).not.toContain('<script>'); expect(html).not.toContain('class="good"');
  });
  it('distinguishes empty enumeration from unknown enumeration and unavailable platforms', () => {
    const empty = fixture(); empty.objects = known([]); expect(render(empty)).toContain(en.settings.coex.observedEmpty);
    const v = fixture(); v.platform = 'win32'; v.objects = unknown('collector unavailable'); const html = render(v);
    expect(html).toContain('win32'); expect(html).toContain(en.settings.coex.sourceUnavailable); expect(html).not.toContain(en.settings.coex.observedEmpty);
  });
  it('limits rendering only through explicit pagination and one expanded interface', () => {
    const v = fixture(); if (v.objects.status !== 'known') throw Error('fixture');
    v.objects.value[0].addresses = known(Array.from({ length: 41 }, (_, i) => ({ address: `host-${i}`, prefixLen: 24 })));
    const html = render(v); expect(html).toContain('1–20 / 41'); expect(html).toContain('host-19'); expect(html).not.toContain('host-20');
    const collapsed = render(v, null); expect(collapsed).not.toContain('host-0'); expect(collapsed).toContain('&lt;fixture&gt;');
  });
  it('renders known-null route and lookup tables explicitly, separately from Unknown', () => {
    const v = fixture(); if (v.objects.status !== 'known') throw Error('fixture');
    v.objects.value[0].routes = known([{ prefix: '10.77.0.0/16', table: known(null), scope: known('global'), role: unknown('role unresolved') }]);
    v.objects.value[0].policyRules = known([{ priority: 10, lookupTable: known(null), addressFamily: known('ipv4'), selectorScope: unknown('selector unresolved'), appliesToObject: unknown('association unresolved') }]);
    const html = render(v);
    expect(html.split(en.settings.coex.noTableNumber)).toHaveLength(3);
    expect(html).toContain('selector unresolved');
  });
  it('displays raw table/family/selector and independent association without joining them', () => {
    const v = fixture(); if (v.objects.status !== 'known') throw Error('fixture');
    v.objects.value[0].routes = known([{ prefix: '0.0.0.0/1', table: known(254), scope: known('interfaceScoped'), role: unknown('role unresolved') }]);
    v.objects.value[0].policyRules = known([{ priority: 10, lookupTable: known(100), addressFamily: known('ipv6'), selectorScope: known({ kind: 'limited', selector: 'fwmark 0x8' }), appliesToObject: unknown('association unresolved') }]);
    const html = render(v); for (const raw of ['254', '100', 'ipv6', 'fwmark 0x8', 'role unresolved', 'association unresolved']) expect(html).toContain(raw);
    expect(html).toContain(en.settings.coex.classification);
  });
});
