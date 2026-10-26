import type { ServerConfig } from '@/contracts/types';

export const VISION_TRANSPORT_REASON = 'vision-transport-unsupported';
/** Read-only raw-token admission mirrors the emitter; it never produces a Network value. */
export function visionTransportReason(server: Pick<ServerConfig, 'protocol' | 'flow'> & { network?: string }): string | undefined {
  const transport = server.network?.trim().toLowerCase();
  return server.protocol === 'vless'
    && server.flow?.trim().toLowerCase() === 'xtls-rprx-vision'
    && ['ws', 'grpc', 'http', 'h2', 'httpupgrade'].includes(transport ?? '')
    ? VISION_TRANSPORT_REASON : undefined;
}
