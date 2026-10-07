/**
 * package(all) → reusable CI 的矩阵入口契约。
 *
 * GitHub workflow_call 会继承外层 workflow_dispatch 的 event_name/payload。package.yml 的输入叫
 * `platform`，没有 ci.yml 自身手动入口的 `os`；因此 package(all) 调 CI 时
 * `github.event.inputs.os === ''`。空串若只经过 `!= 'all'` 判定，会被误当成具体平台并生成 `[""]`，
 * 最终是 `runs-on: ""` / labels=[] 的永久 pending job（run 32357370395 的真实失败形态）。
 */

import { describe, expect, it, onTestFinished } from 'vitest';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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
function evaluateActions(expression: string, github: { event_name: string; ref: string; os: string }): unknown {
  return runInNewContext(expression, {
    github: { event_name: github.event_name, ref: github.ref, event: { inputs: { os: github.os } } },
    fromJSON: (text: string) => JSON.parse(text),
    format: (template: string, value: string) => template.replace('{0}', value),
  });
}

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

  it('ci.yml 三个并行 job：Linux 专属检查恰在 ubuntu 腿存在时运行，且没有任何一个 job 能被静默关掉', () => {
    // 背景：2026-10-07 之前交叉 clippy / Android dep-info 对差 / 豁免反腐烂是矩阵 job 里
    // `if: runner.os == 'Linux'` 的三步 —— 「ubuntu 腿在跑」⇔「这三步在跑」由构造保证。
    // 独立成 `cross` job 后这个等价关系要靠 job 级 `if` 维持，而 android-impact-coverage-contract
    // 的 D2 / D4 只查**步级** `if` / continue-on-error：job 级写一个 `if: false` 它们全绿。
    // 本条补的就是这条新路径。
    const ci = workflow('ci.yml');
    const jobs = [...ci.slice(ci.indexOf('\njobs:\n')).matchAll(/^ {2}([a-z][a-z0-9_-]*):$/gm)].map((m) => m[1]);
    expect(jobs, 'ci.yml 的 job 集合变了 —— 新 job 要有人过目：它的缓存键、它能不能被关掉').toEqual([
      'lint', 'cross', 'test',
    ]);

    const lint = executable(jobBlock(ci, 'lint'));
    const cross = executable(jobBlock(ci, 'cross'));
    const test = executable(jobBlock(ci, 'test'));

    // ① 归属：每条门恰好住在一个 job 里（检查类与构建测试类分开；Linux 专属三步都在 cross）。
    const home: [string, string][] = [
      ['- name: Check formatting', 'lint'],
      ['- name: Clippy (deny warnings)', 'lint'],
      ['- name: Rustdoc documentation invariants (deny four lints)', 'lint'],
      ['- name: Resolve core-owned Cronet dependencies', 'lint'],
      ['- name: Cross-check platform targets', 'cross'],
      ['- name: Android impact face must be registered', 'cross'],
      ['- name: Cross-target exemptions must still be necessary', 'cross'],
      ['- name: Build\n', 'test'],
      ['- name: Test\n', 'test'],
    ];
    const blocks = { lint, cross, test };
    for (const [needle, owner] of home) {
      for (const [name, block] of Object.entries(blocks)) {
        expect(block.includes(needle), `「${needle.trim()}」应只在 ${owner} job 里（现在查的是 ${name}）`)
          .toBe(name === owner);
      }
    }

    // ② 矩阵 job 没有 job 级 if / continue-on-error；cross 只有那一条 dispatch 单平台条件。
    for (const [name, block] of [['lint', lint], ['test', test]] as const) {
      expect(block, `${name} job 出现了 job 级 if —— 它可以被整个关掉而步级判据全绿`).not.toMatch(/^ {4}if:/m);
      expect(block).toMatch(/^ {4}runs-on: \$\{\{ matrix\.os \}\}$/m);
    }
    for (const [name, block] of Object.entries(blocks)) {
      expect(block, `${name} job 挂了 continue-on-error —— 红了也不拦合入`).not.toMatch(/^ {4}continue-on-error:/m);
    }
    expect(cross).toMatch(/^ {4}runs-on: ubuntu-22\.04$/m);

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

  it('Rust 缓存键：每个 job 恰一个 rust-cache，键各自独立且是稳定字面量', () => {
    // 三个 job 的 target/ 内容互不通用（clippy metadata / 交叉 check 单元 / dev 全量 codegen）。
    // 共用键 = 谁最后跑完谁覆盖，另外两个下次恢复到的是别人的产物 —— 等于次次冷编。
    const keysOf = (block: string) =>
      [...block.matchAll(/uses: Swatinem\/rust-cache@v2\n {8}with:\n(?: {10}.*\n)*? {10}key: (.+)\n/g)].map((m) => m[1]);
    const ci = workflow('ci.yml');
    const ciKeys = ['lint', 'cross', 'test'].map((name) => {
      const block = executable(jobBlock(ci, name));
      expect(block.match(/uses: Swatinem\/rust-cache@v2/g) ?? [], `${name} job 的 rust-cache 步数`).toHaveLength(1);
      const keys = keysOf(block);
      expect(keys, `${name} job 的 rust-cache 没有显式 key`).toHaveLength(1);
      return keys[0];
    });
    // 逐字钉住：键是字面量（不含 run_id / sha 之类每次都变的东西 ⇒ 稳定），且三者互不相同。
    expect(ciKeys).toEqual(['lint', 'cross', 'test']);
    expect(ci.match(/uses: Swatinem\/rust-cache@v2/g) ?? [], 'ci.yml 有不属于这三个 job 的 rust-cache').toHaveLength(3);

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
    expect(workflow('ci.yml')).toContain(
      'group: ci-${{ github.workflow }}-${{ github.ref }}',
    );
    expect(workflow('ui.yml')).toContain(
      'group: ui-${{ github.workflow }}-${{ github.ref }}',
    );
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
    expect(androidWorkflow).toContain(
      'group: android-${{ github.workflow }}-${{ github.ref }}',
    );
    expect(workflow('package.yml')).toContain(
      "group: package-${{ github.ref }}-${{ inputs.platforms || inputs.platform || github.event.inputs.platform || 'all' }}",
    );
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
      'python3 scripts/libbox-patches/verify-receipt.py --apk src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release-unsigned.apk --abi arm64-v8a --r8 src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    expect(releaseCheck).toContain(
      'node scripts/assert-r8-evidence.mjs src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    // R8 入口随 source-consumption 收敛到 verifier；守住实际委托、必填证据和失败传播。
    const verifier = readFileSync(join(REPO_ROOT, 'scripts/libbox-patches/verify-receipt.py'), 'utf8');
    const consumption = verifier.slice(verifier.indexOf('def consumption('), verifier.indexOf('\ndef main('));
    expect(consumption).toContain("builder.run(['node', str(ROOT / 'scripts/assert-r8-evidence.mjs'), str(r8)], cwd=ROOT)");
    expect(consumption).toContain("builder.run(['node', str(ROOT / 'scripts/verify-apk.mjs'), str(apk), '--abi', abi], cwd=ROOT)");
    const beforePackage = consumption.slice(0, consumption.indexOf('    with zipfile.ZipFile('));
    expect(beforePackage).not.toMatch(/^\s*try:/m);
    expect(consumption).not.toMatch(/^\s*except\b/m);
    expect(verifier).toContain("builder.require(args.r8 is not None, 'APK source consumption requires actual R8 evidence')");
    expect(verifier).toContain('consumption(args.apk.resolve(), args.abi, args.r8.resolve(), receipt, receipt_path)');
    expect(verifier).toMatch(/except \([^\n]*subprocess\.CalledProcessError\) as error:\n\s*print\([^\n]*\n\s*sys\.exit\(1\)/);
    const builder = readFileSync(join(REPO_ROOT, 'scripts/libbox-patches/build.py'), 'utf8');
    const run = builder.slice(builder.indexOf('def run('), builder.indexOf('\ndef require('));
    expect(run).toContain('return subprocess.run(args, cwd=cwd, env=env, check=True, text=True,');
    expect(run).not.toMatch(/^\s*(?:try:|except\b)/m);
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
      'run: node scripts/assert-r8-evidence.mjs src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    expect(publish).toContain('name: android-release-mapping');
    expect(publish).toContain(
      'node scripts/verify-apk.mjs ${{ steps.asset.outputs.asset }} --abi arm64-v8a',
    );
    expect(publish).toContain(
      'python3 scripts/libbox-patches/verify-receipt.py --apk ${{ steps.asset.outputs.asset }} --abi arm64-v8a --r8 src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    expect(publish).toContain('expected_sha="${{ steps.asset.outputs.sha256 }}"');
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
        mkdirSync(tools, { recursive: true });
        writeFileSync(join(app, 'build.gradle.kts'), 'android { compileSdk = 36 }\n');
        if (properties !== null) {
          writeFileSync(join(app, 'tauri.properties'), properties);
        }
        const bytes = 'synthetic APK bytes; no Android build';
        writeFileSync(join(apkDir, 'app-arm64-release.apk'), bytes);
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
        const renamed = join(fixture, 'polaris-1.0.0-android-arm64.apk');
        expect(existsSync(renamed)).toBe(expected === 0);
        if (expected === 0) {
          expect(readFileSync(renamed, 'utf8')).toBe(bytes);
          expect(readFileSync(output, 'utf8')).toContain('name=polaris-1.0.0-android-arm64.apk\n');
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
      android.slice(android.indexOf("          row=''")),
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
      const name = index === 0 ? 'polaris-1.0.0-android-arm64.apk' : 'polaris-1.0.0-linux-x64.AppImage';
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
          expected_sha: sha, android_asset: 'polaris-1.0.0-android-arm64.apk', tag: 'v1.0.0' },
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
    expect(release).toContain('android_asset=polaris-${tag#v}-android-arm64.apk');
    expect(promote).toContain(
      'VERIFIED_ANDROID_SHA256: ${{ needs.android_release.outputs.signed_apk_sha256 }}',
    );
    expect(promote).toContain(
      'ANDROID_ASSET: ${{ steps.release_identity.outputs.android_asset }}',
    );
    expect(release).toContain('[ "$downloaded_sha" = "$VERIFIED_ANDROID_SHA256" ]');
    expect(release).toContain('[ "$manifest_rows" != "$VERIFIED_ANDROID_SHA256" ]');
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
  const STEP = 'Select installer build weight (release profile vs risk-gate light)';
  const ALLOWED = ['CARGO_PROFILE_RELEASE_CODEGEN_UNITS', 'CARGO_PROFILE_RELEASE_LTO'];

  /** 取出那一步的 shell 脚本本体（去掉 YAML 的 10 格缩进）。 */
  function weightScript(pkg: string): string {
    const step = stepBlock(jobBlock(pkg, 'package'), STEP);
    const runAt = step.indexOf('\n        run: |\n');
    if (runAt < 0) throw new Error('构建权重步没有 run 块 —— 取材面塌了');
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
      // 从宿主环境里剔掉任何 CARGO_PROFILE_*：脚本的第一道闸就是查它，宿主恰好设了会让每条用例都红。
      const base = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('CARGO_PROFILE_')));
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

  /** 整张入口表。任何一格不符即抛 —— 下面的变异对照靠它抛。 */
  function checkWeight(script: string) {
    const cases: [string, Record<string, string>, number, string][] = [
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
    ];
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

  it('构建权重步按各入口真跑：发行路径零写入，风险门恰写两个键，三条 fail-closed 都咬得住', () => {
    const script = weightScript(workflow('package.yml'));
    // 切片自检：拿到的是脚本本体，且没有把下一步卷进来。
    expect(script).toContain('set -euo pipefail');
    expect(script).not.toContain('- name:');
    expect(script, '切片把下一步上方的注释卷进来了').not.toContain('D7');
    expect(script.trimEnd().endsWith('>> "$GITHUB_STEP_SUMMARY"'), '切片没有落在脚本最后一行').toBe(true);
    checkWeight(script);

    // 变异对照：把每一道闸各拆一次，整张表必须红（否则上面的绿没有信息量）。
    const mutations: [string, string, string][] = [
      ['拆掉 tag 闸', 'refs/tags/*) echo "::error::轻量构型不得用于 tag（$GITHUB_REF）：tag 上的产物是发布物"; exit 1 ;;', 'refs/tags/*) ;;'],
      ['拆掉上传闸', '[ "$POLARIS_UPLOAD_ARTIFACTS" = 0 ] || {', 'true || {'],
      ['拆掉环境泄漏闸', 'if [ -n "$leaked" ]; then', 'if false; then'],
      ['非法取值落到发行构型', "*) echo \"::error::POLARIS_LIGHT_BUILD 取值非法：'$POLARIS_LIGHT_BUILD'\"; exit 1 ;;", '*) exit 0 ;;'],
      ['发行分支不再提前返回（会一路走到写覆盖）', 'exit 0 ;;', ';;'],
      ['多写一个键', "  echo 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16'\n", "  echo 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16'\n  echo 'CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true'\n"],
    ];
    for (const [name, from, to] of mutations) {
      const mutated = script.replace(from, to);
      expect(mutated, `变异「${name}」没打上 —— 针与脚本原文不符，这条对照是空的`).not.toBe(script);
      expect(() => checkWeight(mutated), `变异「${name}」没有让入口表转红`).toThrow();
    }
  });

  it('开关只认调用方显式传入的 true，且全仓只有 release-risk 的打包腿在传', () => {
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

    // 传 true 的调用点全仓恰一处，就在 release-risk 的打包腿上，且与「不上传」成对。
    const all = readdirSync(join(REPO_ROOT, '.github/workflows'))
      .filter((name) => /\.ya?ml$/.test(name))
      .map((name) => executable(workflow(name)));
    expect(all.length, '工作流目录一个文件都没读到 —— 取材面塌了').toBeGreaterThan(4);
    expect(all.join('\n').match(/light_build:\s*true/g) ?? []).toHaveLength(1);
    const riskPackage = executable(jobBlock(workflow('release-risk.yml'), 'package'));
    expect(riskPackage).toContain('uses: ./.github/workflows/package.yml');
    expect(riskPackage).toContain('light_build: true');
    expect(riskPackage).toContain('upload_artifacts: false');
  });

  it('cargo profile 的环境变量覆盖只有这一处入口，且排在缓存与两次构建之前', () => {
    // 这是一条绕开 `release_escape_hatches.rs` 的路径：那边的发行构型断言（lto = "fat" /
    // codegen-units = 1 / 不得开 debug-assertions / 不得 panic = "abort"）只读 Cargo.toml，
    // 而 `CARGO_PROFILE_RELEASE_*` 环境变量能改写其中任何一条且不动任何文件。
    // 故把入口钉死：全部 workflow 里只有构建权重步提到它，且只有这两个键。
    const names = readdirSync(join(REPO_ROOT, '.github/workflows')).filter((name) => /\.ya?ml$/.test(name));
    expect(names).toContain('package.yml');
    expect(names).toContain('android.yml');
    const pattern = /CARGO_PROFILE_[A-Z][A-Z_]*/g;
    for (const name of names) {
      const hits = executable(workflow(name)).match(pattern) ?? [];
      if (name === 'package.yml') {
        expect([...new Set(hits)].sort(), 'package.yml 里出现了两个放行键之外的 profile 覆盖').toEqual(ALLOWED);
      } else {
        expect(hits, `${name} 出现了 cargo profile 环境变量覆盖 —— 发行构型的源码门看不见它`).toEqual([]);
      }
    }
    const pkg = workflow('package.yml');
    const pkgJob = executable(jobBlock(pkg, 'package'));
    const step = executable(stepBlock(jobBlock(pkg, 'package'), STEP));
    const count = (text: string) => (text.match(/CARGO_PROFILE_/g) ?? []).length;
    expect(count(step), '构建权重步里一处 CARGO_PROFILE_ 都没有 —— 取材面塌了').toBeGreaterThan(2);
    expect(count(executable(pkg)), 'CARGO_PROFILE_ 出现在构建权重步之外').toBe(count(step));
    // cargo 的 config 文件同样能写 [profile.*]，与环境变量是同一类旁路。
    const cargoConfig = executable(readFileSync(join(REPO_ROOT, '.cargo/config.toml'), 'utf8'));
    expect(cargoConfig).toContain('[target.x86_64-pc-windows-msvc]');
    expect(cargoConfig, '.cargo/config.toml 出现了 [profile.*] —— 它会改写发行构型而 Cargo.toml 一字不动')
      .not.toMatch(/^\s*\[profile[.\]]/m);

    // 顺序：权重先定，再恢复缓存（键靠它分开），再编 helper 与 app（两次 cargo 构建同吃一份构型）。
    const at = (needle: string) => {
      const index = pkgJob.indexOf(needle);
      expect(index, `package job 里找不到「${needle}」`).toBeGreaterThanOrEqual(0);
      return index;
    };
    const weight = at(`- name: ${STEP}\n`);
    expect(weight).toBeLessThan(at('- name: Setup Rust cache\n'));
    expect(at('- name: Setup Rust cache\n')).toBeLessThan(at('- name: Build and stage privileged helper\n'));
    expect(at('- name: Build and stage privileged helper\n')).toBeLessThan(at('- name: Build installers\n'));
    // 这一步自己不能被关掉：没有步级 if / continue-on-error（发行路径上它是「环境里没有覆盖」的唯一检查）。
    expect(step).not.toMatch(/^ {8}(?:if|continue-on-error):/m);
  });
});
