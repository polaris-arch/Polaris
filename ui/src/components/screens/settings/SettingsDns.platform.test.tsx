import { isValidElement, type ComponentProps } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { beforeEach, expect, it, vi } from 'vitest';
import type { DnsConfig, UserConfig } from '@/contracts/types';
import { DEMO_CONFIG } from '../../../../harness-fixture';

type SwitchProps = ComponentProps<typeof import('./Primitives').Switch>;
const platform = vi.hoisted(() => ({ value: undefined as string | undefined }));
const controls = vi.hoisted((): SwitchProps[] => []);
vi.mock('./settings-logic', async (original) => ({
  ...await original<typeof import('./settings-logic')>(),
  shellPlatformFromDataOs: () => platform.value,
}));
vi.mock('./Primitives', async (original) => {
  const actual = await original<typeof import('./Primitives')>();
  return {
    ...actual,
    SetRow: (props: ComponentProps<typeof actual.SetRow>) => {
      // Other rows contain browser portal editors, outside this switch's contract.
      if (props.label !== 'settings.advanced.takeoverSystemDns') return null;
      if (!isValidElement<SwitchProps>(props.children) || props.children.type !== actual.Switch) {
        throw new Error('HOST_DNS_SWITCH_MISSING');
      }
      controls.push(props.children.props);
      return <actual.SetRow {...props} />;
    },
  };
});
vi.mock('react-i18next', async (original) => ({
  ...await original<typeof import('react-i18next')>(),
  useTranslation: () => ({ t: (key: string) => key }),
}));
await (await import('@/i18n')).i18nReady;
const { default: SettingsDns } = await import('./SettingsDns');

beforeEach(() => { platform.value = undefined; controls.length = 0; });
it.each(['win', 'mac', 'lin', undefined])('system DNS takeover on %s preserves saved preference and gates the actual handler', (os) => {
  platform.value = os;
  const update = vi.fn(async () => {});
  const dns: DnsConfig = {
    domesticDns: 'https://223.5.5.5/dns-query', foreignDns: 'https://1.1.1.1/dns-query',
    enableFakeIp: true, takeoverSystemDns: true,
  };
  const config: UserConfig = { ...DEMO_CONFIG, dnsConfig: dns };
  // A real renderer installs the dispatcher used by external Zustand modules too.
  // Capture the real Switch's props while rendering the actual row and control.
  renderToStaticMarkup(<SettingsDns config={config} update={update} section="runtime" />);
  expect(controls).toHaveLength(1);
  const [control] = controls;
  if (!control) throw new Error('HOST_DNS_SWITCH_MISSING');
  expect(control.checked).toBe(true);
  expect(control.disabled).toBe(os === 'win');
  expect(control.tip).toBe(os === 'win' ? 'settings.dns.takeoverSystemDnsUnsupported' : undefined);
  control.onChange(false);
  if (os === 'win') expect(update).not.toHaveBeenCalled();
  else expect(update).toHaveBeenCalledWith({ dnsConfig: { ...dns, takeoverSystemDns: false } });
});
