import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import type { ServerConfig } from '@/contracts/types';

const invoke = vi.hoisted(() => vi.fn(async () => ({ 'ts-1': false })));
vi.mock('../ipc-client', () => ({ invoke, listen: vi.fn() }));
const { serverApi } = await import('./servers');

describe('Tailscale IPC intent and presence', () => {
  beforeEach(() => invoke.mockClear());

  it('keeps the default presence query in directory mode', async () => {
    await serverApi.tailscaleStateExists(['ts-1']);
    expect(invoke).toHaveBeenCalledWith(IPC_CHANNELS.TAILSCALE_STATE_EXISTS, {
      serverIds: ['ts-1'], cachedSessionOnly: false,
    });
  });

  it.each(['browser', 'authkey'] as const)('preserves explicit %s replacement intent and defaults ordinary login to false', async mode => {
    const server = { id: 'ts-1', protocol: 'tailscale' } as ServerConfig;
    await serverApi.tailscaleLogin(server, { attemptId: 'A', mode, replaceIdentity: true, reuseRetainedAuthKey: false });
    expect(invoke).toHaveBeenLastCalledWith(IPC_CHANNELS.TAILSCALE_LOGIN, { server, request: { attemptId: 'A', mode, replaceIdentity: true, reuseRetainedAuthKey: false } });
    await serverApi.tailscaleLogin(server, { attemptId: 'B', mode });
    expect(invoke).toHaveBeenLastCalledWith(IPC_CHANNELS.TAILSCALE_LOGIN, { server, request: { attemptId: 'B', mode, replaceIdentity: false, reuseRetainedAuthKey: false } });
  });

  it('requires explicit presentation opt-in to inspect a cached session', async () => {
    await serverApi.tailscaleStateExists(['ts-1'], true);
    expect(invoke).toHaveBeenCalledWith(IPC_CHANNELS.TAILSCALE_STATE_EXISTS, {
      serverIds: ['ts-1'], cachedSessionOnly: true,
    });
  });
});


it('carries explicit reuse and opaque revision through the original single IPC', async () => {
  const server = { id: 'ts-1', protocol: 'tailscale', tailscaleSettings: {} } as ServerConfig;
  await serverApi.tailscaleLogin(server, { attemptId: 'reuse-A', mode: 'authkey', reuseRetainedAuthKey: true, expectedCredentialRevision: 'opaque-A' });
  expect(invoke).toHaveBeenLastCalledWith(IPC_CHANNELS.TAILSCALE_LOGIN, { server, request: { attemptId: 'reuse-A', mode: 'authkey',
    replaceIdentity: false, reuseRetainedAuthKey: true, expectedCredentialRevision: 'opaque-A' } });
});

it('passes the one-shot clear intent on the original server update rather than fetching secrets', async () => {
  const server = { id: 'ts-1', protocol: 'tailscale', tailscaleSettings: {}, tailscaleCredentialIntent: { action: 'clear', expectedCredentialRevision: 'opaque-A' } } as ServerConfig;
  await serverApi.update(server);
  expect(invoke).toHaveBeenLastCalledWith(IPC_CHANNELS.SERVER_UPDATE, { server });
});
