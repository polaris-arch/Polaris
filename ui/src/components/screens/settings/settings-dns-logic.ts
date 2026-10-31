/**
 * `SettingsDns` 的纯 DNS 输入与配置逻辑。
 *
 * 此处不持有 React state、不发 IPC、也不写配置；页面组件独占草稿、确认和 update 事务。
 * 保留为独立 owner，避免输入校验/竞速池身份规则与呈现层耦合。
 */
import { isIpLiteral as isStrictIpLiteral } from '@/domain/ip-cidr';
import type { CustomDnsUpstream, DnsConfig } from '@/contracts/types';
import type { ListEntryBinding, ListEntryCommitResult, ListEntrySource } from '@/domain/list-entries';

export const MAX_DOH_RACE_UPSTREAMS = 3;

/** Same explicit edge set as the native node parser: Unicode White_Space + BOM. */
export function trimNodeDnsSpec(spec: string): string {
  return spec.replace(/^[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]+|[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]+$/gu, '');
}

/** Node upstream identity follows the native planner, without URL path normalization. */
function nodeUpstream(spec: string): { key: string; tier: 1 | 2 } | null {
  const value = trimNodeDnsSpec(spec);
  const authority = value.includes('://') ? value.slice(value.indexOf('://') + 3).split('/')[0] : value;
  // Validate raw authority before IP helpers trim it or WHATWG URL cleans it.
  if (/[\s\x00-\x1f\x7f-\x9f]/u.test(authority)) return null;
  const url = /^(https|tls|udp):\/\/(\[[^\]]+\]|[^:/?#]+)(?::([0-9]+))?(\/[^]*)?$/.exec(value);
  const protocol = url?.[1] ?? 'udp';
  if (url?.[2].startsWith('[') && !url[2].includes(':')) return null;
  const host = stripBrackets(url?.[2] ?? value);
  if (!isStrictIpLiteral(host) || (url && protocol === 'tls' && url[4] !== undefined)) return null;
  if (!url && value.includes('://')) return null;
  const port = url?.[3] ?? (protocol === 'https' ? '443' : protocol === 'tls' ? '853' : '53');
  if (!isValidPort(port)) return null;
  try {
    const ip = new URL(`http://${host.includes(':') ? `[${host}]` : host}/`).hostname;
    const path = protocol === 'https' ? (!url?.[4] || url[4] === '/' ? '/dns-query' : url[4]) : '';
    return { key: `${protocol}:${ip}:${Number(port)}:${path}`, tier: protocol === 'udp' ? 2 : 1 };
  } catch { return null; }
}

/** URL authority is case insensitive; path/query remain exact protocol data. */
export function dnsUpstreamIdentity(spec: string): string {
  const node = nodeUpstream(spec);
  if (node) return node.key;
  const value = trimNodeDnsSpec(spec);
  try {
    const url = new URL(value);
    url.hostname = url.hostname.toLowerCase();
    return url.href;
  } catch { return value.toLowerCase(); }
}

export function isBuiltinNodeResolverId(id: string): boolean {
  return id === 'ali' || id === 'dnspod' || id === 'system';
}

function selectedNodeUpstream(id: string, custom: readonly CustomDnsUpstream[]) {
  if (!id) return null;
  if (id === 'system') return { key: 'system', tier: 2 as const };
  if (id === 'ali') return nodeUpstream('https://223.5.5.5/dns-query');
  if (id === 'dnspod') return nodeUpstream('https://1.12.12.12/dns-query');
  const item = custom.find((entry) => entry.id === id);
  return item ? nodeUpstream(item.spec) : null;
}

export function nodeResolverTier1Count(pool: readonly string[], custom: readonly CustomDnsUpstream[]): number {
  return new Set(pool.flatMap((id) => {
    const upstream = selectedNodeUpstream(id, custom);
    return upstream?.tier === 1 ? [upstream.key] : [];
  })).size;
}

/** Equivalent encrypted entries and Tier2 consume no additional encrypted quota. */
export function canEnableRaceUpstream(pool: readonly string[], id: string, custom: readonly CustomDnsUpstream[]): boolean {
  if (!selectedNodeUpstream(id, custom)) return false;
  const before = nodeResolverTier1Count(pool, custom);
  const after = nodeResolverTier1Count([...pool, id], custom);
  return after <= MAX_DOH_RACE_UPSTREAMS || after <= before;
}

/** Inventory is separate from enabled IDs; disabling also works for legacy invalid entries. */
export function nextRacePool(pool: readonly string[], id: string, on: boolean, custom: readonly CustomDnsUpstream[] = []): string[] {
  const set = new Set(pool);
  if (!on) set.delete(id);
  else if (canEnableRaceUpstream(pool, id, custom)) set.add(id);
  return [...set];
}

/** Existing IDs come directly from editor rows; undefined marks an added row. */
export interface NodeResolverInventoryRow { id?: string; spec: string }

export type NodeResolverInventoryEdit =
  | { ok: true; custom: CustomDnsUpstream[]; pool: string[]; editor?: ListEntryCommitResult }
  | { ok: false; error: 'invalid' | 'quota' };

interface InventoryContinuation {
  custom: readonly CustomDnsUpstream[];
  pool: readonly string[];
  selection: readonly ({ source: string } | { literal: string })[];
}
function inventoryContinuation(value: unknown, previous: readonly CustomDnsUpstream[], pool: readonly string[]): InventoryContinuation | undefined {
  if (!value || typeof value !== 'object' || !('custom' in value) || !Array.isArray(value.custom)
    || !('pool' in value) || !Array.isArray(value.pool) || !('selection' in value) || !Array.isArray(value.selection)) return undefined;
  if (value.custom.length !== previous.length || !value.custom.every((row, index) => row?.id === previous[index].id && row?.spec === previous[index].spec)
    || value.pool.length !== pool.length || !value.pool.every((id, index) => id === pool[index])
    || !value.selection.every((token) => token && (typeof token.source === 'string' || typeof token.literal === 'string'))) return undefined;
  return value as InventoryContinuation;
}

/** Pure inventory/selection transaction. Row provenance survives until after
 * canonical merging, so an explicit deletion can never be mistaken for editing. */
export function editNodeResolverInventory(
  previous: readonly CustomDnsUpstream[],
  pool: readonly string[],
  rows: readonly NodeResolverInventoryRow[],
  createId: () => string,
  editorInput?: { sources: readonly ListEntrySource[]; context?: unknown },
): NodeResolverInventoryEdit {
  if (editorInput && (editorInput.sources.length !== rows.length || new Set(editorInput.sources.map((source) => source.key)).size !== rows.length)) return { ok: false, error: 'invalid' };
  const bindings: ListEntryBinding[] = [];
  const custom: CustomDnsUpstream[] = [];
  const representatives = new Map<string, string>();
  const sourceIds = new Set<string>();
  const selectedIds = new Map<string, string>();
  const previousIds = new Set(previous.map((item) => item.id));
  for (const [index, row] of rows.entries()) {
    const binding: ListEntryBinding = { key: editorInput?.sources[index].key ?? '', primary: false };
    bindings.push(binding);
    if (row.id !== undefined) {
      if (!previousIds.has(row.id) || sourceIds.has(row.id)) return { ok: false, error: 'invalid' };
      sourceIds.add(row.id);
    }
    const spec = trimNodeDnsSpec(row.spec);
    if (!spec) continue;
    // A custom row with a builtin ID is stored but never used as an endpoint;
    // do not merge an effective custom endpoint into this shadowed ID.
    if (row.id !== undefined && isBuiltinNodeResolverId(row.id)) {
      custom.push({ id: row.id, spec });
      binding.id = row.id; binding.primary = true;
      continue;
    }
    const key = dnsUpstreamIdentity(spec);
    const representative = representatives.get(key);
    if (representative !== undefined) {
      if (row.id !== undefined) selectedIds.set(row.id, representative);
      binding.id = representative;
      continue;
    }
    const id = row.id ?? createId();
    if (row.id === undefined && (previousIds.has(id) || isBuiltinNodeResolverId(id) || custom.some((item) => item.id === id))) return { ok: false, error: 'invalid' };
    representatives.set(key, id);
    if (row.id !== undefined) selectedIds.set(row.id, id);
    custom.push({ id, spec });
    binding.id = id; binding.primary = true;
  }
  let selection: InventoryContinuation['selection'] = [];
  let nextPool = [...new Set(pool.flatMap((id) => {
    if (isBuiltinNodeResolverId(id) || !previousIds.has(id)) return [id];
    const representative = selectedIds.get(id);
    return representative === undefined ? [] : [representative];
  }))];
  if (editorInput) {
    const continuation = inventoryContinuation(editorInput.context, previous, pool);
    if (continuation) {
      const originToSource = new Map<string, string>();
      for (const source of editorInput.sources) {
        for (const origin of source.origins) {
          if (originToSource.has(origin)) return { ok: false, error: 'invalid' };
          originToSource.set(origin, source.key);
        }
      }
      selection = continuation.selection.flatMap<InventoryContinuation['selection'][number]>((token) => {
        if ('literal' in token) return [token];
        const source = originToSource.get(token.source);
        return source === undefined ? [] : [{ source }];
      });
    } else {
      selection = pool.flatMap<InventoryContinuation['selection'][number]>((id) => {
        if (isBuiltinNodeResolverId(id) || !previousIds.has(id)) return [{ literal: id }];
        const index = rows.findIndex((row) => row.id === id);
        return index < 0 ? [] : [{ source: editorInput.sources[index].key }];
      });
    }
    const bySource = new Map(bindings.map((binding) => [binding.key, binding.id]));
    selection = selection.filter((token) => 'literal' in token || bySource.get(token.source) !== undefined);
    nextPool = [...new Set(selection.map((token) => 'literal' in token ? token.literal : bySource.get(token.source)!))];
  }
  // Builtin IDs shadow custom inventory in the native lookup as well.
  const effectiveCustomSelected = (id: string, ids: readonly string[]) => !isBuiltinNodeResolverId(id) && ids.includes(id);
  if (custom.some((item) => !nodeUpstream(item.spec) && (
    effectiveCustomSelected(item.id, nextPool) ||
    !previous.some((old) => old.id === item.id && trimNodeDnsSpec(old.spec) === item.spec && !effectiveCustomSelected(old.id, pool))
  ))) return { ok: false, error: 'invalid' };
  if (nodeResolverTier1Count(nextPool, custom) > MAX_DOH_RACE_UPSTREAMS) return { ok: false, error: 'quota' };
  if (!editorInput) return { ok: true, custom, pool: nextPool };
  return { ok: true, custom, pool: nextPool, editor: {
    acceptedRows: custom.map((item) => ({ value: item.spec, id: item.id })), acceptedSources: bindings,
    context: { custom: custom.map((item) => ({ ...item })), pool: [...nextPool], selection } satisfies InventoryContinuation,
    contextKey: JSON.stringify(nextPool),
  } };
}

export interface ParsedDnsServer {
  /** 主机名或 IP（IPv6 已去方括号）。 */
  server: string;
  /** host 非 IP 字面量：域名形式 DoH 需 bootstrap 引导层。 */
  isDomain: boolean;
}

/** `[::1]` → `::1`；单边畸形原样返回（下游据实拒绝）。 */
function stripBrackets(host: string): string {
  return host.length >= 2 && host.startsWith('[') && host.endsWith(']') ? host.slice(1, -1) : host;
}

function isIpv4Segment(seg: string): boolean {
  return seg.length > 0 && seg.length <= 3 && /^\d+$/.test(seg) && Number(seg) <= 255;
}

function isIpv4(host: string): boolean {
  const parts = host.split('.');
  return parts.length === 4 && parts.every(isIpv4Segment);
}

/** IPv6 字面量（含 IPv4-mapped）；口径与 Rust `is_ipv6_literal` 对齐。 */
function isIpv6Literal(host: string): boolean {
  const h = stripBrackets(host);
  if ((h.match(/:/g)?.length ?? 0) < 2) return false;
  if (/^[0-9a-fA-F:]+$/.test(h)) return true;
  const last = h.lastIndexOf(':');
  return /^[0-9a-fA-F:]+$/.test(h.slice(0, last + 1)) && isIpv4(h.slice(last + 1));
}

function isIpLiteral(host: string): boolean {
  return isIpv4(host) || isIpv6Literal(host);
}

function isValidPort(s: string): boolean {
  if (!/^\d+$/.test(s)) return false;
  const n = Number(s);
  return n >= 1 && n <= 65535;
}

function parseSpecUrl(s: string, scheme: string): ParsedDnsServer | null {
  if (!s.startsWith(scheme)) return null;
  const afterScheme = s.slice(scheme.length);
  if (!afterScheme.startsWith('//')) return null;
  const rest = afterScheme.slice(2);
  const slash = rest.indexOf('/');
  const authority = slash >= 0 ? rest.slice(0, slash) : rest;

  let hostRaw: string;
  const bracketEnd = authority.indexOf(']');
  if (bracketEnd >= 0) {
    hostRaw = authority.slice(0, bracketEnd + 1);
    const after = authority.slice(bracketEnd + 1);
    if (after.startsWith(':') && !isValidPort(after.slice(1))) return null;
  } else {
    const colon = authority.lastIndexOf(':');
    if (colon >= 0) {
      if (!isValidPort(authority.slice(colon + 1))) return null;
      hostRaw = authority.slice(0, colon);
    } else {
      hostRaw = authority;
    }
  }

  const host = stripBrackets(hostRaw);
  if (!host) return null;
  return { server: host, isDomain: !isIpLiteral(host) };
}

/**
 * 与后端 `parse_dns_server_spec` 同口径：接受 https/tls/udp 与裸 IP，拒绝裸域名和非法端口。
 */
export function parseDnsServerSpec(spec: string | undefined | null): ParsedDnsServer | null {
  const s = (spec ?? '').trim();
  if (!s) return null;
  const url =
    parseSpecUrl(s, 'https:') ?? parseSpecUrl(s, 'tls:') ?? parseSpecUrl(s, 'udp:');
  if (url) return url;
  const bare = stripBrackets(s);
  return isIpLiteral(bare) ? { server: bare, isDomain: false } : null;
}

/** DoH host 是字面 IP 时无需 bootstrap。 */
export function isIpDoh(url: string): boolean {
  const parsed = parseDnsServerSpec(url);
  return parsed ? isIpLiteral(parsed.server) : false;
}

/** 只有 TUN 下关闭 FakeIP 需要风险确认。 */
export function needsFakeIpOffConfirm(next: boolean, proxyModeType: string | undefined): boolean {
  return !next && proxyModeType === 'tun';
}

/** 空值回退内核默认；非空值须为 1..60000，非整数按后端 sanitize 一样四舍五入。 */
export function normalizeDnsTimeoutInput(raw: string): { value: number | undefined } | null {
  const v = raw.trim();
  if (!v) return { value: undefined };
  const n = Number(v);
  if (!Number.isFinite(n) || n < 1 || n > 60000) return null;
  return { value: Math.round(n) };
}

/** 用户手改 FakeIP 时消费一次性 TUN 自动修正资格。 */
export function fakeIpTogglePatch(next: boolean): Partial<DnsConfig> {
  return { enableFakeIp: next, fakeIpTunAutoEnable: false };
}

/** 自定义节点解析上游只能是 IP 形态；空行代表尚未输入。 */
export function isPureIpDnsSpec(spec: string): boolean {
  const s = trimNodeDnsSpec(spec);
  if (!s) return true;
  return nodeUpstream(s) !== null;
}

export const DNS_FALLBACK = {
  domesticDns: 'https://223.5.5.5/dns-query',
  foreignDns: 'https://1.1.1.1/dns-query',
} as const;

/* ─────────────────── 国内 / 国外 DNS 的上游预设 ─────────────────── */

/**
 * 一条上游预设：`value` 是要落进 `dnsConfig.foreignDns` / `.domesticDns` 的那串字面量，
 * `label` 是拼装好的可读名。
 */
export interface DnsUpstreamPreset {
  value: string;
  label: string;
}

/**
 * 「自定义」那一档的哨兵值。**只存在于控件里**（`<select>` 的 value 表达不了「不在表内」），
 * 落库时永远不写它 —— 两个消费点都在 `onChange` 里显式挡掉。
 */
export const DNS_PRESET_CUSTOM = '__custom__';

/**
 * 上游预设。标签只有「按 IP / 按域名的 DoH」与「阿里 / 腾讯」两类词需要翻译，服务商域名与 IP
 * 跨语种同形（Cloudflare / Google / DNSPod 亦然），故按 `<类型> · <厂商> <地址>` 拼装而非整句入库。
 *
 * 走函数而非模块级常量：常量在 import 期求值，那时 i18n 语言尚未被 `syncLanguageChoice` 校正，
 * 切语言也不会重算（同 SettingsSidebar 分组表的理由）。
 *
 * 🔴 **2026-09-06 从 `SettingsDns.tsx` 搬到这里**，因为移动端 DNS 页要用同一张表。
 * 上一版这两个函数是那个**组件文件里的私有函数**，于是移动端只有两条路：抄一份（第二份真值源，
 * 且没有任何门守着它们相等），或者 import 一个桌面屏组件（把整条桌面层叠链拖进移动入口，
 * 契约 A1 违约）。搬到这个**已经被两端共用**的纯逻辑模块里，两条路都不用走。
 * ⚠️ 它现在是**共享面**：改这两张表要同时跑桌面与移动端两侧的门。
 */
export function remotePresets(t: (key: string) => string): DnsUpstreamPreset[] {
  const ip = t('settings.dns.dohByIp');
  const dom = t('settings.dns.dohByDomain');
  return [
    { value: 'https://1.1.1.1/dns-query', label: `${ip} · Cloudflare 1.1.1.1` },
    { value: 'https://8.8.8.8/dns-query', label: `${ip} · Google 8.8.8.8` },
    { value: 'https://cloudflare-dns.com/dns-query', label: `${dom} · cloudflare-dns.com` },
    { value: 'https://dns.google/dns-query', label: `${dom} · dns.google` },
  ];
}

export function domesticPresets(t: (key: string) => string): DnsUpstreamPreset[] {
  const ip = t('settings.dns.dohByIp');
  const dom = t('settings.dns.dohByDomain');
  const ali = t('settings.dns.brandAli');
  return [
    { value: 'https://223.5.5.5/dns-query', label: `${ip} · ${ali} 223.5.5.5` },
    { value: 'https://1.12.12.12/dns-query', label: `${ip} · ${t('settings.dns.brandTencent')} 1.12.12.12` },
    { value: 'https://doh.pub/dns-query', label: `${dom} · DNSPod doh.pub` },
    { value: 'https://dns.alidns.com/dns-query', label: `${dom} · ${ali} dns.alidns.com` },
  ];
}
