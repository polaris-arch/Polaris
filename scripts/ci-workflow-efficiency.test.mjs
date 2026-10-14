import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { readFileSync, mkdtempSync, mkdirSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import { handoffMode, verifyWireResult, verifyWireArtifactMetadata, coreWireHandoffName, runCoreWire, surveyWireCores }
  from './desktop-core/wire-input-handoff.mjs';
import { digest } from './desktop-core/source-graph.mjs';
import { portableCanonical } from './desktop-core/release-consumer.mjs';
const root = fileURLToPath(new URL('../', import.meta.url));
const yaml = (name) => readFileSync(`${root}.github/workflows/${name}.yml`, 'utf8');
const names = ['ci', 'ui', 'release-risk', 'package', 'android', 'apple'];
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
      assert.equal(cancel(name, event, ref, false), true, `${name}: automatic ${event}`);
      assert.equal(cancel(name, event, ref, true), false, `${name}: fixed ${event}`);
      assert.equal(cancel(name, event, ref, false, 'true'), false, `${name}: inherited audit`);
    }
    assert.equal(cancel(name, 'merge_group', 'refs/heads/gh-readonly-queue/main/pr-5', false), false);
    for (const event of ['push', 'workflow_dispatch']) assert.equal(cancel(name, event, 'refs/tags/v1.0.0', false), false);
    const ref = 'refs/heads/candidate';
    assert.equal(group(name, 'workflow_dispatch', ref, false, '123'), group(name, 'workflow_dispatch', ref, false, '124'), 'replaceable tips must share a group');
    for (const inherited of ['', 'true']) {
      const fixed = inherited === '';
      assert.notEqual(group(name, 'workflow_dispatch', ref, fixed, '123', inherited), group(name, 'workflow_dispatch', ref, fixed, '124', inherited));
      assert.notEqual(group(name, 'workflow_dispatch', ref, fixed, '123', inherited), group(name, 'workflow_dispatch', ref, false, '123'));
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
