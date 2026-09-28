/**
 * `SettingsDns` 的纯 DNS 输入与配置逻辑。
 *
 * 此处不持有 React state、不发 IPC、也不写配置；页面组件独占草稿、确认和 update 事务。
 * 保留为独立 owner，避免输入校验/竞速池身份规则与呈现层耦合。
 */
import type { CustomDnsUpstream, DnsConfig } from '@/contracts/types';

export const MAX_DOH_RACE_UPSTREAMS = 3;

/** 启用池变更：system 属兜底层不计额度；Tier1 达上限时拒绝第 4 个。 */
export function nextRacePool(pool: readonly string[], id: string, on: boolean): string[] {
  const set = new Set(pool);
  if (!on) {
    set.delete(id);
    return [...set];
  }
  if (id !== 'system' && !set.has(id)) {
    const tier1Count = [...set].filter((value) => value !== 'system').length;
    if (tier1Count >= MAX_DOH_RACE_UPSTREAMS) return [...set];
  }
  set.add(id);
  return [...set];
}

/**
 * 字符串编辑器没有实体 id，故在提交时做稳定对账：同值优先保 id，原位编辑其次保 id，新行才铸 id。
 * 新增项只进入配置库存，不自动进入启用池。
 */
export function reconcileCustomUpstreams(
  previous: readonly CustomDnsUpstream[],
  specs: readonly string[],
  createId: () => string
): CustomDnsUpstream[] {
  const used = new Set<string>();
  const seenSpecs = new Set<string>();
  // 后续仍以原值出现的条目先保留其 id；否则在列表头插入新项会“偷走”下一行的启用身份。
  const incomingSpecs = new Set(specs.map((spec) => spec.trim()).filter(Boolean));
  const reserved = new Set(
    previous.filter((item) => incomingSpecs.has(item.spec.trim())).map((item) => item.id)
  );
  const next: CustomDnsUpstream[] = [];
  for (let index = 0; index < specs.length; index += 1) {
    const spec = specs[index].trim();
    const specKey = spec.toLowerCase();
    if (!spec || seenSpecs.has(specKey)) continue;
    seenSpecs.add(specKey);
    const exact = previous.find((item) => !used.has(item.id) && item.spec.trim() === spec);
    const samePosition = previous[index];
    const samePositionAvailable = samePosition && !used.has(samePosition.id) && !reserved.has(samePosition.id);
    const remaining = previous.find((item) => !used.has(item.id) && !reserved.has(item.id));
    const id = exact?.id ??
      (samePositionAvailable ? samePosition.id : remaining?.id ?? createId());
    used.add(id);
    next.push({ id, spec });
  }
  return next;
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
  return parsed ? !parsed.isDomain : false;
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
  const s = spec.trim();
  if (!s) return true;
  const parsed = parseDnsServerSpec(s);
  return parsed ? !parsed.isDomain : false;
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
