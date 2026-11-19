import { useStagedConfigStore } from './staged-config-store';

/** Native admission owns the start/stop boundary for ordinary edits. */
export function useStagingActive(): boolean {
  return useStagedConfigStore((s) => s.enabled);
}

/** Async editors re-read the same feature switch immediately before dispatch. */
export function getStagingActive(): boolean {
  return useStagedConfigStore.getState().enabled;
}
