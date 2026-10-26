#!/usr/bin/env node
// Exact-source, unprivileged Linux control/data-plane candidate. No release authority.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { executeCommand } from './desktop-core/native-dns-gates.mjs';
import { consumeRelease } from './desktop-core/release-consumer.mjs';
import { FORMAL_PROFILE, FORMAL_SCHEMA, FORMAL_TEST_SCHEMA, loopbackInput, formalSelection, prepareFormalCarrier,
  createFormalInput, verifyFormalFiles, verifyFormalInput, formalFile } from './runtime-config-loopback-formal.mjs';
import { parseBuildInfo } from './desktop-core/source-graph.mjs';
import { exactPass, runtimeEnvironment, selectExecutable } from './run-native-dns-controlled-tests.mjs';

export const TESTS = Object.freeze([
  'runtime::proxy::tests::runtime_config_real_core::real_hot_dispatcher_apply_and_status',
  'runtime::proxy::tests::runtime_config_real_core::real_hot_dispatcher_persistence_retry',
  'runtime::proxy::tests::runtime_config_real_core::real_hot_public_rpc_contract',
  'runtime::proxy::tests::runtime_config_real_core::real_hot_dispatcher_rejections_preserve_runtime',
]);
export const ADMISSION_TESTS = Object.freeze([
  'private_probe_selector_admission_accepts_only_owned_slot_graph',
  'private_probe_selector_admission_rejects_policy_and_reachability_mutations',
  'builtin_rule_set_admission_accepts_owned_sources_and_order',
  'builtin_rule_set_admission_rejects_source_and_order_mutations',
].map((name) => ['runtime', 'proxy', 'tests', 'runtime_config_real_core', name].join('::')));
export const PROFILE = 'linux-amd64-purego-no-naive';
export const REF = 'refs/heads/hot/app-runtime-consumer-20261020';
export const REPOSITORY = 'polaris-arch/Polaris';
export const PROTO = 'b6bb4642d1081f238e5b4f3563b7ddab8e5a1d66b65a306fbd215153164fac5f';
const ROOT = resolve(fileURLToPath(new URL('../', import.meta.url)));
const PIN = join(ROOT, 'scripts/runtime-config-loopback-kernel.json');
const demand = (condition, reason) => { if (!condition) throw Error(reason); };
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
const readJSON = (path) => {
  demand(lstatSync(path).isFile() && !lstatSync(path).isSymbolicLink()
    && lstatSync(path).size <= 4 * 1024 * 1024, 'Bounded regular JSON input required');
  return JSON.parse(readFileSync(path, 'utf8'));
};
const filePin = (path) => {
  demand(lstatSync(path).isFile() && realpathSync(path) === path, 'Regular non-redirected file required');
  return { path, bytes: lstatSync(path).size, sha256: sha(readFileSync(path)) };
};
const writeJSON = (path, data) => {
  demand(!existsSync(path), 'Evidence file already exists');
  writeFileSync(path, JSON.stringify(data, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
};
const gitEnv = (env) => ({ ...Object.fromEntries(Object.entries(env).filter(([key]) => !key.startsWith('GIT_'))),
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_NO_REPLACE_OBJECTS: '1', GIT_TERMINAL_PROMPT: '0' });
const capture = (command, args, cwd = ROOT, env = process.env) => execFileSync(command, args,
  { cwd, env, encoding: 'utf8', timeout: 60_000, maxBuffer: 16 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });

export function validateRequest(env, inputs, event, prepared = false) {
  demand(inputs && typeof inputs === 'object' && !Array.isArray(inputs), 'Dispatch inputs required');
  loopbackInput(inputs);
  const candidate = inputs.runtime_config_loopback_candidate;
  demand(typeof candidate === 'string' && /^[0-9a-f]{40}$/.test(candidate), 'Exact lowercase App SHA required');
  const expected = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: REPOSITORY,
    GITHUB_REPOSITORY_ID: '1347028220', GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REF: REF,
    GITHUB_WORKFLOW_REF: `${REPOSITORY}/.github/workflows/ci.yml@${REF}`,
    GITHUB_SHA: candidate, GITHUB_WORKFLOW_SHA: candidate, RUNNER_OS: 'Linux', RUNNER_ARCH: 'X64' };
  demand(Object.entries(expected).every(([key, value]) => env[key] === value)
    && ['cheap_format', 'runtime_config_loopback'].includes(env.GITHUB_JOB), 'Exact direct Linux candidate context required');
  demand(inputs.preserve_run === true && inputs.os === 'ubuntu-22.04'
    && (inputs.runtime_config_source_only ?? false) === false && (inputs.windows_launcher_diagnostics ?? false) === false
    && (inputs.windows_directory_mode || 'formal') === 'formal'
    && ['app_native_compile_candidate', 'native_dns_candidate', 'windows_system_candidate', 'windows_directory_candidate_source']
      .every((key) => (inputs[key] ?? '') === ''), 'Loopback cannot combine other candidate/source/compile/SYSTEM modes');
  const allowed = ['runtime_config_loopback_input', 'runtime_config_loopback_candidate', 'app_native_compile_candidate', 'runtime_config_source_only',
    'native_dns_candidate', 'windows_system_candidate', 'windows_launcher_diagnostics', 'windows_directory_mode',
    'windows_directory_candidate_source', 'preserve_run', 'os'];
  demand(Object.keys(inputs).every((key) => allowed.includes(key)), 'Unknown dispatch input');
  demand(event && event.ref === REF && event.repository?.full_name === REPOSITORY
    && String(event.repository?.id) === '1347028220' && event.inputs
    && JSON.stringify(Object.keys(event.inputs).sort()) === JSON.stringify(Object.keys(inputs).sort())
    && Object.entries(inputs).every(([key, value]) => event.inputs[key] === String(value)), 'Direct dispatch event identity/inputs differ');
  demand(['GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT'].every((key) => /^[1-9][0-9]*$/.test(env[key] ?? '')), 'Run identity required');
  for (const [key, value] of Object.entries(env)) {
    const forbidden = key.startsWith('POLARIS_') || key.startsWith('GIT_') || key.startsWith('CORE_')
      || /^(?:CGO_|CARGO_PROFILE_|CARGO_TARGET_)/.test(key)
      || ['GOFLAGS', 'GOENV', 'GOWORK', 'GOOS', 'GOARCH', 'GOHOSTOS', 'GOHOSTARCH', 'GOAMD64', 'GOFIPS140', 'GOEXPERIMENT', 'GOTOOLCHAIN', 'GOPROXY', 'GOSUMDB',
        'GONOSUMDB', 'GONOPROXY', 'GOPRIVATE', 'GOINSECURE', 'GOBIN', 'GOMODCACHE', 'GOCACHE', 'GOPATH',
        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC', 'RUSTDOC', 'RUSTUP_TOOLCHAIN',
        'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_RUSTC', 'CARGO_BUILD_RUSTC_WRAPPER',
        'PROTOC', 'PROTOC_INCLUDE', 'GOROOT', 'GOTMPDIR',
        'NODE_OPTIONS', 'BASH_ENV', 'ENV', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'PYTHONPATH', 'PYTHONHOME'].includes(key);
    demand(!(forbidden && value && !(prepared && key === 'GOTOOLCHAIN' && value === 'local')),
      `Inherited runtime/tool override refused: ${key}`);
  }
  return candidate;
}

export function validatePin(pin) {
  demand(pin?.schema === 'polaris-runtime-loopback-kernel-v1' && pin.status === 'reviewed'
    && pin.profile === PROFILE && pin.publicationEligible === false && pin.naiveIncluded === false,
  'Reviewed partial-profile kernel pin required; pending pins cannot run');
  demand(pin.source?.repository === 'polaris-arch/sing-box-polaris' && pin.source.repositoryId === 1421657509
    && /^[0-9a-f]{40}$/.test(pin.source.commit ?? '') && /^[0-9a-f]{40}$/.test(pin.source.tree ?? '')
    && pin.source.files?.['daemon/runtime_config.proto'] === PROTO
    && ['.github/go-toolchains.json', '.github/setup_legacy_go.py'].every((name) =>
      /^[0-9a-f]{64}$/.test(pin.source.files?.[name] ?? '')), 'Exact paired kernel source/proto/toolchain verifiers required');
  demand(pin.goVersion === '1.27.2' && typeof pin.tags === 'string' && pin.tags.endsWith(',with_purego')
    && !pin.tags.split(',').includes('with_naive_outbound')
    && pin.version === `polaris-hot-loopback-${pin.source.commit.slice(0, 12)}`,
  'Exact no-Naive build profile/version required');
  demand(pin.protocol === 'polaris-daily-runtime-v1' && pin.platformBuild === 'linux/amd64', 'Protocol/platform pin differs');
  // An actual first-build manifest is observed and retained, never fabricated.
  // Once independently obtained, a corresponding expected digest may be frozen.
  demand(pin.implementationManifestDigest === null
    || /^[0-9a-f]{64}$/.test(pin.implementationManifestDigest ?? ''), 'Invalid reviewed manifest digest');
  return pin;
}

// Go 1.24+ derives the binary's main-module version from VCS, independently
// of the Version constant set by -X. This runner fetches one commit, with no
// tags, so only its untagged v0 pseudo-version is admissible. The timestamp is
// read from the already verified Git object, never trusted from BuildInfo.
export function expectedBuildVcs(pin, vcs) {
  demand(vcs?.commit === pin.source.commit && vcs.tree === pin.source.tree
    && vcs.tags === '' && vcs.shallow === 'true', 'Exact untagged shallow Git identity required');
  demand(typeof vcs.commitTime === 'string' && /^[1-9][0-9]{0,11}$/.test(vcs.commitTime),
    'Canonical Git commit timestamp required');
  const instant = new Date(Number(vcs.commitTime) * 1000);
  demand(Number.isFinite(instant.getTime()), 'Git commit timestamp out of range');
  const iso = instant.toISOString();
  demand(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.000Z$/.test(iso), 'Git commit timestamp out of range');
  const time = iso.replace('.000Z', 'Z');
  return { revision: vcs.commit, time,
    moduleVersion: `v0.0.0-${time.replace(/[-:TZ]/g, '')}-${vcs.commit.slice(0, 12)}` };
}

function readBuildVcs(source) {
  const identity = capture('git', ['show', '-s', '--format=%H%n%T%n%ct', 'HEAD'], source, gitEnv(process.env))
    .trimEnd().split('\n');
  demand(identity.length === 3, 'Exact Git commit metadata required');
  return { commit: identity[0], tree: identity[1], commitTime: identity[2],
    tags: capture('git', ['for-each-ref', '--format=%(refname)', 'refs/tags'], source, gitEnv(process.env)).trimEnd(),
    shallow: capture('git', ['rev-parse', '--is-shallow-repository'], source, gitEnv(process.env)).trim() };
}

export function recordBuildInfo(raw, path) {
  // Keep the original output even if parsing or identity admission fails.
  writeFileSync(path, raw, { flag: 'wx', mode: 0o600 });
  return filePin(path);
}

export function validateBuild(raw, pin, graph, vcs) {
  const expected = expectedBuildVcs(pin, vcs);
  const facts = parseBuildInfo(raw);
  const main = 'github.com/sagernet/sing-box';
  const paths = raw.split(/\r?\n/).filter((line) => line.split('\t')[1] === 'path');
  demand(paths.length === 1 && paths[0] === `\tpath\t${main}/cmd/sing-box`, 'Executable main package path differs');
  demand(facts.goVersion === pin.goVersion, 'Go compiler version differs');
  // -buildvcs=true is represented by vcs.*; -trimpath deliberately suppresses
  // the -ldflags setting in Go BuildInfo. Neither literal flag is required here.
  for (const [key, value] of Object.entries({ GOOS: 'linux', GOARCH: 'amd64', GOAMD64: 'v1', CGO_ENABLED: '0',
    '-buildmode': 'exe', '-compiler': 'gc', '-trimpath': 'true', vcs: 'git',
    'vcs.revision': expected.revision, 'vcs.time': expected.time, 'vcs.modified': 'false', '-tags': pin.tags }))
    demand(facts.settings.get(key) === value, `Embedded candidate build setting differs: ${key}`);
  const declared = new Map(graph.map((row) => [row.Path, row]));
  const roots = graph.filter((row) => row.Main === true);
  const embeddedRoots = [...facts.modules].filter(([, observed]) => observed.kind === 'mod');
  demand(declared.size === graph.length && roots.length === 1 && roots[0].Path === main
    && !roots[0].Version && !roots[0].Replace && !roots[0].Sum
    && embeddedRoots.length === 1 && embeddedRoots[0][0] === main, 'Unique source and executable main modules required');
  for (const [name, observed] of facts.modules) {
    const row = declared.get(name);
    demand(row, `Embedded dependency absent from verified graph: ${name}`);
    if (row.Main) {
      demand(name === main && row.Main === true && observed.kind === 'mod'
        && observed.version === expected.moduleVersion && !observed.replacement && !observed.sum,
      'Executable main source differs');
      continue;
    }
    demand(observed.kind === 'dep' && observed.version === row.Version, `Dependency version differs: ${name}`);
    if (row.Replace) {
      demand(observed.replacement === row.Replace.Path
        && (row.Replace.Version ? observed.replacementVersion === row.Replace.Version
          : ['', '(devel)'].includes(observed.replacementVersion ?? ''))
        && (observed.replacementSum ?? '') === (row.Replace.Sum ?? ''), `Dependency replacement differs: ${name}`);
    } else demand(!observed.replacement && observed.sum === row.Sum && row.Sum, `Dependency h1 differs: ${name}`);
  }
  return { goVersion: facts.goVersion, settings: Object.fromEntries(facts.settings), modules: Object.fromEntries(facts.modules) };
}

export function validateTestEvidence(value, name, receipt, inputSha256) {
  const formal = receipt.schema === FORMAL_SCHEMA;
  if (formal) demand(value.inputSha256 === inputSha256 && /^[0-9a-f]{64}$/.test(inputSha256 ?? '')
    && value.contractSha256 === receipt.inputs.contract.sha256 && value.consumptionFingerprint === receipt.consumptionFingerprint,
  'Formal test input/contract/original receipt attribution differs');
  demand(value?.schema === (formal ? FORMAL_TEST_SCHEMA : 'polaris-runtime-loopback-test-v1') && value.test === name.split('::').at(-1)
    && value.passed === true && value.profile === (formal ? FORMAL_PROFILE : PROFILE) && value.publicationEligible === false,
  'Successful attributed real-test evidence required');
  for (const key of ['appCommit', 'kernelCommit', 'kernelTree'])
    demand(value[key] === receipt[key], `Real-test source attribution differs: ${key}`);
  demand(value.coreSha256 === receipt.binary.sha256 && Array.isArray(value.events), 'Real-test executable attribution differs');
  const starts = value.events.filter((row) => row.phase === 'started');
  const stops = value.events.filter((row) => row.phase === 'stopped');
  demand(starts.length === 1 && stops.length === 1 && stops[0].ownedChildReaped === true
    && Number.isSafeInteger(starts[0].pid) && starts[0].pid > 0 && stops[0].pid === starts[0].pid
    && value.events.indexOf(starts[0]) < value.events.indexOf(stops[0]), 'Real-test owned birth/terminal evidence required');
  const observed = starts[0].capabilities;
  demand(observed?.contractVersion === 1 && observed.enabled === true
    && /^[0-9a-f]{64}$/.test(observed.implementationManifestDigest ?? ''), 'Actual enabled protocol manifest required');
  demand(receipt.implementationManifestDigest === null
    || observed.implementationManifestDigest === receipt.implementationManifestDigest, 'Reviewed profile manifest differs');
  return observed.implementationManifestDigest;
}

// Download-only candidate retention. The original absolute-path receipt remains
// byte-for-byte intact; this archive never authorizes the formal App consumer.
export const RETAINED_CANDIDATE = String.raw`import hashlib,io,json,os,stat,sys,tarfile
from pathlib import Path
PROFILE='linux-amd64-purego-no-naive'
ARCHIVE='kernel-candidate-linux-amd64.tar'
def require(ok,message):
 if not ok:raise RuntimeError(message)
def digest(data):return hashlib.sha256(data).hexdigest()
def mark(info):return (info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns,info.st_ctime_ns,info.st_mode,info.st_nlink,info.st_uid)
def private_directory(path):
 require(path.is_absolute() and path.resolve()==path,'Candidate directory redirected')
 info=path.lstat()
 require(stat.S_ISDIR(info.st_mode) and info.st_uid==os.getuid() and stat.S_IMODE(info.st_mode)==0o700,'Private candidate directory required')
def regular(path,limit):
 require(path.is_absolute() and path.resolve()==path,'Candidate file redirected')
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
 try:
  before=os.fstat(fd)
  require(stat.S_ISREG(before.st_mode) and before.st_nlink==1 and before.st_uid==os.getuid()
   and not(before.st_mode & 0o7022) and 0<before.st_size<=limit,'Bounded owned regular candidate file required')
  chunks=[];count=0
  while True:
   chunk=os.read(fd,min(1024*1024,limit+1-count))
   if not chunk:break
   chunks.append(chunk);count+=len(chunk);require(count<=limit,'Candidate file exceeds bound')
  require(mark(before)==mark(os.fstat(fd))==mark(path.lstat()) and path.resolve()==path,'Candidate file changed during read')
  data=b''.join(chunks);require(len(data)==before.st_size,'Candidate file size changed')
  return data,{'bytes':len(data),'sha256':digest(data)}
 finally:os.close(fd)
def write_new(path,data,mode):
 fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,mode)
 with os.fdopen(fd,'wb') as output:
  output.write(data);output.flush();os.fchmod(output.fileno(),mode)
def verify_members(path,members):
 data,pin=regular(path,260*1024*1024)
 with tarfile.open(fileobj=io.BytesIO(data),mode='r:') as archive:
  entries=archive.getmembers()
  require([entry.name for entry in entries]==['sing-box','candidate.json'],'Exact candidate archive members required')
  for entry in entries:
   expected=members[entry.name]
   require(entry.isreg() and entry.type==tarfile.REGTYPE and not entry.linkname and not entry.pax_headers
    and entry.mode==expected['mode'] and entry.size==expected['bytes'] and entry.uid==entry.gid==entry.mtime==0
    and entry.uname==entry.gname=='','Candidate archive member type/mode/identity differs')
   body=archive.extractfile(entry).read(expected['bytes']+1)
   require(len(body)==expected['bytes'] and digest(body)==expected['sha256'],'Candidate archive member bytes differ')
 return pin
def retain(directory,evidence,binary,receipt_path,receipt,source_verified):
 require(source_verified is True and receipt.get('sourceVerified') is True and receipt.get('profile')==PROFILE
  and receipt.get('publicationEligible') is False and receipt.get('naiveIncluded') is False,'Only source-verified partial candidates may be retained')
 require(evidence==directory/'evidence' and binary==directory/'bin/sing-box' and receipt_path==evidence/'candidate.json','Fixed candidate retention paths required')
 for path in [directory,evidence,directory/'bin']:private_directory(path)
 require(receipt.get('binary',{}).get('path')==str(binary),'Original receipt binary path differs')
 original,binary_pin=regular(binary,256*1024*1024)
 source_mode=stat.S_IMODE(binary.stat().st_mode)
 require(source_mode & stat.S_IXUSR,'Candidate source must be executable')
 require(binary_pin=={key:receipt['binary'][key] for key in ['bytes','sha256']},'Source candidate bytes differ from receipt')
 raw_receipt,receipt_pin=regular(receipt_path,1024*1024)
 require(json.loads(raw_receipt)==receipt and stat.S_IMODE(receipt_path.stat().st_mode)==0o600,'Original candidate receipt differs')
 members={'sing-box':dict(binary_pin,mode=0o755),'candidate.json':dict(receipt_pin,mode=0o600)}
 stage=directory/'candidate-retention';stage.mkdir(mode=0o700);private_directory(stage)
 require(not (evidence/ARCHIVE).exists() and not (evidence/ARCHIVE).is_symlink(),'Candidate archive already exists')
 for name,data in [('sing-box',original),('candidate.json',raw_receipt)]:
  write_new(stage/name,data,members[name]['mode'])
  copied,copied_pin=regular(stage/name,256*1024*1024)
  require(copied==data and copied_pin=={key:members[name][key] for key in ['bytes','sha256']},'Candidate copy bytes differ')
 require(sorted(path.name for path in stage.iterdir())==['candidate.json','sing-box'],'Candidate staging members differ')
 temporary=stage/ARCHIVE
 fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 with os.fdopen(fd,'wb') as output,tarfile.open(fileobj=output,mode='w',format=tarfile.USTAR_FORMAT) as archive:
  for name in ['sing-box','candidate.json']:
   data,pin=regular(stage/name,256*1024*1024)
   require(pin=={key:members[name][key] for key in ['bytes','sha256']},'Candidate copy changed before archive')
   member=tarfile.TarInfo(name);member.size=len(data);member.mode=members[name]['mode'];member.mtime=0
   archive.addfile(member,io.BytesIO(data))
 archive_pin=verify_members(temporary,members)
 for path,expected,limit in [(binary,binary_pin,256*1024*1024),(receipt_path,receipt_pin,1024*1024),
  (stage/'sing-box',binary_pin,256*1024*1024),(stage/'candidate.json',receipt_pin,1024*1024)]:
  require(regular(path,limit)[1]==expected,'Candidate source or copy changed during retention')
 require(stat.S_IMODE(binary.stat().st_mode)==source_mode and stat.S_IMODE(receipt_path.stat().st_mode)==0o600,'Candidate source mode changed during retention')
 for path in [directory,evidence,directory/'bin',stage]:private_directory(path)
 published=False
 try:
  # link is an atomic no-replace publication; remove its private name before
  # final checks so the retained archive itself has exactly one hard link.
  os.link(temporary,evidence/ARCHIVE,follow_symlinks=False);published=True;temporary.unlink()
  require(verify_members(evidence/ARCHIVE,members)==archive_pin,'Published candidate archive differs')
  return {'schema':'polaris-runtime-loopback-retained-v1','profile':PROFILE,'publicationEligible':False,
   'naiveIncluded':False,'sourceVerified':True,'archive':dict(archive_pin,artifactMember=ARCHIVE),
   'members':members,'sourceBinaryPath':str(binary),'sourceReceiptPath':str(receipt_path)}
 except BaseException:
  if published:(evidence/ARCHIVE).unlink()
  raise
def verify(evidence,retained):
 private_directory(evidence);private_directory(evidence.parent)
 require(retained.get('schema')=='polaris-runtime-loopback-retained-v1' and retained.get('profile')==PROFILE
  and retained.get('publicationEligible') is False and retained.get('naiveIncluded') is False
  and retained.get('sourceVerified') is True and retained.get('archive',{}).get('artifactMember')==ARCHIVE,'Candidate retention metadata differs')
 expected=retained['archive'];members=retained['members']
 require(set(members)=={'sing-box','candidate.json'} and members['sing-box']['mode']==0o755
  and members['candidate.json']['mode']==0o600,'Fixed retained member modes required')
 require(verify_members(evidence/ARCHIVE,members)=={key:expected[key] for key in ['bytes','sha256']},'Retained candidate archive changed')
 return retained
if __name__=='__main__':
 request=json.loads(sys.argv[2])
 if sys.argv[1]=='retain':result=retain(*[Path(request[key]) for key in ['directory','evidence','binary','receiptPath']],request['receipt'],request['sourceVerified'])
 elif sys.argv[1]=='verify':result=verify(Path(request['evidence']),request['retained'])
 else:raise RuntimeError('Unknown candidate retention action')
 print(json.dumps(result,sort_keys=True,separators=(',',':')))
`;

export function retainVerifiedCandidate(request) {
  demand(request.sourceVerified === true && request.receipt.sourceVerified === true,
    'Only source-verified partial candidates may be retained');
  return JSON.parse(capture('python3', ['-I', '-c', RETAINED_CANDIDATE, 'retain', JSON.stringify(request)]));
}

// Preserve the real run failure independently from final archive verification.
export async function withResultEvidence(evidence, result, run, finalize) {
  let failure;
  try { await run(); }
  catch (error) { failure = error; result.status = 'failed'; result.failure = error.message; }
  finally {
    if (finalize) {
      try { await finalize(); }
      catch (error) {
        result.status = 'failed'; result.finalVerificationFailure = error.message;
        if (!failure) { failure = error; result.failure = error.message; }
      }
    }
    if (result.retainedCandidate) {
      try {
        capture('python3', ['-I', '-c', RETAINED_CANDIDATE, 'verify',
          JSON.stringify({ evidence, retained: result.retainedCandidate })]);
        result.retainedCandidateVerified = true;
      } catch (error) {
        result.status = 'failed'; result.retainedCandidateVerified = false; result.retentionFailure = error.message;
        // Only our fixed member is removed; original receipt and diagnostics stay.
        try {
          demand(realpathSync(evidence) === evidence, 'Evidence directory redirected during retention cleanup');
          rmSync(join(evidence, 'kernel-candidate-linux-amd64.tar'), { force: true });
        } catch (cleanupError) { result.retentionCleanupFailure = cleanupError.message; }
        if (!failure) { failure = error; result.failure = error.message; }
      }
    }
    writeJSON(join(evidence, 'result.json'), result);
  }
  if (failure) throw failure;
}

// Calls the existing Android verifier as a read-only library with an explicit
// desktop identity. Its Android entrypoints and frozen defaults are unchanged.
const SOURCE_CHECK = `import importlib.util,json,sys
from pathlib import Path
sys.dont_write_bytecode=True
sys.path.insert(0,str(Path(sys.argv[1])/'scripts/mobile-core'))
s=importlib.util.spec_from_file_location('candidate_source',Path(sys.argv[1])/'scripts/mobile-core/ci-hot-candidate.py')
m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
pin=m.decode(Path(sys.argv[3]).read_bytes(),1024*1024)
source=Path(sys.argv[2]);facts=m.source_facts(source,identity=pin['source'])
if len(sys.argv)>4:
 graph=m.module_stream(Path(sys.argv[4]).read_text());m.graph_facts(graph,source,identity=pin['source']);facts={'source':facts,'graph':graph}
print(json.dumps(facts,sort_keys=True,separators=(',',':')))
`;

export const APP_SOURCE_CHECK = `import importlib.util,json,sys
from pathlib import Path
sys.dont_write_bytecode=True
root=Path(sys.argv[1]);sys.path.insert(0,str(root/'scripts/mobile-core'))
s=importlib.util.spec_from_file_location('candidate_source',root/'scripts/mobile-core/ci-hot-candidate.py')
m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
m.require(sys.argv[2] in ('clean','tracked'),'invalid App source check phase')
if sys.argv[2]=='clean':m.clean(root)
else:m.require(all(row.startswith('H ') for row in m.git(root,'ls-files','-v').splitlines()),'hidden index flags refused')
facts={'commit':m.git(root,'rev-parse','HEAD'),'tree':m.git(root,'rev-parse','HEAD^{tree}'),**m.tracked_mirror(root)}
print(json.dumps(facts,sort_keys=True,separators=(',',':')))
`;

// Import only the reviewed archive/inventory functions, never the legacy
// installer entrypoint. The official unmodified archive supplies every compiler,
// tool and standard-library byte in a fresh private slot; setup-go is not trusted
// as provenance for the executable used below.
export const OFFICIAL_GO_SETUP = String.raw`import hashlib,importlib.util,json,os,stat,sys,tarfile
from pathlib import Path,PurePosixPath
from urllib.request import urlopen
sys.dont_write_bytecode=True

def require(condition,reason):
 if not condition:raise RuntimeError(reason)

def canonical(value):
 return json.dumps(value,sort_keys=True,separators=(',',':')).encode()

def regular(path,maximum):
 require(path.resolve(strict=True)==path and stat.S_ISREG(path.lstat().st_mode) and path.stat().st_size<=maximum,'bounded regular toolchain input required')
 return path.read_bytes()

def inputs(source,pin):
 names=('.github/go-toolchains.json','.github/setup_legacy_go.py')
 data={name:regular(source/name,1024*1024) for name in names}
 for name in names:require(hashlib.sha256(data[name]).hexdigest()==pin['source']['files'][name],'reviewed Go input hash differs: '+name)
 manifest=json.loads(data[names[0]])
 require(manifest['version']==pin['goVersion']=='1.27.2','official Go version differs')
 archive=manifest['archives']['linux-amd64']
 require(archive=={'filename':'go1.27.2.linux-amd64.tar.gz','sha256':'ecbadb99091a3f46e31f5f934b068b1864eafa7995211b39eaddf76996045fe5','bytes':70590635},'reviewed official Go archive differs')
 spec=importlib.util.spec_from_file_location('reviewed_go_archive',source/names[1])
 helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
 return manifest,archive,helper

def members_checked(tar,helper):
 members=tar.getmembers();helper.require_archive_members(members)
 names=set()
 for member in members:
  name=member.name.rstrip('/') if member.isdir() else member.name
  require(name==str(PurePosixPath(name)) and name not in names,'noncanonical or duplicate Go archive member')
  names.add(name)
  require(member.mode&0o400 and not member.mode&0o7022,'unsafe Go archive member permissions')
  require(not (member.isfile() and name=='go/polaris-toolchain.json'),'unexpected toolchain marker')
 return members

def extract_verified(download,archive,target,helper):
 require(download.stat().st_size==archive['bytes'] and helper.file_hash(download)==archive['sha256'],'official Go archive size/hash mismatch')
 require(not target.exists() and not target.is_symlink(),'fresh official Go extraction required')
 target.mkdir(mode=0o700)
 expected={}
 with tarfile.open(download,'r:gz') as tar:
  members=members_checked(tar,helper)
  for member in members:
   path=target/member.name
   if member.isdir():path.mkdir(parents=True,exist_ok=True);continue
   path.parent.mkdir(parents=True,exist_ok=True)
   count=0;digest=hashlib.sha256()
   with tar.extractfile(member) as inp,path.open('xb') as out:
    while chunk:=inp.read(1024*1024):
     count+=len(chunk);require(count<=member.size,'Go member exceeds declared size');digest.update(chunk);out.write(chunk)
   require(count==member.size,'truncated Go archive member');path.chmod(member.mode)
   expected[PurePosixPath(member.name).relative_to('go').as_posix()]={'sha256':digest.hexdigest(),'bytes':count,'mode':member.mode}
 root=target/'go';measured=helper.toolchain_files(root)
 require(measured==expected,'extracted Go files differ from verified archive')
 required={'bin/go','bin/gofmt','VERSION','go.env','src/runtime/runtime2.go','src/cmd/compile/main.go','src/cmd/asm/main.go'}
 required.update('pkg/tool/linux_amd64/'+name for name in ('compile','asm','link'))
 require(required<=measured.keys() and all(measured[name]['mode']&0o111 for name in ('bin/go','pkg/tool/linux_amd64/compile','pkg/tool/linux_amd64/asm','pkg/tool/linux_amd64/link')),'complete executable official Go toolchain required')
 return measured

def main(action,source,pin_path,directory):
 require(action in ('prepare','verify') and directory.resolve(strict=True)==directory,'official Go action/slot differs')
 pin=json.loads(regular(pin_path,1024*1024));manifest,archive,helper=inputs(source,pin)
 slot=directory/'official-go';download=slot/archive['filename'];target=slot/'unpacked';root=target/'go'
 url='https://go.dev/dl/'+archive['filename']
 if action=='prepare':
  require(not slot.exists() and not slot.is_symlink(),'fresh official Go slot required');slot.mkdir(mode=0o700)
  count=0
  with urlopen(url,timeout=60) as response,download.open('xb') as out:
   require(response.geturl() in (url,'https://dl.google.com/go/'+archive['filename']),'official Go download redirected outside allowed archive URLs')
   while chunk:=response.read(1024*1024):
    count+=len(chunk);require(count<=archive['bytes'],'official Go archive exceeded pinned size');out.write(chunk)
  files=extract_verified(download,archive,target,helper)
  inventory=directory/'evidence/official-go-files.json'
  with inventory.open('xb') as out:out.write(canonical(files)+b'\n')
 else:
  require(slot.resolve(strict=True)==slot and root.resolve(strict=True)==root,'official Go slot redirected')
  files=helper.toolchain_files(root)
 require(not (root/'polaris-toolchain.json').exists() and not (root/'polaris-toolchain.json').is_symlink(),'unexpected toolchain marker')
 require((root/'VERSION').read_text().splitlines()[0]=='go1.27.2','extracted official Go VERSION differs')
 return {'schema':'polaris-official-go-v1','root':str(root),'goVersion':manifest['version'],'officialCommit':manifest['officialCommit'],
  'metadataSha256':pin['source']['files']['.github/go-toolchains.json'],'verifierSha256':pin['source']['files']['.github/setup_legacy_go.py'],
  'archive':dict(archive,url=url),'fileCount':len(files),'fileBytes':sum(row['bytes'] for row in files.values()),
  'filesSha256':hashlib.sha256(canonical(files)).hexdigest(),'legacyPatched':False}

if __name__=='__main__':print(json.dumps(main(sys.argv[1],Path(sys.argv[2]),Path(sys.argv[3]),Path(sys.argv[4])),sort_keys=True,separators=(',',':')))
`;

function appSourceCheck(mode) {
  return JSON.parse(capture('python3', ['-I', '-c', APP_SOURCE_CHECK, ROOT, mode], ROOT, gitEnv(process.env)));
}

export function requireSameAppSource(before, after) {
  demand(JSON.stringify(before) === JSON.stringify(after), 'App tracked source changed after admission');
  return after;
}

export function validatePreparedSource(admission, source) {
  demand(admission.appCommit === source.commit && admission.appTree === source.tree
    && JSON.stringify(admission.appSource) === JSON.stringify(source), 'Prepared App source/tree differs');
  return source.tree;
}

function context(prepared) {
  const env = process.env;
  const inputs = JSON.parse(env.RUNTIME_CONFIG_LOOPBACK_INPUTS ?? '{}');
  const candidate = validateRequest(env, inputs, readJSON(env.GITHUB_EVENT_PATH), prepared);
  const inputMode = loopbackInput(inputs);
  demand(realpathSync(process.cwd()) === ROOT && capture('git', ['rev-parse', 'HEAD'], ROOT, gitEnv(env)).trim() === candidate,
    'App checkout differs');
  // Reuse the full tracked-byte/mode verifier, rejecting hidden index flags and
  // all untracked/ignored inputs before any generated resources or Cargo build.
  const appSource = appSourceCheck('clean');
  demand(appSource.commit === candidate, 'App tracked source commit differs');
  const pin = inputMode === 'diagnostic-source' ? validatePin(readJSON(PIN)) : null;
  const formal = inputMode === 'formal-release' ? formalSelection(ROOT) : null;
  demand(sha(readFileSync(join(ROOT, 'crates/singbox-grpc/proto/runtime_config.proto'))) === PROTO, 'App protocol bytes differ');
  const temporary = realpathSync(env.RUNNER_TEMP);
  const directory = join(temporary, `polaris-runtime-loopback-${env.GITHUB_RUN_ID}-${env.GITHUB_RUN_ATTEMPT}`);
  return { candidate, pin, formal, inputMode, appSource, directory, evidence: join(directory, 'evidence') };
}

function admit() {
  const checked = context(false);
  if (process.env.GITHUB_JOB === 'cheap_format') {
    appendFileSync(process.env.GITHUB_OUTPUT, 'loopback_only=true\n');
    return;
  }
  demand(!existsSync(checked.directory), 'Loopback work slot already exists');
  mkdirSync(checked.directory, { mode: 0o700 });
  mkdirSync(checked.evidence, { mode: 0o700 });
  writeJSON(join(checked.evidence, 'admission.json'), { appCommit: checked.candidate,
    appTree: capture('git', ['rev-parse', 'HEAD^{tree}'], ROOT, gitEnv(process.env)).trim(),
    appSource: checked.appSource,
    inputMode: checked.inputMode,
    ...(checked.formal ? { contractSha256: checked.formal.manifest.desktopRelease.contractSha256 } : { pinSha256: sha(readFileSync(PIN)) }), runId: process.env.GITHUB_RUN_ID, runAttempt: process.env.GITHUB_RUN_ATTEMPT,
    workflowRef: process.env.GITHUB_WORKFLOW_REF, profile: checked.formal ? FORMAL_PROFILE : PROFILE, publicationEligible: false });
  appendFileSync(process.env.GITHUB_OUTPUT, `evidence=${checked.evidence}\n`);
}

async function run() {
  const { candidate, pin, formal, inputMode, appSource, directory, evidence } = context(true);
  demand(process.env.GITHUB_JOB === 'runtime_config_loopback' && realpathSync(directory) === directory,
    'Admitted execution runner required');
  const admission = readJSON(join(evidence, 'admission.json'));
  validatePreparedSource(admission, appSource);
  demand(admission.appCommit === candidate && admission.inputMode === inputMode
    && (formal ? admission.contractSha256 === formal.manifest.desktopRelease.contractSha256 : admission.pinSha256 === sha(readFileSync(PIN)))
    && admission.runId === process.env.GITHUB_RUN_ID && admission.runAttempt === process.env.GITHUB_RUN_ATTEMPT
    && JSON.stringify(admission.appSource) === JSON.stringify(appSource),
  'Prepared runner admission differs');
  const source = join(directory, 'kernel'), bin = join(directory, 'bin'), target = join(directory, 'target');
  for (const path of [source, bin, target]) { demand(!existsSync(path), 'Fresh build directory required'); mkdirSync(path, { mode: 0o700 }); }
  const result = { ...admission, schema: 'polaris-runtime-loopback-result-v1', status: 'failed', tests: [], admissionTests: [],
    sourceVerified: false, ...(formal ? {} : { naiveIncluded: false }), fullFeatureAcceptance: false, mobileAcceptance: false, publicationEligible: false };
  const execute = async (id, program, args, cwd = ROOT, env = process.env, timeout = 20 * 60_000) => {
    const observed = await executeCommand({ id, program, args }, { root: cwd, env, directory: evidence, timeout, captureStdout: true });
    demand(observed.status === 0 && !observed.error && !observed.timedOut && !observed.cleanupUncertain && !observed.interrupted,
      `${id} failed or process cleanup was uncertain`);
    return observed;
  };
  await withResultEvidence(evidence, result, async () => {
    let binary, receipt, receiptPath;
    if (!formal) {
      await execute('kernel-init', 'git', ['init', '--template=', source], ROOT, gitEnv(process.env));
      await execute('kernel-fetch', 'git', ['-C', source, '-c', 'protocol.file.allow=never', 'fetch', '--no-tags', '--depth=1',
        'https://github.com/polaris-arch/sing-box-polaris.git', pin.source.commit], ROOT, gitEnv(process.env));
      await execute('kernel-checkout', 'git', ['-C', source, 'checkout', '--detach', pin.source.commit], ROOT, gitEnv(process.env));
      result.sourceBefore = JSON.parse(capture('python3', ['-c', SOURCE_CHECK, ROOT, source, PIN]));
      const buildVcs = readBuildVcs(source);
      result.buildVcs = { ...buildVcs, ...expectedBuildVcs(pin, buildVcs) };
      result.officialGo = JSON.parse((await execute('official-go-prepare', 'python3',
        ['-I', '-c', OFFICIAL_GO_SETUP, 'prepare', source, PIN, directory], ROOT, gitEnv(process.env))).stdout);
      result.officialGoInventory = filePin(join(evidence, 'official-go-files.json'));
      const goRoot = join(directory, 'official-go/unpacked/go'), go = join(goRoot, 'bin/go');
      demand(result.officialGo.root === goRoot, 'Official Go toolchain escaped private slot');
      const cleanGoEnv = { ...process.env, GOTOOLCHAIN: 'local', GOENV: 'off', GOWORK: 'off',
        GOAMD64: 'v1', GOFIPS140: 'off', GOTELEMETRY: 'off' };
      demand(capture(go, ['version'], ROOT, cleanGoEnv).trim() === 'go version go1.27.2 linux/amd64', 'Pinned Linux Go compiler required');
      result.goExecutable = filePin(realpathSync(go));
      demand(result.goExecutable.path === go, 'Selected Go executable is redirected');
      demand(realpathSync(capture(go, ['env', 'GOROOT'], ROOT, cleanGoEnv).trim()) === goRoot, 'Go standard library root differs from selected compiler');
      const goEnv = { ...cleanGoEnv, GOROOT: goRoot, GOFLAGS: '-mod=readonly',
        CGO_ENABLED: '0', GOOS: 'linux', GOARCH: 'amd64', GOCACHE: join(directory, 'go-cache'),
        GOMODCACHE: join(directory, 'go-modules'), GOPATH: join(directory, 'go-path') };
      const graphText = (await execute('kernel-module-graph', go, ['list', '-m', '-json', 'all'], source, goEnv)).stdout;
      const graphFile = join(evidence, 'module-graph.jsonstream');
      writeFileSync(graphFile, graphText, { flag: 'wx', mode: 0o600 });
      const graph = JSON.parse(capture('python3', ['-c', SOURCE_CHECK, ROOT, source, PIN, graphFile]));
      demand(JSON.stringify(graph.source) === JSON.stringify(result.sourceBefore), 'Source changed during module resolution');
      const actualTags = readFileSync(join(source, 'release/DEFAULT_BUILD_TAGS_OTHERS'), 'utf8').trim() + ',with_purego';
      demand(actualTags === pin.tags, 'Reviewed tags differ from exact source');
      const ldflags = readFileSync(join(source, 'release/LDFLAGS'), 'utf8').trim()
        + ` -X github.com/sagernet/sing-box/constant.SourceRevision=${pin.source.commit}`
        + ` -X github.com/sagernet/sing-box/constant.Version=${pin.version}`;
      binary = join(bin, 'sing-box');
      await execute('kernel-build', go, ['build', '-trimpath', '-buildvcs=true', '-tags', pin.tags, '-ldflags', ldflags,
        '-o', binary, './cmd/sing-box'], source, goEnv);
      const buildInfo = capture(go, ['version', '-m', binary], source, goEnv);
      result.buildInfo = recordBuildInfo(buildInfo, join(evidence, 'kernel-build-info.txt'));
      result.observedBinary = filePin(binary);
      result.build = validateBuild(buildInfo, pin, graph.graph, buildVcs);
      result.sourceAfter = JSON.parse(capture('python3', ['-c', SOURCE_CHECK, ROOT, source, PIN]));
      demand(JSON.stringify(result.sourceBefore) === JSON.stringify(result.sourceAfter), 'Build changed source');
      demand(JSON.stringify(readBuildVcs(source)) === JSON.stringify(buildVcs), 'Build changed Git source identity');
      const goAfter = JSON.parse((await execute('official-go-verify', 'python3',
        ['-I', '-c', OFFICIAL_GO_SETUP, 'verify', source, PIN, directory], ROOT, gitEnv(process.env))).stdout);
      demand(JSON.stringify(goAfter) === JSON.stringify(result.officialGo), 'Official Go toolchain changed during build');
      result.officialGoAfterBuild = goAfter;
      demand(JSON.stringify(filePin(binary)) === JSON.stringify(result.observedBinary),
        'Observed executable changed during source verification');
      result.sourceVerified = true;
      receipt = { schema: 'polaris-runtime-loopback-candidate-v1', appCommit: candidate,
        kernelCommit: pin.source.commit, kernelTree: pin.source.tree, protoSha256: PROTO, profile: PROFILE,
        implementationManifestDigest: pin.implementationManifestDigest, binary: result.observedBinary, sourceVerified: true,
        naiveIncluded: false, publicationEligible: false };
      receiptPath = join(evidence, 'candidate.json'); writeJSON(receiptPath, receipt); result.binary = receipt.binary;
      result.retainedCandidate = retainVerifiedCandidate({ directory, evidence, binary, receiptPath, receipt,
        sourceVerified: result.sourceVerified });
    } else {
      for (const platform of ['dashboard', 'linux', 'win', 'mac-arm64', 'mac-x64', '.release-receipts', '.release-consumer.lock'])
        demand(!existsSync(join(ROOT, 'resources', platform)), 'Formal HOT requires fresh resource destinations');
      await consumeRelease(ROOT, formal.manifest, ['linux']);
      binary = join(ROOT, 'resources/linux/sing-box');
    }
    // Diagnostic inputs remain isolated. Formal Linux retains the original consumed carrier.
    for (const platform of ['dashboard', 'linux', 'win', 'mac-arm64', 'mac-x64']) {
      if (formal && platform === 'linux') continue;
      const path = join(ROOT, 'resources', platform);
      demand(!existsSync(path), 'Loopback must not inherit any runtime carrier/resource directory');
      mkdirSync(path, { recursive: true }); writeFileSync(join(path, '.keep'), '', { flag: 'wx' });
    }
    const rustEnv = { ...process.env, CARGO_TARGET_DIR: target, CARGO_INCREMENTAL: '0',
      ...(!formal ? { POLARIS_RUNTIME_CONFIG_PROTO_SOURCE: join(source, 'daemon/runtime_config.proto') } : {}) };
    result.rustCompiler = capture('rustc', ['-vV'], ROOT, rustEnv).trim();
    demand(/^host: x86_64-unknown-linux-gnu$/m.test(result.rustCompiler), 'Native Rust host differs');
    const metadata = JSON.parse(capture('cargo', ['metadata', '--locked', '--no-deps', '--format-version=1'], ROOT, rustEnv));
    if (formal) {
      result.formalCarrier = await prepareFormalCarrier(ROOT, target, metadata, execute, rustEnv);
      receipt = createFormalInput(ROOT, { candidate, appTree: appSource.tree, protoSha256: PROTO,
        runId: admission.runId, runAttempt: admission.runAttempt }, evidence);
      receiptPath = join(evidence, 'formal-input.json'); writeJSON(receiptPath, receipt);
      result.formalInput = receipt; result.formalInputFile = formalFile(receiptPath, 1024 * 1024);
      result.binary = receipt.binary; result.sourceVerified = true; result.naiveIncluded = receipt.naiveIncluded;
    }
    const compiled = await execute('app-test-build', 'cargo', ['test', '--locked', '-p', 'polaris', '--lib', '--no-run', '--message-format=json'], ROOT, rustEnv);
    result.appSourceAfterBuild = requireSameAppSource(appSource, appSourceCheck('tracked'));
    const messages = compiled.stdout.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
    const selected = selectExecutable(messages, metadata, ROOT); selected.sha256 = filePin(selected.executable).sha256;
    result.appTestExecutable = selected;
    const sysroot = capture('rustc', ['--print', 'target-libdir'], ROOT, rustEnv).trim();
    const testEnv = { ...runtimeEnvironment(messages, target, selected.executable, sysroot, rustEnv),
      POLARIS_RUNTIME_LOOPBACK_RECEIPT: receiptPath, ...(!formal ? { POLARIS_SINGBOX_PATH: binary } : {}) };
    const pinCore = formal ? formalFile : filePin;
    const verifyInput = () => {
      if (formal) {
        verifyFormalFiles(receipt);
        demand(JSON.stringify(formalFile(receiptPath, 1024 * 1024)) === JSON.stringify(result.formalInputFile), 'Formal HOT binding changed');
      }
    };
    for (const name of ADMISSION_TESTS) {
      verifyInput();
      demand(pinCore(binary).sha256 === receipt.binary.sha256 && filePin(selected.executable).sha256 === selected.sha256,
        'Admitted executable changed before admission test');
      const observed = await executeCommand({ id: name.split('::').at(-1), program: selected.executable,
        args: [name, '--exact', '--nocapture', '--test-threads=1'] },
      { root: dirname(selected.manifest), env: testEnv, directory: evidence, timeout: 5 * 60_000 });
      result.admissionTests.push({ name, passed: exactPass(observed), logSha256: observed.logSha256, logBytes: observed.logBytes });
      verifyInput();
      demand(pinCore(binary).sha256 === receipt.binary.sha256 && filePin(selected.executable).sha256 === selected.sha256,
        'Admitted executable changed during admission test');
      demand(exactPass(observed), `Selector admission test failed or did not run exactly one test: ${name}`);
    }
    demand(result.admissionTests.length === 4, 'Incomplete selector admission suite');
    for (const name of TESTS) {
      verifyInput();
      demand(pinCore(binary).sha256 === receipt.binary.sha256 && filePin(selected.executable).sha256 === selected.sha256,
        'Admitted executable changed before test');
      const observed = await executeCommand({ id: name.split('::').at(-1), program: selected.executable,
        args: [name, '--exact', '--ignored', '--nocapture', '--test-threads=1'] },
      { root: dirname(selected.manifest), env: testEnv, directory: evidence, timeout: 5 * 60_000 });
      result.tests.push({ name, passed: exactPass(observed), logSha256: observed.logSha256, logBytes: observed.logBytes });
      verifyInput();
      demand(pinCore(binary).sha256 === receipt.binary.sha256 && filePin(selected.executable).sha256 === selected.sha256,
        'Admitted executable changed during test');
      demand(exactPass(observed), `Real loopback test failed or did not run exactly one test: ${name}`);
      const proofPath = join(evidence, `${name.split('::').at(-1)}.json`);
      const manifest = validateTestEvidence(readJSON(proofPath), name, receipt, formal ? result.formalInputFile.sha256 : undefined);
      demand(!result.observedImplementationManifestDigest || result.observedImplementationManifestDigest === manifest,
        'Identical source/profile produced inconsistent implementation manifests');
      result.observedImplementationManifestDigest = manifest;
      result.tests.at(-1).evidence = filePin(proofPath);
    }
    result.appSourceAfterTests = requireSameAppSource(appSource, appSourceCheck('tracked'));
    demand(result.tests.length === 4, 'Incomplete real loopback suite');
    result.status = 'passed';
  }, async () => {
    if (result.formalInput) {
      result.formalInputAfter = verifyFormalInput(ROOT, result.formalInput, evidence);
      demand(JSON.stringify(formalFile(result.formalInputFile.path, 1024 * 1024)) === JSON.stringify(result.formalInputFile), 'Formal HOT binding changed at finalization');
      result.appSourceFinal = requireSameAppSource(appSource, appSourceCheck('tracked'));
    }
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const action = process.argv.slice(2);
  Promise.resolve().then(() => {
    demand(action.length === 1 && ['admit', 'run'].includes(action[0]), 'Only admit/run actions are supported');
    return action[0] === 'admit' ? admit() : run();
  }).catch((error) => { console.error(error.message); process.exitCode = 1; });
}
