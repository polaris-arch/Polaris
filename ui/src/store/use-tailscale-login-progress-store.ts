import { create } from 'zustand';
import { acceptLoginProgress, type TailscaleLoginProgress } from '@/domain/tailscale-login-progress';

interface LoginProgressState {
  attempts: Record<string, TailscaleLoginProgress>;
  begin: (serverId: string, attemptId: string) => void;
  apply: (progress: TailscaleLoginProgress) => boolean;
}

export const useTailscaleLoginProgressStore = create<LoginProgressState>((set, get) => ({
  attempts: {},
  begin: (serverId, attemptId) => set((state) => ({
    attempts: { ...state.attempts, [serverId]: { serverId, attemptId, phase: 'starting' } },
  })),
  apply: (progress) => {
    if (!acceptLoginProgress(get().attempts[progress.serverId], progress)) return false;
    set((state) => ({ attempts: { ...state.attempts, [progress.serverId]: progress } }));
    return true;
  },
}));
