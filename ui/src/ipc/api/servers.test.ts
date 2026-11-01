import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { IPC_CHANNELS } from '@/domain/ipc-channels';
import type { ServerConfig } from '@/contracts/types';
import type { IdentityConflicts } from '@/domain/identity-repair/model';

const invoke = vi.hoisted(() => vi.fn<(...args: unknown[]) => Promise<unknown>>());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const { serverApi } = await import('./servers');
beforeEach(() => {
  invoke.mockReset().mockResolvedValue({ 'ts-1': false });
  vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

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

describe('identity preview through the real non-void IPC boundary', () => {
  const preview: IdentityConflicts = { version: 'opaque revision, not a format contract', groups: [{
    indices: [0, 2], repairable: true, records: [
      { index: 0, name: 'Same name', protocol: 'trojan', address: 'one.invalid', port: 443 },
      { index: 2, name: 'Same name', protocol: 'trojan', address: null, port: null },
    ],
  }, { indices: [1, 3], repairable: false, records: [
    { index: 1, name: '', protocol: 'tailscale', address: '', port: 0 },
    { index: 3, name: 'Other', protocol: '', address: null, port: null },
  ] }] };

  it('preserves a complete clear projection without inventing a version format', async () => {
    const clear = { version: '', groups: [] } satisfies IdentityConflicts;
    invoke.mockResolvedValue({ success: true, data: clear });
    await expect(serverApi.identityConflicts()).resolves.toBe(clear);
    expect(invoke).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.SERVER_IDENTITY_CONFLICTS, {});
  });

  it('preserves every conflict record, explicit unsupported group and opaque revision', async () => {
    invoke.mockResolvedValue({ success: true, data: preview });
    await expect(serverApi.identityConflicts()).resolves.toBe(preview);
    expect(invoke).toHaveBeenCalledExactlyOnceWith(IPC_CHANNELS.SERVER_IDENTITY_CONFLICTS, {});
    // The largest native u64 is represented as 2**64 after JSON.parse.
    const largePort = { ...preview, groups: [{ ...preview.groups[0]!, records: [
      { ...preview.groups[0]!.records[0]!, port: 2 ** 64 }, preview.groups[0]!.records[1]!,
    ] }] };
    invoke.mockResolvedValue({ success: true, data: largePort });
    await expect(serverApi.identityConflicts()).resolves.toBe(largePort);
  });

  it('rejects absent and malformed projections before exposing recovery choices', async () => {
    const group = preview.groups[0]!;
    const record = group.records[0]!;
    const invalid: unknown[] = [undefined, null, { groups: [] }, { version: 1, groups: [] }, { version: 'v' },
      { version: 'v', groups: [null] },
      { version: 'v', groups: [{ ...group, repairable: 'yes' }] },
      { version: 'v', groups: [{ ...group, indices: [0, 0] }] },
      { version: 'v', groups: [{ ...group, indices: [2, 0] }] },
      { version: 'v', groups: [{ ...group, records: [] }] },
      { version: 'v', groups: [{ ...group, records: [null, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, index: 1 }, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, name: null }, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, protocol: 1 }, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, address: 1 }, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, port: '443' }, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, port: -1 }, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, port: Infinity }, group.records[1]] }] },
      { version: 'v', groups: [{ ...group, records: [{ ...record, port: 2 ** 65 }, group.records[1]] }] },
      { version: 'v', groups: [group, group] }];
    for (const data of invalid) {
      invoke.mockResolvedValue({ success: true, data });
      await expect(serverApi.identityConflicts()).rejects.toThrow('Invalid identity conflict preview');
    }
    expect(invoke.mock.calls.every(([command]) => command === IPC_CHANNELS.SERVER_IDENTITY_CONFLICTS)).toBe(true);
  });

  it('routes the real plain-browser unavailable result into rejection, not a false clear state', async () => {
    vi.stubGlobal('window', {});
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(serverApi.identityConflicts()).rejects.toThrow('Invalid identity conflict preview');
    expect(invoke).not.toHaveBeenCalled();
    expect(warning).toHaveBeenCalledOnce();
  });

  it('preserves transport rejection and native failure-envelope meaning', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const transportError = new Error('native read unavailable');
    invoke.mockRejectedValueOnce(transportError);
    await expect(serverApi.identityConflicts()).rejects.toBe(transportError);
    invoke.mockResolvedValueOnce({ success: false, error: 'native read refused', code: 'native-read-error' });
    await expect(serverApi.identityConflicts()).rejects.toMatchObject({
      name: 'IpcError', command: IPC_CHANNELS.SERVER_IDENTITY_CONFLICTS, code: 'native-read-error', message: 'native read refused',
    });
  });

  it('keeps an unrelated native void success legal through that same actual unwrap', async () => {
    invoke.mockResolvedValue({ success: true });
    await expect(serverApi.update({ id: 'unchanged', protocol: 'socks' } as ServerConfig)).resolves.toBeUndefined();
    await expect(serverApi.identityConflicts()).rejects.toThrow('Invalid identity conflict preview');
  });
});
