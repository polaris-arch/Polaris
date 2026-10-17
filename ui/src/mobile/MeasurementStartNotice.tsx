import { useEffect, useRef } from 'react';
import { useSheetFocus } from './use-sheet-focus';
import { create } from 'zustand';
import { useTranslation } from 'react-i18next';
import { useDismissableLayer } from './back-stack';

type ConsentState = { pending: boolean; answer: ((accepted: boolean) => void) | null };
const consent = create<ConsentState>(() => ({ pending: false, answer: null }));

export const requiresMeasurementConsent = (): boolean => typeof document !== 'undefined'
  && document.documentElement.dataset.mobileOs === 'ios';

/** One explicit action grant, never persisted or inferred from an earlier measurement. */
export function requestIOSMeasurementConsent(): Promise<boolean> {
  if (!requiresMeasurementConsent()) return Promise.resolve(true);
  if (consent.getState().pending) return Promise.resolve(false);
  return new Promise(resolve => consent.setState({ pending: true, answer: accepted => {
    consent.setState({ pending: false, answer: null }); resolve(accepted);
  } }));
}

export function cancelMeasurementConsent(): void { consent.getState().answer?.(false); }

export function MeasurementStartNotice({ disabled = false }: { disabled?: boolean }) {
  const { t } = useTranslation();
  const pending = consent(state => state.pending);
  const panel = useRef<HTMLDivElement>(null);
  useSheetFocus(panel, pending && !disabled, cancelMeasurementConsent);
  useDismissableLayer(pending, cancelMeasurementConsent);
  useEffect(() => () => cancelMeasurementConsent(), []);
  useEffect(() => { if (disabled) cancelMeasurementConsent(); }, [disabled]);
  if (!pending || disabled) return null;
  return <div ref={panel} tabIndex={-1} role="dialog" aria-modal="true" aria-labelledby="measurement-start-title"
    style={{ position: 'fixed', inset: 0, zIndex: 10000, display: 'grid', placeItems: 'center', background: '#0008', padding: '1rem' }}>
    <section style={{ background: 'hsl(var(--bg))', color: 'hsl(var(--text))', borderRadius: '1rem', padding: '1.25rem', maxWidth: '28rem' }}>
      <h2 id="measurement-start-title">{t('measurement.iosTitle')}</h2>
      <p>{t('measurement.iosWillStart')}</p>
      <div style={{ display: 'flex', gap: '1rem' }}>
        <button type="button" onClick={cancelMeasurementConsent}>{t('common.cancel')}</button>
        <button type="button" onClick={() => consent.getState().answer?.(true)}>{t('common.confirm')}</button>
      </div>
    </section>
  </div>;
}
