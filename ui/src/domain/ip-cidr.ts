/** IPv4 字面量（严格：每段 0-255，不含前导零之外的宽松形态）。 */
const IPV4_RE = /^(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)$/;

/** IPv4 CIDR `a.b.c.d[/n]` → [网络地址(uint32), 前缀]；非法/IPv6 → null。无 `/n` 视为 `/32`。 */
function parseIpv4Cidr(cidr: string): [number, number] | null {
  const parts = cidr.trim().split('/');
  if (parts.length > 2 || (parts.length === 2 && !/^\d+$/.test(parts[1]))) return null;
  const [ipPart, prefixPart] = parts;
  const prefix = prefixPart === undefined ? 32 : Number(prefixPart);
  if (!IPV4_RE.test(ipPart) || !Number.isInteger(prefix) || prefix < 0 || prefix > 32) return null;
  if (ipPart.split('.').some(o => o.length > 1 && o.startsWith('0'))) return null;
  const o = ipPart.split('.').map(Number);
  const ipInt = ((o[0] << 24) | (o[1] << 16) | (o[2] << 8) | o[3]) >>> 0;
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  return [(ipInt & mask) >>> 0, prefix];
}

/** 两个 IPv4 CIDR 是否相交（按**较短前缀**比对网络地址）。任一非 IPv4 CIDR → false。 */
export function ipv4CidrsOverlap(a: string, b: string): boolean {
  const pa = parseIpv4Cidr(a);
  const pb = parseIpv4Cidr(b);
  if (!pa || !pb) return false;
  const minPrefix = Math.min(pa[1], pb[1]);
  if (minPrefix === 0) return true; // 0.0.0.0/0 覆盖一切
  const mask = (0xffffffff << (32 - minPrefix)) >>> 0;
  return ((pa[0] & mask) >>> 0) === ((pb[0] & mask) >>> 0);
}

/**
 * IPv6 字面量（含 `::` 压缩）→ 8 组 16-bit 无符号整数；非法 → null。
 *
 * **刻意不用 BigInt**：`vite.config.ts` 的 `build.target` 含 `safari13`，而 BigInt 字面量要 Safari 14 —
 * esbuild 无法降级 `0n`，会直接 `Big integer literals are not available` 硬失败（2026-07-28 踩到，
 * 单测在 node 下照绿、只有 `vite build` 会红）。8×16-bit 分组比对与 128-bit 整数按前缀比对等价，
 * 且与上面 v4 腿的 uint32 口径同构。
 */
function ipv6Groups(addr: string): number[] | null {
  let a = addr.trim();
  if (a.includes('.')) {
    const colon = a.lastIndexOf(':');
    const tail = a.slice(colon + 1);
    if (colon < 0 || !IPV4_RE.test(tail) || tail.split('.').some(o => o.length > 1 && o.startsWith('0'))) return null;
    const octets = tail.split('.').map(Number);
    a = a.slice(0, colon + 1) + ((octets[0] << 8) | octets[1]).toString(16)
      + ':' + ((octets[2] << 8) | octets[3]).toString(16);
  }
  if (!a.includes(':') || !/^[0-9a-fA-F:]+$/.test(a)) return null;
  const halves = a.split('::');
  if (halves.length > 2) return null;
  const head = halves[0] ? halves[0].split(':') : [];
  const tail = halves.length === 2 && halves[1] ? halves[1].split(':') : [];
  if (halves.length === 1 && head.length !== 8) return null; // 无 :: 必须满 8 组
  const fill = 8 - head.length - tail.length;
  if (fill < (halves.length === 2 ? 1 : 0)) return null; // :: 至少省 1 组
  const groups = [...head, ...new Array(halves.length === 2 ? fill : 0).fill('0'), ...tail];
  if (groups.length !== 8) return null;
  const out: number[] = [];
  for (const g of groups) {
    if (g.length === 0 || g.length > 4) return null;
    const n = parseInt(g, 16);
    if (Number.isNaN(n)) return null;
    out.push(n);
  }
  return out;
}

/** 第 `i` 组（16 bit）在前缀 `prefix` 下的掩码：整组落在前缀内 → 0xffff，整组落在前缀外 → 0。 */
function groupMask(prefix: number, i: number): number {
  const bits = Math.min(16, Math.max(0, prefix - 16 * i));
  return bits === 0 ? 0 : (0xffff << (16 - bits)) & 0xffff;
}

/** IPv6 CIDR `addr/n` → [网络地址(8×16bit), 前缀]；非法/IPv4 → null。无 `/n` 视为 `/128`。 */
function parseIpv6Cidr(cidr: string): [number[], number] | null {
  const parts = cidr.trim().split('/');
  if (parts.length > 2 || (parts.length === 2 && !/^\d+$/.test(parts[1]))) return null;
  const [ipPart, prefixPart] = parts;
  const prefix = prefixPart === undefined ? 128 : Number(prefixPart);
  if (!Number.isInteger(prefix) || prefix < 0 || prefix > 128) return null;
  const v = ipv6Groups(ipPart);
  if (v === null) return null;
  return [v.map((g, i) => g & groupMask(prefix, i)), prefix];
}

/** 两个 IPv6 CIDR 是否相交（按较短前缀比对网络地址）。任一非 IPv6 CIDR → false。 */
export function ipv6CidrsOverlap(a: string, b: string): boolean {
  const pa = parseIpv6Cidr(a);
  const pb = parseIpv6Cidr(b);
  if (!pa || !pb) return false;
  const minPrefix = Math.min(pa[1], pb[1]);
  if (minPrefix === 0) return true;
  for (let i = 0; i < 8; i += 1) {
    const m = groupMask(minPrefix, i);
    if (m === 0) break; // 后续组全在前缀外，无需比对
    if ((pa[0][i] & m) !== (pb[0][i] & m)) return false;
  }
  return true;
}

/** 两个 CIDR 是否相交（按地址族自动分派 v4/v6；**跨族恒不相交**）。 */
export function cidrsOverlap(a: string, b: string): boolean {
  return ipv4CidrsOverlap(a, b) || ipv6CidrsOverlap(a, b);
}


/** A probe is a single IP, never a network range. */
export function isIpLiteral(input: string): boolean {
  return !input.includes('/') && (parseIpv4Cidr(input) !== null || parseIpv6Cidr(input) !== null);
}
export function cidrContainsIp(cidr: string, ip: string): boolean {
  return isIpLiteral(ip) && cidrsOverlap(cidr, ip);
}
