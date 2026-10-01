import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import type { ServerConfig } from '@/contracts/types';
import type { WarpWireGuardDraft } from '@/domain/warp';
import { groupServersBySubscription } from '@/domain/server-grouping';
import { DETOUR_NONE } from './detour-options';
import { warpDraftFromNode } from './warp-spec';
import { buildRegisteredWarpServer, saveRegisteredWarp, type WarpRegistrationAttempt } from './warp-registration';

const fixtureDir = new URL('../../../../crates/mesh/src/warp/tests/', import.meta.url);
const registered = JSON.parse(readFileSync(new URL('registration-draft.json', fixtureDir), 'utf8')) as WarpWireGuardDraft;
const expected = JSON.parse(readFileSync(new URL('registration-server.json', fixtureDir), 'utf8')) as ServerConfig;

function attempt(): WarpRegistrationAttempt {
  return { registered: structuredClone(registered), id: expected.id, saved: false };
}
function build(a: WarpRegistrationAttempt): ServerConfig {
  return buildRegisteredWarpServer(a, ' WARP ', null, warpDraftFromNode(), DETOUR_NONE, '');
}

describe('WARP registration, save and readback', () => {
  it('maps the Rust IPC fixture into the exact persisted WG payload and mesh list', () => {
    const server = build(attempt());
    expect(server).toEqual(expected);
    const groups = groupServersBySubscription([server], []);
    expect(groups.find((group) => group.id === 'mesh')?.servers).toEqual([server]);
  });

  it('retains the same identity and payload across an ambiguous save failure', async () => {
    const a = attempt();
    const create = vi.fn(() => build(a));
    let nodes: ServerConfig[] = [];
    const add = vi.fn(async (server: ServerConfig) => {
      nodes = [server]; // The write committed, but the response was lost.
      if (add.mock.calls.length === 1) throw new Error('response lost');
    });
    const refresh = vi.fn(async () => {});
    await expect(saveRegisteredWarp(a, create, add, refresh, () => nodes)).rejects.toThrow('response lost');
    expect(a.saved).toBe(false);
    await saveRegisteredWarp(a, create, add, refresh, () => nodes);
    expect(create).toHaveBeenCalledTimes(1);
    expect(add.mock.calls[0]?.[0]).toBe(add.mock.calls[1]?.[0]);
    expect(nodes).toEqual([expected]);
  });

  it('retries only readback after an acknowledged write and a swallowed refresh failure', async () => {
    const a = attempt();
    let nodes: ServerConfig[] = [];
    const add = vi.fn(async () => {});
    const refresh = vi.fn(async () => {});
    await expect(saveRegisteredWarp(a, () => build(a), add, refresh, () => nodes)).rejects.toThrow('WARP_READBACK_PENDING');
    expect(a.saved).toBe(true);
    nodes = [expected];
    await saveRegisteredWarp(a, () => { throw new Error('must reuse payload'); }, add, refresh, () => nodes);
    expect(add).toHaveBeenCalledTimes(1);
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it('does not declare success for a different or incomplete readback', async () => {
    for (const nodes of [[], [{ ...expected, id: 'another' }], [{ ...expected, wireguardSettings: undefined }]]) {
      const a = attempt();
      await expect(saveRegisteredWarp(a, () => build(a), async () => {}, async () => {}, () => nodes)).rejects.toThrow('WARP_READBACK_PENDING');
    }
  });

  it('rejects the former snake_case response before submitting an incomplete node', () => {
    const a = attempt();
    const invalid = a.registered as unknown as Record<string, unknown>;
    invalid.private_key = invalid.privateKey;
    delete invalid.privateKey;
    expect(() => build(a)).toThrow('WARP_REGISTRATION_INCOMPLETE');
  });
});
