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

test('Apple workflow hands one component to three isolated carriers and preserves failed evidence', () => {
  const workflow = readFileSync(new URL('../.github/workflows/apple.yml', import.meta.url), 'utf8');
  assert.match(workflow, /runs-on: xcode-27/);
  for (const action of ['prepare', 'build', 'import', 'carrier', 'aggregate', 'evidence']) {
    assert.match(workflow, new RegExp(`ci_apple.py ${action}`));
  }
  assert.match(workflow, /carrier:\n\s+needs: component/);
  assert.match(workflow, /fail-fast: false/);
  assert.match(workflow, /slot: \[device-arm64, sim-arm64, sim-amd64\]/);
  assert.match(workflow, /unsigned-chain:\n\s+needs: \[component, carrier\]\n\s+if: always\(\)/);
  assert.match(workflow, /if: always\(\)/);
  assert.match(workflow, /contents: read/);
  assert.doesNotMatch(workflow, /contents: write|secrets:|publish_release|codesign --sign/);
  const hook = readFileSync(new URL('../src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj', import.meta.url), 'utf8');
  assert.match(hook, /POLARIS_FORMAL_APP_BUILD/);
  assert.match(hook, /mobile-core\/apple_rust\.py/);
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
