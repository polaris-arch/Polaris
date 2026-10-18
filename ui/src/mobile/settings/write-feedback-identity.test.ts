import { expect, it } from 'vitest';
import type { TFunction } from 'i18next';
import { createCommit, type WriteErrors } from './write-feedback';
for (const outcome of ['permission-refused','permission-rejected','installer-refused','installer-rejected','success']) {
  it(`stale ${outcome} cannot publish or erase feedback belonging to the replacement package`, async () => {
    let errors: WriteErrors = { 'app-update': 'new package failure' };
    let active = true;
    let resolve!: () => void; let reject!: (reason: Error) => void;
    const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
    const commit = createCommit(update => { errors = update(errors); }, ((key: string) => key) as TFunction);
    commit('app-update', promise, error => (error as Error).message, () => active);
    active = false;
    if (outcome === 'success') resolve(); else reject(new Error(outcome));
    await promise.catch(() => {}); await Promise.resolve();
    expect(errors).toEqual({ 'app-update': 'new package failure' });
  });
}
it('current success and current failure retain ordinary feedback semantics', async () => {
  let errors: WriteErrors = {};
  const commit = createCommit(update => { errors = update(errors); }, ((key: string) => key) as TFunction);
  commit('app-update', Promise.reject(new Error('refused')), error => (error as Error).message, () => true);
  await Promise.resolve(); expect(errors['app-update']).toBe('refused');
  commit('app-update', Promise.resolve(), undefined, () => true);
  await Promise.resolve(); expect(errors['app-update']).toBeUndefined();
});
