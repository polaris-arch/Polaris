import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { basename, join } from 'node:path';
import test, { after } from 'node:test';
import { candidateContext, loadCandidate, hasTemporaryDesktop, loadTemporaryDesktopSelection, selectedCandidateContext } from './desktop-core/native-dns-candidate.mjs';
import { regularPath } from './desktop-core/release-consumer.mjs';
import { releaseGoVersion } from './desktop-core/source-graph.mjs';
import { TARGETS, loadFormalGateSelection, formalGateContext } from './desktop-core/native-dns-gates.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const workflow = readFileSync(join(root, '.github/workflows/ci.yml'), 'utf8');
const read = (path) => readFileSync(join(root, path), 'utf8');
// Preserve the old default/manual combination without treating the formal
// checkout's active manifest as temporary or copying unrelated repository files.
const currentManifest = JSON.parse(read('src-tauri/core-manifest.json'));
// Resolve only our owned fixture root: macOS may spell the temp ancestor as
// /var while its physical path is /private/var. Production readers still reject
// any linked artifact path or ancestor.
function historicalSelectionRoot(parent = tmpdir()) {
  return realpathSync(mkdtempSync(join(parent, 'historical-ci-selection-')));
}
const temporaryRoot = historicalSelectionRoot();
after(() => rmSync(temporaryRoot, { recursive: true, force: true }));
mkdirSync(join(temporaryRoot, 'src-tauri'), { recursive: true });
mkdirSync(join(temporaryRoot, 'scripts/desktop-core'), { recursive: true });
for (const name of ['native-dns-candidate.json', 'native-dns-source-binding.json'])
  copyFileSync(join(root, 'scripts/desktop-core', name), join(temporaryRoot, 'scripts/desktop-core', name));
writeFileSync(join(temporaryRoot, 'src-tauri/core-manifest.json'), JSON.stringify({
  desktopTemporary: currentManifest.historicalDesktopTemporary,
  historicalDesktopRelease: currentManifest.historicalDesktopRelease,
}));
const readTemporary = (path) => readFileSync(join(temporaryRoot, path), 'utf8');
const candidateWorkflow = read('.github/workflows/native-dns-candidate.yml');
const sha = 'a'.repeat(40);
const base = { GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REPOSITORY: 'polaris-arch/Polaris',
  GITHUB_REPOSITORY_ID: '1347028220', GITHUB_REF: 'refs/heads/collab/native-node-dns-20261018', GITHUB_SHA: sha,
  GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/ci.yml@refs/heads/collab/native-node-dns-20261018',
  GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1', GITHUB_OUTPUT: 'output', GITHUB_ENV: 'environment' };
function step(name) {
  const start = workflow.indexOf(`      - name: ${name}\n`);
  assert.notEqual(start, -1, `Missing workflow step: ${name}`);
  const end = workflow.indexOf('\n      - ', start + 1);
  return workflow.slice(start, end === -1 ? undefined : end);
}
function nodeScript(name) {
  const match = /node --input-type=module - <<'NODE'\n([\s\S]*?)\n          NODE/.exec(step(name));
  assert.ok(match, `Missing inline Node script: ${name}`);
  return match[1].replace(/^          /gm, '').replace(/^import .*;\n/gm, '');
}
function runAdmission(input = '', extra = {}, observed = { head: sha, status: '' }, temporaryDefault = false) {
  const writes = [];
  const env = { ...base, NATIVE_DNS_INPUT: input, ...extra };
  const run = new Function('process', 'appendFileSync', 'readFileSync', 'candidateContext', 'loadCandidate',
    'hasTemporaryDesktop', 'loadTemporaryDesktopSelection', 'selectedCandidateContext', 'loadFormalGateSelection', 'formalGateContext', 'releaseGoVersion',
    nodeScript('Admit explicit native DNS ordinary CI selection'));
  const directory = temporaryDefault ? temporaryRoot : root;
  const git = (_command, args) => args[0] === 'show' ? readFileSync(join(directory, args[1].slice('HEAD:'.length)), 'utf8')
    : args[0] === 'rev-parse' ? `${args[1] === 'HEAD' ? observed.head : 'b'.repeat(40)}\n` : observed.status;
  run({ env, cwd: () => directory }, (path, value) => writes.push({ path, value }),
    () => temporaryDefault ? readTemporary('src-tauri/core-manifest.json') : '{}',
    (candidate, checked, directory) => candidateContext(candidate, checked, directory, git), loadCandidate,
    hasTemporaryDesktop, loadTemporaryDesktopSelection, (directory, checked) => selectedCandidateContext(directory, checked, git),
    loadFormalGateSelection, formalGateContext, releaseGoVersion);
  return writes;
}
function section(source, name) {
  const start = source.indexOf(`  ${name}:\n`);
  assert.notEqual(start, -1, `Missing workflow section: ${name}`);
  const remaining = source.slice(start + `  ${name}:\n`.length);
  const end = remaining.search(/^(?:  [\w-]+:|\S)/m);
  return remaining.slice(0, end === -1 ? undefined : end);
}
function reusableContract(caller = workflow, callee = candidateWorkflow) {
  const job = section(caller, 'native_dns_candidate_gates');
  assert.match(job, /^    needs: cheap_format$/m);
  assert.match(job, /^    uses: \.\/\.github\/workflows\/native-dns-candidate\.yml$/m);
  assert.match(job, /^    permissions:\n      contents: read$/m);
  assert.ok(job.includes('      candidate: ${{ needs.cheap_format.outputs.native_dns_candidate || needs.cheap_format.outputs.native_dns_formal_app }}'));
  assert.ok(job.includes("      formal_release: ${{ needs.cheap_format.outputs.native_dns_formal == 'true' }}"));
  assert.doesNotMatch(job, /\b(?:secrets|runs-on|steps|env|strategy|environment):|(?:actions|id-token):|contents: write/);
  for (const trigger of ['workflow_dispatch', 'workflow_call']) {
    const inputs = section(callee, trigger);
    assert.deepEqual([...inputs.matchAll(/^      ([\w-]+):/gm)].map((m) => m[1]), trigger === 'workflow_call' ? ['candidate', 'formal_release'] : ['candidate']);
    assert.match(inputs, /        type: string\n        required: true\n        default: ''/);
    assert.doesNotMatch(inputs, /secrets:/);
  }
  assert.match(callee, /^permissions:\n  contents: read$/m);
  assert.equal((callee.match(/permissions:/g) ?? []).length, 1);
  assert.equal((callee.match(/uses: actions\/checkout@/g) ?? []).length, 1);
  assert.match(callee, /uses: actions\/checkout@v7\n        with:\n          ref: \$\{\{ inputs\.candidate \}\}\n          persist-credentials: false/);
  assert.match(callee, /^  CORE_CANDIDATE: \$\{\{ inputs\.candidate \}\}$/m);
  assert.deepEqual([...callee.slice(callee.indexOf('on:\n'), callee.indexOf('\npermissions:'))
    .matchAll(/^  ([\w-]+):/gm)].map((m) => m[1]), ['workflow_dispatch', 'workflow_call']);
  const acquisition = '      - name: Acquire four exact temporary desktop payloads\n'
    + '        if: ${{ inputs.formal_release != true }}\n'
    + '        env:\n          POLARIS_RELEASE_API_TOKEN: ${{ github.token }}\n'
    + '        run: node scripts/desktop-core/native-dns-candidate.mjs acquire --candidate="$CORE_CANDIDATE"';
  assert.ok(callee.includes(acquisition), 'token belongs only to exact acquisition step');
  const formalAcquisition = '      - name: Acquire four committed strict formal desktop payloads\n'
    + '        if: ${{ inputs.formal_release == true }}\n'
    + '        env:\n          POLARIS_RELEASE_API_TOKEN: ${{ github.token }}\n'
    + '        run: node scripts/fetch-core.mjs';
  assert.ok(callee.includes(formalAcquisition));
  assert.doesNotMatch(callee.replace(acquisition, '').replace(formalAcquisition, ''), /github\.token|POLARIS_RELEASE_API_TOKEN/);
  assert.doesNotMatch(callee, /secrets:|secrets\.|contents: write|actions: write|id-token:|cargo (?:build|test) --workspace|run: pnpm/);
}

test('RuntimeConfig source mocks remain independent of payload admission and use the exact package test', () => {
  const job = section(workflow, 'runtime_config_source_tests');
  assert.doesNotMatch(job, /^    needs:/m);
  assert.doesNotMatch(job, /fetch-core|prepare-core-identity|POLARIS_REQUIRE_KERNEL_GATE|POLARIS_NATIVE_DNS_CANDIDATE|--release|continue-on-error/);
  const commands = [...job.matchAll(/^        run: (cargo .+)$/gm)].map((match) => match[1]);
  assert.deepEqual(commands, ['cargo test --locked -p polaris-singbox-grpc --test runtime_config']);
});

test('manual inputs remain opt-in while the committed default preserves the ordinary DAG, matrix and exact workspace commands', () => {
  assert.equal((workflow.match(/^      native_dns_candidate:\n/gm) ?? []).length, 2);
  for (const entry of workflow.matchAll(/^      native_dns_candidate:\n([\s\S]*?)(?=^      \w|^\S)/gm))
    assert.match(entry[1], /type: string\n        default: ''/);
  assert.deepEqual([...workflow.matchAll(/^  (\w+):\n/gm)].filter((m) => m.index > workflow.indexOf('\njobs:\n')).map((m) => m[1]),
    ['cheap_format', 'native_dns_candidate_gates', 'lint', 'cross', 'test', 'runtime_config_source_tests', 'windows_launcher_diagnostics']);
  for (const job of ['lint', 'cross', 'test']) assert.ok(workflow.includes(`  ${job}:\n    needs: cheap_format\n`));
  const testJobHeader = workflow.slice(workflow.indexOf('  test:\n')).split('    strategy:')[0];
  assert.match(testJobHeader, /    permissions:\n      contents: read\n$/);
  assert.doesNotMatch(testJobHeader, /contents: write|actions: write|id-token:/);
  const matrices = [...workflow.matchAll(/        os: >-\n([\s\S]*?\}\})/g)];
  assert.equal(matrices.length, 2);
  assert.equal(matrices[0][1], matrices[1][1]);
  for (const command of ['cargo build --workspace --verbose', 'cargo test --workspace --no-fail-fast'])
    assert.equal(workflow.split(`run: ${command}`).length - 1, 1, command);
  assert.ok(step('Test').includes('shell: node scripts/check-coex-runtime-export.mjs capture {0}'));
  for (const command of ['cargo fmt --all -- --check', 'cargo clippy --workspace --all-targets --keep-going -- -D warnings',
    'cargo doc --no-deps --workspace --bins --lib --document-private-items', 'scripts/fetch-protoc.mjs',
    "NASM_VERSION: '3.02'", 'libwebkit2gtk-4.1-dev']) assert.ok(workflow.includes(command), command);
});

test('same-commit reusable route has required string inputs, read-only permissions and exact checkout', () => {
  reusableContract();
  const cases = [
    ['caller', 'uses: ./.github/workflows/native-dns-candidate.yml', 'uses: ./.github/workflows/native-dns-candidate.yml@main'],
    ['caller', 'uses: ./.github/workflows/native-dns-candidate.yml', 'uses: other/repo/.github/workflows/native-dns-candidate.yml@main'],
    ['caller', 'candidate: ${{ needs.cheap_format.outputs.native_dns_candidate || needs.cheap_format.outputs.native_dns_formal_app }}', 'candidate: ${{ inputs.native_dns_candidate }}'],
    ['caller', '    uses: ./.github/workflows/native-dns-candidate.yml', '    secrets: inherit\n    uses: ./.github/workflows/native-dns-candidate.yml'],
    ['caller', '    uses: ./.github/workflows/native-dns-candidate.yml', '    runs-on: ubuntu-22.04\n    uses: ./.github/workflows/native-dns-candidate.yml'],
    ['callee', 'type: string', 'type: boolean'],
    ['callee', 'required: true', 'required: false'],
    ['callee', "default: ''", `default: '${sha}'`],
    ['callee', 'contents: read', 'contents: write'],
    ['callee', 'POLARIS_RELEASE_API_TOKEN: ${{ github.token }}', 'GITHUB_TOKEN: ${{ github.token }}'],
    ['callee', '  CORE_CANDIDATE:', '  POLARIS_RELEASE_API_TOKEN: ${{ github.token }}\n  CORE_CANDIDATE:'],
    ['callee', 'ref: ${{ inputs.candidate }}', 'ref: main'],
    ['callee', 'persist-credentials: false', 'persist-credentials: true'],
    ['callee', 'CORE_CANDIDATE: ${{ inputs.candidate }}', 'CORE_CANDIDATE: ${{ github.event.inputs.candidate }}'],
    ['callee', 'on:\n', 'on:\n  push:\n'],
  ];
  for (const [target, before, after] of cases) {
    const source = target === 'caller' ? workflow : candidateWorkflow;
    assert.ok(source.includes(before), before);
    const changed = source.replaceAll(before, after);
    assert.throws(() => reusableContract(target === 'caller' ? changed : workflow,
      target === 'callee' ? changed : candidateWorkflow), `${target}: ${after}`);
  }
  for (const [before, after] of [['needs: cheap_format', 'needs: lint'], ['contents: read', 'contents: write']]) {
    const job = section(workflow, 'native_dns_candidate_gates');
    assert.throws(() => reusableContract(workflow.replace(job, job.replace(before, after))));
  }
});

test('reusable caller requires explicit admitted manual CI input and leaves ordinary jobs independent', () => {
  const job = section(workflow, 'native_dns_candidate_gates');
  const expression = /\$\{\{ ([\s\S]*?) \}\}/.exec(job)[1];
  const route = (input = sha, admitted = sha, event = 'workflow_dispatch', workflowRef = base.GITHUB_WORKFLOW_REF) =>
    new Function('github', 'inputs', 'needs', `return (${expression});`)(
      { event_name: event, workflow_ref: workflowRef }, { native_dns_candidate: input },
      { cheap_format: { outputs: { native_dns_candidate: admitted } } });
  assert.equal(route(), true);
  for (const [input, admitted] of [['', ''], ['', sha], [sha, ''], [sha, 'b'.repeat(40)]])
    assert.equal(route(input, admitted), false);
  for (const event of ['pull_request', 'push', 'workflow_call', 'merge_group'])
    assert.equal(route(sha, sha, event), false);
  for (const ref of [base.GITHUB_WORKFLOW_REF.replace('ci.yml', 'package.yml'),
    base.GITHUB_WORKFLOW_REF.replace('ci.yml', 'native-dns-candidate.yml'),
    base.GITHUB_WORKFLOW_REF.replace('collab/native-node-dns-20261018', 'main'), ''])
    assert.equal(route(sha, sha, 'workflow_dispatch', ref), false);
  // Admission must succeed before either path. A default-skipped optional callee
  // must not skip the unchanged workspace gate DAG, or duplicate its commands.
  for (const name of ['lint', 'cross', 'test']) {
    assert.match(section(workflow, name), /^    needs: cheap_format$/m);
    assert.doesNotMatch(section(workflow, name), /needs\.native_dns_candidate_gates/);
  }
});

test('historical selection remains unselected and admission rejects empty or zero inherited mode keys', () => {
  assert.deepEqual(runAdmission(), []);
  for (const input of ['', sha]) for (const key of ['CORE_CANDIDATE', 'POLARIS_NATIVE_DNS_CANDIDATE', 'POLARIS_NATIVE_PAYLOAD_VALIDATION', 'POLARIS_NATIVE_DNS_FORMAL'])
    for (const value of ['', '0', '1']) assert.throws(() => runAdmission(input, { [key]: value }), /must not precede admission/);
  assert.deepEqual(runAdmission(sha), [{ path: 'output', value: `candidate=${sha}\n` }]);
  for (const value of ['', '/original/candidate', '/other/core'])
    assert.throws(() => runAdmission(sha, { POLARIS_SINGBOX_PATH: value }), /must not inherit/);
  assert.deepEqual(runAdmission('', { POLARIS_SINGBOX_PATH: '/historical/fixture' }), []);
  for (const input of ['0', '1', 'false', sha.toUpperCase(), 'b'.repeat(40)]) assert.throws(() => runAdmission(input));
});

test('PR, package, wrong branch, wrong workflow, wrong run and local execution cannot admit a candidate', () => {
  for (const extra of [{ GITHUB_EVENT_NAME: 'pull_request' }, { GITHUB_EVENT_NAME: 'push' },
    { GITHUB_REF: 'refs/heads/main' }, { GITHUB_ACTIONS: 'false' }, { GITHUB_REPOSITORY_ID: '1' },
    { GITHUB_RUN_ID: '0' }, { GITHUB_RUN_ATTEMPT: '0' },
    { GITHUB_WORKFLOW_REF: base.GITHUB_WORKFLOW_REF.replace('ci.yml', 'package.yml') },
    { POLARIS_NO_KERNEL_RUN: '1' }]) assert.throws(() => runAdmission(sha, extra));
  assert.throws(() => runAdmission(sha, {}, { head: 'b'.repeat(40), status: '' }), /checkout differs/);
  assert.throws(() => runAdmission(sha, {}, { head: sha, status: ' M src-tauri/build.rs' }), /checkout differs/);
  assert.deepEqual(runAdmission('', { GITHUB_EVENT_NAME: 'pull_request' }), []);
  assert.deepEqual(runAdmission('', { GITHUB_EVENT_NAME: 'push', GITHUB_WORKFLOW_REF: base.GITHUB_WORKFLOW_REF.replace('ci.yml', 'package.yml') }), []);
});

test('native host activation follows successful context admission and matches the actual Rust triple', () => {
  const source = nodeScript('Activate admitted native DNS test inputs for the actual host');
  for (const [platform, target] of Object.entries(TARGETS)) {
    const writes = [];
    const triple = { linux: 'x86_64-unknown-linux-gnu', win: 'x86_64-pc-windows-msvc',
      'mac-x64': 'x86_64-apple-darwin', 'mac-arm64': 'aarch64-apple-darwin' }[platform];
    let admitted = false;
    const run = (host = triple) => new Function('process', 'execFileSync', 'appendFileSync', 'mkdirSync', 'readFileSync', 'writeFileSync',
      'resolve', 'candidateContext', 'loadCandidate', 'hasTemporaryDesktop', 'loadTemporaryDesktopSelection', 'selectedCandidateContext', 'TARGETS', 'loadFormalGateSelection', 'formalGateContext', 'releaseGoVersion', source)(
      { env: { ...base, ADMITTED_CANDIDATE: sha, ADMITTED_DEFAULT: '' }, platform: target.host, arch: target.arch, cwd: () => root },
      (command) => command === 'rustc' ? `host: ${host}\n` : '',
      (path, value) => { assert.equal(admitted, true); writes.push({ path, value }); }, () => {}, () => '{}', () => {},
      (...parts) => join(root, ...parts), (candidate, env, directory) => {
        const result = candidateContext(candidate, env, directory, (_command, args) => args[0] === 'rev-parse' ? sha : '');
        admitted = true; return result;
      }, loadCandidate, hasTemporaryDesktop, loadTemporaryDesktopSelection, selectedCandidateContext, TARGETS, loadFormalGateSelection, formalGateContext, releaseGoVersion);
    run();
    const active = writes.filter((w) => w.path === 'environment').map((w) => w.value).join('');
    assert.ok(active.includes(`CORE_PLATFORM=${platform}\n`));
    assert.ok(active.includes(`POLARIS_KERNEL_GATE_TARGET=${target.target}\n`));
    assert.ok(active.includes(`POLARIS_TEST_CORE=${join(root, target.core)}\n`));
    assert.ok(!active.includes("POLARIS_SINGBOX_PATH="), "candidate App tests must use the compiled carrier resolver");
    assert.ok(active.includes('POLARIS_REQUIRE_KERNEL_GATE=1\n'));
    assert.ok(writes.some((w) => w.value.includes('go_version=1.27.2')));
    writes.length = 0;
    assert.throws(() => run('wrong-target'), /Rust host differs/);
    assert.deepEqual(writes, []);
  }
});

test('candidate acquisition precedes placeholders and keeps desktop archive membership exact', () => {
  const selected = step('Acquire all four explicit candidate payloads before placeholders');
  assert.ok(selected.includes('node scripts/fetch-core.mjs --native-dns-candidate="$CORE_CANDIDATE"'));
  assert.match(selected, /        env:\n          POLARIS_RELEASE_API_TOKEN: \$\{\{ github\.token \}\}/);
  assert.doesNotMatch(workflow.replace(selected, ''), /POLARIS_RELEASE_API_TOKEN|github\.token/);
  assert.ok(selected.includes("needs.cheap_format.outputs.native_dns_candidate != ''"));
  const testJob = workflow.slice(workflow.indexOf('  test:\n'));
  assert.ok(testJob.indexOf('Acquire all four explicit candidate payloads') < testJob.indexOf('Create resource placeholder dirs'));
  const placeholder = step('Create resource placeholder dirs (tauri build.rs 路径校验)');
  // This name also occurs in lint; inspect the candidate-aware test-job occurrence.
  assert.ok(placeholder.length > 0);
  assert.match(testJob, /if \[ -n "\$\{CORE_CANDIDATE:-\}" \]; then\n[^\n]*\n\s+mkdir -p resources\/dashboard && touch resources\/dashboard\/\.keep\n\s+else/);
  assert.ok(step('Setup candidate archive verifier Python').includes("python-version: '3.12'"));
  assert.ok(step('Setup verified candidate Go metadata reader').includes('steps.native_dns_host.outputs.go_version'));
  assert.ok(step('Verify exact candidate Go toolchain').includes('go env GOVERSION'));
});

test('actual helper bytes and isolated candidate identity are frozen before the unchanged App build', () => {
  const identity = step('Build actual candidate helper bytes and freeze the separate App identity');
  assert.ok(identity.includes('export POLARIS_BUILD_ID="$CORE_CANDIDATE"'));
  assert.ok(identity.includes('cargo build --locked -p polaris-helper --bin polaris-helper'));
  assert.ok(identity.includes('cargo build --locked -p polaris-windows-cleaner --bin polaris-cleaner'));
  assert.ok(identity.includes('--directory="$POLARIS_CORE_CARRIER_DIR"'));
  assert.ok(identity.includes('cp "target/debug/$helper" "$POLARIS_CORE_CARRIER_DIR/$helper"'));
  assert.ok(identity.includes('cp target/debug/polaris-cleaner.exe "$POLARIS_CORE_CARRIER_DIR/polaris-cleaner.exe"'));
  assert.ok(identity.indexOf('codesign --verify --strict') < identity.indexOf('node scripts/prepare-core-identity.mjs'));
  assert.ok(nodeScript('Activate admitted native DNS test inputs for the actual host').includes("temporaryDefault ? 'resources' : 'resources/.native-dns-app'"));
  assert.ok(identity.indexOf('export POLARIS_BUILD_ID=') < identity.indexOf('cargo build --locked'));
  assert.ok(identity.indexOf('node scripts/prepare-core-identity.mjs') < identity.indexOf('appendFileSync(process.env.GITHUB_ENV'));
  assert.ok(workflow.indexOf('id: native_dns_identity') < workflow.indexOf('id: workspace_build'));
  assert.ok(workflow.indexOf('id: native_dns_before') < workflow.indexOf('id: workspace_build'));
  assert.ok(workflow.indexOf('id: native_dns_after') > workflow.indexOf('id: native_dns_app_runtime'));
  const before = step('Verify all candidate payloads and compiled identity before workspace build');
  const after = step('Verify all candidate payloads after workspace and controlled App tests');
  for (const verification of [before, after]) {
    assert.ok(verification.includes('native-dns-candidate.mjs verify "${selection[@]}"'));
    assert.ok(verification.includes('selection=(--candidate="$CORE_CANDIDATE")'));
    assert.ok(verification.includes('selection=()'));
    assert.ok(verification.includes('scripts/prepare-core-identity.mjs'));
    assert.ok(verification.includes('--mode=verify'));
    assert.ok(verification.includes('set -euo pipefail'));
  }
  assert.ok(after.includes('always()'));
  assert.doesNotMatch(identity, /CARGO_TARGET_DIR/);
  assert.ok(step('Record ordinary candidate outcomes without claiming skipped acceptance').includes('compiled-core-manifest.json'));
});

test('controlled real-core tests resolve exact library namespaces and never select all ignored tests', () => {
  const selected = step('Run exact controlled App lifecycle hot-switch and stats tests');
  const names = [...selected.matchAll(/^            (runtime::[^\s]+)$/gm)].map((m) => m[1]);
  assert.equal(names.length, 6);
  assert.equal(new Set(names).size, 6);
  assert.match(read('src-tauri/Cargo.toml'), /\[package\]\nname = "polaris"/);
  assert.match(read('src-tauri/Cargo.toml'), /\[lib\]\nname = "polaris_lib"/);
  assert.match(read('src-tauri/src/lib.rs'), /mod runtime;/);
  for (const qualified of names) {
    const parts = qualified.split('::'), name = parts.pop(), owner = parts[1], module = parts.at(-1);
    assert.match(read('src-tauri/src/runtime.rs'), new RegExp(`mod ${owner};`));
    assert.match(read(`src-tauri/src/runtime/${owner}.rs`), /mod tests;/);
    assert.match(read(`src-tauri/src/runtime/${owner}/tests/mod.rs`), new RegExp(`mod ${module};`));
    assert.match(read(`src-tauri/src/${parts.join('/')}.rs`), new RegExp(`#\\[ignore = [^\\n]+\\]\\nasync fn ${name}\\(`));
  }
  assert.ok(selected.includes('node scripts/run-native-dns-controlled-tests.mjs "${tests[@]}"'));
  assert.ok(read('scripts/run-native-dns-controlled-tests.mjs').includes("'--exact', '--ignored', '--nocapture', '--test-threads=1'"));
  assert.doesNotMatch(selected, /RUNNER_OS|continue|ps\/pgrep/);
  assert.doesNotMatch(selected, /--bin polaris|cargo test[^\n]* -- --ignored/);
});

test('Windows separate producer uses the verified candidate and leaves existing SYSTEM PR scope intact', () => {
  const selected = step('Select explicit formal or candidate Windows directory source');
  assert.ok(selected.includes('loadCandidate(process.cwd()).sourceCommit'));
  assert.ok(selected.includes('Separate Windows source must match the admitted candidate'));
  for (const name of ['Approved batch SYSTEM producer — fresh', 'Approved batch SYSTEM producer — prebuilt-user']) {
    const system = step(name);
    assert.ok(system.includes("github.event_name == 'pull_request'"));
    assert.ok(system.includes("github.event.pull_request.head.ref == 'collab/dots-audit-remediation-20261017'"));
    assert.ok(system.includes('github.event.pull_request.number == 15'));
    assert.ok(system.includes("github.event.pull_request.head.ref == 'collab/apple-evidence-reuse-20261018'"));
    assert.doesNotMatch(system.split('        shell:')[0], /native_dns_candidate|native-node-dns/);
    assert.ok(system.includes('inputs.windows_system_candidate == github.sha'));
    assert.ok(system.includes("$env:RUNNER_ENVIRONMENT -cne 'github-hosted'"));
  }
  assert.ok(step('Record ordinary candidate outcomes without claiming skipped acceptance').includes("scope: 'system-producer-user-consumer-bootstrap-only'"));
  const evidence = step('Always preserve exact ordinary candidate evidence');
  assert.ok(evidence.includes('always()'));
  assert.ok(evidence.includes('github.sha'));
  assert.ok(evidence.includes('github.run_id'));
  assert.ok(evidence.includes('github.run_attempt'));
  assert.ok(evidence.includes('if-no-files-found: error'));
});


test('candidate real-core callers use default compiled-carrier resolution before instance admission', () => {
  const proxy = read('src-tauri/src/runtime/proxy/tests/mod.rs');
  const stats = read('src-tauri/src/runtime/stats/tests/real_core_tests.rs');
  const require = proxy.slice(proxy.indexOf('pub(crate) fn require_core()'), proxy.indexOf('fn real_core_runtime()'));
  assert.ok(require.includes('candidate_real_core_selection('));
  assert.ok(require.includes('crate_file("core-manifest.json")'));
  assert.ok(require.includes('repo_file("scripts/desktop-core/release-contract.json")'));
  assert.doesNotMatch(require, /include_(?:str|bytes)!/);
  assert.ok(require.includes('std::env::var_os("POLARIS_SINGBOX_PATH")'));
  assert.ok(require.includes('resolve_core_binary().expect('));
  assert.ok(require.includes('../resources/.native-dns-app'));
  assert.match(require, /assert_eq!\(\s*path,\s*expected,/);
  assert.ok(proxy.includes('rt.inject_real_core_for_test(core.clone())'));
  assert.ok(stats.includes('let core = crate::runtime::proxy::tests::require_core();'));
  assert.ok(stats.includes('proxy.inject_real_core_for_test(core)'));
  assert.doesNotMatch(stats, /std::env::var\("POLARIS_SINGBOX_PATH"\)/);
  assert.ok(!nodeScript('Activate admitted native DNS test inputs for the actual host').includes('POLARIS_SINGBOX_PATH:'));
});


test('committed temporary default is admitted without manual branch inputs or environment selectors', () => {
  const clean = { head: sha, status: '' };
  assert.equal(loadTemporaryDesktopSelection(temporaryRoot).sourceCommit, 'fe182431495711c09bdb3b074ed6aa6309c00cf8');
  assert.equal(hasTemporaryDesktop(currentManifest), false);
  assert.deepEqual(runAdmission(sha, {}, clean, true), [{ path: 'output', value: `candidate=${sha}\n` }],
    'manual admission remains available beside the historical temporary default');
  for (const [event, ref] of [['push', 'refs/heads/main'], ['pull_request', 'refs/pull/42/merge'],
    ['workflow_dispatch', 'refs/heads/main'], ['merge_group', 'refs/heads/gh-readonly-queue/main/pr-42']]) {
    const env = { GITHUB_EVENT_NAME: event, GITHUB_REF: ref,
      GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/ci.yml@${ref}` };
    assert.deepEqual(runAdmission('', env, clean, true), [{ path: 'output', value: 'temporary_default=true\n' }]);
  }
  for (const key of ['CORE_CANDIDATE', 'POLARIS_NATIVE_DNS_CANDIDATE', 'POLARIS_NATIVE_PAYLOAD_VALIDATION', 'POLARIS_NATIVE_DNS_FORMAL'])
    for (const value of ['', '0', '1']) assert.throws(() => runAdmission('', { [key]: value }, clean, true), /must not precede admission/);
  for (const value of ['', '/other/core']) assert.throws(() => runAdmission('', { POLARIS_SINGBOX_PATH: value }, clean, true), /must not inherit/);
  const acquisition = step('Acquire all four explicit candidate payloads before placeholders');
  assert.ok(acquisition.includes('node scripts/fetch-core.mjs 2>&1'));
  for (const name of ['Setup candidate archive verifier Python', 'Setup verified candidate Go metadata reader',
    'Verify exact candidate Go toolchain', 'Acquire all four explicit candidate payloads before placeholders',
    'Build actual candidate helper bytes and freeze the separate App identity',
    'Verify all candidate payloads and compiled identity before workspace build',
    'Run exact controlled App lifecycle hot-switch and stats tests',
    'Verify all candidate payloads after workspace and controlled App tests',
    'Record ordinary candidate outcomes without claiming skipped acceptance', 'Always preserve exact ordinary candidate evidence'])
    assert.ok(step(name).includes("needs.cheap_format.outputs.native_dns_default == 'true'"), name);
  assert.doesNotMatch(step('Setup candidate admission Node'), /if:/);
});

test('default native activation uses the real bundled carrier and cannot select legacy hidden layout through environment alone', () => {
  const source = nodeScript('Activate admitted native DNS test inputs for the actual host');
  for (const [platform, target] of Object.entries(TARGETS)) {
    const writes = [], triple = { linux: 'x86_64-unknown-linux-gnu', win: 'x86_64-pc-windows-msvc',
      'mac-x64': 'x86_64-apple-darwin', 'mac-arm64': 'aarch64-apple-darwin' }[platform];
    let admitted = false;
    const run = (extra = {}, host = triple) => new Function('process', 'execFileSync', 'appendFileSync', 'mkdirSync', 'readFileSync', 'writeFileSync',
      'resolve', 'candidateContext', 'loadCandidate', 'hasTemporaryDesktop', 'loadTemporaryDesktopSelection', 'selectedCandidateContext', 'TARGETS', 'loadFormalGateSelection', 'formalGateContext', 'releaseGoVersion', source)(
      { env: { ...base, ADMITTED_CANDIDATE: '', ADMITTED_DEFAULT: 'true', ...extra }, platform: target.host, arch: target.arch, cwd: () => temporaryRoot },
      (command) => command === 'rustc' ? `host: ${host}\n` : '',
      (path, value) => { assert.equal(admitted, true); writes.push({ path, value }); }, () => {}, () => readTemporary('src-tauri/core-manifest.json'), () => {},
      (...parts) => join(temporaryRoot, ...parts), () => { throw Error('manual selector used for default'); }, loadCandidate,
      hasTemporaryDesktop, loadTemporaryDesktopSelection, (directory, env) => {
        const result = selectedCandidateContext(directory, env,
          (_command, args) => args[0] === 'show' ? readTemporary(args[1].slice('HEAD:'.length))
            : args[0] === 'rev-parse' ? args[1] === 'HEAD' ? sha : 'b'.repeat(40) : '');
        admitted = true; return result;
      }, TARGETS, loadFormalGateSelection, formalGateContext, releaseGoVersion);
    run();
    const active = writes.filter((w) => w.path === 'environment').map((w) => w.value).join('');
    assert.ok(active.includes(`POLARIS_CORE_CARRIER_DIR=${join(temporaryRoot, 'resources', platform)}\n`));
    assert.ok(active.includes(`CORE_CANDIDATE=${sha}\n`));
    assert.doesNotMatch(active, /POLARIS_NATIVE_DNS_CANDIDATE=|POLARIS_NATIVE_PAYLOAD_VALIDATION=|POLARIS_SINGBOX_PATH=/);
    for (const extra of [{ ADMITTED_CANDIDATE: sha }, { ADMITTED_DEFAULT: '' }, { ADMITTED_DEFAULT: '0' }]) {
      writes.length = 0; assert.throws(() => run(extra), /admi/); assert.deepEqual(writes, []);
    }
    writes.length = 0; assert.throws(() => run({}, 'wrong-target'), /Rust host differs/); assert.deepEqual(writes, []);
  }
});


test('ordinary temporary Windows directory source follows the committed contract with no new SYSTEM admission', () => {
  function assertDefaultSelection(selected) {
    assert.match(selected, /ContainsKey\('desktopTemporary'\)/);
    assert.match(selected, /selectedCandidateContext\(process\.cwd\(\)\)/);
    assert.match(selected, /context\.mode!=="temporary-default"/);
    assert.match(selected, /loadTemporaryDesktopSelection\(process\.cwd\(\)\)\.sourceCommit/);
    assert.match(selected, /Separate Windows source must match the committed default/);
    assert.match(selected, /\$env:DIRECTORY_MODE = 'candidate'/);
    assert.match(selected, /if \(\$LASTEXITCODE -ne 0\) \{ throw 'Verified committed default source unavailable' \}/);
  }
  const selected = step('Select explicit formal or candidate Windows directory source');
  assertDefaultSelection(selected);
  for (const [from, to] of [['selectedCandidateContext(process.cwd())', '{}'],
    ['loadTemporaryDesktopSelection(process.cwd()).sourceCommit', 'process.env.CANDIDATE_SOURCE'],
    ['context.mode!=="temporary-default"', 'false'],
    ['Separate Windows source must match the committed default', 'ignored'],
    ["$env:DIRECTORY_MODE = 'candidate'", "$env:DIRECTORY_MODE = 'formal'"]]) {
    assert.ok(selected.includes(from)); assert.throws(() => assertDefaultSelection(selected.replaceAll(from, to)));
  }
});

test('committed formal admission enables the same ordinary preparation and lifecycle commands', () => {
  const manifest = { desktopRelease: { contractPath: 'scripts/desktop-core/release-contract.json' } };
  const contract = { sourceCommit: 'c'.repeat(40), sourceTree: 'd'.repeat(40) };
  const context = { mode: 'formal-default', candidate: sha, kernelSourceCommit: contract.sourceCommit };
  const writes = [];
  const noTemporary = () => { throw Error('Historical temporary selector must not run for formal default'); };
  const run = new Function('process', 'appendFileSync', 'readFileSync', 'candidateContext', 'loadCandidate',
    'hasTemporaryDesktop', 'loadTemporaryDesktopSelection', 'selectedCandidateContext', 'loadFormalGateSelection',
    'formalGateContext', 'releaseGoVersion', nodeScript('Admit explicit native DNS ordinary CI selection'));
  run({ env: { ...base, NATIVE_DNS_INPUT: '' }, cwd: () => root },
    (path, value) => writes.push({ path, value }), () => JSON.stringify(manifest), noTemporary, noTemporary,
    hasTemporaryDesktop, noTemporary, noTemporary, () => contract, () => context, () => 'go1.27.2');
  assert.deepEqual(writes, [{ path: 'output', value: `formal_default=true\nformal_app=${sha}\n` }]);
  const job = section(workflow, 'native_dns_candidate_gates');
  assert.ok(job.includes("needs.cheap_format.outputs.native_dns_formal == 'true'"));
  assert.ok(job.includes('needs.cheap_format.outputs.native_dns_formal_app'));
  for (const name of ['Setup candidate archive verifier Python', 'Setup verified candidate Go metadata reader',
    'Verify exact candidate Go toolchain', 'Acquire all four explicit candidate payloads before placeholders',
    'Build actual candidate helper bytes and freeze the separate App identity',
    'Verify all candidate payloads and compiled identity before workspace build',
    'Run exact controlled App lifecycle hot-switch and stats tests',
    'Verify all candidate payloads after workspace and controlled App tests',
    'Record ordinary candidate outcomes without claiming skipped acceptance', 'Always preserve exact ordinary candidate evidence'])
    assert.ok(step(name).includes("needs.cheap_format.outputs.native_dns_formal == 'true'"), name);
  for (const name of ['Verify all candidate payloads and compiled identity before workspace build',
    'Verify all candidate payloads after workspace and controlled App tests'])
    assert.ok(step(name).includes('native-dns-gates.mjs verify-formal --platform="$CORE_PLATFORM"'));
  assert.ok(step('Select explicit formal or candidate Windows directory source').includes('formalGateContext(process.cwd()).kernelSourceCommit'));
});

test('formal host activation derives compiler and source from the validated contract and keeps the compiled default carrier', () => {
  const source = nodeScript('Activate admitted native DNS test inputs for the actual host');
  const contract = { sourceCommit: 'c'.repeat(40), sourceTree: 'd'.repeat(40) };
  const manifest = { desktopRelease: {} };
  for (const [platform, target] of Object.entries(TARGETS)) {
    const writes = [], evidence = [];
    const triple = { linux: 'x86_64-unknown-linux-gnu', win: 'x86_64-pc-windows-msvc',
      'mac-x64': 'x86_64-apple-darwin', 'mac-arm64': 'aarch64-apple-darwin' }[platform];
    const noTemporary = () => { throw Error('Historical temporary selector must not run for formal default'); };
    let admitted = false;
    const run = (extra = {}, host = triple) => new Function('process', 'execFileSync', 'appendFileSync', 'mkdirSync', 'readFileSync', 'writeFileSync',
      'resolve', 'candidateContext', 'loadCandidate', 'hasTemporaryDesktop', 'loadTemporaryDesktopSelection', 'selectedCandidateContext', 'TARGETS',
      'loadFormalGateSelection', 'formalGateContext', 'releaseGoVersion', source)(
      { env: { ...base, ADMITTED_CANDIDATE: '', ADMITTED_DEFAULT: '', ADMITTED_FORMAL: 'true', ...extra }, platform: target.host, arch: target.arch, cwd: () => root },
      (command) => command === 'rustc' ? `host: ${host}\n` : '',
      (path, value) => { assert.equal(admitted, true); writes.push({ path, value }); }, () => {},
      () => JSON.stringify(manifest), (_path, value) => evidence.push(JSON.parse(value)),
      (...parts) => join(root, ...parts), noTemporary, noTemporary, hasTemporaryDesktop, noTemporary, noTemporary, TARGETS,
      () => contract, () => { admitted = true; return { mode: 'formal-default', candidate: sha }; },
      (selected) => { assert.equal(selected, contract); return 'go1.27.2'; });
    run();
    const active = writes.filter((w) => w.path === 'environment').map((w) => w.value).join('');
    assert.ok(active.includes('POLARIS_NATIVE_DNS_FORMAL=1\n'));
    assert.ok(active.includes(`POLARIS_CORE_CARRIER_DIR=${join(root, 'resources', platform)}\n`));
    assert.doesNotMatch(active, /POLARIS_NATIVE_DNS_CANDIDATE=|POLARIS_NATIVE_PAYLOAD_VALIDATION=|POLARIS_SINGBOX_PATH=/);
    assert.ok(writes.some((w) => w.value.includes('go_version=1.27.2')));
    assert.equal(evidence[0].kernelSourceCommit, contract.sourceCommit);
    assert.equal(evidence[0].selection, 'formal-default');
    for (const extra of [{ ADMITTED_FORMAL: '' }, { ADMITTED_FORMAL: '0' }, { ADMITTED_DEFAULT: 'true' },
      { ADMITTED_CANDIDATE: sha }, { POLARIS_NATIVE_DNS_FORMAL: '1' }, { POLARIS_SINGBOX_PATH: '' }]) {
      writes.length = 0; assert.throws(() => run(extra)); assert.deepEqual(writes, []);
    }
    writes.length = 0; assert.throws(() => run({}, 'wrong-target'), /Rust host differs/); assert.deepEqual(writes, []);
  }
});


test('Windows controlled runtime summary requires all five exact logs and passed records', () => {
  const source = nodeScript('Record ordinary candidate outcomes without claiming skipped acceptance');
  const names = [...step('Run exact controlled App lifecycle hot-switch and stats tests')
    .matchAll(/^            (runtime::proxy::[^\s]+)$/gm)].map((m) => m[1]);
  assert.equal(names.length, 5);
  function summary(missing = '', status = 'passed', logText = 'test result: ok. 1 passed; 0 failed; 0 ignored;', system = {}) {
    const files = new Map([['native-dns-ci-evidence/runtime-status.txt', names.flatMap((name) =>
      [`${name}: started`, `${name}: ${name === missing ? 'started' : status}`]).join('\n')]]);
    for (const name of names) files.set(`native-dns-ci-evidence/${name.split('::').at(-1)}.log`, logText);
    let result;
    new Function('process', 'copyFileSync', 'existsSync', 'mkdirSync', 'readFileSync', 'writeFileSync', source)(
      { env: { RUNNER_OS: 'Windows', RUNTIME_OUTCOME: 'success', ...system } }, () => {}, (path) => files.has(path),
      () => {}, (path) => files.get(path), (path, value) => { if (path.endsWith('/summary.json')) result = JSON.parse(value); });
    return result;
  }
  const success = summary();
  assert.equal(success.windowsProxyRuntimeTests.state, 'passed');
  assert.equal(success.windowsProxyRuntimeTests.requirementState, 'satisfied');
  assert.equal(success.AppAccepted, false);
  assert.equal(success.systemAcceptance.state, 'unexecuted');
  for (const [fresh, prebuilt, state] of [['success','success','passed'], ['success','skipped','failed-or-incomplete'],
    ['failure','skipped','failed-or-incomplete'], ['skipped','skipped','unexecuted']]) {
    const value = summary('', 'passed', 'test result: ok. 1 passed; 0 failed; 0 ignored;',
      { SYSTEM_FRESH_OUTCOME: fresh, SYSTEM_PREBUILT_OUTCOME: prebuilt });
    assert.equal(value.systemAcceptance.state, state);
    assert.equal(value.AppAccepted, false);
  }
  for (const name of names) assert.equal(summary(name).windowsProxyRuntimeTests.requirementState, 'open');
  for (const log of ['', 'test result: ok. 0 passed; 0 failed; 0 ignored;', 'test result: FAILED.'])
    assert.equal(summary('', 'passed', log).windowsProxyRuntimeTests.requirementState, 'open');
  assert.equal(summary('', 'started').windowsProxyRuntimeTests.requirementState, 'open');
});

test('final SYSTEM request fails before heavy work for wrong identity, platform or diagnostic scope', () => {
  const source = nodeScript('Admit exact final candidate SYSTEM request');
  const env = { ...base, GITHUB_REF: 'refs/heads/ci/governance', SYSTEM_CANDIDATE: sha,
    SYSTEM_OS: 'all', SYSTEM_MODE: 'formal', SYSTEM_DIAGNOSTIC: 'false' };
  const run = (extra = {}, head = sha) => new Function('process', 'execFileSync', source)(
    { env: { ...env, ...extra } }, () => head + '\n');
  run(); run({ GITHUB_REF: 'refs/heads/main', SYSTEM_OS: 'windows-2022' });
  for (const extra of [{ SYSTEM_CANDIDATE: 'b'.repeat(40) }, { SYSTEM_CANDIDATE: sha.toUpperCase() },
    { SYSTEM_OS: 'macos-14' }, { SYSTEM_MODE: 'candidate' }, { SYSTEM_DIAGNOSTIC: 'true' },
    { GITHUB_EVENT_NAME: 'pull_request' }, { GITHUB_REPOSITORY: 'other/Polaris' },
    { GITHUB_REF: 'refs/heads/unapproved' }, { GITHUB_REF: 'refs/tags/v1.0.0' }]) assert.throws(() => run(extra));
  assert.throws(() => run({}, 'b'.repeat(40)));
  run({ SYSTEM_CANDIDATE: '' }, 'b'.repeat(40)); // ordinary CI remains unchanged
});

test('Windows real-core observation is native and fails closed without changing compiled-carrier admission', () => {
  const observer = read('src-tauri/src/runtime/proxy/tests/process_observation.rs');
  assert.doesNotMatch(observer, /\bunsafe\s*\{|allow\(unsafe_code|windows_sys::/);
  assert.match(observer, /use crate::runtime::windows_process::tests::controlled_test_observation as native;/);
  assert.match(read('src-tauri/src/runtime/proxy.rs'), /#!\[forbid\(unsafe_code\)\]/);
  assert.match(read('src-tauri/src/runtime/windows_process.rs'),
    /#\[cfg\(test\)\]\npub\(crate\) mod tests;/);
  assert.match(read('src-tauri/src/runtime/windows_process/tests/mod.rs'), /pub\(crate\) mod controlled_test_observation;/);
  const source = read('src-tauri/src/runtime/windows_process/tests/controlled_test_observation.rs');
  assert.match(source, /OpenProcess\(PROCESS_SYNCHRONIZE, FALSE, pid\)/);
  assert.match(source, /WaitForSingleObject/);
  assert.match(source, /CreateToolhelp32Snapshot/);
  assert.match(source, /ERROR_NO_MORE_FILES/);
  assert.match(source, /is_singbox_name/);
  assert.doesNotMatch(source, /unwrap_or\((?:false|0)\)|tasklist|powershell/);
  const proxy = read('src-tauri/src/runtime/proxy/tests/mod.rs');
  assert.match(proxy, /use process_observation::\{ps_alive, singbox_proc_count\}/);
  assert.match(proxy, /NativeStaleCoreIo.end\(&process, signal\)/);
});


test('historical fixture canonicalizes only its owned temp root while release readers reject linked ancestors', () => {
  const parent = historicalSelectionRoot();
  try {
    const alias = join(parent, 'temp-alias');
    symlinkSync(temporaryRoot, alias, process.platform === 'win32' ? 'junction' : 'dir');
    const fixture = historicalSelectionRoot(alias);
    try {
      regularPath(fixture, true);
      assert.equal(fixture, realpathSync(fixture));
      assert.throws(() => regularPath(join(alias, basename(fixture)), true), /Linked release ancestor/);
    } finally { rmSync(fixture, { recursive: true, force: true }); }
  } finally { rmSync(parent, { recursive: true, force: true }); }
});
