import { useEffect, useState } from 'react';
import { subscriptionApi } from '@/ipc/api/subscriptions';
import { useAppStore } from '@/store/app-store';
import { isSubscriptionUserAgentInfo, type SubscriptionUserAgentInfo } from '@/contracts/subscription-user-agent';
import { useInputRequest } from './use-input-request';

type ReadState =
  | { status: 'loading' }
  | { status: 'failed' }
  | { status: 'ready'; info: SubscriptionUserAgentInfo };
export type SubscriptionUserAgentModel = ReadState & { refresh: () => void };

/** Backend-owned preview; input/config revisions invalidate old replies and old visible values. */
export function useSubscriptionUserAgent(userAgent: string): SubscriptionUserAgentModel {
  // A committed config change invalidates the read; never send a renderer's guessed global UA.
  const globalUserAgent = useAppStore(state => state.config?.subscriptionUserAgent);
  const [revision, setRevision] = useState(0);
  const key = JSON.stringify([userAgent, globalUserAgent, revision]);
  const begin = useInputRequest(key);
  const [state, setState] = useState<{ key: string; read: ReadState; owns: () => boolean }>();
  useEffect(() => {
    const owns = begin();
    setState({ key, read: { status: 'loading' }, owns });
    void subscriptionApi.userAgentInfo(userAgent).then(info => {
      if (!isSubscriptionUserAgentInfo(info)) throw new TypeError('Invalid subscription User-Agent identity');
      if (owns()) setState({ key, read: { status: 'ready', info }, owns });
    }).catch(() => {
      if (owns()) setState({ key, read: { status: 'failed' }, owns });
    });
  }, [begin, key, userAgent]);
  // Clear synchronously on any input revision, before the effect or a new response can run.
  const read: ReadState = state?.key === key && state.owns() ? state.read : { status: 'loading' };
  return { ...read, refresh: () => setRevision(value => value + 1) };
}
