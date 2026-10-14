import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const command = 'cargo test -p polaris-singbox-grpc --test bundled_core_wire vendored_proto_matches_every_bundled_core -- --exact';
const wireStep = 'Bundled core gRPC wire contract (all platforms)';

/** 顶层 job 的整段：`  <name>:` 起，到下一个两空格缩进的 job 头为止。 */
function job(yaml, name) {
  const lines = yaml.split('\n');
  const start = lines.findIndex((line) => line === `  ${name}:`);
  assert.ok(start >= 0, `missing workflow job: ${name}`);
  const next = lines.findIndex((line, index) => index > start && /^  [A-Za-z_][\w-]*:\s*$/.test(line));
  return lines.slice(start, next < 0 ? lines.length : next).join('\n');
}

/** 整行注释不是判据：`if:` / `needs:` 等字样在同一段的解释性注释里逐字出现。 */
function executable(block) {
  return block.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
}

/** job 级 `if: >-` 折叠块，压成一行。 */
function jobIf(block) {
  const match = /^    if: >-\n((?:      .*\n)+)/m.exec(block);
  assert.ok(match, 'job has no folded `if: >-` block');
  return match[1].split('\n').map((line) => line.trim()).filter(Boolean).join(' ');
}

function step(yaml, name) {
  const lines = yaml.split('\n');
  const start = lines.findIndex((line) => line === `      - name: ${name}`);
  assert.ok(start >= 0, `missing workflow step: ${name}`);
  const next = lines.findIndex((line, index) => index > start && /^      - (?:name:|uses:)/.test(line));
  return { start, body: lines.slice(start, next < 0 ? lines.length : next).join('\n') };
}

function assertWireStep(yaml, { kernelOnly }) {
  const { body } = step(yaml, wireStep);
  assert.match(body, /^        env:\n          POLARIS_REQUIRE_KERNEL_GATE: '1'$/m);
  if (kernelOnly) {
    assert.match(body, /run: node scripts\/desktop-core\/wire-input-handoff\.mjs wire --bundle-dir=.* --candidate=/);
    const source = readFileSync(join(root, 'scripts/desktop-core/wire-input-handoff.mjs'), 'utf8');
    assert.match(source, /const WIRE_ARGS = \['test', '-p', 'polaris-singbox-grpc', '--test', 'bundled_core_wire',\n  'vendored_proto_matches_every_bundled_core', '--', '--exact'\]/);
  } else assert.ok(body.includes(`        run: ${command}\n`), 'standalone must run only the exact descriptor test');
  assert.equal(body.includes('POLARIS_NO_KERNEL_RUN'), false, 'the static wire gate does not inherit the local no-kernel-run mode');
  if (kernelOnly) assert.match(body, /^        if: needs\.classify\.outputs\.kernel == 'true'$/m);
  else assert.match(body, /^        if: steps\.wire-import\.outputs\.wire_verified != 'true'$/m, 'only strict input/result import may reuse the check');
}

function assertAllFourSourceConsumption(fetch) {
  assert.match(fetch.body, /^        run: node scripts\/fetch-core\.mjs --bundle-dir="\$CORE_BUNDLE" --candidate="\$CORE_CANDIDATE"$/m);
  assert.match(fetch.body, /^          CORE_BUNDLE: \$\{\{ runner.temp \}\}\/desktop-core-bundle$/m);
  assert.doesNotMatch(fetch.body, /--platform=|--force|continue-on-error|\|\| true/);
}

// 2026-10-07：wire 步从「每条打包腿各跑一次」收成 package.yml 的 `core_wire` job 跑一次
// （判据与宿主平台无关，依据见该 job 头注）。本条随之换形，逐项对应旧判据：
//   · 旧「步上不得有 if」（直接 dispatch 不得跳过）→ 步上仍不得有 if，且 job 级 `if` 必须与
//     package job 的 bundle 可用性 allowlist **逐字相同**（凡打包腿会跑的入口，本 job 都跑）；
//   · 旧「排在可选真核门之前」（= 不受 run_kernel_gates 控制）→ 整个 job 不得引用
//     POLARIS_RUN_KERNEL_GATES / run_kernel_gates；
//   · 旧「protoc < 全量消费 < wire」→ 同序，改在本 job 内判（全文首个同名步如今在打包腿里）；
//   · 新增：全文恰好一处在跑这条命令（「只跑一次」本身）、发布草稿腿必须等它且只收 success
//     （wire 不成立时不再是「没有安装包」，所以要在写 release 之前显式拦）。
test('Package runs the host-independent wire gate exactly once, on every entry that builds installers', () => {
  const yaml = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
  const wireJobRaw = job(yaml, 'core_wire');
  const wireJob = executable(wireJobRaw);
  const packageJob = executable(job(yaml, 'package'));

  // 取材面自检：两个切片都切到了自己的 job，且没有把对方卷进来。
  assert.match(wireJob, /^    runs-on: ubuntu-22\.04$/m);
  assert.match(packageJob, /uses: tauri-apps\/tauri-action@v1/);
  assert.doesNotMatch(wireJob, /tauri-apps\/tauri-action/);

  assertWireStep(wireJobRaw, { kernelOnly: false });
  assert.equal(
    executable(yaml).split(`run: ${command}\n`).length - 1, 1,
    'the wire command must run in exactly one place in package.yml',
  );
  assert.equal(packageJob.includes(wireStep), false, 'installer legs must not each repeat the host-independent wire gate');

  const protoc = step(wireJobRaw, 'Install protoc (pinned URL + sha256)');
  const fetch = step(wireJobRaw, 'Consume sing-box core bundle');
  const wire = step(wireJobRaw, wireStep);
  assert.match(protoc.body, /^        run: node scripts\/fetch-protoc\.mjs$/m);
  assertAllFourSourceConsumption(fetch);
  assert.ok(protoc.start < fetch.start && fetch.start < wire.start);

  assert.doesNotMatch(wireJob, /POLARIS_RUN_KERNEL_GATES|run_kernel_gates/, 'the wire gate is not an optional runtime gate');
  assert.doesNotMatch(wireJob, /continue-on-error/);
  assert.match(wireJob, /^    needs: \[setup, desktop_core\]$/m);
  assert.equal(jobIf(wireJob), jobIf(packageJob), 'core_wire must run on exactly the entries where installer legs run');
  assert.match(wireJob, /^      CORE_BUNDLE_ARTIFACT: \$\{\{ inputs\.core_bundle_artifact \|\| needs\.desktop_core\.outputs\.artifact_name \}\}$/m);
  assert.match(wireJob, /^          ref: \$\{\{ needs\.setup\.outputs\.candidate \}\}$/m);

  const release = executable(job(yaml, 'release_desktop'));
  assert.match(release, /^    needs: \[[^\]]*\bcore_wire\b[^\]]*\]$/m, 'the first release mutation must wait for the wire gate');
  assert.ok(release.includes("needs.core_wire.result == 'success'"), 'release must require the wire gate to succeed');
  assert.equal(release.includes("needs.core_wire.result == 'skipped'"), false, 'a skipped wire gate must not be publishable');
  assert.equal(yaml.includes("needs.core_wire.result != 'failure'"), false, 'cancelled also satisfies != failure');
});

test('切片自检：job() 不越界，executable() 剥掉注释里的同名判据', () => {
  const synthetic = [
    'jobs:',
    '  first:',
    '    # needs: [setup, core_wire]',
    '    needs: [setup]',
    '  second:',
    '    runs-on: ubuntu-22.04',
  ].join('\n');
  const first = executable(job(synthetic, 'first'));
  assert.equal(first.includes('core_wire'), false);
  assert.equal(first.includes('runs-on'), false);
  assert.match(first, /^    needs: \[setup\]$/m);
});

test('Release Risk checks all fetched cores only on kernel-impact changes', () => {
  const yaml = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');
  assertWireStep(yaml, { kernelOnly: true });
  const fetch = step(yaml, 'Fetch sing-box core');
  const rust = step(yaml, 'Install Rust stable for bundled-core gates');
  const protoc = step(yaml, 'Fetch pinned protoc for gRPC wire check');
  const wire = step(yaml, wireStep);
  assertAllFourSourceConsumption(fetch);
  assert.match(protoc.body, /^        if: needs\.classify\.outputs\.kernel == 'true'$/m);
  assert.match(protoc.body, /^        run: node scripts\/fetch-protoc\.mjs$/m);
  assert.ok(fetch.start < rust.start && rust.start < protoc.start && protoc.start < wire.start);
});

test('ordinary preflight passes fresh same-attempt bytes while Package keeps its full wire job and independent default', () => {
  const risk = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');
  const pkg = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
  const preflight = job(risk, 'preflight'), packageCaller = executable(job(risk, 'package'));
  const nativeCaller = executable(job(risk, 'native_payload')), wire = job(pkg, 'core_wire');
  const prepare = step(preflight, 'Prepare current-attempt wire inputs');
  const runtime = step(preflight, 'Run mandatory bundled-core gates');
  assert.ok(runtime.start < prepare.start, 'handoff must follow complete original preflight checks');
  assert.match(prepare.body, /wire-input-handoff\.mjs create --bundle-dir="\$WIRE_INPUTS" --candidate="\$CORE_CANDIDATE"/);
  assert.match(preflight, /core_wire_input_artifact: \$\{\{ steps\.wire-inputs\.outputs\.artifact_name \}\}/);
  assert.match(packageCaller, /core_wire_input_artifact: \$\{\{ needs\.preflight\.outputs\.core_wire_input_artifact \}\}/);
  assert.doesNotMatch(nativeCaller, /core_wire_input_artifact/);
  const fetch = step(wire, 'Consume sing-box core bundle'), imported = step(wire, 'Verify and import current-attempt preflight wire inputs');
  const checked = step(wire, 'Require current-attempt wire artifact identity'), downloaded = step(wire, 'Download current-attempt preflight wire inputs');
  assert.ok(checked.start < downloaded.start && downloaded.start < imported.start);
  assert.match(checked.body, /wire-input-handoff\.mjs check .*--candidate="\$CORE_CANDIDATE" --artifact-name="\$CORE_WIRE_INPUT_ARTIFACT"/);
  assert.match(fetch.body, /if: env\.CORE_WIRE_INPUT_ARTIFACT == ''/);
  assert.match(imported.body, /if: env\.CORE_WIRE_INPUT_ARTIFACT != ''/);
  assert.match(imported.body, /wire-input-handoff\.mjs import .*--candidate="\$CORE_CANDIDATE" --artifact-name="\$CORE_WIRE_INPUT_ARTIFACT"/);
  assert.ok(imported.start < step(wire, wireStep).start); assertWireStep(wire, { kernelOnly: false });
  assert.equal(jobIf(executable(wire)), jobIf(executable(job(pkg, 'package'))));
  assert.match(pkg, /core_wire_input_artifact:\n        type: string\n        required: false\n        default: ''/);
  assert.doesNotMatch(executable(wire), /continue-on-error|\|\| true/);
});

test('all ordinary installers reuse checked current-attempt transport IDs and bytes, with independent default', () => {
  const pkg = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
  const installer = executable(job(pkg, 'package'));
  const selection = installer.match(/^      CORE_WIRE_INPUT_ARTIFACT: \$\{\{ (.+) \}\}$/m);
  assert.ok(selection);
  const selected = (label, artifact) => runInNewContext(selection[1], {
    matrix: { label }, inputs: { core_wire_input_artifact: artifact },
  });
  for (const label of ['linux', 'windows', 'macos-arm64', 'macos-x64']) {
    assert.equal(selected(label, 'current-attempt-artifact'), 'current-attempt-artifact');
    assert.equal(selected(label, ''), '');
  }
  assert.equal(selected('windows', 'current-attempt-artifact'), 'current-attempt-artifact');
  assert.match(step(installer, 'Download current-attempt preflight installer inputs').body, /wire-input-handoff\.mjs download/);
  assert.match(installer, /CORE_WIRE_INPUT_ARTIFACT_DIGEST: \$\{\{ inputs\.core_wire_input_artifact_digest \}\}/);
  const checked = step(installer, 'Require current-attempt installer input identity');
  const downloaded = step(installer, 'Download current-attempt preflight installer inputs');
  const imported = step(installer, 'Verify and import current-attempt preflight installer inputs');
  assert.ok(checked.start < downloaded.start && downloaded.start < imported.start);
  assert.match(checked.body, /wire-input-handoff\.mjs check .*--candidate="\$CORE_CANDIDATE" --artifact-name="\$CORE_WIRE_INPUT_ARTIFACT"/);
  assert.match(imported.body, /wire-input-handoff\.mjs import .*--candidate="\$CORE_CANDIDATE" --artifact-name="\$CORE_WIRE_INPUT_ARTIFACT"/);
  assert.match(step(installer, 'Download exact candidate core bundle').body, /if: env\.CORE_WIRE_INPUT_ARTIFACT == ''/);
  assert.match(step(installer, 'Consume sing-box core bundle').body, /if: env\.CORE_WIRE_INPUT_ARTIFACT == ''/);
  for (const value of [checked, downloaded, imported]) {
    assert.match(value.body, /if: env\.CORE_WIRE_INPUT_ARTIFACT != ''/);
    assert.doesNotMatch(value.body, /run-id:|repository:|github-token:|continue-on-error|\|\| true/);
  }
  // Installer-specific/final checks remain; the host-independent wire result
  // is only reused after full current-attempt source/tool/input verification.
  assertWireStep(job(pkg, 'core_wire'), { kernelOnly: false });
});

test('every production consumption job seeds an absolute deadline before tools and has a finite outer timeout', () => {
  for (const [file, name, timeout] of [['desktop-core.yml', 'produce', 90], ['release-risk.yml', 'preflight', 120],
    ['package.yml', 'core_wire', 120], ['package.yml', 'package', 180], ['ci.yml', 'test', 120]]) {
    const block = job(readFileSync(join(root, '.github/workflows', file), 'utf8'), name);
    assert.match(block, new RegExp(`^    timeout-minutes: ${timeout}$`, 'm'));
    const setter = step(block, 'Set absolute release-consumer job deadline');
    assert.ok(setter.start < block.split('\n').findIndex((line) => /uses: actions\/checkout@/.test(line)));
    assert.ok(setter.body.includes(`Date.now() + ${timeout - 1} * 60000`));
    assert.match(setter.body, /POLARIS_RELEASE_JOB_DEADLINE_MS=/);
    assert(timeout > 61 && timeout <= 180, 'bounded primary wait must fit within a finite job limit');
  }
});
