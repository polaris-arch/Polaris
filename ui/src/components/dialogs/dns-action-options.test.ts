import { describe, expect, it } from 'vitest';
import type { TFunction } from 'i18next';
import type { DnsServerGroup, DnsServerResource } from '@/contracts/types';
import {
  buildDnsActionGroups,
  dnsActionChoice,
  dnsActionFromChoice,
} from './dns-action-options';

const t = ((key: string, values?: Record<string, unknown>) => {
  if (key === 'rules.dnsWorkspace.memberCount') return `${values?.count} members`;
  if (values?.id) return `${key}:${values.id}`;
  if (values?.name) return `${key}:${values.name}`;
  return key;
}) as TFunction;

const servers: DnsServerResource[] = [
  {
    id: 'direct-a',
    name: 'Direct A',
    enabled: true,
    type: 'https',
    endpoint: { host: '1.1.1.1', port: 443, path: '/dns-query' },
    outbound: { type: 'direct' },
  },
  {
    id: 'proxy-b',
    name: 'Proxy B',
    enabled: true,
    type: 'tls',
    endpoint: { host: '8.8.8.8', port: 853 },
    outbound: { type: 'currentExit' },
  },
  {
    id: 'hosts-a',
    name: 'Hosts A',
    enabled: false,
    type: 'hosts',
    outbound: { type: 'direct' },
  },
];

const groups: DnsServerGroup[] = [{
  id: 'race-a',
  name: 'Race A',
  enabled: true,
  mode: 'race',
  members: ['direct-a', 'proxy-b'],
}];

describe('buildDnsActionGroups', () => {
  it('按服务器组、服务器、Hosts、响应动作分组，并为混合出口组生成元信息', () => {
    const result = buildDnsActionGroups({ servers, groups, t, currentValue: 'group:race-a' });
    expect(result.map((group) => group.label)).toEqual([
      'rules.dnsActionGroupHeading',
      'rules.dnsActionServerHeading',
      'rules.dnsActionHostsHeading',
      'rules.dnsActionResponseHeading',
    ]);
    expect(result[0].options[0].description).toContain('rules.dnsActionMixedOutbound');
    expect(result[1].options.map((option) => option.value)).toEqual(['server:direct-a', 'server:proxy-b']);
    expect(result[2].options[0]).toMatchObject({ value: 'hosts:hosts-a', disabled: true });
  });

  it('fallback 复用同一构建器但排除 Hosts 与危险响应', () => {
    const result = buildDnsActionGroups({
      servers,
      groups,
      t,
      currentValue: 'server:direct-a',
      includeHosts: false,
      responses: ['fakeIp'],
    });
    expect(result.map((group) => group.label)).not.toContain('rules.dnsActionHostsHeading');
    expect(result[result.length - 1]?.options.map((option) => option.value)).toEqual(['fakeIp']);
  });

  it('当前引用缺失时保留不可用回显，不把值静默清空', () => {
    const result = buildDnsActionGroups({ servers, groups, t, currentValue: 'group:missing' });
    expect(result[0].options[result[0].options.length - 1]).toMatchObject({
      value: 'group:missing',
      disabled: true,
      label: 'rules.dnsActionMissingGroup:missing',
    });
  });
});

describe('DNS action choice codec', () => {
  it('Hosts 使用分组后备解析并保持可逆', () => {
    const action = dnsActionFromChoice('hosts:hosts-a', 'group:race-a');
    expect(action).toEqual({
      type: 'hostsFirst',
      hostsServerId: 'hosts-a',
      fallback: { type: 'group', groupId: 'race-a' },
    });
    expect(dnsActionChoice(action)).toBe('hosts:hosts-a');
  });

  it('拒绝可作为 Hosts miss 的明确后备动作', () => {
    expect(dnsActionFromChoice('hosts:hosts-a', 'reject')).toEqual({
      type: 'hostsFirst',
      hostsServerId: 'hosts-a',
      fallback: { type: 'reject', method: 'default' },
    });
  });
});

describe('内置解析器「当前网络 DHCP 下发的 DNS」（spec D5）', () => {
  const netenv = 'server:builtin-netenv-dhcp';
  const values = (groups: ReturnType<typeof buildDnsActionGroups>) =>
    groups.flatMap((group) => group.options.map((option) => option));

  it('includeNetenv 时出现在解析器分组里、可选；选中后映射成 server 动作（保留 id 原样落盘）', () => {
    const groups = buildDnsActionGroups({ servers, groups: [], t, currentValue: netenv, includeNetenv: true });
    const serverGroup = groups.find((group) => group.label === 'rules.dnsActionServerHeading');
    const option = serverGroup?.options.find((o) => o.value === netenv);
    expect(option).toMatchObject({ label: 'rules.networkProfile.dnsNetenvName' });
    expect(option?.disabled).toBeFalsy();
    // 选中它时不能再被当成「已删除的解析器」补一条 missing 项。
    expect(values(groups).filter((o) => o.value === netenv)).toHaveLength(1);
    expect(dnsActionFromChoice(netenv, 'server:builtin-domestic')).toEqual({
      type: 'server',
      serverId: 'builtin-netenv-dhcp',
    });
  });

  it('默认不列（hosts 兜底 / 未命中默认动作不提供它）', () => {
    const groups = buildDnsActionGroups({ servers, groups: [], t });
    expect(values(groups).some((o) => o.value === netenv)).toBe(false);
  });
});

describe('内置 DHCP 解析器在本机不可用（builtinDhcpStatus）', () => {
  const netenv = 'server:builtin-netenv-dhcp';
  const find = (status: Parameters<typeof buildDnsActionGroups>[0]['netenvStatus']) =>
    buildDnsActionGroups({ servers, groups: [], t, currentValue: netenv, includeNetenv: true, netenvStatus: status })
      .flatMap((g) => g.options)
      .filter((o) => o.value === netenv);

  it('不可用 ⇒ 置灰并标原因；已选中时仍是这一项（回显值与原因，不补 missing、不清空）', () => {
    const hits = find({ available: false, reason: 'dhcpNeedsPrivilege' });
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({
      label: 'rules.networkProfile.dnsNetenvName',
      disabled: true,
      description: 'rules.dnsActionUnavailable · rules.networkProfile.reasonDhcpNeedsPrivilege',
    });
  });

  it('可用 / 拿不到结果 ⇒ 不置灰（不知道就不猜）', () => {
    expect(find({ available: true, reason: null })[0].disabled).toBeFalsy();
    expect(find(null)[0].disabled).toBeFalsy();
    expect(find(undefined)[0].description).toBe('rules.networkProfile.dnsNetenvDesc');
  });
});
