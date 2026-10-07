import { useState } from 'react';
import type { SpeedTestScheduleLimits } from '@/contracts/speed-test';
import type { SubscriptionConfig } from '@/contracts/types';
import {
  globalPeriodicSpeedTestEnabled,
  parseIntervalMinutes,
  showsGlobalOffHint,
} from '@/domain/periodic-speed-test';
import { useAppStore, useEffectiveConfig } from '@/store/app-store';
import { useSpeedTestSchedule } from './use-speed-test-schedule';

export interface PeriodicSpeedTestFields {
  /** 取值范围与缺省值；后端还没答时为 `undefined`。 */
  limits: SpeedTestScheduleLimits | undefined;
  /** 本订阅的开关显示态（没动过时取后端给的缺省）。 */
  checked: boolean;
  interval: string;
  intervalInvalid: boolean;
  /** 全局关着而本订阅开着：订阅暂不会按周期测速。 */
  showGlobalOff: boolean;
  enablingGlobal: boolean;
  toggle: () => void;
  setInterval: (next: string) => void;
  /** 校验并给出要写回订阅的两个字段；周期填得不对 → 标红并返回 `null`。 */
  commit: () => Pick<SubscriptionConfig, 'periodicSpeedTest' | 'speedTestIntervalMinutes'> | null;
  /** 就地打开全局总开关并保存，不要求离开表单去设置页。 */
  enableGlobal: () => Promise<void>;
}

/**
 * 订阅表单里「周期测速」那两个字段的状态，桌面弹窗与移动端表单共用。
 *
 * 开关没被动过时不写回 `periodicSpeedTest`：缺席即跟随后端的缺省，改缺省只动后端那一处。
 */
export function usePeriodicSpeedTestFields(base: SubscriptionConfig | undefined): PeriodicSpeedTestFields {
  const limits = useSpeedTestSchedule()?.limits;
  const globalConfigured = useEffectiveConfig((c) => c?.periodicSpeedTestEnabled);
  const saveConfig = useAppStore((s) => s.saveConfig);
  const [enabled, setEnabled] = useState(base?.periodicSpeedTest);
  const [interval, setIntervalText] = useState(
    base?.speedTestIntervalMinutes === undefined ? '' : String(base.speedTestIntervalMinutes),
  );
  const [intervalInvalid, setIntervalInvalid] = useState(false);
  const [enablingGlobal, setEnablingGlobal] = useState(false);

  const checked = enabled ?? limits?.subscriptionDefault ?? false;
  return {
    limits,
    checked,
    interval,
    intervalInvalid,
    showGlobalOff: showsGlobalOffHint(globalPeriodicSpeedTestEnabled(globalConfigured, limits), checked),
    enablingGlobal,
    toggle: () => setEnabled(!checked),
    setInterval: (next) => {
      setIntervalText(next);
      setIntervalInvalid(false);
    },
    commit: () => {
      const minutes = parseIntervalMinutes(interval, limits);
      if (minutes === null) {
        setIntervalInvalid(true);
        return null;
      }
      return { periodicSpeedTest: enabled, speedTestIntervalMinutes: minutes };
    },
    enableGlobal: async () => {
      setEnablingGlobal(true);
      try {
        await saveConfig({ periodicSpeedTestEnabled: true });
      } finally {
        setEnablingGlobal(false);
      }
    },
  };
}
