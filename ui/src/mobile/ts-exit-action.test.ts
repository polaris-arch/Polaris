import { describe, expect, it } from 'vitest';
import { tsExitAction } from './ts-exit-action';

describe('mobile Tailscale warning action', () => {
  it('takes a fresh web authorization URL only for a real needs-auth warning', () => {
    expect(tsExitAction('needs-auth', 'https://headscale.example/auth')).toEqual({
      kind: 'login-url', url: 'https://headscale.example/auth',
    });
    expect(tsExitAction('needs-auth', null)).toEqual({ kind: 'login-panel' });
    expect(tsExitAction('needs-auth', 'javascript:alert(1)')).toEqual({ kind: 'login-panel' });
    expect(tsExitAction('needs-auth', '', 'https://headscale.example/current')).toEqual({
      kind: 'login-url', url: 'https://headscale.example/current',
    });
  });

  it('always opens exit setup for exit-device warnings, even with a stale login URL', () => {
    for (const warning of ['no-exit-device', 'exit-device-offline', 'exit-device-not-advertised'] as const) {
      expect(tsExitAction(warning, 'https://headscale.example/old-auth')).toEqual({ kind: 'exit-panel' });
    }
  });
});
