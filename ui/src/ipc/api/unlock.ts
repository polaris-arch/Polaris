import { invoke, listen } from '../ipc-client';
import { IPC_CHANNELS } from '../../domain/ipc-channels';
import type { UnlockSnapshot, UnlockProgress, UnlockInvalidatedPayload } from '../../contracts/unlock-detection';
import type { ManualNetworkCheckResult } from '../../contracts/manual-network-check';

// ============================================================================
// unlockApi —— 解锁检测（AI/流媒体），经当前代理出口。
// ============================================================================

export const unlockApi = {
  /** Explicit iOS action: prepare normal main, then check IP and services through that same main. */
  async manualCheck(requestId: string, force = false): Promise<ManualNetworkCheckResult> {
    return invoke(IPC_CHANNELS.MANUAL_NETWORK_CHECK, { requestId, force });
  },
  async cancelManualCheck(requestId: string): Promise<void> {
    return invoke(IPC_CHANNELS.MANUAL_NETWORK_CHECK_CANCEL, { requestId });
  },
  /** 跑一轮检测（force 绕 TTL，仍受 15s 硬下限约束）。 */
  async run(force = false): Promise<UnlockSnapshot> {
    return invoke(IPC_CHANNELS.UNLOCK_RUN, { force });
  },
  /** 纯读最近快照（页面挂载水合，零网络）；无则 null。 */
  async get(): Promise<UnlockSnapshot | null> {
    return invoke(IPC_CHANNELS.UNLOCK_GET);
  },
  /** 单个服务 settle 逐个点亮。 */
  onProgress(listener: (p: UnlockProgress) => void): () => void {
    return listen(IPC_CHANNELS.EVENT_UNLOCK_PROGRESS, listener);
  },
  /** 切节点/起停代理 → 缓存失效。 */
  onInvalidated(listener: (p: UnlockInvalidatedPayload) => void): () => void {
    return listen(IPC_CHANNELS.EVENT_UNLOCK_INVALIDATED, listener);
  },
  /** 一轮检测完成的完整终态快照。 */
  onUpdated(listener: (snap: UnlockSnapshot) => void): () => void {
    return listen(IPC_CHANNELS.EVENT_UNLOCK_UPDATED, listener);
  },
};
