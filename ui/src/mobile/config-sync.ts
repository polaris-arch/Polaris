import { useEffect } from 'react';
import { acquireConfigSync } from '@/store/app-store';

/** Mobile roots and independently mounted settings share one ready-registered feed. */
export function startConfigSync(): () => void {
  return acquireConfigSync({ scope: 'main' });
}

export function useMobileConfigSync(): void {
  useEffect(startConfigSync, []);
}
