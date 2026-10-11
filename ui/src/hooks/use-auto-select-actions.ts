import type { AutoSelectT } from '@/lib/auto-select-actions';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { UserConfig } from '@/contracts/types';
import { configReadGeneration } from '@/lib/config-read-generation';
import { autoSelectActionFeedback, autoSelectContext, autoSelectErrorFeedback, runAutoSelectAction, type AutoSelectAction, type AutoSelectActionFeedback } from '@/lib/auto-select-actions';

export function useAutoSelectActions(config: UserConfig | null, accept: (config: UserConfig) => void | Promise<void>, t: AutoSelectT, getCurrentConfig?: () => UserConfig | null) {
  const context = autoSelectContext(config);
  const current = useRef(context); current.current = context;
  const currentContext = () => getCurrentConfig ? autoSelectContext(getCurrentConfig()) : current.current;
  const lifetime = useRef({ alive: true, generation: 0 });
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ context: string; value: AutoSelectActionFeedback | null } | null>(null);
  useEffect(() => { lifetime.current.alive = true; return () => { lifetime.current.alive = false; lifetime.current.generation++; }; }, []);
  const run = useCallback((action: AutoSelectAction) => {
    if (!config || pending.current || !lifetime.current.alive) return;
    pending.current = true; setBusy(true); setFeedback(null);
    const generation = lifetime.current.generation;
    const alive = () => lifetime.current.alive && lifetime.current.generation === generation;
    const origin = context;
    const queuedGeneration = configReadGeneration.current();
    void runAutoSelectAction(action, accept, () => alive() && configReadGeneration.isCurrent(queuedGeneration)).then(result => {
      if (!result || !alive()) return;
      const saved = autoSelectContext(result.config);
      if (currentContext() !== origin && currentContext() !== saved) return;
      setFeedback({ context: saved, value: autoSelectActionFeedback(result, t) });
    }).catch(error => {
      // Rejection may follow a partial saved intent. A matching authoritative readback can
      // receive the error, but a late failure must not attach to a newer manual selection.
      if (alive() && (currentContext() === origin || currentContext() === error?.savedContext))
        setFeedback({ context: currentContext(), value: autoSelectErrorFeedback(error, t) });
    }).finally(() => { pending.current = false; if (alive()) setBusy(false); });
  }, [accept, config, context, t, getCurrentConfig]);
  return { busy, run, feedback: feedback?.context === context ? feedback.value : null };
}
