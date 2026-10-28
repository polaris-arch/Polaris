import type { PendingApplyResult } from '@/contracts/types';

/** Shared decision data; platform-specific shells own presentation and dismissal. */
export interface RestartPrompt {
  token: string;
  reasons: NonNullable<PendingApplyResult['reasons']>;
}

/** Render only trusted translations; never show arbitrary kernel messages or path values. */
export function restartReasonKeys(reasons: PendingApplyResult['reasons']): string[] {
  const keys = new Set<string>();
  for (const reason of (reasons ?? []).slice(0, 32)) {
    if (reason.code === 'unsupportedCore') {
      keys.add('home.restartApplyUnsupportedCore');
      continue;
    }
    if (reason.code !== 'structuralChange') continue;
    const path = reason.path;
    if (/^(tun|inbounds\.tun)(\.|$)/.test(path)) keys.add('home.restartApplyTun');
    else if (/^(mixedPort|listenPort|inbounds\.listen_port)$/.test(path)) keys.add('home.restartApplyListener');
    else if (/^(networkNamespace|network_namespace|netns)$/.test(path)) keys.add('home.restartApplyNamespace');
    else if (/^(services|service)(\.|$)/.test(path)) keys.add('home.restartApplyServices');
    else if (/^(cachePath|cacheFile\.path|experimental\.cache_file\.path)$/.test(path)) keys.add('home.restartApplyCache');
    else keys.add('home.restartApplyStructural');
  }
  return keys.size > 0 ? [...keys] : ['home.restartApplyStructural'];
}
