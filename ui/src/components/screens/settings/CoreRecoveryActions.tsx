import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { coreApi } from '@/ipc/api/updater';
import { useConfirmTwice } from '@/lib/confirm-twice';
import { useNavStore } from '@/store/nav-store';
import type { CoreRecoveryState } from '@/contracts/types/update';
import { Button } from './Primitives';

export default function CoreRecoveryActions({ onRecovered }: { onRecovered?: (result: 'restored' | 'notNeeded') => void }) {
  const { t } = useTranslation();
  const [result, setResult] = useState<CoreRecoveryState | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const { armed, confirmTwice } = useConfirmTwice();

  async function perform(apply: boolean) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    // Clear the old offer before a new request. Errors cannot leave a stale
    // Ready/confirmation available for another mutation.
    setResult(null);
    try {
      const next = await (apply ? coreApi.restoreBundledData() : coreApi.checkOfflineRecovery());
      setResult(next);
      if (next === 'restored' || next === 'notNeeded') onRecovered?.(next);
    } catch {
      setResult('unavailable');
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }

  return <div data-core-recovery="current-app">
    <p role="status" aria-live="polite">{t(busy ? 'coreRecovery.working' : result ? 'coreRecovery.' + result : 'coreRecovery.intro')}</p>
    {!busy && result === 'ready' && <Button
      variant="ghost" size="sm" className={armed === 'restore-core-data' ? 'confirming' : undefined}
      onClick={() => confirmTwice('restore-core-data', () => { void perform(true); })}
    >{t(armed === 'restore-core-data' ? 'coreRecovery.confirm' : 'coreRecovery.restore')}</Button>}
    {!busy && (result === null || result === 'stopRequired' || result === 'unavailable') &&
      <Button variant="ghost" size="sm" onClick={() => { void perform(false); }}>{t('coreRecovery.check')}</Button>}
    {!busy && result === 'helperRequired' &&
      <Button variant="ghost" size="sm" onClick={() => useNavStore.getState().enterSettings('helper')}>{t('coreRecovery.helper')}</Button>}
    {result !== 'restored' && result !== 'notNeeded' &&
      <a href="#app-update-card">{t('coreRecovery.reinstall')}</a>}
  </div>;
}
