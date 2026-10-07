/**
 * 周期测速设置项的纯逻辑：输入框的解析，以及「订阅里开着、全局却关着」这条提示的判据。
 *
 * 取值范围与缺省值都由后端给出（计划状态的 `limits`，见 `contracts/speed-test.ts`）：并发上限随
 * 平台不同，这里不写死任何数字。
 */

import type { SpeedTestScheduleLimits } from '@/contracts/speed-test';

/** 解析一个「可留空的整数」输入：留空 → `undefined`；范围内的整数 → 该数；其余 → `null`（非法）。 */
export function parseOptionalInteger(raw: string, min: number, max: number): number | undefined | null {
  const text = raw.trim();
  if (text === '') return undefined;
  if (!/^\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= min && value <= max ? value : null;
}

/**
 * 测速周期（分钟）。留空即缺省周期，存成缺席。范围还没从后端拿到时只挡非整数：越界值由后端
 * 清洗兜底（删除而不是钳到边界）。
 */
export function parseIntervalMinutes(
  raw: string,
  limits: SpeedTestScheduleLimits | undefined,
): number | undefined | null {
  return parseOptionalInteger(
    raw,
    limits?.intervalMinutesMin ?? 1,
    limits?.intervalMinutesMax ?? Number.MAX_SAFE_INTEGER,
  );
}

/** 测速并发。留空即自动，存成缺席。范围未知时的处置同上。 */
export function parseConcurrency(
  raw: string,
  limits: SpeedTestScheduleLimits | undefined,
): number | undefined | null {
  return parseOptionalInteger(
    raw,
    limits?.concurrencyMin ?? 1,
    limits?.concurrencyMax ?? Number.MAX_SAFE_INTEGER,
  );
}

/** 全局总开关的生效值（缺席取后端给的缺省；后端还没答时按缺省为开，不凭空亮出提示）。 */
export function globalPeriodicSpeedTestEnabled(
  configured: boolean | undefined,
  limits: SpeedTestScheduleLimits | undefined,
): boolean {
  return configured ?? limits?.enabledDefault ?? true;
}

/**
 * 是否显示「全局周期测速已关闭，此订阅暂不会按周期测速」：全局关着、而本订阅的开关开着。
 * 订阅里开着却不测，要让人当场看得出原因。
 */
export function showsGlobalOffHint(globalEnabled: boolean, subscriptionEnabled: boolean): boolean {
  return !globalEnabled && subscriptionEnabled;
}
