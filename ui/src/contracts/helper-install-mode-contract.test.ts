import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { maskRustComments, moduleSource } from './rust-source.test-support';

// Source-only wiring contracts supplement the Rust unit tests. They do not
// establish the macOS transaction's runtime behavior or authorize a native build.
function body(source: string, start: string, end: string): string {
  const from = source.indexOf(start);
  expect(from, `missing start: ${start}`).toBeGreaterThanOrEqual(0);
  const to = source.indexOf(end, from + start.length);
  expect(to, `missing end: ${end}`).toBeGreaterThan(from);
  return source.slice(from, to);
}
function before(source: string, first: string, second: string) {
  expect(source.indexOf(first), `missing ${first}`).toBeGreaterThanOrEqual(0);
  expect(source.indexOf(second), `missing ${second}`).toBeGreaterThanOrEqual(0);
  expect(source.indexOf(first)).toBeLessThan(source.indexOf(second));
}
const manager = maskRustComments(moduleSource('crates/helper-client/src/manager'));
const runtime = maskRustComments(moduleSource('src-tauri/src/runtime/helper'));
const command = maskRustComments(moduleSource('src-tauri/src/commands/helper'));

it('ordinary manager and runtime entries delegate without inventing migration intent', () => {
  const managerEntry = body(manager, 'pub fn install(', 'pub fn install_with_mode(');
  expect(managerEntry).toContain('self.install_with_mode(params, executor, InstallMode::Ordinary)');
  expect(managerEntry).not.toContain('StopAndUpgrade');
  const runtimeEntry = body(runtime, 'pub fn install(', 'pub fn install_with_mode(');
  expect(runtimeEntry).toContain('self.install_with_mode(HelperInstallMode::Ordinary)');
  expect(runtimeEntry).not.toContain('StopAndUpgrade');
});
it('explicit manager mode is rejected before input validation, token writes, and elevation', () => {
  const install = body(manager, 'pub fn install_with_mode(', 'pub fn uninstall(');
  for (const effect of ['validate_install_params(', 'self.sysops.exists(', 'self.prepare_token()', 'self.run_privileged_script(']) {
    before(install, 'if !mode.supported_on(self.platform)', effect);
  }
  expect(install).toContain('return Err(ManagerError::UnsupportedPlatform(self.platform))');
  expect(install).toContain('self.build_install_script(params, &token, mode)?');
  expect(install).not.toContain('InstallMode::StopAndUpgrade');
  const dispatch = body(manager, 'fn build_install_script(', 'fn build_uninstall_script(');
  expect(dispatch).toContain('InstallMode::Ordinary => build_mac_install_script(&self.paths, params, token)');
  expect(dispatch).toMatch(/InstallMode::StopAndUpgrade\s*=>\s*(?:\{\s*)?build_mac_migration_script\(&self\.paths, params, token\)/);
});
it('runtime and IPC reject unsupported mode before any staging or proxy-stop guard', () => {
  const install = body(runtime, 'pub fn install_with_mode(', 'pub fn uninstall(');
  for (const effect of ['self.install_params()', 'self.reserve_recovery()', 'manager.install_with_mode(']) {
    before(install, 'if !mode.supported_on(self.platform)', effect);
  }
  expect(install).toContain('mode.manager_mode()');
  const ipc = body(command, 'pub async fn helper_install(', 'const fn helper_install_blocked_by_proxy(');
  expect(ipc).toContain('mode: Option<HelperInstallMode>');
  expect(ipc).toContain('mode.unwrap_or_default()');
  before(ipc, 'mode.supported_on(helper.platform())', 'with_helper_service_mutation_core_guard(');
  expect(ipc).toContain('mode == HelperInstallMode::Ordinary');
  expect(ipc).toContain('helper_install_blocked_by_proxy(');
  expect(ipc).toContain('helper.install_with_mode(mode)');
  expect(ipc).toContain('mode == HelperInstallMode::StopAndUpgrade');
  expect(ipc).toContain('with_linux_legacy_mutation_guard(');
});
it.each(['en-US', 'zh-CN', 'zh-TW', 'ru', 'fa'])('explicit migration confirmation is localized in %s', locale => {
  const { helper } = JSON.parse(readFileSync(new URL(`../i18n/locales/${locale}.json`, import.meta.url), 'utf8'));
  for (const key of ['stopAndUpgradeAction', 'stopAndUpgradeTitle', 'stopAndUpgradeWarning', 'stopAndUpgradeConfirm', 'linuxStopAndUpgradeTitle', 'linuxStopAndUpgradeWarning', 'linuxForwardOnlyRecovery']) {
    expect(helper[key]).toEqual(expect.any(String));
    expect(helper[key].length).toBeGreaterThan(5);
  }
  expect(helper.stopAndUpgradeTitle).toContain('com.polaris.helper');
  expect(helper.stopAndUpgradeWarning).toContain('com.polaris.helper');
});

it('Linux legacy admission consumes the real captured stop and both normal user-domain and root readbacks', () => {
  const guard = body(command, 'async fn with_linux_legacy_mutation_guard<F, Fut>(', 'pub async fn helper_uninstall(');
  before(guard, 'status_with_owned_stop_context()', 'stop_expected_for_linux_helper_maintenance(request)');
  before(guard, 'stop_expected_for_linux_helper_maintenance(request)', 'body(original).await');
  before(guard, 'linux_proxy_restored(&readback)', 'body(original).await');
  expect(guard).toContain('join_linux_watchdog_to_terminal(');
  expect(guard).toContain('completed.load(Ordering::SeqCst)');
  expect(guard).toContain('valid.load(Ordering::SeqCst)');
  expect(guard).not.toContain('proxy.stop().await');
  const native = maskRustComments(moduleSource('crates/helper/src/linux_legacy_maintenance'));
  expect(native).toContain('polaris_helper_client::manager::LINUX_SERVICE_CLOSURE_FUNCTIONS');
  expect(native).toContain('current_sealed_utility()?');
  expect(native).toContain('preserve_existing: true');
  const activation = body(native, 'fn activate(', 'fn remove_business_tree(');
  before(activation, 'journal.set_phase(Phase::CommittedActivationPending)?', 'publish(\n            journal,\n            "candidate-unit",');
  expect(activation).not.toContain('"restart"');
});
