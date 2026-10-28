import { afterEach, describe, expect, it, vi } from 'vitest';
import { startPrivacyModeSync, type PrivacyModePort } from './privacy-mode-sync';
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
async function flush() { for (let n = 0; n < 8; n++) await Promise.resolve(); }
function harness() {
  const enter = deferred<() => void>(), exit = deferred<() => void>(), read = deferred<boolean>();
  let onEnter!: () => void, onExit!: () => void;
  const values: boolean[] = [], offEnter = vi.fn(), offExit = vi.fn();
  const port: PrivacyModePort = { onEnterPrivacyMode: vi.fn((fn) => { onEnter = fn; return enter.promise; }),
    onExitPrivacyMode: vi.fn((fn) => { onExit = fn; return exit.promise; }), getPrivacyMode: vi.fn(() => read.promise) };
  const dispose = startPrivacyModeSync(port, (value) => values.push(value));
  return { enter, exit, read, values, port, dispose, offEnter, offExit,
    ready: async () => { enter.resolve(offEnter); exit.resolve(offExit); await flush(); },
    lock: () => onEnter(), unlock: () => onExit() };
}
afterEach(() => vi.useRealTimers());
describe('privacy state synchronization', () => {
  it('fails closed until both listeners and snapshot are ready', async () => {
    const h = harness(); expect(h.values).toEqual([true]);
    h.enter.resolve(h.offEnter); await flush(); expect(h.port.getPrivacyMode).not.toHaveBeenCalled();
    h.exit.resolve(h.offExit); await flush(); h.read.resolve(false); await flush();
    expect(h.values).toEqual([true, false]); h.dispose();
  });
  it('newer lock event vetoes delayed unlocked snapshot', async () => {
    const h = harness(); await h.ready(); h.lock(); h.read.resolve(false); await flush();
    expect(h.values).toEqual([true, true]); h.dispose();
  });
  it('newer unlock event vetoes delayed locked snapshot', async () => {
    const h = harness(); await h.ready(); h.unlock(); h.read.resolve(true); await flush();
    expect(h.values).toEqual([true, false]); h.dispose();
  });
  it('disposal cleans pending registrations and prevents reading or events', async () => {
    const h = harness(); h.dispose(); await h.ready(); h.lock(); h.unlock();
    expect(h.port.getPrivacyMode).not.toHaveBeenCalled(); expect(h.values).toEqual([true]);
    expect(h.offEnter).toHaveBeenCalledOnce(); expect(h.offExit).toHaveBeenCalledOnce();
  });
  it('late snapshot after disposal never unlocks', async () => {
    const h = harness(); await h.ready(); h.dispose(); h.read.resolve(false); await flush();
    expect(h.values).toEqual([true]);
  });
  it('failed registration remains locked, releases partial listener and retries', async () => {
    vi.useFakeTimers(); const h = harness(); h.enter.resolve(h.offEnter); h.exit.reject(new Error('registration')); await flush();
    expect(h.port.getPrivacyMode).not.toHaveBeenCalled(); expect(h.values.every(Boolean)).toBe(true);
    expect(h.offEnter).toHaveBeenCalledOnce(); h.dispose(); await vi.advanceTimersByTimeAsync(2000);
    expect(h.port.onEnterPrivacyMode).toHaveBeenCalledOnce();
  });
  it('missing snapshot is unknown and cannot unlock', async () => {
    vi.useFakeTimers(); const h = harness(); await h.ready(); h.read.resolve(undefined as unknown as boolean); await flush();
    expect(h.values).toEqual([true]); h.dispose();
  });
  it('failed read keeps lock and retries without registering duplicate listeners', async () => {
    vi.useFakeTimers(); const h = harness(); await h.ready(); h.read.reject(new Error('read')); await flush();
    expect(h.values).toEqual([true]); vi.mocked(h.port.getPrivacyMode).mockResolvedValue(false);
    await vi.advanceTimersByTimeAsync(1000); await flush(); expect(h.values).toEqual([true, false]);
    expect(h.port.onEnterPrivacyMode).toHaveBeenCalledOnce(); h.dispose();
  });
});
