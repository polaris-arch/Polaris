import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import { useAppStore } from '@/store/app-store';

/** Saved-disk recovery must remain reachable when security maintenance blocks config_get.
 * Only the dedicated read-only conflict projection is read; never expose an unmaintained config.
 */
export function IdentityRecoveryEntry({ onOpen }: { onOpen: () => void }) {
  const { t } = useTranslation();
  const publication = useAppStore(state => state.configPublication);
  const [retry, setRetry] = useState(0);
  const [status, setStatus] = useState<'loading' | 'clear' | 'conflict' | 'failed'>('loading');
  useEffect(() => {
    let current = true;
    setStatus('loading');
    void api.server.identityConflicts().then(
      preview => { if (current) setStatus(preview.groups.length ? 'conflict' : 'clear'); },
      () => { if (current) setStatus('failed'); },
    );
    return () => { current = false; };
  }, [publication, retry]);
  if (status === 'conflict') return <div role="alert">
    <p>{t('identityRepair.ambiguous')}</p>
    <button type="button" className="btn" onClick={onOpen}>{t('identityRepair.review')}</button>
  </div>;
  if (status === 'failed') return <div role="status">
    <p>{t('identityRepair.loadFailed')}</p>
    <button type="button" className="btn" onClick={() => setRetry(value => value + 1)}>{t('identityRepair.reload')}</button>
  </div>;
  return null;
}
