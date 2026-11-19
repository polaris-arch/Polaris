// Source contracts and an inert Apple build fixture. No Apple SDK is needed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, mkdirSync, mkdtempSync, openSync, readFileSync, readlinkSync, realpathSync, rmSync, statSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { nativeTunCompilerResponse } from './mobile-core/apple-admission.mjs';

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
  assert.match(platform, /pending == nil && !nativeTun\.hasSessions/);
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
  assert.match(apply, /owner\.settingsTimedOut\(token, error: error\)/);
  assert.match(apply, /guard owner\.acceptsSettings\(token\)/);
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


test('native bridge retains real NE callbacks and PacketTunnel consumes the compiler response before Sources', () => {
  const apply = section(platform, '    private func apply(', '    func revokeNativeTun()');
  assert.match(apply, /let owner = provider/);
  assert.match(apply, /settingsCallbacks\[requestID\] = \(token, result\)/);
  assert.match(apply, /\[self, owner, network, result\]/);
  assert.match(apply, /withExtendedLifetime\(\(owner, network, result\)\)/);
  assert.ok(apply.indexOf('settingsCallbacks.removeValue') < apply.indexOf('result.finish(error)'));
  assert.doesNotMatch(apply, /guard self\.provider\.acceptsSettings/);
  assert.match(section(provider, '#if POLARIS_NATIVE_TUN_V1', '#endif'), /try server\.setRuntimeTunPlatform\(self\.platform\)/);
  assert.ok(provider.indexOf('#endif', provider.indexOf('#if POLARIS_NATIVE_TUN_V1')) < provider.indexOf('try server.start()'));
  assert.match(section(platform, '#if POLARIS_NATIVE_TUN_V1', '#endif'), /extension TunnelPlatform: LibboxRuntimeTunPlatformProtocol/);
  assert.match(platform, /previous: old\?\.state, published: \{ receipt\.published\(\) \}/);
  assert.match(platform, /private let provider: PacketTunnelProvider/);
  assert.match(platform, /let custody = Darwin\.dup\(original\)/);
  assert.match(platform, /Darwin\.close\(custody\)/);
  assert.doesNotMatch(platform, /Darwin\.close\(original\)/);

  const project = readFileSync(new URL('../src-tauri/gen/apple/project.yml', import.meta.url), 'utf8');
  const pbx = readFileSync(new URL('../src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj', import.meta.url), 'utf8');
  for (const text of [project, pbx]) {
    assert.match(text, /OTHER_SWIFT_FLAGS/);
    assert.ok(text.includes('@$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args'));
    assert.doesNotMatch(text, /-D POLARIS_NATIVE_TUN_V1/);
  }
  const target = section(pbx, '78552A5C57D5DDA9797F1E67 /* polaris_PacketTunnel */ =', 'buildRules');
  assert.match(target, /Verify iOS Core[\s\S]*Sources/);
  const phase = section(pbx, 'B395AB4CF19394B13456AAB7 /* Verify iOS Core */ =', '6BA4F5A019704F91829A51D3');
  assert.match(phase, /alwaysOutOfDate = 1/);
  assert.ok(phase.includes('$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args'));
  assert.equal(pbx.split('OTHER_SWIFT_FLAGS = ("$(inherited)", "@$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args",);').length - 1, 2);
});

// Inert declarations from the pinned gomobile ABI fixture, including the
// nullable direct return. These sources never import or execute Libbox.
const tunPlatform = '- (int32_t)runtimeTunVersion;\n'
  + '- (id<LibboxRuntimeTunSession> _Nullable)newTunSession:(id<LibboxTunOptions> _Nullable)options previous:(id<LibboxRuntimeTunSession> _Nullable)previous receipt:(LibboxRuntimeTunReceipt* _Nullable)receipt error:(NSError* _Nullable* _Nullable)error;\n';
const tunSession = '- (NSString* _Nonnull)ownerID;\n'
  + '- (BOOL)prepare:(NSError* _Nullable* _Nullable)error;\n'
  + '- (int32_t)fileDescriptor;\n'
  + '- (BOOL)validate:(NSError* _Nullable* _Nullable)error;\n'
  + '- (BOOL)abort:(NSError* _Nullable* _Nullable)error;\n'
  + '- (BOOL)retire:(NSError* _Nullable* _Nullable)error;\n';
const oldHeader = 'NSString* LibboxVersion(void);';
const tunHeader = '@protocol LibboxRuntimeTunPlatform;\n'
  + '@protocol LibboxRuntimeTunSession;\n'
  + '@class LibboxRuntimeTunReceipt;\n'
  + '@protocol LibboxRuntimeTunPlatform <NSObject>\n' + tunPlatform + '@end\n'
  + '@protocol LibboxRuntimeTunSession <NSObject>\n' + tunSession + '@end\n'
  + '@interface LibboxRuntimeTunPlatform : NSObject <goSeqRefInterface, LibboxRuntimeTunPlatform> {\n}\n'
  + tunPlatform + '@end\n'
  + '@interface LibboxRuntimeTunSession : NSObject <goSeqRefInterface, LibboxRuntimeTunSession> {\n}\n'
  + tunSession + '@end\n'
  + '@interface LibboxRuntimeTunReceipt : NSObject <goSeqRefInterface> {\n}\n'
  + '- (NSString* _Nonnull)ownerID;\n- (BOOL)published;\n@end\n'
  + '@interface LibboxCommandServer : NSObject <goSeqRefInterface> {\n}\n'
  + '- (BOOL)setRuntimeTunPlatform:(id<LibboxRuntimeTunPlatform> _Nullable)platform error:(NSError* _Nullable* _Nullable)error;\n@end\n';
const tunDefine = '-D\nPOLARIS_NATIVE_TUN_V1\n';
const responseName = 'PolarisNativeTunCompiler.args';

function tunDirectories(t) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), 'polaris-native-tun-args-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const prepared = join(root, 'parent-prepared'), derived = join(root, 'derived');
  mkdirSync(prepared);
  mkdirSync(derived);
  return { root, prepared, derived };
}

test('complete native TUN declarations produce the exact compiler response and an old ABI produces empty bytes', t => {
  const input = tunDirectories(t);
  const cases = [[oldHeader, false], ['/*' + tunHeader + '*/', false], [tunHeader, true],
    ['#ifndef __Libbox_H__\n#define __Libbox_H__\n' + tunHeader + '#endif\n', true],
    ['#ifndef __Libbox_H__\u0085\n#define __Libbox_H__\u0085\n' + tunHeader + '#endif\u0085\n', true],
    [tunHeader.replaceAll('fileDescriptor;', 'fileDescriptor\u0085;'), true]];
  for (const [header, present] of cases) {
    const expected = present ? tunDefine : '';
    const receipt = nativeTunCompilerResponse('prepare', { ...input, headers: [header] });
    assert.equal(readFileSync(join(input.prepared, responseName), 'utf8'), expected);
    assert.deepEqual(receipt, {
      contractVersion: 'polaris-native-tun-v1',
      present,
      compilerArgs: { bytes: Buffer.byteLength(expected), sha256: createHash('sha256').update(expected).digest('hex') },
    });
  }
});

test('partial, wrong, duplicate or disabled native TUN declarations fail closed', t => {
  const cases = [
    '@protocol LibboxRuntimeTunPlatform;',
    tunHeader.replace('- (BOOL)published;', ''),
    tunHeader.replace('newTunSession:', 'newTunSessionWrong:'),
    tunHeader.replace('id<LibboxRuntimeTunSession> _Nullable)newTunSession:', 'BOOL)newTunSession:'),
    tunHeader.replace('- (int32_t)fileDescriptor;', '- (int64_t)fileDescriptor;'),
    tunHeader.replace('fileDescriptor;', 'fileDescriptor\uFEFF;'),
    tunHeader.replace('@end', '@endé'),
    tunHeader.replace('- (BOOL)prepare:(NSError* _Nullable* _Nullable)error;',
      '- (BOOL)prepare:(NSError* _Nullable* _Nullable)error;\n- (void)prepare:(NSError* _Nullable* _Nullable)error;'),
    tunHeader + '@protocol LibboxRuntimeTunSession <NSObject>\n' + tunSession + '@end\n',
    '#if 0\n' + tunHeader + '#endif\n',
    '#ifdef POLARIS_NATIVE_TUN_V1\n' + tunHeader + '#endif\n',
    '#ifndef __Libbox_H__\uFEFF\n#define __Libbox_H__\n' + tunHeader + '#endif\n',
  ];
  const input = tunDirectories(t);
  for (const header of cases) {
    assert.throws(() => nativeTunCompilerResponse('prepare', { ...input, headers: [header] }), /native TUN/);
  }
  assert.throws(() => readFileSync(join(input.prepared, responseName)), { code: 'ENOENT' });
});

test('the Apple build CLI binds the native TUN response to each selected device and simulator slice', async t => {
  const sdks = [['iphoneos', 'ios-arm64'], ['iphonesimulator', 'ios-arm64-simulator']];
  const mobileCore = fileURLToPath(new URL('./mobile-core/', import.meta.url));
  const source = JSON.parse(readFileSync(new URL('./mobile-core/source-contract.json', import.meta.url)));
  const receiptName = 'PolarisTLSFingerprintLinker.json';
  const linkerName = 'PolarisTLSFingerprintLinker.args';
  const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
  for (const [platform, selected] of sdks) {
    for (const present of [true, false]) {
      await t.test(`${platform} ${present ? 'native TUN V1' : 'legacy'}`, t => {
        const { root, prepared, derived } = tunDirectories(t);
        const linkedScripts = join(root, 'mobile-core');
        symlinkSync(mobileCore, linkedScripts);
        const script = join(linkedScripts, 'apple_tls_fingerprint.py');
        const framework = join(root, 'Libbox.xcframework');
        const slices = {};
        for (const [, identifier] of sdks) {
          const folder = join(framework, identifier, 'Libbox.framework');
          const version = join(folder, 'Versions', 'A');
          const binary = Buffer.from(`inert selected static archive ${identifier}`);
          mkdirSync(join(version, 'Headers'), { recursive: true });
          writeFileSync(join(version, 'Libbox'), binary);
          writeFileSync(join(version, 'Headers', 'Libbox.h'), (identifier === selected) === present ? tunHeader : oldHeader);
          symlinkSync('A', join(folder, 'Versions', 'Current'));
          symlinkSync('Versions/Current/Libbox', join(folder, 'Libbox'));
          symlinkSync('Versions/Current/Headers', join(folder, 'Headers'));
          slices[identifier] = { libraryIdentifier: identifier, binary: { bytes: binary.length, sha256: sha256(binary) } };
        }
        const link = join(root, 'link.json');
        writeFileSync(link, JSON.stringify({ framework, componentOutput: { slices } }));
        const executable = Buffer.from('inert final PacketTunnel executable');
        writeFileSync(join(root, 'extension'), executable);
        // Substitute only xcrun. The fixture's nm accepts the selected archive and never invokes native tools.
        writeFileSync(join(root, 'xcrun'), '#!/bin/sh\n'
          + 'test "$#" -eq 4 && test "$1 $2 $3" = "nm --no-llvm-bc -gU" && test "$4" = "$FIXTURE_BINARY" || exit 97\n'
          + 'printf "T _LibboxVersion\\n"\n', { mode: 0o700 });

        const budget = join(root, 'budget');
        mkdirSync(budget, { mode: 0o700 });
        const slot = sha256(root) + '.reservation';
        const ledger = join(budget, slot);
        // The native CLI checks and locks this inherited descriptor without allocating a build's disk reserve.
        writeFileSync(ledger, JSON.stringify({ schema: 'polaris-mobile-filesystem-reservation-v1', slot, work: root,
          filesystem: statSync(root).dev, reserveBytes: source.budget.buildPeakReserveBytes, groups: [] }), { mode: 0o600 });
        const fd = openSync(ledger, 'r+');
        t.after(() => closeSync(fd));
        const env = { ...process.env, PATH: `${root}:${process.env.PATH}`, PLATFORM_NAME: platform, ARCHS: 'arm64',
          POLARIS_MOBILE_LINK_INPUT: link, POLARIS_MOBILE_LINK_INPUT_SHA256: sha256(readFileSync(link)),
          POLARIS_APP_WORK: root, POLARIS_BUILD_BUDGET_ROOT: budget, POLARIS_FORMAL_APP_BUILD: '1',
          FIXTURE_BINARY: join(framework, selected, 'Libbox.framework', 'Versions', 'A', 'Libbox'),
          TARGET_BUILD_DIR: root, EXECUTABLE_PATH: 'extension' };
        const invoke = (action, env, parent = false) => {
          const result = spawnSync('python3', ['-B', script, action], { encoding: 'utf8', timeout: 30_000,
            env: { ...env, POLARIS_BUILD_RESERVATION_FD: parent ? '3' : '999999' },
            stdio: ['ignore', 'pipe', 'pipe', parent ? fd : 'ignore'] });
          assert.ifError(result.error);
          return result;
        };

        const parentEnv = { ...env, DERIVED_FILE_DIR: prepared };
        const preparation = invoke('prepare', parentEnv, true);
        assert.equal(preparation.status, 0, preparation.stderr);
        const expected = Buffer.from(present ? tunDefine : '');
        const receipt = readFileSync(join(prepared, receiptName));
        assert.deepEqual(JSON.parse(receipt).nativeTun, { contractVersion: 'polaris-native-tun-v1', present,
          compilerArgs: { bytes: expected.length, sha256: sha256(expected) } });
        assert.deepEqual(readFileSync(join(prepared, responseName)), expected);
        const pins = { POLARIS_TLS_PREPARED_ROOT: prepared, POLARIS_TLS_PREPARED_SHA256: sha256(receipt) };
        const hookEnv = { ...env, ...pins, DERIVED_FILE_DIR: derived, FIXTURE_BINARY: '' };
        const installed = join(derived, responseName);
        writeFileSync(installed, present ? '' : tunDefine);
        const installation = invoke('install', hookEnv);
        assert.equal(installation.status, 0, installation.stderr);
        assert.deepEqual(readFileSync(installed), expected);
        assert.deepEqual(readFileSync(join(derived, receiptName)), receipt);
        assert.deepEqual(readFileSync(join(derived, linkerName)), readFileSync(join(prepared, linkerName)));
        const installedVerification = invoke('verify-installed', hookEnv);
        assert.equal(installedVerification.status, 0, installedVerification.stderr);
        const finalVerification = invoke('verify', { ...parentEnv, ...pins }, true);
        assert.equal(finalVerification.status, 0, finalVerification.stderr);
        assert.equal(JSON.parse(finalVerification.stdout).nativeTunHeaderABIAvailable, present);

        writeFileSync(installed, present ? tunDefine.replace('_V1', '_XX') : tunDefine);
        const tamperedInstallation = invoke('verify-installed', hookEnv);
        assert.equal(tamperedInstallation.status, 1, tamperedInstallation.stderr);
        assert.match(tamperedInstallation.stderr, /installed native TUN compiler input changed/);

        const alteredReceipt = JSON.parse(receipt);
        alteredReceipt.nativeTun.present = !present;
        const alteredBytes = JSON.stringify(alteredReceipt);
        const preparedReceipt = join(prepared, receiptName);
        writeFileSync(preparedReceipt, alteredBytes);
        const alteredPin = { POLARIS_TLS_PREPARED_SHA256: sha256(alteredBytes) };
        const tamperedFinal = invoke('verify', { ...parentEnv, ...pins, ...alteredPin }, true);
        assert.equal(tamperedFinal.status, 1, tamperedFinal.stderr);
        const abiMismatch = /prepared native TUN ABI decision differs from actual headers/;
        assert.match(tamperedFinal.stderr, abiMismatch);

        if (!present) return;
        const prior = Buffer.from('prior installed bytes');
        const derivedFiles = [installed, join(derived, linkerName), join(derived, receiptName)];
        for (const path of derivedFiles) writeFileSync(path, prior);
        const rejectsInstall = (reason, inputs = hookEnv) => {
          const result = invoke('install', inputs);
          assert.equal(result.status, 1, result.stderr);
          assert.match(result.stderr, reason);
          for (const path of derivedFiles) assert.deepEqual(readFileSync(path), prior);
        };

        rejectsInstall(abiMismatch, { ...hookEnv, ...alteredPin });
        alteredReceipt.nativeTun.present = present;
        alteredReceipt.nativeTun.unrecognized = true;
        const extraField = JSON.stringify(alteredReceipt);
        writeFileSync(preparedReceipt, extraField);
        rejectsInstall(abiMismatch, { ...hookEnv, POLARIS_TLS_PREPARED_SHA256: sha256(extraField) });
        writeFileSync(preparedReceipt, receipt);

        const parentArgs = join(prepared, responseName);
        writeFileSync(parentArgs, '');
        rejectsInstall(/parent native TUN compiler response file changed/);
        writeFileSync(parentArgs, expected);

        const foreign = join(root, 'foreign-args');
        writeFileSync(foreign, prior);
        unlinkSync(installed);
        symlinkSync(foreign, installed);
        rejectsInstall(/native compiler output redirected/);
        assert.equal(readlinkSync(installed), foreign);
        assert.deepEqual(readFileSync(foreign), prior);
      });
    }
  }
});

test('native TUN input drift or redirected output leaves derived and foreign bytes intact', t => {
  const faults = [
    [input => writeFileSync(join(input.prepared, responseName), ''), /parent native TUN compiler response file changed/],
    [input => { input.headers = [tunHeader.replace('- (BOOL)published;', '')]; }, /native TUN selector/],
    [input => { input.receipt.present = false; }, /prepared native TUN ABI decision differs/],
    [input => { input.receipt.unrecognized = true; }, /prepared native TUN ABI decision differs/],
  ];
  for (const [fault, reason] of faults) {
    const input = { ...tunDirectories(t), headers: [tunHeader] };
    input.receipt = nativeTunCompilerResponse('prepare', input);
    const installed = join(input.derived, responseName);
    writeFileSync(installed, 'prior response');
    fault(input);
    assert.throws(() => nativeTunCompilerResponse('install', input), reason);
    assert.equal(readFileSync(installed, 'utf8'), 'prior response');
  }
  const input = { ...tunDirectories(t), headers: [tunHeader] };
  input.receipt = nativeTunCompilerResponse('prepare', input);
  const foreign = join(input.derived, 'foreign-args');
  writeFileSync(foreign, 'untouched');
  symlinkSync(foreign, join(input.derived, responseName));
  assert.throws(() => nativeTunCompilerResponse('install', input), /compiler output redirected/);
  assert.equal(readFileSync(foreign, 'utf8'), 'untouched');
});

test('native transaction keeps exact custody, publication lease and sticky failed restoration', () => {
  const native = lifecycle.slice(lifecycle.indexOf('final class TunnelNativeTunOwner'));
  assert.match(native, /current === previous/);
  assert.match(native, /pending === session/);
  assert.match(native, /lastEffect === session/);
  assert.match(native, /session\.published\(\)/);
  assert.match(native, /if error is TunnelSettingsTimeout \|\| !accepts\(session\.token\)/);
  assert.match(native, /previous\.file = restored/);
  assert.match(native, /retainDebt\(session, error\)/);
  const prepare = section(native, '    func prepare(', '    func descriptor(');
  assert.ok(prepare.indexOf('try old.close()') < prepare.indexOf('try apply(session.network'));
  const abort = section(native, '    func abort(', '    func retire(');
  assert.ok(abort.indexOf('try candidate.close()') < abort.indexOf('try apply(session.snapshot'));
  assert.match(provider, /platform\.revokeNativeTun\(\)/);
  assert.match(provider, /if let nativeFailure = platform\.reset\(\)/);
  const completion = section(lifecycle, 'final class TunnelSettingsCompletion', 'struct TunnelSettingsTimeout');
  assert.match(completion, /if pending \{[\s\S]*waitTimedOut = true/);
  assert.doesNotMatch(completion.slice(completion.indexOf('func wait(')), /pending = false/);
});
