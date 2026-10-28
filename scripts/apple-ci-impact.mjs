import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

// Orthogonal to desktop package/Android selection: shared source and final
// iOS link changes require the real unsigned App+PacketTunnel build.
export function appleRequired(paths, full = false) {
  return full || paths.some((path) => {
    if (typeof path !== 'string' || path.startsWith('/') || path.includes('..') || path.includes('\\')) {
      throw new Error('Apple impact requires normalized repository paths');
    }
    return ['Cargo.toml', 'Cargo.lock', '.cargo/config.toml', 'NOTICE', 'LICENSE', 'THIRD-PARTY-LICENSES.md'].includes(path)
      || ['crates/', 'ui/', 'vendor/', 'resources/data/', 'src-tauri/src/', 'src-tauri/capabilities/', 'src-tauri/gen/apple/',
        'scripts/mobile-core/', 'scripts/libbox-patches/'].some((prefix) => path.startsWith(prefix))
      || /^src-tauri\/(?:Cargo\.toml|build\.rs|tauri(?:\.ios)?\.conf\.json)$/.test(path)
      || ['.github/workflows/apple.yml', '.github/workflows/release-risk.yml',
        'scripts/apple-ci-impact.mjs', 'scripts/apple-ci-impact.test.mjs', 'scripts/classify-ci-impact.mjs',
        'scripts/apple-carrier.py', 'scripts/fetch-dashboard.mjs', 'scripts/fetch-protoc.mjs',
        'scripts/lib/extract-zip.mjs', 'scripts/lib/bounded-download.mjs', 'scripts/gate-node-test.sh'].includes(path);
  });
}

// Exact reviewed unittest leaves, not a test_*.py wildcard or a CI-script exemption.
// These files are outside producer.PRODUCER_FILES. The existing Node gate executes
// unittest discovery before classification; native CI (including naming) stays full.
// Production code must not import these leaves. A new dependency requires review
// of this routing contract, just as for the audited Rust test leaf below.
export const MOBILE_UNIT_TEST_FILES = Object.freeze([
  'scripts/mobile-core/test_apple_admission.py',
  'scripts/mobile-core/test_apple_entry.py',
  'scripts/mobile-core/test_apple_handoff.py',
  'scripts/mobile-core/test_apple_merge_reuse.py',
  'scripts/mobile-core/test_apple_progress.py',
  'scripts/mobile-core/test_apple_read_reuse.py',
  'scripts/mobile-core/test_apple_tls_fingerprint.py',
  'scripts/mobile-core/test_delivery_boundaries.py',
  'scripts/mobile-core/test_inputs.py',
  'scripts/mobile-core/test_native_inputs.py',
  'scripts/mobile-core/test_outputs.py',
  'scripts/mobile-core/test_producer.py',
  'scripts/mobile-core/test_source_checkout.py',
  'scripts/mobile-core/test_source_policy.py',
]);
// Bind the small set of producer/discovery/naming owners, not the source tree.
// Owner updates disable this optimization until its boundary is reviewed again.
const MOBILE_UNIT_TEST_OWNERS = Object.freeze([
  ['scripts/mobile-core/producer.py', '100644', '56074f0294450f26ba6ef3934df52fa81d6b4105'],
  ['scripts/mobile-source-unit.test.mjs', '100644', '8cc09d5f633f4f3c2eb6a4246f79644773b5681e'],
  ['scripts/gate-node-test.sh', '100755', '52e873e5cc95361c6723318d398caeb4db442664'],
  ['.github/workflows/release-risk.yml', '100644', '8424964ac3edd11cfe182b852dabae405b7b70ad'],
  ['.github/workflows/ci.yml', '100644', '9407e92d0fbc36897b7915e45511ade217bc39fa'],
  ['src-tauri/tests/test_source_anchors.rs', '100644', '18667d599e46b5fb5cc354642a4bd86528419524'],
]);

export function mobileUnitTestsOnlyForReleaseRisk(paths, env, event, run = execFileSync) {
  if (!paths.length || new Set(paths).size !== paths.length
      || paths.some(path => !MOBILE_UNIT_TEST_FILES.includes(path))) return false;
  if (env.GITHUB_EVENT_NAME !== 'push' || env.GITHUB_REF !== 'refs/heads/main'
      || env.GITHUB_WORKFLOW !== 'Release Risk'
      || env.GITHUB_WORKFLOW_REF !== `${env.GITHUB_REPOSITORY}/.github/workflows/release-risk.yml@refs/heads/main`)
    return false;
  if (!event || event.forced !== false || event.created !== false || event.deleted !== false) return false;
  const sha = /^[0-9a-f]{40}$/;
  if (!sha.test(event.before) || !sha.test(event.after) || /^0+$/.test(event.before)
      || event.after !== env.GITHUB_SHA || event.before === event.after) return false;
  try {
    const options = { encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 };
    if (run('git', ['rev-parse', 'HEAD'], options).trim() !== event.after) return false;
    run('git', ['cat-file', '-e', `${event.before}^{commit}`], options);
    run('git', ['merge-base', '--is-ancestor', event.before, event.after], options);
    const diff = run('git', ['diff', '--raw', '--abbrev=40', '--no-renames', '-z', event.before, event.after, '--'], options);
    if (!diff || !diff.endsWith('\0')) return false;
    const fields = diff.slice(0, -1).split('\0');
    if (fields.length !== paths.length * 2) return false;
    const remaining = new Set(paths);
    for (let i = 0; i < fields.length; i += 2) {
      const row = /^:100644 100644 ([0-9a-f]{40}) ([0-9a-f]{40}) M$/.exec(fields[i]);
      if (!row || row[1] === row[2] || [row[1], row[2]].some(oid => /^0+$/.test(oid))
          || !remaining.delete(fields[i + 1])) return false;
    }
    if (remaining.size) return false;
    for (const ref of [event.before, event.after]) {
      for (const [path, mode, blob] of MOBILE_UNIT_TEST_OWNERS) {
        if (run('git', ['ls-tree', '-z', ref, '--', path], options) !== `${mode} blob ${blob}\t${path}\0`)
          return false;
      }
    }
    return true;
  } catch {
    return false; // Unknown history, modes, owners or oversized output retain builds.
  }
}

// Reviewed single-leaf contract, not a general Rust test-path rule. The owner
// blob binds its actual #[cfg(test)] module declaration. Code review must keep
// production include!/build.rs inputs independent of this test leaf; any new
// such dependency must update this routing contract. A changed owner falls back.
export const APPLE_TEST_LEAF = 'src-tauri/src/runtime/proxy/mesh_apply/candidate/check/tests/mod.rs';
const TEST_OWNER = 'src-tauri/src/runtime/proxy/mesh_apply/candidate/check.rs';
const TEST_OWNER_BLOB = '5c38100fa91a246ed00606832af675b4684c14ea';
const LINUX_TEST_BOUNDARY = '#[cfg(all(target_os = "linux", target_arch = "x86_64"))]\nmod linux_tests {\n';

export function appleRequiredForReleaseRisk(paths, env, event, run = execFileSync) {
  const required = appleRequired(paths);
  if (required && mobileUnitTestsOnlyForReleaseRisk(paths, env, event, run)) return false;
  if (!required || paths.length !== 1 || paths[0] !== APPLE_TEST_LEAF) return required;
  if (env.GITHUB_EVENT_NAME !== 'push' || env.GITHUB_REF !== 'refs/heads/main'
      || env.GITHUB_WORKFLOW !== 'Release Risk'
      || env.GITHUB_WORKFLOW_REF !== `${env.GITHUB_REPOSITORY}/.github/workflows/release-risk.yml@refs/heads/main`)
    return required;
  if (!event || event.forced !== false || event.created !== false || event.deleted !== false)
    return required;
  const sha = /^[0-9a-f]{40}$/;
  if (!sha.test(event.before) || !sha.test(event.after) || /^0+$/.test(event.before)
      || event.after !== env.GITHUB_SHA || event.before === event.after) return required;
  try {
    const options = { encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 };
    if (run('git', ['rev-parse', 'HEAD'], options).trim() !== event.after) return required;
    run('git', ['cat-file', '-e', `${event.before}^{commit}`], options);
    run('git', ['merge-base', '--is-ancestor', event.before, event.after], options);
    const diff = run('git', ['diff', '--raw', '--abbrev=40', '--no-renames', '-z', event.before, event.after, '--'], options);
    // This registered leaf already has mode 100755. Only identical regular-file
    // modes and a single existing M are admitted, never A/D/R/T/chmod or mixed edits.
    const fields = diff.split('\0');
    const row = /^:100755 100755 ([0-9a-f]{40}) ([0-9a-f]{40}) M$/.exec(fields[0]);
    if (fields.length !== 3 || fields[1] !== APPLE_TEST_LEAF || fields[2] !== '' || !row
        || row[1] === row[2] || [row[1], row[2]].some(oid => /^0+$/.test(oid))) return required;
    for (const [ref, blob] of [[event.before, row[1]], [event.after, row[2]]]) {
      if (run('git', ['ls-tree', '-z', ref, '--', TEST_OWNER], options)
          !== `100755 blob ${TEST_OWNER_BLOB}\t${TEST_OWNER}\0`)
        return required;
      const content = run('git', ['cat-file', 'blob', blob], options);
      if (!content.startsWith(LINUX_TEST_BOUNDARY)) return required;
    }
    return false; // Linux native tests still run; this is not an Apple test pass.
  } catch {
    return required; // Missing/shallow history, failed commands or oversized input.
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const paths = readFileSync(0, 'utf8').split('\0').filter(Boolean);
  let required = appleRequired(paths, process.argv.includes('--full'));
  if (required && !process.argv.includes('--full') && process.argv.includes('--release-risk')) {
    try {
      required = appleRequiredForReleaseRisk(paths, process.env,
        JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8')));
      if (!required) process.stderr.write('Apple build not selected: audited test-only content; source controls and native Rust tests remain required. No Apple compilation/test pass is claimed.\n');
    } catch { /* Keep the existing conservative path decision. */ }
  }
  console.log(required);
}
