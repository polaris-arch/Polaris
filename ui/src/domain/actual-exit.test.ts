import { describe, expect, it } from 'vitest';
import type { ProxyStatus, UserConfig } from '@/contracts/types';
import { configuredExitId, displayedExitId } from './actual-exit';

describe('shared desktop/mobile/tray actual exit identity', () => {
  for (const actualServerId of ['__direct__', '__block__', 'node-a']) it(`keeps runtime-attested ${actualServerId} despite pending saved choice`, () => {
    const status = { running: true, actualServerId } as ProxyStatus;
    expect(displayedExitId(status, 'pending-node')).toBe(actualServerId);
    expect(configuredExitId(status, { selectedServerId: 'pending-node' } as UserConfig)).toBe(actualServerId);
  });
  it('unknown readback never falls back to desired Direct/Block', () => {
    for (const selectedServerId of ['__direct__', '__block__']) {
      const status = { running: true, actualServerId: null } as ProxyStatus;
      expect(configuredExitId(status, { selectedServerId } as UserConfig)).toBeNull();
    }
  });
});
