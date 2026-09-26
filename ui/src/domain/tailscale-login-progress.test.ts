import { describe, expect, it, vi } from 'vitest';
import { acceptLoginProgress, authorizeFromMainFrame, copyLoginUrl, claimLoginUrl, mainAuthUrlOwner, openLoginUrl, loginFailureReasonKey, type TailscaleLoginProgress } from './tailscale-login-progress';
import { validatedTailscaleAuthUrl } from './tailscale-auth-url';

const current: TailscaleLoginProgress = { serverId: 'ts1', attemptId: 'new', phase: 'starting' };
describe('Tailscale login request identity and result', () => {
  it('late old URLs/cancel/success do not affect a newer request', () => {
    for (const phase of ['awaitingAuth', 'cancelled', 'authorized'] as const) {
      expect(acceptLoginProgress(current, { ...current, attemptId: 'old', phase })).toBe(false);
    }
    expect(acceptLoginProgress(current, { ...current, phase: 'awaitingAuth' })).toBe(true);
    expect(acceptLoginProgress({ ...current, phase: 'failed' }, { ...current, phase: 'authorized' })).toBe(false);
  });
  it('only a fresh Running frame for a compatible main-core request can authorize', () => {
    const frame = { serverId: 'ts1', backendState: 'Running', expired: false };
    expect(authorizeFromMainFrame(current, frame)).toBeNull();
    const owned = { ...current, phase: 'mainCore' as const };
    expect(authorizeFromMainFrame(owned, frame)?.phase).toBe('authorized');
    expect(authorizeFromMainFrame({ ...owned, reason: 'configurationPending' }, frame)).toBeNull();
    expect(authorizeFromMainFrame({ ...owned, reason: 'confirmingMainCore' }, frame)).toBeNull();
    expect(authorizeFromMainFrame(owned, { ...frame, serverId: 'another' })).toBeNull();
    expect(authorizeFromMainFrame(owned, { ...frame, backendState: 'Starting' })).toBeNull();
    expect(authorizeFromMainFrame(owned, { ...frame, expired: true })).toBeNull();
  });
  it('failure presentation maps stable categories instead of arbitrary private diagnostics', () => {
    expect(loginFailureReasonKey('coreUnavailable')).toBe('ts.reasonCoreUnavailable');
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
  it('main STATUS and AUTH use one owner and open the same URL once', () => {
    const seen = new Map<string, string>();
    const owned = { ...current, phase: 'mainCore' as const };
    for (const _event of ['STATUS', 'AUTH']) {
      const owner = mainAuthUrlOwner(owned)!;
      expect(claimLoginUrl(seen, 'ts1', owner, 'https://hs.example/login')).toBe(_event === 'STATUS');
    }
    expect(claimLoginUrl(seen, 'ts1', 'next', 'https://hs.example/login')).toBe(true);
    expect(seen.size).toBe(1);
    expect(mainAuthUrlOwner(undefined)).toBe('legacy');
  });
  it('Running success and pending credentials ignore residual primary URLs', () => {
    const owned = { ...current, phase: 'mainCore' as const };
    const authorized = authorizeFromMainFrame(owned, { serverId: 'ts1', backendState: 'Running', expired: false })!;
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
