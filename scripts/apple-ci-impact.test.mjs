import assert from 'node:assert/strict';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { execFileSync, spawnSync } from 'node:child_process';
import { runInNewContext } from 'node:vm';
import { APPLE_TEST_LEAF, MOBILE_UNIT_TEST_FILES, appleRequired, appleRequiredForReleaseRisk } from './apple-ci-impact.mjs';
import { classifyImpact, classifyNativeImpact, classifyReleaseRiskImpact } from './classify-ci-impact.mjs';

function aggregateScript() {
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  const gate = risk.slice(risk.indexOf('\n  gate:\n'));
  return gate.slice(gate.indexOf('        run: |\n') + '        run: |\n'.length)
    .split('\n').map((line) => line.slice(10)).join('\n');
}

function aggregateEnvironment() {
  return { CLASSIFY_RESULT: 'success', PC_RUNTIME_POLICY_RESULT: 'success', UNREGISTERED: '',
    NATIVE_VALIDATION_REQUIRED: 'false', NATIVE_VALIDATION_RESULT: 'skipped',
    DESKTOP_CORE_REQUIRED: 'false', DESKTOP_CORE_RESULT: 'skipped',
    PREFLIGHT_REQUIRED: 'false', PREFLIGHT_RESULT: 'skipped', PACKAGE_REQUIRED: 'false',
    PACKAGE_RESULT: 'skipped', ANDROID_REQUIRED: 'false', ANDROID_RESULT: 'skipped',
    APPLE_DEFERRED: 'false', APPLE_EVIDENCE_MODE: 'rebuilt' };
}

test('real Apple and shared producer/application edits select the final App leg', () => {
  for (const path of ['src-tauri/gen/apple/project.yml', 'src-tauri/src/lib.rs',
    'crates/tauri-plugin-polaris-ios/ios/Sources/MeasurementDeviceState.swift',
    'scripts/mobile-core/ci_apple.py', 'scripts/mobile-core/apple_rust.py',
    'scripts/mobile-core/apple-cache.py', 'scripts/mobile-core/test_apple_entry.py',
    'scripts/mobile-core/apple_merge_reuse.py', 'scripts/mobile-core/test_apple_merge_reuse.py',
    'src-tauri/capabilities/default.json', 'src-tauri/capabilities/tray.json',
    'scripts/fetch-protoc.mjs', 'scripts/lib/extract-zip.mjs',
    'ui/src/App.tsx', 'Cargo.lock', 'src-tauri/tauri.ios.conf.json', '.github/workflows/apple.yml']) {
    assert.equal(appleRequired([path]), true, path);
  }
  for (const path of ['docs/notes.md', 'src-tauri/gen/android/app/build.gradle.kts',
    'src-tauri/capabilities-not-real/control.json', 'scripts/lib/unrelated.mjs', 'scripts/fetch-protoc-not-real.mjs']) {
    assert.equal(appleRequired([path]), false, path);
  }
  assert.equal(appleRequired([], true), true);
  assert.throws(() => appleRequired(['../caller-input']), /normalized/);
});

test('selected Apple failure/skip is consumed by the existing aggregate gate', () => {
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  assert.match(risk, /uses: \.\/\.github\/workflows\/apple\.yml/);
  assert.match(risk, /needs: \[[^\]]*apple[^\]]*\]/);
  assert.match(risk, /APPLE_REQUIRED.*needs\.classify\.outputs\.apple/);
  assert.match(risk, /APPLE_RESULT.*needs\.apple\.result/);
  assert.match(risk, /"\$APPLE_REQUIRED" = true[^\n]*"\$APPLE_RESULT" != success/);
  assert.match(risk, /node scripts\/apple-ci-impact\.mjs --full/);
});

test('Apple workflow keeps one admitted host, mandatory device and optional isolated simulator output', () => {
  const workflow = readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
  assert.match(workflow, /runs-on: xcode-27/);
  assert.match(workflow, /targets: \$\{\{ inputs.include_simulator && 'aarch64-apple-ios,aarch64-apple-ios-sim' \|\| 'aarch64-apple-ios' \}\}/);
  assert.match(workflow, /POLARIS_APPLE_SCOPE: \$\{\{ inputs.include_simulator && 'device-and-simulator' \|\| 'device-only' \}\}/);
  assert.equal((workflow.match(/include_simulator:\n\s+description:[^\n]+\n\s+type: boolean\n\s+default: false/g) || []).length, 2);
  assert.doesNotMatch(workflow, /Swatinem\/rust-cache|sim-amd64|sim_amd64|x86_64-apple-ios|three-slot|three-slice/);
  for (const action of ['prepare', 'build', 'import-local', 'carrier', 'aggregate', 'evidence', 'retire-slot']) {
    assert.match(workflow, new RegExp(`ci_apple.py ${action}`));
  }
  assert.doesNotMatch(workflow, /\n  carrier:\n/);
  assert.equal((workflow.match(/ci_apple.py build/g) || []).length, 1);
  assert.equal((workflow.match(/ci_apple.py carrier --slot/g) || []).length, 2);
  assert.match(workflow, /unsigned-chain:\n\s+needs: \[resolve, component\]\n\s+if: always\(\)/);
  const blocks = workflow.split(/^ {6}- /m).slice(1);
  for (const [slot, short] of [['device-arm64', 'device'], ['sim-arm64', 'sim']]) {
    const id = slot.replaceAll('-', '_');
    const start = workflow.indexOf('id: ' + id + '\n');
    const original = workflow.indexOf('id: original_' + id + '\n', start);
    const summary = workflow.indexOf('id: summary_' + id + '\n', original);
    const exported = workflow.indexOf('id: rust_' + short + '_export\n', summary);
    const saved = workflow.indexOf('id: rust_' + short + '_save\n', exported);
    const returned = workflow.indexOf('id: rust_' + short + '_return\n', saved);
    const retire = workflow.indexOf('retire-slot --slot "' + slot + '"', returned);
    assert.ok(start >= 0 && original > start && summary > original && exported > summary &&
      saved > exported && returned > saved && retire > returned);
    const exporting = blocks.find(block => block.includes(`id: rust_${short}_export\n`));
    for (const required of [id, 'original_' + id, 'summary_' + id]) {
      assert.ok(exporting.includes(`steps.${required}.conclusion == 'success'`));
    }
    const saving = blocks.find(block => block.includes(`id: rust_${short}_save\n`));
    assert.match(saving, /uses: actions\/cache\/save@v5/);
    assert.ok(saving.includes(`path: target/apple-ci-cache/${slot}`));
    assert.ok(saving.includes(`key: \${{ steps.rust_${short}_restore.outputs.cache-primary-key }}`));
    const returning = blocks.find(block => block.includes(`id: rust_${short}_return\n`));
    assert.match(returning, /!cancelled\(\)/);
    assert.doesNotMatch(returning, /_save.conclusion/);
    const retiring = blocks.find(block => block.includes(`id: retire_${id}\n`));
    assert.ok(retiring.includes(`steps.rust_${short}_export.conclusion == 'skipped' || steps.rust_${short}_return.conclusion == 'success'`));
  }
  const simBlocks = blocks.filter(block => block.includes('--slot "sim-arm64"') ||
    block.includes('path: target/apple-ci-cache/sim-arm64') || /name: apple-carrier-(?:original|summary)-sim-arm64/.test(block));
  assert.equal(simBlocks.length, 11); // Includes the separately conditional aggregate download.
  for (const block of simBlocks) assert.match(block, /if: \$\{\{ inputs.include_simulator(?: && | \}\})/);
  const aggregate = blocks.find(block => block.includes('name: Strict declared-scope'));
  assert.match(aggregate, /COMPONENT_RESULT: \$\{\{ needs.component.result \}\}/);
  assert.match(aggregate, /\[ "\$COMPONENT_RESULT" = success \] \|\| \{\n\s+echo [^\n]+; exit 1; \}/);
  assert.ok(aggregate.indexOf('[ "$COMPONENT_RESULT" = success ]') < aggregate.indexOf('ci_apple.py aggregate'));
  assert.match(aggregate, /aggregate --scope "\$POLARIS_APPLE_SCOPE"/);
  assert.match(aggregate, /sim-arm64: not requested/);
  assert.match(workflow, /contents: read/);
  assert.doesNotMatch(workflow, /contents: write|secrets:|publish_release|codesign --sign/);
  const hook = readFileSync(new URL('../src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj', import.meta.url), 'utf8');
  assert.match(hook, /POLARIS_FORMAL_APP_BUILD/);
  assert.match(hook, /mobile-core\/apple_rust\.py/);
});

test('Cargo snapshot producers and the complete test gate select and verify Python before parsing', () => {
  const apple = readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  const section = (text, name) => text.split(`\n  ${name}:\n`)[1]?.split(/\n  [\w-]+:\n/)[0] ?? '';
  const check = (text, name, consumer, admitted = false) => {
    const job = section(text, name);
    const blocks = job.split(/^ {6}- /m).slice(1);
    const setup = blocks.find(block => block.startsWith('uses: actions/setup-python@v6\n'));
    const probe = blocks.find(block => block.startsWith('name: Verify Python and Cargo configuration parser\n'));
    assert.ok(setup && probe);
    assert.equal(blocks.filter(block => /uses: actions\/setup-python@/.test(block)).length, 1);
    assert.match(setup, /with:\n\s+python-version: '3\.12'/);
    assert.doesNotMatch(setup + probe, /\bif:|continue-on-error:/);
    assert.match(probe, /import sys, tomllib/);
    assert.match(probe, /sys\.version, sys\.executable/);
    assert.match(probe, /tomllib\.__file__/);
    assert.match(probe, /if sys\.version_info\[:2\] != \(3, 12\) or tomllib\.loads\(/);
    assert.match(probe, /raise SystemExit\(/);
    assert.ok(job.indexOf(setup) < job.indexOf(probe));
    assert.ok(job.indexOf(probe) < job.indexOf(consumer));
    if (admitted) assert.ok(job.indexOf('ci_apple.py admit') >= 0 &&
      job.indexOf('ci_apple.py admit') < job.indexOf(setup));
  };
  for (const [text, name, consumer, admitted] of [
    [risk, 'classify', 'run: bash scripts/gate-node-test.sh', false],
    [apple, 'component', 'run: python3 -B scripts/mobile-core/ci_apple.py prepare', true],
  ]) {
    check(text, name, consumer, admitted);
    for (const mutation of [
      text.replace('actions/setup-python@v6', 'actions/setup-python-removed@v6'),
      text.replace("python-version: '3.12'", "python-version: '3.10'"),
      text.replace('uses: actions/setup-python@v6', 'if: false\n        uses: actions/setup-python@v6'),
      text.replace('name: Verify Python and Cargo configuration parser',
        'continue-on-error: true\n        name: Verify Python and Cargo configuration parser'),
      text.replace('import sys, tomllib', 'import sys'),
      text.replace('if sys.version_info[:2] != (3, 12)', 'if False'),
      text.replace('raise SystemExit(', 'print('),
    ]) {
      assert.notEqual(mutation, text);
      assert.throws(() => check(mutation, name, consumer, admitted));
    }
  }
  const moved = apple.replace('      - uses: actions/setup-python@v6',
    '      - name: premature Cargo parsing\n        run: python3 -B scripts/mobile-core/ci_apple.py prepare\n      - uses: actions/setup-python@v6');
  assert.throws(() => check(moved, 'component', 'run: python3 -B scripts/mobile-core/ci_apple.py prepare', true));
});

test('shared failure builds no carriers; optional simulator requires safe device retirement, not device success', () => {
  const workflow = readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
  const blocks = workflow.split(/^ {6}- /m).slice(1);
  const condition = id => blocks.find(block => block.includes(`id: ${id}\n`)).match(/if: \$\{\{ (.+) \}\}/)[1];
  const shared = condition('shared_ready');
  const prerequisites = [...shared.matchAll(/steps\.(\w+)\.conclusion == 'success'/g)].map(m => m[1]);
  for (const required of ['shared_component', 'component_payload', 'component_manifest',
    'component_original', 'local_component', 'protoc', 'rust_cache', 'rust_registry_restore', 'rust_device_ready']) {
    assert.ok(prerequisites.includes(required));
  }
  const ids = [...workflow.matchAll(/id: ([\w_]+)\n/g)].map(match => match[1]);
  const success = Object.fromEntries(ids.map(id => [id, { conclusion: 'success' }]));
  const evaluate = (text, steps, includeSimulator, cancelled = false) => runInNewContext(text, {
    steps, inputs: { include_simulator: includeSimulator }, cancelled: () => cancelled,
    env: { POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: includeSimulator ? 'false' : 'true' },
  });
  const selectedCount = (steps, includeSimulator) => {
    const ready = evaluate(shared, steps, includeSimulator);
    const state = { ...steps, shared_ready: { conclusion: ready ? 'success' : 'skipped' } };
    return ['device_arm64', 'sim_arm64'].filter(id => evaluate(condition(id), state, includeSimulator)).length;
  };
  assert.equal(selectedCount(success, false), 1);
  assert.equal(selectedCount(success, true), 2);
  for (const id of prerequisites) {
    for (const state of ['failure', 'skipped', 'cancelled', '']) {
      assert.equal(selectedCount({ ...success, [id]: { conclusion: state } }, true), 0);
    }
  }
  assert.equal(selectedCount({ ...success, device_arm64: { conclusion: 'failure' } }, true), 2);
  assert.match(condition('rust_sim_restore'), /steps.retire_device_arm64.conclusion == 'success'/);
  for (const state of ['failure', 'cancelled', 'skipped', '']) {
    assert.equal(evaluate(condition('rust_sim_restore'), { ...success, retire_device_arm64: { conclusion: state } }, true), false);
  }
  assert.equal(evaluate(condition('sim_arm64'), success, false), false);
  assert.equal(evaluate(condition('sim_arm64'), success, true, true), false);
});

test('one native host rejects mismatched tools before setup without carrier transport', () => {
  const workflow = readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
  const component = workflow.slice(workflow.indexOf('\n  component:\n'), workflow.indexOf('\n  unsigned-chain:\n'));
  const admission = component.indexOf('ci_apple.py admit');
  assert.ok(admission > component.indexOf('actions/checkout@v7'));
  assert.ok(admission < component.indexOf('actions/setup-go@v7'));
  assert.doesNotMatch(component, /actions\/download-artifact|ci_apple.py import --handoff/);
});

test('actual aggregate shell refuses every selected non-success Apple status', () => {
  const body = aggregateScript();
  const base = aggregateEnvironment();
  const check = (script) => {
    const invoke = (required, result) => spawnSync('bash', ['-c', script], {
      env: { ...process.env, ...base, APPLE_REQUIRED: required, APPLE_RESULT: result }, encoding: 'utf8',
    });
    assert.equal(invoke('true', 'success').status, 0);
    assert.equal(invoke('false', 'skipped').status, 0);
    for (const status of ['failure', 'cancelled', 'skipped', '', 'unknown']) {
      assert.equal(invoke('true', status).status, 1, status);
    }
  };
  check(body);
  const permissive = body.replace('[ "$APPLE_RESULT" != success ]', '[ "$APPLE_RESULT" = failure ]');
  assert.notEqual(permissive, body);
  assert.throws(() => check(permissive));
});

test('temporary Apple deferral is manual branch-only and keeps publication and source scope guards', () => {
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  const input = risk.slice(risk.indexOf('      defer_apple:'), risk.indexOf('\npermissions:'));
  assert.match(input, /type: boolean\n\s+default: false/);
  const validation = risk.split('      - name: Validate manual Apple deferral scope\n')[1]
    .split('      - uses:')[0];
  assert.match(validation, /if: github.event_name == 'workflow_dispatch' && inputs.defer_apple/);
  const script = validation.split('        run: |\n')[1].split('\n').map(line => line.slice(10)).join('\n');
  for (const ref of ['refs/heads/collab/ui01-status-20261010', 'refs/heads/main']) {
    assert.equal(spawnSync('bash', ['-c', script], { env: { ...process.env, VALIDATION_REF: ref } }).status, 0);
  }
  for (const ref of ['refs/tags/v1.0.0', 'refs/tags/anything', '', 'refs/heads/']) {
    assert.equal(spawnSync('bash', ['-c', script], { env: { ...process.env, VALIDATION_REF: ref } }).status, 1);
  }
  assert.match(risk, /if: needs\.classify\.outputs\.apple == 'true' && !\(github\.event_name == 'workflow_dispatch' && inputs\.defer_apple\)/);
  assert.match(risk, /APPLE_REQUIRED: \$\{\{ needs\.classify\.outputs\.apple \}\}/);
  assert.match(risk, /upload_artifacts: false/);
  assert.match(risk, /light_build: true/);
  assert.doesNotMatch(risk, /gh release (create|upload|edit)|download-artifact[^\n]*apple/);
});

test('deferred Apple never passes the full aggregate and never conceals non-Apple failures', () => {
  const script = aggregateScript();
  const invoke = overrides => spawnSync('bash', ['-c', script], { encoding: 'utf8',
    env: { ...process.env, ...aggregateEnvironment(), APPLE_DEFERRED: 'true',
      APPLE_REQUIRED: 'true', APPLE_RESULT: 'skipped', GITHUB_STEP_SUMMARY: '/dev/null', ...overrides } });
  for (const result of ['skipped', 'success', 'failure', 'cancelled', '']) {
    const value = invoke({ APPLE_RESULT: result });
    assert.equal(value.status, 1);
    assert.match(value.stdout, /APPLE_VALIDATION=NOT_VALIDATED/);
    assert.match(value.stdout, /Non-Apple selected legs passed/);
    assert.doesNotMatch(value.stdout, /发布风险门通过：/);
  }
  assert.equal(invoke({ APPLE_REQUIRED: 'false' }).status, 1);
  for (const prefix of ['DESKTOP_CORE', 'PREFLIGHT', 'PACKAGE', 'ANDROID']) {
    for (const result of ['failure', 'skipped', 'cancelled']) {
      const value = invoke({ [`${prefix}_REQUIRED`]: 'true', [`${prefix}_RESULT`]: result });
      assert.equal(value.status, 1);
      assert.doesNotMatch(value.stdout, /Non-Apple selected legs passed/);
    }
  }
  const full = invoke({ APPLE_DEFERRED: 'false', APPLE_RESULT: 'success' });
  assert.equal(full.status, 0);
  assert.doesNotMatch(full.stdout, /NOT_VALIDATED/);
});


function appleWorkflow() {
  return readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
}

function workflowStep(workflow, name) {
  const step = workflow.split(/^ {6}- /m).find(block => block.includes(`name: ${name}\n`));
  assert.ok(step, `missing workflow step: ${name}`);
  return step;
}

function shellBody(step) {
  const marker = '        run: |\n';
  assert.ok(step.includes(marker), 'missing shell block');
  return step.slice(step.indexOf(marker) + marker.length)
    .split('\n').map(line => line.slice(10)).join('\n');
}

function shellFixture(body, env) {
  const directory = mkdtempSync(join(tmpdir(), 'apple-workflow-'));
  const output = join(directory, 'output');
  const summary = join(directory, 'summary');
  try {
    writeFileSync(output, '');
    writeFileSync(summary, '');
    const result = spawnSync('bash', ['-c', body], { encoding: 'utf8',
      env: { ...process.env, ...env, GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary } });
    return { ...result, stdout: result.stdout + readFileSync(output, 'utf8') + readFileSync(summary, 'utf8') };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function expression(text) {
  const match = text.match(/\$\{\{ (.+) \}\}/);
  assert.ok(match, 'missing workflow expression');
  return match[1];
}

test('Apple merge reuse requires explicit main-push opt-in and narrow read-only permissions', () => {
  const workflow = appleWorkflow();
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  const call = workflow.slice(workflow.indexOf('  workflow_call:'), workflow.indexOf('  workflow_dispatch:'));
  const dispatch = workflow.slice(workflow.indexOf('  workflow_dispatch:'), workflow.indexOf('\npermissions:'));
  assert.match(call, /allow_merge_reuse:\n\s+description:[^\n]+\n\s+type: boolean\n\s+default: false/);
  assert.doesNotMatch(dispatch, /allow_merge_reuse/);
  assert.match(call, /value: \$\{\{ jobs.unsigned-chain.outputs.evidence_mode \}\}/);
  const resolver = workflow.slice(workflow.indexOf('\n  resolve:\n'), workflow.indexOf('\n  component:\n'));
  const component = workflow.slice(workflow.indexOf('\n  component:\n'), workflow.indexOf('\n  unsigned-chain:\n'));
  const final = workflow.slice(workflow.indexOf('\n  unsigned-chain:\n'));
  const caller = risk.slice(risk.indexOf('\n  apple:\n'), risk.indexOf('\n  gate:\n'));
  for (const job of [resolver, final, caller]) assert.match(job, /permissions:\n\s+contents: read\n\s+actions: read/);
  assert.doesNotMatch(component, /actions: read|GITHUB_TOKEN:/);
  assert.match(workflow, /\npermissions:\n  contents: read\n\nenv:/);
  assert.equal((workflow.match(/GITHUB_TOKEN:/g) || []).length, 2);
  assert.equal((workflow.match(/ref: \$\{\{ inputs.candidate \|\| github.sha \}\}/g) || []).length, 3);
  assert.doesNotMatch(workflow, /contents: write|actions: write|secrets:|publish_release|codesign --sign|security import/);
  assert.doesNotMatch(caller, /contents: write|secrets:|upload_artifacts|publish_release/);
  const permission = expression(resolver.split('\n').find(line => line.includes('POLARIS_APPLE_ALLOW_MERGE_REUSE:')));
  const callerPermission = expression(caller.split('\n').find(line => line.includes('allow_merge_reuse:')));
  for (const event of ['push', 'pull_request', 'merge_group', 'workflow_dispatch', 'workflow_call', 'schedule']) {
    for (const ref of ['refs/heads/main', 'refs/heads/topic', 'refs/tags/v1.2.3', 'refs/pull/5/merge', '']) {
      for (const allow of [true, false]) {
        for (const simulator of [true, false]) {
          const actual = runInNewContext(permission, { github: { event_name: event, ref },
            inputs: { allow_merge_reuse: allow, include_simulator: simulator } });
          assert.equal(actual, allow && !simulator && event === 'push' && ref === 'refs/heads/main' ? 'true' : 'false');
        }
      }
      for (const native of ['true', 'false']) {
        assert.equal(runInNewContext(callerPermission, { github: { event_name: event, ref },
          needs: { classify: { outputs: { native_validation: native } } } }),
        event === 'push' && ref === 'refs/heads/main' && native !== 'true');
      }
    }
  }
});

test('resolver failures and partial uploads select a full component build without skipped-green', () => {
  const workflow = appleWorkflow();
  const resolver = workflow.slice(workflow.indexOf('\n  resolve:\n'), workflow.indexOf('\n  component:\n'));
  const component = workflow.slice(workflow.indexOf('\n  component:\n'), workflow.indexOf('\n  unsigned-chain:\n'));
  assert.match(component, /needs: resolve\n\s+if: \$\{\{ !cancelled\(\)/);
  const componentIf = expression(component.split('\n').find(line => line.trim().startsWith('if:')));
  const outputIf = expression(resolver.split('\n').find(line => line.trim().startsWith('reused:')));
  const statuses = ['success', 'failure', 'cancelled', 'skipped', ''];
  for (const resolverOutcome of statuses) {
    for (const receiptOutcome of statuses) {
      for (const reused of ['true', 'false', '', 'unknown']) {
        const actual = runInNewContext(outputIf, { steps: {
          resolve: { outcome: resolverOutcome, conclusion: 'success', outputs: { reused } },
          receipt: { outcome: receiptOutcome, conclusion: 'success' },
        } });
        assert.equal(actual, resolverOutcome === 'success' && receiptOutcome === 'success' && reused === 'true' ? 'true' : 'false');
      }
    }
  }
  for (const result of statuses) {
    for (const reused of ['true', 'false', '', 'unknown']) {
      for (const cancelled of [false, true]) {
        assert.equal(runInNewContext(componentIf, { cancelled: () => cancelled,
          needs: { resolve: { result, outputs: { reused } } } }),
        !cancelled && !(result === 'success' && reused === 'true'));
      }
    }
  }
  assert.match(resolver, /id: resolve\n\s+continue-on-error: true/);
  assert.match(resolver, /id: receipt\n\s+if: \$\{\{ steps.resolve.outcome == 'success' && steps.resolve.outputs.reused == 'true' \}\}\n\s+continue-on-error: true/);
  assert.equal((workflow.match(/continue-on-error: true/g) || []).length, 2);
  assert.match(resolver, /receipt_sha256: \$\{\{ steps.resolve.outputs.receipt_sha256 \}\}/);
  const verification = workflowStep(workflow, 'Verify same-tree evidence receipt against the current candidate and context');
  assert.match(verification, /RECEIPT_SHA256: \$\{\{ needs.resolve.outputs.receipt_sha256 \}\}/);
  assert.equal(expression(verification.split('\n').find(line => line.includes('POLARIS_APPLE_ALLOW_MERGE_REUSE:'))),
    expression(resolver.split('\n').find(line => line.includes('POLARIS_APPLE_ALLOW_MERGE_REUSE:'))));
  assert.match(verification, /check-result --receipt "\$RUNNER_TEMP\/apple-merge-reuse\/apple-merge-reuse.json" --expected-sha "\$RECEIPT_SHA256"/);
});

test('only Release Risk device-only PR snapshots bracket real work and seal uploaded originals after the chain', () => {
  const workflow = appleWorkflow();
  const blocks = workflow.split(/^ {6}- /m).slice(1);
  const commandIndex = command => blocks.findIndex(block => block.includes(command));
  const frontend = commandIndex('ci_apple.py build');
  assert.equal(commandIndex('snapshot --phase frontend-before'), frontend - 1);
  assert.equal(commandIndex('snapshot --phase frontend-after'), frontend + 1);
  const carrier = commandIndex('ci_apple.py carrier --slot "device-arm64"');
  assert.equal(commandIndex('snapshot --phase carrier-before --slot device-arm64'), carrier - 1);
  assert.equal(commandIndex('snapshot --phase carrier-after --slot device-arm64'), carrier + 1);
  assert.equal(commandIndex('ci_apple.py evidence --slot "device-arm64"'), carrier + 2);
  assert.ok(commandIndex('snapshot --phase frontend-after') < commandIndex('ci_apple.py evidence\n'));
  const snapshots = blocks.filter(block => block.includes('apple_merge_reuse.py snapshot'));
  assert.equal(snapshots.length, 4);
  for (const snapshot of snapshots) assert.match(snapshot, /if: \$\{\{ env.POLARIS_APPLE_PRODUCE_REUSE_CONTRACT == 'true'/);
  assert.match(blocks[carrier], /env.POLARIS_APPLE_PRODUCE_REUSE_CONTRACT != 'true' \|\| steps.carrier_before.conclusion == 'success'/);
  const chain = blocks.findIndex(block => block.includes('id: chain\n'));
  const seal = blocks.findIndex(block => block.includes('id: execution_contract\n'));
  const upload = blocks.findIndex(block => block.includes('id: execution_contract_upload\n'));
  assert.ok(chain >= 0 && seal === chain + 1 && upload === seal + 1);
  assert.match(blocks[seal], /env.POLARIS_APPLE_PRODUCE_REUSE_CONTRACT == 'true' && !cancelled\(\) && steps.aggregate.conclusion == 'success' && steps.chain.conclusion == 'success'/);
  assert.match(blocks[seal], /seal --summaries "\$RUNNER_TEMP\/apple-chain" --chain "\$RUNNER_TEMP\/apple-chain.json" --output "\$RUNNER_TEMP\/apple-execution-contract.json"/);
  assert.match(blocks[upload], /env.POLARIS_APPLE_PRODUCE_REUSE_CONTRACT == 'true' && !cancelled\(\) && steps.execution_contract.conclusion == 'success'/);
  assert.match(blocks[upload], /name: apple-execution-contract-\$\{\{ github.run_id \}\}-\$\{\{ github.run_attempt \}\}/);
  for (const block of [blocks[chain], blocks[upload]]) assert.match(block, /if-no-files-found: error/);
  assert.match(blocks[chain], /always\(\) && !\(needs.resolve.result == 'success' && needs.resolve.outputs.reused == 'true'\)/);
  const requirement = expression(workflow.split('\n').find(line => line.startsWith('  POLARIS_APPLE_PRODUCE_REUSE_CONTRACT:')));
  const riskRef = 'owner/repo/.github/workflows/release-risk.yml@refs/pull/7/merge';
  const appleRef = 'owner/repo/.github/workflows/apple.yml@refs/heads/main';
  const finalScript = shellBody(workflowStep(workflow, 'Require verified reuse or the complete current-attempt build chain'));
  const readySteps = Object.fromEntries([...workflow.matchAll(/id: ([\w_]+)\n/g)]
    .map(match => [match[1], { conclusion: 'success', outputs: { eligible: 'true' } }]));
  for (const [label, event, ref, simulator, expected] of [
    ['Release Risk PR producer', 'pull_request', riskRef, false, 'true'],
    ['standalone dispatch', 'workflow_dispatch', appleRef, false, 'false'],
    ['Release Risk dispatch', 'workflow_dispatch', riskRef, false, 'false'],
    ['main fallback', 'push', riskRef.replace('refs/pull/7/merge', 'refs/heads/main'), false, 'false'],
    ['merge group', 'merge_group', riskRef, false, 'false'],
    ['simulator PR', 'pull_request', riskRef, true, 'false'],
    ['standalone simulator', 'workflow_dispatch', appleRef, true, 'false'],
    ['other caller PR', 'pull_request', appleRef, false, 'false'],
    ['unknown caller PR', 'pull_request', '', false, 'false'],
    ['similarly named caller', 'pull_request', riskRef.replace('release-risk.yml@', 'release-risk.yml.bak@'), false, 'false'],
    ['pull request target', 'pull_request_target', riskRef, false, 'false'],
  ]) {
    const flag = runInNewContext(requirement, { github: { event_name: event, workflow_ref: ref },
      inputs: { include_simulator: simulator }, contains: (haystack, needle) => haystack.includes(needle) });
    assert.equal(flag, expected, label);
    const context = { env: { POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: flag },
      inputs: { include_simulator: simulator }, cancelled: () => false, steps: readySteps };
    for (const block of [...snapshots, blocks[seal], blocks[upload]]) {
      const condition = expression(block.split('\n').find(line => line.trim().startsWith('if:')));
      assert.equal(runInNewContext(condition, context), expected === 'true', label);
    }
    const carrierCondition = expression(blocks[carrier].split('\n').find(line => line.trim().startsWith('if:')));
    assert.equal(runInNewContext(carrierCondition, { ...context, steps: {
      ...readySteps, carrier_before: { conclusion: 'skipped' },
    } }), expected !== 'true', label);
    const gateEnvironment = { POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: flag,
      POLARIS_APPLE_SCOPE: simulator ? 'device-and-simulator' : 'device-only',
      RESOLVE_RESULT: label === 'main fallback' ? 'failure' : 'success', REUSED: 'false',
      COMPONENT_RESULT: 'success', REUSE_VERIFIED: 'skipped', AGGREGATE_RESULT: 'success', CHAIN_RESULT: 'success',
      CONTRACT_RESULT: expected === 'true' ? 'success' : 'skipped',
      CONTRACT_ELIGIBLE: expected === 'true' ? 'true' : '',
      CONTRACT_UPLOAD_RESULT: expected === 'true' ? 'success' : 'skipped' };
    assert.equal(shellFixture(finalScript, gateEnvironment).status, 0, label);
    if (expected === 'true') {
      assert.equal(shellFixture(finalScript, { ...gateEnvironment,
        CONTRACT_RESULT: 'skipped', CONTRACT_UPLOAD_RESULT: 'skipped' }).status, 1, label);
    }
  }
});

test('actual unsigned-chain result shell accepts only verified reuse or a complete full build', () => {
  const workflow = appleWorkflow();
  const step = workflowStep(workflow, 'Require verified reuse or the complete current-attempt build chain');
  assert.match(step, /id: result\n\s+if: always\(\)/);
  const script = shellBody(step);
  const rebuilt = { RESOLVE_RESULT: 'success', REUSED: 'false', COMPONENT_RESULT: 'success',
    REUSE_VERIFIED: 'skipped', AGGREGATE_RESULT: 'success', CHAIN_RESULT: 'success',
    CONTRACT_RESULT: 'success', CONTRACT_ELIGIBLE: 'true', CONTRACT_UPLOAD_RESULT: 'success', POLARIS_APPLE_SCOPE: 'device-only',
    POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: 'true' };
  const reused = { ...rebuilt, REUSED: 'true', COMPONENT_RESULT: 'skipped', REUSE_VERIFIED: 'success',
    AGGREGATE_RESULT: 'skipped', CHAIN_RESULT: 'skipped', CONTRACT_RESULT: 'skipped', CONTRACT_UPLOAD_RESULT: 'skipped',
    POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: 'false' };
  const ordinary = { ...rebuilt, POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: 'false',
    CONTRACT_RESULT: 'skipped', CONTRACT_UPLOAD_RESULT: 'skipped' };
  const simulator = { ...ordinary, POLARIS_APPLE_SCOPE: 'device-and-simulator' };
  const ineligible = { ...rebuilt, CONTRACT_ELIGIBLE: 'false', CONTRACT_UPLOAD_RESULT: 'skipped' };
  const invoke = shellFixture;
  const check = body => {
    for (const [mode, env] of [['rebuilt', rebuilt], ['reused', reused], ['rebuilt', ordinary], ['rebuilt', simulator], ['rebuilt', ineligible]]) {
      const result = invoke(body, env);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.match(result.stdout, new RegExp(`evidence_mode=${mode}`));
      for (const key of ['COMPONENT_RESULT', 'REUSE_VERIFIED', 'AGGREGATE_RESULT', 'CHAIN_RESULT', 'CONTRACT_RESULT', 'CONTRACT_UPLOAD_RESULT']) {
        for (const status of ['success', 'failure', 'cancelled', 'skipped', '', 'unknown'].filter(value => value !== env[key])) {
          const invalid = invoke(body, { ...env, [key]: status });
          assert.equal(invalid.status, 1, `${mode} ${key}=${status}: ${invalid.stdout}${invalid.stderr}`);
          assert.doesNotMatch(invalid.stdout, /evidence_mode=/);
        }
      }
    }
    for (const result of ['failure', 'cancelled', 'skipped', '']) {
      assert.equal(invoke(body, { ...reused, RESOLVE_RESULT: result }).status, 1);
      assert.equal(invoke(body, { ...rebuilt, RESOLVE_RESULT: result }).status, 0);
      assert.equal(invoke(body, { ...rebuilt, RESOLVE_RESULT: result, REUSED: 'true' }).status, 0);
    }
    for (const reusedValue of ['false', '', 'unknown']) {
      assert.equal(invoke(body, { ...reused, REUSED: reusedValue }).status, 1);
    }
    for (const scope of ['device-and-simulator', '', 'unknown']) {
      assert.equal(invoke(body, { ...reused, POLARIS_APPLE_SCOPE: scope }).status, 1);
    }
    for (const eligibility of ['', 'unknown', 'false']) {
      assert.equal(invoke(body, { ...rebuilt, CONTRACT_ELIGIBLE: eligibility }).status, 1);
    }
    for (const eligibility of ['', 'unknown', 'true']) {
      assert.equal(invoke(body, { ...ineligible, CONTRACT_ELIGIBLE: eligibility }).status, 1);
    }
    assert.equal(invoke(body, { ...rebuilt, POLARIS_APPLE_SCOPE: 'unknown' }).status, 1);
    assert.equal(invoke(body, { ...rebuilt, POLARIS_APPLE_SCOPE: 'device-and-simulator' }).status, 1);
    assert.equal(invoke(body, { ...reused, POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: 'true' }).status, 1);
    assert.equal(invoke(body, { ...ordinary, POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: 'true' }).status, 1);
    assert.equal(invoke(body, { ...rebuilt, POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: 'false' }).status, 1);
    for (const requirement of ['', 'unknown']) {
      assert.equal(invoke(body, { ...ordinary, POLARIS_APPLE_PRODUCE_REUSE_CONTRACT: requirement }).status, 1);
    }
    assert.equal(invoke(body, { ...reused, COMPONENT_RESULT: 'success' }).status, 1);
  };
  check(script);
  for (const mutation of [script.replace('[ "$REUSE_VERIFIED" = success ]', 'true'),
    script.replace('[ "$COMPONENT_RESULT" = skipped ]', 'true'),
    script.replace('[ "$CONTRACT_UPLOAD_RESULT" = success ]', 'true')]) {
    assert.notEqual(mutation, script);
    assert.throws(() => check(mutation));
  }
});

test('Release Risk reports rebuilt versus reused Apple evidence and rejects missing or incompatible modes', () => {
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  assert.match(risk, /APPLE_EVIDENCE_MODE: \$\{\{ needs.apple.outputs.evidence_mode \}\}/);
  const script = aggregateScript();
  const invoke = overrides => shellFixture(script, {
    ...aggregateEnvironment(), APPLE_REQUIRED: 'true', APPLE_RESULT: 'success',
    APPLE_INCLUDE_SIMULATOR: 'false', ...overrides,
  });
  const rebuilt = invoke({ APPLE_EVIDENCE_MODE: 'rebuilt' });
  assert.equal(rebuilt.status, 0);
  assert.match(rebuilt.stdout, /compile\/link rebuilt and passed in this run/);
  assert.doesNotMatch(rebuilt.stdout, /reused verified/);
  const reused = invoke({ APPLE_EVIDENCE_MODE: 'reused' });
  assert.equal(reused.status, 0);
  assert.match(reused.stdout, /reused verified same-tree PR/);
  assert.match(reused.stdout, /no new compilation in this run/);
  assert.doesNotMatch(reused.stdout, /rebuilt and passed/);
  for (const mode of ['', 'unknown', 'success', 'false']) {
    const result = invoke({ APPLE_EVIDENCE_MODE: mode });
    assert.equal(result.status, 1);
    assert.doesNotMatch(result.stdout, /发布风险门通过：/);
  }
  assert.equal(invoke({ APPLE_EVIDENCE_MODE: 'reused', APPLE_INCLUDE_SIMULATOR: 'true' }).status, 1);
  assert.equal(invoke({ APPLE_EVIDENCE_MODE: 'rebuilt', APPLE_INCLUDE_SIMULATOR: 'true' }).status, 0);
  assert.equal(invoke({ APPLE_REQUIRED: 'false', APPLE_RESULT: 'skipped', APPLE_EVIDENCE_MODE: '' }).status, 0);
});


const reviewedOwner = 'src-tauri/src/runtime/proxy/mesh_apply/candidate/check.rs';
const ownerBlob = '5c38100fa91a246ed00606832af675b4684c14ea';
const linuxBoundary = '#[cfg(all(target_os = "linux", target_arch = "x86_64"))]\nmod linux_tests {\n';
function leafContext() {
  const before = 'a'.repeat(40), after = 'b'.repeat(40);
  return { env: { GITHUB_EVENT_NAME: 'push', GITHUB_REF: 'refs/heads/main',
    GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_WORKFLOW: 'Release Risk',
    GITHUB_WORKFLOW_REF: 'polaris-arch/Polaris/.github/workflows/release-risk.yml@refs/heads/main',
    GITHUB_SHA: after }, event: { before, after, forced: false, created: false, deleted: false } };
}
function leafGit(context, overrides = {}) {
  return (command, args) => {
    assert.equal(command, 'git');
    if (args[0] === 'rev-parse' && args[1] === 'HEAD') return overrides.head ?? context.event.after;
    if (args[0] === 'cat-file' && args[1] === '-e') {
      if (overrides.missingBase) throw new Error('missing base');
      return '';
    }
    if (args[0] === 'merge-base') {
      if (overrides.nonAncestor) throw new Error('not ancestor');
      return '';
    }
    if (args[0] === 'diff') return overrides.diff ?? `:100755 100755 ${'c'.repeat(40)} ${'d'.repeat(40)} M\0${APPLE_TEST_LEAF}\0`;
    if (args[0] === 'ls-tree') return `${overrides.ownerMode ?? '100755'} blob ${overrides.owner ?? ownerBlob}\t${reviewedOwner}\0`;
    if (args[0] === 'cat-file' && args[1] === 'blob') return overrides.content ?? linuxBoundary + '}\n';
    throw new Error('unexpected git call: ' + args.join(' '));
  };
}

test('one audited existing Linux test leaf can omit Apple, never native Rust', async () => {
  const context = leafContext();
  assert.equal(appleRequired([APPLE_TEST_LEAF]), true); // Legacy path-only API stays conservative.
  assert.equal(appleRequiredForReleaseRisk([APPLE_TEST_LEAF], context.env, context.event, leafGit(context)), false);
  const { classifyNativeImpact } = await import('./classify-ci-impact.mjs');
  assert.equal(classifyNativeImpact(leafGit(context)('git', ['diff'])).nativeRequired, true);
  const owner = readFileSync(new URL('../' + reviewedOwner, import.meta.url));
  // Ordinary owner edits merely disable the runtime exemption; no exact-blob
  // assertion blocks unrelated production development.
  assert.match(owner.toString(), /#\[cfg\(test\)\]\nmod tests;/);
  assert.ok(readFileSync(new URL('../' + APPLE_TEST_LEAF, import.meta.url), 'utf8').startsWith(linuxBoundary));
  // Reviewed source-input contract: build.rs must not acquire this test leaf as
  // an input. A future production include/build dependency needs routing review.
  const build = readFileSync(new URL('../src-tauri/build.rs', import.meta.url), 'utf8');
  assert.doesNotMatch(build, /candidate\/check\/tests|check\/tests\/mod\.rs/);
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  assert.match(risk, /git diff --name-only -z "\$base" "\$head" \| node scripts\/apple-ci-impact\.mjs --release-risk/);
});

test('test-leaf routing rejects unsafe or unregistered diff and source boundaries', () => {
  const c = leafContext(), valid = leafGit(c)('git', ['diff']);
  const check = overrides => appleRequiredForReleaseRisk([APPLE_TEST_LEAF], c.env, c.event, leafGit(c, overrides));
  for (const diff of ['', valid.slice(0, -1), valid + valid,
    valid.replace(' M\0', ' A\0'), valid.replace(' M\0', ' D\0'), valid.replace(' M\0', ' R100\0'),
    valid.replace(' M\0', ' C100\0'), valid.replace(' M\0', ' T\0'),
    valid.replace(':100755 100755', ':100644 100755'), valid.replace(':100755 100755', ':100755 100644'),
    valid.replace(':100755 100755', ':120000 120000'), valid.replace(':100755 100755', ':160000 160000'),
    valid.replace('c'.repeat(40), '0'.repeat(40)), valid.replace('d'.repeat(40), 'c'.repeat(40)),
    valid.replace(APPLE_TEST_LEAF, 'src-tauri/src/lib.rs'), valid.replace(APPLE_TEST_LEAF, '../' + APPLE_TEST_LEAF),
    valid + `:100644 100644 ${'c'.repeat(40)} ${'d'.repeat(40)} M\0src-tauri/build.rs\0`]) {
    assert.equal(check({ diff }), true, JSON.stringify(diff));
  }
  for (const overrides of [{ owner: 'e'.repeat(40) }, { ownerMode: '120000' }, { content: '// cfg removed\n' },
    { content: linuxBoundary.replace('linux', 'macos') }, { missingBase: true }, { nonAncestor: true },
    { head: 'e'.repeat(40) }]) assert.equal(check(overrides), true);
  for (const extra of [reviewedOwner, 'src-tauri/build.rs', 'src-tauri/src/lib.rs', 'Cargo.toml',
    '.github/workflows/apple.yml', 'src-tauri/src/another/tests/mod.rs']) {
    assert.equal(appleRequiredForReleaseRisk([APPLE_TEST_LEAF, extra], c.env, c.event, leafGit(c)), true);
  }
});

test('only reliable direct ordinary main push can use the leaf contract', () => {
  const c = leafContext();
  for (const env of [{ GITHUB_EVENT_NAME: 'pull_request' }, { GITHUB_EVENT_NAME: 'merge_group' },
    { GITHUB_EVENT_NAME: 'workflow_dispatch' }, { GITHUB_REF: 'refs/heads/topic' },
    { GITHUB_WORKFLOW: 'CI' }, { GITHUB_WORKFLOW_REF: '' }, { GITHUB_SHA: 'e'.repeat(40) },
    { GITHUB_WORKFLOW_REF: c.env.GITHUB_WORKFLOW_REF.replace('release-risk.yml@', 'apple.yml@') }]) {
    assert.equal(appleRequiredForReleaseRisk([APPLE_TEST_LEAF], { ...c.env, ...env }, c.event, leafGit(c)), true);
  }
  for (const event of [null, {}, { ...c.event, forced: true }, { ...c.event, created: true },
    { ...c.event, deleted: true }, { ...c.event, forced: undefined },
    { ...c.event, before: '0'.repeat(40) }, { ...c.event, before: c.event.after },
    { ...c.event, after: 'unknown' }]) {
    assert.equal(appleRequiredForReleaseRisk([APPLE_TEST_LEAF], c.env, event, leafGit(c)), true);
  }
});

test('actual Git/CLI leaf exception, full override and legacy invocation stay compatible', () => {
  const dir = mkdtempSync(join(tmpdir(), 'apple-leaf-route-'));
  const script = fileURLToPath(new URL('./apple-ci-impact.mjs', import.meta.url));
  const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8' }).trim();
  try {
    git('init', '-q'); git('config', 'user.name', 'fixture'); git('config', 'user.email', 'fixture@example.invalid');
    mkdirSync(dirname(join(dir, APPLE_TEST_LEAF)), { recursive: true });
    writeFileSync(join(dir, reviewedOwner), readFileSync(new URL('../' + reviewedOwner, import.meta.url)));
    chmodSync(join(dir, reviewedOwner), 0o755);
    writeFileSync(join(dir, APPLE_TEST_LEAF), linuxBoundary + '}\n'); chmodSync(join(dir, APPLE_TEST_LEAF), 0o755);
    git('add', '.'); git('commit', '-qm', 'fixture base');
    const before = git('rev-parse', 'HEAD');
    writeFileSync(join(dir, APPLE_TEST_LEAF), linuxBoundary + '    // test fixture only\n}\n');
    git('add', '.'); git('commit', '-qm', 'fixture edit');
    const after = git('rev-parse', 'HEAD'), c = leafContext();
    const ownerCurrent = git('rev-parse', `${after}:${reviewedOwner}`) === ownerBlob;
    const eventPath = join(dir, 'event.json');
    writeFileSync(eventPath, JSON.stringify({ ...c.event, before, after }));
    const invoke = (args, env = {}) => spawnSync(process.execPath, [script, ...args], { cwd: dir,
      input: APPLE_TEST_LEAF + '\0', encoding: 'utf8',
      env: { ...process.env, ...c.env, GITHUB_SHA: after, GITHUB_EVENT_PATH: eventPath, ...env } });
    for (const [args, expected] of [[[], 'true'], [['--release-risk'], ownerCurrent ? 'false' : 'true'],
      [['--release-risk', '--full'], 'true'], [['--full'], 'true']]) {
      const result = invoke(args); assert.equal(result.status, 0, result.stderr); assert.equal(result.stdout.trim(), expected);
    }
    assert.equal(invoke(['--release-risk'], { GITHUB_EVENT_PATH: join(dir, 'missing') }).stdout.trim(), 'true');
    writeFileSync(eventPath, '{broken');
    assert.equal(invoke(['--release-risk']).stdout.trim(), 'true');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

// Frozen fixtures model the reviewed owner versions. Current owner edits are
// allowed and simply disable optimization, including in the real CLI test below.
const mobileOwners = [
  ['scripts/mobile-core/producer.py', '100644', '38c4e3e7804feb181cc17319a45298d4d853a92a'],
  ['scripts/mobile-source-unit.test.mjs', '100644', '8cc09d5f633f4f3c2eb6a4246f79644773b5681e'],
  ['scripts/gate-node-test.sh', '100755', '52e873e5cc95361c6723318d398caeb4db442664'],
  ['.github/workflows/release-risk.yml', '100644', '8424964ac3edd11cfe182b852dabae405b7b70ad'],
  ['.github/workflows/ci.yml', '100644', '9407e92d0fbc36897b7915e45511ade217bc39fa'],
  ['src-tauri/tests/test_source_anchors.rs', '100644', '18667d599e46b5fb5cc354642a4bd86528419524'],
];
const mobilePath = 'scripts/mobile-core/test_apple_merge_reuse.py';
const mobileRaw = paths => paths.map(path => `:100644 100644 ${'c'.repeat(40)} ${'d'.repeat(40)} M\0${path}\0`).join('');
function mobileGit(c, paths = [mobilePath], overrides = {}) {
  const fallback = leafGit(c, { ...overrides, diff: overrides.diff ?? mobileRaw(paths) });
  return (command, args, options) => {
    if (args[0] === 'ls-tree') {
      const owner = mobileOwners.find(([path]) => path === args[4]);
      assert.ok(owner, 'only the six reviewed owners are read');
      if (overrides.owner === owner[0] && (!overrides.ref || overrides.ref === args[2])) return '';
      return `${owner[1]} blob ${owner[2]}\t${owner[0]}\0`;
    }
    if (overrides.fail === args[0]) throw new Error('unavailable history/output budget');
    return fallback(command, args, options);
  };
}
function mobileDecision(paths, c = leafContext(), run = mobileGit(c, paths)) {
  return [appleRequiredForReleaseRisk(paths, c.env, c.event, run),
    classifyReleaseRiskImpact(paths, c.env, c.event, run).android];
}

test('reviewed existing Python unittest content omits both mobile builds but retains native CI', () => {
  const c = leafContext();
  assert.equal(MOBILE_UNIT_TEST_FILES.length, 14);
  for (const paths of [...MOBILE_UNIT_TEST_FILES.map(path => [path]), [...MOBILE_UNIT_TEST_FILES].reverse()]) {
    assert.deepEqual(mobileDecision(paths), [false, false]);
    assert.equal(classifyNativeImpact(mobileRaw(paths)).nativeRequired, true);
    assert.equal(appleRequired(paths), true);
    assert.equal(classifyImpact(paths).android, true);
    assert.deepEqual({ ...classifyReleaseRiskImpact(paths, c.env, c.event, mobileGit(c, paths)), android: true },
      classifyImpact(paths));
  }
});

test('Python routing rejects source modes, added/deleted/renamed/unknown files and mixed changes', () => {
  const c = leafContext(), valid = mobileRaw([mobilePath]);
  for (const diff of ['', valid.slice(0, -1), valid + valid,
    ...['A', 'D', 'R100', 'C100', 'T', 'U', 'M100'].map(status => valid.replace(' M\0', ` ${status}\0`)),
    ...['100644 100755', '100755 100644', '100755 100755', '120000 120000', '160000 160000', '000000 100644']
      .map(modes => valid.replace('100644 100644', modes)),
    valid.replace('c'.repeat(40), '0'.repeat(40)), valid.replace('d'.repeat(40), 'c'.repeat(40)),
    valid.replace(mobilePath, '../' + mobilePath), valid.replace(mobilePath, mobilePath + '\n'),
    valid + mobileRaw(['scripts/mobile-core/producer.py'])]) {
    assert.deepEqual(mobileDecision([mobilePath], c, mobileGit(c, [mobilePath], { diff })), [true, true], JSON.stringify(diff));
  }
  for (const extra of ['scripts/mobile-core/test_unknown.py', 'scripts/mobile-core/nested/test_inputs.py',
    'scripts/mobile-core/test_apple_cargo_timing.py', 'scripts/mobile-core/ci_apple.py',
    'scripts/mobile-core/producer.py', 'src-tauri/src/lib.rs', 'Cargo.toml', '.cargo/config.toml',
    '.github/workflows/release-risk.yml', 'scripts/apple-ci-impact.mjs', 'scripts/classify-ci-impact.mjs',
    'scripts/apple-ci-impact.test.mjs', 'scripts/gate-node-test.sh', 'scripts/mobile-source-unit.test.mjs']) {
    assert.deepEqual(mobileDecision([mobilePath, extra]), [true, true], extra);
  }
  assert.deepEqual(mobileDecision([mobilePath, mobilePath]), [true, true]);
  for (const rule of ['scripts/apple-ci-impact.mjs', 'scripts/classify-ci-impact.mjs', 'scripts/apple-ci-impact.test.mjs']) {
    assert.deepEqual(mobileDecision([rule]), [true, true], 'routing contract edits retain both builds');
  }
  assert.deepEqual(mobileDecision([]), [false, false]); // Preserve an already-unselected result.
  assert.deepEqual(mobileDecision(['docs/README.md']), [false, false]);
});

test('mobile routing requires reliable ordinary direct Release Risk main push and unchanged owners', () => {
  const c = leafContext();
  for (const env of [{ GITHUB_EVENT_NAME: 'pull_request' }, { GITHUB_EVENT_NAME: 'merge_group' },
    { GITHUB_EVENT_NAME: 'workflow_dispatch' }, { GITHUB_EVENT_NAME: 'workflow_call' },
    { GITHUB_REF: 'refs/heads/topic' }, { GITHUB_REF: 'refs/tags/v1' }, { GITHUB_WORKFLOW: 'CI' },
    { GITHUB_WORKFLOW_REF: '' }, { GITHUB_REPOSITORY: 'other/repo' }, { GITHUB_SHA: 'e'.repeat(40) },
    { GITHUB_WORKFLOW_REF: c.env.GITHUB_WORKFLOW_REF.replace('release-risk.yml@', 'apple.yml@') }]) {
    assert.deepEqual(mobileDecision([mobilePath], { ...c, env: { ...c.env, ...env } }), [true, true]);
  }
  for (const event of [null, {}, { ...c.event, forced: true }, { ...c.event, created: true },
    { ...c.event, deleted: true }, { ...c.event, forced: undefined },
    { ...c.event, before: '0'.repeat(40) }, { ...c.event, before: c.event.after },
    { ...c.event, after: 'unknown' }]) {
    assert.deepEqual(mobileDecision([mobilePath], { ...c, event }, mobileGit(c)), [true, true]);
  }
  for (const [owner] of mobileOwners) {
    for (const ref of [c.event.before, c.event.after]) {
      assert.deepEqual(mobileDecision([mobilePath], c, mobileGit(c, [mobilePath], { owner, ref })), [true, true]);
    }
  }
  for (const overrides of [{ missingBase: true }, { nonAncestor: true }, { head: 'e'.repeat(40) },
    ...['rev-parse', 'cat-file', 'merge-base', 'diff'].map(fail => ({ fail }))]) {
    assert.deepEqual(mobileDecision([mobilePath], c, mobileGit(c, [mobilePath], overrides)), [true, true]);
  }
});

test('Python discovery and naming remain existing gates, separate from producer input inventory', () => {
  const source = path => readFileSync(new URL('../' + path, import.meta.url), 'utf8');
  const producer = source('scripts/mobile-core/producer.py');
  const inventory = producer.slice(producer.indexOf('PRODUCER_FILES ='), producer.indexOf('ANDROID_BUILD_CODE_POLICY ='));
  const naming = source('src-tauri/tests/test_source_anchors.rs');
  const registry = naming.slice(naming.indexOf('const PYTHON_MODULE_FILES:'), naming.indexOf('fn registered_python_module'));
  for (const path of MOBILE_UNIT_TEST_FILES) {
    assert.doesNotMatch(inventory, new RegExp(path.replaceAll('.', '\\.')));
    assert.ok(registry.includes(`"${path}"`));
  }
  assert.match(source('scripts/mobile-source-unit.test.mjs'), /'unittest', 'discover', '-s', 'scripts\/mobile-core', '-p', 'test_\*\.py'/);
  assert.match(source('scripts/gate-node-test.sh'), /scripts\/mobile-source-unit\.test\.mjs/);
  const risk = source('.github/workflows/release-risk.yml');
  assert.ok(risk.indexOf('run: bash scripts/gate-node-test.sh') < risk.indexOf('- name: Classify changed paths'));
  assert.match(source('scripts/classify-ci-impact.mjs'), /classifyNativeImpact/);
});

test('actual Git and both CLIs agree; force-full, invalid context and owner evolution retain original decisions', () => {
  const dir = mkdtempSync(join(tmpdir(), 'mobile-unit-route-'));
  const script = name => fileURLToPath(new URL('./' + name, import.meta.url));
  const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8' }).trim();
  const c = leafContext();
  try {
    git('init', '-q'); git('config', 'user.name', 'fixture'); git('config', 'user.email', 'fixture@example.invalid');
    for (const [path, mode] of mobileOwners) {
      mkdirSync(dirname(join(dir, path)), { recursive: true });
      writeFileSync(join(dir, path), readFileSync(new URL('../' + path, import.meta.url)));
      chmodSync(join(dir, path), mode === '100755' ? 0o755 : 0o644);
    }
    writeFileSync(join(dir, mobilePath), '# existing unittest fixture\n');
    git('add', '.'); git('commit', '-qm', 'base');
    const before = git('rev-parse', 'HEAD');
    writeFileSync(join(dir, mobilePath), '# modified unittest fixture\n');
    git('add', '.'); git('commit', '-qm', 'edit');
    let after = git('rev-parse', 'HEAD');
    const ownersCurrent = mobileOwners.every(([path, , blob]) => git('rev-parse', `${after}:${path}`) === blob);
    const eventPath = join(dir, 'event.json');
    const invoke = (name, args, env = {}, input = mobilePath + '\0') => spawnSync(process.execPath,
      [script(name), ...args], { cwd: dir, input, encoding: 'utf8', env: {
        ...process.env, ...c.env, GITHUB_SHA: after, GITHUB_EVENT_PATH: eventPath, ...env } });
    const check = (required, args = [], env = {}) => {
      const apple = invoke('apple-ci-impact.mjs', ['--release-risk', ...args], env);
      const android = invoke('classify-ci-impact.mjs', args, env);
      assert.equal(apple.status, 0, apple.stderr); assert.equal(android.status, 0, android.stderr);
      assert.equal(apple.stdout.trim(), String(required)); assert.equal(JSON.parse(android.stdout).android, required);
    };
    writeFileSync(eventPath, JSON.stringify({ ...c.event, before, after }));
    check(!ownersCurrent); check(true, ['--full']);
    check(true, [], { GITHUB_EVENT_NAME: 'pull_request' });
    check(true, [], { GITHUB_EVENT_PATH: join(dir, 'missing') });
    assert.equal(invoke('apple-ci-impact.mjs', []).stdout.trim(), 'true');
    writeFileSync(eventPath, '{broken'); check(true);
    // An earlier owner-only change must disable a later test-only optimization.
    writeFileSync(join(dir, mobileOwners[0][0]), '# changed producer owner\n');
    git('add', '.'); git('commit', '-qm', 'owner evolution');
    const evolvedBase = git('rev-parse', 'HEAD');
    writeFileSync(join(dir, mobilePath), '# another test-only edit\n');
    git('add', '.'); git('commit', '-qm', 'later edit'); after = git('rev-parse', 'HEAD');
    writeFileSync(eventPath, JSON.stringify({ ...c.event, before: evolvedBase, after })); check(true);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
