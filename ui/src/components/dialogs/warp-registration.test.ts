import { describe, expect, it, vi } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import type { WarpWireGuardDraft } from '@/domain/warp';
import { executeWarpRegistration, type WarpRegistrationExecution, type WarpRegistrationSession } from './warp-registration';

const draft: WarpWireGuardDraft = {
  address: 'engage.cloudflareclient.com', port: 2408, privateKey: 'private', peerPublicKey: 'peer',
  localAddress: ['172.16.0.2/32'], reserved: [1, 2, 3],
  meta: { deviceId: 'device', accountId: 'account', license: '', warpPlus: false },
  warpDevice: { deviceId: 'device', token: 'token' },
};

function execution(overrides: Partial<WarpRegistrationExecution> = {}): WarpRegistrationExecution {
  let saved: ServerConfig[] = [];
  return {
    register: vi.fn(async () => draft), mintId: vi.fn(() => 'warp-stable'),
    build: (draft, id) => ({ id, name: 'WARP', protocol: 'wireguard', address: draft.address, port: draft.port,
      wireguardSettings: { privateKey: draft.privateKey, peerPublicKey: draft.peerPublicKey, localAddress: draft.localAddress, warpDevice: draft.warpDevice, allowInternet: true } }),
    save: vi.fn(async (server) => { saved = [server]; }), refresh: vi.fn(async () => {}),
    readback: vi.fn(async () => saved), ...overrides,
  };
}

describe('WARP registration followed by confirmed persistence', () => {
  it('save failure retries the successful draft and stable ID with one registration', async () => {
    const session: WarpRegistrationSession = {};
    const input = execution();
    const save = input.save;
    input.save = vi.fn().mockRejectedValueOnce(new Error('SAVE_FAILED')).mockImplementation(save);
    await expect(executeWarpRegistration(session, input)).rejects.toThrow('SAVE_FAILED');
    expect(session.id).toBe('warp-stable');
    expect(session.draft).toBe(draft);
    expect(await executeWarpRegistration(session, input)).toBe(true);
    expect(input.register).toHaveBeenCalledOnce();
    expect(input.mintId).toHaveBeenCalledOnce();
    expect(vi.mocked(input.save).mock.calls.map(([server]) => server.id)).toEqual(['warp-stable', 'warp-stable']);
  });

  it('does not finish before refresh/readback or when add silently loses the node', async () => {
    let finishRefresh!: () => void;
    const input = execution({ refresh: () => new Promise<void>((resolve) => { finishRefresh = resolve; }),
      readback: vi.fn(async () => []) });
    let ended = false;
    const pending = executeWarpRegistration({}, input).finally(() => { ended = true; });
    await vi.waitFor(() => expect(input.save).toHaveBeenCalledOnce());
    expect(ended).toBe(false);
    expect(input.readback).not.toHaveBeenCalled();
    finishRefresh();
    await expect(pending).rejects.toThrow('WARP_SAVE_NOT_CONFIRMED');
  });

  it('lost acknowledgement/readback retries the same persisted identity without registering again', async () => {
    const session: WarpRegistrationSession = {};
    const input = execution();
    const readback = input.readback;
    input.readback = vi.fn().mockRejectedValueOnce(new Error('READBACK_FAILED')).mockImplementation(readback);
    await expect(executeWarpRegistration(session, input)).rejects.toThrow('READBACK_FAILED');
    input.build = vi.fn(() => { throw new Error('MUST_REUSE_FIRST_PAYLOAD'); });
    expect(await executeWarpRegistration(session, input)).toBe(true);
    expect(input.register).toHaveBeenCalledOnce();
    expect(input.mintId).toHaveBeenCalledOnce();
    expect(input.save).toHaveBeenCalledOnce();
    expect(input.build).not.toHaveBeenCalled();
  });

  it('readback must match the registered device identity as well as keys', async () => {
    const input = execution();
    const readback = input.readback;
    input.readback = async () => (await readback()).map((server) => ({ ...server,
      wireguardSettings: { ...server.wireguardSettings!, warpDevice: { deviceId: 'another-device', token: 'token' } } }));
    await expect(executeWarpRegistration({}, input)).rejects.toThrow('WARP_SAVE_NOT_CONFIRMED');
  });

  it('invalid draft or an occupied slot cannot report successful persistence', async () => {
    const invalid = execution({ register: async () => ({ ...draft, privateKey: '' }) });
    await expect(executeWarpRegistration({}, invalid)).rejects.toThrow('WARP_REGISTRATION_INVALID');
    expect(invalid.save).not.toHaveBeenCalled();
    const blocked = execution({ register: async () => null });
    expect(await executeWarpRegistration({}, blocked)).toBe(false);
    expect(blocked.save).not.toHaveBeenCalled();
    expect(blocked.mintId).not.toHaveBeenCalled();
  });
});
