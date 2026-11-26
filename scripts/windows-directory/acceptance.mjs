// Native Windows directory acceptance. build-and-test is the fixed private-token child target;
// verify is the CI consumer of the current attempt and never searches receipt-history.
import { execFileSync, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { closeSync, existsSync, lstatSync, mkdirSync, openSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { digest, requireGraph } from '../desktop-core/source-graph.mjs';
import { selectExecutable } from '../run-native-dns-controlled-tests.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const REPOSITORY = 'polaris-arch/sing-box-polaris';
const SOURCE = /^[0-9a-f]{40}$/, SHA256 = /^[0-9a-f]{64}$/, ATTEMPT = /^[0-9a-f]{32}$/;
const COMPONENT_TESTS = ['TestWindowsDirectorySourceContract', 'TestWindowsDirectoryReservedNamespaceWholeConfig'];
const BINDING_TEST = 'TestWindowsDirectoryActivationAndOriginalBinding';
const KERNEL_TESTS = [
  'TestWindowsDirectoryInteropChild',
  'TestWindowsDirectoryCrossProcessWriterAndRetirementExclude',
  'TestWindowsDirectoryCrossProcessRevokeFailureRetainsOldLease',
  'TestWindowsDirectoryCrossProcessCrashReopensSameGeneration',
  'TestWindowsDirectoryCrossProcessCrashCannotUndoRevocation',
  'TestWindowsDirectoryCrossProcessReplacementCannotAdoptOldIdentity',
  'TestWindowsDirectoryCrossProcessDACLRejectsNewOwner',
  'TestWindowsDirectoryCrossProcessOldLeasePoisonsOnDACLChange',
  'TestWindowsDirectoryForeignOwnerRejectsUnverifiedParent',
  'TestWindowsDirectoryForeignOwnerRejectsNonSystemToken',
];
const APP_TESTS = [
  'runtime::windows_directory::tests::interop_writer_retirement_and_stale_intent',
  'runtime::windows_directory::tests::interop_crash_reopen_and_revocation',
  'runtime::windows_directory::tests::interop_replaced_identity_is_retained',
];
const BOUND = ['expected-attempt-id', 'expected-receipt-sha256', 'expected-app-manifest-sha256',
  'expected-kernel-manifest-sha256', 'expected-activation-manifest-sha256'];
const SYSTEM = ['system-evidence-directory', 'previous-system-nonce', 'prebuild-user-domain'];

const fileSha256 = (path) => digest(readFileSync(path));
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));
const writeJson = (path, value) => writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
const lines = (text) => text.split(/\r?\n/).filter(Boolean);
// Directories, symbolic links, junctions and hard-linked files are all rejected.
const regularFile = (path, message) => {
  const info = lstatSync(path);
  requireGraph(info.isFile() && info.nlink === 1, message);
};
function execute(program, args, { cwd, capture = false, stdout = 'inherit' } = {}) {
  const result = spawnSync(program, args, { cwd, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024,
    stdio: ['inherit', capture ? 'pipe' : stdout, 'inherit'] });
  if (result.error) throw result.error;
  return { status: result.status, stdout: result.stdout ?? '' };
}

function checkManifest(manifest, name, field, source, mode) {
  requireGraph(manifest[field] === source && manifest.validationMode === mode, `Invalid current artifact: ${name}`);
  requireGraph(field !== 'kernelSource' || manifest.repository === REPOSITORY, 'Wrong kernel artifact repository');
}

function checkBinary(artifact, name) {
  // Fully qualified only, as .NET IsPathFullyQualified: drive-rooted or UNC on Windows, never `\foo` or `C:foo`.
  requireGraph(typeof artifact.path === 'string' && typeof artifact.sha256 === 'string' && SHA256.test(artifact.sha256)
    && (process.platform === 'win32' ? /^(?:[A-Za-z]:[\\/]|[\\/]{2})/ : /^\//).test(artifact.path),
  `Invalid current artifact: ${name}`);
  requireGraph(fileSha256(artifact.path) === artifact.sha256, `Current binary hash mismatch: ${name}`);
}

function readArtifact(path, field, source, mode) {
  requireGraph(SOURCE.test(source) && mode === 'formal', 'Exact source identity required');
  const manifest = readJson(path);
  checkManifest(manifest, basename(path), field, source, mode);
  checkBinary(manifest, basename(path));
  return manifest;
}

function checkActivation(manifest, evidenceRoot) {
  const bindingFile = 'directory-activation-tests.exe';
  let artifacts = [manifest], expected = [{ file: bindingFile, tests: [...COMPONENT_TESTS, BINDING_TEST] }];
  if (manifest.artifacts != null) {
    artifacts = manifest.artifacts;
    requireGraph(Array.isArray(artifacts) && artifacts.length === 2
      && ['.', './internal/boxruntime'].includes(artifacts[0]?.package)
      && artifacts[1]?.package === './experimental/directorywindows', 'Invalid activation package roster');
    expected = [{ file: bindingFile, tests: [BINDING_TEST] },
      { file: 'directory-activation-component-tests.exe', tests: COMPONENT_TESTS }];
  }
  expected.forEach(({ file, tests }, index) => {
    const artifact = artifacts[index];
    requireGraph(artifact.path === join(evidenceRoot, file) && Array.isArray(artifact.tests)
      && artifact.tests.length === tests.length && artifact.tests.join('|') === tests.join('|'),
    `Invalid activation path or test roster: ${file}`);
    regularFile(artifact.path, `Invalid activation test binary: ${artifact.path}`);
    checkBinary(artifact, file);
  });
}

function buildAndTest(values) {
  requireGraph(values['kernel-root'] && values['output-directory'], 'Kernel root and output directory required');
  const outputRoot = resolve(values['output-directory']);
  mkdirSync(outputRoot, { recursive: true });
  const receiptPath = join(outputRoot, 'validation-receipt.json');
  const history = join(outputRoot, 'receipt-history');
  // Preserve earlier evidence, but invalidate the current success pointer before
  // resolving sources or running any build. Failed archival stops this attempt.
  const attemptId = randomUUID().replaceAll('-', '');
  const attemptPath = join(outputRoot, 'validation-attempt.json');
  writeJson(attemptPath, { attemptId, result: 'inProgress' });
  if (existsSync(receiptPath)) {
    mkdirSync(history, { recursive: true });
    renameSync(receiptPath, join(history, `${fileSha256(receiptPath)}-${attemptId}.json`));
  }
  try {
    const kernelRoot = resolve(values['kernel-root']);
    const win = readJson(join(root, 'scripts/desktop-core/release-contract.json')).platforms.win;
    requireGraph(values.formal, 'Select the explicit formal contract');
    const source = win.sourceCommit, mode = 'formal';
    requireGraph(SOURCE.test(source), 'Exact lowercase source SHA required');
    const git = (cwd, ...args) => execute('git', ['-C', cwd, ...args], { capture: true });
    const kernelClean = (headMessage, dirtyMessage) => {
      const head = git(kernelRoot, 'rev-parse', 'HEAD');
      requireGraph(head.status === 0 && head.stdout.trim() === source, headMessage);
      const dirty = git(kernelRoot, 'status', '--porcelain', '--untracked-files=all');
      requireGraph(dirty.status === 0 && dirty.stdout.trim() === '', dirtyMessage);
    };
    const origin = git(kernelRoot, 'remote', 'get-url', 'origin');
    requireGraph(origin.status === 0 && /^https:\/\/github[.]com\/polaris-arch\/sing-box-polaris([.]git)?$/.test(origin.stdout.trim()),
      'Wrong kernel fork origin');
    kernelClean('Kernel HEAD differs from selected source', 'Kernel producer source is not clean');
    const appHead = git(root, 'rev-parse', 'HEAD');
    const appSource = appHead.stdout.trim();
    requireGraph(appHead.status === 0 && SOURCE.test(appSource), 'App actual source identity unavailable');
    const assertAppTrackedSource = () => {
      const head = git(root, 'rev-parse', 'HEAD');
      requireGraph(head.status === 0 && head.stdout.trim() === appSource, 'App HEAD changed during validation');
      // Staged and unstaged tracked changes both fail; normal untracked resources,
      // target and producer checkout outputs do not invalidate the source claim.
      requireGraph(git(root, 'diff', '--quiet', 'HEAD', '--').status === 0, 'App tracked source dirty or verification failed');
    };
    assertAppTrackedSource();
    const tags = win.tags;
    requireGraph(typeof tags === 'string' && tags.split(',').includes('with_tailscale'), 'Frozen production tags missing tailscale');
    let ldflags;
    if (win.schema === 'polaris-upstream-native-receipt-v1') {
      // This is a new directory-test build, not the published package producer's
      // command. Read flags from the exact clean selected source, and retain the
      // original release row without fabricating buildCommand/ldflags evidence.
      const flagsPath = join(kernelRoot, 'release/LDFLAGS');
      regularFile(flagsPath, 'Directory-test flags must be a regular source file');
      const sharedFlags = readFileSync(flagsPath, 'utf8').trim();
      requireGraph(sharedFlags && !sharedFlags.includes('SourceRevision=') && !sharedFlags.includes('constant.Version='),
        'Conflicting source-owned directory-test flags');
      ldflags = `${sharedFlags} -X github.com/sagernet/sing-box/constant.Version=${win.version}`;
      writeJson(join(outputRoot, 'test-build-inputs.json'), { scope: 'directory-test-build-only', kernelSource: source,
        releaseSource: win.sourceCommit, tags, sharedFlagsPath: 'release/LDFLAGS', sharedFlagsSHA256: fileSha256(flagsPath),
        version: win.version });
    } else {
      const command = [].concat(win.buildCommand ?? []);
      const index = command.indexOf('-ldflags');
      requireGraph(index >= 0 && index + 1 < command.length, 'Frozen production LDFLAGS missing');
      ldflags = String(command[index + 1]);
    }
    const key = 'github.com/sagernet/sing-box/constant.SourceRevision=';
    if (ldflags.includes(key)) {
      const revisions = [...ldflags.matchAll(/github\.com\/sagernet\/sing-box\/constant\.SourceRevision=(\S+)/g)];
      requireGraph(revisions.length === 1 && revisions[0][1] === win.sourceCommit, 'Conflicting formal SourceRevision flag');
    } else ldflags += ` -X ${key}${source}`;
    const kernelExe = join(outputRoot, 'tailscale-tests.exe');
    // The historical checkout already has the component test file, with these cases still in the root package.
    const testOwners = {
      'directory_windows_test.go': '.',
      'internal/boxruntime/directory_windows_test.go': './internal/boxruntime',
      'experimental/directorywindows/prepared_activation_windows_test.go': './experimental/directorywindows',
    };
    const testPackages = {};
    for (const name of [...COMPONENT_TESTS, BINDING_TEST]) {
      const found = git(kernelRoot, 'grep', '-l', '-e', `^func ${name}(`, source, '--', ...Object.keys(testOwners));
      const declarations = lines(found.stdout);
      requireGraph(found.status === 0 && declarations.length === 1, `Expected one tracked owner for activation test: ${name}`);
      testPackages[name] = testOwners[declarations[0].slice(source.length + 1)];
    }
    const bindingPackage = testPackages[BINDING_TEST], componentPackage = testPackages[COMPONENT_TESTS[0]];
    requireGraph(['.', './internal/boxruntime'].includes(bindingPackage)
      && ['.', './experimental/directorywindows'].includes(componentPackage)
      && testPackages[COMPONENT_TESTS[1]] === componentPackage
      && (componentPackage !== '.' || bindingPackage === '.'), 'Unexpected activation test ownership');
    const activation = [{ package: bindingPackage, path: join(outputRoot, 'directory-activation-tests.exe'), tests: [BINDING_TEST] }];
    if (componentPackage === '.') activation[0].tests = [...COMPONENT_TESTS, BINDING_TEST];
    else activation.push({ package: componentPackage, path: join(outputRoot, 'directory-activation-component-tests.exe'), tests: COMPONENT_TESTS });

    // Read effective flags, including persisted GOENV settings; do not modify them.
    const goFlags = execute('go', ['env', 'GOFLAGS'], { cwd: kernelRoot, capture: true });
    requireGraph(goFlags.status === 0, 'Effective Go flags unavailable');
    requireGraph(!/(?:^|[\s"'])-{1,2}overlay(?:=|\s|["']|$)/i.test(goFlags.stdout.trim()), 'Ambient Go overlay is forbidden');
    const actualGo = execute('go', ['env', 'GOVERSION'], { cwd: kernelRoot, capture: true });
    requireGraph(actualGo.status === 0, 'Go compiler identity unavailable');
    const toolchain = execute(process.execPath, [join(root, 'scripts/windows-directory/resolve-toolchain.mjs'),
      '--kernel-root', kernelRoot, '--mode', mode, '--source', source, '--actual-go', actualGo.stdout.trim()],
    { cwd: kernelRoot, capture: true });
    requireGraph(toolchain.status === 0, 'Exact source/compiler policy verification failed');
    writeFileSync(join(outputRoot, 'toolchain-evidence.json'), toolchain.stdout);
    const goBuild = ['test', '-c', '-buildvcs=true', '-mod=readonly', '-trimpath', '-tags', tags, '-ldflags', ldflags, '-o'];
    requireGraph(execute('go', [...goBuild, kernelExe, './protocol/tailscale'], { cwd: kernelRoot }).status === 0,
      'Native kernel test producer build failed');
    for (const target of activation) {
      requireGraph(execute('go', [...goBuild, target.path, target.package], { cwd: kernelRoot }).status === 0,
        `Native kernel activation test build failed: ${target.package}`);
      regularFile(target.path, `Invalid activation test binary: ${target.path}`);
      target.sha256 = fileSha256(target.path);
    }
    kernelClean('Kernel HEAD changed during test build', 'Kernel source changed during test build');

    const buildLog = join(outputRoot, 'app-test-build.jsonl');
    const descriptor = openSync(buildLog, 'w');
    let built;
    try {
      built = execute('cargo', ['test', '-p', 'polaris', '--lib', '--no-run', '--message-format=json-render-diagnostics'],
        { cwd: root, stdout: descriptor });
    } finally { closeSync(descriptor); }
    requireGraph(built.status === 0, 'Native App test consumer build failed');
    const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version=1'],
      { cwd: root, encoding: 'utf8', timeout: 60_000, maxBuffer: 16 * 1024 * 1024 }));
    const appExe = selectExecutable(lines(readFileSync(buildLog, 'utf8')).map((line) => JSON.parse(line)), metadata, root).executable;
    assertAppTrackedSource();
    const kernelManifest = join(outputRoot, 'kernel-artifact.json');
    const appManifest = join(outputRoot, 'app-artifact.json');
    writeJson(kernelManifest, { path: kernelExe, sha256: fileSha256(kernelExe), kernelSource: source, validationMode: mode, repository: REPOSITORY });
    writeJson(appManifest, { path: appExe, sha256: fileSha256(appExe), appSource, validationMode: mode });
    for (const target of activation) {
      requireGraph(fileSha256(target.path) === target.sha256, `Activation test binary changed before execution: ${target.path}`);
      const listed = execute(target.path, ['-test.list=^TestWindowsDirectory'], { capture: true });
      requireGraph(listed.status === 0, `Activation test discovery failed: ${target.package}`);
      for (const name of target.tests) requireGraph(lines(listed.stdout).includes(name), `Missing exact activation regression: ${name}`);
      requireGraph(execute(target.path, [`-test.run=^(${target.tests.join('|')})$`, '-test.v', '-test.timeout=2m']).status === 0,
        `Production activation regressions failed: ${target.package}`);
      requireGraph(fileSha256(target.path) === target.sha256, `Activation test binary changed during execution: ${target.path}`);
    }
    const activationManifest = { kernelSource: source, validationMode: mode, repository: REPOSITORY };
    if (activation.length === 1) Object.assign(activationManifest, { path: activation[0].path, sha256: activation[0].sha256, tests: activation[0].tests });
    else activationManifest.artifacts = activation;
    writeJson(join(outputRoot, 'activation-artifact.json'), activationManifest);
    const kernelListing = execute(kernelExe, ['-test.list=^TestWindows'], { capture: true });
    requireGraph(kernelListing.status === 0, 'Kernel test discovery failed');
    for (const name of KERNEL_TESTS) requireGraph(lines(kernelListing.stdout).includes(name), `Missing exact kernel regression: ${name}`);
    requireGraph(execute(kernelExe, ['-test.run=^TestWindows', '-test.v', '-test.timeout=10m']).status === 0,
      'Native kernel Windows regressions failed');
    requireGraph(process.env.OS === 'Windows_NT', 'Native Windows runner required');
    const app = readArtifact(appManifest, 'appSource', appSource, mode).path;
    readArtifact(kernelManifest, 'kernelSource', source, mode);
    process.env.POLARIS_WINDOWS_KERNEL_TEST_ARTIFACT = kernelManifest;
    process.env.POLARIS_WINDOWS_TEST_VALIDATION_MODE = mode;
    process.env.POLARIS_WINDOWS_TEST_EXPECTED_SOURCE = source;
    const appListing = execute(app, ['--list'], { capture: true });
    requireGraph(appListing.status === 0, 'App test discovery failed');
    for (const name of APP_TESTS) {
      requireGraph(lines(appListing.stdout).includes(`${name}: test`), `Missing exact App integration test: ${name}`);
      requireGraph(execute(app, ['--exact', name, '--ignored', '--nocapture', '--test-threads=1']).status === 0,
        `App integration failed: ${name}`);
    }
    console.log(`Directory integration executed: mode=${mode} source=${source}`);
    assertAppTrackedSource();
    writeJson(receiptPath, { attemptId, validationMode: mode, kernelSource: source, appSource, repository: REPOSITORY,
      result: 'passed', scope: 'directory-protocol-tests-only', formalSourceContractMatched: mode === 'formal' });
    writeJson(attemptPath, { attemptId, result: 'passed', receipt: 'validation-receipt.json', receiptSHA256: fileSha256(receiptPath) });
  } catch (error) {
    // If final receipt publication succeeded but attempt publication failed,
    // revoke that success pointer as well; no failed attempt may advertise pass.
    if (existsSync(receiptPath)) {
      mkdirSync(history, { recursive: true });
      renameSync(receiptPath, join(history, `${fileSha256(receiptPath)}-${attemptId}-incomplete.json`));
    }
    writeJson(attemptPath, { attemptId, result: 'failed', error: error.message });
    throw error;
  }
}

function verify(values) {
  const bound = values['expected-attempt-id'] !== undefined;
  requireGraph(bound ? values['previous-attempt-id'] === undefined && BOUND.every((name) => values[name] !== undefined)
    : values['previous-attempt-id'] !== undefined && [...BOUND, ...SYSTEM].every((name) => values[name] === undefined),
  'Exactly one of the NewAttempt or BoundAttempt parameter sets required');
  const { 'evidence-directory': directory, 'expected-app-source': appSource, 'expected-kernel-source': kernelSource,
    'validation-mode': mode } = values;
  requireGraph(directory && mode === 'formal', 'Evidence directory and formal validation mode required');
  for (const source of [appSource, kernelSource]) requireGraph(SOURCE.test(source), 'Exact source identity required');
  const attemptPath = join(directory, 'validation-attempt.json');
  const receiptPath = join(directory, 'validation-receipt.json');
  const attempt = readJson(attemptPath);
  requireGraph(attempt.result === 'passed' && ATTEMPT.test(attempt.attemptId)
    && attempt.receipt === 'validation-receipt.json' && SHA256.test(attempt.receiptSHA256),
  'Current attempt is missing, incomplete, failed, or malformed');
  if (!bound) {
    const previous = values['previous-attempt-id'];
    requireGraph(!previous || ATTEMPT.test(previous), 'Malformed previous attempt identity');
    requireGraph(attempt.attemptId !== previous, 'Build did not produce a new current attempt');
  } else {
    const expectedAttempt = values['expected-attempt-id'], expectedReceipt = values['expected-receipt-sha256'];
    requireGraph(ATTEMPT.test(expectedAttempt) && attempt.attemptId === expectedAttempt, 'Current attempt differs from this job build output');
    requireGraph(SHA256.test(expectedReceipt) && attempt.receiptSHA256 === expectedReceipt, 'Current receipt differs from this job build output');
  }
  const receiptSHA256 = fileSha256(receiptPath);
  requireGraph(receiptSHA256 === attempt.receiptSHA256, 'Current receipt hash mismatch');
  const receipt = readJson(receiptPath);
  requireGraph(receipt.result === 'passed' && receipt.attemptId === attempt.attemptId
    && receipt.validationMode === mode && receipt.appSource === appSource
    && receipt.kernelSource === kernelSource && receipt.repository === REPOSITORY
    && receipt.scope === 'directory-protocol-tests-only' && receipt.testFixture == null,
  'Current receipt source, mode, attempt or scope mismatch');
  const evidenceRoot = resolve(directory);
  const manifestDigest = (name, field, source, expectedDigest) => {
    const bytes = readFileSync(join(directory, name));
    const hash = digest(bytes);
    if (bound) requireGraph(SHA256.test(expectedDigest) && hash === expectedDigest, `Manifest changed since this job build: ${name}`);
    const manifest = JSON.parse(bytes.toString('utf8'));
    checkManifest(manifest, name, field, source, mode);
    if (name === 'activation-artifact.json') checkActivation(manifest, evidenceRoot);
    else checkBinary(manifest, name);
    return hash;
  };
  const appManifestSHA256 = manifestDigest('app-artifact.json', 'appSource', appSource, values['expected-app-manifest-sha256']);
  const kernelManifestSHA256 = manifestDigest('kernel-artifact.json', 'kernelSource', kernelSource, values['expected-kernel-manifest-sha256']);
  const activationManifestSHA256 = manifestDigest('activation-artifact.json', 'kernelSource', kernelSource,
    values['expected-activation-manifest-sha256']);
  // Re-read the marker after all checks; a replaced pointer cannot be returned as verified.
  const after = readJson(attemptPath);
  requireGraph(after.result === 'passed' && after.attemptId === attempt.attemptId && after.receiptSHA256 === receiptSHA256,
    'Current attempt changed during verification');
  let systemNonce = null, systemReceiptSHA256 = null;
  if (values['system-evidence-directory']) {
    const systemPath = join(values['system-evidence-directory'], 'system-bootstrap-receipt.json');
    const system = readJson(systemPath);
    const previousNonce = values['previous-system-nonce'] ?? '';
    requireGraph(system.result === 'passed' && system.validationMode === mode
      && system.appSource === appSource && system.kernelSource === kernelSource
      && system.scope === 'system-producer-user-consumer-bootstrap-only'
      && ATTEMPT.test(system.nonce) && system.nonce !== previousNonce
      && typeof system.prebuiltUserDomain === 'boolean' && system.prebuiltUserDomain === Boolean(values['prebuild-user-domain'])
      && system.taskExitCode === 0, 'Missing or stale SYSTEM receipt for this invocation');
    requireGraph(!previousNonce || ATTEMPT.test(previousNonce), 'Malformed previous SYSTEM nonce');
    const appArtifact = readJson(join(directory, 'app-artifact.json'));
    const kernelArtifact = readJson(join(directory, 'kernel-artifact.json'));
    requireGraph(system.appSHA256 === appArtifact.sha256 && system.kernelSHA256 === kernelArtifact.sha256,
      'SYSTEM receipt uses different binary identities');
    systemNonce = system.nonce;
    systemReceiptSHA256 = fileSha256(systemPath);
  }
  console.log(JSON.stringify({ systemNonce, systemReceiptSHA256, attemptId: attempt.attemptId, receiptSHA256,
    appManifestSHA256, kernelManifestSHA256, activationManifestSHA256 }));
}

const COMMANDS = {
  'build-and-test': [buildAndTest, {
    'kernel-root': { type: 'string' }, 'output-directory': { type: 'string' }, formal: { type: 'boolean' },
  }],
  verify: [verify, {
    'evidence-directory': { type: 'string' }, 'expected-app-source': { type: 'string' },
    'expected-kernel-source': { type: 'string' }, 'validation-mode': { type: 'string' },
    'previous-attempt-id': { type: 'string' },
    ...Object.fromEntries([...BOUND, ...SYSTEM.slice(0, 2)].map((name) => [name, { type: 'string' }])),
    'prebuild-user-domain': { type: 'boolean' },
  }],
  'read-artifact': [({ manifest, 'source-field': field, 'expected-source': source, 'validation-mode': mode }) => {
    requireGraph(manifest && field, 'Artifact manifest and source field required');
    console.log(JSON.stringify(readArtifact(manifest, field, source, mode)));
  }, {
    manifest: { type: 'string' }, 'source-field': { type: 'string' },
    'expected-source': { type: 'string' }, 'validation-mode': { type: 'string' },
  }],
};

try {
  const [command, ...args] = process.argv.slice(2);
  requireGraph(Object.hasOwn(COMMANDS, command ?? ''), 'Usage: acceptance.mjs build-and-test|verify|read-artifact [options]');
  const [run, options] = COMMANDS[command];
  run(parseArgs({ args, options }).values);
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
