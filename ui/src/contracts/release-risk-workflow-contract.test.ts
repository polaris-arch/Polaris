import { chmodSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import { describe, expect, it } from 'vitest';

const REPO_ROOT = fileURLToPath(new URL('../../..', import.meta.url));
const read = (name: string) => readFileSync(join(REPO_ROOT, '.github/workflows', name), 'utf8');

/**
 * 截出一个顶层 job 的段落：`  <name>:` 那行起，到下一个同级 job 头（两空格缩进）为止。
 *
 * 扫描型判据必须先收射程：`/\n  package:[\s\S]*?…/` 从 `package:` 起可以一路延伸到文件尾，
 * 于是「package job 不带 strategy 矩阵」这条否定断言实际问的是「整份 YAML 此后再没有 strategy」——
 * 取材面宽于意图面，同形字面量落在后面任何 job 里都会误红。
 */
const jobSection = (src: string, name: string, file: string) => {
  const lines = src.split('\n');
  const start = lines.findIndex((l) => l === `  ${name}:`);
  expect(start, `${file} 里找不到 job \`${name}\` —— 取材面塌了，本门此刻没有判据`).toBeGreaterThanOrEqual(0);
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i += 1) {
    if (/^ {2}[A-Za-z_][\w-]*:/.test(lines[i])) {
      end = i;
      break;
    }
  }
  // 切片自检：段边界确实找到了（不是一路吃到文件尾 —— 那就等于没收射程）。
  expect(end, `${file} 的 job \`${name}\` 之后没有同级 job 头，切片延伸到了文件尾`).toBeLessThan(
    lines.length
  );
  return lines.slice(start, end).join('\n');
};

const classifyScript = (risk: string) => {
  const lines = jobSection(risk, 'classify', 'release-risk.yml').split('\n');
  const classifyStep = lines.indexOf('      - name: Classify changed paths');
  expect(classifyStep, 'classify 缺路径分类步骤').toBeGreaterThanOrEqual(0);
  const start = lines.indexOf('        run: |', classifyStep);
  expect(start, 'classify 缺 shell 脚本').toBeGreaterThanOrEqual(0);
  return lines.slice(start + 1).map((line) => line.slice(10)).join('\n');
};

function hasStrictResultGate(body: string, variable: string): boolean {
  const result = `"\\$${variable}"`;
  const fail = '(?:echo (?:"[^"\\n]*"|\'[^\'\\n]*\');?\\s*)?exit 1;?\\s*';
  const negative = new RegExp(`^\\s*if (?:\\[ "\\$[A-Z_]+" = true \\] && )?\\[ ${result} != success \\];?\\s*then\\s*${fail}fi(?:\\s*$)`, 'm');
  const atom = '\\[ "\\$[A-Z_]+" = success \\]';
  const positive = new RegExp(`^\\s*(${atom}(?: && ${atom})*) \\|\\| \\{\\s*${fail}\\}(?:\\s*$)`, 'gm');
  return negative.test(body) || [...body.matchAll(positive)].some((match) =>
    new RegExp(`\\[ ${result} = success \\]`).test(match[1]));
}

// Only the direct Cargo test grammar used here, not a general shell/YAML parser.
// Anchor complete commands so comments, echo, test filters and failure masking do
// not count as executed gates. Fold shell continuations before collecting targets.
const configGateTargets = (src: string) => {
  const joined = src.replace(/\\\r?\n[ \t]*/g, ' ');
  const commands = [...joined.matchAll(/^[ \t]*(?:run:[ \t]*)?cargo[ \t]+test[ \t]+-p[ \t]+polaris-config-engine((?:[ \t]+--test[ \t]+[a-z_]+)+)(?:[ \t]+--[ \t]+--nocapture(?:[ \t]+--test-threads=1)?)?[ \t]*$/gm)];
  return commands.flatMap((command) =>
    [...command[1].matchAll(/--test[ \t]+([a-z_]+)/g)].map((target) => target[1])).sort();
};

describe('合入前发布风险门', () => {
  const risk = read('release-risk.yml');
  const pkg = read('package.yml');

  it('PR、merge queue、main push 与手动分支验证均触发，且 workflow 本身不做路径过滤', () => {
    expect(risk).toContain('pull_request:');
    expect(risk).toContain('merge_group:');
    expect(risk).toContain('push:');
    expect(risk).toContain('workflow_dispatch:');
    expect(risk).not.toMatch(/^\s+paths(?:-ignore)?:/m);
  });

  it('手动验证只接受 branch ref，取其 head 与最新 origin/main 的 merge-base；取证失败走全量门', () => {
    const classify = jobSection(risk, 'classify', 'release-risk.yml');
    const start = classify.indexOf('            workflow_dispatch)');
    const end = classify.indexOf('              fi ;;', start);
    expect(start, 'classify 缺手动事件分支').toBeGreaterThanOrEqual(0);
    expect(end, '手动事件分支缺结束边界').toBeGreaterThan(start);
    const dispatch = classify.slice(start, end);

    expect(classify).toContain('DISPATCH_REF: ${{ github.ref }}');
    expect(classify).toContain('DISPATCH_SHA: ${{ github.sha }}');
    expect(dispatch).toContain('[[ "$DISPATCH_REF" == refs/heads/* ]]');
    expect(dispatch).toContain('echo "::error::手动发布风险门只接受 branch ref：$DISPATCH_REF"; exit 1; }');
    expect(dispatch).toContain('git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main');
    expect(dispatch).toContain('dispatch_head=$(git rev-parse --verify "${DISPATCH_REF}^{commit}" 2>/dev/null)');
    expect(dispatch).toContain('[ "$dispatch_head" = "$DISPATCH_SHA" ]');
    expect(dispatch).toContain('base=$(git merge-base refs/remotes/origin/main "$dispatch_head")');
    expect(dispatch).toContain('head="$dispatch_head"');
    expect(classify).toContain('if [ -n "$base" ] && [ -n "$head" ] \\');
    expect(classify).toContain('git diff --name-only -z "$base" "$head" | node scripts/classify-ci-impact.mjs > "$result"');
    expect(classify).toMatch(/else\n\s+echo "无法取得可靠 diff，故障关闭为内核门 \+ 四平台。"\n\s+node scripts\/classify-ci-impact\.mjs --full > "\$result"/);
    expect(classify).toContain('node scripts/classify-ci-impact.mjs --full > "$result"');
  });

  it('手动分类脚本回放分支、tag 拒绝与取证失败', () => {
    const temp = mkdtempSync(join(tmpdir(), 'polaris-release-risk-contract-'));
    try {
      const git = join(temp, 'git');
      writeFileSync(git, `#!/bin/sh
case "$1" in
  fetch) [ "$FAIL_FETCH" != 1 ] ;;
  rev-parse) printf 'expected-head\\n' ;;
  merge-base) printf 'expected-base\\n' ;;
  cat-file) exit 0 ;;
  diff) printf 'crates/config-engine/src/builder/managed_mesh_emission.rs\\0' ;;
  *) exit 9 ;;
esac
`);
      chmodSync(git, 0o755);
      const script = classifyScript(risk);
      const run = (ref: string, sha: string, failFetch = false) => {
        const output = join(temp, 'output');
        writeFileSync(output, '');
        const result = spawnSync('bash', ['-c', script], {
          cwd: REPO_ROOT,
          env: {
            ...process.env,
            PATH: `${temp}:${process.env.PATH}`,
            EVENT_NAME: 'workflow_dispatch',
            DISPATCH_REF: ref,
            DISPATCH_SHA: sha,
            FAIL_FETCH: failFetch ? '1' : '0',
            RUNNER_TEMP: temp,
            GITHUB_OUTPUT: output,
            POLARIS_NATIVE_PAYLOAD_VALIDATION: '0',
          },
          encoding: 'utf8',
        });
        return { result, outputs: readFileSync(output, 'utf8') };
      };

      const branch = run('refs/heads/ci/d1-check', 'expected-head');
      expect(branch.result.status, branch.result.stderr).toBe(0);
      expect(branch.outputs).toContain('kernel=true');
      expect(branch.outputs).toContain('preflight=true');
      expect(branch.outputs).toContain('unregistered=');
      expect(branch.result.stdout).not.toContain('无法取得可靠 diff');

      const movedHead = run('refs/heads/ci/d1-check', 'stale-head');
      expect(movedHead.result.status, movedHead.result.stderr).toBe(0);
      expect(movedHead.result.stdout).toContain('无法取得可靠 diff');
      expect(movedHead.outputs).toContain('android=true');

      const tag = run('refs/tags/v1.0.0', 'expected-head');
      expect(tag.result.status).toBe(1);
      expect(tag.result.stdout).toContain('手动发布风险门只接受 branch ref');
      expect(tag.result.stdout).not.toContain('无法取得可靠 diff');

      const failure = run('refs/heads/ci/d1-check', 'expected-head', true);
      expect(failure.result.status, failure.result.stderr).toBe(0);
      expect(failure.result.stdout).toContain('无法取得可靠 diff');
      expect(failure.outputs).toContain('kernel=true');
      expect(failure.outputs).toContain('platforms=["linux","windows","macos-arm64","macos-x64"]');
      expect(failure.outputs).toContain('android=true');
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  });

  it('路径判据由仓库脚本持有，最终 required check 始终运行', () => {
    expect(risk).toContain('node scripts/classify-ci-impact.mjs');
    expect(risk).toContain('name: release risk gate');
    expect(risk).toMatch(/\n  gate:[\s\S]*?\n    if: always\(\)/);
  });

  it('native 分类仅接受明确的同候选 Risk 上下文，不借用 ordinary preflight 或 Android 结论', () => {
    const temp = mkdtempSync(join(tmpdir(), 'polaris-native-risk-contract-'));
    try {
      // This is the historical Risk admission contract, before strict formal
      // defaults. Keep its exact release bytes isolated from today's selection.
      for (const directory of ['desktop-core', 'lib'])
        cpSync(join(REPO_ROOT, 'scripts', directory), join(temp, 'scripts', directory), { recursive: true });
      mkdirSync(join(temp, 'src-tauri'));
      const manifest = JSON.parse(readFileSync(join(REPO_ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
      writeFileSync(join(temp, 'src-tauri/core-manifest.json'), JSON.stringify({ desktopRelease: manifest.historicalDesktopRelease }));
      cpSync(join(REPO_ROOT, manifest.historicalDesktopReleaseContractPath),
        join(temp, manifest.historicalDesktopRelease.contractPath));
      const output = join(temp, 'output');
      const candidate = 'a'.repeat(40);
      const ref = 'refs/heads/collab/fk02-native-payload-validation-20261010';
      const base = {
        POLARIS_NATIVE_PAYLOAD_VALIDATION: '1', POLARIS_NO_KERNEL_RUN: '0', DISPATCH_SHA: candidate,
        GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REPOSITORY: 'polaris-arch/Polaris',
        GITHUB_REF: ref, GITHUB_SHA: candidate, GITHUB_RUN_ID: '12', GITHUB_RUN_ATTEMPT: '3',
        GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/release-risk.yml@${ref}`,
        RUNNER_TEMP: temp, GITHUB_OUTPUT: output,
      };
      const run = (changed: Record<string, string>, cwd = temp) => {
        writeFileSync(output, '');
        const result = spawnSync('bash', ['-c', classifyScript(risk)], {
          cwd, env: { ...process.env, ...base, ...changed }, encoding: 'utf8',
        });
        return { result, output: readFileSync(output, 'utf8') };
      };
      const formal = run({}, REPO_ROOT);
      expect(formal.result.status, formal.result.stderr).not.toBe(0);
      expect(formal.result.stderr).toContain('Exact committed formal App/caller/run context required');
      expect(formal.output).toBe('');
      const good = run({});
      expect(good.result.status, good.result.stderr).toBe(0);
      expect(good.output).toBe('kernel=false\nplatforms=[]\npreflight=false\nhas_package=false\nandroid=false\napple=false\nunregistered=\n');
      const invalidContexts: Record<string, string>[] = [
        { GITHUB_EVENT_NAME: 'push' }, { GITHUB_REF: 'refs/tags/v1.0.0' },
        { GITHUB_SHA: 'b'.repeat(40) }, { GITHUB_REPOSITORY: 'other/repo' },
        { GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/package.yml@${ref}` },
        { GITHUB_RUN_ID: '0' }, { GITHUB_RUN_ATTEMPT: '0' }, { POLARIS_NO_KERNEL_RUN: '1' },
      ];
      for (const changed of invalidContexts) {
        const bad = run(changed);
        expect(bad.result.status, bad.result.stderr).not.toBe(0);
        expect(bad.output).toBe('');
      }
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  });

  it('gate 对「未登记影响面」有牙：分类器自曝的 scope 必须当场红并点名', () => {
    // 与 ci-impact-coverage-contract 的完备性门**独立**：那条门在 ui.yml 里跑，ui.yml 挂了/被 skip
    // 就没牙；这条在 release-risk 自己的 required check 里，且 gate 是 `if: always()`。
    // 缺陷类回放：未登记 scope 此前只被 fail-closed 成「内核门 + 四平台」——多花钱但 CI 全绿，
    // 没有任何东西会红，登记表就永远补不上。
    expect(risk).toContain("unregistered=$(jq -r '.unregisteredScopes | join(\" \")' \"$result\")");
    expect(risk).toMatch(/\n\s+unregistered: \$\{\{ steps\.impact\.outputs\.unregistered \}\}/);
    expect(risk).toMatch(/\n\s+UNREGISTERED: \$\{\{ needs\.classify\.outputs\.unregistered \}\}/);
    expect(risk).toMatch(/\[ -z "\$UNREGISTERED" \] \|\| \{\n\s+echo "::error::[^"]*\$UNREGISTERED"; exit 1; \}/);
  });

  it('自动安装包验证复用 Package，但不上传产物也不重复内核门', () => {
    const pkgJob = jobSection(risk, 'package', 'release-risk.yml');
    // 切片自检：拿到的确实是这个 job，且没有把下一个 job 卷进来。
    expect(pkgJob).toContain('uses: ./.github/workflows/package.yml');
    expect(pkgJob).toContain('needs: [classify, preflight, desktop_core]');
    expect(pkgJob).toContain("needs.preflight.result == 'success'");
    expect(pkgJob).toContain('core_candidate: ${{ needs.classify.outputs.candidate }}');
    expect(pkgJob).toContain('core_bundle_artifact: ${{ needs.desktop_core.outputs.artifact_name }}');
    const coreJob = jobSection(risk, 'desktop_core', 'release-risk.yml');
    expect(coreJob).toContain("if: needs.classify.outputs.preflight == 'true'");
    expect(coreJob).toContain('candidate: ${{ needs.classify.outputs.candidate }}');
    const preflight = jobSection(risk, 'preflight', 'release-risk.yml');
    expect(preflight).toContain('needs: [classify, desktop_core]');
    expect(preflight).toContain('ref: ${{ needs.classify.outputs.candidate }}');
    expect(preflight).toContain('name: ${{ needs.desktop_core.outputs.artifact_name }}');
    expect(pkgJob).not.toContain('name: release risk gate');
    expect(pkgJob).toMatch(/permissions:\n\s+contents: write/);
    expect(risk).toContain('run_kernel_gates: false');
    expect(risk).toContain('upload_artifacts: false');
    expect(risk).toContain('platforms: ${{ needs.classify.outputs.platforms }}');
    expect(risk).toContain('skip_quality_gates: true');
    expect(pkgJob).not.toMatch(/\n {4}strategy:/);
    expect(pkg).toContain('workflow_call:');
    expect(pkg).toContain('PLATFORMS_JSON: ${{ inputs.platforms');
    expect(pkg).toContain("if: env.POLARIS_UPLOAD_ARTIFACTS == '1'");
  });

  it('并发组：同一 PR / 分支上新提交取消旧运行，merge queue 不取消，且被取消的腿里没有写 release 的任务', () => {
    // 自动 tip 可替代；手动、固定审计与 merge queue 独立、tag 不取消。求实际表达式语义。
    const match = risk.match(
      /^concurrency:\n  group: (.+)\n  cancel-in-progress: \$\{\{ (.+) \}\}\n/m,
    );
    expect(match, 'release-risk.yml 缺顶层 concurrency 段').not.toBeNull();
    const context = (eventName: string, ref: string, preserve = false, runId = 1) => ({
      github: { workflow: 'Release Risk', event_name: eventName, ref, run_id: runId,
        event: { pull_request: { number: eventName === 'pull_request' ? 7 : undefined },
          merge_group: { head_sha: eventName === 'merge_group' ? 'queued-head' : undefined },
          inputs: { preserve_run: preserve ? 'true' : 'false' } } },
      inputs: { preserve_run: preserve },
      startsWith: (value: string, prefix: string) => value.startsWith(prefix),
      format: (value: string, argument: number) => value.replace('{0}', String(argument)),
    });
    const cancels = (eventName: string, ref: string, preserve = false) =>
      runInNewContext(match![2], context(eventName, ref, preserve));
    const group = (eventName: string, ref: string, preserve = false, runId = 1) =>
      match![1].replace(/\$\{\{ (.+?) \}\}/g, (_text, expression: string) =>
        String(runInNewContext(expression, context(eventName, ref, preserve, runId))));
    expect(cancels('pull_request', 'refs/pull/7/merge')).toBe(true);
    expect(cancels('push', 'refs/heads/main')).toBe(true);
    expect(cancels('workflow_dispatch', 'refs/heads/ci/shorten-pipelines')).toBe(false);
    // 队列里的每一组都必须跑完才能出结论。
    expect(cancels('merge_group', 'refs/heads/gh-readonly-queue/main/pr-7')).toBe(false);
    expect(cancels('workflow_dispatch', 'refs/heads/collab/audit', true)).toBe(false);
    expect(cancels('push', 'refs/tags/v1.0.0')).toBe(false);
    expect(group('pull_request', 'refs/pull/7/merge')).toBe('release-risk-Release Risk-7');
    expect(group('push', 'refs/heads/main')).toBe(group('push', 'refs/heads/main', false, 2));
    expect(group('workflow_dispatch', 'refs/heads/collab/audit', true)).not.toBe(
      group('workflow_dispatch', 'refs/heads/collab/audit', true, 2));
    expect(group('workflow_dispatch', 'refs/heads/collab/audit')).not.toBe(
      group('workflow_dispatch', 'refs/heads/collab/audit', false, 2));
    expect(group('workflow_dispatch', 'refs/heads/collab/audit', true)).toBe(
      group('workflow_dispatch', 'refs/heads/collab/audit'));
    expect(group('merge_group', 'refs/heads/queue')).not.toBe(group('merge_group', 'refs/heads/queue', false, 2));
    expect(group('push', 'refs/tags/v1.0.0')).not.toBe(group('push', 'refs/tags/v2.0.0'));

    // 取消只有在被取消的 run 里没有副作用任务时才安全。三条各自成立才算数：
    // ① 本 workflow 没有 tag 触发；
    const triggers = risk.slice(risk.indexOf('\non:\n'), risk.indexOf('\npermissions:\n'));
    expect(triggers.length).toBeGreaterThan(40);
    expect(triggers).not.toMatch(/^\s+tags:/m);
    expect(triggers).not.toContain('release:');
    // ② package.yml 里第一次写 release 的 job 只在 v* tag 上可达，后两段经 needs 链继承它；
    const executable = (block: string) => block.split('\n').filter((line) => !/^\s*#/.test(line)).join('\n');
    const desktop = executable(jobSection(pkg, 'release_desktop', 'package.yml'));
    expect(desktop).toContain("&& startsWith(github.ref, 'refs/tags/v')");
    expect(executable(jobSection(pkg, 'android_release', 'package.yml'))).toContain('needs: release_desktop');
    const finalStart = pkg.indexOf('\n  release:\n');
    expect(finalStart).toBeGreaterThan(0);
    const finalJob = executable(pkg.slice(finalStart));
    expect(finalJob).toContain('needs: [release_desktop, android_release]');
    const condition = /^    if: >-\n((?:      .*\n)+)/m.exec(finalJob);
    expect(condition).not.toBeNull();
    const expression = condition![1].split('\n').map(line => line.trim()).filter(Boolean).join(' ');
    expect(expression).toBe("(always() && needs.release_desktop.result == 'success' && needs.android_release.result == 'success' )");
    const success = { release_desktop: { result: 'success' }, android_release: { result: 'success' } };
    for (const job of Object.keys(success)) for (const result of ['success', 'failure', 'cancelled', 'skipped']) {
      const needs = { ...success, [job]: { result } };
      expect(runInNewContext(expression, { needs, always: () => true }), `${job}/${result}`)
        .toBe(result === 'success');
    }
    // 写 release 的命令只出现在这三个 job 里（package / core_wire 等构建腿一条都没有）。
    for (const name of ['setup', 'package', 'core_wire'] as const) {
      expect(executable(jobSection(pkg, name, 'package.yml')), `package.yml 的 ${name} job 出现了 release 写操作`)
        .not.toMatch(/gh release (?:create|upload|edit|delete)/);
    }
    // ③ 本 workflow 调 android.yml 时不传 publish_release（写 release 的 release-apk 因此不可达）。
    const androidJob = executable(jobSection(risk, 'android', 'release-risk.yml'));
    expect(androidJob).toContain('uses: ./.github/workflows/android.yml');
    expect(androidJob).not.toContain('publish_release');
    // Android 腿同样是轻量构型（2026-10-08）：被调方对「轻量 + 发布」当场红，故这两条必须同时成立。
    const withAt = androidJob.indexOf('\n    with:\n');
    expect(withAt).toBeGreaterThan(0);
    const androidInputs = androidJob.slice(withAt);
    expect(androidInputs).toMatch(/^ {6}light_build: true$/m);
    expect([...androidInputs.matchAll(/^ {6}([a-z_]+):/gm)].map((m) => m[1]).sort())
      .toEqual(['light_build', 'preserve_run']);
    const preserved = androidInputs.match(/^ {6}preserve_run: \$\{\{ (.+) \}\}$/m);
    expect(preserved).not.toBeNull();
    for (const inherited of [false, true]) {
      for (const input of [false, true]) {
        expect(runInNewContext(preserved![1], { inputs: { preserve_run: input },
          github: { event: { inputs: { preserve_run: inherited ? 'true' : 'false' } } } }))
          .toBe(input || inherited);
      }
    }
    // 安装包不上传、且是轻量构型（不是可分发物）。
    const pkgJob = executable(jobSection(risk, 'package', 'release-risk.yml'));
    expect(pkgJob).toContain('upload_artifacts: false');
    expect(pkgJob).toContain('light_build: true');
  });

  it('Apple 默认 device-only，typed 手动 opt-in 才增加 simulator 并强制选中 Apple 腿', () => {
    const apple = read('apple.yml');
    const inputs = apple.slice(0, apple.indexOf('\npermissions:'));
    const inputBlocks = [...inputs.matchAll(/^ {6}include_simulator:\n([\s\S]*?)(?=^ {6}\w+:|^ {2}\w+:)/gm)];
    expect(inputBlocks).toHaveLength(2);
    for (const [, block] of inputBlocks) {
      expect(block).toContain('type: boolean');
      expect(block).toContain('default: false');
    }
    expect(apple).toContain("POLARIS_APPLE_SCOPE: ${{ inputs.include_simulator && 'device-and-simulator' || 'device-only' }}");
    expect(apple).toContain("targets: ${{ inputs.include_simulator && 'aarch64-apple-ios,aarch64-apple-ios-sim' || 'aarch64-apple-ios' }}");
    const rootInput = risk.slice(risk.indexOf('      apple_include_simulator:'), risk.indexOf('      defer_apple:'));
    expect(rootInput).toContain('type: boolean');
    expect(rootInput).toContain('default: false');
    const caller = jobSection(risk, 'apple', 'release-risk.yml');
    const passed = caller.match(/^ {6}include_simulator: \$\{\{ (.+) \}\}$/m);
    expect(passed).not.toBeNull();
    for (const event of ['pull_request', 'push', 'merge_group', 'workflow_dispatch']) {
      for (const selected of [false, true]) {
        expect(runInNewContext(passed![1], { github: { event_name: event }, inputs: { apple_include_simulator: selected } }))
          .toBe(event === 'workflow_dispatch' && selected);
      }
    }
    expect(caller).toContain('name: apple unsigned App and PacketTunnel gate');
    expect(caller).toContain("needs.classify.outputs.apple == 'true'");
    const classify = jobSection(risk, 'classify', 'release-risk.yml');
    expect(classify).toContain("POLARIS_APPLE_INCLUDE_SIMULATOR: ${{ github.event_name == 'workflow_dispatch' && inputs.apple_include_simulator && 'true' || 'false' }}");
    expect(classifyScript(risk)).toContain('if [ "${POLARIS_APPLE_INCLUDE_SIMULATOR:-false}" = true ]; then\n  printf \'true\\n\' > "$apple_result"\nfi');
    expect(classify).toContain('[ "$VALIDATION_MODE" = ordinary ] && [ "$APPLE_DEFERRED" = false ]');
    expect(risk).toContain('Apple sim-arm64: not requested.');
    expect(risk).toContain('Full release risk gate is not passed.');
  });

  it('Apple 单 host 串行保全，所有 simulator 操作显式条件化且 cache 不替代真实 carrier', () => {
    const apple = read('apple.yml');
    const component = jobSection(apple, 'component', 'apple.yml');
    expect(component).not.toMatch(/strategy:|matrix:/);
    expect(component.match(/ci_apple.py build/g)).toHaveLength(1);
    const blocks = component.split(/^ {6}- /m).slice(1);
    const simulator = blocks.filter((block) => block.includes('--slot "sim-arm64"') ||
      block.includes('path: target/apple-ci-cache/sim-arm64') ||
      /name: apple-carrier-(?:original|summary)-sim-arm64/.test(block));
    expect(simulator).toHaveLength(10);
    for (const block of simulator) {
      expect(block).toMatch(/if: \$\{\{ inputs.include_simulator && !cancelled\(\)/);
      expect(block).not.toContain('continue-on-error');
    }
    const device = blocks.find((block) => block.includes('id: device_arm64\n'))!;
    expect(device).toContain('ci_apple.py carrier --slot "device-arm64"');
    expect(device).not.toContain('inputs.include_simulator');
    expect(device).not.toContain('cache-hit');
    expect(component).not.toContain('Swatinem/rust-cache');
    for (const [slot, short] of [['device-arm64', 'device'], ['sim-arm64', 'sim']]) {
      const id = slot.replaceAll('-', '_');
      const original = component.indexOf(`id: original_${id}\n`);
      const summary = component.indexOf(`id: summary_${id}\n`);
      const exported = component.indexOf(`id: rust_${short}_export\n`);
      const saved = component.indexOf(`id: rust_${short}_save\n`);
      const returned = component.indexOf(`id: rust_${short}_return\n`);
      const retired = component.indexOf(`id: retire_${id}\n`);
      expect(original).toBeGreaterThan(0);
      expect(summary).toBeGreaterThan(original);
      expect(exported).toBeGreaterThan(summary);
      expect(saved).toBeGreaterThan(exported);
      expect(returned).toBeGreaterThan(saved);
      expect(retired).toBeGreaterThan(returned);
      const exportBlock = blocks.find((block) => block.includes(`id: rust_${short}_export\n`))!;
      expect(exportBlock).toContain(`steps.${id}.conclusion == 'success'`);
      expect(exportBlock).toContain(`steps.original_${id}.conclusion == 'success'`);
      expect(exportBlock).toContain(`steps.summary_${id}.conclusion == 'success'`);
      const saveBlock = blocks.find((block) => block.includes(`id: rust_${short}_save\n`))!;
      expect(saveBlock).toContain('uses: actions/cache/save@v5');
      expect(saveBlock).toContain(`path: target/apple-ci-cache/${slot}`);
      expect(saveBlock).toContain(`key: \${{ steps.rust_${short}_restore.outputs.cache-primary-key }}`);
      const returnBlock = blocks.find((block) => block.includes(`id: rust_${short}_return\n`))!;
      expect(returnBlock).toContain('!cancelled()');
      expect(returnBlock).not.toContain(`steps.rust_${short}_save.conclusion`);
      const retireBlock = blocks.find((block) => block.includes(`id: retire_${id}\n`))!;
      expect(retireBlock).toContain(`steps.rust_${short}_export.conclusion == 'skipped' || steps.rust_${short}_return.conclusion == 'success'`);
    }
    const simRestore = blocks.find((block) => block.includes('id: rust_sim_restore\n'))!;
    expect(simRestore).toContain("steps.retire_device_arm64.conclusion == 'success'");
    expect(component).toContain('${{ env.POLARIS_APPLE_SCOPE }}-aarch64-apple-ios-release-');
    expect(component).toContain('${{ env.POLARIS_APPLE_SCOPE }}-aarch64-apple-ios-sim-release-');
  });

  it('Apple aggregate 先拒绝上游失败，再验证显式 scope 的 summary 完整集', () => {
    const apple = read('apple.yml');
    const chainStart = apple.indexOf('\n  unsigned-chain:\n');
    expect(chainStart).toBeGreaterThan(0);
    const chain = apple.slice(chainStart);
    // The final job must observe both the optional resolver and the full-build
    // component, including the deliberately skipped component on verified reuse.
    expect(chain).toContain('needs: [resolve, component]\n    if: always()');
    expect(chain).toContain('COMPONENT_RESULT: ${{ needs.component.result }}');
    const blocks = chain.split(/^ {6}- /m).slice(1);
    const simDownload = blocks.find((block) => block.includes('name: apple-carrier-summary-sim-arm64'))!;
    expect(simDownload).toContain('if: ${{ inputs.include_simulator }}');
    const aggregate = blocks.find((block) => block.includes('name: Strict declared-scope'))!;
    expect(hasStrictResultGate(aggregate, 'COMPONENT_RESULT')).toBe(true);
    expect(aggregate.indexOf('[ "$COMPONENT_RESULT" = success ]')).toBeLessThan(aggregate.indexOf('ci_apple.py aggregate'));
    expect(aggregate).toContain('aggregate --scope "$POLARIS_APPLE_SCOPE"');
    expect(aggregate).toContain('sim-arm64: not requested.');
    expect(aggregate).not.toContain('continue-on-error');
  });

  it('Apple resolver 未完成就回退实编译，复用与全量聚合互斥且最终判据不能被跳过', () => {
    const apple = read('apple.yml');
    const check = (source: string) => {
      const resolver = jobSection(source, 'resolve', 'apple.yml');
      const component = jobSection(source, 'component', 'apple.yml');
      const chainStart = source.indexOf('\n  unsigned-chain:\n');
      expect(chainStart).toBeGreaterThan(0);
      const chain = source.slice(chainStart);
      expect(chain.match(/^ {2}[\w-]+:/gm)).toEqual(['  unsigned-chain:']);
      expect(chain).toMatch(/^    needs: \[resolve, component\]\n    if: always\(\)$/m);
      expect(component).toMatch(/^    needs: resolve$/m);
      const blocks = chain.split(/^ {6}- /m).slice(1);
      const step = (needle: string) => {
        const matches = blocks.filter((block) => block.includes(needle));
        expect(matches, `unsigned-chain step missing or ambiguous: ${needle}`).toHaveLength(1);
        return matches[0];
      };
      const expression = (block: string, indent: number, key = 'if') => {
        const match = block.match(new RegExp(`^ {${indent}}${key}: \\$\\{\\{ (.+) \\}\\}$`, 'm'));
        expect(match, `missing ${key} expression`).not.toBeNull();
        return match![1];
      };
      const componentIf = expression(component, 4);
      const reusedOutput = expression(resolver, 6, 'reused');
      const reuseSteps = [
        step('name: Download the exact resolver receipt for this attempt\n'),
        step('id: reuse_verified\n'),
      ];
      const buildSteps = [
        step('name: apple-component-manifest-'),
        step('name: apple-carrier-summary-device-arm64-'),
        step('id: aggregate\n'),
      ];
      const chainUpload = expression(step('id: chain\n'), 8);
      const result = step('id: result\n');
      expect(result).toMatch(/^        if: always\(\)$/m);
      expect(result).not.toContain('continue-on-error');
      // A status function is required here: GitHub's implicit success() would
      // otherwise suppress fallback after a failed resolver or failed aggregate.
      expect(componentIf).toContain('!cancelled()');
      expect(chainUpload).toContain('always()');
      const statuses = ['success', 'failure', 'cancelled', 'skipped', '', 'unknown'];
      for (const status of statuses) {
        for (const reused of ['true', 'false', '', 'unknown']) {
          const hit = status === 'success' && reused === 'true';
          const context = { needs: { resolve: { result: status, outputs: { reused } } }, always: () => true };
          for (const cancelled of [false, true]) {
            expect(runInNewContext(componentIf, { ...context, cancelled: () => cancelled }))
              .toBe(!cancelled && !hit);
          }
          for (const block of reuseSteps) expect(runInNewContext(expression(block, 8), context)).toBe(hit);
          for (const block of buildSteps) expect(runInNewContext(expression(block, 8), context)).toBe(!hit);
          expect(runInNewContext(chainUpload, context)).toBe(!hit);
          // continue-on-error changes conclusion to success, but cannot turn a
          // resolver/receipt failure into the published reused=true output.
          for (const receipt of statuses) {
            expect(runInNewContext(reusedOutput, { steps: {
              resolve: { outcome: status, conclusion: 'success', outputs: { reused } },
              receipt: { outcome: receipt, conclusion: 'success' },
            } })).toBe(hit && receipt === 'success' ? 'true' : 'false');
          }
        }
      }
    };
    check(apple);
    // Replay dependency removal, skipped final gates, masked resolver failures,
    // and overlapping or missing evidence branches against the same predicates.
    for (const [before, after] of [
      ['needs: [resolve, component]', 'needs: component'],
      ['needs: [resolve, component]', 'needs: resolve'],
      ['needs: resolve\n    if:', 'needs: []\n    if:'],
      ['needs: [resolve, component]\n    if: always()', 'needs: [resolve, component]\n    if: success()'],
      ['id: result\n        if: always()', 'id: result\n        if: success()'],
      ["!cancelled() && !(needs.resolve.result == 'success'", "!(needs.resolve.result == 'success'"],
      ['steps.resolve.outcome', 'steps.resolve.conclusion'],
      ['steps.receipt.outcome', 'steps.receipt.conclusion'],
      ["needs.resolve.result == 'success' && needs.resolve.outputs.reused == 'true'", "needs.resolve.outputs.reused == 'true'"],
      ["id: reuse_verified\n        if: ${{ needs.resolve.result == 'success' && needs.resolve.outputs.reused == 'true' }}", 'id: reuse_verified\n        if: ${{ true }}'],
      ["id: aggregate\n        if: ${{ !(needs.resolve.result == 'success' && needs.resolve.outputs.reused == 'true') }}", 'id: aggregate\n        if: ${{ true }}'],
      ["always() && !(needs.resolve.result == 'success'", "!(needs.resolve.result == 'success'"],
    ]) {
      const changed = apple.replace(before, after);
      expect(changed).not.toBe(apple);
      expect(() => check(changed), `mutation escaped: ${before}`).toThrow();
    }
  });

  it('Android 复用调用 job 满足嵌套发布 job 的权限上限', () => {
    const androidJob = jobSection(risk, 'android', 'release-risk.yml');
    expect(androidJob).toContain('uses: ./.github/workflows/android.yml');
    expect(androidJob).toMatch(/permissions:\n\s+contents: write/);
    const android = read('android.yml');
    expect(android).toMatch(/\n  release-apk:[\s\S]*?\n    permissions:\n      contents: write/);
    expect(risk).toMatch(/\npermissions:\n  contents: read/);
  });

  it('Package 的 tag 与直接手动入口保留强制门，Release Risk 复用尊重 false 输入', () => {
    const packageJob = jobSection(pkg, 'package', 'package.yml');
    for (const key of ['POLARIS_RUN_KERNEL_GATES', 'POLARIS_UPLOAD_ARTIFACTS']) {
      const match = packageJob.match(new RegExp(`^      ${key}: \\$\\{\\{ (.+) \\}\\}$`, 'm'));
      expect(match, `package job 缺 ${key} 表达式`).not.toBeNull();
      const evaluate = (ref: string, eventName: string, skipQualityGates: boolean) =>
        runInNewContext(match![1], {
          github: { ref, event_name: eventName },
          inputs: { skip_quality_gates: skipQualityGates, run_kernel_gates: false, upload_artifacts: false },
          startsWith: (value: string, prefix: string) => value.startsWith(prefix),
        });
      expect(evaluate('refs/tags/v1.0.0', 'push', false)).toBe('1');
      expect(evaluate('refs/heads/feature', 'workflow_dispatch', false)).toBe('1');
      expect(evaluate('refs/heads/feature', 'workflow_dispatch', true)).toBe('0');
    }
  });

  it('preflight 用钉扎 protoc 对拍四平台 wire，D1 CONNECT 仍在 REQUIRE 强制腿', () => {
    const manifest = JSON.parse(readFileSync(join(REPO_ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
    expect(manifest.desktopSourceBuild.sourceMode).toBe('fork-commit-v1');
    const sourceBytes = readFileSync(join(REPO_ROOT, 'scripts/desktop-core/source-manifest.json'));
    expect(manifest.desktopSourceBuild.sourceManifestSha256).toBe(createHash('sha256').update(sourceBytes).digest('hex'));
    expect(JSON.parse(sourceBytes.toString()).sourceCommit).toBe('aafc521b745e0d1a8f50a6a3f41169433f1c2194');
    expect(manifest.windowsBuild).toBeUndefined();
    const preflight = jobSection(risk, 'preflight', 'release-risk.yml');
    const fetchCore = preflight.indexOf('      - name: Fetch sing-box core');
    const fetchProtoc = preflight.indexOf('      - name: Fetch pinned protoc for gRPC wire check');
    const wire = preflight.indexOf('      - name: Bundled core gRPC wire contract (all platforms)');
    const mandatory = preflight.indexOf('      - name: Run mandatory bundled-core gates');
    expect(fetchCore).toBeGreaterThanOrEqual(0);
    expect(fetchProtoc).toBeGreaterThan(fetchCore);
    expect(wire).toBeGreaterThan(fetchProtoc);
    expect(mandatory).toBeGreaterThan(wire);
    expect(preflight).toContain('node scripts/fetch-protoc.mjs');
    expect(preflight).toContain('node scripts/desktop-core/wire-input-handoff.mjs wire');
    const wireSource = readFileSync(join(REPO_ROOT, 'scripts/desktop-core/wire-input-handoff.mjs'), 'utf8');
    expect(wireSource).toMatch(/const WIRE_ARGS = \['test', '-p', 'polaris-singbox-grpc', '--test', 'bundled_core_wire',\s*'vendored_proto_matches_every_bundled_core', '--', '--exact'\]/);
    expect(wireSource).toContain("run('cargo', WIRE_ARGS,");
    expect(wireSource).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
    const mandatoryStep = preflight.slice(mandatory);
    expect(mandatoryStep).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
    expect(mandatoryStep).toContain('cargo test -p polaris-config-engine --test managed_mesh_emission_runtime -- --nocapture');
    expect(mandatoryStep).not.toContain('POLARIS_NO_KERNEL_RUN');
  });

  it('四道随包内核门在 package.yml 与 release-risk.yml 两份定义之间逐条对拍', () => {
    // Rust 侧四条 `ci_step_still_wired`（crates/config-engine/tests/*.rs）只 grep package.yml。
    // 但**合入前路径上真正在跑的是 release-risk.yml 这一份**：删掉它 129-132 里任意一行，
    // 四条 Rust 断言零转红、合入前内核门静默少一道。本条把接线断言扩到两文件。
    const pkgGates = configGateTargets(pkg);
    const riskGates = configGateTargets(risk);
    const command = 'cargo test -p polaris-config-engine';
    expect(configGateTargets(`${command} --test one\n${command} --test two`)).toEqual(['one', 'two']);
    expect(configGateTargets(`${command} --test one --test two -- --nocapture`)).toEqual(['one', 'two']);
    expect(configGateTargets(`${command} \\\n  --test one \\\n  --test two`)).toEqual(['one', 'two']);
    expect(configGateTargets(`${command} --test one -- --nocapture --test-threads=1`)).toEqual(['one']);
    const fingerprints = ['core_dep_fingerprint', 'core_schema_surface', 'core_build_matrix'];
    const batch = `${command} ${fingerprints.map((target) => `--test ${target}`).join(' ')}`;
    expect(configGateTargets(batch)).toEqual([...fingerprints].sort());
    for (const target of fingerprints) {
      expect(configGateTargets(batch.replace(` --test ${target}`, ''))).not.toEqual([...fingerprints].sort());
      expect(configGateTargets(`${batch} --test ${target}`)).not.toEqual([...fingerprints].sort());
    }
    expect(configGateTargets(batch.replaceAll('--test', '--bin'))).toEqual([]);
    for (const rejected of [
      `# ${command} --test one`, `echo "${command} --test one"`,
      `${command} --test one -- --nocapture --test-threads=0`,
      `${command} --test one || true`, `${command} --test one -- ignored_filter`,
      'cargo test -p other-package --test one',
    ]) expect(configGateTargets(rejected)).toEqual([]);
    // 取材面自曝：两侧都枚举不到时不许「空集 === 空集」判绿。
    expect(pkgGates.length, 'package.yml 里一道随包内核门都没枚举到 —— 取材面塌了，本门此刻没有判据')
      .toBeGreaterThan(0);
    expect(riskGates, 'release-risk.yml 与 package.yml 的随包内核门清单不一致').toEqual(pkgGates);
    // 强制腿也是双份定义：少了它「核没拉到」会静默跳过而不是红。
    expect(pkg).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
    expect(risk).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
  });

  it('手动分支内核门保持桌面 fork 角色来源、旧 mobile alpha8 与 REQUIRE 硬化', () => {
    const manifest = JSON.parse(readFileSync(join(REPO_ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
    const source = JSON.parse(readFileSync(join(REPO_ROOT, 'scripts/desktop-core/source-manifest.json'), 'utf8'));
    expect(source.sourceCommit).toBe('aafc521b745e0d1a8f50a6a3f41169433f1c2194');
    expect(source.sourceTagObject).toBe('71876e8b33d3061bd9abba9618aa3e8c3e22db4e');
    expect(manifest.desktopSourceBuild.patchedSourceTree).toBe(source.sourceTree);
    expect(manifest.desktopSourceBuild.version).toBe('1.15.0-alpha.11.polaris.2');
    expect(manifest.sourceBuild.version).toBe('1.15.0-alpha.8.polaris.4');
    for (const path of ['libbox-patches', 'libbox-ios-patches']) {
      expect(JSON.parse(readFileSync(join(REPO_ROOT, 'scripts', path, 'source-manifest.json'), 'utf8')).sourceCommit)
        .toBe('b609f959f57ce34416c51c7b87ce4a76f2e1df56');
    }
    const preflight = jobSection(risk, 'preflight', 'release-risk.yml');
    const mandatory = preflight.slice(preflight.indexOf('      - name: Run mandatory bundled-core gates'));
    expect(preflight).toContain('run: node scripts/fetch-core.mjs');
    expect(mandatory).toContain("if: needs.classify.outputs.kernel == 'true'");
    expect(mandatory).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
    expect(mandatory).toContain('cargo test -p polaris-config-engine --test kernel_accepts_outbounds');
  });

  it('每条选择性腿都必须被 gate 要求，且它的选择开关必须真的接出来', () => {
    // 缺口形态（2026-09-04 加 Android 腿时发现）：本文件此前逐条点名 preflight / package，
    // 对「**新增**一条腿」结构性失明 —— 新腿可以不进 gate 的 needs、gate 可以不判它的 result，
    // 全程零转红。那正是本仓「门在但没牙」的形状，只是这次门是本文件自己。
    // 改成**类**断言：从文件里枚举 job 与 selector，逐个要求接线成立。
    const lines = risk.split('\n');
    // 先收射程到 `jobs:` 之后：两空格缩进的键在 `on:` 下面也有（pull_request / merge_group / push），
    // 不收射程的话枚举面里会混进触发器名，本门会为一个不存在的 job 要求 gate 接线（假红）。
    const jobsAt = lines.indexOf('jobs:');
    expect(jobsAt, 'release-risk.yml 里找不到 `jobs:`').toBeGreaterThanOrEqual(0);
    const jobsBlock = lines.slice(jobsAt).join('\n');
    const jobNames = [...jobsBlock.matchAll(/^ {2}([A-Za-z_][\w-]*):$/gm)].map((m) => m[1]);
    // 切片自检：射程真的收住了（`on:` 下的触发器名不该出现在 job 枚举面里）。
    expect(jobNames, '射程没收住：`on:` 下的触发器名混进了 job 枚举面').not.toContain('pull_request');
    // 取材面自曝：枚举器变哑时下面两个 for 会恒真。
    expect(jobNames, 'release-risk.yml 的 job 枚举面塌了').toContain('classify');
    expect(jobNames, 'release-risk.yml 的 job 枚举面塌了').toContain('gate');
    expect(jobNames.length, `只枚举到 ${jobNames.length} 个 job —— 枚举器坏了`).toBeGreaterThan(4);

    // gate 是最后一个 job，jobSection 的「必须有同级后继」自检对它不成立，故单独切到文件尾。
    const gateStart = lines.findIndex((l) => l === '  gate:');
    expect(gateStart, 'release-risk.yml 里找不到 gate job').toBeGreaterThanOrEqual(0);
    const gate = lines.slice(gateStart).join('\n');

    for (const job of jobNames.filter((n) => n !== 'classify' && n !== 'gate')) {
      // ① gate 必须 needs 它 —— 不 needs 的话 `needs.<job>.result` 求值为空串，
      //    「非 success 就红」那条断言恒不成立，等于**恒绿**。
      expect(
        gate,
        `gate 的 needs 里没有 ${job} —— 它的 result 会是空串，下面那条断言恒绿`,
      ).toMatch(new RegExp(`needs: \\[[^\\]]*\\b${job}\\b`));

      // ② result 必须被读进 env。
      const envMatch = gate.match(
        new RegExp(`^\\s+([A-Z_]+): \\$\\{\\{ needs\\.${job}\\.result \\}\\}$`, 'm'),
      );
      expect(envMatch, `gate 没有读取 needs.${job}.result —— 这条腿挂了也不会拦住合入`).not.toBeNull();

      // ③ 必须有一条「非 success 就 exit 1」的断言。
      expect(
        hasStrictResultGate(gate, envMatch![1]),
        `gate 读了 ${job} 的 result 却没有据它判红 —— 读进来不等于判据`,
      ).toBe(true);
    }

    // ④ 每个「选哪条腿」的开关都必须在 classify 的 outputs 与 impact step 里真的接出来。
    //    少接一处 ⇒ `needs.classify.outputs.<X>` 恒为空串 ⇒ 那条腿**永远不跑**，且没有任何东西会说。
    const classify = jobSection(risk, 'classify', 'release-risk.yml');
    const selectors = [
      ...new Set([...risk.matchAll(/needs\.classify\.outputs\.([a-z_]+)/g)].map((m) => m[1])),
    ].sort();
    expect(selectors.length, '一个 classify 选择开关都没枚举到 —— 取材面塌了').toBeGreaterThan(3);
    for (const selector of selectors) {
      // candidate is immutable event identity, not a classifier decision or
      // impact-step bool. Its original provenance must remain explicit.
      if (selector === 'candidate') {
        expect(classify).toContain('candidate: ${{ github.event.pull_request.head.sha || github.sha }}');
        continue;
      }
      if (selector === 'native_validation') {
        expect(classify).toContain("native_validation: ${{ github.event_name == 'workflow_dispatch' && inputs.mode == 'native-payload' && 'true' || 'false' }}");
        const enabled = (event: string, mode: string) => runInNewContext(
          "github.event_name == 'workflow_dispatch' && inputs.mode == 'native-payload' && 'true' || 'false'",
          { github: { event_name: event }, inputs: { mode } },
        );
        expect(enabled('workflow_dispatch', 'native-payload')).toBe('true');
        expect(enabled('workflow_dispatch', 'ordinary')).toBe('false');
        expect(enabled('push', 'native-payload')).toBe('false');
        continue;
      }
      expect(
        classify,
        `classify job 没有声明 output \`${selector}\` —— 引用它的地方恒为空串`,
      ).toMatch(new RegExp(`^\\s+${selector}: \\$\\{\\{ steps\\.impact\\.outputs\\.${selector} \\}\\}$`, 'm'));
      expect(
        classify,
        `impact step 没有写出 \`${selector}\` —— output 声明了但没人赋值`,
      ).toContain(`echo "${selector}=`);
    }
  });

  it('真实最终 gate 对选中的 desktop_core 只接受 success，未选中允许 skipped', () => {
    const gate = risk.slice(risk.indexOf('\n  gate:\n'));
    expect(gate).toContain('DESKTOP_CORE_REQUIRED: ${{ needs.classify.outputs.preflight }}');
    expect(gate).toContain('DESKTOP_CORE_RESULT: ${{ needs.desktop_core.result }}');
    const runAt = gate.indexOf('        run: |\n');
    expect(runAt).toBeGreaterThan(0);
    const script = gate.slice(runAt + '        run: |\n'.length)
      .split('\n').map((line) => line.slice(10)).join('\n');
    const base = {
      CLASSIFY_RESULT: 'success', PC_RUNTIME_POLICY_RESULT: 'success', UNREGISTERED: '',
      DESKTOP_CORE_REQUIRED: 'true', DESKTOP_CORE_RESULT: 'success',
      PREFLIGHT_REQUIRED: 'true', PREFLIGHT_RESULT: 'success',
      PACKAGE_REQUIRED: 'true', PACKAGE_RESULT: 'success',
      ANDROID_REQUIRED: 'true', ANDROID_RESULT: 'success',
      APPLE_REQUIRED: 'false', APPLE_RESULT: 'skipped',
      NATIVE_VALIDATION_REQUIRED: 'false', NATIVE_VALIDATION_RESULT: 'skipped',
    };
    const run = (body: string, required: string, result: string) => spawnSync('bash', ['-c', body], {
      env: { ...process.env, ...base, DESKTOP_CORE_REQUIRED: required, DESKTOP_CORE_RESULT: result },
      encoding: 'utf8',
    });
    const check = (body: string) => {
      expect(run(body, 'true', 'success').status).toBe(0);
      expect(run(body, 'false', 'skipped').status).toBe(0);
      for (const result of ['failure', 'cancelled', 'skipped', '']) {
        const actual = run(body, 'true', result);
        expect(actual.status, `selected desktop_core=${result}: ${actual.stdout}`).toBe(1);
        expect(actual.stdout).toContain('四平台内核源码产物未通过：');
      }
    };
    check(script);
    // The same actual shell controls must catch permissive status tests and
    // a guard that observes the result but exits successfully.
    for (const changed of [
      script.replace('[ "$DESKTOP_CORE_RESULT" != success ]', '[ "$DESKTOP_CORE_RESULT" = failure ]'),
      script.replace('echo "::error::四平台内核源码产物未通过：$DESKTOP_CORE_RESULT"\n  exit 1', 'echo "::error::四平台内核源码产物未通过：$DESKTOP_CORE_RESULT"\n  exit 0'),
    ]) {
      expect(changed).not.toBe(script);
      expect(() => check(changed)).toThrow();
    }
  });

  it('result 门接受严格等价写法，缺失失败退出或成功绕过仍拒绝', () => {
    const variable = 'PC_RUNTIME_POLICY_RESULT';
    const positive = '[ "$PC_RUNTIME_POLICY_RESULT" = success ] || {\n  echo "failure"; exit 1; }';
    const negative = 'if [ "$PC_RUNTIME_POLICY_RESULT" != success ]; then\n  echo "failure"\n  exit 1\nfi';
    expect(hasStrictResultGate(positive, variable)).toBe(true);
    expect(hasStrictResultGate(negative, variable)).toBe(true);
    const combined = '[ "$DESKTOP_CORE_RESULT" = success ] && [ "$NATIVE_VALIDATION_RESULT" = success ] || {\n  echo \'failure\'; exit 1; }';
    expect(hasStrictResultGate(combined, 'DESKTOP_CORE_RESULT')).toBe(true);
    expect(hasStrictResultGate(combined, 'NATIVE_VALIDATION_RESULT')).toBe(true);
    expect(hasStrictResultGate(combined, 'OTHER_RESULT')).toBe(false);
    for (const changed of [combined.replace(' && ', ' || '), combined.replace('exit 1', 'exit 0'),
      combined.replace('= success', '!= success'), `${combined} || true`]) {
      expect(hasStrictResultGate(changed, 'DESKTOP_CORE_RESULT'), changed).toBe(false);
    }
    for (const body of [positive.replace('exit 1;', ''), positive.replace(variable, 'OTHER_RESULT'),
      positive.replace('exit 1', 'exit 0'), positive.replace('= success', '!= success'),
      positive.replace('exit 1', 'true'), positive.replace('exit 1;', 'exit 1 || true;'),
      `${positive} || true`, negative.replace('exit 1', 'exit 0'),
      negative.replace('exit 1', 'echo skipped'), negative.replace(variable, 'OTHER_RESULT'),
      positive.split('\n').map((line) => `# ${line}`).join('\n'),
      negative.split('\n').map((line) => `# ${line}`).join('\n')]) {
      expect(hasStrictResultGate(body, variable), body).toBe(false);
    }
  });

  it('真实 native 汇总同时要求四核与 final payload success，错误状态及实际门突变均拒绝', () => {
    const gate = risk.slice(risk.indexOf('\n  gate:\n'));
    expect(gate).toContain('NATIVE_VALIDATION_REQUIRED: ${{ needs.classify.outputs.native_validation }}');
    expect(gate).toContain('NATIVE_VALIDATION_RESULT: ${{ needs.native_payload.result }}');
    const runAt = gate.indexOf('        run: |\n');
    expect(runAt).toBeGreaterThan(0);
    const script = gate.slice(runAt + '        run: |\n'.length).split('\n').map((line) => line.slice(10)).join('\n');
    const base = {
      CLASSIFY_RESULT: 'success', PC_RUNTIME_POLICY_RESULT: 'success', UNREGISTERED: '',
      NATIVE_VALIDATION_REQUIRED: 'true', DESKTOP_CORE_RESULT: 'success', NATIVE_VALIDATION_RESULT: 'success',
      DESKTOP_CORE_REQUIRED: 'false', PREFLIGHT_REQUIRED: 'false', PREFLIGHT_RESULT: 'skipped',
      PACKAGE_REQUIRED: 'false', PACKAGE_RESULT: 'skipped', ANDROID_REQUIRED: 'false', ANDROID_RESULT: 'skipped',
      APPLE_REQUIRED: 'false', APPLE_RESULT: 'skipped',
    };
    const run = (body: string, changed: Record<string, string>) => spawnSync('bash', ['-c', body], {
      env: { ...process.env, ...base, ...changed }, encoding: 'utf8',
    });
    const check = (body: string) => {
      expect(run(body, {}).status).toBe(0);
      expect(run(body, { NATIVE_VALIDATION_REQUIRED: 'false', DESKTOP_CORE_RESULT: 'skipped', NATIVE_VALIDATION_RESULT: 'skipped' }).status).toBe(0);
      for (const field of ['DESKTOP_CORE_RESULT', 'NATIVE_VALIDATION_RESULT']) {
        for (const result of ['failure', 'cancelled', 'skipped', '', 'unknown']) {
          const actual = run(body, { [field]: result });
          expect(actual.status, `${field}=${result}: ${actual.stdout + actual.stderr}`).toBe(1);
          expect(actual.stdout).toContain('native source/actual final payload proof did not pass');
        }
      }
    };
    check(script);
    for (const changed of [
      script.replace('[ "$DESKTOP_CORE_RESULT" = success ] && [ "$NATIVE_VALIDATION_RESULT" = success ]',
        '[ "$DESKTOP_CORE_RESULT" = success ] || [ "$NATIVE_VALIDATION_RESULT" = success ]'),
      script.replace("echo '::error::native source/actual final payload proof did not pass'; exit 1;",
        "echo '::error::native source/actual final payload proof did not pass'; exit 0;"),
      script.replace('[ "$NATIVE_VALIDATION_REQUIRED" = true ]', '[ "$NATIVE_VALIDATION_REQUIRED" = false ]'),
    ]) {
      expect(changed).not.toBe(script);
      expect(() => check(changed)).toThrow();
    }
  });

  it('CI 与 UI 都覆盖 merge_group，避免 merge queue 等不到 required check', () => {
    expect(read('ci.yml')).toContain('merge_group:');
    expect(read('ui.yml')).toContain('merge_group:');
  });
});
