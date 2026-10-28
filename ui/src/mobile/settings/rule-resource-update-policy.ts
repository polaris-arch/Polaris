import type { UserConfig } from '@/contracts/types';
import {
  ruleResourceAutoStatus,
  ruleResourceIntervalSelectValue,
} from '@/components/screens/settings/settings-logic';

/** 设置页和资源页展示同一个实际策略；时间戳仍只代表上次成功下载。 */
export function ruleResourceUpdatePolicyText(
  config: UserConfig,
  t: (key: string, vars?: Record<string, unknown>) => string,
): string {
  switch (ruleResourceAutoStatus(config)) {
    case 'off': return t('mobileSettings.update.ruleResourceOff');
    case 'manual': return t('mobileSettings.update.ruleResourceManual');
    case 'active': return t('mobileSettings.update.ruleResourceActive', {
      hours: Number(ruleResourceIntervalSelectValue(config)),
    });
  }
}
