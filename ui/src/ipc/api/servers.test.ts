import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';

const invoke = vi.hoisted(() => vi.fn(async () => ({ 'ts-1': false })));
vi.mock('../ipc-client', () => ({ invoke, listen: vi.fn() }));
const { serverApi } = await import('./servers');

describe('Tailscale presentation never replaces the physical cleanup query', () => {
  beforeEach(() => invoke.mockClear());

  it('keeps the default auth-key replacement query in physical-directory mode', async () => {
    await serverApi.tailscaleStateExists(['ts-1']);
    expect(invoke).toHaveBeenCalledWith(IPC_CHANNELS.TAILSCALE_STATE_EXISTS, {
      serverIds: ['ts-1'], cachedSessionOnly: false,
    });
  });

  it('requires explicit presentation opt-in to inspect a cached session', async () => {
    await serverApi.tailscaleStateExists(['ts-1'], true);
    expect(invoke).toHaveBeenCalledWith(IPC_CHANNELS.TAILSCALE_STATE_EXISTS, {
      serverIds: ['ts-1'], cachedSessionOnly: true,
    });
  });
});
