// One temporary delivery, shared by explicit validation and a committed desktop
// default selection. Its original producer evidence is never a formal/mobile release.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { closeSync, constants, copyFileSync, existsSync, fstatSync, lstatSync, openSync, readSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, digest, expectedTags, reviewedSourcePolicy, requireGraph as demand } from './source-graph.mjs';
import { readUpstreamPublicEvidence, upstreamReleaseName, validateNativeArchiveRow } from './upstream-release.mjs';
import { regularPath, releasePublicReader, RELEASE_TARGETS, verifyReleaseCore } from './release-consumer.mjs';
import { inspectCronetBytes } from '../lib/cronet-payload.mjs';
import { actualCoreFile } from '../lib/core-identity.mjs';

export const CANDIDATE_FILE = 'scripts/desktop-core/native-dns-candidate.json';
export const CANDIDATE_REF = 'refs/heads/collab/native-node-dns-20261018';
export const CANDIDATE_WORKFLOWS = Object.freeze(['native-dns-candidate.yml', 'ci.yml']);
const KEYS = ['linux', 'win', 'mac-x64', 'mac-arm64'], META = 'resources/.native-dns-candidate';
const same = (a, b, reason) => demand(canonical(a) === canonical(b), reason);
const read = (path) => { regularPath(path); return JSON.parse(readFileSync(path)); };
const hex = (v, n) => typeof v === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(v);
const positive = (v) => Number.isSafeInteger(v) && v > 0;
export const hasTemporaryDesktop = (manifest) => Object.hasOwn(manifest, 'desktopTemporary');
export function loadTemporaryDesktopSelection(root, manifest = read(join(root, 'src-tauri/core-manifest.json'))) {
  demand(hasTemporaryDesktop(manifest) && !Object.hasOwn(manifest, 'desktopRelease'), 'One exclusive desktop default selection required');
  const selection = manifest.desktopTemporary, binding = loadCandidate(root);
  const expected = { schema: 'polaris-app-temporary-desktop-selection-v1', contractPath: CANDIDATE_FILE,
    contractSha256: digest(readFileSync(join(root, CANDIDATE_FILE))),
    ...Object.fromEntries(['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map((key) => [key, binding[key]])),
    producer: binding.producer,
    librarySha256: Object.fromEntries(['linux', 'win'].map((key) => [key, binding.platforms[key].sidecar.sha256])) };
  same(selection, expected, 'Committed temporary desktop selection differs');
  return selection;
}
export function selectedCandidateContext(root, env = process.env, run = execFileSync) {
  if (Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE')) return candidateContext(env.CORE_CANDIDATE, env, root, run);
  const selection = loadTemporaryDesktopSelection(root);
  const options = { cwd: root, encoding: 'utf8', env: { ...env } };
  for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete options.env[key];
  const candidate = String(run('git', ['rev-parse', 'HEAD'], options)).trim();
  const tree = String(run('git', ['rev-parse', 'HEAD^{tree}'], options)).trim();
  demand(hex(candidate, 40) && hex(tree, 40), 'Actual App checkout identity required');
  for (const path of ['src-tauri/core-manifest.json', CANDIDATE_FILE, 'scripts/desktop-core/native-dns-source-binding.json'])
    demand(String(run('git', ['show', `HEAD:${path}`], options)) === readFileSync(join(root, path), 'utf8'),
      'Temporary source selection is not the committed checkout');
  demand(String(run('git', ['status', '--porcelain', '--untracked-files=no'], options)).trim() === '',
    'Temporary App checkout has tracked changes');
  demand(!env.CORE_CANDIDATE || env.CORE_CANDIDATE === candidate, 'Default App candidate differs');
  let attempt = null;
  if (env.GITHUB_ACTIONS === 'true') {
    let selectedSHA = env.GITHUB_SHA;
    if (env.GITHUB_EVENT_NAME === 'pull_request' && candidate !== selectedSHA) {
      const event = read(env.GITHUB_EVENT_PATH);
      demand(event.pull_request?.base?.repo?.full_name === 'polaris-arch/Polaris'
        && event.pull_request.head?.sha === candidate, 'Default PR head checkout differs');
      selectedSHA = event.pull_request.head.sha;
    }
    demand(env.GITHUB_REPOSITORY === 'polaris-arch/Polaris' && env.GITHUB_REPOSITORY_ID === '1347028220'
      && selectedSHA === candidate && ['push', 'pull_request', 'merge_group', 'workflow_dispatch', 'workflow_call'].includes(env.GITHUB_EVENT_NAME)
      && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ID ?? '') && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ATTEMPT ?? ''),
    'Default consumer App Actions identity differs');
    attempt = { runId: env.GITHUB_RUN_ID, attempt: env.GITHUB_RUN_ATTEMPT };
  }
  return { mode: 'temporary-default', repository: 'polaris-arch/Polaris', repositoryId: '1347028220',
    candidate, tree, selectionSha256: digest(canonical(selection)), attempt };
}
function selectedOriginalDirectory(root, key, context) {
  return context.mode === 'temporary-default' ? join(root, META, 'payloads', key) : join(root, 'resources', key);
}
function copyOriginal(from, to) {
  regularPath(from, true); mkdirSync(to);
  for (const name of readdirSync(from)) {
    const source = join(from, name), destination = join(to, name);
    if (lstatSync(source).isDirectory()) copyOriginal(source, destination);
    else { regularPath(source); copyFileSync(source, destination, constants.COPYFILE_EXCL); }
  }
}
function checked(path, pin) {
  regularPath(path); const bytes = readFileSync(path);
  demand(bytes.length === pin.bytes && digest(bytes) === pin.sha256, `Candidate bytes differ: ${basename(path)}`);
  return bytes;
}
function asset(binding, name) {
  const selected = binding.assets.filter((p) => p.name === name);
  demand(selected.length === 1, 'Unique candidate asset required'); return selected[0];
}
export function candidateContext(candidate, env = process.env, root = process.cwd(), run = execFileSync) {
  demand(env.POLARIS_NATIVE_DNS_CANDIDATE === '1' && env.POLARIS_NO_KERNEL_RUN !== '1'
    && env.GITHUB_ACTIONS === 'true' && env.GITHUB_EVENT_NAME === 'workflow_dispatch'
    && env.GITHUB_REPOSITORY === 'polaris-arch/Polaris' && env.GITHUB_REPOSITORY_ID === '1347028220'
    && env.GITHUB_REF === CANDIDATE_REF && hex(candidate, 40) && env.GITHUB_SHA === candidate
    && env.CORE_CANDIDATE === candidate
    && CANDIDATE_WORKFLOWS.some((name) => env.GITHUB_WORKFLOW_REF === `${env.GITHUB_REPOSITORY}/.github/workflows/${name}@${CANDIDATE_REF}`)
    && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ID ?? '') && /^[1-9][0-9]*$/.test(env.GITHUB_RUN_ATTEMPT ?? ''),
  'Exact manual native-DNS candidate/run context required');
  const options = { cwd: root, encoding: 'utf8', env: { ...env } };
  for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete options.env[key];
  demand(String(run('git', ['rev-parse', 'HEAD'], options)).trim() === candidate
    && String(run('git', ['status', '--porcelain', '--untracked-files=no'], options)).trim() === '',
  'Candidate checkout differs or tracked source is dirty');
  return { repository: env.GITHUB_REPOSITORY, repositoryId: env.GITHUB_REPOSITORY_ID,
    candidate, runId: env.GITHUB_RUN_ID, attempt: env.GITHUB_RUN_ATTEMPT,
    workflowRef: env.GITHUB_WORKFLOW_REF, ref: env.GITHUB_REF };
}
// Explicit CLI selection only. Environment presence without the matching flag
// must reject rather than silently selecting the historical formal release.
export function candidateFetchSelection(argv, env = process.env) {
  const flags = argv.filter((arg) => arg.startsWith('--native-dns-candidate='));
  if (!flags.length && !Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE')) return null;
  demand(argv.length === 1 && flags.length === 1 && env.POLARIS_NATIVE_DNS_CANDIDATE === '1',
    'Candidate fetch requires one exclusive explicit selector and admitted mode');
  const candidate = flags[0].slice('--native-dns-candidate='.length);
  demand(hex(candidate, 40) && env.CORE_CANDIDATE === candidate, 'Candidate fetch App SHA differs');
  return candidate;
}

// Exact output of the source-pinned cmd/sing-box/cmd_version.go. These expected
// lines derive from independently verified bytes/BuildInfo, not a prior run claim.
export function candidateVersionOutput(row) {
  demand(row?.schema === 'polaris-upstream-native-receipt-v1' && hex(row.sourceCommit, 40)
    && typeof row.version === 'string' && typeof row.goVersion === 'string'
    && typeof row.target === 'string' && typeof row.tags === 'string'
    && ['0', '1'].includes(row.buildGraph?.settings?.CGO_ENABLED), 'Candidate version inputs differ');
  return `sing-box version ${row.version}\n\nEnvironment: ${row.goVersion} ${row.target}\n`
    + `Tags: ${row.tags}\nRevision: ${row.sourceCommit}\nCGO: ${row.buildGraph.settings.CGO_ENABLED === '1' ? 'enabled' : 'disabled'}\n`;
}
export function loadCandidate(root) {
  const binding = read(join(root, CANDIDATE_FILE));
  const source = read(join(root, 'scripts/desktop-core/native-dns-source-binding.json'));
  demand(binding.schema === 'polaris-native-dns-candidate-v1'
    && binding.purpose === 'temporary-four-desktop-runtime-validation'
    && binding.AppAccepted === false && binding.publicationEligible === false
    && binding.releaseId === 416260275
    && binding.tagObject === '127ee419f28a88e207f61f04e011b440176cbf43'
    && binding.tag === `polaris-staging-${source.sourceCommit.slice(0, 12)}-run${source.producer.runId}`
    && binding.originalIndexTag === `v${binding.version}` && binding.originalIndexTagObject === null,
  'Exact temporary delivery identity required');
  for (const key of ['repository', 'repositoryId', 'sourceCommit', 'sourceTree', 'version'])
    same(binding[key], source[key], `Candidate/source binding differs: ${key}`);
  for (const [key, value] of Object.entries(source.producer)) same(binding.producer[key], value, 'Candidate producer differs');
  same(Object.keys(binding.platforms).sort(), KEYS.toSorted(), 'Four desktop candidate pins required');
  demand(binding.assets.length === 8 && new Set(binding.assets.map((p) => p.name)).size === 8
    && new Set(binding.assets.map((p) => p.assetId)).size === 8, 'Exact eight temporary assets required');
  for (const p of binding.assets) demand(/^[A-Za-z0-9][A-Za-z0-9.-]*$/.test(p.name)
    && positive(p.assetId) && positive(p.bytes) && p.bytes <= 128 * 1024 * 1024 && hex(p.sha256, 64)
    && p.url === `https://github.com/${binding.repository}/releases/download/${binding.tag}/${p.name}`,
  'Candidate public asset pin differs');
  for (const pin of [source.index, source.sourceArchive]) {
    const actual = asset(binding, pin.name);
    for (const key of ['bytes', 'sha256']) same(actual[key], pin[key], 'Candidate source/index bytes differ');
  }
  reviewedSourcePolicy(binding);
  return binding;
}
export function validateCandidateIndex(bytes, binding, source) {
  demand(bytes.length === source.index.bytes && digest(bytes) === source.index.sha256, 'Candidate index hash differs');
  const index = JSON.parse(bytes);
  demand(index.schema === 'polaris-upstream-native-staging-v1' && index.tagObject === null
    && index.tag === binding.originalIndexTag, 'Original staging index identity changed');
  for (const key of ['repository', 'repositoryId', 'version', 'sourceCommit', 'sourceTree', 'producer'])
    same(index[key], binding[key], `Candidate index identity differs: ${key}`);
  same(index.sourceArchive, source.sourceArchive, 'Candidate source archive differs');
  demand(Array.isArray(index.platforms) && index.platforms.length === source.nativeRows
    && new Set(index.platforms.map((r) => r.id)).size === source.nativeRows, 'Complete unique staging index required');
  for (const row of index.platforms) {
    same(row.sourceIdentity, source.sourceIdentity, 'Staging row source identity differs');
    demand(row.sourceCommit === binding.sourceCommit && row.sourceTree === binding.sourceTree
      && row.buildGraph.settings['vcs.revision'] === binding.sourceCommit
      && row.buildGraph.settings['vcs.modified'] === 'false', 'Staging source revision differs');
  }
  const rows = {};
  for (const key of KEYS) {
    const pin = binding.platforms[key], row = index.platforms.find((r) => r.id === pin.id);
    demand(row, 'Selected candidate row absent');
    for (const field of ['id', 'archive', 'core', 'sidecar']) same(row[field], pin[field], 'Candidate native pin differs');
    // Original row retains its original version tag and null index tagObject.
    validateNativeArchiveRow({ ...binding, tag: binding.originalIndexTag }, key, row, RELEASE_TARGETS);
    same(row.tags.split(',').sort(), expectedTags(key), 'Candidate feature tag inventory differs');
    const delivery = asset(binding, upstreamReleaseName(row.archive));
    demand(delivery.bytes === row.archive.bytes && delivery.sha256 === row.archive.sha256, 'Candidate transport/archive bytes differ');
    rows[key] = row;
  }
  return rows;
}
export function validateCandidateReadback(binding, source, release, ref, tag, commit, evidence) {
  demand(release.id === binding.releaseId && release.tag_name === binding.tag
    && release.draft === false && release.prerelease === true && release.published_at,
  'Temporary published prerelease differs');
  // target_commitish is a creation reference, not the resolved tag identity.
  demand(ref.ref === `refs/tags/${binding.tag}` && ref.object?.type === 'tag' && ref.object.sha === binding.tagObject
    && tag.sha === binding.tagObject && tag.tag === binding.tag && tag.object?.type === 'commit'
    && tag.object.sha === binding.sourceCommit && commit.sha === binding.sourceCommit
    && commit.tree?.sha === binding.sourceTree, 'Annotated candidate tag/source differs');
  const assets = release.assets.map((a) => ({ name: a.name, assetId: a.id, bytes: a.size,
    sha256: a.digest?.replace(/^sha256:/, ''), url: a.browser_download_url }));
  same(assets.toSorted((a,b) => a.name.localeCompare(b.name)), binding.assets.toSorted((a,b) => a.name.localeCompare(b.name)),
    'Temporary public asset inventory differs');
  same(evidence.sourceGitlinks, source.sourceGitlinks, 'Candidate public gitlinks differ');
  return { releaseId: release.id, tag: binding.tag, tagObject: tag.sha, sourceCommit: commit.sha,
    sourceTree: commit.tree.sha, assets: binding.assets, ...evidence };
}
async function snapshot(binding, source, fetchBytes) {
  const api = `https://api.github.com/repos/${binding.repository}`;
  const request = async (suffix) => JSON.parse(await fetchBytes(api + suffix, 16 * 1024 * 1024));
  const release = await request(`/releases/${binding.releaseId}`);
  const ref = await request(`/git/ref/tags/${binding.tag}`);
  const tag = await request(`/git/tags/${binding.tagObject}`);
  const commit = await request(`/git/commits/${binding.sourceCommit}`);
  const evidence = await readUpstreamPublicEvidence(binding, request);
  return validateCandidateReadback(binding, source, release, ref, tag, commit, evidence);
}
function sourceProof(root, directory, binding, source, rows, run = execFileSync) {
  const path = join(directory, source.sourceArchive.name); checked(path, source.sourceArchive);
  return run(process.platform === 'win32' ? 'python' : 'python3', ['-B', join(root, 'scripts/desktop-core/release-archive.py'), path],
    { input: JSON.stringify({ sourceArchive: source.sourceArchive, sourceCommit: binding.sourceCommit,
      sourceTree: binding.sourceTree, sourceGitlinks: source.sourceGitlinks, sourceIdentity: source.sourceIdentity,
      buildReplacements: Object.values(rows).flatMap((r) => r.buildGraph.localReplacements), sourcePolicy: binding.sourcePolicy }),
    encoding: 'utf8', maxBuffer: 1024 * 1024 });
}
export function verifyCandidateMembers(directory, row) {
  const expected = Object.fromEntries(Object.entries(row.archive.members)
    .map(([name, hash]) => [name.slice(row.archive.root.length + 1), hash]));
  const files = [];
  const visit = (relative = '') => {
    const current = join(directory, relative); regularPath(current, true);
    for (const name of readdirSync(current)) {
      const child = relative ? `${relative}/${name}` : name, path = join(directory, child);
      const info = lstatSync(path);
      if (info.isDirectory()) {
        demand(Object.keys(expected).some((n) => n.startsWith(child + '/')), 'Unexpected candidate directory'); visit(child);
      } else { regularPath(path); files.push(child); demand(digest(readFileSync(path)) === expected[child], 'Candidate member bytes differ'); }
    }
  };
  visit(); same(files.sort(), Object.keys(expected).sort(), 'Candidate member inventory differs');
}
function inspectPayload(path, row, key, binding, run = execFileSync) {
  verifyCandidateMembers(dirname(path), row);
  const mode = lstatSync(path).mode & 0o7777;
  demand((mode & 0o7000) === 0 && (process.platform === 'win32' || mode === (key === 'win' ? 0o644 : 0o755)),
    'Candidate core permissions differ');
  verifyReleaseCore(path, { ...row, sourcePolicy: binding.sourcePolicy }, key, run);
  if (!row.sidecar) return null;
  const library = join(dirname(path), row.sidecar.member); const bytes = checked(library, row.sidecar);
  demand((lstatSync(library).mode & 0o7000) === 0
    && (process.platform === 'win32' || (lstatSync(library).mode & 0o7777) === 0o644), 'Candidate companion permissions differ');
  return { sha256: row.sidecar.sha256, bytes: bytes.length, mode: lstatSync(library).mode & 0o7777,
    ...inspectCronetBytes(bytes, key) };
}
export async function acquireCandidate(root, candidate, options = {}) {
  const context = selectedCandidateContext(root, options.env ?? process.env);
  demand(!candidate || candidate === context.candidate, 'Selected App candidate differs');
  const binding = loadCandidate(root), source = read(join(root, 'scripts/desktop-core/native-dns-source-binding.json'));
  const resources = join(root, 'resources'); regularPath(resources, true);
  // Exclusive, one-shot fresh-checkout route; never replace an existing product.
  const lock = join(resources, '.native-dns-candidate.lock'); mkdirSync(lock);
  let work;
  try {
    for (const name of [...KEYS, '.native-dns-candidate']) demand(!existsSync(join(resources, name)), 'Candidate install requires empty desktop destinations');
    work = mkdtempSync(join(resources, '.native-dns-acquire-'));
    // Reuse the reader's import-time, API-only Actions credential closure. It has
    // already removed the dedicated token from native-tool environments; assets
    // and their approved CDN redirects remain anonymous in the shared reader.
    const fetchBytes = options.read ?? releasePublicReader({ repository: binding.repository });
    const before = await snapshot(binding, source, fetchBytes);
    for (const pin of binding.assets) {
      const bytes = await fetchBytes(pin.url, pin.bytes, true);
      demand(bytes.length === pin.bytes && digest(bytes) === pin.sha256, `Downloaded candidate asset differs: ${pin.name}`);
      writeFileSync(join(work, pin.name), bytes, { flag: 'wx' });
    }
    const rows = validateCandidateIndex(checked(join(work, source.index.name), source.index), binding, source);
    sourceProof(root, work, binding, source, rows);
    const staging = join(work, 'stage'); mkdirSync(staging);
    for (const key of KEYS) {
      const row = rows[key], unpacked = join(work, `unpacked-${key}`);
      execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', join(root, 'scripts/desktop-core/release-archive.py'),
        join(work, upstreamReleaseName(row.archive)), unpacked], { input: JSON.stringify({ archive: row.archive, root: row.archive.root, target: row.target }), encoding: 'utf8' });
      const extracted = join(unpacked, row.archive.root);
      inspectPayload(join(extracted, row.core.member), row, key, binding);
      // Retain the complete original archive payload, including its legal files.
      renameSync(extracted, join(staging, key));
    }
    const after = await snapshot(binding, source, fetchBytes); same(before, after, 'Candidate public identity changed during acquisition');
    const metadata = join(staging, '.native-dns-candidate'); mkdirSync(metadata);
    if (context.mode === 'temporary-default') {
      const originals = join(metadata, 'payloads'); mkdirSync(originals);
      for (const key of KEYS) copyOriginal(join(staging, key), join(originals, key));
    }
    for (const name of [source.index.name, source.sourceArchive.name, 'SHA256SUMS', 'staging-provenance.json'])
      copyFileSync(join(work, name), join(metadata, name));
    const receipt = { schema: 'polaris-native-dns-acquisition-v1', context,
      bindingSha256: digest(readFileSync(join(root, CANDIDATE_FILE))), publicReadback: before,
      payloads: Object.fromEntries(KEYS.map((k) => [k, { core: rows[k].core, sidecar: rows[k].sidecar }])),
      sourceVerified: true, binaryBytesVerified: true, runtimeValidated: false, AppAccepted: false,
      publicationEligible: false, sealedInputProfile: 'fe182-check-only; Rust regressions and launch acceptance not asserted' };
    writeFileSync(join(metadata, 'receipt.json'), JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx' });
    // Receipt is installed last; partial installs after interruption cannot verify.
    installCandidateStage(staging, resources);
    return receipt;
  } finally { if (work) rmSync(work, { recursive: true, force: true }); rmSync(lock, { recursive: true }); }
}
export function installCandidateStage(staging, resources, rename = renameSync) {
  const names = [...KEYS, '.native-dns-candidate'], installed = [];
  for (const name of names) demand(!existsSync(join(resources, name)), 'No overlay onto an existing desktop installation');
  try {
    for (const name of names) { rename(join(staging, name), join(resources, name)); installed.push(name); }
  } catch (error) {
    // Every destination was absent before this transaction. Remove only its
    // own installed paths. An interrupted process remains blocked by the lock.
    for (const name of installed.reverse()) rmSync(join(resources, name), { recursive: true, force: true });
    throw error;
  }
}
export async function consumeTemporaryDesktop(root, manifest, options = {}) {
  loadTemporaryDesktopSelection(root, manifest);
  const context = selectedCandidateContext(root, options.env ?? process.env);
  demand(context.mode === 'temporary-default', 'Committed default cannot use manual validation context');
  const present = [...KEYS, '.native-dns-candidate'].map((name) => existsSync(join(root, 'resources', name)));
  if (present.some(Boolean)) {
    demand(present.every(Boolean), 'Partial/old desktop installation requires explicit cleanup; no overlay allowed');
    for (const key of KEYS) verifyCandidatePayload(root, key, join(root, 'resources', key, RELEASE_TARGETS[key].core));
    return read(join(root, META, 'receipt.json'));
  }
  return acquireCandidate(root, context.candidate, options);
}
export function verifyTemporaryCarrierMembers(directory, row, key) {
  const expected = Object.fromEntries(Object.entries(row.archive.members)
    .map(([name, hash]) => [name.slice(row.archive.root.length + 1), hash]));
  const extras = [key === 'win' ? 'polaris-helper.exe' : 'polaris-helper', 'core-manifest.json', 'candidate-carrier.json',
    ...(key === 'win' ? ['polaris-cleaner.exe'] : [])];
  const files = [];
  const visit = (relative = '') => {
    const current = join(directory, relative); regularPath(current, true);
    for (const name of readdirSync(current)) {
      const child = relative ? `${relative}/${name}` : name, path = join(directory, child);
      if (lstatSync(path).isDirectory()) {
        demand(Object.keys(expected).some((entry) => entry.startsWith(child + '/')), 'Unexpected carrier directory'); visit(child);
      } else {
        regularPath(path); files.push(child);
        demand(Object.hasOwn(expected, child) || extras.includes(child), 'Unexpected temporary carrier member');
        if (Object.hasOwn(expected, child)) demand(digest(readFileSync(path)) === expected[child], 'Temporary carrier original member differs');
        else actualCoreFile(path); // Installed metadata/helpers never accept links.
      }
    }
  };
  visit(); demand(Object.keys(expected).every((name) => files.includes(name)), 'Temporary carrier original member missing');
}
export function validateCandidateAcquisitionReceipt(receipt, binding, source, context, bindingSha256) {
  demand(receipt.schema === 'polaris-native-dns-acquisition-v1'
    && receipt.bindingSha256 === bindingSha256
    && receipt.sourceVerified === true && receipt.binaryBytesVerified === true
    && receipt.runtimeValidated === false && receipt.AppAccepted === false && receipt.publicationEligible === false,
  'Candidate acquisition receipt differs');
  same(receipt.context, context, 'Candidate receipt belongs to another App attempt');
  const observed = receipt.publicReadback;
  demand(observed?.releaseId === binding.releaseId && observed.tag === binding.tag
    && observed.tagObject === binding.tagObject && observed.sourceCommit === binding.sourceCommit
    && observed.sourceTree === binding.sourceTree, 'Candidate receipt public identity differs');
  same(observed.assets, binding.assets, 'Candidate receipt asset inventory differs');
  same(observed.sourceGitlinks, source.sourceGitlinks, 'Candidate receipt source gitlinks differ');
  const producer = observed.producerReadback;
  demand(producer?.runId === binding.producer.runId && producer.runAttempt === binding.producer.runAttempt
    && producer.sourceCommit === binding.sourceCommit && producer.workflow === binding.producer.workflow
    && positive(producer.workflowId) && producer.status === 'completed' && producer.conclusion === 'success',
    'Candidate receipt original producer differs');
  same(Object.keys(producer.jobs).sort(), KEYS.toSorted(), 'Candidate receipt native jobs differ');
  for (const platform of KEYS) {
    const job = producer.jobs[platform], pin = binding.producer.nativeJobs[platform];
    demand(job.id === pin.id && job.name === pin.name && job.runId === producer.runId
      && job.runAttempt === producer.runAttempt && job.sourceCommit === binding.sourceCommit
      && job.status === 'completed' && job.conclusion === 'success', 'Candidate receipt native job differs');
    same(receipt.payloads?.[platform], { core: binding.platforms[platform].core,
      sidecar: binding.platforms[platform].sidecar }, 'Candidate receipt payload inventory differs');
  }
  same(Object.keys(receipt.payloads).sort(), [...KEYS].sort(), 'Complete candidate receipt payload inventory required');
  return receipt;
}
export function verifyCandidatePayload(root, key, core, run = execFileSync) {
  demand(KEYS.includes(key), 'Candidate desktop target required');
  demand(!existsSync(join(root, 'resources/.native-dns-candidate.lock')), 'Candidate acquisition is incomplete');
  const context = selectedCandidateContext(root, process.env, run);
  const binding = loadCandidate(root), source = read(join(root, 'scripts/desktop-core/native-dns-source-binding.json'));
  const metadata = join(root, META), receipt = read(join(metadata, 'receipt.json'));
  validateCandidateAcquisitionReceipt(receipt, binding, source, context, digest(readFileSync(join(root, CANDIDATE_FILE))));
  const rows = validateCandidateIndex(checked(join(metadata, source.index.name), source.index), binding, source);
  for (const name of ['SHA256SUMS', 'staging-provenance.json']) checked(join(metadata, name), asset(binding, name));
  sourceProof(root, metadata, binding, source, rows, run);
  const canonicalCore = join(root, 'resources', key, rows[key].core.member);
  if (context.mode !== 'temporary-default') demand(resolve(core) === resolve(canonicalCore), 'Candidate must use its canonical resource path');
  same(receipt.payloads[key], { core: rows[key].core, sidecar: rows[key].sidecar }, 'Candidate receipt payload differs');
  const original = join(selectedOriginalDirectory(root, key, context), rows[key].core.member);
  const library = inspectPayload(original, rows[key], key, binding, run);
  if (context.mode === 'temporary-default') {
    verifyTemporaryCarrierMembers(dirname(canonicalCore), rows[key], key);
    if (resolve(core) !== resolve(canonicalCore)) verifyTemporaryCarrierMembers(dirname(core), rows[key], key);
    checked(core, rows[key].core);
    demand((lstatSync(core).mode & 0o7777) === (lstatSync(original).mode & 0o7777), 'Final temporary core permissions differ');
    verifyReleaseCore(core, { ...rows[key], sourcePolicy: binding.sourcePolicy }, key, run);
    if (rows[key].sidecar) {
      const library = join(dirname(core), rows[key].sidecar.member);
      checked(library, rows[key].sidecar);
      demand((lstatSync(library).mode & 0o7777) === (lstatSync(join(dirname(original), rows[key].sidecar.member)).mode & 0o7777),
        'Final temporary companion permissions differ');
    }
  }
  return { binding, row: rows[key], receipt, library };
}
export function temporaryDesktopNoticeLines(root) {
  const selection = loadTemporaryDesktopSelection(root), binding = loadCandidate(root);
  const source = read(join(root, 'scripts/desktop-core/native-dns-source-binding.json'));
  return [
    `Temporary desktop default: ${binding.version} — https://github.com/${binding.repository}/releases/tag/${binding.tag}`,
    `Temporary desktop source: https://github.com/${binding.repository}/tree/${binding.sourceCommit}`,
    `Temporary desktop source tree: ${binding.sourceTree}`,
    `Temporary desktop contract: ${CANDIDATE_FILE} (SHA-256 ${selection.contractSha256})`,
    `Temporary desktop producer: run ${binding.producer.runId}, attempt ${binding.producer.runAttempt}; annotated tag ${binding.tagObject}`,
    `Corresponding source archive: https://github.com/${binding.repository}/releases/download/${binding.tag}/${source.sourceArchive.name} (SHA-256 ${source.sourceArchive.sha256})`,
    'Desktop selection uses src-tauri/core-manifest.json desktopTemporary. Original staging evidence is retained without formal-release relabeling.',
    'Mobile delivery remains pending exact new AAR/XCFramework identities and platform acceptance; historical mobile pins do not validate the new DNS policy.',
  ];
}
export function validateTemporaryDesktopNotice(text, root) {
  for (const line of temporaryDesktopNoticeLines(root)) demand(text.includes(line), 'Temporary desktop corresponding-source notice differs');
}
export function inspectCandidateNativePayload(root, key, core, library, run = execFileSync) {
  const proof = verifyCandidatePayload(root, key, core, run), { row, binding, receipt } = proof;
  demand(row.sidecar && resolve(library) === resolve(dirname(core), row.sidecar.member), 'Candidate companion path differs');
  const module = `github.com/sagernet/cronet-go/lib/${key === 'win' ? 'windows' : 'linux'}_amd64`;
  const acquisition = row.buildGraph.prebuiltAcquisitionModules.filter((m) => m.module === module);
  demand(acquisition.length === 1, 'Candidate platform Cronet module missing');
  return { schema: 'polaris-native-dns-payload-input-v1', candidate: receipt.context.candidate, platform: key,
    core: { path: resolve(core), sha256: row.core.sha256, mode: lstatSync(core).mode & 0o7777, buildID: row.buildID, goVersion: row.goVersion },
    library: { path: resolve(library), ...proof.library, module, moduleIdentity: acquisition[0] },
    producer: row, origin: { kind: 'temporary-native-dns-candidate', bindingSha256: receipt.bindingSha256,
      releaseId: binding.releaseId, tagObject: binding.tagObject, context: receipt.context },
    actualAppAcceptance: 'pending runtime gates and packaged App acceptance' };
}
export function verifiedCandidateSource(root) {
  let selected;
  for (const key of KEYS) {
    const proof = verifyCandidatePayload(root, key, join(root, 'resources', key, RELEASE_TARGETS[key].core));
    selected ??= proof;
    same(proof.binding, selected.binding, 'Selected platform source differs');
  }
  const temporary = selected.receipt.context.mode === 'temporary-default';
  return { schema: temporary ? 'polaris-native-dns-compiled-source-v2' : 'polaris-native-dns-compiled-source-v1',
    ...(temporary ? { purpose: 'temporary-default' } : {}), validationOnly: !temporary,
    sourceCommit: selected.binding.sourceCommit, sourceTree: selected.binding.sourceTree,
    bindingSha256: selected.receipt.bindingSha256, context: selected.receipt.context,
    platforms: KEYS, AppAccepted: false, publicationEligible: false };
}
// Internal projection only: the installation identity uses the digest of this
// exact temporary contract, never a formal producer/provider fingerprint.
export function candidateInstallationContract(binding, bindingSha256) {
  demand(binding?.schema === 'polaris-native-dns-candidate-v1' && hex(bindingSha256, 64)
    && binding.AppAccepted === false && binding.publicationEligible === false,
  'Verified candidate installation input required');
  return { version: binding.version, platforms: Object.fromEntries(KEYS.map((key) => [key, {
    core: binding.platforms[key].core, sidecar: binding.platforms[key].sidecar,
    providerFingerprint: bindingSha256,
  }])) };
}

export function candidateBuildSelection(source, root, platform, directory, env = process.env,
  host = { os: process.platform, arch: process.arch }) {
  if (source?.schema === 'polaris-native-dns-compiled-source-v2') {
    loadTemporaryDesktopSelection(root);
    demand(!Object.hasOwn(env, 'POLARIS_NATIVE_DNS_CANDIDATE') && source.purpose === 'temporary-default'
      && source.validationOnly === false && source.AppAccepted === false && source.publicationEligible === false
      && source.context?.mode === 'temporary-default' && hex(source.context.candidate, 40)
      && env.POLARIS_BUILD_ID === source.context.candidate && !Object.hasOwn(env, 'CARGO_TARGET_DIR')
      && KEYS.includes(platform) && directory === join(root, 'resources', platform), 'Same default App/helper build identity required');
    return;
  }
  demand(env.POLARIS_NATIVE_DNS_CANDIDATE === '1'
    && source?.schema === 'polaris-native-dns-compiled-source-v1'
    && source.validationOnly === true && source.AppAccepted === false && source.publicationEligible === false
    && hex(source.context?.candidate, 40) && env.POLARIS_BUILD_ID === source.context.candidate
    && !Object.hasOwn(env, 'CARGO_TARGET_DIR'), 'Same candidate App/helper build identity required');
  const native = { 'linux:x64': 'linux', 'win32:x64': 'win', 'darwin:x64': 'mac-x64', 'darwin:arm64': 'mac-arm64' }[`${host.os}:${host.arch}`];
  demand(platform === native && directory === candidateAppDirectory(root, platform), 'Exact native candidate App carrier required');
}

// Cargo may hard-link a mutable target/debug output to its deps artifact. Only
// this preparation boundary accepts that source; the frozen destination still
// has to pass the unchanged single-link installed-file verifier.
export function freezeCandidateBuildOutput(root, directory, name, copy = copyFileSync) {
  demand(['polaris-helper', 'polaris-helper.exe', 'polaris-cleaner.exe'].includes(name),
    'Candidate helper/cleaner build output required');
  const from = join(root, 'target/debug', name), to = join(directory, name);
  regularPath(from); regularPath(directory, true);
  const before = lstatSync(from);
  const fd = openSync(from, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  let copied = false;
  try {
    const opened = fstatSync(fd);
    const sameObject = (a, b) => a.isFile() && !a.isSymbolicLink() && a.ino === b.ino && a.dev === b.dev
      && a.size === b.size && a.mtimeMs === b.mtimeMs && a.ctimeMs === b.ctimeMs && a.nlink === b.nlink;
    demand(sameObject(opened, before) && opened.nlink >= 1 && opened.size > 0
      && opened.size <= 512 * 1024 ** 2, 'Candidate build output identity differs');
    const hash = createHash('sha256'), buffer = Buffer.alloc(64 * 1024);
    let bytes = 0, count;
    while ((count = readSync(fd, buffer, 0, buffer.length, null)) > 0) {
      bytes += count; demand(bytes <= opened.size, 'Candidate build output grew during freezing');
      hash.update(buffer.subarray(0, count));
    }
    demand(bytes === opened.size, 'Candidate build output truncated during freezing');
    const pin = { bytes, sha256: hash.digest('hex') };
    copy(from, to, constants.COPYFILE_EXCL); copied = true;
    demand(sameObject(fstatSync(fd), opened) && sameObject(lstatSync(from), opened),
      'Candidate build output changed during freezing');
    actualCoreFile(to, pin); // exclusive copy must be an independent single-link object
    return pin;
  } catch (error) {
    if (copied) rmSync(to, { force: true });
    throw error;
  } finally { closeSync(fd); }
}

// Verify the immutable preparation receipt, never Cargo's later mutable outputs.
// The caller obtains source from verifiedCandidateSource before using this check.
export function verifyCandidateAppCarrier(directory, source, platform) {
  demand(KEYS.includes(platform) && ['polaris-native-dns-compiled-source-v1', 'polaris-native-dns-compiled-source-v2'].includes(source?.schema),
    'Verified candidate App source required');
  regularPath(directory, true);
  const receipt = read(join(directory, 'candidate-carrier.json'));
  demand(receipt?.schema === 'polaris-native-dns-app-carrier-v1'
    && receipt.built && typeof receipt.built === 'object' && !Array.isArray(receipt.built),
  'Candidate App carrier receipt required');
  same(receipt, { schema: 'polaris-native-dns-app-carrier-v1', source, built: receipt.built },
    'Candidate App carrier belongs to another build/source/run');
  const names = [platform === 'win' ? 'polaris-helper.exe' : 'polaris-helper',
    ...(platform === 'win' ? ['polaris-cleaner.exe'] : [])];
  same(Object.keys(receipt.built).sort(), names.sort(), 'Exact frozen helper inventory required');
  for (const name of names) {
    const pin = receipt.built[name];
    demand(positive(pin?.bytes) && hex(pin?.sha256, 64), 'Actual frozen helper pin required');
    same(pin, { bytes: pin.bytes, sha256: pin.sha256 }, 'Frozen helper pin fields differ');
    actualCoreFile(join(directory, name), pin);
  }
  return receipt;
}

export function candidateAppDirectory(root, key) {
  demand(KEYS.includes(key), 'Candidate App desktop target required');
  return join(root, 'resources', '.native-dns-app', key);
}

async function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
  const [command, arg, ...extra] = process.argv.slice(2), match = /^--candidate=([a-f0-9]{40})$/.exec(arg ?? '');
  if (arg === undefined) demand(selectedCandidateContext(root).mode === 'temporary-default', 'Committed temporary default required');
  else { demand(match && extra.length === 0, 'Explicit candidate SHA required'); candidateContext(match[1], process.env, root); }
  if (command === 'acquire') console.log(JSON.stringify(await acquireCandidate(root, match?.[1])));
  else if (command === 'verify') {
    for (const key of KEYS) verifyCandidatePayload(root, key, join(root, 'resources', key, RELEASE_TARGETS[key].core));
    console.log(JSON.stringify({ binaryBytesVerified: true, runtimeValidated: false, AppAccepted: false }));
  } else if (command === 'source') console.log(JSON.stringify(verifiedCandidateSource(root)));
  else throw Error('Unknown candidate command');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
