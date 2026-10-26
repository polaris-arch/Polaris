import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { NodeMenuProps } from './NodeMenu';

// Initial-render contract with actual React and grouping; no effect simulation.
// Stateful click/async publication/reopen coverage lives in the existing browser
// app-transactions suite, which mounts the complete production component.
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/mobile/SavedAutoSelectControls', () => ({ SavedAutoSelectControls: () => null }));
vi.mock('@/mobile/MeasurementLabel', () => ({ MeasurementLabel: () => null }));
vi.mock('../nodes/NdFlag', () => ({ flagCodeForName: () => null }));
vi.mock('@/components/FlagImg', () => ({ FlagImg: () => null }));
const { NodeMenu } = await import('./NodeMenu');
const props: NodeMenuProps = {
  open: true,
  servers: [{ id: 'node-a', name: 'Loop A', protocol: 'socks', address: '127.0.0.1', port: 1, subscriptionId: 'fixture' }],
  subscriptions: [{ id: 'fixture', name: 'Package loopback', url: 'https://fixture.invalid', autoUpdate: false, createdAt: '2026-01-01T00:00:00Z' }],
  selectedServerId: 'node-a', latencies: {}, blockDisabledReason: null,
  onPick: () => {}, onPickDirect: () => {}, onPickBlock: () => {}, onTestAll: () => {}, onManage: () => {},
};
describe('NodeMenu initial render (not a browser lifecycle substitute)', () => {
  it('commits the selected group expanded without waiting for a passive effect', () => {
    const html = renderToStaticMarkup(<NodeMenu {...props} />);
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('role="option"');
    expect(html).toContain('Loop A');
  });
  it('keeps unconfirmed selection collapsed instead of guessing a group', () => {
    const html = renderToStaticMarkup(<NodeMenu {...props} selectedServerId={null} />);
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain('role="option"');
  });
  it('retains the closed target identity for the parent aria-controls', () => {
    const html = renderToStaticMarkup(<NodeMenu {...props} open={false} />);
    expect(html).toContain('id="node-menu"');
    expect(html).toContain('role="listbox"');
    expect(html).toContain('hidden=""');
    expect(html).not.toContain('ns-grp');
  });
});
