import { beforeEach, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';

const h = vi.hoisted(() => ({ state: [] as unknown[], refs: [] as Array<{ current: unknown }>, si: 0, ri: 0 }));
vi.mock('react', async (original) => ({
  ...await original<typeof import('react')>(),
  useState: (initial: unknown) => {
    const i = h.si++;
    if (!(i in h.state)) h.state[i] = initial;
    return [h.state[i], (value: unknown) => { h.state[i] = value; }];
  },
  useRef: (initial: unknown) => { const i = h.ri++; return h.refs[i] ?? (h.refs[i] = { current: initial }); },
  useEffect: () => {},
}));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => `translated:${key}` }) }));
vi.mock('@/ipc/api-client', () => ({ updateApi: { check: vi.fn(), clearPortableHandoff: vi.fn() }, versionApi: {} }));
vi.mock('../../dialogs/dialog-store', () => ({ useDialogStore: (selector: (s: unknown) => unknown) => selector({ open: vi.fn(), close: vi.fn() }) }));
vi.mock('../../layout/app-update-banner', () => ({ markAppVersionSkipped: vi.fn() }));
vi.mock('@/lib/error-handler', () => ({ toast: { error: vi.fn() } }));
const { updateApi } = await import('@/ipc/api-client');
const { toast } = await import('@/lib/error-handler');
const { useAppUpdate } = await import('./use-app-update');
function render() { h.si = h.ri = 0; return useAppUpdate(false); }
function manual() { render(); h.state[0] = 'manual'; h.state[9] = true; return render(); }
beforeEach(() => { h.state = []; h.refs = []; vi.clearAllMocks(); });
it.each([false, true])('ordinary check with network failure=%s does not dismiss unconfirmed handoff', async (failed) => {
  const api = manual();
  if (failed) vi.mocked(updateApi.check).mockRejectedValue(new Error('offline'));
  else vi.mocked(updateApi.check).mockResolvedValue({ hasUpdate: false });
  await api.checkUpdate();
  expect(updateApi.clearPortableHandoff).not.toHaveBeenCalled();
  expect(render().manualCompletionUnverified).toBe(true);
});
it('failed durable dismissal keeps the manual card and allows an explicit retry before checking', async () => {
  let api = manual();
  vi.mocked(updateApi.clearPortableHandoff).mockRejectedValueOnce({ code: 'portableReminderClearFailed' });
  await api.discardPortableAndCheck();
  expect(render().us).toBe('manual');
  expect(render().manualCompletionUnverified).toBe(true);
  expect(render().clearingPortable).toBe(false);
  expect(updateApi.check).not.toHaveBeenCalled();
  expect(toast.error).toHaveBeenCalledWith('translated:settings.update.portableReminderClearFailed');
  vi.mocked(updateApi.clearPortableHandoff).mockResolvedValueOnce(undefined);
  vi.mocked(updateApi.check).mockRejectedValueOnce(new Error('offline'));
  api = render(); await api.discardPortableAndCheck();
  expect(updateApi.clearPortableHandoff).toHaveBeenCalledTimes(2);
  expect(updateApi.check).toHaveBeenCalledTimes(1);
  expect(render().manualCompletionUnverified).toBe(false);
  expect(render().us).toBe('error'); // Explicit dismissal stays committed even if the subsequent check fails.
});
it('same-render duplicate dismissals cannot issue two disk mutations or check before durable success', async () => {
  const api = manual();
  let done!: () => void;
  vi.mocked(updateApi.clearPortableHandoff).mockImplementationOnce(() => new Promise<void>((resolve) => { done = resolve; }));
  vi.mocked(updateApi.check).mockResolvedValue({ hasUpdate: false });
  const first = api.discardPortableAndCheck();
  await api.discardPortableAndCheck();
  expect(updateApi.clearPortableHandoff).toHaveBeenCalledTimes(1);
  expect(updateApi.check).not.toHaveBeenCalled();
  expect(render().clearingPortable).toBe(true);
  done(); await first;
  expect(updateApi.check).toHaveBeenCalledTimes(1);
  expect(render().clearingPortable).toBe(false);
});

function rustBody(source: string, signature: string): string {
  const at = source.indexOf(signature);
  expect(at, signature).toBeGreaterThan(-1);
  const tail = source.slice(at);
  const masked = tail.replace(/\/\/[^\n]*|"(?:\\.|[^"\\])*"/g, (s) => ' '.repeat(s.length));
  const begin = masked.indexOf('{');
  let depth = 1, end = begin + 1;
  for (; end < masked.length && depth; end++) {
    if (masked[end] === '{') depth++;
    if (masked[end] === '}') depth--;
  }
  expect(depth).toBe(0);
  return tail.slice(begin, end).replace(/\/\/[^\n]*/g, '');
}
// Real production startup -> command -> runtime path regression, not the old
// claim that only the UI calls update_check. Read exactly the two owned sources.
it('default five-second startup reaches the observational command and cannot clear a handoff before success or failure', () => {
  const startup = readFileSync(new URL('../../../../../src-tauri/src/runtime/startup_tasks.rs', import.meta.url), 'utf8');
  const command = readFileSync(new URL('../../../../../src-tauri/src/commands/updater/app_update.rs', import.meta.url), 'utf8');
  expect(startup).toContain('const AUTO_CHECK_UPDATE_DELAY_MS: u64 = 5_000;');
  expect(startup).toContain('spawn_auto_check_update(app.clone());');
  expect(startup).toContain('config.get("autoCheckUpdate").and_then(Value::as_bool) != Some(false)');
  const chain = rustBody(startup, 'fn spawn_auto_check_update(');
  expect(chain).toMatch(/commands::update_check\(app.clone\(\), state, Some\(include_prerelease\), None\)/);
  expect(chain).not.toContain('update_clear_portable_handoff');
  const check = rustBody(command, 'pub async fn update_check(');
  expect(check).toContain('fetch_releases_json(&state, owner, repo).await');
  expect(check).not.toMatch(/clear_portable_handoff|mutate_state|pending_portable_update\s*=/);
  const clear = rustBody(command, 'pub fn update_clear_portable_handoff(');
  expect(clear).toContain('state.updater().clear_portable_handoff()');
  expect(clear).not.toContain('fetch_releases_json');
});
