// Source contracts only. These checks do not run Swift, NE, or an Apple device.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
const read = name => readFileSync(new URL(`../src-tauri/gen/apple/PacketTunnel/${name}.swift`, import.meta.url), 'utf8');
const platform = read('TunnelPlatform');
const lifecycle = read('TunnelLifecycle');
const provider = read('PacketTunnelProvider');
const section = (s, start, end) => s.slice(s.indexOf(start), s.indexOf(end, s.indexOf(start)));
const toggle = section(platform, '    func setSystemProxyEnabled', '    func triggerNativeCrash');

test('proxy toggle mutates only separately copied network and proxy candidates', () => {
  assert.match(toggle, /settings\.copy\(\) as\? NEPacketTunnelNetworkSettings/);
  assert.match(toggle, /retainedProxy\.copy\(\) as\? NEProxySettings/);
  assert.match(toggle, /candidate\.proxySettings = proxy/);
  assert.doesNotMatch(toggle, /retainedProxy\.(httpEnabled|httpsEnabled)\s*=/);
  assert.ok(toggle.indexOf('try apply(candidate, token: token)') < toggle.indexOf('try publish(candidate,'));
});
test('publication combines generation acceptance and cached identity/reset validation', () => {
  const publish = section(platform, '    private func publish(', '    private func apply(');
  assert.match(publish, /provider\.publishSettings\(token\)/);
  assert.match(publish, /self\.stateLock\.lock\(\); defer \{ self\.stateLock\.unlock\(\) \}/);
  assert.ok(publish.indexOf('self.settingsReset == reset, self.settings === previous') < publish.indexOf('self.settings = network'));
  assert.doesNotMatch(publish, /setTunnelNetworkSettings|try apply\(/);
  const gate = section(lifecycle, '    func publishSettings(', '    @discardableResult\n    func complete(');
  assert.match(gate, /lock\.lock\(\); defer \{ lock\.unlock\(\) \}/);
  assert.ok(gate.indexOf('generation == token && !quarantined && !stopPending') < gate.indexOf('return publication()'));
  assert.match(provider, /lifecycle\.publishSettings\(token, publication: publication\)/);
});
test('reset changes identity even when cached settings were already nil', () => {
  assert.match(platform, /settingsReset = UUID\(\); settings = nil/);
  assert.match(platform, /try publish\(network, replacing: previous, reset: reset, token: token\)/);
});
test('DNS reset keeps one generation and replays the retained accepted snapshot', () => {
  const dns = section(platform, '    func clearDNSCache()', '    func findConnectionOwner');
  assert.equal((dns.match(/settingsGeneration\(\)/g) ?? []).length, 1);
  assert.match(dns, /try apply\(nil, token: token\)/);
  assert.match(dns, /settingsReset == reset && self\.settings === settings/);
  assert.match(dns, /try apply\(settings, token: token\)/);
  assert.doesNotMatch(dns, /\.httpEnabled\s*=/);
});
test('apply preserves the existing timeout quarantine and never publishes by itself', () => {
  const apply = section(platform, '    private func apply(', '    func usePlatformAutoDetectControl');
  assert.doesNotMatch(apply, /settingsGeneration\(\)|self\.settings\s*=/);
  assert.match(apply, /provider\.settingsTimedOut\(token, error: error\)/);
  assert.match(apply, /guard provider\.acceptsSettings\(token\)/);
});

test('runtime receipt retains the exact owner without pretending memory is crash durability', () => {
  const owner = provider.slice(provider.indexOf('private final class TunnelRuntimeRestartOwner'));
  assert.match(owner, /func runtimePersistenceMode\(\) -> String \{ "app_authoritative_regeneration" \}/);
  assert.match(owner, /digest\(configContent\) == effectiveDigest/);
  assert.match(owner, /guard !revoked, self\.incarnation == nil \|\| self\.incarnation == incarnation/);
  assert.match(owner, /revision >= self\.revision/);
  assert.match(owner, /lifecycle\.publishSettings\(token, publication:/);
  assert.doesNotMatch(owner, /\.sync\s*\{|startOrReloadService|write\(to:|saveToPreferences/);
  const start = section(provider, '    private func startRuntimeOwnedService(', '    private func sharedDirectory()');
  assert.ok(start.indexOf('runtimeRestartOwner?.revoke()') < start.indexOf('let owner = TunnelRuntimeRestartOwner'));
  assert.match(start, /try server\.startOrReloadService\(withRetryableRuntimePersistence: config, options: LibboxOverrideOptions\(\), handler: owner\)/);
  // The Objective-C/Java selector is not the imported Swift spelling.
  assert.doesNotMatch(start, /startOrReloadServiceWith(?:Retryable)?RuntimePersistence|startOrReloadService\(config/);
  assert.match(start, /catch \{\s*owner\.revoke\(\)/);
  assert.match(provider, /let config = self\.runtimeRestartOwner\?\.snapshot\(\)/);
});

test('managed bootstrap preserves digest and directory admission before the new owner start', () => {
  const managed = section(provider, '    private func handleManagedBootstrap(', '    override func sleep(');
  const digest = managed.indexOf('digest(config) == identity.configDigest');
  const activate = managed.indexOf('server.activateTailscaleDirectoryDomain(pending.ownerNonce, expectedSourceIdentity: expectedSource)');
  const start = managed.indexOf('startRuntimeOwnedService(server, config: config, token: pending.token)');
  assert.ok(digest >= 0 && digest < activate && activate < start);
  assert.match(managed, /capability\["sourceIdentity"\] as\? String == expectedSource/);
  assert.match(managed, /lifecycle\.accepts\(pending\.token\)/);
});


test('reload invalidates the previous callback generation before reading its retained source', () => {
  const reload = section(provider, '    func enqueueReload()', '    func publishSettings(');
  const claim = reload.indexOf('let token = try lifecycle.reload()');
  const snapshot = reload.indexOf('self.runtimeRestartOwner?.snapshot()');
  assert.ok(claim >= 0 && claim < snapshot);
  const gate = section(lifecycle, '    func reload()', '    func prepareStop(');
  assert.match(gate, /lock\.lock\(\); defer \{ lock\.unlock\(\) \}/);
  assert.match(gate, /generation \+= 1/);
  const owner = provider.slice(provider.indexOf('private final class TunnelRuntimeRestartOwner'));
  assert.match(owner, /lifecycle\.publishSettings\(token, publication:/);
});

test('the real capabilities bridge gates initial, managed and reload readiness', () => {
  const start = section(provider, '    private func startRuntimeOwnedService(', '    private func sharedDirectory()');
  const birth = start.indexOf('server.startOrReloadService(withRetryableRuntimePersistence:');
  const query = start.indexOf('server.runtimeCapabilities(&capabilityError)');
  const decode = start.indexOf('TunnelRuntimeAdmission.decode(document, platform: platformBuild)');
  const bind = start.indexOf('lifecycle.admitRuntimeProtocol(token, admission: admission)');
  assert.ok(birth >= 0 && birth < query && query < decode && decode < bind);
  assert.match(start, /if let capabilityError \{ throw capabilityError \}/);
  assert.equal((start.match(/lifecycle\.accepts\(token\), commandServer === server, runtimeRestartOwner === owner/g) ?? []).length, 2);
  assert.match(start, /platformBuild = "ios\/arm64"/);
  assert.doesNotMatch(start, /dlsym|unsafeBitCast|perform\(|NSSelectorFromString|#if DEBUG/);
  assert.equal((provider.match(/(?:self\.)?startRuntimeOwnedService\(server, config:/g) ?? []).length, 3);
  assert.equal((provider.match(/guard (?:self\.)?lifecycle\.complete\([^\n]+phase: "running"/g) ?? []).length, 3);
});

test('ready publication and admission are generation-atomic against Stop and timeout', () => {
  const admission = section(lifecycle, '    func admitRuntimeProtocol(', '    @discardableResult\n    func complete(');
  assert.match(admission, /lock\.lock\(\); defer \{ lock\.unlock\(\) \}/);
  assert.match(admission, /generation == token, !quarantined, !stopPending/);
  assert.match(admission, /phase == "starting" \|\| phase == "reloadAccepted"/);
  const complete = section(lifecycle, '    func complete(', '    @discardableResult\n    func timeout(');
  const guard = complete.indexOf('phase == "running" && (quarantined || stopPending || runtimeAdmission == nil)');
  assert.ok(guard >= 0 && guard < complete.indexOf('self.phase ='));
  for (const [begin, end] of [['    func start(', '    func stop()'], ['    func stop()', '    func reload()'], ['    func reload()', '    func prepareStop(']]) {
    assert.match(section(lifecycle, begin, end), /runtimeAdmission = nil/);
  }
  const reload = section(provider, '    func enqueueReload()', '    func settingsGeneration()');
  assert.match(reload, /if self\.lifecycle\.complete\(token, phase: "failed"/);
});

test('native capability decoding separates static protocol and dynamic graph state', () => {
  const decoder = lifecycle.slice(lifecycle.indexOf('struct TunnelRuntimeAdmission'));
  for (const name of ['GetRuntimeCapabilities', 'PlanRuntimeConfig', 'ApplyRuntimeConfig',
    'GetRuntimeConfigStatus', 'RetryRuntimePersistence', 'AcknowledgeRuntimePersistence']) assert.ok(decoder.includes(`"${name}"`));
  assert.match(decoder, /protocols\.count == 1/);
  assert.match(decoder, /entry\.variant == "polaris-daily-runtime-v1", entry\.adapter_version == "1"/);
  assert.match(decoder, /entry\.platform_build == platform, entry\.conditions == methods/);
  assert.match(decoder, /entry\.update_class == nil \|\| entry\.update_class == "RUNTIME_UPDATE_UNSPECIFIED"/);
  assert.match(decoder, /case "RUNTIME_STARTED_UNSUPPORTED":/);
  assert.match(decoder, /guard !startedUnsupported, paths == \["runtime"\]/);
  assert.match(decoder, /case "PENDING_ADAPTER":/);
  assert.match(decoder, /guard startedUnsupported, !unsupported\.isEmpty, incarnation\.isEmpty/);
  assert.match(decoder, /case "OWNER_PERSISTENCE_UNBOUND":/);
  assert.match(decoder, /paths == \["runtime.persistence"\]/);
  assert.match(decoder, /default:\s*\/\/[^\n]+\n\s*throw lifecycleError\("Runtime instance is not ready"\)/);
  assert.doesNotMatch(decoder, /full_effective_schema_supported|connected_hot_enabled\s*==\s*true\s*\|\|/);
});

test('native protojson uint64 parsing preserves exact strings and omitted zero', () => {
  const decoder = lifecycle.slice(lifecycle.indexOf('struct TunnelRuntimeAdmission'));
  assert.match(decoder, /let applied_revision: String\?/);
  assert.match(decoder, /let control_revision: String\?/);
  assert.match(decoder, /let value = encoded \?\? "0"/);
  assert.match(decoder, /18446744073709551615/);
  assert.doesNotMatch(decoder, /\.doubleValue|\.uint64Value|Double\(|Float\(/);
  const owner = provider.slice(provider.indexOf('private final class TunnelRuntimeRestartOwner'));
  assert.match(owner, /admittedIncarnation == incarnation/);
  assert.match(owner, /admittedIncarnation = admission.identity\?\.incarnation/);
});
