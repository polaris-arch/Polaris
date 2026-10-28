import type { SpeedTestResultIdentity, SpeedTestFailure, SpeedTestScheduleStatus } from './speed-test';

export type MeasurementState = 'success' | 'failed' | 'unmeasured' | 'expired' | 'paused';
export interface MeasurementRecord {
  state: 'success' | 'failed' | 'expired';
  reason: string | null;
  latency: number | null;
  failure: SpeedTestFailure | null;
  identity: Omit<SpeedTestResultIdentity, 'nodeFingerprint' | 'path' | 'origin' | 'metric'> & { nodeFingerprintDigest: string | null; connectionDigest: string | null; path: SpeedTestResultIdentity['path'] | 'nativeHttp'; origin: SpeedTestResultIdentity['origin'] | 'nativeAuto'; metric: 'warm_ttfb_v1' | 'native_http_v1' };
  disconnected: boolean;
  bindingMismatch: boolean;
  expiresAt: number;
}
export interface MeasurementRows {
  candidate?: MeasurementRecord;
  nativeHttp?: MeasurementRecord;
  system?: MeasurementRecord;
  selected?: MeasurementRecord;
  notIncluded?: string;
}
export interface MeasurementSnapshot {
  schema: 'polaris-measurement-display-v1';
  process: string;
  revision: string;
  configVersion: string;
  frontendConfigDigest: string;
  emissionDigest: string | null;
  consistent: boolean;
  generation: number | null;
  startTime: number | null;
  networkEpoch: number | null;
  foregroundEpoch: number;
  now: number;
  schedule: SpeedTestScheduleStatus;
  rows: Record<string, MeasurementRows>;
}
export interface MeasurementProjection {
  state: MeasurementState;
  latency?: number;
  measuredAt?: number;
  notIncluded?: string;
  disconnected?: boolean;
  purpose?: 'candidate' | 'system' | 'selected' | 'nativeHttp';
  reason?: string | null;
  bindingMismatch?: boolean;
}
