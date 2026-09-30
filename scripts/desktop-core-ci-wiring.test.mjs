// Workflow wiring contracts only: no producers, Go tools, sockets or devices run.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { ANDROID_IMPACT_SCOPES, androidRegistrationOf, classifyImpact } from './classify-ci-impact.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const files = Object.fromEntries(['desktop-core', 'release-risk', 'package'].map((name) =>
  [name, readFileSync(join(root, '.github/workflows', `${name}.yml`), 'utf8')]));
function job(source, name) {
  const start = source.indexOf(`\n  ${name}:\n`);
  assert.ok(start >= 0, `missing job ${name}`);
  const body = source.slice(start + name.length + 5);
  const next = /^  [a-z_]+:\s*$/m.exec(body);
  return next ? body.slice(0, next.index) : body;
}
function candidates(source) {
  const producer = source['desktop-core'];
  const risk = source['release-risk'];
  const pkg = source.package;
  assert.equal((producer.match(/ref: \$\{\{ inputs.candidate \}\}/g) ?? []).length, 2, 'both producer and assembly checkout candidate');
  assert.match(producer, /CORE_CANDIDATE: \$\{\{ inputs.candidate \}\}/);
  assert.match(producer, /git rev-parse HEAD\)" = "\$CORE_CANDIDATE"/);
  assert.match(producer, /git status --porcelain --untracked-files=no/);
  assert.match(job(risk, 'classify'), /candidate: \$\{\{ github.event.pull_request.head.sha \|\| github.sha \}\}/);
  assert.match(job(risk, 'desktop_core'), /candidate: \$\{\{ needs.classify.outputs.candidate \}\}/);
  assert.match(job(risk, 'preflight'), /ref: \$\{\{ needs.classify.outputs.candidate \}\}/);
  assert.match(job(pkg, 'setup'), /CORE_CANDIDATE: \$\{\{ inputs.core_candidate \|\| github.event.pull_request.head.sha \|\| github.sha \}\}/);
  assert.match(job(pkg, 'setup'), /\[ "\$CORE_CANDIDATE" = "\$EXPECTED_CANDIDATE" \]/);
  assert.match(job(pkg, 'setup'), /echo "candidate=\$CORE_CANDIDATE"/);
  assert.match(job(pkg, 'package'), /ref: \$\{\{ needs.setup.outputs.candidate \}\}/);
  assert.match(job(pkg, 'package'), /POLARIS_BUILD_ID: \$\{\{ needs.setup.outputs.candidate \}\}/);
  assert.match(job(pkg, 'package'), /CORE_CANDIDATE: \$\{\{ needs.setup.outputs.candidate \}\}/);
}
function bundles(source) {
  const producer = job(source['desktop-core'], 'produce');
  const assemble = job(source['desktop-core'], 'assemble');
  const preflight = job(source['release-risk'], 'preflight');
  const packageJob = job(source.package, 'package');
  assert.match(producer, /name: desktop-core-producer-\$\{\{ inputs.candidate \}\}-\$\{\{ github.run_id \}\}-\$\{\{ github.run_attempt \}\}-\$\{\{ matrix.platform \}\}/);
  assert.match(producer, /path: \$\{\{ runner.temp \}\}\/desktop-core-bundle\n/, 'upload bundle root retains platform directories');
  assert.match(assemble, /needs: produce/);
  const names = [...assemble.matchAll(/name: desktop-core-producer-\$\{\{ inputs.candidate \}\}-\$\{\{ github.run_id \}\}-\$\{\{ github.run_attempt \}\}-([^\n]+)/g)].map((match) => match[1]);
  assert.deepEqual(names, ['linux', 'win', 'mac-x64', 'mac-arm64'], 'exact four downloads from the current candidate/run/attempt');
  assert.equal((assemble.match(/actions\/download-artifact@v8/g) ?? []).length, 4);
  assert.doesNotMatch(assemble, /^\s+(?:pattern|merge-multiple):/m);
  assert.match(assemble, /artifact_name=desktop-core-bundle-\$CORE_CANDIDATE-\$CORE_RUN_ID-\$CORE_RUN_ATTEMPT/);
  assert.match(source['desktop-core'], /CORE_RUN_ID: \$\{\{ github.run_id \}\}/);
  assert.match(source['desktop-core'], /CORE_RUN_ATTEMPT: \$\{\{ github.run_attempt \}\}/);
  assert.match(assemble, /name: \$\{\{ steps.bundle.outputs.artifact_name \}\}/);
  for (const body of [assemble, preflight, packageJob]) {
    assert.match(body, /actions\/download-artifact@v8/);
    assert.doesNotMatch(body, /^\s+(?:run-id|github-token|repository):/m, 'artifact download must remain within this run');
    assert.match(body, /go-version: \$\{\{ steps.(?:go-pin|core-go).outputs.version \}\}/);
    assert.match(body, /node scripts\/fetch-core.mjs --bundle-dir="\$CORE_BUNDLE" --candidate="\$CORE_CANDIDATE"/);
    assert.doesNotMatch(body, /node scripts\/fetch-core.mjs[^\n]*--platform=/, 'even one installer validates all four');
  }
  assert.ok(assemble.indexOf('fetch-core.mjs --assemble') < assemble.indexOf('fetch-core.mjs --bundle-dir='));
  assert.match(preflight, /needs: \[classify, desktop_core\]/);
  assert.match(preflight, /name: \$\{\{ needs.desktop_core.outputs.artifact_name \}\}/);
  const riskPackage = job(source['release-risk'], 'package');
  assert.match(riskPackage, /needs: \[classify, preflight, desktop_core\]/);
  assert.match(riskPackage, /core_candidate: \$\{\{ needs.classify.outputs.candidate \}\}/);
  assert.match(riskPackage, /core_bundle_artifact: \$\{\{ needs.desktop_core.outputs.artifact_name \}\}/);
  assert.match(job(source.package, 'setup'), /\[ "\$CORE_BUNDLE_ARTIFACT" = "desktop-core-bundle-\$CORE_CANDIDATE-\$CORE_RUN_ID-\$CORE_RUN_ATTEMPT" \]/);
  assert.match(job(source.package, 'setup'), /CORE_RUN_ID: \$\{\{ github.run_id \}\}/);
  assert.match(job(source.package, 'setup'), /CORE_RUN_ATTEMPT: \$\{\{ github.run_attempt \}\}/);
  assert.match(packageJob, /CORE_BUNDLE_ARTIFACT: \$\{\{ inputs.core_bundle_artifact \|\| needs.desktop_core.outputs.artifact_name \}\}/);
  assert.match(packageJob, /name: \$\{\{ env.CORE_BUNDLE_ARTIFACT \}\}/);
}
function packageAllows(source, setup, core, provided) {
  const block = /    if: >-\n((?:      .*\n)+)/.exec(job(source, 'package'));
  assert.ok(block, 'package needs explicit reusable-source skip allowlist');
  const expression = block[1].split('\n').map((line) => line.trim()).join(' ');
  // The actual allowlist is a boolean expression shared by JS and Actions.
  // Only immutable status/input dictionaries are supplied; no workflow is run.
  return Boolean(Function('needs', 'inputs', 'always', `return (${expression});`)(
    { setup: { result: setup }, desktop_core: { result: core } },
    { core_bundle_artifact: provided ? 'desktop-core-bundle-exact-candidate' : '' }, () => true));
}
function checkAllowlist(source) {
  assert.match(job(source, 'desktop_core'), /if: inputs.core_bundle_artifact == ''/);
  for (const [setup, core, provided, allowed] of [
    ['success', 'success', false, true], ['success', 'skipped', true, true],
    ['success', 'skipped', false, false], ['success', 'failure', true, false],
    ['success', 'cancelled', true, false], ['failure', 'success', false, false],
    ['cancelled', 'skipped', true, false], ['skipped', 'success', false, false],
  ]) assert.equal(packageAllows(source, setup, core, provided), allowed, `${setup}/${core}/provided=${provided}`);
}

test('four source producers use exact native hosts, source Go pin and real Darwin SDK', () => {
  const workflow = files['desktop-core'];
  const rows = job(workflow, 'produce').split('\n').filter((line) => /^\s+- \{"platform"/.test(line))
    .map((line) => JSON.parse(line.slice(line.indexOf('{'))));
  assert.deepEqual(rows, [
    { platform: 'linux', os: 'ubuntu-22.04', host: 'linux', arch: 'x64' },
    { platform: 'win', os: 'windows-2022', host: 'win32', arch: 'x64' },
    { platform: 'mac-x64', os: 'macos-15-intel', host: 'darwin', arch: 'x64' },
    { platform: 'mac-arm64', os: 'macos-15', host: 'darwin', arch: 'arm64' },
  ]);
  assert.match(workflow, /process.platform !== process.env.CORE_HOST \|\| process.arch !== process.env.CORE_ARCH/);
  assert.match(workflow, /xcrun --sdk macosx --show-sdk-path/);
  assert.match(workflow, /\[ -d "\$sdk" \]/);
  assert.match(workflow, /xcrun --sdk macosx --find clang\+\+/);
  assert.match(workflow, /echo "SDKROOT=\$sdk"/);
  assert.match(workflow, /echo "CC=\$cc"/);
  assert.match(workflow, /require\("\.\/scripts\/libbox-patches\/source-manifest.json"\).goVersion/);
  assert.match(job(workflow, 'produce'), /--producer --platform="\$CORE_PLATFORM"/);
  assert.doesNotMatch(workflow, /continue-on-error:\s*true/);
});

test('candidate checkout and App/helper identity follow PR head instead of a default merge or old source', () => {
  candidates(files);
  assert.throws(() => candidates({ ...files, 'desktop-core': files['desktop-core'].replace('ref: ${{ inputs.candidate }}', 'ref: ${{ github.sha }}') }));
  assert.throws(() => candidates({ ...files, package: files.package.replace('POLARIS_BUILD_ID: ${{ needs.setup.outputs.candidate }}', 'POLARIS_BUILD_ID: ${{ github.sha }}') }));
});

test('exact same-run artifacts preserve platform directories and pass assemble plus all-four consumption', () => {
  bundles(files);
  for (const changed of [
    { ...files, 'desktop-core': files['desktop-core'].replace('path: ${{ runner.temp }}/desktop-core-bundle\n', 'path: ${{ runner.temp }}/desktop-core-bundle/${{ matrix.platform }}\n') },
    { ...files, 'desktop-core': files['desktop-core'].replace('-${{ github.run_attempt }}-win', '-1-win') },
    { ...files, 'desktop-core': files['desktop-core'].replace('-${{ github.run_attempt }}-linux\n', '-${{ github.run_attempt }}-linux\n          run-id: 1\n') },
    { ...files, 'release-risk': files['release-risk'].replace(' --bundle-dir="$CORE_BUNDLE" --candidate="$CORE_CANDIDATE"', '') },
    { ...files, package: files.package.replace('name: ${{ env.CORE_BUNDLE_ARTIFACT }}', 'name: old-source-artifact') },
    { ...files, package: files.package.replace('desktop-core-bundle-$CORE_CANDIDATE-$CORE_RUN_ID-$CORE_RUN_ATTEMPT" ]', 'desktop-core-bundle-$CORE_CANDIDATE" ]') },
  ]) assert.throws(() => bundles(changed));
});

test('Package reuses the provided bundle without a second producer and rejects failed or cancelled source jobs', () => {
  checkAllowlist(files.package);
  assert.throws(() => checkAllowlist(files.package.replace("needs.desktop_core.result == 'success'", "needs.desktop_core.result != 'failure'")));
  assert.throws(() => checkAllowlist(files.package.replace(" && inputs.core_bundle_artifact != ''", '')));
});

test('new desktop source workflow and pure wiring contracts trigger all four desktop consumers', () => {
  for (const path of ['.github/workflows/desktop-core.yml', 'scripts/desktop-core-ci-wiring.test.mjs']) {
    const impact = classifyImpact([path]);
    assert.equal(impact.kernel, true, path);
    assert.equal(impact.preflight, true, path);
    assert.equal(impact.hasPackage, true, path);
    assert.deepEqual(impact.platforms, ['linux', 'windows', 'macos-arm64', 'macos-x64']);
    assert.equal(impact.android, false, path);
    assert.deepEqual(impact.unregisteredScopes, []);
  }
});

test('actual Android-only PC admission and speedtest files retain exact positive impact registration', () => {
  for (const path of ['src-tauri/src/runtime/proxy/debug_pc_echo.rs', 'src-tauri/src/runtime/speedtest/android.rs']) {
    assert.deepEqual(androidRegistrationOf(path), { key: path, table: 'ANDROID_IMPACT_SCOPES' });
    assert.equal(classifyImpact([path]).android, true, path);
    assert.ok(ANDROID_IMPACT_SCOPES[path].why.length > 50);
  }
});
