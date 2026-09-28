/**
 * DNS 服务器 / 服务器组的**平台无关逻辑** —— 内置保护、Hosts 记录往返、成员重排、表单校验、
 * 删除前的引用图。
 *
 * # 为什么单独一个 `.ts`
 *
 * 这批判据此前分住在两个 `.tsx` 里（`DnsResourceDialog.tsx` 的校验/往返/重排，
 * `DnsPolicyWorkspace.tsx` 的引用图），而移动端要接同一件事：
 * 从 `.tsx` 取一个纯函数会把整棵桌面组件树拖进移动端闭包（门的 import 图按字面量取，
 * 连 `import type` 都算一条真边）—— 与「取类型一律从小写 `field-spec` 而不是大写 `FieldSpec`」
 * 是同一条约束。
 *
 * 两处**原样再导出**本模块的符号，桌面既有调用点与既有单测（`SettingsDns.test.ts` 从
 * `../rules/DnsPolicyWorkspace` import 引用图）一行都不用改。
 *
 * 🔴 **判据只有这一份**。移动端的 DNS 资源表单与删除护栏必须复用它，不许照抄第二份 ——
 * 引用图是「删了会不会让别的东西失效」的唯一依据，漂了会**静默**放行一次把别处打断的删除。
 */

import type { DnsPolicyAction, DnsServerKind, UserConfig } from '@/contracts/types';
import { isIpLiteral } from '@/domain/ip-literal';

/** 三个内置 DNS 服务器：不可停用、不可删（它们是引导链与两条默认解析腿的载体）。 */
const PROTECTED_DNS_SERVER_IDS = new Set([
  'builtin-domestic',
  'builtin-remote',
  'builtin-bootstrap',
]);

export function isProtectedDnsServer(serverId: string): boolean {
  return PROTECTED_DNS_SERVER_IDS.has(serverId);
}

/** Hosts 内联记录编辑格式：每行 `domain = value1, value2`；坏行跳过。 */
export function parseHostsPredefined(raw: string): Record<string, string[]> {
  const records: Record<string, string[]> = {};
  for (const line of raw.split(/\r?\n/)) {
    const separator = line.indexOf('=');
    if (separator <= 0) continue;
    const domain = line.slice(0, separator).trim();
    const values = line
      .slice(separator + 1)
      .split(',')
      .map((value) => value.trim())
      .filter(Boolean);
    if (domain && values.length > 0) records[domain] = [...new Set(values)];
  }
  return records;
}

export function formatHostsPredefined(records: Record<string, string[]> | undefined): string {
  return Object.entries(records ?? {})
    .map(([domain, values]) => `${domain} = ${values.join(', ')}`)
    .join('\n');
}

export function moveDnsGroupMember(
  members: readonly string[],
  from: number,
  to: number,
): string[] {
  if (from < 0 || from >= members.length || to < 0 || to >= members.length || from === to) {
    return [...members];
  }
  const next = [...members];
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

export type DnsServerFormError =
  | 'name'
  | 'host'
  | 'port'
  | 'bootstrapIp'
  | 'bootstrapMissing';

export function validateDnsServerForm(input: {
  name: string;
  type: DnsServerKind;
  host: string;
  port: string;
  isBootstrap: boolean;
  bootstrapServerId: string;
  validBootstrapServerIds: ReadonlySet<string>;
}): DnsServerFormError | null {
  if (!input.name.trim()) return 'name';
  if (input.type === 'local' || input.type === 'hosts') return null;
  const host = input.host.trim();
  if (!host) return 'host';
  if (input.port.trim()) {
    const port = Number(input.port);
    if (!/^\d+$/.test(input.port.trim()) || !Number.isInteger(port) || port < 1 || port > 65535) {
      return 'port';
    }
  }
  if (input.isBootstrap && !isIpLiteral(host)) return 'bootstrapIp';
  if (
    !input.isBootstrap
    && !isIpLiteral(host)
    && !input.validBootstrapServerIds.has(input.bootstrapServerId)
  ) {
    return 'bootstrapMissing';
  }
  return null;
}

export type DnsGroupFormError = 'name' | 'members';

export function validateDnsGroupForm(input: {
  name: string;
  members: readonly string[];
}): DnsGroupFormError | null {
  if (!input.name.trim()) return 'name';
  if (input.members.length === 0) return 'members';
  return null;
}

/* ─────────────────────────── 删除前的引用图 ─────────────────────────── */

export interface DnsResourceReference {
  scope: 'policy' | 'group' | 'server' | 'defaults';
  name: string;
}

function actionReferencesServer(action: DnsPolicyAction | undefined, serverId: string): boolean {
  if (!action) return false;
  if (action.type === 'server') return action.serverId === serverId;
  if (action.type !== 'hostsFirst') return false;
  if (action.hostsServerId === serverId) return true;
  return actionReferencesServer(action.fallback, serverId);
}

function actionReferencesGroup(action: DnsPolicyAction | undefined, groupId: string): boolean {
  if (!action) return false;
  if (action.type === 'group') return action.groupId === groupId;
  return action.type === 'hostsFirst' && actionReferencesGroup(action.fallback, groupId);
}

function uniqueReferences(references: DnsResourceReference[]): DnsResourceReference[] {
  const seen = new Set<string>();
  return references.filter((reference) => {
    const key = `${reference.scope}:${reference.name}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** 删除 DNS Server 前的完整引用图；停用规则同样计入。 */
export function dnsServerReferences(config: UserConfig, serverId: string): DnsResourceReference[] {
  const references: DnsResourceReference[] = [];
  for (const rule of config.dnsRules ?? []) {
    if (actionReferencesServer(rule.effects?.dns?.action, serverId)) {
      references.push({ scope: 'policy', name: rule.remarks?.trim() || rule.id });
    }
  }
  for (const group of config.dnsServerGroups ?? []) {
    if (group.members.includes(serverId) || group.fallbackServerId === serverId) {
      references.push({ scope: 'group', name: group.name.trim() || group.id });
    }
  }
  for (const server of config.dnsServers ?? []) {
    if (server.id !== serverId && server.bootstrapServerId === serverId) {
      references.push({ scope: 'server', name: server.name.trim() || server.id });
    }
  }
  const defaults = config.dnsDefaults;
  if (defaults?.directServerId === serverId) references.push({ scope: 'defaults', name: 'direct' });
  if (defaults?.proxyServerId === serverId) references.push({ scope: 'defaults', name: 'proxy' });
  if (actionReferencesServer(defaults?.unmatchedAction, serverId)) {
    references.push({ scope: 'defaults', name: 'unmatched' });
  }
  return uniqueReferences(references);
}

/** 删除 DNS Group 前的完整引用图。 */
export function dnsGroupReferences(config: UserConfig, groupId: string): DnsResourceReference[] {
  const references: DnsResourceReference[] = [];
  for (const rule of config.dnsRules ?? []) {
    if (actionReferencesGroup(rule.effects?.dns?.action, groupId)) {
      references.push({ scope: 'policy', name: rule.remarks?.trim() || rule.id });
    }
  }
  if (actionReferencesGroup(config.dnsDefaults?.unmatchedAction, groupId)) {
    references.push({ scope: 'defaults', name: 'unmatched' });
  }
  return uniqueReferences(references);
}

/**
 * 一条引用的用户可见文案。两端共用 —— 拒绝删除时要说清「被谁占着」，
 * 而那句话的四个分支（策略 / 分组 / 服务器 / 默认动作）在两端必须字字相同。
 */
export function dnsResourceReferenceText(
  reference: DnsResourceReference,
  t: (key: string, vars?: Record<string, unknown>) => string,
): string {
  if (reference.scope === 'policy') return t('settings.dns.resourceRefPolicy', { name: reference.name });
  if (reference.scope === 'group') return t('settings.dns.resourceRefGroup', { name: reference.name });
  if (reference.scope === 'server') return t('settings.dns.resourceRefServer', { name: reference.name });
  const name = reference.name === 'direct'
    ? t('settings.dns.defaultDirect')
    : reference.name === 'proxy'
      ? t('settings.dns.defaultProxy')
      : t('settings.dns.defaultUnmatched');
  return t('settings.dns.resourceRefDefault', { name });
}
