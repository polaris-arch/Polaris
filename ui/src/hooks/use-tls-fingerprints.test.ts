import { afterEach, expect, it, vi } from 'vitest';
import type { TlsFingerprintPresentation, TlsFingerprintSnapshot, SelectedTlsSupportDirectory } from '@/domain/utls';

const reads = vi.hoisted(() => ({
  pending: [] as ((value: TlsFingerprintPresentation) => void)[],
  started: () => {}, stopped: () => {},
}));
vi.mock('@/ipc/api/updater', () => ({ coreApi: {
  getTlsFingerprints: () => new Promise<TlsFingerprintPresentation>(resolve => { reads.pending.push(resolve); }),
} }));
vi.mock('@/ipc/api/proxy', () => ({ proxyApi: {
  onStarted: (callback: () => void) => { reads.started = callback; return () => { reads.started = () => {}; }; },
  onStopped: (callback: () => void) => { reads.stopped = callback; return () => { reads.stopped = () => {}; }; },
} }));

import { currentTlsFingerprints, subscribeTlsFingerprints } from './use-tls-fingerprints';

const observed = (byte: string): TlsFingerprintSnapshot => ({
  observationScope: 'unavailable', byteIdentityScope: 'cli-executable', identity: null,
  binarySha256: byte.repeat(64), document: null, unavailableReason: 'fixture: old CLI',
});
let off: (() => void) | undefined;
afterEach(async () => {
  off?.(); off = undefined;
  for (const resolve of reads.pending.splice(0)) resolve(observed('a'));
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
  vi.unstubAllGlobals();
});

it('coalesces readers, invalidates at core transitions, and rejects an older in-flight core reply', async () => {
  vi.stubGlobal('window', new EventTarget());
  off = subscribeTlsFingerprints(() => {});
  const offSecond = subscribeTlsFingerprints(() => {});
  expect(reads.pending).toHaveLength(1);
  reads.started();
  const old = reads.pending.shift()!;
  old(observed('a'));
  await vi.waitFor(() => expect(reads.pending).toHaveLength(1));
  expect(currentTlsFingerprints()).toBeUndefined();
  reads.pending.shift()!(observed('b'));
  await vi.waitFor(() => expect(currentTlsFingerprints()).toEqual(observed('b')));
  reads.stopped();
  expect(currentTlsFingerprints()).toBeUndefined();
  offSecond(); off(); off = undefined;
  reads.pending.shift()!(observed('a'));
  await Promise.resolve(); await Promise.resolve();
  expect(currentTlsFingerprints()).toBeUndefined();
});

it('refreshes after delayed native setup completes without focus or a core transition', async () => {
  vi.stubGlobal('window', new EventTarget());
  off = subscribeTlsFingerprints(() => {});
  reads.pending.shift()!(observed('a'));
  await vi.waitFor(() => expect(currentTlsFingerprints()).toEqual(observed('a')));
  window.dispatchEvent(new Event('polaris-libbox-ready'));
  expect(currentTlsFingerprints()).toBeUndefined();
  reads.pending.shift()!(observed('b'));
  await vi.waitFor(() => expect(currentTlsFingerprints()).toEqual(observed('b')));
});

it('retains the complete native refusal in the shared diagnostic snapshot', async () => {
  vi.stubGlobal('window', new EventTarget());
  off = subscribeTlsFingerprints(() => {});
  const refusal = { ...observed('a'), unavailableReason: 'CORE_ADMISSION_UnsupportedExactObject: exact native object unavailable' };
  reads.pending.shift()!(refusal);
  await vi.waitFor(() => expect(currentTlsFingerprints()).toEqual(refusal));
  expect(currentTlsFingerprints()?.unavailableReason).toBe(refusal.unavailableReason);
});

it('publishes expected selected support without converting it to a loaded byte observation', async () => {
  vi.stubGlobal('window', new EventTarget());
  off = subscribeTlsFingerprints(() => {});
  const expected: SelectedTlsSupportDirectory = {
    evidenceKind: 'compiled-selected-support-v1', selectedSpec: null, document: null,
    unavailableReason: 'No reviewed support for this compiled selection',
  };
  reads.pending.shift()!(expected);
  await vi.waitFor(() => expect(currentTlsFingerprints()).toEqual(expected));
  expect(currentTlsFingerprints()).not.toHaveProperty('binarySha256');
  expect(currentTlsFingerprints()).not.toHaveProperty('observationScope');
});
