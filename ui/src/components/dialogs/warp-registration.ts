import type { ServerConfig } from '@/contracts/types';
import type { WarpWireGuardDraft } from '@/domain/warp';
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
