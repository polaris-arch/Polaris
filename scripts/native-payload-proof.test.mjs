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
