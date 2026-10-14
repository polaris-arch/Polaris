import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { appleRequired } from './apple-ci-impact.mjs';

test('real Apple and shared producer/application edits select the final App leg', () => {
  for (const path of ['src-tauri/gen/apple/project.yml', 'src-tauri/src/lib.rs',
    'crates/tauri-plugin-polaris-ios/ios/Sources/MeasurementDeviceState.swift',
    'scripts/mobile-core/ci_apple.py', 'scripts/mobile-core/apple_rust.py',
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

test('Apple workflow keeps one admitted host and three complete isolated carrier outputs', () => {
  const workflow = readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
  assert.match(workflow, /runs-on: xcode-27/);
  for (const action of ['prepare', 'build', 'import-local', 'carrier', 'aggregate', 'evidence', 'retire-slot']) {
    assert.match(workflow, new RegExp(`ci_apple.py ${action}`));
  }
  assert.doesNotMatch(workflow, /\n  carrier:\n/);
  assert.equal((workflow.match(/ci_apple.py build/g) || []).length, 1);
  assert.equal((workflow.match(/ci_apple.py carrier --slot/g) || []).length, 3);
  assert.match(workflow, /unsigned-chain:\n\s+needs: component\n\s+if: always\(\)/);
  for (const slot of ['device-arm64', 'sim-arm64', 'sim-amd64']) {
    const id = slot.replaceAll('-', '_');
    const start = workflow.indexOf('id: ' + id + '\n');
    const original = workflow.indexOf('id: original_' + id + '\n', start);
    const summary = workflow.indexOf('/summaries/' + slot + '/', original);
    const retire = workflow.indexOf('retire-slot --slot "' + slot + '"', summary);
    assert.ok(start >= 0 && original > start && summary > original && retire > summary);
    assert.match(workflow.slice(start, original), /!cancelled\(\) && steps.shared_ready.conclusion == 'success'/);
    assert.match(workflow.slice(summary, retire), new RegExp('steps.original_' + id + ".conclusion == 'success'"));
  }
  assert.match(workflow, /if: always\(\)/);
  assert.match(workflow, /contents: read/);
  assert.doesNotMatch(workflow, /contents: write|secrets:|publish_release|codesign --sign/);
  const hook = readFileSync(new URL('../src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj', import.meta.url), 'utf8');
  assert.match(hook, /POLARIS_FORMAL_APP_BUILD/);
  assert.match(hook, /mobile-core\/apple_rust\.py/);
});

test('shared prerequisite failure builds no carriers while first carrier failure leaves later slots selected', () => {
  const workflow = readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
  const shared = workflow.slice(workflow.indexOf('id: shared_ready'), workflow.indexOf('id: device_arm64'));
  const condition = shared.match(/if: (.+)/)[1];
  const prerequisites = [...condition.matchAll(/steps\.(\w+)\.conclusion == 'success'/g)].map(m => m[1]);
  for (const required of ['shared_component', 'component_payload', 'component_manifest',
    'component_original', 'local_component', 'protoc', 'rust_cache']) {
    assert.ok(prerequisites.includes(required));
  }
  const nativeConditions = ['device_arm64', 'sim_arm64', 'sim_amd64'].map(id => {
    const block = workflow.slice(workflow.indexOf('id: ' + id + '\n'));
    return block.match(/if: (.+)/)[1];
  });
  for (const text of nativeConditions) {
    assert.match(text, /!cancelled\(\) && steps.shared_ready.conclusion == 'success'/);
    assert.doesNotMatch(text, /steps\.(device_arm64|sim_arm64|sim_amd64)\./);
  }
  const selectedCount = statuses => {
    const ready = prerequisites.every(id => statuses[id] === 'success');
    return nativeConditions.filter(text => ready && text.includes('steps.shared_ready')).length;
  };
  const success = Object.fromEntries(prerequisites.map(id => [id, 'success']));
  assert.equal(selectedCount(success), 3);
  for (const id of prerequisites) {
    for (const state of ['failure', 'skipped', 'cancelled']) {
      assert.equal(selectedCount({ ...success, [id]: state }), 0);
    }
  }
  assert.equal(selectedCount({ ...success, device_arm64: 'failure' }), 3);
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
  const risk = readFileSync(new URL('../.github/workflows/release-risk.yml', import.meta.url), 'utf8');
  const gate = risk.slice(risk.indexOf('\n  gate:\n'));
  const body = gate.slice(gate.indexOf('        run: |\n') + '        run: |\n'.length)
    .split('\n').map((line) => line.slice(10)).join('\n');
  const base = { CLASSIFY_RESULT: 'success', PC_RUNTIME_POLICY_RESULT: 'success', UNREGISTERED: '',
    NATIVE_VALIDATION_REQUIRED: 'false', NATIVE_VALIDATION_RESULT: 'skipped',
    DESKTOP_CORE_REQUIRED: 'false', DESKTOP_CORE_RESULT: 'skipped',
    PREFLIGHT_REQUIRED: 'false', PREFLIGHT_RESULT: 'skipped',
    PACKAGE_REQUIRED: 'false', PACKAGE_RESULT: 'skipped', ANDROID_REQUIRED: 'false', ANDROID_RESULT: 'skipped' };
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
