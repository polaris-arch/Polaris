import { describe, expect, it } from 'vitest';
import { tsExitAction } from './ts-exit-action';

describe('mobile Tailscale warning action', () => {
  it('takes a fresh web authorization URL only for a real needs-auth warning', () => {
    expect(tsExitAction('needs-auth', { liveUrl: 'https://headscale.example/auth' })).toEqual({
      kind: 'login-url', url: 'https://headscale.example/auth',
    });
    expect(tsExitAction('needs-auth', {})).toEqual({ kind: 'login-panel' });
    expect(tsExitAction('needs-auth', { liveUrl: 'javascript:alert(1)' })).toEqual({ kind: 'login-panel' });
    expect(tsExitAction('needs-auth', { liveUrl: '', storeUrl: 'https://headscale.example/current' })).toEqual({
      kind: 'login-url', url: 'https://headscale.example/current',
    });
  });

  it('always opens exit setup for exit-device warnings, even with a stale login URL', () => {
    for (const warning of ['no-exit-device', 'exit-device-offline', 'exit-device-not-advertised'] as const) {
      expect(tsExitAction(warning, { liveUrl: 'https://headscale.example/old-auth' })).toEqual({ kind: 'exit-panel' });
    }
  });

  it('an active request owns its progress URL, while terminal/prepare records cannot open STATUS URLs', () => {
    const attempt = { serverId: 'ts', attemptId: 'B', phase: 'awaitingAuth' as const, url: 'https://headscale.example/bound-B' };
    const source = { liveUrl: 'https://headscale.example/old-A', storeUrl: 'https://headscale.example/old-A', attempt };
    expect(tsExitAction('needs-auth', source)).toEqual({ kind: 'login-url', url: attempt.url });
    for (const phase of ['starting', 'preparingConnection', 'waitingForReady', 'authorized', 'failed', 'timedOut', 'cancelled'] as const) {
      expect(tsExitAction('needs-auth', { ...source, attempt: { ...attempt, phase } })).toEqual({ kind: 'login-panel' });
    }
  });

  it('iOS starts from a fresh panel unless active progress supplies the ready main/identity binding', () => {
    const source = { normalMainRequired: true, liveUrl: 'https://headscale.example/status-A', storeUrl: 'https://headscale.example/status-A' };
    expect(tsExitAction('needs-auth', source)).toEqual({ kind: 'login-panel' });
    const attempt = { serverId: 'ts', attemptId: 'B', phase: 'mainCore' as const, url: 'https://headscale.example/bound-B' };
    expect(tsExitAction('needs-auth', { ...source, attempt })).toEqual({ kind: 'login-panel' });
    expect(tsExitAction('needs-auth', { ...source, attempt: { ...attempt, mainGeneration: 7, identityEpoch: 'epoch-B' } })).toEqual({ kind: 'login-url', url: attempt.url });
  });
});
