import { describe, expect, it } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import type { ServerGroup } from '@/domain/server-grouping';
import { projectVisibleServers } from './nodes-list-projection';

const server = (id: string, patch: Partial<ServerConfig> = {}): ServerConfig =>
  ({ id, name: id, protocol: 'vmess', address: `${id}.example`, port: 443, ...patch }) as ServerConfig;

const group = (servers: ServerConfig[]): ServerGroup => ({
  id: 'manual',
  name: 'manual',
  isManual: true,
  servers,
});

describe('projectVisibleServers', () => {
  it('filters by search and protocol before sorting', () => {
    const nodes = [
      server('zeta', { protocol: 'vless' }),
      server('alpha', { protocol: 'vmess' }),
      server('beta', { protocol: 'vless' }),
    ];

    expect(projectVisibleServers(group(nodes), 'beta', 'vless', 'name', {})).toEqual([nodes[2]]);
  });

  it('keeps node-number families together across case and numeric suffixes', () => {
    const names = ['JP10', 'Jp01-MASQUE', 'JP02', 'JP01-H3', 'JP01'];
    const nodes = names.map(name => server(name));
    expect(projectVisibleServers(group(nodes), '', '', 'name', {}).map(node => node.name))
      .toEqual(['JP01', 'JP01-H3', 'Jp01-MASQUE', 'JP02', 'JP10']);
    // The explicit import-order option must still preserve the original persisted array.
    expect(projectVisibleServers(group(nodes), '', '', 'default', {})).toEqual(nodes);
    expect(nodes.map(node => node.name)).toEqual(names);
  });

  it('sorts latency with missing measurements delegated to the domain comparator', () => {
    const nodes = [server('slow'), server('missing'), server('fast')];

    expect(
      projectVisibleServers(group(nodes), '', '', 'lat', { slow: 200, fast: 20 }),
    ).toEqual([nodes[2], nodes[0], nodes[1]]);
  });
});
