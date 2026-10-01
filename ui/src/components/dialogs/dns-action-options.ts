import type { TFunction } from 'i18next';
import type {
  BuiltinDhcpStatus,
  DnsPolicyAction,
  DnsServerGroup,
  DnsServerResource,
  UserConfig,
} from '@/contracts/types';
/*
 * 🔴 类型取自 `./csel-logic`（纯 `.ts`）而**不是** `./Csel`（`.tsx`）。
 *
 * `Csel.tsx` 的 `CselOption` 与这里用到的 `CselOptionLike` 字段逐字相同（前者只是给桌面渲染器的
 * 同形声明，`CselGroup` 本来就是从 `csel-logic` 再导出的），换源零行为改动 —— 换的是**依赖边**：
 * 本模块被移动端规则表单消费，而 `import type ... from './Csel'` 在词法 import 图上是一条真边
 * （`wiring-completeness.test.ts#buildImportGraph` 与 `mobile-entry.test.ts` 的模块图都按字面量取，
 * 不区分 type-only），它会把整棵桌面下拉组件树拖进移动端闭包 —— 与「取类型一律从小写
 * `field-spec` 而不是大写 `FieldSpec`」是同一条约束。
 */
import type { CselGroup, CselOptionLike } from './csel-logic';
import { BUILTIN_NETENV_DHCP_ID, probeReasonKey } from '@/domain/network-profile';

export type DnsResponseChoice = 'fakeIp' | 'reject' | 'predefined';

/**
 * `predefined` 那三段多行文本 → 记录数组。
 *
 * 住在这里而不是 `RuleDnsEffect.tsx`：它是 `rule-submit.ts` 的入参归一腿，而提交腿要被移动端
 * 规则表单复用；留在 `.tsx` 里等于让移动端为一个五行的纯函数拖进整份桌面 DNS 字段组件。
 * 桌面调用点不变（`RuleDnsEffect.tsx` 原样再导出）。
 */
export function splitDnsRecordLines(raw: string): string[] {
  return raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

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

/** v2 配置里那段默认 DNS 策略（`config.dnsDefaults` 非空时的形状）。 */
export type DnsDefaults = NonNullable<UserConfig['dnsDefaults']>;

/**
 * 默认策略两格的**当前档**。两端共用。
 *
 * 🔴 这两个函数是 2026-09-13 从桌面 `DnsPolicyWorkspace.tsx` 的两处行内表达式搬出来的，
 * 搬的理由与本文件头注那条（类型取自 `csel-logic` 而非 `Csel`）是同一条：移动端 DNS 设置页
 * 要画同样的两格，而从那份 `.tsx` 取不到它们 —— 抄一份就是第二份真值源，且它含两个**缺省约定**
 * （未命中缺省落 FakeIP、Hosts 档的 fallback 缺省落 `directServerId`），两端各写一份时
 * 漂了不会有任何门红。桌面调用点已改成 import，行为逐字不变。
 */
export function dnsDefaultActionChoice(defaults: DnsDefaults): string {
  return dnsActionChoice(defaults.unmatchedAction) ?? 'fakeIp';
}

export function dnsDefaultFallbackChoice(defaults: DnsDefaults): string {
  /* 非 hostsFirst 时这一格不渲染，但**仍要算得出**：它是 `dnsActionFromChoice` 把「Hosts 优先」
     这一档拼回 `DnsPolicyAction` 时的 fallback 实参 —— 算不出就等于选中 Hosts 那一下把
     fallback 悄悄换成了 FakeIP。 */
  const direct = `server:${defaults.directServerId || 'builtin-domestic'}`;
  return defaults.unmatchedAction?.type === 'hostsFirst'
    ? dnsActionChoice(defaults.unmatchedAction.fallback) ?? direct
    : direct;
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
  /**
   * 不可用原因的 i18n 键映射（缺省原样），见 [`netenvOption`]。原因码与判据不动，只换措辞。
   */
  netenvReasonKeyOf?: (reasonKey: string) => string;
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
/**
 * `reasonKeyOf`：原因码 → i18n 键之后再过一道键映射（缺省原样）。同一个原因码在不同客户端上的**措辞**可以不同：
 * `dhcpNeedsPrivilege` 在桌面只出现在 Linux 系统代理模式（文案建议改用 TUN），在手机上恒成立、那条建议不适用。
 */
export function netenvOption(
  t: TFunction,
  status: BuiltinDhcpStatus | null | undefined,
  reasonKeyOf: (reasonKey: string) => string = (key) => key,
): CselOptionLike {
  const base = {
    value: `server:${BUILTIN_NETENV_DHCP_ID}`,
    label: netenvDnsDisplayName(t),
  };
  if (status && !status.available) {
    return {
      ...base,
      description: `${t('rules.dnsActionUnavailable')} · ${t(reasonKeyOf(probeReasonKey(status.reason)))}`,
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
): CselOptionLike {
  return {
    value,
    label,
    description: enabled ? description : `${t('rules.dnsActionUnavailable')} · ${description}`,
    disabled: !enabled,
  };
}

function missingOption(value: string, kind: 'server' | 'group' | 'hosts', t: TFunction): CselOptionLike {
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
  netenvReasonKeyOf,
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
    networkOptions.push(netenvOption(t, netenvStatus, netenvReasonKeyOf));
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

  const responseOptions: CselOptionLike[] = responses.map((value) => ({
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
