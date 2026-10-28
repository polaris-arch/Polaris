import { useEffect, type ReactNode } from 'react';
import { api } from './src/ipc';
import { startPrivacyModeSync } from './src/lib/privacy-mode-sync';
import { useAppStore } from './src/store/app-store';

/** Standalone form fixtures omit App, but still need its fail-closed privacy lifecycle. */
export function HarnessPrivacyRoot({ children }: { children: ReactNode }) {
  useEffect(() => startPrivacyModeSync(api.config,
    (locked) => useAppStore.getState().setPrivacyMode(locked)), []);
  return children;
}
