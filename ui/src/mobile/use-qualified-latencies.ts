import { useMemo } from 'react';
import { useLatencyStore, EMPTY_LATENCY_MAP } from '@/store/use-latency-store';
import { useAppStore } from '@/store/app-store';
import { configCanonicalText } from '@/lib/staged-config';
import type { ServerConfig } from '@/contracts/types';

/** Saved ledger values cannot rank a staged edit that happens to reuse the same ID. */
export function useQualifiedLatencies(displayed: readonly ServerConfig[], needed = true): Readonly<Record<string, number | null>> {
  const raw = useLatencyStore(state => needed ? state.latencyMap : EMPTY_LATENCY_MAP);
  const saved = useAppStore(state => state.config?.servers);
  return useMemo(() => {
    if (!needed || !saved) return EMPTY_LATENCY_MAP;
    const versions = new Map(saved.map(server => [server.id, configCanonicalText(server)]));
    return Object.fromEntries(displayed.filter(server => versions.get(server.id) === configCanonicalText(server)
      && Object.hasOwn(raw, server.id)).map(server => [server.id, raw[server.id]]));
  }, [raw, saved, displayed, needed]);
}
