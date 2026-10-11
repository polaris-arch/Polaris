// Same-attempt resource handoff between ordinary Risk preflight and core_wire.
// It carries original checked bytes; it does not cache HTTP for another attempt
// or replace either job's complete checks with a stored success assertion.
import { execFileSync } from 'node:child_process';
import { chmodSync, constants, copyFileSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, digest, requireGraph as demand } from './source-graph.mjs';
import { hasDesktopRelease, loadReleaseContract, portableCanonical, regularPath, RELEASE_TARGETS,
  verifyPackagedRelease } from './release-consumer.mjs';
import { releaseGateContext } from './release-native-gates.mjs';

const DIRECTORIES = ['.release-receipts', ...Object.keys(RELEASE_TARGETS)].sort();
const HANDOFF_ROOT = 'core-wire-inputs';
const same = (a, b, message) => demand(canonical(a) === canonical(b), message);
const present = (path) => { try { lstatSync(path); return true; } catch (e) { if (e.code === 'ENOENT') return false; throw e; } };
function current(root, candidate, env, run) {
  const context = releaseGateContext(env);
  demand(!env.GITHUB_REF?.startsWith('refs/tags/') && env.POLARIS_NATIVE_PAYLOAD_VALIDATION !== '1'
    && /^[a-f0-9]{40}$/.test(candidate ?? '')
    && run('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim() === candidate
    && !run('git', ['status', '--porcelain', '--untracked-files=no'], { cwd: root, encoding: 'utf8' }).trim(),
  'Ordinary current-attempt wire handoff required');
  return context;
}
export function coreWireHandoffName(candidate, context) {
  return `core-wire-inputs-${candidate}-${context.runId}-${context.attempt}`;
}
function inventory(resources) {
  regularPath(resources, true); same(readdirSync(resources).sort(), DIRECTORIES, 'Wire handoff directory inventory differs');
  const files = [];
  function walk(path, relative) {
    const st = lstatSync(path); demand(!st.isSymbolicLink(), 'Linked wire handoff input');
    if (st.isDirectory()) { for (const name of readdirSync(path).sort()) walk(join(path, name), relative ? `${relative}/${name}` : name); return; }
    demand(st.isFile() && (st.mode & 0o7022) === 0, 'Unsafe wire handoff input');
    const raw = readFileSync(path); files.push({ name: relative, bytes: raw.length, sha256: digest(raw), mode: st.mode & 0o777 });
  }
  walk(resources, ''); return files;
}
function copied(files, from, to) {
  for (const file of files) {
    const source = join(from, file.name), destination = join(to, file.name);
    mkdirSync(dirname(destination), { recursive: true, mode: 0o700 });
    copyFileSync(source, destination, constants.COPYFILE_EXCL); chmodSync(destination, file.mode);
  }
}
function verify(root, manifest, run, host) {
  for (const [key, target] of Object.entries(RELEASE_TARGETS)) {
    verifyPackagedRelease(root, manifest, key, join(root, 'resources', key, target.core), run, host);
  }
}
export function createCoreWireHandoff(root, manifest, bundle, candidate, options = {}) {
  const run = options.run ?? execFileSync, context = current(root, candidate, options.env ?? process.env, run);
  const contract = loadReleaseContract(root, manifest); verify(root, manifest, run, options.host ?? process.platform);
  demand(!present(bundle), 'Wire handoff output already exists'); regularPath(dirname(bundle), true);
  const source = join(root, 'resources'), scratch = join(bundle, 'resources');
  // Only the five checked resource directories are transferred, never other
  // tracked assets or a previous-attempt carrier/approval record.
  const stage = join(bundle, 'resources'); mkdirSync(bundle, { mode: 0o700 }); mkdirSync(stage, { mode: 0o700 });
  try {
    for (const name of DIRECTORIES) {
      regularPath(join(source, name), true); mkdirSync(join(stage, name), { mode: 0o700 });
    }
    const files = [];
    for (const name of DIRECTORIES) {
      function walk(path, relative) {
        const st = lstatSync(path); demand(!st.isSymbolicLink(), 'Linked wire handoff input');
        if (st.isDirectory()) { for (const child of readdirSync(path).sort()) walk(join(path, child), `${relative}/${child}`); return; }
        demand(st.isFile() && (st.mode & 0o7022) === 0, `Unsafe wire handoff input ${relative} mode=${(st.mode & 0o7777).toString(8)}`);
        const raw = readFileSync(path); files.push({ name: relative, bytes: raw.length, sha256: digest(raw), mode: st.mode & 0o777 });
      }
      walk(join(source, name), name);
    }
    copied(files, source, scratch); same(inventory(scratch), files, 'Copied wire handoff bytes differ');
    const facts = { schema: 'polaris-core-wire-input-handoff-v1', candidate, context,
      manifestSha256: digest(readFileSync(join(root, 'src-tauri/core-manifest.json'))),
      contractSha256: manifest.desktopRelease.contractSha256, assetIndexSha256: contract.assetIndex.sha256, files };
    writeFileSync(join(bundle, 'handoff.json'), JSON.stringify({ ...facts, fingerprint: digest(portableCanonical(facts)) }, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    return coreWireHandoffName(candidate, context);
  } catch (error) { rmSync(bundle, { recursive: true, force: true }); throw error; }
}
export function importCoreWireHandoff(root, manifest, bundle, candidate, artifactName, options = {}) {
  const run = options.run ?? execFileSync, context = current(root, candidate, options.env ?? process.env, run);
  same(artifactName, coreWireHandoffName(candidate, context), 'Wire handoff artifact identity differs');
  regularPath(bundle, true); same(readdirSync(bundle).sort(), ['handoff.json', 'resources'], 'Unexpected wire handoff payload');
  const path = join(bundle, 'handoff.json'); regularPath(path); const { fingerprint, ...facts } = JSON.parse(readFileSync(path, 'utf8'));
  const contract = loadReleaseContract(root, manifest);
  demand(facts.schema === 'polaris-core-wire-input-handoff-v1' && facts.candidate === candidate
    && fingerprint === digest(portableCanonical(facts))
    && facts.manifestSha256 === digest(readFileSync(join(root, 'src-tauri/core-manifest.json')))
    && facts.contractSha256 === manifest.desktopRelease.contractSha256
    && facts.assetIndexSha256 === contract.assetIndex.sha256, 'Wire handoff source binding differs');
  same(facts.context, context, 'Wire handoff attempt differs');
  const source = join(bundle, 'resources'); same(inventory(source), facts.files, 'Wire handoff bytes or permissions differ');
  const resources = join(root, 'resources'); regularPath(resources, true);
  demand(!present(join(resources, '.release-consumer.lock')) && DIRECTORIES.every((n) => !present(join(resources, n))),
    'Wire handoff requires fresh resource destinations');
  const created = [];
  try {
    for (const name of DIRECTORIES) { const destination = join(resources, name); mkdirSync(destination, { mode: 0o700 }); created.push(destination); }
    copied(facts.files, source, resources); verify(root, manifest, run, options.host ?? process.platform);
    return { candidate, context, files: facts.files.length, fingerprint };
  } catch (error) { for (const path of created) rmSync(path, { recursive: true, force: true }); throw error; }
}
// GitHub artifact transport restores file modes to 0644. Preserve the checked
// core executable bits inside a tar, and reuse the strict pinned extractor.
export function packCoreWireHandoff(bundle, destination) {
  regularPath(bundle, true); demand(basename(bundle) === HANDOFF_ROOT && !present(destination), 'Fresh named wire archive required');
  const proof = join(bundle, 'handoff.json'); regularPath(proof); const parsed = JSON.parse(readFileSync(proof));
  same(inventory(join(bundle, 'resources')), parsed.files, 'Wire archive input differs');
  mkdirSync(destination, { mode: 0o700 });
  try {
    const name = 'core-wire-inputs.tar.gz', path = join(destination, name);
    execFileSync('tar', ['-czf', path, '-C', dirname(bundle), HANDOFF_ROOT]);
    const raw = readFileSync(path), members = { [`${HANDOFF_ROOT}/handoff.json`]: digest(readFileSync(proof)),
      ...Object.fromEntries(parsed.files.map((f) => [`${HANDOFF_ROOT}/resources/${f.name}`, f.sha256])) };
    const pin = { root: HANDOFF_ROOT, archive: { name, bytes: raw.length, sha256: digest(raw), members } };
    writeFileSync(join(destination, 'archive.json'), JSON.stringify(pin, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    return pin;
  } catch (error) { rmSync(destination, { recursive: true, force: true }); throw error; }
}
export function unpackCoreWireHandoff(artifact, destination) {
  regularPath(artifact, true); same(readdirSync(artifact).sort(), ['archive.json', 'core-wire-inputs.tar.gz'], 'Wire transport inventory differs');
  const metadata = join(artifact, 'archive.json'); regularPath(metadata); const pin = JSON.parse(readFileSync(metadata));
  demand(pin.root === HANDOFF_ROOT && pin.archive?.name === 'core-wire-inputs.tar.gz', 'Wire archive identity differs');
  const archive = join(artifact, pin.archive.name); regularPath(archive);
  execFileSync('python3', ['-B', join(dirname(fileURLToPath(import.meta.url)), 'release-archive.py'), archive, destination],
    { input: JSON.stringify(pin), encoding: 'utf8', maxBuffer: 1024 * 1024 });
  return join(destination, HANDOFF_ROOT);
}
async function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..'), [command, ...args] = process.argv.slice(2);
  const values = Object.fromEntries(args.map((arg) => { const m = /^--(bundle-dir|candidate|artifact-name)=(.+)$/.exec(arg); demand(m, 'Unexpected wire handoff argument'); return m.slice(1); }));
  demand(args.length === Object.keys(values).length && values.candidate && values['bundle-dir'], 'Explicit wire handoff identity required');
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json'), 'utf8'));
  if (command === 'create') {
    demand(!values['artifact-name'], 'Create chooses its own artifact identity');
    let name = '';
    if (hasDesktopRelease(manifest)) {
      const destination = resolve(values['bundle-dir']), temporary = mkdtempSync(join(dirname(destination), '.core-wire-export-'));
      try { const payload = join(temporary, HANDOFF_ROOT); name = createCoreWireHandoff(root, manifest, payload, values.candidate);
        packCoreWireHandoff(payload, destination); } finally { rmSync(temporary, { recursive: true, force: true }); }
    }
    demand(process.env.GITHUB_OUTPUT, 'Actions output required'); writeFileSync(process.env.GITHUB_OUTPUT, `artifact_name=${name}\n`, { flag: 'a' });
  } else if (command === 'import' || command === 'check') {
    demand(values['artifact-name'], 'Exact wire handoff artifact required');
    const context = current(root, values.candidate, process.env, execFileSync);
    same(values['artifact-name'], coreWireHandoffName(values.candidate, context), 'Wire handoff artifact identity differs');
    if (command === 'check') return;
    const artifact = resolve(values['bundle-dir']), temporary = mkdtempSync(join(dirname(artifact), '.core-wire-import-'));
    try { const payload = unpackCoreWireHandoff(artifact, join(temporary, 'unpacked'));
      console.log(JSON.stringify(importCoreWireHandoff(root, manifest, payload, values.candidate, values['artifact-name'])));
    } finally { rmSync(temporary, { recursive: true, force: true }); }
  } else throw Error('Unknown wire handoff command');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main().catch((error) => { console.error(error.message); process.exitCode = 1; });
