import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync, spawnSync, spawn } from 'node:child_process';
import { readFileSync, readdirSync, mkdtempSync, mkdirSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import { handoffMode, verifyWireResult, verifyWireArtifactMetadata, coreWireHandoffName, runCoreWire, surveyWireCores }
  from './desktop-core/wire-input-handoff.mjs';
import { digest } from './desktop-core/source-graph.mjs';
import { portableCanonical } from './desktop-core/release-consumer.mjs';
import { installCache, completeCache } from './fetch-protoc.mjs';
const root = fileURLToPath(new URL('../', import.meta.url));
const yaml = (name) => readFileSync(`${root}.github/workflows/${name}.yml`, 'utf8');
const names = ['ci', 'ui', 'release-risk', 'package', 'android', 'apple'];

// Evaluate the actual helper with only its imported command runner/platform replaced.
// No archive tool or downloaded product executes, including on non-Windows test hosts.
function zipExtractorFixture(platform, run) {
  const source = readFileSync(`${root}scripts/lib/extract-zip.mjs`, 'utf8')
    .replace(/^import .*;\r?$/gm, '').replace(/^export /gm, '');
  return runInNewContext(`${source}\nextractZip;`, {
    process: { platform, env: { PATH: 'C:\\Program Files\\Git\\usr\\bin', SystemRoot: 'Z:\\untrusted' } },
    execFileSync: (tool, args, options) => run(tool, Array.from(args), { ...options }),
  });
}

test('Windows ZIP extraction pins System32 bsdtar and passes drive paths/spaces as literal arguments', () => {
  const calls = [], extract = zipExtractorFixture('win32', (...call) => calls.push(call));
  for (const [archive, dest] of [
    [String.raw`C:\Users\RUNNER~1\AppData\Local\Temp\protoc.zip`, String.raw`D:\a\_temp\.protoc-stage-123`],
    [String.raw`C:\temp folder\protoc & inert.zip`, String.raw`D:\cache folder\stage & inert`],
  ]) {
    extract(archive, dest);
    assert.deepEqual(calls.at(-1), [String.raw`C:\Windows\System32\tar.exe`,
      ['-xf', archive, '-C', dest], { stdio: 'inherit', shell: false }]);
  }
  assert.equal(calls.length, 2, 'one full-extraction command per archive, with no PATH probe or fallback');
});

for (const platform of ['linux', 'darwin']) {
  test(`${platform} ZIP extraction retains full unzip semantics without a shell`, () => {
    const calls = [], archive = '/tmp/archive with spaces.zip', dest = '/tmp/destination with spaces';
    zipExtractorFixture(platform, (...call) => calls.push(call))(archive, dest);
    assert.deepEqual(calls, [['unzip', ['-q', '-o', archive, '-d', dest], { stdio: 'inherit', shell: false }]]);
  });
}

for (const platform of ['win32', 'linux', 'darwin']) {
  for (const details of [{ code: 'ENOENT' }, { status: 2 }]) {
    test(`${platform} ZIP extraction propagates ${details.code ?? 'nonzero exit'} without PATH/shell fallback`, () => {
      const calls = [], failure = Object.assign(new Error('inert extractor failure'), details);
      const extract = zipExtractorFixture(platform, (...call) => { calls.push(call); throw failure; });
      assert.throws(() => extract(String.raw`C:\temp folder\protoc.zip`, String.raw`D:\cache folder\stage`),
        (error) => error === failure);
      assert.equal(calls.length, 1, 'extractor failure must never try a second command');
      assert.equal(calls[0][0], platform === 'win32' ? String.raw`C:\Windows\System32\tar.exe` : 'unzip');
      assert.equal(calls[0][2].shell, false);
    });
  }
}

test('Windows protoc uses shared ZIP extraction and retains sealed-cache publication and failure isolation', () => {
  const directory = mkdtempSync(join(tmpdir(), 'windows-protoc-zip-'));
  const archive = String.raw`C:\temp folder\protoc.zip`, dest = join(directory, 'cache with spaces');
  const protos = ['any', 'api', 'descriptor', 'duration', 'empty', 'field_mask',
    'source_context', 'struct', 'timestamp', 'type', 'wrappers'];
  let fail = false, calls = 0;
  const extract = zipExtractorFixture('win32', (tool, args, options) => {
    calls++;
    assert.equal(tool, String.raw`C:\Windows\System32\tar.exe`);
    assert.deepEqual(args.slice(0, 3), ['-xf', archive, '-C']);
    assert.equal(args.length, 4); assert.deepEqual(options, { stdio: 'inherit', shell: false });
    const stage = args[3];
    mkdirSync(join(stage, 'bin')); writeFileSync(join(stage, 'bin/protoc.exe'), 'inert Windows protoc bytes');
    if (fail) throw Error('inert extraction failed after first member');
    mkdirSync(join(stage, 'include/google/protobuf'), { recursive: true });
    for (const name of protos) writeFileSync(join(stage, `include/google/protobuf/${name}.proto`), `inert ${name}`);
  });
  try {
    installCache(archive, dest, extract, 'win32');
    assert.equal(completeCache(dest, 'win32'), true);
    const marker = readFileSync(join(dest, '.polaris-complete.json'), 'utf8');
    fail = true;
    assert.throws(() => installCache(archive, dest, extract, 'win32'), /inert extraction failed/);
    assert.equal(calls, 2, 'failed refresh must not retry with another extractor');
    assert.equal(completeCache(dest, 'win32'), true);
    assert.equal(readFileSync(join(dest, '.polaris-complete.json'), 'utf8'), marker);
    assert.deepEqual(readdirSync(directory), ['cache with spaces'], 'staging is removed after failure');
    writeFileSync(join(dest, 'include/google/protobuf/any.proto'), 'tampered');
    assert.equal(completeCache(dest, 'win32'), false);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

function cancel(name, event, ref, fixed, inherited = '') {
  const source = /^  cancel-in-progress: \$\{\{ (.*) \}\}$/m.exec(yaml(name));
  assert(source, `${name} missing explicit cancellation policy`);
  return runInNewContext(source[1], { inputs: { preserve_run: fixed },
    github: { event_name: event, ref, event: { inputs: { preserve_run: inherited } } },
    startsWith: (a, b) => a.startsWith(b) });
}
function group(name, event, ref, fixed, runId, inherited = '') {
  const source = /^  group: (.*)$/m.exec(yaml(name)); assert(source);
  const github = { workflow: 'caller', event_name: event, ref, run_id: runId,
    event: { inputs: { preserve_run: inherited }, pull_request: { number: event === 'pull_request' ? 5 : '' },
      merge_group: { head_sha: event === 'merge_group' ? 'a'.repeat(40) : '' } } };
  return source[1].replace(/\$\{\{ (.*?) \}\}/g, (_, expression) => String(runInNewContext(expression, {
    github, inputs: { preserve_run: fixed, platforms: '', platform: '' },
    format: (pattern, value) => pattern.replace('{0}', value),
  })));
}
test('all expensive workflows cancel obsolete automatic tips but preserve fixed audits, merge queue and tags', () => {
  for (const name of names) {
    for (const [event, ref] of [['push', 'refs/heads/main'], ['pull_request', 'refs/pull/5/merge'],
      ['workflow_dispatch', 'refs/heads/candidate']]) {
      assert.equal(cancel(name, event, ref, false), event !== 'workflow_dispatch', `${name}: ${event}`);
      assert.equal(cancel(name, event, ref, true), false, `${name}: fixed ${event}`);
      assert.equal(cancel(name, event, ref, false, 'true'), false, `${name}: inherited audit`);
    }
    assert.equal(cancel(name, 'merge_group', 'refs/heads/gh-readonly-queue/main/pr-5', false), false);
    for (const event of ['push', 'workflow_dispatch']) assert.equal(cancel(name, event, 'refs/tags/v1.0.0', false), false);
    const ref = 'refs/heads/candidate';
    assert.notEqual(group(name, 'workflow_dispatch', ref, false, '123'), group(name, 'workflow_dispatch', ref, false, '124'), 'manual campaigns never replace one another');
    for (const inherited of ['', 'true']) {
      const fixed = inherited === '';
      assert.notEqual(group(name, 'workflow_dispatch', ref, fixed, '123', inherited), group(name, 'workflow_dispatch', ref, fixed, '124', inherited));
      assert.equal(group(name, 'workflow_dispatch', ref, fixed, '123', inherited), group(name, 'workflow_dispatch', ref, false, '123'));
    }
    assert.notEqual(group(name, 'merge_group', ref, false, '123'), group(name, 'merge_group', ref, false, '124'));
    assert.equal(group(name, 'merge_group', ref, false, '123'), group(name, 'merge_group', 'refs/heads/other', false, '123'), 'queue head identity must precede ref fallback');
    assert.equal(group(name, 'pull_request', 'refs/pull/5/merge', false, '123'), group(name, 'pull_request', 'refs/heads/other', false, '124'), 'same PR tip must share its number-based group');
    assert.notEqual(group(name, 'push', 'refs/tags/v1.0.0', false, '123'), group(name, 'push', ref, false, '123'));
    assert.equal(group(name, 'push', 'refs/tags/v1.0.0', false, '123'), group(name, 'push', 'refs/tags/v1.0.0', false, '124'));
    assert.notEqual(group(name, 'push', 'refs/tags/v1.0.0', false, '123'), group(name, 'push', 'refs/tags/v2.0.0', false, '123'));
  }
});
test('trigger coverage retains both cross-language contracts, full merge groups and a single tag publishing entry', () => {
  for (const name of ['ci', 'ui']) {
    const source = yaml(name), paths = source.match(/    paths-ignore:\n((?:      .+\n)+)/)?.[1];
    assert(paths); assert.doesNotMatch(paths, /ui\/\*\*|src-tauri\/\*\*/);
    assert.match(source, /  merge_group:\n    types: \[checks_requested\]/);
  }
  const pkg = yaml('package').split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
  assert.match(pkg, /  push:\n    tags:\n      - 'v\*'/); assert.doesNotMatch(pkg, /  release:\n    types:/);
  assert.match(yaml('release-risk'), /skip_quality_gates: true/);
  assert.match(yaml('android'), /POLARIS_PUBLISH_RELEASE: \$\{\{ inputs.publish_release && '1' \|\| '0' \}\}/);
  assert.match(yaml('desktop-core'), /cache: false/, 'published/source/technical modes cannot share compiled-core cache');
});
test('dependency cache only restores matching runtime/manager download store and never replaces frozen install', () => {
  for (const name of ['ui', 'package']) {
    const source = yaml(name);
    assert.match(source, /key: \$\{\{ runner.os \}\}-node26-pnpm11\.24\.0-\$\{\{ hashFiles\('ui\/pnpm-lock.yaml'\) \}\}/);
    assert.match(source, /restore-keys: \$\{\{ runner.os \}\}-node26-pnpm11\.24\.0-/);
    assert.match(source, /pnpm(?: --dir ui)? install --frozen-lockfile/);
    assert.doesNotMatch(source, /if:.*cache-hit/);
  }
});
const candidate = 'a'.repeat(40), context = { repository: 'polaris-arch/Polaris', runId: '123', attempt: '1' };
const facts = { schema: 'polaris-current-attempt-wire-result-v1', candidate, context,
  command: ['cargo', 'test', '-p', 'polaris-singbox-grpc'], source: { 'Cargo.lock': 'b'.repeat(64) },
  tools: { rustc: 'c'.repeat(64), protoc: 'd'.repeat(64), go: 'e'.repeat(64) },
  files: [{ name: 'linux/sing-box', bytes: 1, sha256: 'f'.repeat(64), mode: 0o755 }] };
const proof = (value) => ({ ...value, fingerprint: digest(portableCanonical(value)) });
test('wire reuse rejects foreign attempts, mutated sources/tools/bytes and altered results even with recomputed fingerprints', () => {
  assert.equal(verifyWireResult(proof(facts), facts), true);
  for (const change of [
    (x) => { x.context.attempt = '2'; }, (x) => { x.context.runId = '124'; },
    (x) => { x.candidate = 'b'.repeat(40); }, (x) => { x.tools.rustc = '0'.repeat(64); },
    (x) => { x.source['Cargo.lock'] = '0'.repeat(64); }, (x) => { x.files[0].sha256 = '0'.repeat(64); },
    (x) => { x.files.pop(); }, (x) => { x.command.push('--ignored'); },
  ]) {
    const changed = structuredClone(facts); change(changed);
    assert.throws(() => verifyWireResult(proof(changed), facts), /input\/source\/tool\/attempt differs/);
  }
  assert.throws(() => verifyWireResult({ ...proof(facts), fingerprint: '0'.repeat(64) }, facts), /fingerprint differs/);
  assert.throws(() => verifyWireResult(null, facts));
});
test('artifact API pin requires exact current-run ID, digest, candidate name and head before transport import', () => {
  const name = coreWireHandoffName(candidate, context), sha = 'b'.repeat(64);
  const metadata = { id: 456, name, expired: false, digest: `sha256:${sha}`, workflow_run: { id: 123, head_sha: candidate } };
  verifyWireArtifactMetadata(metadata, candidate, context, name, '456', sha, candidate);
  for (const mutate of [(x) => { x.id++; }, (x) => { x.expired = true; },
    (x) => { x.digest = `sha256:${'0'.repeat(64)}`; }, (x) => { x.name += '-alias'; },
    (x) => { x.workflow_run.id++; }, (x) => { x.workflow_run.head_sha = '0'.repeat(40); }]) {
    const x = structuredClone(metadata); mutate(x);
    assert.throws(() => verifyWireArtifactMetadata(x, candidate, context, name, '456', sha, candidate));
  }
  assert.throws(() => verifyWireArtifactMetadata(metadata, candidate, { ...context, attempt: '2' }, name, '456', sha, candidate));
});
test('Windows permissions compare checked transport modes with NTFS projection without claiming Unix execute bits', () => {
  assert.equal(handoffMode(0o100666, 'win32', 0o755), 0o755);
  assert.equal(handoffMode(0o100666, 'win32', 0o600), 0o600);
  assert.equal(handoffMode(0o100444, 'win32', 0o444), 0o444);
  for (const mode of [undefined, 0o777, 0o100755, 0]) assert.throws(() => handoffMode(0o100666, 'win32', mode));
  assert.throws(() => handoffMode(0o100666, 'win32', 0o444));
  assert.throws(() => handoffMode(0o100444, 'win32', 0o755));
  assert.equal(handoffMode(0o100755, 'linux'), 0o755);
  assert.throws(() => handoffMode(0o100777, 'linux'));
});

test('wire wrapper writes no success record after command failure or input change and forces the full exact gate', () => {
  const root = mkdtempSync(join(tmpdir(), 'polaris-wire-control-'));
  const sourceNames = ['Cargo.lock', 'crates/singbox-grpc/tests/bundled_core_wire.rs'];
  const inputs = ['.release-receipts', 'linux', 'win', 'mac-x64', 'mac-arm64'];
  let failure = false, changed = false, calls = 0;
  try {
    for (const name of sourceNames) { const path = join(root, name); mkdirSync(join(path, '..'), { recursive: true }); writeFileSync(path, name); }
    for (const name of inputs) { const path = join(root, 'resources', name); mkdirSync(path, { recursive: true }); writeFileSync(join(path, name === 'win' ? 'sing-box.exe' : name === '.release-receipts' ? 'input' : 'sing-box'), name, { mode: 0o600 }); }
    mkdirSync(join(root, 'src-tauri'));
    writeFileSync(join(root, 'src-tauri/core-manifest.json'), JSON.stringify({ coreArchiveSha256: Object.fromEntries(inputs.filter((key) => key !== '.release-receipts').map((key) => [key, 'f'.repeat(64)])) }));
    const path = join(root, 'result.json');
    const env = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: context.repository, GITHUB_RUN_ID: context.runId,
      GITHUB_RUN_ATTEMPT: context.attempt, GITHUB_REF: 'refs/heads/control', POLARIS_REQUIRE_KERNEL_GATE: '0' };
    const run = (tool, args, options) => {
      if (tool === 'git') {
        if (args[0] === 'rev-parse') return candidate + '\n';
        if (args[0] === 'status') return '';
        if (args[0] === 'ls-files') return sourceNames.join('\0') + '\0';
      }
      if (['rustc', 'protoc', 'go'].includes(tool)) return `${tool}-fixed`;
      assert.equal(tool, 'cargo'); calls++;
      assert.deepEqual(args, ['test', '-p', 'polaris-singbox-grpc', '--test', 'bundled_core_wire',
        'vendored_proto_matches_every_bundled_core', '--', '--exact']);
      assert.equal(options.env.POLARIS_REQUIRE_KERNEL_GATE, '1');
      if (failure) throw Error('inert command failed');
      if (changed) writeFileSync(join(root, 'resources/linux/sing-box'), 'changed');
    };
    failure = true; assert.throws(() => runCoreWire(root, candidate, path, { env, run }), /inert command failed/);
    assert.equal(existsSync(path), false);
    failure = false; changed = true; assert.throws(() => runCoreWire(root, candidate, path, { env, run }), /changed during command/);
    assert.equal(existsSync(path), false);
    changed = false; runCoreWire(root, candidate, path, { env, run });
    assert.equal(calls, 3); const result = JSON.parse(readFileSync(path, 'utf8'));
    assert.equal(result.context.attempt, '1'); assert.equal(result.files.length, 5);
    assert.throws(() => runCoreWire(root, candidate, path, { env, run }), /already exists/);
    assert.equal(calls, 3, 'existing result must never silently replace a command');
    // Real temporary directory metadata, no executable is ever started.
    rmSync(path); const expectedInventory = surveyWireCores(root);
    assert.deepEqual(result.coreInventory, expectedInventory);
    for (const name of ['dashboard', 'data', 'extra']) {
      mkdirSync(join(root, 'resources', name));
      writeFileSync(join(root, 'resources', name, 'ordinary-resource'), 'legitimate resource');
    }
    assert.deepEqual(surveyWireCores(root), expectedInventory, 'non-core resource directories stay legitimate');
    for (const directory of ['extra', 'dashboard', 'data']) {
      for (const binary of ['sing-box', 'sing-box.exe']) {
        const orphan = join(root, 'resources', directory, binary); writeFileSync(orphan, 'inert orphan bytes');
        assert.throws(() => surveyWireCores(root), /Orphan wire core/);
        assert.throws(() => runCoreWire(root, candidate, path, { env, run }), /Orphan wire core/);
        assert.equal(existsSync(path), false); assert.equal(calls, 3, 'orphan must fail before command/result reuse');
        rmSync(orphan);
      }
    }
    const linux = join(root, 'resources/linux/sing-box'); rmSync(linux);
    assert.throws(() => surveyWireCores(root), /ENOENT/);
    assert.throws(() => runCoreWire(root, candidate, path, { env, run }), /ENOENT/);
    assert.equal(existsSync(path), false); assert.equal(calls, 3);
    writeFileSync(linux, 'restored inert bytes', { mode: 0o600 });
    assert.deepEqual(surveyWireCores(root), expectedInventory);

  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('download workflow uses hard-pinned ZIP readback rather than a digest-warning-only artifact action', () => {
  const pkg = yaml('package');
  for (const label of ['installer', 'wire']) {
    const start = pkg.indexOf(`      - name: Download current-attempt preflight ${label} inputs\n`);
    assert(start >= 0); const end = pkg.indexOf('      - name:', start + 20), block = pkg.slice(start, end);
    assert.match(block, /wire-input-handoff\.mjs download/);
    assert.doesNotMatch(block, /actions\/download-artifact/);
  }
  const transport = readFileSync(`${root}scripts/desktop-core/release-archive.py`, 'utf8');
  assert.match(transport, /hashlib\.sha256\(data\)\.hexdigest\(\) == record\['sha256'\]/);
  assert.match(transport, /len\(infos\) == 2/); assert.match(transport, /archive\.read\(info\)/);
});

test('inert outer ZIP fixture proves tampered bytes, inventory and special members are rejected before writes', () => {
  const source = String.raw`
import hashlib, io, json, pathlib, runpy, stat, sys, tempfile, zipfile
extract = runpy.run_path(sys.argv[1])['extract_wire_transport']
def archive(rows):
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w') as z:
        for name, mode, data in rows:
            info = zipfile.ZipInfo(name); info.external_attr = mode << 16
            z.writestr(info, data)
    return out.getvalue()
valid = [('archive.json', stat.S_IFREG | 0o644, b'{}'),
         ('core-wire-inputs.tar.gz', stat.S_IFREG | 0o644, b'inert tar bytes')]
with tempfile.TemporaryDirectory(prefix='wire-outer-control-') as tmp:
    root = pathlib.Path(tmp); path = root / 'input.zip'
    raw = archive(valid); path.write_bytes(raw)
    pin = {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}
    destination = root / 'valid'; extract(path, destination, pin)
    assert {p.name for p in destination.iterdir()} == {'archive.json', 'core-wire-inputs.tar.gz'}
    cases = [(raw, {**pin, 'sha256': '0' * 64}), (raw, {**pin, 'bytes': len(raw)+1})]
    for rows in [valid[:1], valid + [('extra', stat.S_IFREG | 0o644, b'x')],
        [('../archive.json', stat.S_IFREG | 0o644, b'{}'), valid[1]],
        [(valid[0][0], stat.S_IFLNK | 0o777, b'target'), valid[1]],
        [(valid[0][0], stat.S_IFDIR | 0o755, b'{}'), valid[1]],
        [(valid[0][0], stat.S_IFREG | 0o644, b''), valid[1]]]:
        bad = archive(rows); cases.append((bad, {'bytes': len(bad), 'sha256': hashlib.sha256(bad).hexdigest()}))
    for number, (data, record) in enumerate(cases):
        path.write_bytes(data); target = root / ('rejected-' + str(number))
        try: extract(path, target, record)
        except (ValueError, zipfile.BadZipFile): pass
        else: raise AssertionError('accepted invalid wire transport ' + str(number))
        assert not target.exists()
    print(json.dumps({'negativeCases': len(cases)}))
`;
  const observed = JSON.parse(execFileSync('python3', ['-B', '-c', source,
    `${root}scripts/desktop-core/release-archive.py`], { encoding: 'utf8', timeout: 30000 }));
  assert.equal(observed.negativeCases, 8);
});

// Exercise the actual workflow shell with inert command shims, never real Rust builds.
function helperRunBlock(source = yaml('package')) {
  const step = source.split('      - name: Build and stage privileged helper\n')[1]?.split('\n      - name:')[0];
  const run = step?.split('        run: |\n')[1]?.split('\n').filter((line) => line.startsWith('          '))
    .map((line) => line.slice(10)).join('\n');
  assert(run?.startsWith('set -euo pipefail'), 'helper step/run must exist and fail closed');
  return run;
}
const helperPlatforms = [
  ['linux', 'x86_64-unknown-linux-gnu', 'Linux', 'linux', 'polaris-helper'],
  ['windows', 'x86_64-pc-windows-msvc', 'Windows', 'win', 'polaris-helper.exe'],
  ['macos-arm64', 'aarch64-apple-darwin', 'macOS', 'mac-arm64', 'polaris-helper'],
  ['macos-x64', 'x86_64-apple-darwin', 'macOS', 'mac-x64', 'polaris-helper'],
];
function runHelperFixture(platform, { host = platform[1], source = helperRunBlock(), fail = '', missing = '' } = {}) {
  const [label, target, os] = platform;
  const directory = mkdtempSync(join(tmpdir(), 'polaris-helper-target-'));
  try {
    const binDir = join(directory, 'bin'), log = join(directory, 'calls.jsonl');
    mkdirSync(binDir); writeFileSync(log, '');
    const shim = `#!${process.execPath}
const fs = require('node:fs'), path = require('node:path');
const tool = path.basename(process.argv[1]), args = process.argv.slice(2);
fs.appendFileSync(process.env.CALL_LOG, JSON.stringify({ tool, args, buildId: process.env.POLARIS_BUILD_ID }) + '\\n');
if (tool === process.env.FAIL_TOOL) process.exit(7);
if (tool === 'rustc') { process.stdout.write(process.env.RUSTC_OUTPUT); }
if (tool === 'cargo') {
  const at = args.indexOf('--target'), target = at < 0 ? '' : args[at + 1];
  const binary = args[args.indexOf('--bin') + 1] + (process.env.FIXTURE_OS === 'Windows' ? '.exe' : '');
  if (binary !== process.env.MISSING_BINARY) {
    const output = path.join('target', target, 'release', binary);
    fs.mkdirSync(path.dirname(output), { recursive: true }); fs.writeFileSync(output, 'inert ' + binary);
  }
}
if (tool === 'cp') fs.copyFileSync(args[0], args[1]);
`;
    for (const tool of ['rustc', 'cargo', 'cp', 'codesign', 'node']) writeFileSync(join(binDir, tool), shim, { mode: 0o755 });
    const rendered = source.replaceAll('${{ matrix.label }}', label)
      .replaceAll('${{ matrix.rust_target }}', target).replaceAll('${{ runner.os }}', os);
    assert.doesNotMatch(rendered, /\$\{\{/);
    const result = spawnSync('bash', ['-e', '-o', 'pipefail', '-c', rendered], {
      cwd: directory, encoding: 'utf8', timeout: 15000,
      env: { ...process.env, PATH: `${binDir}:${process.env.PATH}`, CALL_LOG: log,
        RUSTC_OUTPUT: host === null ? 'rustc fixture\n' : `rustc fixture\nhost: ${host}\n`,
        FIXTURE_OS: os, FAIL_TOOL: fail, MISSING_BINARY: missing,
        POLARIS_BUILD_ID: candidate, GITHUB_WORKSPACE: directory, GITHUB_ENV: join(directory, 'env') },
    });
    assert.ifError(result.error);
    return { status: result.status, stderr: result.stderr, stdout: result.stdout,
      calls: readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((line) => JSON.parse(line)),
      identity: existsSync(join(directory, 'env')) ? readFileSync(join(directory, 'env'), 'utf8') : '', directory };
  } finally { rmSync(directory, { recursive: true, force: true }); }
}
function assertHelperFixture(platform, options = {}) {
  const [label, target, os, res, binary] = platform;
  const result = runHelperFixture(platform, options);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  const mac = os === 'macOS', builds = result.calls.filter(({ tool }) => tool === 'cargo');
  const targetArgs = mac ? ['--target', target] : [];
  assert.deepEqual(builds.map(({ args }) => args), [
    ['build', '--release', '-p', 'polaris-helper', '--bin', 'polaris-helper', ...targetArgs],
    ...(label === 'windows' ? [['build', '--release', '-p', 'polaris-windows-cleaner', '--bin', 'polaris-cleaner']] : []),
  ]);
  assert(builds.every(({ buildId }) => buildId === candidate));
  const output = mac ? `target/${target}/release` : 'target/release';
  assert.deepEqual(result.calls.filter(({ tool }) => tool === 'cp').map(({ args }) => args), [
    [`${output}/${binary}`, `resources/${res}/${binary}`],
    ...(label === 'windows' ? [[`${output}/polaris-cleaner.exe`, 'resources/win/polaris-cleaner.exe']] : []),
  ]);
  const tools = result.calls.map(({ tool }) => tool);
  assert.deepEqual(tools, mac ? ['cargo', 'cp', 'codesign', 'codesign', 'node']
    : label === 'windows' ? ['rustc', 'cargo', 'cp', 'cargo', 'cp', 'node'] : ['rustc', 'cargo', 'cp', 'node']);
  assert.deepEqual(result.calls.filter(({ tool }) => tool === 'rustc').map(({ args }) => args), mac ? [] : [['-vV']]);
  if (!mac) assert(tools.indexOf('rustc') < tools.indexOf('cargo'));
  assert.equal(tools.at(-1), 'node', 'identity prepared only after every staged binary');
  assert.deepEqual(result.calls.at(-1).args, ['scripts/prepare-core-identity.mjs', `--platform=${res}`]);
  assert.equal(result.identity, `POLARIS_CORE_IDENTITY_FILE=${result.directory}/resources/${res}/core-manifest.json\n`);
  assert.deepEqual(result.calls.filter(({ tool }) => tool === 'codesign').map(({ args }) => args),
    mac ? Array.from({ length: 2 }, () => ['--verify', '--strict', `resources/${res}/${binary}`]) : []);
}

test('desktop helper uses checked native host output while both mac architectures keep explicit targets', () => {
  for (const platform of helperPlatforms) assertHelperFixture(platform,
    { host: platform[2] === 'macOS' ? 'aarch64-apple-darwin' : platform[1] });
  const source = yaml('package');
  assert(source.indexOf('      - name: Build and stage privileged helper\n') < source.indexOf('      - name: Build installers\n'));
  assert.match(source, /POLARIS_BUILD_ID: \$\{\{ needs.setup.outputs.candidate \}\}/);
});

test('helper host mismatch, absent host, unknown platform and failed staging stop before identity', () => {
  for (const platform of helperPlatforms.slice(0, 2)) {
    for (const host of ['wrong-triple', null, `${platform[1]}\nhost: ${platform[1]}`]) {
      const result = runHelperFixture(platform, { host });
      assert.notEqual(result.status, 0); assert.match(result.stdout, /helper host [\s\S]* differs/);
      assert.deepEqual(result.calls.map(({ tool }) => tool), ['rustc']);
      assert.equal(result.identity, '');
    }
  }
  const unknown = runHelperFixture(['unknown', 'x86_64-unknown-linux-gnu', 'Linux']);
  assert.notEqual(unknown.status, 0); assert.deepEqual(unknown.calls, []);
  for (const fail of ['rustc', 'cargo', 'cp', 'node']) {
    const result = runHelperFixture(helperPlatforms[0], { fail });
    assert.notEqual(result.status, 0); assert.equal(result.identity, '');
    assert.equal(result.calls.at(-1).tool, fail);
  }
  const unsigned = runHelperFixture(helperPlatforms[3], { fail: 'codesign' });
  assert.notEqual(unsigned.status, 0); assert.equal(unsigned.identity, '');
  assert(!unsigned.calls.some(({ tool }) => tool === 'node'));
  for (const missing of ['polaris-helper.exe', 'polaris-cleaner.exe']) {
    const result = runHelperFixture(helperPlatforms[1], { missing });
    assert.notEqual(result.status, 0); assert.equal(result.identity, '');
    assert(!result.calls.some(({ tool }) => tool === 'node'));
  }
});

test('helper target/path controls reject old split-target mode, host drift and incomplete cleaner staging', () => {
  const source = helperRunBlock();
  for (const [platform, before, after] of [
    [helperPlatforms[0], 'target_args=()', 'target_args=(--target "${{ matrix.rust_target }}")'],
    [helperPlatforms[1], 'output_dir=target/release', 'output_dir="target/${{ matrix.rust_target }}/release"'],
    [helperPlatforms[3], 'target_args=(--target "${{ matrix.rust_target }}")', 'target_args=()'],
    [helperPlatforms[1], 'cp "$output_dir/polaris-cleaner.exe" resources/win/polaris-cleaner.exe', ':'],
  ]) {
    const changed = source.replace(before, after); assert.notEqual(changed, source);
    assert.throws(() => assertHelperFixture(platform, { source: changed }));
  }
  const unchecked = source.replace('if [ "$host" != "${{ matrix.rust_target }}" ]; then', 'if false; then');
  assert.notEqual(unchecked, source);
  // The rejection oracle itself must distinguish an unchecked host from the real run block.
  const accepted = runHelperFixture(helperPlatforms[0], { host: 'wrong-triple', source: unchecked });
  assert.equal(accepted.status, 0); assert(accepted.calls.some(({ tool }) => tool === 'cargo'));
});

function assertLauncherDiagnosticJob(source) {
  const job = source.split('\n  windows_launcher_diagnostics:\n')[1];
  assert.ok(job, 'independent diagnostic job');
  const cheap = source.split('\n  cheap_format:\n')[1].split('\n  lint:\n')[0];
  const cheapExpression = cheap.match(/^    if: \$\{\{ (.*) \}\}$/m)?.[1];
  const diagnosticExpression = job.match(/if: >-\s*\$\{\{([\s\S]*?)\}\}/)?.[1];
  assert.ok(cheapExpression && diagnosticExpression);
  for (const event of ['push', 'pull_request', 'merge_group', 'workflow_call', 'workflow_dispatch'])
    for (const selected of [undefined, false, true])
      for (const system of ['', 'a'.repeat(40)])
      for (const repository of ['polaris-arch/Polaris', 'someone/Polaris'])
        for (const ref of ['refs/heads/collab/dots-audit-remediation-20261017', 'refs/heads/main', 'refs/tags/test']) {
          const context = { github: { event_name: event, repository, ref }, inputs: { windows_launcher_diagnostics: selected, windows_system_candidate: system } };
          assert.equal(runInNewContext(cheapExpression, context), event !== 'workflow_dispatch' || selected !== true || system !== '');
          assert.equal(runInNewContext(diagnosticExpression, context), event === 'workflow_dispatch' && selected === true && system === ''
            && repository === 'polaris-arch/Polaris' && ref === 'refs/heads/collab/dots-audit-remediation-20261017');
        }
  assert.match(source, /windows_launcher_diagnostics:\n\s+description:.*\n\s+type: boolean\n\s+default: false/);
  for (const name of ['lint', 'cross', 'test']) {
    const existing = source.split(`\n  ${name}:\n`)[1].split(/\n  [a-z_]+:\n/)[0];
    assert.match(existing, /^    needs: cheap_format$/m);
    assert.doesNotMatch(existing.slice(0, existing.indexOf('    steps:')), /always\(\)|!cancelled\(\)|continue-on-error/);
  }
  assert.match(job, /runs-on: windows-2022/);
  assert.match(job, /timeout-minutes: 60/);
  assert.match(job, /permissions:\n      contents: read/);
  assert.doesNotMatch(job, /\bwrite\b|continue-on-error|ApproveTemporarySystemTask|Run-SystemBootstrap|\$\{\{ inputs\./);
  assert.equal((job.match(/persist-credentials: false/g) ?? []).length, 2);
  assert.match(job, /ref: \$\{\{ github.sha \}\}/);
  assert.match(job, /\$candidate.sourceCommit -cne '68ffb209ded1f9337f9115741cba24f9f4c63421'/);
  assert.match(job, /\$candidate.repository -cne 'polaris-arch\/sing-box-polaris'/);
  assert.match(job, /repository: polaris-arch\/sing-box-polaris/);
  assert.match(job, /ref: \$\{\{ steps.candidate.outputs.source \}\}/);
  assert.equal((job.match(/\.\/scripts\/windows-directory\/Run-WithUserTokenOwner.ps1/g) ?? []).length, 1);
  assert.doesNotMatch(job, /cargo (?:build|test)|Run-UserTokenOwnerChild.ps1|Build-And-Test.ps1/);
  assert.match(job, /if \(\$LASTEXITCODE -ne 0\) \{ throw 'Fixed launcher diagnostic attempt failed' \}/);
  assert.match(job, /if: \$\{\{ always\(\) \}\}\n        uses: actions\/upload-artifact@v7/);
  assert.match(job, /if-no-files-found: error/);
}
test('manual fixed-source launcher diagnosis has no ordinary Rust prerequisite or PR/SYSTEM bypass', () => assertLauncherDiagnosticJob(yaml('ci')));
for (const [label, before, after] of [
  ['PR heavy gate skipped', "github.event_name != 'workflow_dispatch' || inputs.windows_launcher_diagnostics != true", 'false'],
  ['diagnostic admits PR', "github.event_name == 'workflow_dispatch' && inputs.windows_launcher_diagnostics == true", "github.event_name == 'pull_request' && inputs.windows_launcher_diagnostics == true"],
  ['diagnostic admits other branch', "github.ref == 'refs/heads/collab/dots-audit-remediation-20261017'", 'true'],
  ['diagnostic admits other repository', "github.repository == 'polaris-arch/Polaris'", 'true'],
  ['candidate source is arbitrary', "-cne '68ffb209ded1f9337f9115741cba24f9f4c63421'", "-cne $env:INPUT_SOURCE"],
  ['launcher failure ignored', "if ($LASTEXITCODE -ne 0) { throw 'Fixed launcher diagnostic attempt failed' }", '# ignored'],
  ['failure evidence conditional', 'if: ${{ always() }}\n        uses: actions/upload-artifact@v7', 'if: ${{ success() }}\n        uses: actions/upload-artifact@v7'],
]) test(`manual launcher diagnosis rejects ${label}`, () => {
  const source = yaml('ci'); assert.ok(source.includes(before));
  assert.throws(() => assertLauncherDiagnosticJob(source.replaceAll(before, after)));
});

test('Node gate streams diagnostics and rejects runner failure, missing coverage and malformed summaries', async () => {
  const source = readFileSync(new URL('./gate-node-test.sh', import.meta.url), 'utf8');
  const required = [...source.matchAll(/^  (scripts\/[^\s]+\.test\.mjs)$/gm)].map(m => m[1]);
  const minimum = Number(/"\$passed" -lt (\d+)/.exec(source)?.[1]);
  assert(minimum >= 500); assert(required.includes('scripts/ci-workflow-efficiency.test.mjs'));
  const root = mkdtempSync(join(tmpdir(), 'node-gate-'));
  try {
    mkdirSync(join(root, 'scripts')); mkdirSync(join(root, 'bin'));
    writeFileSync(join(root, 'scripts/gate-node-test.sh'), source);
    for (const name of required) writeFileSync(join(root, name), '');
    const shim = join(root, 'bin/node');
    const env = { ...process.env, PATH: `${join(root, 'bin')}:${process.env.PATH}` };
    const run = (body) => {
      writeFileSync(shim, `#!/bin/bash\n${body}\n`, { mode: 0o755 });
      const result = spawnSync('bash', ['scripts/gate-node-test.sh'], { cwd: root, env, encoding: 'utf8', timeout: 5000 });
      assert.ifError(result.error); return result;
    };
    assert.equal(run(`echo '# pass ${minimum}'`).status, 0);
    for (const body of [
      `echo '# pass ${minimum}'; exit 7`, 'echo "not ok 1 - failed"; exit 1',
      `echo '# pass ${minimum - 1}'`, 'echo "# pass 0"', 'echo "changed summary"',
      `echo '# pass ${minimum}'; echo '# pass ${minimum}'`,
    ]) assert.notEqual(run(body).status, 0, body);
    const failure = run('echo "not ok 1 - diagnostic"; exit 1');
    assert.match(failure.stdout, /not ok 1 - diagnostic/);
    writeFileSync(shim, `#!/bin/bash\necho live-marker\nread -r release\necho '# pass ${minimum}'\n`, { mode: 0o755 });
    await new Promise((resolve, reject) => {
      const child = spawn('bash', ['scripts/gate-node-test.sh'], { cwd: root, env });
      let output = '', released = false;
      const timer = setTimeout(() => { child.kill('SIGKILL'); reject(Error('TAP buffered until runner exit')); }, 5000);
      child.stdout.on('data', chunk => {
        output += chunk;
        if (!released && output.includes('live-marker')) { released = true; child.stdin.end('continue\n'); }
      });
      child.on('error', error => { clearTimeout(timer); reject(error); });
      child.on('close', code => { clearTimeout(timer); try { assert(released); assert.equal(code, 0); resolve(); } catch (error) { reject(error); } });
    });
    rmSync(join(root, required[0]));
    assert.notEqual(run(`echo '# pass ${minimum}'`).status, 0);
  } finally { rmSync(root, { recursive: true, force: true }); }
});


test('one complete manual Package campaign forwards exact SYSTEM selection and rejects subsets before build', () => {
  const pkg = yaml('package'), ci = yaml('ci');
  const input = /windows_system_candidate:\n(?:        [^\n]*\n)*?        type: string\n        default: ''/;
  assert.match(pkg.split('  workflow_dispatch:')[1].split('  workflow_call:')[0], input);
  assert.match(ci.split('  workflow_call:')[1].split('\nconcurrency:')[0], input);
  const caller = pkg.split('\n  ci:\n')[1].split('\n  ui:')[0];
  assert.match(caller, /uses: \.\/.github\/workflows\/ci.yml/);
  assert.match(caller, /windows_system_candidate: \$\{\{ inputs.windows_system_candidate \|\| '' \}\}/);
  const block = pkg.split('      - name: Resolve platform matrix\n')[1].split('      - name:')[0];
  assert.match(block, /SYSTEM_CANDIDATE: \$\{\{ inputs.windows_system_candidate \|\| '' \}\}/);
  const prefix = block.split('        run: |\n')[1].split('          if [ "$POLARIS_NATIVE_PAYLOAD_VALIDATION" = 1 ]; then')[0]
    .split('\n').map(line => line.replace(/^          /, '')).join('\n');
  const sha = 'a'.repeat(40);
  const env = { ...process.env, SYSTEM_CANDIDATE: sha, GITHUB_SHA: sha, GITHUB_EVENT_NAME: 'workflow_dispatch',
    GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REF: 'refs/heads/ci/governance', PLATFORM: 'all',
    PLATFORMS_JSON: '', VALIDATION_SKIP_QUALITY: 'false' };
  const run = extra => spawnSync('bash', ['-c', prefix], { env: { ...env, ...extra }, encoding: 'utf8', timeout: 5000 }).status;
  assert.equal(run({}), 0); assert.equal(run({ GITHUB_REF: 'refs/heads/main' }), 0);
  for (const extra of [{ PLATFORM: 'windows' }, { PLATFORM: 'linux' }, { PLATFORMS_JSON: '["linux","windows"]' },
    { VALIDATION_SKIP_QUALITY: 'true' }, { SYSTEM_CANDIDATE: 'b'.repeat(40) }, { SYSTEM_CANDIDATE: sha.toUpperCase() },
    { GITHUB_EVENT_NAME: 'pull_request' }, { GITHUB_REPOSITORY: 'other/Polaris' }, { GITHUB_REF: 'refs/heads/unapproved' },
    { GITHUB_REF: 'refs/tags/v1.0.0' }]) assert.notEqual(run(extra), 0, JSON.stringify(extra));
  assert.equal(run({ SYSTEM_CANDIDATE: '', PLATFORM: 'windows', VALIDATION_SKIP_QUALITY: 'true' }), 0);
});
