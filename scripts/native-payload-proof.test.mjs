// Static contracts and synthetic OS/path fixtures only. No real kernel/loader.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
const root = join(dirname(fileURLToPath(import.meta.url)), '..');
test('native/final payload contracts and no-local-execution rejection', () => {
  execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B', 'scripts/native-payload-proof.test.py'], {
    cwd: root, env: { ...process.env, POLARIS_NO_KERNEL_RUN: '1' }, stdio: 'pipe', timeout: 30000 });
});
test('validation-only native and actual final-payload steps preserve production release policy', () => {
  const risk = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');
  const pkg = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
  const producer = readFileSync(join(root, '.github/workflows/desktop-core.yml'), 'utf8');
  assert.match(risk, /options: \[ordinary, native-payload\]/);
  assert.match(risk, /native_payload:[\s\S]*?run_kernel_gates: true[\s\S]*?light_build: false/);
  assert.match(risk, /NATIVE_VALIDATION_RESULT.*needs\.native_payload\.result/);
  assert.match(producer, /Retrieve reviewed unchanged native origins/);
  assert.match(producer, /--assemble --validation-origin/);
  assert.match(pkg, /Verify actual installed NSIS and final portable payloads/);
  assert.match(pkg, /--kind nsis[\s\S]*?--kind zip/);
  assert.match(pkg, /--kind deb[\s\S]*?--kind AppImage/);
  assert.match(pkg, /--test core_build_matrix/);
  assert.match(pkg, /startsWith\(github.ref, 'refs\/tags\/v'\)/);
  const final = readFileSync(join(root, 'scripts/final-package-receipt.py'), 'utf8');
  assert.doesNotMatch(final, /subprocess\.(?:run|Popen)\([^\n]*(?:uninstall|runas|cleaner)/i);
});


function workflowJob(text, name) {
  const match = new RegExp(`^  ${name}:\\n([\\s\\S]*?)(?=^  [a-z_]+:|$(?![\\s\\S]))`, 'm').exec(text);
  assert.ok(match, `missing job ${name}`); return match[1];
}
function requireOriginReadPermissions(risk, desktop) {
  const expected = '    permissions:\n      contents: read\n      actions: read\n';
  assert.ok(workflowJob(risk, 'desktop_core').includes(expected), 'caller must allow contents/actions read');
  assert.ok(workflowJob(desktop, 'assemble').includes(expected), 'assembler must request contents/actions read');
  assert.doesNotMatch(workflowJob(desktop, 'produce'), /actions:\s*(?:read|write)/, 'producer needs no extra Actions permission');
  assert.doesNotMatch(workflowJob(risk, 'desktop_core'), /:\s*write\b/);
  assert.doesNotMatch(workflowJob(desktop, 'assemble'), /:\s*write\b/);
}
test('original artifact API read permissions are explicit at caller and assembler only', () => {
  const risk = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');
  const desktop = readFileSync(join(root, '.github/workflows/desktop-core.yml'), 'utf8');
  requireOriginReadPermissions(risk, desktop);
  const block = '    permissions:\n      contents: read\n      actions: read\n';
  for (const wrong of [block.replace('      actions: read\n', ''), block.replace('actions: read', 'actions: none'),
    block.replace('actions: read', 'actions: write'), block.replace('contents: read', 'contents: write')]) {
    assert.throws(() => requireOriginReadPermissions(risk.replace(block, wrong), desktop));
    assert.throws(() => requireOriginReadPermissions(risk, desktop.replace(block, wrong)));
  }
  assert.throws(() => requireOriginReadPermissions(risk,
    desktop.replace('  produce:\n', '  produce:\n    permissions:\n      contents: read\n      actions: read\n')));
});


function requireNestedReadCeilings(risk, pkg) {
  const readBlock = '    permissions:\n      contents: read\n      actions: read\n';
  assert.ok(workflowJob(pkg, 'desktop_core').includes(readBlock));
  for (const name of ['native_payload', 'package']) {
    const caller = workflowJob(risk, name);
    assert.match(caller, /uses: \.\/\.github\/workflows\/package\.yml/);
    assert.match(caller, /^      actions: read(?:\s*#.*)?$/m);
    assert.doesNotMatch(caller, /^      actions: write$/m);
  }
}
test('every nested Package edge preserves the assembler Actions read ceiling', () => {
  const risk = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');
  const pkg = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
  requireNestedReadCeilings(risk, pkg);
  assert.throws(() => requireNestedReadCeilings(risk, pkg.replace('      actions: read\n', '')));
  for (const name of ['native_payload', 'package']) {
    const body = workflowJob(risk, name);
    for (const replacement of ['      actions: none', '      actions: write', '']) {
      const mutant = body.replace(/^      actions: read(?:\s*#.*)?$/m, replacement);
      assert.throws(() => requireNestedReadCeilings(risk.replace(body, mutant), pkg));
    }
  }
});

import { validateCurrentProducer } from './desktop-core/validation-origin.mjs';
import { canonical, digest, producerContext } from './desktop-core/source-graph.mjs';
test('native origin proof compares fingerprinted producer context, including same-candidate old attempts', () => {
  const candidate = 'c'.repeat(40), ref = 'refs/heads/collab/fk02-native-payload-validation-20261010';
  const env = { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: 'polaris-arch/Polaris', GITHUB_REPOSITORY_ID: '1347028220',
    GITHUB_SHA: candidate, GITHUB_RUN_ID: '12345', GITHUB_RUN_ATTEMPT: '2', GITHUB_REF: ref,
    GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_WORKFLOW_REF: `polaris-arch/Polaris/.github/workflows/release-risk.yml@${ref}` };
  const sign = (facts) => ({ ...facts, fingerprint: digest(canonical(facts)) });
  const facts = { schema: 'polaris-desktop-core-v1', candidate, producerContext: producerContext(candidate, env) };
  validateCurrentProducer(sign(facts), candidate, env);
  for (const field of ['runId', 'runAttempt', 'repository', 'repositoryId', 'workflowRef', 'workflowHead', 'ref', 'event', 'candidate']) {
    const stale = sign({ ...facts, producerContext: { ...facts.producerContext, [field]: 'wrong' } });
    assert.throws(() => validateCurrentProducer(stale, candidate, env), /producer context/, field);
  }
  assert.throws(() => validateCurrentProducer(sign({ ...facts, schema: 'polaris-dns-technical-native-v1' }), candidate, env));
  assert.throws(() => validateCurrentProducer(sign({ schema: 'polaris-desktop-core-v1', candidate }), candidate, env));
  const input = readFileSync(join(root, 'scripts/native-payload-input.mjs'), 'utf8');
  assert.match(input, /sourceOwnedReplacements: receipt.sourceOwnedReplacements/);
  assert.match(input, /producerContext: receipt.producerContext/);
});
