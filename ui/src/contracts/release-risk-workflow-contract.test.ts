import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
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
  });

  it('手动选 tag 当场失败，不会到全量分类和 Package 发布路径', () => {
    const classify = jobSection(risk, 'classify', 'release-risk.yml');
    const lines = classify.split('\n');
    const classifyStep = lines.indexOf('      - name: Classify changed paths');
    expect(classifyStep, 'classify 缺路径分类步骤').toBeGreaterThanOrEqual(0);
    const start = lines.indexOf('        run: |', classifyStep);
    expect(start, 'classify 缺 shell 脚本').toBeGreaterThanOrEqual(0);
    const script = lines.slice(start + 1).map((line) => line.slice(10)).join('\n');
    const result = spawnSync('bash', ['-c', script], {
      cwd: REPO_ROOT,
      env: { ...process.env, EVENT_NAME: 'workflow_dispatch', DISPATCH_REF: 'refs/tags/v1.0.0' },
      encoding: 'utf8',
    });
    expect(result.status, result.stderr).toBe(1);
    expect(result.stdout).toContain('::error::手动发布风险门只接受 branch ref：refs/tags/v1.0.0');
    expect(result.stdout).not.toContain('无法取得可靠 diff');
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

  it('Package 的 tag/直接手动入口保留强制门；Release Risk 手动复用尊重两个 false 输入', () => {
    const packageJob = jobSection(pkg, 'package', 'package.yml');
    const expressions = ['POLARIS_RUN_KERNEL_GATES', 'POLARIS_UPLOAD_ARTIFACTS'].map((key) => {
      const match = packageJob.match(new RegExp(`^      ${key}: \\$\\{\\{ (.+) \\}\\}$`, 'm'));
      expect(match, `package job 缺 ${key} 表达式`).not.toBeNull();
      return match![1];
    });
    const evaluate = (expression: string, ref: string, event_name: string, skip_quality_gates: boolean) =>
      runInNewContext(expression, {
        github: { ref, event_name },
        inputs: { skip_quality_gates, run_kernel_gates: false, upload_artifacts: false },
        startsWith: (value: string, prefix: string) => value.startsWith(prefix),
      });

    for (const expression of expressions) {
      expect(evaluate(expression, 'refs/tags/v1.0.0', 'push', false)).toBe('1');
      expect(evaluate(expression, 'refs/heads/feature', 'workflow_dispatch', false)).toBe('1');
      expect(evaluate(expression, 'refs/heads/feature', 'workflow_dispatch', true)).toBe('0');
    }
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

  it('手动分支分类命中内核时，仍保持 b609 随包核和现有 REQUIRE 硬化', () => {
    const manifest = JSON.parse(readFileSync(join(REPO_ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
    expect(manifest.windowsBuild.sourceCommit).toBe('b609f959f57ce34416c51c7b87ce4a76f2e1df56');
    const preflight = jobSection(risk, 'preflight', 'release-risk.yml');
    const mandatory = preflight.slice(preflight.indexOf('      - name: Run mandatory bundled-core gates'));
    expect(preflight).toContain('run: node scripts/fetch-core.mjs');
    expect(mandatory).toContain("if: needs.classify.outputs.kernel == 'true'");
    expect(mandatory).toContain("POLARIS_REQUIRE_KERNEL_GATE: '1'");
    expect(mandatory).toContain('cargo test -p polaris-config-engine --test kernel_accepts_outbounds');
  });

  it('CI 与 UI 都覆盖 merge_group，避免 merge queue 等不到 required check', () => {
    expect(read('ci.yml')).toContain('merge_group:');
    expect(read('ui.yml')).toContain('merge_group:');
  });
});
