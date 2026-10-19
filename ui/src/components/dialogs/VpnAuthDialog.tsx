import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { VpnAuthForm } from '@/shared/vpn/VpnAuthForm';
import { Modal } from './Modal';
import { useDialogStore } from './dialog-store';

export function VpnAuthDialog({ instanceId, protocol, serverId }: {
  instanceId: string; protocol: 'openconnect' | 'openvpn'; serverId: string;
}) {
  const { t } = useTranslation();
  const closeInstance = useDialogStore(s => s.closeInstance);
  const close = useCallback(() => closeInstance(instanceId), [closeInstance, instanceId]);
  return <VpnAuthForm protocol={protocol} serverId={serverId} close={close} renderFrame={frame => (
    <Modal icon={<svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M7 11V8a5 5 0 0 1 10 0v3" /><rect x="4" y="11" width="16" height="10" rx="2" /></svg>} titleId="vpn-auth-title" title={frame.title} onClose={frame.requestClose} className="entry-form-dlg" footer={<>
      <button type="button" className="btn ghost" onClick={frame.requestClose}>{t('common.cancel')}</button>
      {frame.openBrowser && <button type="button" className="btn ghost" onClick={frame.openBrowser}>{t('vpnAuth.openBrowser')}</button>}
      {frame.canSubmit && <button type="button" className="btn flow" disabled={frame.submitting} onClick={frame.submit}>{frame.submitting && <span className="spinner spin-inline" style={{ marginRight: 6 }} />}{t('common.confirm')}</button>}
    </>}>{frame.children}</Modal>
  )} />;
}
export default VpnAuthDialog;
