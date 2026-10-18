import type { SpeedTestResultIdentity, SpeedTestFailure, SpeedTestScheduleStatus } from './speed-test';

export type MeasurementState = 'success' | 'failed' | 'unmeasured' | 'expired' | 'paused';
export interface MeasurementRecord {
  state: 'success' | 'failed' | 'expired';
  reason: string | null;
  latency: number | null;
  failure: SpeedTestFailure | null;
  identity: Omit<SpeedTestResultIdentity, 'nodeFingerprint'> & { nodeFingerprintDigest: string | null; connectionDigest: string | null };
  disconnected: boolean;
  bindingMismatch: boolean;
  expiresAt: number;
}
export interface MeasurementRows {
  candidate?: MeasurementRecord;
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
  purpose?: 'candidate' | 'system' | 'selected';
  reason?: string | null;
  bindingMismatch?: boolean;
}
