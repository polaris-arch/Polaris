import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { NetworkInterfaceInfo, SubscriptionConfig, UserConfig } from '@/contracts/types';
import i18n, { i18nReady } from '@/i18n';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useMobileFormStore } from './form-store';
import type { FormSheetProps } from './FormSheet';
import { buildSubItems } from '../nodes/view-model';

// SSR does not run the enumeration effect. Only its data and the IPC sink are replaced;
// the shared option projection, select, form, validation and submission stay real.
const probe = vi.hoisted(() => ({
  items: [] as NetworkInterfaceInfo[],
  loading: false,
  failed: false,
  submit: null as (() => void) | null,
  update: vi.fn<(sub: SubscriptionConfig) => Promise<void>>(),
  load: vi.fn<() => Promise<void>>(),
}));

vi.mock('@/hooks/use-network-interfaces', async (original) => ({
  ...(await original<typeof import('@/hooks/use-network-interfaces')>()),
  useNetworkInterfaces: () => ({ items: probe.items, loading: probe.loading, failed: probe.failed }),
}));
vi.mock('./FormSheet', async (original) => {
  const real = await original<typeof import('./FormSheet')>();
  return { FormSheet: (props: FormSheetProps) => {
    probe.submit = props.onSubmit ?? null;
    return <real.FormSheet {...props} />;
  } };
});
vi.mock('@/ipc', async (original) => {
  const real = await original<typeof import('@/ipc')>();
  return { ...real, api: { ...real.api, subscription: { ...real.api.subscription, update: probe.update } } };
});
const { SubFormPanel } = await import('./SubFormPanel');

beforeAll(async () => {
  await i18nReady;
  await i18n.changeLanguage('en-US');
});

const sub = (over: Partial<SubscriptionConfig> = {}): SubscriptionConfig => ({
  id: 'sub-1', name: 'Airport', url: 'https://example.com/sub', autoUpdate: false,
  createdAt: '2026-09-26', ...over,
});
const esc = (text: string): string => text.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
const tx = (key: string, vars?: Record<string, unknown>): string => esc(i18n.t(key, vars ?? {}));

beforeEach(() => {
  probe.items = [
    { name: 'wlan0', displayName: 'Wi-Fi', isUp: true, addresses: [] },
    { name: 'eth0', displayName: 'Ethernet', isUp: false, addresses: [] },
  ];
  probe.loading = false;
  probe.failed = false;
  probe.submit = null;
  probe.update.mockReset().mockResolvedValue(undefined);
  probe.load.mockReset().mockResolvedValue(undefined);
  useStagedConfigStore.setState({ entries: [] });
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
  useAppStore.setState({ config: { subscriptions: [] } as unknown as UserConfig, loadConfig: probe.load });
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  useMobileFormStore.getState().closeAll();
});

function render(base?: SubscriptionConfig, config: Partial<UserConfig> = {}): string {
  if (base) {
    useAppStore.setState({ config: { ...config, subscriptions: [base] } as UserConfig });
    Object.assign(useAppStore.getInitialState(), useAppStore.getState());
  }
  return renderToStaticMarkup(<SubFormPanel instanceId="sub-form" subId={base?.id} />);
}

describe('subscription interface field', () => {
  it('new form keeps required fields expanded and defaults to inherited proxy egress', () => {
    const html = render();
    expect(html).toContain('id="msf-name"');
    expect(html).toContain('id="msf-url"');
    expect(html).not.toContain('<details');
    expect(html).toContain(`>${tx('sub.bindInterfaceInherit')}</span>`);
    expect(html).toContain(tx('sub.bindInterfaceHint'));
    expect(html).toContain('<option value="wlan0">Wi-Fi · wlan0</option>');
    expect(html).toContain('<option value="eth0" disabled="">');
    expect(html).toContain(tx('settings.network.interfaceDown'));
  });

  it('edit form displays and submits the saved interface without losing URL or other settings', async () => {
    const base = sub({ proxyBindInterface: 'wlan0', userAgent: 'Polaris/test', updateViaProxy: true });
    const html = render(base);
    expect(html).toContain('>Wi-Fi · wlan0</span>');
    expect(html).toContain('value="https://example.com/sub"');
    expect(probe.submit).not.toBeNull();
    probe.submit?.();
    await vi.waitFor(() => expect(probe.update).toHaveBeenCalledExactlyOnceWith(base));
    expect(probe.load).toHaveBeenCalledOnce();
  });

  it('an unavailable saved interface stays visible, disabled and preserved until explicitly changed', async () => {
    const base = sub({ proxyBindInterface: 'old-wifi' });
    const html = render(base);
    expect(html).toContain('<option value="old-wifi" disabled="" selected="">');
    expect(html).toContain(tx('settings.network.interfaceUnavailable', { name: 'old-wifi' }));
    probe.submit?.();
    await vi.waitFor(() => expect(probe.update).toHaveBeenCalledWith({ ...base, updateViaProxy: false }));
  });

  it('inherited selection writes no per-subscription override', async () => {
    render(sub());
    probe.submit?.();
    await vi.waitFor(() => expect(probe.update).toHaveBeenCalledOnce());
    expect(probe.update.mock.calls[0][0].proxyBindInterface).toBeUndefined();
  });

  it('enumeration failure is visible and does not erase an existing choice', () => {
    probe.items = [];
    probe.failed = true;
    const html = render(sub({ proxyBindInterface: 'old-wifi' }));
    expect(html).toContain(tx('settings.network.interfaceListFailed'));
    expect(html).toContain(tx('settings.network.interfaceUnavailable', { name: 'old-wifi' }));
  });

  it('enumeration pending disables the selector only when there are no cached interfaces', () => {
    probe.items = [];
    probe.loading = true;
    const html = render();
    expect(html).toMatch(/<button[^>]*id="msf-interface"[^>]*disabled=""/);
    probe.items = [{ name: 'wlan0', displayName: 'Wi-Fi', isUp: true, addresses: [] }];
    expect(render()).not.toMatch(/<button[^>]*id="msf-interface"[^>]*disabled=""/);
  });

  it('enabled per-sub auto update discloses that the global scheduler is disabled', () => {
    const html = render(sub({ autoUpdate: true }), { autoUpdateSubscriptionOnStart: false });
    expect(html).toContain(tx('sub.autoUpdateNoticeMasterOff'));
    expect(render(sub({ autoUpdate: false }))).not.toContain('m-form-auto-update-notice');
  });

  it.each([false, true])('scheduled notice shows the actual interval and apply policy (auto-apply=%s)', (apply) => {
    const html = render(sub({ autoUpdate: true }), {
      autoUpdateSubscriptionOnStart: true, subscriptionUpdateIntervalHours: 6, restartOnNodeChange: apply,
    });
    const key = apply ? 'sub.autoUpdateNoticeScheduledAutoApply' : 'sub.autoUpdateNoticeScheduledSelective';
    expect(html).toContain(tx(key, { h: 6 }));
    expect(html).not.toContain(tx('sub.autoUpdateNoticeMasterOff'));
  });

  it('subscription menu says Edit while preserving the full editor action', () => {
    const onEditUrl = vi.fn();
    const noop = () => {};
    const item = buildSubItems(i18n.t, { onEditUrl, onRename: noop, onCopyUrl: noop, onInterval: noop, onDelete: noop })
      .find((entry) => entry.id === 'edit-url');
    expect(item?.label).toBe(i18n.t('common.edit'));
    item?.onSelect?.();
    expect(onEditUrl).toHaveBeenCalledOnce();
  });
});
