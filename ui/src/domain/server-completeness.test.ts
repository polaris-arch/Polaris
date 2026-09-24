import { describe, expect, it } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import { ALL_PROTOCOLS, isServerComplete } from './server-completeness';

const base = (protocol: ServerConfig['protocol']): ServerConfig => ({
  id: protocol,
  name: protocol,
  protocol,
  address: 'vpn.example.com',
  port: 443,
});

describe('server completeness endpoint VPN coverage', () => {
  it('runtime registry covers every Protocol member added for endpoint VPNs', () => {
    for (const protocol of ['hysteria', 'tor', 'openconnect', 'openvpn-client', 'masque-client'] as const) {
      expect(ALL_PROTOCOLS).toContain(protocol);
    }
  });

  it('Tor is addressless, while endpoint VPNs require their nested settings', () => {
    expect(isServerComplete({ ...base('tor'), address: '', port: 0 })).toBe(true);
    expect(isServerComplete({
      ...base('openconnect'),
      openconnectSettings: { server: 'vpn.example.com:443', username: 'u', password: 'p', flavor: 'anyconnect' },
    })).toBe(true);
    expect(isServerComplete({
      ...base('openvpn-client'),
      openvpnClientSettings: { server: 'vpn.example.com', server_port: 1194, username: 'u', password: 'p', tls: {} },
    })).toBe(true);
    expect(isServerComplete(base('openvpn-client'))).toBe(false);
  });

  it('MASQUE 只要地址/端口（凭据可选、设置块可缺）；缺地址不完整', () => {
    expect(isServerComplete(base('masque-client'))).toBe(true);
    expect(isServerComplete({ ...base('masque-client'), address: '' })).toBe(false);
  });
});
