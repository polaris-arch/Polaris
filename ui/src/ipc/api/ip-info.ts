import { invoke, listenReady } from '../ipc-client';
import { IPC_CHANNELS } from '../../domain/ipc-channels';
import type { IpInfoSnapshot } from '../../contracts/types';

// ============================================================================
// ipInfoApi
// ============================================================================

export const ipInfoApi = {
  /** 获取出口 IP 快照。force=强制重测；visible=手动重探可见流程。 */
  async get(force = false, visible = false): Promise<IpInfoSnapshot> {
    return invoke(IPC_CHANNELS.IP_INFO_GET, { force, visible });
  },

  /** 纯读当前快照（零探测）：窗口重建后 store 为空时水合状态栏。绝不触发探测。 */
  async peek(): Promise<IpInfoSnapshot> {
    return invoke(IPC_CHANNELS.IP_INFO_GET, { peek: true });
  },

  /** 先等事件登记完成，再零探测读快照，避免窗口重建时漏掉两者之间的更新。 */
  onUpdated(listener: (snap: IpInfoSnapshot) => void): () => void {
    let disposed = false;
    let off: (() => void) | undefined;
    const accept = (snapshot: IpInfoSnapshot) => {
      if (!disposed) listener(snapshot);
    };
    const hydrate = () => {
      if (!disposed) return ipInfoApi.peek().then(accept).catch(() => {});
    };
    void listenReady(IPC_CHANNELS.EVENT_IP_INFO_UPDATED, accept).then((unlisten) => {
      if (disposed) {
        unlisten();
        return;
      }
      off = unlisten;
      return hydrate();
    }, hydrate);
    return () => {
      disposed = true;
      off?.();
      off = undefined;
    };
  },
};
