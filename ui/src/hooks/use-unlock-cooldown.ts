import { useEffect, useState } from 'react';

/** Service checks retain the backend's 15s force limit; latency has no cooldown. */
export function unlockCooldownRemaining(lastRunAt: number | null, now = Date.now()): number {
  if (lastRunAt === null || now < lastRunAt) return 0;
  return Math.max(0, 15_000 - (now - lastRunAt));
}

export function useUnlockCooldown(lastRunAt: number | null): boolean {
  const [cooling, setCooling] = useState(() => unlockCooldownRemaining(lastRunAt) > 0);
  useEffect(() => {
    const remaining = unlockCooldownRemaining(lastRunAt);
    setCooling(remaining > 0);
    if (remaining === 0) return;
    const timer = setTimeout(() => setCooling(false), remaining);
    return () => clearTimeout(timer);
  }, [lastRunAt]);
  return cooling;
}
