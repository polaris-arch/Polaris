import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { appendFileSync, chmodSync, copyFileSync, existsSync, mkdtempSync, readFileSync, readdirSync, realpathSync, renameSync, writeFileSync, mkdirSync, rmSync, symlinkSync, unlinkSync } from 'node:fs';
import { devNull, tmpdir } from 'node:os';
import { delimiter, dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { boundedDownload, DOWNLOAD_TIMEOUT_MS } from './lib/bounded-download.mjs';
import { verifyDashboardArchive, dashboardInventory, dashboardFresh } from './fetch-dashboard.mjs';
import { inspectAppleArchitecture } from './mobile-core/apple-admission.mjs';
import { checkMobileBuildInfo, parseMobileBuildInfo, reviewedSource, validateMobileSource } from './mobile-core/source-admission.mjs';
const nativeId = 'polaris-mobile-' + 'a'.repeat(64);
const requiredNative = ['github.com/metacubex/utls', 'github.com/sagernet/sing-tun'];

test('mobile module list admits exact Go rows and rejects ambiguous or incomplete output', t => {
  const folder = mkdtempSync(join(tmpdir(), 'mobile-module-list-'));
  t.after(() => rmSync(folder, { recursive: true, force: true }));
  const report = join(folder, 'go list.json');
  const verifier = fileURLToPath(new URL('./mobile-core/source-admission.mjs', import.meta.url));
  const read = raw => {
    writeFileSync(report, raw);
    return spawnSync(process.execPath, [verifier, '--module-list', report], { encoding: 'utf8', timeout: 30_000 });
  };
  const firstRow = '{"Path":"example.org/a","Replace":{"Dir":"a } directory"},"futureNumber":9007199254740993}';
  const secondRow = '{"Path":"example.org/b","Indirect":true}';
  const good = read('\n' + firstRow + secondRow + '\r\n');
  assert.equal(good.status, 0, good.stderr);
  assert.equal(good.stdout, '[' + firstRow + ',' + secondRow + ']\n');
  for (const bad of ['', firstRow + firstRow, '[]', '{"Version":"v1"}', firstRow + '{"Path":',
    firstRow + 'incomplete', '{"Path":"a","\\u0050ath":"b"}']) {
    const rejected = read(bad);
    assert.equal(rejected.status, 1, rejected.stderr);
    assert.equal(rejected.stdout, '');
  }
});

test('reviewed source observes every vendor object and enforces the independent Meta binding', { skip: process.platform === 'win32' }, async t => {
  const folder = realpathSync(mkdtempSync(join(tmpdir(), 'reviewed-mobile-source-')));
  try {
    const root = join(folder, 'source');
    mkdirSync(root);
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_')));
    Object.assign(env, { GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: devNull, GIT_NO_REPLACE_OBJECTS: '1', GIT_TERMINAL_PROMPT: '0' });
    const git = (...args) => execFileSync('git', args, { cwd: root, env, encoding: 'utf8' }).trim();
    const write = (path, bytes) => {
      const target = join(root, path);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, bytes);
    };
    const sha = bytes => createHash('sha256').update(bytes).digest('hex');
    const source = JSON.parse(readFileSync(new URL('mobile-core/source-contract.json', import.meta.url)));
    write('.gitignore', '/build/\n*.overlay\n');
    write(source.declaredTagsSource.path, 'package main\n');
    source.declaredTagsSource.sha256 = sha('package main\n');
    const policy = source.sourcePolicy, { metacubex: meta } = policy;
    const vendors = Object.entries(policy.vendors);
    const changedFiles = ['conn.go', 'reality.go'];
    const metaFiles = [...changedFiles, 'nested.txt', ...Array.from({ length: 352 }, (_, n) => 'nested/' + n)];
    for (const [name, pin] of vendors) {
      const files = name === meta.module ? metaFiles : ['package.txt'];
      for (const file of files) write(join(pin.replacementPath, file), 'original\n');
      pin.replacementFileCount = files.length;
    }
    symlinkSync('.gitignore', join(root, 'third_party/tailscale/.stignore'));
    write('third_party/tailscale/.gitignore', '');
    policy.vendors['github.com/sagernet/tailscale'].replacementFileCount += 2;
    write('go.mod', [`module ${source.goModule}`, `go ${policy.goDirective}`, `toolchain ${policy.toolchain}`, 'require (',
      '\tgithub.com/sagernet/gomobile ' + source.gomobileVersion, ...vendors.map(([name, pin]) => `\t${name} ${pin.version}`),
      ')', 'replace (', ...vendors.map(([name, pin]) => `\t${name} => ${pin.replacementPath}`), ')', ''].join('\n'));
    write('go.sum', 'reviewed sum\n');
    for (const [file, key] of [['go.mod', 'goModSha256'], ['go.sum', 'goSumSha256']]) {
      policy[key] = source[key] = sha(readFileSync(join(root, file)));
    }
    const patch = 'reviewed patch\n';
    write(meta.patchPath, patch);
    meta.patchSha256 = sha(patch);
    const binding = { schema: 'polaris-repository-module-replacement-v1', module: meta.module,
      upstreamRepository: 'https://github.com/MetaCubeX/utls', upstreamCommit: meta.upstreamCommit,
      upstreamVersion: meta.version, upstreamH1: meta.upstreamH1, upstreamModuleZipSHA256: meta.upstreamZipSha256,
      replacementPath: meta.replacementPath, patch: meta.patchPath, patchSHA256: meta.patchSha256, changedFiles,
      files: Object.fromEntries(metaFiles.map(name => [name, { bytes: 9,
        beforeSHA256: sha(changedFiles.includes(name) ? 'upstream\n' : 'original\n'), afterSHA256: sha('original\n') }])) };
    function writeBinding() {
      const bytes = JSON.stringify(binding);
      write(meta.bindingPath, bytes);
      meta.bindingSha256 = sha(bytes);
    }
    writeBinding();
    git('init', '-q', '--object-format=sha1');
    git('add', '-f', '.');
    git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'reviewed source fixture');
    const tag = (...args) => git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'tag', '-a', source.sourceTag, ...args);
    const ref = 'refs/tags/' + source.sourceTag;
    tag('-m', 'immutable source object');
    source.sourceTagObject = git('rev-parse', ref);
    policy.sourceCommit = source.sourceCommit = git('rev-parse', 'HEAD');
    policy.sourceTree = source.sourceTree = git('rev-parse', 'HEAD^{tree}');
    for (const [, pin] of vendors) pin.replacementTree = git('rev-parse', 'HEAD:' + pin.replacementPath.slice(2));
    meta.replacementTree = policy.vendors[meta.module].replacementTree;

    const observe = () => reviewedSource(root, validateMobileSource(source));
    const observation = observe();
    assert.equal(observation.schema, 'polaris-reviewed-source-observation-v2');
    assert.equal(observation.replacementFileCount, 355);
    assert.deepEqual(observation.vendors, Object.fromEntries(vendors.map(([name, pin]) =>
      [name, { path: pin.replacementPath, tree: pin.replacementTree, files: pin.replacementFileCount }])));
    await t.test('both live hosted adapters admit the same annotated checkout and canonical receipt', () => {
      // The origin retains the pinned object but advertises a different tag.
      // Remap only this fixture's fetch to local Git while recording exact argv.
      tag('--force', '-m', 'different advertised tag');
      assert.notEqual(git('rev-parse', ref), source.sourceTagObject);
      git('config', 'uploadpack.allowAnySHA1InWant', 'true');
      const wrapperDir = join(folder, 'git-bin');
      mkdirSync(wrapperDir);
      const executable = join(wrapperDir, 'git');
      const fetchLog = join(folder, 'fetches.jsonl');
      const actualGit = execFileSync('which', ['git'], { encoding: 'utf8' }).trim();
      writeFileSync(executable, '#!' + process.execPath + '\n' + `
const fs = require('node:fs');
const { spawnSync } = require('node:child_process');
const args = process.argv.slice(2), fetch = args.indexOf('fetch');
if (args[1] === 'init' && (fs.statSync(args[3]).mode & 0o777) !== 0o700) process.exit(95);
if (fetch >= 0) {
  if (args[fetch + 3] !== ${JSON.stringify(source.sourceURL)}) process.exit(93);
  fs.appendFileSync(${JSON.stringify(fetchLog)}, JSON.stringify(args.slice(fetch)) + '\\n');
  args[fetch + 3] = ${JSON.stringify(pathToFileURL(root).href)};
}
const result = spawnSync(${JSON.stringify(actualGit)}, args, { stdio: 'inherit' });
process.exit(result.status ?? 94);
`, { mode: 0o700 });
      const mobileDir = fileURLToPath(new URL('mobile-core/', import.meta.url));
      const envWithTrap = { ...env, PATH: wrapperDir + delimiter + env.PATH, PYTHONPATH: mobileDir,
        GIT_DIR: join(folder, 'wrong.git'), GIT_WORK_TREE: join(folder, 'wrong-worktree') };
      const input = JSON.stringify(source);
      function adapter(owner, slot, value = source) {
        return spawnSync('python3', ['-B', '-c',
          'import importlib,json,sys; from fork_input import canonical,verify_source; ' +
          'value=json.load(sys.stdin); source=sys.argv[2]; owner=importlib.import_module(sys.argv[1]); ' +
          'print(json.dumps([owner.prepare_source(source,value), verify_source(source,value), canonical(value).decode()]))',
          owner, slot], { cwd: folder, env: envWithTrap, input: JSON.stringify(value), encoding: 'utf8', timeout: 30_000 });
      }
      const requested = ['fetch', '--depth=1', '--no-tags', source.sourceURL, source.sourceTagObject + ':' + ref];
      const android = join(folder, 'android-source'), apple = join(folder, 'apple-source');
      let canonical;
      for (const [owner, slot] of [['ci_android', android], ['ci_apple', apple]]) {
        const prepared = adapter(owner, slot);
        assert.equal(prepared.status, 0, prepared.stderr);
        const [receipt, verified, bytes] = JSON.parse(prepared.stdout);
        canonical = bytes;
        assert.deepEqual(receipt, {
          schema: 'polaris-mobile-source-observation-v1', sourceCommit: source.sourceCommit,
          sourceTree: source.sourceTree, sourceTag: source.sourceTag, sourceTagObject: source.sourceTagObject,
          sourceOverlay: 0, inputContractSha256: sha(bytes), artifactAcceptance: false, reviewedSource: observation,
        });
        assert.deepEqual(verified, receipt);
        assert.equal(execFileSync(actualGit, ['-C', slot, 'rev-parse', ref], { env, encoding: 'utf8' }).trim(), source.sourceTagObject);
        assert.equal(execFileSync(actualGit, ['-C', slot, 'branch', '--show-current'], { env, encoding: 'utf8' }).trim(), '');
      }
      const fetches = () => readFileSync(fetchLog, 'utf8').trim().split('\n').map(line => JSON.parse(line));
      assert.deepEqual(fetches(), [requested, requested]);
      assert.equal(adapter('ci_android', android).status, 0); // Warm source never refetches the moved tag.
      assert.deepEqual(fetches(), [requested, requested]);

      const script = fileURLToPath(new URL('mobile-core/source-admission.mjs', import.meta.url));
      const verify = (slot, ...args) => spawnSync(process.execPath, [script, '--verify-checkout', slot, ...args],
        { env: envWithTrap, input: canonical, encoding: 'utf8' });
      const refuse = (result, reason) => {
        assert.equal(result.status, 1, result.stderr);
        assert.equal(result.stdout, '');
        assert.match(result.stderr, reason);
      };
      refuse(verify(join(android, 'third_party')), /worktree differs/);
      writeFileSync(join(android, 'go.sum'), 'dirty tracked bytes');
      refuse(verify(android, '--generator-outputs'), /exact and clean/);
      writeFileSync(join(android, 'go.sum'), 'reviewed sum\n');
      mkdirSync(join(android, 'build'));
      writeFileSync(join(android, 'build', 'artifact'), 'generator output');
      refuse(verify(android), /zero-overlay/);
      assert.equal(verify(android, '--generator-outputs').status, 0);
      const quotedNames = ['back\\slash', 'double"quote', 'café'];
      for (const name of quotedNames) {
        const path = join(android, 'build', name);
        writeFileSync(path, 'quoted generator name');
        refuse(verify(android, '--generator-outputs'), /zero-overlay/);
        unlinkSync(path);
      }
      execFileSync(actualGit, ['-C', android, 'config', 'core.quotePath', 'false'], { env });
      const unicodePath = join(android, 'build', quotedNames[2]);
      writeFileSync(unicodePath, 'unquoted non-ASCII generator name');
      assert.equal(verify(android, '--generator-outputs').status, 0);
      unlinkSync(unicodePath);
      writeFileSync(join(android, 'ignored.overlay'), 'foreign build input');
      refuse(verify(android, '--generator-outputs'), /zero-overlay/);

      const invalid = { ...source, declaredTagsSource: { ...source.declaredTagsSource, sha256: 'invalid' } };
      const untouched = join(folder, 'invalid-source');
      refuse(adapter('ci_apple', untouched, invalid), /tags declaration differs/);
      assert.equal(existsSync(untouched), false);
      refuse(adapter('ci_apple', untouched, { ...source, extra: 'x'.repeat(65536) }), /mobile input contract is too large/);
      assert.equal(existsSync(untouched), false);
      const redirectedSlot = join(folder, 'redirected-source');
      symlinkSync(join(folder, 'outside-source-slot'), redirectedSlot, 'dir');
      refuse(adapter('ci_android', redirectedSlot), /source slot is redirected/);
      assert.equal(existsSync(join(folder, 'outside-source-slot')), false);
      const absent = { ...source, sourceTagObject: 'b'.repeat(40) };
      const rejected = join(folder, 'missing-object-source');
      refuse(adapter('ci_android', rejected, absent), /immutable tag fetch failed/);
      assert.deepEqual(fetches(), [requested, requested, [...requested.slice(0, -1), absent.sourceTagObject + ':' + ref]]);
      refuse(adapter('ci_apple', rejected, absent), /Git verification failed/);
      assert.equal(fetches().length, 3); // An occupied rejected slot stays rejected; no branch fallback.
      assert(!readdirSync(folder).some(name => name.startsWith('.mobile-source-declaration-')));
      assert.notEqual(canonical, input); // The adapter's exact canonical bytes, not input formatting, were hashed.
    });
    const refuse = reason => assert.throws(observe, reason);
    const target = 'third_party/sing/package.txt';
    write(target, 'different');
    refuse(/vendor bytes differ/);
    write(target, 'original\n');
    chmodSync(join(root, target), 0o755);
    refuse(/vendor mode differs/);
    chmodSync(join(root, target), 0o644);
    const redirected = dirname(join(root, target)), outside = join(folder, 'redirected');
    renameSync(redirected, outside);
    symlinkSync(outside, redirected, 'dir');
    refuse(/source directory redirected/);
    rmSync(redirected);
    renameSync(outside, redirected);
    binding.files['conn.go'].afterSHA256 = sha('different');
    writeBinding();
    refuse(/actual Meta bytes differ: conn\.go/);
    binding.files['conn.go'].afterSHA256 = sha('original\n');
    writeBinding();
    write('go.sum', 'another sum\n');
    refuse(/actual go\.sum differs/);
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});

function mobileBuildFixture(folder) {
  const root = realpathSync(folder);
  const source = JSON.parse(readFileSync(new URL('mobile-core/source-contract.json', import.meta.url)));
  const vendors = Object.entries(source.sourcePolicy.vendors);
  const graph = [
    { Path: 'example.org/module', Version: 'v1.2.3', Sum: 'h1:fixed' },
    { Path: source.goModule, Main: true, Dir: root },
  ];
  const modules = {
    gobind: { kind: 'mod', version: '(devel)' },
    'example.org/module': { kind: 'dep', version: 'v1.2.3', sum: 'h1:fixed' },
    [source.goModule]: { kind: 'dep', version: 'v0.0.0-00010101000000-000000000000', replacement: root },
  };
  for (const [name, pin] of vendors) {
    const directory = join(root, pin.replacementPath);
    mkdirSync(directory, { recursive: true });
    graph.push({ Path: name, Version: pin.version, Replace: { Path: pin.replacementPath, Dir: directory } });
    modules[name] = { kind: 'dep', version: pin.version, replacement: pin.replacementPath, replacementVersion: '(devel)' };
  }
  const input = (osName = 'android', arch = 'arm64', simulator = false) => {
    const android = osName === 'android', mode = android ? 'c-shared' : 'c-archive';
    const tags = [...source[android ? 'android' : 'apple'].tags, ...(android ? [] : ['ios']), ...(simulator ? ['iossimulator'] : [])];
    const facts = { goVersion: source.goVersion, path: 'gobind', modules: structuredClone(modules), settings: {
      GOOS: osName, GOARCH: arch, CGO_ENABLED: '1', '-buildmode': mode, '-trimpath': 'true', '-tags': tags.join(','),
    } };
    return { source, graph: structuredClone(graph), target: { osName, arch, mode, simulator },
      expectedId: nativeId, actualId: nativeId, facts };
  };
  return { input, vendors: vendors.map(([name]) => name) };
}

function buildInfoText(facts, name = 'libbox.so') {
  const lines = [name + ': go' + facts.goVersion, '\tpath\t' + facts.path];
  for (const [name, row] of Object.entries(facts.modules)) {
    lines.push(['', row.kind, name, row.version, row.sum ?? ''].join('\t'));
    if (Object.hasOwn(row, 'replacement')) {
      lines.push(['', '=>', row.replacement, row.replacementVersion ?? '', row.replacementSum ?? ''].join('\t'));
    }
  }
  for (const [key, value] of Object.entries(facts.settings)) lines.push('\tbuild\t' + key + '=' + JSON.stringify(value));
  return lines.join('\n') + '\n';
}

function verifyMobileBuildFixture({ facts, ...input }) {
  return checkMobileBuildInfo({ ...input, raw: buildInfoText(facts) });
}

test('native mobile BuildInfo admits each exact platform and preserves original module projection', { skip: process.platform === 'win32' }, () => {
  const folder = mkdtempSync(join(tmpdir(), 'mobile-buildinfo-cli-'));
  try {
    const { input, vendors } = mobileBuildFixture(folder);
    const script = fileURLToPath(new URL('mobile-core/source-admission.mjs', import.meta.url));
    const report = join(folder, 'report.json');
    const appleTextFingerprint = createHash('sha256').update(
      '{"goVersion":"' + input('ios').facts.goVersion + '","modules":{},"path":null,"settings":{"text":"\\u6a39"}}',
    ).digest('hex');
    const verify = (target, tamper, edit) => {
      const { facts, graph, source, ...binding } = target;
      const evidence = target.target.osName === 'android'
        ? { graph, source, ...binding, raw: buildInfoText(facts) }
        : { graph, source, appleObservations: [
          { raw: 'observed-section: go' + facts.goVersion + '\n\tbuild\ttext="樹"' },
          { ...binding, raw: buildInfoText(facts, 'observed-section') },
        ] };
      edit?.(evidence);
      const bytes = Buffer.from(JSON.stringify(evidence));
      writeFileSync(report, bytes);
      const digest = createHash('sha256').update(bytes).digest('hex');
      tamper?.(bytes);
      return spawnSync(process.execPath, [script, '--native-buildinfo', report, bytes.length, digest], { encoding: 'utf8' });
    };
    for (const target of [
      ...['arm64', 'arm', '386', 'amd64'].map(arch => input('android', arch)),
      input('ios'), input('ios', 'arm64', true),
    ]) {
      const frozen = structuredClone(target);
      const result = verify(target);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(target, frozen);
      const android = target.target.osName === 'android';
      const expected = android ? frozen.facts : [
        { goVersion: frozen.facts.goVersion, modules: {}, settings: { text: '樹' }, path: null }, frozen.facts,
      ];
      assert.deepEqual(JSON.parse(result.stdout), expected);
      if (target.target.arch === 'arm64') {
        const python = execFileSync('python3', ['-B', '-c', `
import json
import sys
sys.path.insert(0, sys.argv[1])
from artifacts import AppleBuildInfoBatch, verify_native_buildinfo
from producer import run

evidence = json.load(sys.stdin)
if 'raw' in evidence:
    target = evidence['target']
    observed = verify_native_buildinfo(evidence['raw'], evidence['source'], evidence['graph'],
                                       os_name=target['osName'], arch=target['arch'], mode=target['mode'],
                                       simulator=target['simulator'], expected_id=evidence['expectedId'],
                                       actual_id=evidence['actualId'], run=run, scratch=sys.argv[2])
else:
    batch, infos = AppleBuildInfoBatch(), []
    for row in evidence['appleObservations']:
        info = {'parserInput': row['raw']}
        batch.observe(info)
        infos.append(info)
        if 'target' in row:
            batch.bind(info, arch=row['target']['arch'], simulator=row['target']['simulator'],
                       expected_id=row['expectedId'], actual_id=row['actualId'])
    batch.verify(run=run, scratch=sys.argv[2], source=evidence['source'], graph=evidence['graph'])
    assert infos[0]['buildInfoFingerprint'] == sys.argv[3]
    observed = [info['parsedBuildInfo'] for info in infos]
print(json.dumps(observed))
`, dirname(script), folder, appleTextFingerprint], { input: readFileSync(report), encoding: 'utf8' });
        assert.deepEqual(JSON.parse(python), expected);
        assert.equal(readdirSync(folder).some(name => name.startsWith('native-buildinfo-')), false);
      }
      for (const name of vendors) {
        const changed = structuredClone(target);
        delete changed.facts.modules[name];
        if (requiredNative.includes(name)) assert.throws(() => verifyMobileBuildFixture(changed), /required local source module/);
        else verifyMobileBuildFixture(changed);
      }
      for (const name of vendors.filter(name => !requiredNative.includes(name))) delete target.facts.modules[name];
      verifyMobileBuildFixture(target);
    }
    const mixed = verify(input('ios'), undefined, evidence => {
      evidence.appleObservations[0].raw += '\n\tbuild\ttext=duplicate';
    });
    assert.equal(mixed.status, 1);
    assert.equal(mixed.stdout, '');
    assert.match(mixed.stderr, /Malformed\/duplicate build setting/);
    const misbound = verify(input('ios'), undefined, evidence => {
      evidence.appleObservations[1].actualId = 'other-carrier';
    });
    assert.equal(misbound.status, 1);
    assert.equal(misbound.stdout, '');
    assert.match(misbound.stderr, /BuildID input differs/);
    const mutated = verify(input(), bytes => writeFileSync(report, Buffer.from(bytes.toString().replace('gobind', 'gobinx'))));
    assert.equal(mutated.status, 1);
    assert.match(mutated.stderr, /native BuildInfo evidence pin differs/);
    const linked = verify(input(), () => {
      const original = join(folder, 'original.json');
      copyFileSync(report, original);
      rmSync(report);
      symlinkSync(original, report);
    });
    assert.equal(linked.status, 1);
    assert.match(linked.stderr, /bounded regular original/);
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});

test('mobile BuildInfo rejects wrong settings, platform tags, BuildID, and exact linker flags', { skip: process.platform === 'win32' }, () => {
  const folder = mkdtempSync(join(tmpdir(), 'mobile-buildinfo-settings-'));
  try {
    const { input } = mobileBuildFixture(folder);
    for (const [field, bad] of Object.entries({ GOOS: 'linux', GOARCH: 'amd64', CGO_ENABLED: '0',
      '-buildmode': 'exe', '-trimpath': 'false', '-tags': 'android' })) {
      const changed = input();
      changed.facts.settings[field] = bad;
      assert.throws(() => verifyMobileBuildFixture(changed), /setting differs|feature tags differ/);
    }
    const wrongVersion = input();
    wrongVersion.facts.goVersion = '1.27.5';
    assert.throws(() => verifyMobileBuildFixture(wrongVersion), /Go version differs/);
    for (const changed of [input(), input('ios'), input('ios', 'arm64', true)]) {
      changed.facts.settings['-tags'] = changed.target.simulator
        ? changed.facts.settings['-tags'].replace(',iossimulator', '')
        : changed.facts.settings['-tags'] + ',iossimulator';
      assert.throws(() => verifyMobileBuildFixture(changed), /feature tags differ/);
    }
    for (const actualId of ['', 'CLI-BuildID', 'polaris-mobile-' + 'b'.repeat(64)]) {
      assert.throws(() => verifyMobileBuildFixture({ ...input(), actualId }), /BuildID input differs/);
    }
    const observed = input();
    const retirementFlag = ' -X github.com/sagernet/sing-box/constant.DirectoryRetirementSourceIdentity=' + observed.source.sourceCommit;
    const flags = '-X github.com/sagernet/sing-box/constant.Version=' + observed.source.version + retirementFlag
      + ' -X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0 -s -w -buildid=' + nativeId;
    verifyMobileBuildFixture(observed);
    observed.facts.settings['-ldflags'] = flags;
    verifyMobileBuildFixture(observed);
    for (const bad of [flags.replace(retirementFlag, ''), flags.replace(observed.source.sourceCommit, '0'.repeat(40))]) {
      observed.facts.settings['-ldflags'] = bad;
      assert.throws(() => verifyMobileBuildFixture(observed), /ldflags differ/);
    }
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});

test('native local modules and the complete pruned source graph retain exact ownership', { skip: process.platform === 'win32' }, () => {
  const folder = mkdtempSync(join(tmpdir(), 'mobile-buildinfo-graph-'));
  try {
    const { input, vendors } = mobileBuildFixture(folder);
    for (const name of vendors) {
      for (const [field, value] of Object.entries({ version: 'v9.9.9', replacement: './third_party/stock',
        replacementVersion: 'v1.0.0', replacementSum: 'h1:invented', sum: 'h1:invented', kind: 'mod' })) {
        const changed = input();
        changed.facts.modules[name][field] = value;
        assert.throws(() => verifyMobileBuildFixture(changed), undefined, name + ' ' + field);
      }
    }
    const local = input();
    const name = 'github.com/sagernet/sing-tun';
    const declared = local.graph.find(row => row.Path === name).Replace;
    for (const alias of [declared.Path, declared.Dir]) {
      for (const version of ['', '(devel)']) {
        Object.assign(local.facts.modules[name], { replacement: alias, replacementVersion: version });
        verifyMobileBuildFixture(local);
      }
    }
    for (const alias of ['third_party/sing-tun', '../third_party/sing-tun', './third_party/sing-tun/',
      './third_party/../third_party/sing-tun', join(folder, 'outside/sing-tun')]) {
      local.facts.modules[name].replacement = alias;
      assert.throws(() => verifyMobileBuildFixture(local), /observed=.*declaredPath=.*declaredDir=/);
    }
    local.facts.modules[name].replacement = declared.Path;
    local.facts.modules[name].replacementVersion = 'devel';
    assert.throws(() => verifyMobileBuildFixture(local), /replacement version differs/);

    const pruned = 'github.com/sagernet/fswatch';
    const graphMutations = [
      (rows, index) => rows.splice(index, 1), (rows, index) => rows.push(structuredClone(rows[index])),
      (rows, index) => { rows[index].Version = 'v9.9.9'; }, (rows, index) => { rows[index].Sum = 'h1:invented'; },
      ...['../outside', './../outside', './third_party/../fswatch', '/absolute', './third_party//fswatch']
        .map(path => (rows, index) => { rows[index].Replace.Path = path; }),
      (rows, index) => { rows[index].Replace.Dir = join(folder, 'third_party/sing'); },
      (rows, index) => { rows[index].Replace.Sum = 'h1:invented'; }, (rows, index) => { delete rows[index].Replace; },
      (rows, index) => { rows[index].Main = true; },
      rows => rows.push({ Path: 'example.invalid/extra', Replace: { Path: './extra', Dir: join(folder, 'extra') } }),
    ];
    for (const mutate of graphMutations) {
      const changed = input();
      delete changed.facts.modules[pruned];
      mutate(changed.graph, changed.graph.findIndex(row => row.Path === pruned));
      assert.throws(() => verifyMobileBuildFixture(changed));
    }
    for (const escaped of [name, pruned]) {
      const changed = input();
      delete changed.facts.modules[pruned];
      const directory = changed.graph.find(row => row.Path === escaped).Replace.Dir;
      rmSync(directory, { recursive: true });
      symlinkSync(dirname(folder), directory, 'dir');
      assert.throws(() => verifyMobileBuildFixture(changed), /local graph path differs/);
      rmSync(directory);
      mkdirSync(directory);
    }
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});

test('native BuildInfo keeps ordinary and remote replacement identity and exact generated source alias', { skip: process.platform === 'win32' }, () => {
  const folder = mkdtempSync(join(tmpdir(), 'mobile-buildinfo-modules-'));
  try {
    const { input } = mobileBuildFixture(folder);
    const name = 'example.org/module';
    for (const [field, value] of Object.entries({ version: 'v9.9.9', sum: 'h1:other', replacement: '/unowned/source' })) {
      const changed = input();
      changed.facts.modules[name][field] = value;
      assert.throws(() => verifyMobileBuildFixture(changed));
    }
    const unreviewed = input();
    unreviewed.facts.modules['example.invalid/unreviewed'] = { kind: 'dep', version: 'v1.0.0' };
    assert.throws(() => verifyMobileBuildFixture(unreviewed), /not in actual source graph/);
    const wrongCore = input();
    wrongCore.facts.modules[wrongCore.source.goModule].replacement += '/elsewhere';
    assert.throws(() => verifyMobileBuildFixture(wrongCore), /exact verified source module/);
    const replacement = input('ios');
    replacement.graph[0].Replace = { Path: 'example.org/replaced', Version: 'v1.4.5', Sum: 'h1:replaced' };
    const original = { replacement: 'example.org/replaced', replacementVersion: 'v1.4.5', replacementSum: 'h1:replaced' };
    Object.assign(replacement.facts.modules[name], original);
    verifyMobileBuildFixture(replacement);
    for (const [field, value] of [['replacement', './example.org/replaced'], ['replacementVersion', 'v9.0.0'],
      ['replacementSum', 'h1:other'], ['replacementVersion', '(devel)'], ['version', 'v9.0.0']]) {
      const changed = structuredClone(replacement);
      changed.facts.modules[name][field] = value;
      assert.throws(() => verifyMobileBuildFixture(changed));
    }
    delete replacement.graph[0].Replace.Path;
    delete replacement.facts.modules[name].replacement;
    assert.throws(() => verifyMobileBuildFixture(replacement), /native replacement differs/);
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});

test('Android textual Go BuildInfo preserves original rows including settings and rejects malformed records', () => {
  const header = 'lib path/libbox.so: go1.27.4';
  const rows = ['\tpath\tgobind', '\tmod\tgobind\t(devel)\t',
    '\tdep\tmodule\tv1.2.3\th1:original', '\t=>\t./third_party/module\t(devel)\t', '\t',
    '\tbuild\tGOOS=android', '\tbuild\t-ldflags="flag \\"value\\"=path"', '\tbuild\tkey=left=right'];
  assert.deepEqual(parseMobileBuildInfo([header, ...rows].join('\r\n') + '\u0085 '), {
    goVersion: '1.27.4', path: 'gobind',
    modules: { gobind: { kind: 'mod', version: '(devel)' },
      module: { kind: 'dep', version: 'v1.2.3', sum: 'h1:original', replacement: './third_party/module', replacementVersion: '(devel)' } },
    settings: { GOOS: 'android', '-ldflags': 'flag "value"=path', key: 'left=right' },
  });
  for (const wrong of ['', 'libbox.so: devel', header + '\ufeff',
    ...['\tunknown\tvalue', '\t=>\t./unbound', '\tdep\tmissing-version', '\tbuild\t-x',
      '\tbuild\t=x', '\tbuild\tkey="missing close', '\tbuild\tkey=duplicate', '\tpath\tsecond',
      '\tdep\tmodule\tv1.2.3', 'first\tdep\tmodule\tv1.2.3'].map(row => [header, ...rows, row].join('\n'))]) {
    assert.throws(() => parseMobileBuildInfo(wrong));
  }
});

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
