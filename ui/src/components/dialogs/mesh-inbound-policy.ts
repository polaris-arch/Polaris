/** Shared mesh inbound policy normalization and validation for desktop and mobile. */
import type { MeshInboundPolicy, ServerConfig } from '@/contracts/types';
import { isValidIpCidr, validateRuleValue } from '@/domain/rules';

const clean = (values: string[]): string[] => values.map((v) => v.trim()).filter(Boolean);
const validCidr = (cidr: string): boolean => cidr.includes('/') && isValidIpCidr(cidr);
const loopbackOnly = (cidr: string): boolean => {
  const [address, mask] = cidr.split('/');
  const prefix = Number(mask);
  if (address?.includes('.') && !address.includes(':')) {
    return (address.split('.')[0] === '127' && prefix >= 8) ||
      (address === '0.0.0.0' && prefix === 32);
  }
  if (!address?.includes(':')) return false;
  // URL 使用浏览器原生 IPv6 解析，把展开式与 dotted mapped 地址归到同一规范表示。
  let host: string;
  try { host = new URL(`http://[${address}]/`).hostname.slice(1, -1); }
  catch { return false; }
  if ((host === '::1' || host === '::') && prefix === 128) return true;
  const mapped = host.match(/^::ffff:([0-9a-f]+):([0-9a-f]+)$/);
  if (!mapped) return false;
  const first = Number.parseInt(mapped[1], 16) >> 8;
  return (first === 127 && prefix >= 104) ||
    (mapped[1] === '0' && mapped[2] === '0' && prefix === 128);
};

export function normalizeMeshInboundPolicy(policy: MeshInboundPolicy | undefined): MeshInboundPolicy | undefined {
  if (!policy || policy.mode === 'block') return policy;
  return {
    mode: 'allowlist',
    rules: policy.rules.map((rule) => ({
      ...rule,
      sourceCidrs: clean(rule.sourceCidrs),
      ports: clean(rule.ports),
      targetCidrs: rule.target === 'forward' ? clean(rule.targetCidrs ?? []) : undefined,
    })),
  };
}

/** 返回本地化错误键；后端仍是最终授权门。 */
export function meshInboundPolicyError(
  policy: MeshInboundPolicy | undefined,
  protocol: ServerConfig['protocol'],
  wgAddresses: string[] = [],
): string | null {
  if (!policy) return null;
  if (!['wireguard', 'tailscale', 'masque-client', 'openconnect', 'openvpn-client'].includes(protocol)) {
    return 'meshInbound.unsupported';
  }
  if (policy.mode === 'block') return null;
  if (protocol === 'wireguard' && (!wgAddresses.length || wgAddresses.some((cidr) =>
    !validCidr(cidr) || !cidr.endsWith(cidr.includes(':') ? '/128' : '/32')
  ))) return 'meshInbound.wgHostPrefix';
  for (const rule of policy.rules) {
    if (!rule.sourceCidrs.length || rule.sourceCidrs.some((cidr) => !validCidr(cidr))) {
      return 'meshInbound.sourceInvalid';
    }
    if (!rule.ports.length || rule.ports.some((port) => !validateRuleValue('port', port))) {
      return 'meshInbound.portInvalid';
    }
    if (rule.target === 'forward' && (!rule.targetCidrs?.length ||
      rule.targetCidrs.some((cidr) => !validCidr(cidr) || loopbackOnly(cidr)))) {
      return 'meshInbound.targetInvalid';
    }
    if (rule.network !== 'tcp' && rule.ports.some((port) => {
      const [start, end = start] = port.split('-').map(Number);
      return start <= 53 && end >= 53;
    })) return 'meshInbound.udp53';
  }
  return null;
}

export function applyMeshInboundPolicy<T extends ServerConfig>(server: T, policy: MeshInboundPolicy | undefined): T {
  const normalized = normalizeMeshInboundPolicy(policy);
  if (normalized) server.meshInboundPolicy = normalized;
  else delete server.meshInboundPolicy;
  return server;
}

