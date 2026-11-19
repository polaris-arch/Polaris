import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync, copyFileSync, existsSync, mkdtempSync, readFileSync, writeFileSync, mkdirSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
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
