import { useEffect, useState } from 'react';
import type { PendingApplyResult } from '@/contracts/types';
import { api } from '@/ipc';
import { configBaseVersion } from '@/lib/staged-config';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';

export interface ImpactPrompt {
  token: string;
  coreImpacts: NonNullable<PendingApplyResult['coreImpacts']>;
  hostEffects: NonNullable<PendingApplyResult['hostEffectPreviews']>;
  generation: number;
  draft: ReturnType<typeof useStagedConfigStore.getState>['entries'];
  version: string | null;
}

function currentVersion(): string | null {
  const config = useAppStore.getState().config;
  return config === null ? null : configBaseVersion(config);
}

export function useImpactConfirmation() {
  const [prompt, setPrompt] = useState<ImpactPrompt | null>(null);
  const entries = useStagedConfigStore((state) => state.entries);
  const config = useAppStore((state) => state.config);
  const status = useAppStore((state) => state.proxyStatus);

  const dismiss = () => {
    const token = prompt?.token;
    setPrompt(null);
    if (token) void api.proxy.cancelRuntimeImpact(token).catch(() => null);
  };
  useEffect(() => {
    if (prompt && (entries !== prompt.draft || currentVersion() !== prompt.version
      || !status?.running || status.mainGeneration !== prompt.generation)) {
      setPrompt(null);
      void api.proxy.cancelRuntimeImpact(prompt.token).catch(() => null);
    }
  }, [prompt, entries, config, status]);

  const offer = (receipt: Pick<PendingApplyResult, 'impactConfirmation' | 'runtimeGeneration' | 'coreImpacts' | 'hostEffectPreviews'>): boolean => {
    const generation = receipt.runtimeGeneration;
    const statusNow = useAppStore.getState().proxyStatus;
    if (!receipt.impactConfirmation || !Number.isSafeInteger(generation) || (generation ?? 0) <= 0
      || !statusNow?.running || statusNow.mainGeneration !== generation) {
      if (receipt.impactConfirmation) void api.proxy.cancelRuntimeImpact(receipt.impactConfirmation).catch(() => null);
      return false;
    }
    setPrompt({ token: receipt.impactConfirmation, coreImpacts: receipt.coreImpacts ?? [],
      hostEffects: receipt.hostEffectPreviews ?? [], generation: generation as number,
      draft: useStagedConfigStore.getState().entries, version: currentVersion() });
    return true;
  };
  return { prompt, offer, dismiss, hide: () => setPrompt(null) };
}
