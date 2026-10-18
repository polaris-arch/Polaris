import { describe, expect, it } from 'vitest';
import { createCommittedChoice } from './committed-appearance';
function deferred() {
  let resolve!: () => void; let reject!: (reason: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
for (const setting of ['theme', 'language']) describe(setting, () => {
  it('a newer failure does not suppress an older durable success', async () => {
    const commit = createCommittedChoice(); const a = deferred(); const b = deferred();
    let live = 'original';
    const first = commit(a.promise, () => { live = 'A'; });
    const second = commit(b.promise, () => { live = 'B'; }).catch(() => {});
    a.resolve(); await first; expect(live).toBe('A');
    b.reject(new Error('failed')); await second; expect(live).toBe('A');
  });
  it('navigation does not discard a success and remounted instances share the same ordering', async () => {
    const commit = createCommittedChoice(); const a = deferred(); const b = deferred();
    let live = 'original';
    const first = commit(a.promise, () => { live = 'A'; });
    // No component-lifetime token: the operation remains owned after the originating page exits.
    a.resolve(); await first; expect(live).toBe('A');
    const second = commit(b.promise, () => { live = 'B'; });
    b.resolve(); await second; expect(live).toBe('B');
  });
  it('a late older success cannot overwrite a newer confirmed choice across instances', async () => {
    const commit = createCommittedChoice(); const a = deferred(); const b = deferred();
    let live = 'original';
    const first = commit(a.promise, () => { live = 'A'; });
    const second = commit(b.promise, () => { live = 'B'; });
    b.resolve(); await second; a.resolve(); await first; expect(live).toBe('B');
  });
});
