/**
 * 网络场景（`UserConfig.networkProfiles[]`）的渲染端纯逻辑：表单校验与清洗、引用计数、
 * 探测源显示态、规则徽标态。
 *
 * 设计真值：`~/docs/polaris/design/polaris-network-aware-rules-spec-2026-09-25.md` §3.3 / §4.4 / §6。
 *
 * 纪律：
 *  - **探测源不在这里算**。`auto` 解析为 system 还是 dhcp、本机可不可用，由后端用与生成器同一个函数
 *    算好经 IPC 返回（`ResolvedProbe`），本文件只把它翻成显示态 —— 渲染端重算一份必然漂移（§4.4 末段）。
 *  - 搜索域规范化与 Rust `normalize_search_domain` 同口径（去首尾空白与首尾点、小写）。
 *  - CIDR 校验复用 `domain/rules.ts` 的 `isValidIpCidr`（规则条件同一个判据）。
 */

import type { NetworkProfile, ResolvedProbe, Rule } from '@/contracts/types';
import { isValidIpCidr } from './rules';

/** DNS 规则动作的内置解析器「当前网络 DHCP 下发的 DNS」的保留 id（D5，Rust `BUILTIN_NETENV_DHCP_ID`）。 */
export const BUILTIN_NETENV_DHCP_ID = 'builtin-netenv-dhcp';

/** 与 Rust `normalize_search_domain` 同口径：去首尾空白与首尾点、小写；空 ⇒ null。 */
export function normalizeSearchDomain(raw: string): string | null {
  const v = raw.trim().replace(/^\.+|\.+$/g, '');
  return v ? v.toLowerCase() : null;
}

/** 搜索域标签：字母数字开头结尾、中间可含连字符与下划线，1–63 字符；不接受通配。 */
const SEARCH_DOMAIN_RE =
  /^([a-z0-9_](?:[a-z0-9_-]{0,61}[a-z0-9_])?\.)*[a-z0-9_](?:[a-z0-9_-]{0,61}[a-z0-9_])?$/;

export function isValidSearchDomain(raw: string): boolean {
  const v = normalizeSearchDomain(raw);
  return v !== null && v.length <= 253 && SEARCH_DOMAIN_RE.test(v);
}

export interface NetworkProfileDraft {
  name: string;
  cidrs: readonly string[];
  domains: readonly string[];
}

export type NetworkProfileFormError =
  | { kind: 'name' }
  | { kind: 'noCriteria' }
  | { kind: 'cidr'; value: string }
  | { kind: 'domain'; value: string };

const filled = (values: readonly string[]) => values.map((v) => v.trim()).filter(Boolean);

/**
 * 表单校验：名称必填；至少一项判据（两项都空 ⇒ 后端按引用失效不生成，§3.4-2，等于建了个永不命中的场景）；
 * 每个地址段 / 搜索域逐值校验，报出第一个坏值让用户照着改。空行忽略。
 */
export function validateNetworkProfileDraft(draft: NetworkProfileDraft): NetworkProfileFormError | null {
  if (!draft.name.trim()) return { kind: 'name' };
  const cidrs = filled(draft.cidrs);
  const domains = filled(draft.domains);
  if (cidrs.length === 0 && domains.length === 0) return { kind: 'noCriteria' };
  const badCidr = cidrs.find((v) => !isValidIpCidr(v));
  if (badCidr !== undefined) return { kind: 'cidr', value: badCidr };
  const badDomain = domains.find((v) => !isValidSearchDomain(v));
  if (badDomain !== undefined) return { kind: 'domain', value: badDomain };
  return null;
}

/** 按草稿组装落盘形态：去空行、搜索域规范化、去重；空数组不写键（与 Rust `skip_serializing_if` 同形）。 */
export function buildNetworkProfile(
  draft: NetworkProfileDraft & { enabled: boolean; probe: NetworkProfile['probe'] },
  id: string,
): NetworkProfile {
  const cidrs = [...new Set(filled(draft.cidrs))];
  const domains = [
    ...new Set(
      draft.domains.map(normalizeSearchDomain).filter((v): v is string => v !== null),
    ),
  ];
  return {
    id,
    name: draft.name.trim(),
    enabled: draft.enabled,
    match: {
      ...(cidrs.length ? { dnsServerCidrs: cidrs } : {}),
      ...(domains.length ? { searchDomains: domains } : {}),
    },
    probe: draft.probe,
  };
}

/** 引用计数：被多少条流量规则、多少条 DNS 规则引用（删除确认与列表都用它）。 */
export function profileRefCounts(
  profileId: string,
  trafficRules: readonly Rule[],
  dnsRules: readonly Rule[],
): { route: number; dns: number } {
  const n = (rules: readonly Rule[]) => rules.filter((r) => r.networkProfileId === profileId).length;
  return { route: n(trafficRules), dns: n(dnsRules) };
}

/**
 * 告警原因码：`available` 仍为 true、规则照常生成，但判据有一部分永远不会命中。
 * `dhcpIpv6Only`：探测源为 dhcp 而场景只写了 IPv6 地址段（DHCPv4 只下发 IPv4 DNS，N0 真机结论）。
 * **判据只在后端**（N2 与生成侧同一函数），渲染端不再按判据重算。
 */
export const PROBE_WARNING_KEYS: Readonly<Record<string, string>> = {
  dhcpIpv6Only: 'rules.networkProfile.ipv6DhcpWarn',
};

/** 后端给出的告警文案键；无告警 / 不可用（那走不可用原因）/ 拿不到结果 ⇒ null。 */
export function probeWarningKey(resolved: ResolvedProbe | undefined): string | null {
  if (!resolved?.available || !resolved.reason) return null;
  return PROBE_WARNING_KEYS[resolved.reason] ?? null;
}

/**
 * 不可用原因码 → i18n 键。原因码是后端 `ProbeReason` 的 camelCase 序列化（N2 最终四个不可用码；告警码见上）。
 * 表外的码（后端新加、前端还没跟上）落通用文案，**不把原始码画到屏幕上**。
 */
export const PROBE_REASON_KEYS: Readonly<Record<string, string>> = {
  profileInvalid: 'rules.networkProfile.reasonProfileInvalid',
  dhcpNeedsPrivilege: 'rules.networkProfile.reasonDhcpNeedsPrivilege',
  systemNoSearchDomain: 'rules.networkProfile.reasonSystemNoSearchDomain',
  dhcpMonitorMissing: 'rules.networkProfile.reasonDhcpMonitorMissing',
};
export const PROBE_REASON_FALLBACK_KEY = 'rules.networkProfile.reasonUnknown';

export function probeReasonKey(reason: string | null): string {
  return (reason && PROBE_REASON_KEYS[reason]) || PROBE_REASON_FALLBACK_KEY;
}

/**
 * 「本机将使用」一行的显示态：
 *  - `pending`：草稿的判据或探测方式与已保存的不同（后端算的是已保存那份），或场景还没保存 —— 说「保存后显示」，
 *    不拿旧结果冒充新结果；
 *  - `unknown`：拿不到后端结果（IPC 失败 / 后端还没这条命令）；
 *  - 其余按后端结果。
 */
export type ProbeDisplay =
  | { kind: 'pending' }
  | { kind: 'unknown' }
  | { kind: 'ok'; source: 'system' | 'dhcp' }
  | { kind: 'unavailable'; source: 'system' | 'dhcp'; reasonKey: string };

export function probeDisplay(
  profileId: string | undefined,
  resolved: readonly ResolvedProbe[] | null,
  draftDiffersFromSaved = false,
): ProbeDisplay {
  if (!profileId || draftDiffersFromSaved) return { kind: 'pending' };
  if (resolved === null) return { kind: 'unknown' };
  const hit = resolved.find((r) => r.profileId === profileId);
  if (!hit) return { kind: 'pending' };
  return hit.available
    ? { kind: 'ok', source: hit.probeSource }
    : { kind: 'unavailable', source: hit.probeSource, reasonKey: probeReasonKey(hit.reason) };
}

/**
 * 草稿里影响后端解析结果的部分（判据 + 探测方式 + 启停）是否与已保存的场景不同。
 * 启停也算：后端对停用场景报 `profileInvalid`。
 */
export function probeInputsDiffer(saved: NetworkProfile | undefined, next: NetworkProfile): boolean {
  if (!saved) return true;
  const key = (p: NetworkProfile) =>
    JSON.stringify([p.probe, p.enabled, p.match.dnsServerCidrs ?? [], p.match.searchDomains ?? []]);
  return key(saved) !== key(next);
}

/**
 * 规则行上的场景徽标态。不挂场景 ⇒ null（不渲染）。
 *  - `missing`：引用的场景已删除 ⇒ 规则不生效（fail-closed，§3.4-2）；
 *  - `disabled`：场景已停用 ⇒ 规则不生效；
 *  - `unavailable`：本机探测源不可用 ⇒ 规则不生效（§4.4）；
 *  - `warning`：规则照常生成，但后端告警部分判据永不命中（如 `dhcpIpv6Only`）；
 *  - `ok`：正常，`source` 为后端给的探测源；拿不到后端结果时 `source` 缺省。
 */
export type RuleProfileBadge =
  | { state: 'missing' }
  | { state: 'disabled'; name: string }
  | { state: 'unavailable'; name: string; reasonKey: string }
  | { state: 'warning'; name: string; warningKey: string }
  | { state: 'ok'; name: string; source?: 'system' | 'dhcp' };

export function ruleProfileBadge(
  rule: Rule,
  profiles: readonly NetworkProfile[],
  resolved: readonly ResolvedProbe[] | null,
): RuleProfileBadge | null {
  const id = rule.networkProfileId;
  if (!id) return null;
  const profile = profiles.find((p) => p.id === id);
  if (!profile) return { state: 'missing' };
  if (!profile.enabled) return { state: 'disabled', name: profile.name };
  const hit = resolved?.find((r) => r.profileId === id);
  if (hit && !hit.available) {
    return { state: 'unavailable', name: profile.name, reasonKey: probeReasonKey(hit.reason) };
  }
  const warningKey = probeWarningKey(hit);
  if (warningKey) return { state: 'warning', name: profile.name, warningKey };
  return { state: 'ok', name: profile.name, source: hit?.probeSource };
}

/** 判据摘要（列表行）：地址段与搜索域各几条，外加前两个值作样例。 */
export function criteriaSummary(profile: NetworkProfile): {
  cidrs: number;
  domains: number;
  sample: string;
} {
  const cidrs = profile.match.dnsServerCidrs ?? [];
  const domains = profile.match.searchDomains ?? [];
  return { cidrs: cidrs.length, domains: domains.length, sample: [...cidrs, ...domains].slice(0, 2).join(', ') };
}

/**
 * 新建带场景的规则默认插到用户块最前（spec §3.4-4 / §5.4）。
 *
 * 不改排序协议：沿用 `rules_reorder` / 暂存 `order:<orderKey>` 条目的「整序列」形态，这里只算出那条序列。
 * 现序口径与规则列表一致（`RulesScreen.visibleRules`）：先按 `persistedOrder` 里认得的 id，再补不在序里的
 * 规则（按集合顺序）；最后把新规则放到第一位，并从原位置剔除（防重复）。
 */
export function orderWithNewRuleFirst(
  ruleIds: readonly string[],
  persistedOrder: readonly string[],
  newId: string,
): string[] {
  const members = new Set(ruleIds);
  const ordered: string[] = [];
  for (const id of persistedOrder) {
    if (members.has(id) && !ordered.includes(id)) ordered.push(id);
  }
  for (const id of ruleIds) if (!ordered.includes(id)) ordered.push(id);
  return [newId, ...ordered.filter((id) => id !== newId)];
}

/**
 * 场景列表一行的状态区：停用的场景**只**显示「已停用」，不显示探测结果（后端对它恒报 `profileInvalid`，
 * 再画一行「不可用」只是把同一件事用更吓人的红字重复一遍）；启用的显示后端结果 + 告警。
 */
export type ProfileRowStatus =
  | { kind: 'disabled' }
  | { kind: 'probe'; display: ProbeDisplay; warningKey: string | null };

export function profileRowStatus(
  profile: NetworkProfile,
  resolved: readonly ResolvedProbe[] | null,
): ProfileRowStatus {
  if (!profile.enabled) return { kind: 'disabled' };
  return {
    kind: 'probe',
    display: probeDisplay(profile.id, resolved),
    warningKey: probeWarningKey(resolved?.find((r) => r.profileId === profile.id)),
  };
}
