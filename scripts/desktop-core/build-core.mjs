// Four desktop consumers of the shared provisioner. No target kernel is run.
import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, renameSync,
  rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { buildInfoFingerprint, canonical, DESKTOP_TARGETS, digest, expectedTags, platformSourceIdentity, requireGraph, validateBuildInfo,
  validateSourceManifest, validateSourcePins, validateSourceReceipt, verifyHash } from './source-graph.mjs';

const SOURCE_MANIFEST = 'scripts/libbox-patches/source-manifest.json';
const PROVISIONER = 'scripts/core-source-provision.py';
const sourceText = (value) => String(value).trim();

export function buildDesktopCore(root, manifest, key, dest, _force = false,
  run = (command, args, options = {}) => execFileSync(command, args, { stdio: 'inherit', ...options }),
  production = {}) {
  // Incomplete pins fail before mkdir, network, cache inspection or execution.
  const spec = validateSourcePins(manifest, key, !production.producer);
  requireGraph(/^[a-f0-9]{40}$/.test(production.candidate ?? ''), 'Explicit candidate source SHA required');
  const sourceManifest = join(root, SOURCE_MANIFEST);
  const provisioner = join(root, PROVISIONER);
  verifyHash(sourceManifest, spec.sourceManifestSha256);
  verifyHash(provisioner, spec.provisionerSha256);
  const source = JSON.parse(readFileSync(sourceManifest, 'utf8'));
  const dependencies = validateSourceManifest(source, spec);
  for (const patch of source.patches) verifyHash(join(dirname(sourceManifest), patch.file), patch.sha256);
  for (const dep of dependencies) verifyHash(join(dirname(sourceManifest), dep.patchFile), dep.patchSha256);
  const overlay = key === 'win' ? join(root, 'scripts/core-patches/windows-dns-refresh.patch') : undefined;
  if (overlay) {
    requireGraph(manifest.windowsBuild?.sourceCommit === source.sourceCommit
      && manifest.windowsBuild.goVersion === source.goVersion && manifest.windowsBuild.version === spec.version
      && (production.producer || manifest.windowsBuild.binarySha256 === spec.platforms.win.binarySha256), 'Windows overlay/output pins differ from combined source graph');
    verifyHash(overlay, manifest.windowsBuild.patchSha256);
  }
  const target = DESKTOP_TARGETS[key];
  const work = mkdtempSync(join(tmpdir(), 'polaris-desktop-core-'));
  let staging;
  try {
    // Always replay fresh sources. Darwin CGO requires a real SDK/compiler.
    const environment = { ...process.env, GOWORK: 'off', GOFLAGS: '', GOEXPERIMENT: '',
      GOOS: target.goos, GOARCH: target.goarch, CGO_ENABLED: target.cgo };
    for (const name of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GOAMD64', 'GOARM', 'GOARM64']) delete environment[name];
    if (target.goarch === 'amd64') environment.GOAMD64 = 'v1';
    const options = { env: { ...environment, GOTOOLCHAIN: `go${source.goVersion}` } };
    const capture = (command, args, extra = {}) => sourceText(run(command, args, { ...options, ...extra, encoding: 'utf8', stdio: 'pipe' }));
    requireGraph(capture('git', ['rev-parse', 'HEAD'], { cwd: root }) === production.candidate
      && capture('git', ['status', '--porcelain', '--untracked-files=no'], { cwd: root }) === '',
    'Producer candidate checkout differs or has tracked modifications');
    const goRoot = capture('go', ['env', 'GOROOT']);
    requireGraph(goRoot.length > 0 && !goRoot.includes('\n'), 'Pinned Go toolchain root missing');
    const go = join(goRoot, 'bin', process.platform === 'win32' ? 'go.exe' : 'go');
    options.env.GOTOOLCHAIN = 'local';
    requireGraph(capture(go, ['version']).startsWith(`go version go${source.goVersion} `), 'Pinned Go toolchain differs');
    const fetch = (url, commit, name) => {
      const repository = join(work, name);
      mkdirSync(repository);
      run('git', ['init', '--quiet', repository], options);
      run('git', ['-C', repository, 'fetch', '--depth=1', url, commit], options);
      requireGraph(capture('git', ['-C', repository, 'rev-parse', `${commit}^{commit}`]) === commit, 'Fetched upstream source commit differs');
      return repository;
    };
    const repository = fetch('https://github.com/SagerNet/sing-box.git', source.sourceCommit, 'upstream');
    const args = [provisioner, '--manifest', sourceManifest, '--source', repository,
      '--checkout', join(work, 'checkout'), '--go', go];
    for (const dep of dependencies) {
      const moduleRepo = fetch(dep.sourceURL, dep.upstreamCommit, `upstream-${dep.name}`);
      args.push('--module-source', `${dep.module}=${moduleRepo}`);
    }
    const receipt = JSON.parse(capture(process.platform === 'win32' ? 'python' : 'python3', args));
    validateSourceReceipt(receipt, source, spec);
    const checkout = join(work, 'checkout');
    requireGraph(canonical(JSON.parse(readFileSync(join(checkout, '.polaris-source-receipt.json'), 'utf8')))
      === canonical(receipt), 'Provider stdout and persisted receipt differ');
    const buildOptions = { ...options, cwd: checkout };
    if (overlay) {
      run('git', ['apply', '--check', overlay], buildOptions);
      run('git', ['apply', overlay], buildOptions);
    }
    // Exclude untracked receipt metadata, retain relative replace source trees.
    run('git', ['add', '-A', '--', '.', ':(exclude).polaris-source-receipt.json'], buildOptions);
    const buildTree = capture('git', ['write-tree'], { cwd: checkout });
    requireGraph(buildTree === spec.platforms[key].buildTree, 'Platform source/overlay tree differs');
    const verifyCompilerInputs = () => {
      verifyHash(join(checkout, 'go.mod'), receipt.mainGoModSha256);
      verifyHash(join(checkout, 'go.sum'), receipt.mainGoSumSha256);
      run('git', ['diff', '--exit-code'], buildOptions);
      const extra = ['--exclude-standard', '--ignored --exclude-standard'].flatMap((flags) =>
        capture('git', ['ls-files', '--others', ...flags.split(' ')], { cwd: checkout }).split('\n'));
      requireGraph(!extra.some((path) => /\.(?:go|c|cc|cpp|h|s|S|syso)$/.test(path)
        || /(?:^|\/)(?:go\.mod|go\.sum|go\.work|go\.work\.sum)$/.test(path) || path.startsWith('vendor/')),
      'Untracked compiler input differs from reviewed source tree');
    };
    verifyCompilerInputs();
    const preset = readFileSync(join(checkout, 'release', key === 'win' ? 'DEFAULT_BUILD_TAGS_WINDOWS' : 'DEFAULT_BUILD_TAGS'), 'utf8').trim();
    const tags = [...new Set([...preset.split(','), ...(target.cgo === '0' ? ['with_purego'] : [])])].sort();
    requireGraph(canonical(tags) === canonical(expectedTags(key)), 'Upstream feature tag preset differs; review required, do not drop features');
    const flags = readFileSync(join(checkout, 'release/LDFLAGS'), 'utf8').trim();
    if (overlay) {
      run(go, process.platform === 'win32'
        ? ['test', '-mod=readonly', '-count=20', './dns/transport/local/systemconfig']
        : ['test', '-mod=readonly', '-c', '-o', join(work, 'systemconfig.test.exe'), './dns/transport/local/systemconfig'], buildOptions);
    }
    const binary = join(work, key === 'win' ? 'sing-box.exe' : 'sing-box');
    const identity = platformSourceIdentity(receipt, source, spec, key, manifest.windowsBuild?.patchSha256);
    run(go, ['build', '-mod=readonly', '-trimpath', '-buildvcs=false', '-tags', tags.join(','), '-ldflags',
      `${flags} -X github.com/sagernet/sing-box/constant.Version=${spec.version} -s -w -buildid=${identity.buildID}`,
      '-o', binary, './cmd/sing-box'], buildOptions);
    // Read actual embedded metadata without executing the target binary.
    const buildInfo = capture(go, ['version', '-m', binary], { cwd: checkout });
    const linked = validateBuildInfo(buildInfo, source, key, spec);
    requireGraph(capture(go, ['tool', 'buildid', binary], { cwd: checkout }) === identity.buildID,
      'Actual binary source buildID differs');
    const binarySha256 = digest(readFileSync(binary));
    if (!production.producer) verifyHash(binary, spec.platforms[key].binarySha256);
    run('git', ['add', '-A', '--', '.', ':(exclude).polaris-source-receipt.json'], buildOptions);
    requireGraph(capture('git', ['write-tree'], { cwd: checkout }) === buildTree, 'Build changed reviewed source graph');
    verifyCompilerInputs();
    const platformReceipt = { schema: 'polaris-desktop-core-v1', candidate: production.candidate, platform: key,
      ...identity, linkedModules: Object.fromEntries(linked.modules),
      sourceReceipt: receipt, buildTree, overlays: overlay ? [{ file: 'windows-dns-refresh.patch', sha256: manifest.windowsBuild.patchSha256 }] : [],
      mainGoModSha256: receipt.mainGoModSha256,
      mainGoSumSha256: receipt.mainGoSumSha256,
      version: spec.version, binarySha256,
      buildInfoSha256: buildInfoFingerprint(buildInfo), goos: target.goos, goarch: target.goarch,
      cgo: target.cgo, tags };
    platformReceipt.fingerprint = digest(canonical(platformReceipt));
    mkdirSync(dirname(dest), { recursive: true });
    staging = mkdtempSync(join(dirname(dest), '.polaris-core-'));
    const stagedBinary = join(staging, 'binary');
    const stagedReceipt = join(staging, 'receipt.json');
    copyFileSync(binary, stagedBinary);
    if (key !== 'win') chmodSync(stagedBinary, 0o755);
    writeFileSync(stagedReceipt, `${JSON.stringify(platformReceipt, null, 2)}\n`);
    renameSync(stagedReceipt, `${dest}.source-receipt.json`);
    renameSync(stagedBinary, dest);
    return platformReceipt;
  } finally {
    if (staging) rmSync(staging, { recursive: true, force: true });
    rmSync(work, { recursive: true, force: true });
  }
}

// First legal native build needs frozen inputs, not a not-yet-known binary SHA.
// Outputs are later bound by the four-producer bundle inventory at consumption.
export function produceDesktopCore(root, manifest, key, dest, candidate, run) {
  return buildDesktopCore(root, manifest, key, dest, true, run, { producer: true, candidate });
}
