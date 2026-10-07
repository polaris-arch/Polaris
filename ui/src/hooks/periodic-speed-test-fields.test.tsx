/**
 * 「订阅里开着周期测速、全局却关着」这条提示：四种组合下出不出现，以及就地打开全局开关。
 * 桌面弹窗与移动端表单共用同一个钩子，提示组件各有一份；两边各渲染一遍。
 */
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { SpeedTestScheduleStatus } from '@/contracts/speed-test';
import type { SubscriptionConfig, UserConfig } from '@/contracts/types';
import i18n, { i18nReady } from '@/i18n';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';

const LIMITS: NonNullable<SpeedTestScheduleStatus['limits']> = {
  concurrencyMin: 4,
  concurrencyMax: 32,
  intervalMinutesMin: 5,
  intervalMinutesMax: 360,
  intervalMinutesDefault: 30,
  enabledDefault: true,
  subscriptionDefault: false,
  meteredPolicyDefault: 'reduced',
};

// SSR 不跑 effect：计划状态与网卡枚举只替换数据来源；表单、钩子里的判据与提示组件都是真的。
const probe = vi.hoisted(() => ({
  limits: undefined as unknown,
  enable: null as (() => void | Promise<void>) | null,
  commit: null as (() => unknown) | null,
  patch: vi.fn<(patch: Partial<UserConfig>) => Promise<UserConfig>>(),
}));

vi.mock('@/hooks/use-speed-test-schedule', () => ({
  useSpeedTestSchedule: () => (probe.limits ? { state: 'disabled', reason: null, stalled: false, limits: probe.limits } : null),
}));
vi.mock('@/hooks/use-network-interfaces', async (original) => ({
  ...(await original<typeof import('@/hooks/use-network-interfaces')>()),
  useNetworkInterfaces: () => ({ items: [], loading: false, failed: false }),
}));
vi.mock('@/components/dialogs/PeriodicGlobalOffHint', async (original) => {
  const real = await original<typeof import('@/components/dialogs/PeriodicGlobalOffHint')>();
  return {
    PeriodicGlobalOffHint: (props: Parameters<typeof real.PeriodicGlobalOffHint>[0]) => {
      probe.enable = props.onEnable;
      return <real.PeriodicGlobalOffHint {...props} />;
    },
  };
});
vi.mock('@/mobile/forms/PeriodicGlobalOffHint', async (original) => {
  const real = await original<typeof import('@/mobile/forms/PeriodicGlobalOffHint')>();
  return {
    PeriodicGlobalOffHint: (props: Parameters<typeof real.PeriodicGlobalOffHint>[0]) => {
      probe.enable = props.onEnable;
      return <real.PeriodicGlobalOffHint {...props} />;
    },
  };
});
vi.mock('@/hooks/use-periodic-speed-test-fields', async (original) => {
  const real = await original<typeof import('@/hooks/use-periodic-speed-test-fields')>();
  return {
    usePeriodicSpeedTestFields: (...args: Parameters<typeof real.usePeriodicSpeedTestFields>) => {
      const fields = real.usePeriodicSpeedTestFields(...args);
      probe.commit = fields.commit;
      return fields;
    },
  };
});
vi.mock('@/ipc', async (original) => {
  const real = await original<typeof import('@/ipc')>();
  return { ...real, api: { ...real.api, config: { ...real.api.config, patch: probe.patch } } };
});

const { SubFormPanel } = await import('@/mobile/forms/SubFormPanel');
const { SubDialog } = await import('@/components/dialogs/SubDialog');

beforeAll(async () => {
  await i18nReady;
  await i18n.changeLanguage('en-US');
});

const sub = (periodicSpeedTest?: boolean): SubscriptionConfig => ({
  id: 'sub-1',
  name: 'Airport',
  url: 'https://example.com/sub',
  autoUpdate: false,
  createdAt: '2026-10-08',
  periodicSpeedTest,
});

function seed(config: Partial<UserConfig>): void {
  useAppStore.setState({ config: config as UserConfig });
  Object.assign(useAppStore.getInitialState(), useAppStore.getState());
}

const surfaces = {
  mobile: () => renderToStaticMarkup(<SubFormPanel instanceId="sub-form" subId="sub-1" />),
  desktop: () => renderToStaticMarkup(<SubDialog instanceId="sub-dialog" subId="sub-1" />),
};

beforeEach(() => {
  probe.limits = LIMITS;
  probe.enable = null;
  probe.commit = null;
  probe.patch.mockReset();
  useStagedConfigStore.setState({ entries: [] });
  Object.assign(useStagedConfigStore.getInitialState(), useStagedConfigStore.getState());
});

describe.each(Object.entries(surfaces))('订阅表单的全局关闭提示（%s）', (_name, render) => {
  const hint = () => i18n.t('sub.periodicGlobalOff');
  const action = () => i18n.t('sub.periodicGlobalOn');

  it.each([
    { global: true, subscription: true, shown: false },
    { global: true, subscription: false, shown: false },
    { global: false, subscription: false, shown: false },
    { global: false, subscription: true, shown: true },
  ])('全局 $global × 订阅 $subscription → 提示 $shown', ({ global, subscription, shown }) => {
    seed({ periodicSpeedTestEnabled: global, subscriptions: [sub(subscription)] });
    const html = render();
    expect(html.includes(hint())).toBe(shown);
    expect(html.includes(action())).toBe(shown);
    // 订阅自己的开关始终可操作，显示的是用户的选择。
    expect(html).toMatch(
      new RegExp(`role="switch"[^>]*aria-checked="${subscription}"[^>]*(msf-periodic-l|Periodic speed test)`),
    );
    expect(html).not.toMatch(/role="switch"[^>]*(msf-periodic-l|Periodic speed test)[^>]*disabled/);
  });

  it('两个开关都没设过时取后端给的缺省：全局开、订阅关，不提示', () => {
    seed({ subscriptions: [sub(undefined)] });
    expect(render()).not.toContain(hint());
    // 缺省值只有后端那一处：后端把每订阅的缺省改成开、全局改成关，提示随之出现。
    probe.limits = { ...LIMITS, enabledDefault: false, subscriptionDefault: true };
    expect(render()).toContain(hint());
  });

  it('点「开启全局周期测速」直接把全局开关打开并保存，提示随之消失', async () => {
    seed({ periodicSpeedTestEnabled: false, subscriptions: [sub(true)] });
    probe.patch.mockImplementation(async (patch) => ({
      ...(useAppStore.getState().config as UserConfig),
      ...patch,
    }));
    expect(render()).toContain(hint());
    expect(probe.enable).not.toBeNull();

    await probe.enable?.();

    expect(probe.patch).toHaveBeenCalledExactlyOnceWith({ periodicSpeedTestEnabled: true });
    await vi.waitFor(() => expect(useAppStore.getState().config?.periodicSpeedTestEnabled).toBe(true));
    Object.assign(useAppStore.getInitialState(), useAppStore.getState());
    expect(render()).not.toContain(hint());
  });

  it('开关关着时周期输入框不可见：里面的非法值不拦提交，写回的仍是已保存的值', () => {
    // SSR 下没法先开后关再填字；已保存的越界值让输入框的初始文本就是非法的，走的是同一条提交路径。
    seed({ subscriptions: [{ ...sub(false), speedTestIntervalMinutes: 2 }] });
    expect(render()).not.toMatch(/id="(msf|sub)-speed-interval"/);
    expect(probe.commit?.()).toEqual({ periodicSpeedTest: false, speedTestIntervalMinutes: 2 });

    // 开关开着时同一个值照常拦下。
    seed({ subscriptions: [{ ...sub(true), speedTestIntervalMinutes: 2 }] });
    expect(render()).toMatch(/id="(msf|sub)-speed-interval"[^>]*value="2"/);
    expect(probe.commit?.()).toBeNull();
  });

  it('周期输入框跟着订阅开关：开着才出现，留空的占位是后端给的缺省周期', () => {
    seed({ subscriptions: [sub(false)] });
    expect(render()).not.toMatch(/id="(msf|sub)-speed-interval"/);
    seed({ subscriptions: [{ ...sub(true), speedTestIntervalMinutes: 45 }] });
    const html = render();
    expect(html).toMatch(/id="(msf|sub)-speed-interval"[^>]*value="45"/);
    expect(html).toMatch(/id="(msf|sub)-speed-interval"[^>]*placeholder="30"/);
  });
});
