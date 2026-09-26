import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { TsLoginModeSwitch } from './TsLoginModeSwitch';
import type { ServerConfig } from '@/contracts/types';
import { executeTsLogin, planTsLoginSubmit, type TsLoginExecution } from './ts-login-server';

function execution(overrides: Partial<TsLoginExecution> = {}): TsLoginExecution {
  return { isActive: () => true, prepare: vi.fn(async () => {}), logout: vi.fn(async () => {}),
    save: vi.fn(async () => {}), onSaved: vi.fn(), refresh: vi.fn(async () => {}),
    start: vi.fn(async () => ({ started: true })), cancel: vi.fn(async () => {}), ...overrides };
}

describe('save followed by authorization', () => {
  it('failed authorization preserves the saved identity and retry updates instead of adding twice', async () => {
    let saved: ServerConfig | undefined;
    const additions: string[] = [];
    const mint = vi.fn(() => 'saved-node');
    for (let index = 0; index < 2; index++) {
      const plan = planTsLoginSubmit({ existing: saved, mode: 'browser', authKey: '', controlUrl: '', hasState: false, mintId: mint });
      const input = execution({ save: async () => { if (plan.persist === 'add') additions.push(plan.server.id); },
        onSaved: () => { saved = plan.server; }, start: async () => { throw new Error('PRIVATE_KEY_MUST_NOT_APPEAR'); } });
      const result = await executeTsLogin(input);
      expect(result).toEqual({ phase: 'failed', reason: 'authorizationRequestFailed' });
      expect(input.cancel).toHaveBeenCalledOnce();
      expect(saved?.id).toBe('saved-node');
    }
    expect(additions).toEqual(['saved-node']);
    expect(mint).toHaveBeenCalledOnce();
  });

  it('close before prepare resolves prevents save/start and awaits cancellation', async () => {
    let active = true;
    let finishPrepare!: () => void;
    let finishCancel!: () => void;
    const input = execution({ isActive: () => active,
      prepare: () => new Promise<void>((resolve) => { finishPrepare = resolve; }),
      cancel: vi.fn(() => new Promise<void>((resolve) => { finishCancel = resolve; })) });
    let ended = false;
    const pending = executeTsLogin(input).then((result) => { ended = true; return result; });
    active = false;
    finishPrepare();
    await vi.waitFor(() => expect(input.cancel).toHaveBeenCalledOnce());
    expect(input.save).not.toHaveBeenCalled();
    expect(input.start).not.toHaveBeenCalled();
    expect(ended).toBe(false);
    finishCancel();
    expect(await pending).toEqual({ phase: 'cancelled' });
  });

  it('close during a successful save retains the persisted node but does not start authorization', async () => {
    let active = true;
    const input = execution({ isActive: () => active, save: async () => { active = false; } });
    expect(await executeTsLogin(input)).toEqual({ phase: 'cancelled' });
    expect(input.onSaved).toHaveBeenCalledOnce();
    expect(input.start).not.toHaveBeenCalled();
    expect(input.cancel).toHaveBeenCalledOnce();
  });

  it('fresh state-query failure blocks logout/save/start and cannot claim save success', async () => {
    const input = execution({ verifyState: async () => { throw new Error('unavailable'); } });
    expect(await executeTsLogin(input)).toEqual({ phase: 'failed', reason: 'stateQueryFailed' });
    expect(input.logout).not.toHaveBeenCalled();
    expect(input.save).not.toHaveBeenCalled();
    expect(input.onSaved).not.toHaveBeenCalled();
    expect(input.start).not.toHaveBeenCalled();
  });

  it('main-core logout rejection retains a specific reason and does not overwrite saved credentials', async () => {
    const input = execution({ verifyState: async () => true,
      logout: async () => { throw { code: 'TAILSCALE_LOGOUT_MAIN_CORE', message: 'PRIVATE_KEY' }; } });
    expect(await executeTsLogin(input)).toEqual({ phase: 'failed', reason: 'mainCoreInUse' });
    expect(input.save).not.toHaveBeenCalled();
    expect(input.start).not.toHaveBeenCalled();
  });

  it('browser selection clears a saved preauth key and persists the explicit mode change', () => {
    const existing = { id: 'ts1', protocol: 'tailscale', tailscaleSettings: { authKey: 'private' } } as ServerConfig;
    const plan = planTsLoginSubmit({ existing, mode: 'browser', authKey: '', controlUrl: '', hasState: true, mintId: () => 'new' });
    expect(plan.persist).toBe('update');
    expect(plan.server.tailscaleSettings?.authKey).toBeUndefined();
    expect(existing.tailscaleSettings?.authKey).toBe('private');
  });
});

it('switching mode is blocked while prepare/save is pending, so the current submit completes once', async () => {
  let active = true;
  let submitting = true;
  let finishPrepare!: () => void;
  const onChange = vi.fn(() => { active = false; });
  const input = execution({ isActive: () => active, prepare: () => new Promise<void>((resolve) => { finishPrepare = resolve; }) });
  const pending = executeTsLogin(input).then((result) => { if (active) submitting = false; return result; });
  const switcher = TsLoginModeSwitch({ mode: 'browser', submitting, label: 'method', browserLabel: 'browser', authkeyLabel: 'key', onChange });
  expect(renderToStaticMarkup(switcher).match(/disabled=""/g)).toHaveLength(2);
  const buttons = switcher.props.children as Array<{ props: { onClick: () => void; disabled: boolean } }>;
  for (const button of buttons) button.props.onClick();
  expect(onChange).not.toHaveBeenCalled();
  finishPrepare();
  expect(await pending).toEqual({ phase: 'handedOff' });
  expect(input.save).toHaveBeenCalledOnce();
  expect(input.start).toHaveBeenCalledOnce();
  expect(submitting).toBe(false);
});
