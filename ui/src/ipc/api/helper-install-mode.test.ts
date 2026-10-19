import { beforeEach, expect, it, vi } from 'vitest';
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('../ipc-client', () => ({ invoke, listen: vi.fn(), listenReady: vi.fn() }));
import { helperApi } from './system';
beforeEach(() => vi.clearAllMocks());
it('omitted mode preserves the existing ordinary IPC frame', async () => {
  await helperApi.install();
  expect(invoke.mock.calls).toEqual([['helper_install']]);
});
it.each(['ordinary', 'stopAndUpgrade'] as const)('passes only the explicit typed %s mode', async mode => {
  await helperApi.install(mode);
  expect(invoke.mock.calls).toEqual([['helper_install', { mode }]]);
});
