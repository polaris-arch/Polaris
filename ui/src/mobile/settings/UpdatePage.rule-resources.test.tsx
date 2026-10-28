import { beforeEach, describe, expect, it, vi } from 'vitest';
import { isValidElement, type ReactElement, type ReactNode } from 'react';
import type { MobileSettingsPageProps } from './settings-pages';
import { ruleResourceUpdatePolicyText } from './rule-resource-update-policy';

vi.mock('react', async (importOriginal) => ({
  ...await importOriginal<typeof import('react')>(),
  useState: (initial: unknown) => [typeof initial === 'function' ? initial() : initial, vi.fn()],
  useRef: (initial: unknown) => ({ current: initial }),
  useEffect: vi.fn(),
  useCallback: (fn: unknown) => fn,
  useSyncExternalStore: (_subscribe: unknown, snapshot: () => unknown) => snapshot(),
}));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock('@/ipc/api-client', () => ({ updateApi: {}, versionApi: {}, systemApi: {} }));

const { UpdatePage } = await import('./UpdatePage');
const { default: SettingsUpdate } = await import('@/components/screens/settings/SettingsUpdate');
type NodeProps = {
  id?: string; children?: ReactNode; control?: ReactNode;
  value?: string; checked?: boolean; onChange?: (value: never) => void;
  'aria-label'?: string;
};
function elements(node: ReactNode): ReactElement<NodeProps>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<NodeProps>(node)) return [];
  return [node, ...elements(node.props.children), ...elements(node.props.control)];
}
const update = vi.fn(() => Promise.resolve());
const commit = vi.fn();
function page(resourceInterval: number | undefined, enabled = true) {
  return UpdatePage({
    config: {
      subscriptionUpdateIntervalHours: 24,
      ruleResourceUpdateIntervalHours: resourceInterval,
      ruleResourceAutoUpdate: enabled,
    }, update, commit,
  } as unknown as MobileSettingsPageProps);
}
function control(tree: ReactNode, id: string): ReactElement<NodeProps> {
  const row = elements(tree).find((node) => node.props.id === id);
  expect(row).toBeDefined();
  expect(isValidElement(row!.props.control)).toBe(true);
  return row!.props.control as ReactElement<NodeProps>;
}
beforeEach(() => vi.clearAllMocks());

describe('mobile rule-resource settings use an independent persisted policy', () => {
  it.each([0, 6])('toggling resources preserves an existing %ih period and subscription period', (interval) => {
    const tree = page(interval, false);
    const toggle = control(tree, 'rule-resource-auto');
    expect(toggle.props.checked).toBe(false);
    toggle.props.onChange!(true as never);
    expect(update).toHaveBeenCalledExactlyOnceWith({ ruleResourceAutoUpdate: true });
    expect(control(tree, 'rule-resource-interval').props.value).toBe(String(interval));
    expect(commit).toHaveBeenCalledWith('rule-resource-auto', expect.any(Promise));
  });
  it('changing subscriptions does not overwrite a distinct resource interval', () => {
    const tree = page(6);
    expect(control(tree, 'update-interval').props.value).toBe('24');
    control(tree, 'update-interval').props.onChange!('0' as never);
    expect(update).toHaveBeenCalledExactlyOnceWith({ subscriptionUpdateIntervalHours: 0 });
  });
  it('changing the resource period writes only its own field, including manual-only', () => {
    const tree = page(6);
    control(tree, 'rule-resource-interval').props.onChange!('0' as never);
    expect(update).toHaveBeenCalledExactlyOnceWith({ ruleResourceUpdateIntervalHours: 0 });
    expect(commit).toHaveBeenCalledWith('rule-resource-interval', expect.any(Promise));
  });
  it('missing resource interval defaults to 12h without copying subscriptions', () => {
    expect(control(page(undefined), 'rule-resource-interval').props.value).toBe('12');
  });
  it.each([
    [{ ruleResourceAutoUpdate: false, ruleResourceUpdateIntervalHours: 6 }, 'mobileSettings.update.ruleResourceOff'],
    [{ ruleResourceAutoUpdate: true, ruleResourceUpdateIntervalHours: 0 }, 'mobileSettings.update.ruleResourceManual'],
    [{ ruleResourceUpdateIntervalHours: 6, subscriptionUpdateIntervalHours: 0 }, 'mobileSettings.update.ruleResourceActive:6'],
    [{}, 'mobileSettings.update.ruleResourceActive:12'],
  ])('settings and resource screen report the actual policy %j', (config, expected) => {
    expect(ruleResourceUpdatePolicyText(
      config as MobileSettingsPageProps['config'],
      (key, vars) => vars ? `${key}:${vars.hours}` : key,
    )).toBe(expected);
  });
});

describe('desktop and mobile cannot overwrite each other’s resource interval', () => {
  function desktop(resourceInterval: number | undefined = 6) {
    return SettingsUpdate({
      config: { subscriptionUpdateIntervalHours: 24, ruleResourceUpdateIntervalHours: resourceInterval },
      update,
    } as unknown as MobileSettingsPageProps);
  }
  function input(tree: ReactNode, label: string) {
    const result = elements(tree).find((node) => node.props['aria-label'] === label);
    expect(result).toBeDefined();
    return result!;
  }
  it.each([0, 6])('resource toggle preserves its %ih setting on desktop', (interval) => {
    const tree = desktop(interval);
    input(tree, 'settings.update.ruleResourceAutoCard').props.onChange!(false as never);
    expect(update).toHaveBeenCalledExactlyOnceWith({ ruleResourceAutoUpdate: false });
    expect(input(tree, 'resources.updateInterval').props.value).toBe(String(interval));
  });
  it('subscription selector writes only subscription policy on desktop', () => {
    input(desktop(), 'settings.update.intervalCard').props.onChange!({ target: { value: '0' } } as never);
    expect(update).toHaveBeenCalledExactlyOnceWith({ subscriptionUpdateIntervalHours: 0 });
  });
  it('resource selector writes only its independent manual policy on desktop', () => {
    input(desktop(), 'resources.updateInterval').props.onChange!({ target: { value: '0' } } as never);
    expect(update).toHaveBeenCalledExactlyOnceWith({ ruleResourceUpdateIntervalHours: 0 });
  });
});
