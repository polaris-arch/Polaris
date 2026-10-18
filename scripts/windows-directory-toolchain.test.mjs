import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
test('Windows directory compiler selection reuses source parsing and preserves formal producer facts', () => {
  const result = spawnSync(process.platform === 'win32' ? 'python' : 'python3',
    ['-B', 'scripts/windows-directory/test-toolchain.py'], {
      cwd: root, encoding: 'utf8', timeout: 30_000,
      env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' },
    });
  assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`);
  assert.match(result.stderr, /Ran 7 tests/);
  const ci = readFileSync(join(root, '.github/workflows/ci.yml'), 'utf8');
  assert.equal(ci.split('python scripts/windows-directory/resolve-toolchain.py').length - 1, 2);
  assert.doesNotMatch(ci, /Expected one exact (?:kernel )?source toolchain pin/);
  const producer = readFileSync(join(root, 'scripts/windows-directory/Build-And-Test.ps1'), 'utf8');
  assert.match(producer, /--mode \$mode --source \$source --actual-go \$actualGo/);
  assert.match(producer, /toolchain-evidence.json/);
  assert.match(ci, /GOTOOLCHAIN: local/);
  // The ordinary candidate route is explicit and shares only the explicitly approved
  // two-batch SYSTEM scope. Formal material and test requirements stay put.
  const selection = ci.split('      - name: Read explicit reviewed batch PR candidate before native compilation\n')[1].split('      - name:')[0];
  const expression = selection.match(/if: >-\s*\$\{\{([\s\S]*?)\}\}/)?.[1];
  assert.ok(expression);
  const evaluate = new Function('github', `return (${expression});`);
  for (const event_name of ['pull_request', 'push', 'workflow_dispatch'])
    for (const repository of ['polaris-arch/Polaris', 'other/Polaris'])
      for (const full_name of ['polaris-arch/Polaris', 'other/Polaris'])
        for (const number of [14, 15, 16])
          for (const ref of ['collab/dots-audit-remediation-20261017', 'collab/apple-evidence-reuse-20261018', 'main', 'collab/apple-evidence-reuse-20261018-extra']) {
            const expected = event_name === 'pull_request' && repository === 'polaris-arch/Polaris'
              && full_name === repository && (ref === 'collab/dots-audit-remediation-20261017'
                || (number === 15 && ref === 'collab/apple-evidence-reuse-20261018'));
            assert.equal(evaluate({ event_name, repository, event: { pull_request: { number, head: { ref, repo: { full_name } } } } }), expected);
          }
  assert.match(selection, /data\['sourceCommit'\] != '68ffb209ded1f9337f9115741cba24f9f4c63421'/);
  assert.match(selection, /mode=candidate/);
  for (const name of ['Approved batch SYSTEM producer — fresh', 'Approved batch SYSTEM producer — prebuilt-user']) {
    const block = ci.split(`      - name: ${name}\n`)[1].split('        shell:')[0];
    const systemExpression = block.match(/if: >-\s*\$\{\{([\s\S]*?)\}\}/)?.[1];
    assert.ok(systemExpression);
    assert.doesNotMatch(systemExpression, /always\(|failure\(|cancelled\(/);
    const systemEvaluate = new Function('runner', 'github', 'steps', `return (${systemExpression});`);
    for (const os of ['Windows', 'Linux', 'macOS'])
      for (const event_name of ['pull_request', 'push', 'workflow_dispatch'])
        for (const repository of ['polaris-arch/Polaris', 'other/Polaris'])
          for (const full_name of ['polaris-arch/Polaris', 'other/Polaris'])
            for (const number of [14, 15, 16])
              for (const ref of ['collab/dots-audit-remediation-20261017', 'collab/apple-evidence-reuse-20261018', 'main', 'collab/apple-evidence-reuse-20261018-extra'])
                for (const mode of ['candidate', 'formal', '']) {
                  const expected = os === 'Windows' && mode === 'candidate'
                    && event_name === 'pull_request' && repository === 'polaris-arch/Polaris'
                    && full_name === repository && (ref === 'collab/dots-audit-remediation-20261017'
                      || (number === 15 && ref === 'collab/apple-evidence-reuse-20261018'));
                  assert.equal(systemEvaluate({ os }, { event_name, repository,
                    event: { pull_request: { number, head: { ref, repo: { full_name } } } } },
                  { windows_directory_source: { outputs: { mode } } }), expected);
                }
  }

});
