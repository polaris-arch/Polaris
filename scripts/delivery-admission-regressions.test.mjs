import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { appendFileSync, copyFileSync, existsSync, mkdtempSync, readFileSync, realpathSync, writeFileSync, mkdirSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { boundedDownload, DOWNLOAD_TIMEOUT_MS } from './lib/bounded-download.mjs';
import { verifyDashboardArchive, dashboardInventory, dashboardFresh } from './fetch-dashboard.mjs';
import { inspectAppleArchitecture } from './mobile-core/apple-admission.mjs';
import { validateMobileSource } from './mobile-core/source-admission.mjs';

test('same-run Apple aggregate preserves original Python wire bytes for both declared scopes', { skip: process.platform !== 'linux' }, () => {
  const repo = fileURLToPath(new URL('../', import.meta.url));
  const handoff = 'scripts/mobile-core/apple-aggregate.mjs';
  const policy = JSON.parse(readFileSync(join(repo, 'scripts/mobile-core/receipt-policy.json')));
  const folder = mkdtempSync(join(tmpdir(), 'polaris-apple-aggregate-')), checkout = join(folder, 'checkout');
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_')));
  const options = { env, cwd: checkout, stdio: ['ignore', 'pipe', 'pipe'] };
  try {
    const imports = execFileSync('git', ['ls-files', '-z', '--', 'scripts/desktop-core', 'scripts/lib'], { ...options, cwd: repo, encoding: 'utf8' });
    for (const name of new Set([handoff, ...Object.values(policy.sourceFiles).flat(), ...imports.split('\0').filter(Boolean)])) {
      const path = join(checkout, name);
      mkdirSync(dirname(path), { recursive: true });
      copyFileSync(join(repo, name), path);
    }
    const git = (...args) => execFileSync('git', args, { ...options, encoding: 'utf8' }).trim();
    git('init', '-q'); git('add', '-A');
    git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'same-run fixture');
    Object.assign(env, { GITHUB_ACTIONS: 'true', GITHUB_RUN_ID: '731', GITHUB_RUN_ATTEMPT: '2',
      GITHUB_REPOSITORY: 'fixture/Polaris', POLARIS_APP_CANDIDATE: git('rev-parse', 'HEAD') });
    for (const scope of Object.keys(policy.appleScopes)) {
      env.POLARIS_APPLE_SCOPE = scope;
      const summaries = join(folder, scope), expected = join(folder, scope + '.expected'), output = join(folder, scope + '.actual');
      execFileSync('python3', ['-B', '-c', `
import sys
from pathlib import Path
sys.path.insert(0, 'scripts/mobile-core')
from apple_handoff import context, scope_fields
from artifacts import file_pin, receipt_digest
from consume import app_identity
from fork_input import load
from producer import producer_code, write_json

folder, output = map(Path, sys.argv[1:])
def signed(path, facts):
    facts['fingerprint'] = receipt_digest(facts)
    write_json(path, facts)
    return facts

scope, run, app, producer = scope_fields(), context(), app_identity(), producer_code()
flags = dict(devicePassed=False, signatureTrustValidated=False, publicationEligible=False)
component = folder / 'component'; component.mkdir(parents=True)
manifest = component / 'component-handoff.json'
signed(manifest, dict(schema='polaris-apple-component-handoff-v2', **scope, context=run, App=producer,
                      sourceContract=load(), producerJob='component', componentFingerprint='component'))
transport = signed(component / 'component-transport.json', dict(schema='polaris-apple-handoff-transport-v1',
                   context=run, manifest=file_pin(manifest)))
carriers = {}
for slot in scope['requiredSlots']:
    sim = slot == 'sim-arm64'
    stage = folder / slot; stage.mkdir()
    rust_path, receipt_path = stage / 'summary-rust-receipt.json', stage / 'summary-app-carrier.json'
    signed(rust_path, dict(schema='polaris-formal-apple-rust-v1', App=app, target='aarch64-apple-ios' + ('-sim' if sim else ''),
                          linkInputSha256='link', devicePassed=False, tools={'pythonEqual':120.0 if sim else 120}, rustc='actual rustc'))
    receipt = signed(receipt_path, dict(schema='polaris-mobile-apple-carrier-v1', App=app, producerCandidate=run['candidate'],
                     componentFingerprint='component', selectedTarget='ios-arm64' + ('-simulator' if sim else ''), AppCarrierValidated=True,
                     AppAccepted=False, devicePassed=False, signatureTrustValidated=False, rustInput=file_pin(rust_path), linkInputSha256='link',
                     wire={'𐀀':[2**255,-2**255,1e-7,-0.0,5e-324], '\\ue000':'许可证\\x7f\\u2028'}))
    carriers[slot] = signed(stage / ('carrier-' + slot + '.json'), dict(schema='polaris-apple-carrier-handoff-v2', **scope,
                            context=run, slot=slot, componentTransport=transport, receiptPin=file_pin(receipt_path), receipt=receipt, **flags))
required = scope['requiredSlots']
signed(output, dict(schema='polaris-apple-unsigned-chain-v3', context=run, App=producer, componentTransport=transport, carriers=carriers,
                   completeSlots=required, **flags, **scope, validatedSlots=required,
                   notRequestedSlots=[slot for slot in ['device-arm64','sim-arm64'] if slot not in required]))
if len(required) == 2:
    transport_path = component / 'component-transport.json'
    transport_path.write_bytes(transport_path.read_text().encode('utf-32'))
`, summaries, expected], options);
      const args = [handoff, '--scope', scope, '--summaries', summaries, '--output', output];
      execFileSync(process.execPath, args, options);
      assert.deepEqual(readFileSync(output), readFileSync(expected), scope);
      appendFileSync(join(summaries, 'device-arm64', 'summary-rust-receipt.json'), ' ');
      rmSync(output);
      const damaged = spawnSync(process.execPath, args, { ...options, encoding: 'utf8' });
      assert.equal(damaged.status, 1);
      assert.match(damaged.stderr, /aggregate original Rust receipt bytes differ/);
      assert.equal(existsSync(output), false);
      assert.equal(existsSync(output + '.tmp'), false);
    }
  } finally { rmSync(folder, { recursive: true, force: true }); }
});

test('D01 authentic bytes required; login HTML and same-size mutation rejected', () => {
  const bytes = Buffer.from('pinned bytes');
  const pin = {bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex')};
  verifyDashboardArchive(bytes, pin);
  assert.throws(() => verifyDashboardArchive(Buffer.from('<html>login</html>'), pin));
  const wrong = Buffer.from(bytes); wrong[0] ^= 1;
  assert.throws(() => verifyDashboardArchive(wrong, pin));
});
test('D01 cache checks nested bytes, additions, removals and links, not index existence', () => {
  const root = mkdtempSync(join(tmpdir(), 'dashboard-regression-'));
  try {
    writeFileSync(join(root, 'index.html'), 'html'); mkdirSync(join(root, 'assets'));
    writeFileSync(join(root, 'assets/app.js'), 'original');
    const before = dashboardInventory(root); assert.equal(dashboardFresh(root), false);
    writeFileSync(join(root, 'assets/app.js'), 'changed'); assert.notEqual(dashboardInventory(root), before);
    writeFileSync(join(root, 'assets/app.js'), 'original'); assert.equal(dashboardInventory(root), before);
    writeFileSync(join(root, 'extra'), 'x'); assert.notEqual(dashboardInventory(root), before);
    rmSync(join(root, 'extra')); rmSync(join(root, 'assets/app.js')); assert.notEqual(dashboardInventory(root), before);
    if (process.platform !== 'win32') {
      symlinkSync('missing', join(root, 'assets/app.js')); assert.throws(() => dashboardInventory(root));
    }
  } finally { rmSync(root, {recursive:true, force:true}); }
});
test('D02 bounded attempts, retry window and child; caller cannot expand hard timeout', () => {
  boundedDownload('https://example.invalid', {timeout:999999}, (cmd, args, options) => {
    assert.equal(cmd,'curl'); assert.equal(args[0],'--disable');
    for (const flag of ['--connect-timeout','--max-time','--speed-limit','--speed-time','--retry-max-time']) assert.ok(Number(args[args.indexOf(flag)+1])>0);
    assert.equal(options.timeout, DOWNLOAD_TIMEOUT_MS); assert.equal(options.killSignal,'SIGKILL');
  });
  assert.throws(() => boundedDownload('http://example.invalid'));
  assert.throws(() => boundedDownload('https://example.invalid',{},()=>{throw Error('timeout');}), /timeout/);
});

test('formal mobile declaration admits the fixed source and rejects platform, policy and acceptance drift', () => {
  const module = new URL('./mobile-core/', import.meta.url);
  const raw = readFileSync(new URL('source-contract.json', module), 'utf8');
  const value = JSON.parse(raw);
  assert.equal(validateMobileSource(value), value);
  const meta = 'github.com/metacubex/utls';
  const mutations = [
    [(row) => { row.sourceTag = 'v' + row.version.replace('-alpha.', '-beta.'); }, /source tag\/version differs/],
    [(row) => { row.android.abis.pop(); }, /four-ABI/],
    [(row) => { row.apple.tags = [...row.android.tags]; }, /Apple target-only tags/],
    [(row) => { row.android.outputName += '/escaped.aar'; }, /basename cannot escape/],
    [(row) => { row.android.outputName += '\\escaped.aar'; }, /basename cannot escape/],
    [(row) => { row.sourceOverlay = false; }, /zero-overlay/],
    [(row) => { row.AppAccepted = true; }, /cannot issue artifact acceptance/],
    [(row) => { row.sourcePolicy.sourceTree = '0'.repeat(40); }, /reviewed source policy: sourceTree differs/],
    [(row) => { delete row.sourcePolicy.vendors[meta]; }, /exact reviewed vendor names/],
    [(row) => { row.sourcePolicy.vendors[meta].replacementFileCount++; }, /Meta and reviewed-vendor declarations disagree/],
  ];
  for (const [mutate, expected] of mutations) {
    const changed = structuredClone(value);
    mutate(changed);
    assert.throws(() => validateMobileSource(changed), expected);
  }

  const root = mkdtempSync(join(tmpdir(), 'polaris-mobile-source-'));
  try {
    const directory = join(root, 'mobile-core');
    mkdirSync(directory);
    mkdirSync(join(root, 'lib'));
    for (const name of ['source-admission.mjs', 'receipt-policy.json']) {
      copyFileSync(new URL(name, module), join(directory, name));
    }
    copyFileSync(new URL('./lib/polaris-version.json', import.meta.url), join(root, 'lib', 'polaris-version.json'));
    const contract = join(directory, 'source-contract.json');
    const versions = (contents) => {
      writeFileSync(contract, contents);
      return spawnSync(process.execPath, [join(directory, 'source-admission.mjs'), '--versions'], { cwd: root, encoding: 'utf8' });
    };
    const limit = raw + ' '.repeat(65536 - Buffer.byteLength(raw));
    const valid = versions(limit);
    assert.equal(valid.status, 0, valid.stderr);
    assert.equal(valid.stdout, `${value.goVersion} ${value.version}\n`);
    const invalid = [
      [raw.replace('{', String.raw`{"\u0067oVersion":"overridden",`), /duplicate mobile contract key/],
      [raw.replace('"sourcePolicy": {', '"sourcePolicy": {"goDirective":"overridden",'), /duplicate mobile contract key/],
      [limit + ' ', /bounded regular original/],
      [raw.replace('{', '{"extra":1e309,'), /nonfinite/],
      [raw.replace('{', String.raw`{"extra":"\ud800",`), /invalid Unicode/],
      [raw.replace('"sourceOverlay": 0', '"sourceOverlay": 0.0'), /zero-overlay/],
      [raw.replace('"replacementFileCount": 355', '"replacementFileCount": 355.0'), /exact Meta source declaration/],
      [raw.replace(`"goVersion": "${value.goVersion}"`, '"goVersion": "1.0.0"'), /formal generator/],
      [raw.replace('"sourceTag": "v', '"sourceTag": "unmatched-v'), /source tag\/version differs/],
    ];
    for (const [contents, reason] of invalid) {
      const result = versions(contents);
      assert.equal(result.status, 1, result.stderr);
      assert.equal(result.stdout, '');
      assert.match(result.stderr, reason);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('the normal mobile declaration worker supplies exact Android and Apple bind plans', () => {
  const module = new URL('./mobile-core/', import.meta.url);
  const raw = readFileSync(new URL('source-contract.json', module));
  const source = JSON.parse(raw);
  const admitted = input => spawnSync(process.execPath, [fileURLToPath(new URL('source-admission.mjs', module))],
    { input, encoding: 'utf8' });
  const run = admitted(raw);
  assert.equal(run.status, 0, run.stderr);
  const { linkerFlagsPrefix, platforms } = JSON.parse(run.stdout);
  const contract = raw.toString('utf8');
  const fractional = contract.replace(/("minimumAPI"\s*:\s*)24\b/, (_, prefix) => prefix + '24.0');
  assert.notEqual(fractional, contract);
  const fractionalAPI = admitted(fractional);
  assert.equal(fractionalAPI.status, 0, fractionalAPI.stderr);
  assert.deepEqual(JSON.parse(fractionalAPI.stdout), { linkerFlagsPrefix, platforms });
  const ldflags = linkerFlagsPrefix + '4'.repeat(64);
  assert.equal(ldflags, '-X github.com/sagernet/sing-box/constant.Version=' + source.version +
    ' -X github.com/sagernet/sing-box/constant.DirectoryRetirementSourceIdentity=' + source.sourceCommit +
    ' -X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0 -s -w -buildid=polaris-mobile-' + '4'.repeat(64));
  assert.deepEqual(Object.keys(platforms), ['android', 'apple']);
  const paths = { gomobile: '/fixture/工具 bin/gomobile', output: '/fixture/output name',
    minimumAPI: '24', bootclasspath: '/fixture/android sdk/android.jar', ldflags };
  for (const [platform, { argv, facts, summary }] of Object.entries(platforms)) {
    const profile = source[platform];
    assert.deepEqual(argv.map(part => typeof part === 'string' ? part : paths[part.input]), [
      paths.gomobile, 'bind', '-o', paths.output, '-target', profile.targets.join(','), '-libname=box',
      '-trimpath', '-buildvcs=false', '-work', '-x', '-ldflags', ldflags, '-tags', profile.tags.join(','),
      ...(platform === 'android' ? ['-androidapi', '24', '-bootclasspath', paths.bootclasspath, '-javapkg=io.nekohasekai'] :
        ['-iosversion', '15.0']), './experimental/libbox',
    ]);
    assert.deepEqual(facts, { schema: 'polaris-mobile-bind-input-v1', platform, targets: profile.targets, tags: profile.tags,
      gomobileVersion: source.gomobileVersion, gobindVersion: source.gobindVersion, artifactAcceptance: false });
    assert.equal(summary.producerReceipt, null);
    assert.equal(summary.AppAccepted, false);
    assert.equal(summary.devicePassed, false);
    assert.equal(summary.publicationEligible, false);
  }
});

test('Apple architecture admission rechecks the explicit extension and retains sanitized observations', () => {
  const app = 'polaris_iOS';
  const extension = 'polaris_PacketTunnel';
  const minimum = '17.0';
  for (const [slot, sdk] of [['device-arm64', 'iphoneos'], ['sim-arm64', 'iphonesimulator']]) {
    const row = (target, settings = {}) => ({ target, buildSettings: {
      ARCHS: 'arm64', VALID_ARCHS: 'arm64', EXCLUDED_ARCHS: target === app && sdk === 'iphoneos' ? 'x86_64' : '',
      PLATFORM_NAME: sdk, SDKROOT: sdk, CONFIGURATION: 'release', IPHONEOS_DEPLOYMENT_TARGET: minimum, ...settings,
    } });
    const inspect = (rows, query, scheme) =>
      inspectAppleArchitecture(Buffer.from(JSON.stringify(rows)), { slot, query, minimum, scheme });
    const appRow = row(app);
    const extensionRow = row(extension);
    delete extensionRow.buildSettings.EXCLUDED_ARCHS;
    const scheme = inspect([appRow], 'scheme');
    const result = inspect([extensionRow], 'extension', scheme.admission);
    assert.deepEqual(Object.keys(result.admission), [app, extension]);
    assert.deepEqual(result.admission[extension].effectiveArchitectures, ['arm64']);
    assert.deepEqual(result.observation.rows[0].architectureSettings.EXCLUDED_ARCHS,
      { present: false, type: 'NoneType' });
    const combined = inspect([extensionRow, appRow], 'scheme').admission;
    assert.deepEqual(inspect([extensionRow], 'extension', combined).admission, result.admission);
    assert.equal(inspect([row(extension, { EXCLUDED_ARCHS: 'arm64e' })], 'extension', combined).error,
      'scheme and explicit extension settings differ');
    const raw = Buffer.from(JSON.stringify([row(app, { ARCHS: '/withheld', SECRET: 'redacted' })]));
    const rejected = inspectAppleArchitecture(raw, { slot, query: 'scheme', minimum });
    assert.deepEqual(rejected.observation.normalizedResponseText,
      { bytes: raw.length, sha256: createHash('sha256').update(raw).digest('hex') });
    assert.equal(rejected.observation.rows[0].architectureSettings.ARCHS.valueWithheld, true);
    assert.doesNotMatch(JSON.stringify(rejected), /\/withheld|redacted/);
    assert.match(inspect([row(extension, { ARCHS: 'arm64 arm64' })], 'extension', rejected.admission).error,
      /duplicate-token/);
    assert.match(inspect([extensionRow], 'extension', rejected.admission).error, /unknown-token/);
  }
});


test('Apple settings entry pins inputs, probes both targets and preserves bounded observation receipts', {
  skip: process.platform === 'win32',
}, async t => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'polaris-apple-settings-')));
  try {
    const repo = join(directory, 'checkout'), output = join(directory, 'output'), bin = join(directory, 'bin');
    const project = join(repo, 'src-tauri/gen/apple/polaris.xcodeproj');
    const projectFile = join(project, 'project.pbxproj');
    const schemeFile = join(project, 'xcshareddata/xcschemes/polaris_iOS.xcscheme');
    const config = join(repo, 'src-tauri/tauri.ios.conf.json');
    const trace = join(directory, 'xcode-calls.jsonl');
    const script = join(repo, 'scripts/mobile-core/apple-admission.mjs');
    const developer = '/fixture/selected Xcode/Contents/Developer';
    for (const name of [bin, output, dirname(schemeFile), dirname(script), join(repo, 'scripts/lib')]) {
      mkdirSync(name, { recursive: true });
    }
    for (const name of ['apple-admission.mjs', 'source-admission.mjs', 'receipt-policy.json', '../lib/polaris-version.json']) {
      copyFileSync(new URL('./mobile-core/' + name, import.meta.url), join(dirname(script), name));
    }
    writeFileSync(projectFile, 'fixed project bytes\n');
    writeFileSync(schemeFile, 'fixed scheme bytes\n');
    writeFileSync(config, JSON.stringify({ bundle: { iOS: { minimumSystemVersion: '17.0' } } }));
    const inputs = [projectFile, schemeFile, config];
    const pin = bytes => ({ bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
    const expectedPins = inputs.map(path => pin(readFileSync(path)));

    writeFileSync(join(bin, 'xcodebuild'), '#!' + process.execPath + '\n' + String.raw`
const fs = require('node:fs');
const args = process.argv.slice(2), sdk = args[args.indexOf('-sdk') + 1];
const extension = args[0] === '-target';
const { FIXTURE_MODE, FIXTURE_TRACE, DEVELOPER_DIR } = process.env;
const settings = {
  ARCHS: FIXTURE_MODE === 'wrong-arch' && !extension ? 'withheld/path' : 'arm64',
  VALID_ARCHS: 'arm64', EXCLUDED_ARCHS: !extension && sdk === 'iphoneos' ? 'x86_64' : '',
  PLATFORM_NAME: sdk, SDKROOT: sdk, CONFIGURATION: 'release', IPHONEOS_DEPLOYMENT_TARGET: '17.0',
  PRIVATE_VALUE: '/never/retain/full/settings',
};
const normalized = JSON.stringify([{ target: args[1], buildSettings: settings }], null, 2) + '\n';
fs.appendFileSync(FIXTURE_TRACE, JSON.stringify({ args, cwd: process.cwd(), DEVELOPER_DIR, normalized }) + '\n');
if (FIXTURE_MODE === 'interrupt') {
  process.on('SIGTERM', () => process.exit(0));
  const child = require('node:child_process').spawn(process.execPath, ['-e', [
    "const fs = require('node:fs')",
    "process.on('SIGTERM', () => setTimeout(() => { fs.writeFileSync(process.env.FIXTURE_SIGNAL_DONE, 'terminated'); process.exit(0); }, 200))",
    "fs.writeFileSync(process.env.FIXTURE_SIGNAL_READY, JSON.stringify({group: Number(process.env.FIXTURE_XCODE_PID), owner: Number(process.env.FIXTURE_OWNER_PID)}))",
    "setInterval(() => {}, 1000)",
  ].join(';')], { stdio: 'ignore', env: { ...process.env, FIXTURE_XCODE_PID: String(process.pid), FIXTURE_OWNER_PID: String(process.ppid) } });
  child.unref();
  setInterval(() => {}, 1000);
  return;
}
if (FIXTURE_MODE === 'drift' && extension) fs.appendFileSync(args[args.indexOf('-project') + 1] + '/project.pbxproj', 'drift');
if (FIXTURE_MODE === 'oversize') process.stdout.write('x'.repeat(2 * 1024 ** 2 + 1));
else process.stdout.write(normalized.replace(/\n/g, '\r\n'));
`, { mode: 0o700 });
    const invokeEnv = (scope, mode) => ({
      ...process.env, POLARIS_APPLE_SCOPE: scope, DEVELOPER_DIR: developer, FIXTURE_MODE: mode, FIXTURE_TRACE: trace,
      PATH: bin + delimiter + process.env.PATH,
    });
    const invoke = (scope, mode, location) => spawnSync(process.execPath, [script, '--build-settings', location], {
      cwd: directory, encoding: 'utf8', env: invokeEnv(scope, mode),
    });
    const recorded = () => readFileSync(trace, 'utf8').trim().split('\n').map(line => JSON.parse(line));
    const receipt = (location, name) => JSON.parse(readFileSync(join(location, name), 'utf8'));
    const runs = [
      ['device-and-simulator', [['device-arm64', 'iphoneos', 'iOS'], ['sim-arm64', 'iphonesimulator', 'iOS Simulator']]],
      ['device-only', [['device-arm64', 'iphoneos', 'iOS']]],
    ];
    for (const [scope, targets] of runs) {
      const location = join(output, scope);
      mkdirSync(location);
      const previousCalls = existsSync(trace) ? recorded().length : 0;
      const result = invoke(scope, 'valid', location);
      assert.equal(result.status, 0, result.stderr);
      const calls = recorded().slice(previousCalls);
      assert.equal(calls.length, 2 * targets.length);
      const progress = result.stdout.trim().split('\n').map(line => {
        assert.match(line, /^APPLE_TOOL_PROGRESS=/);
        return JSON.parse(line.slice('APPLE_TOOL_PROGRESS='.length));
      });
      assert.equal(progress.filter(row => row.event === 'end').length, calls.length);
      assert.ok(progress.every(row => row.phase === 'build-settings-admission' && row.peakRSSBytes === null));
      assert.match(result.stderr, /"phase":"serialize-before-install"/);

      const checked = receipt(location, 'build-settings-admission.json');
      const slots = targets.map(([slot]) => slot);
      assert.equal(checked.schema, 'polaris-apple-build-settings-admission-v2');
      assert.deepEqual(checked.requiredSlots, slots);
      assert.deepEqual(checked.observedSlots, slots);
      assert.deepEqual(checked.inputPins, expectedPins);
      for (const [index, [slot, sdk, destination]] of targets.entries()) {
        const common = ['-project', project, '-configuration', 'release', '-sdk', sdk, '-showBuildSettings', '-json',
          'ARCHS=arm64', 'ONLY_ACTIVE_ARCH=YES', 'CODE_SIGNING_ALLOWED=NO', 'CODE_SIGNING_REQUIRED=NO'];
        const [scheme, extension] = calls.slice(2 * index, 2 * index + 2);
        assert.deepEqual(scheme.args, ['-scheme', 'polaris_iOS', '-destination', 'generic/platform=' + destination, ...common]);
        assert.deepEqual(extension.args, ['-target', 'polaris_PacketTunnel', ...common]);
        assert.deepEqual(Object.keys(checked.slots[slot]), ['polaris_PacketTunnel', 'polaris_iOS']);
        for (const [query, call] of [['scheme', scheme], ['extension', extension]]) {
          assert.equal(call.cwd, repo);
          assert.equal(call.DEVELOPER_DIR, developer);
          const observed = receipt(location, 'build-settings-' + slot + '-' + query + '.json');
          assert.deepEqual(observed.normalizedResponseText, pin(Buffer.from(call.normalized)));
          assert.equal(observed.componentAccepted, false);
          assert.equal(observed.devicePassed, false);
          assert.doesNotMatch(JSON.stringify(observed), /never\/retain/);
        }
      }
    }

    for (const [mode, error, queryCount] of [
      ['wrong-arch', /unresolved Apple architecture setting/, 2],
      ['oversize', /response exceeds observation bound/, 1],
      ['drift', /inputs changed during inspection/, 2],
    ]) {
      const location = join(output, mode);
      mkdirSync(location);
      const previousCalls = recorded().length;
      const result = invoke('device-only', mode, location);
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, error);
      assert.equal(recorded().length - previousCalls, queryCount);
      assert.equal(existsSync(join(location, 'build-settings-admission.json')), false);
      if (mode !== 'oversize') {
        assert.equal(receipt(location, 'build-settings-device-arm64-extension.json').devicePassed, false);
      }
      if (mode === 'wrong-arch') assert.doesNotMatch(result.stderr, /withheld\/path/);
    }

    await t.test('Python-only SIGTERM waits for the detached Xcode group', async () => {
      const location = join(output, 'interrupt');
      const signalReady = join(directory, 'xcode-child-ready'), signalDone = join(directory, 'xcode-child-done');
      mkdirSync(location);
      const mobileCore = fileURLToPath(new URL('./mobile-core', import.meta.url));
      const python = spawn('python3', ['-B', '-c', [
        'import sys', 'from pathlib import Path', 'sys.path.insert(0, sys.argv[1])', 'import ci_apple',
        'ci_apple.ROOT = Path(sys.argv[2])', 'ci_apple.run_build_settings(Path(sys.argv[3]), dict(ci_apple.os.environ))',
      ].join('; '), mobileCore, repo, location], {
        stdio: ['ignore', 'pipe', 'pipe'], env: {
          ...invokeEnv('device-only', 'interrupt'), FIXTURE_SIGNAL_READY: signalReady, FIXTURE_SIGNAL_DONE: signalDone,
        },
      });
      python.stdout.resume();
      const diagnostics = [];
      python.stderr.on('data', bytes => diagnostics.push(bytes));
      const deadline = AbortSignal.timeout(15000);
      const terminal = once(python, 'close', { signal: deadline });
      terminal.catch(() => {});
      let observed;
      try {
        while (!existsSync(signalReady) || !readFileSync(signalReady).length) await delay(10, undefined, { signal: deadline });
        observed = JSON.parse(readFileSync(signalReady, 'utf8'));
        // Signal only the Python PID. Neither isolated child initially receives it.
        process.kill(python.pid, 'SIGTERM');
        assert.deepEqual(await terminal, [143, null], Buffer.concat(diagnostics).toString('utf8'));
        assert.match(Buffer.concat(diagnostics).toString('utf8'), /Apple build-settings observation interrupted/);
        assert.equal(readFileSync(signalDone, 'utf8'), 'terminated');
        assert.throws(() => process.kill(-observed.group, 0), { code: 'ESRCH' });
        assert.equal(existsSync(join(location, 'build-settings-admission.json')), false);
      } finally {
        if (python.exitCode === null && python.signalCode === null) python.kill('SIGKILL');
        if (observed) {
          try { process.kill(-observed.group, 0); } catch (error) { if (error.code === 'ESRCH') observed = undefined; else throw error; }
          if (observed) {
            process.kill(-observed.group, 'SIGKILL');
            try { process.kill(observed.owner, 'SIGTERM'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
          }
        }
      }
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
