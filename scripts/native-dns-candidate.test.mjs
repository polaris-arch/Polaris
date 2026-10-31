import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { mkdtempSync, readFileSync, mkdirSync, writeFileSync, rmSync, symlinkSync, copyFileSync, linkSync, lstatSync, renameSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { after, test } from 'node:test';
import { candidateContext, candidateFetchSelection, candidateVersionOutput, candidateInstallationContract, candidateAppDirectory, candidateBuildSelection, verifyCandidateAppCarrier, freezeCandidateBuildOutput, CANDIDATE_REF, loadCandidate, validateCandidateIndex, validateCandidateReadback, verifyCandidateMembers } from './desktop-core/native-dns-candidate.mjs';
import { verifyManagedMeshCore, managedCheckPlatform, managedCheckSelection } from './desktop-core/managed-mesh-core.mjs';
import { validateReleaseContract } from './desktop-core/release-consumer.mjs';
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
  const contract = candidateInstallationContract(binding, hash);
  const formal = JSON.parse(readFileSync(join(root, 'scripts/desktop-core/release-contract.json')));
  for (const key of Object.keys(binding.platforms)) {
    assert.equal(contract.platforms[key].providerFingerprint, hash);
    assert.notEqual(hash, formal.platforms[key].providerFingerprint);
    assert.deepEqual(contract.platforms[key].core, binding.platforms[key].core);
    assert.equal(candidateAppDirectory(root, key), join(root, 'resources/.native-dns-app', key));
  }
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
  assert.equal(defaultManifest.desktopRelease, undefined);
  assert.equal(defaultManifest.desktopTemporary.sourceCommit, 'fe182431495711c09bdb3b074ed6aa6309c00cf8');
  assert.deepEqual(defaultManifest.desktopTemporary, currentManifest.historicalDesktopTemporary);
  assert.equal(temporary.hasTemporaryDesktop(currentManifest), false);
  assert.throws(() => temporary.loadTemporaryDesktopSelection(root), /exclusive desktop default/);
  assert.equal(defaultManifest.historicalDesktopRelease.sourceCommit, '577d4b9a089b71d2f0a4ca334b87264b55c10478');
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
  const caller = readFileSync(join(root, 'crates/config-engine/src/builder/managed_mesh_emission/tests/mod.rs'), 'utf8');
  assert.match(caller, /manifest\s*\.get\("desktopTemporary"\)/);
  assert.match(caller, /get\("desktopRelease"\)/);
  assert.match(caller, /verified\["sourceCommit"\], selection\["sourceCommit"\]/);
  assert.match(caller, /verified\["sourceTree"\], selection\["sourceTree"\]/);
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

test('closed descriptor check preserves selected-core refusal, descriptor closure, completion and reap boundaries', () => {
  const file = readFileSync(join(root, 'src-tauri/src/runtime/proxy/mesh_apply/candidate/check/tests/mod.rs'), 'utf8');
  const body = file.slice(file.indexOf('async fn closed_standard_descriptors'), file.indexOf('fn sealed_fixture_binary'));
  function check(text) {
    for (const value of ['child_marker.is_none()', 'struct RestoreStandard', 'impl Drop for RestoreStandard',
      'nix::unistd::close(fd)', 'fs::symlink_metadata(format!("/proc/self/fd/{fd}"))',
      'linux::sealed_binary(&binary_path, profile)', 'linux::sealed_config(',
      'linux::run_check(binary, config, Duration::from_secs(8))', 'saved.restore().unwrap()',
      'ReapedChild(', 'Duration::from_secs(20)', 'child.0.kill()', 'child.0.wait()',
      '!timed_out && status.success()', 'fs::read(marker).unwrap(), b"refused with 0/1/2 closed"', 'Err(CandidateCheckError::Unsupported)', 'old file pin refused before factory'])
      assert.ok(text.includes(value), value);
    assert.doesNotMatch(text, /exec 0<&-|Command::new\("sh"\)/);
    assert.ok(text.indexOf('status.success()') < text.indexOf('fs::read(marker)'));
  }
  check(body);
  for (const value of ['nix::unistd::close(fd)', 'linux::run_check(binary, config, Duration::from_secs(8))',
    'saved.restore().unwrap()', 'ReapedChild(', '!timed_out && status.success()', 'child_marker.is_none()', 'Err(CandidateCheckError::Unsupported)', 'old file pin refused before factory'])
    assert.throws(() => check(body.replace(value, 'REMOVED')));
});
