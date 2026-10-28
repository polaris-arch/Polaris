import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { visionTransportReason } from './node-compatibility';
import { invalidNodeIndex } from '@/components/screens/nodes/nodes-logic';
import type { Network, ServerConfig } from '@/contracts/types';

describe('current-core Vision transport admission', () => {
  it('rejects only the emitted extra transports and preserves the input', () => {
    const networks: Network[] = ['ws', 'grpc', 'http', 'httpupgrade'];
    for (const network of networks) {
      const server: ServerConfig = { id: 'bad', name: 'RepairMe', protocol: 'vless', address: 'fixture.example', port: 443, flow: ' XTls-RPRX-Vision ', network };
      const before = JSON.stringify(server);
      expect(visionTransportReason(server)).toBe('vision-transport-unsupported');
      expect(invalidNodeIndex([], [server])).toEqual({ bad: 'vision-transport-unsupported' });
      expect(JSON.stringify(server)).toBe(before);
      expect(invalidNodeIndex([{ id: 'bad', tag: 'RepairMe', reason: 'vision-transport-unsupported' }], [{ ...server, network: 'tcp' }])).toEqual({});
      expect(invalidNodeIndex([{ id: 'bad', tag: 'RepairMe', reason: 'detour-cascade' }], [{ ...server, network: 'tcp' }])).toEqual({ bad: 'detour-cascade' });

      expect(visionTransportReason({ ...server, flow: undefined })).toBeUndefined();
      expect(visionTransportReason({ ...server, protocol: 'vmess' })).toBeUndefined();
    }
    // Raw aliases are input to the read-only predicate, not falsely typed persisted nodes.
    for (const network of ['h2', ' WS ', 'H2']) {
      expect(visionTransportReason({ protocol: 'vless', flow: 'xtls-rprx-vision', network })).toBe('vision-transport-unsupported');
    }
    for (const network of [undefined, '', 'tcp']) {
      expect(visionTransportReason({ protocol: 'vless', flow: 'xtls-rprx-vision', network })).toBeUndefined();
    }
  });
  it('desktop and mobile reject before save without clearing the draft', () => {
    for (const path of ['src/components/dialogs/NodeDialog.tsx', 'src/mobile/forms/NodeFormPanel.tsx']) {
      const source = readFileSync(path, 'utf8');
      expect(source).toContain('visionTransportReason(full)');
      expect(source).toContain("'nodes.invalidVisionTransport'");
      expect(source).toMatch(/if \(utlsError\) \{[\s\S]*?return;/);
    }
  });
});
