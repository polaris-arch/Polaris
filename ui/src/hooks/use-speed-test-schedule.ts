import { useEffect, useState } from 'react';
import type { SpeedTestScheduleStatus } from '@/contracts/speed-test';
import { serverApi } from '@/ipc/api-client';

/**
 * 周期测速的计划状态：挂载时拉一次，此后跟随后端的状态事件。还没拉到（或拉取失败）时为 `null`
 * —— 调用方据此不显示取值范围，而不是自己编一个。
 */
export function useSpeedTestSchedule(): SpeedTestScheduleStatus | null {
  const [status, setStatus] = useState<SpeedTestScheduleStatus | null>(null);
  useEffect(() => {
    let live = true;
    serverApi
      .speedTestScheduleStatus()
      .then((next) => {
        if (live) setStatus(next);
      })
      .catch((error) => console.error('[speed-test-schedule] status failed:', error));
    const off = serverApi.onSpeedTestSchedule((next) => {
      if (live) setStatus(next);
    });
    return () => {
      live = false;
      off();
    };
  }, []);
  return status;
}
