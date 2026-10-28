import type { HelperActionErrorCode } from '@/ipc/api-client';

/** Renderer-session feedback only. Never persisted, never proof of helper custody,
 * and never permission to install/stop. Confirmation remains owned by the mounted page. */
type Snapshot = Readonly<{ busy: boolean; failure: HelperActionErrorCode | null }>;
let snapshot: Snapshot = { busy: false, failure: null };
let sequence = 0;
let active: number | null = null;
const listeners = new Set<(value: Snapshot) => void>();
function publish(value: Snapshot) {
  snapshot = value;
  for (const listener of listeners) listener(value);
}
export function helperInstallSnapshot(): Snapshot { return snapshot; }
export function subscribeHelperInstall(listener: (value: Snapshot) => void): () => void {
  listeners.add(listener);
  listener(snapshot);
  return () => { listeners.delete(listener); };
}
/** Shared across real page unmount/remount, so a second page cannot duplicate an in-flight request. */
export function beginHelperInstall(): number | null {
  if (active !== null) return null;
  const request = ++sequence;
  active = request;
  publish({ busy: true, failure: null });
  return request;
}
export function completeHelperInstall(request: number, failure: HelperActionErrorCode | null): void {
  if (active !== request) return;
  active = null;
  publish({ busy: false, failure });
}
/** Successful uninstall can clear old presentation; status reads cannot erase a failed action. */
export function clearHelperInstallFeedback(): void {
  if (active === null) publish({ busy: false, failure: null });
}
