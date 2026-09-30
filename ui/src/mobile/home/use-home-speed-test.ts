import { runMobileSpeedTest, useMobileSpeedTestStore, mobileSpeedTestBusy } from '../use-mobile-speed-test';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ServerConfig } from '@/contracts/types';
import { getEffectiveConfig, useAppStore } from '@/store/app-store';
import { isSentinelSelection } from '@/domain/direct-selection';
import { stagedOnlyIds } from '@/lib/staged-config';
import { speedTestBlockReason } from '@/components/screens/nodes/nodes-logic';
import {
  notInPoolMessage,
  speedTestBlockedMessage,
  speedTestErrorMessage,
} from '@/components/screens/shared/speedtest-feedback';
import { batchSpeedTestFeedback, planAllHomeSpeedTest } from './home-speedtest';
import type { LatencyCheckVM } from './view-model';

/** Manual current/all actions; automatic current RTT is scheduled by Rust after IP probing. */
export function useHomeSpeedTest(args: {
  servers: readonly ServerConfig[];
  diskServers: readonly ServerConfig[];
  selectedId: string | null;
  running: boolean;
  routing: string;
}): { view: LatencyCheckVM; runCurrent: () => Promise<void> } {
  const { t } = useTranslation();
  const globalBusy = useMobileSpeedTestStore(mobileSpeedTestBusy);
  const [busyKind, setBusyKind] = useState<'current' | 'all' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<LatencyCheckVM['feedback']>(null);
  const inFlight = useRef<'current' | 'all' | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const selected = args.servers.find((s) => s.id === args.selectedId);
  const direct = args.routing === 'direct' || isSentinelSelection(args.selectedId);
  const stagedOnly = stagedOnlyIds(args.servers, args.diskServers);
  const blocked = !args.running
    ? t('mobileHome.connectToTest')
    : direct
      ? t('nodes.speedTestSentinelExit')
      : selected
        ? (() => {
            const reason = speedTestBlockReason(
              selected,
              { mainCorePool: args.running },
              stagedOnly.has(selected.id),
            );
            return reason ? speedTestBlockedMessage(reason, t) : null;
          })()
        : t('nodes.speedTestNoActiveExit');
  const allIds = useMemo(
    () => planAllHomeSpeedTest(args.servers, args.diskServers, args.running),
    [args.servers, args.diskServers, args.running],
  );
  const currentKey = useCallback(() => {
    const state = useAppStore.getState();
    return `${state.proxyStatus?.running === true}|${state.proxyStatus?.startTime ?? 0}|${state.selectedServerId ?? ''}|${getEffectiveConfig()?.proxyMode ?? 'smart'}`;
  }, []);

  const runCurrent = useCallback(async (): Promise<void> => {
    if (blocked || !selected) {
      setError(blocked);
      return;
    }
    const requestKey = currentKey();
    if (inFlight.current !== null) return;
    inFlight.current = 'current';
    setBusyKind('current');
    setError(null);
    setFeedback(null);
    try {
      const result = await runMobileSpeedTest([selected.id], 'current', () => currentKey() === requestKey);
      if (!result || currentKey() !== requestKey) return;
      const skipped = notInPoolMessage(result, t);
      if (skipped && mounted.current) setError(skipped);
    } catch (cause) {
      if (currentKey() === requestKey && mounted.current) setError(speedTestErrorMessage(cause, t));
    } finally {
      inFlight.current = null;
      if (mounted.current) setBusyKind(null);
    }
  }, [blocked, currentKey, selected, t]);

  const runAll = useCallback(async (): Promise<void> => {
    if (inFlight.current !== null) return;
    if (allIds.length === 0) {
      setFeedback({ tone: 'info', text: t('nodes.noTestableNodes') });
      return;
    }
    inFlight.current = 'all';
    setBusyKind('all');
    setError(null);
    setFeedback(null);
    try {
      const result = await runMobileSpeedTest(allIds, 'all');
      if (!result) return;
      // A batch belongs to all eligible server IDs, not to the selected exit or route.
      if (mounted.current) setFeedback(batchSpeedTestFeedback(result, allIds.length, t));
    } catch (cause) {
      if (mounted.current) setError(speedTestErrorMessage(cause, t));
    } finally {
      inFlight.current = null;
      if (mounted.current) setBusyKind(null);
    }
  }, [allIds, t]);

  return {
    runCurrent,
    view: {
      busyKind,
      busy: globalBusy,
      error,
      feedback,
      blocked,
      blockedStatus: !args.running ? t('home.unlockStatus.idle') : t('mobileHome.notApplicable'),
      allUnavailable: allIds.length === 0 ? t('nodes.noTestableNodes') : null,
      onRunCurrent: () => void runCurrent(),
      onRunAll: () => void runAll(),
    },
  };
}
