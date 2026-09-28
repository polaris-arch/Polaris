import { describe, expect, it } from 'vitest';
import type { MeshInboundPolicy } from '@/contracts/types';
import {
  applyMeshInboundPolicy,
  meshInboundPolicyError,
  normalizeMeshInboundPolicy,
} from './MeshInboundPolicyEditor';

const grant = (overrides: Record<string, unknown> = {}): MeshInboundPolicy => ({
  mode: 'allowlist',
  rules: [{
    sourceCidrs: ['10.8.0.2/32'],
    network: 'tcp',
    ports: ['8080'],
    target: 'local',
    ...overrides,
  }],
});

describe('mesh inbound policy editor boundary', () => {
  it('keeps legacy absent and round trips explicit modes', () => {
    const server = { id: 'ts', name: 'TS', protocol: 'tailscale' as const, address: '', port: 0 };
    expect(normalizeMeshInboundPolicy(undefined)).toBeUndefined();
    applyMeshInboundPolicy(server, { mode: 'block' });
    expect(server).toHaveProperty('meshInboundPolicy', { mode: 'block' });
    applyMeshInboundPolicy(server, undefined);
    expect(server).not.toHaveProperty('meshInboundPolicy');
  });

  it('requires source, port, and WG host prefixes for an allowlist', () => {
    expect(meshInboundPolicyError(grant({ sourceCidrs: [] }), 'tailscale')).toBe('meshInbound.sourceInvalid');
    expect(meshInboundPolicyError(grant({ ports: [] }), 'tailscale')).toBe('meshInbound.portInvalid');
    expect(meshInboundPolicyError(grant(), 'wireguard', ['10.8.0.1/24'])).toBe('meshInbound.wgHostPrefix');
    expect(meshInboundPolicyError(grant(), 'wireguard', ['10.8.0.1/32'])).toBeNull();
    expect(meshInboundPolicyError({ mode: 'block' }, 'wireguard', [])).toBeNull();
  });

  it('cannot grant DNS UDP53 or pure loopback and unspecified target aliases', () => {
    for (const network of ['udp', 'both'] as const) {
      expect(meshInboundPolicyError(grant({ network, ports: ['50-60'] }), 'tailscale'))
        .toBe('meshInbound.udp53');
    }
    for (const cidr of ['127.0.0.1/32', '0.0.0.0/32', '0:0:0:0:0:0:0:1/128',
      '::ffff:127.0.0.1/128', '::ffff:0.0.0.0/128']) {
      expect(meshInboundPolicyError(grant({ target: 'forward', targetCidrs: [cidr] }), 'tailscale'))
        .toBe('meshInbound.targetInvalid');
    }
    expect(meshInboundPolicyError(grant({ target: 'forward', targetCidrs: ['0.0.0.0/0'] }), 'tailscale'))
      .toBeNull();
  });
});
