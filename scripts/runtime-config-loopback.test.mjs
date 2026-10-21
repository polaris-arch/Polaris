import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { validateRequest, validatePin, validateBuild, validateTestEvidence, requireSameAppSource, APP_SOURCE_CHECK, OFFICIAL_GO_SETUP, TESTS, REF, REPOSITORY, PROFILE, PROTO } from './runtime-config-loopback.mjs';
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
test('build metadata binds exact source, profile and module graph', () => {
  const graph = [{ Path: 'github.com/sagernet/sing-box', Main: true }, { Path: 'example.org/module', Version: 'v1.0.0', Sum: 'h1:fixture' }];
  const raw = `/fixture/sing-box: go1.27.2\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n`
    + '\tmod\tgithub.com/sagernet/sing-box\t(devel)\n\tdep\texample.org/module\tv1.0.0\th1:fixture\n'
    + Object.entries({ GOOS: 'linux', GOARCH: 'amd64', GOAMD64: 'v1', CGO_ENABLED: '0', '-trimpath': 'true',
      'vcs.revision': kernel, 'vcs.modified': 'false', '-tags': pin.tags }).map(([key, value]) => `\tbuild\t${key}=${value}\n`).join('');
  assert.equal(validateBuild(raw, pin, graph).goVersion, '1.27.2');
  for (const [before, after] of [['vcs.modified=false', 'vcs.modified=true'], [kernel, app], ['CGO_ENABLED=0', 'CGO_ENABLED=1'],
    ['h1:fixture', 'h1:other'], ['go1.27.2', 'go1.27.1'], ['GOARCH=amd64', 'GOARCH=arm64'], ['GOAMD64=v1', 'GOAMD64=v3']])
    assert.throws(() => validateBuild(raw.replace(before, after), pin, graph));
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
