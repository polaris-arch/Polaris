import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { runInNewContext } from 'node:vm';
import { appleRequired } from './apple-ci-impact.mjs';

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
    APPLE_DEFERRED: 'false' };
}

test('real Apple and shared producer/application edits select the final App leg', () => {
  for (const path of ['src-tauri/gen/apple/project.yml', 'src-tauri/src/lib.rs',
    'crates/tauri-plugin-polaris-ios/ios/Sources/MeasurementDeviceState.swift',
    'scripts/mobile-core/ci_apple.py', 'scripts/mobile-core/apple_rust.py',
    'scripts/mobile-core/apple_cache.py', 'scripts/mobile-core/test_apple_cache.py',
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
  assert.match(workflow, /unsigned-chain:\n\s+needs: component\n\s+if: always\(\)/);
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
