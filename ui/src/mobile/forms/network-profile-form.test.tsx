/**
 * 移动端网络场景表单（`NetworkProfileFormPanel`）的**渲染期**判据。
 *
 * 本仓 vitest 是 `environment:'node'`，`renderToStaticMarkup` 不跑 effect ⇒ 两条后端取数
 * （`useResolvedProbes` / `useBuiltinDhcpStatus`）与配置读取（`useConfig`）在这里换成定值探针。
 * 只换**取数**，不换判据：显示态（`probeDisplay` / `probeInputsDiffer` / `probeWarningKey`）、
 * 文案函数（`probeDisplayText`）、原因措辞映射（`mobileReasonKey`）全是真身，i18n 用真 en-US 词典 ——
 * 断言比的是用户真的会读到的那句话。
 *
 * 守的事（spec §4.6 / 本批 brief「Android 上 dhcp 恒不可用，UI 必须如实显示原因而不是提供一个点了无效的选项」）：
 *  ① 本机 DHCP 不可用 ⇒ DHCP 那一档**置灰**、下面一行写原因（手机上那一句，不是桌面 Linux 那句）；
 *  ② 已经选着 DHCP 的存量场景（桌面备份导进来）⇒ 那一档照常回显、不置灰，「本机将使用」一行红字写原因；
 *  ③ 本机可用 / 拿不到结果 ⇒ 不置灰、不写原因（不知道就不猜）；
 *  ④ 读取态：读失败带重试、编辑的场景已被删 ⇒ 说清楚。
 */
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { BuiltinDhcpStatus, NetworkProfile, ResolvedProbe, UserConfig } from '@/contracts/types';
import i18n, { i18nReady } from '@/i18n';

const probe = vi.hoisted(() => ({
  config: null as UserConfig | null,
  loading: false,
  dhcp: null as BuiltinDhcpStatus | null,
  resolved: null as ResolvedProbe[] | null,
}));

vi.mock('@/components/screens/settings/use-config', () => ({
  useConfig: () => ({
    config: probe.config,
    loading: probe.loading,
    error: null,
    update: () => Promise.resolve(),
    reload: () => Promise.resolve(),
  }),
}));
vi.mock('@/components/dialogs/rule-effect-state', async (orig) => ({
  ...(await orig<typeof import('@/components/dialogs/rule-effect-state')>()),
  useBuiltinDhcpStatus: () => probe.dhcp,
}));
vi.mock('@/components/screens/rules/network-profile-probes', async (orig) => ({
  ...(await orig<typeof import('@/components/screens/rules/network-profile-probes')>()),
  useResolvedProbes: () => probe.resolved,
}));

const { NetworkProfileFormPanel } = await import('./NetworkProfileFormPanel');

beforeAll(async () => {
  await i18nReady;
  await i18n.changeLanguage('en-US');
});

const PROFILE: NetworkProfile = {
  id: 'np-1',
  name: 'Office',
  enabled: true,
  match: { dnsServerCidrs: ['10.0.0.0/8'] },
  probe: 'dhcp',
};

beforeEach(() => {
  probe.config = { networkProfiles: [PROFILE] } as unknown as UserConfig;
  probe.loading = false;
  probe.dhcp = null;
  probe.resolved = null;
});

const en = (key: string, vars?: Record<string, unknown>): string => i18n.t(key, vars ?? {}) as string;
const esc = (s: string): string => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
const MOBILE_REASON = (): string => en('mobileRules.networkProfile.reasonDhcpNoPermission');
const DESKTOP_REASON = (): string => en('rules.networkProfile.reasonDhcpNeedsPrivilege');
/** 三段里 DHCP 那一颗的开标签。 */
const dhcpButton = (html: string): string => {
  const label = esc(en('rules.networkProfile.probeDhcp'));
  const end = html.indexOf(`>${label}</button>`);
  expect(end, 'DHCP 那一颗没渲染出来 —— 取材面塌了').toBeGreaterThan(-1);
  return html.slice(html.lastIndexOf('<button', end), end);
};

describe('⓪ 自检', () => {
  it('i18n 真的加载了，且两句原因确实不同（否则 ① 的反向断言说明不了什么）', () => {
    expect(MOBILE_REASON()).not.toBe('mobileRules.networkProfile.reasonDhcpNoPermission');
    expect(DESKTOP_REASON()).toContain('Linux');
    expect(MOBILE_REASON()).not.toContain('Linux');
  });
});

describe('① ~ ③ DHCP 那一档与本机可用性', () => {
  it('① 新建 + 本机 DHCP 不可用 ⇒ DHCP 置灰，下面一行写手机上的原因', () => {
    probe.dhcp = { available: false, reason: 'dhcpNeedsPrivilege' };
    const html = renderToStaticMarkup(<NetworkProfileFormPanel instanceId="f1" />);
    expect(dhcpButton(html), 'DHCP 那一档没置灰 —— 用户会选一个永远不生效的探测方式').toContain('disabled');
    expect(html).toContain(esc(MOBILE_REASON()));
    expect(html, '写成了桌面那句（Linux 系统代理 / 改用 TUN）').not.toContain(esc(DESKTOP_REASON()));
    // 另两档不受影响。
    const auto = esc(en('rules.networkProfile.probeAuto'));
    const autoBtn = html.slice(html.lastIndexOf('<button', html.indexOf(`>${auto}</button>`)), html.indexOf(`>${auto}</button>`));
    expect(autoBtn).not.toContain('disabled');
  });

  it('② 编辑一个已经选着 DHCP 的场景 + 本机不可用 ⇒ 照常回显（不置灰），「本机将使用」一行红字写原因', () => {
    probe.dhcp = { available: false, reason: 'dhcpNeedsPrivilege' };
    probe.resolved = [{ profileId: 'np-1', probeSource: 'dhcp', available: false, reason: 'dhcpNeedsPrivilege', matched: null }];
    const html = renderToStaticMarkup(<NetworkProfileFormPanel instanceId="f2" profileId="np-1" />);
    const btn = dhcpButton(html);
    expect(btn).toContain('aria-pressed="true"');
    expect(btn, '存量值被置灰 —— 用户连当前选的是什么都改不回来').not.toContain('disabled');
    const unavailable = esc(en('rules.networkProfile.probeUnavailable', { reason: MOBILE_REASON() }));
    expect(html).toContain(`<p class="m-form-err">${unavailable}</p>`);
  });

  it('③ 本机可用 / 拿不到结果 ⇒ 不置灰、不写原因', () => {
    for (const dhcp of [{ available: true, reason: null }, null] as const) {
      probe.dhcp = dhcp;
      const html = renderToStaticMarkup(<NetworkProfileFormPanel instanceId="f3" />);
      expect(dhcpButton(html)).not.toContain('disabled');
      expect(html).not.toContain(esc(MOBILE_REASON()));
    }
  });

  it('正常可用的存量场景：「本机将使用：系统 DNS」走后端结果（不是渲染端重算）', () => {
    probe.config = { networkProfiles: [{ ...PROFILE, probe: 'auto' }] } as unknown as UserConfig;
    probe.resolved = [{ profileId: 'np-1', probeSource: 'system', available: true, reason: null, matched: null }];
    const html = renderToStaticMarkup(<NetworkProfileFormPanel instanceId="f4" profileId="np-1" />);
    const uses = esc(en('rules.networkProfile.probeUses', { source: en('rules.networkProfile.sourceSystem') }));
    expect(html).toContain(`<p class="m-form-hint">${uses}</p>`);
  });
});

describe('④ 读取态', () => {
  it('读失败 ⇒ 说清楚 + 重试，且不画表单（空表一提交会覆盖盘上全部场景）', () => {
    probe.config = null;
    const html = renderToStaticMarkup(<NetworkProfileFormPanel instanceId="f5" />);
    expect(html).toContain(esc(en('common.configLoadFail')));
    expect(html).toContain(`>${esc(en('common.retry'))}</button>`);
    expect(html).not.toContain('m-form-seg');
  });

  it('加载中 ⇒ 加载中，不给重试', () => {
    probe.config = null;
    probe.loading = true;
    const html = renderToStaticMarkup(<NetworkProfileFormPanel instanceId="f6" />);
    expect(html).toContain(esc(en('common.loading')));
    expect(html).not.toContain(`>${esc(en('common.retry'))}</button>`);
  });

  it('编辑的场景已被删 ⇒ 说「已被删除」，不悄悄变成新建', () => {
    const html = renderToStaticMarkup(<NetworkProfileFormPanel instanceId="f7" profileId="np-gone" />);
    expect(html).toContain(esc(en('rules.networkProfile.missing')));
    expect(html).not.toContain('m-form-seg');
  });
});
