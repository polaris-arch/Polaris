// Source contracts and an inert Apple build fixture. No Apple SDK is needed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, mkdirSync, mkdtempSync, openSync, readFileSync, readlinkSync, realpathSync, renameSync, rmSync, statSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { buildInfo, container } from './apple-carrier.mjs';
import { nativeTunCompilerResponse, sdkHeaderContracts } from './mobile-core/apple-admission.mjs';

function appleMachHeader(filetype, commands) {
  const header = Buffer.alloc(32);
  for (const [offset, value] of [[0, 0xfeedfacf], [4, 0x100000c], [12, filetype], [16, commands.length],
    [20, commands.reduce((bytes, command) => bytes + command.length, 0)]]) header.writeUInt32LE(value, offset);
  return header;
}

function appleBuildInfoSection() {
  const header = Buffer.alloc(32);
  header.write('\xff Go buildinf:', 'latin1');
  header[14] = 8;
  header[15] = 2;
  const version = Buffer.from('go1.27.4');
  const frame = Buffer.concat([
    Buffer.from('3077af0c9274080241e1c107e6d618e6', 'hex'),
    Buffer.from('path\texample.org/fixture\u0085build\tGOOS=ios\n'),
    Buffer.from('f932433186182072008242104116d8f2', 'hex'),
  ]);
  const inputs = [header, Buffer.from([version.length]), version, Buffer.from([frame.length]), frame];
  const bytes = inputs.reduce((size, part) => size + part.length, 0);
  return Buffer.concat(inputs, Math.ceil(bytes / 16) * 16);
}

function appleGoObject(platform) {
  const text = Buffer.from('\xff Go build ID: "source-bound-test"\n \xff', 'latin1');
  const info = appleBuildInfoSection();
  const version = Buffer.alloc(24);
  version.writeUInt32LE(0x32);
  version.writeUInt32LE(version.length, 4);
  version.writeUInt32LE(platform, 8);
  version.writeUInt32LE(15 << 16, 12);
  version.writeUInt32LE(27 << 16 | 2 << 8, 16);
  const segment = Buffer.alloc(72 + 2 * 80);
  const end = 32 + segment.length + version.length;
  const address = 9007199254740993n;
  segment.writeUInt32LE(0x19);
  segment.writeUInt32LE(segment.length, 4);
  for (const [offset, value] of [[24, address], [32, BigInt(text.length + info.length)],
    [40, BigInt(end)], [48, BigInt(text.length + info.length)]]) segment.writeBigUInt64LE(value, offset);
  segment.writeUInt32LE(2, 64);
  for (const [index, name, parent, payload, start] of [[0, '__text', '__TEXT', text, 0],
    [1, '__go_buildinfo', '__DATA', info, text.length]]) {
    const row = 72 + index * 80;
    segment.write(name, row);
    segment.write(parent, row + 16);
    segment.writeBigUInt64LE(address + BigInt(start), row + 32);
    segment.writeBigUInt64LE(BigInt(payload.length), row + 40);
    segment.writeUInt32LE(end + start, row + 48);
  }
  return Buffer.concat([appleMachHeader(1, [segment, version]), segment, version, text, info]);
}

function appleArchiveMember(name, payload) {
  const rawName = Buffer.from(name + '\0');
  const bytes = rawName.length + payload.length;
  const fields = [['#1/' + rawName.length, 16], [0, 12], [0, 6], [0, 6], [644, 8], [bytes, 10]];
  const header = Buffer.from(fields.map(([value, width]) => String(value).padEnd(width)).join('') + '\x60\n');
  return Buffer.concat([header, rawName, payload, Buffer.from(bytes % 2 ? '\n' : '')]);
}

test('Apple raw archive observations preserve slice, compiler bytes and exact Go provenance', () => {
  const observe = [];
  const callback = result => observe.push(result);
  const wrap = (archive, cpu = 0x100000c) => {
    const header = Buffer.alloc(32);
    for (const [offset, value] of [[0, 0xcafebabe], [4, 1], [8, cpu], [16, 32], [20, archive.length], [24, 3]]) {
      header.writeUInt32BE(value, offset);
    }
    return Buffer.concat([header, archive]);
  };
  for (const platform of [2, 7]) {
    const object = appleGoObject(platform);
    const symbols = Buffer.alloc(20);
    symbols.writeUInt32LE(8, 0);
    symbols.writeUInt32LE(4, 12);
    symbols.write('go\0', 16);
    const symbolName = '__.SYMDEF SORTED';
    const codeOffset = 8 + appleArchiveMember(symbolName, symbols).length;
    symbols.writeUInt32LE(codeOffset, 8);
    const ar = Buffer.concat([Buffer.from('!<arch>\n'), appleArchiveMember(symbolName, symbols),
      appleArchiveMember('original.o', object)]);
    const parsed = container(wrap(ar), callback);
    assert.deepEqual([parsed.kind, parsed.slices[0].offset, parsed.slices[0].archive.kind], ['fat-ar', 32, 'bsd-ar']);
    const [table, member] = parsed.slices[0].archive.members;
    assert.equal(table.classification, 'symbol-table');
    assert.equal(member.headerOffset, codeOffset);
    assert.equal(member.sha256, createHash('sha256').update(object).digest('hex'));
    assert.equal(member.machO.platformCommands[0].platform, platform);
    const [text, info] = member.machO.sections;
    assert.equal(text.address, 9007199254740993n);
    assert.deepEqual(text.buildIDs.map(note => [note.value, note.sectionOffset, note.fileOffset]),
      [['source-bound-test', 0, text.offset]]);
    assert.deepEqual(info.buildInfo, observe.at(-1));
    assert.equal(info.buildInfo.parserInput, 'observed-section: go1.27.4\n\tpath\texample.org/fixture\n\tbuild\tGOOS=ios\n');
    assert.throws(() => container(wrap(ar, 0x1000007), callback), /fat header\/payload CPU differs/);
    const dirtyPad = wrap(ar);
    dirtyPad[31] = 1;
    assert.throws(() => container(dirtyPad, callback), /fat padding/);
    const badBinding = Buffer.from(ar);
    badBinding.writeUInt32LE(codeOffset + 1, table.payloadOffset + 8);
    assert.throws(() => container(badBinding, callback), /BSD symbol\/member binding/);
    const badReloc = Buffer.from(object);
    badReloc.writeUInt32LE(32, 32 + 72 + 56);
    badReloc.writeUInt32LE(1, 32 + 72 + 60);
    assert.throws(() => container(badReloc, callback), /section relocation in headers/);
    const badIdentity = Buffer.from(object);
    badIdentity[text.offset] = 0;
    assert.throws(() => container(badIdentity, callback), /missing\/extra Go identity/);
  }
  const invalid = appleBuildInfoSection();
  invalid[32] = 128;
  invalid[33] = 0;
  assert.throws(() => buildInfo(invalid), /noncanonical inline varint/);
});

test('Apple raw executable receipts pin opaque signature bytes only inside the original linkedit segment', () => {
  const segment = Buffer.alloc(72);
  const signature = Buffer.alloc(16);
  const offset = 32 + segment.length + signature.length;
  const payload = Buffer.from('opaque native signature');
  segment.writeUInt32LE(0x19);
  segment.writeUInt32LE(segment.length, 4);
  segment.write('__LINKEDIT', 8);
  for (const [at, value] of [[32, payload.length], [40, offset], [48, payload.length]]) {
    segment.writeBigUInt64LE(BigInt(value), at);
  }
  signature.writeUInt32LE(0x1d);
  signature.writeUInt32LE(signature.length, 4);
  signature.writeUInt32LE(offset, 8);
  signature.writeUInt32LE(payload.length, 12);
  const executable = Buffer.concat([appleMachHeader(2, [segment, signature]), segment, signature, payload]);
  const parsed = container(executable);
  assert.deepEqual(parsed.loadCommands[1], { ordinal: 1, command: 0x1d, offset: 104, bytes: 16,
    dataOffset: offset, dataBytes: payload.length, payloadSha256: createHash('sha256').update(payload).digest('hex'),
    payloadKind: 'opaque-code-signature', trustValidated: false });
  executable.writeBigUInt64LE(BigInt(payload.length - 1), 32 + 48);
  assert.throws(() => container(executable), /code signature outside unique __LINKEDIT segment/);
});
// Source-derived gomobile v0.1.13 declarations, with the result spelling
// independently observed in the preserved original Libbox.objc.h.
const sdkCapability = '- (NSString* _Nonnull)runtimeCapabilities:(NSError* _Nullable* _Nullable)error;';
const sdkRetry = '- (BOOL)startOrReloadServiceWithRetryableRuntimePersistence:(NSString* _Nullable)configContent options:(LibboxOverrideOptions* _Nullable)options handler:(id<LibboxRuntimePersistenceHandler> _Nullable)handler error:(NSError* _Nullable* _Nullable)error;';
const sdkPersist = '- (BOOL)persistRuntimeConfiguration:(NSString* _Nullable)incarnation revision:(int64_t)revision effectiveDigest:(NSString* _Nullable)effectiveDigest configContent:(NSString* _Nullable)configContent error:(NSError* _Nullable* _Nullable)error;';
const sdkMode = '- (NSString* _Nonnull)runtimePersistenceMode;';
const sdkInit = '- (nonnull instancetype)init;';
const sdkGetters = ['getCleanup', 'getCleanupError', 'getConfigDigest', 'getContractVersion', 'getRequestID',
  'getTailscaleStoreMembership', 'getTailscaleStoreRetirement', 'getValidation', 'getValidationError'];
const sdkGetter = name => '- (NSString* _Nonnull)' + name + ';';
const sdkProductSymbols = ['NewManagedStrictCommandServer', 'exportTailscaleDirectoryCapability', 'activateTailscaleDirectoryDomain',
  'exportTailscaleDirectoryRetirement', 'NewTransientCommandServer', 'NewStrictCommandServer', 'InterfaceUpdateListenerIdentity',
  'CheckConfigWithResult', 'ConfigValidationContractVersion', 'exportTailscaleStoreRetirement', 'wakeNow', 'recordScreenState'];
const sdkDeclarations = sdkProductSymbols.map(name => 'void Libbox' + name + '(void);').join('\n');
const sdkResult = ['@interface LibboxConfigValidationResult : NSObject <goSeqRefInterface> {\n}',
  '@property(strong, readonly) _Nonnull id _ref;', ...sdkGetters.map(sdkGetter), '@end'].join('\n');
const sdkHot = [
  '@class LibboxCommandServer;', '@protocol LibboxRuntimePersistenceHandler;', '@class LibboxRuntimePersistenceHandler;',
  '@protocol LibboxRuntimePersistenceHandler <NSObject>', sdkPersist, sdkMode, '@end',
  '@interface LibboxCommandServer : NSObject <goSeqRefInterface> {\n}',
  '@property(strong, readonly) _Nonnull id _ref;', sdkCapability, sdkRetry, '@end',
  '@interface LibboxOverrideOptions : NSObject <goSeqRefInterface> {\n}',
  '@property(strong, readonly) _Nonnull id _ref;', '- (nonnull instancetype)initWithRef:(_Nonnull id)ref;', sdkInit, '@end',
  '@class LibboxRuntimePersistenceHandler;',
  '@interface LibboxRuntimePersistenceHandler : NSObject <goSeqRefInterface, LibboxRuntimePersistenceHandler> {\n}',
  '@property(strong, readonly) _Nonnull id _ref;', '- (nonnull instancetype)initWithRef:(_Nonnull id)ref;', sdkPersist, sdkMode, '@end',
].join('\n');
const sdkHeader = [sdkDeclarations, sdkHot, sdkResult].join('\n');

test('Apple SDK HOT declarations require exact receivers, conformance and generator types', () => {
  const inspect = header => sdkHeaderContracts([sdkDeclarations, header, sdkResult].join('\n')).hotObjectiveCContract;
  const facts = inspect(sdkHot);
  const handler = { runtimePersistenceMode: sdkMode, persistRuntimeConfiguration: sdkPersist };
  assert.deepEqual(facts, {
    '@interface LibboxCommandServer': {
      runtimeCapabilities: sdkCapability, startOrReloadServiceWithRetryableRuntimePersistence: sdkRetry,
    },
    '@protocol LibboxRuntimePersistenceHandler': handler,
    '@interface LibboxRuntimePersistenceHandler': handler,
    '@interface LibboxOverrideOptions': { init: sdkInit },
  });
  assert.deepEqual(inspect(sdkHot.replaceAll('\n', ' ')), facts);
  assert.deepEqual(inspect(sdkHot.replaceAll('*', ' * ').replaceAll(':', ' : ')), facts);
  assert.deepEqual(inspect(sdkHot.replaceAll(' ', '\u0085')), facts);
  const types = '<goSeqRefInterface, LibboxRuntimePersistenceHandler>';
  const protocol = '@protocol LibboxRuntimePersistenceHandler <NSObject>';
  const cases = [
    sdkHot.replace('LibboxCommandServer :', 'LibboxOtherReceiver :'),
    sdkHot.replace(sdkCapability, '') + '\n@interface Other : NSObject\n' + sdkCapability + '\n@end',
    sdkHot.replace(sdkCapability, '// ' + sdkCapability),
    sdkHot.replace(sdkCapability, '/* ' + sdkCapability + ' */'),
    sdkHot.replace(sdkCapability, sdkCapability + ' ' + sdkCapability),
    sdkHot.replace(sdkCapability, sdkCapability.replace('- ', '+ ')),
    sdkHot.replace(sdkCapability, sdkCapability.replace('NSString* _Nonnull', 'BOOL')),
    sdkHot.replace(sdkCapability, sdkCapability.replace('_Nonnull', '_Nullable')),
    sdkHot.replace(sdkCapability, '- (BOOL)runtimeCapabilities:(NSString* _Nonnull* _Nullable)ret0_ error:(NSError* _Nullable* _Nullable)error;'),
    sdkHot.replace(sdkRetry, sdkRetry.replace('BOOL', 'void')),
    sdkHot.replace(sdkRetry, sdkRetry.replace('options:', 'overrideOptions:')),
    sdkHot.replace(sdkRetry, sdkRetry.replace('options:(LibboxOverrideOptions* _Nullable)options handler:(id<LibboxRuntimePersistenceHandler> _Nullable)handler',
      'handler:(id<LibboxRuntimePersistenceHandler> _Nullable)handler options:(LibboxOverrideOptions* _Nullable)options')),
    sdkHot.replace(sdkRetry, sdkRetry.replace('id<LibboxRuntimePersistenceHandler> _Nullable', 'LibboxRuntimePersistenceHandler* _Nullable')),
    sdkHot.replace(sdkPersist, sdkPersist.replace('revision:(int64_t)', 'revision:(int32_t)')),
    sdkHot.replace(sdkPersist, sdkPersist.replace('effectiveDigest:', 'effectiveDigestWrong:')),
    sdkHot.replace(sdkPersist, sdkPersist.replace('BOOL', 'void')),
    sdkHot.replace(sdkMode, sdkMode.replace('_Nonnull', '_Nullable')),
    sdkHot.replace(protocol, protocol.replace('NSObject', 'Other')),
    sdkHot.replace(types, '<goSeqRefInterface>'),
    sdkHot.replace(types, '<LibboxRuntimePersistenceHandler>'),
    sdkHot.replace(types, '<goSeqRefInterface, Other>'),
    sdkHot.replace(sdkInit, sdkInit.replace('nonnull', 'nullable')),
    sdkHot.replace(protocol, protocol + '\n@optional'),
    sdkHot.replace('LibboxCommandServer :', 'LibboxCommandServeré :'),
    sdkHot.replace('LibboxCommandServer :', 'LibboxCommandServer\ufeff:'),
    sdkHot + sdkHot,
  ];
  for (const declaration of [sdkCapability, sdkRetry, sdkPersist]) {
    for (const type of ['NSError* _Nullable', 'NSError* _Nonnull* _Nullable', 'NSError* _Nullable* _Nonnull', 'NSString* _Nullable* _Nullable']) {
      cases.push(sdkHot.replace(declaration, declaration.replace('NSError* _Nullable* _Nullable', type)));
    }
  }
  for (const type of ['NSString* _Nullable', 'LibboxOverrideOptions* _Nullable', 'id<LibboxRuntimePersistenceHandler> _Nullable']) {
    cases.push(sdkHot.replace(sdkRetry, sdkRetry.replace(type, type.replace('_Nullable', '_Nonnull'))));
  }
  const proxy = sdkHot.indexOf('@interface LibboxRuntimePersistenceHandler :');
  for (const declaration of [sdkPersist, sdkMode]) {
    const preceding = sdkHot.slice(0, proxy), remaining = sdkHot.slice(proxy);
    cases.push(preceding + remaining.replace(declaration, ''), preceding + remaining.replace(declaration, declaration.replace('- ', '+ ')));
  }
  const protocolStart = sdkHot.indexOf(protocol);
  const protocolEnd = sdkHot.indexOf('@end', protocolStart) + '@end'.length;
  cases.push(sdkHot.slice(0, protocolStart) + sdkHot.slice(protocolEnd));
  for (const changed of cases) assert.throws(() => inspect(changed), /generated Objective-C HOT|unexpected Objective-C HOT/);
});

test('Apple SDK validation result requires exact immutable getters on its own receiver', () => {
  const inspect = header => sdkHeaderContracts([sdkDeclarations, sdkHot, header].join('\n')).validationResultGetterContract;
  const facts = inspect(sdkResult);
  assert.deepEqual(facts, { receiver: 'LibboxConfigValidationResult',
    getters: Object.fromEntries(sdkGetters.map(name => [name, sdkGetter(name)])) });
  assert.deepEqual(inspect(sdkResult.replaceAll('\n', ' ')), facts);
  const getter = sdkGetter('getTailscaleStoreRetirement');
  const membership = sdkGetter('getTailscaleStoreMembership');
  for (const changed of [
    sdkResult.replace(getter, ''), sdkResult.replace(membership, ''),
    sdkResult.replace(getter, '// ' + getter), sdkResult.replace(getter, '/* ' + getter + ' */'),
    sdkResult.replace(getter, getter.replace('getTailscaleStoreRetirement', 'tailscaleStoreRetirement')),
    sdkResult.replace(getter, getter.replace('NSString* _Nonnull', 'int64_t')),
    sdkResult.replace(getter, getter.replace('_Nonnull', '_Nullable')),
    sdkResult.replace(getter, getter.replace(';', ': (NSString*)argument;')),
    sdkResult.replace(getter, getter.replace('- ', '+ ')),
    sdkResult.replace(getter, getter + '\n' + getter),
    sdkResult.replace(getter, getter + ' ' + getter),
    sdkResult.replace(getter, getter + ' - (void)setTailscaleStoreRetirement:(NSString*)value;'),
    sdkResult.replace('LibboxConfigValidationResult', 'LibboxOtherReceiver'),
    sdkResult.replace('goSeqRefInterface', 'OtherProtocol'), sdkResult + sdkResult,
    sdkResult.replace('@end', '- (void)setTailscaleStoreRetirement:(NSString*)value;\n@end'),
    sdkResult.replace('@end', '@property(strong) NSString* tailscaleStoreMembership;\n@end'),
    sdkResult.replace(getter, '') + '\n@interface LibboxOtherReceiver : NSObject\n' + getter + '\n@end',
  ]) assert.throws(() => inspect(changed), /generated Objective-C/);
});

test('Apple SDK plist inventory binds separate device and simulator declarations to one arm64 slice each', () => {
  const root = mkdtempSync(join(tmpdir(), 'apple-framework-libraries-'));
  try {
    const script = fileURLToPath(new URL('./mobile-core/apple-admission.mjs', import.meta.url));
    const report = join(root, 'libraries.json');
    const device = { LibraryIdentifier: 'device', LibraryPath: 'Libbox.framework', SupportedPlatform: 'ios',
      SupportedArchitectures: ['arm64'] };
    const sim = { ...device, LibraryIdentifier: 'simulator', SupportedPlatformVariant: 'simulator' };
    const projected = [
      { identifier: 'device', path: 'Libbox.framework', architectures: ['arm64'], target: 'ios-arm64' },
      { identifier: 'simulator', path: 'Libbox.framework', architectures: ['arm64'], target: 'ios-arm64-simulator' },
    ];
    const verify = (rows, pinnedRows = rows) => {
      const bytes = Buffer.from(JSON.stringify(pinnedRows));
      writeFileSync(report, JSON.stringify(rows));
      return spawnSync(process.execPath, [script, '--sdk-framework-libraries', report, bytes.length,
        createHash('sha256').update(bytes).digest('hex')], { encoding: 'utf8' });
    };
    for (const [rows, expected] of [
      [[device, sim], projected],
      [[sim, { ...device, SupportedPlatformVariant: null }], [...projected].reverse()],
    ]) {
      const result = verify(rows);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(result.stdout), expected);
    }
    const invalid = [
      [null, /device\/simulator library inventory/],
      [[device], /device\/simulator library inventory/],
      [[device, sim, sim], /device\/simulator library inventory/],
      [[device, { ...sim, LibraryIdentifier: 'device' }], /device\/simulator library inventory/],
      [[device, { ...sim, SupportedPlatformVariant: null }], /one device and one simulator/],
      [[device, { ...sim, SupportedPlatformVariant: 'maccatalyst' }], /one device and one simulator/],
      [[{ ...device, SupportedPlatform: 'macos' }, sim], /only device\/simulator iOS/],
      [[{ ...device, LibraryIdentifier: '/device' }, sim], /only device\/simulator iOS/],
      [[{ ...device, LibraryIdentifier: '..' }, sim], /only device\/simulator iOS/],
      [[device, { ...sim, LibraryPath: '' }], /only device\/simulator iOS/],
      [[device, { ...sim, LibraryPath: '../Libbox.framework' }], /only device\/simulator iOS/],
      [[device, { ...sim, LibraryPath: 'Libbox.framework/' }], /only device\/simulator iOS/],
      ...[[], ['arm64', 'x86_64']].map(architectures =>
        [[{ ...device, SupportedArchitectures: architectures }, sim], /declared architecture inventory/]),
      ...[['arm64', 'x86_64'], ['x86_64'], ['arm64', 'arm64'], ['arm64', 'x86_64', 'armv7']].map(architectures =>
        [[device, { ...sim, SupportedArchitectures: architectures }], /declared architecture inventory/]),
    ];
    for (const [rows, reason] of invalid) {
      const refused = verify(rows);
      assert.equal(refused.status, 1, refused.stderr);
      assert.match(refused.stderr, reason);
      assert.equal(refused.stdout, '');
    }
    const tampered = verify([device, { ...sim, LibraryIdentifier: 'simulatof' }], [device, sim]);
    assert.equal(tampered.status, 1);
    assert.match(tampered.stderr, /Apple SDK library inventory pin differs/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Apple SDK header command admits exact original observations and fails closed on mutations', () => {
  const root = mkdtempSync(join(tmpdir(), 'apple-sdk-bindings-'));
  try {
    const script = fileURLToPath(new URL('./mobile-core/apple-admission.mjs', import.meta.url));
    const header = join(root, 'header.h');
    const verify = (text, original = text) => {
      const bytes = Buffer.from(original);
      writeFileSync(header, text);
      return spawnSync(process.execPath, [script, '--sdk-header-contract', header, bytes.length,
        createHash('sha256').update(bytes).digest('hex')], { encoding: 'utf8' });
    };
    const admitted = verify(sdkHeader);
    assert.equal(admitted.status, 0, admitted.stderr);
    assert.deepEqual(JSON.parse(admitted.stdout), sdkHeaderContracts(sdkHeader));
    const changed = sdkHeader.replace('recordScreenState', 'recordScreenStatf');
    const tampered = verify(changed, sdkHeader);
    assert.equal(tampered.status, 1);
    assert.match(tampered.stderr, /header observation pin differs/);
    const missing = verify(changed);
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, /generated Objective-C API missing: recordScreenState/);
    const denied = verify(sdkHeader.replace(sdkCapability, sdkCapability + sdkCapability));
    assert.equal(denied.status, 1);
    assert.match(denied.stderr, /HOT method differs: interface LibboxCommandServer.runtimeCapabilities/);
    assert.equal(denied.stdout, '');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});


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
  const reset = section(platform, '    func reset()', '    func clearDNSCache()');
  assert.match(reset, /let pending = settingsCallbacks\.values\.first/);
  assert.match(reset, /let tunCustodyUncertain = nativeTun\.hasSessions/);
  assert.match(reset, /if pending == nil && !tunCustodyUncertain \{ settingsReset = UUID\(\); settings = nil \}/);
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
  assert.match(start, /let options = LibboxOverrideOptions\(\)/);
  assert.match(start, /try server\.startOrReloadService\(withRetryableRuntimePersistence: config, options: options, handler: owner\)/);
  // The Objective-C/Java selector is not the imported Swift spelling.
  assert.doesNotMatch(start, /startOrReloadServiceWith(?:Retryable)?RuntimePersistence|startOrReloadService\(config/);
  assert.match(start, /catch \{\s*owner\.revoke\(\)/);
  assert.match(provider, /let config = self\.runtimeRestartOwner\?\.snapshot\(\)/);
});

test('managed bootstrap preserves digest and directory admission before the new owner start', () => {
  const managed = section(provider, '    private func handleManagedBootstrap(', '    override func sleep(');
  const digest = managed.indexOf('digest(config) == identity.configDigest');
  const activate = managed.indexOf('server.activateTailscaleDirectoryDomain(pending.ownerNonce, expectedSourceIdentity: expectedSource)');
  const start = managed.indexOf('startRuntimeOwnedService(server, config: config, token: pending.token, managedCold: true)');
  const retain = managed.indexOf('lifecycle.retainManagedColdStoreCohort(cohort, identity: identity, token: pending.token)');
  const ready = managed.indexOf('lifecycle.complete(pending.token, phase: "running"');
  assert.ok(digest >= 0 && digest < activate && activate < start);
  assert.ok(start < retain && retain < ready);
  assert.match(managed, /if let cohort, !lifecycle\.retainManagedColdStoreCohort\(cohort, identity: identity, token: pending\.token\) \{\s*throw tunnelError/);
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
  const typedBirth = start.indexOf('let document = start(server, startSelector, config as NSString, options, owner, &error)');
  const legacyBirth = start.indexOf('server.startOrReloadService(withRetryableRuntimePersistence:');
  const query = start.indexOf('server.runtimeCapabilities(&capabilityError)');
  const decode = start.indexOf('TunnelRuntimeAdmission.decode(document, platform: platformBuild)');
  const bind = start.indexOf('lifecycle.admitRuntimeProtocol(token, admission: admission)');
  assert.ok(typedBirth >= 0 && typedBirth < legacyBirth && legacyBirth < query && query < decode && decode < bind);
  const optionalSelector = 'NSSelectorFromString("startOrReloadServiceWithTailscaleStartup:options:handler:error:")';
  assert.ok(start.includes(optionalSelector));
  assert.doesNotMatch(start.replace(optionalSelector, ''), /\bNSSelectorFromString\b/);
  const managedBirth = section(start, 'if managedCold && server.responds(to: startSelector)', '} else {');
  assert.match(managedBirth, /method_getNumberOfArguments\(method\) == 6/);
  assert.match(managedBirth, /typealias Start = @convention\(c\) \(AnyObject, Selector, NSString, AnyObject, AnyObject,\s*AutoreleasingUnsafeMutablePointer<NSError\?>\?\) -> Unmanaged<NSString>\?/);
  assert.match(managedBirth, /if let error \{ throw error \}/);
  assert.match(managedBirth, /guard let document else \{ throw tunnelError/);
  assert.match(managedBirth, /unsafeBitCast\(method_getImplementation\(method\), to: Start\.self\)/);
  assert.match(start, /if let capabilityError \{ throw capabilityError \}/);
  assert.equal((start.match(/lifecycle\.accepts\(token\), commandServer === server, runtimeRestartOwner === owner/g) ?? []).length, 2);
  assert.match(start, /platformBuild = "ios\/arm64"/);
  assert.doesNotMatch(start, /dlsym|perform\(|#if DEBUG/);
  assert.equal((managedBirth.match(/unsafeBitCast\(/g) ?? []).length, 1);
  assert.doesNotMatch(start.replace(managedBirth, ''), /unsafeBitCast/);
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
    assert.match(text, /apple-admission\.mjs/);
    assert.match(text, /--tls-link verify-installed/);
    assert.match(text, /--tls-link install/);
    assert.doesNotMatch(text, / prepare/);
  }
  const target = section(pbx, '78552A5C57D5DDA9797F1E67 /* polaris_PacketTunnel */ =', 'buildRules');
  assert.match(target, /Verify iOS Core[\s\S]*Sources/);
  const phase = section(pbx, 'B395AB4CF19394B13456AAB7 /* Verify iOS Core */ =', '6BA4F5A019704F91829A51D3');
  assert.match(phase, /alwaysOutOfDate = 1/);
  assert.ok(phase.includes('$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args'));
  assert.equal(pbx.split('OTHER_SWIFT_FLAGS = ("$(inherited)", "@$(DERIVED_FILE_DIR)/PolarisNativeTunCompiler.args",);').length - 1, 2);

  const builder = readFileSync(new URL('./mobile-core/build-apple-app.mjs', import.meta.url), 'utf8');
  const phases = ["'original-tls-prepare.log'", "timed('app-extension-archive'",
    "'original-tls-final-verify.log'", "timed('final-carrier-inspection'"];
  let previous = -1;
  for (const name of phases) {
    const position = builder.indexOf(name);
    assert.ok(position > previous, `Apple parent phase is absent or out of order: ${name}`);
    previous = position;
  }
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

test('the Apple build CLI binds TLS and native TUN inputs to each selected device and simulator slice', async t => {
  const sdks = [['iphoneos', 'ios-arm64'], ['iphonesimulator', 'ios-arm64-simulator']];
  const mobileCore = fileURLToPath(new URL('./mobile-core/', import.meta.url));
  const source = JSON.parse(readFileSync(new URL('./mobile-core/source-contract.json', import.meta.url)));
  const lockTool = join(tunDirectories(t).root, 'native-fd');
  const compileLock = spawnSync('rustc', ['--edition=2024', '-o', lockTool, join(mobileCore, 'native-fd.rs')],
    { encoding: 'utf8', timeout: 30_000 });
  assert.ifError(compileLock.error);
  assert.equal(compileLock.status, 0, compileLock.stderr);
  const receiptName = 'PolarisTLSFingerprintLinker.json';
  const linkerName = 'PolarisTLSFingerprintLinker.args';
  const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
  const tlsSymbol = '_LibboxTLSFingerprintCapabilities';
  const tlsHeader = `NSString* _Nonnull ${tlsSymbol.slice(1)}(NSError* _Nullable* _Nullable error);`;
  const wrongTlsHeader = `void ${tlsSymbol.slice(1)}(void);`;
  for (const [platform, selected] of sdks) {
    for (const present of [true, false]) {
      await t.test(`${platform} ${present ? 'native TUN V1' : 'legacy'}`, t => {
        const tlsPresent = (platform === 'iphoneos') === present;
        const { root, prepared, derived } = tunDirectories(t);
        const linkedScripts = join(root, 'mobile-core');
        symlinkSync(mobileCore, linkedScripts);
        const parent = 'import { pathToFileURL } from "node:url"; import { join } from "node:path"; '
          + 'const [root, action, log] = process.argv.slice(1); '
          + 'const { createNativeRunner } = await import(pathToFileURL(join(root, "native-runner.mjs"))); '
          + 'const { observeTLS } = await import(pathToFileURL(join(root, "apple-admission.mjs"))); '
          + 'process.stdout.write(await observeTLS(action, process.env, log, createNativeRunner()));';
        const hook = join(linkedScripts, 'apple-admission.mjs');
        const framework = join(root, 'Libbox.xcframework');
        const slices = {};
        for (const [, identifier] of sdks) {
          const folder = join(framework, identifier, 'Libbox.framework');
          const version = join(folder, 'Versions', 'A');
          const binary = Buffer.from(`inert selected static archive ${identifier}`);
          mkdirSync(join(version, 'Headers'), { recursive: true });
          writeFileSync(join(version, 'Libbox'), binary);
          const nativeHeader = (identifier === selected) === present ? tunHeader : oldHeader;
          const header = (identifier === selected) === tlsPresent ? `${tlsHeader}\n${nativeHeader}` : nativeHeader;
          writeFileSync(join(version, 'Headers', 'Libbox.h'), header);
          symlinkSync('A', join(folder, 'Versions', 'Current'));
          symlinkSync('Versions/Current/Libbox', join(folder, 'Libbox'));
          symlinkSync('Versions/Current/Headers', join(folder, 'Headers'));
          slices[identifier] = { libraryIdentifier: identifier, binary: { bytes: binary.length, sha256: sha256(binary) } };
        }
        const link = join(root, 'link.json');
        writeFileSync(link, JSON.stringify({ framework, componentOutput: { slices } }));
        const executable = Buffer.from('inert final PacketTunnel executable');
        writeFileSync(join(root, 'extension'), executable);
        const nativeTrace = join(root, 'native-tools.log');
        // Substitute only the native observations, checking their exact source and final paths.
        writeFileSync(join(root, 'xcrun'), `#!/bin/sh
printf '%s\\n' "$1" >> "$FIXTURE_NATIVE_TRACE"
test -n "$FIXTURE_BINARY" || exit 97
if [ "$#" -eq 4 ] && [ "$1 $2 $3" = "nm --no-llvm-bc -gU" ]; then
  if [ "$4" = "$FIXTURE_BINARY" ] && [ -n "$FIXTURE_RESERVATION_PROBE" ]; then
    exec "$FIXTURE_NODE" "$FIXTURE_RESERVATION_PROBE"
  fi
  if [ "$4" = "$FIXTURE_BINARY" ]; then printf 'T %s\\n' "$FIXTURE_SYMBOL"; exit; fi
  if [ "$4" = "$TARGET_BUILD_DIR/$EXECUTABLE_PATH" ]; then
    printf 'T %s\\n' "\${FIXTURE_FINAL_NM-$FIXTURE_SYMBOL}"; exit
  fi
fi
if [ "$#" -eq 3 ] && [ "$1 $2" = "dyld_info -exports" ] && [ "$3" = "$TARGET_BUILD_DIR/$EXECUTABLE_PATH" ]; then
  printf '%s\\n' "\${FIXTURE_FINAL_EXPORT-$FIXTURE_SYMBOL}"; exit
fi
exit 97
`, { mode: 0o700 });

        const budget = join(root, 'budget');
        mkdirSync(budget, { mode: 0o700 });
        const slot = sha256(root) + '.reservation';
        const ledger = join(budget, slot);
        // Hold the original open-file description with the actual POSIX lock
        // helper. An inherited descriptor for an unlocked ledger is insufficient.
        writeFileSync(ledger, JSON.stringify({ schema: 'polaris-mobile-filesystem-reservation-v1', slot, work: root,
          filesystem: statSync(root).dev, reserveBytes: source.budget.buildPeakReserveBytes, groups: [] }), { mode: 0o600 });
        const fd = openSync(ledger, 'r+');
        t.after(() => closeSync(fd));
        const lock = spawnSync(lockTool, ['exclusive'], { encoding: 'utf8', stdio: ['ignore', 'ignore', 'pipe', fd] });
        assert.ifError(lock.error);
        assert.equal(lock.status, 0, lock.stderr);
        const env = { ...process.env, PATH: `${root}:${process.env.PATH}`, PLATFORM_NAME: platform, ARCHS: 'arm64',
          POLARIS_MOBILE_LINK_INPUT: link, POLARIS_MOBILE_LINK_INPUT_SHA256: sha256(readFileSync(link)),
          POLARIS_APP_WORK: root, POLARIS_BUILD_BUDGET_ROOT: budget, POLARIS_FORMAL_APP_BUILD: '1',
          FIXTURE_BINARY: join(framework, selected, 'Libbox.framework', 'Versions', 'A', 'Libbox'),
          FIXTURE_SYMBOL: tlsPresent ? tlsSymbol : '_LibboxVersion',
          FIXTURE_NATIVE_TRACE: nativeTrace,
          FIXTURE_RESERVATION_PROBE: '',
          TARGET_BUILD_DIR: root, EXECUTABLE_PATH: 'extension' };
        let invocation = 0;
        const invoke = (action, env, native = false, reservation = native ? '3' : '999999') => {
          const log = join(root, `fixture-native-${++invocation}.log`);
          const result = spawnSync(process.execPath,
            native ? ['--input-type=module', '-e', parent, linkedScripts, action, log] : [hook, '--tls-link', action], { encoding: 'utf8', timeout: 30_000,
            env: { ...env, POLARIS_BUILD_RESERVATION_FD: reservation ?? undefined },
            stdio: ['ignore', 'pipe', 'pipe', native && reservation === '3' ? fd : 'ignore'] });
          assert.ifError(result.error);
          return result;
        };

        const parentEnv = { ...env, DERIVED_FILE_DIR: prepared };
        const frameworkPath = join(framework, selected, 'Libbox.framework');
        const chosenHeader = join(frameworkPath, 'Headers', 'Libbox.h');
        const headerBytes = readFileSync(chosenHeader);
        if (platform === 'iphoneos' && present) {
          const rejectsPreparation = (reason, inputs = parentEnv) => {
            const result = invoke('prepare', inputs, true);
            assert.equal(result.status, 1, result.stderr);
            assert.match(result.stderr, reason);
            assert.throws(() => readFileSync(join(prepared, receiptName)), { code: 'ENOENT' });
          };
          rejectsPreparation(/declaration\/definition mismatch/, { ...parentEnv, FIXTURE_SYMBOL: '_LibboxVersion' });
          for (const [prefix, reason] of [
            [`/* ${tlsHeader} */\n`, /declaration\/definition mismatch/],
            [`${wrongTlsHeader} // ${tlsHeader}\n`, /capability C ABI differs/],
            [`${tlsHeader}\n${tlsHeader}\n`, /capability declaration is not unique/],
            [`${tlsHeader}\n${wrongTlsHeader}\n`, /capability declaration is not unique/],
            [`${tlsHeader.slice(0, -1)}${wrongTlsHeader}\n`, /capability C ABI differs/],
          ]) {
            writeFileSync(chosenHeader, prefix + tunHeader);
            rejectsPreparation(reason);
          }
          writeFileSync(chosenHeader, '');
          rejectsPreparation(/bounded regular original receipt or artifact required/);
          writeFileSync(chosenHeader, headerBytes);

          const linkBytes = readFileSync(link);
          const wrongSlice = JSON.parse(linkBytes);
          wrongSlice.componentOutput.slices[selected].libraryIdentifier = 'ios-arm64-simulator';
          const wrongLink = JSON.stringify(wrongSlice);
          writeFileSync(link, wrongLink);
          rejectsPreparation(/selected libbox slice differs/, { ...parentEnv, POLARIS_MOBILE_LINK_INPUT_SHA256: sha256(wrongLink) });
          writeFileSync(link, linkBytes);

          const binary = readFileSync(env.FIXTURE_BINARY);
          writeFileSync(env.FIXTURE_BINARY, '');
          rejectsPreparation(/bounded regular original receipt or artifact required/);
          writeFileSync(env.FIXTURE_BINARY, 'different');
          rejectsPreparation(/actual libbox link bytes differ/);
          writeFileSync(env.FIXTURE_BINARY, binary);
          const outside = join(root, 'foreign-binary');
          const binaryAlias = join(frameworkPath, 'Libbox');
          const currentAlias = join(frameworkPath, 'Versions', 'Current');
          writeFileSync(outside, binary);
          unlinkSync(binaryAlias);
          for (const [target, reason] of [['../../../foreign-binary', /escaped selected framework/],
            [outside, /escaped selected framework/], ['Libbox', /input missing or unresolvable/]]) {
            symlinkSync(target, binaryAlias);
            rejectsPreparation(reason);
            unlinkSync(binaryAlias);
          }
          symlinkSync('Versions/Current/Libbox', binaryAlias);
          unlinkSync(currentAlias);
          symlinkSync('missing', currentAlias);
          rejectsPreparation(/input missing or unresolvable/);
          unlinkSync(currentAlias);
          symlinkSync('A', currentAlias);

          const outsideHeaders = join(root, 'foreign-headers');
          const headersAlias = join(frameworkPath, 'Headers');
          mkdirSync(outsideHeaders);
          writeFileSync(join(outsideHeaders, 'Libbox.h'), headerBytes);
          unlinkSync(headersAlias);
          symlinkSync(outsideHeaders, headersAlias);
          rejectsPreparation(/escaped selected framework/);
          unlinkSync(headersAlias);
          symlinkSync('Versions/Current/Headers', headersAlias);
          const movedFramework = join(framework, selected, 'moved.framework');
          renameSync(frameworkPath, movedFramework);
          symlinkSync(movedFramework, frameworkPath);
          rejectsPreparation(/framework root redirected\/missing/);
          unlinkSync(frameworkPath);
          renameSync(movedFramework, frameworkPath);
        }
        writeFileSync(nativeTrace, '');
        const reservationProbe = platform === 'iphoneos' && present;
        const prepareEnv = { ...parentEnv };
        if (reservationProbe) {
          for (const [reservation, reason] of [[null, /admitted parent reservation/], ['999999', /EBADF|bad file descriptor|reservation descriptor invalid/i]]) {
            const result = invoke('prepare', parentEnv, true, reservation);
            assert.equal(result.status, 1, result.stderr);
            assert.match(result.stderr, reason);
            assert.equal(readFileSync(nativeTrace, 'utf8'), '');
            assert.throws(() => readFileSync(join(prepared, receiptName)), { code: 'ENOENT' });
          }
          const probe = join(root, 'reservation-probe.mjs');
          writeFileSync(probe, `import assert from 'node:assert/strict';
import { appendFileSync, fstatSync, readSync, statSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
const fd = Number(process.env.POLARIS_BUILD_RESERVATION_FD);
assert.equal(fd, 3);
const inherited = fstatSync(fd), ledger = statSync(process.env.FIXTURE_RESERVATION_LEDGER);
assert.deepEqual([inherited.dev, inherited.ino], [ledger.dev, ledger.ino]);
const raw = Buffer.alloc(8192), deadline = performance.now() + 3000;
for (;;) {
  assert.ok(performance.now() < deadline, 'native child group was never registered in the inherited ledger');
  const observed = raw.toString('utf8', 0, readSync(fd, raw, 0, raw.length, 0));
  let groups;
  try { ({ groups } = JSON.parse(observed)); }
  catch (cause) { if (!(cause instanceof SyntaxError)) throw cause; }
  if (groups?.includes(process.pid)) break;
  await delay(10);
}
appendFileSync(process.env.FIXTURE_NATIVE_TRACE, 'reservation group seen\\n');
console.log('T ' + process.env.FIXTURE_SYMBOL);
`);
          Object.assign(prepareEnv, { FIXTURE_NODE: process.execPath, FIXTURE_RESERVATION_PROBE: probe,
            FIXTURE_RESERVATION_LEDGER: ledger });
        }
        const preparation = invoke('prepare', prepareEnv, true);
        assert.equal(preparation.status, 0, preparation.stderr);
        if (reservationProbe) assert.deepEqual(JSON.parse(readFileSync(ledger)).groups, []);
        const expected = Buffer.from(present ? tunDefine : '');
        const preparedReceipt = join(prepared, receiptName);
        const generatedReceipt = readFileSync(preparedReceipt);
        const parsed = JSON.parse(generatedReceipt);
        // A retained receipt's pinned formatting survives byte-for-byte installation.
        const receipt = platform === 'iphoneos' && present ? Buffer.from(`${JSON.stringify(parsed, null, 2)}\n`) : generatedReceipt;
        if (receipt !== generatedReceipt) writeFileSync(preparedReceipt, receipt);
        const expectedLinker = Buffer.from(tlsPresent ? `-Xlinker\n-u\n-Xlinker\n${tlsSymbol}\n` : '');
        assert.equal(parsed.capabilityPresent, tlsPresent);
        assert.deepEqual(parsed.binary, slices[selected].binary);
        assert.deepEqual(parsed.headers, { 'Libbox.h': { bytes: headerBytes.length, sha256: sha256(headerBytes) } });
        assert.deepEqual(parsed.flags, { bytes: expectedLinker.length, sha256: sha256(expectedLinker) });
        assert.deepEqual(parsed.nativeTun, { contractVersion: 'polaris-native-tun-v1', present,
          compilerArgs: { bytes: expected.length, sha256: sha256(expected) } });
        assert.deepEqual(readFileSync(join(prepared, responseName)), expected);
        assert.deepEqual(readFileSync(join(prepared, linkerName)), expectedLinker);
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
        assert.deepEqual(JSON.parse(finalVerification.stdout), {
          scope: 'actual-final-PacketTunnel-static-link', capabilityPresent: tlsPresent, nativeTunHeaderABIAvailable: present,
          executable: { bytes: executable.length, sha256: sha256(executable) }, loadedQueryExecuted: false, devicePassed: false,
        });
        assert.equal(readFileSync(nativeTrace, 'utf8'), tlsPresent
          ? `nm\n${reservationProbe ? 'reservation group seen\n' : ''}nm\ndyld_info\n`
          : 'nm\n');
        if (platform === 'iphoneos' && present) {
          for (const [changes, reason] of [
            [{ FIXTURE_FINAL_NM: '_LibboxVersion', FIXTURE_FINAL_EXPORT: tlsSymbol }, /final PacketTunnel stripped/],
            [{ FIXTURE_FINAL_EXPORT: `${tlsSymbol}_extra` }, /not dynamically discoverable/],
            [{ DERIVED_FILE_DIR: derived }, /final TLS verifier must use parent prepared inputs/],
          ]) {
            const result = invoke('verify', { ...parentEnv, ...pins, ...changes }, true);
            assert.equal(result.status, 1, result.stderr);
            assert.match(result.stderr, reason);
          }
          const installedFlags = join(derived, linkerName);
          writeFileSync(installedFlags, 'tampered linker flags');
          const result = invoke('verify-installed', hookEnv);
          assert.equal(result.status, 1, result.stderr);
          assert.match(result.stderr, /installed TLS linker inputs changed/);
          writeFileSync(installedFlags, expectedLinker);
          const flags = join(prepared, linkerName);
          unlinkSync(flags);
          symlinkSync(installedFlags, flags);
          const redirected = invoke('verify', { ...parentEnv, ...pins }, true);
          assert.equal(redirected.status, 1, redirected.stderr);
          assert.match(redirected.stderr, /bounded regular original receipt or artifact required/);
          unlinkSync(flags);
          writeFileSync(flags, expectedLinker);
          const finalExecutable = join(root, 'extension');
          writeFileSync(finalExecutable, '');
          const empty = invoke('verify', { ...parentEnv, ...pins }, true);
          assert.equal(empty.status, 1, empty.stderr);
          assert.match(empty.stderr, /bounded regular original receipt or artifact required/);
          writeFileSync(finalExecutable, executable);
        }

        writeFileSync(installed, present ? tunDefine.replace('_V1', '_XX') : tunDefine);
        const tamperedInstallation = invoke('verify-installed', hookEnv);
        assert.equal(tamperedInstallation.status, 1, tamperedInstallation.stderr);
        assert.match(tamperedInstallation.stderr, /installed native TUN compiler input changed/);

        const alteredReceipt = JSON.parse(receipt);
        alteredReceipt.nativeTun.present = !present;
        const alteredBytes = JSON.stringify(alteredReceipt);
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

        if (platform === 'iphoneos') {
          for (const [path, reason] of [
            [preparedReceipt, /parent TLS capability receipt changed/],
            [join(prepared, linkerName), /parent TLS capability flags changed/],
            [env.FIXTURE_BINARY, /actual libbox link bytes differ/],
            [chosenHeader, /prepared TLS capability inputs differ from selected framework/],
            [link, /actual mobile link context changed/],
          ]) {
            const bytes = readFileSync(path);
            writeFileSync(path, Buffer.concat([bytes, Buffer.from('changed')]));
            rejectsInstall(reason);
            writeFileSync(path, bytes);
          }
        }

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
  assert.match(provider, /return TunnelCleanupStages\(coreClose: coreClose, nativeReset: platform\.reset\(\)\)/);
  const reset = section(platform, '    func reset()', '    func clearDNSCache()');
  assert.match(reset, /if let failure = stages\.failure \{\s*provider\.nativeResetIncomplete\(settingsGeneration: pending\?\.0, message: failure\)/);
  const completion = section(lifecycle, 'final class TunnelSettingsCompletion', 'struct TunnelSettingsTimeout');
  assert.match(completion, /if pending \{[\s\S]*waitTimedOut = true/);
  assert.doesNotMatch(completion.slice(completion.indexOf('func wait(')), /pending = false/);
});
