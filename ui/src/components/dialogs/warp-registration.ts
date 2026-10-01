import type { ServerConfig } from '@/contracts/types';
import { isWarpServer, type WarpWireGuardDraft } from '@/domain/warp';
import type { FormValues } from './field-spec';
import { applyDetour } from './detour-options';
import { buildWarpSettings } from './wg-logic';
import { isServerComplete } from '@/domain/server-completeness';

/** One successful remote registration is retained across local save/readback retries. */
export interface WarpRegistrationSession {
  draft?: WarpWireGuardDraft;
  id?: string;
  server?: ServerConfig;
  saved?: boolean;
}

export interface WarpRegistrationExecution {
  register: () => Promise<WarpWireGuardDraft | null>;
  mintId: () => string;
  build: (draft: WarpWireGuardDraft, id: string) => ServerConfig;
  save: (server: ServerConfig) => Promise<void>;
  refresh: () => Promise<void>;
  readback: () => Promise<ServerConfig[]>;
}

export async function executeWarpRegistration(
  session: WarpRegistrationSession,
  execution: WarpRegistrationExecution,
): Promise<boolean> {
  if (!session.draft) {
    const draft = await execution.register();
    if (!draft) return false;
    session.draft = draft;
  }
  session.id ??= execution.mintId();
  const server = session.server ??= execution.build(session.draft, session.id);
  if (!isServerComplete(server)) throw new Error('WARP_REGISTRATION_INVALID');
  if (!session.saved) {
    await execution.save(server);
    session.saved = true;
  }
  await execution.refresh();
  const stored = (await execution.readback()).find((node) => node.id === server.id);
  if (!stored || !isServerComplete(stored) || stored.protocol !== server.protocol ||
      stored.address !== server.address || stored.port !== server.port ||
      stored.wireguardSettings?.privateKey !== server.wireguardSettings?.privateKey ||
      stored.wireguardSettings?.peerPublicKey !== server.wireguardSettings?.peerPublicKey ||
      stored.wireguardSettings?.warpDevice?.deviceId !== server.wireguardSettings?.warpDevice?.deviceId) {
    throw new Error('WARP_SAVE_NOT_CONFIRMED');
  }
  return true;
}

/** Kept by the open form: retries reuse both the remote identity and the local id. */
export interface WarpRegistrationAttempt {
  registered: WarpWireGuardDraft;
  id: string;
  server?: ServerConfig;
  saved: boolean;
}

export function buildRegisteredWarpServer(
  attempt: WarpRegistrationAttempt,
  name: string,
  endpoint: { host: string; port: number } | null,
  values: FormValues,
  detour: string,
  bindInterface: string,
): ServerConfig {
  const registered = attempt.registered;
  // A malformed IPC response must never turn into a successful empty WG node.
  if (!registered.privateKey || !registered.peerPublicKey || !registered.localAddress?.length) {
    throw new Error('WARP_REGISTRATION_INCOMPLETE');
  }
  const server: ServerConfig = {
    id: attempt.id,
    name: name.trim(),
    protocol: 'wireguard',
    address: endpoint?.host ?? registered.address,
    port: endpoint?.port ?? registered.port,
    wireguardSettings: buildWarpSettings({
      privateKey: registered.privateKey,
      localAddress: registered.localAddress,
      peerPublicKey: registered.peerPublicKey,
      reserved: registered.reserved,
      warpDevice: registered.warpDevice,
    }, values),
  };
  applyDetour(server, detour);
  if (bindInterface) server.bindInterface = bindInterface;
  return server;
}

/** The first payload is immutable across ambiguous save failures; acknowledged saves only refresh. */
export async function saveRegisteredWarp(
  attempt: WarpRegistrationAttempt,
  build: () => ServerConfig,
  add: (server: ServerConfig) => Promise<void>,
  refresh: () => Promise<void>,
  readServers: () => readonly ServerConfig[],
): Promise<void> {
  attempt.server ??= build();
  if (!attempt.saved) {
    await add(attempt.server);
    attempt.saved = true;
  }
  await refresh();
  const expected = attempt.server.wireguardSettings;
  const node = readServers().find((server) => server.id === attempt.id);
  if (!node || !isWarpServer(node)
    || node.wireguardSettings?.privateKey !== expected?.privateKey
    || node.wireguardSettings?.peerPublicKey !== expected?.peerPublicKey
    || node.wireguardSettings?.warpDevice?.deviceId !== expected?.warpDevice?.deviceId) {
    throw new Error('WARP_READBACK_PENDING');
  }
}
