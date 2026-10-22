import assert from 'node:assert/strict';
import test from 'node:test';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { validateRequest, validatePin, expectedBuildVcs, recordBuildInfo, validateBuild, validateTestEvidence, requireSameAppSource, APP_SOURCE_CHECK, OFFICIAL_GO_SETUP, RETAINED_CANDIDATE, retainVerifiedCandidate, withResultEvidence, ADMISSION_TESTS, TESTS, REF, REPOSITORY, PROFILE, PROTO } from './runtime-config-loopback.mjs';
import { exactPass } from './run-native-dns-controlled-tests.mjs';

// Pure data/source fixtures. Importing the runner must never execute its entrypoint.
const app = 'a'.repeat(40), kernel = 'b'.repeat(40);
const inputs = { runtime_config_loopback_candidate: app, app_native_compile_candidate: '', runtime_config_source_only: false,
  native_dns_candidate: '', windows_system_candidate: '', windows_launcher_diagnostics: false, windows_directory_mode: 'formal',
  windows_directory_candidate_source: '', preserve_run: true, os: 'ubuntu-22.04' };
const env = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: REPOSITORY, GITHUB_REPOSITORY_ID: '1347028220',
  GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REF: REF, GITHUB_WORKFLOW_REF: `${REPOSITORY}/.github/workflows/ci.yml@${REF}`,
  GITHUB_SHA: app, GITHUB_WORKFLOW_SHA: app, RUNNER_OS: 'Linux', RUNNER_ARCH: 'X64', GITHUB_JOB: 'cheap_format',
  GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1' };
const event = (value = inputs) => ({ ref: REF, repository: { full_name: REPOSITORY, id: 1347028220 },
  inputs: Object.fromEntries(Object.entries(value).map(([key, val]) => [key, String(val)])) });
const pin = { schema: 'polaris-runtime-loopback-kernel-v1', status: 'reviewed', profile: PROFILE,
  source: { repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, commit: kernel, tree: 'c'.repeat(40),
    files: { 'daemon/runtime_config.proto': PROTO, '.github/go-toolchains.json': 'd'.repeat(64), '.github/setup_legacy_go.py': 'e'.repeat(64) } }, goVersion: '1.27.2', tags: 'with_gvisor,with_purego',
  version: `polaris-hot-loopback-${kernel.slice(0, 12)}`, protocol: 'polaris-daily-runtime-v1', platformBuild: 'linux/amd64',
  implementationManifestDigest: null, naiveIncluded: false, publicationEligible: false };

test('exact direct dispatch is admitted on both expected Linux jobs', () => {
  for (const GITHUB_JOB of ['cheap_format', 'runtime_config_loopback'])
    assert.equal(validateRequest({ ...env, GITHUB_JOB }, inputs, event()), app);
});
for (const [key, value] of Object.entries({ GITHUB_ACTIONS: 'false', GITHUB_EVENT_NAME: 'workflow_call',
  GITHUB_REPOSITORY: 'other/Polaris', GITHUB_REPOSITORY_ID: '1', GITHUB_REF: 'refs/heads/main',
  GITHUB_WORKFLOW_REF: `${REPOSITORY}/.github/workflows/package.yml@${REF}`, GITHUB_SHA: kernel,
  GITHUB_WORKFLOW_SHA: kernel, RUNNER_OS: 'Windows', RUNNER_ARCH: 'ARM64', GITHUB_JOB: 'test',
  GITHUB_RUN_ID: '0', GITHUB_RUN_ATTEMPT: '' })) {
  test(`reject context mismatch ${key}`, () => assert.throws(() => validateRequest({ ...env, [key]: value }, inputs, event())));
}
for (const selected of [{ runtime_config_source_only: true }, { app_native_compile_candidate: app },
  { native_dns_candidate: app }, { windows_system_candidate: app }, { windows_launcher_diagnostics: true },
  { windows_directory_candidate_source: kernel }, { windows_directory_mode: 'candidate' }, { os: 'all' },
  { preserve_run: false }, { arbitrary_kernel: kernel }, { runtime_config_loopback_candidate: 'main' }]) {
  test(`reject conflicting input ${Object.keys(selected)[0]}`, () => {
    const value = { ...inputs, ...selected };
    assert.throws(() => validateRequest(env, value, event(value)));
  });
}
test('event full branch ref, repository and actual string inputs are mandatory', () => {
  for (const changed of [{ ...event(), ref: REF.slice('refs/heads/'.length) },
    { ...event(), repository: { full_name: REPOSITORY, id: 1 } }, { ...event(), inputs: inputs },
    { ...event(), inputs: { ...event().inputs, preserve_run: 'false' } }])
    assert.throws(() => validateRequest(env, inputs, changed));
});
for (const key of ['POLARIS_SINGBOX_PATH', 'POLARIS_RUNTIME_LOOPBACK_RECEIPT', 'CORE_CANDIDATE', 'GOFLAGS', 'GOWORK',
  'CGO_ENABLED', 'GOOS', 'GOHOSTOS', 'GOHOSTARCH', 'GOAMD64', 'GOFIPS140', 'GOPROXY', 'RUSTFLAGS', 'RUSTC_WRAPPER', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET',
  'GIT_CONFIG_COUNT', 'NODE_OPTIONS', 'BASH_ENV', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'PYTHONPATH']) {
  test(`reject inherited selector ${key}`, () => assert.throws(() => validateRequest({ ...env, [key]: 'injected' }, inputs, event(), true)));
}
test('setup-go local is accepted only in the prepared stage', () => {
  assert.throws(() => validateRequest({ ...env, GOTOOLCHAIN: 'local' }, inputs, event()));
  assert.equal(validateRequest({ ...env, GOTOOLCHAIN: 'local' }, inputs, event(), true), app);
  assert.throws(() => validateRequest({ ...env, GOTOOLCHAIN: 'auto' }, inputs, event(), true));
});
test('partial-profile pin cannot issue release or complete-feature acceptance', () => {
  assert.equal(validatePin(pin), pin);
  for (const change of [{ status: 'pending-kernel-source-review' }, { source: null }, { publicationEligible: true },
    { naiveIncluded: true }, { profile: 'formal-linux' }, { tags: 'with_naive_outbound,with_purego' },
    { version: '1.13.0-polaris-alpha.12' }, { implementationManifestDigest: 'invented' }])
    assert.throws(() => validatePin({ ...pin, ...change }));
});
// Synthetic Go output, not a claim about the first CI run's missing raw output.
// The module-list root has no Version; Go 1.24+ gives the binary a VCS version.
const buildVcs = { commit: kernel, tree: pin.source.tree, commitTime: '1704067200', tags: '', shallow: 'true' };
const mainVersion = `v0.0.0-20240101000000-${kernel.slice(0, 12)}`;
const moduleGraph = [{ Path: 'github.com/sagernet/sing-box', Main: true, GoVersion: '1.27.0' },
  { Path: 'example.org/module', Version: 'v1.0.0', Sum: 'h1:fixture' }];
const buildRaw = `/fixture/sing-box: go1.27.2\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n`
  + `\tmod\tgithub.com/sagernet/sing-box\t${mainVersion}\n\tdep\texample.org/module\tv1.0.0\th1:fixture\n`
  + Object.entries({ GOOS: 'linux', GOARCH: 'amd64', GOAMD64: 'v1', CGO_ENABLED: '0', '-buildmode': 'exe', '-compiler': 'gc',
    '-trimpath': 'true', vcs: 'git', 'vcs.revision': kernel, 'vcs.time': '2024-01-01T00:00:00Z', 'vcs.modified': 'false',
    '-tags': pin.tags }).map(([key, value]) => `\tbuild\t${key}=${value}\n`).join('');

test('untagged pseudo-version is derived only from independently pinned Git metadata', () => {
  assert.deepEqual(expectedBuildVcs(pin, buildVcs), { revision: kernel, time: '2024-01-01T00:00:00Z', moduleVersion: mainVersion });
  for (const changed of [undefined, { ...buildVcs, commit: app }, { ...buildVcs, tree: app },
    { ...buildVcs, tags: 'refs/tags/v1.0.0' }, { ...buildVcs, shallow: 'false' }])
    assert.throws(() => expectedBuildVcs(pin, changed), /Exact untagged shallow Git identity/);
  for (const commitTime of [1704067200, '', '(devel)', '2024-01-01T00:00:00Z', '01704067200', '-1', '1e9',
    '1704067200\n', '999999999999', '9007199254740993'])
    assert.throws(() => expectedBuildVcs(pin, { ...buildVcs, commitTime }), /Git commit timestamp/);
});
test('build metadata binds exact source, profile and module graph using Go VCS semantics', () => {
  const facts = validateBuild(buildRaw, pin, moduleGraph, buildVcs);
  assert.equal(facts.goVersion, '1.27.2');
  assert.equal(facts.modules['github.com/sagernet/sing-box'].version, mainVersion);
  // Go omits -ldflags with -trimpath, and records -buildvcs via vcs.* settings.
  assert.equal(facts.settings['-ldflags'], undefined);
  assert.equal(facts.settings['-buildvcs'], undefined);
  for (const [before, after] of [['vcs.modified=false', 'vcs.modified=true'], [`vcs.revision=${kernel}`, `vcs.revision=${app}`],
    ['vcs.time=2024-01-01T00:00:00Z', 'vcs.time=2024-01-01T00:00:01Z'], ['vcs=git', 'vcs=hg'],
    ['CGO_ENABLED=0', 'CGO_ENABLED=1'], ['h1:fixture', 'h1:other'], ['v1.0.0', 'v1.0.1'], ['go1.27.2', 'go1.27.1'],
    ['GOARCH=amd64', 'GOARCH=arm64'], ['GOAMD64=v1', 'GOAMD64=v3'], ['-buildmode=exe', '-buildmode=pie'],
    ['-compiler=gc', '-compiler=gccgo'], ['-trimpath=true', '-trimpath=false'], [pin.tags, 'with_naive_outbound']])
    assert.throws(() => validateBuild(buildRaw.replace(before, after), pin, moduleGraph, buildVcs), before);
  for (const key of ['vcs', 'vcs.revision', 'vcs.time', 'vcs.modified']) {
    const without = buildRaw.split('\n').filter((line) => !line.startsWith(`\tbuild\t${key}=`)).join('\n');
    assert.throws(() => validateBuild(without, pin, moduleGraph, buildVcs), /Embedded candidate build setting differs/);
  }
  assert.throws(() => validateBuild(buildRaw, pin, moduleGraph, { ...buildVcs, commitTime: '1704067201' }), /vcs.time/);
  const consistentWrongTime = buildRaw.replace('20240101000000', '20240101000001').replace('2024-01-01T00:00:00Z', '2024-01-01T00:00:01Z');
  assert.throws(() => validateBuild(consistentWrongTime, pin, moduleGraph, buildVcs), /vcs.time/);
});
test('main version rejects devel, tags, unrelated or dirty pseudo-versions and extra metadata', () => {
  for (const value of ['(devel)', 'v1.0.0', 'arbitrary', `v0.0.0-20240101000001-${kernel.slice(0, 12)}`,
    `v0.0.0-20240101000000-${app.slice(0, 12)}`, `v1.0.0-20240101000000-${kernel.slice(0, 12)}`,
    mainVersion + '+dirty', mainVersion + '+incompatible', mainVersion + '\th1:unexpected'])
    assert.throws(() => validateBuild(buildRaw.replace(mainVersion, value), pin, moduleGraph, buildVcs), /Executable main source differs/);
  const replacement = buildRaw.replace(`\t${mainVersion}\n`, `\t${mainVersion}\n\t=>\t./other\t(devel)\n\t\n`);
  assert.throws(() => validateBuild(replacement, pin, moduleGraph, buildVcs), /Executable main source differs/);
});
test('main package, module root and module kinds are unique and exact', () => {
  const path = '\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n';
  for (const changed of [buildRaw.replace(path, ''), buildRaw.replace(path, path + path),
    buildRaw + '\tpath\n', buildRaw + '\tpath\r\n',
    buildRaw.replace('/cmd/sing-box\n', '/cmd/other\n'), buildRaw.replace(path, path.trimEnd() + '\textra\n')])
    assert.throws(() => validateBuild(changed, pin, moduleGraph, buildVcs), /Executable main package path differs/);
  for (const changed of [buildRaw.replace('\tmod\t', '\tdep\t'), buildRaw.replace('\tdep\t', '\tmod\t'),
    buildRaw.replace('\tmod\tgithub.com/sagernet/sing-box\t', '\tmod\texample.org/other\t')])
    assert.throws(() => validateBuild(changed, pin, moduleGraph, buildVcs), /Unique source and executable main modules/);
  for (const graph of [moduleGraph.slice(1), [...moduleGraph, moduleGraph[0]],
    [moduleGraph[0], { ...moduleGraph[1], Main: true }], [{ ...moduleGraph[0], Main: false }, moduleGraph[1]],
    [{ ...moduleGraph[0], Version: 'v1.0.0' }, moduleGraph[1]], [{ ...moduleGraph[0], Sum: 'h1:unexpected' }, moduleGraph[1]],
    [{ ...moduleGraph[0], Replace: { Path: './other' } }, moduleGraph[1]]])
    assert.throws(() => validateBuild(buildRaw, pin, graph, buildVcs), /Unique source and executable main modules/);
  assert.throws(() => validateBuild(buildRaw.replace('\tdep\t', '\tmod\t'), pin,
    [moduleGraph[0], { ...moduleGraph[1], Main: true }], buildVcs), /Unique source and executable main modules/);
});
test('module h1 and local or versioned replacement guards still bind every embedded dependency', () => {
  const local = { Path: 'example.org/local', Version: 'v1.2.3', Replace: { Path: './third_party/local' } };
  const remote = { Path: 'example.org/remote', Version: 'v1.2.3', Replace: { Path: 'example.org/replacement', Version: 'v1.2.4', Sum: 'h1:replacement' } };
  const graph = [...moduleGraph, local, remote];
  const extra = '\tdep\texample.org/local\tv1.2.3\n\t=>\t./third_party/local\t(devel)\n\t\n'
    + '\tdep\texample.org/remote\tv1.2.3\n\t=>\texample.org/replacement\tv1.2.4\th1:replacement\n';
  const raw = buildRaw + extra;
  assert.equal(Object.keys(validateBuild(raw, pin, graph, buildVcs).modules).length, 4);
  for (const [before, after] of [['./third_party/local', './third_party/other'], ['(devel)', 'v1.0.0'],
    ['example.org/replacement', 'example.org/unreviewed'], ['v1.2.4', 'v1.2.5'], ['h1:replacement', 'h1:other'],
    ['\t(devel)\n', '\t(devel)\th1:unexpected\n']])
    assert.throws(() => validateBuild(raw.replace(before, after), pin, graph, buildVcs), /Dependency replacement differs/);
  assert.throws(() => validateBuild(raw, pin, moduleGraph, buildVcs), /Embedded dependency absent/);
  const unsigned = buildRaw.replace('\th1:fixture', '');
  assert.throws(() => validateBuild(unsigned, pin, [moduleGraph[0], { Path: 'example.org/module', Version: 'v1.0.0' }], buildVcs), /Dependency h1 differs/);
  assert.throws(() => validateBuild(buildRaw + '\tbuild\tvcs.modified=false\n', pin, moduleGraph, buildVcs), /Duplicate embedded build setting/);
  assert.throws(() => validateBuild(buildRaw + `\tmod\tgithub.com/sagernet/sing-box\t${mainVersion}\n`, pin, moduleGraph, buildVcs), /Duplicate\/malformed embedded module/);
});
test('raw BuildInfo survives admission or parse failure with exact bytes, hash and no overwrite', () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'loopback-build-info-')));
  try {
    for (const [name, raw] of [['identity', buildRaw.replace(mainVersion, '(devel)')], ['parse', 'unparseable raw output\r\n']]) {
      const path = join(directory, `${name}.txt`);
      const result = { sourceVerified: false, tests: [] };
      result.buildInfo = recordBuildInfo(raw, path);
      assert.throws(() => validateBuild(raw, pin, moduleGraph, buildVcs));
      assert.equal(readFileSync(path, 'utf8'), raw);
      assert.deepEqual(result.buildInfo, { path, bytes: Buffer.byteLength(raw), sha256: createHash('sha256').update(raw).digest('hex') });
      assert.equal(statSync(path).mode & 0o777, 0o600);
      assert.throws(() => recordBuildInfo('replacement', path), /EEXIST/);
      assert.equal(readFileSync(path, 'utf8'), raw);
      assert.equal(result.sourceVerified, false); assert.deepEqual(result.tests, []);
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
  const source = readFileSync(fileURLToPath(new URL('./runtime-config-loopback.mjs', import.meta.url)), 'utf8');
  const write = source.indexOf("result.buildInfo = recordBuildInfo(buildInfo, join(evidence, 'kernel-build-info.txt'))");
  const validate = source.indexOf('result.build = validateBuild(buildInfo, pin, graph.graph, buildVcs)');
  assert.ok(write > 0 && write < validate && validate < source.indexOf('result.sourceVerified = true'));
  assert.ok(source.indexOf('result.observedBinary = filePin(binary)') < validate);
  assert.ok(source.includes("'--format=%H%n%T%n%ct'"));
  assert.ok(source.includes("'for-each-ref', '--format=%(refname)', 'refs/tags'"));
  assert.ok(source.includes("'rev-parse', '--is-shallow-repository'"));
  assert.ok(source.includes("JSON.stringify(readBuildVcs(source)) === JSON.stringify(buildVcs)"));
  assert.ok(source.includes("JSON.stringify(filePin(binary)) === JSON.stringify(result.observedBinary)"));
  assert.ok(source.includes('binary: result.observedBinary, sourceVerified: true'));
  assert.ok(source.indexOf('Observed executable changed during source verification') < source.indexOf('result.sourceVerified = true'));
});
test('exact four real tests require one executed pass, no skip or uncertain cleanup', () => {
  assert.equal(TESTS.length, 4); assert.equal(new Set(TESTS).size, 4);
  const value = { status: 0, output: 'test result: ok. 1 passed; 0 failed; 0 ignored; 10 filtered out;' };
  assert.equal(exactPass(value), true);
  for (const changed of [{ ...value, cleanupUncertain: true }, { ...value, status: 1 },
    { ...value, output: 'test result: ok. 0 passed; 0 failed; 1 ignored;' }, { ...value, output: value.output + '\n' + value.output }])
    assert.equal(exactPass(changed), false);
});
test('test evidence must prove attributed start, actual manifest and owned cleanup', () => {
  const receipt = { appCommit: app, kernelCommit: kernel, kernelTree: 'c'.repeat(40), binary: { sha256: 'd'.repeat(64) },
    implementationManifestDigest: null };
  const value = { schema: 'polaris-runtime-loopback-test-v1', test: TESTS[0].split('::').at(-1), passed: true,
    profile: PROFILE, publicationEligible: false, ...receipt, coreSha256: receipt.binary.sha256,
    events: [{ phase: 'started', pid: 42, capabilities: { contractVersion: 1, enabled: true, implementationManifestDigest: 'e'.repeat(64) } },
      { phase: 'stopped', pid: 42, ownedChildReaped: true }] };
  assert.equal(validateTestEvidence(value, TESTS[0], receipt), 'e'.repeat(64));
  for (const mutation of [{ passed: false }, { kernelCommit: app }, { events: [] }, { coreSha256: 'a'.repeat(64) },
    { publicationEligible: true }, { test: 'another_test' }])
    assert.throws(() => validateTestEvidence({ ...value, ...mutation }, TESTS[0], receipt));
  assert.throws(() => validateTestEvidence(value, TESTS[0], { ...receipt, implementationManifestDigest: 'a'.repeat(64) }));
});
test('runner remains one scoped source build, actual dispatcher libtests and artifact-only evidence', () => {
  const source = readFileSync(fileURLToPath(new URL('./runtime-config-loopback.mjs', import.meta.url)), 'utf8');
  assert.match(source, /source_facts\(source,identity=pin\['source'\]\)/);
  assert.match(source, /graph_facts\(graph,source,identity=pin\['source'\]\)/);
  assert.ok(source.includes("['test', '--locked', '-p', 'polaris', '--lib', '--no-run', '--message-format=json']"));
  assert.ok(source.includes("args: [name, '--exact', '--ignored', '--nocapture', '--test-threads=1']"));
  assert.match(source, /exactPass\(observed\)/);
  for (const forbidden of ['sudo ', 'netns ', 'sdkmanager', 'gh release', 'fetch-core.mjs', '--no-sandbox', 'core-manifest.json'])
    assert.ok(!source.includes(forbidden), forbidden);
});


test('App source admission and build/test mirrors reject source drift', () => {
  const before = { commit: app, tree: kernel, leaves: 3, gitlinks: [], inventorySha256: 'a'.repeat(64) };
  assert.equal(requireSameAppSource(before, before), before);
  for (const mutation of [{ commit: kernel }, { tree: app }, { leaves: 2 }, { inventorySha256: 'b'.repeat(64) },
    { gitlinks: [{ path: 'nested', commit: kernel }] }])
    assert.throws(() => requireSameAppSource(before, { ...before, ...mutation }));
  assert.ok(APP_SOURCE_CHECK.includes("m.clean(root)"));
  assert.ok(APP_SOURCE_CHECK.includes("m.tracked_mirror(root)"));
  assert.ok(APP_SOURCE_CHECK.includes("m.git(root,'ls-files','-v')"));
  const source = readFileSync(fileURLToPath(new URL('./runtime-config-loopback.mjs', import.meta.url)), 'utf8');
  assert.match(source, /appSourceAfterBuild = requireSameAppSource\(appSource, appSourceCheck\('tracked'\)\)/);
  assert.match(source, /appSourceAfterTests = requireSameAppSource\(appSource, appSourceCheck\('tracked'\)\)/);
  assert.ok(source.indexOf("const appSource = appSourceCheck('clean')") < source.indexOf("for (const platform of"));
});

test('App checker pure fixtures close ignored, untracked, index-flag and tracked-byte/mode gaps', () => {
  const root = fileURLToPath(new URL('../', import.meta.url));
  const fixture = String.raw`import hashlib,importlib.util,json,os,stat,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
sys.dont_write_bytecode=True
sys.path.insert(0,str(Path(sys.argv[1])/'scripts/mobile-core'))
spec=importlib.util.spec_from_file_location('candidate_source',Path(sys.argv[1])/'scripts/mobile-core/ci-hot-candidate.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def rejected(call):
 try:call()
 except RuntimeError:return
 raise AssertionError('unsafe fixture accepted')
with patch.object(subprocess,'Popen',side_effect=AssertionError('fixture cannot execute tools')),tempfile.TemporaryDirectory() as directory:
 root=Path(directory);path=root/'source.txt';path.write_bytes(b'original');path.chmod(0o644)
 blob=hashlib.sha1(b'blob 8\0original').hexdigest()
 tree='100644 blob '+blob+'\tsource.txt'
 state={'flags':'H source.txt','others':'','ignored':'','status':''}
 def git(root,*args):
  if args==('ls-tree','-r','HEAD'):return tree
  if args==('ls-files','-v'):return state['flags']
  if args==('ls-files','--others','--exclude-standard'):return state['others']
  if args==('ls-files','--others','--ignored','--exclude-standard'):return state['ignored']
  if 'status' in args:return state['status']
  raise AssertionError(args)
 with patch.object(m,'git',side_effect=git):
  m.clean(root);before=m.tracked_mirror(root)
  for key,value in [('flags','S source.txt'),('flags','h source.txt'),('others','hidden.rs'),('ignored','resources/linux/injected'),('status',' M source.txt')]:
   old=state[key];state[key]=value;rejected(lambda:m.clean(root));state[key]=old
  # The post-build path deliberately tolerates newly generated resources, while
  # all HEAD-tracked bytes and executable modes remain immutable.
  state['ignored']='resources/linux/.keep';assert m.tracked_mirror(root)==before
  path.write_bytes(b'changed');rejected(lambda:m.tracked_mirror(root))
  path.write_bytes(b'original');path.chmod(0o755);rejected(lambda:m.tracked_mirror(root))
  path.chmod(0o644);path.unlink();path.symlink_to('/dev/null');rejected(lambda:m.tracked_mirror(root))
print('App source fixtures passed')
`;
  const result = execFileSync('python3', ['-I', '-c', fixture, root], { encoding: 'utf8', timeout: 20_000 });
  assert.match(result, /App source fixtures passed/);
});

test('official Go uses a pinned safe archive, complete private tree and never setup-go provenance', () => {
  const source = readFileSync(fileURLToPath(new URL('./runtime-config-loopback.mjs', import.meta.url)), 'utf8');
  assert.ok(!source.includes("capture('which', ['go'])"));
  assert.ok(source.includes("go = join(goRoot, 'bin/go')"));
  assert.ok(source.includes("JSON.stringify(goAfter) === JSON.stringify(result.officialGo)"));
  assert.ok(OFFICIAL_GO_SETUP.includes('helper.require_archive_members(members)'));
  assert.ok(OFFICIAL_GO_SETUP.includes('helper.toolchain_files(root)'));
  assert.ok(!OFFICIAL_GO_SETUP.includes('helper.main('));
  const fixture = String.raw`import copy,hashlib,io,json,os,stat,subprocess,sys,tarfile,tempfile,types,urllib.request
from pathlib import Path,PurePosixPath
from unittest.mock import patch
ns={'__name__':'fixture'};exec(compile(sys.argv[1],'<official-go-fixture>','exec'),ns)
ns['urlopen']=lambda *args,**kwargs:(_ for _ in ()).throw(AssertionError('fixture cannot use network'))
def rejected(call):
 try:call()
 except RuntimeError:return
 raise AssertionError('unsafe fixture accepted')
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def inventory(root):
 result={}
 for path in root.rglob('*'):
  if path.is_symlink():raise RuntimeError('symlink')
  if path.is_file():result[path.relative_to(root).as_posix()]={'sha256':digest(path),'bytes':path.stat().st_size,'mode':stat.S_IMODE(path.stat().st_mode)}
 return result
def require_members(members):
 for m in members:
  p=PurePosixPath(m.name)
  if p.is_absolute() or '..' in p.parts or p.parts[0]!='go' or not(m.isfile() or m.isdir()):raise RuntimeError('layout')
helper=types.SimpleNamespace(file_hash=digest,toolchain_files=inventory,require_archive_members=require_members)
required={'bin/go':b'fixture-not-executable','bin/gofmt':b'fixture','VERSION':b'go1.27.2\n','go.env':b'fixture',
 'src/runtime/runtime2.go':b'fixture','src/cmd/compile/main.go':b'fixture','src/cmd/asm/main.go':b'fixture',
 'pkg/tool/linux_amd64/compile':b'fixture','pkg/tool/linux_amd64/asm':b'fixture','pkg/tool/linux_amd64/link':b'fixture'}
def archive_file(path,extra=None):
 with tarfile.open(path,'w:gz') as tar:
  for name,body in required.items():
   member=tarfile.TarInfo('go/'+name);member.size=len(body);member.mode=0o755 if name.startswith(('bin/','pkg/')) else 0o644
   tar.addfile(member,io.BytesIO(body))
  if extra:
   member=tarfile.TarInfo(extra[0]);member.mode=extra[1]
   if len(extra)>2:member.type=extra[2];member.linkname='/tmp/escape'
   tar.addfile(member,io.BytesIO(b''))
 return {'filename':path.name,'sha256':digest(path),'bytes':path.stat().st_size}
with patch.object(subprocess,'Popen',side_effect=AssertionError('fixture cannot execute tools')),patch.object(urllib.request,'urlopen',side_effect=AssertionError('fixture cannot use network')),tempfile.TemporaryDirectory() as directory:
 root=Path(directory);download=root/'valid.tar.gz';archive=archive_file(download)
 measured=ns['extract_verified'](download,archive,root/'valid',helper)
 assert set(measured)==set(required) and measured['bin/go']['sha256']==hashlib.sha256(required['bin/go']).hexdigest()
 rejected(lambda:ns['extract_verified'](download,archive,root/'valid',helper))
 for change in [{'bytes':archive['bytes']+1},{'sha256':'0'*64}]:rejected(lambda:ns['extract_verified'](download,dict(archive,**change),root/'mismatch',helper))
 for index,extra in enumerate([('go/bin/go',0o644),('../escaped',0o644),('/tmp/escaped',0o644),('go/./unexpected',0o644),('go/world-writable',0o666),('go/setuid',0o4644),('go/link',0o644,tarfile.SYMTYPE),('go/hardlink',0o644,tarfile.LNKTYPE),('go/device',0o644,tarfile.CHRTYPE),('go/polaris-toolchain.json',0o644)]):
  file=root/('bad'+str(index)+'.tar.gz');bad=archive_file(file,extra)
  rejected(lambda:ns['extract_verified'](file,bad,root/('bad'+str(index)),helper))
 # Validate pinned source inputs separately, before any helper code is imported.
 source=root/'source';(source/'.github').mkdir(parents=True)
 manifest={'version':'1.27.2','officialCommit':'a'*40,'archives':{'linux-amd64':{'filename':'go1.27.2.linux-amd64.tar.gz','sha256':'ecbadb99091a3f46e31f5f934b068b1864eafa7995211b39eaddf76996045fe5','bytes':70590635}}}
 meta=source/'.github/go-toolchains.json';meta.write_text(json.dumps(manifest))
 verifier=source/'.github/setup_legacy_go.py';verifier.write_text('FIXTURE_ONLY = True\n')
 pin={'goVersion':'1.27.2','source':{'files':{'.github/go-toolchains.json':digest(meta),'.github/setup_legacy_go.py':digest(verifier)}}}
 got=ns['inputs'](source,pin);assert got[1]==manifest['archives']['linux-amd64'] and got[2].FIXTURE_ONLY
 for name in pin['source']['files']:
  changed=copy.deepcopy(pin);changed['source']['files'][name]='0'*64;rejected(lambda:ns['inputs'](source,changed))
 for change in [{'filename':'alternate.tar.gz'},{'sha256':'0'*64},{'bytes':1}]:
  changed=copy.deepcopy(manifest);changed['archives']['linux-amd64'].update(change);meta.write_text(json.dumps(changed));pin['source']['files']['.github/go-toolchains.json']=digest(meta)
  rejected(lambda:ns['inputs'](source,pin))
 # Exercise both production phases with an in-memory download; no fixture
 # executable is ever launched and actual network/process calls remain denied.
 working=root/'working';(working/'evidence').mkdir(parents=True)
 pin_path=root/'pin.json';pin_path.write_text(json.dumps(pin))
 class Download(io.BytesIO):
  def geturl(self):return 'https://dl.google.com/go/'+archive['filename']
 original_inputs=ns['inputs'];original_urlopen=ns['urlopen']
 ns['inputs']=lambda source,pin:(manifest,archive,helper)
 ns['urlopen']=lambda *args,**kwargs:Download(download.read_bytes())
 try:
  before=ns['main']('prepare',source,pin_path,working)
  assert ns['main']('verify',source,pin_path,working)==before
  extracted=working/'official-go/unpacked/go'
  assert json.loads((working/'evidence/official-go-files.json').read_text())==inventory(extracted)
  driver=extracted/'bin/go';original=driver.read_bytes();driver.write_bytes(b'changed')
  assert ns['main']('verify',source,pin_path,working)['filesSha256']!=before['filesSha256']
  driver.write_bytes(original);driver.chmod(0o644)
  assert ns['main']('verify',source,pin_path,working)['filesSha256']!=before['filesSha256']
  driver.chmod(0o755);marker=extracted/'polaris-toolchain.json';marker.write_text('{}')
  rejected(lambda:ns['main']('verify',source,pin_path,working))
  marker.unlink();assert ns['main']('verify',source,pin_path,working)==before
  rejected(lambda:ns['main']('prepare',source,pin_path,working))
 finally:ns['inputs']=original_inputs;ns['urlopen']=original_urlopen
print('Official Go archive fixtures passed')
`;
  const result = execFileSync('python3', ['-I', '-c', fixture, OFFICIAL_GO_SETUP], { encoding: 'utf8', timeout: 20_000 });
  assert.match(result, /Official Go archive fixtures passed/);
});

function retainedFixture() {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'loopback-retention-')));
  const evidence = join(directory, 'evidence'), bin = join(directory, 'bin');
  mkdirSync(evidence, { mode: 0o700 }); mkdirSync(bin, { mode: 0o700 });
  const binary = join(bin, 'sing-box'), receiptPath = join(evidence, 'candidate.json');
  const body = Buffer.from('fixture bytes, never executed\n');
  writeFileSync(binary, body, { mode: 0o755 }); chmodSync(binary, 0o755);
  const receipt = { schema: 'polaris-runtime-loopback-candidate-v1', appCommit: app,
    kernelCommit: kernel, kernelTree: 'c'.repeat(40), protoSha256: PROTO, profile: PROFILE,
    implementationManifestDigest: null, sourceVerified: true, naiveIncluded: false, publicationEligible: false,
    binary: { path: binary, bytes: body.length, sha256: createHash('sha256').update(body).digest('hex') } };
  writeFileSync(receiptPath, JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 });
  return { directory, evidence, binary, receiptPath, receipt, sourceVerified: true };
}

test('retention binds the unchanged original receipt and binary to a fixed portable tar', () => {
  const request = retainedFixture();
  try {
    const original = readFileSync(request.receiptPath);
    const retained = retainVerifiedCandidate(request);
    assert.deepEqual(readFileSync(request.receiptPath), original);
    assert.equal(retained.sourceBinaryPath, request.binary);
    assert.equal(retained.members['sing-box'].sha256, request.receipt.binary.sha256);
    assert.equal(retained.members['sing-box'].bytes, request.receipt.binary.bytes);
    assert.equal(retained.members['sing-box'].mode, 0o755);
    assert.equal(retained.members['candidate.json'].mode, 0o600);
    assert.equal(retained.members['candidate.json'].sha256, createHash('sha256').update(original).digest('hex'));
    assert.equal(retained.publicationEligible, false); assert.equal(retained.naiveIncluded, false);
    assert.equal(retained.profile, PROFILE); assert.equal(retained.sourceVerified, true);
    const archive = readFileSync(join(request.evidence, retained.archive.artifactMember));
    assert.equal(retained.archive.bytes, archive.length);
    assert.equal(retained.archive.sha256, createHash('sha256').update(archive).digest('hex'));
    assert.throws(() => retainVerifiedCandidate(request), /FileExistsError/);
  } finally { rmSync(request.directory, { recursive: true, force: true }); }
});

test('unverified candidates never produce a downloadable package', () => {
  const request = retainedFixture();
  try {
    assert.throws(() => retainVerifiedCandidate({ ...request, sourceVerified: false }), /Only source-verified/);
    assert.throws(() => retainVerifiedCandidate({ ...request, receipt: { ...request.receipt, sourceVerified: false } }), /Only source-verified/);
    assert.equal(existsSync(join(request.evidence, 'kernel-candidate-linux-amd64.tar')), false);
  } finally { rmSync(request.directory, { recursive: true, force: true }); }
});

test('actual failed-run finalizer retains a verified candidate without claiming runtime acceptance', async () => {
  const request = retainedFixture();
  try {
    const result = { status: 'failed', sourceVerified: true, tests: [], admissionTests: [],
      fullFeatureAcceptance: false, mobileAcceptance: false, publicationEligible: false };
    await assert.rejects(withResultEvidence(request.evidence, result, async () => {
      result.retainedCandidate = retainVerifiedCandidate(request);
      throw Error('fixture App compilation or runtime failed');
    }), /fixture App compilation or runtime failed/);
    const saved = JSON.parse(readFileSync(join(request.evidence, 'result.json')));
    assert.equal(saved.status, 'failed'); assert.equal(saved.failure, 'fixture App compilation or runtime failed');
    assert.equal(saved.sourceVerified, true); assert.equal(saved.retainedCandidateVerified, true);
    assert.deepEqual(saved.tests, []); assert.deepEqual(saved.admissionTests, []);
    assert.equal(saved.fullFeatureAcceptance, false); assert.equal(saved.mobileAcceptance, false);
    assert.equal(saved.publicationEligible, false);
    assert.equal(existsSync(join(request.evidence, saved.retainedCandidate.archive.artifactMember)), true);
  } finally { rmSync(request.directory, { recursive: true, force: true }); }
});

for (const earlierFailure of [false, true]) {
  test(`archive drift forces failed final receipt and preserves original failure (${earlierFailure})`, async () => {
    const request = retainedFixture();
    try {
      const result = { status: 'failed', tests: [], sourceVerified: true, publicationEligible: false };
      await assert.rejects(withResultEvidence(request.evidence, result, async () => {
        result.retainedCandidate = retainVerifiedCandidate(request); result.status = 'passed';
        writeFileSync(join(request.evidence, result.retainedCandidate.archive.artifactMember), 'corrupted archive');
        if (earlierFailure) throw Error('original runtime failure');
      }));
      const saved = JSON.parse(readFileSync(join(request.evidence, 'result.json')));
      assert.equal(saved.status, 'failed'); assert.equal(saved.retainedCandidateVerified, false);
      assert.ok(saved.retentionFailure); assert.ok(saved.failure);
      if (earlierFailure) assert.equal(saved.failure, 'original runtime failure');
      assert.equal(existsSync(join(request.evidence, 'kernel-candidate-linux-amd64.tar')), false);
    } finally { rmSync(request.directory, { recursive: true, force: true }); }
  });
}

test('candidate archive filesystem, mutation and member negatives execute without any product binary', () => {
  const fixture = String.raw`import copy,hashlib,io,json,os,stat,subprocess,sys,tarfile,tempfile
from pathlib import Path
from unittest.mock import patch
ns={'__name__':'fixture'};exec(compile(sys.argv[1],'<retention-fixture>','exec'),ns)
def rejected(call):
 try:call()
 except (RuntimeError,OSError,ValueError,KeyError,tarfile.TarError):return
 raise AssertionError('unsafe candidate retention accepted')
def setup(root,name):
 directory=root/name;directory.mkdir(mode=0o700);evidence=directory/'evidence';evidence.mkdir(mode=0o700)
 binary_dir=directory/'bin';binary_dir.mkdir(mode=0o700);binary=binary_dir/'sing-box';binary.write_bytes(b'not an executable\n');binary.chmod(0o755)
 receipt={'schema':'polaris-runtime-loopback-candidate-v1','sourceVerified':True,'profile':ns['PROFILE'],
  'naiveIncluded':False,'publicationEligible':False,'binary':{'path':str(binary),'bytes':binary.stat().st_size,'sha256':hashlib.sha256(binary.read_bytes()).hexdigest()}}
 receipt_path=evidence/'candidate.json';receipt_path.write_text(json.dumps(receipt)+'\n');receipt_path.chmod(0o600)
 return directory,evidence,binary,receipt_path,receipt,True
with patch.object(subprocess,'Popen',side_effect=AssertionError('fixture cannot execute tools')),tempfile.TemporaryDirectory() as temporary:
 root=Path(temporary)
 for index,kind in enumerate(['bytes','size','receipt','symlink','hardlink','directory','fifo','setuid','writable','redirected','existing','unverified','profile','formal','naive']):
  args=list(setup(root,'negative-'+str(index)));directory,evidence,binary,receipt_path,receipt,_=args
  if kind=='bytes':binary.write_bytes(b'different bytes!!')
  elif kind=='size':receipt['binary']['bytes']+=1
  elif kind=='receipt':receipt_path.write_text('{}')
  elif kind=='symlink':binary.rename(directory/'other');binary.symlink_to(directory/'other')
  elif kind=='hardlink':os.link(binary,directory/'other')
  elif kind=='directory':binary.unlink();binary.mkdir()
  elif kind=='fifo':binary.unlink();os.mkfifo(binary)
  elif kind=='setuid':binary.chmod(0o4755)
  elif kind=='writable':binary.chmod(0o777)
  elif kind=='redirected':(directory/'bin').rename(directory/'other');(directory/'bin').symlink_to(directory/'other',target_is_directory=True)
  elif kind=='existing':(evidence/ns['ARCHIVE']).write_bytes(b'previous');(evidence/ns['ARCHIVE']).chmod(0o600)
  elif kind=='unverified':args[-1]=False
  elif kind=='profile':receipt['profile']='formal-linux'
  elif kind=='formal':receipt['publicationEligible']=True
  elif kind=='naive':receipt['naiveIncluded']=True
  rejected(lambda:ns['retain'](*args))
  if kind=='existing':assert (evidence/ns['ARCHIVE']).read_bytes()==b'previous'
  else:assert not (evidence/ns['ARCHIVE']).exists()
 # Simulate actual source/copy mutation at precise production helper calls,
 # with no test-only injection hook in the runner.
 for index,target in enumerate(['source','copy','receipt','mode']):
  args=setup(root,'drift-'+str(index));directory,evidence,binary,receipt_path,_,_=args
  original=ns['regular'];changed=[False]
  def drifting(path,limit):
   data=original(path,limit)
   if path==directory/'candidate-retention/sing-box' and not changed[0]:
    changed[0]=True
    if target=='source':binary.write_bytes(b'changed source')
    elif target=='copy':path.write_bytes(b'changed copy')
    elif target=='receipt':receipt_path.write_text('{}')
    else:binary.chmod(0o700)
   return data
  ns['regular']=drifting
  try:rejected(lambda:ns['retain'](*args));assert not (evidence/ns['ARCHIVE']).exists()
  finally:ns['regular']=original
 args=setup(root,'valid');retained=ns['retain'](*args);assert ns['verify'](args[1],retained)==retained
 members=retained['members']
 for index,kind in enumerate(['extra','duplicate','absolute','traversal','symlink','hardlink','directory','mode','bytes','reorder']):
  path=root/('malformed-'+str(index)+'.tar')
  entries=[('sing-box',args[2].read_bytes()),('candidate.json',args[3].read_bytes())]
  if kind=='extra':entries.append(('config.json',b'secret sentinel'))
  elif kind=='duplicate':entries.append(entries[0])
  elif kind=='reorder':entries.reverse()
  with tarfile.open(path,'w',format=tarfile.USTAR_FORMAT) as archive:
   for position,(name,body) in enumerate(entries):
    member=tarfile.TarInfo(name);member.mode=members.get(name,{'mode':0o600})['mode'];member.size=len(body)
    if position==0:
     if kind=='absolute':member.name='/sing-box'
     elif kind=='traversal':member.name='../sing-box'
     elif kind=='symlink':member.type=tarfile.SYMTYPE;member.linkname='/tmp/escape';member.size=0
     elif kind=='hardlink':member.type=tarfile.LNKTYPE;member.linkname='candidate.json';member.size=0
     elif kind=='directory':member.type=tarfile.DIRTYPE;member.size=0
     elif kind=='mode':member.mode=0o644
     elif kind=='bytes':body=b'x'*len(body)
    archive.addfile(member,io.BytesIO(body))
  path.chmod(0o600);rejected(lambda:ns['verify_members'](path,members))
print('Candidate retention negative fixtures passed')
`;
  const output = execFileSync('python3', ['-I', '-c', fixture, RETAINED_CANDIDATE],
    { encoding: 'utf8', timeout: 20_000 });
  assert.match(output, /Candidate retention negative fixtures passed/);
});

test('ordinary selector/source admission fixtures execute separately before the four ignored real HOT tests', () => {
  assert.deepEqual(ADMISSION_TESTS.map((name) => name.split('::').at(-1)), [
    'private_probe_selector_admission_accepts_only_owned_slot_graph',
    'private_probe_selector_admission_rejects_policy_and_reachability_mutations',
    'builtin_rule_set_admission_accepts_owned_sources_and_order',
    'builtin_rule_set_admission_rejects_source_and_order_mutations',
  ]);
  assert.equal(new Set([...ADMISSION_TESTS, ...TESTS]).size, 8);
  const source = readFileSync(fileURLToPath(new URL('./runtime-config-loopback.mjs', import.meta.url)), 'utf8');
  const start = source.indexOf('for (const name of ADMISSION_TESTS)');
  const stop = source.indexOf('for (const name of TESTS)');
  const admission = source.slice(start, stop);
  assert.ok(start > source.indexOf('const selected = selectExecutable') && stop > start);
  assert.match(admission, /program: selected\.executable/);
  assert.match(admission, /args: \[name, '--exact', '--nocapture', '--test-threads=1'\]/);
  assert.doesNotMatch(admission, /--ignored|result\.tests\.push/);
  assert.match(admission, /result\.admissionTests\.push/);
  assert.match(admission, /demand\(exactPass\(observed\),/);
  assert.match(admission, /result\.admissionTests\.length === 4/);
  assert.match(source.slice(stop), /result\.tests\.length === 4/);
  assert.ok(source.indexOf('result.retainedCandidate = retainVerifiedCandidate') > source.indexOf('result.sourceVerified = true'));
  assert.ok(source.indexOf('result.retainedCandidate = retainVerifiedCandidate') < source.indexOf("execute('app-test-build'"));
});


test('formal choice preserves exact direct App source dispatch and rejects all invalid discriminants', () => {
  const formal = { ...inputs, runtime_config_loopback_input: 'formal-release' };
  assert.equal(validateRequest(env, formal, event(formal)), app);
  for (const mode of ['', null, false, 'formal', 'unknown']) {
    const selected = { ...inputs, runtime_config_loopback_input: mode };
    assert.throws(() => validateRequest(env, selected, event(selected)));
  }
  for (const key of ['POLARIS_SINGBOX_PATH', 'POLARIS_CORE_IDENTITY_FILE', 'POLARIS_NATIVE_DNS_FORMAL'])
    assert.throws(() => validateRequest({ ...env, [key]: '1' }, formal, event(formal)));
});
