import { invoke } from '../ipc-client';
import { IPC_CHANNELS } from '../../domain/ipc-channels';
import type { BuiltinDhcpStatus, ResolvedProbe } from '../../contracts/types/rules';

/**
 * 网络场景的只读查询。场景本身的增删改走 `api.config.patch({ networkProfiles })`；
 * 规则挂场景走规则 IPC 的 `networkProfileId` 字段。
 */
export const networkProfileApi = {
  /** 每个场景在本机实际使用的探测源 / 是否可用 / 原因码（后端与生成侧同一判据）。 */
  resolvedSources(): Promise<ResolvedProbe[]> {
    return invoke(IPC_CHANNELS.NETWORK_PROFILE_RESOLVED_SOURCES);
  },
  /** 内置解析器 builtin-netenv-dhcp 在本机是否可用（与生成侧同一判据）。 */
  builtinDhcpStatus(): Promise<BuiltinDhcpStatus> {
    return invoke(IPC_CHANNELS.NETWORK_PROFILE_BUILTIN_DHCP_STATUS);
  },
};

export const resolvedSources = networkProfileApi.resolvedSources;
export const builtinDhcpStatus = networkProfileApi.builtinDhcpStatus;
