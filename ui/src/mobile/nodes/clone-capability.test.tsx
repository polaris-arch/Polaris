import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ServerConfig } from '@/contracts/types';
import { buildRowItems, type NodeRowVM } from './view-model';

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: 'zh-CN' } }),
}));

(globalThis as unknown as { document: unknown }).document = {
  documentElement: { dir: '', lang: '', getAttribute: () => null, setAttribute: () => {} },
  body: { nodeType: 1 },
};

const base = (id: string): ServerConfig => ({
  id,
  name: id,
  protocol: 'vless',
  address: 'example.invalid',
  port: 443,
}) as ServerConfig;
const warp = (id: string): ServerConfig => ({
  ...base(id),
  protocol: 'wireguard',
  address: '162.159.192.1',
  wireguardSettings: {
    privateKey: 'synthetic',
    localAddress: ['10.0.0.2/32'],
    peerPublicKey: 'synthetic',
    warpDevice: { deviceId: 'synthetic', token: 'synthetic' },
  },
});
const legacyWarp = (id: string): ServerConfig => ({
  ...warp(id),
  address: 'engage.cloudflareclient.com',
  wireguardSettings: {
    privateKey: 'synthetic',
    localAddress: ['10.0.0.2/32'],
    peerPublicKey: 'synthetic',
  },
});
const plainWg = (id: string): ServerConfig => ({
  ...legacyWarp(id),
  address: 'wg.example.invalid',
});
const ts = (id: string): ServerConfig => ({ ...base(id), protocol: 'tailscale' });

function row(server: ServerConfig): NodeRowVM {
  return {
    server,
    isCurrent: false,
    isExit: false,
    lanOnly: false,
    latencyMs: undefined,
    latencyStale: false,
    speedTestable: true,
    stagedOnly: false,
    deletable: true,
    transport: '',
    protocolLabel: server.protocol,
  };
}

function mobileHasClone(server: ServerConfig): boolean {
  const items = buildRowItems({
    t: (key) => key,
    row: row(server),
    coreRunning: false,
    tsLoginState: null,
    disposition: () => ({ kind: 'ported' }),
    handlers: {
      onSpeedTest: () => {},
      onConnect: () => {},
      onCopyLink: () => {},
      onTsLogin: () => {},
      onClone: () => {},
      onEdit: () => {},
      onDelete: () => {},
    },
  });
  return items.some((item) => item.id === 'clone');
}

async function desktopHasClone(server: ServerConfig): Promise<boolean> {
  const { NodeCard } = await import('@/components/screens/nodes/NodeCard');
  return renderToStaticMarkup(<NodeCard server={server} />).includes('aria-label="nodes.clone"');
}

describe('clone capability across mobile sheet and desktop card', () => {
  it('hides clone for registered and legacy WARP', async () => {
    for (const server of [warp('registered'), legacyWarp('legacy')]) {
      expect(mobileHasClone(server)).toBe(false);
      expect(await desktopHasClone(server)).toBe(false);
    }
  });

  it('keeps clone for Tailscale, ordinary WireGuard and proxy nodes', async () => {
    for (const server of [ts('ts'), plainWg('wg'), base('vless')]) {
      expect(mobileHasClone(server)).toBe(true);
      expect(await desktopHasClone(server)).toBe(true);
    }
  });
});
