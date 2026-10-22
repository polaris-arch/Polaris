#!/usr/bin/env node
// One current Cargo artifact, six isolated libtest processes. No cached verdicts.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, lstatSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { homedir } from 'node:os';
import { executeCommand } from './desktop-core/native-dns-gates.mjs';

export const TESTS = Object.freeze([
  'runtime::proxy::tests::lifecycle::real_core_full_lifecycle',
  'runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop',
  'runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid',
  'runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt',
  'runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation',
  'runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames',
]);
const demand = (ok, message) => { if (!ok) throw Error(message); };
const hash = (path) => {
  demand(lstatSync(path).isFile() && realpathSync(path) === path, 'Controlled executable is no longer a regular unlinked file');
  return createHash('sha256').update(readFileSync(path)).digest('hex');
};
const inside = (root, path) => { const r = relative(root, path); return r !== '' && !r.startsWith('..') && !isAbsolute(r); };

export function selectExecutable(messages, metadata, root) {
  const packages = metadata.packages.filter((p) => p.name === 'polaris'
    && resolve(p.manifest_path) === join(root, 'src-tauri/Cargo.toml'));
  demand(packages.length === 1, 'Expected the current polaris package manifest');
  const pkg = packages[0];
  const targets = pkg.targets.filter((t) => t.name === 'polaris_lib' && t.kind.includes('rlib'));
  demand(targets.length === 1, 'Expected the polaris_lib library target');
  const target = targets[0];
  const artifacts = messages.filter((m) => m.reason === 'compiler-artifact' && m.package_id === pkg.id
    && m.profile?.test === true && m.executable && m.target?.name === target.name
    && JSON.stringify(m.target.kind) === JSON.stringify(target.kind)
    && resolve(m.target.src_path) === resolve(target.src_path));
  demand(artifacts.length === 1, 'Expected exactly one current polaris library test executable');
  const executable = artifacts[0].executable;
  demand(isAbsolute(executable) && inside(resolve(metadata.target_directory), executable), 'Test executable escaped the current Cargo target directory');
  demand(lstatSync(executable).isFile() && realpathSync(executable) === executable, 'Test executable must be a regular unlinked file');
  return { executable, packageId: pkg.id, features: artifacts[0].features, manifest: pkg.manifest_path };
}

export function runtimeEnvironment(messages, targetDirectory, executable, sysroot, env = process.env, platform = process.platform) {
  const key = platform === 'win32' ? 'PATH' : platform === 'darwin' ? 'DYLD_FALLBACK_LIBRARY_PATH' : 'LD_LIBRARY_PATH';
  const oldKey = platform === 'win32' ? Object.keys(env).find((k) => k.toUpperCase() === 'PATH') ?? key : key;
  const inherited = env[oldKey] ?? (platform === 'darwin' ? [join(homedir(), 'lib'), '/usr/local/lib', '/usr/lib'].join(':') : '');
  const linked = messages.filter((m) => m.reason === 'build-script-executed').flatMap((m) => m.linked_paths ?? [])
    .map((p) => p.replace(/^(?:native|dependency|framework|all)=/, ''))
    .filter((p) => isAbsolute(p) && inside(targetDirectory, p));
  const output = dirname(executable);
  const result = { ...env };
  delete result[oldKey];
  result[key] = [...new Set([...linked, output, dirname(output), sysroot, ...(inherited ? inherited.split(platform === 'win32' ? ';' : ':') : [])])]
    .join(platform === 'win32' ? ';' : ':');
  return result;
}

export function exactPass(result) {
  const summaries = result.output.match(/test result: [^\r\n]+/g) ?? [];
  return result.status === 0 && !result.error && !result.timedOut && !result.cleanupUncertain && !result.interrupted
    && summaries.length === 1 && summaries[0].startsWith('test result: ok. 1 passed; 0 failed; 0 ignored;');
}

function records(output, prefix) {
  return output.split(/\r?\n/).filter((line) => line.startsWith(prefix))
    .map((line) => JSON.parse(line.slice(prefix.length)));
}

export function confirmedCleanup(result, name) {
  if (result.error || result.timedOut || result.cleanupUncertain || result.interrupted
      || !Number.isSafeInteger(result.processId) || result.processId <= 0
      || !Number.isInteger(result.status)) return false;
  try {
    const rows = records(result.output, 'POLARIS_CONTROLLED_CLEANUP ');
    if (rows.length !== 1) return false;
    const r = rows[0], c = r.custody;
    if (r.schema !== 'polaris-controlled-cleanup-v1' || r.testName !== name
        || r.processId !== result.processId || r.shutdownFenced !== true
        || !Number.isSafeInteger(r.generation) || r.generation < 0
        || r.publicationEligible !== false || !c) return false;
    if (c.state === 'empty-after-shutdown-fence') return c.runRef === null && c.corePid === null;
    if (!['retained-child-exit-observed', 'emergency-owned-child-reaped'].includes(c.state)
        || !/^direct-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(c.runRef)) return false;
    return (Number.isSafeInteger(c.corePid) && c.corePid > 0)
      || (c.corePid === null && c.state === 'retained-child-exit-observed');
  } catch { return false; }
}

function oneExecutedTest(result) {
  const counts = [...result.output.matchAll(/test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;/g)];
  return counts.length === 1 && Number(counts[0][1]) + Number(counts[0][2]) === 1
    && Number(counts[0][3]) === 0;
}

export function preserveProductionGraph(result, name, directory, executableSha256) {
  const rows = records(result.output, 'POLARIS_DEFAULT_GRAPH ');
  if (!rows.length) return false;
  demand(rows.length === 1, 'Duplicate controlled production graph record');
  const r = rows[0];
  demand(name === TESTS[2] && r.schema === 'polaris-controlled-production-graph-v1'
    && r.processId === result.processId && r.stage === 'initial-start'
    && r.source === 'actual-runtime-config-path' && r.dashboardEnabled === true
    && typeof r.startSucceeded === 'boolean' && typeof r.configJson === 'string'
    && r.publicationEligible === false, 'Controlled production graph provenance mismatch');
  const bytes = Buffer.from(r.configJson, 'utf8');
  demand(bytes.length > 0 && bytes.length <= 4 * 1024 * 1024 && bytes.length === r.bytes
    && createHash('sha256').update(bytes).digest('hex') === r.sha256,
  'Controlled production graph bytes/hash mismatch');
  const graph = JSON.parse(r.configJson), apis = graph.services?.filter((s) => s.type === 'api');
  demand(apis?.length === 1 && typeof r.dashboardPath === 'string'
    && apis[0].dashboard?.path === r.dashboardPath, 'Production default dashboard was not retained');
  demand(Array.isArray(graph.inbounds) && graph.inbounds.length > 0
    && graph.inbounds.every((i) => ['mixed', 'http', 'socks'].includes(i.type) && i.listen === '127.0.0.1'),
  'Controlled production graph must remain loopback/manual');
  const path = join(directory, 'default-production-graph.json');
  writeFileSync(path, bytes, { flag: 'wx', mode: 0o600 });
  writeFileSync(join(directory, 'default-production-graph.receipt.json'), JSON.stringify({
    ...r, configJson: undefined, testName: name, executableSha256, graphFile: 'default-production-graph.json',
    outboundCount: graph.outbounds?.length ?? null, appAccepted: false,
  }, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  return true;
}

export async function runTests(selected, env, directory, execute = executeCommand) {
  const digest = selected.sha256 ?? hash(selected.executable);
  const statusPath = join(directory, 'runtime-status.txt');
  const failures = [];
  for (const name of TESTS) {
    demand(hash(selected.executable) === digest, 'Controlled executable changed before a test');
    appendFileSync(statusPath, `${name}: started\n`);
    const result = await execute({ id: name.split('::').at(-1), program: selected.executable,
      args: [name, '--exact', '--ignored', '--nocapture', '--test-threads=1'] },
    { root: dirname(selected.manifest), env, directory, timeout: 20 * 60_000 });
    demand(hash(selected.executable) === digest, 'Controlled executable changed during a test');
    demand(confirmedCleanup(result, name), `Controlled owned-child cleanup is unconfirmed: ${name}`);
    demand(oneExecutedTest(result), `Controlled invocation did not execute exactly one reviewed test: ${name}`);
    const graph = preserveProductionGraph(result, name, directory, digest);
    const passed = exactPass(result) && (name !== TESTS[2] || graph);
    appendFileSync(statusPath, `${name}: ${passed ? 'passed' : 'failed; owned-child cleanup confirmed'}\n`);
    if (!passed) failures.push(name);
  }
  demand(!failures.length, `Controlled business/evidence failures after confirmed cleanup: ${failures.join(', ')}`);
}

async function main() {
  demand(JSON.stringify(process.argv.slice(2)) === JSON.stringify(TESTS), 'Controlled test scope/order differs from the six reviewed exact names');
  const root = resolve(fileURLToPath(new URL('../', import.meta.url)));
  demand(realpathSync(process.cwd()) === realpathSync(root), 'Run the controlled tests from the workspace root');
  const directory = join(root, 'native-dns-ci-evidence');
  mkdirSync(directory, { recursive: true });
  const statusPath = join(directory, 'runtime-status.txt');
  writeFileSync(statusPath, '');
  const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version=1'],
    { cwd: root, encoding: 'utf8', timeout: 60_000, maxBuffer: 16 * 1024 * 1024 }));
  const prepared = await executeCommand({ id: 'controlled-build', program: 'cargo',
    args: ['test', '--locked', '-p', 'polaris', '--lib', '--no-run', '--message-format=json'] },
  { root, env: process.env, directory, timeout: 20 * 60_000, captureStdout: true });
  demand(prepared.status === 0 && !prepared.error && !prepared.cleanupUncertain && !prepared.interrupted, 'Controlled library test preparation failed');
  const messages = prepared.stdout.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  const selected = selectExecutable(messages, metadata, root);
  selected.sha256 = hash(selected.executable);
  writeFileSync(join(directory, 'controlled-executable.json'), JSON.stringify({ ...selected, preparationCalls: 1 }, null, 2) + '\n');
  const sysroot = execFileSync('rustc', ['--print', 'target-libdir'], { encoding: 'utf8', timeout: 30_000 }).trim();
  const env = runtimeEnvironment(messages, resolve(metadata.target_directory), selected.executable, sysroot);
  // Cargo runs libtests in the package directory, rather than the workspace root.
  await runTests(selected, env, directory);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
