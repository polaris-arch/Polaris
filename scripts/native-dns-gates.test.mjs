import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { digest } from './desktop-core/source-graph.mjs';
import { ADAPTER_FILES, RELEASE_TARGETS, loadReleaseContract } from './desktop-core/release-consumer.mjs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { TARGETS, gatePlan, classifyResult, collectGates, summaryFor, gateEnvironment, loadFormalGateSelection, formalWorkflowContext, formalGateContext } from './desktop-core/native-dns-gates.mjs';
import { candidateContext } from './desktop-core/native-dns-candidate.mjs';
import { validationContext } from './desktop-core/validation-origin.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const green = { status: 0, stdout: 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n',
  stderr: '', output: 'Human-readable mixed log; never a verdict source.\n' };
const env = { CORE_CANDIDATE: 'a'.repeat(40), GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1' };

function withHistoricalTemporaryFixture(run) {
  const directory = mkdtempSync(join(tmpdir(), 'historical-gate-selection-'));
  try {
    const current = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
    const manifest = { desktopTemporary: current.historicalDesktopTemporary,
      historicalDesktopRelease: current.historicalDesktopRelease };
    mkdirSync(join(directory, 'src-tauri'), { recursive: true });
    mkdirSync(join(directory, 'scripts/desktop-core'), { recursive: true });
    for (const name of ['native-dns-candidate.json', 'native-dns-source-binding.json'])
      copyFileSync(join(root, 'scripts/desktop-core', name), join(directory, 'scripts/desktop-core', name));
    // The old release is meaningful only with its original exact contract bytes.
    copyFileSync(join(root, 'scripts/desktop-core/historical-release-contract-577d4b9.json'),
      join(directory, 'scripts/desktop-core/release-contract.json'));
    writeFileSync(join(directory, 'src-tauri/core-manifest.json'), JSON.stringify(manifest));
    run(directory, manifest);
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

test('four native targets include every existing mandatory integration target and exact test', () => {
  assert.deepEqual(Object.keys(TARGETS), ['linux', 'win', 'mac-x64', 'mac-arm64']);
  for (const key of Object.keys(TARGETS)) {
    const plan = gatePlan(key, root);
    assert.equal(plan.length, TARGETS[key].library ? 16 : 15);
    assert.equal(new Set(plan.map((gate) => gate.id)).size, plan.length);
    for (const gate of plan.filter((item) => item.rustTests)) {
      const pkg = gate.args[gate.args.indexOf('-p') + 1];
      const packagePath = pkg.replace(/^polaris-/, '');
      assert.match(readFileSync(join(root, 'crates', packagePath, 'Cargo.toml'), 'utf8'), new RegExp(`name = "${pkg}"`));
      if (gate.args.includes('--test')) {
        const name = gate.args[gate.args.indexOf('--test') + 1];
        const source = readFileSync(join(root, 'crates', packagePath, 'tests', `${name}.rs`), 'utf8');
        const exact = gate.args[gate.args.indexOf('--test') + 2];
        if (exact !== '--') assert.match(source, new RegExp(`fn ${exact}\\(`));
      } else {
        const name = gate.args[gate.args.indexOf('--lib') + 1].split('::').at(-1);
        assert.match(readFileSync(join(root, 'crates/net-stack/src/singbox_import/tests/mod.rs'), 'utf8'), new RegExp(`fn ${name}\\(`));
      }
      assert.ok(gate.args.includes('--locked'));
      assert.ok(gate.args.includes('--nocapture'));
    }
    const clash = plan.find((gate) => gate.id === 'native_clash_mode');
    assert.ok(clash.args.includes('--ignored'));
    assert.ok(clash.args.includes('--exact'));
  }
});

test('ordinary failure does not hide independent later gates, and every required result is needed', async () => {
  const plan = gatePlan('linux', '/fixture'), seen = [], checkpoints = [];
  const results = await collectGates(plan, 'linux', async (gate) => {
    seen.push(gate.id);
    if (gate.id === 'core_schema_surface') return { status: 101, stdout: '', stderr: 'schema drift', output: 'schema drift' };
    if (gate.id === 'kernel_accepts_full_config') throw Error('compiler failed');
    return { ...green, proofValid: true };
  }, (partial) => checkpoints.push(partial.length));
  assert.deepEqual(seen, plan.map((gate) => gate.id));
  assert.equal(checkpoints.at(-1), plan.length);
  assert.equal(results.filter((gate) => gate.state === 'failed').length, 2);
  const summary = summaryFor('linux', env, plan, results);
  assert.equal(summary.mandatoryGatesPassed, false);
  assert.equal(summary.appAccepted, false);
  assert.equal(summary.publicationEligible, false);
  assert.match(summary.sealedInputCompatibility, /unvalidated/);
  const passed = results.map((gate) => ({ ...gate, state: 'passed' }));
  assert.equal(summaryFor('linux', env, plan, passed).mandatoryGatesPassed, true);
  assert.equal(summaryFor('linux', env, plan, passed.slice(1)).mandatoryGatesPassed, false);
  assert.equal(summaryFor('linux', env, plan, [...passed].reverse()).mandatoryGatesPassed, false);
});

test('zero tests, ignored tests, spawn errors, timeout, skip and missing Cronet proof fail closed', () => {
  const gate = gatePlan('linux', '/fixture')[0];
  for (const raw of [
    { ...green, stdout: green.stdout.replace('1 passed', '0 passed') },
    { ...green, stdout: green.stdout.replace('0 ignored', '1 ignored') },
    { ...green, status: null, error: 'ENOENT' }, { ...green, timedOut: true }, { ...green, interrupted: true },
    { ...green, stdout: `${green.stdout}⚠ skipped missing kernel\n` },
    { ...green, stdout: `test foo ... ⚠ skipped missing kernel\n${green.stdout}` },
    { ...green, stdout: `${green.stdout}NOT_EXECUTED: kernel unavailable\n` },
  ]) assert.equal(classifyResult(gate, raw, 'linux').state, 'failed');
  const cronet = gatePlan('linux', '/fixture').at(-1);
  assert.equal(classifyResult(cronet, { status: 0, stdout: '', stderr: '', output: '' }, 'linux').state, 'failed');
  assert.equal(classifyResult(cronet, { status: 0, stdout: '', stderr: '', output: '', proofValid: true }, 'linux').state, 'passed');
});

test('only the documented non-Linux DHCP exclusion is recorded as inapplicable', () => {
  const gate = gatePlan('win', '/fixture').find((item) => item.id === 'network_profile_runtime');
  const exclusion = '⚠ 跳过 dhcp 起核门：不触网前置只在 Linux 上可证\n';
  for (const channel of ['stdout', 'stderr']) {
    const raw = { ...green, [channel]: green[channel] + exclusion };
    assert.equal(classifyResult(gate, raw, 'win').state, 'passed');
    assert.equal(classifyResult(gate, raw, 'win').platformExclusions.length, 1);
    assert.equal(classifyResult(gate, raw, 'linux').state, 'failed');
    assert.equal(classifyResult({ ...gate, id: 'unrelated_gate' }, raw, 'win').state, 'failed');
    assert.equal(classifyResult(gate, { ...raw, stderr: raw.stderr + 'NOT_EXECUTED: another gate\n' }, 'win').state, 'failed');
  }
});

test('only stdout supplies test counts while either channel can veto skipped execution', () => {
  const gate = gatePlan('linux', '/fixture')[0];
  for (const stdout of ['', undefined, green.stdout.replace('1 passed', '0 passed')]) {
    const raw = { ...green, stdout, stderr: green.stdout, output: green.stdout };
    assert.equal(classifyResult(gate, raw, 'linux').state, 'failed', 'stderr/mixed log cannot supply a missing verdict');
  }
  const diagnostic = { ...green, stderr: 'test result: ok. 999 passed; 0 failed; 99 ignored;\n',
    output: 'NOT_EXECUTED: retained human log is not a machine input\n' };
  const passed = classifyResult(gate, diagnostic, 'linux');
  assert.equal(passed.state, 'passed');
  assert.equal(passed.testsPassed, 1);
  assert.equal(passed.testsIgnored, 0);
  for (const channel of ['stdout', 'stderr']) {
    for (const warning of ['⚠ skipped missing kernel\n', 'test foo ... ⛔ 跳过起核\n', 'NOT_EXECUTED: unavailable\n'])
      assert.equal(classifyResult(gate, { ...green, [channel]: green[channel] + warning }, 'linux').state, 'failed');
  }
  assert.equal(classifyResult(gate, { ...green, stdout: green.stdout + 'NOT_EXEC', stderr: 'UTED\n' }, 'linux').state,
    'passed', 'independent channels must not manufacture a skip token across their boundary');
});

test('uncertain child cleanup blocks remaining launches', async () => {
  const plan = gatePlan('linux', '/fixture');
  let called = 0;
  const results = await collectGates(plan, 'linux', async () => {
    called++; return { ...green, cleanupUncertain: true };
  });
  assert.equal(called, 1);
  assert.equal(results[0].state, 'failed');
  assert.ok(results.slice(1).every((gate) => gate.state === 'blocked'));
});

test('interruption stops additional gates even when the current child settled', async () => {
  let called = 0;
  const results = await collectGates(gatePlan('win', '/fixture'), 'win', async () => {
    called++; return { ...green, interrupted: true, error: 'Runner interrupted' };
  });
  assert.equal(called, 1);
  assert.ok(results.slice(1).every((gate) => gate.state === 'blocked'));
});

// Run the real executor in an isolated Node wrapper. Its forwarded bytes are
// drained independently, so large/invalid fixture output never pollutes TAP and
// no test needs to replace the parent process's global stdout/stderr methods.
async function commandFixture(t, source, timeout = 30_000) {
  const directory = mkdtempSync(join(tmpdir(), 'native-gate-streams-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const childPath = join(directory, 'child.mjs'), wrapperPath = join(directory, 'wrapper.mjs');
  writeFileSync(childPath, source);
  writeFileSync(wrapperPath, `
    import { writeFileSync } from 'node:fs';
    import { join } from 'node:path';
    import { executeCommand, classifyResult } from ${JSON.stringify(new URL('./desktop-core/native-dns-gates.mjs', import.meta.url).href)};
    const [directory, childPath, timeout] = process.argv.slice(2);
    const result = await executeCommand({ id: 'fixture', program: process.execPath, args: [childPath] },
      { root: directory, env: process.env, directory, timeout: Number(timeout) });
    const { stdout, stderr, output, ...facts } = result;
    const small = text => typeof text === 'string' && Buffer.byteLength(text) <= 256 * 1024 ? text : null;
    writeFileSync(join(directory, 'result.json'), JSON.stringify({ ...facts,
      stdout: small(stdout), stderr: small(stderr), output: small(output),
      stdoutIsString: typeof stdout === 'string', stderrIsString: typeof stderr === 'string',
      inertState: classifyResult({ id: 'inert-transport', rustTests: false }, result, 'linux').state }));
  `);
  const forwarded = Object.fromEntries(['stdout', 'stderr'].map(channel =>
    [channel, { bytes: 0, hash: createHash('sha256') }]));
  const wrapper = spawn(process.execPath, [wrapperPath, directory, childPath, String(timeout)], {
    cwd: directory, env: { ...process.env, NODE_NO_WARNINGS: '1' },
    stdio: ['ignore', 'pipe', 'pipe'], timeout: timeout + 35_000,
  });
  for (const channel of ['stdout', 'stderr']) wrapper[channel].on('data', chunk => {
    forwarded[channel].bytes += chunk.length;
    forwarded[channel].hash.update(chunk);
  });
  const status = await new Promise((resolve, reject) => {
    wrapper.once('error', reject);
    wrapper.once('close', (code, signal) => resolve({ code, signal }));
  });
  assert.deepEqual(status, { code: 0, signal: null }, 'isolated executor wrapper must settle normally');
  return { directory, result: JSON.parse(readFileSync(join(directory, 'result.json'), 'utf8')),
    forwarded: Object.fromEntries(Object.entries(forwarded).map(([channel, value]) =>
      [channel, { bytes: value.bytes, sha256: value.hash.digest('hex') }])) };
}

function assertChannelEvidence(observed, expectedStdout = null, expectedStderr = null) {
  const { directory, result, forwarded } = observed;
  assert.equal(result.stdoutIsString, true);
  assert.equal(result.stderrIsString, true);
  for (const [channel, expected] of [['stdout', expectedStdout], ['stderr', expectedStderr]]) {
    const bytes = readFileSync(join(directory, `fixture.${channel}.log`));
    if (expected !== null) assert.deepEqual(bytes, Buffer.from(expected));
    assert.equal(result[`${channel}Bytes`], bytes.length, `${channel} byte count binds the raw file`);
    assert.equal(result[`${channel}Sha256`], digest(bytes), `${channel} hash binds the raw file`);
    assert.deepEqual(forwarded[channel], { bytes: bytes.length, sha256: digest(bytes) },
      `${channel} is forwarded only to its own parent channel`);
  }
  const combined = readFileSync(join(directory, 'fixture.log'));
  assert.equal(result.logBytes, combined.length);
  assert.equal(result.logSha256, digest(combined), 'legacy human log keeps its original byte identity');
}

const childWrite = `const write = (stream, bytes) => new Promise((resolve, reject) =>
  stream.write(bytes, error => error ? reject(error) : resolve()));`;

test('real executor preserves a large JSON record across stdout fragments and concurrent stderr diagnostics',
  { timeout: 70_000 }, async (t) => {
    const config = JSON.stringify({ fixture: 'x'.repeat(80_516), unicode: '汉🙂', escaped: 'line one\nline two' });
    const record = { configJson: config, bytes: Buffer.byteLength(config), sha256: digest(config) };
    const line = 'POLARIS_DEFAULT_GRAPH ' + JSON.stringify(record);
    const diagnostic = 'INFO inert diagnostic\r\nPOLARIS_CONTROLLED_CLEANUP {"fake":true}\n';
    const observed = await commandFixture(t, `${childWrite}
      const line = 'POLARIS_DEFAULT_GRAPH ' + JSON.stringify({
        configJson: JSON.stringify({ fixture: 'x'.repeat(80516), unicode: '汉🙂', escaped: 'line one\\nline two' }),
        bytes: ${record.bytes}, sha256: ${JSON.stringify(record.sha256)} });
      const raw = Buffer.from(line);
      await write(process.stdout, raw.subarray(0, 35791));
      await write(process.stderr, Buffer.from(${JSON.stringify(diagnostic)}));
      await write(process.stdout, raw.subarray(35791));
      await write(process.stdout, Buffer.from('\\r'));
      await write(process.stdout, Buffer.from('\\n' + ${JSON.stringify(green.stdout)}));
    `);
    const stdout = line + '\r\n' + green.stdout;
    assertChannelEvidence(observed, stdout, diagnostic);
    assert.equal(observed.result.stdout, stdout);
    assert.equal(observed.result.stderr, diagnostic);
    assert.equal(observed.result.error, null);
    const parsed = JSON.parse(observed.result.stdout.split('\r\n')[0].slice('POLARIS_DEFAULT_GRAPH '.length));
    assert.deepEqual(parsed, record);
    assert.equal(classifyResult(gatePlan('linux', '/fixture')[0], observed.result, 'linux').state, 'passed');
  });

test('real executor keeps split multibyte UTF-8, LF/CRLF and a final missing newline byte-for-byte',
  { timeout: 70_000 }, async (t) => {
    const stdout = `协议🙂é\r\n${JSON.stringify({ text: '汉🙂', escaped: 'a\nb' })}\n${green.stdout}tail without newline`;
    const stderr = '诊断🙂\r\nsecond diagnostic\n';
    const observed = await commandFixture(t, `${childWrite}
      const send = async (stream, bytes) => {
        for (const byte of bytes) {
          await write(stream, Buffer.from([byte]));
          await new Promise(resolve => setImmediate(resolve));
        }
      };
      await Promise.all([
        send(process.stdout, Buffer.from(${JSON.stringify(stdout)})),
        send(process.stderr, Buffer.from(${JSON.stringify(stderr)})),
      ]);
    `);
    assertChannelEvidence(observed, stdout, stderr);
    assert.equal(observed.result.stdout, stdout);
    assert.equal(observed.result.stderr, stderr);
    assert.equal(observed.result.error, null);
    assert.ok(!observed.result.stdout.endsWith('\n'), 'transport must not invent an EOF record terminator');
  });

test('invalid or incomplete stdout UTF-8 is sticky failure even after a valid pass verdict',
  { timeout: 70_000 }, async (t) => {
    for (const tail of [[0xc3, 0x28], [0xe6, 0xb1]]) {
      const stdout = Buffer.concat([Buffer.from(green.stdout), Buffer.from(tail)]);
      const observed = await commandFixture(t, `${childWrite}
        await write(process.stdout, Buffer.concat([Buffer.from(${JSON.stringify(green.stdout)}), Buffer.from(${JSON.stringify(tail)})]));
      `);
      assertChannelEvidence(observed, stdout, '');
      assert.ok(observed.result.error, 'malformed stdout must not be silently decoded with replacement characters');
      assert.equal(classifyResult(gatePlan('linux', '/fixture')[0], observed.result, 'linux').state, 'failed');
    }
  });

test('invalid diagnostic UTF-8 retains raw stderr evidence without corrupting valid stdout',
  { timeout: 70_000 }, async (t) => {
    const stderr = Buffer.from([0xff, 0x0a]);
    const observed = await commandFixture(t, `${childWrite}
      await write(process.stdout, Buffer.from(${JSON.stringify(green.stdout)}));
      await write(process.stderr, Buffer.from([255, 10]));
    `);
    assertChannelEvidence(observed, green.stdout, stderr);
    assert.equal(observed.result.stdout, green.stdout);
    assert.equal(observed.result.error, null);
    assert.equal(classifyResult(gatePlan('linux', '/fixture')[0], observed.result, 'linux').state, 'passed');
  });

test('the real executor enforces one combined 32 MiB budget, including stderr, at the exact boundary',
  { timeout: 140_000 }, async (t) => {
    const half = 16 * 1024 * 1024;
    for (const extra of [0, 1]) {
      const observed = await commandFixture(t, `${childWrite}
        await write(process.stdout, Buffer.alloc(${half}, 97));
        await write(process.stderr, Buffer.alloc(${half}, 98));
        ${extra ? 'await write(process.stderr, Buffer.from([99]));' : ''}
      `);
      assertChannelEvidence(observed);
      if (extra === 0) {
        assert.equal(observed.result.stdoutBytes, half);
        assert.equal(observed.result.stderrBytes, half);
        assert.equal(observed.result.error, null);
        assert.equal(observed.result.inertState, 'passed');
      } else {
        assert.ok(observed.result.error, 'the extra stderr byte exceeds the shared budget');
        assert.equal(observed.result.inertState, 'failed');
      }
    }
  });

test('executor waits for inherited stdout and stderr pipes to drain after the original child exits',
  { timeout: 70_000 }, async (t) => {
    // The descendant's stdin receives EOF only when its original parent exits.
    // That event, rather than a sleep or cross-channel ordering assumption,
    // releases late bytes on both inherited output pipes.
    const tail = `${childWrite}
      process.stdin.once('end', async () => {
        await write(process.stdout, Buffer.from(${JSON.stringify(green.stdout)}));
        await write(process.stderr, Buffer.from('drained after original child exit\\n'));
      });
      process.stdin.resume();
    `;
    const observed = await commandFixture(t, `${childWrite}
      import { spawn } from 'node:child_process';
      await write(process.stdout, Buffer.from('head\\n'));
      const child = spawn(process.execPath, ['--input-type=module', '-e', ${JSON.stringify(tail)}],
        { stdio: ['pipe', 1, 2] });
      child.once('error', () => process.exit(1));
      child.once('spawn', () => process.exit(0));
    `);
    assertChannelEvidence(observed, 'head\n' + green.stdout, 'drained after original child exit\n');
    assert.equal(observed.result.status, 0);
    assert.equal(observed.result.error, null);
    assert.equal(classifyResult(gatePlan('linux', '/fixture')[0], observed.result, 'linux').state, 'passed');
  });

test('CLI local no-kernel boundary fails before verifying payloads or spawning tests', () => {
  const directory = mkdtempSync(join(tmpdir(), 'native-dns-gates-'));
  try {
    const result = spawnSync(process.execPath, [join(root, 'scripts/desktop-core/native-dns-gates.mjs'), 'run', '--platform=linux'], {
      cwd: directory, env: { ...process.env, ...env, POLARIS_NO_KERNEL_RUN: '1' }, encoding: 'utf8', timeout: 5000,
    });
    assert.equal(result.status, 1);
    const summary = JSON.parse(readFileSync(join(directory, 'native-dns-evidence/linux/summary.json')));
    assert.match(summary.failure, /no-kernel boundary/);
    assert.deepEqual(summary.gates, []);
    assert.equal(summary.appAccepted, false);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('finalize marks setup failure incomplete without requiring candidate payloads', () => {
  const directory = mkdtempSync(join(tmpdir(), 'native-dns-finalize-'));
  try {
    const result = spawnSync(process.execPath, [join(root, 'scripts/desktop-core/native-dns-gates.mjs'), 'finalize', '--platform=linux'], {
      cwd: directory, env: { ...process.env, ...env, POLARIS_NO_KERNEL_RUN: '1' }, encoding: 'utf8', timeout: 5000,
    });
    assert.equal(result.status, 0, result.stderr);
    const summary = JSON.parse(readFileSync(join(directory, 'native-dns-evidence/linux/summary.json')));
    assert.equal(summary.mandatoryGatesPassed, false);
    assert.match(summary.failure, /did not start/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('workflow admission is failing, native, read-only, all-gate and always retains evidence', () => {
  const workflow = readFileSync(join(root, '.github/workflows/native-dns-candidate.yml'), 'utf8');
  for (const text of ['workflow_dispatch:', 'contents: read', 'needs: admit',
    'collab/native-node-dns-20261018', "e.CORE_CANDIDATE !== e.GITHUB_SHA", '1347028220',
    'ubuntu-22.04', 'windows-2022', 'macos-15-intel', 'macos-15', 'persist-credentials: false',
    'acquire --candidate=', 'verify --candidate=', 'native-dns-gates.mjs run',
    'native-dns-gates.mjs finalize', 'if-no-files-found: error']) assert.ok(workflow.includes(text), text);
  for (const text of ['actions/setup-go@v6', 'sourcePolicy.goVersion', 'go env GOVERSION',
    'xcrun --sdk macosx --show-sdk-path', "'x86_64-pc-windows-msvc'", 'rustup toolchain install stable',
    'scripts/fetch-protoc.mjs']) assert.ok(workflow.includes(text), text);
  assert.equal((workflow.match(/if: always\(\)/g) ?? []).length, 2);
  const acquisition = '      - name: Acquire four exact temporary desktop payloads\n'
    + '        if: ${{ inputs.formal_release != true }}\n'
    + '        env:\n          POLARIS_RELEASE_API_TOKEN: ${{ github.token }}\n'
    + '        run: node scripts/desktop-core/native-dns-candidate.mjs acquire --candidate="$CORE_CANDIDATE"';
  assert.ok(workflow.includes(acquisition));
  const formalAcquisition = '      - name: Acquire four committed strict formal desktop payloads\n'
    + '        if: ${{ inputs.formal_release == true }}\n'
    + '        env:\n          POLARIS_RELEASE_API_TOKEN: ${{ github.token }}\n'
    + '        run: node scripts/fetch-core.mjs';
  assert.ok(workflow.includes(formalAcquisition));
  assert.doesNotMatch(workflow.replace(acquisition, '').replace(formalAcquisition, ''), /github\.token|POLARIS_RELEASE_API_TOKEN/);
  assert.doesNotMatch(workflow, /sudo|SYSTEM|secrets\.|actions: write|contents: write|fetch-cronet\.mjs/);
});

test('callee admission retains manual caller context for direct and local reusable entry points', () => {
  const workflow = readFileSync(join(root, '.github/workflows/native-dns-candidate.yml'), 'utf8');
  const source = /node - <<'NODE'\n([\s\S]*?)\n          NODE/.exec(workflow)[1].replace(/^          /gm, '');
  const admit = new Function('process', source);
  const sha = 'a'.repeat(40), ref = 'refs/heads/collab/native-node-dns-20261018';
  const base = { GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'workflow_dispatch',
    GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REPOSITORY_ID: '1347028220',
    GITHUB_REF: ref, GITHUB_SHA: sha, CORE_CANDIDATE: sha, POLARIS_NATIVE_DNS_CANDIDATE: '1',
    GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1' };
  const checkout = (head = sha, status = '') => (_command, args) => args[0] === 'rev-parse' ? head : status;
  for (const name of ['ci.yml', 'native-dns-candidate.yml']) {
    const exact = { ...base, GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/${name}@${ref}` };
    admit({ env: exact });
    assert.equal(candidateContext(sha, exact, root, checkout()).workflowRef, exact.GITHUB_WORKFLOW_REF);
    for (const changed of [
      { GITHUB_ACTIONS: 'false' }, { GITHUB_EVENT_NAME: 'workflow_call' },
      { GITHUB_EVENT_NAME: 'pull_request' }, { GITHUB_EVENT_NAME: 'push' },
      { GITHUB_REPOSITORY: 'other/Polaris' }, { GITHUB_REPOSITORY_ID: '1' },
      { GITHUB_REF: 'refs/heads/main' }, { GITHUB_REF: 'refs/tags/native-node-dns-20261018' },
      { GITHUB_SHA: 'b'.repeat(40) }, { CORE_CANDIDATE: '' }, { CORE_CANDIDATE: undefined },
      { CORE_CANDIDATE: sha.toUpperCase() }, { CORE_CANDIDATE: '1' },
      { GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/package.yml@${ref}` },
      { GITHUB_WORKFLOW_REF: exact.GITHUB_WORKFLOW_REF.replace(ref, 'refs/heads/main') },
      { GITHUB_WORKFLOW_REF: '' }, { POLARIS_NO_KERNEL_RUN: '1' },
      ...['GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT'].flatMap((key) =>
        ['', '0', '-1', '1.1', '01', undefined].map((value) => ({ [key]: value }))),
    ]) {
      const rejected = { ...exact, ...changed };
      assert.throws(() => admit({ env: rejected }), /Exact native DNS manual candidate context/);
      assert.throws(() => candidateContext(rejected.CORE_CANDIDATE, rejected, root, checkout()),
        /Exact manual native-DNS candidate\/run context/);
    }
    assert.throws(() => candidateContext(sha, exact, root, checkout('b'.repeat(40))), /checkout differs/);
    assert.throws(() => candidateContext(sha, exact, root, checkout(sha, ' M Cargo.lock')), /checkout differs/);
  }
  assert.ok(workflow.indexOf('Verify exact manual candidate context') < workflow.indexOf('Acquire four exact temporary desktop payloads'));
  assert.ok(workflow.includes("candidateContext(e.CORE_CANDIDATE, {...e, POLARIS_NATIVE_DNS_CANDIDATE: '1'}, process.cwd())"));
});

// Synthetic pins exercise admission only; no release or payload is asserted.
const hash = 'a'.repeat(64), commit = '55300c55560bcc604fd9675c5526b5713db81cc1',
  tree = '7767ba9a9901a061ff07db7716baed7896cfefbc';
function formalFixture() {
  let id = 1;
  const contract = { schema: 'polaris-upstream-native-release-consumer-v1', providerContract: 'polaris-upstream-native-release-v1',
    repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, releaseId: 100,
    version: '1.15.0-polaris-alpha.12', tag: 'v1.15.0-polaris-alpha.12', tagObject: 'd'.repeat(40), sourceCommit: commit, sourceTree: tree,
    producer: { repository: 'polaris-arch/sing-box-polaris', repositoryId: 1421657509, workflow: '.github/workflows/build.yml', workflowCommit: commit, runId: 1, runAttempt: 1, nativeJobs: Object.fromEntries(Object.entries({ linux: 'linux-amd64-purego', win: 'windows-amd64', 'mac-x64': 'darwin-amd64', 'mac-arm64': 'darwin-arm64' }).map(([key, id], i) => [key, { id: i + 1, name: `Native final-package Cronet and gVisor smoke (${id})` }])) },
    legalFiles: ['LICENSE', 'NOTICE'].map((name) => ({ path: `licenses/${name}`, content: name, bytes: Buffer.byteLength(name), sha256: digest(name) })), platforms: {} };
  const asset = (name) => ({ name, bytes: 1, sha256: hash, assetId: id++, releaseId: 100,
    url: `https://github.com/${contract.repository}/releases/download/${contract.tag}/${name}` });
  contract.assetIndex = asset('polaris-upstream-native-assets.json');
  contract.sourceArchive = asset(`sing-box-polaris-source-${commit}.tar.gz`);
  for (const [key, target] of Object.entries(RELEASE_TARGETS)) {
    const root = `sing-box-${contract.version}-${target.os}-${target.arch}`;
    const payload = (member) => ({ member, bytes: 1, sha256: hash });
    const linkage = target.sidecar ? 'shared-purego' : 'static-cgo';
    const row = { schema: 'polaris-upstream-native-receipt-v1', id: root, target: `${target.os}/${target.arch}`,
      variant: { linkage, libc: '', goarm: '', gomips: '', go386: '', legacy: false },
      version: contract.version, tag: contract.tag, sourceCommit: commit, sourceTree: tree, goVersion: 'go1.26.8', buildID: '', rawBuildInfo: 'fixture: go1.26.8\n',
      tags: ['with_gvisor', 'with_naive_outbound', ...(target.sidecar ? ['with_purego'] : [])].join(','),
      sourceIdentity: { sourceOverlay: 0, goModSha256: hash, goSumSha256: hash, replacements: [{ module: 'github.com/sagernet/sing-tun', path: './third_party/sing-tun', tree }] },
      buildGraph: { settings: {}, normalModules: [{ module: 'example.invalid/fixture', version: 'v1.0.0', h1: 'h1:YQ=' }], prebuiltAcquisitionModules: [],
        localReplacements: [{ module: 'github.com/sagernet/sing-tun', version: 'v1.0.0', h1: '', replacement: { module: './third_party/sing-tun', version: '(devel)', h1: '' } }] },
      core: payload(target.core), sidecar: target.sidecar ? payload(target.sidecar) : null,
      archive: { name: root + target.suffix, root, bytes: 1, sha256: hash, members: Object.fromEntries([target.core, 'LICENSE', ...(target.sidecar ? [target.sidecar] : [])].map((name) => [`${root}/${name}`, hash])) },
      signatureObservation: key.startsWith('mac-') ? { state: 'unsigned', cdHash: null, trustValidated: false } : null,
      finalPackageCronetSmoke: { status: 'passed', os: target.os, arch: target.arch, go: 'go1.26.8', gvisorCompiled: true, naiveOutbound: true, linkage,
        version: '150.0.0.1', librarySha256: target.sidecar ? hash : null, coreSha256: hash }, releaseAsset: asset(root + target.suffix) };
    contract.platforms[key] = row;
  }
  const { schema, providerContract, releaseId, assetIndex, platforms, sourceArchive, ...base } = contract;
  const index = { schema: 'polaris-upstream-native-assets-v1', ...base,
    sourceArchive: { name: sourceArchive.name, bytes: sourceArchive.bytes, sha256: sourceArchive.sha256 },
    platforms: Object.values(platforms).map(({ releaseAsset, ...row }) => row) };
  return { contract, index };
}

function withFormalFixture(run) {
  const directory = mkdtempSync(join(tmpdir(), 'formal-gate-selection-'));
  const { contract } = formalFixture();
  const contractPath = 'scripts/desktop-core/release-contract.json';
  const bytes = JSON.stringify(contract);
  const manifest = { desktopRelease: { schema: 'polaris-app-release-selection-v1', contractPath,
    contractSha256: digest(bytes), ...Object.fromEntries(['version', 'tag', 'tagObject', 'sourceCommit', 'sourceTree', 'releaseId'].map((key) => [key, contract[key]])),
    librarySha256: { linux: contract.platforms.linux.sidecar.sha256, win: contract.platforms.win.sidecar.sha256 } } };
  mkdirSync(join(directory, 'src-tauri'), { recursive: true });
  mkdirSync(join(directory, 'scripts/desktop-core'), { recursive: true });
  writeFileSync(join(directory, contractPath), bytes);
  writeFileSync(join(directory, 'src-tauri/core-manifest.json'), JSON.stringify(manifest));
  try { run(directory, manifest, contract); } finally { rmSync(directory, { recursive: true, force: true }); }
}
const formalEnv = { GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'push', GITHUB_REPOSITORY: 'polaris-arch/Polaris',
  GITHUB_REPOSITORY_ID: '1347028220', GITHUB_REF: 'refs/heads/main', GITHUB_SHA: 'a'.repeat(40),
  GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/ci.yml@refs/heads/main',
  GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1' };

test('formal enablement requires validated committed upstream bytes, never historical or temporary selection', () => {
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
  const bytes = readFileSync(join(root, 'scripts/desktop-core/release-contract.json'));
  const current = loadFormalGateSelection(root);
  assert.deepEqual(current, JSON.parse(bytes));
  assert.equal(current.sourceCommit, 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e');
  assert.equal(current.sourceTree, '4c0fdb31d59967f41c808ea6b63f87ca9661c2cf');
  assert.equal(manifest.desktopRelease.contractSha256, digest(bytes));
  assert.equal(manifest.desktopTemporary, undefined);
  const historical = JSON.parse(readFileSync(join(root, 'scripts/desktop-core/pending-release-selection.json'))).historicalSelection;
  assert.deepEqual(historical, manifest.historicalDesktopRelease);
  assert.throws(() => loadFormalGateSelection(root, { desktopRelease: historical }), /SHA differs/,
    'historical selection cannot admit the current canonical contract');
  assert.equal(loadFormalGateSelection(root, {}), null);
  withHistoricalTemporaryFixture((directory, temporaryManifest) => {
    assert.equal(temporaryManifest.desktopTemporary.sourceCommit, 'fe182431495711c09bdb3b074ed6aa6309c00cf8');
    assert.equal(loadFormalGateSelection(directory), null);
    assert.equal(loadFormalGateSelection(directory, { desktopRelease: historical }), null);
    assert.throws(() => loadFormalGateSelection(directory, { ...temporaryManifest, desktopRelease: historical }),
      /exclusive desktop gate selection/);
  });
  withFormalFixture((directory, manifest, contract) => {
    assert.deepEqual(loadFormalGateSelection(directory), contract);
    const alias = structuredClone(manifest);
    alias.desktopRelease.contractPath = 'scripts/desktop-core/pending-release-contract.json';
    writeFileSync(join(directory, alias.desktopRelease.contractPath), JSON.stringify(contract));
    assert.throws(() => loadFormalGateSelection(directory, alias), /canonical upstream contract path/);
    const changed = structuredClone(manifest);
    changed.desktopRelease.contractSha256 = '0'.repeat(64);
    assert.throws(() => loadFormalGateSelection(directory, changed), /SHA differs/);
    changed.desktopRelease = { ...manifest.desktopRelease, sourceCommit: '0'.repeat(40) };
    assert.throws(() => loadFormalGateSelection(directory, changed), /sourceCommit differs/);
    contract.platforms.linux.finalPackageCronetSmoke = null;
    const invalidBytes = JSON.stringify(contract);
    writeFileSync(join(directory, manifest.desktopRelease.contractPath), invalidBytes);
    changed.desktopRelease = { ...manifest.desktopRelease, contractSha256: digest(invalidBytes) };
    assert.throws(() => loadFormalGateSelection(directory, changed), /smoke/i);
  });
});

test('formal reusable mode retains exact current App caller/run and rejects direct historical dispatch or injected selectors', () => {
  const workflow = readFileSync(join(root, '.github/workflows/native-dns-candidate.yml'), 'utf8');
  const source = /node - <<'NODE'\n([\s\S]*?)\n          NODE/.exec(workflow)[1].replace(/^          /gm, '');
  const admit = new Function('process', source);
  for (const name of ['ci.yml', 'package.yml']) for (const event of ['push', 'pull_request', 'merge_group', 'workflow_dispatch']) {
    const selected = { ...formalEnv, GITHUB_EVENT_NAME: event,
      GITHUB_WORKFLOW_REF: formalEnv.GITHUB_WORKFLOW_REF.replace('ci.yml', name) };
    assert.equal(formalWorkflowContext(selected.GITHUB_SHA, selected).mode, 'formal-default');
    admit({ env: { ...selected, CORE_CANDIDATE: selected.GITHUB_SHA, NATIVE_DNS_FORMAL_INPUT: '1' } });
  }
  for (const changed of [{ GITHUB_REPOSITORY: 'other/Polaris' }, { GITHUB_REPOSITORY_ID: '1' },
    { GITHUB_SHA: 'b'.repeat(40) }, { GITHUB_EVENT_NAME: 'workflow_call' }, { GITHUB_REF: 'refs/heads/other' },
    { GITHUB_WORKFLOW_REF: formalEnv.GITHUB_WORKFLOW_REF.replace('ci.yml', 'native-dns-candidate.yml') },
    { GITHUB_WORKFLOW_REF: '' }, { GITHUB_RUN_ID: '0' }, { GITHUB_RUN_ATTEMPT: '0' },
    { GITHUB_ACTIONS: 'false' }, { POLARIS_NO_KERNEL_RUN: '1' }]) {
    const selected = { ...formalEnv, ...changed };
    assert.throws(() => formalWorkflowContext(formalEnv.GITHUB_SHA, selected), /formal App/);
    assert.throws(() => admit({ env: { ...selected, CORE_CANDIDATE: formalEnv.GITHUB_SHA, NATIVE_DNS_FORMAL_INPUT: '1' } }));
  }
  for (const key of ['POLARIS_NATIVE_DNS_CANDIDATE', 'POLARIS_NATIVE_PAYLOAD_VALIDATION', 'POLARIS_SINGBOX_PATH'])
    for (const value of ['', '0', '1']) assert.throws(() => formalWorkflowContext(formalEnv.GITHUB_SHA, { ...formalEnv, [key]: value }));
  assert.doesNotMatch(workflow.split('  workflow_call:')[0], /formal_release:/, 'formal mode cannot broaden historical manual inputs');
});

test('formal source and App tree must match the clean committed checkout before payload acquisition', () => {
  withFormalFixture((directory, manifest, contract) => {
    const git = (changed = {}) => (_command, args, options) => {
      for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) assert.equal(Object.hasOwn(options.env, key), false);
      if (args[0] === 'show') return changed.bytes ?? readFileSync(join(directory, args[1].slice('HEAD:'.length)), 'utf8');
      if (args[0] === 'rev-parse') return args[1] === 'HEAD' ? changed.head ?? formalEnv.GITHUB_SHA : 'b'.repeat(40);
      return changed.status ?? '';
    };
    const context = formalGateContext(directory, { ...formalEnv, GIT_DIR: 'other', GIT_INDEX_FILE: 'other', GIT_WORK_TREE: 'other' }, git());
    assert.equal(context.contractSha256, manifest.desktopRelease.contractSha256);
    assert.equal(context.kernelSourceCommit, contract.sourceCommit);
    assert.equal(context.kernelSourceTree, contract.sourceTree);
    assert.equal(context.tree, 'b'.repeat(40));
    for (const changed of [{ head: 'c'.repeat(40) }, { status: ' M src-tauri/core-manifest.json' }, { bytes: '{}' }])
      assert.throws(() => formalGateContext(directory, formalEnv, git(changed)), /checkout|committed/);
  });
});


test('formal Cronet validation transport reuses exact committed context without historical Risk or path overrides', () => {
  withFormalFixture((directory) => {
    const git = (changed = {}) => (_command, args) => {
      if (args[0] === 'show') return changed.bytes ?? readFileSync(join(directory, args[1].slice('HEAD:'.length)), 'utf8');
      if (args[0] === 'rev-parse') return args[1] === 'HEAD' ? changed.head ?? formalEnv.GITHUB_SHA : 'b'.repeat(40);
      return changed.status ?? '';
    };
    for (const flag of [{}, { POLARIS_NATIVE_DNS_FORMAL: '1' }]) {
      const env = { ...formalEnv, ...flag, CORE_CANDIDATE: formalEnv.GITHUB_SHA };
      assert.deepEqual(validationContext(formalEnv.GITHUB_SHA, env, directory, git()), { runId: '123', attempt: '1' });
      assert.throws(() => validationContext('c'.repeat(40), env, directory, git()), /candidate differs/);
      for (const changed of [{ head: 'c'.repeat(40) }, { status: ' M Cargo.lock' }, { bytes: '{}' }])
        assert.throws(() => validationContext(formalEnv.GITHUB_SHA, env, directory, git(changed)), /checkout|committed/);
    }
    for (const changed of [{ POLARIS_NATIVE_DNS_FORMAL: '' }, { POLARIS_NATIVE_DNS_FORMAL: '0' },
      { POLARIS_NATIVE_DNS_FORMAL: '1', POLARIS_NATIVE_DNS_CANDIDATE: '1' },
      { POLARIS_NATIVE_PAYLOAD_VALIDATION: '1' }, { POLARIS_SINGBOX_PATH: '/other/core' },
      { GITHUB_REPOSITORY_ID: '1' }, { GITHUB_SHA: 'c'.repeat(40) }, { GITHUB_RUN_ID: '0' },
      { GITHUB_WORKFLOW_REF: formalEnv.GITHUB_WORKFLOW_REF.replace('ci.yml', 'release-risk.yml') },
      { POLARIS_NO_KERNEL_RUN: '1' }])
      assert.throws(() => validationContext(formalEnv.GITHUB_SHA, { ...formalEnv, ...changed }, directory, git()));
  });
  withHistoricalTemporaryFixture((directory) => {
    assert.throws(() => validationContext(formalEnv.GITHUB_SHA, { ...formalEnv, POLARIS_NATIVE_DNS_FORMAL: '1' }, directory),
      /strict upstream formal release/);
  });
  const proof = readFileSync(join(root, 'scripts/native-payload-proof.py'), 'utf8');
  assert.ok(proof.includes("source_root / 'scripts/desktop-core/validation-origin.mjs'"));
  const origin = readFileSync(join(root, 'scripts/desktop-core/validation-origin.mjs'), 'utf8');
  assert.ok(origin.includes('const context = formalGateContext(root, env, run)'));
  assert.ok(origin.includes('context.candidate === candidate'));
  for (const name of ['scripts/desktop-core/validation-origin.mjs', 'scripts/desktop-core/native-dns-gates.mjs'])
    assert.ok(ADAPTER_FILES.includes(name), `Native proof admission dependency must be adapter-bound: ${name}`);
});

test('only the exact native clash child receives its verified native path; formal Cronet stays override-free', () => {
  const env = { ...formalEnv, POLARIS_NATIVE_DNS_FORMAL: '1' };
  for (const key of Object.keys(TARGETS)) {
    const plan = gatePlan(key, root);
    for (const gate of plan) {
      const child = gateEnvironment(gate, key, root, env);
      if (gate.id === 'native_clash_mode') assert.equal(child.POLARIS_SINGBOX_PATH, join(root, TARGETS[key].core));
      else assert.equal(Object.hasOwn(child, 'POLARIS_SINGBOX_PATH'), false, gate.id);
      assert.equal(Object.hasOwn(env, 'POLARIS_SINGBOX_PATH'), false, 'shared environment must not be mutated');
      assert.throws(() => gateEnvironment(gate, key, root, { ...env, POLARIS_SINGBOX_PATH: '' }), /inherit/);
    }
    const clash = plan.find((gate) => gate.id === 'native_clash_mode');
    for (const changed of [{ ...clash, program: 'other' }, { ...clash, args: [...clash.args, '--other'] },
      { ...plan[0], id: 'native_clash_mode' }])
      assert.throws(() => gateEnvironment(changed, key, root, env), /Exact native clash-mode command/);
    const manual = { POLARIS_NATIVE_DNS_CANDIDATE: '1', POLARIS_SINGBOX_PATH: join(root, TARGETS[key].core) };
    assert.deepEqual(gateEnvironment(clash, key, root, manual), manual, 'historical candidate behavior is unchanged');
  }
  const source = readFileSync(join(root, 'scripts/desktop-core/native-dns-gates.mjs'), 'utf8');
  assert.ok(source.includes('executeCommand(gate, { root, env: gateEnvironment(gate, key, root, env), directory,'));
  assert.ok(source.indexOf('const before = await verifyAll()') < source.indexOf('env: gateEnvironment(gate, key, root, env)'));
  const clashSource = readFileSync(join(root, 'crates/singbox-grpc/tests/native_clash_mode.rs'), 'utf8');
  assert.ok(clashSource.includes('std::env::var("POLARIS_SINGBOX_PATH")'));
  assert.ok(clashSource.includes('resources/linux/sing-box'), 'existing Linux fallback is why the child needs the bounded override');
});

test('published replacement is selected only through the complete exact contract', () => {
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
  const pending = JSON.parse(readFileSync(join(root, 'scripts/desktop-core/pending-release-selection.json'))).nextDelivery;
  const current = loadReleaseContract(root, manifest);
  assert.equal(current.sourceCommit, 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e');
  assert.equal(pending.sourceCommit, 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e');
  assert.equal(pending.activationEligible, true);
  assert.equal(pending.releaseId, current.releaseId);
  assert.equal(pending.contractSha256, manifest.desktopRelease.contractSha256);
  assert.equal(pending.tagObject, '8a230c1d34cb4855b261944e2935ce36f9cd14de');
  assert.equal(pending.producerRunId, 40478887755);
  assert.equal(pending.producerRunAttempt, 1);
  assert.equal(pending.originalIndexSha256, '5ddcb1b623492bc555d7fcc0bc17798ea16200df21d2207cfd9a228ffc340caa');
  // Final desktop and mobile share bfcdf96, while the original a4d input remains
  // immutable history and cannot be relabeled as this delivery.
  const historicalMobileBytes = readFileSync(join(root, 'scripts/mobile-core/fixtures/selected-source-contract-a4d.json'));
  assert.equal(digest(historicalMobileBytes), 'd2defda63a5b8609edf8d4e86c5dc9c6c49bd9ad3dbfe437bdc863b6bd2345c0');
  const historicalMobile = JSON.parse(historicalMobileBytes);
  assert.notEqual(historicalMobile.sourceCommit, pending.sourceCommit);
  assert.notEqual(historicalMobile.sourceTree, pending.sourceTree);
  assert.notEqual(historicalMobile.sourceTagObject, pending.tagObject);
  assert.notEqual(pending.mobileSourceContractSha256, digest(historicalMobileBytes));
  const selectedMobileBytes = readFileSync(join(root, 'scripts/mobile-core/source-contract.json'));
  const selectedMobile = JSON.parse(selectedMobileBytes);
  assert.equal(selectedMobile.sourceCommit, 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e');
  assert.equal(selectedMobile.sourceTagObject, '8a230c1d34cb4855b261944e2935ce36f9cd14de');
  assert.equal(selectedMobile.sourceCommit, current.sourceCommit);
  assert.equal(selectedMobile.sourceTree, current.sourceTree);
  assert.equal(selectedMobile.sourceTagObject, current.tagObject);
  assert.deepEqual(selectedMobile.sourcePolicy, current.sourcePolicy);
  assert.equal(digest(selectedMobileBytes), pending.mobileSourceContractSha256);
  assert.equal(selectedMobile.artifactPins, null);
  assert.equal(selectedMobile.producerReceipt, null);
  for (const flag of ['AppAccepted', 'devicePassed', 'publicationEligible']) assert.equal(selectedMobile[flag], false);
  assert.equal(pending.sourceArchive.url, `https://github.com/polaris-arch/sing-box-polaris/archive/${pending.sourceCommit}.tar.gz`);
  assert.equal(pending.sourceArchive.sha256, '70d0bed894f62b557a37c5f3a9211ced21807e135f144138189663ff2fc73566');
  assert.equal(pending.sourceArchive.sha256, current.sourceArchive.sha256);
  const notice = readFileSync(join(root, 'NOTICE'), 'utf8');
  assert.ok(notice.includes(`完整对应源码：${current.sourceArchive.url}`));
  assert.ok(notice.includes(`实际传输 SHA-256：${current.sourceArchive.sha256}`));
  assert.throws(() => loadReleaseContract(root, { ...manifest, desktopRelease: pending }));
  assert.throws(() => loadReleaseContract(root, { ...manifest,
    desktopRelease: { ...manifest.desktopRelease, sourceCommit: 'c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e', sourceTree: 'cf520419eafa63bc789a67f12417b7a82e3d7bd9' } }));
  assert.equal(loadFormalGateSelection(root).sourceCommit, current.sourceCommit,
    'gate and consumer must share the exact active source');
});


test('formal committed-byte proof handles the real multi-megabyte contract within its existing bound', () => {
  const directory = mkdtempSync(join(tmpdir(), 'formal-large-committed-selection-'));
  const env = { ...process.env };
  for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'CORE_CANDIDATE',
    'POLARIS_NATIVE_DNS_CANDIDATE', 'POLARIS_NATIVE_PAYLOAD_VALIDATION', 'POLARIS_SINGBOX_PATH',
    'POLARIS_NATIVE_DNS_FORMAL', 'POLARIS_NO_KERNEL_RUN']) delete env[key];
  const git = (...args) => execFileSync('git', args, { cwd: directory, env, encoding: 'utf8' });
  try {
    for (const name of ['src-tauri/core-manifest.json', 'scripts/desktop-core/release-contract.json',
      'scripts/desktop-core/release-asset-index.json']) {
      mkdirSync(join(directory, name, '..'), { recursive: true });
      copyFileSync(join(root, name), join(directory, name));
    }
    const bytes = readFileSync(join(directory, 'scripts/desktop-core/release-contract.json'));
    assert.ok(bytes.length > 1024 * 1024 && bytes.length <= 8 * 1024 * 1024);
    git('init', '-q'); git('config', 'core.autocrlf', 'false'); git('add', '.');
    git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
      '-c', 'commit.gpgsign=false', 'commit', '-q', '-m', 'Synthetic App checkout with actual committed selection bytes');
    const candidate = git('rev-parse', 'HEAD').trim();
    const oldLimit = spawnSync('git', ['show', 'HEAD:scripts/desktop-core/release-contract.json'],
      { cwd: directory, env, encoding: 'utf8', maxBuffer: 1024 * 1024 });
    assert.equal(oldLimit.error?.code, 'ENOBUFS');
    const context = formalGateContext(directory, { ...env, ...formalEnv, GITHUB_SHA: candidate });
    assert.equal(context.candidate, candidate);
    assert.equal(context.contractSha256, digest(bytes));
    // Keep the entire comparison, including bytes beyond the former truncation.
    const changed = (command, args, options) => {
      assert.equal(options.maxBuffer, 8 * 1024 * 1024);
      const output = execFileSync(command, args, options);
      return args[0] === 'show' && args[1].endsWith('/release-contract.json')
        ? output.slice(0, -1) + ' ' : output;
    };
    assert.throws(() => formalGateContext(directory, { ...env, ...formalEnv, GITHUB_SHA: candidate }, changed), /committed checkout/);
    const oversized = Buffer.concat([bytes, Buffer.alloc(8 * 1024 * 1024 + 1 - bytes.length, ' ')]);
    const manifest = JSON.parse(readFileSync(join(directory, 'src-tauri/core-manifest.json')));
    manifest.desktopRelease.contractSha256 = digest(oversized);
    writeFileSync(join(directory, 'scripts/desktop-core/release-contract.json'), oversized);
    writeFileSync(join(directory, 'src-tauri/core-manifest.json'), JSON.stringify(manifest));
    assert.throws(() => formalGateContext(directory, { ...env, ...formalEnv, GITHUB_SHA: candidate }), /consumer contract SHA differs/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
