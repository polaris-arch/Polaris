import { describe, expect, it } from 'vitest';
import { createLatestReportLoader, type ReportLoadState } from './latest-report-loader';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

describe('latest report refresh', () => {
  it('an older success cannot overwrite a newer refresh', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const jobs = [first, second];
    const states: ReportLoadState<string>[] = [];
    const loader = createLatestReportLoader(() => jobs.shift()!.promise, (s) => states.push(s));
    loader.refresh();
    loader.refresh();
    first.resolve('old disk snapshot');
    await first.promise;
    await Promise.resolve();
    expect(states[states.length - 1]).toEqual({ report: null, loading: true, error: false });
    second.resolve('current disk snapshot');
    await second.promise;
    await Promise.resolve();
    expect(states[states.length - 1]).toEqual({ report: 'current disk snapshot', loading: false, error: false });
  });

  it('an older failure cannot clear a newer successful report', async () => {
    const older = deferred<string>();
    const newer = deferred<string>();
    const jobs = [older, newer];
    const states: ReportLoadState<string>[] = [];
    const loader = createLatestReportLoader(() => jobs.shift()!.promise, (s) => states.push(s));
    loader.refresh();
    loader.refresh();
    newer.resolve('current report');
    await newer.promise;
    await Promise.resolve();
    older.reject(new Error('obsolete failure'));
    await older.promise.catch(() => undefined);
    await Promise.resolve();
    expect(states[states.length - 1]).toEqual({
      report: 'current report', loading: false, error: false,
    });
    expect(states).toHaveLength(3); // two loading publications and only the new success
  });

  it('a failed refresh clears the previous success; cleanup discards an in-flight response', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const third = deferred<string>();
    const jobs = [first, second, third];
    const states: ReportLoadState<string>[] = [];
    const loader = createLatestReportLoader(() => jobs.shift()!.promise, (s) => states.push(s));
    loader.refresh();
    first.resolve('old');
    await first.promise;
    await Promise.resolve();
    loader.refresh();
    expect(states[states.length - 1]).toEqual({ report: null, loading: true, error: false });
    second.reject(new Error('IPC failed'));
    await second.promise.catch(() => undefined);
    await Promise.resolve();
    expect(states[states.length - 1]).toEqual({ report: null, loading: false, error: true });
    loader.refresh();
    loader.invalidate();
    third.resolve('after unmount');
    await third.promise;
    await Promise.resolve();
    expect(states[states.length - 1]).toEqual({ report: null, loading: true, error: false });
  });
});
