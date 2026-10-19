import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { VpnAuthForm } from '@/components/vpn/VpnAuthForm';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

export function VpnAuthPanel({ instanceId, protocol, serverId }: {
  instanceId: string; protocol: 'openconnect' | 'openvpn'; serverId: string;
}) {
  const { t } = useTranslation();
  const closeInstance = useMobileFormStore(s => s.closeInstance);
  const close = useCallback(() => closeInstance(instanceId), [closeInstance, instanceId]);
  return <VpnAuthForm mobile protocol={protocol} serverId={serverId} close={close} renderFrame={frame => (
    <FormSheet title={frame.title} onRequestClose={frame.requestClose} closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')} submitLabel={frame.canSubmit ? t('common.confirm') : undefined}
      submitDisabled={frame.submitting} onSubmit={frame.submit}>
      {frame.children}
      {frame.openBrowser && <button type="button" className="m-form-btn" onClick={frame.openBrowser}>{t('vpnAuth.openBrowser')}</button>}
    </FormSheet>
  )} />;
}
