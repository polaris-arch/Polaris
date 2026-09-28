import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ServerConfig } from '@/contracts/types';

vi.mock('react-i18next', () => ({
  initReactI18next: { type: '3rdParty', init: () => {} },
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: 'zh-CN' } }),
}));

(globalThis as unknown as { document: unknown }).document = {
  documentElement: { dir: '', lang: '', getAttribute: () => null, setAttribute: () => {} },
  body: { nodeType: 1 },
};

const { useAppStore } = await import('@/store/app-store');
const { TsLoginDialog } = await import('./TsLoginDialog');

function node(id: string, name: string, protocol: ServerConfig['protocol']): ServerConfig {
  return { id, name, protocol, address: '', port: 0 } as ServerConfig;
}

function render(servers: ServerConfig[], serverId?: string): string {
  useAppStore.setState({ servers } as never);
  Object.assign(useAppStore.getInitialState(), { servers });
  return renderToStaticMarkup(<TsLoginDialog serverId={serverId} />);
}

describe('TsLoginDialog node name', () => {
  it('numbers the default against every name in the real mesh tab, ignoring the manual tab', () => {
    const html = render([
      node('wg', ' TAILSCALE ', 'wireguard'),
      node('ts', 'Tailscale 3', 'tailscale'),
      node('manual', 'Tailscale 2', 'shadowsocks'),
    ]);
    expect(html).toMatch(/id="ts-login-name"[^>]*value="Tailscale 2"/);
    expect(html).toContain('ts.nodeName');
  });

  it('loads the selected Tailscale node display name for editing', () => {
    const html = render([node('ts-existing', 'Office TS', 'tailscale')], 'ts-existing');
    expect(html).toMatch(/id="ts-login-name"[^>]*value="Office TS"/);
  });
});
