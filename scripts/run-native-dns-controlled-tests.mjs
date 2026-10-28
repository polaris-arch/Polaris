#!/usr/bin/env node
// Current Cargo artifacts, isolated exact libtest processes. No cached verdicts.
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

// Fixed ordinary source contracts, separate from the six real-core processes.
export const SOURCE_TESTS = Object.freeze([
  "polaris-store:tests::store_tests::unsupported_vision_transport_remains_repairable_across_real_store",
  "polaris:runtime::speedtest::tests::vision_transport_temp_plan_reports_fixed_failure_without_losing_good_nodes",
  "polaris-config-engine:builder::outbound::tests::vision_transport_admission_preserves_supported_native_streams",
  "polaris-config-engine:builder::outbounds::tests::vision_invalid_nodes_preserve_explicit_targets_and_good_auto_candidates",
  "polaris-net-stack:share_link::tests::vision_transport_import_retains_original_nodes_and_safe_warnings",
  "polaris-net-stack:singbox_import::tests::vision_extra_transports_are_retained_with_repairable_warning",
  "polaris-net-stack:singbox_import::tests::naive_json_h2_h3_survive_dedup_serde_and_full_generation",
  "polaris-net-stack:singbox_import::tests::naive_json_quic_flag_is_not_an_unsupported_transport_object",
  "polaris-net-stack:share_link::tests::naive_quic_community_uri_preserves_h3_and_existing_tls_query",
  "polaris-net-stack:share_link::tests::naive_quic_multiline_subscription_keeps_h2_and_h3",
  "polaris-net-stack:subscription::tests::fetch_tests::generated_default_and_custom_ua_reach_real_fetch_init",
  "polaris:commands::subscription::tests::reconcile_tests::naive_json_refresh_reorder_preserves_h3_through_real_store",
  "polaris:commands::subscription::tests::ua_tests::effective_ua_default_and_persisted_override_matrix",
  "polaris:runtime::http::tests::subscription_default_and_override_ua_are_real_wire_headers",
  "polaris:runtime::config::tests::subscription_edit_publishes_owner_only_after_real_storage_success",
  "polaris:commands::subscription::tests::boundary_regressions::edit_supersession_drops_the_actual_awaited_pipeline_and_cancels_providers",
  "polaris:commands::subscription::tests::boundary_regressions::refresh_pipeline_obeys_existing_deadline_without_retrying_or_disabling_owner",
  "polaris:commands::subscription::tests::boundary_regressions::late_refresh_publication_cannot_invalidate_new_saved_edit_intent",
  "polaris:commands::subscription::tests::boundary_regressions::edit_refresh_uses_effective_request_identity_not_metadata_or_requested_ua_text",
  "polaris:runtime::subscription_refresh::tests::saved_edit_supersedes_old_work_without_old_drop_releasing_new_owner",
  "polaris:runtime::subscription_refresh::tests::repeated_edits_and_aba_cannot_publish_old_progress_or_commit",
  "polaris:runtime::subscription_scheduler::tests::superseded_refresh_is_not_a_scheduler_failure_or_announcement",
  "polaris-config-engine:user_config::app_config::tests::utls_default_initialization_and_missing_fields_preserve_explicit_policies",
  "polaris-config-engine:user_config::utls::tests::initial_missing_and_null_policy_follow_saved_fingerprint_in_real_generator",
  "polaris-config-engine:user_config::utls::tests::policy_projection_enters_refresh_and_missing_is_follow",
  "polaris-config-engine:user_config::utls::tests::node_intent_global_policy_protocol_matrix_keeps_stored_nodes_unchanged",
  "polaris-store:tests::store_tests::new_installation_uses_follow_and_preserves_explicit_saved_policies",
  "polaris:runtime::proxy::pending_changes::tests::global_utls_change_creates_runtime_debt_and_missing_restores_follow_projection"
]);
export const BOOTSTRAP_TESTS = Object.freeze([
  'polaris-helper-client:manager::tests::bootstrap_identity::bootstrap_identity_render_is_platform_specific',
  'polaris-helper-client:manager::tests::bootstrap_identity::native_bootstrap_descriptor_identity',
]);
const CONTRACT_MANIFESTS = Object.freeze({
  'polaris': 'src-tauri/Cargo.toml',
  'polaris-helper-client': 'crates/helper-client/Cargo.toml',
  'polaris-store': 'crates/store/Cargo.toml',
  'polaris-config-engine': 'crates/config-engine/Cargo.toml',
  'polaris-net-stack': 'crates/net-stack/Cargo.toml',
});

const demand = (ok, message) => { if (!ok) throw Error(message); };
const hash = (path) => {
  demand(lstatSync(path).isFile() && realpathSync(path) === path, 'Controlled executable is no longer a regular unlinked file');
  return createHash('sha256').update(readFileSync(path)).digest('hex');
};
const inside = (root, path) => { const r = relative(root, path); return r !== '' && !r.startsWith('..') && !isAbsolute(r); };

export function selectExecutable(messages, metadata, root, packageName = 'polaris') {
  demand(Object.hasOwn(CONTRACT_MANIFESTS, packageName), 'Unreviewed test package');
  const packages = metadata.packages.filter((p) => p.name === packageName
    && resolve(p.manifest_path) === join(root, CONTRACT_MANIFESTS[packageName]));
  demand(packages.length === 1, 'Expected the current polaris package manifest');
  const pkg = packages[0];
  const targets = pkg.targets.filter((t) => (packageName !== 'polaris' || t.name === 'polaris_lib') && (t.kind.includes('rlib') || t.kind.includes('lib')));
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
  demand(Array.isArray(artifacts[0].features) && artifacts[0].features.every((value) => typeof value === 'string')
    && new Set(artifacts[0].features).size === artifacts[0].features.length, 'Malformed Cargo artifact feature identity');
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
  const summaries = (typeof result.stdout === 'string' ? result.stdout : '').match(/test result: [^\r\n]+/g) ?? [];
  return result.status === 0 && !result.error && !result.timedOut && !result.cleanupUncertain && !result.interrupted
    && summaries.length === 1 && summaries[0].startsWith('test result: ok. 1 passed; 0 failed; 0 ignored;');
}

function records(output, prefix) {
  if (typeof output !== 'string') return [];
  const lines = output.split(/\r?\n/);
  demand(!lines.at(-1).startsWith(prefix), 'Incomplete controlled protocol line');
  return lines.slice(0, -1).filter((line) => line.startsWith(prefix))
    .map((line) => JSON.parse(line.slice(prefix.length)));
}

export function confirmedCleanup(result, name) {
  if (result.error || result.timedOut || result.cleanupUncertain || result.interrupted
      || !Number.isSafeInteger(result.processId) || result.processId <= 0
      || !Number.isInteger(result.status)) return false;
  try {
    const rows = records(result.stdout, 'POLARIS_CONTROLLED_CLEANUP ');
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
  const counts = [...(typeof result.stdout === 'string' ? result.stdout : '').matchAll(/test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;/g)];
  return counts.length === 1 && Number(counts[0][1]) + Number(counts[0][2]) === 1
    && Number(counts[0][3]) === 0;
}

export function preserveProductionGraph(result, name, directory, executableSha256) {
  const rows = records(result.stdout, 'POLARIS_DEFAULT_GRAPH ');
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
    stdoutSha256: result.stdoutSha256, stdoutBytes: result.stdoutBytes,
    stderrSha256: result.stderrSha256, stderrBytes: result.stderrBytes,
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
    let graph = false, evidenceError = null;
    try { graph = preserveProductionGraph(result, name, directory, digest); }
    catch (cause) { evidenceError = String(cause.message ?? cause).replace(/[\r\n]/g, ' '); }
    const passed = !evidenceError && exactPass(result) && (name !== TESTS[2] || graph);
    appendFileSync(statusPath, `${name}: ${passed ? 'passed' : `failed; owned-child cleanup confirmed${evidenceError ? `; evidence: ${evidenceError}` : ''}`}\n`);
    if (!passed) failures.push(evidenceError ? `${name} (evidence: ${evidenceError})` : name);
  }
  demand(!failures.length, `Controlled business/evidence failures after confirmed cleanup: ${failures.join(', ')}`);
}


export function contractGroups(specs) {
  const reviewed = specs === SOURCE_TESTS || specs === BOOTSTRAP_TESTS;
  demand(reviewed, 'Unreviewed native source test scope');
  const groups = new Map();
  for (const spec of specs) {
    const cut = spec.indexOf(':'), pkg = spec.slice(0, cut), name = spec.slice(cut + 1);
    demand(Object.hasOwn(CONTRACT_MANIFESTS, pkg) && /^[a-zA-Z0-9_]+(?:::[a-zA-Z0-9_]+)+$/.test(name), 'Malformed exact contract');
    const names = groups.get(pkg) ?? [];
    demand(!names.includes(name), 'Duplicate exact contract');
    names.push(name); groups.set(pkg, names);
  }
  return groups;
}

export async function runContractTests(selected, names, env, directory, execute = executeCommand) {
  const digest = selected.sha256 ?? hash(selected.executable);
  for (const name of names) {
    demand(hash(selected.executable) === digest, 'Source test executable changed before invocation');
    const result = await execute({ id: name.split('::').at(-1), program: selected.executable,
      args: [name, '--exact', '--nocapture', '--test-threads=1'] },
    { root: dirname(selected.manifest), env, directory, timeout: 20 * 60_000 });
    demand(hash(selected.executable) === digest, 'Source test executable changed during invocation');
    demand(exactPass(result), `Native source contract did not pass exactly once: ${name}`);
    appendFileSync(join(directory, 'source-contract-status.txt'), `${name}: passed\n`);
  }
}

async function runContractSuite(specs, root) {
  const groups = contractGroups(specs);
  const sourceCommit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8', timeout: 30_000 }).trim();
  demand(/^[0-9a-f]{40}$/.test(sourceCommit) && sourceCommit === process.env.GITHUB_SHA
    && (!process.env.POLARIS_BUILD_ID || sourceCommit === process.env.POLARIS_BUILD_ID),
    'Exact source contract build identity differs from the admitted checkout');
  const directory = join(root, 'native-dns-ci-evidence', specs === SOURCE_TESTS ? 'source-contracts' : 'bootstrap-contracts');
  mkdirSync(directory, { recursive: true });
  const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version=1'],
    { cwd: root, encoding: 'utf8', timeout: 60_000, maxBuffer: 16 * 1024 * 1024 }));
  const sysroot = execFileSync('rustc', ['--print', 'target-libdir'], { encoding: 'utf8', timeout: 30_000 }).trim();
  for (const [packageName, names] of groups) {
    // No feature/profile override: identical default library test profile to the prior cargo invocations.
    const prepared = await executeCommand({ id: `source-build-${packageName}`, program: 'cargo',
      args: ['test', '--locked', '-p', packageName, '--lib', '--no-run', '--message-format=json'] },
    { root, env: process.env, directory, timeout: 20 * 60_000, captureStdout: true });
    demand(prepared.status === 0 && !prepared.error && !prepared.timedOut && !prepared.cleanupUncertain && !prepared.interrupted,
      `Source library test preparation failed: ${packageName}`);
    const messages = prepared.stdout.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
    const selected = selectExecutable(messages, metadata, root, packageName);
    selected.sha256 = hash(selected.executable);
    writeFileSync(join(directory, `${packageName}-executable.json`), JSON.stringify({
      ...selected, sourceCommit, packageName, profile: 'test', featureSelection: 'Cargo defaults; actual features recorded above',
      preparationCalls: 1, tests: names,
    }, null, 2) + '\n');
    const env = runtimeEnvironment(messages, resolve(metadata.target_directory), selected.executable, sysroot);
    await runContractTests(selected, names, env, directory);
  }
}

async function main() {
  const ordinary = process.argv.length === 3 && ({ '--bootstrap-contracts': BOOTSTRAP_TESTS, '--source-contracts': SOURCE_TESTS })[process.argv[2]];
  if (ordinary) {
    const root = resolve(fileURLToPath(new URL('../', import.meta.url)));
    demand(realpathSync(process.cwd()) === realpathSync(root), 'Run native source contracts from the workspace root');
    return runContractSuite(ordinary, root);
  }

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
