// Same-attempt resource handoff between ordinary Risk preflight and core_wire.
// It carries original checked bytes; it does not cache HTTP for another attempt
// or accept a job-green assertion. An optional exact command result is reused
// only after rechecking this attempt, complete inputs, source and real tools.
import { execFileSync } from 'node:child_process';
import { chmodSync, constants, copyFileSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, writeFileSync, openSync, closeSync } from 'node:fs';
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
export function verifyWireArtifactMetadata(metadata, candidate, context, artifactName, id, sha256, head) {
  demand(/^[1-9][0-9]*$/.test(id ?? '') && Number.isSafeInteger(Number(id))
    && /^[a-f0-9]{64}$/.test(sha256 ?? '')
    && metadata?.id === Number(id) && metadata.name === artifactName && metadata.expired === false
    && metadata.digest === `sha256:${sha256}` && metadata.workflow_run?.id === Number(context.runId)
    && metadata.workflow_run?.head_sha === head && artifactName === coreWireHandoffName(candidate, context),
  'Current-attempt wire artifact API pin differs');
}
function checkWireArtifact(candidate, context, artifactName, env = process.env, run = execFileSync) {
  const id = env.CORE_WIRE_INPUT_ARTIFACT_ID, sha256 = env.CORE_WIRE_INPUT_ARTIFACT_DIGEST;
  demand(/^[1-9][0-9]*$/.test(id ?? '') && /^[a-f0-9]{64}$/.test(sha256 ?? '')
    && /^[a-f0-9]{40}$/.test(env.GITHUB_SHA ?? ''), 'Exact wire transport ID/digest required');
  const metadata = JSON.parse(run('gh', ['api', `/repos/${context.repository}/actions/artifacts/${id}`],
    { env, encoding: 'utf8', maxBuffer: 1024 * 1024 }));
  // PR workflow_run API binds the head commit; GITHUB_SHA can be the synthetic
  // merge checkout. The candidate is already the explicit PR head checkout.
  const head = env.GITHUB_EVENT_NAME === 'pull_request' ? candidate : env.GITHUB_SHA;
  verifyWireArtifactMetadata(metadata, candidate, context, artifactName, id, sha256, head);
  demand(Number.isSafeInteger(metadata.size_in_bytes) && metadata.size_in_bytes > 0
    && metadata.size_in_bytes <= 512 * 1024 * 1024, 'Wire artifact ZIP size limit');
  return metadata;
}
export function downloadWireArtifact(root, destination, candidate, context, name, options = {}) {
  const run = options.run ?? execFileSync, env = options.env ?? process.env;
  const metadata = checkWireArtifact(candidate, context, name, env, run);
  demand(!present(destination), 'Fresh wire transport destination required'); regularPath(dirname(destination), true);
  const temporary = mkdtempSync(join(dirname(destination), '.wire-download-'));
  try {
    const zip = join(temporary, 'artifact.zip'), fd = openSync(zip, 'wx', 0o600);
    try { run('gh', ['api', `/repos/${context.repository}/actions/artifacts/${metadata.id}/zip`],
      { env, stdio: ['ignore', fd, 'pipe'], timeout: 300000, maxBuffer: 1024 * 1024 }); } finally { closeSync(fd); }
    run(process.platform === 'win32' ? 'python' : 'python3', ['-B',
      join(root, 'scripts/desktop-core/release-archive.py'), zip, destination],
    { input: JSON.stringify({ wireTransport: { bytes: metadata.size_in_bytes, sha256: metadata.digest.slice(7) } }),
      encoding: 'utf8', timeout: 300000, maxBuffer: 1024 * 1024 });
  } finally { rmSync(temporary, { recursive: true, force: true }); }
}
function inventory(resources, host = process.platform, declared = [], strict = true) {
  regularPath(resources, true);
  if (strict) same(readdirSync(resources).sort(), DIRECTORIES, 'Wire handoff directory inventory differs');
  else demand(DIRECTORIES.every((name) => present(join(resources, name))), 'Missing wire inputs');
  const files = [];
  function walk(path, relative) {
    const st = lstatSync(path); demand(!st.isSymbolicLink(), 'Linked wire handoff input');
    if (st.isDirectory()) { for (const name of readdirSync(path).sort()) walk(join(path, name), relative ? `${relative}/${name}` : name); return; }
    const transport = declared.find((file) => file.name === relative);
    // NTFS exposes only writable/read-only through Node's POSIX mode facade.
    // Pinned transport bytes include each original safe POSIX mode; preserve
    // the write/read-only projection, never assert NTFS has Unix execute bits.
    const mode = handoffMode(st.mode, host, transport?.mode);
    demand(st.isFile(), 'Unsafe wire handoff input');
    const raw = readFileSync(path); files.push({ name: relative, bytes: raw.length, sha256: digest(raw), mode });
  }
  for (const name of DIRECTORIES) walk(join(resources, name), name); return files;
}
export function handoffMode(actual, host, declared) {
  if (host !== 'win32') {
    demand((actual & 0o7022) === 0, 'Unsafe wire handoff input');
    return actual & 0o777;
  }
  demand(Number.isInteger(declared) && declared > 0 && declared <= 0o777
    && (declared & 0o022) === 0
    && (actual & 0o222) === ((declared & 0o200) ? 0o222 : 0), 'Windows wire permission projection differs');
  return declared;
}
const WIRE_ARGS = ['test', '-p', 'polaris-singbox-grpc', '--test', 'bundled_core_wire',
  'vendored_proto_matches_every_bundled_core', '--', '--exact'];
function wireTools(root, run) {
  return Object.fromEntries([['rustc', ['-Vv']], ['protoc', ['--version']], ['go', ['version']]]
    .map(([tool, args]) => [tool, digest(run(tool, args, { cwd: root, encoding: 'utf8' }))]));
}
function wireSource(root, run) {
  const names = run('git', ['ls-files', '-z', '--', 'Cargo.lock', 'Cargo.toml', 'rust-toolchain*',
    'crates/singbox-grpc', 'scripts/desktop-core/wire-input-handoff.mjs'], { cwd: root, encoding: 'utf8' })
    .split('\0').filter(Boolean).sort();
  demand(names.includes('Cargo.lock') && names.includes('crates/singbox-grpc/tests/bundled_core_wire.rs'), 'Wire source inventory missing');
  return Object.fromEntries(names.map((name) => { const path = join(root, name); regularPath(path); return [name, digest(readFileSync(path))]; }));
}
// Keep the original wire gate's coverage axis: all manifest platforms must
// exist and every immediate resources directory is surveyed for orphan cores.
// Dashboard/data may coexist, but cannot hide an extra sing-box executable.
export function surveyWireCores(root) {
  const resources = join(root, 'resources'), path = join(root, 'src-tauri/core-manifest.json');
  regularPath(resources, true); regularPath(path);
  const manifest = JSON.parse(readFileSync(path, 'utf8'));
  demand(manifest.coreArchiveSha256 && typeof manifest.coreArchiveSha256 === 'object'
    && !Array.isArray(manifest.coreArchiveSha256), 'Wire platform manifest missing');
  const declared = Object.keys(manifest.coreArchiveSha256).sort();
  demand(declared.length && declared.every((key) => /^[a-z0-9-]+$/.test(key)), 'Wire platform inventory invalid');
  const cores = declared.map((key) => {
    const name = `${key}/${key === 'win' || key.startsWith('win-') ? 'sing-box.exe' : 'sing-box'}`;
    regularPath(join(resources, name)); return name;
  });
  for (const name of readdirSync(resources).sort()) {
    const directory = join(resources, name), st = lstatSync(directory);
    demand(!st.isSymbolicLink(), 'Linked wire resource directory');
    if (!st.isDirectory() || declared.includes(name)) continue;
    demand(!['sing-box', 'sing-box.exe'].some((binary) => present(join(directory, binary))), 'Orphan wire core');
  }
  return { declared, cores };
}
function wireFacts(root, candidate, env, run) {
  return { schema: 'polaris-current-attempt-wire-result-v1', candidate, context: current(root, candidate, env, run),
    command: ['cargo', ...WIRE_ARGS], source: wireSource(root, run), tools: wireTools(root, run),
    coreInventory: surveyWireCores(root),
    files: inventory(join(root, 'resources'), process.platform, [], false) };
}
export function verifyWireResult(result, expected) {
  const { fingerprint, ...facts } = result ?? {};
  demand(facts.schema === 'polaris-current-attempt-wire-result-v1'
    && fingerprint === digest(portableCanonical(facts)), 'Wire result fingerprint differs');
  same(facts, expected, 'Wire result input/source/tool/attempt differs');
  return true;
}
export function runCoreWire(root, candidate, destination, options = {}) {
  const run = options.run ?? execFileSync, env = options.env ?? process.env;
  demand(!present(destination), 'Wire result output already exists');
  const before = wireFacts(root, candidate, env, run);
  run('cargo', WIRE_ARGS, { cwd: root, stdio: 'inherit', env: { ...env, POLARIS_REQUIRE_KERNEL_GATE: '1' } });
  const after = wireFacts(root, candidate, env, run);
  same(before, after, 'Wire inputs changed during command');
  writeFileSync(destination, JSON.stringify({ ...after, fingerprint: digest(portableCanonical(after)) }, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
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
      contractSha256: manifest.desktopRelease.contractSha256, assetIndexSha256: contract.assetIndex.sha256, files,
      wireResult: options.wireResult ? (regularPath(options.wireResult), JSON.parse(readFileSync(options.wireResult, 'utf8'))) : null };
    if (facts.wireResult) verifyWireResult(facts.wireResult, wireFacts(root, candidate, options.env ?? process.env, run));
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
  const source = join(bundle, 'resources'); same(inventory(source, options.host ?? process.platform, facts.files), facts.files, 'Wire handoff bytes or permissions differ');
  const resources = join(root, 'resources'); regularPath(resources, true);
  demand(!present(join(resources, '.release-consumer.lock')) && DIRECTORIES.every((n) => !present(join(resources, n))),
    'Wire handoff requires fresh resource destinations');
  const created = [];
  try {
    for (const name of DIRECTORIES) { const destination = join(resources, name); mkdirSync(destination, { mode: 0o700 }); created.push(destination); }
    copied(facts.files, source, resources);
    same(inventory(resources, options.host ?? process.platform, facts.files, false), facts.files, 'Imported wire bytes or permissions differ');
    verify(root, manifest, run, options.host ?? process.platform);
    // Descriptor result reuse is only on the original Linux check host; Windows
    // receives identical bytes but retains its own platform/final carrier checks.
    const wireVerified = !!facts.wireResult && (options.host ?? process.platform) === 'linux';
    if (wireVerified) verifyWireResult(facts.wireResult, wireFacts(root, candidate, options.env ?? process.env, run));
    return { candidate, context, files: facts.files.length, fingerprint, wireVerified };
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
    const memberModes = { [`${HANDOFF_ROOT}/handoff.json`]: 0o600,
      ...Object.fromEntries(parsed.files.map((f) => [`${HANDOFF_ROOT}/resources/${f.name}`, f.mode])) };
    const pin = { root: HANDOFF_ROOT, archive: { name, bytes: raw.length, sha256: digest(raw), members, memberModes } };
    writeFileSync(join(destination, 'archive.json'), JSON.stringify(pin, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    return pin;
  } catch (error) { rmSync(destination, { recursive: true, force: true }); throw error; }
}
export function unpackCoreWireHandoff(artifact, destination) {
  regularPath(artifact, true); same(readdirSync(artifact).sort(), ['archive.json', 'core-wire-inputs.tar.gz'], 'Wire transport inventory differs');
  const metadata = join(artifact, 'archive.json'); regularPath(metadata); const pin = JSON.parse(readFileSync(metadata));
  demand(pin.root === HANDOFF_ROOT && pin.archive?.name === 'core-wire-inputs.tar.gz', 'Wire archive identity differs');
  const archive = join(artifact, pin.archive.name); regularPath(archive);
  execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', join(dirname(fileURLToPath(import.meta.url)), 'release-archive.py'), archive, destination],
    { input: JSON.stringify(pin), encoding: 'utf8', maxBuffer: 1024 * 1024 });
  const payload = join(destination, HANDOFF_ROOT);
  const proof = JSON.parse(readFileSync(join(payload, 'handoff.json'), 'utf8'));
  same(pin.archive.memberModes, { [`${HANDOFF_ROOT}/handoff.json`]: 0o600,
    ...Object.fromEntries(proof.files.map((f) => [`${HANDOFF_ROOT}/resources/${f.name}`, f.mode])) }, 'Wire tar modes differ from original receipt');
  return payload;
}
async function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..'), [command, ...args] = process.argv.slice(2);
  const values = Object.fromEntries(args.map((arg) => { const m = /^--(bundle-dir|candidate|artifact-name|wire-result)=(.*)$/.exec(arg); demand(m, 'Unexpected wire handoff argument'); return m.slice(1); }));
  demand(args.length === Object.keys(values).length && values.candidate && values['bundle-dir'], 'Explicit wire handoff identity required');
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json'), 'utf8'));
  if (command === 'wire') {
    demand(!values['artifact-name'] && !values['wire-result'], 'Wire command chooses its own proof');
    if (hasDesktopRelease(manifest)) runCoreWire(root, values.candidate, resolve(values['bundle-dir']));
    else execFileSync('cargo', WIRE_ARGS, { cwd: root, stdio: 'inherit', env: { ...process.env, POLARIS_REQUIRE_KERNEL_GATE: '1' } });
  } else if (command === 'create') {
    demand(!values['artifact-name'], 'Create chooses its own artifact identity');
    let name = '';
    if (hasDesktopRelease(manifest)) {
      const destination = resolve(values['bundle-dir']), temporary = mkdtempSync(join(dirname(destination), '.core-wire-export-'));
      try { const payload = join(temporary, HANDOFF_ROOT); name = createCoreWireHandoff(root, manifest, payload, values.candidate, { wireResult: values['wire-result'] || undefined });
        packCoreWireHandoff(payload, destination); } finally { rmSync(temporary, { recursive: true, force: true }); }
    }
    demand(process.env.GITHUB_OUTPUT, 'Actions output required'); writeFileSync(process.env.GITHUB_OUTPUT, `artifact_name=${name}\n`, { flag: 'a' });
  } else if (command === 'import' || command === 'check' || command === 'download') {
    demand(values['artifact-name'], 'Exact wire handoff artifact required');
    const context = current(root, values.candidate, process.env, execFileSync);
    same(values['artifact-name'], coreWireHandoffName(values.candidate, context), 'Wire handoff artifact identity differs');
    // Download checks inside its own boundary once. Separate check/import
    // invocations still perform their own current-attempt API validation.
    if (command === 'download') { downloadWireArtifact(root, resolve(values['bundle-dir']), values.candidate, context, values['artifact-name']); return; }
    checkWireArtifact(values.candidate, context, values['artifact-name']);
    if (command === 'check') return;
    const artifact = resolve(values['bundle-dir']), temporary = mkdtempSync(join(dirname(artifact), '.core-wire-import-'));
    try { const payload = unpackCoreWireHandoff(artifact, join(temporary, 'unpacked'));
      const result = importCoreWireHandoff(root, manifest, payload, values.candidate, values['artifact-name']);
      console.log(JSON.stringify(result));
      demand(process.env.GITHUB_OUTPUT, 'Actions output required');
      writeFileSync(process.env.GITHUB_OUTPUT, `wire_verified=${result.wireVerified}\n`, { flag: 'a' });
    } finally { rmSync(temporary, { recursive: true, force: true }); }
  } else throw Error('Unknown wire handoff command');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main().catch((error) => { console.error(error.message); process.exitCode = Number.isInteger(error.status) && error.status > 0 && error.status < 256 ? error.status : 1; });
