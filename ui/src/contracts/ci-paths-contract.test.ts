/**
 * CI 触发面的**对称契约门**（CI-5，2026-08-18）。
 *
 * # 背景：一个已经咬过人的假门
 *
 * 本仓的判据面**横跨 Rust 与前端两侧**：`lib.rs` 遍历整棵 `ui/src`、`i18n.rs`
 * `include_str!` 五个 locale、`config.rs` 读 App.tsx / TrayMenu.tsx 等前端源码；反向地，
 * 约 30 个前端测试文件读 `src-tauri/` 与 `crates/` 的 Rust 源码
 * 当判据。于是两个 workflow 的 push 过滤器**都不得 ignore 对侧的树**，也没理由 ignore
 * 自己的树——只检查一个方向等于没检查。
 *
 * `ci.yml` 此前 ignore 了 `ui/**`（注释自辩「UI 改动不碰 Rust 链」——错，见上），实证：
 * `0742de0`（G1，纯 ui/ diff）push 后 CI workflow **零 run**。CI-5 删除该条；本门钉死
 * 对称规则不回潮。
 *
 * # 这门抓不到什么（如实登记）
 *
 * - `paths`（白名单）形态：若将来有人把 ignore 改成**白名单**，白名单漏掉对侧树同样造假门。
 *   白名单是会漂的枚举表（CI-5 立项时已弃），真要用请连同本门一起改判据。
 * - **纯 workflow 改动的 push**：`.github/**` 也在两张禁入表里，但本门自己就住在 ui/ 树——
 *   若 ui.yml ignore 掉 `.github/**`，纯 workflow push 连让本门跑起来的触发都没有（执法依赖
 *   触发）。这层极限是声明式过滤器的固有边界。
 * - 端到端触发验证：过滤器是声明式，真触发要等下一次纯 UI push（CI-5 登记为待验）。
 */
import { describe, expect, it } from 'vitest';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import playwrightConfig from '../../playwright.config';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';

const REPO_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

function read(workflow: string): string {
  return readFileSync(join(REPO_ROOT, '.github/workflows', workflow), 'utf8');
}

/** 取 push 段的 paths-ignore 列表条目（无该段 ⇒ 空表 = 不忽略任何路径）。 */
function pushIgnoreEntries(src: string): string[] {
  // `push:` 是 `on:` 下的缩进键，不能按列 0 找；从它起截到下一个同级触发键或 `jobs:`。
  const pushMatch = /^(\s*)push:/m.exec(src);
  if (!pushMatch) return [];
  const pushAt = pushMatch.index;
  const nextTrigger = src
    .slice(pushAt + pushMatch[0].length)
    .search(/\n\s*(pull_request|workflow_dispatch|workflow_call|schedule|jobs):/);
  const pushBlock = src.slice(
    pushAt,
    nextTrigger >= 0 ? pushAt + pushMatch[0].length + nextTrigger : src.length
  );
  const ignAt = pushBlock.indexOf('paths-ignore:');
  if (ignAt < 0) return [];
  const listBlock = pushBlock.slice(ignAt);
  // 同行流式形态（`paths-ignore: ["a", "b"]`）先拆——块式正则只认 `- ` 行，流式会静默逃逸。
  const restOfLine = listBlock.slice('paths-ignore:'.length);
  if (restOfLine.trimStart().startsWith('[')) {
    const arr = restOfLine.slice(restOfLine.indexOf('['), restOfLine.indexOf(']') + 1);
    return [...arr.matchAll(/['"]([^'"]+)['"]/g)].map((q) => q[1]);
  }
  // 词法收宽（F1）：认未引号 / 单引号 / 双引号三种条目形态。
  // 「解析器变哑 ⇒ 恒绿」由调用方的哨兵断言兜（见 KNOWN_*）。
  return [...listBlock.matchAll(/^[ \t]*-[ \t]+(.+?)[ \t]*(?=#|$)/gm)]
    .map((m) => m[1].replace(/^['"]|['"]$/g, ''));
}

/** 哨兵（F1-b）：解析器必须完整读到今天已知的全部条目——解析变哑（空表/漏条）在此显式红。 */
const KNOWN_CI = ['**.md', 'docs/**', 'LICENSE', 'NOTICE'];
const KNOWN_UI = ['**.md', 'docs/**', 'LICENSE', 'NOTICE'];

describe('CI 触发面对称契约（判据面横跨两侧 ⇒ 过滤器不得 ignore 对侧树）', () => {
  it('自动分支运行替换旧运行，手动与固定审计独立分组，tag与复用调用保持保护', () => {
    for (const workflow of ['ci.yml', 'ui.yml']) {
      const source = read(workflow);
      const match = source.match(/^  group: (.+)\n  cancel-in-progress: \$\{\{ (.+) \}\}$/m);
      expect(match).not.toBeNull();
      expect(source).toMatch(/preserve_run:\n\s+description: .+\n\s+type: boolean\n\s+default: false/);
      const context = (event: string, ref: string, preserve: boolean, runId: number) => ({
        github: { event_name: event, ref, workflow: 'fixture-caller', run_id: runId,
          event: { inputs: { preserve_run: preserve ? 'true' : 'false' }, pull_request: {}, merge_group: {} } },
        inputs: { preserve_run: preserve },
        startsWith: (value: string, prefix: string) => value.startsWith(prefix),
        format: (value: string, argument: number) => value.replace('{0}', String(argument)),
      });
      const evaluate = (event: string, ref: string, preserve = false) =>
        runInNewContext(match![2], context(event, ref, preserve, 1));
      expect(evaluate('workflow_dispatch', 'refs/heads/collab/test')).toBe(false);
      expect(evaluate('workflow_dispatch', 'refs/heads/main', true)).toBe(false);
      expect(evaluate('workflow_dispatch', 'refs/tags/v1.0.0')).toBe(false);
      expect(evaluate('pull_request', 'refs/pull/1/merge')).toBe(true);
      expect(evaluate('push', 'refs/heads/main')).toBe(true);
      expect(evaluate('merge_group', 'refs/heads/gh-readonly-queue/main/pr-1')).toBe(false);
      const group = (preserve: boolean, runId: number) => match![1].replace(
        /\$\{\{ (.+?) \}\}/g,
        (_text, expression: string) => String(runInNewContext(expression, context(
          'workflow_dispatch', 'refs/heads/collab/test', preserve, runId))),
      );
      expect(group(false, 1)).not.toBe(group(false, 2));
      expect(group(true, 1)).toBe(group(false, 1));
      expect(group(true, 1)).not.toBe(group(true, 2));
      // A reusable workflow inherits its caller's event and workflow name;
      // github.event_name is not "workflow_call" at runtime.
      const called = (event: string, ref: string, caller: string, runId: number) => {
        const value = context(event, ref, false, runId);
        value.github.workflow = caller;
        return {
          cancels: runInNewContext(match![2], value),
          group: match![1].replace(/\$\{\{ (.+?) \}\}/g,
            (_text, expression: string) => String(runInNewContext(expression, value))),
        };
      };
      const tagPublish = called('push', 'refs/tags/v1.0.0', 'Package', 1);
      expect(tagPublish.cancels).toBe(false);
      expect(tagPublish.group).toBe(called('push', 'refs/tags/v1.0.0', 'Package', 2).group);
      const directTag = called('workflow_dispatch', 'refs/tags/v1.0.0', 'Package', 1);
      expect(directTag.cancels).toBe(false);
      const ordinaryCall = called('workflow_dispatch', 'refs/heads/collab/test', 'Package', 1);
      expect(ordinaryCall.cancels).toBe(false);
      expect(ordinaryCall.group).not.toBe(called('workflow_dispatch', 'refs/heads/collab/test', 'Package', 2).group);
      expect(ordinaryCall.group).not.toBe(group(false, 1));
      for (const [event, ref] of [['pull_request', 'refs/pull/1/merge'], ['push', 'refs/heads/main']]) {
        const automatic = called(event, ref, 'Package', 1);
        expect(automatic.cancels).toBe(true);
        expect(automatic.group).toBe(called(event, ref, 'Package', 2).group);
        expect(automatic.group).not.toBe(called(event, ref, 'CI', 1).group);
      }
      expect(called('pull_request', 'refs/pull/1/merge', 'Release Risk', 1).cancels).toBe(true);
      expect(called('merge_group', 'refs/heads/gh-readonly-queue/main/pr-1', 'Package', 1).cancels).toBe(false);
    }
  });

  it('ci.yml（Rust 门）：不 ignore ui/**（CI-5 正主）、不 ignore 自己的 Rust 树、不 ignore workflow 树', () => {
    const entries = pushIgnoreEntries(read('ci.yml'));
    // 哨兵先行：entries 必须逐字包含今天的已知全集（防解析器哑掉后下面全绿）。
    // 哨兵 = 逐字等值：解析变哑（空表/漏条）在此红；**新增 ignore 条目也在此红**——那是
    // 「过目登记」语义：加条目必须连 KNOWN_CI 一起改，顺手确认它不在禁入表里。
    expect(
      [...entries].sort(),
      'ci.yml 的 ignore 表与哨兵 KNOWN_CI 不等：要么解析器变哑（门在裸奔，先修解析），' +
        '要么有人加了新条目——加之前先过下面的禁入表，并同步 KNOWN_CI',
    ).toEqual([...KNOWN_CI].sort());
    for (const banned of ['ui/**', 'src-tauri/**', 'crates/**', '.github/**']) {
      expect(
        entries.includes(banned),
        `ci.yml 的 paths-ignore 含 ${banned} —— Rust 判据面横跨 ui/（lib.rs 遍历 ui/src、` +
          `i18n.rs include_str! locale），ignore 它就造出「纯 UI 改动 Rust 门零 run」的假门（CI-5 实证形态）`,
      ).toBe(false);
    }
  });

  it('ui.yml（前端门）：不 ignore Rust 树 / workflow 树 / 自己的 ui 树（对称）', () => {
    const entries = pushIgnoreEntries(read('ui.yml'));
    expect(
      [...entries].sort(),
      'ui.yml 的 ignore 表与哨兵 KNOWN_UI 不等：要么解析器变哑（门在裸奔，先修解析），' +
        '要么有人加了新条目——加之前先过下面的禁入表，并同步 KNOWN_UI',
    ).toEqual([...KNOWN_UI].sort());
    for (const banned of ['src-tauri/**', 'crates/**', 'ui/**', '.github/**']) {
      expect(
        entries.includes(banned),
        `ui.yml 的 paths-ignore 含 ${banned} —— 前端约 30 个测试文件读 Rust 源码当判据，` +
          `ignore 对侧就造出反方向的同类假门（ui.yml 头注早写明这条禁令，这里给牙）；` +
          `ignore ui/** 则让 UI 门与本门在纯 UI push 上全部转黑`,
      ).toBe(false);
    }
  });
});


describe('UI evidence ownership across the real installed Playwright runner', () => {
  const require = createRequire(import.meta.url);
  const cli = require.resolve('@playwright/test/cli');
  const runner = require.resolve('@playwright/test');

  it.each([
    ['legacy-shared-root', './test-results', false, false],
    ['isolated-success', playwrightConfig.outputDir, false, true],
    ['isolated-failure', playwrightConfig.outputDir, true, true],
  ] as const)('%s cleans only its own output and preserves sibling producer evidence', (_name, outputDir, fail, survives) => {
    expect(playwrightConfig.outputDir).toBe('./test-results/playwright');
    expect(typeof outputDir).toBe('string');
    const dir = mkdtempSync(join(tmpdir(), 'polaris-ui-evidence-'));
    try {
      const feedback = join(dir, 'test-results', 'feedback', 'synthetic.txt');
      const measurement = join(dir, 'test-results', 'ui01-measurement', 'synthetic.txt');
      for (const file of [feedback, measurement]) {
        mkdirSync(join(file, '..'), { recursive: true });
        writeFileSync(file, 'synthetic prior producer evidence');
      }
      const destination = join(dir, outputDir!);
      mkdirSync(destination, { recursive: true });
      const oldRunner = join(destination, 'stale-runner.txt');
      writeFileSync(oldRunner, 'must be removed by the actual runner');
      writeFileSync(join(dir, 'playwright.config.cjs'), 'module.exports = ' + JSON.stringify({
        testDir: dir, testMatch: 'receipt.spec.cjs', outputDir: destination,
        workers: 1, reporter: 'line', preserveOutput: 'always',
      }));
      // No browser fixture, web server, app, core, network or user data. This
      // executes the installed runner's real pre-run deletion and artifact write.
      writeFileSync(join(dir, 'receipt.spec.cjs'), `
const { test, expect } = require(${JSON.stringify(runner)});
const fs = require('node:fs');
test('synthetic output receipt', async ({}, info) => {
  fs.writeFileSync(info.outputPath('runner-receipt.txt'), 'synthetic current output');
  expect(${JSON.stringify(fail)}).toBe(false);
});
`);
      const result = spawnSync(process.execPath, [cli, 'test', '--config', join(dir, 'playwright.config.cjs')], {
        cwd: dir, encoding: 'utf8', timeout: 8000, maxBuffer: 1024 * 1024,
      });
      expect(result.error, result.stdout + result.stderr).toBeUndefined();
      expect(result.signal).toBeNull();
      expect(result.status, result.stdout + result.stderr).toBe(fail ? 1 : 0);
      expect(result.stdout).toContain(fail ? '1 failed' : '1 passed');
      expect(existsSync(oldRunner)).toBe(false);
      for (const file of [feedback, measurement]) {
        expect(existsSync(file)).toBe(survives);
        if (survives) expect(readFileSync(file, 'utf8')).toBe('synthetic prior producer evidence');
      }
      expect(readdirSync(destination, { recursive: true }).some(p => p.toString().endsWith('runner-receipt.txt'))).toBe(true);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }, 10000);

  it('uploads both producer trees on failure and requires screenshots after successful browser gates', () => {
    const workflow = read('ui.yml');
    const failure = workflow.slice(workflow.indexOf('      - name: Browser form failure evidence'), workflow.indexOf('      - name: Feedback fixture screenshots'));
    const feedback = workflow.slice(workflow.indexOf('      - name: Feedback fixture screenshots'));
    expect(failure).toContain('path: ui/test-results/');
    expect(failure).toContain('if: failure()');
    expect(feedback).toContain('if: always()');
    expect(feedback).toContain('path: ui/test-results/feedback/');
    expect(feedback).toContain("if-no-files-found: ${{ steps.browser_forms.outcome == 'success' && 'error' || 'ignore' }}");
    expect(workflow).toContain('      - name: Browser form round trip (mock IPC, no app or core)\n        id: browser_forms');
    // Status functions are permitted in a step's if, not action inputs. This
    // outcome data comes from the exact browser step, including skipped/failure.
    const expression = /if-no-files-found: \$\{\{ (.+) \}\}/.exec(feedback)![1];
    for (const outcome of ['success', 'failure', 'cancelled', 'skipped', undefined]) {
      const actual = runInNewContext(expression, { steps: { browser_forms: { outcome } } });
      expect(actual).toBe(outcome === 'success' ? 'error' : 'ignore');
    }
    expect(expression).not.toMatch(/\b(success|failure|cancelled|always)\s*\(/);

    for (const block of [failure, feedback]) expect(block).toContain('retention-days: 3');
  });

  it('runs the shared UA lifecycle and both platform retry surfaces in the existing browser gate', () => {
    const workflow = read('ui.yml');
    const browser = workflow.slice(workflow.indexOf('      - name: Browser form round trip'), workflow.indexOf('      - name: Browser form failure evidence'));
    expect(browser).toMatch(/POLARIS_BROWSER_TESTS=1 pnpm exec vitest run[^\n]*src\/hooks\/use-subscription-user-agent\.browser\.test\.tsx[^\n]*--maxWorkers=1/);
  });
});
