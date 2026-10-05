import { describe, expect, it, vi } from 'vitest';
import { acceptLoginProgress, copyLoginUrl, claimLoginUrl, mainAuthUrlOwner, openLoginUrl, loginAttemptActive, loginFailureReasonKey, progressForLoginRequest, type TailscaleLoginProgress } from './tailscale-login-progress';
import { validatedTailscaleAuthUrl } from './tailscale-auth-url';

const current: TailscaleLoginProgress = { serverId: 'ts1', attemptId: 'new', phase: 'starting' };
describe('Tailscale login request identity and result', () => {
  it('an older panel cannot display the URL or result of a newer attempt on the same node', () => {
    const a = { serverId: 'ts1', attemptId: 'A' };
    const b = { serverId: 'ts1', attemptId: 'B' };
    const waiting: TailscaleLoginProgress = { ...b, phase: 'awaitingAuth', url: 'https://hs.example/B' };
    expect(progressForLoginRequest(waiting, a)).toBeUndefined();
    expect(progressForLoginRequest({ ...waiting, phase: 'authorized' }, a)).toBeUndefined();
    expect(progressForLoginRequest({ ...waiting, phase: 'failed', reason: 'authorizationTimedOut' }, a)).toBeUndefined();
    expect(progressForLoginRequest(waiting, b)).toEqual(waiting);
    expect(progressForLoginRequest({ ...waiting, attemptId: 'A' }, a)?.url).toBe('https://hs.example/B');
    expect(progressForLoginRequest(waiting, { serverId: 'other', attemptId: 'B' })).toBeUndefined();
    expect(progressForLoginRequest(waiting, null)).toBeUndefined();
  });
  it('late old URLs/cancel/success do not affect a newer request', () => {
    for (const phase of ['awaitingAuth', 'cancelled', 'authorized'] as const) {
      expect(acceptLoginProgress(current, { ...current, attemptId: 'old', phase })).toBe(false);
    }
    expect(acceptLoginProgress(current, { ...current, phase: 'awaitingAuth' })).toBe(true);
    expect(acceptLoginProgress({ ...current, phase: 'failed' }, { ...current, phase: 'authorized' })).toBe(false);
  });
  it('a bound producer cannot move an attempt to another main generation or identity epoch', () => {
    const owned: TailscaleLoginProgress = { ...current, phase: 'mainCore', mainGeneration: 7, identityEpoch: 'epoch-A' };
    expect(acceptLoginProgress(owned, { ...owned, phase: 'authorized' })).toBe(true);
    expect(acceptLoginProgress(owned, { ...owned, phase: 'authorized', mainGeneration: 8 })).toBe(false);
    expect(acceptLoginProgress(owned, { ...owned, phase: 'authorized', identityEpoch: 'epoch-B' })).toBe(false);
    expect(acceptLoginProgress(owned, { ...current, phase: 'authorized' })).toBe(false);
    expect(acceptLoginProgress(owned, { ...owned, serverId: 'other', phase: 'authorized' })).toBe(false);
    for (const phase of ['starting', 'preparingConnection', 'waitingForReady', 'awaitingAuth', 'mainCore'] as const) {
      expect(loginAttemptActive(phase)).toBe(true);
    }
  });
  it('failure presentation maps stable categories instead of arbitrary private diagnostics', () => {
    expect(loginFailureReasonKey('coreUnavailable')).toBe('ts.reasonCoreUnavailable');
    expect(loginFailureReasonKey('ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED')).toBe('errors.androidNativeCapacityClosed');
    expect(loginFailureReasonKey('ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED ')).toBe('ts.reasonAuthorization');
    expect(loginFailureReasonKey('mainCoreInUse')).toBe('ts.reasonMainCoreInUse');
    expect(loginFailureReasonKey('PRIVATE_KEY_RAW_DIAGNOSTIC')).toBe('ts.reasonAuthorization');
  });
});

describe('authorization URL and clipboard', () => {
  it('supports official and custom Headscale HTTP(S) hosts and rejects executable/non-web URLs', () => {
    for (const url of ['https://login.tailscale.com/a/example', 'http://headscale.example:8080/register/key', 'https://hs.example/custom?token=value']) {
      expect(validatedTailscaleAuthUrl(url)).toBe(url);
    }
    for (const url of ['javascript:alert(1)', 'file:///etc/passwd', 'ftp://headscale.example/login', 'https://', '/login', 'not a url']) {
      expect(validatedTailscaleAuthUrl(url)).toBeNull();
    }
  });
  it('missing or denied clipboard does not produce success', async () => {
    await expect(copyLoginUrl('https://hs.example', undefined)).rejects.toThrow();
    await expect(copyLoginUrl('https://hs.example', { writeText: async () => { throw new Error('denied'); } })).rejects.toThrow('denied');
  });
  it('copy waits for the clipboard promise', async () => {
    let resolve!: () => void;
    let completed = false;
    const pending = copyLoginUrl('https://hs.example', { writeText: () => new Promise<void>((r) => { resolve = r; }) }).then(() => { completed = true; });
    await Promise.resolve();
    expect(completed).toBe(false);
    resolve();
    await pending;
    expect(completed).toBe(true);
  });
});

describe('browser event delivery', () => {
  it('bound progress opens a URL once; bare AUTH serves only an absent request', () => {
    const seen = new Map<string, string>();
    const owned = { ...current, phase: 'mainCore' as const };
    for (const _event of ['STATUS', 'AUTH']) {
      const owner = owned.attemptId;
      expect(claimLoginUrl(seen, 'ts1', owner, 'https://hs.example/login')).toBe(_event === 'STATUS');
    }
    expect(claimLoginUrl(seen, 'ts1', 'next', 'https://hs.example/login')).toBe(true);
    expect(seen.size).toBe(1);
    expect(mainAuthUrlOwner(undefined)).toBe('legacy');
    expect(mainAuthUrlOwner(owned)).toBeNull();
  });
  it('Running success and pending credentials ignore residual primary URLs', () => {
    const owned = { ...current, phase: 'mainCore' as const };
    const authorized: TailscaleLoginProgress = { ...owned, phase: 'authorized' };
    expect(mainAuthUrlOwner(authorized)).toBeNull();
    expect(mainAuthUrlOwner({ ...owned, reason: 'configurationPending' })).toBeNull();
    expect(mainAuthUrlOwner(current)).toBeNull();
  });
  it('browser rejection reports a copy fallback without changing saved authorization progress', async () => {
    const owned = { ...current, phase: 'awaitingAuth' as const, url: 'https://hs.example/login' };
    const failed = vi.fn();
    expect(await openLoginUrl(owned.url, async () => { throw new Error('private system diagnostic'); }, failed)).toBe(false);
    expect(failed).toHaveBeenCalledOnce();
    expect(owned.phase).toBe('awaitingAuth');
    expect(owned.url).toBe('https://hs.example/login');
  });
});
