import { beforeEach, expect, it } from 'vitest';
import { beginHelperInstall, clearHelperInstallFeedback, completeHelperInstall, helperInstallSnapshot, subscribeHelperInstall } from './helper-install-session';

beforeEach(() => clearHelperInstallFeedback());
it('retains only bounded feedback after subscriber unmount and late completion', () => {
  const seen: unknown[] = [];
  const off = subscribeHelperInstall(value => seen.push(value));
  const request = beginHelperInstall()!;
  expect(beginHelperInstall()).toBeNull();
  off();
  completeHelperInstall(request, 'upgradeDeferred');
  expect(helperInstallSnapshot()).toEqual({ busy: false, failure: 'upgradeDeferred' });
  const restored: unknown[] = [];
  const release = subscribeHelperInstall(value => restored.push(value));
  expect(restored).toEqual([{ busy: false, failure: 'upgradeDeferred' }]);
  expect(seen).toEqual([{ busy: false, failure: null }, { busy: true, failure: null }]);
  release();
});
it('cannot clear or duplicate a live request, and stale completion cannot replace a newer outcome', () => {
  const first = beginHelperInstall()!;
  clearHelperInstallFeedback();
  expect(helperInstallSnapshot().busy).toBe(true);
  completeHelperInstall(first, 'upgradeDeferred');
  const second = beginHelperInstall()!;
  completeHelperInstall(first, null);
  expect(helperInstallSnapshot().busy).toBe(true);
  expect(beginHelperInstall()).toBeNull();
  completeHelperInstall(second, 'installerFailed');
  completeHelperInstall(first, 'upgradeDeferred');
  expect(helperInstallSnapshot()).toEqual({ busy: false, failure: 'installerFailed' });
});
it('only a completed successful operation or explicit clear retires old presentation', () => {
  const first = beginHelperInstall()!; completeHelperInstall(first, 'upgradeDeferred');
  const second = beginHelperInstall()!; completeHelperInstall(second, null);
  expect(helperInstallSnapshot()).toEqual({ busy: false, failure: null });
  const third = beginHelperInstall()!; completeHelperInstall(third, 'cancelled');
  clearHelperInstallFeedback();
  expect(helperInstallSnapshot()).toEqual({ busy: false, failure: null });
});
