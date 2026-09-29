import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
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

  it('手动分支使用对应 head 与最新 main 的 merge-base，取证失败退回全量门', () => {
    const classify = jobSection(risk, 'classify', 'release-risk.yml');
    const start = classify.indexOf('            workflow_dispatch)');
    const end = classify.indexOf('              fi ;;', start);
    expect(start, 'classify 缺手动事件分支').toBeGreaterThanOrEqual(0);
    expect(end, '手动事件分支缺结束边界').toBeGreaterThan(start);
    const dispatch = classify.slice(start, end);
    expect(classify).toContain('DISPATCH_REF: ${{ github.ref }}');
    expect(classify).toContain('DISPATCH_SHA: ${{ github.sha }}');
    expect(dispatch).toContain('[[ "$DISPATCH_REF" == refs/heads/* ]]');
    expect(dispatch).toContain('git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main');
    expect(dispatch).toContain('dispatch_head=$(git rev-parse --verify "${DISPATCH_REF}^{commit}" 2>/dev/null)');
    expect(dispatch).toContain('[ "$dispatch_head" = "$DISPATCH_SHA" ]');
    expect(dispatch).toContain('base=$(git merge-base refs/remotes/origin/main "$dispatch_head")');
    expect(dispatch).toContain('head="$dispatch_head"');
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
    expect(risk).toContain('uses: ./.github/workflows/package.yml');
    expect(risk).toContain('needs: [classify, preflight]');
    expect(risk).toContain("needs.preflight.result == 'success'");
    const pkgJob = jobSection(risk, 'package', 'release-risk.yml');
    // 切片自检：拿到的确实是这个 job，且没有把下一个 job 卷进来。
    expect(pkgJob).toContain('uses: ./.github/workflows/package.yml');
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
    expect(manifest.windowsBuild.sourceCommit).toBe('b609f959f57ce34416c51c7b87ce4a76f2e1df56');
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
    expect(preflight).toContain('cargo test -p polaris-singbox-grpc --test bundled_core_wire vendored_proto_matches_every_bundled_core -- --exact');
    const mandatoryStep = preflight.slice(mandatory);
    expect(mandatoryStep).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
    expect(mandatoryStep).toContain('cargo test -p polaris-config-engine --test managed_mesh_emission_runtime -- --nocapture');
    expect(mandatoryStep).not.toContain('POLARIS_NO_KERNEL_RUN');
  });

  it('四道随包内核门在 package.yml 与 release-risk.yml 两份定义之间逐条对拍', () => {
    // Rust 侧四条 `ci_step_still_wired`（crates/config-engine/tests/*.rs）只 grep package.yml。
    // 但**合入前路径上真正在跑的是 release-risk.yml 这一份**：删掉它 129-132 里任意一行，
    // 四条 Rust 断言零转红、合入前内核门静默少一道。本条把接线断言扩到两文件。
    const gatesIn = (src: string) => [
      ...new Set(
        [...src.matchAll(/cargo test -p polaris-config-engine --test ([a-z_]+)/g)].map((m) => m[1])
      ),
    ].sort();
    const pkgGates = gatesIn(pkg);
    const riskGates = gatesIn(risk);
    // 取材面自曝：两侧都枚举不到时不许「空集 === 空集」判绿。
    expect(pkgGates.length, 'package.yml 里一道随包内核门都没枚举到 —— 取材面塌了，本门此刻没有判据')
      .toBeGreaterThan(0);
    expect(riskGates, 'release-risk.yml 与 package.yml 的随包内核门清单不一致').toEqual(pkgGates);
    // 强制腿也是双份定义：少了它「核没拉到」会静默跳过而不是红。
    expect(pkg).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
    expect(risk).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
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
        gate,
        `gate 读了 ${job} 的 result 却没有据它判红 —— 读进来不等于判据`,
      ).toMatch(new RegExp(`\\$${envMatch![1]}" != success[\\s\\S]{0,240}?exit 1`));
    }

    // ④ 每个「选哪条腿」的开关都必须在 classify 的 outputs 与 impact step 里真的接出来。
    //    少接一处 ⇒ `needs.classify.outputs.<X>` 恒为空串 ⇒ 那条腿**永远不跑**，且没有任何东西会说。
    const classify = jobSection(risk, 'classify', 'release-risk.yml');
    const selectors = [
      ...new Set([...risk.matchAll(/needs\.classify\.outputs\.([a-z_]+)/g)].map((m) => m[1])),
    ].sort();
    expect(selectors.length, '一个 classify 选择开关都没枚举到 —— 取材面塌了').toBeGreaterThan(3);
    for (const selector of selectors) {
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

  it('CI 与 UI 都覆盖 merge_group，避免 merge queue 等不到 required check', () => {
    expect(read('ci.yml')).toContain('merge_group:');
    expect(read('ui.yml')).toContain('merge_group:');
  });
});
