import type { NativeAutoStatus } from './auto-select';
export function nativeAutoFixture(patch: Partial<NativeAutoStatus> = {}): NativeAutoStatus {
  return { featureEnabled: true, generation: 1, actualServerId: null, nativeAutoGroups: [], nativeMeasurements: [], ...patch };
}
