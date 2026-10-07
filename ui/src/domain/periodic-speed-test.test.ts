import { describe, expect, it } from 'vitest';
import type { SpeedTestScheduleLimits } from '@/contracts/speed-test';
import {
  globalPeriodicSpeedTestEnabled,
  parseConcurrency,
  parseIntervalMinutes,
  showsGlobalOffHint,
} from './periodic-speed-test';

const LIMITS: SpeedTestScheduleLimits = {
  concurrencyMin: 4,
  concurrencyMax: 32,
  intervalMinutesMin: 5,
  intervalMinutesMax: 360,
  intervalMinutesDefault: 30,
  enabledDefault: true,
  subscriptionDefault: false,
  meteredPolicyDefault: 'reduced',
};

describe('周期测速设置项的输入解析', () => {
  it('周期：留空存成缺席，范围内的整数保留，其余判非法（不钳到边界）', () => {
    expect(parseIntervalMinutes('', LIMITS)).toBeUndefined();
    expect(parseIntervalMinutes('  ', LIMITS)).toBeUndefined();
    expect(parseIntervalMinutes('5', LIMITS)).toBe(5);
    expect(parseIntervalMinutes(' 360 ', LIMITS)).toBe(360);
    for (const bad of ['4', '361', '30.5', '-5', '1e2', 'abc']) {
      expect(parseIntervalMinutes(bad, LIMITS), bad).toBeNull();
    }
  });

  it('并发：上限取后端给的本设备上限', () => {
    expect(parseConcurrency('', LIMITS)).toBeUndefined();
    expect(parseConcurrency('4', LIMITS)).toBe(4);
    expect(parseConcurrency('32', LIMITS)).toBe(32);
    expect(parseConcurrency('33', LIMITS)).toBeNull();
    expect(parseConcurrency('48', { ...LIMITS, concurrencyMax: 64 })).toBe(48);
    expect(parseConcurrency('3', LIMITS)).toBeNull();
  });

  it('范围还没从后端拿到时只挡非整数（越界值由后端清洗兜底）', () => {
    expect(parseConcurrency('48', undefined)).toBe(48);
    expect(parseIntervalMinutes('1000', undefined)).toBe(1000);
    expect(parseIntervalMinutes('', undefined)).toBeUndefined();
    expect(parseConcurrency('8.5', undefined)).toBeNull();
    expect(parseIntervalMinutes('abc', undefined)).toBeNull();
  });
});

describe('全局开关与每订阅开关的四种组合', () => {
  it('只有「全局关 × 订阅开」显示提示', () => {
    expect(showsGlobalOffHint(true, true)).toBe(false);
    expect(showsGlobalOffHint(true, false)).toBe(false);
    expect(showsGlobalOffHint(false, false)).toBe(false);
    expect(showsGlobalOffHint(false, true)).toBe(true);
  });

  it('全局开关缺席时取后端给的缺省；后端还没答时按开', () => {
    expect(globalPeriodicSpeedTestEnabled(undefined, LIMITS)).toBe(true);
    expect(globalPeriodicSpeedTestEnabled(undefined, { ...LIMITS, enabledDefault: false })).toBe(false);
    expect(globalPeriodicSpeedTestEnabled(false, LIMITS)).toBe(false);
    expect(globalPeriodicSpeedTestEnabled(true, { ...LIMITS, enabledDefault: false })).toBe(true);
    expect(globalPeriodicSpeedTestEnabled(undefined, undefined)).toBe(true);
  });
});
