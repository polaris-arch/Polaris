import { readFileSync } from 'node:fs';
import { expect, it, vi } from 'vitest';

const state = vi.hoisted(() => ({
  effect: vi.fn(), sync: vi.fn(), setPrivacyMode: vi.fn(), port: {},
}));
vi.mock('react', () => ({ useEffect: state.effect }));
vi.mock('./ipc', () => ({ api: { config: state.port } }));
vi.mock('./lib/privacy-mode-sync', () => ({ startPrivacyModeSync: state.sync }));
vi.mock('./store/app-store', () => ({ useAppStore: { getState: () => ({ setPrivacyMode: state.setPrivacyMode }) } }));
import { HarnessPrivacyRoot } from '../harness-privacy-root';

it('standalone fixture root reuses the privacy feed and its disposal instead of forcing unlock', () => {
  const dispose = vi.fn();
  state.sync.mockReturnValue(dispose);
  expect(HarnessPrivacyRoot({ children: 'fixture' })).toBe('fixture');
  expect(state.effect).toHaveBeenCalledWith(expect.any(Function), []);
  const cleanup = state.effect.mock.calls[0][0]();
  expect(state.sync).toHaveBeenCalledWith(state.port, expect.any(Function));
  const apply = state.sync.mock.calls[0][1];
  apply(true); apply(false);
  expect(state.setPrivacyMode.mock.calls).toEqual([[true], [false]]);
  expect(cleanup).toBe(dispose);
  cleanup(); expect(dispose).toHaveBeenCalledOnce();
});

it('both standalone fixture branches install the feed; the full App retains its own lifecycle', () => {
  const source = readFileSync(new URL('../harness-main.tsx', import.meta.url), 'utf8');
  expect(source).toContain('<HarnessPrivacyRoot><MeshJoinDialogView');
  expect(source).toContain('<HarnessPrivacyRoot><DialogHost /></HarnessPrivacyRoot>');
  expect(source.match(/<HarnessPrivacyRoot>/g)).toHaveLength(2);
  expect(source).toContain('<App />');
  expect(source).not.toContain('setPrivacyMode(false)');
});
