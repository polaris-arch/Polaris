import { beforeEach, describe, expect, it, vi } from 'vitest';
const invoke = vi.hoisted(() => vi.fn(async () => ({ context: { requestId: 'requested', mainGeneration: 7 }, ipInfo: {}, unlock: {} })));
vi.mock('../ipc-client', () => ({ invoke, listen: vi.fn() }));
import { unlockApi } from './unlock';
import { IPC_CHANNELS } from '@/domain/ipc-channels';

describe('explicit manual network IPC', () => {
  beforeEach(() => invoke.mockClear());
  it('one bound request returns independent leg results without issuing unbound reads', async () => {
    expect(await unlockApi.manualCheck('requested', true)).toMatchObject({ context: { requestId: 'requested', mainGeneration: 7 } });
    expect(invoke).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.MANUAL_NETWORK_CHECK, { requestId: 'requested', force: true });
  });
  it('cancels only its action observer', async () => {
    await unlockApi.cancelManualCheck('requested');
    expect(invoke).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.MANUAL_NETWORK_CHECK_CANCEL, { requestId: 'requested' });
  });
});
