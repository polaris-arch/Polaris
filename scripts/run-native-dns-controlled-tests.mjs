#!/usr/bin/env node
// Current Cargo artifacts: source libtests and a separate normal-library native carrier. No cached verdicts.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, renameSync, writeFileSync } from 'node:fs';
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
  'polaris:runtime::proxy::tests::native_carrier::normal_native_carrier_preserves_all_six_original_business_assertion_sets',
  'polaris:runtime::proxy::tests::startup::execution_issuer_does_not_enable_network_takeover_for_manual',
  'polaris:runtime::tls_fingerprints::tests::authenticated_document_is_strict_data_and_cannot_replace_native_channel_binding',
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
const nativeSelections = new WeakMap(), nativeBindings = new WeakMap();
const NATIVE_RESULTS = 'native-runtime-results.json';
const digestBytes = (bytes) => createHash('sha256').update(bytes).digest('hex');
const problem = (error) => String(error?.message ?? error).replace(/[\r\n]/g, ' ').slice(0, 2048);
function boundedBytes(path, limit = 32 * 1024 * 1024) {
  path = resolve(path);
  demand(lstatSync(path).isFile() && realpathSync(path) === path && lstatSync(path).size <= limit,
    'Native evidence must be a bounded regular unlinked file');
  const bytes = readFileSync(path);
  demand(bytes.length <= limit, 'Native evidence grew beyond its bound');
  return bytes;
}
const textBytes = (bytes) => new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
const byteIdentity = (bytes) => ({ bytes: bytes.length, sha256: digestBytes(bytes) });
const selectionIdentity = (selected) => Object.fromEntries([
  'executable', 'packageId', 'manifest', 'features', 'carrier', 'nativeProduction', 'nonTest', 'debugAssertions',
].map((key) => [key, selected[key]]));

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

export function selectNativeExecutable(messages, metadata, root) {
  const packages=metadata.packages.filter((p)=>p.name==='polaris' && resolve(p.manifest_path)===join(root,'src-tauri/Cargo.toml'));
  demand(packages.length===1,'Expected current App package');
  const pkg=packages[0];
  const targets=pkg.targets.filter((t)=>t.name==='native-admission-verifier' && t.kind.length===1 && t.kind[0]==='bin'
    && resolve(t.src_path)===join(root,'src-tauri/src/bin/native-admission-verifier.rs'));
  demand(targets.length===1,'Expected reviewed production verification binary');
  const rows=messages.filter((m)=>m.reason==='compiler-artifact' && m.package_id===pkg.id && m.profile?.test===false && m.profile?.debug_assertions===true
    && m.target?.name===targets[0].name && JSON.stringify(m.target.kind)===JSON.stringify(targets[0].kind)
    && resolve(m.target.src_path)===resolve(targets[0].src_path) && m.executable
    && Array.isArray(m.features) && m.features.includes('native-production-verification'));
  demand(rows.length===1,'Expected one normal-library verification artifact');
  const artifact=rows[0], executable=artifact.executable;
  demand(isAbsolute(executable) && inside(resolve(metadata.target_directory),executable),'Native artifact escaped Cargo target');
  demand(lstatSync(executable).isFile() && realpathSync(executable)===executable,'Native artifact must be an unlinked regular file');
  demand(artifact.features.every((value) => typeof value === 'string')
    && new Set(artifact.features).size === artifact.features.length, 'Malformed native artifact features');
  const selected = {executable,packageId:pkg.id,manifest:pkg.manifest_path,features:[...artifact.features],
    carrier:'normal-production-library',nativeProduction:true,nonTest:true,debugAssertions:true};
  nativeSelections.set(selected, JSON.stringify(selectionIdentity(selected)));
  return selected;
}
export function nativeCaseRecord(result,name) {
  try {
    const rows=records(result.stdout,'POLARIS_NATIVE_CASE ');
    if(rows.length!==1)return null;
    const r=rows[0];
    return r.schema==='polaris-native-production-case-v1' && r.case===name && r.processId===result.processId
      && Number.isSafeInteger(r.processId) && r.processId>0 && r.carrier==='normal-production-library'
      && ['passed','failed'].includes(r.result) && r.validationCustodyDrained===true && r.publicationEligible===false ? r:null;
  } catch{return null;}
}
export function exactNativePass(result,name) {
  return result.status===0 && !result.error && !result.timedOut && !result.cleanupUncertain && !result.interrupted
    && nativeCaseRecord(result,name)?.result==='passed';
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

function productionGraphRecord(result, name) {
  const rows = records(result.stdout, 'POLARIS_DEFAULT_GRAPH ');
  if (!rows.length) return null;
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
  return r;
}

export function preserveProductionGraph(result, name, directory, executableSha256) {
  const r = productionGraphRecord(result, name);
  if (!r) return false;
  const bytes = Buffer.from(r.configJson, 'utf8'), graph = JSON.parse(r.configJson);
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

function nativeArtifact(root, directory) {
  const metadataBytes = boundedBytes(join(directory, 'controlled-cargo-metadata.json'), 16 * 1024 * 1024);
  const buildBytes = boundedBytes(join(directory, 'controlled-build.stdout.log'));
  const metadata = JSON.parse(textBytes(metadataBytes));
  const messages = textBytes(buildBytes).split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  const selected = selectNativeExecutable(messages, metadata, root);
  selected.sha256 = hash(selected.executable);
  const executableBytes = boundedBytes(join(directory, 'controlled-executable.json'), 1024 * 1024);
  demand(JSON.stringify(JSON.parse(textBytes(executableBytes))) === JSON.stringify({ ...selected, preparationCalls: 1 }),
    'Controlled executable receipt differs from current normal Cargo artifact');
  return { selected, files: { metadata: byteIdentity(metadataBytes), build: byteIdentity(buildBytes), executable: byteIdentity(executableBytes) } };
}

function nativeBinding(root, directory, env, run) {
  root = resolve(root);
  const options = { cwd: root, encoding: 'utf8', timeout: 30_000, maxBuffer: 8 * 1024 * 1024, env: { ...env } };
  for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete options.env[key];
  const git = (args) => String(run('git', args, options));
  const sourceCommit = git(['rev-parse', 'HEAD']).trim(), sourceTree = git(['rev-parse', 'HEAD^{tree}']).trim();
  demand(/^[a-f0-9]{40}$/.test(sourceCommit) && /^[a-f0-9]{40}$/.test(sourceTree)
    && sourceCommit === (env.CORE_CANDIDATE || env.GITHUB_SHA)
    && (!env.POLARIS_BUILD_ID || env.POLARIS_BUILD_ID === sourceCommit)
    && git(['status', '--porcelain', '--untracked-files=no']).trim() === '', 'Native summary source checkout differs');
  const sourceHashes = {};
  for (const path of ['scripts/run-native-dns-controlled-tests.mjs', '.github/workflows/ci.yml']) {
    const bytes = boundedBytes(join(root, path));
    demand(Buffer.from(git(['show', `${sourceCommit}:${path}`]), 'utf8').equals(bytes), 'Native summary source bytes differ');
    sourceHashes[path] = digestBytes(bytes);
  }
  demand(sourceHashes['scripts/run-native-dns-controlled-tests.mjs'] === digestBytes(boundedBytes(fileURLToPath(import.meta.url))),
    'Consumer was imported from another source checkout');
  const contextBytes = boundedBytes(join(directory, 'context.json'), 1024 * 1024);
  const context = JSON.parse(textBytes(contextBytes)), c = context?.context;
  const attempt = typeof c?.attempt === 'object' ? c.attempt?.attempt : c?.attempt;
  const runId = c?.runId ?? c?.attempt?.runId;
  demand(c && c.repository === 'polaris-arch/Polaris' && c.repositoryId === '1347028220'
    && env.GITHUB_REPOSITORY === c.repository && env.GITHUB_REPOSITORY_ID === c.repositoryId
    && c.candidate === sourceCommit && (!c.tree || c.tree === sourceTree)
    && /^[1-9][0-9]*$/.test(runId ?? '') && runId === env.GITHUB_RUN_ID
    && /^[1-9][0-9]*$/.test(attempt ?? '') && attempt === env.GITHUB_RUN_ATTEMPT
    && typeof env.GITHUB_WORKFLOW_REF === 'string'
    && ['ci.yml', 'package.yml'].some((name) => env.GITHUB_WORKFLOW_REF === `${c.repository}/.github/workflows/${name}@${env.GITHUB_REF}`)
    && (!c.workflowRef || c.workflowRef === env.GITHUB_WORKFLOW_REF) && (!c.ref || c.ref === env.GITHUB_REF)
    && ['linux', 'win', 'mac-x64', 'mac-arm64'].includes(context.platform) && context.platform === env.CORE_PLATFORM
    && context.selection === (env.POLARIS_NATIVE_DNS_FORMAL === '1' ? 'formal-default'
      : env.POLARIS_NATIVE_DNS_CANDIDATE === '1' ? 'manual-candidate' : 'temporary-default')
    && /^[a-f0-9]{40}$/.test(context.kernelSourceCommit ?? '') && /^[a-f0-9]{40}$/.test(context.kernelSourceTree ?? '')
    && (context.selection !== 'formal-default' || /^[a-f0-9]{64}$/.test(c.contractSha256 ?? '')),
  'Native summary candidate/run/platform/selection context differs');
  demand(typeof env.POLARIS_CORE_IDENTITY_FILE === 'string' && env.POLARIS_CORE_IDENTITY_FILE.length > 0,
    'Compiled Core manifest authority is missing');
  const compiledBytes = boundedBytes(resolve(root, env.POLARIS_CORE_IDENTITY_FILE), 1024 * 1024);
  const compiled = JSON.parse(textBytes(compiledBytes));
  demand(compiled?.schema === 'polaris-core-manifest-v1' && /^[a-f0-9]{64}$/.test(compiled.id ?? '')
    && compiled.core?.platform === context.platform && Array.isArray(compiled.core.files) && compiled.core.files.length > 0,
  'Compiled Core manifest authority is invalid');
  demand(boundedBytes(join(directory, 'compiled-core-manifest.json'), 1024 * 1024).equals(compiledBytes),
    'Preserved compiled Core manifest differs');
  return { repository: c.repository, repositoryId: c.repositoryId, candidate: sourceCommit, sourceTree,
    runId, attempt, workflowRef: env.GITHUB_WORKFLOW_REF, ref: env.GITHUB_REF, platform: context.platform,
    selection: context.selection, kernelSourceCommit: context.kernelSourceCommit, kernelSourceTree: context.kernelSourceTree,
    sourceHashes, context: byteIdentity(contextBytes), compiledManifest: { ...byteIdentity(compiledBytes), id: compiled.id } };
}

// Only a validated Cargo selection can obtain producer authority. The executor's
// PID/status are preserved separately from the native process's protocol claims.
export function bindNativeRuntime(selected, root, env, directory, run = execFileSync) {
  demand(nativeSelections.get(selected) === JSON.stringify(selectionIdentity(selected)), 'Native selection is not validated');
  demand(typeof env.POLARIS_CORE_IDENTITY_FILE === 'string' && env.POLARIS_CORE_IDENTITY_FILE.length > 0,
    'Compiled Core manifest authority is missing');
  writeFileSync(join(directory, 'compiled-core-manifest.json'), boundedBytes(resolve(root, env.POLARIS_CORE_IDENTITY_FILE), 1024 * 1024));
  const artifact = nativeArtifact(root, directory);
  demand(JSON.stringify({ ...selectionIdentity(selected), sha256: selected.sha256 })
    === JSON.stringify({ ...selectionIdentity(artifact.selected), sha256: artifact.selected.sha256 }), 'Native selection changed');
  const binding = { ...nativeBinding(root, directory, env, run), files: artifact.files };
  nativeBindings.set(selected, { binding, selection: JSON.stringify(selectionIdentity(selected)), sha256: selected.sha256 });
  return structuredClone(binding);
}

function saveNativeResults(directory, receipt) {
  const bytes = JSON.stringify(receipt, null, 2) + '\n';
  demand(Buffer.byteLength(bytes) <= 2 * 1024 * 1024, 'Native result receipt exceeded its bound');
  const path = join(directory, NATIVE_RESULTS);
  writeFileSync(`${path}.tmp`, bytes, { mode: 0o600 });
  renameSync(`${path}.tmp`, path);
}
const nativeFields = (record) => record && Object.fromEntries([
  'schema', 'case', 'processId', 'result', 'carrier', 'validationCustodyDrained', 'publicationEligible',
].map((key) => [key, record[key]]));
function preserveExecutor(row, result, name) {
  row.executor = Object.fromEntries(['processId', 'status', 'error', 'timedOut', 'cleanupUncertain', 'interrupted']
    .map((key) => [key, result[key] ?? null]));
  row.streams = Object.fromEntries([['log', 'log'], ['stdout', 'stdout'], ['stderr', 'stderr']].map(([key, prefix]) =>
    [key, { file: `${name.split('::').at(-1)}${key === 'log' ? '' : `.${key}`}.log`,
      bytes: result[`${prefix}Bytes`] ?? null, sha256: result[`${prefix}Sha256`] ?? null }]));
  row.state = 'cleanup-unknown';
}

function readCaseResult(row, name, directory, executableSha256) {
  const e = row.executor;
  demand(e && (e.processId === null || Number.isSafeInteger(e.processId) && e.processId > 0)
    && (e.status === null || Number.isInteger(e.status)) && (e.error === null || typeof e.error === 'string')
    && ['timedOut', 'cleanupUncertain', 'interrupted'].every((key) => typeof e[key] === 'boolean'), 'Incomplete executor authority');
  demand(row.executableBeforeSha256 === executableSha256 && row.executableAfterSha256 === executableSha256,
    'Per-case executable identity differs or was not confirmed');
  const result = { ...e };
  for (const key of ['log', 'stdout', 'stderr']) {
    const expectedFile = `${name.split('::').at(-1)}${key === 'log' ? '' : `.${key}`}.log`, s = row.streams?.[key];
    demand(s?.file === expectedFile && Number.isSafeInteger(s.bytes) && s.bytes >= 0 && /^[a-f0-9]{64}$/.test(s.sha256 ?? ''),
      'Missing or foreign native stream identity');
    const bytes = boundedBytes(join(directory, expectedFile));
    demand(bytes.length === s.bytes && digestBytes(bytes) === s.sha256, 'Native stream bytes/hash differ');
    if (key !== 'log') result[key] = textBytes(bytes);
    result[`${key}Bytes`] = bytes.length; result[`${key}Sha256`] = s.sha256;
  }
  return result;
}
function confirmedGraph(result, name, directory, executableSha256) {
  const r = productionGraphRecord(result, name);
  if (!r) return name !== TESTS[2];
  const bytes = boundedBytes(join(directory, 'default-production-graph.json'), 4 * 1024 * 1024);
  const receipt = JSON.parse(textBytes(boundedBytes(join(directory, 'default-production-graph.receipt.json'), 1024 * 1024)));
  demand(bytes.equals(Buffer.from(r.configJson, 'utf8')) && receipt.sha256 === r.sha256 && receipt.bytes === r.bytes
    && receipt.processId === result.processId && receipt.testName === name && receipt.executableSha256 === executableSha256
    && receipt.stdoutSha256 === result.stdoutSha256 && receipt.stdoutBytes === result.stdoutBytes
    && receipt.stderrSha256 === result.stderrSha256 && receipt.stderrBytes === result.stderrBytes && receipt.appAccepted === false,
  'Native graph receipt binding differs');
  return true;
}

// Read-only consumer: reselect the current Cargo artifact and re-run the same
// native/cleanup/graph decoders over hash-bound original streams. No libtest fallback.
export function nativeRuntimeSummary(directory, env, root = process.cwd(), run = execFileSync) {
  const tests = TESTS.map((name) => ({ name, state: 'unexecuted', cleanup: 'not-started', invoked: false,
    log: join(directory, `${name.split('::').at(-1)}.stdout.log`) }));
  const summary = { state: 'unexecuted', requirementState: 'open', scope: 'Six exact normal-production-library lifecycle, hot-switch and stats cases; no SYSTEM or packaged acceptance',
    nativeProduction: false, nonTest: false, tests, AppAccepted: false, packagedAcceptance: false, publicationEligible: false };
  if (!existsSync(join(directory, NATIVE_RESULTS))) return { ...summary, evidenceState: 'missing' };
  try {
    const receipt = JSON.parse(textBytes(boundedBytes(join(directory, NATIVE_RESULTS), 2 * 1024 * 1024)));
    const artifact = nativeArtifact(resolve(root), directory);
    const binding = { ...nativeBinding(root, directory, env, run), files: artifact.files };
    demand(receipt.schema === 'polaris-native-production-runtime-v1' && JSON.stringify(receipt.binding) === JSON.stringify(binding)
      && JSON.stringify(receipt.executable) === JSON.stringify({ ...selectionIdentity(artifact.selected), sha256: artifact.selected.sha256 })
      && ['in-progress', 'completed', 'aborted'].includes(receipt.phase)
      && Array.isArray(receipt.cases) && JSON.stringify(receipt.cases.map((row) => row.name)) === JSON.stringify(TESTS),
    'Native runtime receipt belongs to another source/candidate/run/artifact or case scope');
    Object.assign(summary, { nativeProduction: true, nonTest: true, binding, executable: receipt.executable,
      evidenceState: 'bound', phase: receipt.phase, failure: receipt.failure });
    for (const [index, row] of receipt.cases.entries()) {
      const view = tests[index], name = TESTS[index];
      try {
        if (row.state === 'unexecuted') {
          demand(row.executor === null && row.streams === null && row.executableBeforeSha256 === null
            && row.executableAfterSha256 === null && row.failure === null, 'Unexecuted native row contains execution facts');
          continue;
        }
        demand(['started', 'passed', 'failed', 'cleanup-unknown'].includes(row.state), 'Unknown native case state');
        Object.assign(view, { invoked: true, state: 'cleanup-unknown', cleanup: 'unknown', failure: row.failure });
        if (!row.executor) continue;
        const result = readCaseResult(row, name, directory, artifact.selected.sha256);
        view.processId = result.processId; view.status = result.status;
        view.executor = { ...row.executor };
        view.streams = row.streams;
        const clean = confirmedCleanup(result, name), record = nativeCaseRecord(result, name);
        view.cleanup = clean ? 'confirmed' : 'unknown'; view.nativeResult = record?.result ?? null;
        view.nativeRecord = nativeFields(record);
        if (!clean) continue;
        const graph = confirmedGraph(result, name, directory, artifact.selected.sha256);
        view.graphRequired = name === TESTS[2]; view.graphVerified = view.graphRequired ? graph : null;
        view.state = row.state === 'passed' && !row.failure && graph && exactNativePass(result, name) ? 'passed' : 'failed';
      } catch (error) { Object.assign(view, { state: 'invalid-evidence', failure: problem(error) }); }
    }
    const passed = receipt.phase === 'completed' && receipt.failure === null && env.RUNTIME_OUTCOME === 'success'
      && tests.every((row) => row.state === 'passed');
    summary.state = passed ? 'passed' : tests.some((row) => row.invoked || row.state === 'invalid-evidence') ? 'failed-or-incomplete' : 'unexecuted';
    summary.requirementState = passed ? 'satisfied' : 'open';
    return summary;
  } catch (error) {
    return { ...summary, state: 'failed-or-incomplete', evidenceState: 'invalid', failure: problem(error),
      tests: tests.map((row) => ({ ...row, state: 'invalid-evidence', cleanup: 'unknown' })) };
  }
}

export async function runTests(selected, env, directory, execute = executeCommand) {
  const digest = selected.sha256 ?? hash(selected.executable);
  const statusPath = join(directory, 'runtime-status.txt');
  const failures = [];
  const authority = selected.carrier === 'normal-production-library' ? nativeBindings.get(selected) : null;
  if (selected.carrier === 'normal-production-library') demand(authority && authority.sha256 === digest
    && authority.selection === JSON.stringify(selectionIdentity(selected)), 'Normal runtime requires bound validated selection');
  const receipt = authority ? { schema: 'polaris-native-production-runtime-v1', binding: authority.binding,
    executable: { ...selectionIdentity(selected), sha256: digest }, phase: 'in-progress', failure: null,
    cases: TESTS.map((name) => ({ name, state: 'unexecuted', executor: null, streams: null,
      executableBeforeSha256: null, executableAfterSha256: null, failure: null })) } : null;
  const save = () => { if (receipt) saveNativeResults(directory, receipt); };
  save();
  let current = null;
  try {
  for (const name of TESTS) {
    demand(hash(selected.executable) === digest, 'Controlled executable changed before a test');
    current = receipt?.cases[TESTS.indexOf(name)] ?? null;
    if (current) { current.state = 'started'; current.executableBeforeSha256 = digest; save(); }
    appendFileSync(statusPath, `${name}: started\n`);
    const result = await execute({ id: name.split('::').at(-1), program: selected.executable,
      args: selected.carrier === 'normal-production-library' ? [name] : [name, '--exact', '--ignored', '--nocapture', '--test-threads=1'] },
    { root: dirname(selected.manifest), env, directory, timeout: 20 * 60_000 });
    if (current) { preserveExecutor(current, result, name); save(); }
    demand(hash(selected.executable) === digest, 'Controlled executable changed during a test');
    if (current) {
      current.executableAfterSha256 = digest;
      current.cleanupConfirmed = confirmedCleanup(result, name);
      current.nativeRecord = nativeFields(nativeCaseRecord(result, name));
      current.state = current.cleanupConfirmed ? 'failed' : 'cleanup-unknown';
      save();
    }
    demand(confirmedCleanup(result, name), `Controlled owned-child cleanup is unconfirmed: ${name}`);
    demand(selected.carrier === 'normal-production-library' ? nativeCaseRecord(result,name) : oneExecutedTest(result), `Controlled invocation did not execute exactly one reviewed case: ${name}`);
    let graph = false, evidenceError = null;
    try { graph = preserveProductionGraph(result, name, directory, digest); }
    catch (cause) { evidenceError = String(cause.message ?? cause).replace(/[\r\n]/g, ' '); }
    const passed = !evidenceError && (selected.carrier === 'normal-production-library' ? exactNativePass(result,name) : exactPass(result)) && (name !== TESTS[2] || graph);
    if (current) { current.state = passed ? 'passed' : 'failed'; current.failure = evidenceError; current.graphVerified = graph; save(); }
    appendFileSync(statusPath, `${name}: ${passed ? 'passed' : `failed; owned-child cleanup confirmed${evidenceError ? `; evidence: ${evidenceError}` : ''}`}\n`);
    if (!passed) failures.push(evidenceError ? `${name} (evidence: ${evidenceError})` : name);
    current = null;
  }
  if (receipt) { receipt.phase = 'completed'; save(); }
  demand(!failures.length, `Controlled business/evidence failures after confirmed cleanup: ${failures.join(', ')}`);
  } catch (error) {
    if (receipt) {
      receipt.phase = 'aborted'; receipt.failure = problem(error);
      if (current) { current.failure ||= receipt.failure; if (current.state === 'started') current.state = 'cleanup-unknown'; }
      save();
    }
    throw error;
  }
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
    args: ['build', '--locked', '-p', 'polaris', '--features', 'native-production-verification', '--bin', 'native-admission-verifier', '--message-format=json'] },
  { root, env: process.env, directory, timeout: 20 * 60_000, captureStdout: true });
  demand(prepared.status === 0 && !prepared.error && !prepared.cleanupUncertain && !prepared.interrupted, 'Controlled production verification preparation failed');
  const messages = prepared.stdout.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line));
  const selected = selectNativeExecutable(messages, metadata, root);
  selected.sha256 = hash(selected.executable);
  writeFileSync(join(directory, 'controlled-cargo-metadata.json'), JSON.stringify(metadata) + '\n');
  writeFileSync(join(directory, 'controlled-executable.json'), JSON.stringify({ ...selected, preparationCalls: 1 }, null, 2) + '\n');
  bindNativeRuntime(selected, root, process.env, directory);
  const sysroot = execFileSync('rustc', ['--print', 'target-libdir'], { encoding: 'utf8', timeout: 30_000 }).trim();
  const env = runtimeEnvironment(messages, resolve(metadata.target_directory), selected.executable, sysroot);
  // Cargo runs libtests in the package directory, rather than the workspace root.
  await runTests(selected, env, directory);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
