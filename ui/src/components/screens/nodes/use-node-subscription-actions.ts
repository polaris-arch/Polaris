import { useCallback } from 'react';
import type { TFunction } from 'i18next';
import type { DialogDesc } from '@/components/dialogs/dialog-store';
import type { ServerConfig, SubscriptionConfig } from '@/contracts/types';
import { refreshSubscriptionWithToast } from '@/domain/subscription-refresh';
import { api } from '@/ipc';
import { editRoute, type StagedEntry } from '@/lib/staged-config';
import { toast } from '@/lib/error-handler';
import { useNavStore } from '@/store/nav-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { subDeleteNodeCount, type SubMenuItem } from './nodes-logic';

interface Args {
  diskServers: ServerConfig[];
  buildServerDeletionEntries: (
    targets: readonly ServerConfig[],
    removedIds: ReadonlySet<string>,
    groupId?: string
  ) => StagedEntry[];
  openDialog: (dialog: DialogDesc) => void;
  closeDialog: () => void;
  t: TFunction;
}

export function useNodeSubscriptionActions({
  diskServers,
  buildServerDeletionEntries,
  openDialog,
  closeDialog,
  t,
}: Args) {
  const stagingEnabled = useStagingActive();
  const submitEdit = useStagedConfigStore((state) => state.submit);
  const enterSettings = useNavStore((state) => state.enterSettings);

  const refreshSub = useCallback(
    async (sub: SubscriptionConfig) => {
      await refreshSubscriptionWithToast(sub.id, t);
    },
    [t]
  );

  const requestSubDelete = useCallback(
    (sub: SubscriptionConfig) => {
      const count = subDeleteNodeCount(diskServers, sub);
      openDialog({
        kind: 'confirm',
        payload: {
          title: t('nodes.subDeleteTitle'),
          message: t('nodes.subDeleteConfirm', { name: sub.name, count }),
          confirmLabel: t('common.delete'),
          danger: true,
          onConfirm: () => {
            closeDialog();
            if (editRoute('subscriptions', stagingEnabled) === 'staged') {
              const targets = diskServers.filter(
                (server) => server.subscriptionId === sub.id
              );
              const removedIds = new Set(targets.map((server) => server.id));
              const groupId = `subscriptionDelete:${crypto.randomUUID()}`;
              void submitEdit({
                id: `subscription:${sub.id}`,
                kind: 'subscription',
                label: `${t('common.delete')} ${sub.name}`,
                entityPath: ['subscriptions', sub.id],
                nextValue: null,
                groupId,
              }, ...buildServerDeletionEntries(targets, removedIds, groupId));
              return;
            }
            void api.subscription
              .delete(sub.id)
              .then(() => toast.info(t('nodes.subDeleteOk', { count })))
              .catch((err) => {
                console.error('[NodesScreen] sub delete:', err);
                toast.error(t('nodes.deleteFail'));
              });
          },
        },
      });
    },
    [
      closeDialog,
      diskServers,
      openDialog,
      submitEdit,
      buildServerDeletionEntries,
      stagingEnabled,
      t,
    ]
  );

  const onSubMenuAction = useCallback(
    (item: SubMenuItem, sub: SubscriptionConfig) => {
      switch (item) {
        case 'rename':
          openDialog({ kind: 'sub', subId: sub.id, focus: 'name' });
          return;
        case 'edit-url':
          openDialog({ kind: 'sub', subId: sub.id, focus: 'url' });
          return;
        case 'copy-url':
          void navigator.clipboard
            .writeText(sub.url)
            .then(() => toast.success(t('nodes.subCopyUrlOk')))
            .catch((err) => {
              console.error('[NodesScreen] copy sub url failed:', err);
              toast.error(t('nodes.copyLinksFailed'));
            });
          return;
        case 'interval':
          enterSettings('update');
          return;
        case 'delete':
          requestSubDelete(sub);
          return;
      }
    },
    [enterSettings, openDialog, requestSubDelete, t]
  );

  return { refreshSub, requestSubDelete, onSubMenuAction };
}
