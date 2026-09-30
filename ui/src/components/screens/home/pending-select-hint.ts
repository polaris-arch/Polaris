import type { PendingNodeChanges } from '@/contracts/types';

/**
 * 选中「待入池(added)/待生效(modified)」节点时，提示仍有未应用的配置。
 *
 * 待应用差集里的 added/modified 均为**未被引用**节点（差集计算已按 referencedServerIds 过滤掉被引用者）。
 * 这是确认前的提示判据，不预测热切换结果。后端 server_switch 收据才说明本次选择
 * 是 applied / pending / deferred / notRunning；尤其 dirty 节点可能必须显式 Apply。
 *
 * 核未运行时 pendingChanges 恒空 → 返回 false，不误报。
 *
 * **刻意不看 `removed`**：`removed` 里的节点已经不在 config 的 servers 里了，出口选单根本列不出它，
 * 这个函数永远不会被拿它的 id 调用。把它并进来是死代码，还会误导后来者以为「能选一个已删的节点」。
 */
export function needsApplyOnSelect(
  pending: PendingNodeChanges,
  serverId: string
): boolean {
  return !!pending?.added?.includes(serverId) || !!pending?.modified?.includes(serverId);
}
