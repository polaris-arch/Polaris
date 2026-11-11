import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { mkdtempSync, readFileSync, mkdirSync, writeFileSync, rmSync, symlinkSync, copyFileSync, linkSync, lstatSync, renameSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { after, test } from 'node:test';
import { acquireCandidate, consumeTemporaryDesktop, candidateContext, candidateFetchSelection, candidateVersionOutput, candidateInstallationContract, candidateAppDirectory, candidateBuildSelection, verifyCandidateAppCarrier, freezeCandidateBuildOutput, CANDIDATE_REF, loadCandidate, validateCandidateIndex, validateCandidateReadback, verifyCandidateMembers } from './desktop-core/native-dns-candidate.mjs';
import { verifyManagedMeshCore, managedCheckPlatform, managedCheckSelection } from './desktop-core/managed-mesh-core.mjs';
import { validateReleaseContract, RELEASE_TARGETS } from './desktop-core/release-consumer.mjs';
import { expectedTags } from './desktop-core/source-graph.mjs';
import { producerReleaseName } from './desktop-core/upstream-release.mjs';
import { cronetFixture } from './lib/cronet-payload.test.mjs';
const root = resolve(import.meta.dirname, '..'), binding = loadCandidate(root);
const source = JSON.parse(readFileSync(join(root, 'scripts/desktop-core/native-dns-source-binding.json')));
const sha = 'a'.repeat(40);
const env = { POLARIS_NATIVE_DNS_CANDIDATE: '1', GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'workflow_dispatch',
  GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REPOSITORY_ID: '1347028220', GITHUB_REF: CANDIDATE_REF,
  GITHUB_SHA: sha, CORE_CANDIDATE: sha, GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/native-dns-candidate.yml@${CANDIDATE_REF}`,
  GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1' };
const run = (_cmd, args) => args[0] === 'rev-parse' ? sha : '';
test('candidate requires exact manual branch, workflow, App SHA, run and clean checkout', () => {
  assert.equal(candidateContext(sha, env, root, run).candidate, sha);
  for (const [key, value] of Object.entries({ POLARIS_NATIVE_DNS_CANDIDATE: '0', POLARIS_NO_KERNEL_RUN: '1',
    GITHUB_ACTIONS: 'false', GITHUB_EVENT_NAME: 'pull_request', GITHUB_REF: 'refs/heads/main', GITHUB_SHA: 'b'.repeat(40),
    CORE_CANDIDATE: 'b'.repeat(40), GITHUB_WORKFLOW_REF: 'other', GITHUB_REPOSITORY_ID: '1', GITHUB_REPOSITORY: 'fork/Polaris',
    GITHUB_RUN_ID: '0', GITHUB_RUN_ATTEMPT: '0' }))
    assert.throws(() => candidateContext(sha, { ...env, [key]: value }, root, run), /context/);
  assert.throws(() => candidateContext(sha, env, root, () => 'bad'), /checkout/);
  assert.throws(() => candidateContext(sha, env, root, (_cmd, args) => args[0] === 'rev-parse' ? sha : ' M code'), /dirty/);
});
function readback() {
  return [structuredClone(binding), structuredClone(source), {
    id: binding.releaseId, tag_name: binding.tag, target_commitish: 'polaris-main', draft: false, prerelease: true, published_at: '2026-10-18',
    assets: binding.assets.map((a) => ({ name: a.name, id: a.assetId, size: a.bytes, digest: `sha256:${a.sha256}`, browser_download_url: a.url })) },
  { ref: `refs/tags/${binding.tag}`, object: { type: 'tag', sha: binding.tagObject } },
  { sha: binding.tagObject, tag: binding.tag, object: { type: 'commit', sha: binding.sourceCommit } },
  { sha: binding.sourceCommit, tree: { sha: binding.sourceTree } }, { sourceGitlinks: source.sourceGitlinks, producerReadback: {} }];
}
test('actual annotated tag proves source; creation branch need not equal SHA', () => {
  const args = readback(); assert.equal(validateCandidateReadback(...args).sourceCommit, binding.sourceCommit);
});
for (const [name, mutate] of [
  ['draft release', (a) => { a[2].draft = true; }], ['formal relabel', (a) => { a[2].prerelease = false; }],
  ['other release', (a) => { a[2].id++; }], ['lightweight tag', (a) => { a[3].object.type = 'commit'; }],
  ['changed tag object', (a) => { a[3].object.sha = 'b'.repeat(40); }], ['changed source', (a) => { a[4].object.sha = 'b'.repeat(40); }],
  ['changed tree', (a) => { a[5].tree.sha = 'b'.repeat(40); }], ['missing asset', (a) => { a[2].assets.pop(); }],
  ['changed asset ID', (a) => { a[2].assets[0].id++; }], ['changed asset digest', (a) => { a[2].assets[0].digest = 'sha256:'+'0'.repeat(64); }],
  ['changed asset URL', (a) => { a[2].assets[0].browser_download_url += '?other'; }],
  ['changed submodule', (a) => { a[6].sourceGitlinks = []; }],
]) test(`candidate public readback rejects ${name}`, () => { const args = readback(); mutate(args); assert.throws(() => validateCandidateReadback(...args)); });
test('candidate binding is excluded from formal release acceptance', () => {
  assert.throws(() => validateReleaseContract(binding));
  assert.equal(binding.AppAccepted, false); assert.equal(binding.publicationEligible, false);
  assert.equal(binding.originalIndexTagObject, null);
  assert.throws(() => validateCandidateIndex(Buffer.from('{}'), binding, source), /index hash/);
});
test('candidate binding cannot activate formal or mobile state', () => {
  const temp = mkdtempSync(join(tmpdir(), 'native-dns-binding-'));
  try {
    mkdirSync(join(temp, 'scripts/desktop-core'), { recursive: true });
    writeFileSync(join(temp, 'scripts/desktop-core/native-dns-source-binding.json'), JSON.stringify(source));
    for (const mutate of [b => { b.AppAccepted = true; }, b => { b.tagObject = null; }, b => { b.sourceCommit = 'b'.repeat(40); },
      b => { b.assets[0].url = 'https://example.com/other'; }, b => { b.sourcePolicy.metacubex.replacementFileCount = 354; }]) {
      const bad = structuredClone(binding); mutate(bad);
      writeFileSync(join(temp, 'scripts/desktop-core/native-dns-candidate.json'), JSON.stringify(bad));
      assert.throws(() => loadCandidate(temp));
    }
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
test('candidate selector is inactive for ordinary PR and package workflows', () => {
  for (const workflow of ['package.yml', 'release-risk.yml', 'desktop-core.yml']) {
    const text = readFileSync(join(root, '.github/workflows', workflow), 'utf8');
    assert.ok(!text.includes('POLARIS_NATIVE_DNS_CANDIDATE'));
  }
});

test('installed candidate members reject byte changes, omissions, additions and links', () => {
  const temp = mkdtempSync(join(tmpdir(), 'native-dns-members-'));
  const hash = (v) => createHash('sha256').update(v).digest('hex');
  const row = { archive: { root: 'original', members: { 'original/sing-box': hash('core'), 'original/LICENSE': hash('license') } } };
  const core = join(temp, 'sing-box'), license = join(temp, 'LICENSE');
  try {
    writeFileSync(core, 'core'); writeFileSync(license, 'license'); verifyCandidateMembers(temp, row);
    writeFileSync(core, 'wrong'); assert.throws(() => verifyCandidateMembers(temp, row), /bytes/); writeFileSync(core, 'core');
    rmSync(license); assert.throws(() => verifyCandidateMembers(temp, row), /inventory/); writeFileSync(license, 'license');
    writeFileSync(join(temp, 'other.dll'), 'extra'); assert.throws(() => verifyCandidateMembers(temp, row), /bytes/); rmSync(join(temp, 'other.dll'));
    mkdirSync(join(temp, 'extra')); assert.throws(() => verifyCandidateMembers(temp, row), /directory/); rmSync(join(temp, 'extra'), { recursive: true });
    if (process.platform !== 'win32') {
      rmSync(core); symlinkSync(license, core); assert.throws(() => verifyCandidateMembers(temp, row), /regular|link/i);
    }
  } finally { rmSync(temp, { recursive: true, force: true }); }
});


test('ordinary CI candidate remains exclusive manual opt-in with no selector fallback', () => {
  const ciEnv = { ...env, GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/ci.yml@${CANDIDATE_REF}` };
  assert.equal(candidateContext(sha, ciEnv, root, run).candidate, sha);
  assert.equal(candidateFetchSelection([], {}), null);
  assert.equal(candidateFetchSelection([`--native-dns-candidate=${sha}`], ciEnv), sha);
  for (const bad of [{}, { ...ciEnv, POLARIS_NATIVE_DNS_CANDIDATE: '0' }, { ...ciEnv, CORE_CANDIDATE: 'b'.repeat(40) }])
    assert.throws(() => candidateFetchSelection([`--native-dns-candidate=${sha}`], bad));
  for (const args of [[], [`--native-dns-candidate=${sha}`, '--platform=linux'], [`--native-dns-candidate=${sha}`, `--native-dns-candidate=${sha}`]])
    assert.throws(() => candidateFetchSelection(args, ciEnv));
  assert.throws(() => candidateContext(sha, { ...ciEnv, GITHUB_EVENT_NAME: 'pull_request' }, root, run));
});

test('candidate exact version text includes actual Revision and CGO setting', () => {
  const row = { schema: 'polaris-upstream-native-receipt-v1', sourceCommit: binding.sourceCommit,
    version: binding.version, goVersion: 'go1.27.2', target: 'linux/amd64', tags: 'one,two',
    buildGraph: { settings: { CGO_ENABLED: '0' } } };
  assert.equal(candidateVersionOutput(row), `sing-box version ${binding.version}\n\nEnvironment: go1.27.2 linux/amd64\nTags: one,two\nRevision: ${binding.sourceCommit}\nCGO: disabled\n`);
  assert.throws(() => candidateVersionOutput({ ...row, sourceCommit: 'old' }));
  assert.throws(() => candidateVersionOutput({ ...row, buildGraph: { settings: {} } }));
});

test('candidate installation identity uses its distinct exact contract digest and carrier namespace', () => {
  const hash = createHash('sha256').update(readFileSync(join(root, 'scripts/desktop-core/native-dns-candidate.json'))).digest('hex');
  const rows=Object.fromEntries(Object.entries(binding.platforms).map(([key,row])=>[key,{...row,schema:'polaris-upstream-native-receipt-v1',target:({linux:'linux/amd64',win:'windows/amd64','mac-x64':'darwin/amd64','mac-arm64':'darwin/arm64'})[key],sourceCommit:binding.sourceCommit,sourceTree:binding.sourceTree,version:binding.version,
    finalPackageCronetSmoke:{windowsExecution:{schema:'synthetic-transport-only',native:{fileId:'1'.repeat(32)}}}}]));
  const contract = candidateInstallationContract(binding, hash, rows);
  const formal = JSON.parse(readFileSync(join(root, 'scripts/desktop-core/release-contract.json')));
  for (const key of Object.keys(binding.platforms)) {
    assert.equal(contract.platforms[key].providerFingerprint, hash);
    assert.notEqual(hash, formal.platforms[key].providerFingerprint);
    assert.deepEqual(contract.platforms[key].core, binding.platforms[key].core);
    assert.equal(candidateAppDirectory(root, key), join(root, 'resources/.native-dns-app', key));
  }
  assert.deepEqual(contract.platforms.win.finalPackageCronetSmoke.windowsExecution,rows.win.finalPackageCronetSmoke.windowsExecution);
  assert.equal(contract.platforms.win.sourceCommit,binding.sourceCommit);
  assert.throws(()=>candidateInstallationContract(binding,hash));
  assert.throws(()=>candidateInstallationContract(binding,hash,{...rows,win:{...rows.win,target:'linux/amd64'}}));
  assert.throws(()=>candidateInstallationContract(binding,hash,{...rows,win:{...rows.win,sourceCommit:'0'.repeat(40)}}));
  assert.throws(() => candidateAppDirectory(root, 'android'));
  assert.throws(() => candidateInstallationContract({ ...binding, AppAccepted: true }, hash));
  assert.throws(() => candidateInstallationContract(binding, 'fake'));
});


test('candidate build rejects mode, mixed App identity, foreign target and alternate carrier', () => {
  const source = { schema: 'polaris-native-dns-compiled-source-v1', validationOnly: true,
    AppAccepted: false, publicationEligible: false, context: { candidate: sha } };
  const buildEnv = { ...env, POLARIS_BUILD_ID: sha };
  const directory = candidateAppDirectory(root, 'linux');
  const host = { os: 'linux', arch: 'x64' };
  assert.doesNotThrow(() => candidateBuildSelection(source, root, 'linux', directory, buildEnv, host));
  for (const bad of [{ ...buildEnv, POLARIS_NATIVE_DNS_CANDIDATE: '0' },
    { ...buildEnv, POLARIS_BUILD_ID: 'b'.repeat(40) }, { ...buildEnv, CARGO_TARGET_DIR: '' }])
    assert.throws(() => candidateBuildSelection(source, root, 'linux', directory, bad, host));
  assert.throws(() => candidateBuildSelection(source, root, 'win', directory, buildEnv, host));
  assert.throws(() => candidateBuildSelection(source, root, 'linux', join(root, 'resources/linux'), buildEnv, host));
  assert.throws(() => candidateBuildSelection({ ...source, validationOnly: false }, root, 'linux', directory, buildEnv, host));
});

test('managed mesh rejects malformed candidate before any core execution or formal fallback', () => {
  for (const value of ['', '0', 'true']) {
    let calls = 0;
    assert.throws(() => verifyManagedMeshCore(root, {}, '/never/execute', () => { calls++; },
      { os: 'linux', arch: 'x64' }, { ...env, POLARIS_NATIVE_DNS_CANDIDATE: value }), /context/);
    assert.equal(calls, 0);
  }
});


for (const platform of ['linux', 'win', 'mac-x64', 'mac-arm64']) test(`frozen ${platform} helpers survive mutable Cargo outputs but reject carrier or provenance changes`, (t) => {
  const temp = mkdtempSync(join(tmpdir(), 'native-dns-frozen-helper-'));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  const directory = candidateAppDirectory(temp, platform), target = join(temp, 'target/debug');
  mkdirSync(directory, { recursive: true }); mkdirSync(target, { recursive: true });
  const selectedSource = { schema: 'polaris-native-dns-compiled-source-v1', validationOnly: true,
    sourceCommit: binding.sourceCommit, sourceTree: binding.sourceTree, bindingSha256: 'c'.repeat(64),
    context: { candidate: sha, runId: '123', attempt: '1' },
    platforms: ['linux', 'win', 'mac-x64', 'mac-arm64'], AppAccepted: false, publicationEligible: false };
  const names = [platform === 'win' ? 'polaris-helper.exe' : 'polaris-helper',
    ...(platform === 'win' ? ['polaris-cleaner.exe'] : [])];
  const bytes = Object.fromEntries(names.map((name) => [name, Buffer.from(`inert same-App ${name}`)]));
  const built = Object.fromEntries(names.map((name) => {
    writeFileSync(join(target, name), bytes[name]); writeFileSync(join(directory, name), bytes[name]);
    return [name, { bytes: bytes[name].length, sha256: createHash('sha256').update(bytes[name]).digest('hex') }];
  }));
  const receipt = { schema: 'polaris-native-dns-app-carrier-v1', source: selectedSource, built };
  const save = (value) => writeFileSync(join(directory, 'candidate-carrier.json'), JSON.stringify(value));
  const verify = () => verifyCandidateAppCarrier(directory, selectedSource, platform);
  save(receipt); assert.deepEqual(verify(), receipt);
  // Legitimate workspace feature unification may replace every mutable output.
  for (const name of names) writeFileSync(join(target, name), `different workspace features: ${name}`);
  assert.deepEqual(verify(), receipt);
  rmSync(target, { recursive: true }); assert.deepEqual(verify(), receipt);
  for (const name of names) {
    writeFileSync(join(directory, name), 'changed frozen carrier');
    assert.throws(verify, /selected bytes differ/);
    writeFileSync(join(directory, name), bytes[name]);
    const missing = structuredClone(receipt); delete missing.built[name]; save(missing);
    assert.throws(verify, /helper inventory/); save(receipt);
    const noHash = structuredClone(receipt); noHash.built[name].sha256 = null; save(noHash);
    assert.throws(verify, /helper pin/); save(receipt);
  }
  for (const mutate of [
    r => { r.schema = 'other'; }, r => { r.extra = true; },
    r => { r.source.sourceCommit = 'b'.repeat(40); }, r => { r.source.context.candidate = 'b'.repeat(40); },
    r => { r.source.context.runId = '124'; }, r => { r.source.context.attempt = '2'; },
    r => { r.source.bindingSha256 = 'd'.repeat(64); }, r => { r.built.other = built[names[0]]; },
    r => { r.built[names[0]].bytes = 0; }, r => { r.built[names[0]].extra = true; },
  ]) {
    const changed = structuredClone(receipt); mutate(changed); save(changed); assert.throws(verify); save(receipt);
  }
});

function assertPreparationBranches(prepare) {
  const branch = (needle, after = 0) => {
    const at = prepare.indexOf('\n' + needle, after);
    assert(at >= 0, `actual preparation branch missing: ${needle}`);
    assert.equal(prepare.indexOf('\n' + needle, at + 1), -1, 'branch anchor must be unique');
    return at + 1;
  };
  const maintenance = branch("  if (mode === 'maintenance-anchor') {");
  const start = branch("  } else if (mode === 'prepare' && temporaryDefault && !candidateMode) {", maintenance);
  const manual = branch("  } else if (mode === 'prepare') {", start);
  const end = branch('  } else {', manual);
  assert(maintenance < start && start < manual && manual < end);
  const anchor = prepare.slice(maintenance, start);
  assert.doesNotMatch(anchor, /freezeCandidateBuildOutput|writeSelectedCoreManifest|writeFileSync/);
  assert.ok(prepare.slice(start, manual).includes('actualCoreFile(join(directory, name))'));
  assert.doesNotMatch(prepare.slice(start, manual), /freezeCandidateBuildOutput|target\/debug/);
  assert.ok(prepare.slice(manual, end).includes('freezeCandidateBuildOutput(root, directory, name)'));
  assert.ok(prepare.slice(manual, end).includes('actualCoreFile(join(directory, name), expected)'));
  assert.doesNotMatch(prepare.slice(end), /target\/debug/);
  assert.ok(prepare.slice(end).includes('verifyCandidateAppCarrier(directory, source, platform)'));
  assert.ok(prepare.slice(end).includes('old.core.helperSha256'));
  assert.ok(prepare.slice(end).includes('old.core.cleanerSha256'));
}
test('candidate preparation alone reads mutable build outputs; verification uses frozen receipt plus manifest', () => {
  const prepare = readFileSync(join(root, 'scripts/prepare-core-identity.mjs'), 'utf8');
  assertPreparationBranches(prepare);
  for (const required of [
    'actualCoreFile(join(directory, name))',
    'freezeCandidateBuildOutput(root, directory, name)',
    'actualCoreFile(join(directory, name), expected)',
    'verifyCandidateAppCarrier(directory, source, platform)',
    'old.core.helperSha256', 'old.core.cleanerSha256',
  ]) {
    assert(prepare.includes(required));
    assert.throws(() => assertPreparationBranches(prepare.replaceAll(required, 'OMITTED')));
  }
});


test('candidate acquisition uses captured API-only Actions credential without leaking to assets or tools', () => {
  // Exercise the actual reader-construction expression from acquisition, rather
  // than a parallel imitation that would miss an accidental env:{} override.
  const source = readFileSync(join(root, 'scripts/desktop-core/native-dns-candidate.mjs'), 'utf8');
  const expression = /const fetchBytes = (options\.read \?\? releasePublicReader\([^;]+\));/.exec(source)?.[1];
  assert.ok(expression);
  const program = `
    import assert from 'node:assert/strict';
    import {execFileSync} from 'node:child_process';
    const {releasePublicReader} = await import(${JSON.stringify(pathToFileURL(join(root, 'scripts/desktop-core/release-consumer.mjs')).href)});
    assert.equal(Object.hasOwn(process.env, 'POLARIS_RELEASE_API_TOKEN'), false);
    assert.equal(execFileSync(process.execPath, ['-e', "process.stdout.write(String(Object.hasOwn(process.env,'POLARIS_RELEASE_API_TOKEN')))"], {encoding:'utf8'}), 'false');
    const calls=[];
    globalThis.fetch=async(url,options)=>{calls.push({url,...options});return new Response('ok');};
    const repository='polaris-arch/sing-box-polaris';
    const read=new Function('options','releasePublicReader','binding', ${JSON.stringify('return '+expression)})({},releasePublicReader,{repository});
    const api='https://api.github.com/repos/'+repository;
    await read(api+'/releases/416260275',2);
    assert.equal(calls.pop().headers.Authorization,'Bearer inert-candidate-fixture-token');
    for(const url of ['https://api.github.com/repos/other/repo',api+'?query=inert',api+'#fragment',api.replace('https:','http:'),'https://user@api.github.com/repos/'+repository]) {
      await read(url,2);assert.equal(calls.pop().headers.Authorization,undefined);
    }
    globalThis.fetch=async(url,options)=>{calls.push({url,...options});return url.startsWith('https://github.com/')
      ?new Response(null,{status:302,headers:{location:'https://release-assets.githubusercontent.com/inert?signature=fixture'}}):new Response('ok');};
    await read('https://github.com/'+repository+'/releases/download/inert/core.zip',2,true);
    assert.equal(calls.length,2);assert(calls.every(c=>c.headers.Authorization===undefined&&c.redirect==='manual'));calls.length=0;
    globalThis.fetch=async(url,options)=>{calls.push({url,...options});return new Response(null,{status:302,headers:{location:'https://example.com/never-follow'}});};
    await assert.rejects(read(api+'/releases/416260275',2),/Unapproved release redirect/);
    assert.equal(calls.length,1);assert.equal(calls[0].headers.Authorization,'Bearer inert-candidate-fixture-token');
    assert.equal(Object.hasOwn(process.env,'POLARIS_RELEASE_API_TOKEN'),false);
  `;
  execFileSync(process.execPath, ['--input-type=module', '-e', program], {
    env: { ...process.env, GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'inert-candidate-fixture-token' },
    encoding: 'utf8', timeout: 10000,
  });
});

for (const name of ['polaris-helper', 'polaris-helper.exe', 'polaris-cleaner.exe']) test(`Cargo hardlinked ${name} freezes into independent strict carrier`, (t) => {
  const temp = mkdtempSync(join(tmpdir(), 'native-dns-cargo-hardlink-'));
  t.after(() => rmSync(temp, {recursive:true,force:true}));
  const target=join(temp,'target/debug'), directory=join(temp,'carrier');
  mkdirSync(target,{recursive:true});mkdirSync(directory);
  const artifact=join(target,'deps-artifact'), from=join(target,name), to=join(directory,name);
  writeFileSync(artifact,'inert actual build bytes');linkSync(artifact,from);
  assert.equal(lstatSync(from).nlink,2);
  const pin=freezeCandidateBuildOutput(temp,directory,name);
  assert.equal(pin.sha256,createHash('sha256').update('inert actual build bytes').digest('hex'));
  assert.equal(lstatSync(to).nlink,1);assert.equal(readFileSync(to,'utf8'),'inert actual build bytes');
  writeFileSync(artifact,'later Cargo build');assert.equal(readFileSync(to,'utf8'),'inert actual build bytes');
  assert.throws(()=>freezeCandidateBuildOutput(temp,directory,name),/EEXIST/);
  assert.equal(readFileSync(to,'utf8'),'inert actual build bytes');
});

test('candidate build freeze rejects linked carrier, replaced source, mixed copy and invalid source', (t) => {
  const temp=mkdtempSync(join(tmpdir(),'native-dns-freeze-negative-'));
  t.after(()=>rmSync(temp,{recursive:true,force:true}));
  const target=join(temp,'target/debug'),directory=join(temp,'carrier'),name='polaris-helper';
  mkdirSync(target,{recursive:true});mkdirSync(directory);
  const from=join(target,name),to=join(directory,name);
  const reset=()=>{rmSync(from,{force:true,recursive:true});rmSync(to,{force:true,recursive:true});writeFileSync(from,'original build bytes');};
  reset();
  assert.throws(()=>freezeCandidateBuildOutput(temp,directory,'sing-box'),/helper\/cleaner/);
  for(const copy of [
    (a,b,flags)=>{renameSync(a,a+'.old');writeFileSync(a,'original build bytes');copyFileSync(a,b,flags);},
    (a,b,flags)=>{writeFileSync(a,'replacement build');copyFileSync(a,b,flags);},
    (_a,b)=>writeFileSync(b,'mixed other build',{flag:'wx'}),
    (a,b,flags)=>{copyFileSync(a,b,flags);linkSync(b,b+'.alias');},
  ]) {
    reset();assert.throws(()=>freezeCandidateBuildOutput(temp,directory,name,copy),/changed during freezing|selected bytes differ|single-link/);
  }
  reset();rmSync(from);mkdirSync(from);assert.throws(()=>freezeCandidateBuildOutput(temp,directory,name),/Non-regular/);
  rmSync(from,{recursive:true});writeFileSync(from,'');assert.throws(()=>freezeCandidateBuildOutput(temp,directory,name),/identity differs/);
  rmSync(from);assert.throws(()=>freezeCandidateBuildOutput(temp,directory,name),/ENOENT/);
  if(process.platform!=='win32') {
    writeFileSync(join(target,'real'),'bytes');symlinkSync('real',from);
    assert.throws(()=>freezeCandidateBuildOutput(temp,directory,name),/Non-regular/);
  }
});

// Recreate the historical temporary default explicitly. The actual checkout now
// selects the formal release; these fixtures retain the exact old binding bytes.
const temporary = await import('./desktop-core/native-dns-candidate.mjs');
const currentManifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
const defaultManifest = {
  desktopTemporary: structuredClone(currentManifest.historicalDesktopTemporary),
  historicalDesktopRelease: structuredClone(currentManifest.historicalDesktopRelease),
};
const defaultRoot = mkdtempSync(join(tmpdir(), 'historical-temporary-selection-'));
after(() => rmSync(defaultRoot, { recursive: true, force: true }));
mkdirSync(join(defaultRoot, 'scripts/desktop-core'), { recursive: true });
mkdirSync(join(defaultRoot, 'src-tauri'), { recursive: true });
for (const name of ['native-dns-candidate.json', 'native-dns-source-binding.json'])
  copyFileSync(join(root, 'scripts/desktop-core', name), join(defaultRoot, 'scripts/desktop-core', name));
writeFileSync(join(defaultRoot, 'src-tauri/core-manifest.json'), JSON.stringify(defaultManifest));
writeFileSync(join(defaultRoot, 'NOTICE'), temporary.temporaryDesktopNoticeLines(defaultRoot).join('\n') + '\n');
const defaultRun = (_cmd, args) => args[0] === 'show'
  ? readFileSync(join(defaultRoot, args[1].slice('HEAD:'.length)), 'utf8')
  : args[0] === 'status' ? '' : args[1] === 'HEAD^{tree}' ? 'c'.repeat(40) : sha;
test('temporary default has exact independent pins and ordinary committed App contexts', () => {
  assert.equal(temporary.loadTemporaryDesktopSelection(defaultRoot).sourceCommit, binding.sourceCommit);
  assert.equal(temporary.hasTemporaryDesktop(currentManifest), false);
  assert.throws(() => temporary.loadTemporaryDesktopSelection(root), /exclusive desktop default/);
  for (const event of ['push', 'pull_request', 'merge_group', 'workflow_dispatch']) {
    const ordinary = { ...env, GITHUB_EVENT_NAME: event, GITHUB_REF: 'refs/heads/main' };
    delete ordinary.POLARIS_NATIVE_DNS_CANDIDATE;
    assert.equal(temporary.selectedCandidateContext(defaultRoot, ordinary, defaultRun).mode, 'temporary-default');
  }
  assert.equal(temporary.selectedCandidateContext(defaultRoot, {}, defaultRun).candidate, sha);
});
test('temporary selection rejects mixed formal, old source, other provider and missing real pins', () => {
  for (const key of Object.keys(defaultManifest.desktopTemporary)) {
    const changed = structuredClone(defaultManifest); delete changed.desktopTemporary[key];
    assert.throws(() => temporary.loadTemporaryDesktopSelection(defaultRoot, changed), /selection/);
  }
  for (const mutate of [m => { m.desktopRelease = m.historicalDesktopRelease; },
    m => { m.desktopTemporary.sourceCommit = m.historicalDesktopRelease.sourceCommit; },
    m => { m.desktopTemporary.contractSha256 = m.historicalDesktopRelease.contractSha256; },
    m => { m.desktopTemporary.producer.runId++; },
    m => { m.desktopTemporary.librarySha256.linux = '0'.repeat(64); }]) {
    const changed = structuredClone(defaultManifest); mutate(changed);
    assert.throws(() => temporary.loadTemporaryDesktopSelection(defaultRoot, changed));
  }
});
test('default context rejects uncommitted selector and mismatched App SHA or foreign repository', () => {
  assert.throws(() => temporary.selectedCandidateContext(defaultRoot, {}, (_cmd, args) => args[0] === 'show' ? '{}' : sha), /committed/);
  assert.throws(() => temporary.selectedCandidateContext(defaultRoot, {}, (cmd, args) => args[0] === 'status'
    ? ' M src-tauri/src/lib.rs' : defaultRun(cmd, args)), /tracked changes/);
  const ordinary = { ...env, GITHUB_EVENT_NAME: 'push', GITHUB_REF: 'refs/heads/main' }; delete ordinary.POLARIS_NATIVE_DNS_CANDIDATE;
  for (const patch of [{ GITHUB_SHA: '0'.repeat(40) }, { GITHUB_REPOSITORY: 'foreign/repo' },
    { GITHUB_RUN_ATTEMPT: '0' }, { CORE_CANDIDATE: '0'.repeat(40) }])
    assert.throws(() => temporary.selectedCandidateContext(defaultRoot, { ...ordinary, ...patch }, defaultRun));
  assert.throws(() => temporary.selectedCandidateContext(defaultRoot, { ...ordinary, POLARIS_NATIVE_DNS_CANDIDATE: '1' }, defaultRun), /manual/);
});
test('temporary notice retains exact public source and makes mobile binding explicitly pending', () => {
  const text = readFileSync(join(defaultRoot, 'NOTICE'), 'utf8'); temporary.validateTemporaryDesktopNotice(text, defaultRoot);
  for (const line of temporary.temporaryDesktopNoticeLines(defaultRoot))
    assert.throws(() => temporary.validateTemporaryDesktopNotice(text.replace(line, ''), defaultRoot));
});
test('temporary installation rejects existing/partial destinations and rolls back only its own writes', () => {
  const temp = mkdtempSync(join(tmpdir(), 'temporary-commit-'));
  const resources = join(temp, 'resources'), staging = join(temp, 'stage');
  const names = ['linux', 'win', 'mac-x64', 'mac-arm64', '.native-dns-candidate'];
  try {
    mkdirSync(resources); mkdirSync(staging);
    for (const name of names) { mkdirSync(join(staging, name)); writeFileSync(join(staging, name, 'fixture'), name); }
    mkdirSync(join(resources, 'linux')); writeFileSync(join(resources, 'linux', 'old-577d'), 'historical');
    assert.throws(() => temporary.installCandidateStage(staging, resources), /overlay/);
    assert.equal(readFileSync(join(resources, 'linux', 'old-577d'), 'utf8'), 'historical');
    rmSync(join(resources, 'linux'), { recursive: true });
    let calls = 0;
    assert.throws(() => temporary.installCandidateStage(staging, resources, (from, to) => {
      if (++calls === 3) throw Error('injected install failure'); renameSync(from, to);
    }), /injected/);
    for (const name of names) assert.equal(existsSyncForTest(join(resources, name)), false);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
function existsSyncForTest(path) { try { lstatSync(path); return true; } catch (error) { if (error.code === 'ENOENT') return false; throw error; } }
test('temporary carrier only permits original members plus exact installation files', () => {
  const temp = mkdtempSync(join(tmpdir(), 'temporary-carrier-'));
  const hash = value => createHash('sha256').update(value).digest('hex');
  const row = { archive: { root: 'original', members: { 'original/sing-box': hash('core'), 'original/LICENSE': hash('license') } } };
  try {
    writeFileSync(join(temp, 'sing-box'), 'core'); writeFileSync(join(temp, 'LICENSE'), 'license');
    temporary.verifyTemporaryCarrierMembers(temp, row, 'linux');
    for (const name of ['polaris-helper', 'core-manifest.json', 'candidate-carrier.json']) writeFileSync(join(temp, name), '{}');
    temporary.verifyTemporaryCarrierMembers(temp, row, 'linux');
    writeFileSync(join(temp, 'other-executable'), 'evil');
    assert.throws(() => temporary.verifyTemporaryCarrierMembers(temp, row, 'linux'), /Unexpected/);
    rmSync(join(temp, 'other-executable')); writeFileSync(join(temp, 'LICENSE'), 'changed');
    assert.throws(() => temporary.verifyTemporaryCarrierMembers(temp, row, 'linux'), /differs/);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});


test('selected temporary managed configuration checks map exactly four native desktop hosts', () => {
  for (const [os, arch, key] of [['linux','x64','linux'], ['win32','x64','win'],
    ['darwin','x64','mac-x64'], ['darwin','arm64','mac-arm64']])
    assert.equal(managedCheckPlatform({ os, arch }), key);
  for (const host of [{ os: 'linux', arch: 'arm64' }, { os: 'win32', arch: 'arm64' },
    { os: 'android', arch: 'x64' }, { os: 'darwin', arch: '' }])
    assert.throws(() => managedCheckPlatform(host), /native desktop/);
});

test('default PR head context requires the exact event head and correct base repository', () => {
  const temp = mkdtempSync(join(tmpdir(), 'temporary-pr-event-')), eventPath = join(temp, 'event.json');
  const ordinary = { ...env, GITHUB_EVENT_NAME: 'pull_request', GITHUB_SHA: 'b'.repeat(40), GITHUB_EVENT_PATH: eventPath };
  delete ordinary.POLARIS_NATIVE_DNS_CANDIDATE;
  const event = { pull_request: { base: { repo: { full_name: 'polaris-arch/Polaris' } }, head: { sha } } };
  try {
    writeFileSync(eventPath, JSON.stringify(event));
    assert.equal(temporary.selectedCandidateContext(defaultRoot, ordinary, defaultRun).candidate, sha);
    for (const mutate of [e => { e.pull_request.head.sha = 'b'.repeat(40); },
      e => { e.pull_request.base.repo.full_name = 'foreign/repo'; }, e => { delete e.pull_request; }]) {
      const bad = structuredClone(event); mutate(bad); writeFileSync(eventPath, JSON.stringify(bad));
      assert.throws(() => temporary.selectedCandidateContext(defaultRoot, ordinary, defaultRun));
    }
  } finally { rmSync(temp, { recursive: true, force: true }); }
});

const nativeTemporary = await import('./desktop-core/release-native-gates.mjs');
const { portableCanonical } = await import('./desktop-core/release-consumer.mjs');
const hashBytes = bytes => createHash('sha256').update(bytes).digest('hex');
function temporaryGateFixture() {
  const ordinary = { ...env, GITHUB_EVENT_NAME: 'push', GITHUB_REF: 'refs/heads/main' };
  delete ordinary.POLARIS_NATIVE_DNS_CANDIDATE;
  const context = temporary.selectedCandidateContext(defaultRoot, ordinary, defaultRun), selection = defaultManifest.desktopTemporary;
  const producerReadback = { runId: binding.producer.runId, runAttempt: binding.producer.runAttempt,
    sourceCommit: binding.sourceCommit, workflow: binding.producer.workflow, workflowId: 123,
    status: 'completed', conclusion: 'success', jobs: Object.fromEntries(Object.entries(binding.producer.nativeJobs)
      .map(([key, pin]) => [key, { ...pin, runId: binding.producer.runId, runAttempt: binding.producer.runAttempt,
        sourceCommit: binding.sourceCommit, status: 'completed', conclusion: 'success' }])) };
  // Synthetic offline observations use the exact pinned values. No test calls
  // GitHub or claims these generated receipts came from an actual native run.
  const receipt = { schema: 'polaris-native-dns-acquisition-v1', context, bindingSha256: selection.contractSha256,
    sourceVerified: true, binaryBytesVerified: true, runtimeValidated: false, AppAccepted: false, publicationEligible: false,
    publicReadback: { releaseId: binding.releaseId, tag: binding.tag, tagObject: binding.tagObject,
      sourceCommit: binding.sourceCommit, sourceTree: binding.sourceTree, assets: binding.assets,
      sourceGitlinks: source.sourceGitlinks, producerReadback },
    payloads: Object.fromEntries(Object.entries(binding.platforms).map(([key, row]) => [key, { core: row.core, sidecar: row.sidecar }])) };
  const hosts = { linux: ['linux','x64'], win: ['win32','x64'], 'mac-x64': ['darwin','x64'], 'mac-arm64': ['darwin','arm64'] };
  const gates = Object.fromEntries(Object.entries(hosts).map(([key, [host, arch]]) => {
    const facts = { schema: 'polaris-app-native-temporary-gate-v1', candidate: sha, context,
      contractSha256: selection.contractSha256, platform: key, host, arch, receipt: structuredClone(receipt),
      indexSha256: source.index.sha256, inspection: 'download-extract-source-buildinfo-buildid-cronet-native-signature',
      actualAppAcceptance: 'pending load, feature and final carrier' };
    return [key, { ...facts, fingerprint: hashBytes(portableCanonical(facts)) }];
  }));
  return { ordinary, context, selection, gates };
}
test('managed check labels follow admitted receipts when manual and committed default coexist', () => {
  const f = temporaryGateFixture();
  assert.equal(temporary.hasTemporaryDesktop(defaultManifest), true);
  const manualEnv = { ...env, GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/ci.yml@${CANDIDATE_REF}` };
  const manualContext = temporary.selectedCandidateContext(defaultRoot, manualEnv, run);
  const manualReceipt = structuredClone(f.gates.linux.receipt);
  manualReceipt.context = manualContext;
  const verify = (receipt, context) => temporary.validateCandidateAcquisitionReceipt(
    receipt, binding, source, context, f.selection.contractSha256);
  assert.equal(managedCheckSelection(verify(manualReceipt, manualContext).context, true), 'native-dns-candidate');
  assert.equal(managedCheckSelection(verify(f.gates.linux.receipt, f.context).context, false), 'temporary-desktop-default');
  assert.throws(() => verify(manualReceipt, f.context), /another App attempt/);
  assert.throws(() => verify(f.gates.linux.receipt, manualContext), /another App attempt/);
  assert.throws(() => managedCheckSelection(manualContext, false), /mode differs/);
  assert.throws(() => managedCheckSelection(f.context, true), /mode differs/);
  for (const mode of ['manual', 'formal', null, undefined]) {
    const bad = { ...manualContext, mode };
    assert.throws(() => managedCheckSelection(bad, true), /mode differs/);
    assert.throws(() => managedCheckSelection(bad, false), /mode differs/);
  }
  const verifier = readFileSync(join(root, 'scripts/desktop-core/managed-mesh-core.mjs'), 'utf8');
  assert.match(verifier, /verifyCandidatePayload\(root, key, core, run\);\s*const selection = managedCheckSelection\(receipt.context, manual\);\s*const output = run\(core, \['version'\]/);
  assert.match(verifier, /return \{ selection, platform: key/);
  // Selection labels belong to this acquisition verifier and its context-bound
  // receipts above. Rust's selected-core check independently requires compiled
  // execution capability and typed staged receipts; it must refuse a legacy
  // carrier before publication, not run it merely to obtain this CLI label.
});
test('temporary native gates reject source, attempt, platform, payload and acceptance relabels', () => {
  const f = temporaryGateFixture();
  for (const [key, gate] of Object.entries(f.gates)) {
    nativeTemporary.verifyNativeTemporaryGate(gate, binding, f.selection, source, f.context, key);
    for (const mutate of [g => { g.schema = 'polaris-app-native-release-gate-v1'; },
      g => { g.context.attempt.attempt = '2'; }, g => { g.candidate = 'b'.repeat(40); },
      g => { g.host = 'other'; }, g => { g.arch = 'other'; }, g => { g.indexSha256 = '0'.repeat(64); },
      g => { g.receipt.publicReadback.sourceCommit = 'b'.repeat(40); },
      g => { g.receipt.publicReadback.producerReadback.jobs[key].conclusion = 'failure'; },
      g => { g.receipt.publicReadback.producerReadback.runId++; },
      g => { g.receipt.payloads[key].core.sha256 = '0'.repeat(64); },
      g => { delete g.receipt.payloads[key]; }, g => { g.receipt.AppAccepted = true; },
      g => { g.receipt.runtimeValidated = true; }, g => { g.actualAppAcceptance = 'passed'; }]) {
      const changed = structuredClone(gate); mutate(changed);
      const { fingerprint: _, ...facts } = changed; changed.fingerprint = hashBytes(portableCanonical(facts));
      assert.throws(() => nativeTemporary.verifyNativeTemporaryGate(changed, binding, f.selection, source, f.context, key));
    }
  }
});
test('temporary bundle requires exactly four same-attempt native gate files and their complete hashes', () => {
  const f = temporaryGateFixture(), temp = mkdtempSync(join(tmpdir(), 'temporary-native-bundle-'));
  const inventory = { schema: 'polaris-app-native-temporary-bundle-v1', candidate: sha, context: f.context,
    contractSha256: f.selection.contractSha256, platforms: {} };
  try {
    for (const [key, gate] of Object.entries(f.gates)) {
      mkdirSync(join(temp, key)); const bytes = JSON.stringify(gate);
      writeFileSync(join(temp, key, 'native-gate.json'), bytes); inventory.platforms[key] = hashBytes(bytes);
    }
    const verify = () => nativeTemporary.verifyNativeTemporaryBundle(defaultRoot, defaultManifest, temp, sha, f.ordinary, defaultRun);
    const save = value => writeFileSync(join(temp, 'bundle.json'), JSON.stringify(value));
    save(inventory); verify();
    for (const mutate of [i => { i.schema = 'polaris-app-native-release-bundle-v1'; },
      i => { delete i.platforms.win; }, i => { i.platforms.linux = '0'.repeat(64); },
      i => { i.context.attempt.runId = '999'; }, i => { i.candidate = 'b'.repeat(40); }]) {
      const bad = structuredClone(inventory); mutate(bad); save(bad); assert.throws(verify);
    }
    save(inventory); writeFileSync(join(temp, 'extra'), 'no'); assert.throws(verify); rmSync(join(temp, 'extra'));
    writeFileSync(join(temp, 'linux', 'extra'), 'no'); assert.throws(verify); rmSync(join(temp, 'linux', 'extra'));
    writeFileSync(join(temp, 'linux', 'native-gate.json'), '{}'); assert.throws(verify);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});


test('default fetch cannot invoke historical formal recovery', () => {
  const fetch = readFileSync(join(root, 'scripts/fetch-core.mjs'), 'utf8');
  assert.match(fetch, /if \(hasTemporaryDesktop\(manifest\)\) throw Error\('Temporary default cannot recover/);
  assert.ok(fetch.indexOf('Temporary default cannot recover') < fetch.indexOf('recoverRelease(ROOT)'));
});


// Actual acquireCandidate and the unchanged shared API reader run below. The
// committed input's four-key compact shape/source coordinates are retained,
// but private test pins bind inert bytes. Git/Go/archive/source-verifier and
// signature subprocesses are dependency-injected simulations, never native
// execution, genuine release material or full source-owned proof acceptance.
function candidateAcquisitionFixture(t, options = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'candidate-owner-acquisition-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  mkdirSync(join(directory, 'resources'));
  mkdirSync(join(directory, 'scripts/desktop-core'), { recursive: true });
  mkdirSync(join(directory, 'src-tauri'));
  const b = structuredClone(binding), s = structuredClone(source), keys = Object.keys(RELEASE_TARGETS);
  const hash = bytes => createHash('sha256').update(bytes).digest('hex');
  const encode = value => Buffer.from(JSON.stringify(value));
  const blobs = new Map(), files = {}, rows = {}, events = [], sourceChecks = [], extraction = [], goChecks = [];
  const controls = {}, counts = new Map();
  const assetPin = name => { const pin = b.assets.find(p => p.name === name); assert(pin, name); return pin; };
  const repin = (pin, bytes) => { Object.assign(pin, { bytes: bytes.length, sha256: hash(bytes) }); blobs.set(pin.url, bytes); };
  const sourceBytes = Buffer.from('inert source; full source-owned verifier is mocked');
  repin(assetPin(s.sourceArchive.name), sourceBytes);
  Object.assign(s.sourceArchive, { bytes: sourceBytes.length, sha256: hash(sourceBytes) });
  for (const key of keys) {
    const target = RELEASE_TARGETS[key], pin = b.platforms[key], core = Buffer.alloc(256);
    if (key === 'linux') { core.set([127, 69, 76, 70, 2, 1]); core.writeUInt16LE(62, 18); }
    else if (key === 'win') { core.write('MZ'); core.writeUInt32LE(128, 60); core.write('PE\0\0', 128); core.writeUInt16LE(0x8664, 132); }
    else { core.writeUInt32LE(0xfeedfacf); core.writeUInt32LE(key === 'mac-x64' ? 0x1000007 : 0x100000c, 4); }
    core.write(key, 200);
    files[key] = { [target.core]: core, LICENSE: Buffer.from('inert license'), 'TLS-DEPENDENCY-NOTICES.txt': Buffer.from('inert notices') };
    if (target.sidecar) files[key][target.sidecar] = cronetFixture(key);
    const payload = name => ({ member: name, bytes: files[key][name].length, sha256: hash(files[key][name]) });
    const archiveBytes = encode({ synthetic: 'not a real archive', key });
    const archive = { ...pin.archive, bytes: archiveBytes.length, sha256: hash(archiveBytes),
      members: Object.fromEntries(Object.entries(files[key]).map(([name, bytes]) => [pin.archive.root + '/' + name, hash(bytes)])) };
    const tags = expectedTags(key).join(','), goVersion = 'go' + b.sourcePolicy.goVersion;
    const buildGraph = {
      settings: { GOOS: target.os, GOARCH: target.arch, CGO_ENABLED: target.cgo,
        '-trimpath': 'true', '-tags': tags, 'vcs.revision': b.sourceCommit, 'vcs.modified': 'false' },
      normalModules: [{ module: 'example.invalid/inert', version: 'v1.0.0', h1: 'h1:YQ=' }],
      prebuiltAcquisitionModules: [],
      localReplacements: s.sourceIdentity.replacements.map(p => ({ module: p.module,
        version: p.module === b.sourcePolicy.metacubex.module ? b.sourcePolicy.metacubex.version : 'v0.0.0', h1: '',
        replacement: { module: p.path, version: '(devel)', h1: '' } })),
    };
    const rawBuildInfo = 'inert: ' + goVersion + '\n\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n\tmod\tgithub.com/sagernet/sing-box\t(devel)\n'
      + buildGraph.normalModules.map(m => '\tdep\t' + m.module + '\t' + m.version + '\t' + m.h1 + '\n').join('')
      + buildGraph.localReplacements.map(m => '\tdep\t' + m.module + '\t' + m.version + '\t\n\t=>\t' + m.replacement.module + '\t(devel)\t\n').join('')
      + Object.entries(buildGraph.settings).map(([k, v]) => '\tbuild\t' + k + '=' + v + '\n').join('');
    rows[key] = { schema: 'polaris-upstream-native-receipt-v1', id: pin.id, target: target.os + '/' + target.arch,
      variant: { linkage: target.sidecar ? 'shared-purego' : 'static-cgo', libc: '', goarm: '', gomips: '', go386: '', legacy: false },
      version: b.version, tag: b.originalIndexTag, sourceCommit: b.sourceCommit, sourceTree: b.sourceTree,
      goVersion, buildID: '', rawBuildInfo, tags, sourceIdentity: structuredClone(s.sourceIdentity), buildGraph,
      archive, core: payload(target.core), sidecar: target.sidecar ? payload(target.sidecar) : null,
      signatureObservation: key.startsWith('mac-') ? { state: 'signed', cdHash: '4'.repeat(40), trustValidated: false } : null,
      finalPackageCronetSmoke: { status: 'passed', os: target.os, arch: target.arch, go: goVersion,
        gvisorCompiled: true, naiveOutbound: true, linkage: target.sidecar ? 'shared-purego' : 'static-cgo',
        version: '150.0.0.1', coreSha256: hash(core), librarySha256: target.sidecar ? hash(files[key][target.sidecar]) : null } };
    b.platforms[key] = Object.fromEntries(['id', 'archive', 'core', 'sidecar'].map(name => [name, structuredClone(rows[key][name])]));
    repin(assetPin(archive.releaseName), archiveBytes);
  }
  if (options.modernWindows) {
    const row = rows.win;
    row.trustedExecution = { schema: 'polaris-trusted-execution-v1', sourceCommit: b.sourceCommit, sourceTree: b.sourceTree,
      loader: 'windows-helper-private-cohort-v1', purposes: ['check-config-v1', 'run-main-v1'] };
    row.finalPackageCronetSmoke.windowsExecution = { sourceCommit: b.sourceCommit, sourceTree: b.sourceTree,
      version: b.version, target: row.target, runId: b.producer.runId, runAttempt: b.producer.runAttempt,
      archiveSha256: row.archive.sha256, core: structuredClone(row.core), sidecar: structuredClone(row.sidecar),
      tlsProbe: { synthetic: 'not a TLS receipt' }, cronet: { synthetic: 'not a native observation' },
      schema: 'polaris-windows-native-execution-proof-v1', native: { synthetic: 'not a SYSTEM execution' } };
    b.producer.nativeJobs.win.name = 'Explicit SYSTEM final-package Windows native execution proof';
  }
  const index = { schema: 'polaris-upstream-native-staging-v1', repository: b.repository, repositoryId: b.repositoryId,
    version: b.version, tag: b.originalIndexTag, tagObject: null, sourceCommit: b.sourceCommit, sourceTree: b.sourceTree,
    producer: structuredClone(b.producer), sourceArchive: structuredClone(s.sourceArchive), platforms: structuredClone(Object.values(rows)) };
  // Keep the real committed 44-row census. Unselected rows are synthetic metadata
  // variants; none becomes selected or gains native execution authority.
  while (index.platforms.length < s.nativeRows) {
    const row = structuredClone(rows.linux); row.id += '-inert-unselected-' + index.platforms.length; index.platforms.push(row);
  }
  const renameAsset = (name, replacement) => {
    const pin = assetPin(name), bytes = blobs.get(pin.url); assert(bytes);
    pin.name = replacement;
    pin.url = 'https://github.com/' + b.repository + '/releases/download/' + b.tag + '/' + replacement;
    blobs.set(pin.url, bytes);
  };
  options.mutate?.({ binding: b, source: s, index, renameAsset });
  const indexBytes = options.rawIndexBytes ?? encode(index);
  Object.assign(s.index, { bytes: indexBytes.length, sha256: hash(indexBytes) });
  repin(assetPin(s.index.name), indexBytes);
  for (const pin of b.assets) if (!blobs.has(pin.url)) repin(pin, Buffer.from('inert ' + pin.name));
  const bindingBytes = encode(b);
  writeFileSync(join(directory, 'scripts/desktop-core/native-dns-candidate.json'), bindingBytes);
  writeFileSync(join(directory, 'scripts/desktop-core/native-dns-source-binding.json'), encode(s));
  const selection = { ...structuredClone(currentManifest.historicalDesktopTemporary),
    contractSha256: hash(bindingBytes), producer: structuredClone(b.producer),
    librarySha256: Object.fromEntries(['linux', 'win'].map(key => [key, b.platforms[key].sidecar.sha256])) };
  const manifest = { desktopTemporary: selection };
  writeFileSync(join(directory, 'src-tauri/core-manifest.json'), encode(manifest));
  const api = 'https://api.github.com/repos/' + b.repository;
  const paths = { release: '/releases/' + b.releaseId, ref: '/git/ref/tags/' + b.tag, tag: '/git/tags/' + b.tagObject,
    commit: '/git/commits/' + b.sourceCommit, tree: '/git/trees/' + b.sourceTree + '?recursive=1',
    run: '/actions/runs/' + b.producer.runId + '/attempts/' + b.producer.runAttempt,
    workflow: '/actions/workflows/10', win: '/actions/jobs/' + b.producer.nativeJobs.win.id };
  const responses = new Map([
    [paths.release, { id: b.releaseId, tag_name: b.tag, draft: false, prerelease: true, published_at: 'inert publication',
      assets: b.assets.map(p => ({ name: p.name, id: p.assetId, size: p.bytes, digest: 'sha256:' + p.sha256, browser_download_url: p.url })) }],
    [paths.ref, { ref: 'refs/tags/' + b.tag, object: { type: 'tag', sha: b.tagObject } }],
    [paths.tag, { sha: b.tagObject, tag: b.tag, object: { type: 'commit', sha: b.sourceCommit } }],
    [paths.commit, { sha: b.sourceCommit, tree: { sha: b.sourceTree } }],
    [paths.tree, { sha: b.sourceTree, truncated: false, tree: s.sourceGitlinks.map(p => ({ ...p, type: 'commit', mode: '160000' })) }],
    [paths.run, { id: b.producer.runId, run_attempt: b.producer.runAttempt, head_sha: b.sourceCommit, status: 'completed',
      conclusion: 'success', path: b.producer.workflow, workflow_id: 10, event: 'workflow_dispatch',
      repository: { id: b.repositoryId, full_name: b.repository }, head_repository: { id: b.repositoryId, full_name: b.repository } }],
    [paths.workflow, { id: 10, path: b.producer.workflow }],
    ['/actions/workflows/11', { id: 11, path: b.producer.workflow }],
  ]);
  for (const job of Object.values(b.producer.nativeJobs)) responses.set('/actions/jobs/' + job.id,
    { id: job.id, name: job.name, run_id: b.producer.runId, run_attempt: b.producer.runAttempt,
      head_sha: b.sourceCommit, status: 'completed', conclusion: 'success' });
  let phase = 0;
  const read = async (url, max, redirects) => {
    const suffix = url.startsWith(api) ? url.slice(api.length) : null;
    if (suffix === paths.release) phase++;
    const count = (counts.get(url) ?? 0) + 1; counts.set(url, count);
    events.push({ url, suffix, max, redirects, phase, count });
    if (suffix !== null) {
      assert.equal(max, 16 * 1024 * 1024); assert.notEqual(redirects, true);
      assert(responses.has(suffix), 'unexpected actual acquisition API request: ' + suffix);
      const value = structuredClone(responses.get(suffix));
      controls.api?.(suffix, value, phase, count);
      return encode(value);
    }
    assert(blobs.has(url), 'unexpected actual acquisition asset: ' + url);
    assert.equal(redirects, true);
    const pin = b.assets.find(p => p.url === url); assert.equal(max, pin.bytes);
    return controls.blob ? controls.blob(pin, Buffer.from(blobs.get(url))) : Buffer.from(blobs.get(url));
  };
  const run = (command, args, settings) => {
    if (command === 'git') {
      assert.equal(settings.cwd, directory);
      if (args[0] === 'rev-parse') return args[1] === 'HEAD^{tree}' ? 'c'.repeat(40) : sha;
      if (args[0] === 'status') return '';
      if (args[0] === 'show') return readFileSync(join(directory, args[1].slice('HEAD:'.length)), 'utf8');
      assert.fail('unexpected Git metadata request');
    }
    if (args[1]?.endsWith('release-archive.py')) {
      assert(['python', 'python3'].includes(command));
      const request = JSON.parse(settings.input);
      if (args.at(-1) === '--source-trusted-proofs') {
        assert.equal(settings.timeout, 120_000); assert.equal(settings.killSignal, 'SIGKILL');
        assert.deepEqual(readFileSync(args[2]), sourceBytes);
        assert.equal(request.trustedSourceSelection, 'candidate');
        assert.deepEqual(request.trustedRows, keys.map(key => index.platforms.find(r => r.id === b.platforms[key].id)));
        assert.deepEqual(request.trustedProducer, b.producer);
        assert.equal(request.sourceCommit, b.sourceCommit); assert.equal(request.sourceTree, b.sourceTree);
        assert.deepEqual(request.sourceIdentity, s.sourceIdentity); assert.deepEqual(request.sourceGitlinks, s.sourceGitlinks);
        assert.deepEqual(request.sourcePolicy, b.sourcePolicy);
        sourceChecks.push(request);
        if (controls.sourceError) throw controls.sourceError;
        return '{}';
      }
      const key = keys.find(k => b.platforms[k].archive.root === request.root); assert(key);
      const row = index.platforms.find(r => r.id === b.platforms[key].id);
      assert.deepEqual(request.archive, row.archive); assert.equal(request.target, row.target);
      const transport = Object.hasOwn(row.archive, 'releaseName') ? row.archive.releaseName : row.archive.name;
      assert.deepEqual(readFileSync(args[2]), blobs.get(assetPin(transport).url));
      extraction.push(key);
      const destination = join(args[3], request.root); mkdirSync(destination, { recursive: true });
      for (const [name, bytes] of Object.entries(files[key])) {
        writeFileSync(join(destination, name), bytes, { mode: name === row.core.member && key !== 'win' ? 0o755 : 0o644 });
      }
      if (controls.corruptPayload === key) writeFileSync(join(destination, row.core.member), 'changed inert payload');
      if (controls.corruptIndex && extraction.length === 4)
        writeFileSync(join(dirname(args[2]), s.index.name), Buffer.alloc(indexBytes.length));
      return '';
    }
    if (command === 'go') {
      const bytes = readFileSync(args.at(-1)), key = keys.find(k => hash(bytes) === rows[k].core.sha256); assert(key);
      goChecks.push({ key, args });
      return args[0] === 'version' ? (controls.wrongBuildInfo === key ? rows[key].rawBuildInfo.replace('vcs.modified=false', 'vcs.modified=true') : rows[key].rawBuildInfo) : rows[key].buildID;
    }
    if (command === 'codesign') return { status: 0, stdout: args[0] === '--display' ? 'CDHash=' + '4'.repeat(40) + '\n' : '', stderr: '' };
    assert.fail('no native/tool command permitted in inert acquisition fixture: ' + command);
  };
  const selectedEnv = mode => {
    if (mode === 'explicit') return { ...env };
    const value = { ...env, GITHUB_EVENT_NAME: 'push', GITHUB_REF: 'refs/heads/main' };
    delete value.POLARIS_NATIVE_DNS_CANDIDATE; return value;
  };
  return { directory, binding: b, source: s, index, indexBytes, rows, files, events, controls, paths, sourceChecks, extraction, goChecks,
    acquire: mode => mode === 'explicit'
      ? acquireCandidate(directory, sha, { env: selectedEnv(mode), read, run })
      : consumeTemporaryDesktop(directory, manifest, { env: selectedEnv(mode), read, run }),
    assertEmpty: () => assert.deepEqual(readdirSync(join(directory, 'resources')), []),
    assetEvents: () => events.filter(e => e.suffix === null),
    actionEvents: () => events.filter(e => e.suffix?.startsWith('/actions/')),
  };
}
test('candidate acquisition fixture starts with the actual committed four-key compact pin shape', t => {
  for (const pin of Object.values(binding.platforms)) assert.deepEqual(Object.keys(pin).sort(), ['archive', 'core', 'id', 'sidecar']);
  const f = candidateAcquisitionFixture(t), loaded = loadCandidate(f.directory);
  // A present undefined alias is invalid even though JSON cannot encode it.
  assert.throws(() => producerReleaseName({ ...binding.platforms.win.archive, releaseName: undefined }),
    { message: 'Upstream producer release archive name differs' });
  for (const pin of Object.values(loaded.platforms)) {
    assert.deepEqual(Object.keys(pin).sort(), ['archive', 'core', 'id', 'sidecar']);
    assert.equal(Object.hasOwn(pin, 'finalPackageCronetSmoke'), false);
  }
});
for (const mode of ['explicit', 'temporary-default']) for (const modernWindows of [false, true]) for (const legacyNames of [false, true])
  test('actual compact ' + mode + ' acquisition closes verified full-index and both snapshots; Windows header-only owner=' + modernWindows
    + (legacyNames ? '; producer alias absent' : ''), async t => {
    const f = candidateAcquisitionFixture(t, { modernWindows, mutate: legacyNames ? x => {
      for (const key of Object.keys(RELEASE_TARGETS)) {
        const pin = x.binding.platforms[key], row = x.index.platforms.find(r => r.id === pin.id);
        const previous = pin.archive.releaseName;
        delete pin.archive.releaseName; delete row.archive.releaseName;
        x.renameAsset(previous, pin.archive.name);
      }
    } : undefined }), receipt = await f.acquire(mode);
    assert.equal(receipt.context.mode, mode === 'temporary-default' ? mode : undefined);
    assert.equal(receipt.runtimeValidated, false); assert.equal(receipt.AppAccepted, false); assert.equal(receipt.publicationEligible, false);
    assert.equal(receipt.publicReadback.producerReadback.jobs.win.name, f.binding.producer.nativeJobs.win.name);
    assert.deepEqual(receipt.publicReadback.sourceGitlinks, f.source.sourceGitlinks);
    assert.equal(f.sourceChecks.length, 1); assert.equal(f.extraction.length, 4); assert.equal(f.goChecks.length, 8);
    assert.equal(f.assetEvents().length, 8);
    for (const pin of f.binding.assets) assert.equal(f.assetEvents().filter(e => e.url === pin.url).length, 1);
    const firstAsset = f.events.findIndex(e => e.suffix === null), firstActions = f.events.findIndex(e => e.suffix?.startsWith('/actions/'));
    assert(f.events.slice(0, firstAsset).some(e => e.suffix === f.paths.tree));
    assert.equal(f.events[firstAsset].url, f.binding.assets.find(p => p.name === f.source.index.name).url);
    assert(firstAsset < firstActions);
    assert(f.events.slice(firstAsset + 1, firstActions).every(e => e.suffix !== null));
    for (const phase of [1, 2]) {
      assert.equal(f.events.filter(e => e.phase === phase && e.suffix === f.paths.release).length, 1);
      assert.equal(f.events.filter(e => e.phase === phase && e.suffix === f.paths.tree).length, 2);
      assert.equal(f.events.filter(e => e.phase === phase && e.suffix?.startsWith('/actions/jobs/')).length, 4);
    }
    const retained = join(f.directory, 'resources/.native-dns-candidate');
    assert.deepEqual(JSON.parse(readFileSync(join(retained, f.source.index.name))), f.index);
    const retainedRows = validateCandidateIndex(readFileSync(join(retained, f.source.index.name)), f.binding, f.source);
    for (const key of Object.keys(RELEASE_TARGETS)) {
      assert.deepEqual(retainedRows[key], f.index.platforms.find(row => row.id === f.binding.platforms[key].id));
      assert.deepEqual(readFileSync(join(f.directory, 'resources', key, f.rows[key].core.member)), f.files[key][f.rows[key].core.member]);
      if (mode === 'temporary-default') assert.deepEqual(readFileSync(join(retained, 'payloads', key, f.rows[key].core.member)), f.files[key][f.rows[key].core.member]);
    }
    assert.equal(existsSyncForTest(join(f.directory, 'resources/.native-dns-candidate.lock')), false);
    assert.equal(readdirSync(join(f.directory, 'resources')).some(name => name.startsWith('.native-dns-acquire-')), false);
  });
test('candidate producer transport rejects explicit invalid aliases and incoherent native recipes without fallback', async t => {
  const aliasError = 'Upstream producer release archive name differs', recipeError = 'Upstream native archive recipe/layout differs';
  const mutations = [
    ...[null, false, 0, [], {}].map(value => ['alias type ' + JSON.stringify(value), a => { a.releaseName = value; }, aliasError]),
    ['empty alias', a => { a.releaseName = ''; }, aliasError],
    ['formal public alias', a => { a.releaseName = 'sing-box-polaris-1.15.0-alpha.12-windows-amd64.zip'; }, aliasError],
    ['explicit native-name fallback', a => { a.releaseName = a.name; }, aliasError],
    ['foreign alias', a => { a.releaseName = 'https://example.invalid/archive.zip'; }, aliasError],
    ['duplicate other-target alias', a => { a.releaseName = binding.platforms.linux.archive.releaseName; }, aliasError],
    ['wrong alias version', a => { a.releaseName = 'sing-box-polaris-1.15.1-polaris-alpha.12-windows-amd64.zip'; }, aliasError],
    ['wrong alias prerelease', a => { a.releaseName = 'sing-box-polaris-1.15.0-polaris-alpha.12.7-windows-amd64.zip'; }, aliasError],
    ['wrong alias channel', a => { a.releaseName = 'sing-box-polaris-1.15.0-polaris-beta.12-windows-amd64.zip'; }, aliasError],
    ['wrong alias target', a => { a.releaseName = 'sing-box-polaris-1.15.0-polaris-alpha.12-windows-arm64.zip'; }, aliasError],
    ['wrong alias extension', a => { a.releaseName = 'sing-box-polaris-1.15.0-polaris-alpha.12-windows-amd64.tar.gz'; }, aliasError],
    ['wrong alias prefix', a => { a.releaseName = 'other-1.15.0-polaris-alpha.12-windows-amd64.zip'; }, aliasError],
    ['trailing newline alias', a => { a.releaseName += '\n'; }, aliasError],
    ['unsupported publicName', a => { a.publicName = a.releaseName; }, 'Unsupported upstream public-name alias'],
    ['missing native name', a => { delete a.name; }, 'Upstream file pin differs'],
    ['native name type', a => { a.name = null; }, 'Upstream file pin differs'],
    ['native path', a => { a.name = '../' + a.name; }, 'Upstream file pin differs'],
    ['coherent foreign native version and alias', a => {
      a.name = 'sing-box-1.15.1-polaris-alpha.12-windows-amd64.zip';
      a.releaseName = 'sing-box-polaris-1.15.1-polaris-alpha.12-windows-amd64.zip';
    }, recipeError],
    ['coherent foreign native target and alias', a => {
      a.name = 'sing-box-1.15.0-polaris-alpha.12-linux-amd64.tar.gz';
      a.releaseName = 'sing-box-polaris-1.15.0-polaris-alpha.12-linux-amd64.tar.gz';
    }, recipeError],
    ['coherent foreign native extension and alias', a => {
      a.name = 'sing-box-1.15.0-polaris-alpha.12-windows-amd64.tar.gz';
      a.releaseName = 'sing-box-polaris-1.15.0-polaris-alpha.12-windows-amd64.tar.gz';
    }, recipeError],
    ['native prefix', a => { a.name = 'other-' + a.name; }, recipeError],
    ['legacy name cannot bypass the selected version', a => {
      delete a.releaseName; a.name = 'sing-box-1.15.0-alpha.12-windows-amd64.zip';
    }, recipeError],
  ];
  for (const [name, mutate, message] of mutations) await t.test(name, async t => {
    const f = candidateAcquisitionFixture(t, { mutate: x => {
      const pin = x.binding.platforms.win, row = x.index.platforms.find(r => r.id === pin.id);
      mutate(pin.archive); row.archive = structuredClone(pin.archive);
    } });
    await assert.rejects(f.acquire('explicit'), { message });
    assert.equal(f.assetEvents().length, 1); assert.equal(f.actionEvents().length, 0);
    assert.equal(f.sourceChecks.length, 0); assert.equal(f.extraction.length, 0); f.assertEmpty();
  });
});
test('candidate producer delivery rejects missing duplicate foreign or mistyped public asset names at their real boundary', async t => {
  const mutations = [
    ['missing', x => {
      x.binding.assets.splice(x.binding.assets.findIndex(p => p.name === x.binding.platforms.win.archive.releaseName), 1);
    }, 'Exact eight temporary assets required', 0],
    ['duplicate', x => {
      const first = x.binding.assets.find(p => p.name === x.binding.platforms.win.archive.releaseName);
      const other = x.binding.assets.find(p => p.name === x.binding.platforms['mac-x64'].archive.releaseName);
      other.name = first.name; other.url = first.url;
    }, 'Exact eight temporary assets required', 0],
    ['foreign', x => {
      x.renameAsset(x.binding.platforms.win.archive.releaseName, 'foreign-windows-amd64.zip');
    }, 'Unique candidate asset required', 1],
    ['type', x => {
      x.binding.assets.find(p => p.name === x.binding.platforms.win.archive.releaseName).name = null;
    }, 'Candidate public asset pin differs', 0],
  ];
  for (const [name, mutate, message, assets] of mutations) await t.test(name, async t => {
    const f = candidateAcquisitionFixture(t, { mutate });
    await assert.rejects(f.acquire('explicit'), { message });
    assert.equal(f.assetEvents().length, assets); assert.equal(f.actionEvents().length, 0);
    assert.equal(f.sourceChecks.length, 0); assert.equal(f.extraction.length, 0); f.assertEmpty();
  });
});
test('compact acquisition rejects full-index/source/pin/producer mismatches before native Actions or other assets', async t => {
  const selected = i => i.platforms.find(r => r.target === 'windows/amd64');
  const mutations = [
    ['schema', x => { x.index.schema = 'polaris-upstream-native-assets-v1'; }],
    ['original tag', x => { x.index.tag += '-foreign'; }],
    ['tag object', x => { x.index.tagObject = 'f'.repeat(40); }],
    ...['sourceCommit', 'sourceTree', 'version'].map(key => [key, x => { x.index[key] += '-foreign'; }]),
    ['row census', x => { x.index.platforms.pop(); }],
    ['non-array row census', x => { x.index.platforms = 44; }],
    ['duplicate ID', x => { x.index.platforms.at(-1).id = x.index.platforms[0].id; }],
    ['selected ID absent', x => { selected(x.index).id += '-missing'; }],
    ['unselected source identity', x => { x.index.platforms.at(-1).sourceIdentity.goModSha256 = 'f'.repeat(64); }],
    ['unselected source revision', x => { x.index.platforms.at(-1).buildGraph.settings['vcs.revision'] = 'f'.repeat(40); }],
    ['unselected dirty revision', x => { x.index.platforms.at(-1).buildGraph.settings['vcs.modified'] = 'true'; }],
    ['unselected source tree', x => { x.index.platforms.at(-1).sourceTree = 'f'.repeat(40); }],
    ['selected row schema', x => { selected(x.index).schema = 'unknown-native-receipt-v1'; }],
    ['selected version', x => { selected(x.index).version += '.1'; }],
    ['selected target', x => { selected(x.index).target = 'linux/amd64'; }],
    ['selected variant', x => { selected(x.index).variant.legacy = true; }],
    ['selected capability tags', x => { selected(x.index).tags = 'with_gvisor,with_purego'; }],
    ['selected smoke', x => { selected(x.index).finalPackageCronetSmoke.status = 'skipped'; }],
    ['compact Core pin', x => { x.binding.platforms.win.core.bytes++; }],
    ['compact companion pin', x => { x.binding.platforms.win.sidecar.sha256 = 'f'.repeat(64); }],
    ['original archive pin', x => { selected(x.index).archive.members[selected(x.index).archive.root + '/LICENSE'] = 'f'.repeat(64); }],
    ...['workflowCommit', 'runId', 'runAttempt'].map(key => [key, x => { x.index.producer[key] = key === 'workflowCommit' ? 'f'.repeat(40) : x.index.producer[key] + 1; }]),
    ['source archive identity', x => { x.index.sourceArchive.sha256 = 'f'.repeat(64); }],
    ['archive delivery pin', x => { x.binding.assets.find(p => p.name === x.binding.platforms.win.archive.releaseName).bytes++; }],
  ];
  const messages = {
    schema: 'Original staging index identity changed',
    'original tag': 'Original staging index identity changed',
    'tag object': 'Original staging index identity changed',
    sourceCommit: 'Candidate index identity differs: sourceCommit',
    sourceTree: 'Candidate index identity differs: sourceTree',
    version: 'Candidate index identity differs: version',
    'row census': 'Complete unique staging index required',
    'non-array row census': 'Complete unique staging index required',
    'duplicate ID': 'Complete unique staging index required',
    'selected ID absent': 'Selected candidate row absent',
    'unselected source identity': 'Staging row source identity differs',
    'unselected source revision': 'Staging source revision differs',
    'unselected dirty revision': 'Staging source revision differs',
    'unselected source tree': 'Staging source revision differs',
    'selected row schema': 'Upstream target/source differs',
    'selected version': 'Upstream target/source differs',
    'selected target': 'Upstream target/source differs',
    'selected variant': 'Upstream App recipe differs',
    'selected capability tags': 'Upstream required capabilities differ',
    'selected smoke': 'Actual native capability smoke missing or unbound',
    'compact Core pin': 'Candidate native pin differs',
    'compact companion pin': 'Candidate native pin differs',
    'original archive pin': 'Candidate native pin differs',
    workflowCommit: 'Candidate index identity differs: producer',
    runId: 'Candidate index identity differs: producer',
    runAttempt: 'Candidate index identity differs: producer',
    'source archive identity': 'Candidate source archive differs',
    'archive delivery pin': 'Candidate transport/archive bytes differ',
  };
  for (const [name, mutate] of mutations) await t.test(name, async t => {
    const f = candidateAcquisitionFixture(t, { mutate });
    assert.equal(typeof messages[name], 'string', name);
    await assert.rejects(f.acquire('explicit'), { message: messages[name] });
    assert.equal(f.assetEvents().length, 1); assert.equal(f.actionEvents().length, 0);
    assert.equal(f.sourceChecks.length, 0); assert.equal(f.extraction.length, 0); f.assertEmpty();
  });
});
test('compact acquisition feeds genuine selected proof headers to the strict shared owner classifier', async t => {
  const win = x => x.index.platforms.find(r => r.target === 'windows/amd64');
  const owner = (x, name) => { x.binding.producer.nativeJobs.win.name = name; x.index.producer.nativeJobs.win.name = name; };
  const mutations = [
    ['Windows proof with ordinary owner', x => owner(x, binding.producer.nativeJobs.win.name)],
    ['unrelated green owner', x => owner(x, 'Unrelated successful job')],
    ['SYSTEM without either proof or policy', x => { delete win(x).trustedExecution; delete win(x).finalPackageCronetSmoke.windowsExecution; }],
    ['policy only', x => { delete win(x).finalPackageCronetSmoke.windowsExecution; }],
    ['proof only', x => { delete win(x).trustedExecution; }],
    ...[null, true, []].map(value => ['proof shape ' + JSON.stringify(value), x => { win(x).finalPackageCronetSmoke.windowsExecution = value; }]),
    ['unknown proof schema', x => { win(x).finalPackageCronetSmoke.windowsExecution.schema += '-unknown'; }],
    ['mixed proof kinds', x => { win(x).finalPackageCronetSmoke.macosExecution = {}; }],
    ['null policy', x => { win(x).trustedExecution = null; }],
    ['extra policy field', x => { win(x).trustedExecution.extra = true; }],
    ['wrong loader', x => { win(x).trustedExecution.loader = 'macos-root-private-v1'; }],
    ['wrong purposes', x => { win(x).trustedExecution.purposes.reverse(); }],
    ['proof source', x => { win(x).finalPackageCronetSmoke.windowsExecution.sourceCommit = 'f'.repeat(40); }],
    ['proof run', x => { win(x).finalPackageCronetSmoke.windowsExecution.runId++; }],
    ['proof attempt', x => { win(x).finalPackageCronetSmoke.windowsExecution.runAttempt++; }],
    ['cross-target Windows proof', x => { const row = x.index.platforms.find(r => r.target === 'linux/amd64');
      row.finalPackageCronetSmoke.windowsExecution = structuredClone(win(x).finalPackageCronetSmoke.windowsExecution);
      row.trustedExecution = structuredClone(win(x).trustedExecution); }],
    ['duplicate native job ID', x => { const id = x.binding.producer.nativeJobs.linux.id;
      x.binding.producer.nativeJobs.win.id = id; x.index.producer.nativeJobs.win.id = id; }],
  ];
  const messages = {
    'Windows proof with ordinary owner': 'Upstream native job identity differs',
    'unrelated green owner': 'Upstream native job identity differs',
    'SYSTEM without either proof or policy': 'Upstream native job identity differs',
    'policy only': 'Upstream native owner requires one target proof and policy',
    'proof only': 'Upstream native owner requires one target proof and policy',
    'proof shape null': 'Upstream native owner proof/policy shape differs',
    'proof shape true': 'Upstream native owner proof/policy shape differs',
    'proof shape []': 'Upstream native owner proof/policy shape differs',
    'unknown proof schema': 'Upstream native owner proof schema differs',
    'mixed proof kinds': 'Upstream native owner requires one target proof and policy',
    'null policy': 'Upstream native owner proof/policy shape differs',
    'extra policy field': 'Upstream native owner policy fields differ',
    'wrong loader': 'Upstream native owner policy differs',
    'wrong purposes': 'Upstream native owner policy differs',
    'proof source': 'Upstream native owner proof source/target differs',
    'proof run': 'Upstream native owner proof run/archive header differs',
    'proof attempt': 'Upstream native owner proof run/archive header differs',
    'cross-target Windows proof': 'Upstream native owner requires one target proof and policy',
    'duplicate native job ID': 'Duplicate upstream native job',
  };
  for (const [name, mutate] of mutations) await t.test(name, async t => {
    const f = candidateAcquisitionFixture(t, { modernWindows: true, mutate });
    assert.doesNotThrow(() => validateCandidateIndex(f.indexBytes, f.binding, f.source));
    assert.equal(typeof messages[name], 'string', name);
    await assert.rejects(f.acquire('explicit'), { message: messages[name] });
    assert.equal(f.assetEvents().length, 1); assert.equal(f.actionEvents().length, 0);
    assert.equal(f.sourceChecks.length, 0); assert.equal(f.extraction.length, 0); f.assertEmpty();
  });
});
test('compact acquisition rejects malformed public identity in either genuine snapshot', async t => {
  const changes = [
    ['release', r => { r.id++; }], ['release', r => { r.prerelease = false; }],
    ['release', r => { r.draft = true; }], ['release', r => { r.published_at = null; }],
    ['ref', r => { r.object.sha = 'f'.repeat(40); }], ['tag', r => { r.object.sha = 'f'.repeat(40); }],
    ['commit', r => { r.tree.sha = 'f'.repeat(40); }],
    ['release', r => { r.assets[0].id++; }], ['release', r => { r.assets[0].size++; }],
    ['release', r => { r.assets[0].digest = 'sha256:' + 'f'.repeat(64); }],
    ['release', r => { r.assets[0].browser_download_url += '?foreign'; }], ['release', r => { r.assets.pop(); }],
    ['release', r => { r.assets.push(structuredClone(r.assets[0])); }],
    ['tree', r => { r.tree[0].sha = 'f'.repeat(40); }],
  ];
  for (const phase of [1, 2]) for (const [i, [target, change]] of changes.entries()) await t.test('phase ' + phase + ' ' + target + ' ' + i, async t => {
    const f = candidateAcquisitionFixture(t); let reached = 0;
    f.controls.api = (suffix, value, current) => {
      if (current === phase && suffix === f.paths[target]) { reached++; change(value); }
    };
    const message = target === 'tree' ? 'Candidate public gitlinks differ'
      : ['ref', 'tag', 'commit'].includes(target) ? 'Annotated candidate tag/source differs'
      : i < 4 ? 'Temporary published prerelease differs' : 'Temporary public asset inventory differs';
    await assert.rejects(f.acquire('explicit'), { message });
    assert.equal(reached, 1);
    assert.equal(f.assetEvents().length, phase === 1 ? 0 : 8);
    assert.equal(f.sourceChecks.length, phase === 1 ? 0 : 1);
    assert.equal(f.extraction.length, phase === 1 ? 0 : 4);
    if (phase === 1) assert.equal(f.actionEvents().length, 0);
    f.assertEmpty();
  });
});
test('compact acquisition preserves native run/workflow/job/API rejection on both snapshots', async t => {
  const changes = [
    ['win', j => { j.id++; }], ['win', j => { j.name = binding.producer.nativeJobs.win.name; }],
    ['win', j => { j.head_sha = 'f'.repeat(40); }], ['win', j => { j.run_id++; }],
    ['win', j => { j.run_attempt++; }], ['win', j => { j.status = 'in_progress'; }],
    ['win', j => { j.conclusion = 'failure'; }], ['run', r => { r.head_sha = 'f'.repeat(40); }],
    ['run', r => { r.run_attempt++; }], ['run', r => { r.status = 'queued'; }],
    ['run', r => { r.conclusion = null; }], ['run', r => { r.repository.id++; }],
    ['run', r => { r.head_repository.full_name = 'foreign/repository'; }],
    ['workflow', w => { w.path = '.github/workflows/foreign.yml'; }],
  ];
  for (const phase of [1, 2]) for (const [i, [target, change]] of changes.entries()) await t.test('phase ' + phase + ' ' + target + ' ' + i, async t => {
    const f = candidateAcquisitionFixture(t, { modernWindows: true }); let reached = 0;
    f.controls.api = (suffix, value, current) => {
      if (current === phase && suffix === f.paths[target]) { reached++; change(value); }
    };
    const message = target === 'win' ? 'Published upstream native job differs'
      : target === 'run' ? 'Published upstream producer run differs' : 'Published upstream workflow differs';
    await assert.rejects(f.acquire('explicit'), { message });
    assert.equal(reached, 1);
    assert.equal(f.assetEvents().length, phase === 1 ? 1 : 8);
    assert.equal(f.sourceChecks.length, phase === 1 ? 0 : 1);
    assert.equal(f.extraction.length, phase === 1 ? 0 : 4); f.assertEmpty();
  });
});
test('early source observations cannot be replaced by the index-enriched before snapshot', async t => {
  const f = candidateAcquisitionFixture(t); let reached = 0;
  f.controls.api = (suffix, value, phase, count) => {
    if (suffix === f.paths.tree && phase === 1 && count === 2) { reached++; value.tree[0].sha = 'f'.repeat(40); }
  };
  await assert.rejects(f.acquire('explicit'), /source identity changed during producer readback/);
  assert.equal(reached, 1);
  assert.equal(f.assetEvents().length, 1); assert.equal(f.sourceChecks.length, 0); f.assertEmpty();
});
test('after-only individually valid producer workflow ID drift reaches full snapshot comparison', async t => {
  const f = candidateAcquisitionFixture(t); let reached = 0;
  f.controls.api = (suffix, value, phase) => {
    if (suffix === f.paths.run && phase === 2) { reached++; value.workflow_id = 11; }
  };
  await assert.rejects(f.acquire('temporary-default'), /public identity changed during acquisition/);
  assert.equal(reached, 1);
  assert(f.events.some(e => e.phase === 2 && e.suffix === '/actions/workflows/11'));
  assert.equal(f.sourceChecks.length, 1); assert.equal(f.extraction.length, 4); f.assertEmpty();
});
test('raw index bytes and size reject before owner reads and are rechecked before the after snapshot', async t => {
  for (const corruption of ['hash', 'size', 'json', 'late']) await t.test(corruption, async t => {
    const f = candidateAcquisitionFixture(t, corruption === 'json' ? { rawIndexBytes: Buffer.from('{') } : {});
    if (corruption === 'late') f.controls.corruptIndex = true;
    else if (corruption !== 'json') f.controls.blob = (pin, bytes) => pin.name !== f.source.index.name ? bytes
      : corruption === 'size' ? Buffer.concat([bytes, Buffer.from('x')]) : Buffer.alloc(bytes.length);
    await assert.rejects(f.acquire('explicit'), error => corruption === 'json' ? error instanceof SyntaxError
      : /candidate asset differs|Candidate bytes differ/.test(error.message));
    assert.equal(f.assetEvents().filter(e => e.url.endsWith('/' + f.source.index.name)).length, 1);
    assert.equal(f.assetEvents().length, corruption === 'late' ? 8 : 1);
    assert.equal(f.sourceChecks.length, corruption === 'late' ? 1 : 0);
    assert.equal(f.extraction.length, corruption === 'late' ? 4 : 0);
    assert.equal(f.goChecks.length, corruption === 'late' ? 8 : 0);
    if (corruption !== 'late') assert.equal(f.actionEvents().length, 0);
    assert.equal(f.events.filter(e => e.suffix === f.paths.release).length, 1); f.assertEmpty();
  });
});
for (const mode of ['explicit', 'temporary-default']) test('actual ' + mode + ' acquisition propagates full source-verifier rejection without accepting header-only proof', async t => {
  const f = candidateAcquisitionFixture(t, { modernWindows: true }), failure = Error('inert injected full source-owned validator rejection');
  f.controls.sourceError = failure;
  await assert.rejects(f.acquire(mode), error => error === failure);
  assert.equal(f.assetEvents().length, 8); assert.equal(f.sourceChecks.length, 1);
  assert.equal(f.extraction.length, 0); f.assertEmpty();
});
test('actual acquisition retains member bytes and Go graph inspection after owner closure', async t => {
  const keys = Object.keys(RELEASE_TARGETS);
  for (const failure of ['corruptPayload', 'wrongBuildInfo']) for (const key of keys)
    await t.test(failure + (key === 'linux' ? '' : ' ' + key), async t => {
      const f = candidateAcquisitionFixture(t); f.controls[failure] = key;
      await assert.rejects(f.acquire('explicit'), {
        message: failure === 'corruptPayload' ? 'Candidate member bytes differ' : 'Actual upstream raw BuildInfo differs',
      });
      assert.deepEqual(f.extraction, keys.slice(0, keys.indexOf(key) + 1));
      assert.equal(f.goChecks.length, 2 * keys.indexOf(key) + (failure === 'wrongBuildInfo' ? 1 : 0));
      assert.equal(f.assetEvents().length, 8); assert.equal(f.sourceChecks.length, 1);
      assert.equal(f.events.filter(e => e.suffix === f.paths.release).length, 1); f.assertEmpty();
    });
});
test('actual acquisition propagates receiver errors before and after payloads without receipt or partial install', async t => {
  for (const phase of [1, 2]) await t.test('phase ' + phase, async t => {
    const f = candidateAcquisitionFixture(t), failure = Error('inert API receiver refusal'); let reached = 0;
    f.controls.api = (suffix, _value, current) => {
      if (current === phase && suffix === f.paths.run) { reached++; throw failure; }
    };
    await assert.rejects(f.acquire('temporary-default'), error => error === failure);
    assert.equal(reached, 1);
    assert.equal(f.assetEvents().length, phase === 1 ? 1 : 8);
    assert.equal(f.sourceChecks.length, phase === 1 ? 0 : 1);
    assert.equal(f.extraction.length, phase === 1 ? 0 : 4); f.assertEmpty();
  });
});
