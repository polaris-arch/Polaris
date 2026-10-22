/**
 * package(all) → reusable CI 的矩阵入口契约。
 *
 * GitHub workflow_call 会继承外层 workflow_dispatch 的 event_name/payload。package.yml 的输入叫
 * `platform`，没有 ci.yml 自身手动入口的 `os`；因此 package(all) 调 CI 时
 * `github.event.inputs.os === ''`。空串若只经过 `!= 'all'` 判定，会被误当成具体平台并生成 `[""]`，
 * 最终是 `runs-on: ""` / labels=[] 的永久 pending job（run 32357370395 的真实失败形态）。
 */

import { describe, expect, it, onTestFinished } from 'vitest';
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { execFile, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { runInNewContext } from 'node:vm';

const REPO_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

function workflow(name: string): string {
  return readFileSync(join(REPO_ROOT, '.github/workflows', name), 'utf8');
}

function workflowConcurrency(source: string, caller: string, eventName = 'workflow_dispatch',
  ref = 'refs/heads/collab/contract', preserve = false, runId = 1,
  selection: { platform?: string; platforms?: string; pullRequestNumber?: number;
    mergeGroupHead?: string; eventPreserveRun?: boolean } = {}) {
  const match = source.match(/^  group: (.+)\n  cancel-in-progress: \$\{\{ (.+) \}\}$/m);
  expect(match, 'workflow concurrency must be executable').not.toBeNull();
  const context = {
    github: { workflow: caller, event_name: eventName, ref, run_id: runId,
      event: { inputs: { preserve_run: preserve || selection.eventPreserveRun ? 'true' : 'false' },
        pull_request: { number: selection.pullRequestNumber }, merge_group: { head_sha: selection.mergeGroupHead } } },
    inputs: { preserve_run: preserve, platform: selection.platform ?? '', platforms: selection.platforms ?? '' },
    startsWith: (value: string, prefix: string) => value.startsWith(prefix),
    format: (value: string, argument: number) => value.replace('{0}', String(argument)),
  };
  return { group: match![1].replace(/\$\{\{ (.+?) \}\}/g,
    (_text, expression: string) => String(runInNewContext(expression, context))),
  cancels: runInNewContext(match![2], context) };
}

/**
 * 取出 `jobs:` 下某个 job 的整段文本（含其注释），从 `\n  <name>:\n` 到下一个同缩进 job 键为止。
 *
 * 为什么必须按 job 切片、而不是对整个文件 `toContain`：判据是「这套 allowlist 挂在**哪个** job 上」。
 * 2026-09-04 打包腿与质量门改并行，两门的 allowlist 从 `package` 搬到了发布写入腿；如果断言只看
 * 全文，搬家前后同样通过——门在，但对「挂错 job」这件事完全没有牙。
 */
function jobBlock(src: string, name: string): string {
  const start = src.indexOf(`\n  ${name}:\n`);
  if (start < 0) throw new Error(`package.yml 里找不到 job '${name}' —— 取材面塌了，本门此刻没有判据`);
  const rest = src.slice(start + 1);
  const next = rest.slice(1).search(/\n {2}[a-z][a-z0-9_-]*:\n/);
  return next < 0 ? rest : rest.slice(0, next + 1);
}

/** 取出 job 内一个具名 step，避免跨 step 的同名环境变量让接线断言假绿。 */
function stepBlock(src: string, name: string): string {
  const start = src.indexOf(`\n      - name: ${name}\n`);
  if (start < 0) throw new Error(`job 里找不到 step '${name}' —— 取材面塌了，本门此刻没有判据`);
  const rest = src.slice(start + 1);
  const next = rest.slice(1).search(/\n {6}- (?:name:|uses:)/);
  return next < 0 ? rest : rest.slice(0, next + 1);
}

/**
 * 剥掉整行注释，只留 job 的可执行部分。
 *
 * 为什么必须剥：本文件的判据字符串（`needs.package.result == 'success'` 等）在同一段 YAML 的
 * **解释性注释里也逐字出现**。不剥注释就是「判据被自己污染」——把 `if:` 里的那条判据整行删掉，
 * 断言仍然从注释里读到同一串字而通过。2026-09-04 的变异实测确实出现过这个假绿。
 */
function executable(block: string): string {
  return block
    .split('\n')
    .filter((line) => !/^\s*#/.test(line))
    .join('\n');
}

/** job 级 `if: >-` 折叠块压成一行；喂进来的必须是 `executable()` 剥过注释的 job 块。 */
function foldedJobIf(job: string, name: string): string {
  const match = /^ {4}if: >-\n((?: {6}.*\n)+)/m.exec(job);
  if (!match) throw new Error(`job '${name}' 没有折叠形态的 job 级 if —— 取材面塌了，本门此刻没有判据`);
  return match[1].split('\n').map((line) => line.trim()).filter(Boolean).join(' ');
}

/** 矩阵 job 的 `os: >-` 表达式（去掉 `${{ }}` 外壳、压成一行）。 */
function matrixOsExpression(job: string, name: string): string {
  const match = /^ {8}os: >-\n((?: {10}.*\n)+)/m.exec(job);
  if (!match) throw new Error(`job '${name}' 没有 \`os: >-\` 矩阵表达式 —— 取材面塌了，本门此刻没有判据`);
  const flat = match[1].split('\n').map((line) => line.trim()).filter(Boolean).join(' ');
  const inner = /^\$\{\{ (.+) \}\}$/.exec(flat);
  if (!inner) throw new Error(`job '${name}' 的矩阵表达式不是单个 \${{ }} —— 求值器喂不进去`);
  return inner[1];
}

/**
 * 用 JS 求值一条 Actions 表达式。两者在这里用到的子集上同构（`==` / `!=` / `&&` / `||` / `!`，
 * 且 `&&` / `||` 都返回操作数本身），只差一处：Actions 把**缺失的属性**当 null 并在比较时
 * 强转成 `''`，JS 里它是 undefined 且 `undefined != ''` 为真。故调用方对「没有这个输入」
 * 一律显式传 `''`，那正是 Actions 那一侧比较时看到的值。
 */
function evaluateActions(expression: string, github: { event_name: string; ref: string; os: string }, nativeRequired = ''): unknown {
  return runInNewContext(expression, {
    github: { event_name: github.event_name, ref: github.ref, event: { inputs: { os: github.os } } },
    needs: { cheap_format: { outputs: { native_required: nativeRequired } } },
    fromJSON: (text: string) => JSON.parse(text),
    format: (template: string, value: string) => template.replace('{0}', value),
  });
}

// Match Actions' case-insensitive mode routing; the actual Node admission remains
// the case-sensitive authority. This is routing, never authorization to run HOT.
const runtimeModeFunctions = {
  contains: (haystack: string, needle: string) => String(haystack).toLowerCase().includes(String(needle).toLowerCase()),
  toJSON: JSON.stringify,
  diagnosticMode: (mode: unknown) => typeof mode === 'string' && mode.toLowerCase() === 'diagnostic-source',
};
const runtimeModeExpression = (expression: string) => expression.replaceAll(
  "inputs.runtime_config_loopback_input != 'diagnostic-source'", '!diagnosticMode(inputs.runtime_config_loopback_input)',
);

/** 本仓 ci.yml 会遇到的全部入口（`os` 列：没有该输入的事件按 Actions 语义记作 ''）。 */
const CI_ENTRIES = [
  { label: 'push main', event_name: 'push', ref: 'refs/heads/main', os: '' },
  { label: 'pull_request', event_name: 'pull_request', ref: 'refs/pull/7/merge', os: '' },
  { label: 'merge_group', event_name: 'merge_group', ref: 'refs/heads/gh-readonly-queue/main/pr-7', os: '' },
  { label: 'tag push 经 package.yml workflow_call', event_name: 'push', ref: 'refs/tags/v1.0.0', os: '' },
  { label: 'package.yml 手动 dispatch 经 workflow_call（payload 无 os）', event_name: 'workflow_dispatch', ref: 'refs/heads/main', os: '' },
  { label: 'ci dispatch os=all', event_name: 'workflow_dispatch', ref: 'refs/heads/main', os: 'all' },
  { label: 'ci dispatch os=ubuntu-22.04', event_name: 'workflow_dispatch', ref: 'refs/heads/main', os: 'ubuntu-22.04' },
  { label: 'ci dispatch os=windows-2022', event_name: 'workflow_dispatch', ref: 'refs/heads/main', os: 'windows-2022' },
  { label: 'ci dispatch os=macos-14', event_name: 'workflow_dispatch', ref: 'refs/heads/main', os: 'macos-14' },
] as const;

/**
 * 「四处远端 digest 裁判」那组用例的两个限时。
 *
 * 每条用例真起一个 bash，里面再起 1–3 次 `gh api`（打本机回环上的夹具），脚本自己的重试 sleep 已被
 * 桩掉。本机空载实测单条 90–650ms。此前子进程限时 3500ms + SIGKILL：远端 runner 上整套 vitest
 * 并行跑时（三百多个测试文件抢 4 个核，gh 是个冷启动的 Go 二进制）偶发超过它，子进程被杀、
 * `status` 为 null，用例红 —— 红的是机器负载，不是被测脚本。
 *
 * 20s 约为本机最慢实测的 30 倍：负载抖动撞不到；真挂住（gh 等不到回环夹具的响应）仍会在 20s
 * 被 SIGKILL 并以 status=null 判红，不会无限等。限时保留、只是不再贴着正常耗时。
 * 用例自身的限时必须**大于**子进程限时：vitest 默认 5000ms，若它先到，报出来的是一句没有
 * stdout / stderr 的 `Test timed out`，子进程到底卡在哪就看不到了。
 */
const DIGEST_GUARD_CHILD_TIMEOUT_MS = 20_000;
const DIGEST_GUARD_TEST_TIMEOUT_MS = 30_000;

describe('package 全平台前置 CI 的矩阵输入', () => {
  it('空 os 必须回到完整矩阵，只有显式非 all 的 os 才能走单平台', () => {
    const ci = workflow('ci.yml');
    expect(ci).toMatch(
      /github\.event_name == 'workflow_dispatch'\s*&& github\.event\.inputs\.os != ''\s*&& github\.event\.inputs\.os != 'all'/s,
    );
    expect(ci).toContain(`fromJSON(format('["{0}"]', github.event.inputs.os))`);
    expect(ci).toContain(`fromJSON('["ubuntu-22.04","windows-2022","macos-14"]')`);

    // 2026-10-07 拆成并行 job 后矩阵表达式有两份（lint / test）。上面三条 `toContain` 对「只改坏
    // 其中一份」是瞎的（另一份照样喂饱它们），故按 job 取出来逐字比，并把表达式真的求值一遍：
    // 字面量在不在 ≠ 表达式算出来对不对。
    const lint = matrixOsExpression(executable(jobBlock(ci, 'lint')), 'lint');
    const test = matrixOsExpression(executable(jobBlock(ci, 'test')), 'test');
    expect(test, 'lint 与 test 两个矩阵 job 的 os 表达式不再逐字相同 —— 同一事件下两边跑的平台分家了').toBe(lint);
    const all = ['ubuntu-22.04', 'windows-2022', 'macos-14'];
    const expected: Record<(typeof CI_ENTRIES)[number]['label'], string[]> = {
      'push main': ['ubuntu-22.04'],
      pull_request: all,
      merge_group: all,
      'tag push 经 package.yml workflow_call': all,
      // run 32357370395 的真实失败形态：这一格若算出 [""]，job 永久 pending。
      'package.yml 手动 dispatch 经 workflow_call（payload 无 os）': all,
      'ci dispatch os=all': all,
      'ci dispatch os=ubuntu-22.04': ['ubuntu-22.04'],
      'ci dispatch os=windows-2022': ['windows-2022'],
      'ci dispatch os=macos-14': ['macos-14'],
    };
    for (const entry of CI_ENTRIES) {
      expect(evaluateActions(lint, entry), entry.label).toEqual(expected[entry.label]);
    }
  });

  it('ci.yml 格式前置门与三个并行 job：Linux 专属检查恰在 ubuntu 腿存在时运行，且不能静默关闭', () => {
    // 背景：2026-10-07 之前交叉 clippy / Android dep-info 对差 / 豁免反腐烂是矩阵 job 里
    // `if: runner.os == 'Linux'` 的三步 —— 「ubuntu 腿在跑」⇔「这三步在跑」由构造保证。
    // 独立成 `cross` job 后这个等价关系要靠 job 级 `if` 维持，而 android-impact-coverage-contract
    // 的 D2 / D4 只查**步级** `if` / continue-on-error：job 级写一个 `if: false` 它们全绿。
    // 本条补的就是这条新路径。
    const ci = workflow('ci.yml');
    const jobs = [...ci.slice(ci.indexOf('\njobs:\n')).matchAll(/^ {2}([a-z][a-z0-9_-]*):$/gm)].map((m) => m[1]);
    expect(jobs, 'ci.yml 的 job 集合变了 —— 新 job 要有人过目：它的缓存键、它能不能被关掉').toEqual([
      'cheap_format', 'native_dns_candidate_gates', 'lint', 'cross', 'test', 'runtime_config_loopback', 'runtime_config_source_tests', 'windows_launcher_diagnostics',
    ]);

    // Manual candidate and committed formal validation reuse the same App commit; ordinary gates remain mandatory.
    const candidate = executable(jobBlock(ci, 'native_dns_candidate_gates'));
    expect(candidate).toContain('needs: cheap_format');
    expect(candidate).toContain("github.event_name == 'workflow_dispatch'");
    expect(candidate).toContain("github.workflow_ref == 'polaris-arch/Polaris/.github/workflows/ci.yml@refs/heads/collab/native-node-dns-20261018'");
    expect(candidate).toContain("inputs.native_dns_candidate != ''");
    expect(candidate).toContain('needs.cheap_format.outputs.native_dns_candidate == inputs.native_dns_candidate');
    expect(candidate).toContain('uses: ./.github/workflows/native-dns-candidate.yml');
    expect(candidate).toContain('candidate: ${{ needs.cheap_format.outputs.native_dns_candidate || needs.cheap_format.outputs.native_dns_formal_app }}');
    expect(candidate).toContain("formal_release: ${{ needs.cheap_format.outputs.native_dns_formal == 'true' }}");
    const candidateIf = /^\$\{\{ (.+) \}\}$/.exec(foldedJobIf(candidate, 'native_dns_candidate_gates'));
    expect(candidateIf).not.toBeNull();
    const app = 'a'.repeat(40);
    const manualWorkflow = 'polaris-arch/Polaris/.github/workflows/ci.yml@refs/heads/collab/native-node-dns-20261018';
    const context = (event: string, workflowRef: string, input = '', selected = '', formal = '', formalApp = '') => ({
      github: { event_name: event, workflow_ref: workflowRef }, inputs: { native_dns_candidate: input },
      needs: { cheap_format: { outputs: { native_dns_candidate: selected, native_dns_formal: formal,
        native_dns_formal_app: formalApp } } },
    });
    const admissionCases = [
      { name: 'exact manual candidate', env: context('workflow_dispatch', manualWorkflow, app, app), admitted: true, app },
      { name: 'candidate differs', env: context('workflow_dispatch', manualWorkflow, app, 'b'.repeat(40)), admitted: false },
      { name: 'candidate absent', env: context('workflow_dispatch', manualWorkflow), admitted: false },
      { name: 'candidate wrong event', env: context('push', manualWorkflow, app, app), admitted: false },
      { name: 'candidate wrong caller', env: context('workflow_dispatch', 'other/workflow', app, app), admitted: false },
      { name: 'formal flag absent', env: context('push', 'ordinary CI', '', '', '', app), admitted: false },
      { name: 'formal flag false', env: context('push', 'ordinary CI', '', '', 'false', app), admitted: false },
      ...['push', 'pull_request', 'merge_group', 'workflow_dispatch'].map((event) => ({
        name: `admitted formal ${event}`, env: context(event, 'ordinary CI', '', '', 'true', app), admitted: true, app,
      })),
    ];
    const candidateInput = /^ {6}candidate: \$\{\{ (.+) \}\}$/m.exec(candidate);
    expect(candidateInput).not.toBeNull();
    for (const entry of admissionCases) {
      expect(runInNewContext(candidateIf![1], entry.env), entry.name).toBe(entry.admitted);
      if (entry.admitted) expect(runInNewContext(candidateInput![1], entry.env), entry.name).toBe(entry.app);
    }
    for (const entry of admissionCases) {
      for (const nativeRequired of ['', 'true', 'unknown', 'false']) {
        const env = structuredClone(entry.env);
        Object.assign(env.needs.cheap_format.outputs, { native_required: nativeRequired });
        expect(runInNewContext(candidateIf![1], env)).toBe(entry.admitted && nativeRequired !== 'false');
      }
    }
    expect(candidate).toMatch(/permissions:\s*\n\s*contents: read/);
    expect(candidate).not.toMatch(/secrets:|continue-on-error:|steps:|runs-on:|actions\/cache|contents: write/);

    const cheapFormat = executable(jobBlock(ci, 'cheap_format'));
    const lint = executable(jobBlock(ci, 'lint'));
    const cross = executable(jobBlock(ci, 'cross'));
    const test = executable(jobBlock(ci, 'test'));

    // ① 归属：每条门恰好住在一个 job 里（检查类与构建测试类分开；Linux 专属四步都在 cross）。
    const home: [string, string][] = [
      ['- name: Check workspace formatting before native compilation', 'cheap_format'],
      ['- name: Check formatting', 'lint'],
      ['- name: Clippy (deny warnings)', 'lint'],
      ['- name: Rustdoc documentation invariants (deny four lints)', 'lint'],
      ['- name: Resolve core-owned Cronet dependencies', 'lint'],
      ['- name: Cross-check platform targets', 'cross'],
      ['- name: Vendored path sources stay diagnostic-free', 'cross'],
      ['- name: Android impact face must be registered', 'cross'],
      ['- name: Cross-target exemptions must still be necessary', 'cross'],
      ['- name: Build\n', 'test'],
      ['- name: Test\n', 'test'],
    ];
    const blocks = { cheap_format: cheapFormat, lint, cross, test };
    for (const [needle, owner] of home) {
      for (const [name, block] of Object.entries(blocks)) {
        expect(block.includes(needle), `「${needle.trim()}」应只在 ${owner} job 里（现在查的是 ${name}）`)
          .toBe(name === owner);
      }
    }

    // ② Only the classifier's explicit false may omit native jobs. Missing or
    // malformed output keeps them mandatory; PR/call/manual classification is full.
    for (const [name, block] of [['lint', lint], ['test', test]] as const) {
      const condition = foldedJobIf(block, name);
      expect(condition).toBe(name === 'lint'
        ? "needs.cheap_format.outputs.runtime_config_loopback_only != 'true' && needs.cheap_format.outputs.app_native_compile_only != 'true' && needs.cheap_format.outputs.native_required != 'false'"
        : "needs.cheap_format.outputs.runtime_config_loopback_only != 'true' && needs.cheap_format.outputs.native_required != 'false'");
      for (const value of ['', 'true', 'unknown', 'false'])
        expect(evaluateActions(condition, CI_ENTRIES[0], value)).toBe(value !== 'false');
      expect(block).toMatch(/^ {4}runs-on: \$\{\{ matrix\.os \}\}$/m);
    }
    for (const [name, block] of Object.entries(blocks)) {
      expect(block, `${name} job 挂了 continue-on-error —— 红了也不拦合入`).not.toMatch(/^ {4}continue-on-error:/m);
    }
    expect(cross).toMatch(/^ {4}runs-on: ubuntu-22\.04$/m);

    // The cheap formatter is a required predecessor, never a best-effort probe.
    // It runs for every normal gate event, including non-Linux dispatches.
    // Explicit source-only dispatch has its own failing admission job. Otherwise,
    // launcher diagnosis and conflicting SYSTEM requests retain the original routing.
    const requireCheapFormat = (block: string) => {
      expect(block).not.toMatch(/^ {4}(?:needs|continue-on-error):/m);
      const conditions = [...block.matchAll(/^ {4}if: \$\{\{ (.*) \}\}$/gm)];
      expect(block.match(/^ {4}if:/gm) ?? []).toHaveLength(1);
      expect(conditions).toHaveLength(1);
      expect(conditions[0][1]).toBe("((contains(toJSON(inputs), '\"runtime_config_loopback_input\"') && inputs.runtime_config_loopback_input != 'diagnostic-source') || (github.event_name == 'workflow_dispatch' && ((inputs.runtime_config_loopback_candidate || '') != '' || (inputs.app_native_compile_candidate || '') != ''))) || (github.event_name != 'workflow_dispatch' || inputs.runtime_config_source_only != true) && (github.event_name != 'workflow_dispatch' || inputs.windows_launcher_diagnostics != true || (inputs.windows_system_candidate || '') != '')");
      for (const event of ['pull_request', 'push', 'merge_group', 'workflow_call', 'workflow_dispatch']) {
        for (const sourceOnly of [undefined, false, true]) {
          for (const selected of [undefined, false, true]) {
            for (const candidate of [undefined, '', 'a'.repeat(40), 'malformed']) {
              expect(runInNewContext(runtimeModeExpression(conditions[0][1]), {
                ...runtimeModeFunctions,
                github: { event_name: event }, inputs: {
                  runtime_config_source_only: sourceOnly,
                  windows_launcher_diagnostics: selected, windows_system_candidate: candidate,
                },
              })).toBe((event !== 'workflow_dispatch' || sourceOnly !== true)
                && (event !== 'workflow_dispatch' || selected !== true || !!candidate));
            }
          }
        }
      }
      // Non-default raw input must reach failing admission even when a conflicting
      // source-only/launcher request would otherwise omit the formatter ancestor.
      for (const event of ['pull_request', 'push', 'workflow_call', 'workflow_dispatch'])
        for (const mode of [undefined, 'diagnostic-source', 'DIAGNOSTIC-SOURCE', 'formal-release', 'FORMAL-RELEASE', '', 'unknown']) {
          const rawRequest = mode !== undefined && mode.toLowerCase() !== 'diagnostic-source';
          expect(runInNewContext(runtimeModeExpression(conditions[0][1]), {
            ...runtimeModeFunctions, github: { event_name: event }, inputs: {
              ...(mode === undefined ? {} : { runtime_config_loopback_input: mode }),
              runtime_config_source_only: true, windows_launcher_diagnostics: true,
            },
          })).toBe(rawRequest || event !== 'workflow_dispatch');
        }
      expect(block).not.toMatch(/^ {8}continue-on-error:/m);
      const stepConditions = [...block.matchAll(/^ {8}if: (.+)$/gm)];
      expect(stepConditions).toHaveLength(1);
      expect(stepConditions[0][1]).toBe("steps.native_impact.outputs.native_required != 'false'");
      for (const value of ['', 'true', 'unknown', 'false'])
        expect(runInNewContext(stepConditions[0][1], { steps: { native_impact: { outputs: { native_required: value } } } }))
          .toBe(value !== 'false');
      expect(block).toMatch(/^ {4}runs-on: ubuntu-22\.04$/m);
      expect(block).toMatch(/^ {8}run: cargo fmt --all -- --check$/m);
      expect(block).not.toMatch(/Swatinem\/rust-cache@|actions\/cache@|cargo (?:build|test|clippy)\b/);
    };
    // Native formatting shares the same explicit test-only exemption as the
    // expensive DAG; all other events and unknown outputs keep it mandatory.
    const formatStep = executable(stepBlock(cheapFormat, 'Check workspace formatting before native compilation'));
    const formatGate = cheapFormat.slice(0, cheapFormat.indexOf('    steps:')) + '\n' + formatStep;
    requireCheapFormat(formatGate);
    expect(cheapFormat).not.toMatch(/Swatinem\/rust-cache@|actions\/cache@/);
    expect(cheapFormat).toContain('native_required: ${{ steps.native_impact.outputs.native_required }}');
    expect(stepBlock(cheapFormat, 'Classify native CI impact')).toContain('run: node scripts/classify-ci-impact.mjs --native-ci');
    expect(cheapFormat).toContain('fetch-depth: 2');
    expect(cheapFormat.indexOf('- name: Classify native CI impact')).toBeLessThan(cheapFormat.indexOf('- name: Install existing Rust formatting toolchain'));
    const toolchain = stepBlock(cheapFormat, 'Install existing Rust formatting toolchain');
    expect(toolchain).toContain("if: steps.native_impact.outputs.native_required != 'false'");

    for (const block of [lint, cross, test]) expect(block).toMatch(/^ {4}needs: cheap_format$/m);
    for (const mutant of [
      formatGate.replace('    runs-on:', '    if: false\n    runs-on:'),
      formatGate.replace('        run:', '        continue-on-error: true\n        run:'),
      formatGate.replace('cargo fmt --all -- --check', 'cargo fmt --all -- --check || true'),
      formatGate.replace("steps.native_impact.outputs.native_required != 'false'", 'false'),
      formatGate.replace("steps.native_impact.outputs.native_required != 'false'", "steps.native_impact.outputs.native_required == 'true'"),
    ]) expect(() => requireCheapFormat(mutant)).toThrow();


    // ③ cross 的 job 级 if 逐入口求值：它跑 ⇔ 同一入口下矩阵里有 ubuntu 腿（与拆分前逐格等价）。
    const crossIf = foldedJobIf(cross, 'cross');
    const matrix = matrixOsExpression(lint, 'lint');
    let ran = 0;
    let skipped = 0;
    for (const entry of CI_ENTRIES) {
      const hasUbuntuLeg = (evaluateActions(matrix, entry) as string[]).includes('ubuntu-22.04');
      const runs = evaluateActions(crossIf, entry);
      expect(typeof runs, `${entry.label}: cross 的 if 没有求出布尔值`).toBe('boolean');
      expect(runs, `${entry.label}: cross job 是否运行，必须等于该入口下是否存在 ubuntu 腿`).toBe(hasUbuntuLeg);
      expect(evaluateActions(crossIf, entry, 'false')).toBe(false);
      for (const value of ['', 'true', 'unknown'])
        expect(evaluateActions(crossIf, entry, value)).toBe(hasUbuntuLeg);
      if (runs) ran += 1; else skipped += 1;
    }
    // 反向对照：两种结论都真的出现过（否则「相等」可以被两边同时恒真 / 恒假满足）。
    expect(ran).toBe(7);
    expect(skipped).toBe(2);
    // 求值器自身的牙：把条件改成常量假，必须被上面那套逐入口比对判红。
    expect(evaluateActions('!(true)', CI_ENTRIES[0])).toBe(false);

    // ④ 豁免反腐烂步的 PR 排除原样保留（去掉的只是在 ubuntu 专属 job 里恒真的 runner.os 那一半）。
    expect(cross).toContain(
      "- name: Cross-target exemptions must still be necessary\n        if: github.event_name != 'pull_request'\n",
    );
  });

  it('RuntimeConfig isolated source and real-loopback dispatches preserve the original default job decisions', () => {
    const ci = workflow('ci.yml');
    const jobs = [...ci.slice(ci.indexOf('\njobs:\n')).matchAll(/^ {2}([a-z][a-z0-9_-]*):$/gm)].map((m) => m[1]);
    const source = executable(jobBlock(ci, 'runtime_config_source_tests'));
    const defaults = { runtime_config_loopback_candidate: '', app_native_compile_candidate: '', runtime_config_source_only: false, windows_launcher_diagnostics: false,
      native_dns_candidate: '', windows_system_candidate: '', windows_directory_mode: 'formal',
      windows_directory_candidate_source: '', os: 'all', preserve_run: false };
    type Selection = Partial<typeof defaults> & { runtime_config_loopback_input?: string };
    const conflicts: Selection[] = [{ windows_launcher_diagnostics: true }, { native_dns_candidate: 'a'.repeat(40) },
      { windows_system_candidate: 'a'.repeat(40) }, { windows_directory_mode: 'candidate' },
      { windows_directory_candidate_source: 'a'.repeat(40) }, { os: 'windows-2022' }, { os: 'macos-14' },
      { windows_launcher_diagnostics: true, windows_system_candidate: 'a'.repeat(40) }];
    // Compile each actual expression once; exhaustive truth tables should not
    // create thousands of VM contexts or depend on a relaxed test timeout.
    const conditions = Object.fromEntries(jobs.map((name) => {
      const block = executable(jobBlock(ci, name));
      const line = /^ {4}if: (?!>-)(.+)$/m.exec(block)?.[1].replace(/^\$\{\{ | \}\}$/g, '');
      const expression = line ?? foldedJobIf(block, name).replace(/^\$\{\{ | \}\}$/g, '');
      return [name, { needs: /^ {4}needs: (\w+)$/m.exec(block)?.[1],
        allows: runInNewContext(`(inputs, github, needs) => (${runtimeModeExpression(expression)})`, runtimeModeFunctions) }];
    }));
    const decision = (name: string, event: string, selection: Selection, nativeRequired = 'true', formal = 'true', loopbackOutput = '') => {
      const inputs = { ...defaults, ...selection };
      return Boolean(conditions[name].allows(inputs,
        { event_name: event, repository: 'polaris-arch/Polaris',
          ref: 'refs/heads/collab/dots-audit-remediation-20261017', workflow_ref: 'ordinary CI',
          event: { inputs: { ...inputs, runtime_config_source_only: 'true' } } },
        { cheap_format: { outputs: { native_required: nativeRequired, native_dns_formal: formal,
          native_dns_candidate: '', native_dns_formal_app: 'a'.repeat(40), runtime_config_loopback_only: loopbackOutput } } },
      ));
    };
    const scheduled = (event: string, selection: Selection, nativeRequired = 'true', formal = 'true', loopbackOutput = '', cheapResult = 'success') => {
      const started = new Set<string>();
      for (const name of jobs) {
        const needs = conditions[name].needs;
        if ((!needs || (started.has(needs) && cheapResult === 'success'))
          && decision(name, event, selection, nativeRequired, formal, loopbackOutput)) started.add(name);
      }
      return [...started];
    };
    for (const name of ['native_dns_candidate_gates', 'lint', 'cross', 'test', 'runtime_config_loopback']) {
      const header = executable(jobBlock(ci, name)).split('    steps:')[0];
      expect(header).toMatch(/^ {4}needs: cheap_format$/m);
      expect(header).not.toMatch(/always\(|cancelled\(|success\(|failure\(|continue-on-error/);
    }
    const compileOnly = { ...defaults, app_native_compile_candidate: 'a'.repeat(40), os: 'ubuntu-22.04' };
    for (const selection of [compileOnly, { ...compileOnly, runtime_config_source_only: true },
      { ...compileOnly, windows_launcher_diagnostics: true }]) {
      expect(decision('cheap_format', 'workflow_dispatch', selection)).toBe(true);
      expect(decision('runtime_config_source_tests', 'workflow_dispatch', selection)).toBe(false);
      expect(decision('windows_launcher_diagnostics', 'workflow_dispatch', selection)).toBe(false);
    }
    const loopback = executable(jobBlock(ci, 'runtime_config_loopback'));
    const loopbackOnly = { ...defaults, runtime_config_loopback_candidate: 'a'.repeat(40), os: 'ubuntu-22.04', preserve_run: true };
    expect(scheduled('workflow_dispatch', loopbackOnly, 'true', 'true', 'true'))
      .toEqual(['cheap_format', 'runtime_config_loopback']);
    for (const output of ['', 'false', 'unknown', 'TRUE']) {
      expect(scheduled('workflow_dispatch', loopbackOnly, 'true', 'true', output))
        .toEqual(['cheap_format', 'native_dns_candidate_gates', 'lint', 'cross', 'test']);
      expect(decision('runtime_config_loopback', 'workflow_dispatch', loopbackOnly, 'true', 'true', output)).toBe(false);
    }
    for (const selection of [{}, ...conflicts, { runtime_config_source_only: true },
      { app_native_compile_candidate: 'a'.repeat(40) }, { runtime_config_loopback_candidate: 'malformed' },
      { runtime_config_loopback_candidate: ' ' },
      { app_native_compile_candidate: 'a'.repeat(40), runtime_config_source_only: true, windows_launcher_diagnostics: true }]) {
      const selected = { ...loopbackOnly, ...selection };
      expect(decision('cheap_format', 'workflow_dispatch', selected)).toBe(true);
      expect(decision('runtime_config_source_tests', 'workflow_dispatch', selected)).toBe(false);
      expect(decision('windows_launcher_diagnostics', 'workflow_dispatch', selected)).toBe(false);
      for (const outcome of ['failure', 'cancelled', 'skipped'])
        expect(scheduled('workflow_dispatch', selected, 'true', 'true', 'true', outcome)).toEqual(['cheap_format']);
    }
    for (const mode of ['formal-release', 'FORMAL-RELEASE', '', 'unknown']) {
      const selected = { runtime_config_loopback_input: mode, runtime_config_source_only: true, windows_launcher_diagnostics: true };
      expect(decision('cheap_format', 'workflow_dispatch', selected)).toBe(true);
      expect(decision('runtime_config_source_tests', 'workflow_dispatch', selected)).toBe(false);
      expect(decision('windows_launcher_diagnostics', 'workflow_dispatch', selected)).toBe(false);
      for (const outcome of ['failure', 'cancelled', 'skipped'])
        expect(scheduled('workflow_dispatch', selected, 'true', 'true', 'true', outcome)).toEqual(['cheap_format']);
    }
    expect(loopback).toContain("if: needs.cheap_format.outputs.runtime_config_loopback_only == 'true'");
    expect(loopback).toContain('runs-on: ubuntu-22.04');
    expect(loopback).toContain('permissions:\n      contents: read');
    expect(loopback).not.toMatch(/strategy:|Swatinem\/rust-cache@|actions\/cache@|fetch-core|prepare-core-identity|continue-on-error|contents: write|actions: write|id-token:|secrets:/);
    const loopbackAdmission = stepBlock(loopback, 'Admit execution runner for real HOT loopback');
    expect(loopbackAdmission).toContain('RUNTIME_CONFIG_LOOPBACK_INPUTS: ${{ toJSON(inputs) }}');
    expect(loopbackAdmission).toContain('run: node scripts/runtime-config-loopback.mjs admit');
    expect(loopback.indexOf(loopbackAdmission)).toBeLessThan(loopback.indexOf('      - name: Install Rust toolchain'));
    expect(stepBlock(loopback, 'Run exact real App to paired kernel HOT loopback tests'))
      .toContain('run: node scripts/runtime-config-loopback.mjs run');
    const dispatch = ci.split('\n  workflow_dispatch:\n')[1].split('\n  workflow_call:\n')[0];
    expect([...dispatch.matchAll(/^ {6}([a-z][a-z0-9_]*):$/gm)].map((match) => match[1])).toEqual([
      'runtime_config_loopback_input',
      'runtime_config_loopback_candidate', 'app_native_compile_candidate', 'runtime_config_source_only',
      'native_dns_candidate', 'windows_system_candidate', 'windows_launcher_diagnostics', 'windows_directory_mode',
      'windows_directory_candidate_source', 'preserve_run', 'os',
    ]);
    expect(dispatch).toMatch(/runtime_config_loopback_input:\n {8}description:.*\n {8}type: choice\n {8}default: diagnostic-source\n {8}options: \[diagnostic-source, formal-release\]/);
    expect(dispatch).toMatch(/runtime_config_loopback_candidate:\n {8}description:.*\n {8}type: string\n {8}default: ''/);
    expect(ci.split('\n  workflow_call:\n')[1].split('\njobs:\n')[0]).not.toContain('runtime_config_loopback_candidate');
    expect(source).not.toMatch(/^ {4}needs:|continue-on-error|fetch-core|sudo/m);
    expect(source).toContain('runs-on: ubuntu-22.04');
    expect(source).toContain('RuntimeConfig source only (not full CI/native/HOT acceptance)');
    expect(source).toContain('This job is not full CI, native or HOT acceptance.');
    expect(source).toContain('run: node scripts/fetch-protoc.mjs');
    expect(source.match(/^ {8}run: cargo .+$/gm)).toEqual(['        run: cargo test --locked -p polaris-singbox-grpc --test runtime_config']);
    const input = ci.split('  workflow_dispatch:\n')[1].split('  workflow_call:')[0];
    expect(input).toMatch(/runtime_config_source_only:\n {8}description: .+\n {8}type: boolean\n {8}default: false/);
    expect(ci.split('  workflow_call:\n')[1].split('\nconcurrency:')[0]).not.toContain('runtime_config_source_only');
    const admission = stepBlock(source, 'Admit isolated RuntimeConfig source-only selection');
    expect(admission).toContain("if: github.event_name == 'workflow_dispatch' && inputs.runtime_config_source_only == true");
    expect(source.indexOf(admission)).toBeLessThan(source.indexOf('      - name: Install Rust toolchain'));
    const script = /<<'NODE'\n([\s\S]*?)\n {10}NODE/.exec(admission)![1].replace(/^ {10}/gm, '');
    const admit = (selection: Selection) => runInNewContext(script, { process: { env: {
      GITHUB_EVENT_NAME: 'workflow_dispatch', RUNTIME_CONFIG_SOURCE_INPUTS: JSON.stringify({ ...defaults,
        runtime_config_source_only: true, ...selection }),
    } } });
    for (const selection of [{}, { os: 'ubuntu-22.04' }, ...conflicts])
      expect(scheduled('workflow_dispatch', { ...selection, runtime_config_source_only: true })).toEqual(['runtime_config_source_tests']);
    expect(() => admit({})).not.toThrow();
    expect(() => admit({ os: 'ubuntu-22.04' })).not.toThrow();
    expect(() => admit({ runtime_config_loopback_input: 'diagnostic-source' })).not.toThrow();
    for (const mode of ['formal-release', 'FORMAL-RELEASE', 'DIAGNOSTIC-SOURCE', '', 'unknown'])
      expect(() => admit({ runtime_config_loopback_input: mode })).toThrow(/cannot combine/);
    for (const selection of [...conflicts, { app_native_compile_candidate: 'a'.repeat(40) }, { runtime_config_loopback_candidate: 'a'.repeat(40) }]) expect(() => admit(selection)).toThrow(/cannot combine/);
    for (const event of ['push', 'pull_request', 'merge_group', 'workflow_call', 'workflow_dispatch'])
      for (const sourceOnly of [undefined, false, ...(event === 'workflow_dispatch' ? [] : [true])])
        for (const diagnostic of [false, true]) for (const system of ['', 'a'.repeat(40)])
          for (const os of ['', 'all', 'ubuntu-22.04', 'windows-2022', 'macos-14'])
            for (const nativeRequired of ['', 'true', 'false']) for (const formal of ['', 'true']) {
              const ordinary = event !== 'workflow_dispatch' || !diagnostic || system !== '';
              const native = ordinary && nativeRequired !== 'false';
              const expected = [ordinary && 'cheap_format', native && formal === 'true' && 'native_dns_candidate_gates',
                native && 'lint', native && (event !== 'workflow_dispatch' || ['', 'all', 'ubuntu-22.04'].includes(os)) && 'cross',
                native && 'test', ordinary && 'runtime_config_source_tests', !ordinary && 'windows_launcher_diagnostics'].filter(Boolean);
              expect(scheduled(event, { runtime_config_source_only: sourceOnly, windows_launcher_diagnostics: diagnostic,
                windows_system_candidate: system, os }, nativeRequired, formal)).toEqual(expected);
            }
  });

  it('Rust 缓存键：每个 job 恰一个 rust-cache，键各自独立且是稳定字面量', () => {
    // 三个 job 的 target/ 内容互不通用（clippy metadata / 交叉 check 单元 / dev 全量 codegen）。
    // 共用键 = 谁最后跑完谁覆盖，另外两个下次恢复到的是别人的产物 —— 等于次次冷编。
    const keysOf = (block: string) =>
      [...block.matchAll(/uses: Swatinem\/rust-cache@v2\n {8}with:\n(?: {10}.*\n)*? {10}key: (.+)\n/g)].map((m) => m[1]);
    const ci = workflow('ci.yml');
    expect(executable(jobBlock(ci, 'runtime_config_loopback'))).not.toMatch(/Swatinem\/rust-cache@|actions\/cache@/);
    const ciKeys = ['lint', 'cross', 'test', 'runtime_config_source_tests'].map((name) => {
      const block = executable(jobBlock(ci, name));
      expect(block.match(/uses: Swatinem\/rust-cache@v2/g) ?? [], `${name} job 的 rust-cache 步数`).toHaveLength(1);
      const keys = keysOf(block);
      expect(keys, `${name} job 的 rust-cache 没有显式 key`).toHaveLength(1);
      return keys[0];
    });
    // 逐字钉住：键是字面量（不含 run_id / sha 之类每次都变的东西 ⇒ 稳定），且四者互不相同。
    expect(ciKeys).toEqual(['lint', 'cross', 'test', 'runtime-config-source']);
    expect(ci.match(/uses: Swatinem\/rust-cache@v2/g) ?? [], 'ci.yml 有不属于这四个 job 的 rust-cache').toHaveLength(4);

    const pkg = workflow('package.yml');
    const pkgKeys = keysOf(executable(jobBlock(pkg, 'package')));
    const wireKeys = keysOf(executable(jobBlock(pkg, 'core_wire')));
    // 发行构型的键仍是裸 label（与 2026-10-07 之前逐字相同）；轻量构型只多一个后缀。
    expect(pkgKeys).toEqual(["${{ matrix.label }}${{ env.POLARIS_LIGHT_BUILD == '1' && '-light' || '' }}"]);
    expect(wireKeys).toEqual(['core-wire']);
    expect(executable(pkg).match(/uses: Swatinem\/rust-cache@v2/g) ?? []).toHaveLength(2);
    const suffix = (light: string) =>
      runInNewContext("env.POLARIS_LIGHT_BUILD == '1' && '-light' || ''", { env: { POLARIS_LIGHT_BUILD: light } });
    expect(suffix('0')).toBe('');
    expect(suffix('1')).toBe('-light');

    // Android 两个 job 各恰一个 rust-cache，键同形（2026-10-08 起纳入本门；此前 android.yml 的键无门）：
    // 发行构型仍是改动前的 `android-aarch64`，轻量构型只多同一个后缀 —— 后缀表达式与桌面逐字相同。
    const android = workflow('android.yml');
    for (const name of ['release_check', 'release-apk']) {
      const block = executable(jobBlock(android, name));
      expect(block.match(/uses: Swatinem\/rust-cache@v2/g) ?? [], `${name} job 的 rust-cache 步数`).toHaveLength(1);
      expect(keysOf(block), `${name} job 的 rust-cache 键`)
        .toEqual(["android-aarch64${{ env.POLARIS_LIGHT_BUILD == '1' && '-light' || '' }}"]);
    }
    expect(executable(android).match(/uses: Swatinem\/rust-cache@v2/g) ?? []).toHaveLength(2);
    const androidKey = (light: string) =>
      'android-aarch64' + runInNewContext("env.POLARIS_LIGHT_BUILD == '1' && '-light' || ''", { env: { POLARIS_LIGHT_BUILD: light } });
    expect(androidKey('0')).toBe('android-aarch64');
    expect(androidKey('1')).toBe('android-aarch64-light');
  });

  it('package 的 dispatch 输入确实只有 platform，并复用 ci.yml 作全平台门', () => {
    const pkg = workflow('package.yml');
    const dispatch = pkg.slice(pkg.indexOf('workflow_dispatch:'), pkg.indexOf('# 最小权限'));
    expect(dispatch).toContain('platform:');
    expect(dispatch).not.toMatch(/^\s+os:/m);
    expect(pkg).toContain('uses: ./.github/workflows/ci.yml');
    expect(pkg).toContain("needs.setup.outputs.full == 'true'");
    expect(pkg).toContain('inputs.skip_quality_gates != true');
  });

  it('独立门与 Package 复用门必须按调用方隔离并发组，不能互相取消制造假红', () => {
    for (const name of ['ci.yml', 'ui.yml']) {
      const source = workflow(name);
      const direct = workflowConcurrency(source, name);
      const called = workflowConcurrency(source, 'Package');
      expect(direct.cancels).toBe(false);
      expect(called.cancels).toBe(false);
      expect(direct.group).not.toBe(called.group);
      expect(called.group).not.toBe(workflowConcurrency(source, 'Package', undefined, undefined, false, 2).group);
      const fixed = workflowConcurrency(source, 'Package', undefined, undefined, true, 1);
      expect(fixed.cancels).toBe(false);
      expect(fixed.group).toBe(called.group);
      expect(fixed.group).not.toBe(workflowConcurrency(source, 'Package', undefined, undefined, true, 2).group);
      for (const event of ['push', 'pull_request']) {
        const automatic = workflowConcurrency(source, 'Package', event);
        expect(automatic.cancels).toBe(true);
        expect(automatic.group).toBe(workflowConcurrency(source, 'Package', event, undefined, false, 2).group);
        expect(automatic.group).not.toBe(workflowConcurrency(source, name, event).group);
      }
    }
  });

  it('切片器本身有效：两个 job 都切得出非平凡的块', () => {
    // 正面自检。少了这条，jobBlock 若因缩进/命名变化退化成空串，下面两条全部真空通过。
    const pkg = workflow('package.yml');
    for (const name of ['package', 'release_desktop', 'android_release', 'release']) {
      const block = jobBlock(pkg, name);
      expect(block.length, `job '${name}' 的切片过短，取材面可疑`).toBeGreaterThan(200);
      expect(block, `job '${name}' 的切片没包含它自己的 needs`).toContain('needs:');
      // 切片不得越界到下一个 job。
      expect(block).not.toMatch(
        /\n {2}(?!$)(?:package|release_desktop|android_release|release):\n(?![\s\S]*^$)/m,
      );
    }
  });

  it('发布腿只接受成功或按设计跳过的质量门，取消态不得被当成可放行', () => {
    const pkg = workflow('package.yml');
    const release = executable(jobBlock(pkg, 'release_desktop'));

    // 剥注释自检（正面断言）：剥完必须还剩下 if 与 needs，否则下面全是真空通过。
    expect(release, '剥注释后 release 块空了 —— 取材面塌了').toMatch(/^\s+if:/m);
    expect(release).toMatch(/^\s+needs:/m);
    expect(release, '剥注释没生效 —— 块里仍有整行注释').not.toMatch(/^\s*#/m);

    // 判据挂在第一次 release mutation 上（桌面草稿 job）；门未过时连草稿都不写。
    // 2026-10-07：needs 多了 core_wire —— wire 契约从打包腿内的一步变成并行 job，
    // 「契约不成立就没有安装包」变成「契约不成立就写不进草稿」，判据必须在这里接回（见下）。
    expect(release).toContain("needs: [setup, ci, ui, package, core_wire]");
    expect(release).toContain(
      "needs.ci.result == 'success' || needs.ci.result == 'skipped'",
    );
    expect(release).toContain(
      "needs.ui.result == 'success' || needs.ui.result == 'skipped'",
    );

    // always() 关掉了「needs 全绿才跑」的默认语义，故这条必须被显式写回，否则打包腿挂了也照发。
    expect(release).toContain('always()');
    expect(
      release,
      'release 的 if 里加了 always() 却没显式要求 package 成功 —— 打包腿全挂也会发布',
    ).toContain("needs.package.result == 'success'");
    // wire job 同理，且只收 success：它没有「按设计跳过」的形态（凡能走到发布的入口它都必跑）。
    expect(
      release,
      'release 的 if 没有显式要求 core_wire 成功 —— wire 契约不成立也会发布',
    ).toContain("needs.core_wire.result == 'success'");
    expect(release).not.toContain("needs.core_wire.result == 'skipped'");
    expect(pkg).not.toContain("needs.core_wire.result != 'failure'");

    // cancelled 也满足「不等于 failure」：全文范围内都不许出现这种放行式判据。
    expect(pkg).not.toContain("needs.ci.result != 'failure'");
    expect(pkg).not.toContain("needs.ui.result != 'failure'");
    expect(pkg).not.toContain("needs.package.result != 'failure'");
  });

  it('打包腿与质量门并行：package 不得把 ci/ui 写进 needs（写了就退回串行）', () => {
    const pkg = workflow('package.yml');
    const pkgJob = executable(jobBlock(pkg, 'package'));
    expect(pkgJob, '剥注释后 package 块空了 —— 取材面塌了').toMatch(/^\s+if:/m);
    expect(pkgJob).toContain('needs: [setup, desktop_core]');
    const coreJob = executable(jobBlock(pkg, 'desktop_core'));
    expect(coreJob).toContain('needs: setup');
    expect(coreJob).toContain('uses: ./.github/workflows/desktop-core.yml');
    expect(coreJob).toContain('candidate: ${{ needs.setup.outputs.candidate }}');
    expect(pkgJob).toContain("needs.setup.result == 'success'");
    expect(pkgJob).toContain("needs.desktop_core.result == 'success'");
    expect(pkgJob).toContain("needs.desktop_core.result == 'skipped' && inputs.core_bundle_artifact != ''");
    // 状态函数必须是 `!cancelled()`：`always()` 恒真，run 被取消时在途打包腿不会停，
    // 旧提交的四条腿会把并发组占到跑完（Release Risk run 37638779024 实证，见 package.yml 注释）。
    // 完整真值表（含取消态）在 scripts/desktop-core-ci-wiring.test.mjs 里按表达式求值。
    for (const [name, block] of [['package', pkgJob], ['core_wire', executable(jobBlock(pkg, 'core_wire'))]] as const) {
      const condition = foldedJobIf(block, name);
      expect(condition.startsWith('!cancelled() && '), `${name} 的 if 不以 !cancelled() 起头：${condition}`).toBe(true);
      expect(condition, `${name} 的 if 里出现了 always() —— 它对取消免疫`).not.toContain('always()');
    }
    // needs 即等待。package 一旦能读到 needs.ci/needs.ui，就说明它在等两门，并行拓扑已被推翻。
    expect(
      pkgJob,
      'package job 引用了 needs.ci / needs.ui —— 它又串回门后面了',
    ).not.toMatch(/needs\.(ci|ui)\./);
  });

  it('复用 bundle 的真实 setup 前置只接受同 candidate、run 和 attempt', () => {
    const resolve = stepBlock(jobBlock(workflow('package.yml'), 'setup'), 'Resolve platform matrix');
    expect(resolve).toContain('EXPECTED_CANDIDATE: ${{ github.event.pull_request.head.sha || github.sha }}');
    expect(resolve).toContain('CORE_CANDIDATE: ${{ inputs.core_candidate || github.event.pull_request.head.sha || github.sha }}');
    expect(resolve).toContain('CORE_RUN_ID: ${{ github.run_id }}');
    expect(resolve).toContain('CORE_RUN_ATTEMPT: ${{ github.run_attempt }}');
    expect(resolve).toContain("SYSTEM_CANDIDATE: ${{ inputs.windows_system_candidate || '' }}");
    expect(resolve).toContain("POLARIS_NATIVE_PAYLOAD_VALIDATION: ${{ inputs.native_validation && '1' || '0' }}");
    const runAt = resolve.indexOf('\n        run: |\n');
    const matrixAt = resolve.indexOf('\n          all=', runAt);
    expect(runAt).toBeGreaterThan(0);
    expect(matrixAt).toBeGreaterThan(runAt);
    // Only the actual identity prelude runs: no matrix tools, producers or workflow jobs.
    const script = resolve.slice(runAt + '\n        run: |\n'.length, matrixAt)
      .split('\n').map((line) => line.slice(10)).join('\n');
    const candidate = 'a'.repeat(40);
    const base = {
      CORE_CANDIDATE: candidate, EXPECTED_CANDIDATE: candidate,
      CORE_RUN_ID: '12', CORE_RUN_ATTEMPT: '3',
      CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-12-3`,
      POLARIS_NATIVE_PAYLOAD_VALIDATION: '0', SYSTEM_CANDIDATE: '',
    };
    const cases: [string, Record<string, string>, number][] = [
      ['exact reuse', {}, 0], ['standalone producer', { CORE_BUNDLE_ARTIFACT: '' }, 0],
      ['different candidate', { CORE_CANDIDATE: 'b'.repeat(40) }, 1],
      ['malformed candidate', { CORE_CANDIDATE: 'not-a-commit' }, 1],
      ['previous run', { CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-11-3` }, 1],
      ['previous attempt', { CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-12-2` }, 1],
    ];
    for (const [name, changed, status] of cases) {
      const result = spawnSync('bash', ['-c', script], { env: { ...process.env, ...base, ...changed }, encoding: 'utf8' });
      expect(result.status, `${name}: ${result.stderr}`).toBe(status);
    }
    const system = {
      ...base, SYSTEM_CANDIDATE: candidate, GITHUB_SHA: candidate,
      GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REPOSITORY: 'polaris-arch/Polaris',
      GITHUB_REF: 'refs/heads/ci/governance', PLATFORM: 'all', PLATFORMS_JSON: '',
      VALIDATION_SKIP_QUALITY: 'false',
    };
    const systemCases: [string, Record<string, string>, number][] = [
      ['SYSTEM exact candidate', {}, 0], ['SYSTEM main', { GITHUB_REF: 'refs/heads/main' }, 0],
      ['SYSTEM wrong SHA', { SYSTEM_CANDIDATE: 'b'.repeat(40) }, 1],
      ['SYSTEM malformed SHA', { SYSTEM_CANDIDATE: 'not-a-commit' }, 1],
      ['SYSTEM push', { GITHUB_EVENT_NAME: 'push' }, 1],
      ['SYSTEM other repository', { GITHUB_REPOSITORY: 'other/repo' }, 1],
      ['SYSTEM other branch', { GITHUB_REF: 'refs/heads/other' }, 1],
      ['SYSTEM tag', { GITHUB_REF: 'refs/tags/v1.0.0' }, 1],
      ['SYSTEM narrowed platform', { PLATFORM: 'windows' }, 1],
      ['SYSTEM custom matrix', { PLATFORMS_JSON: '["windows"]' }, 1],
      ['SYSTEM skipped quality gates', { VALIDATION_SKIP_QUALITY: 'true' }, 1],
    ];
    for (const [name, changed, status] of systemCases) {
      const result = spawnSync('bash', ['-c', script], { env: { ...process.env, ...system, ...changed }, encoding: 'utf8' });
      expect(result.status, `${name}: ${result.stderr}`).toBe(status);
    }
    const ref = 'refs/heads/collab/fk02-native-payload-validation-20261010';
    const native = {
      ...base, POLARIS_NATIVE_PAYLOAD_VALIDATION: '1', GITHUB_ACTIONS: 'true',
      POLARIS_NO_KERNEL_RUN: '0', GITHUB_SHA: candidate, GITHUB_RUN_ID: '12', GITHUB_RUN_ATTEMPT: '3',
      GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REF: ref,
      GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/release-risk.yml@${ref}`,
      PLATFORMS_JSON: '["linux","windows"]', VALIDATION_KERNEL: 'true', VALIDATION_UPLOAD: 'true',
      VALIDATION_LIGHT: 'false', VALIDATION_SKIP_QUALITY: 'true',
    };
    const nativeCases: [string, Record<string, string>, number][] = [
      ['native exact reuse', {}, 0], ['native wrong event', { GITHUB_EVENT_NAME: 'push' }, 1],
      ['native outside Actions', { GITHUB_ACTIONS: 'false' }, 1],
      ['native wrong repository', { GITHUB_REPOSITORY: 'other/repo' }, 1],
      ['native tag', { GITHUB_REF: 'refs/tags/v1.0.0' }, 1],
      ['native wrong caller', { GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/package.yml@${ref}` }, 1],
      ['native extra platform', { PLATFORMS_JSON: '["linux","windows","macos-x64"]' }, 1],
      ['native no kernel gate', { VALIDATION_KERNEL: 'false' }, 1],
      ['native no original artifacts', { VALIDATION_UPLOAD: 'false' }, 1],
      ['native light build', { VALIDATION_LIGHT: 'true' }, 1],
      ['native duplicate quality gates', { VALIDATION_SKIP_QUALITY: 'false' }, 1],
      ['native no bundle', { CORE_BUNDLE_ARTIFACT: '' }, 1],
      ['native previous attempt', { CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-12-2` }, 1],
    ];
    // Replay the historical Risk contract against its own exact selection. The
    // current committed formal default must not inherit that historical exemption.
    const historicalRoot = mkdtempSync(join(tmpdir(), 'polaris-package-historical-risk-'));
    onTestFinished(() => rmSync(historicalRoot, { recursive: true, force: true }));
    for (const directory of ['desktop-core', 'lib'])
      cpSync(join(REPO_ROOT, 'scripts', directory), join(historicalRoot, 'scripts', directory), { recursive: true });
    mkdirSync(join(historicalRoot, 'src-tauri'));
    const manifest = JSON.parse(readFileSync(join(REPO_ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
    writeFileSync(join(historicalRoot, 'src-tauri/core-manifest.json'), JSON.stringify({ desktopRelease: manifest.historicalDesktopRelease }));
    cpSync(join(REPO_ROOT, manifest.historicalDesktopReleaseContractPath),
      join(historicalRoot, manifest.historicalDesktopRelease.contractPath));
    const formal = spawnSync('bash', ['-c', script], { cwd: REPO_ROOT, env: { ...process.env, ...native }, encoding: 'utf8' });
    expect(formal.status, formal.stderr).not.toBe(0);
    expect(formal.stderr).toContain('Exact committed formal App/caller/run context required');
    for (const [name, changed, status] of nativeCases) {
      const result = spawnSync('bash', ['-c', script], { cwd: historicalRoot, env: { ...process.env, ...native, ...changed }, encoding: 'utf8' });
      expect(result.status, `${name}: ${result.stderr}`).toBe(status);
    }
  });

  it('发布 DAG 必须是桌面草稿 → 签名 APK → 全量核验公开，且取消态一律不放行', () => {
    const pkg = workflow('package.yml');
    const desktopAt = pkg.indexOf('\n  release_desktop:\n');
    const androidAt = pkg.indexOf('\n  android_release:\n');
    const finalAt = pkg.indexOf('\n  release:\n');
    expect(desktopAt).toBeGreaterThan(0);
    expect(androidAt).toBeGreaterThan(desktopAt);
    expect(finalAt).toBeGreaterThan(androidAt);

    const desktop = executable(jobBlock(pkg, 'release_desktop'));
    const android = executable(jobBlock(pkg, 'android_release'));
    const release = executable(jobBlock(pkg, 'release'));

    expect(desktop).toContain('release_tag: ${{ steps.release_identity.outputs.tag }}');
    expect(desktop).toContain('release_sha: ${{ steps.release_identity.outputs.sha }}');
    expect(android).toContain('needs: release_desktop');
    expect(android).toContain('permissions:\n      contents: write');
    expect(android).toContain('uses: ./.github/workflows/android.yml');
    expect(android).toContain('publish_release: true');
    expect(android).toContain(
      'release_tag: ${{ needs.release_desktop.outputs.release_tag }}',
    );
    expect(android).toContain('secrets: inherit');

    expect(release).toContain('needs: [release_desktop, android_release]');
    expect(release).toContain("needs.release_desktop.result == 'success'");
    expect(release).toContain("needs.android_release.result == 'success'");
    expect(release).not.toContain("needs.android_release.result != 'failure'");
  });

  it('只有最终 job 能公开；已有公开 release 与 tag/commit 漂移都在写入前失败', () => {
    const pkg = workflow('package.yml');
    const desktop = executable(jobBlock(pkg, 'release_desktop'));
    const android = executable(jobBlock(pkg, 'android_release'));
    const release = executable(jobBlock(pkg, 'release'));

    expect(executable(pkg).match(/--draft=false/g) ?? []).toHaveLength(1);
    expect(desktop).not.toContain('--draft=false');
    expect(android).not.toContain('--draft=false');
    expect(release).toContain('gh release edit "$TAG" --repo "$GITHUB_REPOSITORY" --draft=false');
    expect(release.indexOf('Verify combined asset set and digests')).toBeLessThan(
      release.indexOf('Promote release to public'),
    );

    expect(desktop).toContain('tag_sha="$(git rev-list -n 1 "$tag")"');
    expect(desktop).toContain(
      'git fetch --force --depth=1 origin "+refs/tags/$tag:refs/tags/$tag"',
    );
    expect(desktop).toContain(
      'version="$(node -p \'require("./src-tauri/tauri.conf.json").version\')"',
    );
    expect(desktop).toContain('[ "$tag" != "v$version" ]');
    expect(desktop).toContain('[ "$tag_sha" != "$GITHUB_SHA" ]');
    expect(desktop).toContain('[ "$is_draft" != "true" ]');
    expect(desktop).not.toContain('[ "$target_commitish" != "$GITHUB_SHA" ]');
    expect(desktop).not.toContain('--target "$GITHUB_SHA"');
    expect(desktop).toContain('targetCommitish=$target_commitish（诊断信息');
    expect(desktop.match(/^\s*verify_remote_tag$/gm) ?? []).toHaveLength(3);
    expect(desktop.indexOf('[ "$is_draft" != "true" ]')).toBeLessThan(
      desktop.indexOf('gh release upload "$tag" "${files[@]}"'),
    );
    expect(desktop).toContain('已不再是同 tag 草稿，拒绝上传桌面资产');
    const desktopUploadAt = desktop.indexOf('gh release upload "$tag" "${files[@]}"');
    const lastDraftReadBeforeUpload = desktop.lastIndexOf(
      'gh release view "$tag"',
      desktopUploadAt,
    );
    expect(lastDraftReadBeforeUpload).toBeGreaterThan(0);
    expect(lastDraftReadBeforeUpload).toBeLessThan(desktopUploadAt);
    expect(release).toContain('EXPECTED_SHA: ${{ needs.release_desktop.outputs.release_sha }}');
    expect(release).toContain('[ "$tag_sha" != "$GITHUB_SHA" ]');
    expect(release.match(/git fetch --force/g) ?? []).toHaveLength(3);
    expect(release).toContain('[ "$remote_tag_sha" != "$GITHUB_SHA" ]');
    expect(release).toContain('.targetCommitish');
    expect(release).not.toContain('[ "$target_commitish" != "$GITHUB_SHA" ]');
    expect(release).not.toContain('targetCommitish <<<"$release_json")" != "$GITHUB_SHA"');
    expect(release).toContain('拒绝覆盖 SHA256SUMS');
  });

  it('Android 发布腿只写同 commit 草稿，并验真实 APK 签名与远端 digest', () => {
    const androidWorkflow = workflow('android.yml');
    const allExecutable = executable(androidWorkflow);
    const releaseCheck = executable(jobBlock(androidWorkflow, 'release_check'));
    const publish = executable(jobBlock(androidWorkflow, 'release-apk'));
    const dispatch = androidWorkflow.slice(
      androidWorkflow.indexOf('  workflow_dispatch:'),
      androidWorkflow.indexOf('\npermissions:'),
    );

    expect(publish).toContain('if: inputs.publish_release');
    expect(dispatch).not.toContain('publish_release:');
    expect(dispatch).not.toContain('release_tag:');
    expect(androidWorkflow).toContain(
      'value: ${{ jobs.release-apk.outputs.signed_apk_sha256 }}',
    );
    expect(
      [androidWorkflow, workflow('package.yml'), workflow('release-risk.yml')]
        .join('\n')
        .match(/publish_release:\s*true/g) ?? [],
    ).toHaveLength(1);
    const tagRun = workflowConcurrency(androidWorkflow, 'Package', 'push', 'refs/tags/v1.0.0');
    expect(tagRun.cancels).toBe(false);
    expect(tagRun.group).toBe(workflowConcurrency(androidWorkflow, 'Package', 'push', 'refs/tags/v1.0.0', false, 2).group);
    expect(tagRun.group).not.toBe(workflowConcurrency(androidWorkflow, 'Package', 'push', 'refs/tags/v2.0.0').group);
    expect(workflowConcurrency(androidWorkflow, 'Android').group).not.toBe(workflowConcurrency(androidWorkflow, 'Package').group);
    const packageSource = workflow('package.yml');
    const packageGroup = (selection: Parameters<typeof workflowConcurrency>[6] = {},
      caller = 'Package', ref = 'refs/heads/collab/contract', preserve = false, runId = 1,
      event = 'workflow_dispatch') => workflowConcurrency(packageSource, caller, event, ref, preserve, runId, selection);
    const tip = packageGroup();
    expect(tip.group).toBe('package-Package-refs/heads/collab/contract-all-fixed-1');
    expect(tip.cancels).toBe(false);
    expect(tip.group).not.toBe(packageGroup({}, 'Package', undefined, false, 2).group);
    expect(tip.group).not.toBe(packageGroup({}, 'Release Risk').group);
    expect(tip.group).not.toBe(packageGroup({}, 'Package', 'refs/heads/collab/other').group);
    expect(packageGroup({ platform: 'linux' }).group).not.toBe(packageGroup({ platform: 'windows' }).group);
    expect(packageGroup({ platforms: '["linux"]' }).group).not.toBe(packageGroup({ platforms: '["windows"]' }).group);
    expect(packageGroup({ platforms: '["linux"]', platform: 'windows' }).group).toBe(
      packageGroup({ platforms: '["linux"]' }).group);
    expect(packageGroup({ pullRequestNumber: 7 }, 'Release Risk', 'refs/pull/7/merge', false, 1, 'pull_request').group)
      .toBe('package-Release Risk-7-all');
    const fixed = packageGroup({}, 'Package', undefined, true);
    expect(fixed.cancels).toBe(false);
    expect(fixed.group).toBe(tip.group);
    expect(fixed.group).not.toBe(packageGroup({}, 'Package', undefined, true, 2).group);
    const inherited = packageGroup({ eventPreserveRun: true }, 'Release Risk');
    expect(inherited.cancels).toBe(false);
    expect(inherited.group).not.toBe(packageGroup({ eventPreserveRun: true }, 'Release Risk', undefined, false, 2).group);
    const packageTag = packageGroup({}, 'Package', 'refs/tags/v1.0.0', false, 1, 'push');
    expect(packageTag.cancels).toBe(false);
    expect(packageTag.group).toBe(packageGroup({}, 'Package', 'refs/tags/v1.0.0', false, 2, 'push').group);
    expect(packageTag.group).not.toBe(packageGroup({}, 'Package', 'refs/tags/v2.0.0', false, 1, 'push').group);
    const queue = packageGroup({ mergeGroupHead: 'queued-source' }, 'Package', 'refs/heads/queue', false, 1, 'merge_group');
    expect(queue.cancels).toBe(false);
    expect(queue.group).not.toBe(packageGroup({ mergeGroupHead: 'queued-source' }, 'Package', 'refs/heads/queue', false, 2, 'merge_group').group);
    expect(releaseCheck).toContain('if: inputs.publish_release != true');
    expect(androidWorkflow).not.toContain('\n  apk:\n');
    expect(allExecutable).not.toContain('--debug');
    expect(allExecutable).not.toContain('app-arm64-debug.apk');
    expect(allExecutable).not.toContain('assembleArm64Debug');

    expect(releaseCheck).toContain(
      ':app:assembleArm64Release -PpolarisAllowUnsigned=true',
    );
    expect(releaseCheck).toContain(
      'run: bash scripts/gate-android-release-behavior.sh',
    );
    expect(releaseCheck).toContain(
      'python3 scripts/mobile-core/ci_android.py verify --apk src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release-unsigned.apk --abi arm64-v8a --r8 src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    expect(releaseCheck).toContain(
      'node scripts/assert-r8-evidence.mjs src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    // Real workflow → formal input wrapper → final carrier → R8/APK checks.
    const verifier = readFileSync(join(REPO_ROOT, 'scripts/mobile-core/ci_android.py'), 'utf8');
    const verify = verifier.slice(verifier.indexOf('def verify('), verifier.indexOf('\ndef main('));
    expect(verify).toContain("require(r8 is not None, 'actual APK R8 output required')");
    expect(verify).toContain("ROOT / 'scripts/mobile-core/carriers.py', 'android'");
    expect(verify).toContain("'--artifact', apk, '--abi', abi, '--r8', r8");
    expect(verify).toContain("'--component', Path(paths['work']) / value['android']['outputName']");
    expect(verify).toContain("'--receipt', Path(paths['work']) / 'component-receipt.json'");
    expect(verify).toContain('subprocess.run([str(arg) for arg in args], check=True)');
    expect(verify).not.toMatch(/^\s*(?:try:|except\b)/m);
    const carrier = readFileSync(join(REPO_ROOT, 'scripts/mobile-core/carriers.py'), 'utf8');
    const consumption = carrier.slice(carrier.indexOf('def android('), carrier.indexOf('\ndef apple('));
    expect(consumption).toContain("verify_original(component, receipt_path, 'android', tools, scratch)");
    expect(consumption).toContain("run(['node', ROOT / 'scripts/assert-r8-evidence.mjs', r8], cwd=ROOT)");
    expect(consumption).toContain("run(['node', ROOT / 'scripts/verify-apk.mjs', apk, '--abi', target], cwd=ROOT)");
    expect(consumption).toContain("require(current == material['App'], 'App candidate changed after SDK materialization')");
    expect(consumption).not.toMatch(/^\s*(?:try:|except\b)/m);
    const producer = readFileSync(join(REPO_ROOT, 'scripts/mobile-core/producer.py'), 'utf8');
    const run = producer.slice(producer.indexOf('def run('), producer.indexOf('\ndef ', producer.indexOf('def run(') + 1));
    expect(run).toContain('require(process.returncode == 0,');
    // Parse the actual handler and binding; aliases/indentation do not decide
    // whether cleanup preserves and rethrows the primary exception.
    const primaryPolicy = (mutation = '') => {
      const result = spawnSync('python3', ['-c', `
import ast,json,sys
request=json.load(sys.stdin)
tree=ast.parse(request['source'])
function=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='run')
# Select the lifecycle try itself, not the first exception handler in run.
# Nested function bodies (e.g. drain), spawn and final log projection are
# separate failure domains and must not change the primary cleanup target.
def local_nodes(node):
 yield node
 for child in ast.iter_child_nodes(node):
  if not isinstance(child,(ast.FunctionDef,ast.AsyncFunctionDef,ast.ClassDef,ast.Lambda)):
   yield from local_nodes(child)
def lifecycle_tries():
 matches=[]
 for node in local_nodes(function):
  if not isinstance(node,ast.Try): continue
  calls=[s.value for s in node.body if isinstance(s,ast.Expr) and isinstance(s.value,ast.Call)]
  registered=any(isinstance(c.func,ast.Name) and c.func.id=='register_group'
                 and len(c.args)==1 and isinstance(c.args[0],ast.Attribute)
                 and c.args[0].attr=='pid' and isinstance(c.args[0].value,ast.Name)
                 and c.args[0].value.id=='process' and not c.keywords for c in calls)
  drained=any(isinstance(c.func,ast.Name) and c.func.id=='drain' for c in calls)
  if registered and drained: matches.append(node)
 return matches
candidates=lifecycle_tries()
if len(candidates)!=1:
 print(json.dumps({'valid':False}));sys.exit(0)
if request['mutation']=='missing-lifecycle':
 candidates[0].body=[s for s in candidates[0].body
                     if not (isinstance(s,ast.Expr) and isinstance(s.value,ast.Call)
                             and isinstance(s.value.func,ast.Name) and s.value.func.id=='drain')]
if request['mutation']=='duplicate-lifecycle':
 import copy
 function.body.append(copy.deepcopy(candidates[0]))
candidates=lifecycle_tries()
if len(candidates)!=1:
 print(json.dumps({'valid':False}));sys.exit(0)
handlers=[h for h in candidates[0].handlers if isinstance(h.type,ast.Name)
          and h.type.id=='BaseException' and h.name]
if len(handlers)!=1:
 print(json.dumps({'valid':False}));sys.exit(0)
handler=handlers[0]
notes=[n for n in ast.walk(handler) if isinstance(n,ast.Call)
       and isinstance(n.func,ast.Name) and n.func.id=='failure_note']
if request['mutation']=='silence': handler.body[-1]=ast.Pass()
if request['mutation']=='replace': handler.body[-1]=ast.Raise(exc=ast.Call(func=ast.Name(id='RuntimeError'),args=[],keywords=[]))
if request['mutation']=='wrong-note' and notes: notes[0].args[0]=ast.Name(id='secondary')
last=handler.body[-1]
bare=isinstance(last,ast.Raise) and last.exc is None and last.cause is None
replacement=any(isinstance(n,ast.Raise) and n.exc is not None for n in ast.walk(handler))
bound=len(notes)==1 and isinstance(notes[0].args[0],ast.Name) and notes[0].args[0].id==handler.name
print(json.dumps({'valid':bare and not replacement and bound,
                 'bareReraise':bare,'replacementRaise':replacement,'noteBoundToPrimary':bound}))
`], { input: JSON.stringify({ source: producer, mutation }), encoding: 'utf8' });
      expect(result.status, result.stderr).toBe(0);
      return JSON.parse(result.stdout);
    };
    expect(primaryPolicy()).toEqual({ valid: true, bareReraise: true,
      replacementRaise: false, noteBoundToPrimary: true });
    for (const mutation of ['silence', 'replace', 'wrong-note', 'missing-lifecycle', 'duplicate-lifecycle']) {
      expect(primaryPolicy(mutation).valid, mutation).toBe(false);
    }
    const cleanupControls = readFileSync(join(REPO_ROOT, 'scripts/mobile-core/test_apple_progress.py'), 'utf8');
    expect(cleanupControls).toContain('def test_cleanup_failures_do_not_replace_original_or_release_live_group(');
    expect(cleanupControls).toContain("self.assertRaisesRegex(OSError, 'PRIMARY_LOG_FAILURE')");
    expect(cleanupControls).toContain("self.assertFalse(any(call.kwargs.get('finished') for call in register.call_args_list))");
    expect(cleanupControls).toContain("self.assertIn('cleanup incomplete', ' '.join(raised.exception.__notes__))");
    expect(run).toContain('register_group(process.pid, finished=True)');
    expect(releaseCheck).toContain(
      'run: node scripts/verify-apk.mjs src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release-unsigned.apk --abi arm64-v8a',
    );
    expect(releaseCheck).toContain('name: android-release-mapping');
    expect(releaseCheck).not.toContain('gh release upload');
    expect(releaseCheck).not.toContain('actions/upload-artifact@v7\n        with:\n          name: android-apk');

    expect(publish).toContain(
      'git fetch --force --depth=1 origin "+refs/tags/$tag:refs/tags/$tag"',
    );
    expect(publish.match(/git fetch --force/g) ?? []).toHaveLength(2);
    expect(publish).toContain('tag_sha="$(git rev-list -n 1 "$tag")"');
    expect(publish).toContain('[ "$tag_sha" != "$GITHUB_SHA" ]');
    expect(publish).toContain('[ "$is_draft" != "true" ]');
    expect(publish).not.toContain('[ "$target_commitish" != "$GITHUB_SHA" ]');
    expect(publish).toContain('targetCommitish=$target_commitish（仅诊断');
    expect(publish).toContain('[ "$remote_tag_sha" != "$GITHUB_SHA" ]');
    expect(publish.indexOf('[ "$is_draft" != "true" ]')).toBeLessThan(
      publish.indexOf('gh release upload "$tag" "$asset"'),
    );
    expect(publish).toContain('keytool -exportcert -keystore "$jks"');
    expect(publish).toContain(
      "pinned_cert_sha256='22c183cb41d3dcbe6d355ba58d0a012953c5036592b61fb1efacfe3fa36eabd7'",
    );
    expect(publish).toContain('[ "$cert_sha256" != "$pinned_cert_sha256" ]');
    expect(publish).toContain(
      'signature_report="$("$apksigner" verify --verbose --print-certs "$asset")"',
    );
    expect(publish).toContain('[ "${certs[0]:-}" != "$POLARIS_RELEASE_CERT_SHA256" ]');
    expect(publish).toContain(
      'expected_id="$(node -p \'require("./src-tauri/tauri.android.conf.json").identifier\')"',
    );
    expect(publish).toContain('[ "$actual_id" != "$expected_id" ]');
    expect(publish).toContain('run: bash scripts/gate-android-release-behavior.sh');
    expect(publish).toContain(
      'node scripts/assert-r8-evidence.mjs "src-tauri/gen/android/app/build/outputs/mapping/${flavor}Release"',
    );
    expect(publish).toContain('for flavor in arm64 arm universal; do');
    expect(publish).toContain('name: android-release-mapping');
    expect(publish).toContain(
      'node scripts/release-assets.mjs abis "$RUNNER_TEMP/$asset_name" "$abi"',
    );
    expect(publish).toContain(
      '--apk "$RUNNER_TEMP/Polaris_${version}_${abi}-android.apk" --abi "$abi"',
    );
    expect(publish).toContain("digests='${{ steps.asset.outputs.digests }}'");
    expect(publish).toContain('signed_apks: ${{ steps.asset.outputs.digests }}');
    expect(publish).toContain('for pair in arm64:arm64-v8a arm:armeabi-v7a universal:universal; do');
    expect(publish).toContain('[ "$actual_sha" != "sha256:$expected_sha" ]');
    expect(publish).toContain(
      'signed_apk_sha256: ${{ steps.asset.outputs.sha256 }}',
    );
    expect(publish).not.toContain('--draft=false');
  });

  it('Android 资产命名读取实际 Gradle metadata，并与 APK manifest 的双版本对拍', () => {
    const publish = jobBlock(workflow('android.yml'), 'release-apk');
    const asset = stepBlock(publish, '改名成 release 资产名');
    const run = asset.split('        run: |\n')[1];
    expect(run).toBeDefined();
    const good = "package: name='com.polaris2.app' versionCode='1000000' versionName='1.0.0' platformBuildVersionName='16'";
    const goodProps = 'tauri.android.versionName=1.0.0\ntauri.android.versionCode=1000000\n';
    // APK contents are identical across metadata/error cases. Generate the
    // three real ZIP inventories once; every case still executes the actual
    // asset script against its own files, manifest probe and Gradle metadata.
    const zipSeed = mkdtempSync(join(tmpdir(), 'polaris-android-zip-seed-'));
    onTestFinished(() => rmSync(zipSeed, { recursive: true, force: true }));
    const apkFixtures = new Map<string, Buffer>();
    for (const [flavor, abis] of [['arm64', ['arm64-v8a']], ['arm', ['armeabi-v7a']],
      ['universal', ['arm64-v8a', 'armeabi-v7a']]] as const) {
      const path = join(zipSeed, `${flavor}.apk`);
      const made = spawnSync('python3', ['-c',
        'import sys,json,zipfile\nwith zipfile.ZipFile(sys.argv[1], "w") as z:\n for abi in json.loads(sys.argv[2]):\n  for lib in ["libbox.so", "libpolaris_lib.so", "libc++_shared.so"]: z.writestr("lib/"+abi+"/"+lib, "inert fixture")',
        path, JSON.stringify(abis)]);
      expect(made.status).toBe(0);
      apkFixtures.set(flavor, readFileSync(path));
    }
    for (const [badging, properties, tag, aaptExit, expected] of [
      [good, goodProps, 'v1.0.0', 0, 0],
      [good.replace("versionName='1.0.0'", "versionName='0.9.0'"), goodProps, 'v1.0.0', 0, 1],
      [good.replace("versionCode='1000000'", "versionCode='999999'"), goodProps, 'v1.0.0', 0, 1],
      [good, goodProps.replace('versionName=1.0.0', 'versionName=0.9.0'), 'v1.0.0', 0, 1],
      [good, goodProps.replace('versionCode=1000000', 'versionCode=999999'), 'v1.0.0', 0, 1],
      [good, goodProps + 'tauri.android.versionName=1.0.0\n', 'v1.0.0', 0, 1],
      [good, goodProps + 'tauri.android.versionCode=1000000\n', 'v1.0.0', 0, 1],
      [good, 'tauri.android.versionName=1.0.0\n', 'v1.0.0', 0, 1],
      [good, 'tauri.android.versionCode=1000000\n', 'v1.0.0', 0, 1],
      [good, null, 'v1.0.0', 0, 1],
      ['', goodProps, 'v1.0.0', 0, 1],
      ["package: name='com.polaris2.app'", goodProps, 'v1.0.0', 0, 1],
      [good + '\n' + good, goodProps, 'v1.0.0', 0, 1],
      [good.replace("versionCode='1000000'", "versionCode='invalid'"), goodProps, 'v1.0.0', 0, 1],
      [good, goodProps, 'v1.0.0', 1, 1],
      [good, goodProps, 'v0.9.0', 0, 1],
    ] as const) {
      const fixture = mkdtempSync(join(tmpdir(), 'polaris-android-asset-'));
      try {
        const app = join(fixture, 'src-tauri/gen/android/app');
        const apkDir = join(app, 'build/outputs/apk/arm64/release');
        const tools = join(fixture, 'sdk/build-tools/36.0.0');
        const output = join(fixture, 'github-output');
        mkdirSync(apkDir, { recursive: true });
        mkdirSync(join(fixture, 'scripts'));
        writeFileSync(join(fixture, 'scripts/release-assets.mjs'), readFileSync(join(REPO_ROOT, 'scripts/release-assets.mjs')));
        mkdirSync(tools, { recursive: true });
        writeFileSync(join(app, 'build.gradle.kts'), 'android { compileSdk = 36 }\n');
        if (properties !== null) {
          writeFileSync(join(app, 'tauri.properties'), properties);
        }
        for (const [flavor, bytes] of apkFixtures) {
          const directory = join(app, `build/outputs/apk/${flavor}/release`);
          mkdirSync(directory, { recursive: true });
          writeFileSync(join(directory, `app-${flavor}-release.apk`), bytes);
        }
        const bytes = readFileSync(join(apkDir, 'app-arm64-release.apk'));
        // Only the real asset step's metadata input is synthetic. No SDK or APK
        // command runs; the shell probe returns the manifest fixture verbatim.
        const aapt2 = join(tools, 'aapt2');
        writeFileSync(aapt2, '#!/bin/sh\n[ "$1" = dump ] && [ "$2" = badging ] || exit 2\nprintf \'%s\\n\' "$APK_BADGING"\nexit "$AAPT_EXIT"\n');
        chmodSync(aapt2, 0o755);
        expect(existsSync(join(fixture, 'src-tauri/gen/android/.tauri/tauri.properties'))).toBe(false);
        const script = run.replace(/^ {10}/gm, '').replaceAll(
          '${{ steps.release_identity.outputs.tag }}', tag,
        );
        const result = spawnSync('bash', ['-c', script], {
          cwd: fixture,
          env: { ...process.env, RUNNER_TEMP: fixture, GITHUB_OUTPUT: output,
            ANDROID_HOME: join(fixture, 'sdk'), APK_BADGING: badging, AAPT_EXIT: String(aaptExit) },
          encoding: 'utf8',
        });
        expect(result.status, result.stdout + result.stderr).toBe(expected);
        const renamed = join(fixture, 'Polaris_1.0.0_arm64-v8a-android.apk');
        expect(existsSync(renamed)).toBe(expected === 0);
        if (expected === 0) {
          expect(readFileSync(renamed)).toEqual(bytes);
          expect(readFileSync(output, 'utf8')).toContain('name=Polaris_1.0.0_arm64-v8a-android.apk\n');
        } else {
          expect(existsSync(output)).toBe(false);
        }
      } finally {
        rmSync(fixture, { recursive: true, force: true });
      }
    }
  });

  it.each(([0, 1, 2, 3] as const).flatMap((index) =>
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9].map((caseIndex) => [index, caseIndex] as const),
  ))('四处远端 digest 裁判 %i 场景 %i 经真实 gh/gojq 解析，且拒绝缺失或漂移的资产', async (index, caseIndex) => {
    const android = stepBlock(jobBlock(workflow('android.yml'), 'release-apk'), '上传成 release 资产');
    const desktop = stepBlock(jobBlock(workflow('package.yml'), 'release_desktop'),
      'Verify draft desktop asset digests against SHA256SUMS');
    const release = jobBlock(workflow('package.yml'), 'release');
    const combined = stepBlock(release, 'Verify combined asset set and digests');
    const promote = stepBlock(release, 'Promote release to public');
    const guards = [
      android.slice(android.indexOf("          row=''"), android.indexOf('          done < <')),
      desktop.slice(desktop.indexOf('          raw_all=')),
      combined.slice(combined.indexOf('          raw="$(mktemp)"')),
      promote.slice(promote.indexOf('          raw="$(mktemp)"'), promote.indexOf('          gh release edit')),
    ] as const;
    const guard = guards[index];
    const sha = 'a'.repeat(64);
    let assets: Array<{ name: string; digest?: string | null; state?: string }> = [];
    let requests = 0;
    const server = createServer((request, response) => {
      expect(request.method).toBe('GET');
      expect(request.url).toBe('/release');
      requests += 1;
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify({ assets }));
    });
    const fixture = mkdtempSync(join(tmpdir(), 'polaris-release-digest-'));
    const run = promisify(execFile);
    const controller = new AbortController();
    let cleanup: Promise<void> | undefined;
    const dispose = () => {
      if (cleanup) return cleanup;
      controller.abort();
      server.closeAllConnections();
      cleanup = new Promise<void>((resolve, reject) => {
        if (!server.listening) resolve();
        else server.close((error) => error ? reject(error) : resolve());
      }).finally(() => rmSync(fixture, { recursive: true, force: true }));
      return cleanup;
    };
    onTestFinished(dispose);
    try {
      await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
      const address = server.address();
      if (address === null || typeof address === 'string') throw new Error('REST fixture has no TCP port');
      const name = index === 0 ? 'Polaris_1.0.0_arm64-v8a-android.apk' : 'Polaris_1.0.0_amd64-linux.AppImage';
      const good = { name, digest: `sha256:${sha}`, state: 'uploaded' };
      const [responseAssets, expectedStatus] = ([
        [[good], 0],
        [[{ name, state: 'uploaded' }], 1],
        [[{ ...good, digest: null }], 1],
        [[{ ...good, digest: `sha256:${'b'.repeat(64)}` }], 1],
        [[{ ...good, state: 'starting' }], 1],
        [[{ name, digest: good.digest }], 1],
        [[{ ...good, name: 'wrong-name.apk' }], 1],
        [[], 1],
        [[good, good], 1],
        [[good, { ...good, name: 'unexpected-asset.zip' }], index === 0 ? 0 : 1],
      ] as const)[caseIndex];
      assets = [...responseAssets];
      const expected = join(fixture, 'expected');
      writeFileSync(expected, `${sha}  ${name}\n`);
      const before = requests;
      // Execute the actual post-upload/read guards, ending before any
      // mutation. Only retry sleeps are elided for the fixed REST fixture.
      const result = await run('bash', ['-c',
        `set -euo pipefail\nsleep() { :; }\n${guard.replace(/^ {10}/gm, '')}`], {
        env: { ...process.env, GH_TOKEN: 'localhost-fixture-not-a-secret',
          GH_CONFIG_DIR: fixture, TMPDIR: fixture, NO_PROXY: '127.0.0.1', no_proxy: '127.0.0.1',
          api_url: `http://127.0.0.1:${address.port}/release`, name, expected,
          expected_sha: sha, android_asset: 'Polaris_1.0.0_arm64-v8a-android.apk', tag: 'v1.0.0' },
        encoding: 'utf8',
        timeout: DIGEST_GUARD_CHILD_TIMEOUT_MS,
        killSignal: 'SIGKILL',
        signal: controller.signal,
      }).then(({ stdout, stderr }) => ({ status: 0, signal: null as string | null, stdout, stderr }),
        (error: { code: number | null; signal: string | null; stdout: string; stderr: string }) =>
          ({ status: error.code, signal: error.signal, stdout: error.stdout, stderr: error.stderr }));
      // 被限时杀掉时 status 是 null、signal 是 SIGKILL：把它写进失败文案，免得读成「脚本判错了」。
      expect(
        result.status,
        `guard ${index}, case ${caseIndex} (signal=${result.signal}): ${result.stdout}${result.stderr}`,
      ).toBe(expectedStatus);
      expect(requests).toBeGreaterThan(before);
    } finally {
      await dispose();
    }
  }, DIGEST_GUARD_TEST_TIMEOUT_MS);

  it('最终清单包含从草稿回读的 APK，并与远端完整资产集合双向对账后才公开', () => {
    const release = executable(jobBlock(workflow('package.yml'), 'release'));
    const promote = stepBlock(release, 'Promote release to public');
    const downloadAt = release.indexOf('gh release download "$TAG"');
    const sumsAt = release.indexOf('- name: Generate combined SHA256SUMS');
    const uploadSumsAt = release.indexOf('- name: Upload combined SHA256SUMS');
    const verifyAt = release.indexOf('- name: Verify combined asset set and digests');
    const promoteAt = release.indexOf('- name: Promote release to public');
    const remoteTagRefetchAt = release.lastIndexOf('git fetch --force --depth=1 origin');
    const publishCommandAt = release.indexOf(
      'gh release edit "$TAG" --repo "$GITHUB_REPOSITORY" --draft=false',
    );

    expect(downloadAt).toBeGreaterThan(0);
    expect(downloadAt).toBeLessThan(sumsAt);
    expect(sumsAt).toBeLessThan(uploadSumsAt);
    expect(uploadSumsAt).toBeLessThan(verifyAt);
    expect(verifyAt).toBeLessThan(promoteAt);
    expect(remoteTagRefetchAt).toBeGreaterThan(verifyAt);
    expect(remoteTagRefetchAt).toBeLessThan(publishCommandAt);
    expect(release).toContain('node scripts/verify-packaging.mjs assets --label release-all --dir dist-release');
    expect(promote).toContain(
      'VERIFIED_ANDROID_APKS: ${{ needs.android_release.outputs.signed_apks }}',
    );
    expect(promote).toContain('node scripts/release-assets.mjs android-digests "${TAG#v}" "$VERIFIED_ANDROID_APKS" "$sums"');
    expect(release).toContain('[ "$downloaded_sha" = "$verified_sha" ]');
    expect(release).toContain('node scripts/release-assets.mjs android-digests "${TAG#v}" "$VERIFIED_ANDROID_APKS" dist-release/SHA256SUMS');
    expect(release).toContain('find . -type f ! -name SHA256SUMS');
    expect(release).toContain("cat \"$sums\"");
    expect(release).toContain(
      `printf '%s  %s\\n' "$(sha256sum "$sums" | cut -d' ' -f1)" 'SHA256SUMS'`,
    );
    expect(release).toContain(
      `pending="$(awk -F'\\t' '$1 == "MISSING" || $2 != "uploaded"' "$raw")"`,
    );
    expect(release).toContain("'SHA256SUMS'");
    expect(release).toContain('diff -u "$expected" "$actual"');
    expect(release.match(/gh api "\$api_url" --jq/g) ?? []).toHaveLength(2);
    expect(release.match(/diff -u "\$expected" "\$actual"/g) ?? []).toHaveLength(2);
    expect(release).toContain('拒绝覆盖 SHA256SUMS');
    expect(release).toContain('公开紧前远端资产已被替换，拒绝公开');
    expect(release).toContain('公开前 tag/release 身份漂移，拒绝继续');
    expect(promote.indexOf('gh api "$api_url" --jq')).toBeLessThan(
      promote.indexOf('diff -u "$expected" "$actual"'),
    );
    expect(promote.indexOf('diff -u "$expected" "$actual"')).toBeLessThan(
      promote.indexOf('gh release edit "$TAG"'),
    );
  });
});

describe('风险门轻量构型：只在 release-risk 路径生效，发行构型一字不动', () => {
  const ALLOWED = ['CARGO_PROFILE_RELEASE_CODEGEN_UNITS', 'CARGO_PROFILE_RELEASE_LTO'];

  /**
   * 同一机制的两条腿（2026-10-08 起 Android 验证腿加入）。**每条腿各切各片、各跑各的入口表、
   * 各有各的变异收据** —— 只对其中一条腿取材的判据，在另一条腿被改坏时一条都不红。
   * `guard` 是各自第三条闸读的变量：桌面是「要不要上传 artifact」，Android 是「是不是签名发布」。
   */
  const DESKTOP = {
    file: 'package.yml',
    jobs: ['package'],
    step: 'Select installer build weight (release profile vs risk-gate light)',
    guard: 'POLARIS_UPLOAD_ARTIFACTS',
    builds: { package: ['Build and stage privileged helper', 'Build installers'] } as Record<string, string[]>,
  };
  const ANDROID = {
    file: 'android.yml',
    jobs: ['release_check', 'release-apk'],
    step: 'Select APK build weight (release profile vs risk-gate light)',
    guard: 'POLARIS_PUBLISH_RELEASE',
    builds: {
      release_check: ['Build unsigned release-profile validation APK'],
      'release-apk': ['Build signed release APK'],
    } as Record<string, string[]>,
  };
  const LEGS = [DESKTOP, ANDROID];

  /** 取出某个 job 里那一步的 shell 脚本本体（去掉 YAML 的 10 格缩进）。 */
  function weightScript(src: string, job: string, stepName: string): string {
    const step = stepBlock(jobBlock(src, job), stepName);
    const runAt = step.indexOf('\n        run: |\n');
    if (runAt < 0) throw new Error(`${job} 的构建权重步没有 run 块 —— 取材面塌了`);
    // 只取 run 块本体：遇到第一行缩进不足 10 格的非空行即止。不截的话，下一步上方的整段
    // YAML 注释（缩进 6 格）会被 `slice(10)` 削成裸词当命令执行。
    const body: string[] = [];
    for (const line of step.slice(runAt + '\n        run: |\n'.length).split('\n')) {
      if (line.trim() !== '' && !line.startsWith(' '.repeat(10))) break;
      body.push(line.slice(10));
    }
    return body.join('\n');
  }

  /** 按给定入口真跑一遍那段脚本，返回退出码与它写进 GITHUB_ENV 的内容。 */
  function runWeight(script: string, env: Record<string, string>) {
    const dir = mkdtempSync(join(tmpdir(), 'polaris-build-weight-'));
    try {
      const githubEnv = join(dir, 'github-env');
      const summary = join(dir, 'summary');
      writeFileSync(githubEnv, '');
      writeFileSync(summary, '');
      // 从宿主环境里剔掉任何 CARGO_PROFILE_* 与三个开关：脚本的第一道闸就是查前者，宿主恰好设了
      // 会让每条用例都红；后者若从宿主漏进来，「开关没接上」那几格就不是在测脚本了。
      const base = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
        !key.startsWith('CARGO_PROFILE_')
        && !['POLARIS_LIGHT_BUILD', 'POLARIS_UPLOAD_ARTIFACTS', 'POLARIS_PUBLISH_RELEASE'].includes(key)));
      const result = spawnSync('bash', ['-c', script], {
        env: { ...base, GITHUB_ENV: githubEnv, GITHUB_STEP_SUMMARY: summary, ...env },
        encoding: 'utf8',
      });
      return { status: result.status, output: result.stdout + result.stderr, written: readFileSync(githubEnv, 'utf8') };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }

  const LIGHT = 'CARGO_PROFILE_RELEASE_LTO=off\nCARGO_PROFILE_RELEASE_CODEGEN_UNITS=16\n';
  const branch = 'refs/heads/main';
  const tag = 'refs/tags/v1.0.0';

  type WeightCase = [string, Record<string, string>, number, string];

  /** 整张入口表。任何一格不符即抛 —— 下面的变异对照靠它抛。 */
  function checkTable(script: string, cases: WeightCase[]) {
    for (const [name, env, status, written] of cases) {
      const actual = runWeight(script, env);
      if (actual.status !== status || actual.written !== written) {
        throw new Error(
          `${name}: 期望 rc=${status} 且写入 ${JSON.stringify(written)}，` +
            `实为 rc=${actual.status} 写入 ${JSON.stringify(actual.written)}\n${actual.output}`,
        );
      }
    }
  }

  /** 桌面打包腿的入口表。 */
  function checkWeight(script: string) {
    checkTable(script, [
      // 发行路径：什么都不写，cargo 读到的就是 Cargo.toml 原样。
      ['tag 发布', { POLARIS_LIGHT_BUILD: '0', GITHUB_REF: tag, POLARIS_UPLOAD_ARTIFACTS: '1' }, 0, ''],
      ['手动全量打包', { POLARIS_LIGHT_BUILD: '0', GITHUB_REF: branch, POLARIS_UPLOAD_ARTIFACTS: '1' }, 0, ''],
      // 不上传 ≠ 轻量：只有开关本身能选中轻量构型（这一格同时是「发行分支不再提前返回」变异的判据）。
      ['复用但未要求轻量', { POLARIS_LIGHT_BUILD: '0', GITHUB_REF: branch, POLARIS_UPLOAD_ARTIFACTS: '0' }, 0, ''],
      // 风险门：恰好两个键。
      ['release-risk 复用', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_UPLOAD_ARTIFACTS: '0' }, 0, LIGHT],
      // fail-closed 三条。
      ['轻量落在 tag 上', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: tag, POLARIS_UPLOAD_ARTIFACTS: '0' }, 1, ''],
      ['轻量却要上传', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_UPLOAD_ARTIFACTS: '1' }, 1, ''],
      ['发行路径上环境里已有 profile 覆盖', {
        POLARIS_LIGHT_BUILD: '0', GITHUB_REF: tag, POLARIS_UPLOAD_ARTIFACTS: '1',
        CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS: 'true',
      }, 1, ''],
      ['风险门上环境里已有别的 profile 覆盖', {
        POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_UPLOAD_ARTIFACTS: '0',
        CARGO_PROFILE_RELEASE_PANIC: 'abort',
      }, 1, ''],
      // 开关取值只认 0 / 1：空串（变量没接上）与 'true' 都不得被当成「发行构型」放过去。
      ['开关没接上', { POLARIS_LIGHT_BUILD: '', GITHUB_REF: branch, POLARIS_UPLOAD_ARTIFACTS: '0' }, 1, ''],
      ['开关写成 true', { POLARIS_LIGHT_BUILD: 'true', GITHUB_REF: branch, POLARIS_UPLOAD_ARTIFACTS: '0' }, 1, ''],
    ]);
  }

  /** Android 两个 job 共用的入口表（那一步在两个 job 里逐字相同，入口由 workflow 级 env 决定）。 */
  function checkAndroidWeight(script: string) {
    checkTable(script, [
      // 发行路径：什么都不写。签名发布腿只可能落在前两格。
      ['tag 签名发布', { POLARIS_LIGHT_BUILD: '0', GITHUB_REF: tag, POLARIS_PUBLISH_RELEASE: '1' }, 0, ''],
      ['显式 release_tag 的签名发布（ref 不是 tag）', { POLARIS_LIGHT_BUILD: '0', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '1' }, 0, ''],
      // 不发布 ≠ 轻量：手动 dispatch 的验证腿是发行构型（发 tag 前预验 fat LTO 的入口）。
      ['手动 dispatch 验证腿', { POLARIS_LIGHT_BUILD: '0', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '0' }, 0, ''],
      // 风险门：恰好两个键。
      ['release-risk 复用', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '0' }, 0, LIGHT],
      // fail-closed 三条。
      ['轻量落在 tag 上', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: tag, POLARIS_PUBLISH_RELEASE: '0' }, 1, ''],
      ['轻量却要签名发布', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '1' }, 1, ''],
      ['发布路径上环境里已有 profile 覆盖', {
        POLARIS_LIGHT_BUILD: '0', GITHUB_REF: tag, POLARIS_PUBLISH_RELEASE: '1',
        CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS: 'true',
      }, 1, ''],
      ['风险门上环境里已有别的 profile 覆盖', {
        POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '0',
        CARGO_PROFILE_RELEASE_PANIC: 'abort',
      }, 1, ''],
      // 两个开关都只认 0 / 1：没接上（空串 / 未定义）与 'true' 一律红，不得落到任何一种构型上。
      ['构建权重开关没接上', { POLARIS_LIGHT_BUILD: '', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '0' }, 1, ''],
      ['构建权重开关写成 true', { POLARIS_LIGHT_BUILD: 'true', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '0' }, 1, ''],
      ['轻量但发布开关没接上', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: '' }, 1, ''],
      ['轻量但发布开关未定义', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch }, 1, ''],
      ['轻量但发布开关写成 false', { POLARIS_LIGHT_BUILD: '1', GITHUB_REF: branch, POLARIS_PUBLISH_RELEASE: 'false' }, 1, ''],
    ]);
  }

  /** 两条腿共用的五条变异 + 各自第三条闸的那一条。 */
  function mutationsFor(guard: string): [string, string, string][] {
    return [
      ['拆掉 tag 闸', 'refs/tags/*) echo "::error::轻量构型不得用于 tag（$GITHUB_REF）：tag 上的产物是发布物"; exit 1 ;;', 'refs/tags/*) ;;'],
      ['拆掉第三条闸', `[ "$${guard}" = 0 ] || {`, 'true || {'],
      ['拆掉环境泄漏闸', 'if [ -n "$leaked" ]; then', 'if false; then'],
      ['非法取值落到发行构型', "*) echo \"::error::POLARIS_LIGHT_BUILD 取值非法：'$POLARIS_LIGHT_BUILD'\"; exit 1 ;;", '*) exit 0 ;;'],
      ['发行分支不再提前返回（会一路走到写覆盖）', 'exit 0 ;;', ';;'],
      ['多写一个键', "  echo 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16'\n", "  echo 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16'\n  echo 'CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true'\n"],
      ['改一个取值', "echo 'CARGO_PROFILE_RELEASE_LTO=off'", "echo 'CARGO_PROFILE_RELEASE_LTO=thin'"],
    ];
  }

  function expectSliceIsTheScript(script: string, forbidden: string) {
    // 切片自检：拿到的是脚本本体，且没有把下一步卷进来。
    expect(script).toContain('set -euo pipefail');
    expect(script).not.toContain('- name:');
    expect(script, '切片把下一步上方的注释卷进来了').not.toContain(forbidden);
    expect(script.trimEnd().endsWith('>> "$GITHUB_STEP_SUMMARY"'), '切片没有落在脚本最后一行').toBe(true);
  }

  function expectMutationsBite(script: string, guard: string, check: (script: string) => void) {
    // 变异对照：把每一道闸各拆一次，整张表必须红（否则上面的绿没有信息量）。
    for (const [name, from, to] of mutationsFor(guard)) {
      const mutated = script.replace(from, to);
      expect(mutated, `变异「${name}」没打上 —— 针与脚本原文不符，这条对照是空的`).not.toBe(script);
      expect(() => check(mutated), `变异「${name}」没有让入口表转红`).toThrow();
    }
  }

  /**
   * 两处脚本的**共用骨架**：去掉各自的第三条闸（两行）与结尾两行提示文案后剩下的全部。
   * 泄漏闸、开关取值判定、tag 闸、写入块都在骨架里。
   */
  function skeleton(script: string, guard: string, origin: string): string {
    const lines = script.trimEnd().split('\n');
    const guardAt = lines.findIndex((line) => line.startsWith(`[ "$${guard}" = 0 ] || {`));
    const envAt = lines.findIndex((line) => line === '} >> "$GITHUB_ENV"');
    if (guardAt < 0 || envAt < guardAt) throw new Error(`${origin}: 找不到第三条闸或写入块 —— 骨架切不出来`);
    // 第三条闸恰两行（条件行 + 报错并退出行）；写入块之后恰两行提示（notice + step summary）。
    expect(lines[guardAt + 1].trimEnd().endsWith('exit 1; }'), `${origin}: 第三条闸不是两行形态`).toBe(true);
    expect(lines.slice(envAt + 1), `${origin}: 写入块之后不是恰两行提示`).toHaveLength(2);
    expect(lines[envAt + 1].startsWith('echo "::notice title=Risk-gate light build::')).toBe(true);
    expect(lines[envAt + 2].startsWith("printf '%s\\n' '**Risk-gate light build**")).toBe(true);
    return [...lines.slice(0, guardAt), ...lines.slice(guardAt + 2, envAt + 1)].join('\n');
  }

  /** 一段可执行文本里全部 `CARGO_PROFILE_X=值` 的写入，按出现顺序。 */
  function assignments(text: string): string[] {
    return executable(text).match(/CARGO_PROFILE_[A-Z_]+=[^'"\s]+/g) ?? [];
  }

  it('构建权重步按各入口真跑：发行路径零写入，风险门恰写两个键，三条 fail-closed 都咬得住', () => {
    const script = weightScript(workflow(DESKTOP.file), 'package', DESKTOP.step);
    expectSliceIsTheScript(script, 'D7');
    checkWeight(script);
    expectMutationsBite(script, DESKTOP.guard, checkWeight);
    // 逐入口、逐变异起 bash 子进程；与全量并行时超过默认的 5s。
  }, 30_000);

  it('Android 两个 job 的构建权重步各自真跑：签名发布零写入，验证腿只在风险门写两个键，三条 fail-closed 都咬得住', () => {
    const android = workflow(ANDROID.file);
    for (const job of ANDROID.jobs) {
      // 每个 job 各切各片：只跑其中一个的话，另一个 job 里的那份被改坏时这里照绿。
      const script = weightScript(android, job, ANDROID.step);
      expectSliceIsTheScript(script, 'key 用固定串');
      checkAndroidWeight(script);
      expectMutationsBite(script, ANDROID.guard, checkAndroidWeight);
    }
    // 两个 job 里那一步（连同它上方的注释）逐字相同；Rust 侧 ⑦ 的 prelude 对拍也覆盖它，这里就近再钉一次。
    const steps = ANDROID.jobs.map((job) => stepBlock(jobBlock(android, job), ANDROID.step));
    expect(steps[0].length).toBeGreaterThan(800);
    expect(steps[1]).toBe(steps[0]);
  }, 30_000);

  it('桌面与 Android 两处是同一段判定：骨架逐行相同，写入的键值逐字相等', () => {
    const desktop = weightScript(workflow(DESKTOP.file), 'package', DESKTOP.step);
    const desktopSkeleton = skeleton(desktop, DESKTOP.guard, DESKTOP.file);
    // 骨架自检：四样东西都在里面，且被剔掉的恰是 4 行（第三条闸 2 + 提示 2）。
    for (const needle of [
      `leaked="$(env | grep '^CARGO_PROFILE_' || true)"`,
      'case "$POLARIS_LIGHT_BUILD" in',
      'refs/tags/*) echo "::error::',
      "echo 'CARGO_PROFILE_RELEASE_LTO=off'",
      "echo 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16'",
    ]) {
      expect(desktopSkeleton, `骨架里没有 ${needle} —— 切片塌了`).toContain(needle);
    }
    expect(desktopSkeleton.split('\n').length).toBe(desktop.trimEnd().split('\n').length - 4);
    expect(desktopSkeleton).not.toContain(DESKTOP.guard);

    const desktopStep = stepBlock(jobBlock(workflow(DESKTOP.file), 'package'), DESKTOP.step);
    const desktopWrites = assignments(desktopStep);
    expect(desktopWrites.join('\n') + '\n').toBe(LIGHT);

    const android = workflow(ANDROID.file);
    for (const job of ANDROID.jobs) {
      const script = weightScript(android, job, ANDROID.step);
      const androidSkeleton = skeleton(script, ANDROID.guard, `${ANDROID.file} ${job}`);
      expect(androidSkeleton, `${job} 的判定骨架与桌面漂了 —— 两处必须同步改`).toBe(desktopSkeleton);
      expect(script.trimEnd().split('\n').length).toBe(desktop.trimEnd().split('\n').length);
      // 取值相等单独钉一次：骨架相等已蕴含它，但这条的失败信息直接点名「两处取值不同」。
      expect(assignments(stepBlock(jobBlock(android, job), ANDROID.step)), `${job} 写入的键值与桌面不同`)
        .toEqual(desktopWrites);
    }
  });

  it('开关只认调用方显式传入的 true，且全仓只有 release-risk 的两条腿在传', () => {
    const pkg = workflow('package.yml');
    const pkgJob = executable(jobBlock(pkg, 'package'));
    const match = pkgJob.match(/^ {6}POLARIS_LIGHT_BUILD: \$\{\{ (.+) \}\}$/m);
    expect(match, 'package job 缺 POLARIS_LIGHT_BUILD 表达式').not.toBeNull();
    const evaluate = (inputs: Record<string, unknown>) => runInNewContext(match![1], { inputs });
    // tag push 与本 workflow 的手动 dispatch 都没有这个输入。
    expect(evaluate({})).toBe('0');
    expect(evaluate({ platform: 'all' })).toBe('0');
    expect(evaluate({ light_build: false })).toBe('0');
    expect(evaluate({ light_build: true })).toBe('1');
    expect(pkgJob.match(/^\s+POLARIS_LIGHT_BUILD:/gm) ?? [], 'step 不得另行覆盖构建权重开关').toHaveLength(1);

    // 输入只在 workflow_call 上声明、缺省 false；手动 dispatch 入口没有它（手动 = 发行语义）。
    const dispatch = pkg.slice(pkg.indexOf('\n  workflow_dispatch:\n'), pkg.indexOf('\n  workflow_call:\n'));
    expect(dispatch.length).toBeGreaterThan(50);
    expect(executable(dispatch)).not.toContain('light_build');
    expect(executable(pkg)).toContain(
      '      light_build:\n        type: boolean\n        required: false\n        default: false\n',
    );

    // ── Android：两个开关在 workflow 级各定义一次，两个 job 与任何 step 都不得另行覆盖 ──
    const android = workflow('android.yml');
    const androidExecutable = executable(android);
    const androidEnv = (key: string) => {
      const found = androidExecutable.match(new RegExp(`^ {2}${key}: \\$\\{\\{ (.+) \\}\\}$`, 'm'));
      expect(found, `android.yml 的 workflow 级 env 缺 ${key} 表达式`).not.toBeNull();
      expect(androidExecutable.match(new RegExp(`^\\s+${key}:`, 'gm')) ?? [], `${key} 被 job / step 另行覆盖`)
        .toHaveLength(1);
      return (inputs: Record<string, unknown>) => runInNewContext(found![1], { inputs });
    };
    const light = androidEnv('POLARIS_LIGHT_BUILD');
    const publish = androidEnv('POLARIS_PUBLISH_RELEASE');
    // 手动 dispatch 不接收构型/发布输入；preserve_run 不改变轻量或签名模式。
    expect([light({}), publish({})]).toEqual(['0', '0']);
    expect([
      light({ publish_release: true, release_tag: 'v1.0.0' }),
      publish({ publish_release: true, release_tag: 'v1.0.0' }),
    ]).toEqual(['0', '1']);
    expect([light({ light_build: false }), publish({ light_build: false })]).toEqual(['0', '0']);
    expect([light({ light_build: true }), publish({ light_build: true })]).toEqual(['1', '0']);
    // 两个都传 ⇒ ('1','1')，入口表的「轻量却要签名发布」一格证明它当场红。
    expect([
      light({ light_build: true, publish_release: true }),
      publish({ light_build: true, publish_release: true }),
    ]).toEqual(['1', '1']);
    // 构型/发布输入只在 workflow_call 上声明、缺省 false；手动入口只允许保护审计运行。
    const androidDispatch = executable(
      android.slice(android.indexOf('\n  workflow_dispatch:'), android.indexOf('\npermissions:')),
    );
    expect(androidDispatch).toMatch(/\n {6}preserve_run:\n {8}type: boolean\n {8}default: false/);
    expect([...androidDispatch.matchAll(/^ {6}([a-z_]+):$/gm)].map((match) => match[1])).toEqual(['preserve_run', 'hot_consumer_candidate']);
    expect(androidDispatch).not.toMatch(/light_build:|publish_release:|release_tag:/);
    expect(androidExecutable).toMatch(/\n {6}light_build:\n(?: {8}(?!default:).*\n)* {8}default: false\n/);
    // 签名发布腿只在 publish_release 为真时跑 ⇒ 在它身上「轻量」必然是 ('1','1') 那一格。
    expect(executable(jobBlock(android, 'release-apk'))).toMatch(/^ {4}if: inputs\.publish_release$/m);
    // Three formal evidence allowlists remain; the isolated candidate adds only JSON/XML/log evidence.
    const releaseCheck = jobBlock(android, 'release_check');
    expect(executable(releaseCheck).match(/uses: actions\/upload-artifact@/g) ?? []).toHaveLength(4);
    const uploaded = executable(stepBlock(releaseCheck, '留存 mapping.txt / usage.txt（唯一能把 logcat 反混淆回来的东西）'))
      .split('\n')
      .filter((line) => line.startsWith(' '.repeat(12)))
      .map((line) => line.trim());
    expect(uploaded).toEqual([
      ...['mapping.txt', 'usage.txt', 'seeds.txt', 'configuration.txt'].map(
        (name) => `src-tauri/gen/android/app/build/outputs/mapping/arm64Release/${name}`,
      ),
      ...['DefaultNetworkObservationTest', 'NetworkMonitorSessionTest', 'LoadedLibboxIdentityTest', 'AndroidCallbackSchedulingSourceTest', 'AndroidNativeAdmissionTest', 'AndroidNativeMainControlsTest', 'MainNativeAdmissionWiringTest', 'DualModeEndpointWiringTest', 'RuntimePersistenceSourceTest', 'RuntimeProtocolAdmissionTest'].map(
        (name) => `src-tauri/gen/android/app/build/test-results/testArm64ReleaseUnitTest/TEST-com.polaris2.app.vpn.${name}.xml`,
      ),
      '${{ runner.temp }}/s5-android-unit-results.json',
      '${{ runner.temp }}/release-stage1.log',
    ]);
    const stage1 = stepBlock(releaseCheck, 'Build unsigned release-profile validation APK');
    expect(stage1).toContain('stage1_log="$RUNNER_TEMP/release-stage1.log"');
    expect(stage1).toContain('>"$stage1_log" 2>&1');
    const validationEvidence = stepBlock(releaseCheck, '留存 mapping.txt / usage.txt（唯一能把 logcat 反混淆回来的东西）');
    expect(executable(validationEvidence).match(/^ {8}if: (.+)$/m)?.[1])
      .toBe("always() && (inputs.hot_consumer_candidate || '') == ''");
    const formalEvidence = [
      '${{ steps.libbox_input.outputs.work }}/polaris-box-*.aar',
      '${{ steps.libbox_input.outputs.work }}/staging/stage.aar',
      '${{ steps.libbox_input.outputs.work }}/*receipt.json',
      '${{ steps.libbox_input.outputs.work }}/*bind.log',
      '${{ steps.libbox_input.outputs.work }}/budget-observation.json',
      '${{ steps.libbox_input.outputs.work }}/cache-binding-observation.json',
      '${{ steps.libbox_input.outputs.work }}/module-cache-admission-observation.json',
      'scripts/mobile-core/android-consumption.json',
      'src-tauri/gen/android/app/build/outputs/apk/**/*.source-consumption.json',
      '${{ runner.temp }}/*.source-consumption.json',
    ];
    // A bind/inspect rejection leaves the real producer staging component in place.
    // Bind this whitelist to its source-owned path, rather than guessing the final name.
    const producer = readFileSync(join(REPO_ROOT, 'scripts/mobile-core/producer.py'), 'utf8');
    expect(producer).toContain("stage_root = private_dir(work / 'staging')");
    expect(producer).toContain("stage_component = stage_root / ('stage.aar' if args.platform == 'android' else 'Libbox.xcframework')");
    expect(formalEvidence).toHaveLength(10);
    const ndkObservations = [
      '~/Code/polaris-mobile-ci/ndk-source.properties',
      '~/Code/polaris-mobile-ci/sdk-input-observation.json',
    ];
    // Match actual payload filename extensions, not the '.so' prefix of
    // '*.source-consumption.json'. Exact evidence inventories remain required.
    const isApkOrSoPayload = (path: string) => /(?:^|\/)[^/]*\.(?:apk|so)$/i.test(path.trim());
    for (const path of ['app-arm64-release.apk', '/work/**/*.apk', '/work/libbox.so', '/work/**/*.so']) {
      expect(isApkOrSoPayload(path), `actual payload must be refused: ${path}`).toBe(true);
    }
    expect(isApkOrSoPayload('${{ runner.temp }}/*.source-consumption.json')).toBe(false);
    for (const job of ['release_check', 'release-apk']) {
      const evidence = executable(stepBlock(jobBlock(android, job), 'Retain actual formal mobile component and original receipts'));
      const paths = evidence.split('\n').filter(line => line.startsWith(' '.repeat(12))).map(line => line.trim());
      expect(paths, `${job}: exact formal evidence whitelist`).toEqual(formalEvidence);
      expect(evidence.match(/^ {8}if: (.+)$/m)?.[1]).toBe(job === 'release_check'
        ? "always() && steps.libbox_input.outcome == 'success' && (inputs.hot_consumer_candidate || '') == ''"
        : "always() && steps.libbox_input.outcome == 'success'");
      expect(evidence).toContain('if-no-files-found: error');
      expect(evidence).not.toMatch(/\bgh\s+release\b/);
      expect(paths.filter(isApkOrSoPayload), `${job}: no APK/SO payload in evidence upload`).toEqual([]);
      const block = jobBlock(android, job);
      expect(executable(block).match(/uses: actions\/upload-artifact@/g) ?? [], `${job}: exact evidence artifacts`).toHaveLength(job === 'release_check' ? 4 : 3);
      const observations = executable(stepBlock(block, 'Retain actual NDK input observations even on prepare failure'));
      const observedPaths = observations.split('\n').filter(line => line.startsWith(' '.repeat(12))).map(line => line.trim());
      expect(observedPaths, `${job}: exact failure-reachable NDK evidence whitelist`).toEqual(ndkObservations);
      expect(observations.match(/^ {8}if: (.+)$/m)?.[1]).toBe(job === 'release_check'
        ? "always() && (inputs.hot_consumer_candidate || '') == ''" : 'always()');
      expect(observations).not.toMatch(/libbox_input|outputs\.|\bgh\s+release\b/);
      expect(observations).toContain('name: formal-mobile-ndk-input-${{ github.job }}');
      expect(observations).toContain('if-no-files-found: error');
      expect(observedPaths.filter(isApkOrSoPayload), `${job}: no APK/SO in failed prepare evidence`).toEqual([]);
      expect([...paths, ...observedPaths], `${job}: exact combined ten formal evidence paths`).toEqual([...formalEvidence, ...ndkObservations]);
    }
    expect(executable(releaseCheck)).not.toMatch(/gh release/);
    // 留存 SDK 与原观察不分发 APK/SO；S5 必须走真实 app Gradle/AAR，不能借 host/stub 结果冒签。
    const s5Units = executable(stepBlock(releaseCheck, 'Verify S5 Android network observation and admission unit tests'));
    expect(s5Units).toContain('bash src-tauri/gen/android/gradlew --project-dir src-tauri/gen/android');
    expect(s5Units).toContain(':app:cleanTestArm64ReleaseUnitTest :app:testArm64ReleaseUnitTest');
    for (const name of ['DefaultNetworkObservationTest', 'NetworkMonitorSessionTest', 'LoadedLibboxIdentityTest', 'AndroidCallbackSchedulingSourceTest', 'AndroidNativeAdmissionTest', 'AndroidNativeMainControlsTest', 'MainNativeAdmissionWiringTest', 'DualModeEndpointWiringTest', 'RuntimePersistenceSourceTest', 'RuntimeProtocolAdmissionTest']) {
      expect(s5Units).toContain(`--tests com.polaris2.app.vpn.${name}`);
    }
    expect(s5Units).toContain('python3 -I -');
    expect(s5Units).toContain("for k in ('failures', 'errors', 'skipped')");
    expect(s5Units).toContain('len(cases) == len(methods)');
    expect(s5Units).toContain("sourceSha256");
    expect(s5Units).toContain("reportSha256");
    expect(s5Units).not.toMatch(/continue-on-error|kotlinc|\|\| true/);


    // 传 true 的调用点全仓恰两处，都在 release-risk：打包腿（与「不上传」成对）与 Android 腿（不传发布）。
    // 旧判据是「全仓恰一处」；现在按文件、按 job 各自点名，别的 workflow（含 tag 发布所在的 package.yml）为零。
    const names = readdirSync(join(REPO_ROOT, '.github/workflows')).filter((name) => /\.ya?ml$/.test(name));
    expect(names.length, '工作流目录一个文件都没读到 —— 取材面塌了').toBeGreaterThan(4);
    const passes = (text: string) => (executable(text).match(/light_build:\s*true/g) ?? []).length;
    for (const name of names) {
      expect(passes(workflow(name)), `${name} 里传 light_build: true 的次数`).toBe(name === 'release-risk.yml' ? 2 : 0);
    }
    const risk = workflow('release-risk.yml');
    const riskPackage = executable(jobBlock(risk, 'package'));
    expect(riskPackage).toContain('uses: ./.github/workflows/package.yml');
    expect(riskPackage).toContain('light_build: true');
    expect(riskPackage).toContain('upload_artifacts: false');
    const riskAndroid = executable(jobBlock(risk, 'android'));
    expect(riskAndroid).toContain('uses: ./.github/workflows/android.yml');
    expect(riskAndroid).toContain('light_build: true');
    expect(riskAndroid).not.toContain('publish_release');
    // tag 发布段调 android.yml 时只传发布，不传轻量。
    const tagAndroid = executable(jobBlock(pkg, 'android_release'));
    expect(tagAndroid).toContain('publish_release: true');
    expect(tagAndroid).not.toContain('light_build');
  });

  it('cargo profile 的环境变量覆盖只有两处同形入口，且各自排在缓存与构建之前', () => {
    // 这是一条绕开 `release_escape_hatches.rs` 的路径：那边的发行构型断言（lto = "fat" /
    // codegen-units = 1 / 不得开 debug-assertions / 不得 panic = "abort"）只读 Cargo.toml，
    // 而 `CARGO_PROFILE_RELEASE_*` 环境变量能改写其中任何一条且不动任何文件。
    // 故把入口钉死：全部 workflow 里只有两处构建权重步提到它，且只有这两个键。
    const names = readdirSync(join(REPO_ROOT, '.github/workflows')).filter((name) => /\.ya?ml$/.test(name));
    expect(names).toContain('package.yml');
    expect(names).toContain('android.yml');
    const pattern = /CARGO_PROFILE_[A-Z][A-Z_]*/g;
    const count = (text: string) => (text.match(/CARGO_PROFILE_/g) ?? []).length;
    for (const name of names) {
      const source = workflow(name);
      const hits = executable(source).match(pattern) ?? [];
      const leg = LEGS.find((candidate) => candidate.file === name);
      if (!leg) {
        expect(hits, `${name} 出现了 cargo profile 环境变量覆盖 —— 发行构型的源码门看不见它`).toEqual([]);
        continue;
      }
      expect([...new Set(hits)].sort(), `${name} 里出现了两个放行键之外的 profile 覆盖`).toEqual(ALLOWED);
      // 文件里每一处 CARGO_PROFILE_ 都落在该文件各 job 的构建权重步之内（步外一处都没有）。
      let inSteps = 0;
      for (const job of leg.jobs) {
        const rawJob = jobBlock(source, job);
        const jobText = executable(rawJob);
        const step = executable(stepBlock(rawJob, leg.step));
        expect(count(step), `${name} ${job} 的构建权重步里一处 CARGO_PROFILE_ 都没有 —— 取材面塌了`).toBeGreaterThan(2);
        expect(count(jobText), `${name} ${job}：CARGO_PROFILE_ 出现在构建权重步之外`).toBe(count(step));
        inSteps += count(step);

        // 顺序：权重先定，再恢复缓存（键靠它分开），再构建（同一 job 的每次 cargo 构建同吃一份构型）。
        const at = (needle: string) => {
          const index = jobText.indexOf(needle);
          expect(index, `${name} ${job} 里找不到「${needle}」`).toBeGreaterThanOrEqual(0);
          return index;
        };
        let previous = at(`- name: ${leg.step}\n`);
        for (const next of ['Setup Rust cache', ...leg.builds[job]]) {
          const index = at(`- name: ${next}\n`);
          expect(previous, `${name} ${job}：「${next}」排到了它前一步之前`).toBeLessThan(index);
          previous = index;
        }
        // 发行路径上这一步不能被关掉或忽略失败；Android 只允许隔离显式候选的原样条件。
        expect(step).not.toMatch(/^ {8}continue-on-error:/m);
        // 连同缩进续行一起取值，避免首行原样、下一行 `|| true` / `&& false` 绕过精确条件。
        expect(step.match(/^ {8}if:.*(?:\n(?: {9,}.*|[ \t]*)$)*/gm) ?? []).toEqual(leg === ANDROID
          ? ["        if: (inputs.hot_consumer_candidate || '') == ''"] : []);
      }
      expect(count(executable(source)), `${name}：CARGO_PROFILE_ 出现在各 job 的构建权重步之外`).toBe(inSteps);
    }
    // cargo 的 config 文件同样能写 [profile.*]，与环境变量是同一类旁路。
    const cargoConfig = executable(readFileSync(join(REPO_ROOT, '.cargo/config.toml'), 'utf8'));
    expect(cargoConfig).toContain('[target.x86_64-pc-windows-msvc]');
    expect(cargoConfig, '.cargo/config.toml 出现了 [profile.*] —— 它会改写发行构型而 Cargo.toml 一字不动')
      .not.toMatch(/^\s*\[profile[.\]]/m);
  });
});

describe('Android frozen arm64 diagnostic consumer routing', () => {
  it('nonempty selectors reach admission, while empty defaults keep release routing', () => {
    const android = workflow('android.yml');
    const check = jobBlock(android, 'release_check');
    const expression = check.match(/^ {4}if: (.+)$/m)?.[1];
    expect(expression).toBe("inputs.publish_release != true || (inputs.hot_consumer_candidate || '') != ''");
    if (typeof expression !== 'string') throw new Error('Android candidate job condition missing');
    const evaluate = (source: string, selected: string | undefined, publish: boolean): boolean => {
      const result: unknown = runInNewContext(source, {
        inputs: { hot_consumer_candidate: selected, publish_release: publish },
      });
      if (typeof result !== 'boolean') throw new Error('Android route did not return a boolean');
      return result;
    };
    for (const selected of [undefined, '', 'bad', ' ', 'a'.repeat(40)]) {
      for (const publish of [false, true]) {
        expect(evaluate(expression, selected, publish)).toBe(!publish || !!selected);
        for (const name of ['Build unsigned release-profile validation APK',
          'Verify S5 Android network observation and admission unit tests',
          'Prepare formal mobile source and isolated tools',
          'Select APK build weight (release profile vs risk-gate light)', 'Setup Rust cache']) {
          const guard = stepBlock(check, name).match(/^ {8}if: (.+)$/m)?.[1];
          expect(guard).toBe("(inputs.hot_consumer_candidate || '') == ''");
          if (typeof guard !== 'string') throw new Error('Exact Android isolation guard missing');
          expect(evaluate(guard, selected, publish)).toBe(!selected);
        }
      }
    }
    const admission = stepBlock(check, 'Admit exact Android consumer candidate');
    expect(admission).toContain('HOT_CONSUMER_INPUTS: ${{ toJSON(inputs) }}');
    expect(admission).toContain('run: python3 scripts/mobile-core/ci-hot-candidate.py admit');
    expect(check.indexOf(admission)).toBeLessThan(check.indexOf('uses: actions/setup-node@v7'));
    expect(check).toContain('run: python3 scripts/mobile-core/test-ci-hot-candidate.py');
    expect(android).not.toContain('github.event.inputs.hot_consumer_candidate');
    expect(android).not.toContain('steps.hot_candidate.outputs.mode');
    const publish = jobBlock(android, 'release-apk');
    expect(publish).not.toContain('ci-hot-candidate.py build');
    expect(publish).toContain('ci-hot-candidate.py admit');
    const evidence = stepBlock(check, 'Retain Android candidate evidence only');
    expect(evidence).toContain("if: always() && (inputs.hot_consumer_candidate || '') != '' && steps.hot_candidate.outcome == 'success'");
    expect(evidence).not.toMatch(/\.(aar|apk|aab|jks)\b/);
    expect(evidence).toContain('if-no-files-found: error');
  });
});
