import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { startPrivacyModeSync } from '../../lib/privacy-mode-sync';
const transport = vi.hoisted(() => vi.fn());
const events = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: transport }));
vi.mock('@tauri-apps/api/event', () => ({ listen: events }));
const { configApi } = await import('./config');
async function flush() { for (let n = 0; n < 16; n++) await Promise.resolve(); }
let stop: (() => void) | undefined;
beforeEach(() => {
  vi.useFakeTimers(); transport.mockReset(); events.mockReset();
  events.mockResolvedValue(() => {});
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});
afterEach(() => { stop?.(); stop = undefined; vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
describe('privacy authority through actual IPC wrapper', () => {
  it('plain-browser preview unlocks with boolean authority and schedules no retry', async () => {
    vi.stubGlobal('window', {});
    const read = vi.spyOn(configApi, 'getPrivacyMode');
    const values: boolean[] = [];
    stop = startPrivacyModeSync(configApi, value => values.push(value)); await flush();
    expect(values).toEqual([true, false]); expect(await read.mock.results[0].value).toBe(false);
    await vi.advanceTimersByTimeAsync(3000);
    expect(read).toHaveBeenCalledOnce(); expect(vi.getTimerCount()).toBe(0);
    expect(transport).not.toHaveBeenCalled(); expect(events).not.toHaveBeenCalled();
  });
  it.each(['rejection', 'missing boolean'])('native %s stays locked and retries without preview fallback', async mode => {
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
    if (mode === 'rejection') transport.mockRejectedValue(new Error('native unavailable'));
    else transport.mockResolvedValue({ success: true });
    const values: boolean[] = [];
    stop = startPrivacyModeSync(configApi, value => values.push(value)); await flush();
    await vi.advanceTimersByTimeAsync(1000);
    expect(values).toEqual([true]); expect(transport).toHaveBeenCalledTimes(2);
    expect(events).toHaveBeenCalledTimes(2);
  });
  it('native lock event wins against an older unlocked snapshot', async () => {
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} });
    let resolve!: (value: unknown) => void;
    transport.mockReturnValue(new Promise(r => { resolve = r; }));
    const values: boolean[] = [];
    stop = startPrivacyModeSync(configApi, value => values.push(value)); await flush();
    events.mock.calls[0][1]({ payload: undefined });
    resolve({ success: true, data: false }); await flush();
    expect(values).toEqual([true, true]); expect(vi.getTimerCount()).toBe(0);
  });
});
