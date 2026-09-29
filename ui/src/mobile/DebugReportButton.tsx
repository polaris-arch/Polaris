import { useEffect, useRef, useState, type ReactElement } from 'react';
import { diagnosticApi, versionApi } from '@/ipc/api-client';
import { recoveryText } from '@/i18n/recovery-text';

/** Also works inside ErrorBoundary: no main i18n/provider or application store dependency. */
export function DebugReportButton(): ReactElement | null {
  const [available, setAvailable] = useState(false);
  const [status, setStatus] = useState<'idle' | 'busy' | 'done' | 'failed'>('idle');
  const inFlight = useRef(false);
  useEffect(() => {
    let active = true;
    void versionApi.getInfo().then((info) => {
      if (active) setAvailable(info?.debugReportAvailable === true);
    }).catch(() => {});
    return () => { active = false; };
  }, []);

  async function exportReport(): Promise<void> {
    if (inFlight.current) return;
    inFlight.current = true;
    setStatus('busy');
    try {
      const result = await diagnosticApi.export();
      setStatus(result?.success === true ? 'done' : 'failed');
    } catch {
      setStatus('failed');
    } finally {
      inFlight.current = false;
    }
  }

  if (!available) return null;
  return (
    <div style={{ marginTop: 16, overflowWrap: 'anywhere' }}>
      <button
        type="button"
        disabled={status === 'busy'}
        aria-busy={status === 'busy'}
        onClick={() => { void exportReport(); }}
        style={{
          font: 'inherit', color: 'inherit', background: 'transparent', border: '1px solid currentColor',
          borderRadius: 8, padding: '9px 16px', minHeight: 44, maxWidth: '100%', cursor: 'pointer',
        }}
      >
        {recoveryText(status === 'busy' ? 'reportBusy' : 'report')}
      </button>
      <p style={{ fontSize: 12, lineHeight: 1.6, margin: '8px 0 0' }}>
        {recoveryText('reportHint')}
      </p>
      <p role="status" style={{ fontSize: 13, margin: '8px 0 0' }}>
        {status === 'done' ? recoveryText('reportDone') : status === 'failed' ? recoveryText('reportFailed') : ''}
      </p>
    </div>
  );
}
