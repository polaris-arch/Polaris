import type { TFunction } from 'i18next';
import type {
  BuiltinDhcpStatus,
  DnsPolicyAction,
  DnsServerGroup,
  DnsServerResource,
} from '@/contracts/types';
import type { CselGroup, CselOption } from './Csel';
import { BUILTIN_NETENV_DHCP_ID, probeReasonKey } from '@/domain/network-profile';

export type DnsResponseChoice = 'fakeIp' | 'reject' | 'predefined';

export function dnsActionChoice(action: DnsPolicyAction | undefined): string | null {
  if (!action) return null;
  switch (action.type) {
    case 'server': return `server:${action.serverId}`;
    case 'group': return `group:${action.groupId}`;
    case 'hostsFirst': return `hosts:${action.hostsServerId}`;
    case 'fakeIp': return 'fakeIp';
    case 'reject': return 'reject';
    case 'followRouteDefault': return 'followRouteDefault';
    case 'predefined': return 'predefined';
  }
}

export function dnsActionFromChoice(
  choice: string,
  fallbackChoice: string,
  predefined?: Extract<DnsPolicyAction, { type: 'predefined' }>,
): DnsPolicyAction {
  if (choice.startsWith('server:')) return { type: 'server', serverId: choice.slice(7) };
  if (choice.startsWith('group:')) return { type: 'group', groupId: choice.slice(6) };
  if (choice.startsWith('hosts:')) {
    const fallback: DnsPolicyAction = fallbackChoice.startsWith('server:')
      ? { type: 'server', serverId: fallbackChoice.slice(7) }
      : fallbackChoice.startsWith('group:')
        ? { type: 'group', groupId: fallbackChoice.slice(6) }
        : fallbackChoice === 'reject'
          ? { type: 'reject', method: 'default' }
          : { type: 'fakeIp' };
    return { type: 'hostsFirst', hostsServerId: choice.slice(6), fallback };
  }
  if (choice === 'fakeIp') return { type: 'fakeIp' };
  if (choice === 'reject') return { type: 'reject', method: 'default' };
  if (choice === 'predefined') {
    return predefined ?? { type: 'predefined', rcode: 'NOERROR', answer: [], ns: [], extra: [] };
  }
  return { type: 'followRouteDefault' };
}

interface BuildDnsActionGroupsArgs {
  servers: readonly DnsServerResource[];
  groups: readonly DnsServerGroup[];
  nodes?: readonly { id: string; name: string }[];
  t: TFunction;
  currentValue?: string;
  includeHosts?: boolean;
  responses?: readonly DnsResponseChoice[];
  /**
   * 列出内置解析器「当前网络 DHCP 下发的 DNS」（`builtin-netenv-dhcp` → 内核 `dns-netenv`，spec D5）。
   * 它不是 `dnsServers` 里的资源（生成器按需产出），故不在资源列表里，需显式加。只给规则主动作用。
   */
  includeNetenv?: boolean;
  /**
   * 该内置解析器在本机是否可用（`api.networkProfile.builtinDhcpStatus()`，与生成侧同一判据）。
   * 不可用 ⇒ 置灰并标原因；`null`/缺省 = 拿不到结果 ⇒ 不置灰（不知道就不猜）。
   */
  netenvStatus?: BuiltinDhcpStatus | null;
}

export function dnsServerDisplayName(server: DnsServerResource, t: TFunction): string {
  if (server.id === 'builtin-domestic') return t('settings.dns.builtinDomesticName');
  if (server.id === 'builtin-remote') return t('settings.dns.builtinRemoteName');
  if (server.id === 'builtin-bootstrap') return t('settings.dns.builtinBootstrapName');
  return server.name;
}

/**
 * 内置解析器「当前网络 DHCP 下发的 DNS」的下拉项。不可用时置灰并把原因写进说明 ——
 * 已选中它的旧规则仍能在触发框里看到这个值（Csel 按 value 取 label，置灰不影响回显），不会被悄悄清空。
 */
export function netenvOption(t: TFunction, status: BuiltinDhcpStatus | null | undefined): CselOption {
  const base = {
    value: `server:${BUILTIN_NETENV_DHCP_ID}`,
    label: netenvDnsDisplayName(t),
  };
  if (status && !status.available) {
    return {
      ...base,
      description: `${t('rules.dnsActionUnavailable')} · ${t(probeReasonKey(status.reason))}`,
      disabled: true,
    };
  }
  return { ...base, description: t('rules.networkProfile.dnsNetenvDesc') };
}

/** 内置解析器「当前网络 DHCP 下发的 DNS」的显示名（规则列表与动作下拉共用）。 */
export function netenvDnsDisplayName(t: TFunction): string {
  return t('rules.networkProfile.dnsNetenvName');
}

function dnsOutboundDescription(
  server: DnsServerResource,
  nodes: readonly { id: string; name: string }[],
  t: TFunction,
): string {
  const outbound = server.outbound;
  if (outbound.type === 'direct') return t('settings.dns.outboundDirect');
  if (outbound.type === 'currentExit') return t('settings.dns.outboundCurrentExit');
  const node = nodes.find((candidate) => candidate.id === outbound.nodeId);
  return t('settings.dns.outboundNode', {
    name: node?.name ?? t('rules.dnsActionMissingNode'),
  });
}

export function dnsServerDescription(
  server: DnsServerResource,
  nodes: readonly { id: string; name: string }[],
  t: TFunction,
): string {
  const type = t(`settings.dns.serverType_${server.type}`);
  if (server.type === 'local' || server.type === 'hosts') return type;
  const endpoint = server.endpoint;
  const host = endpoint?.host?.trim() || t('rules.dnsActionEndpointMissing');
  const port = endpoint?.port ? `:${endpoint.port}` : '';
  const path = server.type === 'https' ? (endpoint?.path || '/dns-query') : '';
  return `${type} · ${host}${port}${path} · ${dnsOutboundDescription(server, nodes, t)}`;
}

function dnsGroupDescription(
  group: DnsServerGroup,
  servers: readonly DnsServerResource[],
  nodes: readonly { id: string; name: string }[],
  t: TFunction,
): string {
  const mode = t(group.mode === 'race' ? 'settings.dns.groupRace' : 'settings.dns.groupFallback');
  const members = group.members
    .map((id) => servers.find((server) => server.id === id))
    .filter((server): server is DnsServerResource => server != null);
  const exits = new Set(members.map((server) => dnsOutboundDescription(server, nodes, t)));
  const exit = exits.size > 1
    ? t('rules.dnsActionMixedOutbound')
    : exits.values().next().value ?? t('rules.dnsActionNoMembers');
  return `${mode} · ${t('rules.dnsWorkspace.memberCount', { count: group.members.length })} · ${exit}`;
}

function resourceOption(
  value: string,
  label: string,
  description: string,
  enabled: boolean,
  t: TFunction,
): CselOption {
  return {
    value,
    label,
    description: enabled ? description : `${t('rules.dnsActionUnavailable')} · ${description}`,
    disabled: !enabled,
  };
}

function missingOption(value: string, kind: 'server' | 'group' | 'hosts', t: TFunction): CselOption {
  return {
    value,
    label: t(
      kind === 'group'
        ? 'rules.dnsActionMissingGroup'
        : kind === 'hosts'
          ? 'rules.dnsActionMissingHosts'
          : 'rules.dnsActionMissingServer',
      { id: value.slice(value.indexOf(':') + 1) },
    ),
    description: t('rules.dnsActionUnavailable'),
    disabled: true,
  };
}

/**
 * DNS 动作候选的唯一构建器。规则主动作、Hosts fallback 与未命中默认动作只通过参数裁剪能力，
 * 不再各自拼接 Server/Group/FakeIP 列表。
 */
export function buildDnsActionGroups({
  servers,
  groups,
  nodes = [],
  t,
  currentValue = '',
  includeHosts = true,
  responses = ['fakeIp', 'reject', 'predefined'],
  includeNetenv = false,
  netenvStatus = null,
}: BuildDnsActionGroupsArgs): CselGroup[] {
  const networkOptions = servers
    .filter((server) => server.type !== 'hosts')
    .map((server) => resourceOption(
      `server:${server.id}`,
      dnsServerDisplayName(server, t),
      dnsServerDescription(server, nodes, t),
      server.enabled,
      t,
    ));
  if (includeNetenv) {
    networkOptions.push(netenvOption(t, netenvStatus));
  }
  const groupOptions = groups.map((group) => resourceOption(
    `group:${group.id}`,
    group.name,
    dnsGroupDescription(group, servers, nodes, t),
    group.enabled,
    t,
  ));
  const hostsOptions = includeHosts
    ? servers
      .filter((server) => server.type === 'hosts')
      .map((server) => resourceOption(
        `hosts:${server.id}`,
        dnsServerDisplayName(server, t),
        dnsServerDescription(server, nodes, t),
        server.enabled,
        t,
      ))
    : [];

  if (currentValue.startsWith('server:') && !networkOptions.some((option) => option.value === currentValue)) {
    networkOptions.push(missingOption(currentValue, 'server', t));
  }
  if (currentValue.startsWith('group:') && !groupOptions.some((option) => option.value === currentValue)) {
    groupOptions.push(missingOption(currentValue, 'group', t));
  }
  if (includeHosts && currentValue.startsWith('hosts:') && !hostsOptions.some((option) => option.value === currentValue)) {
    hostsOptions.push(missingOption(currentValue, 'hosts', t));
  }

  const responseOptions: CselOption[] = responses.map((value) => ({
    value,
    label: t(
      value === 'fakeIp'
        ? 'rules.dnsActionFakeIp'
        : value === 'reject'
          ? 'rules.dnsActionReject'
          : 'rules.dnsActionPredefined',
    ),
    description: t(
      value === 'fakeIp'
        ? 'rules.dnsActionDescription_fakeIp'
        : value === 'reject'
          ? 'rules.dnsActionDescription_reject'
          : 'rules.dnsActionDescription_predefined',
    ),
    danger: value === 'reject',
  }));
  if (currentValue === 'followRouteDefault') {
    responseOptions.unshift({
      value: currentValue,
      label: t('settings.dns.defaultAdvanced', { type: currentValue }),
      description: t('rules.dnsActionLegacy'),
      disabled: true,
    });
  }

  return [
    { label: t('rules.dnsActionGroupHeading'), options: groupOptions },
    { label: t('rules.dnsActionServerHeading'), options: networkOptions },
    ...(includeHosts ? [{ label: t('rules.dnsActionHostsHeading'), options: hostsOptions }] : []),
    { label: t('rules.dnsActionResponseHeading'), options: responseOptions },
  ].filter((group) => group.options.length > 0);
}
