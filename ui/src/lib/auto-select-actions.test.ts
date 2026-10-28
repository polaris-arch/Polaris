import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { UserConfig } from '@/contracts/types';
import type { NativeAutoReceipt as AutoSelectReceipt } from '@/contracts/auto-select';
import { nativeAutoFixture } from '@/contracts/native-auto.test-support';
import { autoSelectActionFeedback, autoSelectContext, autoSelectErrorFeedback, runAutoSelectAction } from './auto-select-actions';
import { withConfigWriteLock } from './config-write-lock';
import { configReadGeneration } from './config-read-generation';
const native = vi.hoisted(() => ({ server: { autoSelectEnable: vi.fn(), autoSelectDisable: vi.fn(), autoSelectSetEnabled: vi.fn(), autoSelectSwitchNow: vi.fn(), switch: vi.fn() }, config: { get: vi.fn() } }));
vi.mock('@/ipc', () => ({ api: native }));
const t = (key: string, vars?: Readonly<Record<string, string | number>>) => key + (vars ? JSON.stringify(vars) : '');
const config = (): UserConfig => ({ servers: [{ id: 'node-a', name: 'Node A', protocol: 'socks', address: 'fixture.invalid', port: 1, subscriptionId: 'sub-a' }], subscriptions: [{ id: 'sub-a', name: 'Sub A', url: 'https://fixture.invalid' }], selectedServerId: 'node-a', selectionIntent: { mode: 'auto', scope: 'subscription', subscriptionId: 'sub-a' } } as UserConfig);
const receipt = (_status?: string): AutoSelectReceipt => ({ intentApplied: true, status: nativeAutoFixture() });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(ok => { resolve = ok; }); return { resolve, promise }; }
beforeEach(() => { vi.resetAllMocks(); native.config.get.mockResolvedValue(config()); native.server.autoSelectEnable.mockResolvedValue(receipt()); native.server.autoSelectSwitchNow.mockResolvedValue(receipt('applied')); native.server.switch.mockResolvedValue({ status: 'applied', serverId: 'node-a' }); });
describe('authoritative Auto selection actions', () => {
  it('dedicated enable command adopts saved truth without patching intent', async () => {
    const accept = vi.fn(); const result = await runAutoSelectAction({ kind: 'enable', subscriptionId: 'sub-a' }, accept);
    expect(native.server.autoSelectEnable).toHaveBeenCalledExactlyOnceWith('sub-a', undefined); expect(native.server.switch).not.toHaveBeenCalled();
    expect(accept).toHaveBeenCalledExactlyOnceWith(config()); expect(autoSelectActionFeedback(result!, t)?.text).toBe('autoSelect.intentSaved');
  });
  it('switch-now invokes its command, without computing a winner', async () => {
    const result = await runAutoSelectAction({ kind: 'switchNow' }, vi.fn());
    expect(native.server.autoSelectSwitchNow).toHaveBeenCalledExactlyOnceWith();
    expect(autoSelectActionFeedback(result!, t)).toEqual({ tone: 'info', text: 'autoSelect.intentSaved' });
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
  it('saved native reference never advertises a runtime winner from an acknowledgement', () => {
    const feedback = autoSelectActionFeedback({ action: { kind: 'enable', subscriptionId: 'sub-a' }, receipt: receipt(), config: config() }, t);
    expect(feedback).toEqual({ tone: 'info', text: 'autoSelect.intentSaved' });
  });
  it('newer manual readback suppresses the old Auto receipt', () => {
    const manual = config(); delete manual.selectionIntent;
    expect(autoSelectActionFeedback({ action: { kind: 'enable', subscriptionId: 'sub-a' }, receipt: receipt(), config: manual }, t)).toBeNull();
  });
  it('unapplied receipt never claims selected reference', () => {
    expect(autoSelectActionFeedback({ action: { kind: 'enable', subscriptionId: 'sub-a' }, receipt: { ...receipt(), intentApplied: false }, config: config() }, t)).toBeNull();
  });
  it('unconfirmed cancellation cannot claim saved manual mode', () => {
    expect(autoSelectActionFeedback({ action: { kind: 'manual', serverId: 'node-a' }, receipt: { status: 'applied' }, config: config() }, t)?.text).not.toContain('manualSaved');
  });
  for (const code of ['AUTO_SELECT_SWITCHED_OFF', 'AUTO_SELECT_PLATFORM_NOT_OPEN', 'AUTO_SELECT_SUBSCRIPTION_NOT_FOUND', 'AUTO_SELECT_SUBSCRIPTION_EMPTY', 'AUTO_SELECT_NOT_AUTO', 'AUTO_SELECT_NO_TARGET', 'AUTO_SELECT_SWITCH_FAILED', 'AUTO_SELECT_READBACK_FAILED']) it(`stable ${code} error is actionable`, () => { expect(autoSelectErrorFeedback({ code }, t)).toEqual({ tone: 'error', text: `autoSelect.error.${code}` }); });
});


it('disable clears empty stopped Auto without inventing a manual node or restarting', async () => {
  const saved = { ...config(), servers: [], selectedServerId: null, selectionIntent: undefined };
  native.server.autoSelectDisable.mockResolvedValue({ ...receipt(), intentApplied: true, status: nativeAutoFixture({ applyOutcome: 'saved', featureEnabled: false }) });
  native.config.get.mockResolvedValue(saved);
  const result = await runAutoSelectAction({ kind: 'disable' }, vi.fn());
  expect(native.server.autoSelectDisable).toHaveBeenCalledExactlyOnceWith();
  expect(native.server.switch).not.toHaveBeenCalled();
  expect(autoSelectActionFeedback(result!, t)?.text).toBe('autoSelect.featureOff');
});

it('global scope is sent explicitly through the existing selector command', async () => {
  await runAutoSelectAction({ kind: 'enable', scope: 'global' }, vi.fn());
  expect(native.server.autoSelectEnable).toHaveBeenCalledExactlyOnceWith(undefined, 'global');
});

it('OFF feedback only claims pinning when the native receipt includes groups', () => {
  const report = nativeAutoFixture({ applyOutcome: 'applied', featureEnabled: false, nativeAutoGroups: [{ target: { scope: 'global' }, actualServerId: 'node-a', confirmed: true, state: 'pinned' }] });
  const result = autoSelectActionFeedback({ action: { kind: 'setEnabled', enabled: false }, receipt: { intentApplied: true, status: report }, config: { ...config(), autoSelectEnabled: false } }, t);
  expect(result?.text).toBe('autoSelect.fixedCurrent');
});

for (const applyOutcome of ['restarting', 'pending', 'deferred'] as const) it(`saved enabled flag does not claim activation during ${applyOutcome}`, () => {
  const status = nativeAutoFixture({ featureEnabled: true, applyOutcome });
  const feedback = autoSelectActionFeedback({ action: { kind: 'setEnabled', enabled: true }, receipt: { intentApplied: true, status }, config: { ...config(), autoSelectEnabled: true } }, t);
  expect(feedback?.text).toBe(t(`autoSelect.receipt.${applyOutcome}`, { name: t('autoSelect.auto') }));
  expect(feedback?.tone).not.toBe('success');
});
for (const applyOutcome of ['saved', 'applied'] as const) it(`explicit ${applyOutcome} mode receipt permits success`, () => {
  const status = nativeAutoFixture({ featureEnabled: true, applyOutcome });
  expect(autoSelectActionFeedback({ action: { kind: 'setEnabled', enabled: true }, receipt: { intentApplied: true, status }, config: { ...config(), autoSelectEnabled: true } }, t)?.text).toBe('autoSelect.featureOn');
});

it('a legacy flag-only mode receipt stays pending rather than claiming activation', () => {
  const feedback = autoSelectActionFeedback({ action: { kind: 'setEnabled', enabled: true }, receipt: receipt(), config: { ...config(), autoSelectEnabled: true } }, t);
  expect(feedback?.tone).toBe('info');
  expect(feedback?.text).toBe(t('autoSelect.receipt.pending', { name: t('autoSelect.auto') }));
});
