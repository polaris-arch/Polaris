/**
 * 切换出口节点 —— **全仓唯一**的切换腿（首页出口选单 / 节点页卡片共用）。
 *
 * 原先只活在 `HomeScreen.onPickNode` 里。节点页补「点卡片切换」时把它提出来，而不是照抄一份：
 * 共用同一个后端选择收据，避免一个入口把“已保存”误报成“已生效”。
 *
 * 后端先保存选择，再返回有归属的活核收据。只有 applied 可提示已切换；pending、
 * notRunning、deferred 分别说明仍在应用、下次连接或需明确 Apply。节点名在 await 前定格。
 */
import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { IpcError } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { serverSwitchErrorText } from '@/domain/action-error-text';
import { useAppStore } from '@/store/app-store';
import { switchReceiptFeedback } from './switch-receipt-feedback';

export function useSwitchNode(): (id: string) => Promise<void> {
  const { t } = useTranslation();
  const servers = useAppStore((s) => s.servers);
  const switchServer = useAppStore((s) => s.switchServer);

  return useCallback(
    async (id: string) => {
      const nodeName = servers.find((s) => s.id === id)?.name ?? '';
      try {
        const receipt = await switchServer(id);
        const feedback = switchReceiptFeedback(receipt, nodeName, t);
        if (feedback?.tone === 'success') toast.success(feedback.text);
        else if (feedback?.tone === 'warning') toast.warning(feedback.text);
        else if (feedback) toast.info(feedback.text);
      } catch (err) {
        console.error('[switch-node] switch server failed:', err);
        toast.error(
          serverSwitchErrorText(err instanceof IpcError ? err.code : undefined, t)
        );
      }
    },
    [switchServer, servers, t]
  );
}
