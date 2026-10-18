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
  assert.match(result.stderr, /Ran 8 tests/);
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
    const systemEvaluate = new Function('runner', 'github', 'steps', 'inputs', `return (${systemExpression});`);
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
                  { windows_directory_source: { outputs: { mode } }, windows_directory_acceptance: { outcome: 'success' } },
                  { windows_system_candidate: '' }), expected);
                }
    const exact = 'a'.repeat(40);
    for (const event_name of ['workflow_dispatch', 'push', 'pull_request'])
      for (const ref of ['refs/heads/main', 'refs/heads/ci/governance', 'refs/heads/unapproved', 'refs/tags/v1.0.0'])
        for (const request of ['', exact, 'b'.repeat(40)])
          for (const mode of ['formal', 'candidate'])
            for (const outcome of ['success', 'failure'])
              for (const diagnostic of [false, true]) {
                const expected = event_name === 'workflow_dispatch' && ['refs/heads/main', 'refs/heads/ci/governance'].includes(ref)
                  && request === exact && mode === 'formal' && outcome === 'success' && !diagnostic;
                const github = { event_name, ref, sha: exact, repository: 'polaris-arch/Polaris',
                  event: { pull_request: { head: { ref: 'unapproved', repo: { full_name: 'polaris-arch/Polaris' } } } } };
                const steps = { windows_directory_source: { outputs: { mode } }, windows_directory_acceptance: { outcome } };
                const inputs = { windows_system_candidate: request, windows_launcher_diagnostics: diagnostic };
                assert.equal(systemEvaluate({ os: 'Windows' }, github, steps, inputs), expected);
                assert.equal(systemEvaluate({ os: 'Linux' }, github, steps, inputs), false);
                assert.equal(systemEvaluate({ os: 'Windows' }, { ...github, repository: 'other/Polaris' }, steps, inputs), false);
              }
  }

});


test('committed formal directory output reaches both SYSTEM guards and explicit requests require both successes', () => {
  const ci = readFileSync(join(root, '.github/workflows/ci.yml'), 'utf8');
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json'), 'utf8'));
  assert.ok(manifest.desktopRelease);
  const source = ci.split('      - name: Select explicit formal or candidate Windows directory source\n')[1].split('      - name:')[0];
  const formal = source.split("} elseif ($env:POLARIS_NATIVE_DNS_FORMAL -ceq '1') {")[1].split("} elseif ($env:DIRECTORY_MODE -ceq 'candidate') {")[0];
  assert.match(formal, /formalGateContext\(process.cwd\(\)\).kernelSourceCommit/);
  const mode = formal.match(/\$env:DIRECTORY_MODE = '(\w+)'/)[1];
  assert.equal(mode, 'formal');
  assert.match(source, /"mode=\$env:DIRECTORY_MODE" >> \$env:GITHUB_OUTPUT/);
  const sha = 'a'.repeat(40);
  for (const name of ['Approved batch SYSTEM producer — fresh', 'Approved batch SYSTEM producer — prebuilt-user']) {
    const block = ci.split(`      - name: ${name}\n`)[1].split('        shell:')[0];
    const expr = block.match(/if: >-\s*\$\{\{([\s\S]*?)\}\}/)[1];
    const evaluate = new Function('runner', 'github', 'steps', 'inputs', `return (${expr});`);
    assert.equal(evaluate({ os: 'Windows' }, { repository: 'polaris-arch/Polaris', event_name: 'workflow_dispatch', sha,
      ref: 'refs/heads/ci/governance' }, { windows_directory_source: { outputs: { mode } },
      windows_directory_acceptance: { outcome: 'success' } }, { windows_system_candidate: sha }), true);
  }
  const gate = ci.split('      - name: Require both explicitly requested SYSTEM checks\n')[1].split('      - name:')[0];
  assert.match(gate, /always\(\) && runner.os == 'Windows' && inputs.windows_system_candidate != ''/);
  const expr = gate.match(/if \(([^\n]+)\) \{/)[1].replaceAll(/\$env:(\w+)/g, 'env.$1').replaceAll('-cne', '!==').replaceAll('-or', '||');
  const fails = new Function('env', `return (${expr});`);
  assert.match(gate, /throw 'Explicit SYSTEM request requires fresh and prebuilt-user success in this attempt'/);
  for (const fresh of ['success', 'failure', 'cancelled', 'skipped', ''])
    for (const prebuilt of ['success', 'failure', 'cancelled', 'skipped', ''])
      assert.equal(fails({ SYSTEM_FRESH_OUTCOME: fresh, SYSTEM_PREBUILT_OUTCOME: prebuilt }), fresh !== 'success' || prebuilt !== 'success');
});
