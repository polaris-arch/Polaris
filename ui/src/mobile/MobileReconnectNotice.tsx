import { useTranslation } from 'react-i18next';
import { useAppStore } from '@/store/app-store';

/** Persisted native reload refusal is visible even when Android notifications are disabled. */
export function MobileReconnectNotice() {
  const { t } = useTranslation();
  const required = useAppStore((state) => state.proxyStatus?.reconnectRequired ?? false);
  if (!required) return null;
  return (
    <div className="m-pending m-pending-err" role="alert">
      <div className="m-pending-text">
        <b>{t('home.nativeReconnectRequired')}</b>
      </div>
    </div>
  );
}
