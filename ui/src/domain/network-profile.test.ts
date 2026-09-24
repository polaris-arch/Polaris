import { describe, expect, it } from 'vitest';
import type { NetworkProfile, ResolvedProbe, Rule } from '@/contracts/types';
import {
  BUILTIN_NETENV_DHCP_ID,
  PROBE_REASON_FALLBACK_KEY,
  PROBE_REASON_KEYS,
  buildNetworkProfile,
  criteriaSummary,
  isValidSearchDomain,
  normalizeSearchDomain,
  orderWithNewRuleFirst,
  probeDisplay,
  probeInputsDiffer,
  probeReasonKey,
  probeWarningKey,
  profileRowStatus,
  PROBE_WARNING_KEYS,
  profileRefCounts,
  ruleProfileBadge,
  validateNetworkProfileDraft,
} from './network-profile';

const profile = (over: Partial<NetworkProfile> = {}): NetworkProfile => ({
  id: 'np-corp',
  name: '公司网络',
  enabled: true,
  match: { dnsServerCidrs: ['10.20.0.0/16'], searchDomains: ['corp.example'] },
  probe: 'auto',
  ...over,
});
const rule = (id: string, networkProfileId?: string): Rule => ({
  id,
  type: 'domainSuffix',
  values: ['corp.example'],
  action: 'direct',
  enabled: true,
  ...(networkProfileId ? { networkProfileId } : {}),
});
const resolved = (over: Partial<ResolvedProbe> = {}): ResolvedProbe => ({
  profileId: 'np-corp',
  probeSource: 'system',
  available: true,
  reason: null,
  ...over,
});

describe('保留 id 与 Rust 同值', () => {
  it('builtin-netenv-dhcp（D5，Rust BUILTIN_NETENV_DHCP_ID）', () => {
    expect(BUILTIN_NETENV_DHCP_ID).toBe('builtin-netenv-dhcp');
  });
});

describe('搜索域规范化与校验（与 Rust normalize_search_domain 同口径）', () => {
  it('去首尾空白与首尾点、小写；空 ⇒ null', () => {
    expect(normalizeSearchDomain('  .Corp.Example. ')).toBe('corp.example');
    expect(normalizeSearchDomain('...')).toBeNull();
    expect(normalizeSearchDomain('   ')).toBeNull();
  });
  it('合法：单标签、多标签、首尾点；非法：通配、空格、冒号、超长标签', () => {
    for (const ok of ['lan', 'corp.example', '.corp.example.', 'eng-1.corp.example']) {
      expect(isValidSearchDomain(ok), ok).toBe(true);
    }
    for (const bad of ['*.corp.example', 'corp example', 'corp:example', `${'a'.repeat(64)}.x`, '-corp.example', '']) {
      expect(isValidSearchDomain(bad), bad).toBe(false);
    }
  });
});

describe('validateNetworkProfileDraft', () => {
  it('名称必填（空白算空）', () => {
    expect(validateNetworkProfileDraft({ name: '  ', cidrs: ['10.0.0.0/8'], domains: [] })).toEqual({ kind: 'name' });
  });
  it('两项判据都空（空行忽略）⇒ noCriteria', () => {
    expect(validateNetworkProfileDraft({ name: 'x', cidrs: ['', ' '], domains: [''] })).toEqual({ kind: 'noCriteria' });
  });
  it('只填一项即可（两种判据之间是「任一命中」）', () => {
    expect(validateNetworkProfileDraft({ name: 'x', cidrs: ['10.0.0.0/8'], domains: [] })).toBeNull();
    expect(validateNetworkProfileDraft({ name: 'x', cidrs: [], domains: ['lan'] })).toBeNull();
  });
  it('报出第一个坏地址段 / 坏搜索域的原值', () => {
    expect(validateNetworkProfileDraft({ name: 'x', cidrs: ['10.0.0.0/8', '10.0.0.0/40'], domains: [] })).toEqual({
      kind: 'cidr',
      value: '10.0.0.0/40',
    });
    expect(validateNetworkProfileDraft({ name: 'x', cidrs: [], domains: ['lan', '*.corp'] })).toEqual({
      kind: 'domain',
      value: '*.corp',
    });
  });
  it('IPv6 地址段与裸 IP 都合法', () => {
    expect(validateNetworkProfileDraft({ name: 'x', cidrs: ['240e::/16', '192.168.10.1'], domains: [] })).toBeNull();
  });
});

describe('buildNetworkProfile', () => {
  it('去空行、搜索域规范化并去重、名称 trim；空数组不写键', () => {
    const p = buildNetworkProfile(
      { name: ' 家 ', cidrs: [' 10.0.0.0/8 ', '', '10.0.0.0/8'], domains: ['Corp.Example.', 'corp.example', ''], enabled: true, probe: 'dhcp' },
      'np-1',
    );
    expect(p).toEqual({
      id: 'np-1',
      name: '家',
      enabled: true,
      match: { dnsServerCidrs: ['10.0.0.0/8'], searchDomains: ['corp.example'] },
      probe: 'dhcp',
    });
    const onlyDomain = buildNetworkProfile({ name: 'x', cidrs: [''], domains: ['lan'], enabled: false, probe: 'auto' }, 'np-2');
    expect(Object.keys(onlyDomain.match)).toEqual(['searchDomains']);
  });
});

describe('profileRefCounts', () => {
  it('两个平面分别计数，只数引用本场景的规则', () => {
    const traffic = [rule('a', 'np-corp'), rule('b'), rule('c', 'np-home')];
    const dns = [rule('d', 'np-corp'), rule('e', 'np-corp')];
    expect(profileRefCounts('np-corp', traffic, dns)).toEqual({ route: 1, dns: 2 });
  });
});

describe('probeWarningKey（告警判据只在后端：dhcpIpv6Only）', () => {
  it('available 且原因为 dhcpIpv6Only ⇒ IPv6 提示文案键', () => {
    expect(probeWarningKey(resolved({ probeSource: 'dhcp', reason: 'dhcpIpv6Only' }))).toBe(
      'rules.networkProfile.ipv6DhcpWarn',
    );
  });
  it('反向对照：无原因 / 不可用（走不可用原因，不当告警）/ 拿不到结果 ⇒ null；前端不按判据重算', () => {
    expect(probeWarningKey(resolved({ probeSource: 'dhcp' }))).toBeNull();
    expect(probeWarningKey(resolved({ available: false, reason: 'dhcpNeedsPrivilege' }))).toBeNull();
    // 防御：不可用时即便带着告警码也不当告警（可用性优先，走不可用那一行）。
    expect(probeWarningKey(resolved({ available: false, reason: 'dhcpIpv6Only' }))).toBeNull();
    expect(probeWarningKey(undefined)).toBeNull();
    // 纯 IPv6 地址段 + dhcp，但后端没报告警 ⇒ 不提示（单一判据在后端）。
    expect(probeWarningKey(resolved({ probeSource: 'dhcp', reason: null }))).toBeNull();
  });
});

describe('probeReasonKey', () => {
  it('N2 最终四个不可用码各有专属文案；告警码不混进不可用表；未知 / null ⇒ 通用文案', () => {
    expect(Object.keys(PROBE_REASON_KEYS).sort()).toEqual(
      ['dhcpMonitorMissing', 'dhcpNeedsPrivilege', 'profileInvalid', 'systemNoSearchDomain'],
    );
    expect(Object.keys(PROBE_WARNING_KEYS)).toEqual(['dhcpIpv6Only']);
    expect(probeReasonKey('dhcpNeedsPrivilege')).toBe(PROBE_REASON_KEYS.dhcpNeedsPrivilege);
    expect(probeReasonKey('systemNoSearchDomain')).toBe(PROBE_REASON_KEYS.systemNoSearchDomain);
    expect(probeReasonKey('profileInvalid')).toBe('rules.networkProfile.reasonProfileInvalid');
    expect(probeReasonKey('dhcpMonitorMissing')).toBe('rules.networkProfile.reasonDhcpMonitorMissing');
    expect(probeReasonKey('somethingNew')).toBe(PROBE_REASON_FALLBACK_KEY);
    expect(probeReasonKey(null)).toBe(PROBE_REASON_FALLBACK_KEY);
    expect(new Set(Object.values(PROBE_REASON_KEYS)).size).toBe(Object.keys(PROBE_REASON_KEYS).length);
  });
});

describe('probeDisplay', () => {
  it('未保存 / 草稿改了判据或探测方式 ⇒ pending（不拿旧结果冒充新结果）', () => {
    expect(probeDisplay(undefined, [resolved()])).toEqual({ kind: 'pending' });
    expect(probeDisplay('np-corp', [resolved()], true)).toEqual({ kind: 'pending' });
  });
  it('拿不到后端结果 ⇒ unknown；后端没有这条 ⇒ pending', () => {
    expect(probeDisplay('np-corp', null)).toEqual({ kind: 'unknown' });
    expect(probeDisplay('np-corp', [resolved({ profileId: 'np-other' })])).toEqual({ kind: 'pending' });
  });
  it('按后端结果：可用 ⇒ ok；不可用 ⇒ unavailable + 原因文案键', () => {
    expect(probeDisplay('np-corp', [resolved({ probeSource: 'dhcp' })])).toEqual({ kind: 'ok', source: 'dhcp' });
    // 告警码不改变可用性：仍是 ok（告警另走 probeWarningKey）。
    expect(probeDisplay('np-corp', [resolved({ probeSource: 'dhcp', reason: 'dhcpIpv6Only' })])).toEqual({
      kind: 'ok',
      source: 'dhcp',
    });
    expect(
      probeDisplay('np-corp', [resolved({ probeSource: 'dhcp', available: false, reason: 'dhcpNeedsPrivilege' })]),
    ).toEqual({ kind: 'unavailable', source: 'dhcp', reasonKey: PROBE_REASON_KEYS.dhcpNeedsPrivilege });
  });
});

describe('probeInputsDiffer', () => {
  it('看判据、探测方式与启停（后端对停用场景报 profileInvalid）；改名称不算', () => {
    const saved = profile();
    expect(probeInputsDiffer(saved, { ...saved, name: '别名' })).toBe(false);
    expect(probeInputsDiffer(saved, { ...saved, enabled: false })).toBe(true);
    expect(probeInputsDiffer(saved, { ...saved, probe: 'dhcp' })).toBe(true);
    expect(probeInputsDiffer(saved, { ...saved, match: { dnsServerCidrs: ['10.20.0.0/16'] } })).toBe(true);
    expect(probeInputsDiffer(undefined, saved)).toBe(true);
  });
});

describe('ruleProfileBadge', () => {
  const profiles = [profile(), profile({ id: 'np-off', name: '停用的', enabled: false })];
  it('不挂场景 ⇒ null', () => {
    expect(ruleProfileBadge(rule('r'), profiles, [])).toBeNull();
  });
  it('场景已删除 ⇒ missing（规则不生效，fail-closed）', () => {
    expect(ruleProfileBadge(rule('r', 'np-gone'), profiles, [])).toEqual({ state: 'missing' });
  });
  it('场景已停用 ⇒ disabled（优先于探测源结果）', () => {
    expect(ruleProfileBadge(rule('r', 'np-off'), profiles, [resolved({ profileId: 'np-off' })])).toEqual({
      state: 'disabled',
      name: '停用的',
    });
  });
  it('探测源不可用 ⇒ unavailable；可用 ⇒ ok + 后端给的探测源；拿不到结果 ⇒ ok 无探测源', () => {
    expect(
      ruleProfileBadge(rule('r', 'np-corp'), profiles, [resolved({ available: false, reason: 'systemNoSearchDomain' })]),
    ).toEqual({ state: 'unavailable', name: '公司网络', reasonKey: PROBE_REASON_KEYS.systemNoSearchDomain });
    expect(ruleProfileBadge(rule('r', 'np-corp'), profiles, [resolved({ probeSource: 'dhcp' })])).toEqual({
      state: 'ok',
      name: '公司网络',
      source: 'dhcp',
    });
    expect(ruleProfileBadge(rule('r', 'np-corp'), profiles, null)).toEqual({ state: 'ok', name: '公司网络', source: undefined });
  });
  it('后端告警 dhcpIpv6Only ⇒ warning（规则照常生成，但判据永不命中）', () => {
    expect(
      ruleProfileBadge(rule('r', 'np-corp'), profiles, [resolved({ probeSource: 'dhcp', reason: 'dhcpIpv6Only' })]),
    ).toEqual({ state: 'warning', name: '公司网络', warningKey: 'rules.networkProfile.ipv6DhcpWarn' });
  });
});

describe('criteriaSummary', () => {
  it('两类各计数，样例取前两个值', () => {
    expect(criteriaSummary(profile({ match: { dnsServerCidrs: ['a', 'b'], searchDomains: ['c'] } }))).toEqual({
      cidrs: 2,
      domains: 1,
      sample: 'a, b',
    });
    expect(criteriaSummary(profile({ match: {} }))).toEqual({ cidrs: 0, domains: 0, sample: '' });
  });
});

describe('orderWithNewRuleFirst（spec §3.4-4：新建带场景的规则插到最前）', () => {
  it('新 id 在首位，其余按持久化顺序、不在序里的按集合顺序补在后面', () => {
    expect(orderWithNewRuleFirst(['a', 'b', 'c', 'new'], ['c', 'a'], 'new')).toEqual(['new', 'c', 'a', 'b']);
  });
  it('持久化顺序里的陈旧 id 与重复 id 被丢弃；结果恰是集合的一个排列（rules_reorder 的前提）', () => {
    const out = orderWithNewRuleFirst(['a', 'b', 'new'], ['gone', 'b', 'b', 'a'], 'new');
    expect(out).toEqual(['new', 'b', 'a']);
    expect([...out].sort()).toEqual(['a', 'b', 'new']);
  });
  it('新 id 尚不在集合里（直写腿：先算后 add 之前的集合）也只出现一次', () => {
    expect(orderWithNewRuleFirst(['a', 'b'], ['b', 'a'], 'new')).toEqual(['new', 'b', 'a']);
  });
});

describe('profileRowStatus（停用场景只显示「已停用」）', () => {
  it('停用 ⇒ disabled，不带探测结果（即便后端给了 profileInvalid）', () => {
    expect(
      profileRowStatus(profile({ enabled: false }), [resolved({ available: false, reason: 'profileInvalid' })]),
    ).toEqual({ kind: 'disabled' });
  });
  it('启用 ⇒ 后端结果 + 告警', () => {
    expect(profileRowStatus(profile(), [resolved({ probeSource: 'dhcp', reason: 'dhcpIpv6Only' })])).toEqual({
      kind: 'probe',
      display: { kind: 'ok', source: 'dhcp' },
      warningKey: 'rules.networkProfile.ipv6DhcpWarn',
    });
    expect(profileRowStatus(profile(), null)).toEqual({ kind: 'probe', display: { kind: 'unknown' }, warningKey: null });
  });
});
