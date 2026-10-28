import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { UserConfig } from '@/contracts/types';
import type { AutoSelectReceipt } from '@/contracts/auto-select';
import { statusFixture } from '@/contracts/auto-select-status.test-support';
import { autoSelectActionFeedback, autoSelectContext, autoSelectErrorFeedback, runAutoSelectAction } from './auto-select-actions';
import { withConfigWriteLock } from './config-write-lock';
import { configReadGeneration } from './config-read-generation';
const native = vi.hoisted(() => ({ server: { autoSelectEnable: vi.fn(), autoSelectSwitchNow: vi.fn(), switch: vi.fn() }, config: { get: vi.fn() } }));
vi.mock('@/ipc', () => ({ api: native }));
const t = (key: string, vars?: Readonly<Record<string, string | number>>) => key + (vars ? JSON.stringify(vars) : '');
const config = (): UserConfig => ({ servers: [{ id: 'node-a', name: 'Node A', protocol: 'socks', address: 'fixture.invalid', port: 1, subscriptionId: 'sub-a' }], subscriptions: [{ id: 'sub-a', name: 'Sub A', url: 'https://fixture.invalid' }], selectedServerId: 'node-a', selectionIntent: { mode: 'auto', scope: 'subscription', subscriptionId: 'sub-a' } } as UserConfig);
const receipt = (status?: NonNullable<AutoSelectReceipt['placement']>['status']): AutoSelectReceipt => ({ intentApplied: true, placement: status ? { status, serverId: 'node-a' } : null, status: statusFixture() });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(ok => { resolve = ok; }); return { resolve, promise }; }
beforeEach(() => { vi.resetAllMocks(); native.config.get.mockResolvedValue(config()); native.server.autoSelectEnable.mockResolvedValue(receipt()); native.server.autoSelectSwitchNow.mockResolvedValue(receipt('applied')); native.server.switch.mockResolvedValue({ status: 'applied', serverId: 'node-a' }); });
describe('authoritative Auto selection actions', () => {
  it('dedicated enable command adopts saved truth without patching intent', async () => {
    const accept = vi.fn(); const result = await runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, accept);
    expect(native.server.autoSelectEnable).toHaveBeenCalledExactlyOnceWith('sub-a'); expect(native.server.switch).not.toHaveBeenCalled();
    expect(accept).toHaveBeenCalledExactlyOnceWith(config()); expect(autoSelectActionFeedback(result!, t)?.text).toBe('autoSelect.intentSaved');
  });
  it('switch-now invokes its command, without computing a winner', async () => {
    const result = await runAutoSelectAction({ kind: 'switchNow' }, vi.fn());
    expect(native.server.autoSelectSwitchNow).toHaveBeenCalledExactlyOnceWith();
    expect(autoSelectActionFeedback(result!, t)).toEqual({ tone: 'success', text: 'autoSelect.receipt.applied{"name":"Node A"}' });
  });
  it('manual cancellation uses the actual real node and adopts absent intent', async () => {
    const manual = config(); delete manual.selectionIntent; native.config.get.mockResolvedValue(manual);
    const result = await runAutoSelectAction({ kind: 'manual', serverId: 'node-a' }, vi.fn());
    expect(native.server.switch).toHaveBeenCalledExactlyOnceWith('node-a'); expect(autoSelectActionFeedback(result!, t)?.text).toContain('autoSelect.manualSaved');
  });
  it('failed cancellation keeps saved Auto and stable error identity', async () => {
    const accept = vi.fn(); native.server.switch.mockRejectedValue({ code: 'AUTO_SELECT_SWITCH_FAILED' });
    await expect(runAutoSelectAction({ kind: 'manual', serverId: 'node-a' }, accept)).rejects.toMatchObject({ code: 'AUTO_SELECT_SWITCH_FAILED', savedContext: autoSelectContext(config()) });
    expect(accept).toHaveBeenCalledExactlyOnceWith(config());
  });
  it('partial-save rejection still adopts authoritative readback', async () => {
    const accept = vi.fn(); native.server.autoSelectEnable.mockRejectedValue({ code: 'AUTO_SELECT_NO_TARGET' });
    await expect(runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, accept)).rejects.toMatchObject({ code: 'AUTO_SELECT_NO_TARGET' }); expect(accept).toHaveBeenCalledExactlyOnceWith(config());
  });
  it('readback failure never adopts or claims native success', async () => {
    const accept = vi.fn(); native.config.get.mockRejectedValue(Error('offline'));
    await expect(runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, accept)).rejects.toMatchObject({ code: 'AUTO_SELECT_READBACK_FAILED' }); expect(accept).not.toHaveBeenCalled();
  });
  it('saved adoption failure retains authoritative context and cannot announce native success', async () => {
    const accept = vi.fn().mockRejectedValue(Error('fixture adoption'));
    await expect(runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, accept)).rejects.toMatchObject({ code: 'AUTO_SELECT_READBACK_FAILED', savedContext: autoSelectContext(config()) });
    expect(native.config.get).toHaveBeenCalledTimes(1);
    expect(accept).toHaveBeenCalledExactlyOnceWith(config());
  });
  it('a late readback cannot overwrite a newer saved selection', async () => {
    const accept = vi.fn();
    native.config.get.mockImplementation(async () => { configReadGeneration.published(); return config(); });
    const result = await runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, accept);
    expect(result).toBeNull();
    expect(native.server.autoSelectEnable).toHaveBeenCalledTimes(1);
    expect(native.config.get).toHaveBeenCalledTimes(1);
    expect(accept).not.toHaveBeenCalled();
  });
  for (const fails of [false, true]) it(`a newer whole-config read supersedes a late ${fails ? 'failed' : 'successful'} action read`, async () => {
    const accept = vi.fn();
    native.config.get.mockImplementation(async () => {
      configReadGeneration.beginRead();
      if (fails) throw Error('old read failed');
      return config();
    });
    expect(await runAutoSelectAction({ kind: 'switchNow' }, accept)).toBeNull();
    expect(accept).not.toHaveBeenCalled();
  });
  it('queued work cancels synchronously after a preceding selection change', async () => {
    let current = autoSelectContext(config()); const release = deferred<void>();
    const first = withConfigWriteLock(async () => { await release.promise; current = 'manual'; });
    const queued = runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, vi.fn(), () => current === autoSelectContext(config()));
    release.resolve(); await first; expect(await queued).toBeNull(); expect(native.server.autoSelectEnable).not.toHaveBeenCalled(); expect(native.config.get).not.toHaveBeenCalled();
  });
  it('a failed write releases the shared queue for a later manual choice', async () => {
    native.server.autoSelectEnable.mockRejectedValue({ code: 'AUTO_SELECT_NO_TARGET' });
    const first = runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, vi.fn()); const later = runAutoSelectAction({ kind: 'manual', serverId: 'node-a' }, vi.fn());
    await expect(first).rejects.toMatchObject({ code: 'AUTO_SELECT_NO_TARGET' }); await later; expect(native.server.switch).toHaveBeenCalledTimes(1); expect(native.config.get).toHaveBeenCalledTimes(2);
  });
  for (const status of ['pending', 'restarting', 'notRunning', 'deferred'] as const) it(`${status} placement is never success feedback`, () => {
    const feedback = autoSelectActionFeedback({ action: { kind: 'switchNow' }, receipt: receipt(status), config: config() }, t);
    expect(feedback?.tone).not.toBe('success'); expect(feedback?.text).toContain(`autoSelect.receipt.${status}`);
  });
  it('failed placement reports saved intent and incomplete runtime', () => { expect(autoSelectActionFeedback({ action: { kind: 'enable', subscriptionId: 'sub-a' }, receipt: receipt('failed'), config: config() }, t)).toEqual({ tone: 'warning', text: 'autoSelect.placementFailed' }); });
  it('newer manual readback suppresses the old Auto receipt', () => {
    const manual = config(); delete manual.selectionIntent;
    expect(autoSelectActionFeedback({ action: { kind: 'enable', subscriptionId: 'sub-a' }, receipt: receipt('applied'), config: manual }, t)).toBeNull();
  });
  it('superseded and divergent receipts never advertise an applied exit', () => {
    expect(autoSelectActionFeedback({ action: { kind: 'switchNow' }, receipt: receipt('superseded'), config: config() }, t)).toBeNull();
    expect(autoSelectActionFeedback({ action: { kind: 'switchNow' }, receipt: receipt('applied'), config: { ...config(), selectedServerId: 'newer' } }, t)?.text).toBe('autoSelect.selectionChanged');
  });
  it('unconfirmed cancellation cannot claim saved manual mode', () => {
    expect(autoSelectActionFeedback({ action: { kind: 'manual', serverId: 'node-a' }, receipt: { status: 'applied' }, config: config() }, t)?.text).not.toContain('manualSaved');
  });
  for (const code of ['AUTO_SELECT_SWITCHED_OFF', 'AUTO_SELECT_PLATFORM_NOT_OPEN', 'AUTO_SELECT_SUBSCRIPTION_NOT_FOUND', 'AUTO_SELECT_SUBSCRIPTION_EMPTY', 'AUTO_SELECT_NOT_AUTO', 'AUTO_SELECT_NO_TARGET', 'AUTO_SELECT_SWITCH_FAILED', 'AUTO_SELECT_READBACK_FAILED']) it(`stable ${code} error is actionable`, () => { expect(autoSelectErrorFeedback({ code }, t)).toEqual({ tone: 'error', text: `autoSelect.error.${code}` }); });
});
