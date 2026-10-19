import { useCallback, useEffect, useRef, useState } from 'react';
import { api, IpcError } from '@/ipc';
import { refreshConfig } from '@/store/app-store';
import { identityRepairErrorKey, identityRepairRequest, type IdentityConflicts } from './model';

export function useIdentityRepair() {
  const [preview, setPreview] = useState<IdentityConflicts | null>(null);
  const [keepers, setKeepers] = useState<Record<number, number>>({});
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const savedMessage = useRef('identityRepair.saved');
  const alive = useRef(true);
  const active = useRef(false);
  const load = useCallback(async () => {
    if (active.current) return;
    active.current = true; setBusy(true); setPreview(null); setKeepers({}); setAcknowledged(false);
    try {
      const value = await api.server.identityConflicts();
      if (alive.current) setPreview(value);
    } catch { if (alive.current) setMessage('identityRepair.loadFailed'); }
    finally { active.current = false; if (alive.current) setBusy(false); }
  }, []);
  useEffect(() => { alive.current = true; void load(); return () => { alive.current = false; }; }, [load]);
  const request = identityRepairRequest(preview, keepers);
  const save = async () => {
    if (!request || !acknowledged || active.current || done) return;
    active.current = true; setBusy(true); setMessage(null);
    try {
      const receipt = await api.server.identityRepair(request);
      if (!alive.current) return;
      setDone(true);
      savedMessage.current = receipt.requiresApply ? 'identityRepair.savedPending' : 'identityRepair.saved';
      setMessage(savedMessage.current);
      try { await refreshConfig({ fresh: true, reason: 'identity-repair' }); }
      catch { if (alive.current) setMessage('identityRepair.savedRefreshFailed'); }
    } catch (error) {
      if (!alive.current) return;
      // A failed/uncertain commit is never retried with the same version or choices.
      setPreview(null); setKeepers({}); setAcknowledged(false);
      setMessage(identityRepairErrorKey(error instanceof IpcError ? error.code : undefined));
    } finally { active.current = false; if (alive.current) setBusy(false); }
  };
  const retryRefresh = async () => {
    if (!done || active.current) return;
    active.current = true; setBusy(true);
    try {
      await refreshConfig({ fresh: true, reason: 'identity-repair-retry' });
      if (alive.current) setMessage(savedMessage.current);
    } catch { if (alive.current) setMessage('identityRepair.savedRefreshFailed'); }
    finally { active.current = false; if (alive.current) setBusy(false); }
  };
  return { retryRefresh, preview, keepers, setKeepers, acknowledged, setAcknowledged, busy, message, done, load, save, canSave: Boolean(request) && acknowledged && !busy && !done };
}
