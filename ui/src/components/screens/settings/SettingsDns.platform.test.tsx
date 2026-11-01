import { isValidElement, type ReactElement } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';

const platform = vi.hoisted(() => ({ value: undefined as string | undefined }));
vi.mock('./settings-logic', async (original) => ({
  ...await original<typeof import('./settings-logic')>(),
  shellPlatformFromDataOs: () => platform.value,
}));
vi.mock('react', async (original) => ({
  ...await original<typeof import('react')>(),
  useState: (value: unknown) => [value, vi.fn()],
  useRef: (value: unknown) => ({ current: value }),
  useEffect: () => {},
}));
vi.mock('react-i18next', async (original) => ({
  ...await original<typeof import('react-i18next')>(),
  useTranslation: () => ({ t: (key: string) => key }),
}));
await (await import('@/i18n')).i18nReady;
const { default: SettingsDns } = await import('./SettingsDns');
const { SetRow, Switch } = await import('./Primitives');

function nodes(node: unknown): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!isValidElement<Record<string, unknown>>(node)) return [];
  return [node, ...nodes(node.props.children)];
}
beforeEach(() => { platform.value = undefined; });
it.each(['win', 'mac', 'lin', undefined])('system DNS takeover on %s preserves saved preference and gates the actual handler', (os) => {
  platform.value = os;
  const update = vi.fn(async () => {});
  const screen = SettingsDns({ config: { dnsConfig: { takeoverSystemDns: true } } as never, update, section: 'runtime' });
  const row = nodes(screen).find((node) => node.type === SetRow && node.props.label === 'settings.advanced.takeoverSystemDns');
  expect(row).toBeDefined();
  const control = nodes(row).find((node) => node.type === Switch)!;
  expect(control.props.checked).toBe(true);
  expect(control.props.disabled).toBe(os === 'win');
  expect(control.props.tip).toBe(os === 'win' ? 'settings.dns.takeoverSystemDnsUnsupported' : undefined);
  (control.props.onChange as (value: boolean) => void)(false);
  if (os === 'win') expect(update).not.toHaveBeenCalled();
  else expect(update).toHaveBeenCalledWith({ dnsConfig: { takeoverSystemDns: false } });
});
