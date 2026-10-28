import { useTranslation } from 'react-i18next';
import { useAppStore } from '@/store/app-store';
import { useNavStore } from '@/store/nav-store';
import { useDialogStore } from './dialog-store';
import { ConfirmDialog } from './ConfirmDialog';

/** Main-window explanation only. No install, stop, retry, or native authorization lives here. */
export function HelperRecoveryDialog({ instanceId }: { instanceId: string }) {
  const { t } = useTranslation();
  return <ConfirmDialog instanceId={instanceId} payload={{
    title: t('helper.connectionRecoveryTitle'),
    message: t('helper.connectionRecoveryMessage'),
    confirmLabel: t('helper.connectionRecoverySettings'),
    onConfirm: () => {
      const dialogs = useDialogStore.getState();
      // Modal hides itself while locked, but an old callback is not authority either.
      if (useAppStore.getState().privacyMode || dialogs.stack[dialogs.stack.length - 1]?.instanceId !== instanceId) return;
      dialogs.closeInstance(instanceId);
      useNavStore.getState().enterSettings('helper');
    },
  }} />;
}
